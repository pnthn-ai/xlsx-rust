//! Excel `VALUE` kernel (en-US number / date / time text).
//!
//! Desktop Excel / Microsoft VALUE help (no golden-reading):
//! - `VALUE(text)` converts a text string in a **constant number, date, or
//!   time format** Excel would accept when typed into a cell.
//! - Microsoft examples: `VALUE("$1,000")` → `1000`;
//!   `VALUE("16:48:00")-VALUE("12:00:00")` → `0.2`.
//! - Locale is **en-US**: `.` decimal, `,` thousands (groups of 3), `$`
//!   currency, `M/D/Y` dates. Other locales are not implemented.
//!
//! Number formats:
//! - Trim ASCII whitespace (`0x20` / tab / CR / LF) only. NBSP stays and
//!   is `#VALUE!`.
//! - Sign `+`/`-`, accounting `(…)`, optional `$`, trailing `%` (each `%`
//!   divides by 100). `"50%"` → `0.5`; `"($1,234.50)"` → `-1234.5`.
//! - Thousands commas are validated (first group 1–3 digits, then groups
//!   of 3). `"1,23"` / `"1,2345"` are `#VALUE!`.
//! - Scientific `1E3` / `1.5e-2`. Commas + exponent together are `#VALUE!`.
//! - Empty / spaces-only / `"TRUE"` text / other junk → `#VALUE!`.
//! - Overflow to non-finite → `#NUM!`.
//!
//! Dates / times (1900 leap-year bug via [`date_serial`]):
//! - `M/D/YYYY`, `M-D-YYYY`, `YYYY-MM-DD` (4-digit year first). Two-digit
//!   years use the Windows 00–29 → 2000–2029 / 30–99 → 1930–1999 window.
//! - Invalid civil days (including `2/30/2020`) are `#VALUE!` — no `DATE`
//!   month overflow.
//! - Incomplete dates (`1/2` with no year) and month names are **not**
//!   implemented (current-year / locale names); those strings are `#VALUE!`.
//! - Times `H:MM` / `H:MM:SS`[.frac]; optional `AM`/`PM`. Minutes /
//!   seconds must be `< 60`. Date + time: `"1/1/2020 16:48:00"`.
//! - Mixed fractions `"1 1/2"` → `1.5`. Bare `"1/2"` is not a fraction
//!   (Excel would treat it as a current-year date).
//!
//! Production path is a no-alloc byte walk (stack buffer when commas are
//! stripped). The allocating cleanup baseline lives beside it so benches
//! can print before/after.

use super::{Ctx, Evaluator};
use crate::ast::Expr;
use crate::dates::{date_serial, days_in_month};
use xlsx_types::{DateSystem, EvalError, ExcelError, ExcelValue};

/// `VALUE(text)` — scalar context, wrong arity → `#VALUE!`.
pub(crate) fn eval(
    ev: &Evaluator,
    args: &[Expr],
    ctx: &mut Ctx<'_>,
) -> Result<ExcelValue, EvalError> {
    if args.len() != 1 {
        return Ok(ExcelValue::Error(ExcelError::Value));
    }
    let v = ev.eval_scalar(&args[0], ctx)?;
    match v {
        ExcelValue::Number(n) => Ok(ExcelValue::Number(n)),
        ExcelValue::Bool(true) => Ok(ExcelValue::Number(1.0)),
        ExcelValue::Bool(false) => Ok(ExcelValue::Number(0.0)),
        ExcelValue::Empty => Ok(ExcelValue::Number(0.0)),
        ExcelValue::Text(s) => match parse(&s, ctx.spec.options.date_system) {
            Ok(n) => Ok(ExcelValue::Number(n)),
            Err(e) => Ok(ExcelValue::Error(e)),
        },
        ExcelValue::Error(e) => Ok(ExcelValue::Error(e)),
        ExcelValue::Array(_) => Ok(ExcelValue::Error(ExcelError::Value)),
    }
}

/// Production `VALUE` text parser.
pub fn parse(text: &str, system: DateSystem) -> Result<f64, ExcelError> {
    parse_fast(text, system)
}

/// Allocating cleanup baseline used for the hill-climb bench.
///
/// Same Excel semantics as [`parse`]. Kept so
/// `cargo bench -p xlsx-engine-core --bench value` can print before/after.
pub fn parse_naive(text: &str, system: DateSystem) -> Result<f64, ExcelError> {
    let t = ascii_trim(text);
    if t.is_empty() {
        return Err(ExcelError::Value);
    }
    if t.as_bytes().contains(&b':') {
        return parse_datetime_naive(t, system);
    }
    if let Some(n) = try_mixed_fraction_naive(t) {
        return n;
    }
    if looks_like_date(t.as_bytes()) {
        return parse_date_only_naive(t, system);
    }
    parse_number_naive(t)
}

fn parse_fast(text: &str, system: DateSystem) -> Result<f64, ExcelError> {
    let t = ascii_trim(text);
    if t.is_empty() {
        return Err(ExcelError::Value);
    }
    let b = t.as_bytes();
    if b.contains(&b':') {
        return parse_datetime_fast(b, system);
    }
    if let Some(n) = try_mixed_fraction_fast(b) {
        return n;
    }
    if looks_like_date(b) {
        return parse_date_only_fast(b, system);
    }
    parse_number_fast(b)
}

fn ascii_trim(s: &str) -> &str {
    let b = s.as_bytes();
    let mut start = 0;
    let mut end = b.len();
    while start < end && is_ascii_ws(b[start]) {
        start += 1;
    }
    while end > start && is_ascii_ws(b[end - 1]) {
        end -= 1;
    }
    // SAFETY: we only skip ASCII whitespace bytes.
    unsafe { std::str::from_utf8_unchecked(&b[start..end]) }
}

fn is_ascii_ws(c: u8) -> bool {
    matches!(c, b' ' | b'\t' | b'\n' | b'\r')
}

fn looks_like_date(b: &[u8]) -> bool {
    if b.contains(&b'/') {
        return true;
    }
    // A date dash sits between digits (`2020-01-01`, `1-1-2020`). A leading
    // minus, `$-100`, or scientific `1E-3` is not a date.
    for (i, &c) in b.iter().enumerate() {
        if c == b'-'
            && i > 0
            && i + 1 < b.len()
            && b[i - 1].is_ascii_digit()
            && b[i + 1].is_ascii_digit()
        {
            return true;
        }
    }
    false
}

// ---------------------------------------------------------------------------
// Number formats
// ---------------------------------------------------------------------------

fn parse_number_naive(t: &str) -> Result<f64, ExcelError> {
    let mut s = t.to_owned();
    let mut neg = false;
    if s.starts_with('(') && s.ends_with(')') && s.len() >= 2 {
        neg = true;
        s = s[1..s.len() - 1].to_owned();
        s = ascii_trim(&s).to_owned();
    }
    take_sign_owned(&mut s, &mut neg);
    if s.starts_with('$') {
        s.remove(0);
        take_sign_owned(&mut s, &mut neg);
    }
    let mut pct = 0u32;
    while s.ends_with('%') {
        pct += 1;
        s.pop();
        s = ascii_trim(&s).to_owned();
    }
    if s.is_empty() {
        return Err(ExcelError::Value);
    }
    if s.contains(',') {
        if s.contains('e') || s.contains('E') {
            return Err(ExcelError::Value);
        }
        if !comma_groups_ok(s.as_bytes()) {
            return Err(ExcelError::Value);
        }
        s.retain(|c| c != ',');
    }
    let mut n = parse_f64_strict(&s)?;
    if neg {
        n = -n;
    }
    for _ in 0..pct {
        n /= 100.0;
    }
    finite_or_num(n)
}

fn take_sign_owned(s: &mut String, neg: &mut bool) {
    if s.starts_with('+') {
        s.remove(0);
    } else if s.starts_with('-') {
        *neg = !*neg;
        s.remove(0);
    }
}

fn parse_number_fast(b: &[u8]) -> Result<f64, ExcelError> {
    let n = b.len();
    let mut i = 0;
    let mut end = n;
    let mut neg = false;
    if n >= 2 && b[0] == b'(' && b[n - 1] == b')' {
        neg = true;
        i = 1;
        end = n - 1;
        while i < end && is_ascii_ws(b[i]) {
            i += 1;
        }
        while end > i && is_ascii_ws(b[end - 1]) {
            end -= 1;
        }
    }
    take_sign(b, &mut i, end, &mut neg);
    if i < end && b[i] == b'$' {
        i += 1;
        take_sign(b, &mut i, end, &mut neg);
    }
    let mut pct = 0u32;
    while end > i && b[end - 1] == b'%' {
        pct += 1;
        end -= 1;
        while end > i && is_ascii_ws(b[end - 1]) {
            end -= 1;
        }
    }
    if i >= end {
        return Err(ExcelError::Value);
    }
    let body = &b[i..end];
    let mut n = parse_plain_number(body)?;
    if neg {
        n = -n;
    }
    for _ in 0..pct {
        n /= 100.0;
    }
    finite_or_num(n)
}

fn take_sign(b: &[u8], i: &mut usize, end: usize, neg: &mut bool) {
    if *i < end && (b[*i] == b'+' || b[*i] == b'-') {
        if b[*i] == b'-' {
            *neg = !*neg;
        }
        *i += 1;
    }
}

fn parse_plain_number(body: &[u8]) -> Result<f64, ExcelError> {
    if body.is_empty() {
        return Err(ExcelError::Value);
    }
    if body.contains(&b',') {
        if body.iter().any(|&c| c == b'e' || c == b'E') {
            return Err(ExcelError::Value);
        }
        if !comma_groups_ok(body) {
            return Err(ExcelError::Value);
        }
        let mut buf = [0u8; 64];
        let mut w = 0usize;
        for &c in body {
            if c == b',' {
                continue;
            }
            if w >= buf.len() {
                return Err(ExcelError::Value);
            }
            buf[w] = c;
            w += 1;
        }
        let s = std::str::from_utf8(&buf[..w]).map_err(|_| ExcelError::Value)?;
        return parse_f64_strict(s);
    }
    let s = std::str::from_utf8(body).map_err(|_| ExcelError::Value)?;
    parse_f64_strict(s)
}

fn comma_groups_ok(body: &[u8]) -> bool {
    let mut i = 0;
    while i < body.len() && body[i] != b'.' && body[i] != b'e' && body[i] != b'E' {
        i += 1;
    }
    integer_commas_ok(&body[..i])
}

fn integer_commas_ok(intp: &[u8]) -> bool {
    if intp.is_empty() {
        return true;
    }
    if !intp.contains(&b',') {
        return intp.iter().all(|&c| c.is_ascii_digit());
    }
    let mut it = intp.split(|&c| c == b',');
    let Some(first) = it.next() else {
        return false;
    };
    if first.is_empty() || first.len() > 3 || !first.iter().all(|c| c.is_ascii_digit()) {
        return false;
    }
    let mut saw_rest = false;
    for part in it {
        saw_rest = true;
        if part.len() != 3 || !part.iter().all(|c| c.is_ascii_digit()) {
            return false;
        }
    }
    saw_rest
}

fn parse_f64_strict(s: &str) -> Result<f64, ExcelError> {
    if s.is_empty() || s == "+" || s == "-" || s == "." || s == "+." || s == "-." {
        return Err(ExcelError::Value);
    }
    let n: f64 = s.parse().map_err(|_| ExcelError::Value)?;
    finite_or_num(n)
}

fn finite_or_num(n: f64) -> Result<f64, ExcelError> {
    if n.is_finite() {
        Ok(n)
    } else {
        Err(ExcelError::Num)
    }
}

// ---------------------------------------------------------------------------
// Mixed fractions (`1 1/2`)
// ---------------------------------------------------------------------------

fn try_mixed_fraction_naive(t: &str) -> Option<Result<f64, ExcelError>> {
    try_mixed_fraction_fast(t.as_bytes())
}

fn try_mixed_fraction_fast(b: &[u8]) -> Option<Result<f64, ExcelError>> {
    let (body, neg) = strip_paren_sign(b)?;
    // whole SP num / den — exactly one space run, one slash, digits only.
    let mut space = None;
    let mut slash = None;
    for (i, &c) in body.iter().enumerate() {
        match c {
            b' ' => {
                if space.is_some() {
                    return None;
                }
                space = Some(i);
            }
            b'/' => {
                if slash.is_some() {
                    return None;
                }
                slash = Some(i);
            }
            b'0'..=b'9' => {}
            _ => return None,
        }
    }
    let space = space?;
    let slash = slash?;
    if slash < space {
        return None;
    }
    let whole = &body[..space];
    let num = &body[space + 1..slash];
    let den = &body[slash + 1..];
    if whole.is_empty() || num.is_empty() || den.is_empty() {
        return Some(Err(ExcelError::Value));
    }
    Some(mixed_fraction_value(whole, num, den, neg))
}

fn strip_paren_sign(b: &[u8]) -> Option<(&[u8], bool)> {
    let mut i = 0;
    let mut end = b.len();
    let mut neg = false;
    if end >= 2 && b[0] == b'(' && b[end - 1] == b')' {
        neg = true;
        i = 1;
        end -= 1;
    }
    if i < end && (b[i] == b'+' || b[i] == b'-') {
        if b[i] == b'-' {
            neg = !neg;
        }
        i += 1;
    }
    if i >= end {
        return None;
    }
    Some((&b[i..end], neg))
}

fn mixed_fraction_value(
    whole: &[u8],
    num: &[u8],
    den: &[u8],
    neg: bool,
) -> Result<f64, ExcelError> {
    let w = parse_u32_bytes(whole)? as f64;
    let n = parse_u32_bytes(num)? as f64;
    let d = parse_u32_bytes(den)? as f64;
    if d == 0.0 {
        return Err(ExcelError::Value);
    }
    let v = w + n / d;
    finite_or_num(if neg { -v } else { v })
}

fn parse_u32_bytes(b: &[u8]) -> Result<u32, ExcelError> {
    if b.is_empty() {
        return Err(ExcelError::Value);
    }
    let mut n: u32 = 0;
    for &c in b {
        if !c.is_ascii_digit() {
            return Err(ExcelError::Value);
        }
        n = n
            .checked_mul(10)
            .and_then(|n| n.checked_add((c - b'0') as u32))
            .ok_or(ExcelError::Value)?;
    }
    Ok(n)
}

// ---------------------------------------------------------------------------
// Dates / times
// ---------------------------------------------------------------------------

fn parse_datetime_naive(t: &str, system: DateSystem) -> Result<f64, ExcelError> {
    parse_datetime_fast(t.as_bytes(), system)
}

fn parse_date_only_naive(t: &str, system: DateSystem) -> Result<f64, ExcelError> {
    parse_date_only_fast(t.as_bytes(), system)
}

fn parse_datetime_fast(b: &[u8], system: DateSystem) -> Result<f64, ExcelError> {
    if let Some(split) = date_time_split(b) {
        let (date, time) = split;
        let serial = parse_date_only_fast(date, system)?;
        let frac = parse_time_fast(time)?;
        return finite_or_num(serial + frac);
    }
    parse_time_fast(b)
}

fn date_time_split(b: &[u8]) -> Option<(&[u8], &[u8])> {
    // First ASCII space (or `T` between digits) separates date from time.
    if let Some(i) = b.iter().position(|&c| c == b' ') {
        let date = &b[..i];
        let mut j = i + 1;
        while j < b.len() && b[j] == b' ' {
            j += 1;
        }
        if looks_like_date(date) && j < b.len() {
            return Some((date, &b[j..]));
        }
        return None;
    }
    if let Some(i) = b.iter().position(|&c| c == b'T') {
        if i > 0 && i + 1 < b.len() && b[i - 1].is_ascii_digit() && b[i + 1].is_ascii_digit() {
            return Some((&b[..i], &b[i + 1..]));
        }
    }
    None
}

fn parse_date_only_fast(b: &[u8], system: DateSystem) -> Result<f64, ExcelError> {
    let (y, m, d) = parse_ymd(b)?;
    date_serial(y, m, d, system).map_err(|_| ExcelError::Value)
}

fn parse_ymd(b: &[u8]) -> Result<(i32, i32, i32), ExcelError> {
    let slash = b.contains(&b'/');
    let dash = b.iter().any(|&c| c == b'-');
    if slash && dash {
        return Err(ExcelError::Value);
    }
    let sep = if slash {
        b'/'
    } else if dash {
        b'-'
    } else {
        return Err(ExcelError::Value);
    };
    let mut parts = [0i32; 3];
    let mut lens = [0u8; 3];
    let mut nparts = 0usize;
    let mut cur: i32 = 0;
    let mut clen: u8 = 0;
    let mut saw_digit = false;
    for &c in b {
        if c == sep {
            if !saw_digit || nparts >= 3 {
                return Err(ExcelError::Value);
            }
            parts[nparts] = cur;
            lens[nparts] = clen;
            nparts += 1;
            cur = 0;
            clen = 0;
            saw_digit = false;
        } else if c.is_ascii_digit() {
            saw_digit = true;
            clen = clen.saturating_add(1);
            cur = cur
                .checked_mul(10)
                .and_then(|n| n.checked_add((c - b'0') as i32))
                .ok_or(ExcelError::Value)?;
        } else {
            return Err(ExcelError::Value);
        }
    }
    if !saw_digit || nparts != 2 {
        return Err(ExcelError::Value);
    }
    parts[2] = cur;
    lens[2] = clen;
    let (y, m, d) = if lens[0] == 4 || parts[0] >= 100 {
        (parts[0], parts[1], parts[2])
    } else {
        (expand_year(parts[2]), parts[0], parts[1])
    };
    if !(1..=12).contains(&m) {
        return Err(ExcelError::Value);
    }
    let dim = days_in_month(y, m);
    if d < 1 || d > dim {
        return Err(ExcelError::Value);
    }
    Ok((y, m, d))
}

fn expand_year(y: i32) -> i32 {
    if (0..100).contains(&y) {
        if y <= 29 {
            2000 + y
        } else {
            1900 + y
        }
    } else {
        y
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Ampm {
    None,
    Am,
    Pm,
}

fn parse_time_fast(b: &[u8]) -> Result<f64, ExcelError> {
    let (body, ampm) = strip_ampm(b)?;
    let mut parts: [&[u8]; 3] = [&[]; 3];
    let mut nparts = 0usize;
    let mut start = 0usize;
    for (i, &c) in body.iter().enumerate() {
        if c == b':' {
            if nparts >= 3 {
                return Err(ExcelError::Value);
            }
            parts[nparts] = &body[start..i];
            nparts += 1;
            start = i + 1;
        }
    }
    if nparts < 1 || nparts > 2 {
        return Err(ExcelError::Value);
    }
    if start >= body.len() {
        return Err(ExcelError::Value);
    }
    parts[nparts] = &body[start..];
    nparts += 1;
    if nparts < 2 {
        return Err(ExcelError::Value);
    }
    let h = parse_time_hours(parts[0])?;
    let m = parse_time_minutes(parts[1])?;
    let s = if nparts == 3 {
        parse_time_seconds(parts[2])?
    } else {
        0.0
    };
    let h = apply_ampm(h, ampm)?;
    // VALUE does **not** wrap at 24h the way `TIME` does: `"24:00:00"` is 1,
    // not 0. Minutes / seconds were already rejected at 60.
    if !h.is_finite() || !m.is_finite() || !s.is_finite() {
        return Err(ExcelError::Value);
    }
    if h < 0.0 {
        return Err(ExcelError::Value);
    }
    if h.abs() >= 32767.0 {
        return Err(ExcelError::Value);
    }
    finite_or_num((h * 3600.0 + m * 60.0 + s) / 86_400.0)
}

fn parse_time_hours(b: &[u8]) -> Result<f64, ExcelError> {
    let s = std::str::from_utf8(b).map_err(|_| ExcelError::Value)?;
    let n = parse_f64_strict(s)?;
    if n < 0.0 || n.fract() != 0.0 {
        return Err(ExcelError::Value);
    }
    Ok(n)
}

fn parse_time_minutes(b: &[u8]) -> Result<f64, ExcelError> {
    let s = std::str::from_utf8(b).map_err(|_| ExcelError::Value)?;
    let n = parse_f64_strict(s)?;
    if n < 0.0 || n >= 60.0 || n.fract() != 0.0 {
        return Err(ExcelError::Value);
    }
    Ok(n)
}

fn parse_time_seconds(b: &[u8]) -> Result<f64, ExcelError> {
    let s = std::str::from_utf8(b).map_err(|_| ExcelError::Value)?;
    let n = parse_f64_strict(s)?;
    if n < 0.0 || n >= 60.0 {
        return Err(ExcelError::Value);
    }
    Ok(n)
}

fn apply_ampm(h: f64, ampm: Ampm) -> Result<f64, ExcelError> {
    match ampm {
        Ampm::None => Ok(h),
        Ampm::Am => {
            if !(0.0..=12.0).contains(&h) {
                return Err(ExcelError::Value);
            }
            Ok(if h == 12.0 { 0.0 } else { h })
        }
        Ampm::Pm => {
            if !(0.0..=12.0).contains(&h) {
                return Err(ExcelError::Value);
            }
            Ok(if h == 12.0 { 12.0 } else { h + 12.0 })
        }
    }
}

fn strip_ampm(b: &[u8]) -> Result<(&[u8], Ampm), ExcelError> {
    let t = trim_bytes(b);
    if let Some(rest) = strip_suffix_ignore_ascii(t, b"AM") {
        return Ok((trim_bytes(rest), Ampm::Am));
    }
    if let Some(rest) = strip_suffix_ignore_ascii(t, b"PM") {
        return Ok((trim_bytes(rest), Ampm::Pm));
    }
    if let Some(rest) = strip_suffix_ignore_ascii(t, b"A.M.") {
        return Ok((trim_bytes(rest), Ampm::Am));
    }
    if let Some(rest) = strip_suffix_ignore_ascii(t, b"P.M.") {
        return Ok((trim_bytes(rest), Ampm::Pm));
    }
    Ok((t, Ampm::None))
}

fn trim_bytes(b: &[u8]) -> &[u8] {
    let mut start = 0;
    let mut end = b.len();
    while start < end && is_ascii_ws(b[start]) {
        start += 1;
    }
    while end > start && is_ascii_ws(b[end - 1]) {
        end -= 1;
    }
    &b[start..end]
}

fn strip_suffix_ignore_ascii<'a>(hay: &'a [u8], needle: &[u8]) -> Option<&'a [u8]> {
    if hay.len() < needle.len() {
        return None;
    }
    let start = hay.len() - needle.len();
    for i in 0..needle.len() {
        if hay[start + i].to_ascii_uppercase() != needle[i] {
            return None;
        }
    }
    // Do not steal the last digit of `12:00AM` — AM/PM may sit flush.
    Some(&hay[..start])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::eval::eval_formula_in;
    use xlsx_types::{Cell, ExcelValue, Sheet, Workbook};

    fn both(s: &str) -> Result<f64, ExcelError> {
        let a = parse_naive(s, DateSystem::Excel1900);
        let b = parse(s, DateSystem::Excel1900);
        assert_eq!(a, b, "naive/fast mismatch for {s:?}");
        b
    }

    fn ok(s: &str, n: f64) {
        let v = both(s).unwrap_or_else(|e| panic!("{s:?} → {e:?}"));
        assert!(
            (v - n).abs() < 1e-12 || v == n,
            "{s:?}: got {v} expected {n}"
        );
    }

    fn err(s: &str, e: ExcelError) {
        assert_eq!(both(s), Err(e), "{s:?}");
    }

    #[test]
    fn microsoft_examples() {
        ok("$1,000", 1000.0);
        ok("16:48:00", 0.7);
        ok("12:00:00", 0.5);
        assert!((both("16:48:00").unwrap() - both("12:00:00").unwrap() - 0.2).abs() < 1e-12);
    }

    #[test]
    fn plain_and_scientific() {
        ok("123", 123.0);
        ok("123.45", 123.45);
        ok("  2  ", 2.0);
        ok("\t2\r\n", 2.0);
        ok("+7", 7.0);
        ok("-7", -7.0);
        ok(".5", 0.5);
        ok("5.", 5.0);
        ok("007", 7.0);
        ok("1E3", 1000.0);
        ok("1.5e-2", 0.015);
        ok("+1E+3", 1000.0);
        err("x", ExcelError::Value);
        err("", ExcelError::Value);
        err("   ", ExcelError::Value);
        err("TRUE", ExcelError::Value);
        err("FALSE", ExcelError::Value);
        err("1 2", ExcelError::Value);
        err("\u{00a0}2", ExcelError::Value);
    }

    #[test]
    fn currency_comma_percent_parens() {
        ok("1,000", 1000.0);
        ok("1,234.56", 1234.56);
        ok("1,234,567.89", 1_234_567.89);
        ok("$100", 100.0);
        ok("-$100", -100.0);
        ok("$-100", -100.0);
        ok("+$100", 100.0);
        ok("$1,000.00", 1000.0);
        ok("50%", 0.5);
        ok("50.5%", 0.505);
        ok("1,000%", 10.0);
        ok("50%%", 0.005);
        ok("(123)", -123.0);
        ok("(1,234.5)", -1234.5);
        ok("($1,234.50)", -1234.5);
        ok("(50%)", -0.5);
        ok("  $1,000  ", 1000.0);
        err("1,23", ExcelError::Value);
        err("1,2345", ExcelError::Value);
        err("12,34", ExcelError::Value);
        err("1,", ExcelError::Value);
        err(",123", ExcelError::Value);
        err("$", ExcelError::Value);
        err("%", ExcelError::Value);
        err("()", ExcelError::Value);
        err(",", ExcelError::Value);
        err("1,234E2", ExcelError::Value);
        err("1 000", ExcelError::Value);
    }

    #[test]
    fn mixed_fractions() {
        ok("1 1/2", 1.5);
        ok("3 3/4", 3.75);
        ok("-1 1/2", -1.5);
        ok("(1 1/2)", -1.5);
        err("1 1/0", ExcelError::Value);
        err("1/2", ExcelError::Value); // incomplete date, not a fraction
    }

    #[test]
    fn times() {
        ok("0:00", 0.0);
        ok("6:00", 0.25);
        ok("12:00", 0.5);
        ok("18:00:00", 0.75);
        ok("24:00:00", 1.0);
        ok("12:00 AM", 0.0);
        ok("12:00 PM", 0.5);
        ok("1:00 PM", 13.0 / 24.0);
        ok("12:00am", 0.0);
        ok("4:48 PM", 16.8 / 24.0);
        err("1:60", ExcelError::Value);
        err("1:00:60", ExcelError::Value);
        err("13:00 PM", ExcelError::Value);
    }

    #[test]
    fn dates() {
        let jan1_2020 = date_serial(2020, 1, 1, DateSystem::Excel1900).unwrap();
        ok("1/1/2020", jan1_2020);
        ok("1-1-2020", jan1_2020);
        ok("2020-01-01", jan1_2020);
        ok("2020/1/1", jan1_2020);
        ok("1/1/20", jan1_2020);
        let leap = date_serial(1900, 2, 29, DateSystem::Excel1900).unwrap();
        ok("2/29/1900", leap);
        assert_eq!(leap, 60.0);
        err("2/30/2020", ExcelError::Value);
        err("2/29/1901", ExcelError::Value);
        err("13/1/2020", ExcelError::Value);
        err("1/2", ExcelError::Value);
        let noon = jan1_2020 + 0.5;
        ok("1/1/2020 12:00:00", noon);
        ok("2020-01-01T12:00:00", noon);
        let y1904 = date_serial(1904, 1, 1, DateSystem::Excel1904).unwrap();
        assert_eq!(parse("1/1/1904", DateSystem::Excel1904).unwrap(), y1904);
        assert_eq!(y1904, 0.0);
        // 2-digit window: 30 → 1930
        let y1930 = date_serial(1930, 1, 1, DateSystem::Excel1900).unwrap();
        ok("1/1/30", y1930);
    }

    #[test]
    fn formula_coercion_and_arity() {
        let wb = Workbook::default();
        assert_eq!(
            eval_formula_in(&wb, "=VALUE(\"  2  \")").unwrap(),
            ExcelValue::Number(2.0)
        );
        assert_eq!(
            eval_formula_in(&wb, "=VALUE(\"$1,000\")").unwrap(),
            ExcelValue::Number(1000.0)
        );
        assert_eq!(
            eval_formula_in(&wb, "=VALUE(\"50%\")").unwrap(),
            ExcelValue::Number(0.5)
        );
        assert_eq!(
            eval_formula_in(&wb, "=VALUE(TRUE)").unwrap(),
            ExcelValue::Number(1.0)
        );
        assert_eq!(
            eval_formula_in(&wb, "=VALUE(FALSE)").unwrap(),
            ExcelValue::Number(0.0)
        );
        assert_eq!(
            eval_formula_in(&wb, "=VALUE(42)").unwrap(),
            ExcelValue::Number(42.0)
        );
        assert_eq!(
            eval_formula_in(&wb, "=VALUE()").unwrap(),
            ExcelValue::Error(ExcelError::Value)
        );
        assert_eq!(
            eval_formula_in(&wb, "=VALUE(1,2)").unwrap(),
            ExcelValue::Error(ExcelError::Value)
        );
        assert_eq!(
            eval_formula_in(&wb, "=VALUE(1/0)").unwrap(),
            ExcelValue::Error(ExcelError::Div0)
        );
        assert_eq!(
            eval_formula_in(&wb, "=VALUE(NA())").unwrap(),
            ExcelValue::Error(ExcelError::Na)
        );
        assert_eq!(
            eval_formula_in(&wb, "=\"1,000\"+0").unwrap(),
            ExcelValue::Error(ExcelError::Value)
        );
        assert_eq!(
            eval_formula_in(&wb, "=VALUE(\"$1,000\")+1").unwrap(),
            ExcelValue::Number(1001.0)
        );
        assert_eq!(
            eval_formula_in(&wb, "=VALUE(\"16:48:00\")-VALUE(\"12:00:00\")=0.2").unwrap(),
            ExcelValue::Bool(true)
        );
        assert_eq!(
            eval_formula_in(&wb, "=VALUE(\"1/1/2020\")=DATE(2020,1,1)").unwrap(),
            ExcelValue::Bool(true)
        );
    }

    #[test]
    fn formula_blank_and_cell() {
        let mut sheet = Sheet::new("Sheet1");
        sheet.cells.insert(
            "A1".into(),
            Cell::value(ExcelValue::Text("  $1,250  ".into())),
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
            eval_formula_in(&wb, "=VALUE(A1)").unwrap(),
            ExcelValue::Number(1250.0)
        );
        assert_eq!(
            eval_formula_in(&wb, "=VALUE(A2)").unwrap(),
            ExcelValue::Number(0.0)
        );
        assert_eq!(
            eval_formula_in(&wb, "=VALUE(A3)").unwrap(),
            ExcelValue::Error(ExcelError::Value)
        );
        assert_eq!(
            eval_formula_in(&wb, "=VALUE(B1)").unwrap(),
            ExcelValue::Number(0.0)
        );
    }
}
