//! Criterion microbench for `SUMIFS` over a 20k-row multi-criteria range.
//!
//! Compares the materializing baseline (`eval_range` + zip) with the
//! production range-walk used by `calc-core`.

use criterion::{black_box, criterion_group, criterion_main, Criterion};
use xlsx_engine_core::{eval_formula_in, eval_sumifs_materialized, CalcCoreEngine};
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
        sheet.insert(CellAddr::new(2, i), Cell::value(ExcelValue::Number(1.0)));
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
        sheet.insert(CellAddr::new(2, i), Cell::value(ExcelValue::Number(1.0)));
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
        sheet.insert(CellAddr::new(3, i), Cell::value(ExcelValue::Number(1.0)));
    }
    Workbook {
        sheets: vec![sheet],
        names: vec![],
    }
}

fn spec(workbook: Workbook, formula: &str) -> EvalSpec {
    EvalSpec {
        case_id: "sumifs-bench".into(),
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

fn bench_sumifs(c: &mut Criterion) {
    let numeric_wb = dense_workbook();
    let numeric_formula = "=SUMIFS(C1:C20000,A1:A20000,\">5\",B1:B20000,1)";
    let numeric_spec = spec(numeric_wb.clone(), numeric_formula);
    let engine = CalcCoreEngine::new();

    let want = ExcelValue::Number(expected_two_numeric());
    let got = engine.evaluate(&numeric_spec).unwrap();
    assert_eq!(got, want, "numeric two-criteria SUMIFS golden");
    let baseline = eval_sumifs_materialized(&numeric_wb, numeric_formula).unwrap();
    assert_eq!(baseline, got, "materialize and walk must agree");

    let mut g = c.benchmark_group("sumifs_20k");
    g.sample_size(20);

    g.bench_function("materialize_eval_range", |b| {
        b.iter(|| {
            black_box(eval_sumifs_materialized(
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
    let three_formula = "=SUMIFS(D1:D20000,A1:A20000,\">5\",B1:B20000,1,C1:C20000,0)";
    let three_spec = spec(three_wb, three_formula);
    let three_got = engine.evaluate(&three_spec).unwrap();
    assert_eq!(
        three_got,
        ExcelValue::Number(expected_three_numeric()),
        "three-criteria SUMIFS golden"
    );
    g.bench_function("walk_3crit", |b| {
        b.iter(|| black_box(engine.evaluate(black_box(&three_spec))).unwrap())
    });

    let text_wb = text_workbook();
    let text_formula = "=SUMIFS(C1:C20000,A1:A20000,\">5\",B1:B20000,\"*a*\")";
    let text_spec = spec(text_wb, text_formula);
    let text_got = engine.evaluate(&text_spec).unwrap();
    assert_eq!(
        text_got,
        ExcelValue::Number(expected_wildcard()),
        "wildcard SUMIFS golden"
    );
    g.bench_function("walk_wildcard", |b| {
        b.iter(|| black_box(engine.evaluate(black_box(&text_spec))).unwrap())
    });

    g.bench_function("sum_floor", |b| {
        b.iter(|| {
            black_box(eval_formula_in(
                black_box(&numeric_wb),
                black_box("=SUM(C1:C20000)"),
            ))
            .unwrap()
        })
    });

    g.finish();
}

criterion_group!(benches, bench_sumifs);
criterion_main!(benches);
