//! Criterion microbench for `AVERAGEIFS` over a 20k-row multi-criteria range.
//!
//! Compares the materializing baseline (`eval_range` + zip) with the
//! production range-walk used by `calc-core`.

use criterion::{black_box, criterion_group, criterion_main, Criterion};
use xlsx_engine_core::{eval_averageifs_materialized, eval_formula_in, CalcCoreEngine};
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
        sheet.insert(
            CellAddr::new(2, i),
            Cell::value(ExcelValue::Number((i % 10) as f64 + 1.0)),
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
        sheet.insert(
            CellAddr::new(3, i),
            Cell::value(ExcelValue::Number((i % 10) as f64 + 1.0)),
        );
    }
    Workbook {
        sheets: vec![sheet],
        names: vec![],
    }
}

fn spec(workbook: Workbook, formula: &str) -> EvalSpec {
    EvalSpec {
        case_id: "averageifs-bench".into(),
        workbook,
        target: EvalTarget::formula(formula),
        options: Default::default(),
    }
}

fn expected_two_numeric() -> f64 {
    // C = i%10+1 when i%10 in {6,7,8,9} AND i%3 == 1.
    // 20000 is not a multiple of 30, so the four residues are not equally often.
    let (sum, count) = (0..ROWS).fold((0.0, 0u64), |(sum, count), i| {
        if i % 10 > 5 && i % 3 == 1 {
            (sum + (i % 10) as f64 + 1.0, count + 1)
        } else {
            (sum, count)
        }
    });
    sum / count as f64
}

fn expected_three_numeric() -> f64 {
    let (sum, count) = (0..ROWS).fold((0.0, 0u64), |(sum, count), i| {
        if i % 10 > 5 && i % 3 == 1 && i % 2 == 0 {
            (sum + (i % 10) as f64 + 1.0, count + 1)
        } else {
            (sum, count)
        }
    });
    sum / count as f64
}

fn bench_averageifs(c: &mut Criterion) {
    let numeric_wb = dense_workbook();
    let numeric_formula = "=AVERAGEIFS(C1:C20000,A1:A20000,\">5\",B1:B20000,1)";
    let numeric_spec = spec(numeric_wb.clone(), numeric_formula);
    let engine = CalcCoreEngine::new();

    let want = ExcelValue::Number(expected_two_numeric());
    let got = engine.evaluate(&numeric_spec).unwrap();
    assert_eq!(got, want, "numeric two-criteria AVERAGEIFS golden");
    let baseline = eval_averageifs_materialized(&numeric_wb, numeric_formula).unwrap();
    assert_eq!(baseline, got, "materialize and walk must agree");

    let mut g = c.benchmark_group("averageifs_20k");
    g.sample_size(20);

    g.bench_function("materialize_eval_range", |b| {
        b.iter(|| {
            black_box(eval_averageifs_materialized(
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
    let three_formula = "=AVERAGEIFS(D1:D20000,A1:A20000,\">5\",B1:B20000,1,C1:C20000,0)";
    let three_spec = spec(three_wb, three_formula);
    let three_got = engine.evaluate(&three_spec).unwrap();
    assert_eq!(
        three_got,
        ExcelValue::Number(expected_three_numeric()),
        "three-criteria AVERAGEIFS golden"
    );
    g.bench_function("walk_3crit", |b| {
        b.iter(|| black_box(engine.evaluate(black_box(&three_spec))).unwrap())
    });

    let text_wb = text_workbook();
    let text_formula = "=AVERAGEIFS(C1:C20000,A1:A20000,\">5\",B1:B20000,\"*a*\")";
    let text_spec = spec(text_wb, text_formula);
    let text_got = engine.evaluate(&text_spec).unwrap();
    assert_eq!(
        text_got,
        ExcelValue::Number(1.0),
        "wildcard AVERAGEIFS golden"
    );
    g.bench_function("walk_wildcard", |b| {
        b.iter(|| black_box(engine.evaluate(black_box(&text_spec))).unwrap())
    });

    g.bench_function("average_floor", |b| {
        b.iter(|| {
            black_box(eval_formula_in(
                black_box(&numeric_wb),
                black_box("=AVERAGE(C1:C20000)"),
            ))
            .unwrap()
        })
    });

    g.finish();
}

criterion_group!(benches, bench_averageifs);
criterion_main!(benches);
