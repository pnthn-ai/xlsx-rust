//! Criterion harness for Excel `MAP`.
//!
//! Times `calc-core` evaluate of `MAP(array, LAMBDA(…))`.
//! Setup (spec construction) stays outside `iter`.

use xlsx_bench::prelude::*;
use xlsx_bench::snippet::formula_spec;

fn bench_map(c: &mut Criterion) {
    bench_fn(c, "MAP", |g| {
        let col = numeric_column(10_000, |i| (i + 1) as f64);
        let spec_times2 = formula_spec(
            "map.10k.times2",
            format!("=MAP({},LAMBDA(x,x*2))", col.a1_range()),
            col.workbook.clone(),
        );
        g.throughput(Throughput::Elements(col.cell_count));
        g.bench_function("times2_10k", |b| {
            b.iter(|| black_box(eval_calc_core(black_box(&spec_times2))))
        });

        let spec_add = formula_spec(
            "map.10k.add",
            format!(
                "=MAP({},SEQUENCE(10000,1,10,10),LAMBDA(a,b,a+b))",
                col.a1_range()
            ),
            col.workbook.clone(),
        );
        g.throughput(Throughput::Elements(col.cell_count));
        g.bench_function("add_seq_10k", |b| {
            b.iter(|| black_box(eval_calc_core(black_box(&spec_add))))
        });

        let spec_if = formula_spec(
            "map.10k.if",
            format!("=MAP({},LAMBDA(x,IF(x>5000,x,0)))", col.a1_range()),
            col.workbook,
        );
        g.throughput(Throughput::Elements(10_000));
        g.bench_function("if_10k", |b| {
            b.iter(|| black_box(eval_calc_core(black_box(&spec_if))))
        });
    });
}

criterion_group!(benches, bench_map);
criterion_main!(benches);
