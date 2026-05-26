//! Shared helpers for tool-call progress labels, duration formatting,
//! heartbeat text, and result summaries. Used by the REPL, team loops,
//! shared-session worker, and event renderer to avoid divergent label logic.

use std::time::Duration;

// ── constants ──────────────────────────────────────────────────────

/// Maximum visible characters for a tool preview inside brackets.
const PREVIEW_CAP: usize = 60;

/// First heartbeat fires after this many seconds.
pub(crate) const HEARTBEAT_FIRST_AFTER: Duration = Duration::from_secs(10);

/// Subsequent heartbeats fire every this many seconds.
pub(crate) const HEARTBEAT_EVERY: Duration = Duration::from_secs(30);

// ── secret redaction ───────────────────────────────────────────────

/// Secret patterns to redact from tool previews. Each tuple is
/// `(prefix_to_match, replacement_text)`. The replacement must NOT
/// contain the prefix itself, otherwise the `find()` loop re-matches
/// forever.
const SECRET_PATTERNS: &[(&str, &str)] = &[
    ("token=", "<redacted>"),
    ("api_key=", "<redacted>"),
    ("apikey=", "<redacted>"),
    ("password=", "<redacted>"),
    ("secret=", "<redacted>"),
    ("Bearer ", "<redacted>"),
    ("Authorization: ", "<redacted>"),
];

/// Redact known secret patterns from `s`. Case-sensitive to avoid
/// false positives on words like "secretary". Advances past each
/// replacement to prevent infinite loops.
fn redact_secrets(s: &str) -> String {
    let mut out = s.to_string();
    for (pat, repl) in SECRET_PATTERNS {
        let mut search_from = 0;
        while let Some(pos) = out[search_from..].find(pat) {
            let abs_pos = search_from + pos;
            let val_start = abs_pos + pat.len();
            let val_end = out[val_start..]
                .find(|c: char| c == ' ' || c == '&' || c == ';' || c == '\n')
                .map_or(out.len(), |i| val_start + i);
            out.replace_range(abs_pos..val_end, repl);
            // Advance past the replacement so we don't re-match it.
            search_from = abs_pos + repl.len();
        }
    }
    out
}

// ── sanitization ───────────────────────────────────────────────────

/// Strip control characters and collapse whitespace into single spaces.
pub(crate) fn sanitize_label_field(s: &str) -> String {
    let cleaned: String = s
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    cleaned.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Truncate to `cap` visible characters, appending "…" if truncated.
fn truncate(s: &str, cap: usize) -> String {
    if s.chars().count() <= cap {
        s.to_string()
    } else {
        let taken: String = s.chars().take(cap).collect();
        format!("{taken}…")
    }
}

// ── label building ─────────────────────────────────────────────────

/// Build a compact tool label suitable for inline display.
///
/// Returns something like:
/// - `Bash (cargo test -p thclaws-core)`
/// - `Read (crates/core/src/repl.rs)`
/// - `Grep (ToolCallStart)`
/// - `WebFetch (https://example.com/…)`
/// - `Task (agent=dev-glm)`
/// - fallback: `ToolName`
pub(crate) fn tool_label(name: &str, input: &serde_json::Value) -> String {
    let detail = match name {
        "Bash" => input
            .get("command")
            .and_then(|v| v.as_str())
            .map(|c| truncate(&sanitize_label_field(&redact_secrets(c)), PREVIEW_CAP)),
        "Read" | "Write" | "Edit" => input
            .get("path")
            .and_then(|v| v.as_str())
            .map(|p| p.to_string()),
        "Glob" => input
            .get("pattern")
            .and_then(|v| v.as_str())
            .map(|p| truncate(p, PREVIEW_CAP)),
        "Grep" => input
            .get("pattern")
            .and_then(|v| v.as_str())
            .map(|p| truncate(p, PREVIEW_CAP)),
        "WebFetch" => input
            .get("url")
            .and_then(|v| v.as_str())
            .map(|u| truncate(u, 60)),
        "WebSearch" => input
            .get("query")
            .and_then(|v| v.as_str())
            .map(|q| truncate(&sanitize_label_field(q), PREVIEW_CAP)),
        "Skill" => input
            .get("name")
            .and_then(|v| v.as_str())
            .map(|n| n.to_string()),
        "Task" => input
            .get("agent")
            .and_then(|v| v.as_str())
            .map(|a| format!("agent={a}")),
        "AskUserQuestion" => input
            .get("question")
            .and_then(|v| v.as_str())
            .map(|q| truncate(&sanitize_label_field(q), PREVIEW_CAP)),
        _ => None,
    };

    match detail {
        Some(d) if !d.is_empty() => format!("{name} ({d})"),
        _ => name.to_string(),
    }
}

// ── duration formatting ────────────────────────────────────────────

/// Format a duration as a human-friendly string like `1m12s`, `42s`, `0.3s`.
pub(crate) fn format_duration(d: Duration) -> String {
    let total_secs = d.as_secs_f64();
    if total_secs < 1.0 {
        format!("{:.1}s", total_secs)
    } else if total_secs < 60.0 {
        format!("{}s", total_secs.round() as u32)
    } else {
        let mins = total_secs as u32 / 60;
        let secs = total_secs as u32 % 60;
        format!("{mins}m{secs:02}s")
    }
}

// ── formatted output strings ───────────────────────────────────────

/// Heartbeat text for a long-running tool.
///
/// Example: `[tool: Bash (sleep 60)] still running 45s`
pub(crate) fn format_tool_heartbeat(label: &str, elapsed: Duration) -> String {
    format!("[tool: {label}] still running {}", format_duration(elapsed))
}

// ── active tool state ──────────────────────────────────────────────

/// Tracks a tool call in progress for heartbeat and duration reporting.
#[derive(Debug, Clone)]
pub(crate) struct ActiveToolDisplay {
    pub label: String,
    pub started_at: std::time::Instant,
    pub last_heartbeat_at: std::time::Instant,
}

impl ActiveToolDisplay {
    pub fn new(label: String) -> Self {
        let now = std::time::Instant::now();
        Self {
            label,
            started_at: now,
            last_heartbeat_at: now,
        }
    }

    /// Elapsed time since the tool started.
    pub fn elapsed(&self) -> Duration {
        self.started_at.elapsed()
    }

    /// Whether a heartbeat is due (first after `HEARTBEAT_FIRST_AFTER`,
    /// then every `HEARTBEAT_EVERY`).
    pub fn heartbeat_due(&self) -> bool {
        let since_start = self.started_at.elapsed();
        let since_last = self.last_heartbeat_at.elapsed();
        if since_start < HEARTBEAT_FIRST_AFTER {
            return false;
        }
        since_last >= HEARTBEAT_EVERY
    }
}

/// Pick the oldest tool that is due for a heartbeat, if any.
pub(crate) fn oldest_due_heartbeat(
    active: &std::collections::HashMap<String, ActiveToolDisplay>,
) -> Option<(&String, &ActiveToolDisplay)> {
    active
        .iter()
        .filter(|(_, td)| td.heartbeat_due())
        .min_by_key(|(_, td)| td.started_at)
}

/// Compute the delay until the next heartbeat should fire.
/// Returns `HEARTBEAT_EVERY` when no tools are active. The caller can
/// still safely poll the timer, but avoids scheduling an extreme
/// `Duration::MAX` sleep.
pub(crate) fn next_heartbeat_delay(
    active: &std::collections::HashMap<String, ActiveToolDisplay>,
) -> Duration {
    if active.is_empty() {
        return HEARTBEAT_EVERY;
    }
    active
        .values()
        .map(|td| {
            let elapsed = td.started_at.elapsed();
            if elapsed < HEARTBEAT_FIRST_AFTER {
                HEARTBEAT_FIRST_AFTER - elapsed
            } else {
                let since_last = td.last_heartbeat_at.elapsed();
                if since_last >= HEARTBEAT_EVERY {
                    Duration::ZERO
                } else {
                    HEARTBEAT_EVERY - since_last
                }
            }
        })
        .min()
        .unwrap_or(Duration::MAX)
}

// ── tests ──────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn label_bash_command() {
        let label = tool_label("Bash", &json!({"command": "cargo test -p thclaws-core"}));
        assert!(label.starts_with("Bash ("));
        assert!(label.contains("cargo test"));
    }

    #[test]
    fn label_bash_truncation() {
        let long = "x".repeat(200);
        let label = tool_label("Bash", &json!({"command": long}));
        assert!(label.contains('…'));
        let inner = label
            .strip_prefix("Bash (")
            .unwrap()
            .strip_suffix(')')
            .unwrap();
        assert!(inner.chars().count() <= PREVIEW_CAP + 1); // +1 for …
    }

    #[test]
    fn label_read_path() {
        let label = tool_label("Read", &json!({"path": "src/main.rs"}));
        assert_eq!(label, "Read (src/main.rs)");
    }

    #[test]
    fn label_edit_path() {
        let label = tool_label("Edit", &json!({"path": "crates/core/src/repl.rs"}));
        assert_eq!(label, "Edit (crates/core/src/repl.rs)");
    }

    #[test]
    fn label_grep_pattern() {
        let label = tool_label("Grep", &json!({"pattern": "ToolCallStart"}));
        assert_eq!(label, "Grep (ToolCallStart)");
    }

    #[test]
    fn label_webfetch_url() {
        let label = tool_label(
            "WebFetch",
            &json!({"url": "https://example.com/api/v1/data"}),
        );
        assert!(label.starts_with("WebFetch ("));
        assert!(label.contains("example.com"));
    }

    #[test]
    fn label_task_agent() {
        let label = tool_label("Task", &json!({"agent": "dev-glm"}));
        assert_eq!(label, "Task (agent=dev-glm)");
    }

    #[test]
    fn label_unknown_tool() {
        let label = tool_label("FancyTool", &json!({}));
        assert_eq!(label, "FancyTool");
    }

    #[test]
    fn label_fallback_on_missing_field() {
        let label = tool_label("Bash", &json!({}));
        assert_eq!(label, "Bash");
    }

    #[test]
    fn redact_token() {
        let result = redact_secrets("curl -H token=abc123 https://api.example.com");
        assert!(result.contains("<redacted>"));
        assert!(!result.contains("abc123"));
    }

    #[test]
    fn redact_bearer() {
        let result = redact_secrets("Authorization: Bearer sk-12345");
        assert!(result.contains("<redacted>"));
        assert!(!result.contains("sk-12345"));
    }

    #[test]
    fn redact_api_key() {
        let result = redact_secrets("api_key=deadbeef123&other=val");
        assert!(result.contains("<redacted>"));
        assert!(!result.contains("deadbeef"));
        assert!(result.contains("&other=val"));
    }

    #[test]
    fn redact_preserves_normal_text() {
        let input = "cargo test --no-run";
        assert_eq!(redact_secrets(input), input);
    }

    #[test]
    fn sanitize_strips_control_chars() {
        let result = sanitize_label_field("hello\nworld\ttab");
        assert_eq!(result, "hello world tab");
    }

    #[test]
    fn format_duration_sub_second() {
        assert_eq!(format_duration(Duration::from_millis(300)), "0.3s");
    }

    #[test]
    fn format_duration_seconds() {
        assert_eq!(format_duration(Duration::from_secs(42)), "42s");
    }

    #[test]
    fn format_duration_minutes_seconds() {
        assert_eq!(format_duration(Duration::from_secs(72)), "1m12s");
    }

    #[test]
    fn format_tool_heartbeat_text() {
        let text = format_tool_heartbeat("Bash (sleep 60)", Duration::from_secs(45));
        assert!(text.contains("still running"));
        assert!(text.contains("45s"));
    }

    #[test]
    fn active_tool_heartbeat_not_due_immediately() {
        let td = ActiveToolDisplay::new("test".to_string());
        assert!(!td.heartbeat_due());
    }

    #[test]
    fn label_ask_user_question() {
        let label = tool_label(
            "AskUserQuestion",
            &json!({"question": "Which library should we use?"}),
        );
        assert!(label.starts_with("AskUserQuestion ("));
        assert!(label.contains("Which library"));
    }

    #[test]
    fn label_skill_name() {
        let label = tool_label("Skill", &json!({"name": "prp-plan"}));
        assert_eq!(label, "Skill (prp-plan)");
    }

    #[test]
    fn label_websearch_query() {
        let label = tool_label("WebSearch", &json!({"query": "rust tokio select"}));
        assert_eq!(label, "WebSearch (rust tokio select)");
    }
}
