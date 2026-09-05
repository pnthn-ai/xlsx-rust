//! Example per-function Criterion bench: `SUM` over large numeric ranges.
//!
//! Pattern later agents should copy:
//! 1. File name `benches/fn_<name>.rs` + `[[bench]] name = "fn_<name>"`.
//! 2. `bench_fn(c, "NAME", |g| { ... })`.
//! 3. Build the snippet **once** with `xlsx_bench::snippet` helpers.
//! 4. Time only `evaluate` (`eval_calc_core` / `Candidate::evaluate`).
//!
//! ```text
//! cargo bench -p xlsx-bench --bench fn_sum
//! XLSX_BENCH_LARGE=1 cargo bench -p xlsx-bench --bench fn_sum
//! ```

use xlsx_bench::prelude::*;
use xlsx_bench::{large_range_enabled, RANGE_100K, RANGE_10K};

fn bench_sum(c: &mut Criterion) {
    bench_fn(c, "SUM", |g| {
        // 10k numeric column — default hot-path size.
        let range_10k = numeric_column(RANGE_10K, |i| (i + 1) as f64);
        let spec_10k = range_10k.call_spec("sum.range_10k", "SUM");
        g.throughput(Throughput::Elements(range_10k.cell_count));
        g.bench_function("range_10k", |b| {
            b.iter(|| black_box(eval_calc_core(black_box(&spec_10k))))
        });

        // Mixed column: numbers / blanks / text / bools (SUM skip semantics).
        let mixed = mixed_column(RANGE_10K);
        let spec_mixed = mixed.call_spec("sum.mixed_10k", "SUM");
        g.bench_function("mixed_10k", |b| {
            b.iter(|| black_box(eval_calc_core(black_box(&spec_mixed))))
        });

        // 100k is opt-in: slower Criterion run, same helpers.
        if large_range_enabled() {
            let range_100k = numeric_column(RANGE_100K, |i| (i + 1) as f64);
            let spec_100k = range_100k.call_spec("sum.range_100k", "SUM");
            g.throughput(Throughput::Elements(range_100k.cell_count));
            g.bench_function("range_100k", |b| {
                b.iter(|| black_box(eval_calc_core(black_box(&spec_100k))))
            });
        }
    });
}

criterion_group!(benches, bench_sum);
criterion_main!(benches);
