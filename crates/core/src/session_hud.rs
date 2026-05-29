//! Sticky session HUD for the CLI REPL.
//!
//! Uses ANSI scroll regions to reserve the bottom terminal line for a
//! persistent status bar. Content (spinner, thinking, tool markers, text)
//! scrolls normally inside the region — zero changes to existing rendering.
//!
//! ```text
//! ┌───────────────────────────────────┐
//! │ content area (scroll region)      │
//! │ ⠹ Thinking (12s)                  │ ← spinner unchanged
//! │ [tool: Bash: sleep 10] ✓ 3s       │
//! ├───────────────────────────────────┤
//! │ ⏱ 42s · Bash 16s · $0.02         │ ← HUD (reserved line)
//! └───────────────────────────────────┘
//! ```
//!
//! Enabled by default when stdout is a terminal.
//! - `THCLAWS_HUD=0` — disable
//! - `THCLAWS_HUD=1` — force enable

use std::io::Write;
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
    active: bool,
    terminal_rows: u16,
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
            active: false,
            terminal_rows: 0,
            session_cost: 0.0,
            context_pct: None,
        }
    }

    pub fn on_turn_start(&mut self) {
        self.turn_started = Instant::now();
        self.phase = HudPhase::Thinking;
        self.active_tool = None;
        self.activate();
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
        self.deactivate();
    }

    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    pub fn is_active(&self) -> bool {
        self.active
    }

    pub fn set_session_cost(&mut self, cost: f64) {
        self.session_cost = cost;
    }

    pub fn set_context_pct(&mut self, pct: f32) {
        self.context_pct = Some(pct.clamp(0.0, 100.0));
    }

    /// Set scroll region to reserve bottom line, draw initial HUD.
    fn activate(&mut self) {
        if !self.enabled {
            return;
        }
        let rows = match terminal_rows() {
            Some(r) if r > 3 => r,
            _ => {
                return;
            }
        };
        self.terminal_rows = rows;
        self.active = true;
        let content = self.render_hud_content();
        print!(
            "\x1b[1;{}r\x1b[{};1H\x1b[2K{}\x1b[{};1H",
            rows - 1,
            rows,
            content,
            rows - 1
        );
        let _ = std::io::stdout().flush();
    }

    /// Restore full scroll region, clear HUD line.
    fn deactivate(&mut self) {
        if !self.active {
            return;
        }
        self.active = false;
        print!(
            "\x1b[{};1H\x1b[2K\x1b[r\x1b[{};1H",
            self.terminal_rows, self.terminal_rows
        );
        let _ = std::io::stdout().flush();
    }

    /// Update the HUD line in-place without disturbing content area.
    pub fn render_tick(&mut self) {
        if !self.active {
            return;
        }
        let content = self.render_hud_content();
        print!(
            "\x1b[s\x1b[{};1H\x1b[2K{}\x1b[u",
            self.terminal_rows, content
        );
        let _ = std::io::stdout().flush();
    }

    /// Format the HUD content string with phase color.
    fn render_hud_content(&self) -> String {
        let elapsed = crate::tool_display::format_duration(self.turn_started.elapsed());

        let (color, phase_str) = match (&self.phase, &self.active_tool) {
            (HudPhase::ToolRunning, Some(tool)) => {
                let t_elapsed = crate::tool_display::format_duration(tool.started.elapsed());
                ("\x1b[33m", format!("{} {}", tool.name, t_elapsed))
            }
            _ => ("\x1b[36m", "thinking".into()),
        };

        let cost_str = if self.session_cost > 0.0 {
            format!(" · ${:.2}", self.session_cost)
        } else {
            String::new()
        };

        let ctx_str = self
            .context_pct
            .map(|pct| format!(" · ctx {}", render_gauge(pct)))
            .unwrap_or_default();

        format!(
            "{color}⏱ {elapsed} · {phase_str}{cost_str}{ctx_str}\x1b[0m"
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
            active: false,
            terminal_rows: 0,
            session_cost: 0.0,
            context_pct: None,
        }
    }
}

fn terminal_rows() -> Option<u16> {
    #[cfg(unix)]
    unsafe {
        let mut ws: libc::winsize = std::mem::zeroed();
        if libc::ioctl(libc::STDOUT_FILENO, libc::TIOCGWINSZ, &mut ws) == 0 && ws.ws_row > 0 {
            return Some(ws.ws_row);
        }
    }
    None
}

fn enabled_from_env(val: Option<&str>) -> Option<bool> {
    match val {
        Some("0") => Some(false),
        Some("1") => Some(true),
        _ => None,
    }
}

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

    fn hud_test() -> HudState {
        HudState {
            turn_started: Instant::now(),
            phase: HudPhase::Thinking,
            active_tool: None,
            enabled: true,
            active: false,
            terminal_rows: 0,
            session_cost: 0.0,
            context_pct: None,
        }
    }

    #[test]
    fn env_disabled() {
        assert_eq!(enabled_from_env(Some("0")), Some(false));
    }

    #[test]
    fn env_enabled() {
        assert_eq!(enabled_from_env(Some("1")), Some(true));
    }

    #[test]
    fn env_none_falls_through() {
        assert_eq!(enabled_from_env(None), None);
    }

    #[test]
    fn env_unknown_falls_through() {
        assert_eq!(enabled_from_env(Some("yes")), None);
    }

    #[test]
    fn render_thinking_cyan() {
        let hud = hud_test();
        let content = hud.render_hud_content();
        assert!(content.contains("thinking"));
        assert!(content.contains("\x1b[36m"));
        assert!(content.contains("⏱"));
    }

    #[test]
    fn render_tool_yellow() {
        let mut hud = hud_test();
        hud.on_tool_start("Bash");
        let content = hud.render_hud_content();
        assert!(content.contains("Bash"));
        assert!(content.contains("\x1b[33m"));
    }

    #[test]
    fn render_cost_when_nonzero() {
        let mut hud = hud_test();
        hud.set_session_cost(0.05);
        let content = hud.render_hud_content();
        assert!(content.contains("$0.05"));
    }

    #[test]
    fn render_no_cost_when_zero() {
        let hud = hud_test();
        let content = hud.render_hud_content();
        assert!(!content.contains('$'));
    }

    #[test]
    fn render_context_gauge() {
        let mut hud = hud_test();
        hud.set_context_pct(60.0);
        let content = hud.render_hud_content();
        assert!(content.contains("ctx"));
        assert!(content.contains("60%"));
        assert!(content.contains("▓▓▓░░"));
    }

    #[test]
    fn render_no_gauge_when_unset() {
        let hud = hud_test();
        assert!(!hud.render_hud_content().contains("ctx"));
    }

    #[test]
    fn gauge_boundaries() {
        assert_eq!(render_gauge(0.0), "░░░░░ 0%");
        assert_eq!(render_gauge(100.0), "▓▓▓▓▓ 100%");
        assert_eq!(render_gauge(50.0), "▓▓▓░░ 50%");
    }

    #[test]
    fn gauge_clamps() {
        let mut hud = hud_test();
        hud.set_context_pct(150.0);
        assert_eq!(hud.context_pct, Some(100.0));
        hud.set_context_pct(-10.0);
        assert_eq!(hud.context_pct, Some(0.0));
    }

    #[test]
    fn tool_name_sanitized() {
        let mut hud = hud_test();
        hud.on_tool_start("Bash\nrm -rf /\tmalicious");
        let content = hud.render_hud_content();
        assert!(!content.contains('\n'));
        assert!(!content.contains('\t'));
    }

    #[test]
    fn transitions() {
        let mut hud = hud_test();
        assert_eq!(hud.phase, HudPhase::Thinking);

        hud.on_tool_start("Read");
        assert_eq!(hud.phase, HudPhase::ToolRunning);

        hud.on_tool_done();
        assert_eq!(hud.phase, HudPhase::Thinking);
        assert!(hud.active_tool.is_none());
    }

    #[test]
    fn tool_done_without_start_is_safe() {
        let mut hud = hud_test();
        hud.on_tool_done();
        assert_eq!(hud.phase, HudPhase::Thinking);
    }

    #[test]
    fn default_is_disabled() {
        let hud = HudState::default();
        assert!(!hud.is_enabled());
        assert!(!hud.is_active());
    }

    #[test]
    fn terminal_rows_returns_none_in_ci() {
        // In CI/test env there's no TTY — should return None, not panic
        let _ = terminal_rows();
    }

    #[test]
    fn duration_format() {
        let long = crate::tool_display::format_duration(std::time::Duration::from_secs(70));
        assert_eq!(long, "1m 10s");
        let short = crate::tool_display::format_duration(std::time::Duration::from_secs(5));
        assert_eq!(short, "5s");
    }
}
