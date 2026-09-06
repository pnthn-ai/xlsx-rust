//! Per-function Criterion bench: `SEARCH` case-insensitive + wildcards.
//!
//! Long cells measure that evaluate does not collect `Vec<char>` or clone
//! the haystack. The MAP column case times many scalar SEARCH calls.
//!
//! ```text
//! cargo bench -p xlsx-bench --bench fn_search
//! XLSX_BENCH_LARGE=1 cargo bench -p xlsx-bench --bench fn_search
//! ```

use xlsx_bench::prelude::*;
use xlsx_bench::{formula_spec, large_range_enabled, SnippetBuilder};
use xlsx_types::ExcelValue;

fn spec_cell(case_id: &str, formula: &str, text: String) -> EvalSpec {
    let mut b = SnippetBuilder::new("Sheet1");
    b.set(0, 0, ExcelValue::Text(text));
    formula_spec(case_id, formula, b.finish().workbook)
}

fn spec_literal(case_id: &str, formula: &str) -> EvalSpec {
    formula_spec(case_id, formula, Default::default())
}

fn bench_search(c: &mut Criterion) {
    bench_fn(c, "SEARCH", |g| {
        let miss = "x".repeat(200_000);
        let spec_miss = spec_cell("search.ascii_200k_miss", "=SEARCH(\"NEEDLE\", A1)", miss);
        g.throughput(Throughput::Bytes(200_000));
        g.bench_function("cell_ascii_200k_miss_ci", |b| {
            b.iter(|| black_box(eval_calc_core(black_box(&spec_miss))))
        });

        let mut late = "x".repeat(200_000);
        late.push_str("needle");
        let spec_late = spec_cell("search.ascii_200k_late", "=SEARCH(\"NEEDLE\", A1)", late);
        g.bench_function("cell_ascii_200k_late_ci", |b| {
            b.iter(|| black_box(eval_calc_core(black_box(&spec_late))))
        });

        let mut one_byte = "x".repeat(200_000);
        one_byte.push('z');
        let spec_z = spec_cell("search.ascii_200k_z", "=SEARCH(\"Z\", A1)", one_byte);
        g.bench_function("cell_ascii_200k_one_byte_ci", |b| {
            b.iter(|| black_box(eval_calc_core(black_box(&spec_z))))
        });

        let almost = "aaa".repeat(80_000) + "aab";
        let spec_almost = spec_cell("search.almost_aab", "=SEARCH(\"AAB\", A1)", almost);
        g.bench_function("cell_almost_match_aab_ci", |b| {
            b.iter(|| black_box(eval_calc_core(black_box(&spec_almost))))
        });

        let cafe = "cafe".repeat(50_000) + "café";
        let spec_cafe = spec_cell("search.cafe_200k", "=SEARCH(\"É\", A1)", cafe);
        g.bench_function("cell_unicode_cafe", |b| {
            b.iter(|| black_box(eval_calc_core(black_box(&spec_cafe))))
        });

        let wild_miss = "x".repeat(20_000);
        let spec_lead = spec_cell(
            "search.lead_star_miss",
            "=SEARCH(\"*NEEDLE\", A1)",
            wild_miss.clone(),
        );
        g.bench_function("cell_lead_star_miss_20k", |b| {
            b.iter(|| black_box(eval_calc_core(black_box(&spec_lead))))
        });

        let spec_ab = spec_cell("search.a_star_b_miss", "=SEARCH(\"a*b\", A1)", wild_miss);
        g.bench_function("cell_a_star_b_miss_20k", |b| {
            b.iter(|| black_box(eval_calc_core(black_box(&spec_ab))))
        });

        let spec_ms = spec_literal("search.ms_n", "=SEARCH(\"n\", \"printer\")");
        g.bench_function("literal_microsoft_n", |b| {
            b.iter(|| black_box(eval_calc_core(black_box(&spec_ms))))
        });

        let spec_empty = spec_cell(
            "search.empty_200k",
            "=SEARCH(\"\", A1, 50000)",
            "x".repeat(200_000),
        );
        g.bench_function("cell_empty_find_text", |b| {
            b.iter(|| black_box(eval_calc_core(black_box(&spec_empty))))
        });

        let mut col = SnippetBuilder::with_capacity("Sheet1", 10_000);
        col.fill_rect(xlsx_types::CellAddr::new(0, 0), 10_000, 1, |r, _| {
            Some(ExcelValue::Text(format!("Cat{r}")))
        });
        let filled = col.finish();
        let spec_map = formula_spec(
            "search.map_10k",
            "=SUM(MAP(A1:A10000, LAMBDA(x, SEARCH(\"a\", x))))",
            filled.workbook,
        );
        g.throughput(Throughput::Elements(10_000));
        g.bench_function("map_10k", |b| {
            b.iter(|| black_box(eval_calc_core(black_box(&spec_map))))
        });

        if large_range_enabled() {
            let huge = "x".repeat(1_000_000) + "needle";
            let spec_1m = spec_cell("search.ascii_1m_late", "=SEARCH(\"NEEDLE\", A1)", huge);
            g.throughput(Throughput::Bytes(1_000_000));
            g.bench_function("cell_ascii_1m_late_ci", |b| {
                b.iter(|| black_box(eval_calc_core(black_box(&spec_1m))))
            });
        }
    });
}

criterion_group!(benches, bench_search);
criterion_main!(benches);
