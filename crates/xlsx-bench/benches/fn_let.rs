//! Criterion harness for Excel `LET`.
//!
//! Times `calc-core` evaluate of `LET(s, SUM(range), s+s+…)`.
//! Setup (range fill, spec) stays outside `iter`.

use xlsx_bench::prelude::*;

fn bench_let(c: &mut Criterion) {
    bench_fn(c, "LET", |g| {
        let range = numeric_column(10_000, |i| (i + 1) as f64);
        let spec = formula_spec(
            "let.sum.10k.reuse4",
            &format!("=LET(s, SUM({}), s+s+s+s)", range.a1_range()),
            range.workbook.clone(),
        );
        g.throughput(Throughput::Elements(range.cell_count));
        g.bench_function("sum_10k_reuse4", |b| {
            b.iter(|| black_box(eval_calc_core(black_box(&spec))))
        });

        let spec_pair = formula_spec(
            "let.sum.10k.pair",
            &format!("=LET(s, SUM({}), t, s*s, t+s)", range.a1_range()),
            range.workbook.clone(),
        );
        g.throughput(Throughput::Elements(range.cell_count));
        g.bench_function("sum_10k_pair", |b| {
            b.iter(|| black_box(eval_calc_core(black_box(&spec_pair))))
        });
    });
}

criterion_group!(benches, bench_let);
criterion_main!(benches);
