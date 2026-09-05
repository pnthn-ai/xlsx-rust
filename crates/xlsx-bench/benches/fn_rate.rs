//! Per-function Criterion bench: Excel `RATE` on representative TVM formulas.
//!
//! ```text
//! cargo bench -p xlsx-bench --bench fn_rate
//! ```

use xlsx_bench::formula_spec;
use xlsx_bench::prelude::*;
use xlsx_types::Workbook;

fn bench_rate(c: &mut Criterion) {
    bench_fn(c, "RATE", |g| {
        let wb = Workbook::default();

        let ms = formula_spec("rate.ms_loan", "=RATE(4*12,-200,8000)", wb.clone());
        g.bench_function("ms_loan", |b| {
            b.iter(|| black_box(eval_calc_core(black_box(&ms))))
        });

        let invert = formula_spec(
            "rate.invert_pmt",
            "=RATE(10,PMT(0.1,10,1000),1000)",
            wb.clone(),
        );
        g.bench_function("invert_pmt", |b| {
            b.iter(|| black_box(eval_calc_core(black_box(&invert))))
        });

        let mortgage = formula_spec(
            "rate.mortgage_guess",
            "=RATE(360,PMT(0.05/12,360,200000),200000,0,0,0.01)",
            wb.clone(),
        );
        g.bench_function("mortgage_guess", |b| {
            b.iter(|| black_box(eval_calc_core(black_box(&mortgage))))
        });

        let one = formula_spec("rate.one_period", "=RATE(1,-110,100)", wb.clone());
        g.bench_function("one_period", |b| {
            b.iter(|| black_box(eval_calc_core(black_box(&one))))
        });

        let compound = formula_spec("rate.compound", "=RATE(10,0,-1000,2000)", wb);
        g.bench_function("pmt_zero_compound", |b| {
            b.iter(|| black_box(eval_calc_core(black_box(&compound))))
        });
    });
}

criterion_group!(benches, bench_rate);
criterion_main!(benches);
