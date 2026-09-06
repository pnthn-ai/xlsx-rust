//! Per-function Criterion bench: `MID` Unicode-scalar slice path.
//!
//! Long cells measure that evaluate slices UTF-8 instead of collecting
//! every scalar. The MAP column case times many scalar MID calls.
//!
//! ```text
//! cargo bench -p xlsx-bench --bench fn_mid
//! XLSX_BENCH_LARGE=1 cargo bench -p xlsx-bench --bench fn_mid
//! ```

use xlsx_bench::prelude::*;
use xlsx_bench::{formula_spec, large_range_enabled, SnippetBuilder};
use xlsx_types::ExcelValue;

fn spec_cell(case_id: &str, text: String, formula: &str) -> EvalSpec {
    let mut b = SnippetBuilder::new("Sheet1");
    b.set(0, 0, ExcelValue::Text(text));
    let filled = b.finish();
    formula_spec(case_id, formula, filled.workbook)
}

fn bench_mid(c: &mut Criterion) {
    bench_fn(c, "MID", |g| {
        let ascii_200k = "a".repeat(200_000);
        let spec_ascii = spec_cell("mid.ascii_200k_mid1", ascii_200k, "=MID(A1, 100000, 1)");
        g.throughput(Throughput::Bytes(200_000));
        g.bench_function("cell_ascii_200k_mid1", |b| {
            b.iter(|| black_box(eval_calc_core(black_box(&spec_ascii))))
        });

        let spec_prefix = spec_cell(
            "mid.ascii_200k_prefix",
            "a".repeat(200_000),
            "=MID(A1, 1, 8)",
        );
        g.bench_function("cell_ascii_200k_prefix", |b| {
            b.iter(|| black_box(eval_calc_core(black_box(&spec_prefix))))
        });

        let spec_past = spec_cell(
            "mid.ascii_200k_past",
            "a".repeat(200_000),
            "=MID(A1, 200001, 1)",
        );
        g.bench_function("cell_ascii_200k_past_end", |b| {
            b.iter(|| black_box(eval_calc_core(black_box(&spec_past))))
        });

        let spec_cafe = spec_cell("mid.cafe_50k", "é".repeat(50_000), "=MID(A1, 25000, 1)");
        g.bench_function("cell_latin1_50k_mid1", |b| {
            b.iter(|| black_box(eval_calc_core(black_box(&spec_cafe))))
        });

        let spec_emoji = spec_cell("mid.emoji_10k", "😀".repeat(10_000), "=MID(A1, 5000, 1)");
        g.bench_function("cell_emoji_10k_mid1", |b| {
            b.iter(|| black_box(eval_calc_core(black_box(&spec_emoji))))
        });

        let spec_ms = formula_spec(
            "mid.ms_fluid",
            "=MID(\"Fluid Flow\", 1, 5)",
            xlsx_types::Workbook::default(),
        );
        g.bench_function("literal_microsoft_fluid", |b| {
            b.iter(|| black_box(eval_calc_core(black_box(&spec_ms))))
        });

        let mut col = SnippetBuilder::with_capacity("Sheet1", 10_000);
        col.fill_rect(xlsx_types::CellAddr::new(0, 0), 10_000, 1, |r, _| {
            Some(ExcelValue::Text(format!("A{r:04}")))
        });
        let filled = col.finish();
        let spec_map = formula_spec(
            "mid.map_10k",
            "=TEXTJOIN(\"\",TRUE,MAP(A1:A10000, LAMBDA(x, MID(x, 2, 1))))",
            filled.workbook,
        );
        g.throughput(Throughput::Elements(10_000));
        g.bench_function("map_10k", |b| {
            b.iter(|| black_box(eval_calc_core(black_box(&spec_map))))
        });

        if large_range_enabled() {
            let huge = "a".repeat(1_000_000);
            let spec_1m = spec_cell("mid.ascii_1m_mid1", huge, "=MID(A1, 500000, 1)");
            g.throughput(Throughput::Bytes(1_000_000));
            g.bench_function("cell_ascii_1m_mid1", |b| {
                b.iter(|| black_box(eval_calc_core(black_box(&spec_1m))))
            });
        }
    });
}

criterion_group!(benches, bench_mid);
criterion_main!(benches);
