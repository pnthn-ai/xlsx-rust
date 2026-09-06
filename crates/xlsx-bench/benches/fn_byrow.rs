//! Criterion harness for Excel `BYROW`.
//!
//! Times `calc-core` evaluate of `BYROW(range, LAMBDA(row, SUM(row)))`.
//! Setup (spec construction) stays outside `iter`.

use xlsx_bench::prelude::*;
use xlsx_bench::snippet::{formula_spec, numeric_grid};

fn bench_byrow(c: &mut Criterion) {
    bench_fn(c, "BYROW", |g| {
        let range = numeric_grid(1_024, 8, |r, c| (r * 8 + c + 1) as f64);
        let spec_sum = formula_spec(
            "byrow.1024x8.sum",
            format!("=BYROW({},LAMBDA(row,SUM(row)))", range.a1_range()),
            range.workbook.clone(),
        );
        g.throughput(Throughput::Elements(range.cell_count));
        g.bench_function("sum_1024x8", |b| {
            b.iter(|| black_box(eval_calc_core(black_box(&spec_sum))))
        });

        let spec_eta = formula_spec(
            "byrow.1024x8.eta",
            format!("=BYROW({},SUM)", range.a1_range()),
            range.workbook.clone(),
        );
        g.bench_function("eta_sum_1024x8", |b| {
            b.iter(|| black_box(eval_calc_core(black_box(&spec_eta))))
        });

        let spec_if = formula_spec(
            "byrow.512x8.if",
            format!(
                "=BYROW({},LAMBDA(row,IF(SUM(row)>10,1,0)))",
                range.a1_range()
            ),
            range.workbook,
        );
        g.bench_function("if_sum_1024x8", |b| {
            b.iter(|| black_box(eval_calc_core(black_box(&spec_if))))
        });
    });
}

criterion_group!(benches, bench_byrow);
criterion_main!(benches);
