# PR #17 — Multi-Agent Review: session-hud-v6 heartbeat (#335)

**Repo**: gobikom/thClaws · **Branch**: fix/session-hud-v6-335 · **Base**: main
**Reviewed**: 2026-05-29 · **Reviewers**: code-reviewer, security-reviewer, silent-failure-hunter, pr-test-analyzer

## Final status (after fix round 1)

**0 critical / 0 high / 0 medium / 0 suggestion**

All findings from the initial review were addressed in a single fix round. Re-validation:
- Unit tests: **22 passed / 0 failed** (`cargo test -p thclaws-core --lib session_hud`)
- PTY tests: **2 passed / 0 failed** (`cargo test -p thclaws-core --test session_hud_pty`)
- clippy: **no warnings from changed files** (`cargo clippy -p thclaws-core --all-targets`)
- fmt: clean (`cargo fmt --check`)

## Round 1 findings → resolution

| # | Reviewer | Sev | Finding | Resolution |
|---|----------|-----|---------|-----------|
| 1 | code-reviewer | high (85) | Idle-path heartbeat suppression: when no spinner is active the loop arms a 300s idle sleep, so the heartbeat could lag its interval by up to 5 min during long streamed-text stretches. | Added `HudState::poll_delay(now)`; `repl.rs` idle branch now caps the sleep to the next-heartbeat time when the HUD is on. Unit test `poll_delay_*` added. |
| 2 | code-reviewer / security / test | med (80) | PTY temp filename used `pid` only → collision risk if a 2nd test runs in the same process. | Added an `AtomicU64` counter suffix to the temp filename. |
| 3 | pr-test-analyzer | med (6) | AC4 (short turn → no heartbeat) not asserted at integration level. | Added PTY test `short_turn_emits_no_heartbeat_line`. |
| 4 | pr-test-analyzer | low (5) | Redaction contract not asserted at the heartbeat formatter boundary. | Added unit test `heartbeat_formatter_does_not_double_redact_or_introduce_secrets`. |
| 5 | pr-test-analyzer | low (5) | Per-turn `HudState` reset not guarded by a test. | Added unit test `fresh_hud_state_per_turn_resets_cadence`. |
| 6 | pr-test-analyzer | low | `raw.contains(&b'\r')` assertion is vacuous (PTY ONLCR maps `\n`→`\r\n`). | Removed the `\r` assertion; kept the meaningful `\x1b[2K` check; comment updated to explain ONLCR. |

## Clean dimensions (round 1)

- **security-reviewer**: clean. Redaction holds (label sanitized upstream via `tool_label`/`redact_secrets` before storage); env parsing is panic/overflow/DoS-safe (interval clamped ≥5s, parse-fail warns + defaults); `eprintln!` passes values as args (no format-string injection). Only a low/test-only insecure-temp note → addressed by #2.
- **silent-failure-hunter**: PASS, 0 issues. Both env-parse failure paths emit explicit `eprintln!` warnings (no silent swallow); `let _ = stdout().flush()` matches 34 pre-existing identical call sites in the same loop; test-cleanup `let _ =` are post-assertion.

## Constraint verification (the v1–v5 failure mode)

Confirmed by both code-reviewer and the PTY test: the heartbeat is a pure `println!`/`String` with **no `\r`/`\x1b[2K`** (locked by unit test `heartbeat_is_a_permanent_line_no_cursor_control`), and **no spinner format fns or callsites were modified** (repl.rs diff is additions-only). The PTY+vt100 test asserts the heartbeat renders as a permanent row above exactly one live spinner row.
