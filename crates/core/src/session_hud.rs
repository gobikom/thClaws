//! Lightweight session status HUD for thClaws terminal sessions.
//!
//! Shows elapsed time and current agent phase (thinking / tool running) as an
//! in-place overwriting status line during a turn. Token usage is printed at
//! turn completion by the existing token summary renderer.
//!
//! Enabled by default when stdout is a terminal. Override with env vars:
//! - `THCLAWS_HUD=0` — disable, `THCLAWS_HUD=1` — force enable
//! - `THCLAWS_HUD_INTERVAL=<seconds>` — update cadence (default: 2)

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

/// HUD state machine. One instance per agent turn.
///
/// Call [`on_turn_start`] at the beginning of each turn, then update with
/// [`on_tool_start`] / [`on_tool_done`] as tools run, and [`on_turn_done`]
/// at completion. Call [`render_line`] to get the current display string.
pub struct HudState {
    turn_started: Instant,
    pub phase: HudPhase,
    active_tool: Option<ActiveTool>,
    enabled: bool,
}

impl HudState {
    /// Create a new HUD state. Checks `THCLAWS_HUD` and stdout TTY status.
    pub fn new() -> Self {
        let enabled = match std::env::var("THCLAWS_HUD").as_deref() {
            Ok("0") => false,
            Ok("1") => true,
            _ => {
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

    /// The update interval in seconds (from `THCLAWS_HUD_INTERVAL`, default 2).
    pub fn interval_secs() -> u64 {
        std::env::var("THCLAWS_HUD_INTERVAL")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(2)
    }

    /// Render the current HUD line for in-place terminal display.
    ///
    /// Returns an empty string when the turn is Done (signals "clear the line").
    /// The caller wraps with `\r\x1b[2K` to overwrite the current terminal line.
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
    fn default() -> Self {
        Self::new()
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

    #[test]
    fn hud_disabled_when_env_zero() {
        // Safety: unit tests may run in parallel; this is best-effort.
        unsafe { std::env::set_var("THCLAWS_HUD", "0") };
        let hud = HudState::new();
        unsafe { std::env::remove_var("THCLAWS_HUD") };
        assert!(!hud.is_enabled());
    }

    #[test]
    fn hud_enabled_when_env_one() {
        unsafe { std::env::set_var("THCLAWS_HUD", "1") };
        let hud = HudState::new();
        unsafe { std::env::remove_var("THCLAWS_HUD") };
        assert!(hud.is_enabled());
    }

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
    }

    #[test]
    fn hud_render_formats_duration() {
        let line = crate::tool_display::format_duration(std::time::Duration::from_secs(70));
        assert_eq!(line, "1m 10s");
        let short = crate::tool_display::format_duration(std::time::Duration::from_secs(5));
        assert_eq!(short, "5s");
    }

    #[test]
    fn hud_tool_name_sanitized() {
        let mut hud = hud_forced(true);
        hud.on_tool_start("Bash\nrm -rf /\tmalicious");
        let line = hud.render_line();
        assert!(!line.contains('\n'), "no newlines in HUD line");
        assert!(!line.contains('\t'), "no tabs in HUD line");
    }

    #[test]
    fn hud_transitions() {
        let mut hud = hud_forced(true);
        assert_eq!(hud.phase, HudPhase::Thinking);
        assert!(hud.is_active());

        hud.on_tool_start("Read");
        assert_eq!(hud.phase, HudPhase::ToolRunning);
        assert!(hud.active_tool.is_some());

        hud.on_tool_done();
        assert_eq!(hud.phase, HudPhase::Thinking);
        assert!(hud.active_tool.is_none());

        hud.on_turn_done();
        assert_eq!(hud.phase, HudPhase::Done);
        assert!(!hud.is_active());
        assert_eq!(hud.render_line(), "");
    }
}
