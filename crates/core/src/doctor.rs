//! Per-agent dependency preflight for `/doctor`.
//!
//! The engine-level `/doctor` reports the runtime environment (version,
//! provider, API key, sandbox…). This module adds the *agent's own*
//! declared requirements. A published agent's `manifest.json` (at the
//! workspace root, written by `/cloud get`) carries a `requires` block —
//! provider keys, Python packages, system binaries. `/doctor` checks each
//! is actually present so a user who just pulled an agent learns what to
//! install BEFORE the first tool call dies with `command not found`.
//! `/doctor --fix` then installs the missing pieces via each requirement's
//! declared recipe (pip / npm / brew / apt) — the consented second step.
//!
//! thClaws' goal is to make *user-generated* agents like this convenient:
//! an author declares deps in the manifest, the engine doctors them. No
//! per-agent doctor script needed.

use std::path::PathBuf;
use std::process::Command;

// ── manifest `requires` schema (all fields optional) ──────────────────

#[derive(serde::Deserialize, Default)]
struct ManifestFile {
    #[serde(default)]
    requires: Requires,
}

#[derive(serde::Deserialize, Default)]
struct Requires {
    #[serde(default)]
    provider_keys: Vec<ProviderKey>,
    #[serde(default)]
    python: Vec<PyReq>,
    #[serde(default)]
    system: Vec<SysReq>,
}

#[derive(serde::Deserialize)]
struct ProviderKey {
    name: String,
    #[serde(default)]
    required: bool,
    #[serde(default)]
    purpose: Option<String>,
}

#[derive(serde::Deserialize)]
struct PyReq {
    /// pip package name, e.g. `Pillow`.
    pip: String,
    /// Import name when it differs from the pip name (`Pillow` → `PIL`).
    #[serde(rename = "import", default)]
    import_name: Option<String>,
    #[serde(default)]
    purpose: Option<String>,
}

#[derive(serde::Deserialize)]
struct SysReq {
    /// Binary name to look for on `$PATH`, e.g. `ffmpeg`, `mmdc`.
    name: String,
    #[serde(default)]
    install: Install,
    #[serde(default)]
    purpose: Option<String>,
}

#[derive(serde::Deserialize, Default, Clone)]
struct Install {
    // brew is read on macOS, apt on Linux — each is dead code on the
    // other platform, which is expected.
    #[serde(default)]
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    brew: Option<String>,
    #[serde(default)]
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    apt: Option<String>,
    #[serde(default)]
    npm: Option<String>,
    #[serde(default)]
    pip: Option<String>,
}

// ── report model ──────────────────────────────────────────────────────

#[derive(PartialEq, Clone, Copy)]
pub enum Status {
    Ok,
    Missing,
    Optional,
}

pub struct Check {
    pub label: String,
    pub status: Status,
    pub note: String,
    /// Ordered candidate install commands (argv). Empty ⇒ no auto-fix
    /// (provider keys can't be installed).
    pub fix: Vec<Vec<String>>,
}

pub struct Report {
    /// A manifest with a non-empty `requires` block was found.
    pub found: bool,
    pub checks: Vec<Check>,
}

impl Report {
    pub fn has_installable_gaps(&self) -> bool {
        self.checks
            .iter()
            .any(|c| c.status == Status::Missing && !c.fix.is_empty())
    }
}

fn manifest_path() -> PathBuf {
    std::env::current_dir()
        .unwrap_or_default()
        .join("manifest.json")
}

/// Read `<cwd>/manifest.json` and check every declared requirement.
/// Never fails: a missing / unparsable manifest just yields `found:false`.
pub fn diagnose() -> Report {
    let Ok(body) = std::fs::read_to_string(manifest_path()) else {
        return Report {
            found: false,
            checks: vec![],
        };
    };
    let Ok(m) = serde_json::from_str::<ManifestFile>(&body) else {
        return Report {
            found: false,
            checks: vec![],
        };
    };
    let r = m.requires;
    let mut checks = Vec::new();

    for k in &r.provider_keys {
        let present = key_present(&k.name);
        checks.push(Check {
            label: format!("key {}", k.name),
            status: if present {
                Status::Ok
            } else if k.required {
                Status::Missing
            } else {
                Status::Optional
            },
            note: k.purpose.clone().unwrap_or_default(),
            fix: vec![],
        });
    }

    let py = python_interp();
    for p in &r.python {
        let module = p
            .import_name
            .clone()
            .unwrap_or_else(|| default_import(&p.pip));
        let present = py
            .as_deref()
            .map(|py| py_has_module(py, &module))
            .unwrap_or(false);
        checks.push(Check {
            label: format!("python {} ({})", p.pip, module),
            status: if present { Status::Ok } else { Status::Missing },
            note: p.purpose.clone().unwrap_or_default(),
            fix: pip_candidates(py.as_deref(), &p.pip),
        });
    }

    for s in &r.system {
        let present = on_path(&s.name);
        checks.push(Check {
            label: format!("bin {}", s.name),
            status: if present { Status::Ok } else { Status::Missing },
            note: s.purpose.clone().unwrap_or_default(),
            fix: system_candidates(&s.install),
        });
    }

    Report {
        found: !checks.is_empty(),
        checks,
    }
}

/// Plain-text section appended to the `/doctor` diagnostics. Empty string
/// when the workspace has no agent manifest (so the base report is
/// unchanged for non-agent projects).
pub fn render(report: &Report) -> String {
    if !report.found {
        return String::new();
    }
    let mut s = String::from("\n── agent dependencies (manifest requires) ──\n");
    for c in &report.checks {
        let mark = match c.status {
            Status::Ok => "✓",
            Status::Missing => "✗",
            Status::Optional => "○",
        };
        s.push_str(mark);
        s.push(' ');
        s.push_str(&c.label);
        if !c.note.is_empty() {
            let note: String = c.note.chars().take(72).collect();
            let ellipsis = if c.note.chars().count() > 72 {
                "…"
            } else {
                ""
            };
            s.push_str(&format!(" — {note}{ellipsis}"));
        }
        s.push('\n');
    }
    if report.has_installable_gaps() {
        s.push_str("→ run /doctor --fix to install the missing pieces\n");
    }
    s
}

/// Install every missing requirement that has a fix recipe, trying each
/// candidate command until one succeeds. Returns a text log. Blocking —
/// call from an explicit user action, not a hot path.
pub fn apply(report: &Report) -> String {
    let mut s = String::new();
    for c in &report.checks {
        if c.status != Status::Missing || c.fix.is_empty() {
            continue;
        }
        s.push_str(&format!("\n🔧 {}\n", c.label));
        let mut ok = false;
        for cmd in &c.fix {
            s.push_str(&format!("   $ {}\n", cmd.join(" ")));
            match Command::new(&cmd[0]).args(&cmd[1..]).output() {
                Ok(out) if out.status.success() => {
                    s.push_str("   ✓ installed\n");
                    ok = true;
                    break;
                }
                Ok(out) => {
                    let err = String::from_utf8_lossy(&out.stderr);
                    let last = err.lines().rev().find(|l| !l.trim().is_empty());
                    if let Some(last) = last {
                        s.push_str(&format!("   ✗ {}\n", last.trim()));
                    } else {
                        s.push_str("   ✗ failed\n");
                    }
                }
                Err(e) => s.push_str(&format!("   ✗ {e}\n")),
            }
        }
        if !ok {
            s.push_str("   ⚠ could not install automatically — install it manually\n");
        }
    }
    if s.is_empty() {
        "nothing to install — all agent dependencies satisfied".into()
    } else {
        s
    }
}

// ── checks ────────────────────────────────────────────────────────────

fn key_present(env_name: &str) -> bool {
    if let Ok(v) = std::env::var(env_name) {
        if !v.trim().is_empty() {
            return true;
        }
    }
    if let Some(short) = provider_short(env_name) {
        if crate::secrets::get(short)
            .map(|s| !s.trim().is_empty())
            .unwrap_or(false)
        {
            return true;
        }
    }
    false
}

fn provider_short(env_name: &str) -> Option<&'static str> {
    match env_name {
        "ANTHROPIC_API_KEY" => Some("anthropic"),
        "OPENAI_API_KEY" => Some("openai"),
        "GEMINI_API_KEY" | "GOOGLE_API_KEY" => Some("gemini"),
        "OPENROUTER_API_KEY" => Some("openrouter"),
        "DEEPSEEK_API_KEY" => Some("deepseek"),
        "DASHSCOPE_API_KEY" => Some("dashscope"),
        _ => None,
    }
}

fn python_interp() -> Option<String> {
    ["python3", "python"]
        .into_iter()
        .find(|c| on_path(c))
        .map(String::from)
}

fn py_has_module(py: &str, module: &str) -> bool {
    Command::new(py)
        .arg("-c")
        .arg(format!("import {module}"))
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Best-effort import name from a pip name (`python-pptx` → `python_pptx`).
/// Authors override via the `import` field when it differs (`Pillow`→`PIL`).
fn default_import(pip: &str) -> String {
    pip.to_lowercase().replace('-', "_")
}

fn on_path(name: &str) -> bool {
    let Ok(paths) = std::env::var("PATH") else {
        return false;
    };
    for dir in std::env::split_paths(&paths) {
        if dir.join(name).is_file() {
            return true;
        }
        #[cfg(windows)]
        for ext in ["exe", "cmd", "bat"] {
            if dir.join(format!("{name}.{ext}")).is_file() {
                return true;
            }
        }
    }
    false
}

fn pip_candidates(py: Option<&str>, pkg: &str) -> Vec<Vec<String>> {
    let py = py.unwrap_or("python3").to_string();
    let base = |extra: &[&str]| {
        let mut v = vec![py.clone(), "-m".into(), "pip".into(), "install".into()];
        v.extend(extra.iter().map(|s| s.to_string()));
        v.push(pkg.to_string());
        v
    };
    vec![
        base(&[]),
        base(&["--user"]),
        base(&["--break-system-packages"]),
    ]
}

fn system_candidates(install: &Install) -> Vec<Vec<String>> {
    let mut out = Vec::new();
    #[cfg(target_os = "macos")]
    if let Some(f) = &install.brew {
        out.push(vec!["brew".into(), "install".into(), f.clone()]);
    }
    #[cfg(target_os = "linux")]
    if let Some(f) = &install.apt {
        out.push(vec![
            "sudo".into(),
            "apt-get".into(),
            "install".into(),
            "-y".into(),
            f.clone(),
        ]);
    }
    if let Some(f) = &install.npm {
        out.push(vec!["npm".into(), "install".into(), "-g".into(), f.clone()]);
    }
    if let Some(f) = &install.pip {
        out.push(vec![
            "python3".into(),
            "-m".into(),
            "pip".into(),
            "install".into(),
            f.clone(),
        ]);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_import_normalizes() {
        assert_eq!(default_import("python-pptx"), "python_pptx");
        assert_eq!(default_import("Pillow"), "pillow");
    }

    #[test]
    fn unparsable_manifest_is_not_found() {
        // diagnose() reads cwd; here we just exercise the empty path.
        let r = Report {
            found: false,
            checks: vec![],
        };
        assert!(!r.has_installable_gaps());
        assert!(render(&r).is_empty());
    }

    #[test]
    fn pip_candidates_have_three_fallbacks() {
        let c = pip_candidates(Some("python3"), "Pillow");
        assert_eq!(c.len(), 3);
        assert!(c[0].ends_with(&["Pillow".to_string()]));
        assert!(c[1].contains(&"--user".to_string()));
        assert!(c[2].contains(&"--break-system-packages".to_string()));
    }

    #[test]
    fn render_lists_and_flags_gaps() {
        let r = Report {
            found: true,
            checks: vec![
                Check {
                    label: "bin ffmpeg".into(),
                    status: Status::Missing,
                    note: "video".into(),
                    fix: vec![vec!["brew".into(), "install".into(), "ffmpeg".into()]],
                },
                Check {
                    label: "key GEMINI_API_KEY".into(),
                    status: Status::Ok,
                    note: String::new(),
                    fix: vec![],
                },
            ],
        };
        let out = render(&r);
        assert!(out.contains("✗ bin ffmpeg"));
        assert!(out.contains("✓ key GEMINI_API_KEY"));
        assert!(out.contains("/doctor --fix"));
    }
}
