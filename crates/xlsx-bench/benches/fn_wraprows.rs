//! Per-function Criterion bench: `WRAPROWS` over large numeric columns.
//!
//! ```text
//! cargo bench -p xlsx-bench --bench fn_wraprows
//! XLSX_BENCH_LARGE=1 cargo bench -p xlsx-bench --bench fn_wraprows
//! ```

use xlsx_bench::prelude::*;
use xlsx_bench::{formula_spec, large_range_enabled, RANGE_100K, RANGE_10K};

fn bench_wraprows(c: &mut Criterion) {
    bench_fn(c, "WRAPROWS", |g| {
        let range_10k = numeric_column(RANGE_10K, |i| (i + 1) as f64);
        let spec_8 = range_10k.spec(
            "wraprows.10k.wrap8",
            format!("=WRAPROWS({}, 8)", range_10k.a1_range()),
        );
        g.throughput(Throughput::Elements(range_10k.cell_count));
        g.bench_function("col_10k_wrap8", |b| {
            b.iter(|| black_box(eval_calc_core(black_box(&spec_8))))
        });

        let spec_1 = range_10k.spec(
            "wraprows.10k.wrap1",
            format!("=WRAPROWS({}, 1)", range_10k.a1_range()),
        );
        g.bench_function("col_10k_wrap1", |b| {
            b.iter(|| black_box(eval_calc_core(black_box(&spec_1))))
        });

        let spec_pad = range_10k.spec(
            "wraprows.10k.wrap7.pad0",
            format!("=WRAPROWS({}, 7, 0)", range_10k.a1_range()),
        );
        g.bench_function("col_10k_wrap7_pad0", |b| {
            b.iter(|| black_box(eval_calc_core(black_box(&spec_pad))))
        });

        if large_range_enabled() {
            let range_100k = numeric_column(RANGE_100K, |i| (i + 1) as f64);
            let spec_100k = formula_spec(
                "wraprows.100k.wrap16",
                format!("=WRAPROWS({}, 16)", range_100k.a1_range()),
                range_100k.workbook.clone(),
            );
            g.throughput(Throughput::Elements(range_100k.cell_count));
            g.bench_function("col_100k_wrap16", |b| {
                b.iter(|| black_box(eval_calc_core(black_box(&spec_100k))))
            });
        }
    });
}

criterion_group!(benches, bench_wraprows);
criterion_main!(benches);
