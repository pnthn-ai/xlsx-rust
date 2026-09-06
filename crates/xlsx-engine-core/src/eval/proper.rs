//! Excel `PROPER` kernel.
//!
//! Microsoft (PROPER function):
//!
//! > Capitalizes the first letter in a text string and any other letters in
//! > text that follow any character other than a letter. Converts all other
//! > letters to lowercase letters.
//!
//! This engine’s “letter” is **ASCII** `A–Z` / `a–z`, the same test
//! [`super::functions`] uses for `UPPER` / `LOWER` (`to_ascii_*`). Non-ASCII
//! scalars (é, ß, CJK, emoji) are left unchanged and count as non-letters, so
//! the next ASCII letter is capitalized (`don't` → `Don'T`, `école` → `éCole`).
//! Apostrophes, hyphens, digits, and punctuation are all word breaks.
//!
//! [`proper`] is the production path: one allocation, then an in-place ASCII
//! byte walk. UTF-8 stays valid because only `A–Z`/`a–z` bytes are rewritten
//! (those never appear as continuation bytes). [`proper_naive`] materializes
//! `Vec<char>` and allocates a temporary `String` per scalar so
//! `cargo bench -p xlsx-engine-core --bench proper` can print before/after.

/// Production `PROPER` kernel.
pub fn proper(text: &str) -> String {
    if text.is_empty() {
        return String::new();
    }
    let mut out = text.to_owned();
    // SAFETY: only ASCII alphabetic bytes are rewritten. In UTF-8 those
    // values never occur inside a multi-byte scalar, so the buffer remains
    // valid UTF-8.
    let bytes = unsafe { out.as_bytes_mut() };
    let mut prev_letter = false;
    for b in bytes {
        if b.is_ascii_alphabetic() {
            *b = if prev_letter {
                b.to_ascii_lowercase()
            } else {
                b.to_ascii_uppercase()
            };
            prev_letter = true;
        } else {
            prev_letter = false;
        }
    }
    out
}

/// `Vec<char>` + per-scalar `to_string` baseline used for the hill-climb bench.
///
/// Same Excel semantics as [`proper`].
pub fn proper_naive(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::new();
    let mut prev_letter = false;
    for c in chars {
        if c.is_ascii_alphabetic() {
            let mapped = if prev_letter {
                c.to_ascii_lowercase()
            } else {
                c.to_ascii_uppercase()
            };
            out.push_str(&mapped.to_string());
            prev_letter = true;
        } else {
            out.push_str(&c.to_string());
            prev_letter = false;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn both(s: &str) -> String {
        let fast = proper(s);
        let slow = proper_naive(s);
        assert_eq!(fast, slow, "naive/fast mismatch for {s:?}");
        fast
    }

    #[test]
    fn microsoft_examples() {
        assert_eq!(both("this is a TITLE"), "This Is A Title");
        assert_eq!(both("2-way street"), "2-Way Street");
        assert_eq!(both("76BudGet"), "76Budget");
    }

    #[test]
    fn apostrophe_is_a_word_break() {
        assert_eq!(both("don't"), "Don'T");
        assert_eq!(both("o'brien"), "O'Brien");
        assert_eq!(both("mary's"), "Mary'S");
        assert_eq!(both("it's"), "It'S");
    }

    #[test]
    fn hyphen_digit_and_punctuation_breaks() {
        assert_eq!(both("mary-jane"), "Mary-Jane");
        assert_eq!(both("anne-marie"), "Anne-Marie");
        assert_eq!(both("2nd floor"), "2Nd Floor");
        assert_eq!(both("...hello"), "...Hello");
        assert_eq!(both("hello...world"), "Hello...World");
        assert_eq!(both("hello_world"), "Hello_World");
    }

    #[test]
    fn flattens_midword_and_acronyms() {
        assert_eq!(both("mcdonald"), "Mcdonald");
        assert_eq!(both("McDonald"), "Mcdonald");
        assert_eq!(both("IBM"), "Ibm");
        assert_eq!(both("HELLO WORLD"), "Hello World");
    }

    #[test]
    fn already_proper_and_empty() {
        assert_eq!(both("Hello World"), "Hello World");
        assert_eq!(both(""), "");
        assert_eq!(both("   "), "   ");
        assert_eq!(both("  hello  "), "  Hello  ");
    }

    #[test]
    fn non_ascii_is_not_a_letter() {
        // Same ASCII-letter rule as UPPER/LOWER in this crate.
        assert_eq!(both("école"), "éCole");
        assert_eq!(both("CAFÉ"), "CafÉ");
        assert_eq!(both("straße"), "StraßE");
        assert_eq!(both("日本語hello"), "日本語Hello");
        assert_eq!(both("hello日本語"), "Hello日本語");
        assert_eq!(both("a😀b"), "A😀B");
    }

    #[test]
    fn bool_and_number_shaped_text() {
        assert_eq!(both("TRUE"), "True");
        assert_eq!(both("FALSE"), "False");
        assert_eq!(both("123"), "123");
        assert_eq!(both("1.5"), "1.5");
    }

    #[test]
    fn large_ascii_roundtrip() {
        let src = "aB-cD'eF 76x ".repeat(4_000);
        let a = proper(&src);
        let b = proper_naive(&src);
        assert_eq!(a, b);
        assert!(a.starts_with("Ab-Cd'Ef 76X "));
        assert_eq!(a.len(), src.len());
    }

    #[test]
    fn formula_dispatch_and_coercion() {
        use crate::eval::eval_formula_in;
        use xlsx_types::{ExcelError, ExcelValue, Workbook};
        let wb = Workbook::default();
        assert_eq!(
            eval_formula_in(&wb, "=PROPER(\"this is a TITLE\")").unwrap(),
            ExcelValue::Text("This Is A Title".into())
        );
        assert_eq!(
            eval_formula_in(&wb, "=PROPER(\"don't\")").unwrap(),
            ExcelValue::Text("Don'T".into())
        );
        assert_eq!(
            eval_formula_in(&wb, "=PROPER(TRUE)").unwrap(),
            ExcelValue::Text("True".into())
        );
        assert_eq!(
            eval_formula_in(&wb, "=PROPER()").unwrap(),
            ExcelValue::Error(ExcelError::Value)
        );
        assert_eq!(
            eval_formula_in(&wb, "=PROPER(\"abc\",1)").unwrap(),
            ExcelValue::Error(ExcelError::Value)
        );
        assert_eq!(
            eval_formula_in(&wb, "=PROPER(1/0)").unwrap(),
            ExcelValue::Error(ExcelError::Div0)
        );
    }
}
