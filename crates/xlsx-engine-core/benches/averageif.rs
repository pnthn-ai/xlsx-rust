//! Criterion microbench for `AVERAGEIF` over a 20k-row range.
//!
//! Compares the materializing baseline (`eval_range` + zip) with the
//! production range-walk used by `calc-core`.

use criterion::{black_box, criterion_group, criterion_main, Criterion};
use xlsx_engine_core::{eval_averageif_materialized, eval_formula_in, CalcCoreEngine};
use xlsx_types::{Candidate, Cell, CellAddr, EvalSpec, EvalTarget, ExcelValue, Sheet, Workbook};

const ROWS: u32 = 20_000;

fn dense_workbook() -> Workbook {
    let mut sheet = Sheet::new("Sheet1");
    for i in 0..ROWS {
        let a = CellAddr::new(0, i);
        let b = CellAddr::new(1, i);
        sheet.insert(a, Cell::value(ExcelValue::Number((i % 10) as f64)));
        sheet.insert(b, Cell::value(ExcelValue::Number((i % 10) as f64 + 1.0)));
    }
    Workbook {
        sheets: vec![sheet],
        names: vec![],
    }
}

fn text_workbook() -> Workbook {
    let labels = ["apple", "pear", "x", "apricot", "banana"];
    let mut sheet = Sheet::new("Sheet1");
    for i in 0..ROWS {
        let a = CellAddr::new(0, i);
        let b = CellAddr::new(1, i);
        sheet.insert(
            a,
            Cell::value(ExcelValue::Text(labels[(i as usize) % labels.len()].into())),
        );
        sheet.insert(b, Cell::value(ExcelValue::Number(1.0)));
    }
    Workbook {
        sheets: vec![sheet],
        names: vec![],
    }
}

fn spec(workbook: Workbook, formula: &str) -> EvalSpec {
    EvalSpec {
        case_id: "averageif-bench".into(),
        workbook,
        target: EvalTarget::formula(formula),
        options: Default::default(),
    }
}

fn bench_averageif(c: &mut Criterion) {
    let numeric_wb = dense_workbook();
    let numeric_formula = "=AVERAGEIF(A1:A20000,\">5\",B1:B20000)";
    let numeric_spec = spec(numeric_wb.clone(), numeric_formula);
    let engine = CalcCoreEngine::new();

    let got = engine.evaluate(&numeric_spec).unwrap();
    // i%10 in {6,7,8,9} → 4/10 of 20000; B = i%10+1 → {7,8,9,10}; avg = 8.5
    assert_eq!(got, ExcelValue::Number(8.5), "numeric AVERAGEIF golden");
    let baseline = eval_averageif_materialized(&numeric_wb, numeric_formula).unwrap();
    assert_eq!(baseline, got, "materialize and walk must agree");

    let mut g = c.benchmark_group("averageif_20k");
    g.sample_size(20);

    g.bench_function("materialize_eval_range", |b| {
        b.iter(|| {
            black_box(eval_averageif_materialized(
                black_box(&numeric_wb),
                black_box(numeric_formula),
            ))
            .unwrap()
        })
    });
    g.bench_function("walk_fast", |b| {
        b.iter(|| black_box(engine.evaluate(black_box(&numeric_spec))).unwrap())
    });

    let text_wb = text_workbook();
    let text_formula = "=AVERAGEIF(A1:A20000,\"*a*\",B1:B20000)";
    let text_spec = spec(text_wb, text_formula);
    let text_got = engine.evaluate(&text_spec).unwrap();
    // apple, pear, apricot, banana contain 'a'; x does not → 4/5 of 20000, each 1
    assert_eq!(
        text_got,
        ExcelValue::Number(1.0),
        "wildcard AVERAGEIF golden"
    );

    g.bench_function("walk_wildcard", |b| {
        b.iter(|| black_box(engine.evaluate(black_box(&text_spec))).unwrap())
    });

    g.bench_function("average_floor", |b| {
        b.iter(|| {
            black_box(eval_formula_in(
                black_box(&numeric_wb),
                black_box("=AVERAGE(B1:B20000)"),
            ))
            .unwrap()
        })
    });

    g.finish();
}

criterion_group!(benches, bench_averageif);
criterion_main!(benches);
