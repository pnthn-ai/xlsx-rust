//! Criterion harness for Excel `MAKEARRAY`.
//!
//! Times `calc-core` evaluate of `MAKEARRAY(rows, cols, LAMBDA(r,c,…))`.
//! Setup (spec construction) stays outside `iter`.

use xlsx_bench::prelude::*;
use xlsx_bench::snippet::formula_spec;
use xlsx_types::Workbook;

fn bench_makearray(c: &mut Criterion) {
    bench_fn(c, "MAKEARRAY", |g| {
        let wb = Workbook::default();
        let spec_64 = formula_spec(
            "makearray.64x64.mul",
            "=MAKEARRAY(64,64,LAMBDA(r,c,r*c))",
            wb.clone(),
        );
        g.throughput(Throughput::Elements(64 * 64));
        g.bench_function("mul_64x64", |b| {
            b.iter(|| black_box(eval_calc_core(black_box(&spec_64))))
        });

        let spec_128 = formula_spec(
            "makearray.128x128.mul",
            "=MAKEARRAY(128,128,LAMBDA(r,c,r*c))",
            wb.clone(),
        );
        g.throughput(Throughput::Elements(128 * 128));
        g.bench_function("mul_128x128", |b| {
            b.iter(|| black_box(eval_calc_core(black_box(&spec_128))))
        });

        let spec_if = formula_spec(
            "makearray.64x64.if",
            "=MAKEARRAY(64,64,LAMBDA(r,c,IF(r=c,1,0)))",
            wb,
        );
        g.throughput(Throughput::Elements(64 * 64));
        g.bench_function("if_diag_64x64", |b| {
            b.iter(|| black_box(eval_calc_core(black_box(&spec_if))))
        });
    });
}

criterion_group!(benches, bench_makearray);
criterion_main!(benches);
