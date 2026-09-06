//! Criterion harness for Excel `ISOMITTED`.
//!
//! Times `calc-core` evaluate of IIFE LAMBDA / MAKEARRAY formulas that
//! call `ISOMITTED`. Setup stays outside `iter`.

use xlsx_bench::prelude::*;
use xlsx_bench::snippet::formula_spec;
use xlsx_types::Workbook;

fn bench_isomitted(c: &mut Criterion) {
    bench_fn(c, "ISOMITTED", |g| {
        let wb = Workbook::default();
        let omitted = formula_spec(
            "isomitted.iife_omitted",
            "=LAMBDA(x,y,ISOMITTED(y))(1,)",
            wb.clone(),
        );
        g.bench_function("iife_omitted", |b| {
            b.iter(|| black_box(eval_calc_core(black_box(&omitted))))
        });

        let provided = formula_spec(
            "isomitted.iife_provided",
            "=LAMBDA(x,y,ISOMITTED(y))(1,2)",
            wb.clone(),
        );
        g.bench_function("iife_provided", |b| {
            b.iter(|| black_box(eval_calc_core(black_box(&provided))))
        });

        let grid = formula_spec(
            "isomitted.makearray_64",
            "=MAKEARRAY(64,64,LAMBDA(r,c,ISOMITTED(c)))",
            wb,
        );
        g.throughput(Throughput::Elements(64 * 64));
        g.bench_function("makearray_64x64", |b| {
            b.iter(|| black_box(eval_calc_core(black_box(&grid))))
        });
    });
}

criterion_group!(benches, bench_isomitted);
criterion_main!(benches);
