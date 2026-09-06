//! Per-function Criterion bench: `HSTACK` over large ranges.
//!
//! ```text
//! cargo bench -p xlsx-bench --bench fn_hstack
//! XLSX_BENCH_LARGE=1 cargo bench -p xlsx-bench --bench fn_hstack
//! ```

use xlsx_bench::prelude::*;
use xlsx_bench::{formula_spec, large_range_enabled, numeric_grid, RANGE_100K, RANGE_10K};

fn bench_hstack(c: &mut Criterion) {
    bench_fn(c, "HSTACK", |g| {
        let equal = numeric_grid(RANGE_10K, 2, |r, c| (r * 2 + c + 1) as f64);
        let spec_equal = formula_spec(
            "hstack.equal_10k",
            "=HSTACK(A1:A10000,B1:B10000)",
            equal.workbook.clone(),
        );
        g.throughput(Throughput::Elements(equal.cell_count));
        g.bench_function("two_cols_10k", |b| {
            b.iter(|| black_box(eval_calc_core(black_box(&spec_equal))))
        });

        let pad = numeric_grid(RANGE_10K, 2, |r, c| (r * 2 + c + 1) as f64);
        let spec_pad = formula_spec(
            "hstack.pad_10k",
            "=HSTACK(A1:A10000,B1:B100)",
            pad.workbook,
        );
        g.bench_function("pad_10k_x_100", |b| {
            b.iter(|| black_box(eval_calc_core(black_box(&spec_pad))))
        });

        let wide = numeric_grid(1_000, 8, |r, c| (r * 8 + c) as f64);
        let spec_wide = formula_spec(
            "hstack.wide_1k",
            "=HSTACK(A1:B1000,C1:D1000,E1:F1000,G1:H1000)",
            wide.workbook,
        );
        g.bench_function("four_pairs_1k", |b| {
            b.iter(|| black_box(eval_calc_core(black_box(&spec_wide))))
        });

        if large_range_enabled() {
            let large = numeric_grid(RANGE_100K, 2, |r, c| (r * 2 + c + 1) as f64);
            let spec_large = formula_spec(
                "hstack.equal_100k",
                "=HSTACK(A1:A100000,B1:B100000)",
                large.workbook,
            );
            g.throughput(Throughput::Elements(large.cell_count));
            g.bench_function("two_cols_100k", |b| {
                b.iter(|| black_box(eval_calc_core(black_box(&spec_large))))
            });
        }
    });
}

criterion_group!(benches, bench_hstack);
criterion_main!(benches);
