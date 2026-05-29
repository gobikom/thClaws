# PR #18 — Review: spinner width cap (#369)

**Repo**: gobikom/thClaws · **Branch**: fix/spinner-width-wrap · **Base**: main
**Reviewers**: code-reviewer, silent-failure-hunter

## Final status (after fix round 1)
**0 critical / 0 high / 0 medium / 0 suggestion**

- 35 `tool_display` unit tests pass · clippy clean on changed file · fmt clean
- Narrow-pane live verify: 52-col & 56-col → 1 spinner line (was 19 stacked); 140-col unchanged
- `unsafe` ioctl confirmed SOUND by both reviewers

## Round 1 findings → resolution
| # | Reviewer | Sev | Finding | Resolution |
|---|----------|-----|---------|-----------|
| 1 | code-reviewer | high(87) | `chars().count()` ≠ display columns → CJK/wide chars could still wrap | Added `unicode-width` dep; `fit_label_field`/`truncate_to_cols`/thinking-spinner now measure display columns. New tests `spinner_body_never_exceeds_width_wide_chars`, `truncate_to_cols_counts_display_width`. |
| 2 | code-reviewer | med(82) | doc overhead said 5, code uses 6 (margin) | Rewrote doc to state the cols-1 wrap-safety margin explicitly |
| 3 | silent-failure | med | cols<7 → empty label + 2 literal spaces → 7-col line wraps | `spinner_body` drops the label+separator when empty → `  {marker} {dur}`; test floor extended to 7 |
| 4 | silent-failure | med | `term_cols` stdout coupling undocumented | doc note: callers must write to stdout |

## Clean dimensions
- silent-failure: PASS — the ioctl silent fallback to 80 is the correct choice (real narrow PTYs always report size; failure cases are piped/dumb where wrap is invisible). No spam from per-frame query.
- code-reviewer: unsafe block sound (zeroed winsize, valid fd, accurate SAFETY comment); arithmetic correct.
