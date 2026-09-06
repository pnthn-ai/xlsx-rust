//! Per-function Criterion bench: `REPLACE` over large haystacks.
//!
//! Long cells measure that evaluate does not collect `Vec<char>`.
//! The MAP column case times many scalar REPLACE calls.
//!
//! ```text
//! cargo bench -p xlsx-bench --bench fn_replace
//! XLSX_BENCH_LARGE=1 cargo bench -p xlsx-bench --bench fn_replace
//! ```

use xlsx_bench::prelude::*;
use xlsx_bench::{formula_spec, large_range_enabled, SnippetBuilder};
use xlsx_types::ExcelValue;

fn spec_cell(case_id: &str, formula: &str, text: String) -> EvalSpec {
    let mut b = SnippetBuilder::new("Sheet1");
    b.set(0, 0, ExcelValue::Text(text));
    let filled = b.finish();
    formula_spec(case_id, formula, filled.workbook)
}

fn bench_replace(c: &mut Criterion) {
    bench_fn(c, "REPLACE", |g| {
        let ascii_200k = "a".repeat(200_000);
        let spec_mid = spec_cell(
            "replace.ascii_200k_mid",
            "=REPLACE(A1,100000,1,\"b\")",
            ascii_200k.clone(),
        );
        g.throughput(Throughput::Bytes(200_000));
        g.bench_function("cell_ascii_200k_mid1", |b| {
            b.iter(|| black_box(eval_calc_core(black_box(&spec_mid))))
        });

        let spec_whole = spec_cell(
            "replace.ascii_200k_whole",
            "=REPLACE(A1,1,200000,\"Z\")",
            ascii_200k.clone(),
        );
        g.bench_function("cell_ascii_200k_whole", |b| {
            b.iter(|| black_box(eval_calc_core(black_box(&spec_whole))))
        });

        let spec_append = spec_cell(
            "replace.ascii_200k_append",
            "=REPLACE(A1,200001,1,\"Z\")",
            ascii_200k.clone(),
        );
        g.bench_function("cell_ascii_200k_append", |b| {
            b.iter(|| black_box(eval_calc_core(black_box(&spec_append))))
        });

        let spec_noop = spec_cell(
            "replace.ascii_200k_noop",
            "=REPLACE(A1,1,0,\"\")",
            ascii_200k,
        );
        g.bench_function("cell_ascii_200k_noop", |b| {
            b.iter(|| black_box(eval_calc_core(black_box(&spec_noop))))
        });

        let spec_cafe = spec_cell(
            "replace.cafe_50k",
            "=REPLACE(A1,25000,1,\"e\")",
            "é".repeat(50_000),
        );
        g.bench_function("cell_latin1_50k_mid", |b| {
            b.iter(|| black_box(eval_calc_core(black_box(&spec_cafe))))
        });

        let spec_emoji = spec_cell(
            "replace.emoji_10k",
            "=REPLACE(A1,5000,1,\"X\")",
            "😀".repeat(10_000),
        );
        g.bench_function("cell_emoji_10k_mid", |b| {
            b.iter(|| black_box(eval_calc_core(black_box(&spec_emoji))))
        });

        let spec_ms = spec_cell(
            "replace.ms_abcdefghijk",
            "=REPLACE(A1,6,5,\"*\")",
            "abcdefghijk".into(),
        );
        g.bench_function("cell_microsoft_star", |b| {
            b.iter(|| black_box(eval_calc_core(black_box(&spec_ms))))
        });

        let mut col = SnippetBuilder::with_capacity("Sheet1", 10_000);
        col.fill_rect(xlsx_types::CellAddr::new(0, 0), 10_000, 1, |r, _| {
            Some(ExcelValue::Text(format!("row{r:04}")))
        });
        let filled = col.finish();
        let spec_map = formula_spec(
            "replace.map_10k",
            "=TEXTJOIN(\",\",TRUE,MAP(A1:A10000,LAMBDA(x,REPLACE(x,1,1,\"X\"))))",
            filled.workbook,
        );
        g.throughput(Throughput::Elements(10_000));
        g.bench_function("map_10k", |b| {
            b.iter(|| black_box(eval_calc_core(black_box(&spec_map))))
        });

        if large_range_enabled() {
            let spec_1m = spec_cell(
                "replace.ascii_1m_mid",
                "=REPLACE(A1,500000,1,\"b\")",
                "a".repeat(1_000_000),
            );
            g.throughput(Throughput::Bytes(1_000_000));
            g.bench_function("cell_ascii_1m_mid1", |b| {
                b.iter(|| black_box(eval_calc_core(black_box(&spec_1m))))
            });
        }
    });
}

criterion_group!(benches, bench_replace);
criterion_main!(benches);
