//! Per-function Criterion bench: `EXPAND` over large numeric columns.
//!
//! ```text
//! cargo bench -p xlsx-bench --bench fn_expand
//! XLSX_BENCH_LARGE=1 cargo bench -p xlsx-bench --bench fn_expand
//! ```

use xlsx_bench::prelude::*;
use xlsx_bench::{formula_spec, large_range_enabled, RANGE_100K, RANGE_10K};

fn bench_expand(c: &mut Criterion) {
    bench_fn(c, "EXPAND", |g| {
        let range_10k = numeric_column(RANGE_10K, |i| (i + 1) as f64);
        let spec_cols = range_10k.spec(
            "expand.10k.x8",
            format!("=EXPAND({}, 10000, 8, 0)", range_10k.a1_range()),
        );
        g.throughput(Throughput::Elements(range_10k.cell_count));
        g.bench_function("col_10k_to_10k_x8", |b| {
            b.iter(|| black_box(eval_calc_core(black_box(&spec_cols))))
        });

        let spec_rows = range_10k.spec(
            "expand.10k.to12k",
            format!("=EXPAND({}, 12000, 1, 0)", range_10k.a1_range()),
        );
        g.bench_function("col_10k_to_12k_x1", |b| {
            b.iter(|| black_box(eval_calc_core(black_box(&spec_rows))))
        });

        let spec_na = range_10k.spec(
            "expand.10k.x4.na",
            format!("=EXPAND({}, 10000, 4)", range_10k.a1_range()),
        );
        g.bench_function("col_10k_to_10k_x4_na", |b| {
            b.iter(|| black_box(eval_calc_core(black_box(&spec_na))))
        });

        if large_range_enabled() {
            let range_100k = numeric_column(RANGE_100K, |i| (i + 1) as f64);
            let spec_100k = formula_spec(
                "expand.100k.x2",
                format!("=EXPAND({}, 100000, 2, 0)", range_100k.a1_range()),
                range_100k.workbook.clone(),
            );
            g.throughput(Throughput::Elements(range_100k.cell_count));
            g.bench_function("col_100k_to_100k_x2", |b| {
                b.iter(|| black_box(eval_calc_core(black_box(&spec_100k))))
            });
        }
    });
}

criterion_group!(benches, bench_expand);
criterion_main!(benches);
