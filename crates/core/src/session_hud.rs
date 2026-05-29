//! Session HUD — sticky status bar on stderr.
//!
//! Spinner stays on stdout (unchanged). HUD writes to stderr using
//! `EraseDown` + content + `CursorPrevLine`. Both render on the same
//! terminal but through separate file descriptors — zero conflict.
//!
//! Bonus: when `$TMUX` is set, also updates the tmux pane border via OSC 2.
//!
//! `THCLAWS_HUD=0` disables. `THCLAWS_HUD=1` forces enable.

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
    prev_hud_lines: u16,
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
                std::io::stderr().is_terminal()
            }
        };
        Self {
            turn_started: Instant::now(),
            phase: HudPhase::Thinking,
            active_tool: None,
            enabled,
            active: false,
            prev_hud_lines: 0,
            session_cost: 0.0,
            context_pct: None,
            in_tmux: std::env::var("TMUX").is_ok(),
        }
    }

    pub fn on_turn_start(&mut self) {
        self.turn_started = Instant::now();
        self.phase = HudPhase::Thinking;
        self.active_tool = None;
        self.prev_hud_lines = 0;
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
        if self.active {
            self.clear_hud();
        }
        self.phase = HudPhase::Done;
        self.active_tool = None;
        self.active = false;
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

    pub fn render_tick(&mut self) {
        if !self.active {
            return;
        }
        let content = self.render_hud_content();
        let mut err = std::io::stderr();
        self.write_hud(&mut err, &content);
    }

    fn write_hud(&mut self, w: &mut dyn Write, content: &str) {
        if self.prev_hud_lines > 0 {
            for _ in 0..self.prev_hud_lines {
                let _ = write!(w, "\x1b[F");
            }
            let _ = write!(w, "\x1b[J");
        }
        let _ = writeln!(w, "{content}");
        self.prev_hud_lines = 1;
        let _ = write!(w, "\x1b[F");
        let _ = w.flush();

        if self.in_tmux {
            let plain = strip_ansi(content);
            let _ = write!(w, "\x1b]2;{plain}\x1b\\");
            let _ = w.flush();
        }
    }

    fn clear_hud(&mut self) {
        if self.prev_hud_lines == 0 {
            return;
        }
        let mut err = std::io::stderr();
        for _ in 0..self.prev_hud_lines {
            let _ = write!(err, "\x1b[F");
        }
        let _ = write!(err, "\x1b[J");
        let _ = err.flush();
        self.prev_hud_lines = 0;

        if self.in_tmux {
            let _ = write!(err, "\x1b]2;\x1b\\");
            let _ = err.flush();
        }
    }

    fn render_hud_content(&self) -> String {
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
}

impl Default for HudState {
    fn default() -> Self {
        Self {
            turn_started: Instant::now(),
            phase: HudPhase::Thinking,
            active_tool: None,
            enabled: false,
            active: false,
            prev_hud_lines: 0,
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

fn strip_ansi(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut in_escape = false;
    for ch in s.chars() {
        if in_escape {
            if ch.is_ascii_alphabetic() {
                in_escape = false;
            }
            continue;
        }
        if ch == '\x1b' {
            in_escape = true;
            continue;
        }
        out.push(ch);
    }
    out
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
            prev_hud_lines: 0,
            session_cost: 0.0,
            context_pct: None,
            in_tmux: false,
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
        let c = hud.render_hud_content();
        assert!(c.contains("thinking"));
        assert!(c.contains("\x1b[36m"));
        assert!(c.contains("⏱"));
    }

    #[test]
    fn render_tool_yellow() {
        let mut hud = hud_test();
        hud.on_tool_start("Bash");
        let c = hud.render_hud_content();
        assert!(c.contains("Bash"));
        assert!(c.contains("\x1b[33m"));
    }

    #[test]
    fn render_cost() {
        let mut hud = hud_test();
        hud.set_session_cost(0.05);
        assert!(hud.render_hud_content().contains("$0.05"));
    }

    #[test]
    fn render_no_cost_zero() {
        let hud = hud_test();
        assert!(!hud.render_hud_content().contains('$'));
    }

    #[test]
    fn render_context_gauge() {
        let mut hud = hud_test();
        hud.set_context_pct(60.0);
        let c = hud.render_hud_content();
        assert!(c.contains("ctx"));
        assert!(c.contains("60%"));
        assert!(c.contains("▓▓▓░░"));
    }

    #[test]
    fn render_no_gauge_unset() {
        assert!(!hud_test().render_hud_content().contains("ctx"));
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
    fn tool_sanitized() {
        let mut hud = hud_test();
        hud.on_tool_start("Bash\nrm -rf /\tmalicious");
        let c = hud.render_hud_content();
        assert!(!c.contains('\n'));
        assert!(!c.contains('\t'));
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
    fn tool_done_without_start_safe() {
        let mut hud = hud_test();
        hud.on_tool_done();
        assert_eq!(hud.phase, HudPhase::Thinking);
    }

    #[test]
    fn strip_ansi_removes_codes() {
        assert_eq!(strip_ansi("\x1b[36mhello\x1b[0m"), "hello");
        assert_eq!(strip_ansi("no codes"), "no codes");
        assert_eq!(strip_ansi("\x1b[33m⏱ 5s\x1b[0m"), "⏱ 5s");
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
        assert_eq!(
            crate::tool_display::format_duration(std::time::Duration::from_secs(5)),
            "5s"
        );
    }
}
