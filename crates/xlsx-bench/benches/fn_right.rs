//! Per-function Criterion bench: `RIGHT` on scalar / cell text.
//!
//! ```text
//! cargo bench -p xlsx-bench --bench fn_right
//! XLSX_BENCH_LARGE=1 cargo bench -p xlsx-bench --bench fn_right
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

fn bench_right(c: &mut Criterion) {
    bench_fn(c, "RIGHT", |g| {
        let spec_ms = spec_literal("right.ms_sale_price", "=RIGHT(\"Sale Price\", 5)");
        g.bench_function("literal_sale_price_5", |b| {
            b.iter(|| black_box(eval_calc_core(black_box(&spec_ms))))
        });

        let spec_default = spec_literal("right.default", "=RIGHT(\"Stock Number\")");
        g.bench_function("literal_default_1", |b| {
            b.iter(|| black_box(eval_calc_core(black_box(&spec_default))))
        });

        let spec_cell = spec_cell(
            "right.cell_hello",
            "=RIGHT(A1, 2)",
            ExcelValue::Text("Hello".into()),
        );
        g.bench_function("cell_hello_2", |b| {
            b.iter(|| black_box(eval_calc_core(black_box(&spec_cell))))
        });

        let spec_frac = spec_literal("right.trunc", "=RIGHT(\"Sale Price\", 4.9)");
        g.bench_function("literal_trunc_4_9", |b| {
            b.iter(|| black_box(eval_calc_core(black_box(&spec_frac))))
        });

        let spec_emoji = spec_literal("right.emoji", "=RIGHT(\"a😀b\", 2)");
        g.bench_function("literal_emoji_2", |b| {
            b.iter(|| black_box(eval_calc_core(black_box(&spec_emoji))))
        });

        let spec_neg = spec_literal("right.neg", "=RIGHT(\"abc\", -1)");
        g.bench_function("literal_neg_value", |b| {
            b.iter(|| black_box(eval_calc_core(black_box(&spec_neg))))
        });

        if large_range_enabled() {
            let spec_rept = spec_literal("right.rept_1000", "=LEN(RIGHT(REPT(\"a\", 1000), 5))");
            g.bench_function("len_rept_1000_last_5", |b| {
                b.iter(|| black_box(eval_calc_core(black_box(&spec_rept))))
            });
        }
    });
}

criterion_group!(benches, bench_right);
criterion_main!(benches);
