//! Per-function Criterion bench: Excel `NPER`.
//!
//! NPER is a closed-form scalar. The interesting evaluate path is dispatch +
//! the `ln1p` kernel, not a range walk.
//!
//! ```text
//! cargo bench -p xlsx-bench --bench fn_nper
//! ```

use xlsx_bench::prelude::*;
use xlsx_bench::{formula_spec, numeric_column};
use xlsx_types::{ExcelValue, Workbook};

fn bench_nper(c: &mut Criterion) {
    bench_fn(c, "NPER", |g| {
        let spec_ms = formula_spec(
            "nper.ms",
            "=NPER(12%/12,-100,-1000,10000,1)",
            Workbook::default(),
        );
        g.bench_function("ms_example", |b| {
            b.iter(|| black_box(eval_calc_core(black_box(&spec_ms))))
        });

        let spec_invert = formula_spec(
            "nper.invert",
            "=NPER(0.05/12, PMT(0.05/12, 360, 200000), 200000)",
            Workbook::default(),
        );
        g.bench_function("invert_pmt", |b| {
            b.iter(|| black_box(eval_calc_core(black_box(&spec_invert))))
        });

        let spec_zero = formula_spec("nper.zero", "=NPER(0,-100,1000)", Workbook::default());
        g.bench_function("zero_rate", |b| {
            b.iter(|| black_box(eval_calc_core(black_box(&spec_zero))))
        });

        let spec_tiny = formula_spec(
            "nper.tiny",
            "=NPER(0.00000001,-300,100000)",
            Workbook::default(),
        );
        g.bench_function("tiny_rate", |b| {
            b.iter(|| black_box(eval_calc_core(black_box(&spec_tiny))))
        });

        // 10k rates in A; formula is still scalar (A1) — setup-scale only.
        let rates = numeric_column(10_000, |i| 0.01 + (i as f64) * 1e-6);
        let mut wb = rates.workbook;
        wb.sheets[0]
            .cells
            .insert("B1".into(), xlsx_types::Cell::value(ExcelValue::Number(-200.0)));
        wb.sheets[0]
            .cells
            .insert("C1".into(), xlsx_types::Cell::value(ExcelValue::Number(10_000.0)));
        let spec_cells = formula_spec("nper.cells", "=NPER(A1,B1,C1)", wb);
        g.throughput(Throughput::Elements(1));
        g.bench_function("cell_refs", |b| {
            b.iter(|| black_box(eval_calc_core(black_box(&spec_cells))))
        });
    });
}

criterion_group!(benches, bench_nper);
criterion_main!(benches);
