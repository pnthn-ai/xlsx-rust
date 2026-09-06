//! Per-function Criterion bench: `ABS` on scalar / cell numbers.
//!
//! ```text
//! cargo bench -p xlsx-bench --bench fn_abs
//! XLSX_BENCH_LARGE=1 cargo bench -p xlsx-bench --bench fn_abs
//! ```

use xlsx_bench::prelude::*;
use xlsx_bench::{formula_spec, large_range_enabled, SnippetBuilder};
use xlsx_types::ExcelValue;

fn spec_literal(case_id: &str, formula: &str) -> EvalSpec {
    formula_spec(case_id, formula, Default::default())
}

fn spec_cell(case_id: &str, formula: &str, value: ExcelValue) -> EvalSpec {
    let mut b = SnippetBuilder::new("Sheet1");
    b.set(0, 0, value);
    formula_spec(case_id, formula, b.finish().workbook)
}

fn bench_abs(c: &mut Criterion) {
    bench_fn(c, "ABS", |g| {
        let spec_neg = spec_literal("abs.literal_neg7", "=ABS(-7)");
        g.bench_function("literal_neg7", |b| {
            b.iter(|| black_box(eval_calc_core(black_box(&spec_neg))))
        });

        let spec_pos = spec_literal("abs.literal_2", "=ABS(2)");
        g.bench_function("literal_pos2", |b| {
            b.iter(|| black_box(eval_calc_core(black_box(&spec_pos))))
        });

        let spec_cell_neg = spec_cell("abs.cell_neg", "=ABS(A1)", ExcelValue::Number(-123.45));
        g.bench_function("cell_neg", |b| {
            b.iter(|| black_box(eval_calc_core(black_box(&spec_cell_neg))))
        });

        let spec_text = spec_cell(
            "abs.cell_text_neg",
            "=ABS(A1)",
            ExcelValue::Text("-7".into()),
        );
        g.bench_function("cell_text_neg", |b| {
            b.iter(|| black_box(eval_calc_core(black_box(&spec_text))))
        });

        let spec_true = spec_literal("abs.literal_true", "=ABS(TRUE)");
        g.bench_function("literal_true", |b| {
            b.iter(|| black_box(eval_calc_core(black_box(&spec_true))))
        });

        let spec_junk = spec_literal("abs.literal_junk", "=ABS(\"x\")");
        g.bench_function("literal_junk_value", |b| {
            b.iter(|| black_box(eval_calc_core(black_box(&spec_junk))))
        });

        if large_range_enabled() {
            let spec_map = spec_literal(
                "abs.map_makearray",
                "=SUM(MAP(MAKEARRAY(100,100,LAMBDA(r,c,r-c)),LAMBDA(x,ABS(x))))",
            );
            g.bench_function("map_makearray_100x100", |b| {
                b.iter(|| black_box(eval_calc_core(black_box(&spec_map))))
            });
        }
    });
}

criterion_group!(benches, bench_abs);
criterion_main!(benches);
