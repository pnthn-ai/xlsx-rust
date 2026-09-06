//! Criterion harness for Excel `SCAN`.
//!
//! Times `calc-core` evaluate of `SCAN(initial, array, LAMBDA(a,v,…))`.
//! Setup (spec construction) stays outside `iter`.

use xlsx_bench::prelude::*;
use xlsx_bench::snippet::formula_spec;
use xlsx_types::Workbook;

fn bench_scan(c: &mut Criterion) {
    bench_fn(c, "SCAN", |g| {
        let wb = Workbook::default();
        let spec_64 = formula_spec(
            "scan.64x64.sum",
            "=SCAN(0,SEQUENCE(64,64),LAMBDA(a,v,a+v))",
            wb.clone(),
        );
        g.throughput(Throughput::Elements(64 * 64));
        g.bench_function("sum_64x64", |b| {
            b.iter(|| black_box(eval_calc_core(black_box(&spec_64))))
        });

        let spec_128 = formula_spec(
            "scan.128x128.sum",
            "=SCAN(0,SEQUENCE(128,128),LAMBDA(a,v,a+v))",
            wb.clone(),
        );
        g.throughput(Throughput::Elements(128 * 128));
        g.bench_function("sum_128x128", |b| {
            b.iter(|| black_box(eval_calc_core(black_box(&spec_128))))
        });

        let spec_if = formula_spec(
            "scan.64x64.max",
            "=SCAN(0,SEQUENCE(64,64),LAMBDA(a,v,IF(a>v,a,v)))",
            wb,
        );
        g.throughput(Throughput::Elements(64 * 64));
        g.bench_function("running_max_64x64", |b| {
            b.iter(|| black_box(eval_calc_core(black_box(&spec_if))))
        });
    });
}

criterion_group!(benches, bench_scan);
criterion_main!(benches);
