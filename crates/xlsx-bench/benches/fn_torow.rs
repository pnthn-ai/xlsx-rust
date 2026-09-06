//! Per-function Criterion bench: `TOROW` over large ranges.
//!
//! ```text
//! cargo bench -p xlsx-bench --bench fn_torow
//! XLSX_BENCH_LARGE=1 cargo bench -p xlsx-bench --bench fn_torow
//! ```

use xlsx_bench::prelude::*;
use xlsx_bench::{formula_spec, large_range_enabled, numeric_grid, RANGE_100K, RANGE_10K};

fn bench_torow(c: &mut Criterion) {
    bench_fn(c, "TOROW", |g| {
        let col_10k = numeric_column(RANGE_10K, |i| (i + 1) as f64);
        let spec_col = col_10k.call_spec("torow.col_10k", "TOROW");
        g.throughput(Throughput::Elements(col_10k.cell_count));
        g.bench_function("column_10k", |b| {
            b.iter(|| black_box(eval_calc_core(black_box(&spec_col))))
        });

        let grid_8k = numeric_grid(128, 64, |r, c| (r * 64 + c + 1) as f64);
        let spec_grid = grid_8k.call_spec("torow.grid_128x64", "TOROW");
        g.bench_function("grid_128x64", |b| {
            b.iter(|| black_box(eval_calc_core(black_box(&spec_grid))))
        });

        let spec_by_col = formula_spec(
            "torow.grid_128x64_by_col",
            format!("=TOROW({}, 0, TRUE)", grid_8k.a1_range()),
            grid_8k.workbook.clone(),
        );
        g.bench_function("grid_128x64_scan_by_col", |b| {
            b.iter(|| black_box(eval_calc_core(black_box(&spec_by_col))))
        });

        let mixed = mixed_column(RANGE_10K);
        let spec_ignore = formula_spec(
            "torow.mixed_10k_ignore_blanks",
            format!("=TOROW({}, 1)", mixed.a1_range()),
            mixed.workbook.clone(),
        );
        g.bench_function("mixed_10k_ignore_blanks", |b| {
            b.iter(|| black_box(eval_calc_core(black_box(&spec_ignore))))
        });

        if large_range_enabled() {
            let col_100k = numeric_column(RANGE_100K, |i| (i + 1) as f64);
            let spec_100k = col_100k.call_spec("torow.col_100k", "TOROW");
            g.throughput(Throughput::Elements(col_100k.cell_count));
            g.bench_function("column_100k", |b| {
                b.iter(|| black_box(eval_calc_core(black_box(&spec_100k))))
            });
        }
    });
}

criterion_group!(benches, bench_torow);
criterion_main!(benches);
