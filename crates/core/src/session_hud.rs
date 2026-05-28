//! Unified session status HUD for thClaws terminal sessions.
//!
//! Renders a single in-place status line at the bottom of terminal output,
//! combining the spinner animation, elapsed time, current phase/tool,
//! session cost, and context window gauge into one compact bar:
//!
//! ```text
//! ─── ⠹ ⏱ 42s · Bash 16s · $0.02 · ctx ▓▓▓░░ 48%
//! ```
//!
//! Phase-based color coding: cyan = thinking, yellow = tool running.
//!
//! Enabled by default when stdout is a terminal. Override with env vars:
//! - `THCLAWS_HUD=0` — disable, `THCLAWS_HUD=1` — force enable

use std::time::Instant;

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

pub struct HudState {
    turn_started: Instant,
    phase: HudPhase,
    active_tool: Option<ActiveTool>,
    enabled: bool,
    spinner_tick: u32,
    session_cost: f64,
    context_pct: Option<f32>,
}

impl HudState {
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
            spinner_tick: 0,
            session_cost: 0.0,
            context_pct: None,
        }
    }

    pub fn on_turn_start(&mut self) {
        self.turn_started = Instant::now();
        self.phase = HudPhase::Thinking;
        self.active_tool = None;
        self.spinner_tick = 0;
    }

    pub fn on_tool_start(&mut self, name: &str) {
        self.active_tool = Some(ActiveTool {
            name: crate::tool_display::sanitize_label_field(name),
            started: Instant::now(),
        });
        self.phase = HudPhase::ToolRunning;
    }

    pub fn on_tool_done(&mut self) {
        self.active_tool = None;
        self.phase = HudPhase::Thinking;
    }

    pub fn on_turn_done(&mut self) {
        self.active_tool = None;
        self.phase = HudPhase::Done;
    }

    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    pub fn is_active(&self) -> bool {
        self.phase != HudPhase::Done
    }

    pub fn phase(&self) -> &HudPhase {
        &self.phase
    }

    /// Advance the spinner frame. Call once per animation tick (~100ms).
    pub fn tick(&mut self) {
        self.spinner_tick = self.spinner_tick.wrapping_add(1);
    }

    /// Update cumulative session cost (USD). Called after each turn.
    pub fn set_session_cost(&mut self, cost: f64) {
        self.session_cost = cost;
    }

    /// Update context window usage percentage (0–100). Called at turn
    /// boundaries when the estimate is available.
    pub fn set_context_pct(&mut self, pct: f32) {
        self.context_pct = Some(pct.clamp(0.0, 100.0));
    }

    /// Render the unified HUD line with separator, spinner, phase color,
    /// elapsed time, tool state, cost, and context gauge.
    ///
    /// Returns empty when the turn is Done.
    pub fn render_line(&self) -> String {
        if self.phase == HudPhase::Done {
            return String::new();
        }
        let frame = crate::tool_display::spinner_frame(self.spinner_tick);
        let elapsed = crate::tool_display::format_duration(self.turn_started.elapsed());

        let color = match self.phase {
            HudPhase::Thinking => "\x1b[36m",
            HudPhase::ToolRunning => "\x1b[33m",
            HudPhase::Done => "\x1b[32m",
        };

        let phase_str = match &self.active_tool {
            Some(tool) => {
                let t_elapsed = crate::tool_display::format_duration(tool.started.elapsed());
                format!("{} {}", tool.name, t_elapsed)
            }
            None => "thinking".into(),
        };

        let cost_str = if self.session_cost > 0.0 {
            format!(" · ${:.2}", self.session_cost)
        } else {
            String::new()
        };

        let ctx_str = self.context_pct
            .map(|pct| format!(" · ctx {}", render_gauge(pct)))
            .unwrap_or_default();

        format!(
            "{color}─── {frame} ⏱ {elapsed} · {phase_str}{cost_str}{ctx_str}\x1b[0m"
        )
    }
}

impl Default for HudState {
    fn default() -> Self {
        Self {
            turn_started: Instant::now(),
            phase: HudPhase::Thinking,
            active_tool: None,
            enabled: false,
            spinner_tick: 0,
            session_cost: 0.0,
            context_pct: None,
        }
    }
}

fn enabled_from_env(val: Option<&str>) -> Option<bool> {
    match val {
        Some("0") => Some(false),
        Some("1") => Some(true),
        _ => None,
    }
}

/// Render a 5-segment gauge bar with percentage.
fn render_gauge(pct: f32) -> String {
    let filled = ((pct / 100.0) * 5.0).round() as usize;
    let filled = filled.min(5);
    let empty = 5 - filled;
    format!(
        "{}{} {}%",
        "▓".repeat(filled),
        "░".repeat(empty),
        pct as u32
    )
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
            spinner_tick: 0,
            session_cost: 0.0,
            context_pct: None,
        }
    }

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
    fn hud_env_unknown_value_falls_through() {
        assert_eq!(enabled_from_env(Some("yes")), None);
    }

    #[test]
    fn hud_render_thinking_phase_has_cyan_and_separator() {
        let hud = hud_forced(true);
        let line = hud.render_line();
        assert!(line.contains("thinking"), "expected 'thinking' in '{line}'");
        assert!(line.contains("───"), "expected separator in '{line}'");
        assert!(line.contains("\x1b[36m"), "expected cyan color in '{line}'");
    }

    #[test]
    fn hud_render_tool_running_has_yellow() {
        let mut hud = hud_forced(true);
        hud.on_tool_start("Bash");
        let line = hud.render_line();
        assert!(line.contains("Bash"), "expected 'Bash' in '{line}'");
        assert!(line.contains("\x1b[33m"), "expected yellow color in '{line}'");
    }

    #[test]
    fn hud_render_includes_spinner_frame() {
        let mut hud = hud_forced(true);
        hud.tick();
        let line = hud.render_line();
        assert!(!line.is_empty());
        assert!(line.contains('⏱'));
    }

    #[test]
    fn hud_render_includes_cost_when_nonzero() {
        let mut hud = hud_forced(true);
        hud.set_session_cost(0.05);
        let line = hud.render_line();
        assert!(line.contains("$0.05"), "expected cost in '{line}'");
    }

    #[test]
    fn hud_render_hides_cost_when_zero() {
        let hud = hud_forced(true);
        let line = hud.render_line();
        assert!(!line.contains('$'), "cost should not appear when zero");
    }

    #[test]
    fn hud_render_includes_context_gauge() {
        let mut hud = hud_forced(true);
        hud.set_context_pct(60.0);
        let line = hud.render_line();
        assert!(line.contains("ctx"), "expected ctx gauge in '{line}'");
        assert!(line.contains("60%"), "expected 60% in '{line}'");
        assert!(line.contains("▓▓▓░░"), "expected 3 filled + 2 empty");
    }

    #[test]
    fn hud_render_no_context_gauge_when_unset() {
        let hud = hud_forced(true);
        let line = hud.render_line();
        assert!(!line.contains("ctx"), "no gauge when unset");
    }

    #[test]
    fn gauge_boundary_values() {
        assert_eq!(render_gauge(0.0), "░░░░░ 0%");
        assert_eq!(render_gauge(100.0), "▓▓▓▓▓ 100%");
        assert_eq!(render_gauge(50.0), "▓▓▓░░ 50%");
    }

    #[test]
    fn gauge_clamps_out_of_range() {
        let mut hud = hud_forced(true);
        hud.set_context_pct(150.0);
        assert_eq!(hud.context_pct, Some(100.0));
        hud.set_context_pct(-10.0);
        assert_eq!(hud.context_pct, Some(0.0));
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
        assert!(!line.contains('\n'), "no newlines in HUD line");
        assert!(!line.contains('\t'), "no tabs in HUD line");
    }

    #[test]
    fn hud_transitions() {
        let mut hud = hud_forced(true);
        assert_eq!(hud.phase(), &HudPhase::Thinking);
        assert!(hud.is_active());

        hud.on_tool_start("Read");
        assert_eq!(hud.phase(), &HudPhase::ToolRunning);

        hud.on_tool_done();
        assert_eq!(hud.phase(), &HudPhase::Thinking);

        hud.on_turn_done();
        assert_eq!(hud.phase(), &HudPhase::Done);
        assert!(!hud.is_active());
        assert_eq!(hud.render_line(), "");
    }

    #[test]
    fn hud_tool_done_without_prior_start_is_idempotent() {
        let mut hud = hud_forced(true);
        hud.on_tool_done();
        assert_eq!(hud.phase(), &HudPhase::Thinking);
    }

    #[test]
    fn hud_turn_start_resets_state() {
        let mut hud = hud_forced(true);
        hud.on_tool_start("Bash");
        hud.set_session_cost(1.0);
        hud.on_turn_start();
        assert_eq!(hud.phase(), &HudPhase::Thinking);
        assert!(hud.active_tool.is_none());
        assert_eq!(hud.spinner_tick, 0);
        assert_eq!(hud.session_cost, 1.0, "cost persists across turns");
    }

    #[test]
    fn hud_default_is_disabled() {
        let hud = HudState::default();
        assert!(!hud.is_enabled());
    }
}
