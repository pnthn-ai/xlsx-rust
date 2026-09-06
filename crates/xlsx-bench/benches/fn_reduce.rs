//! Criterion harness for Excel `REDUCE`.
//!
//! Times `calc-core` evaluate of `REDUCE(initial, array, LAMBDA(a,b,…))`.
//! Setup (spec construction) stays outside `iter`.

use xlsx_bench::prelude::*;
use xlsx_bench::snippet::formula_spec;
use xlsx_types::Workbook;

fn lit_row(n: usize) -> String {
    (1..=n).map(|i| i.to_string()).collect::<Vec<_>>().join(",")
}

fn bench_reduce(c: &mut Criterion) {
    bench_fn(c, "REDUCE", |g| {
        let wb = Workbook::default();
        let row_1k = lit_row(1_024);
        let spec_sum = formula_spec(
            "reduce.1k.sum",
            &format!("=REDUCE(0,{{{row_1k}}},LAMBDA(a,b,a+b))"),
            wb.clone(),
        );
        g.throughput(Throughput::Elements(1_024));
        g.bench_function("sum_1k", |b| {
            b.iter(|| black_box(eval_calc_core(black_box(&spec_sum))))
        });

        let row_4k = lit_row(4_096);
        let spec_sum_4k = formula_spec(
            "reduce.4k.sum",
            &format!("=REDUCE(0,{{{row_4k}}},LAMBDA(a,b,a+b))"),
            wb.clone(),
        );
        g.throughput(Throughput::Elements(4_096));
        g.bench_function("sum_4k", |b| {
            b.iter(|| black_box(eval_calc_core(black_box(&spec_sum_4k))))
        });

        let spec_if = formula_spec(
            "reduce.1k.if",
            &format!("=REDUCE(0,{{{row_1k}}},LAMBDA(a,b,IF(b>0,a+b,a)))"),
            wb,
        );
        g.throughput(Throughput::Elements(1_024));
        g.bench_function("if_pos_1k", |b| {
            b.iter(|| black_box(eval_calc_core(black_box(&spec_if))))
        });
    });
}

criterion_group!(benches, bench_reduce);
criterion_main!(benches);
