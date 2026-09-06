//! Catalog of Excel compatibility quirk categories.
//!
//! Not every category is exercised by the seed corpus. The list exists so
//! fixtures, candidates, and future subagents share a vocabulary.

use serde::{Deserialize, Serialize};
use std::fmt;

/// A named Excel compatibility quirk a fixture may exercise.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum QuirkCategory {
    /// IEEE vs Excel 15-digit comparison / display crossover.
    IeeeVsExcelPrecision,
    /// Blank cells behaving as `0` *and* `""` depending on operator.
    EmptyCellDuality,
    /// Text vs number vs logical ranking for `<` / `>` (logical > text > number).
    TypeComparisonRanking,
    /// `"2"=2` is FALSE but `"2"+1` is `3`.
    EqualityVsArithmeticCoercion,
    /// Case-insensitive text equality (`"A"="a"`).
    CaseInsensitiveText,
    /// `TRUE=1` / `FALSE=0`.
    BoolNumberEquality,
    /// `SUM` ignores logicals/text in ranges but coerces scalar arguments.
    SumArgVsRange,
    /// `VLOOKUP` approximate match assumes an ascending sort.
    VlookupApproximateUnsorted,
    /// `IF` short-circuits; unused branch errors do not fire.
    /// `IFS` does **not** short-circuit (unused pair errors still fire).
    IfShortCircuit,
    /// Date serials and the 1900 leap-year bug (documented, not implemented).
    Date1900LeapYear,
    /// 1900 vs 1904 date system.
    DateSystem,
    /// Implicit intersection of a range in a scalar context.
    ImplicitIntersection,
    /// Dynamic array / CSE / scalar evaluation mode.
    /// `FILTER` / `UNIQUE` / `SORT` / `BYROW` return an array value; worksheet spill / `#SPILL!` is not modeled.
    /// `FILTER` / `UNIQUE` / `SORTBY` return an array value; worksheet spill / `#SPILL!` is not modeled.
    /// `FILTER` / `UNIQUE` / `TOCOL` return an array value; worksheet spill /
    /// `FILTER` / `UNIQUE` / `TOROW` return an array value; worksheet spill /
    /// `#SPILL!` is not modeled.
    /// `FILTER` / `UNIQUE` / `SEQUENCE` return an array value; worksheet
    /// spill / `#SPILL!` is not modeled.
    /// `FILTER` / `VSTACK` / `UNIQUE` return an array value; worksheet spill /
    /// `#SPILL!` is not modeled. `VSTACK` width-pads with `#N/A`.
    /// `FILTER` / `WRAPCOLS` return an array value; worksheet spill / `#SPILL!` is not modeled.
    /// `FILTER` / `WRAPROWS` / `UNIQUE` return an array value; worksheet
    /// spill / `#SPILL!` from occupancy is not modeled.
    /// `FILTER` / `HSTACK` return an array value; worksheet spill / `#SPILL!` is not modeled.
    /// `FILTER` / `UNIQUE` / `TAKE` return an array value; worksheet spill / `#SPILL!` is not modeled.
    /// `FILTER` / `CHOOSECOLS` return an array value; worksheet spill / `#SPILL!` is not modeled.
    /// `FILTER` / `UNIQUE` / `DROP` return an array value; worksheet spill /
    /// `#SPILL!` is not modeled.
    /// `FILTER` / `EXPAND` / `UNIQUE` return an array value; worksheet
    /// spill / `#SPILL!` from occupancy is not modeled.
    /// `FILTER` / `CHOOSEROWS` return an array value; worksheet spill / `#SPILL!` is not modeled.
    /// `FILTER` / `UNIQUE` / `TEXTSPLIT` return an array value; worksheet
    /// spill / `#SPILL!` is not modeled. TEXTSPLIT pad cells are `#N/A`.
    /// `MAP` returns an array value; worksheet spill / `#SPILL!` is not
    /// modeled. Unequal MAP arrays union-pad with `#N/A` (no broadcast).
    ArrayEvalMode,
    /// Volatile functions (`NOW`, `RAND`, `RANDARRAY`, `INDIRECT`, …).
    Volatile,
    /// Locale argument separators and decimal commas.
    Locale,
    /// Circular references.
    CircularReference,
    /// Precision as displayed.
    PrecisionAsDisplayed,
    /// Hidden-row / `SUBTOTAL` semantics.
    HiddenRows,
    /// Wildcard matching in `VLOOKUP` / `COUNTIF` / `COUNTIFS` / `MATCH` / `SEARCH`.
    Wildcards,
    /// Left-to-right Excel error propagation (`#DIV/0!+#VALUE!` keeps `#DIV/0!`).
    ErrorPrecedence,
    /// Unary `+`/`-` and the postfix `%` operator (including `--"2"` coercion).
    PercentUnary,
    /// Space intersection / comma union / `#NULL!` when ranges do not overlap.
    RangeOperators,
    /// Other / not yet classified.
    Other,
}

impl QuirkCategory {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::IeeeVsExcelPrecision => "ieee-vs-excel-precision",
            Self::EmptyCellDuality => "empty-cell-duality",
            Self::TypeComparisonRanking => "type-comparison-ranking",
            Self::EqualityVsArithmeticCoercion => "equality-vs-arithmetic-coercion",
            Self::CaseInsensitiveText => "case-insensitive-text",
            Self::BoolNumberEquality => "bool-number-equality",
            Self::SumArgVsRange => "sum-arg-vs-range",
            Self::VlookupApproximateUnsorted => "vlookup-approximate-unsorted",
            Self::IfShortCircuit => "if-short-circuit",
            Self::Date1900LeapYear => "date-1900-leap-year",
            Self::DateSystem => "date-system",
            Self::ImplicitIntersection => "implicit-intersection",
            Self::ArrayEvalMode => "array-eval-mode",
            Self::Volatile => "volatile",
            Self::Locale => "locale",
            Self::CircularReference => "circular-reference",
            Self::PrecisionAsDisplayed => "precision-as-displayed",
            Self::HiddenRows => "hidden-rows",
            Self::Wildcards => "wildcards",
            Self::ErrorPrecedence => "error-precedence",
            Self::PercentUnary => "percent-unary",
            Self::RangeOperators => "range-operators",
            Self::Other => "other",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Some(
            match s.trim().to_ascii_lowercase().replace('_', "-").as_str() {
                "ieee-vs-excel-precision" | "ieee" => Self::IeeeVsExcelPrecision,
                "empty-cell-duality" | "empty" => Self::EmptyCellDuality,
                "type-comparison-ranking" | "type-rank" => Self::TypeComparisonRanking,
                "equality-vs-arithmetic-coercion" | "coercion" => {
                    Self::EqualityVsArithmeticCoercion
                }
                "case-insensitive-text" | "casefold" => Self::CaseInsensitiveText,
                "bool-number-equality" | "bool-eq" => Self::BoolNumberEquality,
                "sum-arg-vs-range" | "agg-arg-vs-range" => Self::SumArgVsRange,
                "vlookup-approximate-unsorted" => Self::VlookupApproximateUnsorted,
                "if-short-circuit" => Self::IfShortCircuit,
                "date-1900-leap-year" => Self::Date1900LeapYear,
                "date-system" => Self::DateSystem,
                "implicit-intersection" => Self::ImplicitIntersection,
                "array-eval-mode" | "array" => Self::ArrayEvalMode,
                "volatile" => Self::Volatile,
                "locale" => Self::Locale,
                "circular-reference" | "circular" => Self::CircularReference,
                "precision-as-displayed" => Self::PrecisionAsDisplayed,
                "hidden-rows" => Self::HiddenRows,
                "wildcards" => Self::Wildcards,
                "error-precedence" | "error-propagation" => Self::ErrorPrecedence,
                "percent-unary" | "unary-percent" => Self::PercentUnary,
                "range-operators" | "intersection" | "union" => Self::RangeOperators,
                "other" => Self::Other,
                _ => return None,
            },
        )
    }
}

impl fmt::Display for QuirkCategory {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}
