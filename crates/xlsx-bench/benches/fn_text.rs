//! Per-function Criterion bench: Excel `TEXT` on scalar cells.
//!
//! ```text
//! cargo bench -p xlsx-bench --bench fn_text
//! XLSX_BENCH_LARGE=1 cargo bench -p xlsx-bench --bench fn_text
//! ```

use xlsx_bench::prelude::*;
use xlsx_bench::{formula_spec, large_range_enabled, SnippetBuilder};
use xlsx_types::ExcelValue;

fn spec_num(case_id: &str, n: f64, formula: &str) -> EvalSpec {
    let mut b = SnippetBuilder::new("Sheet1");
    b.set(0, 0, ExcelValue::Number(n));
    let filled = b.finish();
    formula_spec(case_id, formula, filled.workbook)
}

fn spec_text(case_id: &str, text: &str, formula: &str) -> EvalSpec {
    let mut b = SnippetBuilder::new("Sheet1");
    b.set(0, 0, ExcelValue::Text(text.into()));
    let filled = b.finish();
    formula_spec(case_id, formula, filled.workbook)
}

fn bench_text(c: &mut Criterion) {
    bench_fn(c, "TEXT", |g| {
        let spec_fixed = spec_num("text.fixed_0.00", 1234.567, "=TEXT(A1,\"0.00\")");
        g.bench_function("cell_0.00", |b| {
            b.iter(|| black_box(eval_calc_core(black_box(&spec_fixed))))
        });

        let spec_grp = spec_num("text.grouped", 1_234_567.89, "=TEXT(A1,\"#,##0.00\")");
        g.bench_function("cell_grouped", |b| {
            b.iter(|| black_box(eval_calc_core(black_box(&spec_grp))))
        });

        let spec_cur = spec_num("text.currency", 1234.567, "=TEXT(A1,\"$#,##0.00\")");
        g.bench_function("cell_currency", |b| {
            b.iter(|| black_box(eval_calc_core(black_box(&spec_cur))))
        });

        let spec_pct = spec_num("text.pct", 0.285, "=TEXT(A1,\"0.00%\")");
        g.bench_function("cell_percent", |b| {
            b.iter(|| black_box(eval_calc_core(black_box(&spec_pct))))
        });

        let spec_iso = spec_num("text.iso", 45366.0, "=TEXT(A1,\"yyyy-mm-dd\")");
        g.bench_function("cell_iso_date", |b| {
            b.iter(|| black_box(eval_calc_core(black_box(&spec_iso))))
        });

        let spec_mdy = spec_num("text.mdy", 45366.0, "=TEXT(A1,\"mm/dd/yyyy\")");
        g.bench_function("cell_mdy_date", |b| {
            b.iter(|| black_box(eval_calc_core(black_box(&spec_mdy))))
        });

        let spec_at = spec_num("text.at", 1234.5, "=TEXT(A1,\"@\")");
        g.bench_function("cell_at", |b| {
            b.iter(|| black_box(eval_calc_core(black_box(&spec_at))))
        });

        let spec_txt = spec_text("text.nonnum", "abc", "=TEXT(A1,\"0.00\")");
        g.bench_function("cell_non_numeric", |b| {
            b.iter(|| black_box(eval_calc_core(black_box(&spec_txt))))
        });

        if large_range_enabled() {
            let spec_pad = spec_num("text.pad", 1234.0, "=TEXT(A1,\"0000000\")");
            g.bench_function("cell_pad_0000000", |b| {
                b.iter(|| black_box(eval_calc_core(black_box(&spec_pad))))
            });
        }
    });
}

criterion_group!(benches, bench_text);
criterion_main!(benches);
