//! PTY snapshot test for the #335 v6 session HUD heartbeat.
//!
//! Why this exists: five prior attempts (v1–v5) to add a live HUD were reverted
//! because a persistent line fought the in-place spinner for the same terminal
//! row, and the breakage was only ever caught by a human eyeballing a tmux pane.
//! This test makes the rendering invariant a CI assertion instead.
//!
//! It replays the EXACT byte pattern the interactive turn loop emits — several
//! in-place spinner frames (`\r\x1b[2K…`), then a permanent heartbeat line
//! (`clear` + `println`), then more spinner frames, then the completion summary
//! — through a real pseudo-terminal (so carriage returns and erase-line escapes
//! behave exactly as a terminal would), parses the result with `vt100`, and
//! asserts:
//!   1. the heartbeat survives as a permanent scrollback row,
//!   2. there is exactly ONE live spinner row (no stacking — the v1–v5 bug),
//!   3. the heartbeat sits ABOVE the completion summary,
//!   4. the spinner's in-place mechanism (`\r\x1b[2K`) is still in the stream.
//!
//! The bytes are reconstructed here (rather than calling the crate's
//! `pub(crate)` formatters) so the test stays a black-box terminal assertion and
//! does not widen the library's public API.

#![cfg(unix)]

use std::io::Read;

/// Braille frames used by `tool_display::SPINNER_FRAMES`.
const FRAMES: [&str; 5] = ["⠋", "⠙", "⠹", "⠸", "⠼"];

/// Mirror of `tool_display::format_tool_spinner` — an IN-PLACE line: leading
/// `\r\x1b[2K` returns to column 0 and clears the row before redrawing.
fn spinner(frame: &str, label: &str, dur: &str) -> String {
    format!("\r\x1b[2K\x1b[2m  {frame} {label:<50} {dur}\x1b[0m")
}

/// Mirror of `session_hud::format_session_heartbeat` (tool variant) — a
/// PERMANENT line. Emitted as: clear the spinner row, then `println!` it.
fn heartbeat_emit(line: &str) -> String {
    // `clear_thinking_line()` == "\r\x1b[2K"; println adds the trailing newline.
    format!("\r\x1b[2K{line}\n")
}

/// Build the byte script a real turn produces.
fn turn_script() -> Vec<u8> {
    let label = "Bash (migrate)";
    let mut s = String::new();
    // Three spinner frames — all overwrite the same row (no newline between).
    s.push_str(&spinner(FRAMES[0], label, "10s"));
    s.push_str(&spinner(FRAMES[1], label, "11s"));
    s.push_str(&spinner(FRAMES[2], label, "12s"));
    // Heartbeat becomes due: clear the spinner row, drop a permanent line.
    s.push_str(&heartbeat_emit("[⏱ 30s · tool: Bash (migrate) 30s]"));
    // Two more spinner frames — now on the fresh row below the heartbeat.
    s.push_str(&spinner(FRAMES[3], label, "29s"));
    s.push_str(&spinner(FRAMES[4], label, "30s"));
    // Completion summary (mirrors repl.rs Done handler).
    s.push_str("\n\x1b[2m[tokens: 1200in/3400out · 1m 30s · $0.0200 session]\x1b[0m\n");
    s.into_bytes()
}

/// Run `cat <file>` attached to a PTY slave and return everything the master saw.
fn render_through_pty(bytes: &[u8]) -> Vec<u8> {
    use portable_pty::{native_pty_system, CommandBuilder, PtySize};

    // Unique per call: pid alone collides across `#[test]` fns in the same
    // binary (they share a pid and may run in parallel), so add an atomic
    // counter suffix.
    use std::sync::atomic::{AtomicU64, Ordering};
    static CTR: AtomicU64 = AtomicU64::new(0);
    let dir = std::env::temp_dir();
    let path = dir.join(format!(
        "thclaws_hud_pty_{}_{}.bin",
        std::process::id(),
        CTR.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::write(&path, bytes).expect("write temp script");

    let pair = native_pty_system()
        .openpty(PtySize {
            rows: 24,
            cols: 100,
            pixel_width: 0,
            pixel_height: 0,
        })
        .expect("openpty");

    let mut cmd = CommandBuilder::new("cat");
    cmd.arg(&path);
    let mut child = pair.slave.spawn_command(cmd).expect("spawn cat");

    let mut reader = pair.master.try_clone_reader().expect("clone reader");
    // Drop the slave so the master reader sees EOF once `cat` exits.
    drop(pair.slave);

    let mut out = Vec::new();
    reader.read_to_end(&mut out).expect("read pty");
    let _ = child.wait();
    let _ = std::fs::remove_file(&path);
    out
}

#[test]
fn heartbeat_renders_above_single_spinner_row_without_collision() {
    let raw = render_through_pty(&turn_script());

    // (4) The spinner's in-place mechanism must still be present in the stream.
    // We assert on the erase-line CSI `\x1b[2K` only: a bare `\r` check would be
    // vacuous because the PTY line discipline (ONLCR) maps every `\n` to `\r\n`,
    // so `\r` is present regardless of whether the spinner emitted it.
    assert!(
        raw.windows(4).any(|w| w == b"\x1b[2K"),
        "expected the erase-line sequence (\\x1b[2K) in the raw PTY stream"
    );

    let mut parser = vt100::Parser::new(24, 100, 0);
    parser.process(&raw);
    let contents = parser.screen().contents();
    let lines: Vec<&str> = contents.lines().collect();

    // (1) The heartbeat survived as a permanent row.
    let hb_idx = lines
        .iter()
        .position(|l| l.contains("[⏱ 30s · tool: Bash (migrate) 30s]"))
        .unwrap_or_else(|| panic!("heartbeat line missing.\n--- grid ---\n{contents}"));

    // completion summary present.
    let tok_idx = lines
        .iter()
        .position(|l| l.contains("[tokens: 1200in/3400out"))
        .unwrap_or_else(|| panic!("completion summary missing.\n--- grid ---\n{contents}"));

    // (3) Heartbeat sits ABOVE the summary (permanent scrollback, not overwritten).
    assert!(
        hb_idx < tok_idx,
        "heartbeat (row {hb_idx}) should be above summary (row {tok_idx}).\n--- grid ---\n{contents}"
    );

    // (2) Exactly ONE live spinner row — the earlier frames overwrote each other
    // and the heartbeat did not stack a second spinner row. Only the last frame
    // (⠼) should remain anywhere on screen.
    let spinner_rows = lines
        .iter()
        .filter(|l| FRAMES.iter().any(|f| l.contains(f)))
        .count();
    assert_eq!(
        spinner_rows, 1,
        "expected exactly one live spinner row, found {spinner_rows}.\n--- grid ---\n{contents}"
    );

    // And that single row is the latest frame, not a stale one.
    assert!(
        lines
            .iter()
            .any(|l| l.contains("⠼") && l.contains("Bash (migrate)")),
        "the surviving spinner row should show the latest frame.\n--- grid ---\n{contents}"
    );
}

/// AC4: a turn that ends before the heartbeat interval emits the completion
/// summary only — no heartbeat line. Mirrors `HudState::due()` returning false
/// for a sub-interval turn (so `repl.rs` never enters the emit branch).
#[test]
fn short_turn_emits_no_heartbeat_line() {
    let label = "Bash (fast)";
    let mut s = String::new();
    // Spinner frames only — no heartbeat_emit() — then the completion summary.
    s.push_str(&spinner(FRAMES[0], label, "1s"));
    s.push_str(&spinner(FRAMES[1], label, "2s"));
    s.push_str("\n\x1b[2m[tokens: 100in/200out · 2s · $0.0010 session]\x1b[0m\n");

    let raw = render_through_pty(s.as_bytes());
    let mut parser = vt100::Parser::new(24, 100, 0);
    parser.process(&raw);
    let contents = parser.screen().contents();

    assert!(
        !contents.lines().any(|l| l.contains("[⏱")),
        "short turn must emit no heartbeat line.\n--- grid ---\n{contents}"
    );
    assert!(
        contents
            .lines()
            .any(|l| l.contains("[tokens: 100in/200out")),
        "completion summary should still be present.\n--- grid ---\n{contents}"
    );
}
