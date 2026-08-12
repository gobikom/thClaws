//! Pre-parse repair for malformed GFM pipe tables.
//!
//! LLM-authored markdown routinely miscounts a table's **delimiter row** (the
//! `|---|:--:|` line under the header). GFM requires the delimiter row to have
//! the *same* number of cells as the header; when it doesn't, pulldown-cmark
//! (used by every doc renderer — PDF/DOCX/EPUB) rejects the whole block as a
//! table and re-emits it as a paragraph, so the pipes leak out as literal text
//! and the single newlines between rows collapse to spaces. Padding (or
//! truncating) the delimiter to the header's column count restores the table.
//!
//! This only rewrites delimiter rows whose cell count disagrees with the row
//! directly above them; well-formed tables, horizontal rules (`---`, no pipe),
//! and anything inside a fenced code block are left untouched.

use std::borrow::Cow;

/// GFM cell count for a table row, tolerant of the optional leading/trailing
/// pipes (`| a | b |` and `a | b` both count as 2).
fn cell_count(line: &str) -> usize {
    let t = line.trim();
    let t = t.strip_prefix('|').unwrap_or(t);
    let t = t.strip_suffix('|').unwrap_or(t);
    t.split('|').count()
}

/// A delimiter cell is `:?-+:?` — optional leading/trailing colon around one or
/// more dashes (`---`, `:--`, `--:`, `:-:`).
fn is_delim_cell(c: &str) -> bool {
    let b = c.trim().as_bytes();
    if b.is_empty() {
        return false;
    }
    let mut i = 0;
    if b[i] == b':' {
        i += 1;
    }
    let dash_start = i;
    while i < b.len() && b[i] == b'-' {
        i += 1;
    }
    if i == dash_start {
        return false; // needs at least one dash
    }
    if i < b.len() && b[i] == b':' {
        i += 1;
    }
    i == b.len()
}

/// If `line` is a GFM delimiter row, return its cells (trimmed). Requires a pipe
/// so a bare `---`/`***` horizontal rule is excluded.
fn delim_cells(line: &str) -> Option<Vec<String>> {
    let t = line.trim();
    if !t.contains('|') || !t.contains('-') {
        return None;
    }
    let inner = {
        let s = t.strip_prefix('|').unwrap_or(t);
        s.strip_suffix('|').unwrap_or(s)
    };
    let cells: Vec<&str> = inner.split('|').collect();
    if !cells.is_empty() && cells.iter().all(|c| is_delim_cell(c)) {
        Some(cells.iter().map(|c| c.trim().to_string()).collect())
    } else {
        None
    }
}

/// Repair delimiter rows whose column count doesn't match the header above them,
/// so pulldown-cmark accepts the table. Returns the input unchanged (borrowed)
/// when there's nothing to fix.
pub fn repair_table_delimiters(md: &str) -> Cow<'_, str> {
    // No pipe → no pipe table anywhere.
    if !md.contains('|') {
        return Cow::Borrowed(md);
    }

    let lines: Vec<&str> = md.split('\n').collect();
    let mut out: Vec<Cow<str>> = Vec::with_capacity(lines.len());
    let mut in_fence = false;
    let mut fence_marker = "";
    let mut changed = false;

    for (i, &line) in lines.iter().enumerate() {
        let trimmed = line.trim_start();

        // Never touch fenced code blocks — a `|---|` line there is code.
        if !in_fence && (trimmed.starts_with("```") || trimmed.starts_with("~~~")) {
            in_fence = true;
            fence_marker = if trimmed.starts_with("```") {
                "```"
            } else {
                "~~~"
            };
            out.push(Cow::Borrowed(line));
            continue;
        } else if in_fence {
            if trimmed.starts_with(fence_marker) {
                in_fence = false;
            }
            out.push(Cow::Borrowed(line));
            continue;
        }

        if i > 0 {
            if let Some(cells) = delim_cells(line) {
                let header = lines[i - 1];
                let hcount = cell_count(header);
                // The line above must itself read as a multi-column table row,
                // and the counts must actually disagree, before we rewrite.
                if header.contains('|')
                    && !header.trim().is_empty()
                    && hcount >= 2
                    && hcount != cells.len()
                {
                    let indent = &line[..line.len() - trimmed.len()];
                    let mut cells = cells;
                    if cells.len() > hcount {
                        cells.truncate(hcount);
                    } else {
                        while cells.len() < hcount {
                            cells.push("---".to_string());
                        }
                    }
                    out.push(Cow::Owned(format!("{indent}| {} |", cells.join(" | "))));
                    changed = true;
                    continue;
                }
            }
        }
        out.push(Cow::Borrowed(line));
    }

    if changed {
        Cow::Owned(out.join("\n"))
    } else {
        Cow::Borrowed(md)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pads_short_delimiter_to_header_width() {
        // 4-column header, 3-cell delimiter (the real book-export bug).
        let md = "| a | b | c | d |\n|---|---:|---:|\n| 1 | 2 | 3 | 4 |\n";
        let out = repair_table_delimiters(md);
        let delim = out.lines().nth(1).unwrap();
        assert_eq!(
            cell_count(delim),
            4,
            "delimiter should be padded to 4: {delim}"
        );
        // Existing alignment kept, padding is default-left `---`.
        assert!(delim.contains("---:"), "kept alignment: {delim}");
    }

    #[test]
    fn truncates_over_wide_delimiter() {
        let md = "| a | b |\n|---|---|---|---|\n| 1 | 2 |\n";
        let out = repair_table_delimiters(md);
        assert_eq!(cell_count(out.lines().nth(1).unwrap()), 2);
    }

    #[test]
    fn leaves_well_formed_table_untouched() {
        let md = "| a | b |\n|---|---|\n| 1 | 2 |\n";
        assert!(matches!(repair_table_delimiters(md), Cow::Borrowed(_)));
    }

    #[test]
    fn ignores_horizontal_rule() {
        // `---` with no pipe is a thematic break, not a delimiter row.
        let md = "para\n\n---\n\nmore\n";
        assert!(matches!(repair_table_delimiters(md), Cow::Borrowed(_)));
    }

    #[test]
    fn ignores_delimiter_like_line_in_code_fence() {
        let md = "```\n| a | b | c |\n|---|---|\n```\n";
        assert!(
            matches!(repair_table_delimiters(md), Cow::Borrowed(_)),
            "must not rewrite inside a code fence"
        );
    }

    #[test]
    fn repaired_table_is_recognized_by_pulldown_but_raw_is_not() {
        use pulldown_cmark::{Event, Options, Parser, Tag};
        // The exact book-export shape: 4-col header, 3-cell delimiter.
        let md =
            "para\n\n| รุ่น | ปี | bw | note |\n|---|---:|---:|\n| NVLink 1.0 | 2016 | 160 | x |\n";
        let mut opts = Options::empty();
        opts.insert(Options::ENABLE_TABLES);
        let has_table =
            |s: &str| Parser::new_ext(s, opts).any(|e| matches!(e, Event::Start(Tag::Table(_))));
        assert!(
            !has_table(md),
            "raw malformed table must NOT be a table (the bug)"
        );
        let repaired = repair_table_delimiters(md);
        assert!(
            has_table(&repaired),
            "repaired table must be recognized (the fix)"
        );
    }

    #[test]
    fn preserves_indentation() {
        let md = "  | a | b | c |\n  |---|---|\n  | 1 | 2 | 3 |\n";
        let out = repair_table_delimiters(md);
        let delim = out.lines().nth(1).unwrap();
        assert!(delim.starts_with("  | "), "indent kept: {delim:?}");
        assert_eq!(cell_count(delim), 3);
    }
}
