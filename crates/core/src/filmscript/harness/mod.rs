//! FilmScript harness (dev-plan/52, Tier 3) — the impure half. The
//! compiler stays pure; everything with a side effect lives here:
//! file→URL uploads, TTS synthesis, ffmpeg/ffprobe work, the Kie.ai
//! client, and the per-film job worker that walks the shot DAG.
//!
//! Money-safety rules this module enforces (all T0-verified):
//! - never fire a paid createTask for a script with compile errors
//!   (`compile_phase2` refuses; the job starter re-checks),
//! - the confirmed budget is a hard gate, checked against the
//!   T0-calibrated estimate before the first task,
//! - every completed shot lands in the shot-result cache keyed by its
//!   payload hash, so resume/re-render never double-spends,
//! - Kie is Cloudflare-fronted: every request carries a browser-normal
//!   User-Agent (default UAs get 403 / error 1010).

pub mod assemble;
pub mod dispatch;
pub mod job;
pub mod kie;
pub mod tts;
pub mod upload;

use crate::error::{Error, Result};
use std::path::{Path, PathBuf};

pub(crate) const USER_AGENT: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) thclaws-film";

pub(crate) fn film_root() -> PathBuf {
    Path::new(".thclaws").join("film")
}

pub(crate) fn cache_dir() -> PathBuf {
    film_root().join("cache")
}

pub(crate) fn job_dir(job_id: &str) -> PathBuf {
    film_root().join(job_id)
}

/// Crash-safe JSON write: temp file + rename, so a poll mid-write never
/// reads a torn `job.json`.
pub(crate) fn atomic_write_json<T: serde::Serialize>(path: &Path, value: &T) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, serde_json::to_vec_pretty(value)?)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

/// ffmpeg + ffprobe preflight — runs at compile/submit time, never 20
/// paid minutes into a job. The hosted engine image ships both; desktop
/// installs are on the user.
pub fn check_av_tools() -> Result<()> {
    for tool in ["ffmpeg", "ffprobe"] {
        let found = std::process::Command::new(tool)
            .arg("-version")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        if !found {
            return Err(Error::Tool(format!(
                "{tool} not found — install it first (macOS: `brew install ffmpeg`, \
                 Windows: `winget install ffmpeg`, Debian/Ubuntu: `apt install ffmpeg`)"
            )));
        }
    }
    Ok(())
}

/// Clip/audio duration via ffprobe — the single source of truth for
/// media timing (provider-reported numbers are advisory only).
pub(crate) fn ffprobe_duration_ms(path: &Path) -> Result<u64> {
    let out = std::process::Command::new("ffprobe")
        .args([
            "-v",
            "quiet",
            "-show_entries",
            "format=duration",
            "-of",
            "csv=p=0",
        ])
        .arg(path)
        .output()?;
    let s = String::from_utf8_lossy(&out.stdout);
    let secs: f64 = s
        .trim()
        .parse()
        .map_err(|_| Error::Tool(format!("ffprobe gave no duration for {}", path.display())))?;
    Ok((secs * 1000.0) as u64)
}

pub(crate) fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(bytes);
    let d = h.finalize();
    d.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn atomic_write_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("x.json");
        atomic_write_json(&p, &serde_json::json!({"a": 1})).unwrap();
        let v: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&p).unwrap()).unwrap();
        assert_eq!(v["a"], 1);
        assert!(!p.with_extension("json.tmp").exists());
    }
}
