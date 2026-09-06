//! Per-function Criterion bench: `SUBSTITUTE` over large haystacks.
//!
//! ```text
//! cargo bench -p xlsx-bench --bench fn_substitute
//! XLSX_BENCH_LARGE=1 cargo bench -p xlsx-bench --bench fn_substitute
//! ```

use xlsx_bench::prelude::*;
use xlsx_bench::{formula_spec, large_range_enabled, RANGE_100K, RANGE_10K};
use xlsx_types::{Cell, CellAddr, ExcelValue};

fn long_text_spec(case_id: &str, formula: &str, text: String) -> EvalSpec {
    let mut b = xlsx_bench::SnippetBuilder::new("Sheet1");
    b.put(CellAddr::new(0, 0), Cell::value(ExcelValue::Text(text)));
    let filled = b.finish();
    formula_spec(case_id, formula, filled.workbook)
}

fn bench_substitute(c: &mut Criterion) {
    bench_fn(c, "SUBSTITUTE", |g| {
        let dense = "a".repeat(RANGE_10K as usize);
        let spec_swap = long_text_spec(
            "substitute.swap_10k",
            "=SUBSTITUTE(A1, \"a\", \"b\")",
            dense.clone(),
        );
        g.throughput(Throughput::Elements(RANGE_10K as u64));
        g.bench_function("ascii_byte_swap_10k", |b| {
            b.iter(|| black_box(eval_calc_core(black_box(&spec_swap))))
        });

        let hyphens = "foo-".repeat(RANGE_10K as usize / 4);
        let spec_eq = long_text_spec(
            "substitute.eq_10k",
            "=SUBSTITUTE(A1, \"foo\", \"bar\")",
            hyphens.clone(),
        );
        g.bench_function("equal_width_foo_bar", |b| {
            b.iter(|| black_box(eval_calc_core(black_box(&spec_eq))))
        });

        let spec_nth = long_text_spec(
            "substitute.nth_10k",
            "=SUBSTITUTE(A1, \"foo\", \"bar\", 1000)",
            hyphens,
        );
        g.bench_function("nth_instance_1000", |b| {
            b.iter(|| black_box(eval_calc_core(black_box(&spec_nth))))
        });

        let spec_miss = long_text_spec(
            "substitute.miss_10k",
            "=SUBSTITUTE(A1, \"z\", \"x\")",
            "x".repeat(RANGE_10K as usize),
        );
        g.bench_function("ascii_miss_10k", |b| {
            b.iter(|| black_box(eval_calc_core(black_box(&spec_miss))))
        });

        let spec_del = long_text_spec(
            "substitute.del_10k",
            "=SUBSTITUTE(A1, \"-\", \"\")",
            "a-".repeat(RANGE_10K as usize / 2),
        );
        g.bench_function("ascii_delete_hyphen", |b| {
            b.iter(|| black_box(eval_calc_core(black_box(&spec_del))))
        });

        if large_range_enabled() {
            let spec_100k = long_text_spec(
                "substitute.swap_100k",
                "=SUBSTITUTE(A1, \"a\", \"b\")",
                "a".repeat(RANGE_100K as usize),
            );
            g.throughput(Throughput::Elements(RANGE_100K as u64));
            g.bench_function("ascii_byte_swap_100k", |b| {
                b.iter(|| black_box(eval_calc_core(black_box(&spec_100k))))
            });
        }
    });
}

criterion_group!(benches, bench_substitute);
criterion_main!(benches);
