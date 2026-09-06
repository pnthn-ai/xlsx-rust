//! Per-function Criterion bench: `DOLLAR` on scalar / cell numbers.
//!
//! ```text
//! cargo bench -p xlsx-bench --bench fn_dollar
//! XLSX_BENCH_LARGE=1 cargo bench -p xlsx-bench --bench fn_dollar
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

fn bench_dollar(c: &mut Criterion) {
    bench_fn(c, "DOLLAR", |g| {
        let spec_ms = spec_literal("dollar.literal_ms", "=DOLLAR(1234.567, 2)");
        g.bench_function("literal_ms_2", |b| {
            b.iter(|| black_box(eval_calc_core(black_box(&spec_ms))))
        });

        let spec_omitted = spec_literal("dollar.literal_omitted", "=DOLLAR(99.888)");
        g.bench_function("literal_omitted", |b| {
            b.iter(|| black_box(eval_calc_core(black_box(&spec_omitted))))
        });

        let spec_neg = spec_literal("dollar.literal_neg", "=DOLLAR(-1234.567)");
        g.bench_function("literal_neg_parens", |b| {
            b.iter(|| black_box(eval_calc_core(black_box(&spec_neg))))
        });

        let spec_neg2 = spec_literal("dollar.literal_neg2", "=DOLLAR(1234.567, -2)");
        g.bench_function("literal_neg_decimals", |b| {
            b.iter(|| black_box(eval_calc_core(black_box(&spec_neg2))))
        });

        let spec_cell_n = spec_cell(
            "dollar.cell_n",
            "=DOLLAR(A1)",
            ExcelValue::Number(-1234.56),
        );
        g.bench_function("cell_neg", |b| {
            b.iter(|| black_box(eval_calc_core(black_box(&spec_cell_n))))
        });

        let spec_text = spec_cell(
            "dollar.cell_text",
            "=DOLLAR(A1)",
            ExcelValue::Text("1234.5".into()),
        );
        g.bench_function("cell_text_num", |b| {
            b.iter(|| black_box(eval_calc_core(black_box(&spec_text))))
        });

        let spec_junk = spec_literal("dollar.literal_junk", "=DOLLAR(\"x\")");
        g.bench_function("literal_junk_value", |b| {
            b.iter(|| black_box(eval_calc_core(black_box(&spec_junk))))
        });

        if large_range_enabled() {
            let spec_map = spec_literal(
                "dollar.map_makearray",
                "=COUNTA(MAP(MAKEARRAY(40,40,LAMBDA(r,c,r-c)),LAMBDA(x,DOLLAR(x))))",
            );
            g.bench_function("map_makearray_40x40", |b| {
                b.iter(|| black_box(eval_calc_core(black_box(&spec_map))))
            });
        }
    });
}

criterion_group!(benches, bench_dollar);
criterion_main!(benches);
