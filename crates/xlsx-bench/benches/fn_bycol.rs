//! Criterion harness for Excel `BYCOL`.
//!
//! Times `calc-core` evaluate of `BYCOL(range, LAMBDA(c,SUM(c)))`.
//! Setup (spec construction) stays outside `iter`.

use xlsx_bench::prelude::*;
use xlsx_bench::snippet::numeric_grid;

fn bench_bycol(c: &mut Criterion) {
    bench_fn(c, "BYCOL", |g| {
        let grid_64 = numeric_grid(64, 64, |r, col| ((r * 64 + col + 1) % 17) as f64);
        let spec_64 = grid_64.spec(
            "bycol.64x64.sum",
            format!("=BYCOL({},LAMBDA(c,SUM(c)))", grid_64.a1_range()),
        );
        g.throughput(Throughput::Elements(64 * 64));
        g.bench_function("sum_64x64", |b| {
            b.iter(|| black_box(eval_calc_core(black_box(&spec_64))))
        });

        let grid_128 = numeric_grid(128, 64, |r, col| ((r * 64 + col + 1) % 17) as f64);
        let spec_128 = grid_128.spec(
            "bycol.128x64.sum",
            format!("=BYCOL({},LAMBDA(c,SUM(c)))", grid_128.a1_range()),
        );
        g.throughput(Throughput::Elements(128 * 64));
        g.bench_function("sum_128x64", |b| {
            b.iter(|| black_box(eval_calc_core(black_box(&spec_128))))
        });

        let spec_if = grid_64.spec(
            "bycol.64x64.if",
            format!("=BYCOL({},LAMBDA(c,IF(SUM(c)>8,1,0)))", grid_64.a1_range()),
        );
        g.throughput(Throughput::Elements(64 * 64));
        g.bench_function("if_sum_64x64", |b| {
            b.iter(|| black_box(eval_calc_core(black_box(&spec_if))))
        });
    });
}

criterion_group!(benches, bench_bycol);
criterion_main!(benches);
