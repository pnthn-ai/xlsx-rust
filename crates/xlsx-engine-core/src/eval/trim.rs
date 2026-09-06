//! Excel `TRIM` kernel.
//!
//! Semantics (desktop Excel / Microsoft docs):
//! - `TRIM(text)` strips **leading and trailing ASCII spaces** (byte `0x20`)
//!   and collapses each internal run of ASCII spaces to a single space.
//! - Only the 7-bit space is removed. Tab (`0x09`), CR/LF, NBSP (`U+00A0`,
//!   `CHAR(160)`), ideographic space, and every other Unicode scalar stay.
//! - UTF-8 lead/continuation bytes are all `>= 0x80`, so a byte walk that
//!   looks only for `0x20` cannot split a scalar.
//!
//! Production path: SWAR space / non-space probes, identity / end-trim-only
//! copies, then a single-allocation run collapse. The `Vec<char>` baseline
//! lives beside that path so benches can report a before/after.

/// Production `TRIM` kernel.
pub fn trim(text: &str) -> String {
    trim_fast(text)
}

/// `Vec<char>` baseline used for the hill-climb bench.
///
/// Same Excel semantics as [`trim`]. Kept so
/// `cargo bench -p xlsx-engine-core --bench trim` can print before/after.
pub fn trim_naive(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut start = 0;
    while start < chars.len() && chars[start] == ' ' {
        start += 1;
    }
    let mut end = chars.len();
    while end > start && chars[end - 1] == ' ' {
        end -= 1;
    }
    let mut out = String::new();
    let mut prev_space = false;
    for &c in &chars[start..end] {
        if c == ' ' {
            if !prev_space {
                out.push(' ');
            }
            prev_space = true;
        } else {
            out.push(c);
            prev_space = false;
        }
    }
    out
}

fn trim_fast(text: &str) -> String {
    let bytes = text.as_bytes();
    if bytes.is_empty() {
        return String::new();
    }
    // Common identity: no ASCII space at all (still copies; ExcelValue owns).
    if find_byte(bytes, b' ').is_none() {
        return text.to_owned();
    }
    let Some(start) = find_non_space(bytes) else {
        return String::new();
    };
    let end = rfind_non_space(bytes).expect("non-space exists") + 1;
    let mid = &bytes[start..end];
    if !has_double_space(mid) {
        // Leading/trailing-only (or already clean). Mid is a UTF-8 slice
        // because we only cut `0x20` bytes.
        return copy_utf8(mid);
    }
    collapse(mid)
}

fn collapse(src: &[u8]) -> String {
    debug_assert!(!src.is_empty());
    debug_assert!(src[0] != b' ' && src[src.len() - 1] != b' ');
    let mut out = Vec::with_capacity(src.len());
    let mut i = 0;
    while i < src.len() {
        if src[i] == b' ' {
            out.push(b' ');
            i += 1;
            match find_non_space(&src[i..]) {
                Some(n) => i += n,
                None => break,
            }
        } else {
            let run = find_byte(&src[i..], b' ').unwrap_or(src.len() - i);
            out.extend_from_slice(&src[i..i + run]);
            i += run;
        }
    }
    // SAFETY: only `0x20` bytes were dropped; remaining bytes are a
    // subsequence of valid UTF-8 and `0x20` is a 1-byte scalar.
    unsafe { String::from_utf8_unchecked(out) }
}

fn copy_utf8(bytes: &[u8]) -> String {
    // SAFETY: `bytes` is a substring of a `&str` cut on `0x20` boundaries.
    unsafe { std::str::from_utf8_unchecked(bytes) }.to_owned()
}

const LO: u64 = 0x0101_0101_0101_0101;
const HI: u64 = 0x8080_8080_8080_8080;
const SP: u64 = 0x2020_2020_2020_2020;

fn find_byte(hay: &[u8], needle: u8) -> Option<usize> {
    let mut i = 0;
    let n = hay.len();
    let splat = u64::from(needle).wrapping_mul(LO);
    while i + 8 <= n {
        let w = u64::from_le_bytes(hay[i..i + 8].try_into().unwrap());
        let x = w ^ splat;
        let mask = x.wrapping_sub(LO) & !x & HI;
        if mask != 0 {
            return Some(i + (mask.trailing_zeros() as usize / 8));
        }
        i += 8;
    }
    hay[i..].iter().position(|&b| b == needle).map(|p| i + p)
}

fn find_non_space(hay: &[u8]) -> Option<usize> {
    let mut i = 0;
    let n = hay.len();
    while i + 8 <= n {
        let w = u64::from_le_bytes(hay[i..i + 8].try_into().unwrap());
        if w != SP {
            for j in 0..8 {
                if hay[i + j] != b' ' {
                    return Some(i + j);
                }
            }
        }
        i += 8;
    }
    hay[i..].iter().position(|&b| b != b' ').map(|p| i + p)
}

fn rfind_non_space(hay: &[u8]) -> Option<usize> {
    let mut i = hay.len();
    while i >= 8 {
        let start = i - 8;
        let w = u64::from_le_bytes(hay[start..i].try_into().unwrap());
        if w != SP {
            for j in (0..8).rev() {
                if hay[start + j] != b' ' {
                    return Some(start + j);
                }
            }
        }
        i = start;
    }
    hay[..i].iter().rposition(|&b| b != b' ')
}

fn has_double_space(hay: &[u8]) -> bool {
    let mut i = 0;
    while let Some(p) = find_byte(&hay[i..], b' ') {
        let abs = i + p;
        if abs + 1 < hay.len() && hay[abs + 1] == b' ' {
            return true;
        }
        i = abs + 1;
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::eval::eval_formula_in;
    use xlsx_types::{Cell, ExcelError, ExcelValue, Sheet, Workbook};

    fn both(s: &str) -> String {
        let fast = trim(s);
        let slow = trim_naive(s);
        assert_eq!(fast, slow, "naive/fast mismatch for {s:?}");
        fast
    }

    #[test]
    fn microsoft_example() {
        assert_eq!(both("  First Quarter Earnings  "), "First Quarter Earnings");
    }

    #[test]
    fn collapse_and_ends() {
        assert_eq!(both(""), "");
        assert_eq!(both("   "), "");
        assert_eq!(both(" "), "");
        assert_eq!(both("a"), "a");
        assert_eq!(both("a b"), "a b");
        assert_eq!(both("  a  b  "), "a b");
        assert_eq!(both("a   b   c"), "a b c");
        assert_eq!(both("   hello"), "hello");
        assert_eq!(both("hello   "), "hello");
        assert_eq!(both("already clean"), "already clean");
    }

    #[test]
    fn only_ascii_space_is_removed() {
        assert_eq!(both("\ta\t"), "\ta\t");
        assert_eq!(both(" \ta\t "), "\ta\t");
        assert_eq!(both("a\t\tb"), "a\t\tb");
        assert_eq!(both("a  \t  b"), "a \t b");
        assert_eq!(both("\n\n"), "\n\n");
        assert_eq!(both(" \n a \n "), "\n a \n");
        assert_eq!(both("\u{00a0}"), "\u{00a0}");
        assert_eq!(both("  \u{00a0}\u{00a0}  "), "\u{00a0}\u{00a0}");
        assert_eq!(both("a\u{00a0}  b"), "a\u{00a0} b");
        assert_eq!(both(" \u{3000} "), "\u{3000}");
        assert_eq!(both("\t\t"), "\t\t");
    }

    #[test]
    fn unicode_scalars_stay_intact() {
        assert_eq!(both("  café  "), "café");
        assert_eq!(both("  日本語  "), "日本語");
        assert_eq!(both("  😀  🎉  "), "😀 🎉");
        assert_eq!(both("café  café"), "café café");
    }

    #[test]
    fn identity_and_large() {
        let clean = "x".repeat(4096);
        assert_eq!(both(&clean), clean);
        let spaces = " ".repeat(4096);
        assert_eq!(both(&spaces), "");
        let doubled = "a  ".repeat(1024);
        assert_eq!(both(&doubled), "a ".repeat(1023) + "a");
        let mixed = format!("{}café  ", "x".repeat(100));
        assert_eq!(both(&mixed), format!("{}café", "x".repeat(100)));
    }

    #[test]
    fn formula_coercion_and_arity() {
        let wb = Workbook::default();
        assert_eq!(
            eval_formula_in(&wb, "=TRIM(\"  a  b  \")").unwrap(),
            ExcelValue::Text("a b".into())
        );
        assert_eq!(
            eval_formula_in(&wb, "=TRIM(123)").unwrap(),
            ExcelValue::Text("123".into())
        );
        assert_eq!(
            eval_formula_in(&wb, "=TRIM(1.5)").unwrap(),
            ExcelValue::Text("1.5".into())
        );
        assert_eq!(
            eval_formula_in(&wb, "=TRIM(TRUE)").unwrap(),
            ExcelValue::Text("TRUE".into())
        );
        assert_eq!(
            eval_formula_in(&wb, "=TRIM(FALSE)").unwrap(),
            ExcelValue::Text("FALSE".into())
        );
        assert_eq!(
            eval_formula_in(&wb, "=TRIM()").unwrap(),
            ExcelValue::Error(ExcelError::Value)
        );
        assert_eq!(
            eval_formula_in(&wb, "=TRIM(\"a\",\"b\")").unwrap(),
            ExcelValue::Error(ExcelError::Value)
        );
        assert_eq!(
            eval_formula_in(&wb, "=TRIM(1/0)").unwrap(),
            ExcelValue::Error(ExcelError::Div0)
        );
        assert_eq!(
            eval_formula_in(&wb, "=TRIM(NA())").unwrap(),
            ExcelValue::Error(ExcelError::Na)
        );
        assert_eq!(
            eval_formula_in(&wb, "=TRIM(TRIM(\"  a  b  \"))").unwrap(),
            ExcelValue::Text("a b".into())
        );
        assert_eq!(
            eval_formula_in(&wb, "=LEN(TRIM(\"  a  b  \"))").unwrap(),
            ExcelValue::Number(3.0)
        );
    }

    #[test]
    fn formula_blank_and_cell() {
        let mut sheet = Sheet::new("Sheet1");
        sheet.cells.insert(
            "A1".into(),
            Cell::value(ExcelValue::Text("  x  y  ".into())),
        );
        sheet
            .cells
            .insert("A2".into(), Cell::value(ExcelValue::Empty));
        sheet
            .cells
            .insert("A3".into(), Cell::value(ExcelValue::Text(String::new())));
        let wb = Workbook {
            sheets: vec![sheet],
            names: vec![],
        };
        assert_eq!(
            eval_formula_in(&wb, "=TRIM(A1)").unwrap(),
            ExcelValue::Text("x y".into())
        );
        assert_eq!(
            eval_formula_in(&wb, "=TRIM(A2)").unwrap(),
            ExcelValue::Text(String::new())
        );
        assert_eq!(
            eval_formula_in(&wb, "=TRIM(A3)").unwrap(),
            ExcelValue::Text(String::new())
        );
        assert_eq!(
            eval_formula_in(&wb, "=TRIM(B1)").unwrap(),
            ExcelValue::Text(String::new())
        );
    }

    #[test]
    fn swar_helpers_on_short_and_long() {
        assert_eq!(find_byte(b"", b' '), None);
        assert_eq!(find_byte(b"abc", b' '), None);
        assert_eq!(find_byte(b"ab c", b' '), Some(2));
        assert_eq!(find_byte(b"1234567 ", b' '), Some(7));
        assert_eq!(find_byte(b"12345678 ", b' '), Some(8));
        assert_eq!(find_non_space(b"   abc"), Some(3));
        assert_eq!(find_non_space(b"        x"), Some(8));
        assert_eq!(find_non_space(b"        "), None);
        assert_eq!(rfind_non_space(b"abc   "), Some(2));
        assert_eq!(rfind_non_space(b"x        "), Some(0));
        assert!(!has_double_space(b"a b c"));
        assert!(has_double_space(b"a  b"));
        assert!(has_double_space(b"  "));
        assert!(!has_double_space(b" "));
    }
}
