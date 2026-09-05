//! Per-function Criterion bench: Excel `WRAPCOLS`.
//!
//! ```text
//! cargo bench -p xlsx-bench --bench fn_wrapcols
//! XLSX_BENCH_LARGE=1 cargo bench -p xlsx-bench --bench fn_wrapcols
//! ```

use xlsx_bench::prelude::*;
use xlsx_bench::{formula_spec, large_range_enabled, RANGE_100K, RANGE_10K};

fn bench_wrapcols(c: &mut Criterion) {
    bench_fn(c, "WRAPCOLS", |g| {
        let range_10k = numeric_column(RANGE_10K, |i| (i + 1) as f64);
        let spec_even = range_10k.spec(
            "wrapcols.10k.wrap100",
            format!("=WRAPCOLS({},100)", range_10k.a1_range()),
        );
        g.throughput(Throughput::Elements(range_10k.cell_count));
        g.bench_function("col_10k_wrap_100", |b| {
            b.iter(|| black_box(eval_calc_core(black_box(&spec_even))))
        });

        let spec_pad = range_10k.spec(
            "wrapcols.10k.wrap7",
            format!("=WRAPCOLS({},7,\"x\")", range_10k.a1_range()),
        );
        g.bench_function("col_10k_wrap_7_pad", |b| {
            b.iter(|| black_box(eval_calc_core(black_box(&spec_pad))))
        });

        let spec_tall = range_10k.spec(
            "wrapcols.10k.single-col",
            format!("=WRAPCOLS({},10000)", range_10k.a1_range()),
        );
        g.bench_function("col_10k_wrap_ge_n", |b| {
            b.iter(|| black_box(eval_calc_core(black_box(&spec_tall))))
        });

        let mixed = mixed_column(RANGE_10K);
        let spec_mixed = formula_spec(
            "wrapcols.mixed_10k",
            format!("=WRAPCOLS({},50)", mixed.a1_range()),
            mixed.workbook.clone(),
        );
        g.bench_function("mixed_10k_wrap_50", |b| {
            b.iter(|| black_box(eval_calc_core(black_box(&spec_mixed))))
        });

        if large_range_enabled() {
            let range_100k = numeric_column(RANGE_100K, |i| (i + 1) as f64);
            let spec_100k = range_100k.spec(
                "wrapcols.100k.wrap200",
                format!("=WRAPCOLS({},200)", range_100k.a1_range()),
            );
            g.throughput(Throughput::Elements(range_100k.cell_count));
            g.bench_function("col_100k_wrap_200", |b| {
                b.iter(|| black_box(eval_calc_core(black_box(&spec_100k))))
            });
        }
    });
}

criterion_group!(benches, bench_wrapcols);
criterion_main!(benches);
