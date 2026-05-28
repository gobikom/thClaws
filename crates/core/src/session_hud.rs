//! Lightweight session status HUD for thClaws terminal sessions.
//!
//! Shows elapsed time and current agent phase (thinking / tool running) as an
//! in-place overwriting status line during a turn. Token usage is printed at
//! turn completion by the existing token summary renderer.
//!
//! Enabled by default when stdout is a terminal. Override with env vars:
//! - `THCLAWS_HUD=0` — disable, `THCLAWS_HUD=1` — force enable
//! - `THCLAWS_HUD_INTERVAL=<seconds>` — update cadence (default: 2, clamped 1–60)

use std::time::Instant;

/// Current agent turn phase tracked by the HUD.
#[derive(Debug, Clone, PartialEq)]
pub enum HudPhase {
    Thinking,
    ToolRunning,
    Done,
}

struct ActiveTool {
    name: String,
    started: Instant,
}

/// HUD state machine for one REPL session.
///
/// Create once per session via [`HudState::new()`], then call
/// [`on_turn_start`] to reset for each new agent turn. Update with
/// [`on_tool_start`] / [`on_tool_done`] as tools run, and [`on_turn_done`]
/// at completion. Call [`render_line`] to get the current display string.
pub struct HudState {
    turn_started: Instant,
    phase: HudPhase,
    active_tool: Option<ActiveTool>,
    enabled: bool,
}

impl HudState {
    /// Create a new HUD state. Checks `THCLAWS_HUD` and stdout TTY status.
    pub fn new() -> Self {
        let enabled = match enabled_from_env(std::env::var("THCLAWS_HUD").ok().as_deref()) {
            Some(v) => v,
            None => {
                use std::io::IsTerminal as _;
                std::io::stdout().is_terminal()
            }
        };
        Self {
            turn_started: Instant::now(),
            phase: HudPhase::Thinking,
            active_tool: None,
            enabled,
        }
    }

    /// Reset state for the start of a new agent turn.
    pub fn on_turn_start(&mut self) {
        self.turn_started = Instant::now();
        self.phase = HudPhase::Thinking;
        self.active_tool = None;
    }

    /// Record that a named tool started running.
    ///
    /// `name` is sanitized via [`crate::tool_display::sanitize_label_field`]
    /// so no control characters or secrets leak into the HUD line.
    pub fn on_tool_start(&mut self, name: &str) {
        self.active_tool = Some(ActiveTool {
            name: crate::tool_display::sanitize_label_field(name),
            started: Instant::now(),
        });
        self.phase = HudPhase::ToolRunning;
    }

    /// Record that the current tool finished; returns to Thinking phase.
    /// Safe to call even if no tool was started (idempotent).
    pub fn on_tool_done(&mut self) {
        self.active_tool = None;
        self.phase = HudPhase::Thinking;
    }

    /// Record turn completion. After this, [`render_line`] returns empty.
    pub fn on_turn_done(&mut self) {
        self.active_tool = None;
        self.phase = HudPhase::Done;
    }

    /// Whether HUD output is enabled for this session.
    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    /// Whether the turn is still in progress (not yet Done).
    pub fn is_active(&self) -> bool {
        self.phase != HudPhase::Done
    }

    /// Read-only access to the current phase.
    pub fn phase(&self) -> &HudPhase {
        &self.phase
    }

    /// Update interval in seconds (from `THCLAWS_HUD_INTERVAL`, clamped 1–60, default 2).
    ///
    /// Zero and out-of-range values fall back to 2 so `tokio::time::interval`
    /// never receives a zero-duration (which would panic).
    pub fn interval_secs() -> u64 {
        std::env::var("THCLAWS_HUD_INTERVAL")
            .ok()
            .and_then(|s| s.parse::<u64>().ok())
            .map(|n| n.clamp(1, 60))
            .unwrap_or(2)
    }

    /// Render the current HUD line for in-place terminal display.
    ///
    /// Returns empty string when the turn is Done — callers should then clear
    /// the current terminal row separately. For active turns, callers prefix
    /// the returned string with `\r\x1b[2K` to overwrite the current line.
    pub fn render_line(&self) -> String {
        if self.phase == HudPhase::Done {
            return String::new();
        }
        let elapsed_str = crate::tool_display::format_duration(self.turn_started.elapsed());
        match &self.active_tool {
            Some(tool) => {
                let tool_elapsed =
                    crate::tool_display::format_duration(tool.started.elapsed());
                format!("⏱ {} · {} {}", elapsed_str, tool.name, tool_elapsed)
            }
            None => format!("⏱ {} · thinking", elapsed_str),
        }
    }
}

impl Default for HudState {
    /// Returns a disabled HUD state with no I/O or env-var side effects.
    /// Use [`HudState::new()`] to respect the `THCLAWS_HUD` env var.
    fn default() -> Self {
        Self {
            turn_started: Instant::now(),
            phase: HudPhase::Thinking,
            active_tool: None,
            enabled: false,
        }
    }
}

/// Pure function: parse `THCLAWS_HUD` env value → `Some(bool)` or `None`
/// (falls through to TTY detection). Extracted for test isolation — tests
/// call this directly instead of mutating process environment.
fn enabled_from_env(val: Option<&str>) -> Option<bool> {
    match val {
        Some("0") => Some(false),
        Some("1") => Some(true),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hud_forced(enabled: bool) -> HudState {
        HudState {
            turn_started: Instant::now(),
            phase: HudPhase::Thinking,
            active_tool: None,
            enabled,
        }
    }

    // ── env parsing (pure, no process env mutation) ──────────────────

    #[test]
    fn hud_disabled_when_env_zero() {
        assert_eq!(enabled_from_env(Some("0")), Some(false));
    }

    #[test]
    fn hud_enabled_when_env_one() {
        assert_eq!(enabled_from_env(Some("1")), Some(true));
    }

    #[test]
    fn hud_env_none_falls_through_to_tty() {
        assert_eq!(enabled_from_env(None), None);
    }

    #[test]
    fn hud_env_unknown_value_falls_through_to_tty() {
        assert_eq!(enabled_from_env(Some("yes")), None);
    }

    // ── interval_secs ─────────────────────────────────────────────────

    #[test]
    fn interval_secs_defaults_to_two() {
        // When env var is absent the default must be 2 (never 0, which would panic tokio).
        // We read the real env here, so the test is stable as long as the var is unset in CI.
        if std::env::var("THCLAWS_HUD_INTERVAL").is_err() {
            assert_eq!(HudState::interval_secs(), 2);
        }
    }

    #[test]
    fn interval_secs_valid_range_is_enforced() {
        // Verify the clamp bounds used by interval_secs().
        assert_eq!(0u64.clamp(1, 60), 1, "0 must clamp to 1 (prevents tokio panic)");
        assert_eq!(1u64.clamp(1, 60), 1, "1 is the minimum valid value");
        assert_eq!(60u64.clamp(1, 60), 60, "60 is the maximum valid value");
        assert_eq!(3600u64.clamp(1, 60), 60, "large values clamp to 60");
        // Confirm interval_secs() itself never returns 0.
        let result = HudState::interval_secs();
        assert!(result >= 1, "interval_secs() must always return ≥1; got {result}");
        assert!(result <= 60, "interval_secs() must always return ≤60; got {result}");
    }

    // ── render ────────────────────────────────────────────────────────

    #[test]
    fn hud_render_thinking_phase() {
        let hud = hud_forced(true);
        let line = hud.render_line();
        assert!(line.contains("thinking"), "expected 'thinking' in '{line}'");
        assert!(line.starts_with('⏱'), "expected ⏱ prefix in '{line}'");
    }

    #[test]
    fn hud_render_tool_running() {
        let mut hud = hud_forced(true);
        hud.on_tool_start("Bash");
        let line = hud.render_line();
        assert!(line.contains("Bash"), "expected 'Bash' in '{line}'");
        assert!(line.contains('⏱'), "expected ⏱ in '{line}'");
        assert!(line.contains('·'), "expected separator in '{line}'");
    }

    #[test]
    fn hud_render_formats_duration() {
        let long = crate::tool_display::format_duration(std::time::Duration::from_secs(70));
        assert_eq!(long, "1m 10s");
        let short = crate::tool_display::format_duration(std::time::Duration::from_secs(5));
        assert_eq!(short, "5s");
    }

    #[test]
    fn hud_tool_name_sanitized() {
        let mut hud = hud_forced(true);
        hud.on_tool_start("Bash\nrm -rf /\tmalicious");
        let line = hud.render_line();
        assert!(!line.is_empty(), "HUD line should not be empty after sanitization");
        assert!(!line.contains('\n'), "no newlines in HUD line");
        assert!(!line.contains('\t'), "no tabs in HUD line");
    }

    // ── state transitions ─────────────────────────────────────────────

    #[test]
    fn hud_transitions() {
        let mut hud = hud_forced(true);
        assert_eq!(hud.phase(), &HudPhase::Thinking);
        assert!(hud.is_active());

        hud.on_tool_start("Read");
        assert_eq!(hud.phase(), &HudPhase::ToolRunning);
        assert!(hud.active_tool.is_some());

        hud.on_tool_done();
        assert_eq!(hud.phase(), &HudPhase::Thinking);
        assert!(hud.active_tool.is_none());

        hud.on_turn_done();
        assert_eq!(hud.phase(), &HudPhase::Done);
        assert!(!hud.is_active());
        assert_eq!(hud.render_line(), "");
    }

    #[test]
    fn hud_tool_done_without_prior_start_is_idempotent() {
        let mut hud = hud_forced(true);
        // Should not panic or corrupt state when no tool was started.
        hud.on_tool_done();
        assert_eq!(hud.phase(), &HudPhase::Thinking);
        assert!(hud.render_line().contains("thinking"));
    }

    #[test]
    fn hud_turn_start_mid_turn_resets_state() {
        let mut hud = hud_forced(true);
        hud.on_tool_start("Bash");
        hud.on_turn_start(); // reset mid-turn
        assert_eq!(hud.phase(), &HudPhase::Thinking);
        assert!(hud.active_tool.is_none());
        assert!(hud.render_line().contains("thinking"));
    }

    #[test]
    fn hud_default_is_disabled_and_pure() {
        let hud = HudState::default();
        assert!(!hud.is_enabled(), "Default should be disabled (no I/O)");
        assert_eq!(hud.phase(), &HudPhase::Thinking);
        assert!(hud.active_tool.is_none());
    }
}
