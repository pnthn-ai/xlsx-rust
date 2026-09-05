//! Large-range COUNTIF microbench (numeric `>` criterion).
//!
//! Compares the production walk path against the previous materialize-the-range
//! implementation so hill-climbs stay honest.

use criterion::{black_box, criterion_group, criterion_main, Criterion};
use xlsx_engine_core::{CalcCoreEngine, Evaluator};
use xlsx_types::{Candidate, Cell, CellAddr, EvalSpec, EvalTarget, ExcelValue, Sheet, Workbook};

const ROWS: u32 = 50_000;

fn large_countif_spec() -> EvalSpec {
    let mut sheet = Sheet::new("Sheet1");
    for i in 0..ROWS {
        sheet.insert(
            CellAddr::new(0, i),
            Cell::value(ExcelValue::Number((i % 100) as f64)),
        );
    }
    EvalSpec {
        case_id: "bench.countif".into(),
        workbook: Workbook {
            sheets: vec![sheet],
            names: vec![],
        },
        target: EvalTarget::Formula {
            formula: format!("=COUNTIF(A1:A{ROWS},\">50\")"),
            at: None,
        },
        options: Default::default(),
    }
}

fn countif_large_range(c: &mut Criterion) {
    let spec = large_countif_spec();
    let engine = CalcCoreEngine::new();
    let ev = Evaluator::new();

    // Sanity: both paths must agree before we time them.
    let walk = engine.evaluate(&spec).expect("walk COUNTIF");
    let mat = ev
        .countif_materialized(&spec)
        .expect("materialized COUNTIF");
    assert_eq!(walk, mat, "walk and materialize must stay equivalent");

    let mut group = c.benchmark_group("countif_gt_50k");
    group.bench_function("materialize_range", |b| {
        b.iter(|| {
            black_box(
                ev.countif_materialized(black_box(&spec))
                    .expect("materialize"),
            )
        })
    });
    group.bench_function("walk_range", |b| {
        b.iter(|| black_box(engine.evaluate(black_box(&spec)).expect("walk")))
    });
    group.finish();
}

criterion_group!(benches, countif_large_range);
criterion_main!(benches);
