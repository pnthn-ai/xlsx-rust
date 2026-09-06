//! Excel `TEXTBEFORE` kernel.
//!
//! Semantics (Microsoft 365 / Excel 2024):
//!
//! ```text
//! TEXTBEFORE(text, delimiter, [instance_num], [match_mode], [match_end], [if_not_found])
//! ```
//!
//! - Returns the substring **before** the Nth occurrence of `delimiter`.
//! - `instance_num` defaults to 1. `0` is `#VALUE!`. A negative value counts
//!   from the end. `|instance_num| > LEN(text)` is `#VALUE!`.
//! - `match_mode` 0 (default) is case-sensitive; 1 is case-insensitive.
//!   `*` / `?` are literal (unlike `SEARCH`).
//! - `match_end` 1 treats an unmatched end of the search as one extra
//!   delimiter: the end of `text` when `instance_num > 0` (result is the
//!   whole string) and the start when `instance_num < 0` (result is `""`).
//! - Empty `delimiter` matches immediately: positive `instance_num` → `""`,
//!   negative → the whole `text` (Microsoft remarks).
//! - Empty `text` returns `""` (Microsoft `text` argument remark).
//! - Missing delimiter / too few instances → `#N/A` (caller may substitute
//!   `if_not_found`).
//! - Multiple delimiters: leftmost match; same position prefers the longest.
//! - Occurrences are non-overlapping (advance by the matched delimiter).
//! - Character indexing matches this crate's `LEN` / `MID` (Unicode scalars).
//!
//! [`textbefore`] is the production path (`str::find` / `rfind`, ASCII last-byte
//! SWAR probe, case-insensitive ASCII twin). [`textbefore_naive`] is the
//! `Vec<char>` sliding-window baseline kept so
//! `cargo bench -p xlsx-engine-core --bench textbefore` can print before/after.

use xlsx_types::ExcelError;

/// Production `TEXTBEFORE` kernel.
///
/// `instance_num` is already truncated toward zero. `delimiters` is a
/// flattened list (scalar or array). Returns the prefix, or
/// [`ExcelError::Na`] / [`ExcelError::Value`].
pub fn textbefore(
    text: &str,
    delimiters: &[&str],
    instance_num: i64,
    ignore_case: bool,
    match_end: bool,
) -> Result<String, ExcelError> {
    textbefore_impl(
        text,
        delimiters,
        instance_num,
        ignore_case,
        match_end,
        Mode::Fast,
    )
}

/// Quadratic `Vec<char>` baseline used for the hill-climb bench.
///
/// Same Excel semantics as [`textbefore`].
pub fn textbefore_naive(
    text: &str,
    delimiters: &[&str],
    instance_num: i64,
    ignore_case: bool,
    match_end: bool,
) -> Result<String, ExcelError> {
    textbefore_impl(
        text,
        delimiters,
        instance_num,
        ignore_case,
        match_end,
        Mode::Naive,
    )
}

#[derive(Clone, Copy)]
enum Mode {
    Fast,
    Naive,
}

fn textbefore_impl(
    text: &str,
    delimiters: &[&str],
    instance_num: i64,
    ignore_case: bool,
    match_end: bool,
    mode: Mode,
) -> Result<String, ExcelError> {
    if instance_num == 0 {
        return Err(ExcelError::Value);
    }
    // Microsoft: empty text returns empty text.
    if text.is_empty() {
        return Ok(String::new());
    }
    let char_len = if text.is_ascii() {
        text.len()
    } else {
        text.chars().count()
    };
    let want = instance_num.unsigned_abs();
    if want as u64 > char_len as u64 {
        return Err(ExcelError::Value);
    }

    if delimiters.is_empty() {
        return not_found(text, instance_num, match_end);
    }
    if delimiters.iter().any(|d| d.is_empty()) {
        // Empty delimiter matches immediately (Microsoft remarks).
        return if instance_num > 0 {
            Ok(String::new())
        } else {
            Ok(text.to_string())
        };
    }

    match mode {
        Mode::Naive => apply_chars(text, delimiters, instance_num, ignore_case, match_end),
        Mode::Fast => apply_fast(text, delimiters, instance_num, ignore_case, match_end),
    }
}

fn not_found(text: &str, instance_num: i64, match_end: bool) -> Result<String, ExcelError> {
    if !match_end {
        return Err(ExcelError::Na);
    }
    // Virtual delimiter: end-of-text (forward) or start-of-text (backward).
    // Only the first missing instance is filled.
    if instance_num == 1 {
        Ok(text.to_string())
    } else if instance_num == -1 {
        Ok(String::new())
    } else {
        Err(ExcelError::Na)
    }
}

fn apply_chars(
    text: &str,
    delimiters: &[&str],
    instance_num: i64,
    ignore_case: bool,
    match_end: bool,
) -> Result<String, ExcelError> {
    let hay: Vec<char> = text.chars().collect();
    let needles: Vec<Vec<char>> = delimiters.iter().map(|d| d.chars().collect()).collect();
    let mut hits: Vec<usize> = Vec::new();
    let mut i = 0usize;
    while i < hay.len() {
        if let Some(nlen) = match_at_chars(&hay[i..], &needles, ignore_case) {
            hits.push(i);
            i += nlen.max(1);
        } else {
            i += 1;
        }
    }
    pick_hit(&hay, &hits, instance_num, match_end, text)
}

fn match_at_chars(hay: &[char], needles: &[Vec<char>], ignore_case: bool) -> Option<usize> {
    let mut best = 0usize;
    for n in needles {
        if n.is_empty() || n.len() > hay.len() {
            continue;
        }
        let ok = if ignore_case {
            n.iter().zip(hay.iter()).all(|(a, b)| ci_eq(*a, *b))
        } else {
            hay[..n.len()] == n[..]
        };
        if ok && n.len() > best {
            best = n.len();
        }
    }
    (best > 0).then_some(best)
}

fn pick_hit(
    hay: &[char],
    hits: &[usize],
    instance_num: i64,
    match_end: bool,
    text: &str,
) -> Result<String, ExcelError> {
    let n = instance_num.unsigned_abs() as usize;
    let pos = if instance_num > 0 {
        if n <= hits.len() {
            Some(hits[n - 1])
        } else if match_end && n == hits.len() + 1 {
            return Ok(text.to_string());
        } else {
            None
        }
    } else if n <= hits.len() {
        Some(hits[hits.len() - n])
    } else if match_end && n == hits.len() + 1 {
        return Ok(String::new());
    } else {
        None
    };
    match pos {
        Some(i) => Ok(hay[..i].iter().collect()),
        None => Err(ExcelError::Na),
    }
}

fn apply_fast(
    text: &str,
    delimiters: &[&str],
    instance_num: i64,
    ignore_case: bool,
    match_end: bool,
) -> Result<String, ExcelError> {
    let want = instance_num.unsigned_abs() as usize;
    if instance_num > 0 {
        let mut start = 0usize;
        let mut found = 0usize;
        loop {
            match next_match(&text[start..], delimiters, ignore_case) {
                Some((rel, mlen)) => {
                    let pos = start + rel;
                    found += 1;
                    if found == want {
                        return Ok(text[..pos].to_string());
                    }
                    start = pos + mlen.max(1);
                    if start > text.len() {
                        break;
                    }
                }
                None => {
                    return if match_end && found + 1 == want {
                        Ok(text.to_string())
                    } else {
                        Err(ExcelError::Na)
                    };
                }
            }
        }
        return if match_end && found + 1 == want {
            Ok(text.to_string())
        } else {
            Err(ExcelError::Na)
        };
    }

    // Negative instance: last match is the common path (instance_num = -1).
    if want == 1 {
        return match last_match(text, delimiters, ignore_case) {
            Some(pos) => Ok(text[..pos].to_string()),
            None => not_found(text, instance_num, match_end),
        };
    }

    // General negative: collect byte offsets, then index from the end.
    let mut hits: Vec<usize> = Vec::new();
    let mut start = 0usize;
    while start <= text.len() {
        match next_match(&text[start..], delimiters, ignore_case) {
            Some((rel, mlen)) => {
                let pos = start + rel;
                hits.push(pos);
                start = pos + mlen.max(1);
                if start > text.len() {
                    break;
                }
            }
            None => break,
        }
    }
    if want <= hits.len() {
        let pos = hits[hits.len() - want];
        Ok(text[..pos].to_string())
    } else if match_end && want == hits.len() + 1 {
        Ok(String::new())
    } else {
        Err(ExcelError::Na)
    }
}

/// Leftmost match in `hay`. Same start prefers the longest delimiter.
fn next_match(hay: &str, delimiters: &[&str], ignore_case: bool) -> Option<(usize, usize)> {
    let mut best: Option<(usize, usize)> = None;
    for d in delimiters {
        if d.is_empty() {
            return Some((0, 0));
        }
        if let Some(pos) = find_one(hay, d, ignore_case) {
            let mlen = match_byte_len(hay, pos, d);
            match best {
                None => best = Some((pos, mlen)),
                Some((bp, bl)) => {
                    if pos < bp || (pos == bp && mlen > bl) {
                        best = Some((pos, mlen));
                    }
                }
            }
        }
    }
    best
}

fn last_match(hay: &str, delimiters: &[&str], ignore_case: bool) -> Option<usize> {
    let mut best: Option<(usize, usize)> = None;
    for d in delimiters {
        if d.is_empty() {
            continue;
        }
        if let Some(pos) = rfind_one(hay, d, ignore_case) {
            let mlen = match_byte_len(hay, pos, d);
            match best {
                None => best = Some((pos, mlen)),
                Some((bp, bl)) => {
                    if pos > bp || (pos == bp && mlen > bl) {
                        best = Some((pos, mlen));
                    }
                }
            }
        }
    }
    best.map(|(p, _)| p)
}

fn find_one(hay: &str, needle: &str, ignore_case: bool) -> Option<usize> {
    if ignore_case {
        ci_find(hay, needle)
    } else {
        search_bytes(hay, needle)
    }
}

fn rfind_one(hay: &str, needle: &str, ignore_case: bool) -> Option<usize> {
    if ignore_case {
        ci_rfind(hay, needle)
    } else {
        hay.rfind(needle)
    }
}

fn match_byte_len(hay: &str, pos: usize, needle: &str) -> usize {
    if hay.is_ascii() && needle.is_ascii() {
        return needle.len();
    }
    let nchars = needle.chars().count();
    let rest = &hay[pos..];
    rest.chars().take(nchars).map(|c| c.len_utf8()).sum()
}

/// `str::find` plus the ASCII last-byte SWAR probe (FIND hill-climb).
fn search_bytes(hay: &str, needle: &str) -> Option<usize> {
    if hay.is_ascii() && needle.is_ascii() && needle.len() >= 2 {
        return find_ascii_last_byte(hay.as_bytes(), needle.as_bytes());
    }
    hay.find(needle)
}

fn find_ascii_last_byte(hay: &[u8], needle: &[u8]) -> Option<usize> {
    debug_assert!(needle.len() >= 2);
    let nlen = needle.len();
    if hay.len() < nlen {
        return None;
    }
    let last = needle[nlen - 1];
    let mut i = nlen - 1;
    while i < hay.len() {
        let Some(rel) = memchr_byte(&hay[i..], last) else {
            return None;
        };
        let end = i + rel;
        let start = end + 1 - nlen;
        if &hay[start..=end] == needle {
            return Some(start);
        }
        i = end + 1;
    }
    None
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

fn ci_rfind(hay: &str, needle: &str) -> Option<usize> {
    if needle.is_empty() {
        return Some(hay.len());
    }
    if hay.is_ascii() && needle.is_ascii() {
        return ci_rfind_ascii(hay.as_bytes(), needle.as_bytes());
    }
    let mut last = None;
    let mut pos = 0usize;
    let mut rest = hay;
    while let Some(rel) = ci_find_unicode(rest, needle) {
        last = Some(pos + rel);
        let adv = rel + match_byte_len(rest, rel, needle).max(1);
        if adv >= rest.len() {
            break;
        }
        pos += adv;
        rest = &rest[adv..];
    }
    last
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

fn ci_rfind_ascii(hay: &[u8], needle: &[u8]) -> Option<usize> {
    if hay.len() < needle.len() {
        return None;
    }
    if needle.len() == 1 {
        return memrchr_ci(hay, needle[0]);
    }
    let nlen = needle.len();
    let last = needle[nlen - 1];
    let mut i = hay.len();
    while i >= nlen {
        let Some(rel) = memrchr_ci(&hay[..i], last) else {
            return None;
        };
        if rel + 1 >= nlen {
            let start = rel + 1 - nlen;
            if ci_bytes_eq(&hay[start..=rel], needle) {
                return Some(start);
            }
        }
        i = rel;
        if i == 0 {
            break;
        }
    }
    None
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
    a.len() == b.len()
        && a.iter()
            .zip(b.iter())
            .all(|(x, y)| x.eq_ignore_ascii_case(y))
}

fn ci_find_unicode(hay: &str, needle: &str) -> Option<usize> {
    let mut pos = 0usize;
    let mut rest = hay;
    while !rest.is_empty() {
        if ci_starts_with(rest, needle) {
            return Some(pos);
        }
        let ch = rest.chars().next()?;
        let adv = ch.len_utf8();
        pos += adv;
        rest = &rest[adv..];
    }
    None
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
    if a.is_ascii() || b.is_ascii() {
        return false;
    }
    a.to_lowercase().eq(b.to_lowercase())
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

fn memrchr_ci(hay: &[u8], needle: u8) -> Option<usize> {
    if needle.is_ascii_alphabetic() {
        let lo = needle.to_ascii_lowercase();
        let up = needle.to_ascii_uppercase();
        if lo != up {
            return hay.iter().rposition(|&c| c == lo || c == up);
        }
    }
    hay.iter().rposition(|&c| c == needle)
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

#[cfg(test)]
mod tests {
    use super::*;

    fn both(
        text: &str,
        delims: &[&str],
        instance: i64,
        ignore_case: bool,
        match_end: bool,
    ) -> Result<String, ExcelError> {
        let fast = textbefore(text, delims, instance, ignore_case, match_end);
        let slow = textbefore_naive(text, delims, instance, ignore_case, match_end);
        assert_eq!(
            fast, slow,
            "naive/fast mismatch text={text:?} delims={delims:?} n={instance} ci={ignore_case} end={match_end}"
        );
        fast
    }

    fn d(s: &str) -> [&str; 1] {
        [s]
    }

    #[test]
    fn microsoft_examples() {
        assert_eq!(
            both("Red riding hood's, red hood", &d("hood"), 1, false, false),
            Ok("Red riding ".into())
        );
        assert_eq!(
            both("Red riding hood's, red hood", &d(""), 1, false, false),
            Ok("".into())
        );
        assert_eq!(
            both("Red riding hood's, red hood", &d(""), -1, false, false),
            Ok("Red riding hood's, red hood".into())
        );
        assert_eq!(
            both(
                "Little Red Riding Hood's red hood",
                &d("Red"),
                1,
                false,
                false
            ),
            Ok("Little ".into())
        );
        assert_eq!(
            both(
                "Little red Riding Hood's red hood",
                &d("Red"),
                1,
                false,
                false
            ),
            Err(ExcelError::Na)
        );
        assert_eq!(
            both(
                "Little red Riding Hood's red hood",
                &d("red"),
                2,
                false,
                false
            ),
            Ok("Little red Riding Hood's ".into())
        );
        assert_eq!(
            both(
                "Little red Riding Hood's red hood",
                &d("red"),
                -2,
                false,
                false
            ),
            Ok("Little ".into())
        );
        assert_eq!(
            both(
                "Little red Riding Hood's red hood",
                &d("red"),
                3,
                false,
                false
            ),
            Err(ExcelError::Na)
        );
        assert_eq!(
            both("Marcus Aurelius", &d(" "), 1, false, true),
            Ok("Marcus".into())
        );
        assert_eq!(
            both("Socrates", &d(" "), 1, false, false),
            Err(ExcelError::Na)
        );
        assert_eq!(
            both("Socrates", &d(" "), 1, false, true),
            Ok("Socrates".into())
        );
        assert_eq!(
            both("Immanuel Kant", &d(" "), 1, false, true),
            Ok("Immanuel".into())
        );
    }

    #[test]
    fn instance_and_errors() {
        assert_eq!(
            both("a-b-c", &d("-"), 0, false, false),
            Err(ExcelError::Value)
        );
        assert_eq!(
            both("abc", &d("x"), 4, false, false),
            Err(ExcelError::Value)
        );
        assert_eq!(
            both("abc", &d("x"), -4, false, false),
            Err(ExcelError::Value)
        );
        assert_eq!(both("abc", &d("x"), 1, false, false), Err(ExcelError::Na));
        assert_eq!(both("a-b-c", &d("-"), 1, false, false), Ok("a".into()));
        assert_eq!(both("a-b-c", &d("-"), 2, false, false), Ok("a-b".into()));
        assert_eq!(both("a-b-c", &d("-"), -1, false, false), Ok("a-b".into()));
        assert_eq!(both("a-b-c", &d("-"), 3, false, false), Err(ExcelError::Na));
    }

    #[test]
    fn empty_text_and_delimiter() {
        assert_eq!(both("", &d("x"), 1, false, false), Ok("".into()));
        assert_eq!(both("", &d(""), -1, false, false), Ok("".into()));
        assert_eq!(both("abc", &d(""), 2, false, false), Ok("".into()));
        assert_eq!(both("abc", &d(""), -2, false, false), Ok("abc".into()));
        assert_eq!(both("abc", &d(""), 4, false, false), Err(ExcelError::Value));
    }

    #[test]
    fn match_mode_and_end() {
        assert_eq!(both("AbC", &d("b"), 1, false, false), Ok("A".into()));
        assert_eq!(both("AbC", &d("B"), 1, false, false), Err(ExcelError::Na));
        assert_eq!(both("AbC", &d("B"), 1, true, false), Ok("A".into()));
        assert_eq!(both("apple", &d("-"), 1, false, true), Ok("apple".into()));
        assert_eq!(both("apple", &d("-"), -1, false, true), Ok("".into()));
        assert_eq!(both("a-b", &d("-"), 2, false, true), Ok("a-b".into()));
        assert_eq!(both("a-b", &d("-"), -2, false, true), Ok("".into()));
        assert_eq!(both("a-b", &d("-"), 3, false, true), Err(ExcelError::Na));
    }

    #[test]
    fn multi_delimiter_and_unicode() {
        assert_eq!(both("a-b,c", &[",", "-"], 1, false, false), Ok("a".into()));
        assert_eq!(both("a,b-c", &[",", "-"], 1, false, false), Ok("a".into()));
        assert_eq!(
            both("foo::bar", &[":", "::"], 1, false, false),
            Ok("foo".into())
        );
        assert_eq!(
            both("café-latte", &d("-"), 1, false, false),
            Ok("café".into())
        );
        assert_eq!(both("日本語", &d("本"), 1, false, false), Ok("日".into()));
        assert_eq!(both("CAFÉ", &d("é"), 1, true, false), Ok("CAF".into()));
    }

    #[test]
    fn edges() {
        assert_eq!(both("-abc", &d("-"), 1, false, false), Ok("".into()));
        assert_eq!(both("abc-", &d("-"), 1, false, false), Ok("abc".into()));
        assert_eq!(both("abc", &d("abc"), 1, false, false), Ok("".into()));
        assert_eq!(both("aaa", &d("aa"), 1, false, false), Ok("".into()));
        assert_eq!(both("aaa", &d("aa"), 2, false, false), Err(ExcelError::Na));
        assert_eq!(both("ababab", &d("ab"), 2, false, false), Ok("ab".into()));
        assert!(both("abc", &[], 1, false, false).is_err());
    }

    #[test]
    fn almost_match_suffix() {
        let hay = format!("{}aab", "aaa".repeat(80));
        assert_eq!(both(&hay, &d("aab"), 1, false, false), Ok("aaa".repeat(80)));
        assert_eq!(both(&hay, &d("aac"), 1, false, false), Err(ExcelError::Na));
    }
}
