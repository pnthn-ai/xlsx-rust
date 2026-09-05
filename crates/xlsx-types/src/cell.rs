//! A1 cell addresses, sheet-qualified refs, and rectangular ranges.

use serde::{Deserialize, Serialize};
use std::fmt;
use thiserror::Error;

/// 0-based column / row address (`A1` is `{ col: 0, row: 0 }`).
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Debug, Serialize, Deserialize)]
pub struct CellAddr {
    pub col: u32,
    pub row: u32,
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum AddrError {
    #[error("invalid A1 address: {0}")]
    Invalid(String),
}

impl CellAddr {
    pub fn new(col: u32, row: u32) -> Self {
        Self { col, row }
    }

    /// Parse `A1`, `$A$1`, `a1`. Sheet qualification is stripped by [`CellRef`].
    pub fn parse(input: &str) -> Result<Self, AddrError> {
        let s = input.trim();
        let bytes = s.as_bytes();
        if bytes.is_empty() {
            return Err(AddrError::Invalid(s.to_string()));
        }
        let mut i = 0;
        if bytes.get(i) == Some(&b'$') {
            i += 1;
        }
        let col_start = i;
        while i < bytes.len() && bytes[i].is_ascii_alphabetic() {
            i += 1;
        }
        if i == col_start {
            return Err(AddrError::Invalid(s.to_string()));
        }
        let col = parse_col(&s[col_start..i]).ok_or_else(|| AddrError::Invalid(s.to_string()))?;
        if bytes.get(i) == Some(&b'$') {
            i += 1;
        }
        let row_start = i;
        while i < bytes.len() && bytes[i].is_ascii_digit() {
            i += 1;
        }
        if i == row_start || i != bytes.len() {
            return Err(AddrError::Invalid(s.to_string()));
        }
        let row_1based: u32 = s[row_start..i]
            .parse()
            .map_err(|_| AddrError::Invalid(s.to_string()))?;
        if row_1based == 0 {
            return Err(AddrError::Invalid(s.to_string()));
        }
        Ok(Self {
            col,
            row: row_1based - 1,
        })
    }

    pub fn a1(self) -> String {
        let mut s = String::with_capacity(8);
        self.write_a1(&mut s);
        s
    }

    /// Append this address in A1 notation (`A1`, `AA10`) without an extra alloc
    /// for the return value. Used by tight range walks (`SUMIF`).
    pub fn write_a1(self, out: &mut String) {
        write_col_name(self.col, out);
        let mut row = self.row + 1;
        if row == 0 {
            out.push('0');
            return;
        }
        let start = out.len();
        while row > 0 {
            out.push(char::from(b'0' + (row % 10) as u8));
            row /= 10;
        }
        reverse_ascii_tail(out, start);
    }
}

impl fmt::Display for CellAddr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.a1())
    }
}

/// Sheet + address. Missing sheet means "the evaluation sheet".
#[derive(Clone, PartialEq, Eq, Hash, Debug, Serialize, Deserialize)]
pub struct CellRef {
    pub sheet: Option<String>,
    pub addr: CellAddr,
}

impl CellRef {
    pub fn new(sheet: impl Into<Option<String>>, addr: CellAddr) -> Self {
        Self {
            sheet: sheet.into(),
            addr,
        }
    }

    pub fn parse(input: &str) -> Result<Self, AddrError> {
        let s = input.trim();
        if let Some((sheet, rest)) = split_sheet(s) {
            Ok(Self {
                sheet: Some(sheet),
                addr: CellAddr::parse(rest)?,
            })
        } else {
            Ok(Self {
                sheet: None,
                addr: CellAddr::parse(s)?,
            })
        }
    }

    pub fn a1(&self) -> String {
        match &self.sheet {
            Some(sheet) => format!("{}!{}", quote_sheet(sheet), self.addr.a1()),
            None => self.addr.a1(),
        }
    }
}

impl fmt::Display for CellRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.a1())
    }
}

/// Inclusive rectangular range, optionally sheet-qualified on the start ref.
#[derive(Clone, PartialEq, Eq, Hash, Debug, Serialize, Deserialize)]
pub struct RangeRef {
    pub sheet: Option<String>,
    pub start: CellAddr,
    pub end: CellAddr,
}

impl RangeRef {
    pub fn new(sheet: Option<String>, a: CellAddr, b: CellAddr) -> Self {
        let (start, end) = normalize(a, b);
        Self { sheet, start, end }
    }

    pub fn parse(input: &str) -> Result<Self, AddrError> {
        let s = input.trim();
        let (sheet, rest) = match split_sheet(s) {
            Some((sheet, rest)) => (Some(sheet), rest),
            None => (None, s),
        };
        let (left, right) = rest
            .split_once(':')
            .ok_or_else(|| AddrError::Invalid(s.to_string()))?;
        // `Sheet1!A1:B2` — right side is an address only.
        let start = CellAddr::parse(left)?;
        let end = CellAddr::parse(right)?;
        Ok(Self::new(sheet, start, end))
    }

    pub fn row_count(&self) -> u32 {
        self.end.row - self.start.row + 1
    }

    pub fn col_count(&self) -> u32 {
        self.end.col - self.start.col + 1
    }

    pub fn cells(&self) -> impl Iterator<Item = CellAddr> + '_ {
        let sr = self.start.row;
        let er = self.end.row;
        let sc = self.start.col;
        let ec = self.end.col;
        (sr..=er).flat_map(move |row| (sc..=ec).map(move |col| CellAddr { col, row }))
    }
}

impl fmt::Display for RangeRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let body = format!("{}:{}", self.start.a1(), self.end.a1());
        match &self.sheet {
            Some(sheet) => write!(f, "{}!{}", quote_sheet(sheet), body),
            None => f.write_str(&body),
        }
    }
}

fn normalize(a: CellAddr, b: CellAddr) -> (CellAddr, CellAddr) {
    (
        CellAddr {
            col: a.col.min(b.col),
            row: a.row.min(b.row),
        },
        CellAddr {
            col: a.col.max(b.col),
            row: a.row.max(b.row),
        },
    )
}

fn parse_col(s: &str) -> Option<u32> {
    if s.is_empty() || s.len() > 3 {
        return None;
    }
    let mut n: u32 = 0;
    for c in s.chars() {
        let c = c.to_ascii_uppercase();
        if !c.is_ascii_uppercase() {
            return None;
        }
        n = n
            .checked_mul(26)?
            .checked_add((c as u32) - ('A' as u32) + 1)?;
    }
    n.checked_sub(1)
}

#[cfg(test)]
fn col_name(col: u32) -> String {
    let mut s = String::with_capacity(3);
    write_col_name(col, &mut s);
    s
}

fn write_col_name(mut col: u32, out: &mut String) {
    let start = out.len();
    col += 1;
    while col > 0 {
        col -= 1;
        out.push(char::from(b'A' + (col % 26) as u8));
        col /= 26;
    }
    reverse_ascii_tail(out, start);
}

fn reverse_ascii_tail(out: &mut String, start: usize) {
    let n = out.len() - start;
    let mut buf = [0u8; 16];
    let src = &out.as_bytes()[start..];
    for i in 0..n {
        buf[i] = src[n - 1 - i];
    }
    out.truncate(start);
    out.push_str(std::str::from_utf8(&buf[..n]).unwrap());
}

/// Split `Sheet1!A1` or `'My Sheet'!A1`.
fn split_sheet(s: &str) -> Option<(String, &str)> {
    let bang = s.rfind('!')?;
    let sheet_raw = &s[..bang];
    let rest = &s[bang + 1..];
    let sheet = if sheet_raw.starts_with('\'') && sheet_raw.ends_with('\'') && sheet_raw.len() >= 2
    {
        sheet_raw[1..sheet_raw.len() - 1].replace("''", "'")
    } else {
        sheet_raw.to_string()
    };
    Some((sheet, rest))
}

fn quote_sheet(sheet: &str) -> String {
    if sheet
        .chars()
        .any(|c| !c.is_ascii_alphanumeric() && c != '_')
    {
        format!("'{}'", sheet.replace('\'', "''"))
    } else {
        sheet.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_a1_and_dollar() {
        assert_eq!(CellAddr::parse("A1").unwrap(), CellAddr::new(0, 0));
        assert_eq!(CellAddr::parse("$B$2").unwrap(), CellAddr::new(1, 1));
        assert_eq!(CellAddr::parse("AA10").unwrap().a1(), "AA10");
        assert_eq!(col_name(0), "A");
        assert_eq!(col_name(26), "AA");
        let mut buf = String::new();
        CellAddr::parse("AA10").unwrap().write_a1(&mut buf);
        assert_eq!(buf, "AA10");
        buf.clear();
        CellAddr::new(0, 0).write_a1(&mut buf);
        assert_eq!(buf, "A1");
    }

    #[test]
    fn parse_sheet_and_range() {
        let r = CellRef::parse("Sheet1!C3").unwrap();
        assert_eq!(r.sheet.as_deref(), Some("Sheet1"));
        assert_eq!(r.addr, CellAddr::new(2, 2));
        let range = RangeRef::parse("'Data Set'!B2:A4").unwrap();
        assert_eq!(range.sheet.as_deref(), Some("Data Set"));
        assert_eq!(range.start, CellAddr::new(0, 1));
        assert_eq!(range.end, CellAddr::new(1, 3));
        assert_eq!(range.cells().count(), 6);
    }
}
