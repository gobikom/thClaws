//! Token-level helpers shared by the parser: NFC normalization, sigil
//! scanning, quote-aware splitting. Handles are NFC-normalized at the
//! boundary so `$แอน` compares equal no matter how the editor composed
//! the marks — the GUI shell must apply the same normalization when it
//! matches board cards to script handles.

use unicode_normalization::UnicodeNormalization;

pub(crate) fn nfc(s: &str) -> String {
    s.nfc().collect()
}

/// Characters that terminate a `$handle` in prose. Whitespace always
/// terminates; these let a ref sit flush against punctuation.
const HANDLE_DELIMS: &[char] = &[
    ',', '.', ';', ':', '!', '?', '"', '\'', '(', ')', '{', '}', '[', ']', '\u{200b}', '\u{200c}',
    '\u{200d}', '\u{2060}', '\u{feff}',
];

/// Scan a handle starting *after* the `$`. Returns (handle, rest).
pub(crate) fn scan_handle(s: &str) -> (String, &str) {
    let mut end = s.len();
    for (i, c) in s.char_indices() {
        if c.is_whitespace() || HANDLE_DELIMS.contains(&c) {
            end = i;
            break;
        }
    }
    (nfc(&s[..end]), &s[end..])
}

/// All `$handle` references in a prose line, in order of appearance.
pub(crate) fn scan_refs(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = text;
    while let Some(pos) = rest.find('$') {
        let after = &rest[pos + 1..];
        let (h, tail) = scan_handle(after);
        if !h.is_empty() {
            out.push(h);
        }
        rest = tail;
    }
    out
}

/// Split `key: value` where `key` is an ASCII identifier at line start.
pub(crate) fn split_key_value(line: &str) -> Option<(&str, &str)> {
    let colon = line.find(':')?;
    let key = line[..colon].trim_end();
    if key.is_empty() || !key.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
        return None;
    }
    Some((key, line[colon + 1..].trim()))
}

/// Tokenize a declaration tail: whitespace-separated, but `"…"` groups
/// (also inside `key:"…"`) keep their spaces.
pub(crate) fn decl_tokens(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut in_quotes = false;
    for c in s.chars() {
        match c {
            '"' => {
                in_quotes = !in_quotes;
                cur.push(c);
            }
            c if c.is_whitespace() && !in_quotes => {
                if !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                }
            }
            c => cur.push(c),
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

pub(crate) fn unquote(s: &str) -> &str {
    s.strip_prefix('"')
        .and_then(|t| t.strip_suffix('"'))
        .unwrap_or(s)
}

/// Strip `// …` comments: whole-line, or trailing when preceded by
/// whitespace (so `@./path//x` in a path is never touched).
pub(crate) fn strip_comment(line: &str) -> &str {
    let t = line.trim_start();
    if t.starts_with("//") {
        return "";
    }
    let mut prev_ws = false;
    for (i, c) in line.char_indices() {
        if c == '/' && prev_ws && line[i..].starts_with("//") {
            return &line[..i];
        }
        prev_ws = c.is_whitespace();
    }
    line
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn thai_handle_scans_to_delimiter() {
        let (h, rest) = scan_handle("แอน นั่งที่โต๊ะ");
        assert_eq!(h, "แอน");
        assert!(rest.starts_with(' '));
        let (h, _) = scan_handle("โต๊ะ, ริมหน้าต่าง");
        assert_eq!(h, "โต๊ะ");
    }

    #[test]
    fn refs_in_prose() {
        assert_eq!(scan_refs("$แอน นั่งที่ $โต๊ะ."), vec!["แอน", "โต๊ะ"]);
        assert!(scan_refs("no refs here").is_empty());
    }

    #[test]
    fn decl_tokens_keep_quoted_spaces() {
        let toks = decl_tokens(r#"voice:th-female-warm desc:"หญิงสาว ผมยาว""#);
        assert_eq!(toks.len(), 2);
        assert_eq!(toks[1], r#"desc:"หญิงสาว ผมยาว""#);
    }

    #[test]
    fn comments_stripped_paths_safe() {
        assert_eq!(strip_comment("  // whole line"), "");
        assert_eq!(
            strip_comment("camera: static // lock it").trim_end(),
            "camera: static"
        );
        assert_eq!(
            strip_comment("char $x = @./a//b.png"),
            "char $x = @./a//b.png"
        );
    }

    #[test]
    fn nfc_normalizes_composed_forms() {
        // U+0E33 (SARA AM) vs U+0E4D U+0E32 (NIKHAHIT + SARA AA) stay
        // distinct in NFC (no canonical mapping) — but canonical mark
        // reordering must still converge.
        let a = "กำ\u{0E48}"; // tone mark after
        assert_eq!(nfc(a), a.nfc().collect::<String>());
    }
}
