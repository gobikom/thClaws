//! Session HUD — periodic heartbeat liveness line for the interactive REPL.
//!
//! Issue #335 (v6). The interactive turn loop already animates an *in-place*
//! spinner (`tool_display::format_thinking_spinner` / `format_tool_spinner`,
//! rewritten via `\r\x1b[2K` every 100ms) and prints a completion summary at
//! turn end (`repl.rs`, `AgentEvent::Done`). What it lacked was a *permanent*
//! scrollback line proving the session is alive and how long the current turn
//! has run — useful when the in-place spinner can't animate reliably (xterm.js
//! / CodeWebway) or has scrolled out of view.
//!
//! The heartbeat is deliberately a NORMAL line (no `\r`, no cursor control): it
//! scrolls above the spinner like any other output and therefore can never
//! collide with the spinner's row. Five prior attempts (v1–v5) failed by trying
//! to render a persistent bottom line that shared the spinner's row; v6 avoids
//! that class of bug structurally. The unit tests assert the heartbeat string
//! contains no `\r`/`\x1b[2K` to lock that invariant in.

use std::time::{Duration, Instant};

/// Default seconds between heartbeats when `THCLAWS_HUD_INTERVAL` is unset.
const DEFAULT_INTERVAL_SECS: u64 = 30;

/// Lower bound for the heartbeat interval. Guards against a hostile/typo
/// `THCLAWS_HUD_INTERVAL=0` busy-spamming scrollback.
const MIN_INTERVAL_SECS: u64 = 5;

/// Whether the session HUD heartbeat is enabled.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum HudMode {
    Off,
    Heartbeat,
}

/// Heartbeat cadence state for one interactive turn.
#[derive(Debug, Clone)]
pub(crate) struct HudState {
    mode: HudMode,
    interval: Duration,
    last_emit: Instant,
}

impl HudState {
    /// Build from the environment.
    ///
    /// `THCLAWS_HUD`: `off` → disabled; `heartbeat` → enabled; unset → enabled
    /// only when attached to a TTY (`is_tty`), so piped / `--print` output stays
    /// scriptable by default.
    ///
    /// `THCLAWS_HUD_INTERVAL`: heartbeat period in seconds, clamped to at least
    /// `MIN_INTERVAL_SECS`. Unset → `DEFAULT_INTERVAL_SECS`. A non-numeric value
    /// is not silently ignored — it logs a one-line warning and falls back to
    /// the default (per the "no silent failures" rule).
    pub(crate) fn from_env(is_tty: bool) -> Self {
        Self::from_env_inner(
            std::env::var("THCLAWS_HUD").ok().as_deref(),
            std::env::var("THCLAWS_HUD_INTERVAL").ok().as_deref(),
            is_tty,
            Instant::now(),
        )
    }

    /// Pure constructor — env values injected so it is unit-testable without
    /// touching process-global state or the clock.
    fn from_env_inner(
        hud: Option<&str>,
        interval: Option<&str>,
        is_tty: bool,
        now: Instant,
    ) -> Self {
        let default_mode = if is_tty {
            HudMode::Heartbeat
        } else {
            HudMode::Off
        };
        let mode = match hud.map(|s| s.trim().to_ascii_lowercase()).as_deref() {
            Some("off") => HudMode::Off,
            Some("heartbeat") => HudMode::Heartbeat,
            Some("") | None => default_mode,
            Some(other) => {
                eprintln!(
                    "[thclaws] THCLAWS_HUD='{other}' not recognized (use 'off' or 'heartbeat'); \
                     defaulting to {}",
                    if is_tty { "heartbeat" } else { "off" }
                );
                default_mode
            }
        };

        let interval_secs = match interval.map(str::trim) {
            None | Some("") => DEFAULT_INTERVAL_SECS,
            Some(raw) => match raw.parse::<u64>() {
                Ok(n) => n.max(MIN_INTERVAL_SECS),
                Err(_) => {
                    eprintln!(
                        "[thclaws] THCLAWS_HUD_INTERVAL='{raw}' is not a positive integer; \
                         using default {DEFAULT_INTERVAL_SECS}s"
                    );
                    DEFAULT_INTERVAL_SECS
                }
            },
        };

        Self {
            mode,
            interval: Duration::from_secs(interval_secs),
            last_emit: now,
        }
    }

    /// Whether the HUD heartbeat is enabled at all.
    pub(crate) fn enabled(&self) -> bool {
        matches!(self.mode, HudMode::Heartbeat)
    }

    /// Whether a heartbeat is due as of `now`. `now` is injected so callers
    /// (and tests) control the clock.
    pub(crate) fn due(&self, now: Instant) -> bool {
        self.enabled() && now.duration_since(self.last_emit) >= self.interval
    }

    /// Record that a heartbeat was emitted at `now`.
    pub(crate) fn mark(&mut self, now: Instant) {
        self.last_emit = now;
    }

    /// How long the caller may sleep before it must re-check `due()`, given
    /// `now`. Returns `None` when the HUD is off (the caller keeps its own idle
    /// delay). When on, this is the time remaining until the next heartbeat,
    /// floored so a due heartbeat fires promptly without busy-spinning. The
    /// interactive loop uses this to cap its otherwise-long idle sleep so the
    /// heartbeat keeps its cadence even when no spinner is animating (e.g.
    /// during a long stretch of streamed text).
    pub(crate) fn poll_delay(&self, now: Instant) -> Option<Duration> {
        if !self.enabled() {
            return None;
        }
        let remaining = self
            .interval
            .saturating_sub(now.duration_since(self.last_emit));
        Some(remaining.max(crate::tool_display::SPINNER_INTERVAL))
    }
}

/// Format a heartbeat line for the session HUD.
///
/// PERMANENT scrollback line — MUST NOT contain `\r` or `\x1b[2K`. If a tool is
/// active, `active` carries its (already-redacted) label and elapsed time. The
/// label is produced by `tool_display::tool_label`, which redacts secrets, so no
/// extra sanitization is needed here.
///
/// Examples:
/// - `[⏱ 1m 00s · tool: Bash (psql … migrate) 30s]`
/// - `[⏱ 45s · thinking]`
pub(crate) fn format_session_heartbeat(
    elapsed: Duration,
    active: Option<(&str, Duration)>,
) -> String {
    use crate::tool_display::format_duration;
    match active {
        Some((label, tool_elapsed)) => format!(
            "[⏱ {} · tool: {} {}]",
            format_duration(elapsed),
            label,
            format_duration(tool_elapsed)
        ),
        None => format!("[⏱ {} · thinking]", format_duration(elapsed)),
    }
}

// ── tests ──────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── from_env_inner: mode matrix ──────────────────────────────────

    #[test]
    fn mode_off_explicit_disables_even_on_tty() {
        let s = HudState::from_env_inner(Some("off"), None, true, Instant::now());
        assert!(!s.enabled());
    }

    #[test]
    fn mode_heartbeat_explicit_enables_even_without_tty() {
        let s = HudState::from_env_inner(Some("heartbeat"), None, false, Instant::now());
        assert!(s.enabled());
    }

    #[test]
    fn mode_unset_defaults_on_with_tty() {
        let s = HudState::from_env_inner(None, None, true, Instant::now());
        assert!(s.enabled());
    }

    #[test]
    fn mode_unset_defaults_off_without_tty() {
        // Proves `--print` / piped output never heartbeats by default (AC7).
        let s = HudState::from_env_inner(None, None, false, Instant::now());
        assert!(!s.enabled());
    }

    #[test]
    fn mode_is_case_insensitive_and_trimmed() {
        assert!(!HudState::from_env_inner(Some("  OFF "), None, true, Instant::now()).enabled());
        assert!(HudState::from_env_inner(Some("HeartBeat"), None, false, Instant::now()).enabled());
    }

    #[test]
    fn mode_unrecognized_falls_back_to_tty_default() {
        // Non-empty garbage → default for the surface (warns, doesn't crash).
        assert!(HudState::from_env_inner(Some("loud"), None, true, Instant::now()).enabled());
        assert!(!HudState::from_env_inner(Some("loud"), None, false, Instant::now()).enabled());
    }

    #[test]
    fn mode_empty_string_treated_as_unset() {
        assert!(HudState::from_env_inner(Some(""), None, true, Instant::now()).enabled());
        assert!(!HudState::from_env_inner(Some(""), None, false, Instant::now()).enabled());
    }

    // ── from_env_inner: interval parsing ─────────────────────────────

    #[test]
    fn interval_default_when_unset() {
        let s = HudState::from_env_inner(Some("heartbeat"), None, true, Instant::now());
        assert_eq!(s.interval, Duration::from_secs(DEFAULT_INTERVAL_SECS));
    }

    #[test]
    fn interval_parsed_when_valid() {
        let s = HudState::from_env_inner(Some("heartbeat"), Some("12"), true, Instant::now());
        assert_eq!(s.interval, Duration::from_secs(12));
    }

    #[test]
    fn interval_clamped_to_minimum() {
        // Hostile/typo 0 (or anything below MIN) must not busy-spam scrollback.
        let s = HudState::from_env_inner(Some("heartbeat"), Some("0"), true, Instant::now());
        assert_eq!(s.interval, Duration::from_secs(MIN_INTERVAL_SECS));
        let s2 = HudState::from_env_inner(Some("heartbeat"), Some("1"), true, Instant::now());
        assert_eq!(s2.interval, Duration::from_secs(MIN_INTERVAL_SECS));
    }

    #[test]
    fn interval_invalid_falls_back_to_default_not_silent() {
        let s = HudState::from_env_inner(Some("heartbeat"), Some("soon"), true, Instant::now());
        assert_eq!(s.interval, Duration::from_secs(DEFAULT_INTERVAL_SECS));
    }

    // ── due / mark cadence (deterministic via injected clock) ────────

    #[test]
    fn not_due_before_interval() {
        let now = Instant::now();
        let s = HudState::from_env_inner(Some("heartbeat"), Some("30"), true, now);
        assert!(!s.due(now));
        assert!(!s.due(now + Duration::from_secs(29)));
    }

    #[test]
    fn due_at_and_after_interval() {
        let now = Instant::now();
        let s = HudState::from_env_inner(Some("heartbeat"), Some("30"), true, now);
        assert!(s.due(now + Duration::from_secs(30)));
        assert!(s.due(now + Duration::from_secs(31)));
    }

    #[test]
    fn never_due_when_off() {
        let now = Instant::now();
        let s = HudState::from_env_inner(Some("off"), Some("5"), true, now);
        assert!(!s.due(now + Duration::from_secs(3600)));
    }

    #[test]
    fn mark_resets_cadence() {
        let now = Instant::now();
        let mut s = HudState::from_env_inner(Some("heartbeat"), Some("30"), true, now);
        let t1 = now + Duration::from_secs(30);
        assert!(s.due(t1));
        s.mark(t1);
        assert!(!s.due(t1 + Duration::from_secs(29)));
        assert!(s.due(t1 + Duration::from_secs(30)));
    }

    // ── format_session_heartbeat: content + the v1–v5 invariant ──────

    #[test]
    fn heartbeat_thinking_variant() {
        let s = format_session_heartbeat(Duration::from_secs(45), None);
        assert_eq!(s, "[⏱ 45s · thinking]");
    }

    #[test]
    fn heartbeat_tool_variant() {
        let s = format_session_heartbeat(
            Duration::from_secs(60),
            Some(("Bash (migrate)", Duration::from_secs(30))),
        );
        assert_eq!(s, "[⏱ 1m 00s · tool: Bash (migrate) 30s]");
    }

    #[test]
    fn heartbeat_is_a_permanent_line_no_cursor_control() {
        // The core invariant: a heartbeat must NEVER contain in-place cursor
        // control, or it would fight the spinner for the same row — exactly the
        // bug that reverted v1–v5.
        for s in [
            format_session_heartbeat(Duration::from_secs(45), None),
            format_session_heartbeat(
                Duration::from_secs(60),
                Some(("Bash (x)", Duration::from_secs(30))),
            ),
        ] {
            assert!(!s.contains('\r'), "heartbeat must not contain CR: {s:?}");
            assert!(
                !s.contains("\x1b[2K"),
                "heartbeat must not contain erase-line: {s:?}"
            );
            assert!(
                !s.contains('\n'),
                "heartbeat line adds its own newline via println: {s:?}"
            );
        }
    }

    #[test]
    fn heartbeat_formatter_does_not_double_redact_or_introduce_secrets() {
        // Contract: format_session_heartbeat is a pure formatter; redaction is
        // the caller's responsibility (the label arrives via tool_display::
        // tool_label, which already redacts). This guards against a future
        // change that accidentally injects unredacted data here.
        let already_redacted = "Bash (--api-key=<redacted>)";
        let s = format_session_heartbeat(
            Duration::from_secs(45),
            Some((already_redacted, Duration::from_secs(10))),
        );
        assert!(s.contains("<redacted>"), "redacted marker preserved: {s:?}");
        assert!(
            !s.contains("sk-"),
            "formatter must not introduce secrets: {s:?}"
        );
    }

    // ── poll_delay: idle wake honors heartbeat cadence ───────────────

    #[test]
    fn poll_delay_none_when_off() {
        let now = Instant::now();
        let s = HudState::from_env_inner(Some("off"), None, true, now);
        assert!(s.poll_delay(now).is_none());
    }

    #[test]
    fn poll_delay_tracks_remaining_until_due() {
        let now = Instant::now();
        let s = HudState::from_env_inner(Some("heartbeat"), Some("30"), true, now);
        // Far from due → ~remaining time (floored at SPINNER_INTERVAL).
        let d = s.poll_delay(now + Duration::from_secs(10)).unwrap();
        assert!(d <= Duration::from_secs(20) && d >= crate::tool_display::SPINNER_INTERVAL);
        // Past due → floored to SPINNER_INTERVAL (prompt fire, no busy spin).
        let d2 = s.poll_delay(now + Duration::from_secs(60)).unwrap();
        assert_eq!(d2, crate::tool_display::SPINNER_INTERVAL);
    }

    // ── per-turn reset: fresh HudState restarts the cadence ──────────

    #[test]
    fn fresh_hud_state_per_turn_resets_cadence() {
        // repl.rs constructs a new HudState inside the per-turn block, so each
        // turn's cadence starts from its own init time. A refactor that lifted
        // construction out of the loop would break this — this test guards it.
        let t0 = Instant::now();
        let turn1 = HudState::from_env_inner(Some("heartbeat"), Some("5"), true, t0);
        assert!(turn1.due(t0 + Duration::from_secs(5)));

        let t1 = t0 + Duration::from_secs(3); // turn 2 starts fresh, 3s later
        let turn2 = HudState::from_env_inner(Some("heartbeat"), Some("5"), true, t1);
        assert!(!turn2.due(t1 + Duration::from_secs(4)));
        assert!(turn2.due(t1 + Duration::from_secs(5)));
    }
}
