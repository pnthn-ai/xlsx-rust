//! Excel cell / formula result values.

use crate::error::ExcelError;
use serde::de::{self, MapAccess, Visitor};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::fmt;

/// The Excel type tag a fixture or report can name independently of the payload.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExcelType {
    Empty,
    Number,
    Text,
    Bool,
    Error,
    Array,
}

impl fmt::Display for ExcelType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => f.write_str("empty"),
            Self::Number => f.write_str("number"),
            Self::Text => f.write_str("text"),
            Self::Bool => f.write_str("bool"),
            Self::Error => f.write_str("error"),
            Self::Array => f.write_str("array"),
        }
    }
}

/// An Excel-compatible computed value.
///
/// Empty is a first-class type: a blank cell is **not** the number `0` and
/// **not** the text `""`, even though many operators treat it as one or the
/// other. Compatibility fixtures should keep that distinction.
#[derive(Clone, PartialEq, Debug)]
pub enum ExcelValue {
    Empty,
    Number(f64),
    Text(String),
    Bool(bool),
    Error(ExcelError),
    /// Row-major 2-D array (dynamic-array / CSE result).
    Array(Vec<Vec<ExcelValue>>),
}

impl ExcelValue {
    pub fn excel_type(&self) -> ExcelType {
        match self {
            Self::Empty => ExcelType::Empty,
            Self::Number(_) => ExcelType::Number,
            Self::Text(_) => ExcelType::Text,
            Self::Bool(_) => ExcelType::Bool,
            Self::Error(_) => ExcelType::Error,
            Self::Array(_) => ExcelType::Array,
        }
    }

    pub fn is_error(&self) -> bool {
        matches!(self, Self::Error(_))
    }

    pub fn as_error(&self) -> Option<ExcelError> {
        match self {
            Self::Error(e) => Some(*e),
            _ => None,
        }
    }

    pub fn error(code: ExcelError) -> Self {
        Self::Error(code)
    }

    /// Compact display used in CLI diffs (`3`, `"A"`, `#DIV/0!`, `{1,2}`).
    pub fn display_compact(&self) -> String {
        match self {
            Self::Empty => "<empty>".to_string(),
            Self::Number(n) => format_number(*n),
            Self::Text(s) => format!("\"{}\"", s.replace('"', "\"\"")),
            Self::Bool(true) => "TRUE".to_string(),
            Self::Bool(false) => "FALSE".to_string(),
            Self::Error(e) => e.excel_text().to_string(),
            Self::Array(rows) => {
                let body: Vec<String> = rows
                    .iter()
                    .map(|row| {
                        row.iter()
                            .map(Self::display_compact)
                            .collect::<Vec<_>>()
                            .join(",")
                    })
                    .collect();
                format!("{{{}}}", body.join(";"))
            }
        }
    }
}

impl fmt::Display for ExcelValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.display_compact())
    }
}

fn format_number(n: f64) -> String {
    if n.is_nan() {
        return "NaN".to_string();
    }
    if n.is_infinite() {
        return if n.is_sign_positive() {
            "+inf".to_string()
        } else {
            "-inf".to_string()
        };
    }
    // Prefer a short decimal when it is exact enough for humans.
    if n.fract() == 0.0 && n.abs() < 1e15 {
        return format!("{n:.0}");
    }
    let s = format!("{n}");
    s
}

impl Serialize for ExcelValue {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeMap;
        match self {
            Self::Empty => {
                let mut m = serializer.serialize_map(Some(1))?;
                m.serialize_entry("empty", &())?;
                m.end()
            }
            Self::Number(n) => {
                let mut m = serializer.serialize_map(Some(1))?;
                m.serialize_entry("number", n)?;
                m.end()
            }
            Self::Text(t) => {
                let mut m = serializer.serialize_map(Some(1))?;
                m.serialize_entry("text", t)?;
                m.end()
            }
            Self::Bool(b) => {
                let mut m = serializer.serialize_map(Some(1))?;
                m.serialize_entry("bool", b)?;
                m.end()
            }
            Self::Error(e) => {
                let mut m = serializer.serialize_map(Some(1))?;
                m.serialize_entry("error", e)?;
                m.end()
            }
            Self::Array(rows) => {
                let mut m = serializer.serialize_map(Some(1))?;
                m.serialize_entry("array", rows)?;
                m.end()
            }
        }
    }
}

impl<'de> Deserialize<'de> for ExcelValue {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_any(ExcelValueVisitor)
    }
}

struct ExcelValueVisitor;

impl<'de> Visitor<'de> for ExcelValueVisitor {
    type Value = ExcelValue;

    fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str("an Excel value ({number}, {text}, {bool}, {error}, {empty}, {array}, or a JSON literal)")
    }

    fn visit_unit<E: de::Error>(self) -> Result<Self::Value, E> {
        Ok(ExcelValue::Empty)
    }

    fn visit_none<E: de::Error>(self) -> Result<Self::Value, E> {
        Ok(ExcelValue::Empty)
    }

    fn visit_bool<E: de::Error>(self, v: bool) -> Result<Self::Value, E> {
        Ok(ExcelValue::Bool(v))
    }

    fn visit_i64<E: de::Error>(self, v: i64) -> Result<Self::Value, E> {
        Ok(ExcelValue::Number(v as f64))
    }

    fn visit_u64<E: de::Error>(self, v: u64) -> Result<Self::Value, E> {
        Ok(ExcelValue::Number(v as f64))
    }

    fn visit_f64<E: de::Error>(self, v: f64) -> Result<Self::Value, E> {
        Ok(ExcelValue::Number(v))
    }

    fn visit_str<E: de::Error>(self, v: &str) -> Result<Self::Value, E> {
        if let Some(err) = ExcelError::parse(v) {
            if v.starts_with('#') {
                return Ok(ExcelValue::Error(err));
            }
        }
        Ok(ExcelValue::Text(v.to_string()))
    }

    fn visit_seq<A: de::SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
        let mut rows = Vec::new();
        while let Some(row) = seq.next_element::<Vec<ExcelValue>>()? {
            rows.push(row);
        }
        Ok(ExcelValue::Array(rows))
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
        let mut seen: Option<ExcelValue> = None;
        while let Some(key) = map.next_key::<String>()? {
            let value = match key.as_str() {
                "empty" => {
                    let _: de::IgnoredAny = map.next_value()?;
                    ExcelValue::Empty
                }
                "number" => ExcelValue::Number(map.next_value()?),
                "text" => ExcelValue::Text(map.next_value()?),
                "bool" => ExcelValue::Bool(map.next_value()?),
                "error" => ExcelValue::Error(map.next_value()?),
                "array" => ExcelValue::Array(map.next_value()?),
                other => {
                    return Err(de::Error::unknown_field(
                        other,
                        &["empty", "number", "text", "bool", "error", "array"],
                    ));
                }
            };
            if seen.is_some() {
                return Err(de::Error::custom("Excel value maps must have a single tag"));
            }
            seen = Some(value);
        }
        seen.ok_or_else(|| de::Error::custom("empty Excel value object"))
    }
}

/// Round toward Excel's 15-significant-digit comparison / display model.
///
/// `0.1 + 0.2` stored as IEEE still compares equal to `0.3` in Excel. This is
/// an approximation of that crossover, not a bit-identical replica of the
/// Microsoft CRT rounding used by Excel.
pub fn excel_round_15(n: f64) -> f64 {
    if !n.is_finite() || n == 0.0 {
        return n;
    }
    let exp = n.abs().log10().floor();
    let digits = 14 - exp as i32;
    if !(-308..=308).contains(&digits) {
        return n;
    }
    let factor = 10f64.powi(digits);
    (n * factor).round() / factor
}

/// Excel-like numeric equality (15 significant digits).
pub fn excel_num_eq(a: f64, b: f64) -> bool {
    if a.is_nan() || b.is_nan() {
        return false;
    }
    if a == b {
        return true;
    }
    excel_round_15(a) == excel_round_15(b)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_roundtrip_tagged() {
        let cases = [
            ExcelValue::Number(3.0),
            ExcelValue::Text("hi".into()),
            ExcelValue::Bool(true),
            ExcelValue::Error(ExcelError::Div0),
            ExcelValue::Empty,
        ];
        for v in cases {
            let s = serde_json::to_string(&v).unwrap();
            let back: ExcelValue = serde_json::from_str(&s).unwrap();
            assert_eq!(back, v, "roundtrip {s}");
        }
    }

    #[test]
    fn json_accepts_literals() {
        let n: ExcelValue = serde_json::from_str("3").unwrap();
        assert_eq!(n, ExcelValue::Number(3.0));
        let e: ExcelValue = serde_json::from_str("null").unwrap();
        assert_eq!(e, ExcelValue::Empty);
        let err: ExcelValue = serde_json::from_str("{\"error\":\"#VALUE!\"}").unwrap();
        assert_eq!(err, ExcelValue::Error(ExcelError::Value));
    }

    #[test]
    fn fuzzy_eq_matches_excel_crossover() {
        assert!(excel_num_eq(0.1 + 0.2, 0.3));
        assert!(!excel_num_eq(1.0, 2.0));
    }
}
