//! Shared performance harness for per-function hill-climbing.
//!
//! # Convention
//!
//! Add **one Criterion bench file per Excel function**:
//!
//! ```text
//! crates/xlsx-bench/benches/fn_<name>.rs
//! ```
//!
//! `<name>` is the lowercase function id (`sum`, `vlookup`, `error_type`, …).
//! Register it in this crate’s `Cargo.toml`:
//!
//! ```toml
//! [[bench]]
//! name = "fn_sum"
//! harness = false
//! ```
//!
//! Inside the file, call [`bench_fn`] so the Criterion group is always
//! `fn_<name>` and later agents do not collide:
//!
//! ```ignore
//! use xlsx_bench::prelude::*;
//!
//! fn bench_sum(c: &mut Criterion) {
//!     bench_fn(c, "SUM", |g| {
//!         let range = numeric_column(10_000, |i| (i + 1) as f64);
//!         let spec = range.call_spec("sum.10k", "SUM");
//!         g.throughput(Throughput::Elements(range.cell_count));
//!         g.bench_function("range_10k", |b| {
//!             b.iter(|| black_box(eval_calc_core(&spec)))
//!         });
//!     });
//! }
//!
//! criterion_group!(benches, bench_sum);
//! criterion_main!(benches);
//! ```
//!
//! Build the workbook **once**, outside `iter`. Only [`xlsx_types::Candidate::evaluate`]
//! belongs on the timed path.
//!
//! # Correctness gate
//!
//! Timings are advisory. A faster `SUM` that fails
//! `xlsx-verify --candidate calc-core` is not a win. Run the gate before
//! claiming any improvement.
//!
//! # Machine-readable timings
//!
//! Criterion already writes `target/criterion/**/estimates.json`. For a
//! compact JSON/CSV snapshot (future Excel-oracle comparison, no live Excel
//! required) use `xlsx-bench-snapshot` or [`export`].

pub mod export;
pub mod snippet;

pub use export::{write_csv, write_json, BenchRecord, TimingReport};
pub use snippet::{
    formula_spec, grid, mixed_column, numeric_column, numeric_grid, FilledRange, SnippetBuilder,
};

use criterion::measurement::WallTime;
use criterion::{BenchmarkGroup, Criterion};
use xlsx_engine_core::CalcCoreEngine;
use xlsx_types::{Candidate, EvalSpec, ExcelValue};

/// Imports used by every `fn_*.rs` bench file.
pub mod prelude {
    pub use crate::{
        bench_fn, eval_calc_core, formula_spec, function_slug, mixed_column, numeric_column,
        numeric_grid,
    };
    pub use criterion::{black_box, criterion_group, criterion_main, Criterion, Throughput};
    pub use xlsx_engine_core::CalcCoreEngine;
    pub use xlsx_types::{Candidate, EvalSpec, ExcelValue};
}

/// `SUM` → `fn_sum`, `ERROR.TYPE` → `fn_error_type`, `fn_sum` stays `fn_sum`.
pub fn function_slug(name: &str) -> String {
    let n = name.trim().to_ascii_lowercase().replace(['.', ' '], "_");
    if n.starts_with("fn_") {
        n
    } else {
        format!("fn_{n}")
    }
}

/// Open a Criterion group named `fn_<NAME>` and hand it to `configure`.
///
/// This is the registration hook later agents should call — one function,
/// one file, one group.
pub fn bench_fn<F>(c: &mut Criterion, name: &str, configure: F)
where
    F: FnOnce(&mut BenchmarkGroup<'_, WallTime>),
{
    let slug = function_slug(name);
    let mut group = c.benchmark_group(&slug);
    configure(&mut group);
    group.finish();
}

/// Evaluate with the default `calc-core` candidate. Panics on infrastructure
/// failure so a broken bench fails loudly instead of silently timing errors.
pub fn eval_calc_core(spec: &EvalSpec) -> ExcelValue {
    CalcCoreEngine::new()
        .evaluate(spec)
        .expect("calc-core evaluate failed (infrastructure error, not an Excel value)")
}

/// Sizes the example benches use. `100_000` is opt-in via [`large_range_enabled`].
pub const RANGE_10K: u32 = 10_000;
pub const RANGE_100K: u32 = 100_000;

/// `XLSX_BENCH_LARGE=1` includes 100k-cell cases (slower; keep off in casual runs).
pub fn large_range_enabled() -> bool {
    matches!(
        std::env::var("XLSX_BENCH_LARGE").as_deref(),
        Ok("1") | Ok("true") | Ok("TRUE") | Ok("yes")
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use xlsx_types::ExcelValue;

    #[test]
    fn slug_convention() {
        assert_eq!(function_slug("SUM"), "fn_sum");
        assert_eq!(function_slug("fn_sum"), "fn_sum");
        assert_eq!(function_slug("ERROR.TYPE"), "fn_error_type");
        assert_eq!(function_slug(" XLOOKUP "), "fn_xlookup");
    }

    #[test]
    fn numeric_column_sum_matches_closed_form() {
        let n = 1_000u32;
        let range = numeric_column(n, |i| (i + 1) as f64);
        assert_eq!(range.cell_count, n as u64);
        assert_eq!(range.a1_range(), "A1:A1000");
        let spec = range.call_spec("sum.1k", "SUM");
        match eval_calc_core(&spec) {
            ExcelValue::Number(v) => {
                let expect = (n as f64) * (n as f64 + 1.0) / 2.0;
                assert!((v - expect).abs() < 1e-6, "{v} != {expect}");
            }
            other => panic!("expected number, got {other:?}"),
        }
    }

    #[test]
    fn mixed_column_sum_skips_non_numbers() {
        // Pattern: number(i+1), blank, text, bool — only numbers contribute.
        let n = 40u32;
        let range = mixed_column(n);
        let spec = range.call_spec("sum.mixed", "SUM");
        let ExcelValue::Number(v) = eval_calc_core(&spec) else {
            panic!("expected number");
        };
        let mut expect = 0.0;
        for i in 0..n {
            if i % 4 == 0 {
                expect += (i + 1) as f64;
            }
        }
        assert!((v - expect).abs() < 1e-9, "{v} != {expect}");
    }

    #[test]
    fn hstack_two_cols_is_wide_array() {
        let range = numeric_grid(4, 2, |r, c| (r * 10 + c) as f64);
        let spec = formula_spec(
            "hstack.4x2",
            "=HSTACK(A1:A4,B1:B4)",
            range.workbook,
        );
        match eval_calc_core(&spec) {
            ExcelValue::Array(rows) => {
                assert_eq!(rows.len(), 4);
                assert_eq!(rows[0], vec![ExcelValue::Number(0.0), ExcelValue::Number(1.0)]);
                assert_eq!(rows[3], vec![ExcelValue::Number(30.0), ExcelValue::Number(31.0)]);
            }
            other => panic!("expected array, got {other:?}"),
        }
    }

    #[test]
    fn grid_a1_and_count() {
        let range = numeric_grid(3, 2, |r, c| (r * 10 + c) as f64);
        assert_eq!(range.a1_range(), "A1:B3");
        assert_eq!(range.cell_count, 6);
        assert_eq!(range.call("AVERAGE"), "=AVERAGE(A1:B3)");
    }
}
