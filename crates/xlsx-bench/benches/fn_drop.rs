//! Per-function Criterion bench: `DROP` over large ranges.
//!
//! ```text
//! cargo bench -p xlsx-bench --bench fn_drop
//! XLSX_BENCH_LARGE=1 cargo bench -p xlsx-bench --bench fn_drop
//! ```

use xlsx_bench::prelude::*;
use xlsx_bench::{formula_spec, large_range_enabled, numeric_grid, RANGE_100K, RANGE_10K};

fn bench_drop(c: &mut Criterion) {
    bench_fn(c, "DROP", |g| {
        let col_10k = numeric_column(RANGE_10K, |i| (i + 1) as f64);
        let spec_header = col_10k.spec(
            "drop.header_10k",
            format!("=DROP({}, 1)", col_10k.a1_range()),
        );
        g.throughput(Throughput::Elements(col_10k.cell_count));
        g.bench_function("col_drop_header_10k", |b| {
            b.iter(|| black_box(eval_calc_core(black_box(&spec_header))))
        });

        let spec_footer = col_10k.spec(
            "drop.footer_10k",
            format!("=DROP({}, -1)", col_10k.a1_range()),
        );
        g.bench_function("col_drop_footer_10k", |b| {
            b.iter(|| black_box(eval_calc_core(black_box(&spec_footer))))
        });

        let spec_half = col_10k.spec(
            "drop.half_10k",
            format!("=DROP({}, 5000)", col_10k.a1_range()),
        );
        g.bench_function("col_drop_half_10k", |b| {
            b.iter(|| black_box(eval_calc_core(black_box(&spec_half))))
        });

        let grid = numeric_grid(1_024, 8, |r, c| (r * 8 + c + 1) as f64);
        let spec_2d = formula_spec(
            "drop.grid_1k_x8",
            format!("=DROP({}, 8, 1)", grid.a1_range()),
            grid.workbook.clone(),
        );
        g.throughput(Throughput::Elements(grid.cell_count));
        g.bench_function("grid_drop_rows_cols_1k_x8", |b| {
            b.iter(|| black_box(eval_calc_core(black_box(&spec_2d))))
        });

        if large_range_enabled() {
            let col_100k = numeric_column(RANGE_100K, |i| (i + 1) as f64);
            let spec_100k = col_100k.spec(
                "drop.header_100k",
                format!("=DROP({}, 1)", col_100k.a1_range()),
            );
            g.throughput(Throughput::Elements(col_100k.cell_count));
            g.bench_function("col_drop_header_100k", |b| {
                b.iter(|| black_box(eval_calc_core(black_box(&spec_100k))))
            });
        }
    });
}

criterion_group!(benches, bench_drop);
criterion_main!(benches);
