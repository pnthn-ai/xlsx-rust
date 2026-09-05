//! Excel `TEXTAFTER` kernel.
//!
//! Semantics (Microsoft 365 / Excel 2024):
//! - `TEXTAFTER(text, delimiter, [instance_num], [match_mode], [match_end], [if_not_found])`
//! - Returns the substring **after** the chosen delimiter instance.
//! - `instance_num` defaults to 1. `0` is `#VALUE!`. Negative values count
//!   from the end (`-1` is the last instance).
//! - `|instance_num| > LEN(text)` is `#VALUE!` (character length).
//! - `|instance_num|` greater than the number of matches is `#N/A`
//!   (caller may substitute `if_not_found`).
//! - `match_mode` 0 / FALSE: case-sensitive (default). Non-zero / TRUE:
//!   case-insensitive (ASCII + Unicode letter fold; no wildcards).
//! - `match_end` 1 / TRUE: treat the end of text as an extra delimiter when
//!   counting forward, and the start of text when counting backward.
//! - Empty delimiter: matches immediately. Positive `instance_num` returns
//!   the entire text; negative returns `""`.
//! - Matches are **non-overlapping**. Several delimiters (array) take the
//!   leftmost hit; a start-index tie keeps the first delimiter in list order.
//!
//! Production search uses `str::find` / `rfind` plus an ASCII last-byte SWAR
//! probe (case-sensitive and case-insensitive). The `Vec<char>` sliding-window
//! baseline lives beside that path so benches can print before/after.

use xlsx_types::ExcelError;

/// Production `TEXTAFTER` kernel.
///
/// `delimiters` may be empty or contain only `""` (empty-delimiter rule).
/// `instance_num` is already truncated toward zero. `ignore_case` is
/// `match_mode != 0`. `match_end` adds a virtual delimiter at the end
/// (forward) or start (backward).
pub fn textafter(
    text: &str,
    delimiters: &[&str],
    instance_num: i64,
    ignore_case: bool,
    match_end: bool,
) -> Result<String, ExcelError> {
    textafter_impl(
        text,
        delimiters,
        instance_num,
        ignore_case,
        match_end,
        TextAfterMode::Fast,
    )
}

/// Quadratic / collect-every-char baseline used for the hill-climb bench.
///
/// Same Excel semantics as [`textafter`].
pub fn textafter_naive(
    text: &str,
    delimiters: &[&str],
    instance_num: i64,
    ignore_case: bool,
    match_end: bool,
) -> Result<String, ExcelError> {
    textafter_impl(
        text,
        delimiters,
        instance_num,
        ignore_case,
        match_end,
        TextAfterMode::Naive,
    )
}

#[derive(Clone, Copy)]
enum TextAfterMode {
    Fast,
    Naive,
}

fn textafter_impl(
    text: &str,
    delimiters: &[&str],
    instance_num: i64,
    ignore_case: bool,
    match_end: bool,
    mode: TextAfterMode,
) -> Result<String, ExcelError> {
    if instance_num == 0 {
        return Err(ExcelError::Value);
    }
    let want = instance_num.unsigned_abs();
    // Characters ≤ bytes. |instance| past byte-len cannot be a valid char index.
    if want > text.len() as u64 {
        return Err(ExcelError::Value);
    }
    let real: Vec<&str> = delimiters
        .iter()
        .copied()
        .filter(|d| !d.is_empty())
        .collect();
    if real.is_empty() {
        return empty_delimiter(text, instance_num, want);
    }
    match mode {
        TextAfterMode::Naive => apply_positions(
            text,
            &collect_after_chars(text, &real, ignore_case),
            instance_num,
            match_end,
        ),
        TextAfterMode::Fast => textafter_fast(text, &real, instance_num, ignore_case, match_end),
    }
}

fn empty_delimiter(text: &str, instance_num: i64, want: u64) -> Result<String, ExcelError> {
    if !text.is_ascii() && want > text.chars().count() as u64 {
        return Err(ExcelError::Value);
    }
    if instance_num > 0 {
        Ok(text.to_owned())
    } else {
        Ok(String::new())
    }
}

fn apply_positions(
    text: &str,
    after_bytes: &[usize],
    instance_num: i64,
    match_end: bool,
) -> Result<String, ExcelError> {
    let mut pos = after_bytes.to_vec();
    if match_end {
        if instance_num < 0 {
            pos.insert(0, 0);
        } else {
            pos.push(text.len());
        }
    }
    let idx = resolve_instance(instance_num, pos.len())?;
    let at = pos[idx];
    Ok(text[at..].to_owned())
}

fn resolve_instance(instance_num: i64, n: usize) -> Result<usize, ExcelError> {
    if n == 0 {
        return Err(ExcelError::Na);
    }
    let i = if instance_num < 0 {
        n as i64 + instance_num + 1
    } else {
        instance_num
    };
    if i < 1 || i as usize > n {
        Err(ExcelError::Na)
    } else {
        Ok((i as usize) - 1)
    }
}

/// After-byte of every non-overlapping match, using a `Vec<char>` walk.
fn collect_after_chars(text: &str, delims: &[&str], ignore_case: bool) -> Vec<usize> {
    let hay: Vec<char> = text.chars().collect();
    let needles: Vec<Vec<char>> = delims.iter().map(|d| d.chars().collect()).collect();
    let mut after = Vec::new();
    let mut start = 0usize;
    while start < hay.len() {
        let mut best_i: Option<usize> = None;
        let mut best_len = 0usize;
        for n in &needles {
            if n.is_empty() {
                continue;
            }
            if let Some(rel) = find_chars_from(&hay[start..], n, ignore_case) {
                let at = start + rel;
                if best_i.map_or(true, |b| at < b) {
                    best_i = Some(at);
                    best_len = n.len();
                }
            }
        }
        match best_i {
            Some(at) => {
                let end = at + best_len;
                after.push(char_byte_at(text, end));
                start = end;
            }
            None => break,
        }
    }
    after
}

fn find_chars_from(hay: &[char], needle: &[char], ignore_case: bool) -> Option<usize> {
    if needle.is_empty() || needle.len() > hay.len() {
        return None;
    }
    let last = hay.len() - needle.len();
    for i in 0..=last {
        if chars_eq(&hay[i..i + needle.len()], needle, ignore_case) {
            return Some(i);
        }
    }
    None
}

fn chars_eq(a: &[char], b: &[char], ignore_case: bool) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.iter()
        .zip(b.iter())
        .all(|(x, y)| if ignore_case { ci_eq(*x, *y) } else { x == y })
}

fn char_byte_at(s: &str, nchars: usize) -> usize {
    if s.is_ascii() {
        nchars.min(s.len())
    } else {
        s.char_indices()
            .nth(nchars)
            .map(|(i, _)| i)
            .unwrap_or(s.len())
    }
}

fn textafter_fast(
    text: &str,
    delims: &[&str],
    instance_num: i64,
    ignore_case: bool,
    match_end: bool,
) -> Result<String, ExcelError> {
    if !text.is_ascii() {
        let want = instance_num.unsigned_abs();
        if want > text.chars().count() as u64 {
            return Err(ExcelError::Value);
        }
    }

    let want = instance_num.unsigned_abs() as usize;
    // Last instance: one reverse scan. match_end on a miss returns the
    // whole string (virtual start delimiter).
    if instance_num == -1 && delims.len() == 1 {
        return last_one(text, delims[0], ignore_case, match_end);
    }
    if instance_num == -1 {
        return last_any(text, delims, ignore_case, match_end);
    }
    // First instance: one forward scan. match_end on a miss returns "".
    if instance_num == 1 && delims.len() == 1 {
        return first_one(text, delims[0], ignore_case, match_end);
    }
    if instance_num == 1 {
        return first_any(text, delims, ignore_case, match_end);
    }

    // General: collect after-bytes, stop early on a forward count.
    let cap = if instance_num > 0 { want } else { usize::MAX };
    let after = collect_after_bytes(text, delims, ignore_case, cap);
    apply_positions(text, &after, instance_num, match_end)
}

fn first_one(
    text: &str,
    delim: &str,
    ignore_case: bool,
    match_end: bool,
) -> Result<String, ExcelError> {
    match find_rel(text, delim, ignore_case) {
        Some(rel) => {
            let after = rel + hay_match_bytes(&text[rel..], delim);
            Ok(text[after..].to_owned())
        }
        None if match_end => Ok(String::new()),
        None => Err(ExcelError::Na),
    }
}

fn first_any(
    text: &str,
    delims: &[&str],
    ignore_case: bool,
    match_end: bool,
) -> Result<String, ExcelError> {
    match next_after(text, 0, delims, ignore_case) {
        Some(after) => Ok(text[after..].to_owned()),
        None if match_end => Ok(String::new()),
        None => Err(ExcelError::Na),
    }
}

fn last_one(
    text: &str,
    delim: &str,
    ignore_case: bool,
    match_end: bool,
) -> Result<String, ExcelError> {
    match rfind_rel(text, delim, ignore_case) {
        Some(rel) => {
            let after = rel + hay_match_bytes(&text[rel..], delim);
            Ok(text[after..].to_owned())
        }
        None if match_end => Ok(text.to_owned()),
        None => Err(ExcelError::Na),
    }
}

fn last_any(
    text: &str,
    delims: &[&str],
    ignore_case: bool,
    match_end: bool,
) -> Result<String, ExcelError> {
    let mut best_start: Option<usize> = None;
    let mut best_after = 0usize;
    for d in delims {
        if let Some(rel) = rfind_rel(text, d, ignore_case) {
            if best_start.map_or(true, |b| rel > b) {
                best_start = Some(rel);
                best_after = rel + hay_match_bytes(&text[rel..], d);
            }
        }
    }
    match best_start {
        Some(_) => Ok(text[best_after..].to_owned()),
        None if match_end => Ok(text.to_owned()),
        None => Err(ExcelError::Na),
    }
}

fn collect_after_bytes(text: &str, delims: &[&str], ignore_case: bool, limit: usize) -> Vec<usize> {
    let mut after = Vec::new();
    let mut from = 0usize;
    while after.len() < limit {
        match next_after(text, from, delims, ignore_case) {
            Some(end) => {
                after.push(end);
                from = end;
            }
            None => break,
        }
    }
    after
}

fn next_after(hay: &str, from: usize, delims: &[&str], ignore_case: bool) -> Option<usize> {
    let rest = &hay[from..];
    let mut best_rel: Option<usize> = None;
    let mut best_after_rel = 0usize;
    for d in delims {
        if let Some(rel) = find_rel(rest, d, ignore_case) {
            if best_rel.map_or(true, |b| rel < b) {
                best_rel = Some(rel);
                best_after_rel = rel + hay_match_bytes(&rest[rel..], d);
            }
        }
    }
    best_rel.map(|_| from + best_after_rel)
}

fn hay_match_bytes(at: &str, delim: &str) -> usize {
    if at.is_ascii() && delim.is_ascii() {
        delim.len()
    } else {
        let n = delim.chars().count();
        at.len() - skip_n_chars(at, n).map(|s| s.len()).unwrap_or(0)
    }
}

fn find_rel(hay: &str, needle: &str, ignore_case: bool) -> Option<usize> {
    if needle.is_empty() {
        return Some(0);
    }
    if !ignore_case {
        return hay.find(needle);
    }
    ci_find(hay, needle)
}

fn rfind_rel(hay: &str, needle: &str, ignore_case: bool) -> Option<usize> {
    if needle.is_empty() {
        return Some(hay.len());
    }
    if !ignore_case {
        return hay.rfind(needle);
    }
    ci_rfind(hay, needle)
}

fn ci_find(hay: &str, needle: &str) -> Option<usize> {
    if hay.is_ascii() && needle.is_ascii() {
        return ci_find_ascii(hay.as_bytes(), needle.as_bytes());
    }
    ci_find_unicode(hay, needle)
}

fn ci_rfind(hay: &str, needle: &str) -> Option<usize> {
    if hay.is_ascii() && needle.is_ascii() {
        return ci_rfind_ascii(hay.as_bytes(), needle.as_bytes());
    }
    let mut last = None;
    let mut pos = 0usize;
    let mut rest = hay;
    while !rest.is_empty() {
        if ci_starts_with(rest, needle) {
            last = Some(pos);
        }
        rest = skip_n_chars(rest, 1)?;
        pos = hay.len() - rest.len();
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
        let n = needle[0];
        return hay.iter().rposition(|&b| ci_byte_eq(b, n));
    }
    let nlen = needle.len();
    let mut i = hay.len();
    while i >= nlen {
        let start = i - nlen;
        if ci_bytes_eq(&hay[start..i], needle) {
            return Some(start);
        }
        i -= 1;
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

fn ci_find_unicode(hay: &str, needle: &str) -> Option<usize> {
    let mut pos = 0usize;
    let mut rest = hay;
    while !rest.is_empty() {
        if ci_starts_with(rest, needle) {
            return Some(pos);
        }
        rest = skip_n_chars(rest, 1)?;
        pos = hay.len() - rest.len();
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

fn ci_bytes_eq(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len()
        && a.iter()
            .zip(b.iter())
            .all(|(x, y)| x.eq_ignore_ascii_case(y))
}

fn ci_byte_eq(a: u8, b: u8) -> bool {
    a.eq_ignore_ascii_case(&b)
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

    fn both(
        text: &str,
        delims: &[&str],
        instance: i64,
        ignore_case: bool,
        match_end: bool,
    ) -> Result<String, ExcelError> {
        let fast = textafter(text, delims, instance, ignore_case, match_end);
        let slow = textafter_naive(text, delims, instance, ignore_case, match_end);
        assert_eq!(
            fast, slow,
            "naive/fast mismatch for {text:?} delims={delims:?} n={instance} ci={ignore_case} end={match_end}"
        );
        fast
    }

    fn d(s: &str) -> [&str; 1] {
        [s]
    }

    #[test]
    fn microsoft_hood() {
        assert_eq!(
            both("Red riding hood's, red hood", &d("hood"), 1, false, false).unwrap(),
            "'s, red hood"
        );
        assert_eq!(
            both("Red riding hood's, red hood", &d(""), 1, false, false).unwrap(),
            "Red riding hood's, red hood"
        );
        assert_eq!(
            both("Red riding hood's, red hood", &d(""), -1, false, false).unwrap(),
            ""
        );
    }

    #[test]
    fn microsoft_riding_hood() {
        let a2 = "Little Red Riding Hood's red hood";
        let a3 = "Little red Riding Hood's red hood";
        assert_eq!(
            both(a2, &d("Red"), 1, false, false).unwrap(),
            " Riding Hood's red hood"
        );
        assert_eq!(both(a2, &d("basket"), 1, false, false), Err(ExcelError::Na));
        assert_eq!(both(a3, &d("red"), 2, false, false).unwrap(), " hood");
        assert_eq!(
            both(a3, &d("red"), -2, false, false).unwrap(),
            " Riding Hood's red hood"
        );
        assert_eq!(both(a3, &d("Red"), 1, false, false), Err(ExcelError::Na));
        assert_eq!(both(a2, &d("red"), 3, false, false), Err(ExcelError::Na));
    }

    #[test]
    fn microsoft_match_end_names() {
        assert_eq!(
            both("Marcus Aurelius", &d(" "), 1, false, true).unwrap(),
            "Aurelius"
        );
        assert_eq!(
            both("Socrates", &d(" "), 1, false, false),
            Err(ExcelError::Na)
        );
        assert_eq!(both("Socrates", &d(" "), 1, false, true).unwrap(), "");
        assert_eq!(
            both("Immanuel Kant", &d(" "), 1, false, true).unwrap(),
            "Kant"
        );
    }

    #[test]
    fn better_solutions_examples() {
        assert_eq!(
            both("better solutions", &d(" "), 1, false, false).unwrap(),
            "solutions"
        );
        assert_eq!(
            both("better solutions", &d("t"), 1, false, false).unwrap(),
            "ter solutions"
        );
        assert_eq!(
            both("better solutions", &d("t"), 2, false, false).unwrap(),
            "er solutions"
        );
        assert_eq!(
            both("better solutions", &d("t"), 3, false, false).unwrap(),
            "ions"
        );
        assert_eq!(
            both("better solutions", &d("T"), 3, false, false),
            Err(ExcelError::Na)
        );
        assert_eq!(
            both("better solutions", &d("T"), 3, true, false).unwrap(),
            "ions"
        );
        assert_eq!(both("a-b-c-d", &d("-"), 1, false, false).unwrap(), "b-c-d");
        assert_eq!(both("a-b-c-d", &d("-"), -1, false, false).unwrap(), "d");
        assert_eq!(
            both("a-b-c-d", &d("-"), -4, false, true).unwrap(),
            "a-b-c-d"
        );
        assert_eq!(
            both("a-b-c-d", &d("-"), -4, false, false),
            Err(ExcelError::Na)
        );
        assert_eq!(
            both("a-b-c-d", &d(" "), 0, false, false),
            Err(ExcelError::Value)
        );
    }

    #[test]
    fn instance_vs_len_is_value() {
        assert_eq!(
            both("abc", &d("x"), 4, false, false),
            Err(ExcelError::Value)
        );
        assert_eq!(
            both("abc", &d("x"), -4, false, false),
            Err(ExcelError::Value)
        );
        assert_eq!(both("", &d("x"), 1, false, false), Err(ExcelError::Value));
        assert_eq!(both("abc", &d("x"), 3, false, false), Err(ExcelError::Na));
    }

    #[test]
    fn match_end_extra_delimiter() {
        assert_eq!(
            both("apple-orange-banana", &d("-"), 3, false, false),
            Err(ExcelError::Na)
        );
        assert_eq!(
            both("apple-orange-banana", &d("-"), 3, false, true).unwrap(),
            ""
        );
        assert_eq!(
            both("apple-orange-banana", &d("-"), 4, false, true),
            Err(ExcelError::Na)
        );
    }

    #[test]
    fn empty_delimiter_positive_and_negative() {
        assert_eq!(both("abc", &d(""), 1, false, false).unwrap(), "abc");
        assert_eq!(both("abc", &d(""), 2, false, false).unwrap(), "abc");
        assert_eq!(both("abc", &d(""), 3, false, false).unwrap(), "abc");
        assert_eq!(both("abc", &d(""), 4, false, false), Err(ExcelError::Value));
        assert_eq!(both("abc", &d(""), -1, false, false).unwrap(), "");
        assert_eq!(both("abc", &d(""), -3, false, false).unwrap(), "");
    }

    #[test]
    fn multi_delimiter_leftmost() {
        assert_eq!(both("a;b,c", &[";", ","], 1, false, false).unwrap(), "b,c");
        assert_eq!(both("a;b,c", &[";", ","], 2, false, false).unwrap(), "c");
        assert_eq!(both("a;b,c", &[",", ";"], 1, false, false).unwrap(), "b,c");
    }

    #[test]
    fn unicode_scalar() {
        assert_eq!(both("café", &d("é"), 1, false, false).unwrap(), "");
        assert_eq!(both("café", &d("é"), 1, true, false).unwrap(), "");
        assert_eq!(both("café", &d("É"), 1, true, false).unwrap(), "");
        assert_eq!(both("日本語", &d("本"), 1, false, false).unwrap(), "語");
        assert_eq!(
            both("café", &d("é"), 5, false, false),
            Err(ExcelError::Value)
        );
    }

    #[test]
    fn non_overlapping() {
        assert_eq!(both("aaa", &d("aa"), 1, false, false).unwrap(), "a");
        assert_eq!(both("aaa", &d("aa"), 2, false, false), Err(ExcelError::Na));
    }

    #[test]
    fn almost_match_suffix() {
        let hay = format!("{}aab", "aaa".repeat(80));
        assert_eq!(both(&hay, &d("aab"), 1, false, false).unwrap(), "");
        assert_eq!(both(&hay, &d("aac"), 1, false, false), Err(ExcelError::Na));
        assert_eq!(both(&hay, &d("AAB"), 1, true, false).unwrap(), "");
    }
}
