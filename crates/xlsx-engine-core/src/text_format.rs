//! Excel `TEXT` for a documented, honest subset of format codes.
//!
//! Supported (`en-US` punctuation):
//! - number: `0`, `#`, `.`, grouping `,` between digit placeholders, `%`
//! - text placeholder: `@` (alone or with literals; not mixed with `0`/`#`/dates)
//! - literals: `$`, punctuation, quoted `"..."`, escaped `\`
//! - dates: `yyyy`/`yy`, `mm`/`m`, `dd`/`d` (month, not minutes)
//! - `General` (case-insensitive)
//!
//! Not implemented — corpus cases use `ignore` (do **not** invent goldens).
//! The kernel fails closed with `#VALUE!` if those tokens appear, so a wrong
//! string is never emitted. Unsupported: scientific `E+`, fractions `?/?`,
//! sections `;`, colors/conditions `[…]`, fill/skip `*` `_`, `?` placeholders,
//! trailing-comma scaling, time (`h`/`s`, `AM/PM`), month/day names
//! (`mmm`/`dddd`), mixed date+number skeletons, `@` mixed with digits/dates.
//!
//! Quirks this subset does implement:
//! - non-numeric text is returned unchanged (Excel does not `#VALUE!` it)
//! - numeric text is parsed (`VALUE`-style trim + `f64`) except under `@`,
//!   which echoes the original text
//! - blanks coerce to 0; stored `""` is text and is returned as `""`
//! - `TRUE`/`FALSE` coerce to 1/0 except under `General` / `@` (`TRUE`/`FALSE`)
//! - `#` omits a leading integer `0` (`TEXT(0.5,"#.#")` → `.5`)
//! - sign is taken from the **rounded** value (`TEXT(-0.001,"0.00")` → `0.00`)
//! - 1900 leap-year bug and serial 0 → `1900-01-00` via [`crate::dates`]

use crate::dates::serial_to_ymd;
use crate::eval::coerce;
use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::Arc;
use xlsx_types::{excel_round_15, DateSystem, ExcelError, ExcelValue};

/// Apply `TEXT(value, format)` for the supported subset.
pub fn apply(
    value: &ExcelValue,
    format: &str,
    date_system: DateSystem,
) -> Result<String, ExcelError> {
    if let ExcelValue::Error(e) = value {
        return Err(*e);
    }
    if format.is_empty() {
        return Ok(String::new());
    }
    if let Some(s) = try_fast(value, format, date_system) {
        return Ok(s);
    }
    apply_generic(value, format, date_system)
}

/// Parser path (no literal-format fast paths). Exposed for microbenches.
pub fn apply_generic(
    value: &ExcelValue,
    format: &str,
    date_system: DateSystem,
) -> Result<String, ExcelError> {
    if let ExcelValue::Error(e) = value {
        return Err(*e);
    }
    if format.is_empty() {
        return Ok(String::new());
    }
    if is_general(format) {
        return Ok(general_display(value));
    }
    let plan = parse_cached(format)?;
    format_with_plan(value, &plan, date_system)
}

/// Parsed-plan path after a caller already interned [`FormatPlan`].
pub fn apply_plan(
    value: &ExcelValue,
    plan: &FormatPlan,
    date_system: DateSystem,
) -> Result<String, ExcelError> {
    if let ExcelValue::Error(e) = value {
        return Err(*e);
    }
    format_with_plan(value, plan, date_system)
}

/// Baseline: no literal fast path, no plan cache. Same public contract as
/// [`apply`]. Used by microbenches so hill-climbs stay honest.
pub fn apply_naive(
    value: &ExcelValue,
    format: &str,
    date_system: DateSystem,
) -> Result<String, ExcelError> {
    if let ExcelValue::Error(e) = value {
        return Err(*e);
    }
    if format.is_empty() {
        return Ok(String::new());
    }
    if is_general(format) {
        return Ok(general_display(value));
    }
    let plan = FormatPlan::parse(format)?;
    format_with_plan(value, &plan, date_system)
}

fn is_general(format: &str) -> bool {
    format.eq_ignore_ascii_case("general")
}

fn general_display(value: &ExcelValue) -> String {
    match value {
        ExcelValue::Bool(true) => "TRUE".into(),
        ExcelValue::Bool(false) => "FALSE".into(),
        ExcelValue::Text(s) => match coerce::parse_numeric_text(s) {
            Ok(n) => coerce::format_plain(n),
            Err(_) => s.clone(),
        },
        ExcelValue::Empty => "0".into(),
        ExcelValue::Number(n) => coerce::format_plain(*n),
        ExcelValue::Error(e) => e.excel_text().to_string(),
        ExcelValue::Array(_) => "#VALUE!".into(),
    }
}

fn coerce_number(value: &ExcelValue) -> Result<Option<f64>, ExcelError> {
    match value {
        ExcelValue::Number(n) => Ok(Some(*n)),
        ExcelValue::Empty => Ok(Some(0.0)),
        ExcelValue::Bool(true) => Ok(Some(1.0)),
        ExcelValue::Bool(false) => Ok(Some(0.0)),
        ExcelValue::Text(s) => match coerce::parse_numeric_text(s) {
            Ok(n) => Ok(Some(n)),
            Err(_) => Ok(None),
        },
        ExcelValue::Error(e) => Err(*e),
        ExcelValue::Array(_) => Err(ExcelError::Value),
    }
}

fn format_with_plan(
    value: &ExcelValue,
    plan: &FormatPlan,
    date_system: DateSystem,
) -> Result<String, ExcelError> {
    if plan.kind == Kind::Text {
        return Ok(emit_at(value, plan));
    }
    match coerce_number(value)? {
        Some(n) => plan.emit(n, date_system),
        None => match value {
            ExcelValue::Text(s) => Ok(s.clone()),
            _ => Err(ExcelError::Value),
        },
    }
}

fn emit_at(value: &ExcelValue, plan: &FormatPlan) -> String {
    let mut out = String::with_capacity(plan.tokens.len() + 8);
    for t in &plan.tokens {
        match t {
            Token::Literal(c) => out.push(*c),
            Token::Percent => out.push('%'),
            Token::At => match value {
                ExcelValue::Text(s) => out.push_str(s),
                _ => out.push_str(&general_display(value)),
            },
            _ => {}
        }
    }
    out
}

#[derive(Clone, Copy)]
enum FastKind {
    Fixed(u32),
    Grouped(u32),
    Currency2,
    Percent(u32),
    Date {
        y: u8,
        m: u8,
        d: u8,
        sep1: u8,
        sep2: u8,
        order: DateOrder,
    },
    PadInt(u32),
    At,
}

#[derive(Clone, Copy)]
enum DateOrder {
    Ymd,
    Mdy,
    Dmy,
}

fn classify_fast(format: &str) -> Option<FastKind> {
    match format {
        "0.00" => Some(FastKind::Fixed(2)),
        "0" => Some(FastKind::Fixed(0)),
        "0.0" => Some(FastKind::Fixed(1)),
        "0.000" => Some(FastKind::Fixed(3)),
        "#,##0" => Some(FastKind::Grouped(0)),
        "#,##0.00" => Some(FastKind::Grouped(2)),
        "$#,##0.00" => Some(FastKind::Currency2),
        "0%" => Some(FastKind::Percent(0)),
        "0.0%" => Some(FastKind::Percent(1)),
        "0.00%" => Some(FastKind::Percent(2)),
        "000" => Some(FastKind::PadInt(3)),
        "0000000" => Some(FastKind::PadInt(7)),
        "@" => Some(FastKind::At),
        "yyyy-mm-dd" | "YYYY-MM-DD" => Some(FastKind::Date {
            y: 4,
            m: 2,
            d: 2,
            sep1: b'-',
            sep2: b'-',
            order: DateOrder::Ymd,
        }),
        "yyyy/mm/dd" | "YYYY/MM/DD" => Some(FastKind::Date {
            y: 4,
            m: 2,
            d: 2,
            sep1: b'/',
            sep2: b'/',
            order: DateOrder::Ymd,
        }),
        "mm/dd/yyyy" | "MM/DD/YYYY" => Some(FastKind::Date {
            y: 4,
            m: 2,
            d: 2,
            sep1: b'/',
            sep2: b'/',
            order: DateOrder::Mdy,
        }),
        "dd-mm-yyyy" | "DD-MM-YYYY" => Some(FastKind::Date {
            y: 4,
            m: 2,
            d: 2,
            sep1: b'-',
            sep2: b'-',
            order: DateOrder::Dmy,
        }),
        "yy-mm-dd" | "YY-MM-DD" => Some(FastKind::Date {
            y: 2,
            m: 2,
            d: 2,
            sep1: b'-',
            sep2: b'-',
            order: DateOrder::Ymd,
        }),
        _ if format.eq_ignore_ascii_case("yyyy-mm-dd") => Some(FastKind::Date {
            y: 4,
            m: 2,
            d: 2,
            sep1: b'-',
            sep2: b'-',
            order: DateOrder::Ymd,
        }),
        _ => None,
    }
}

fn try_fast(value: &ExcelValue, format: &str, date_system: DateSystem) -> Option<String> {
    if is_general(format) {
        return Some(general_display(value));
    }
    let kind = classify_fast(format)?;
    if matches!(kind, FastKind::At) {
        return Some(match value {
            ExcelValue::Text(s) => s.clone(),
            _ => general_display(value),
        });
    }
    let n = match coerce_number(value) {
        Ok(Some(n)) => n,
        Ok(None) => {
            return match value {
                ExcelValue::Text(s) => Some(s.clone()),
                _ => None,
            };
        }
        Err(_) => return None,
    };
    if !n.is_finite() {
        return None;
    }
    match kind {
        FastKind::Fixed(p) => fast_fixed(n, p),
        FastKind::Grouped(p) => fast_grouped(n, p),
        FastKind::Currency2 => fast_grouped(n, 2).map(|s| {
            if let Some(rest) = s.strip_prefix('-') {
                format!("-${rest}")
            } else {
                format!("${s}")
            }
        }),
        FastKind::Percent(p) => fast_percent(n, p),
        FastKind::Date {
            y,
            m,
            d,
            sep1,
            sep2,
            order,
        } => fast_date(n, date_system, y, m, d, sep1, sep2, order),
        FastKind::PadInt(width) => fast_pad_int(n, width),
        FastKind::At => Some(general_display(value)),
    }
}

fn fast_pad_int(n: f64, width: u32) -> Option<String> {
    let (neg, int_part, frac_part) = split_rounded(n, 0, 1)?;
    let neg = drop_minus_if_rounded_zero(neg, int_part, frac_part);
    let mut s = String::with_capacity(width as usize + 1);
    if neg {
        s.push('-');
    }
    let mut digits = [0u8; 40];
    let mut x = int_part;
    let mut len = 0usize;
    if x == 0 {
        digits[0] = b'0';
        len = 1;
    } else {
        while x > 0 {
            digits[len] = b'0' + (x % 10) as u8;
            x /= 10;
            len += 1;
        }
    }
    while len < width as usize {
        digits[len] = b'0';
        len += 1;
    }
    for i in (0..len).rev() {
        s.push(digits[i] as char);
    }
    Some(s)
}

fn drop_minus_if_rounded_zero(neg: bool, int_part: u128, frac_part: u128) -> bool {
    neg && (int_part != 0 || frac_part != 0)
}

fn fast_fixed(n: f64, places: u32) -> Option<String> {
    let (neg, int_part, frac_part) = split_rounded(n, places, 1)?;
    let neg = drop_minus_if_rounded_zero(neg, int_part, frac_part);
    let mut s = String::with_capacity(20);
    if neg {
        s.push('-');
    }
    push_u128(&mut s, int_part);
    if places > 0 {
        s.push('.');
        push_frac(&mut s, frac_part, places);
    }
    Some(s)
}

fn fast_grouped(n: f64, places: u32) -> Option<String> {
    let (neg, int_part, frac_part) = split_rounded(n, places, 1)?;
    let neg = drop_minus_if_rounded_zero(neg, int_part, frac_part);
    let mut s = String::with_capacity(24);
    if neg {
        s.push('-');
    }
    push_grouped_u128(&mut s, int_part);
    if places > 0 {
        s.push('.');
        push_frac(&mut s, frac_part, places);
    }
    Some(s)
}

fn fast_percent(n: f64, places: u32) -> Option<String> {
    let (neg, int_part, frac_part) = split_rounded(n, places, 100)?;
    let neg = drop_minus_if_rounded_zero(neg, int_part, frac_part);
    let mut s = String::with_capacity(16);
    if neg {
        s.push('-');
    }
    push_u128(&mut s, int_part);
    if places > 0 {
        s.push('.');
        push_frac(&mut s, frac_part, places);
    }
    s.push('%');
    Some(s)
}

fn fast_date(
    n: f64,
    date_system: DateSystem,
    y_width: u8,
    m_width: u8,
    d_width: u8,
    sep1: u8,
    sep2: u8,
    order: DateOrder,
) -> Option<String> {
    if n < 0.0 {
        return None;
    }
    let (y, m, d) = serial_to_ymd(n, date_system).ok()?;
    let year = if y_width >= 4 {
        y.unsigned_abs()
    } else {
        y.unsigned_abs() % 100
    };
    let mut s = String::with_capacity(12);
    match order {
        DateOrder::Ymd => {
            push_date_part(&mut s, year, y_width as usize);
            s.push(sep1 as char);
            push_date_part(&mut s, m, m_width as usize);
            s.push(sep2 as char);
            push_date_part(&mut s, d, d_width as usize);
        }
        DateOrder::Mdy => {
            push_date_part(&mut s, m, m_width as usize);
            s.push(sep1 as char);
            push_date_part(&mut s, d, d_width as usize);
            s.push(sep2 as char);
            push_date_part(&mut s, year, y_width as usize);
        }
        DateOrder::Dmy => {
            push_date_part(&mut s, d, d_width as usize);
            s.push(sep1 as char);
            push_date_part(&mut s, m, m_width as usize);
            s.push(sep2 as char);
            push_date_part(&mut s, year, y_width as usize);
        }
    }
    Some(s)
}

fn split_rounded(n: f64, places: u32, scale: u32) -> Option<(bool, u128, u128)> {
    if !n.is_finite() || n.abs() >= 1e15 {
        return None;
    }
    let neg = n.is_sign_negative() && n != 0.0;
    let abs = excel_round_15(n.abs()) * scale as f64;
    let factor = 10f64.powi(places as i32);
    let scaled = abs * factor;
    let rnd = (scaled + 0.5).floor();
    if !rnd.is_finite() || rnd < 0.0 || rnd >= 1e18 {
        return None;
    }
    let rnd = rnd as u128;
    let div = 10u128.pow(places);
    Some((neg, rnd / div, rnd % div))
}

fn push_u128(s: &mut String, mut n: u128) {
    if n == 0 {
        s.push('0');
        return;
    }
    let mut buf = [0u8; 40];
    let mut i = 40;
    while n > 0 {
        i -= 1;
        buf[i] = b'0' + (n % 10) as u8;
        n /= 10;
    }
    s.push_str(unsafe { std::str::from_utf8_unchecked(&buf[i..]) });
}

fn push_grouped_u128(s: &mut String, n: u128) {
    if n == 0 {
        s.push('0');
        return;
    }
    let mut buf = [0u8; 48];
    let mut i = 48;
    let mut digits = 0u32;
    let mut x = n;
    while x > 0 {
        if digits > 0 && digits % 3 == 0 {
            i -= 1;
            buf[i] = b',';
        }
        i -= 1;
        buf[i] = b'0' + (x % 10) as u8;
        x /= 10;
        digits += 1;
    }
    s.push_str(unsafe { std::str::from_utf8_unchecked(&buf[i..]) });
}

fn push_frac(s: &mut String, mut frac: u128, places: u32) {
    let mut buf = [b'0'; 16];
    let p = places as usize;
    for i in (0..p).rev() {
        buf[i] = b'0' + (frac % 10) as u8;
        frac /= 10;
    }
    s.push_str(unsafe { std::str::from_utf8_unchecked(&buf[..p]) });
}

fn push_u32_pad(s: &mut String, mut n: u32, width: usize) {
    let mut buf = [b'0'; 8];
    for i in (0..width).rev() {
        buf[i] = b'0' + (n % 10) as u8;
        n /= 10;
    }
    s.push_str(unsafe { std::str::from_utf8_unchecked(&buf[..width]) });
}

/// `m` / `d` write every digit (15 stays "15"); `mm` / `dd` / `yy` / `yyyy` pad.
fn push_date_part(s: &mut String, n: u32, width: usize) {
    if width <= 1 {
        push_u128(s, n as u128);
    } else {
        push_u32_pad(s, n, width);
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Token {
    Digit0,
    DigitHash,
    Decimal,
    Group,
    Percent,
    At,
    Year { width: u8 },
    Month { width: u8 },
    Day { width: u8 },
    Literal(char),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Number,
    Date,
    Text,
}

/// Parsed format — intern and reuse on hot generic paths.
#[derive(Clone, Debug)]
pub struct FormatPlan {
    kind: Kind,
    tokens: Vec<Token>,
    pct_scale: u32,
    group: bool,
    min_int: usize,
    frac_keep: Vec<bool>,
    force_zero: bool,
    has_decimal: bool,
}

impl FormatPlan {
    pub fn parse(format: &str) -> Result<Self, ExcelError> {
        let tokens = tokenize(format)?;
        classify(tokens)
    }

    fn emit(&self, n: f64, date_system: DateSystem) -> Result<String, ExcelError> {
        if !n.is_finite() {
            return Err(ExcelError::Num);
        }
        match self.kind {
            Kind::Date => emit_date(&self.tokens, n, date_system),
            Kind::Number => emit_number(self, n),
            Kind::Text => Err(ExcelError::Value),
        }
    }
}

thread_local! {
    static PLAN_CACHE: RefCell<HashMap<String, Arc<FormatPlan>>> =
        RefCell::new(HashMap::new());
}

fn parse_cached(format: &str) -> Result<Arc<FormatPlan>, ExcelError> {
    PLAN_CACHE.with(|cache| {
        if let Some(p) = cache.borrow().get(format) {
            return Ok(Arc::clone(p));
        }
        let plan = Arc::new(FormatPlan::parse(format)?);
        cache
            .borrow_mut()
            .insert(format.to_string(), Arc::clone(&plan));
        Ok(plan)
    })
}

/// Clear the thread-local parse cache (tests / benches).
pub fn clear_plan_cache() {
    PLAN_CACHE.with(|c| c.borrow_mut().clear());
}

fn tokenize(format: &str) -> Result<Vec<Token>, ExcelError> {
    let chars: Vec<char> = format.chars().collect();
    let mut i = 0;
    let mut out = Vec::with_capacity(chars.len());
    while i < chars.len() {
        let c = chars[i];
        match c {
            '"' => {
                i += 1;
                while i < chars.len() && chars[i] != '"' {
                    out.push(Token::Literal(chars[i]));
                    i += 1;
                }
                if i >= chars.len() {
                    return Err(ExcelError::Value);
                }
                i += 1;
            }
            '\\' => {
                i += 1;
                if i >= chars.len() {
                    return Err(ExcelError::Value);
                }
                out.push(Token::Literal(chars[i]));
                i += 1;
            }
            '0' => {
                out.push(Token::Digit0);
                i += 1;
            }
            '#' => {
                out.push(Token::DigitHash);
                i += 1;
            }
            '@' => {
                out.push(Token::At);
                i += 1;
            }
            '.' => {
                out.push(Token::Decimal);
                i += 1;
            }
            ',' => {
                out.push(Token::Group);
                i += 1;
            }
            '%' => {
                out.push(Token::Percent);
                i += 1;
            }
            'y' | 'Y' => {
                let n = take_run(&chars, i, &['y', 'Y']);
                if n >= 3 {
                    out.push(Token::Year { width: 4 });
                } else {
                    out.push(Token::Year { width: 2 });
                }
                i += n;
            }
            'm' | 'M' => {
                let n = take_run(&chars, i, &['m', 'M']);
                if n >= 3 {
                    return Err(ExcelError::Value);
                }
                out.push(Token::Month {
                    width: if n >= 2 { 2 } else { 1 },
                });
                i += n;
            }
            'd' | 'D' => {
                let n = take_run(&chars, i, &['d', 'D']);
                if n >= 3 {
                    return Err(ExcelError::Value);
                }
                out.push(Token::Day {
                    width: if n >= 2 { 2 } else { 1 },
                });
                i += n;
            }
            'h' | 'H' | 's' | 'S' | '?' | ';' | '[' | '*' | '_' => {
                return Err(ExcelError::Value);
            }
            'e' | 'E' => {
                let next = chars.get(i + 1).copied();
                if matches!(next, Some('+') | Some('-') | Some('0') | Some('#')) {
                    return Err(ExcelError::Value);
                }
                out.push(Token::Literal(c));
                i += 1;
            }
            'a' | 'A' => {
                if is_ampm_run(&chars, i) {
                    return Err(ExcelError::Value);
                }
                out.push(Token::Literal(c));
                i += 1;
            }
            other => {
                out.push(Token::Literal(other));
                i += 1;
            }
        }
    }
    Ok(out)
}

fn take_run(chars: &[char], start: usize, set: &[char]) -> usize {
    let mut n = 0;
    while start + n < chars.len() && set.contains(&chars[start + n]) {
        n += 1;
    }
    n
}

fn is_ampm_run(chars: &[char], i: usize) -> bool {
    fn fold(c: char) -> char {
        c.to_ascii_lowercase()
    }
    let n = chars.len().saturating_sub(i);
    if n >= 5
        && fold(chars[i]) == 'a'
        && fold(chars[i + 1]) == 'm'
        && chars[i + 2] == '/'
        && fold(chars[i + 3]) == 'p'
        && fold(chars[i + 4]) == 'm'
    {
        return true;
    }
    n >= 3 && fold(chars[i]) == 'a' && chars[i + 1] == '/' && fold(chars[i + 2]) == 'p'
}

fn classify(tokens: Vec<Token>) -> Result<FormatPlan, ExcelError> {
    let mut has_date = false;
    let mut has_digit = false;
    let mut has_at = false;
    let mut pct = 0u32;
    let mut seen_decimal = false;
    let mut group = false;
    let mut min_int = 0usize;
    let mut frac_keep = Vec::new();
    let mut last_int_was_digit = false;
    let mut trailing_scale = false;
    let mut force_zero = false;

    for t in &tokens {
        match t {
            Token::Year { .. } | Token::Month { .. } | Token::Day { .. } => has_date = true,
            Token::Digit0 => {
                has_digit = true;
                force_zero = true;
                if seen_decimal {
                    frac_keep.push(true);
                } else {
                    min_int += 1;
                    last_int_was_digit = true;
                    trailing_scale = false;
                }
            }
            Token::DigitHash => {
                has_digit = true;
                if seen_decimal {
                    frac_keep.push(false);
                } else {
                    last_int_was_digit = true;
                    trailing_scale = false;
                }
            }
            Token::Decimal => {
                if seen_decimal {
                    // second `.` is a literal-ish oddity; treat as unsupported
                    return Err(ExcelError::Value);
                }
                seen_decimal = true;
                last_int_was_digit = false;
            }
            Token::Group => {
                if seen_decimal {
                    return Err(ExcelError::Value);
                }
                if last_int_was_digit {
                    group = true;
                    last_int_was_digit = false;
                    trailing_scale = true;
                } else {
                    // leading / doubled comma → scaling or junk
                    return Err(ExcelError::Value);
                }
            }
            Token::Percent => pct += 1,
            Token::At => has_at = true,
            Token::Literal(_) => {}
        }
    }
    if trailing_scale {
        // last integer-side comma had no digit after it → `/1000` scaling
        return Err(ExcelError::Value);
    }
    if has_at && (has_date || has_digit || pct > 0) {
        return Err(ExcelError::Value);
    }
    if has_date && has_digit {
        return Err(ExcelError::Value);
    }
    if has_at {
        return Ok(FormatPlan {
            kind: Kind::Text,
            tokens,
            pct_scale: 1,
            group: false,
            min_int: 0,
            frac_keep: Vec::new(),
            force_zero: false,
            has_decimal: false,
        });
    }
    if has_date {
        return Ok(FormatPlan {
            kind: Kind::Date,
            tokens,
            pct_scale: 1,
            group: false,
            min_int: 0,
            frac_keep: Vec::new(),
            force_zero: false,
            has_decimal: false,
        });
    }
    Ok(FormatPlan {
        kind: Kind::Number,
        tokens,
        pct_scale: 100u32.saturating_pow(pct),
        group,
        min_int,
        frac_keep,
        force_zero,
        has_decimal: seen_decimal,
    })
}

fn emit_date(tokens: &[Token], n: f64, date_system: DateSystem) -> Result<String, ExcelError> {
    if n < 0.0 {
        return Err(ExcelError::Value);
    }
    let (y, m, d) = serial_to_ymd(n, date_system)?;
    let mut out = String::with_capacity(tokens.len() + 8);
    for t in tokens {
        match *t {
            Token::Year { width } => {
                let v = if width >= 4 {
                    y.unsigned_abs()
                } else {
                    y.unsigned_abs() % 100
                };
                push_date_part(&mut out, v, width as usize);
            }
            Token::Month { width } => push_date_part(&mut out, m, width as usize),
            Token::Day { width } => push_date_part(&mut out, d, width as usize),
            Token::Literal(c) => out.push(c),
            Token::Percent => out.push('%'),
            Token::At | Token::Digit0 | Token::DigitHash | Token::Decimal | Token::Group => {
                return Err(ExcelError::Value)
            }
        }
    }
    Ok(out)
}

fn emit_number(plan: &FormatPlan, n: f64) -> Result<String, ExcelError> {
    let places = plan.frac_keep.len() as u32;
    let scale = plan.pct_scale.max(1);
    let abs = excel_round_15(n.abs()) * scale as f64;
    if !abs.is_finite() {
        return Err(ExcelError::Num);
    }
    let (int_part, frac_part) = match split_rounded_parts(abs, places) {
        Some(p) => p,
        None => {
            return emit_number_fallback(plan, abs, n.is_sign_negative() && n != 0.0, places);
        }
    };
    let rounded_zero = int_part == 0 && frac_part == 0;
    let neg = n.is_sign_negative() && n != 0.0 && !rounded_zero;

    let mut frac = [0u8; 16];
    let mut frac_len = places as usize;
    let mut fp = frac_part;
    for i in (0..frac_len).rev() {
        frac[i] = (fp % 10) as u8;
        fp /= 10;
    }
    while frac_len > 0 && !plan.frac_keep[frac_len - 1] && frac[frac_len - 1] == 0 {
        frac_len -= 1;
    }

    let mut int_buf = [0u8; 40];
    let int_len = fill_int_digits(&mut int_buf, int_part, plan.min_int);

    let mut out = String::with_capacity(int_len + int_len / 3 + frac_len + 8);
    if neg && (int_len > 0 || frac_len > 0 || plan.force_zero) {
        out.push('-');
    }

    let mut emitted_number = false;
    for t in &plan.tokens {
        match t {
            Token::Literal(c) => out.push(*c),
            Token::Percent => out.push('%'),
            Token::Digit0 | Token::DigitHash | Token::Group | Token::Decimal => {
                if emitted_number {
                    continue;
                }
                emitted_number = true;
                if plan.group && int_len > 0 {
                    push_grouped_digits(&mut out, &int_buf[..int_len]);
                } else {
                    for &d in &int_buf[..int_len] {
                        out.push((b'0' + d) as char);
                    }
                }
                if plan.has_decimal {
                    out.push('.');
                    for &d in &frac[..frac_len] {
                        out.push((b'0' + d) as char);
                    }
                }
            }
            Token::At | Token::Year { .. } | Token::Month { .. } | Token::Day { .. } => {
                return Err(ExcelError::Value);
            }
        }
    }
    Ok(out)
}

fn fill_int_digits(buf: &mut [u8; 40], int_part: u128, min_int: usize) -> usize {
    if int_part == 0 && min_int == 0 {
        return 0;
    }
    if int_part == 0 {
        return min_int.min(40);
    }
    let mut raw = [0u8; 40];
    let mut raw_len = 0usize;
    let mut x = int_part;
    while x > 0 && raw_len < 40 {
        raw[raw_len] = (x % 10) as u8;
        x /= 10;
        raw_len += 1;
    }
    raw[..raw_len].reverse();
    if raw_len < min_int {
        let pad = (min_int - raw_len).min(40 - raw_len);
        buf[..pad].fill(0);
        buf[pad..pad + raw_len].copy_from_slice(&raw[..raw_len]);
        pad + raw_len
    } else {
        buf[..raw_len].copy_from_slice(&raw[..raw_len]);
        raw_len
    }
}

fn push_grouped_digits(out: &mut String, digits: &[u8]) {
    for (i, &d) in digits.iter().enumerate() {
        if i > 0 && (digits.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push((b'0' + d) as char);
    }
}

fn split_rounded_parts(abs: f64, places: u32) -> Option<(u128, u128)> {
    if abs >= 1e18 {
        return None;
    }
    let factor = 10f64.powi(places as i32);
    let scaled = abs * factor;
    let rnd = (scaled + 0.5).floor();
    if !rnd.is_finite() || rnd < 0.0 || rnd >= 1e18 {
        return None;
    }
    let rnd = rnd as u128;
    let div = 10u128.pow(places);
    Some((rnd / div, rnd % div))
}

fn emit_number_fallback(
    plan: &FormatPlan,
    abs: f64,
    neg: bool,
    places: u32,
) -> Result<String, ExcelError> {
    // Rare huge magnitudes: fall back to Debug-ish Excel general + skeleton.
    let _ = (plan, places);
    let mut s = coerce::format_plain(abs);
    if neg {
        s.insert(0, '-');
    }
    Ok(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(n: f64, fmt: &str) -> String {
        apply(&ExcelValue::Number(n), fmt, DateSystem::Excel1900)
            .unwrap_or_else(|e| panic!("{n} {fmt:?}: {e:?}"))
    }

    #[test]
    fn fixed_decimals() {
        assert_eq!(t(1234.567, "0.00"), "1234.57");
        assert_eq!(t(1234.5, "0.00"), "1234.50");
        assert_eq!(t(0.0, "0.00"), "0.00");
        assert_eq!(t(-1234.5, "0.00"), "-1234.50");
        assert_eq!(t(1234.5, "0"), "1235");
        assert_eq!(t(2.5, "0"), "3");
        assert_eq!(t(-1.5, "0"), "-2");
        assert_eq!(t(1.0 / 3.0, "0.00"), "0.33");
    }

    #[test]
    fn thousands() {
        assert_eq!(t(1234.0, "#,##0"), "1,234");
        assert_eq!(t(1234.5, "#,##0"), "1,235");
        assert_eq!(t(0.0, "#,##0"), "0");
        assert_eq!(t(12.0, "#,##0"), "12");
        assert_eq!(t(1_000_000.0, "#,##0"), "1,000,000");
        assert_eq!(t(-1234.0, "#,##0"), "-1,234");
        assert_eq!(t(1234.567, "#,##0.00"), "1,234.57");
        assert_eq!(t(1234.567, "$#,##0.00"), "$1,234.57");
    }

    #[test]
    fn percent_and_pad() {
        assert_eq!(t(0.5, "0%"), "50%");
        assert_eq!(t(0.125, "0%"), "13%");
        assert_eq!(t(0.285, "0.0%"), "28.5%");
        assert_eq!(t(0.285, "0.00%"), "28.50%");
        assert_eq!(t(1.0, "0%"), "100%");
        assert_eq!(t(1234.0, "0000000"), "0001234");
        assert_eq!(t(5.0, "000"), "005");
    }

    #[test]
    fn dates() {
        assert_eq!(t(45366.0, "yyyy-mm-dd"), "2024-03-15");
        assert_eq!(t(36526.0, "yyyy-mm-dd"), "2000-01-01");
        assert_eq!(t(60.0, "yyyy-mm-dd"), "1900-02-29");
        assert_eq!(t(0.0, "yyyy-mm-dd"), "1900-01-00");
        assert_eq!(t(1.0, "yyyy-mm-dd"), "1900-01-01");
        assert_eq!(t(45366.0, "yyyy-m-d"), "2024-3-15");
        assert_eq!(t(45356.0, "yyyy-m-d"), "2024-3-5");
        assert_eq!(t(45366.0, "YY-MM-DD"), "24-03-15");
        let s = apply(
            &ExcelValue::Number(0.0),
            "yyyy-mm-dd",
            DateSystem::Excel1904,
        )
        .unwrap();
        assert_eq!(s, "1904-01-01");
    }

    #[test]
    fn quirks() {
        assert_eq!(
            apply(
                &ExcelValue::Text("abc".into()),
                "0.00",
                DateSystem::Excel1900
            )
            .unwrap(),
            "abc"
        );
        assert_eq!(
            apply(
                &ExcelValue::Text("  12  ".into()),
                "0",
                DateSystem::Excel1900
            )
            .unwrap(),
            "12"
        );
        assert_eq!(
            apply(&ExcelValue::Bool(true), "0", DateSystem::Excel1900).unwrap(),
            "1"
        );
        assert_eq!(
            apply(&ExcelValue::Bool(true), "General", DateSystem::Excel1900).unwrap(),
            "TRUE"
        );
        assert_eq!(
            apply(&ExcelValue::Empty, "0", DateSystem::Excel1900).unwrap(),
            "0"
        );
        assert_eq!(
            apply(&ExcelValue::Number(123.0), "", DateSystem::Excel1900).unwrap(),
            ""
        );
        assert_eq!(t(1234.5, "General"), "1234.5");
        assert_eq!(t(0.0, "#"), "");
        assert_eq!(t(0.4, "#"), "");
        assert_eq!(t(0.5, "#"), "1");
        assert_eq!(t(0.5, "#.#"), ".5");
        assert_eq!(t(1.5, "0.##"), "1.5");
        assert_eq!(t(1.0, "0.##"), "1.");
        assert_eq!(t(-0.001, "0.00"), "0.00");
        assert_eq!(t(-0.006, "0.00"), "-0.01");
        assert_eq!(t(-0.5, "#.#"), "-.5");
        assert_eq!(t(9.99, "0.0"), "10.0");
        assert_eq!(t(999.5, "#,##0"), "1,000");
        assert_eq!(t(0.5, "0%%"), "5000%%");
        assert_eq!(t(0.5, "0\\%"), "1%");
        assert_eq!(t(1234.0, "\"USD \"#,##0"), "USD 1,234");
        assert_eq!(t(12.0, "0\\k"), "12k");
        assert_eq!(t(1234.5, "@"), "1234.5");
        assert_eq!(t(1234.0, "n=@"), "n=1234");
        assert_eq!(t(1234.0, "\"ID: \"@"), "ID: 1234");
        assert_eq!(
            apply(
                &ExcelValue::Text("  12  ".into()),
                "@",
                DateSystem::Excel1900
            )
            .unwrap(),
            "  12  "
        );
        assert_eq!(
            apply(&ExcelValue::Bool(true), "@", DateSystem::Excel1900).unwrap(),
            "TRUE"
        );
        assert_eq!(t(-1234.567, "$#,##0.00"), "-$1,234.57");
        assert_eq!(t(1.0, "GENERAL"), "1");
        assert_eq!(t(45366.75, "yyyy-mm-dd"), "2024-03-15");
    }

    #[test]
    fn unsupported_is_value() {
        assert_eq!(
            apply(&ExcelValue::Number(1.0), "0.00E+00", DateSystem::Excel1900),
            Err(ExcelError::Value)
        );
        assert_eq!(
            apply(
                &ExcelValue::Number(1.0),
                "0.00;(0.00)",
                DateSystem::Excel1900
            ),
            Err(ExcelError::Value)
        );
        assert_eq!(
            apply(&ExcelValue::Number(1.0), "hh:mm", DateSystem::Excel1900),
            Err(ExcelError::Value)
        );
        assert_eq!(
            apply(&ExcelValue::Number(1.0), "mmm", DateSystem::Excel1900),
            Err(ExcelError::Value)
        );
        assert_eq!(
            apply(&ExcelValue::Number(1000.0), "#,##0,", DateSystem::Excel1900),
            Err(ExcelError::Value)
        );
        assert_eq!(
            apply(&ExcelValue::Number(1.0), "0@", DateSystem::Excel1900),
            Err(ExcelError::Value)
        );
        assert_eq!(
            apply(&ExcelValue::Number(1.0), "# ?/?", DateSystem::Excel1900),
            Err(ExcelError::Value)
        );
        assert_eq!(
            apply(&ExcelValue::Number(1.0), "[Red]0", DateSystem::Excel1900),
            Err(ExcelError::Value)
        );
    }

    #[test]
    fn fast_matches_generic_hot_paths() {
        clear_plan_cache();
        for (n, fmt) in [
            (1234.567, "0.00"),
            (-88.1, "0.00"),
            (1_234_567.0, "#,##0"),
            (0.285, "0.0%"),
            (45366.0, "yyyy-mm-dd"),
            (1234.0, "0000000"),
            (-0.001, "0.00"),
            (0.5, "#.#"),
            (1234.567, "$#,##0.00"),
            (45366.0, "mm/dd/yyyy"),
            (45366.0, "yyyy/mm/dd"),
        ] {
            let a = apply(&ExcelValue::Number(n), fmt, DateSystem::Excel1900).unwrap();
            clear_plan_cache();
            let b = apply_generic(&ExcelValue::Number(n), fmt, DateSystem::Excel1900).unwrap();
            let c = apply_naive(&ExcelValue::Number(n), fmt, DateSystem::Excel1900).unwrap();
            assert_eq!(a, b, "{n} {fmt}");
            assert_eq!(a, c, "naive {n} {fmt}");
        }
    }
}
