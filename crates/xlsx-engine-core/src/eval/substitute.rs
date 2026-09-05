//! Excel `SUBSTITUTE` kernel.
//!
//! Semantics (desktop Excel):
//! - Case-sensitive, no wildcards.
//! - Left-to-right, **non-overlapping** matches. Replacements are not re-scanned.
//! - Empty `old_text` matches nothing — the original text is returned.
//! - `instance_num` is 1-based; omitted means replace every match.
//!
//! Production replace-all specializes ASCII byte swaps, equal-width in-place
//! overwrites, and a single-pass resize. The quadratic `replace_range`
//! baseline lives beside that path so benches can report a before/after.

/// Production `SUBSTITUTE` kernel.
///
/// `instance_num` is 1-based. `None` replaces every non-overlapping match.
pub fn substitute(text: &str, old_text: &str, new_text: &str, instance_num: Option<u32>) -> String {
    if old_text.is_empty() {
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
    if old == new {
        return text.to_owned();
    }
    // ASCII byte→byte: one linear scan, no `find` per hit.
    if let (Some(&o), Some(&n)) = (old.as_bytes().first(), new.as_bytes().first()) {
        if old.len() == 1 && new.len() == 1 && o.is_ascii() && n.is_ascii() {
            return replace_all_ascii_byte(text, o, n);
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
    let mut buf = text.to_owned();
    // SAFETY: `old` and `new` are ASCII. Replacing one ASCII byte with
    // another cannot create invalid UTF-8.
    for b in unsafe { buf.as_bytes_mut() } {
        if *b == old {
            *b = new;
        }
    }
    buf
}

fn replace_all_equal_len(text: &str, old: &str, new: &str) -> String {
    debug_assert_eq!(old.len(), new.len());
    let mut buf = text.to_owned();
    let n = old.len();
    let mut from = 0usize;
    while from < buf.len() {
        let Some(rel) = buf[from..].find(old) else {
            break;
        };
        let pos = from + rel;
        // SAFETY: `old` and `new` are valid UTF-8 of equal byte length, so
        // overwriting a found `old` span with `new` leaves the string valid.
        unsafe {
            buf.as_bytes_mut()[pos..pos + n].copy_from_slice(new.as_bytes());
        }
        from = pos + n;
    }
    buf
}

fn replace_all_resized(text: &str, old: &str, new: &str) -> String {
    let Some(first) = text.find(old) else {
        return text.to_owned();
    };
    // One pass. Shrinking fits in `text.len()`. Growing: reserve every
    // non-overlapping slot so a dense needle does not realloc.
    let cap = if new.len() <= old.len() {
        text.len()
    } else {
        let extra = new.len() - old.len();
        text.len() + (text.len() / old.len()) * extra
    };
    let mut out = String::with_capacity(cap);
    out.push_str(&text[..first]);
    out.push_str(new);
    let mut from = first + old.len();
    while let Some(rel) = text[from..].find(old) {
        let pos = from + rel;
        out.push_str(&text[from..pos]);
        out.push_str(new);
        from = pos + old.len();
    }
    out.push_str(&text[from..]);
    out
}

fn replace_nth(text: &str, old: &str, new: &str, n: u32) -> String {
    let mut from = 0usize;
    let mut seen = 0u32;
    while let Some(rel) = text[from..].find(old) {
        let pos = from + rel;
        seen += 1;
        if seen == n {
            let cap = text.len() + new.len() - old.len();
            let mut out = String::with_capacity(cap);
            out.push_str(&text[..pos]);
            out.push_str(new);
            out.push_str(&text[pos + old.len()..]);
            return out;
        }
        from = pos + old.len();
    }
    text.to_owned()
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
    }

    #[test]
    fn delete_and_unicode() {
        assert_eq!(both("a-b-c", "-", "", None), "abc");
        assert_eq!(both("café", "é", "e", None), "cafe");
        assert_eq!(both("café", "é", "è", None), "cafè");
        assert_eq!(both("日本語", "本", "X", None), "日X語");
    }

    #[test]
    fn wildcards_are_literal() {
        assert_eq!(both("a*b*c", "*", "-", None), "a-b-c");
        assert_eq!(both("a?b", "?", "x", None), "axb");
    }
}
