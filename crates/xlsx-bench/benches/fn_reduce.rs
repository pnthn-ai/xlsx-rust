//! Criterion harness for Excel `REDUCE`.
//!
//! Times `calc-core` evaluate of `REDUCE(initial, range, LAMBDA(a,b,…))`.
//! Setup (spec construction) stays outside `iter`.

use xlsx_bench::prelude::*;

fn bench_reduce(c: &mut Criterion) {
    bench_fn(c, "REDUCE", |g| {
        let col_1k = numeric_column(1_024, |i| (i + 1) as f64);
        let spec_sum = col_1k.spec(
            "reduce.1k.sum",
            format!("=REDUCE(0,{},LAMBDA(a,b,a+b))", col_1k.a1_range()),
        );
        g.throughput(Throughput::Elements(1_024));
        g.bench_function("sum_1k", |b| {
            b.iter(|| black_box(eval_calc_core(black_box(&spec_sum))))
        });

        let col_4k = numeric_column(4_096, |i| (i + 1) as f64);
        let spec_sum_4k = col_4k.spec(
            "reduce.4k.sum",
            format!("=REDUCE(0,{},LAMBDA(a,b,a+b))", col_4k.a1_range()),
        );
        g.throughput(Throughput::Elements(4_096));
        g.bench_function("sum_4k", |b| {
            b.iter(|| black_box(eval_calc_core(black_box(&spec_sum_4k))))
        });

        let spec_if = col_1k.spec(
            "reduce.1k.if",
            format!("=REDUCE(0,{},LAMBDA(a,b,IF(b>0,a+b,a)))", col_1k.a1_range()),
        );
        g.throughput(Throughput::Elements(1_024));
        g.bench_function("if_pos_1k", |b| {
            b.iter(|| black_box(eval_calc_core(black_box(&spec_if))))
        });
    });
}

criterion_group!(benches, bench_reduce);
criterion_main!(benches);
