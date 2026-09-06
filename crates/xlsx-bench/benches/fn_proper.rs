//! Per-function Criterion bench: `PROPER` on long scalar text.
//!
//! ```text
//! cargo bench -p xlsx-bench --bench fn_proper
//! XLSX_BENCH_LARGE=1 cargo bench -p xlsx-bench --bench fn_proper
//! ```

use xlsx_bench::prelude::*;
use xlsx_bench::{formula_spec, large_range_enabled, SnippetBuilder};
use xlsx_types::ExcelValue;

fn spec_for(case_id: &str, text: String) -> EvalSpec {
    let mut b = SnippetBuilder::new("Sheet1");
    b.set(0, 0, ExcelValue::Text(text));
    let filled = b.finish();
    formula_spec(case_id, "=PROPER(A1)", filled.workbook)
}

fn bench_proper(c: &mut Criterion) {
    bench_fn(c, "PROPER", |g| {
        let mixed_200k = "aB-cD'eF 76x ".repeat(14_286);
        let spec_mixed = spec_for("proper.mixed_200k", mixed_200k);
        g.throughput(Throughput::Bytes(200_004));
        g.bench_function("cell_mixed_200k", |b| {
            b.iter(|| black_box(eval_calc_core(black_box(&spec_mixed))))
        });

        let caps_200k = "HELLO WORLD ".repeat(16_667);
        let spec_caps = spec_for("proper.caps_200k", caps_200k);
        g.bench_function("cell_allcaps_200k", |b| {
            b.iter(|| black_box(eval_calc_core(black_box(&spec_caps))))
        });

        let already = "This Is A Title ".repeat(12_500);
        let spec_already = spec_for("proper.already_200k", already);
        g.bench_function("cell_already_proper_200k", |b| {
            b.iter(|| black_box(eval_calc_core(black_box(&spec_already))))
        });

        let spec_ms = spec_for("proper.ms_title", "this is a TITLE".into());
        g.bench_function("cell_microsoft_title", |b| {
            b.iter(|| black_box(eval_calc_core(black_box(&spec_ms))))
        });

        if large_range_enabled() {
            let huge = "aB-cD'eF 76x ".repeat(71_429);
            let spec_1m = spec_for("proper.mixed_1m", huge);
            g.throughput(Throughput::Bytes(1_000_006));
            g.bench_function("cell_mixed_1m", |b| {
                b.iter(|| black_box(eval_calc_core(black_box(&spec_1m))))
            });
        }
    });
}

criterion_group!(benches, bench_proper);
criterion_main!(benches);
