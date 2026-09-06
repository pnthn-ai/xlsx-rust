//! Per-function Criterion bench: `LEN` Compat v2 scalar count.
//!
//! Long cells measure the SWAR count (no `Vec<char>`). The MAP column
//! case times many scalar LEN calls.
//!
//! ```text
//! cargo bench -p xlsx-bench --bench fn_len
//! XLSX_BENCH_LARGE=1 cargo bench -p xlsx-bench --bench fn_len
//! ```

use xlsx_bench::prelude::*;
use xlsx_bench::{formula_spec, large_range_enabled, SnippetBuilder};
use xlsx_types::ExcelValue;

fn spec_cell(case_id: &str, text: String) -> EvalSpec {
    let mut b = SnippetBuilder::new("Sheet1");
    b.set(0, 0, ExcelValue::Text(text));
    let filled = b.finish();
    formula_spec(case_id, "=LEN(A1)", filled.workbook)
}

fn bench_len(c: &mut Criterion) {
    bench_fn(c, "LEN", |g| {
        let ascii_200k = "x".repeat(200_000);
        let spec_ascii = spec_cell("len.ascii_200k", ascii_200k);
        g.throughput(Throughput::Bytes(200_000));
        g.bench_function("cell_ascii_200k", |b| {
            b.iter(|| black_box(eval_calc_core(black_box(&spec_ascii))))
        });

        let cafe_200k = "é".repeat(100_000);
        let spec_cafe = spec_cell("len.cafe_200k", cafe_200k);
        g.bench_function("cell_latin1_200k", |b| {
            b.iter(|| black_box(eval_calc_core(black_box(&spec_cafe))))
        });

        let cjk = "中".repeat(66_667);
        let spec_cjk = spec_cell("len.cjk_200k", cjk);
        g.bench_function("cell_cjk_200k", |b| {
            b.iter(|| black_box(eval_calc_core(black_box(&spec_cjk))))
        });

        let emoji = "😀".repeat(50_000);
        let spec_emoji = spec_cell("len.emoji_50k", emoji);
        g.bench_function("cell_emoji_50k", |b| {
            b.iter(|| black_box(eval_calc_core(black_box(&spec_emoji))))
        });

        let spec_ms = spec_cell("len.ms_phoenix", "Phoenix, AZ".into());
        g.bench_function("cell_microsoft_phoenix", |b| {
            b.iter(|| black_box(eval_calc_core(black_box(&spec_ms))))
        });

        let mut col = SnippetBuilder::with_capacity("Sheet1", 10_000);
        col.fill_rect(xlsx_types::CellAddr::new(0, 0), 10_000, 1, |r, _| {
            Some(ExcelValue::Text(format!("A{r}")))
        });
        let filled = col.finish();
        let spec_map = formula_spec(
            "len.map_10k",
            "=SUM(MAP(A1:A10000, LAMBDA(x, LEN(x))))",
            filled.workbook,
        );
        g.throughput(Throughput::Elements(10_000));
        g.bench_function("map_10k", |b| {
            b.iter(|| black_box(eval_calc_core(black_box(&spec_map))))
        });

        if large_range_enabled() {
            let huge = "x".repeat(1_000_000);
            let spec_1m = spec_cell("len.ascii_1m", huge);
            g.throughput(Throughput::Bytes(1_000_000));
            g.bench_function("cell_ascii_1m", |b| {
                b.iter(|| black_box(eval_calc_core(black_box(&spec_1m))))
            });
        }
    });
}

criterion_group!(benches, bench_len);
criterion_main!(benches);
