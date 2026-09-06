//! Per-function Criterion bench: Excel `PPMT` through calc-core evaluate.
//!
//! Scalar TVM — time representative formulas, not a giant range fold.
//!
//! ```text
//! cargo bench -p xlsx-bench --bench fn_ppmt
//! ```

use xlsx_bench::prelude::*;
use xlsx_types::Workbook;

fn spec(id: &str, formula: &str) -> EvalSpec {
    formula_spec(id, formula, Workbook::default())
}

fn bench_ppmt(c: &mut Criterion) {
    bench_fn(c, "PPMT", |g| {
        let ms_m1 = spec("ppmt.ms_m1", "=PPMT(10%/12,1,24,2000)");
        g.bench_function("ms_loan_month1", |b| {
            b.iter(|| black_box(eval_calc_core(black_box(&ms_m1))))
        });

        let mortgage = spec("ppmt.mortgage_m180", "=PPMT(0.05/12,180,360,200000)");
        g.bench_function("mortgage_month180", |b| {
            b.iter(|| black_box(eval_calc_core(black_box(&mortgage))))
        });

        let horizon = spec("ppmt.long_m600", "=PPMT(0.05/12,600,1200,100000)");
        g.bench_function("horizon_100y_month600", |b| {
            b.iter(|| black_box(eval_calc_core(black_box(&horizon))))
        });

        let begin = spec("ppmt.begin_m1", "=PPMT(0.05/12,1,360,200000,0,1)");
        g.bench_function("mortgage_type1_month1", |b| {
            b.iter(|| black_box(eval_calc_core(black_box(&begin))))
        });
    });
}

criterion_group!(benches, bench_ppmt);
criterion_main!(benches);
