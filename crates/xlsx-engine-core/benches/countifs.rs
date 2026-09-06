//! Criterion microbench for `COUNTIFS` over a 20k-row multi-criteria range.
//!
//! Compares the materializing baseline (`eval_range` + zip) with the
//! production range-walk used by `calc-core`. Expected counts are derived
//! from the generated workbook, not fixture goldens.

use criterion::{black_box, criterion_group, criterion_main, Criterion};
use xlsx_engine_core::{eval_countifs_materialized, eval_formula_in, CalcCoreEngine};
use xlsx_types::{Candidate, Cell, CellAddr, EvalSpec, EvalTarget, ExcelValue, Sheet, Workbook};

const ROWS: u32 = 20_000;

fn dense_workbook() -> Workbook {
    let mut sheet = Sheet::new("Sheet1");
    for i in 0..ROWS {
        sheet.insert(
            CellAddr::new(0, i),
            Cell::value(ExcelValue::Number((i % 10) as f64)),
        );
        sheet.insert(
            CellAddr::new(1, i),
            Cell::value(ExcelValue::Number((i % 3) as f64)),
        );
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
        sheet.insert(
            CellAddr::new(0, i),
            Cell::value(ExcelValue::Number((i % 10) as f64)),
        );
        sheet.insert(
            CellAddr::new(1, i),
            Cell::value(ExcelValue::Text(labels[(i as usize) % labels.len()].into())),
        );
    }
    Workbook {
        sheets: vec![sheet],
        names: vec![],
    }
}

fn three_crit_workbook() -> Workbook {
    let mut sheet = Sheet::new("Sheet1");
    for i in 0..ROWS {
        sheet.insert(
            CellAddr::new(0, i),
            Cell::value(ExcelValue::Number((i % 10) as f64)),
        );
        sheet.insert(
            CellAddr::new(1, i),
            Cell::value(ExcelValue::Number((i % 3) as f64)),
        );
        sheet.insert(
            CellAddr::new(2, i),
            Cell::value(ExcelValue::Number((i % 2) as f64)),
        );
    }
    Workbook {
        sheets: vec![sheet],
        names: vec![],
    }
}

fn spec(workbook: Workbook, formula: &str) -> EvalSpec {
    EvalSpec {
        case_id: "countifs-bench".into(),
        workbook,
        target: EvalTarget::formula(formula),
        options: Default::default(),
    }
}

fn expected_two_numeric() -> f64 {
    // i%10 in {6,7,8,9} AND i%3 == 1
    (0..ROWS)
        .filter(|i| i % 10 > 5 && i % 3 == 1)
        .count() as f64
}

fn expected_three_numeric() -> f64 {
    (0..ROWS)
        .filter(|i| i % 10 > 5 && i % 3 == 1 && i % 2 == 0)
        .count() as f64
}

fn expected_wildcard() -> f64 {
    let labels = ["apple", "pear", "x", "apricot", "banana"];
    (0..ROWS)
        .filter(|i| i % 10 > 5 && labels[(*i as usize) % labels.len()].contains('a'))
        .count() as f64
}

fn bench_countifs(c: &mut Criterion) {
    let numeric_wb = dense_workbook();
    let numeric_formula = "=COUNTIFS(A1:A20000,\">5\",B1:B20000,1)";
    let numeric_spec = spec(numeric_wb.clone(), numeric_formula);
    let engine = CalcCoreEngine::new();

    let want = ExcelValue::Number(expected_two_numeric());
    let got = engine.evaluate(&numeric_spec).unwrap();
    assert_eq!(got, want, "numeric two-criteria COUNTIFS derived count");
    let baseline = eval_countifs_materialized(&numeric_wb, numeric_formula).unwrap();
    assert_eq!(baseline, got, "materialize and walk must agree");

    let mut g = c.benchmark_group("countifs_20k");
    g.sample_size(20);

    g.bench_function("materialize_eval_range", |b| {
        b.iter(|| {
            black_box(eval_countifs_materialized(
                black_box(&numeric_wb),
                black_box(numeric_formula),
            ))
            .unwrap()
        })
    });
    g.bench_function("walk_fast", |b| {
        b.iter(|| black_box(engine.evaluate(black_box(&numeric_spec))).unwrap())
    });

    let three_wb = three_crit_workbook();
    let three_formula = "=COUNTIFS(A1:A20000,\">5\",B1:B20000,1,C1:C20000,0)";
    let three_spec = spec(three_wb, three_formula);
    let three_got = engine.evaluate(&three_spec).unwrap();
    assert_eq!(
        three_got,
        ExcelValue::Number(expected_three_numeric()),
        "three-criteria COUNTIFS derived count"
    );
    g.bench_function("walk_3crit", |b| {
        b.iter(|| black_box(engine.evaluate(black_box(&three_spec))).unwrap())
    });

    let text_wb = text_workbook();
    let text_formula = "=COUNTIFS(A1:A20000,\">5\",B1:B20000,\"*a*\")";
    let text_spec = spec(text_wb, text_formula);
    let text_got = engine.evaluate(&text_spec).unwrap();
    assert_eq!(
        text_got,
        ExcelValue::Number(expected_wildcard()),
        "wildcard COUNTIFS derived count"
    );
    g.bench_function("walk_wildcard", |b| {
        b.iter(|| black_box(engine.evaluate(black_box(&text_spec))).unwrap())
    });

    g.bench_function("count_floor", |b| {
        b.iter(|| {
            black_box(eval_formula_in(
                black_box(&numeric_wb),
                black_box("=COUNT(A1:A20000)"),
            ))
            .unwrap()
        })
    });

    g.finish();
}

criterion_group!(benches, bench_countifs);
criterion_main!(benches);
