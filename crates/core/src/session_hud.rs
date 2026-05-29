//! Session HUD + Canvas — single-owner rendering for sticky bottom area.
//!
//! `Canvas` renders spinner + HUD as ONE atomic block using cursor-up +
//! erase-down. This is the Superconsole/indicatif pattern: one render pass
//! owns the in-place area, so no cursor conflicts.
//!
//! `THCLAWS_HUD=0` disables the HUD line; Canvas still renders spinner alone.

use std::io::Write;
use std::time::Instant;

// ── Canvas: single-owner in-place renderer ─────────────────────────

/// Manages a multi-line in-place block at the bottom of terminal output.
/// Each `draw()` call clears the previous block and redraws.
pub(crate) struct Canvas {
    prev_lines: u16,
}

impl Canvas {
    pub fn new() -> Self {
        Self { prev_lines: 0 }
    }

    /// Clear previous canvas, draw new lines. Cursor ends on the last line.
    pub fn draw(&mut self, lines: &[&str]) {
        if lines.is_empty() {
            self.clear();
            return;
        }
        let mut out = String::new();
        if self.prev_lines > 0 {
            // Move cursor up to top of previous canvas
            for _ in 0..self.prev_lines {
                out.push_str("\x1b[A");
            }
            out.push('\r');
            out.push_str("\x1b[J"); // erase from cursor down
        }
        for (i, line) in lines.iter().enumerate() {
            out.push_str(line);
            if i < lines.len() - 1 {
                out.push('\n');
            }
        }
        self.prev_lines = lines.len() as u16;
        print!("{out}");
        let _ = std::io::stdout().flush();
    }

    /// Clear the canvas area — call before writing normal content.
    pub fn clear(&mut self) {
        if self.prev_lines > 0 {
            let mut out = String::new();
            for _ in 0..self.prev_lines {
                out.push_str("\x1b[A");
            }
            out.push('\r');
            out.push_str("\x1b[J");
            print!("{out}");
            let _ = std::io::stdout().flush();
            self.prev_lines = 0;
        }
    }

    pub fn is_drawn(&self) -> bool {
        self.prev_lines > 0
    }
}

/// Strip the `\r\x1b[2K` prefix that `format_thinking_spinner` / `format_tool_spinner`
/// prepend. Canvas handles clearing itself.
pub(crate) fn strip_line_clear(s: &str) -> &str {
    s.strip_prefix("\r\x1b[2K").unwrap_or(s)
}

// ── HUD state ──────────────────────────────────────────────────────

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
    session_cost: f64,
    context_pct: Option<f32>,
    in_tmux: bool,
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
            session_cost: 0.0,
            context_pct: None,
            in_tmux: std::env::var("TMUX").is_ok(),
        }
    }

    pub fn on_turn_start(&mut self) {
        self.turn_started = Instant::now();
        self.phase = HudPhase::Thinking;
        self.active_tool = None;
        if self.enabled {
            self.active = true;
        }
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
        self.phase = HudPhase::Done;
        self.active_tool = None;
        self.active = false;
        if self.in_tmux {
            eprint!("\x1b]2;\x1b\\");
            let _ = std::io::stderr().flush();
        }
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

    /// Render the HUD content line (no cursor control — Canvas handles that).
    pub fn render_line(&self) -> String {
        let elapsed = crate::tool_display::format_duration(self.turn_started.elapsed());

        let (color, phase_str) = match (&self.phase, &self.active_tool) {
            (HudPhase::ToolRunning, Some(tool)) => {
                let t = crate::tool_display::format_duration(tool.started.elapsed());
                ("\x1b[33m", format!("{} {}", tool.name, t))
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

        format!("{color}⏱ {elapsed} · {phase_str}{cost_str}{ctx_str}\x1b[0m")
    }

    /// Update tmux pane title with HUD summary.
    pub fn update_tmux_title(&self) {
        if !self.in_tmux || !self.active {
            return;
        }
        let elapsed = crate::tool_display::format_duration(self.turn_started.elapsed());
        let phase = match (&self.phase, &self.active_tool) {
            (HudPhase::ToolRunning, Some(tool)) => {
                let t = crate::tool_display::format_duration(tool.started.elapsed());
                format!("{} {}", tool.name, t)
            }
            _ => "thinking".into(),
        };
        let cost = if self.session_cost > 0.0 {
            format!(" ${:.2}", self.session_cost)
        } else {
            String::new()
        };
        eprint!("\x1b]2;⏱ {} · {}{}\x1b\\", elapsed, phase, cost);
        let _ = std::io::stderr().flush();
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
            session_cost: 0.0,
            context_pct: None,
            in_tmux: false,
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
            active: true,
            session_cost: 0.0,
            context_pct: None,
            in_tmux: false,
        }
    }

    // ── Canvas ──────────────────────────────────────────────────────

    #[test]
    fn canvas_new_has_zero_lines() {
        let c = Canvas::new();
        assert_eq!(c.prev_lines, 0);
        assert!(!c.is_drawn());
    }

    #[test]
    fn strip_line_clear_strips_prefix() {
        assert_eq!(strip_line_clear("\r\x1b[2Khello"), "hello");
        assert_eq!(strip_line_clear("no prefix"), "no prefix");
    }

    // ── Env ─────────────────────────────────────────────────────────

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

    // ── Render ──────────────────────────────────────────────────────

    #[test]
    fn render_thinking_cyan() {
        let hud = hud_test();
        let c = hud.render_line();
        assert!(c.contains("thinking"));
        assert!(c.contains("\x1b[36m"));
        assert!(c.contains("⏱"));
    }

    #[test]
    fn render_tool_yellow() {
        let mut hud = hud_test();
        hud.on_tool_start("Bash");
        let c = hud.render_line();
        assert!(c.contains("Bash"));
        assert!(c.contains("\x1b[33m"));
    }

    #[test]
    fn render_cost() {
        let mut hud = hud_test();
        hud.set_session_cost(0.05);
        assert!(hud.render_line().contains("$0.05"));
    }

    #[test]
    fn render_no_cost_zero() {
        assert!(!hud_test().render_line().contains('$'));
    }

    #[test]
    fn render_gauge_values() {
        let mut hud = hud_test();
        hud.set_context_pct(60.0);
        let c = hud.render_line();
        assert!(c.contains("▓▓▓░░"));
        assert!(c.contains("60%"));
    }

    #[test]
    fn render_no_gauge_unset() {
        assert!(!hud_test().render_line().contains("ctx"));
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
    }

    #[test]
    fn tool_sanitized() {
        let mut hud = hud_test();
        hud.on_tool_start("Bash\nrm\t/");
        assert!(!hud.render_line().contains('\n'));
        assert!(!hud.render_line().contains('\t'));
    }

    #[test]
    fn transitions() {
        let mut hud = hud_test();
        assert_eq!(hud.phase, HudPhase::Thinking);
        hud.on_tool_start("Read");
        assert_eq!(hud.phase, HudPhase::ToolRunning);
        hud.on_tool_done();
        assert_eq!(hud.phase, HudPhase::Thinking);
    }

    #[test]
    fn default_disabled() {
        let hud = HudState::default();
        assert!(!hud.is_enabled());
        assert!(!hud.is_active());
    }

    #[test]
    fn duration_format() {
        assert_eq!(
            crate::tool_display::format_duration(std::time::Duration::from_secs(70)),
            "1m 10s"
        );
    }
}
