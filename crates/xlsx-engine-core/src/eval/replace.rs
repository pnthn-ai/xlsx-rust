//! Excel `REPLACE` kernel.
//!
//! `REPLACE(old_text, start_num, num_chars, new_text)` overwrites a
//! 1-based character span. The production path finds UTF-8 byte offsets
//! (ASCII is O(1) index arithmetic) and builds the result in one allocation.
//! The `Vec<char>` baseline lives beside it so benches can report a
//! before/after.
//!
//! Character indexing matches current `LEN` / `MID` / `LEFT` / `RIGHT`:
//! Unicode scalar values (`str::chars`), which is Excel Compatibility
//! Version 2 (a surrogate-pair emoji is **one** character). Version 1
//! counted UTF-16 code units (`😀` = 2); that legacy mode is not
//! implemented. Combining marks and variation selectors stay separate
//! scalars in both versions.

/// Production `REPLACE` kernel.
///
/// `start_num` is 1-based and must be `>= 1`. `num_chars` must be `>= 0`.
/// Callers reject out-of-range / non-finite numeric arguments as `#VALUE!`
/// before calling this. A start past `LEN(old_text)` appends `new_text`.
pub fn replace(old_text: &str, start_num: u64, num_chars: u64, new_text: &str) -> String {
    debug_assert!(start_num >= 1);
    if num_chars == 0 && new_text.is_empty() {
        return old_text.to_owned();
    }
    if old_text.is_ascii() {
        return replace_ascii(old_text, start_num, num_chars, new_text);
    }
    replace_utf8(old_text, start_num, num_chars, new_text)
}

/// Quadratic-ish baseline: materialize every Unicode scalar, then rebuild.
///
/// Same Excel semantics as [`replace`]. Kept so
/// `cargo bench -p xlsx-engine-core --bench replace` can print before/after.
pub fn replace_naive(old_text: &str, start_num: u64, num_chars: u64, new_text: &str) -> String {
    debug_assert!(start_num >= 1);
    let chars: Vec<char> = old_text.chars().collect();
    let start0 = match usize::try_from(start_num.saturating_sub(1)) {
        Ok(n) => n,
        Err(_) => {
            let mut out = String::with_capacity(old_text.len() + new_text.len());
            out.push_str(old_text);
            out.push_str(new_text);
            return out;
        }
    };
    if start0 >= chars.len() {
        let mut out = String::with_capacity(old_text.len() + new_text.len());
        out.push_str(old_text);
        out.push_str(new_text);
        return out;
    }
    let take = match usize::try_from(num_chars) {
        Ok(n) => n,
        Err(_) => chars.len() - start0,
    };
    let end = start0.saturating_add(take).min(chars.len());
    let mut out = String::new();
    out.extend(chars[..start0].iter());
    out.push_str(new_text);
    out.extend(chars[end..].iter());
    out
}

fn replace_ascii(old: &str, start_num: u64, num_chars: u64, new_text: &str) -> String {
    debug_assert!(old.is_ascii());
    let n = old.len() as u64;
    let start0 = start_num - 1;
    if start0 >= n {
        let mut out = String::with_capacity(old.len() + new_text.len());
        out.push_str(old);
        out.push_str(new_text);
        return out;
    }
    let lo = start0 as usize;
    let hi = start0.saturating_add(num_chars).min(n) as usize;
    if lo == hi && new_text.is_empty() {
        return old.to_owned();
    }
    // Equal-width ASCII overwrite: clone once and patch in place.
    if new_text.is_ascii() && hi - lo == new_text.len() {
        let mut buf = old.to_owned();
        // SAFETY: `old` and `new_text` are ASCII of equal byte length, so
        // overwriting the span cannot produce invalid UTF-8.
        unsafe {
            buf.as_bytes_mut()[lo..hi].copy_from_slice(new_text.as_bytes());
        }
        return buf;
    }
    let mut out = String::with_capacity(lo + new_text.len() + (old.len() - hi));
    out.push_str(&old[..lo]);
    out.push_str(new_text);
    out.push_str(&old[hi..]);
    out
}

fn replace_utf8(old: &str, start_num: u64, num_chars: u64, new_text: &str) -> String {
    let (lo, hi) = utf8_span(old, start_num, num_chars);
    if lo == hi && new_text.is_empty() {
        return old.to_owned();
    }
    let mut out = String::with_capacity(lo + new_text.len() + (old.len() - hi));
    out.push_str(&old[..lo]);
    out.push_str(new_text);
    out.push_str(&old[hi..]);
    out
}

/// Byte offsets `[lo, hi)` of the 1-based Unicode-scalar span.
fn utf8_span(s: &str, start_num: u64, num_chars: u64) -> (usize, usize) {
    let start0 = start_num - 1;
    let mut seen = 0u64;
    let mut prefix_end = s.len();
    let mut found = false;
    for (byte_i, _) in s.char_indices() {
        if !found {
            if seen == start0 {
                prefix_end = byte_i;
                found = true;
                if num_chars == 0 {
                    return (byte_i, byte_i);
                }
            }
        } else if seen - start0 == num_chars {
            return (prefix_end, byte_i);
        }
        seen += 1;
    }
    if found {
        (prefix_end, s.len())
    } else {
        (s.len(), s.len())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn both(old: &str, start: u64, n: u64, new: &str) -> String {
        let fast = replace(old, start, n, new);
        let slow = replace_naive(old, start, n, new);
        assert_eq!(
            fast, slow,
            "naive/fast mismatch for {old:?} start={start} n={n} new={new:?}"
        );
        fast
    }

    #[test]
    fn microsoft_examples() {
        assert_eq!(both("abcdefghijk", 6, 5, "*"), "abcde*k");
        assert_eq!(both("2009", 3, 2, "10"), "2010");
        assert_eq!(both("123456", 1, 3, "@"), "@456");
    }

    #[test]
    fn one_based_start() {
        assert_eq!(both("abc", 1, 1, "X"), "Xbc");
        assert_eq!(both("abc", 2, 1, "X"), "aXc");
        assert_eq!(both("abc", 3, 1, "X"), "abX");
    }

    #[test]
    fn num_chars_zero_inserts() {
        assert_eq!(both("abc", 1, 0, "X"), "Xabc");
        assert_eq!(both("abc", 2, 0, "X"), "aXbc");
        assert_eq!(both("abc", 4, 0, "X"), "abcX");
    }

    #[test]
    fn empty_new_text_deletes() {
        assert_eq!(both("abc", 2, 1, ""), "ac");
        assert_eq!(both("abc", 1, 3, ""), "");
        assert_eq!(both("abc", 2, 0, ""), "abc");
    }

    #[test]
    fn out_of_range_appends() {
        assert_eq!(both("abc", 4, 1, "X"), "abcX");
        assert_eq!(both("abc", 100, 5, "X"), "abcX");
        assert_eq!(both("abc", 2, 10, "X"), "aX");
    }

    #[test]
    fn empty_old_text() {
        assert_eq!(both("", 1, 0, "X"), "X");
        assert_eq!(both("", 1, 1, "X"), "X");
        assert_eq!(both("", 2, 1, "X"), "X");
    }

    #[test]
    fn unicode_scalars_not_utf16() {
        assert_eq!(both("café", 4, 1, "e"), "cafe");
        assert_eq!(both("日本語", 2, 1, "X"), "日X語");
        // U+1F600 is one scalar (Excel Compatibility Version 2).
        assert_eq!(both("a😀b", 2, 1, "X"), "aXb");
        assert_eq!(both("a😀b", 3, 1, "X"), "a😀X");
        // Combining acute is its own scalar.
        assert_eq!(both("e\u{0301}", 2, 1, ""), "e");
        assert_eq!(both("e\u{0301}", 1, 1, "o"), "o\u{0301}");
    }

    #[test]
    fn large_start_appends() {
        assert_eq!(both("ab", u64::MAX, 1, "Z"), "abZ");
    }
}
