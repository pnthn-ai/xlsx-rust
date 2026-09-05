//! Excel `FIND` kernel.
//!
//! Semantics (desktop Excel):
//! - `FIND(find_text, within_text, [start_num])` — case-sensitive, no wildcards.
//! - 1-based character index (Unicode scalars, matching this crate's `LEN`/`MID`).
//! - Missing needle → `#VALUE!` (not `#N/A`).
//! - Empty `find_text` matches at `start_num`, including one past `LEN(within_text)`.
//! - `start_num` is 1-based; omitted means 1. `< 1` is `#VALUE!`.
//!
//! Production search uses `str::find` (Two-Way / `memchr`) for Unicode, plus
//! an ASCII last-byte SWAR probe for multi-byte needles (the `aaa…aab`
//! almost-match hill-climb) and an ASCII index path so the returned position
//! does not need a second character walk. The quadratic `Vec<char>`
//! sliding-window baseline lives beside that path so benches can report a
//! before/after.

use xlsx_types::ExcelError;

/// Production `FIND` kernel.
///
/// `start_num` is already truncated toward zero (1-based). Returns the
/// 1-based character position, or [`ExcelError::Value`].
pub fn find(find_text: &str, within_text: &str, start_num: i64) -> Result<f64, ExcelError> {
    find_impl(find_text, within_text, start_num, FindMode::Fast)
}

/// Quadratic baseline used for the hill-climb bench (`Vec<char>` + window).
///
/// Same Excel semantics as [`find`]; slower on large haystacks. Kept so
/// `cargo bench -p xlsx-engine-core --bench find` can print before/after.
pub fn find_naive(find_text: &str, within_text: &str, start_num: i64) -> Result<f64, ExcelError> {
    find_impl(find_text, within_text, start_num, FindMode::Naive)
}

#[derive(Clone, Copy)]
enum FindMode {
    Fast,
    Naive,
}

fn find_impl(
    find_text: &str,
    within_text: &str,
    start_num: i64,
    mode: FindMode,
) -> Result<f64, ExcelError> {
    if start_num < 1 {
        return Err(ExcelError::Value);
    }
    // Characters ≤ bytes. A start past byte-len+1 cannot be a valid char index.
    if start_num as u64 > within_text.len() as u64 + 1 {
        return Err(ExcelError::Value);
    }
    match mode {
        FindMode::Naive => find_chars(find_text, within_text, start_num),
        FindMode::Fast => find_twoway(find_text, within_text, start_num),
    }
}

fn find_chars(find_text: &str, within_text: &str, start_num: i64) -> Result<f64, ExcelError> {
    let hay: Vec<char> = within_text.chars().collect();
    let needle: Vec<char> = find_text.chars().collect();
    let start = (start_num as usize) - 1;
    if needle.is_empty() {
        return if start <= hay.len() {
            Ok(start_num as f64)
        } else {
            Err(ExcelError::Value)
        };
    }
    if start >= hay.len() || start + needle.len() > hay.len() {
        return Err(ExcelError::Value);
    }
    let nlen = needle.len();
    for i in start..=hay.len() - nlen {
        if hay[i..i + nlen] == needle[..] {
            return Ok((i + 1) as f64);
        }
    }
    Err(ExcelError::Value)
}

fn find_twoway(find_text: &str, within_text: &str, start_num: i64) -> Result<f64, ExcelError> {
    let skip = (start_num as usize) - 1;
    let Some(suffix) = skip_chars(within_text, skip) else {
        return Err(ExcelError::Value);
    };
    if find_text.is_empty() {
        return Ok(start_num as f64);
    }
    let Some(byte_off) = search_bytes(suffix, find_text) else {
        return Err(ExcelError::Value);
    };
    let extra = if suffix.is_ascii() {
        byte_off
    } else {
        suffix[..byte_off].chars().count()
    };
    Ok((start_num as usize + extra) as f64)
}

/// `str::find` is already Two-Way/`memchr`. For ASCII needles longer than one
/// byte whose last byte is rare in the haystack (the `aaa…aab` hill-climb),
/// probing that last byte with `memchr` then verifying beats a prefix-heavy
/// Two-Way scan.
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

/// Word-at-a-time `memchr`. Faster than a scalar scan on large haystacks;
/// enough of a hill-climb that we do not need a `memchr` crate dep.
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

fn skip_chars(s: &str, n: usize) -> Option<&str> {
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
        let fast = find(needle, hay, start);
        let slow = find_naive(needle, hay, start);
        assert_eq!(
            fast, slow,
            "naive/fast mismatch for {needle:?} in {hay:?} start={start}"
        );
        fast
    }

    #[test]
    fn microsoft_examples() {
        assert_eq!(both("M", "Miriam McGovern", 1), Ok(1.0));
        assert_eq!(both("m", "Miriam McGovern", 1), Ok(6.0));
        assert_eq!(both("M", "Miriam McGovern", 3), Ok(8.0));
    }

    #[test]
    fn start_num_and_miss() {
        assert_eq!(both("a", "banana", 3), Ok(4.0));
        assert_eq!(both("a", "banana", 6), Ok(6.0));
        assert_eq!(both("a", "banana", 7), Err(ExcelError::Value));
        assert_eq!(both("z", "abc", 1), Err(ExcelError::Value));
        assert_eq!(both("a", "abc", 0), Err(ExcelError::Value));
        assert_eq!(both("a", "abc", -1), Err(ExcelError::Value));
    }

    #[test]
    fn empty_find_text() {
        assert_eq!(both("", "abc", 1), Ok(1.0));
        assert_eq!(both("", "abc", 3), Ok(3.0));
        assert_eq!(both("", "abc", 4), Ok(4.0));
        assert_eq!(both("", "abc", 5), Err(ExcelError::Value));
        assert_eq!(both("", "", 1), Ok(1.0));
        assert_eq!(both("a", "", 1), Err(ExcelError::Value));
    }

    #[test]
    fn case_sensitive_unlike_search() {
        assert_eq!(both("a", "ABC", 1), Err(ExcelError::Value));
        assert_eq!(both("A", "ABC", 1), Ok(1.0));
        assert_eq!(both("bc", "ABC", 1), Err(ExcelError::Value));
        assert_eq!(both("BC", "ABC", 1), Ok(2.0));
    }

    #[test]
    fn wildcards_are_literal() {
        assert_eq!(both("*", "abc", 1), Err(ExcelError::Value));
        assert_eq!(both("a*", "abc", 1), Err(ExcelError::Value));
        assert_eq!(both("*", "a*b", 1), Ok(2.0));
        assert_eq!(both("?", "a?b", 1), Ok(2.0));
    }

    #[test]
    fn unicode_scalar_index() {
        assert_eq!(both("é", "café", 1), Ok(4.0));
        assert_eq!(both("日", "日本語", 1), Ok(1.0));
        assert_eq!(both("語", "日本語", 1), Ok(3.0));
        assert_eq!(both("é", "cafe", 1), Err(ExcelError::Value));
    }

    #[test]
    fn huge_start_rejects_without_scan() {
        assert_eq!(both("a", "abc", i64::MAX), Err(ExcelError::Value));
    }

    #[test]
    fn almost_match_suffix() {
        let hay = format!("{}aab", "aaa".repeat(80));
        assert_eq!(both("aab", &hay, 1), Ok((hay.len() - 2) as f64));
        assert_eq!(both("aac", &hay, 1), Err(ExcelError::Value));
    }
}
