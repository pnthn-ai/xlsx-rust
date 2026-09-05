//! Excel `SEARCH` kernel.
//!
//! Semantics (desktop Excel):
//! - `SEARCH(find_text, within_text, [start_num])` — case-insensitive.
//! - Wildcards in `find_text`: `*` (any sequence), `?` (one character),
//!   `~` escapes the next character (`~*`, `~?`, `~~`).
//! - 1-based character index (Unicode scalars, matching this crate's `LEN`/`MID`).
//! - Missing needle → `#VALUE!` (not `#N/A`).
//! - Empty `find_text` matches at `start_num` when `start_num <= LEN(within_text)`.
//! - `start_num` is 1-based; omitted means 1. `< 1` or `> LEN(within_text)` is
//!   `#VALUE!`. This is stricter than `FIND`, which allows empty `find_text`
//!   one past `LEN`.
//! - A leading `*` still reports `start_num` (`SEARCH("*z","XYZ")` is 1),
//!   because `*` can match the empty prefix. `*` cannot test "ends with".
//!
//! Production path: ASCII case-insensitive last-byte SWAR probe for literal
//! needles, leading-`*` shortcut, first-literal skip for general wildcards.
//! The `Vec<char>` try-every-index baseline lives beside that path so benches
//! can print before/after.

use xlsx_types::ExcelError;

/// Production `SEARCH` kernel.
///
/// `start_num` is already truncated toward zero (1-based). Returns the
/// 1-based character position, or [`ExcelError::Value`].
pub fn search(find_text: &str, within_text: &str, start_num: i64) -> Result<f64, ExcelError> {
    search_impl(find_text, within_text, start_num, SearchMode::Fast)
}

/// Quadratic / try-every-index baseline used for the hill-climb bench.
///
/// Same Excel semantics as [`search`]; slower on large haystacks. Kept so
/// `cargo bench -p xlsx-engine-core --bench search` can print before/after.
pub fn search_naive(find_text: &str, within_text: &str, start_num: i64) -> Result<f64, ExcelError> {
    search_impl(find_text, within_text, start_num, SearchMode::Naive)
}

#[derive(Clone, Copy)]
enum SearchMode {
    Fast,
    Naive,
}

fn search_impl(
    find_text: &str,
    within_text: &str,
    start_num: i64,
    mode: SearchMode,
) -> Result<f64, ExcelError> {
    if start_num < 1 {
        return Err(ExcelError::Value);
    }
    // Characters ≤ bytes. A start past byte-len is past char-len.
    if start_num as u64 > within_text.len() as u64 {
        return Err(ExcelError::Value);
    }
    match mode {
        SearchMode::Naive => search_chars(find_text, within_text, start_num),
        SearchMode::Fast => search_fast(find_text, within_text, start_num),
    }
}

fn search_chars(find_text: &str, within_text: &str, start_num: i64) -> Result<f64, ExcelError> {
    let hay: Vec<char> = within_text.chars().collect();
    let start = (start_num as usize) - 1;
    if start >= hay.len() {
        return Err(ExcelError::Value);
    }
    if find_text.is_empty() {
        return Ok(start_num as f64);
    }
    let pat: Vec<char> = find_text.chars().collect();
    for i in start..hay.len() {
        if match_here_chars(&pat, &hay[i..]) {
            return Ok((i + 1) as f64);
        }
    }
    Err(ExcelError::Value)
}

fn match_here_chars(pat: &[char], hay: &[char]) -> bool {
    fn rec(p: &[char], t: &[char]) -> bool {
        if p.is_empty() {
            return true;
        }
        if p[0] == '~' {
            if p.len() >= 2 {
                return !t.is_empty() && ci_eq(p[1], t[0]) && rec(&p[2..], &t[1..]);
            }
            return !t.is_empty() && ci_eq('~', t[0]) && rec(&p[1..], &t[1..]);
        }
        if p[0] == '*' {
            let mut rest = p;
            while rest.first() == Some(&'*') {
                rest = &rest[1..];
            }
            let mut cur = t;
            loop {
                if rec(rest, cur) {
                    return true;
                }
                if cur.is_empty() {
                    return false;
                }
                cur = &cur[1..];
            }
        }
        if p[0] == '?' {
            return !t.is_empty() && rec(&p[1..], &t[1..]);
        }
        !t.is_empty() && ci_eq(p[0], t[0]) && rec(&p[1..], &t[1..])
    }
    rec(pat, hay)
}

fn search_fast(find_text: &str, within_text: &str, start_num: i64) -> Result<f64, ExcelError> {
    let skip = (start_num as usize) - 1;
    let Some(suffix) = skip_chars(within_text, skip) else {
        return Err(ExcelError::Value);
    };
    // `start_num > LEN` (including empty within_text): leftover is empty.
    if suffix.is_empty() {
        return Err(ExcelError::Value);
    }
    if find_text.is_empty() {
        return Ok(start_num as f64);
    }
    let toks = parse_pat(find_text);
    if toks.iter().all(|t| matches!(t, Tok::Star)) {
        return Ok(start_num as f64);
    }
    // Leading `*` matches the empty prefix, so a hit always reports start_num.
    if matches!(toks.first(), Some(Tok::Star)) {
        return if first_match(&toks[1..], suffix).is_some() {
            Ok(start_num as f64)
        } else {
            Err(ExcelError::Value)
        };
    }
    match first_match(&toks, suffix) {
        Some(rel) => Ok((start_num as usize + rel) as f64),
        None => Err(ExcelError::Value),
    }
}

#[derive(Clone, Debug)]
enum Tok {
    Lit(String),
    Any,
    Star,
}

fn parse_pat(s: &str) -> Vec<Tok> {
    let mut out = Vec::new();
    let mut lit = String::new();
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '~' {
            if let Some(n) = chars.next() {
                lit.push(n);
            } else {
                lit.push('~');
            }
        } else if c == '*' {
            flush_lit(&mut out, &mut lit);
            if !matches!(out.last(), Some(Tok::Star)) {
                out.push(Tok::Star);
            }
        } else if c == '?' {
            flush_lit(&mut out, &mut lit);
            out.push(Tok::Any);
        } else {
            lit.push(c);
        }
    }
    flush_lit(&mut out, &mut lit);
    out
}

fn flush_lit(out: &mut Vec<Tok>, lit: &mut String) {
    if !lit.is_empty() {
        out.push(Tok::Lit(std::mem::take(lit)));
    }
}

fn first_match(toks: &[Tok], hay: &str) -> Option<usize> {
    if toks.is_empty() {
        return Some(0);
    }
    if let [Tok::Lit(s)] = toks {
        return ci_find(hay, s);
    }
    match &toks[0] {
        Tok::Lit(s) => {
            let mut pos = 0usize;
            let mut rest = hay;
            loop {
                let rel = ci_find(rest, s)?;
                if match_here(toks, skip_n_chars(rest, rel).unwrap_or("")) {
                    return Some(pos + rel);
                }
                let adv = rel + 1;
                rest = skip_n_chars(rest, adv)?;
                pos += adv;
            }
        }
        Tok::Any => {
            let mut pos = 0usize;
            let mut rest = hay;
            while !rest.is_empty() {
                if match_here(toks, rest) {
                    return Some(pos);
                }
                rest = skip_n_chars(rest, 1)?;
                pos += 1;
            }
            None
        }
        Tok::Star => first_match(&toks[1..], hay).map(|_| 0),
    }
}

fn match_here(toks: &[Tok], hay: &str) -> bool {
    if toks.is_empty() {
        return true;
    }
    match &toks[0] {
        Tok::Star => {
            if toks.len() == 1 {
                return true;
            }
            let rest = &toks[1..];
            let mut cur = hay;
            loop {
                if match_here(rest, cur) {
                    return true;
                }
                if cur.is_empty() {
                    return false;
                }
                cur = skip_n_chars(cur, 1).unwrap_or("");
            }
        }
        Tok::Any => match skip_n_chars(hay, 1) {
            Some(rest) if !hay.is_empty() => match_here(&toks[1..], rest),
            _ => false,
        },
        Tok::Lit(s) => {
            if !ci_starts_with(hay, s) {
                return false;
            }
            let n = s.chars().count();
            match skip_n_chars(hay, n) {
                Some(rest) => match_here(&toks[1..], rest),
                None => false,
            }
        }
    }
}

fn ci_find(hay: &str, needle: &str) -> Option<usize> {
    if needle.is_empty() {
        return Some(0);
    }
    if hay.is_ascii() && needle.is_ascii() {
        return ci_find_ascii(hay.as_bytes(), needle.as_bytes());
    }
    ci_find_unicode(hay, needle)
}

fn ci_find_ascii(hay: &[u8], needle: &[u8]) -> Option<usize> {
    if hay.len() < needle.len() {
        return None;
    }
    if needle.len() == 1 {
        return memchr_ci(hay, needle[0]);
    }
    find_ascii_last_byte_ci(hay, needle)
}

fn find_ascii_last_byte_ci(hay: &[u8], needle: &[u8]) -> Option<usize> {
    debug_assert!(needle.len() >= 2);
    let nlen = needle.len();
    if hay.len() < nlen {
        return None;
    }
    let last = needle[nlen - 1];
    let mut i = nlen - 1;
    while i < hay.len() {
        let Some(rel) = memchr_ci(&hay[i..], last) else {
            return None;
        };
        let end = i + rel;
        let start = end + 1 - nlen;
        if ci_bytes_eq(&hay[start..=end], needle) {
            return Some(start);
        }
        i = end + 1;
    }
    None
}

fn ci_bytes_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.iter()
        .zip(b.iter())
        .all(|(x, y)| x.eq_ignore_ascii_case(y))
}

fn ci_find_unicode(hay: &str, needle: &str) -> Option<usize> {
    let mut nchars = needle.chars();
    if let (Some(n), None) = (nchars.next(), nchars.next()) {
        return find_unicode_char_ci(hay, n);
    }
    let mut pos = 0usize;
    let mut rest = hay;
    while !rest.is_empty() {
        if ci_starts_with(rest, needle) {
            return Some(pos);
        }
        rest = skip_n_chars(rest, 1)?;
        pos += 1;
    }
    None
}

/// Case-insensitive single-char search. Probes the first UTF-8 byte of each
/// case-folded encoding so an ASCII-heavy haystack does not walk every scalar.
fn find_unicode_char_ci(hay: &str, n: char) -> Option<usize> {
    let mut encs = Vec::new();
    push_char_utf8(&mut encs, n);
    for c in n.to_lowercase() {
        push_char_utf8(&mut encs, c);
    }
    for c in n.to_uppercase() {
        push_char_utf8(&mut encs, c);
    }
    encs.sort_unstable();
    encs.dedup();
    if encs.is_empty() {
        return None;
    }
    let mut firsts: Vec<u8> = encs.iter().map(|e| e[0]).collect();
    firsts.sort_unstable();
    firsts.dedup();
    let bytes = hay.as_bytes();
    let mut i = 0usize;
    while i < bytes.len() {
        let rel = if firsts.len() == 1 {
            memchr_byte(&bytes[i..], firsts[0])
        } else if firsts.len() == 2 {
            memchr2_byte(&bytes[i..], firsts[0], firsts[1])
        } else {
            bytes[i..].iter().position(|b| firsts.contains(b))
        };
        let Some(rel) = rel else {
            return None;
        };
        let pos = i + rel;
        for enc in &encs {
            if bytes[pos..].starts_with(enc.as_slice()) {
                let prefix = &hay[..pos];
                let idx = if prefix.is_ascii() {
                    pos
                } else {
                    prefix.chars().count()
                };
                return Some(idx);
            }
        }
        i = pos + 1;
    }
    None
}

fn push_char_utf8(out: &mut Vec<Vec<u8>>, c: char) {
    let mut buf = [0u8; 4];
    out.push(c.encode_utf8(&mut buf).as_bytes().to_vec());
}

fn ci_starts_with(hay: &str, needle: &str) -> bool {
    if hay.is_ascii() && needle.is_ascii() {
        return hay.len() >= needle.len()
            && hay.as_bytes()[..needle.len()].eq_ignore_ascii_case(needle.as_bytes());
    }
    let mut h = hay.chars();
    for n in needle.chars() {
        match h.next() {
            Some(c) if ci_eq(c, n) => {}
            _ => return false,
        }
    }
    true
}

fn ci_eq(a: char, b: char) -> bool {
    if a == b {
        return true;
    }
    if a.is_ascii() && b.is_ascii() {
        return a.eq_ignore_ascii_case(&b);
    }
    // ASCII never case-folds onto a non-ASCII letter (`e` ≠ `é`).
    if a.is_ascii() || b.is_ascii() {
        return false;
    }
    a.to_lowercase().eq(b.to_lowercase())
}

/// Word-at-a-time `memchr` for `needle` and its ASCII case twin.
fn memchr_ci(hay: &[u8], needle: u8) -> Option<usize> {
    if needle.is_ascii_alphabetic() {
        let lo = needle.to_ascii_lowercase();
        let up = needle.to_ascii_uppercase();
        if lo != up {
            return memchr2_byte(hay, lo, up);
        }
    }
    memchr_byte(hay, needle)
}

fn memchr_byte(hay: &[u8], needle: u8) -> Option<usize> {
    const W: usize = std::mem::size_of::<usize>();
    let splat = usize::from(needle).wrapping_mul(usize::from_ne_bytes([0x01; W]));
    let ones = usize::from_ne_bytes([0x01; W]);
    let highs = usize::from_ne_bytes([0x80; W]);
    let mut i = 0;
    while i + W <= hay.len() {
        // SAFETY: `i + W <= hay.len()`, and we only read `W` bytes.
        let word = unsafe { std::ptr::read_unaligned(hay.as_ptr().add(i).cast::<usize>()) };
        let xor = word ^ splat;
        let mask = xor.wrapping_sub(ones) & !xor & highs;
        if mask != 0 {
            for j in 0..W {
                if hay[i + j] == needle {
                    return Some(i + j);
                }
            }
        }
        i += W;
    }
    hay[i..].iter().position(|&b| b == needle).map(|p| i + p)
}

fn memchr2_byte(hay: &[u8], a: u8, b: u8) -> Option<usize> {
    const W: usize = std::mem::size_of::<usize>();
    let splat_a = usize::from(a).wrapping_mul(usize::from_ne_bytes([0x01; W]));
    let splat_b = usize::from(b).wrapping_mul(usize::from_ne_bytes([0x01; W]));
    let ones = usize::from_ne_bytes([0x01; W]);
    let highs = usize::from_ne_bytes([0x80; W]);
    let mut i = 0;
    while i + W <= hay.len() {
        // SAFETY: `i + W <= hay.len()`, and we only read `W` bytes.
        let word = unsafe { std::ptr::read_unaligned(hay.as_ptr().add(i).cast::<usize>()) };
        let xor_a = word ^ splat_a;
        let xor_b = word ^ splat_b;
        let mask_a = xor_a.wrapping_sub(ones) & !xor_a & highs;
        let mask_b = xor_b.wrapping_sub(ones) & !xor_b & highs;
        if mask_a != 0 || mask_b != 0 {
            for j in 0..W {
                if hay[i + j] == a || hay[i + j] == b {
                    return Some(i + j);
                }
            }
        }
        i += W;
    }
    hay[i..]
        .iter()
        .position(|&c| c == a || c == b)
        .map(|p| i + p)
}

fn skip_chars(s: &str, n: usize) -> Option<&str> {
    skip_n_chars(s, n)
}

fn skip_n_chars(s: &str, n: usize) -> Option<&str> {
    if s.is_ascii() {
        if n > s.len() {
            None
        } else {
            Some(&s[n..])
        }
    } else {
        let mut iter = s.chars();
        for _ in 0..n {
            iter.next()?;
        }
        Some(iter.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn both(needle: &str, hay: &str, start: i64) -> Result<f64, ExcelError> {
        let fast = search(needle, hay, start);
        let slow = search_naive(needle, hay, start);
        assert_eq!(
            fast, slow,
            "naive/fast mismatch for {needle:?} in {hay:?} start={start}"
        );
        fast
    }

    #[test]
    fn microsoft_examples() {
        assert_eq!(both("n", "printer", 1), Ok(4.0));
        assert_eq!(both("base", "database", 1), Ok(5.0));
        assert_eq!(both("e", "Statements", 6), Ok(7.0));
        assert_eq!(both("margin", "Profit Margin", 1), Ok(8.0));
        assert_eq!(both("Y", "AYF0093.YoungMensApparel", 8), Ok(9.0));
    }

    #[test]
    fn case_insensitive_unlike_find() {
        assert_eq!(both("a", "ABC", 1), Ok(1.0));
        assert_eq!(both("bc", "ABC", 1), Ok(2.0));
        assert_eq!(both("m", "Miriam McGovern", 1), Ok(1.0));
        assert_eq!(both("MARGIN", "Profit Margin", 1), Ok(8.0));
        assert_eq!(both("z", "abc", 1), Err(ExcelError::Value));
    }

    #[test]
    fn start_num_and_len() {
        assert_eq!(both("a", "banana", 3), Ok(4.0));
        assert_eq!(both("a", "banana", 6), Ok(6.0));
        assert_eq!(both("a", "banana", 7), Err(ExcelError::Value));
        assert_eq!(both("c", "abc", 3), Ok(3.0));
        assert_eq!(both("c", "abc", 4), Err(ExcelError::Value));
        assert_eq!(both("a", "abc", 0), Err(ExcelError::Value));
        assert_eq!(both("a", "abc", -1), Err(ExcelError::Value));
    }

    #[test]
    fn empty_find_text_stricter_than_find() {
        assert_eq!(both("", "abc", 1), Ok(1.0));
        assert_eq!(both("", "abc", 3), Ok(3.0));
        // FIND allows LEN+1 for empty find_text; SEARCH does not.
        assert_eq!(both("", "abc", 4), Err(ExcelError::Value));
        assert_eq!(both("", "", 1), Err(ExcelError::Value));
        assert_eq!(both("a", "", 1), Err(ExcelError::Value));
    }

    #[test]
    fn wildcards() {
        assert_eq!(both("*", "abc", 1), Ok(1.0));
        assert_eq!(both("a*", "abc", 1), Ok(1.0));
        assert_eq!(both("*c", "abc", 1), Ok(1.0));
        assert_eq!(both("*z", "XYZ", 1), Ok(1.0));
        assert_eq!(both("*z", "XYZXY", 1), Ok(1.0));
        assert_eq!(both("?c", "abc", 1), Ok(2.0));
        assert_eq!(both("a?", "abc", 1), Ok(1.0));
        assert_eq!(both("???", "abc", 1), Ok(1.0));
        assert_eq!(both("????", "abc", 1), Err(ExcelError::Value));
        assert_eq!(both("a*b", "ab", 1), Ok(1.0));
        assert_eq!(both("x*b", "AAAXYZ123ABCZZZ", 1), Ok(4.0));
        assert_eq!(both("*", "abc", 2), Ok(2.0));
        assert_eq!(both("*c", "abc", 3), Ok(3.0));
        assert_eq!(both("*x", "abc", 2), Err(ExcelError::Value));
    }

    #[test]
    fn tilde_escape() {
        assert_eq!(both("~*", "a*b", 1), Ok(2.0));
        assert_eq!(both("~?", "a?b", 1), Ok(2.0));
        assert_eq!(both("~~", "a~b", 1), Ok(2.0));
        assert_eq!(both("~*", "apple*", 1), Ok(6.0));
        assert_eq!(both("~?", "apple?", 1), Ok(6.0));
        assert_eq!(both("~~", "apple~", 1), Ok(6.0));
        assert_eq!(both("~", "a~b", 1), Ok(2.0));
        assert_eq!(both("~x", "ax", 1), Ok(2.0));
        assert_eq!(both("a~*", "a*b", 1), Ok(1.0));
        assert_eq!(both("*", "a*b", 1), Ok(1.0));
    }

    #[test]
    fn unicode_scalar_index_and_casefold() {
        assert_eq!(both("é", "café", 1), Ok(4.0));
        assert_eq!(both("É", "café", 1), Ok(4.0));
        assert_eq!(both("日", "日本語", 1), Ok(1.0));
        assert_eq!(both("語", "日本語", 1), Ok(3.0));
    }

    #[test]
    fn huge_start_rejects_without_scan() {
        assert_eq!(both("a", "abc", i64::MAX), Err(ExcelError::Value));
    }

    #[test]
    fn almost_match_suffix_casefold() {
        let hay = format!("{}aab", "aaa".repeat(80));
        assert_eq!(both("AAB", &hay, 1), Ok((hay.len() - 2) as f64));
        assert_eq!(both("AAC", &hay, 1), Err(ExcelError::Value));
    }
}
