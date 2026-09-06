//! Per-function Criterion bench: Excel `IPMT` via calc-core evaluate.
//!
//! Scalar TVM — workbook/spec is built once; only `evaluate` is timed.
//!
//! ```text
//! cargo bench -p xlsx-bench --bench fn_ipmt
//! ```

use xlsx_bench::prelude::*;
use xlsx_types::Workbook;

fn bench_ipmt(c: &mut Criterion) {
    bench_fn(c, "IPMT", |g| {
        let wb = Workbook::default();

        let ms = formula_spec("ipmt.ms_month1", "=IPMT(10%/12, 1, 36, 8000)", wb.clone());
        g.bench_function("ms_month1", |b| {
            b.iter(|| black_box(eval_calc_core(black_box(&ms))))
        });

        let mid = formula_spec(
            "ipmt.mortgage_mid",
            "=IPMT(0.05/12, 180, 360, 200000)",
            wb.clone(),
        );
        g.bench_function("mortgage_mid", |b| {
            b.iter(|| black_box(eval_calc_core(black_box(&mid))))
        });

        let last = formula_spec(
            "ipmt.mortgage_last",
            "=IPMT(0.05/12, 360, 360, 200000)",
            wb.clone(),
        );
        g.bench_function("mortgage_last", |b| {
            b.iter(|| black_box(eval_calc_core(black_box(&last))))
        });

        let begin = formula_spec(
            "ipmt.mortgage_begin",
            "=IPMT(0.05/12, 2, 360, 200000, 0, 1)",
            wb,
        );
        g.bench_function("mortgage_begin_p2", |b| {
            b.iter(|| black_box(eval_calc_core(black_box(&begin))))
        });
    });
}

criterion_group!(benches, bench_ipmt);
criterion_main!(benches);
