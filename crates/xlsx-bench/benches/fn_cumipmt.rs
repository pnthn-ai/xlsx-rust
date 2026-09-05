//! Per-function Criterion bench: Excel `CUMIPMT` via calc-core evaluate.
//!
//! Scalar TVM — workbook/spec is built once; only `evaluate` is timed.
//!
//! ```text
//! cargo bench -p xlsx-bench --bench fn_cumipmt
//! ```

use xlsx_bench::prelude::*;
use xlsx_types::Workbook;

fn bench_cumipmt(c: &mut Criterion) {
    bench_fn(c, "CUMIPMT", |g| {
        let wb = Workbook::default();

        let ms = formula_spec(
            "cumipmt.ms_year2",
            "=CUMIPMT(9%/12, 360, 125000, 13, 24, 0)",
            wb.clone(),
        );
        g.bench_function("ms_year2", |b| {
            b.iter(|| black_box(eval_calc_core(black_box(&ms))))
        });

        let first = formula_spec(
            "cumipmt.ms_month1",
            "=CUMIPMT(9%/12, 360, 125000, 1, 1, 0)",
            wb.clone(),
        );
        g.bench_function("ms_month1", |b| {
            b.iter(|| black_box(eval_calc_core(black_box(&first))))
        });

        let full = formula_spec(
            "cumipmt.mortgage_full",
            "=CUMIPMT(0.05/12, 360, 200000, 1, 360, 0)",
            wb.clone(),
        );
        g.bench_function("mortgage_full", |b| {
            b.iter(|| black_box(eval_calc_core(black_box(&full))))
        });

        let begin = formula_spec(
            "cumipmt.mortgage_begin_y1",
            "=CUMIPMT(0.05/12, 360, 200000, 1, 12, 1)",
            wb,
        );
        g.bench_function("mortgage_begin_y1", |b| {
            b.iter(|| black_box(eval_calc_core(black_box(&begin))))
        });
    });
}

criterion_group!(benches, bench_cumipmt);
criterion_main!(benches);
