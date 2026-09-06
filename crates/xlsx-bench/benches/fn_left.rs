//! Per-function Criterion bench: `LEFT` on scalars / cells.
//!
//! ```text
//! cargo bench -p xlsx-bench --bench fn_left
//! XLSX_BENCH_LARGE=1 cargo bench -p xlsx-bench --bench fn_left
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

fn bench_left(c: &mut Criterion) {
    bench_fn(c, "LEFT", |g| {
        let spec_ms = spec_literal("left.ms_sale", "=LEFT(\"Sale Price\", 4)");
        g.bench_function("literal_sale_4", |b| {
            b.iter(|| black_box(eval_calc_core(black_box(&spec_ms))))
        });

        let spec_default = spec_literal("left.default", "=LEFT(\"Sweden\")");
        g.bench_function("literal_default_1", |b| {
            b.iter(|| black_box(eval_calc_core(black_box(&spec_default))))
        });

        let spec_cell = spec_cell(
            "left.cell_text",
            "=LEFT(A1, 3)",
            ExcelValue::Text("abcdef".into()),
        );
        g.bench_function("cell_text_3", |b| {
            b.iter(|| black_box(eval_calc_core(black_box(&spec_cell))))
        });

        let spec_num = spec_cell("left.cell_num", "=LEFT(A1, 2)", ExcelValue::Number(12345.0));
        g.bench_function("cell_number_2", |b| {
            b.iter(|| black_box(eval_calc_core(black_box(&spec_num))))
        });

        let spec_emoji = spec_literal("left.emoji", "=LEFT(\"😀abc\", 1)");
        g.bench_function("literal_emoji_1", |b| {
            b.iter(|| black_box(eval_calc_core(black_box(&spec_emoji))))
        });

        let spec_neg = spec_literal("left.neg", "=LEFT(\"abc\", -1)");
        g.bench_function("literal_neg_value", |b| {
            b.iter(|| black_box(eval_calc_core(black_box(&spec_neg))))
        });

        if large_range_enabled() {
            let spec_long = spec_cell(
                "left.long_ascii",
                "=LEFT(A1, 16)",
                ExcelValue::Text("x".repeat(10_000)),
            );
            g.bench_function("cell_10k_ascii_16", |b| {
                b.iter(|| black_box(eval_calc_core(black_box(&spec_long))))
            });
        }
    });
}

criterion_group!(benches, bench_left);
criterion_main!(benches);
