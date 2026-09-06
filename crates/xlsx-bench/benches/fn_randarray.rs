//! Per-function Criterion bench: Excel `RANDARRAY`.
//!
//! ```text
//! cargo bench -p xlsx-bench --bench fn_randarray
//! XLSX_BENCH_LARGE=1 cargo bench -p xlsx-bench --bench fn_randarray
//! ```

use xlsx_bench::prelude::*;
use xlsx_bench::{formula_spec, large_range_enabled, RANGE_100K, RANGE_10K};
use xlsx_types::Workbook;

fn bench_randarray(c: &mut Criterion) {
    bench_fn(c, "RANDARRAY", |g| {
        let spec_10k = formula_spec(
            "randarray.col_10k",
            format!("=RANDARRAY({RANGE_10K},1)"),
            Workbook::default(),
        );
        g.throughput(Throughput::Elements(RANGE_10K as u64));
        g.bench_function("col_10k", |b| {
            b.iter(|| black_box(eval_calc_core(black_box(&spec_10k))))
        });

        let spec_int = formula_spec(
            "randarray.grid_100x100_int",
            "=RANDARRAY(100,100,1,100,TRUE)",
            Workbook::default(),
        );
        g.throughput(Throughput::Elements(10_000));
        g.bench_function("grid_100x100_int", |b| {
            b.iter(|| black_box(eval_calc_core(black_box(&spec_int))))
        });

        let spec_const = formula_spec(
            "randarray.const_10k",
            format!("=RANDARRAY({RANGE_10K},1,7,7,TRUE)"),
            Workbook::default(),
        );
        g.bench_function("const_10k", |b| {
            b.iter(|| black_box(eval_calc_core(black_box(&spec_const))))
        });

        if large_range_enabled() {
            let spec_100k = formula_spec(
                "randarray.col_100k",
                format!("=RANDARRAY({RANGE_100K},1)"),
                Workbook::default(),
            );
            g.throughput(Throughput::Elements(RANGE_100K as u64));
            g.bench_function("col_100k", |b| {
                b.iter(|| black_box(eval_calc_core(black_box(&spec_100k))))
            });
        }
    });
}

criterion_group!(benches, bench_randarray);
criterion_main!(benches);
