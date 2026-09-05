//! Excel worksheet error codes.
//!
//! These are the values a formula may *return*. They are distinct from
//! engine/infrastructure failures (parse bugs, unsupported functions), which
//! use [`crate::eval::EvalError`].

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::fmt;

/// An Excel-compatible error value (`#DIV/0!`, `#VALUE!`, …).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Ord, PartialOrd)]
pub enum ExcelError {
    /// `#NULL!` — intersecting ranges do not intersect.
    Null,
    /// `#DIV/0!` — division by zero (also `0/0`; Excel does not yield NaN).
    Div0,
    /// `#VALUE!` — wrong type / uncoercible operand.
    Value,
    /// `#REF!` — invalid cell reference.
    Ref,
    /// `#NAME?` — unknown name or function.
    Name,
    /// `#NUM!` — invalid numeric domain (e.g. `SQRT(-1)`).
    Num,
    /// `#N/A` — not available (lookup miss, `NA()`).
    Na,
    /// `#GETTING_DATA` — placeholder while a data pull is in flight.
    GettingData,
    /// `#SPILL!` — dynamic array cannot write its results.
    Spill,
    /// `#CALC!` — calculation could not produce a result.
    Calc,
    /// `#FIELD!` — missing linked-data field.
    Field,
    /// `#BLOCKED!` — linked data type is blocked.
    Blocked,
    /// `#CONNECT!` — could not connect to a data service.
    Connect,
    /// `#UNKNOWN!` — unrecognized data type.
    Unknown,
    /// `#BUSY!` — external resource is busy.
    Busy,
    /// `#CIRCULAR!` — modeled circular reference (Excel surfaces this as a
    /// status, not always a cell error; we keep a code so fixtures can name it).
    Circular,
}

impl ExcelError {
    /// Canonical Excel display text, including `#` and trailing `!` / `?`.
    pub fn excel_text(self) -> &'static str {
        match self {
            Self::Null => "#NULL!",
            Self::Div0 => "#DIV/0!",
            Self::Value => "#VALUE!",
            Self::Ref => "#REF!",
            Self::Name => "#NAME?",
            Self::Num => "#NUM!",
            Self::Na => "#N/A",
            Self::GettingData => "#GETTING_DATA",
            Self::Spill => "#SPILL!",
            Self::Calc => "#CALC!",
            Self::Field => "#FIELD!",
            Self::Blocked => "#BLOCKED!",
            Self::Connect => "#CONNECT!",
            Self::Unknown => "#UNKNOWN!",
            Self::Busy => "#BUSY!",
            Self::Circular => "#CIRCULAR!",
        }
    }

    /// Short machine id used in some fixture files (`DIV0`, `NA`, …).
    pub fn short_id(self) -> &'static str {
        match self {
            Self::Null => "NULL",
            Self::Div0 => "DIV0",
            Self::Value => "VALUE",
            Self::Ref => "REF",
            Self::Name => "NAME",
            Self::Num => "NUM",
            Self::Na => "NA",
            Self::GettingData => "GETTING_DATA",
            Self::Spill => "SPILL",
            Self::Calc => "CALC",
            Self::Field => "FIELD",
            Self::Blocked => "BLOCKED",
            Self::Connect => "CONNECT",
            Self::Unknown => "UNKNOWN",
            Self::Busy => "BUSY",
            Self::Circular => "CIRCULAR",
        }
    }

    /// Parse Excel display text, a short id, or a few informal aliases.
    pub fn parse(s: &str) -> Option<Self> {
        let t = s.trim();
        let upper = t.to_ascii_uppercase();
        let stripped = upper
            .trim_start_matches('#')
            .trim_end_matches('!')
            .trim_end_matches('?');
        let compact: String = stripped
            .chars()
            .filter(|c| *c != '/' && *c != '_')
            .collect();

        Some(match compact.as_str() {
            "NULL" => Self::Null,
            "DIV0" | "DIV" => Self::Div0,
            "VALUE" => Self::Value,
            "REF" => Self::Ref,
            "NAME" => Self::Name,
            "NUM" => Self::Num,
            "NA" => Self::Na,
            "GETTINGDATA" => Self::GettingData,
            "SPILL" => Self::Spill,
            "CALC" => Self::Calc,
            "FIELD" => Self::Field,
            "BLOCKED" => Self::Blocked,
            "CONNECT" => Self::Connect,
            "UNKNOWN" => Self::Unknown,
            "BUSY" => Self::Busy,
            "CIRCULAR" | "CIRC" => Self::Circular,
            _ => return None,
        })
    }
}

impl fmt::Display for ExcelError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.excel_text())
    }
}

impl Serialize for ExcelError {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.excel_text())
    }
}

impl<'de> Deserialize<'de> for ExcelError {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        Self::parse(&s).ok_or_else(|| serde::de::Error::custom(format!("unknown Excel error: {s}")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_display_and_aliases() {
        assert_eq!(ExcelError::parse("#DIV/0!"), Some(ExcelError::Div0));
        assert_eq!(ExcelError::parse("div0"), Some(ExcelError::Div0));
        assert_eq!(ExcelError::parse("#N/A"), Some(ExcelError::Na));
        assert_eq!(ExcelError::parse("NA"), Some(ExcelError::Na));
        assert_eq!(ExcelError::parse("#NAME?"), Some(ExcelError::Name));
        assert_eq!(ExcelError::Div0.excel_text(), "#DIV/0!");
    }
}
