//! tmux pane-border live HUD (agent-devops#368).
//!
//! Renders a persistent status line on the tmux **pane border** instead of
//! inside the pane content. tmux draws the border server-side, so the HUD
//! updates live even in web terminals (CodeWebway/xterm.js) and cannot collide
//! with the in-pane spinner or scrollback. This is the realtime, always-visible
//! HUD that the periodic scrollback heartbeat (#335) could not provide.
//!
//! Mechanism (verified):
//! ```text
//! tmux set -w pane-border-status top
//! tmux set -w pane-border-format "#{@thclaws_hud}"
//! tmux set -p @thclaws_hud " ⏱ 42s · $0.12 · model "   # each ~1s
//! tmux refresh-client                                    # force redraw
//! ```

use std::process::Command;

/// The pane-scoped user option the border format reads from.
const HUD_OPTION: &str = "@thclaws_hud";

/// Controls the tmux pane-border HUD for the current REPL session.
///
/// `setup()` records the prior `pane-border-status` / `pane-border-format`
/// window options and switches the border on. `update()` changes the displayed
/// text. The prior options are restored on `Drop`, so the user's border returns
/// to normal on `/quit`, Ctrl-C, or an error exit.
pub struct TmuxHud {
    active: bool,
    prev_status: Option<String>,
    prev_format: Option<String>,
}

impl TmuxHud {
    /// Set up the pane border when running inside tmux. Returns an inactive
    /// (no-op) HUD when `$TMUX` is unset or any tmux command fails — callers can
    /// always construct it and call `update()` unconditionally.
    pub fn setup() -> Self {
        if std::env::var_os("TMUX").is_none() {
            return Self::inactive();
        }
        let prev_status = tmux_capture(&["show-options", "-wv", "pane-border-status"]);
        let prev_format = tmux_capture(&["show-options", "-wv", "pane-border-format"]);
        let ok = tmux_ok(&["set-option", "-w", "pane-border-status", "top"])
            && tmux_ok(&["set-option", "-w", "pane-border-format", "#{@thclaws_hud}"]);
        if !ok {
            // Best-effort revert of any partial change, then stay inactive.
            let mut partial = Self {
                active: true,
                prev_status,
                prev_format,
            };
            partial.restore();
            return Self::inactive();
        }
        Self {
            active: true,
            prev_status,
            prev_format,
        }
    }

    fn inactive() -> Self {
        Self {
            active: false,
            prev_status: None,
            prev_format: None,
        }
    }

    /// Whether the HUD is rendering (inside tmux and set up successfully).
    pub fn is_active(&self) -> bool {
        self.active
    }

    /// Update the border HUD text. Best-effort; failures are ignored (a missing
    /// HUD is cosmetic and must never disrupt the REPL).
    pub fn update(&self, text: &str) {
        if !self.active {
            return;
        }
        let _ = tmux_ok(&["set-option", "-p", HUD_OPTION, &format!(" {text} ")]);
        let _ = tmux_ok(&["refresh-client"]);
    }

    fn restore(&mut self) {
        if !self.active {
            return;
        }
        restore_window_option("pane-border-status", &self.prev_status);
        restore_window_option("pane-border-format", &self.prev_format);
        let _ = tmux_ok(&["set-option", "-pu", HUD_OPTION]);
        let _ = tmux_ok(&["refresh-client"]);
        self.active = false;
    }
}

impl Drop for TmuxHud {
    fn drop(&mut self) {
        self.restore();
    }
}

/// Restore a window option to its prior value, or unset it (`-u`, back to the
/// inherited default) when it had no explicit value before.
fn restore_window_option(name: &str, prev: &Option<String>) {
    match prev {
        Some(v) if !v.is_empty() => {
            let _ = tmux_ok(&["set-option", "-w", name, v]);
        }
        _ => {
            let _ = tmux_ok(&["set-option", "-wu", name]);
        }
    }
}

/// Run a tmux command, returning whether it succeeded.
fn tmux_ok(args: &[&str]) -> bool {
    Command::new("tmux")
        .args(args)
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Run a tmux command and capture trimmed stdout, or `None` on failure.
fn tmux_capture(args: &[&str]) -> Option<String> {
    let out = Command::new("tmux").args(args).output().ok()?;
    if out.status.success() {
        Some(String::from_utf8_lossy(&out.stdout).trim().to_string())
    } else {
        None
    }
}
