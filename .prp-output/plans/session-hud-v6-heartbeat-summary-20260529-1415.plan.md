---
status: pending
runner: cargo
mode: full
issue: gobikom/agent-devops#335
feature: session-hud-v6-heartbeat-summary
complexity: MEDIUM
created: 2026-05-29
---

# Plan: thClaws #335 v6 — Live Session Heartbeat + Completion Summary

## 1. Summary

Add a **periodic heartbeat scrollback line** to the thClaws interactive REPL turn loop so a
human watching a `cw-thclaws-*` (tmux/CodeWebway) session can see, at a glance, that the
session is alive and how long the current turn has run — even when the in-place spinner can't
animate reliably (xterm.js) or has scrolled out of view.

This is the **v6 approach** after v1–v5 were all reverted. v1–v5 tried to render a *persistent
bottom HUD line* that fought the existing in-place spinner (`\r\x1b[2K`) for the same terminal
row. v6 abandons the persistent-bottom-line idea entirely: the heartbeat is a **normal
scrollback line** (`println!`, no `\r`, no cursor control) emitted on its own cadence. It
**cannot** collide with the spinner because it never shares the spinner's row — it scrolls
above it like any other output line. This is the structural reason v6 succeeds where v1–v5 failed.

The completion summary (`[tokens: …in/…out · cache · elapsed · $cost]`) **already exists** in
the interactive REPL (`repl.rs:9093-9141`) — this plan only ensures it stays correct and adds
the heartbeat + config gate + a PTY-snapshot test that makes the spinner/heartbeat coexistence
a CI assertion (the meta-fix for the revert loop).

## 2. User Story

```
As นุด watching a long-running thClaws turn in a cw-thclaws-* tmux pane
I want a periodic heartbeat line showing elapsed time + current phase/tool
So that I can tell the session is alive (not hung/looping) without staring at an
   in-place spinner that may not animate in the web terminal and leaves no scrollback trail
```

## 3. Problem / Solution

- **Problem**: In `cw-thclaws-*` sessions the only liveness signal is the in-place spinner
  (`format_thinking_spinner`/`format_tool_spinner`, rewritten every 100ms via `\r\x1b[2K`).
  It leaves no scrollback trail and may not animate reliably in xterm.js, so the human cannot
  tell "is it alive / how long / is it looping". Team mode (`repl.rs:4607`) already solves this
  with a heartbeat; the interactive REPL does not.
- **Solution**: Emit a periodic **normal** scrollback line (default every 30s) during a turn,
  showing elapsed + phase (thinking / tool) + active tool label & tool-elapsed. Gate via
  `THCLAWS_HUD` / `THCLAWS_HUD_INTERVAL`. Reuse the existing `tool_display` helpers. Short turns
  (< interval) emit nothing extra. Completion summary already present — verify + keep.

## 4. Metadata

| Field | Value |
|-------|-------|
| Type | ENHANCEMENT |
| Complexity | MEDIUM |
| Affected systems | `crates/core` REPL interactive turn loop, `tool_display` module, new test |
| Dependencies | none (heartbeat/cost/redaction infra already present) |
| Task count | 6 |
| Runner | cargo |
| Type Check | `cargo check -p thclaws-core` |
| Lint | `cargo clippy -p thclaws-core --all-targets -- -D warnings` |
| Test | `cargo test -p thclaws-core` |
| Build | `cargo build -p thclaws-core` |
| Format | `cargo fmt --check` |

## 5. UX Design

### BEFORE (interactive turn, long Bash tool)
```
❯ run the migration
  ⠹ Bash (psql … migrate)                          42s        ← in-place spinner, rewrites this row
                                                                  every 100ms; nothing in scrollback;
                                                                  in xterm.js may look frozen
[tokens: 1.2kin/3.4kout · 1m 30s · $0.02 session]              ← summary already exists at turn end
```

### AFTER (`THCLAWS_HUD=heartbeat`, default; interval 30s)
```
❯ run the migration
[⏱ 30s · tool: Bash (psql … migrate) 30s]                      ← NEW permanent heartbeat line (scrollback)
[⏱ 60s · tool: Bash (psql … migrate) 60s]                      ← NEW, every 30s
  ⠹ Bash (psql … migrate)                          72s        ← spinner unchanged, still animates in-place
[tokens: 1.2kin/3.4kout · 1m 30s · $0.02 session]              ← summary unchanged
```
Short turn (< 30s): no heartbeat lines, summary only — identical to BEFORE.

### Interaction changes

| Location | Before | After | Trigger | Impact |
|----------|--------|-------|---------|--------|
| interactive turn loop | spinner only (ephemeral) | spinner + periodic permanent heartbeat | turn elapsed ≥ interval | liveness survives scroll + works without animation |
| `--print` / non-TTY | no heartbeat | no heartbeat (gated off) | n/a | scriptability preserved |

## 6. Mandatory Reading (implementer reads FIRST)

| Priority | File:Lines | Why |
|----------|-----------|-----|
| P0 | `crates/core/src/repl.rs:8835-9156` | The interactive turn loop — the ONLY integration site. Note `turn_start`, `active_tools`, the `tokio::select!` timer arm (8862-8882), and the existing Done summary (9093-9141). |
| P0 | `crates/core/src/tool_display.rs:1-372` | Heartbeat/spinner helpers to reuse: `format_tool_heartbeat` (permanent line, NO `\r`), `format_duration`, `ActiveToolDisplay`, redaction. |
| P1 | `crates/core/src/repl.rs:4600-4681` | Team-mode loop — the existing heartbeat-in-select pattern to mirror. |
| P1 | `crates/core/src/repl.rs:3879-3940` | `--print`/`-p` path (`run_print_mode`) — must stay heartbeat-free. |
| P2 | `crates/core/src/providers/anthropic.rs:520+` | Example `wiremock` MockServer test pattern, if the e2e PTY variant is attempted. |
| P2 | `crates/core/src/bin/app.rs:5-66` | How `--print` / `--verbose` flags reach the REPL. |

External docs:
- [`portable-pty` v0.8 docs](https://docs.rs/portable-pty/0.8) — KEY_INSIGHT: `native_pty_system().openpty()` gives a master/slave pair; spawn a command on the slave, read rendered bytes from the master. APPLIES_TO: Task 5. GOTCHA: must drain the master reader on a thread or the child blocks on a full pipe.
- [`vt100` v0.15 docs](https://docs.rs/vt100/0.15) — KEY_INSIGHT: `vt100::Parser::new(rows, cols, 0).process(&bytes)` then `screen().contents()` returns the rendered grid as text — handles `\r\x1b[2K` exactly like a real terminal. APPLIES_TO: Task 5 assertions.

## 7. Patterns to Mirror (ACTUAL snippets)

**Heartbeat is a PERMANENT line (no `\r`) — SOURCE: codebase `tool_display.rs:229-231`**
```rust
pub(crate) fn format_tool_heartbeat(label: &str, elapsed: Duration) -> String {
    format!("[tool: {label}] still running {}", format_duration(elapsed))
}
```
Contrast — spinner IS in-place (`\r\x1b[2K`). SOURCE: `tool_display.rs:234-238`:
```rust
pub(crate) fn format_tool_spinner(label: &str, elapsed: Duration, tick: u32) -> String {
    let frame = spinner_frame(tick);
    let dur = format_duration(elapsed);
    format!("\r\x1b[2K\x1b[2m  {frame} {label:<50} {dur}\x1b[0m")
}
```

**Heartbeat-in-select pattern — SOURCE: codebase `repl.rs:4619-4627` (team loop)**
```rust
_ = tokio::time::sleep(heartbeat_delay) => {
    if let Some(id) = crate::tool_display::oldest_due_heartbeat(&team_active_tools).map(|(k, _)| k.clone()) {
        if let Some(td) = team_active_tools.get_mut(&id) {
            team_println!("\n{}", crate::tool_display::format_tool_heartbeat(&td.label, td.elapsed()));
            td.last_heartbeat_at = std::time::Instant::now();
        }
    }
    continue;
}
```

**Env-var gate — SOURCE: codebase `mcp.rs:173` style**
```rust
std::env::var("THCLAWS_MCP_ALLOW_ALL").ok().as_deref() == Some("1")
```

**TTY detection — SOURCE: codebase `repl.rs:5091-5092`**
```rust
use std::io::IsTerminal as _;
let is_tty = std::io::stdout().is_terminal();
```

**Existing completion summary (KEEP, do not duplicate) — SOURCE: `repl.rs:9129-9132`**
```rust
println!(
    "\n{COLOR_DIM}[tokens: {}in/{}out{} · {}{}]{COLOR_RESET}",
    usage.input_tokens, usage.output_tokens, cache_info, elapsed, cost_str
);
```

## 8. Files to Change

| Action | File | Insert At | Justification |
|--------|------|-----------|---------------|
| CREATE | `crates/core/src/session_hud.rs` | new file | Pure, unit-testable HUD config + cadence + formatting. Keeps the `repl.rs` integration thin and makes the revert-prone logic testable without a terminal. |
| UPDATE | `crates/core/src/lib.rs` | after existing `mod` lines (~`mod tool_display;`) | Register `mod session_hud;`. |
| UPDATE | `crates/core/src/repl.rs` | turn-loop init (~8843) + timer arm (~8862-8882) | Init `HudState` at turn start; emit heartbeat in the existing select timer arm. ZERO changes to spinner format calls. |
| CREATE | `crates/core/tests/session_hud_pty.rs` | new file | PTY-snapshot test (AC9) — asserts heartbeat + spinner coexist in a real vt100 grid. |
| UPDATE | `crates/core/Cargo.toml` | `[dev-dependencies]` | Add `portable-pty = "0.8"`, `vt100 = "0.15"`. |

## 9. Integration Points

1. **Turn-loop state init** — `repl.rs:~8843`, right after `active_tools` is declared:
   construct `let mut hud = crate::session_hud::HudState::from_env(std::io::stdout().is_terminal());`
2. **Heartbeat emission** — inside the existing `tokio::select!` timer arm (`repl.rs:8862-8882`),
   after `spinner_tick += 1;`: ask `hud` if a heartbeat is due; if so, clear the spinner's
   in-place row, `println!` the permanent heartbeat, reset the cadence. The spinner redraws on
   the next 100ms tick. (Detail in Task 4.)
3. **Print mode** — `run_print_mode` (`repl.rs:3879`) is NOT touched → heartbeat never fires there.
4. **Completion summary** — already at `repl.rs:9093-9141`; no change (verify in Task 6).

## 10. NOT Building (explicit scope limits)

- **No persistent bottom-line / status-bar HUD** (the v1–v5 idea). Deferred to a follow-up issue;
  recommended future approach: tmux `pane-border-format` + `@user-option` (renders outside the
  pane, cannot conflict with the spinner). Out of scope here.
- **No new cost model / pricing** — reuse existing `model_catalogue::compute_cost_usd`.
- **No changes to the spinner animation** — `format_thinking_spinner` / `format_tool_spinner` /
  their callsites are untouched.
- **No live token telemetry mid-turn** — tokens shown at completion only (provider gives `usage`
  on `Done`). Heartbeat shows elapsed + phase/tool, not tokens.
- **No `--print` heartbeat** — scriptability preserved.

## 11. Step-by-Step Tasks

### Task 1 — CREATE `session_hud.rs`: config + cadence (pure logic)
- **ACTION**: CREATE `crates/core/src/session_hud.rs`.
- **IMPLEMENT**:
  - `pub(crate) enum HudMode { Off, Heartbeat }`.
  - `pub(crate) struct HudState { mode: HudMode, interval: Duration, last_emit: Instant }`.
  - `HudState::from_env(is_tty: bool) -> Self`:
    - `THCLAWS_HUD`: `off` → `Off`; `heartbeat` → `Heartbeat`; unset → `Heartbeat` if `is_tty` else `Off` (conservative: non-TTY/piped defaults off).
    - `THCLAWS_HUD_INTERVAL`: parse u64 seconds, clamp to `>= 5`, default `30`. Invalid → default + `eprintln!` one-line warning (explicit logging per philosophy; no silent fallback).
  - `fn due(&self, now: Instant) -> bool` — `matches!(mode, Heartbeat)` AND `now.duration_since(last_emit) >= interval`.
  - `fn mark(&mut self, now: Instant)` — `last_emit = now`.
- **MIRROR**: env pattern `mcp.rs:173`; struct/impl style `tool_display.rs:297-331`.
- **IMPORTS**: `use std::time::{Duration, Instant};`
- **GOTCHA**: clamp interval `>= 5s` so a hostile/typo `THCLAWS_HUD_INTERVAL=0` can't busy-spam scrollback. Inject `now` as a param (don't call `Instant::now()` inside `due`) so unit tests are deterministic.
- **VALIDATE**: `cargo test -p thclaws-core session_hud`

### Task 2 — CREATE heartbeat formatter (permanent line, no `\r`)
- **ACTION**: add to `session_hud.rs`.
- **IMPLEMENT**: `pub(crate) fn format_session_heartbeat(elapsed: Duration, active: Option<(&str, Duration)>) -> String`:
  - tool active → `format!("[⏱ {} · tool: {} {}]", format_duration(elapsed), label, format_duration(tool_elapsed))`
  - else → `format!("[⏱ {} · thinking]", format_duration(elapsed))`
  - Reuse `crate::tool_display::format_duration`. Label already redacted at `ActiveToolDisplay` creation (`tool_label` → `preview` → `redact_secrets`), so no secret leak (AC6).
  - MUST NOT contain `\r` or `\x1b[2K` — it is a permanent scrollback line.
- **MIRROR**: `tool_display.rs:229-231`.
- **GOTCHA**: do not prefix with `\r` — that would turn it into an in-place line and reintroduce the v1–v5 collision. Add a unit test asserting `!s.contains('\r')`.
- **VALIDATE**: `cargo test -p thclaws-core session_hud`

### Task 3 — Register module
- **ACTION**: UPDATE `crates/core/src/lib.rs` — add `mod session_hud;` next to `mod tool_display;`.
- **VALIDATE**: `cargo check -p thclaws-core`

### Task 4 — Integrate into the interactive turn loop
- **ACTION**: UPDATE `crates/core/src/repl.rs`.
- **IMPLEMENT**:
  1. At `~8843` (after `active_tools` decl): `let mut hud = crate::session_hud::HudState::from_env(std::io::stdout().is_terminal());`
  2. In the select timer arm (`~8862`, after `spinner_tick += 1;`), add:
     ```rust
     let now = std::time::Instant::now();
     if hud.due(now) {
         // Clear the spinner's in-place row, drop a PERMANENT heartbeat line,
         // then let the next 100ms tick redraw the spinner on the fresh row.
         let active = active_tools
             .values()
             .min_by_key(|td| td.started_at)
             .map(|td| (td.label.as_str(), td.elapsed()));
         print!("{}", crate::tool_display::clear_thinking_line()); // "\r\x1b[2K"
         println!("{}", crate::session_hud::format_session_heartbeat(turn_start.elapsed(), active));
         let _ = std::io::stdout().flush();
         hud.mark(now);
     }
     ```
- **MIRROR**: team-loop heartbeat `repl.rs:4619-4627`; `clear_thinking_line` usage `repl.rs:8856`.
- **GOTCHA**:
  - Place the heartbeat block BEFORE the `if is_connecting … else …` spinner-render block in the
    timer arm so the row is cleared, the permanent line printed, then the spinner re-renders in the
    same tick (clean) — OR after, accepting one stale spinner row that the next tick overwrites.
    Prefer BEFORE.
  - The timer arm only fires every 100ms while `is_connecting || is_thinking_after_tool ||
    !active_tools.is_empty()` (anim_delay). During pure Text streaming the arm is idle (300s) — that's
    fine: streaming text IS the liveness signal, mirroring the team loop which only heartbeats around
    Text/tool events. Document this; do not try to heartbeat during text streaming.
  - ZERO edits to `format_thinking_spinner` / `format_tool_spinner` or their existing call sites.
- **VALIDATE**: `cargo build -p thclaws-core && cargo clippy -p thclaws-core --all-targets -- -D warnings`

### Task 5 — CREATE PTY snapshot test (AC9 — the meta-fix)
- **ACTION**: CREATE `crates/core/tests/session_hud_pty.rs`; UPDATE `Cargo.toml` dev-deps.
- **IMPLEMENT** (primary, deterministic — preferred over full-binary e2e):
  - Open a PTY via `portable_pty::native_pty_system().openpty(PtySize{rows:24, cols:100,..})`.
  - On the slave writer, replay a realistic byte sequence the loop produces: several
    `format_tool_spinner(label, t, tick)` frames (each starts `\r\x1b[2K`), then a
    `clear_thinking_line()` + `println!`-style permanent `format_session_heartbeat(...)` line,
    then more spinner frames, then the completion `[tokens: …]` line. (Import the `pub(crate)`
    helpers is not possible from an integration test, so the test reconstructs the exact byte
    pattern the loop emits, OR expose a tiny `#[doc(hidden)] pub fn` shim; prefer reconstructing
    the documented byte pattern to avoid widening the public API.)
  - Read all master bytes; feed to `vt100::Parser::new(24, 100, 0)`; assert on `screen().contents()`:
    1. a line containing `⏱` and `thinking`/`tool:` is present in the grid (heartbeat survived as scrollback),
    2. exactly one "live" spinner row remains (the heartbeat did NOT stack multiple spinner rows),
    3. the `[tokens:` summary line is present,
    4. raw byte stream still contains `\r\x1b[2K` (spinner mechanism intact).
- **FALLBACK (optional, mark `#[ignore]`)**: full e2e — spawn `thclaws --cli` in the PTY against a
  `wiremock` MockServer streaming a slow response with `THCLAWS_HUD_INTERVAL=1`; assert a heartbeat
  line appears. Heavier/flakier (needs provider base-URL override + fake key); keep `#[ignore]` so
  CI stays green and a human can run it on demand.
- **MIRROR**: wiremock pattern `providers/anthropic.rs:520+`.
- **IMPORTS**: `use portable_pty::{native_pty_system, PtySize, CommandBuilder};` `use vt100::Parser;`
- **GOTCHA**: drain the master reader on a separate thread before/while writing or the slave write
  can block on a full buffer. Use a bounded read loop with a timeout, not `read_to_end` on a live PTY.
- **VALIDATE**: `cargo test -p thclaws-core --test session_hud_pty`

### Task 6 — Verify completion summary unchanged + add regression note
- **ACTION**: confirm `repl.rs:9093-9141` still prints `[tokens: …in/…out · cache · elapsed · $cost]`
  and that `--print`/`run_print_mode` emits no heartbeat.
- **IMPLEMENT**: add a unit test in `session_hud.rs`: `from_env` with `THCLAWS_HUD` unset + `is_tty=false`
  → `Off` (proves `--print`/piped never heartbeats). And `THCLAWS_HUD=off` → `Off` regardless of TTY.
- **GOTCHA**: env-var tests share process env — use distinct var values per test and set/remove within
  the test, or gate behind a serial test helper; clippy may warn on `set_var` in tests (allow locally).
- **VALIDATE**: `cargo test -p thclaws-core session_hud`

## 12. Testing Strategy

| Test | Type | Asserts |
|------|------|---------|
| `from_env` matrix | unit | off/heartbeat/unset×TTY mapping; interval parse+clamp(≥5)+default 30; invalid→default+warn |
| `due`/`mark` cadence | unit | no emit before interval; emit at/after; deterministic via injected `now` |
| `format_session_heartbeat` | unit | contains `⏱`+elapsed; tool vs thinking variants; **`!contains('\r')` and `!contains("\x1b[2K")`** (permanent-line invariant) |
| `session_hud_pty` | integration (PTY+vt100) | heartbeat line present in grid; single spinner row; summary present; `\r\x1b[2K` intact in raw bytes |
| full e2e (wiremock+binary) | integration `#[ignore]` | heartbeat appears in real `thclaws --cli` turn |

Edge cases checklist:
- [ ] `THCLAWS_HUD_INTERVAL=0` / negative / non-numeric → clamped/defaulted, warned, never spams
- [ ] short turn (< interval) → zero heartbeat lines (AC4)
- [ ] non-TTY / piped / `--print` → no heartbeat (AC7)
- [ ] tool label with a secret-shaped arg → redacted in heartbeat (AC6, inherited from `tool_label`)
- [ ] heartbeat does not leave a duplicated/stacked spinner row (AC8)

## 13. Validation Commands

```bash
# L1 Static analysis
cd /home/openclaw/repos/agents/thclaws
cargo fmt --check
cargo clippy -p thclaws-core --all-targets -- -D warnings

# L2 Unit tests (new module)
cargo test -p thclaws-core session_hud

# L3 Full crate test suite
cargo test -p thclaws-core

# L4 PTY snapshot integration test
cargo test -p thclaws-core --test session_hud_pty

# L5 Build
cargo build -p thclaws-core

# L6 Manual smoke (TTY) — long turn shows heartbeat, short turn does not
THCLAWS_HUD=heartbeat THCLAWS_HUD_INTERVAL=5 cargo run -p thclaws-core --bin thclaws -- --cli
#   then: ask something that triggers a long tool; observe periodic [⏱ …] lines + intact spinner
THCLAWS_HUD=off       cargo run -p thclaws-core --bin thclaws -- --cli   # no heartbeat
echo "hi" | cargo run -p thclaws-core --bin thclaws -- --print "say hi"  # no heartbeat (piped)
```

## 14. Confidence Score: 9/10

(Patterns:2 + Gotchas:2 + Integration:2 + Validation:2 + Testing:1)
- −1 on Testing: the PTY/vt100 integration test is the one genuinely new technique in this repo;
  primary deterministic variant de-risks it, but first wiring of `portable-pty`+`vt100` may need a
  short iteration. Everything else mirrors existing, tested code.

## 15. Acceptance Criteria (maps to issue #335 v6)

- [ ] AC1: periodic heartbeat scrollback line during a turn at configurable interval (default 30s)
- [ ] AC2: heartbeat shows elapsed + phase (thinking/tool) + active tool label & tool-elapsed
- [ ] AC3: completion summary shows tokens in/out + cache + elapsed + cost (already present — verified)
- [ ] AC4: short turns (< interval) emit summary only, no heartbeat
- [ ] AC5: `THCLAWS_HUD=off|heartbeat` + `THCLAWS_HUD_INTERVAL=<sec>`; conservative default (off on non-TTY/`--print`)
- [ ] AC6: no secrets in heartbeat (inherited from `tool_label`/`redact_secrets`)
- [ ] AC7: `--print` scriptability preserved (no heartbeat)
- [ ] AC8: existing spinner/thinking animation unchanged (zero spinner-callsite edits)
- [ ] AC9: PTY snapshot test asserts heartbeat + summary render and do not corrupt spinner/prompt
- [ ] Unit tests cover ≥ 90% of new `session_hud.rs` code

## 16. Completion Checklist

- [ ] L1 fmt + clippy clean
- [ ] L2 session_hud unit tests pass
- [ ] L3 full `thclaws-core` suite green
- [ ] L4 PTY snapshot test passes
- [ ] L5 release build OK
- [ ] L6 manual TTY smoke confirms heartbeat + intact spinner; piped/`--print` silent

## 17. Risks and Mitigations

| Risk | Likelihood | Impact | Mitigation |
|------|-----------|--------|-----------|
| Heartbeat reintroduces spinner-row collision (the v1–v5 failure) | Low | High | Heartbeat is a permanent `println!` line (no `\r`); unit test asserts no `\r`/`\x1b[2K`; PTY test asserts single spinner row. Structural, not cosmetic, guarantee. |
| `portable-pty`+`vt100` test flaky / hard to wire | Med | Med | Primary test is deterministic (replays byte pattern, no live binary/API); full e2e is `#[ignore]`. |
| Heartbeat too chatty in normal use | Low | Low | 30s default + ≥5s clamp + short-turn suppression; default off on non-TTY. |
| Spinner timer arm idle during text streaming → no heartbeat then | Low | Low | By design (text streaming is its own liveness); matches team-loop behavior; documented. |
| env-var test cross-contamination | Med | Low | inject `now`/`is_tty` as params; set/remove env within each test; avoid global `Instant::now()` in pure fns. |
