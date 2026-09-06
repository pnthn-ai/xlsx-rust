//! Excel `CLEAN` kernel.
//!
//! Desktop Excel removes the first 32 non-printing 7-bit ASCII characters
//! (code points `0` through `31`) and leaves everything else, including
//! the additional Unicode non-printables Microsoft documents as *not*
//! stripped: `127`, `129`, `141`, `143`, `144`, and `157`. Space (`32`),
//! NBSP (`160`), and other Unicode scalars stay.
//!
//! Those C0 bytes are never part of a multi-byte UTF-8 sequence, so the
//! production path is a byte scan (SWAR 8-wide) plus run copies. The
//! `Vec<char>` filter baseline lives beside it so benches can print
//! before/after.

/// Production `CLEAN` kernel.
pub fn clean(text: &str) -> String {
    match first_c0(text.as_bytes()) {
        None => text.to_owned(),
        Some(first) => clean_from(text, first),
    }
}

/// Like [`clean`], but keeps `text` when it has no C0 bytes.
pub fn clean_owned(text: String) -> String {
    match first_c0(text.as_bytes()) {
        None => text,
        Some(first) => clean_from(&text, first),
    }
}

/// Quadratic baseline: materialize every Unicode scalar, then keep `>= 32`.
///
/// Same Excel semantics as [`clean`]. Kept so
/// `cargo bench -p xlsx-engine-core --bench clean` can print before/after.
pub fn clean_naive(text: &str) -> String {
    text.chars().filter(|&c| (c as u32) >= 32).collect()
}

fn clean_from(text: &str, first: usize) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len() - 1);
    out.extend_from_slice(&bytes[..first]);
    let mut i = first + 1;
    // 8-wide: memcpy a clean chunk, else keep bytes `>= 32` one by one.
    // Avoids restarting a SWAR scan after every 1-byte run (dense C0).
    while i + 8 <= bytes.len() {
        let chunk = u64::from_ne_bytes(bytes[i..i + 8].try_into().unwrap());
        if chunk_has_c0(chunk) {
            for &b in &bytes[i..i + 8] {
                if b >= 32 {
                    out.push(b);
                }
            }
        } else {
            out.extend_from_slice(&bytes[i..i + 8]);
        }
        i += 8;
    }
    for &b in &bytes[i..] {
        if b >= 32 {
            out.push(b);
        }
    }
    // SAFETY: every dropped byte is `< 32` (a standalone C0 code unit).
    // Remaining bytes are the original UTF-8 with those units removed.
    unsafe { String::from_utf8_unchecked(out) }
}

/// Index of the first ASCII C0 byte (`0..=31`), if any.
fn first_c0(bytes: &[u8]) -> Option<usize> {
    let mut i = 0;
    while i + 8 <= bytes.len() {
        let chunk = u64::from_ne_bytes(bytes[i..i + 8].try_into().unwrap());
        if chunk_has_c0(chunk) {
            for j in 0..8 {
                if bytes[i + j] < 32 {
                    return Some(i + j);
                }
            }
        }
        i += 8;
    }
    bytes[i..].iter().position(|&b| b < 32).map(|p| i + p)
}

/// True when any byte of `chunk` is in `0..=31`.
///
/// C0 bytes have the top 3 bits clear. Mask those bits; a zero byte in
/// the result is a C0, detected with the usual SWAR zero-byte test.
#[inline]
fn chunk_has_c0(chunk: u64) -> bool {
    let high = chunk & 0xE0E0_E0E0_E0E0_E0E0;
    high.wrapping_sub(0x0101_0101_0101_0101) & !high & 0x8080_8080_8080_8080 != 0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn both(text: &str) -> String {
        let fast = clean(text);
        let owned = clean_owned(text.to_owned());
        let slow = clean_naive(text);
        assert_eq!(fast, slow, "naive/fast mismatch for {text:?}");
        assert_eq!(owned, slow, "owned/fast mismatch for {text:?}");
        fast
    }

    #[test]
    fn microsoft_monthly_report() {
        // CHAR(9)&"Monthly report"&CHAR(10)
        assert_eq!(both("\tMonthly report\n"), "Monthly report");
    }

    #[test]
    fn already_clean_is_identity() {
        assert_eq!(both("hello"), "hello");
        assert_eq!(both(""), "");
        assert_eq!(both("a b"), "a b");
    }

    #[test]
    fn removes_each_c0() {
        for n in 0u8..=31 {
            let s = format!("a{}b", n as char);
            assert_eq!(both(&s), "ab", "CHAR({n})");
        }
    }

    #[test]
    fn all_c0_is_empty() {
        let dirty: String = (0u8..=31).map(|n| n as char).collect();
        assert_eq!(both(&dirty), "");
    }

    #[test]
    fn keeps_space_and_del() {
        assert_eq!(both(" a "), " a ");
        assert_eq!(both("a\u{7f}b"), "a\u{7f}b");
    }

    #[test]
    fn keeps_documented_unicode_nonprintables() {
        for n in [127u32, 129, 141, 143, 144, 157] {
            let ch = char::from_u32(n).unwrap();
            let s = format!("x{ch}y");
            assert_eq!(both(&s), s, "U+{n:04X}");
        }
        assert_eq!(both("a\u{00a0}b"), "a\u{00a0}b");
        assert_eq!(both("a\u{200b}b"), "a\u{200b}b");
    }

    #[test]
    fn consecutive_and_edges() {
        assert_eq!(both("\x01\x02ab\x03\x04"), "ab");
        assert_eq!(both("\x07hello\x07"), "hello");
        assert_eq!(both("\r\n"), "");
    }

    #[test]
    fn unicode_scalars_untouched() {
        assert_eq!(both("café"), "café");
        assert_eq!(both("日本語"), "日本語");
        assert_eq!(both("a😀b"), "a😀b");
        assert_eq!(both("e\u{0301}"), "e\u{0301}");
        assert_eq!(both("café\u{0007}!"), "café!");
        assert_eq!(both("日\n本"), "日本");
    }

    #[test]
    fn swar_chunk_boundaries() {
        // 8-byte SWAR: C0 just before / on / after an 8-byte boundary.
        let mut s = "abcdefgh".to_string();
        s.push('\u{0001}');
        s.push_str("ijklmnop");
        assert_eq!(both(&s), "abcdefghijklmnop");
        assert_eq!(both("1234567\u{0002}9"), "12345679");
        assert_eq!(both("\u{0003}12345678"), "12345678");
    }
}
