//! Excel `SUBSTITUTE` kernel.
//!
//! Semantics (desktop Excel):
//! - Case-sensitive, no wildcards (`*` / `?` / `~` are literal).
//! - Left-to-right, **non-overlapping** matches. Replacements are not re-scanned.
//! - Empty `old_text` matches nothing — the original text is returned.
//! - `instance_num` is 1-based; omitted means replace every match.
//!
//! Production replace-all specializes:
//! - ASCII byte→byte SWAR swap (word-at-a-time, no `find` per hit)
//! - ASCII single-byte delete (filter)
//! - Equal-width in-place overwrite
//! - Single-pass resize
//!
//! Search uses a word-at-a-time `memchr` for single ASCII bytes and an
//! ASCII last-byte SWAR probe for multi-byte needles (the `aaa…aab`
//! almost-match hill-climb). The quadratic `replace_range` baseline lives
//! beside that path so benches can report a before/after.

/// Production `SUBSTITUTE` kernel.
///
/// `instance_num` is 1-based. `None` replaces every non-overlapping match.
pub fn substitute(text: &str, old_text: &str, new_text: &str, instance_num: Option<u32>) -> String {
    if old_text.is_empty() || old_text == new_text {
        return text.to_owned();
    }
    match instance_num {
        None => replace_all(text, old_text, new_text),
        Some(n) => replace_nth(text, old_text, new_text, n),
    }
}

/// Quadratic baseline used for the hill-climb bench (`replace_range` per hit).
///
/// Same Excel semantics as [`substitute`]; slower when many matches land in a
/// large string. Kept so `cargo bench -p xlsx-engine-core` can print before/after.
pub fn substitute_naive(
    text: &str,
    old_text: &str,
    new_text: &str,
    instance_num: Option<u32>,
) -> String {
    if old_text.is_empty() {
        return text.to_owned();
    }
    let mut s = text.to_owned();
    let mut from = 0usize;
    let mut seen = 0u32;
    while from < s.len() {
        let Some(rel) = s[from..].find(old_text) else {
            break;
        };
        let pos = from + rel;
        seen += 1;
        let replace = match instance_num {
            None => true,
            Some(n) => seen == n,
        };
        if replace {
            s.replace_range(pos..pos + old_text.len(), new_text);
            from = pos + new_text.len();
            if instance_num.is_some() {
                break;
            }
        } else {
            from = pos + old_text.len();
        }
    }
    s
}

fn replace_all(text: &str, old: &str, new: &str) -> String {
    debug_assert!(!old.is_empty() && old != new);
    // ASCII byte→byte: one SWAR scan, no `find` per hit.
    if old.len() == 1 && new.len() == 1 {
        let o = old.as_bytes()[0];
        let n = new.as_bytes()[0];
        if o.is_ascii() && n.is_ascii() {
            return replace_all_ascii_byte(text, o, n);
        }
    }
    // ASCII byte delete: filter the haystack once.
    if old.len() == 1 && new.is_empty() {
        let o = old.as_bytes()[0];
        if o.is_ascii() {
            return replace_all_ascii_delete(text, o);
        }
    }
    // Equal UTF-8 width: clone once and overwrite matches in place.
    if old.len() == new.len() {
        return replace_all_equal_len(text, old, new);
    }
    replace_all_resized(text, old, new)
}

fn replace_all_ascii_byte(text: &str, old: u8, new: u8) -> String {
    debug_assert!(old.is_ascii() && new.is_ascii());
    let hay = text.as_bytes();
    let Some(first) = memchr_byte(hay, old) else {
        return text.to_owned();
    };
    let mut buf = text.to_owned();
    // SAFETY: `old` and `new` are ASCII. Replacing one ASCII byte with
    // another cannot create invalid UTF-8.
    swar_replace_byte(unsafe { buf.as_bytes_mut() }, old, new, first);
    buf
}

fn replace_all_ascii_delete(text: &str, old: u8) -> String {
    debug_assert!(old.is_ascii());
    let hay = text.as_bytes();
    let Some(first) = memchr_byte(hay, old) else {
        return text.to_owned();
    };
    let mut buf = Vec::with_capacity(text.len());
    buf.extend_from_slice(&hay[..first]);
    for &b in &hay[first + 1..] {
        if b != old {
            buf.push(b);
        }
    }
    // SAFETY: we only dropped ASCII `old` bytes; remaining bytes are unchanged UTF-8.
    unsafe { String::from_utf8_unchecked(buf) }
}

fn replace_all_equal_len(text: &str, old: &str, new: &str) -> String {
    debug_assert_eq!(old.len(), new.len());
    debug_assert_ne!(old, new);
    let Some(first) = search_bytes(text, old) else {
        return text.to_owned();
    };
    let mut buf = text.to_owned();
    let n = old.len();
    let mut pos = first;
    loop {
        // SAFETY: `old` and `new` are valid UTF-8 of equal byte length, so
        // overwriting a found `old` span with `new` leaves the string valid.
        unsafe {
            buf.as_bytes_mut()[pos..pos + n].copy_from_slice(new.as_bytes());
        }
        let from = pos + n;
        match search_bytes(&text[from..], old) {
            Some(rel) => pos = from + rel,
            None => break,
        }
    }
    buf
}

fn replace_all_resized(text: &str, old: &str, new: &str) -> String {
    let Some(first) = search_bytes(text, old) else {
        return text.to_owned();
    };
    // One pass. Shrinking fits in `text.len()`. Growing: reserve every
    // non-overlapping slot so a dense needle does not realloc.
    let cap = if new.len() <= old.len() {
        text.len()
    } else {
        let extra = new.len() - old.len();
        text.len()
            .saturating_add((text.len() / old.len()).saturating_mul(extra))
    };
    let mut out = String::with_capacity(cap);
    out.push_str(&text[..first]);
    out.push_str(new);
    let mut from = first + old.len();
    while let Some(rel) = search_bytes(&text[from..], old) {
        let pos = from + rel;
        out.push_str(&text[from..pos]);
        out.push_str(new);
        from = pos + old.len();
    }
    out.push_str(&text[from..]);
    out
}

fn replace_nth(text: &str, old: &str, new: &str, n: u32) -> String {
    debug_assert!(!old.is_empty() && old != new);
    let mut from = 0usize;
    let mut seen = 0u32;
    loop {
        let Some(rel) = search_bytes(&text[from..], old) else {
            return text.to_owned();
        };
        let pos = from + rel;
        seen += 1;
        if seen == n {
            return patch_at(text, pos, old.len(), new);
        }
        from = pos + old.len();
    }
}

fn patch_at(text: &str, pos: usize, old_len: usize, new: &str) -> String {
    if old_len == new.len() {
        let mut buf = text.to_owned();
        // SAFETY: equal UTF-8 width overwrite of a previously found span.
        unsafe {
            buf.as_bytes_mut()[pos..pos + old_len].copy_from_slice(new.as_bytes());
        }
        return buf;
    }
    let cap = text.len() - old_len + new.len();
    let mut out = String::with_capacity(cap);
    out.push_str(&text[..pos]);
    out.push_str(new);
    out.push_str(&text[pos + old_len..]);
    out
}

/// `str::find` is already Two-Way/`memchr`. Single ASCII bytes use a SWAR
/// `memchr`; multi-byte ASCII needles probe the last byte (FIND hill-climb).
fn search_bytes(hay: &str, needle: &str) -> Option<usize> {
    if needle.len() == 1 {
        let b = needle.as_bytes()[0];
        if b.is_ascii() {
            return memchr_byte(hay.as_bytes(), b);
        }
    }
    if needle.is_ascii() && needle.len() >= 2 {
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

/// Word-at-a-time ASCII byte swap from `start` (first known hit).
fn swar_replace_byte(buf: &mut [u8], old: u8, new: u8, start: usize) {
    debug_assert!(old.is_ascii() && new.is_ascii());
    debug_assert!(start <= buf.len());
    const W: usize = std::mem::size_of::<usize>();
    let splat_old = usize::from(old).wrapping_mul(usize::from_ne_bytes([0x01; W]));
    let splat_new = usize::from(new).wrapping_mul(usize::from_ne_bytes([0x01; W]));
    let ones = usize::from_ne_bytes([0x01; W]);
    let highs = usize::from_ne_bytes([0x80; W]);
    let mut i = start;
    while i + W <= buf.len() {
        // SAFETY: `i + W <= buf.len()`.
        let word = unsafe { std::ptr::read_unaligned(buf.as_ptr().add(i).cast::<usize>()) };
        let xor = word ^ splat_old;
        let hz = xor.wrapping_sub(ones) & !xor & highs;
        if hz != 0 {
            // `hz` has 0x80 in matching bytes. `>> 7` lands 0x01 in the same
            // byte (bit 8k+7 → bit 8k). Times 0xFF fills that byte.
            let eq = ((hz >> 7) & ones).wrapping_mul(0xFF);
            let replaced = (word & !eq) | (splat_new & eq);
            unsafe {
                std::ptr::write_unaligned(buf.as_mut_ptr().add(i).cast::<usize>(), replaced);
            }
        }
        i += W;
    }
    for b in &mut buf[i..] {
        if *b == old {
            *b = new;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn both(text: &str, old: &str, new: &str, n: Option<u32>) -> String {
        let fast = substitute(text, old, new, n);
        let slow = substitute_naive(text, old, new, n);
        assert_eq!(
            fast, slow,
            "naive/fast mismatch for {text:?} {old:?} {new:?} {n:?}"
        );
        fast
    }

    #[test]
    fn replace_all_and_nth() {
        assert_eq!(both("a-b-c", "-", "/", None), "a/b/c");
        assert_eq!(both("a-b-c", "-", "/", Some(1)), "a/b-c");
        assert_eq!(both("a-b-c", "-", "/", Some(2)), "a-b/c");
        assert_eq!(both("a-b-c", "-", "/", Some(3)), "a-b-c");
    }

    #[test]
    fn microsoft_examples() {
        assert_eq!(both("Sales Data", "Sales", "Cost", None), "Cost Data");
        assert_eq!(
            both("Quarter 1, 2011", "1", "2", Some(1)),
            "Quarter 2, 2011"
        );
        assert_eq!(
            both("Quarter 1, 2011", "1", "2", Some(3)),
            "Quarter 1, 2012"
        );
    }

    #[test]
    fn case_sensitive() {
        assert_eq!(both("ABC", "a", "x", None), "ABC");
        assert_eq!(both("AaA", "A", "x", None), "xax");
    }

    #[test]
    fn empty_old_text_is_noop() {
        assert_eq!(both("abc", "", "x", None), "abc");
        assert_eq!(both("abc", "", "x", Some(1)), "abc");
        assert_eq!(both("", "", "x", None), "");
    }

    #[test]
    fn old_eq_new_is_identity() {
        assert_eq!(both("abc", "a", "a", None), "abc");
        assert_eq!(both("aaa", "a", "a", Some(2)), "aaa");
        assert_eq!(both("foobar", "foo", "foo", None), "foobar");
    }

    #[test]
    fn overlapping_is_non_overlapping_ltr() {
        assert_eq!(both("aaa", "aa", "b", None), "ba");
        assert_eq!(both("aaa", "aa", "b", Some(1)), "ba");
        assert_eq!(both("aaa", "aa", "b", Some(2)), "aaa");
        assert_eq!(both("aaaa", "aa", "b", None), "bb");
        assert_eq!(both("aaaaa", "aa", "b", None), "bba");
    }

    #[test]
    fn replacements_are_not_rescanned() {
        assert_eq!(both("a", "a", "aa", None), "aa");
        assert_eq!(both("aaa", "a", "aa", None), "aaaaaa");
        assert_eq!(both("xx", "x", "xy", None), "xyxy");
        assert_eq!(both("aaa", "aa", "aaa", None), "aaaa");
        assert_eq!(both("aaa", "a", "aa", Some(2)), "aaaa");
    }

    #[test]
    fn delete_and_unicode() {
        assert_eq!(both("a-b-c", "-", "", None), "abc");
        assert_eq!(both("a-b-c", "-", "", Some(2)), "a-bc");
        assert_eq!(both("café", "é", "e", None), "cafe");
        assert_eq!(both("café", "é", "è", None), "cafè");
        assert_eq!(both("日本語", "本", "X", None), "日X語");
        assert_eq!(both("ééé", "é", "e", Some(2)), "éeé");
    }

    #[test]
    fn wildcards_are_literal() {
        assert_eq!(both("a*b*c", "*", "-", None), "a-b-c");
        assert_eq!(both("a?b", "?", "x", None), "axb");
        assert_eq!(both("a~b", "~", "x", None), "axb");
    }

    #[test]
    fn grow_shrink_and_entire() {
        assert_eq!(both("a-a-a", "-", "--", None), "a--a--a");
        assert_eq!(both("foobar", "foo", "bar", None), "barbar");
        assert_eq!(both("abc", "abc", "x", None), "x");
        assert_eq!(both("ab", "abc", "x", None), "ab");
        assert_eq!(both("", "a", "b", None), "");
        assert_eq!(both("a b c", " ", "-", None), "a-b-c");
    }

    #[test]
    fn swar_word_boundaries() {
        // Lengths that are not a multiple of usize so the scalar tail runs.
        let s: String = (0..17)
            .map(|i| if i % 3 == 0 { 'a' } else { 'x' })
            .collect();
        let expect: String = s.chars().map(|c| if c == 'a' { 'b' } else { c }).collect();
        assert_eq!(both(&s, "a", "b", None), expect);

        let miss = "x".repeat(64);
        assert_eq!(both(&miss, "a", "b", None), miss);
        assert_eq!(both(&miss, "-", "", None), miss);

        let almost = "aaa".repeat(40) + "aab";
        assert_eq!(both(&almost, "aab", "X", None), "aaa".repeat(40) + "X");
        assert_eq!(both(&almost, "aab", "X", Some(1)), "aaa".repeat(40) + "X");
        assert_eq!(both(&almost, "aab", "X", Some(2)), almost);
    }

    #[test]
    fn nth_past_end_and_first_miss() {
        assert_eq!(both("hello", "z", "x", None), "hello");
        assert_eq!(both("hello", "z", "x", Some(1)), "hello");
        assert_eq!(both("aaa", "a", "b", Some(4)), "aaa");
    }
}
