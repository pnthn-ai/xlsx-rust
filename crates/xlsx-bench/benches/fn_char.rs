//! Per-function Criterion bench: `CHAR` on scalar / cell codes.
//!
//! ```text
//! cargo bench -p xlsx-bench --bench fn_char
//! XLSX_BENCH_LARGE=1 cargo bench -p xlsx-bench --bench fn_char
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

fn bench_char(c: &mut Criterion) {
    bench_fn(c, "CHAR", |g| {
        let spec_a = spec_literal("char.literal_65", "=CHAR(65)");
        g.bench_function("literal_65", |b| {
            b.iter(|| black_box(eval_calc_core(black_box(&spec_a))))
        });

        let spec_euro = spec_literal("char.literal_128", "=CHAR(128)");
        g.bench_function("literal_128_euro", |b| {
            b.iter(|| black_box(eval_calc_core(black_box(&spec_euro))))
        });

        let spec_cell65 = spec_cell("char.cell_65", "=CHAR(A1)", ExcelValue::Number(65.0));
        g.bench_function("cell_65", |b| {
            b.iter(|| black_box(eval_calc_core(black_box(&spec_cell65))))
        });

        let spec_frac = spec_cell("char.cell_128_7", "=CHAR(A1)", ExcelValue::Number(128.7));
        g.bench_function("cell_128_7_trunc", |b| {
            b.iter(|| black_box(eval_calc_core(black_box(&spec_frac))))
        });

        let spec_text = spec_cell(
            "char.cell_text_255",
            "=CHAR(A1)",
            ExcelValue::Text("255".into()),
        );
        g.bench_function("cell_text_255", |b| {
            b.iter(|| black_box(eval_calc_core(black_box(&spec_text))))
        });

        let spec_oob = spec_literal("char.literal_0", "=CHAR(0)");
        g.bench_function("literal_0_value", |b| {
            b.iter(|| black_box(eval_calc_core(black_box(&spec_oob))))
        });

        if large_range_enabled() {
            let spec_clean = spec_literal(
                "char.clean_tab_lf",
                "=CLEAN(CHAR(9)&\"Monthly report\"&CHAR(10))",
            );
            g.bench_function("clean_char9_char10", |b| {
                b.iter(|| black_box(eval_calc_core(black_box(&spec_clean))))
            });
        }
    });
}

criterion_group!(benches, bench_char);
criterion_main!(benches);
