//! Before/after microbench for Excel `CHOOSEROWS`.
//!
//! Compares the materializing baseline (`excel_chooserows_naive`: clone-all
//! then pick) with the production kernel (`excel_chooserows`: resolve indices,
//! clone only selected rows). Also times a full `calc-core` evaluate of
//! `CHOOSEROWS(A1:An, …)`.
//!
//! ```text
//! cargo bench -p xlsx-engine-core --bench chooserows
//! ```

use std::hint::black_box;
use std::time::{Duration, Instant};
use xlsx_engine_core::{excel_chooserows, excel_chooserows_naive, CalcCoreEngine};
use xlsx_types::{Candidate, Cell, EvalSpec, EvalTarget, ExcelValue, Sheet, Workbook};

const ITERS: u32 = 40;
const ROWS: usize = 8_192;
const COLS: usize = 8;
const PAYLOAD: usize = 64;

struct Case {
    name: &'static str,
    array: ExcelValue,
    row_nums: Vec<ExcelValue>,
}

fn text_row(tag: usize) -> Vec<ExcelValue> {
    (0..COLS)
        .map(|c| ExcelValue::Text(format!("{tag:04}-{c}-{}", "x".repeat(PAYLOAD))))
        .collect()
}

fn number_row(tag: usize) -> Vec<ExcelValue> {
    (0..COLS)
        .map(|c| ExcelValue::Number((tag * COLS + c) as f64))
        .collect()
}

fn matrix(rows: usize, cell: impl Fn(usize) -> Vec<ExcelValue>) -> ExcelValue {
    ExcelValue::Array((0..rows).map(cell).collect())
}

fn n(x: f64) -> ExcelValue {
    ExcelValue::Number(x)
}

fn cases() -> Vec<Case> {
    let every_8: Vec<ExcelValue> = (0..ROWS)
        .filter(|i| i % 8 == 0)
        .map(|i| n((i + 1) as f64))
        .collect();
    vec![
        Case {
            name: "8k×8 text, last 2 via -1,-2",
            array: matrix(ROWS, text_row),
            row_nums: vec![n(-1.0), n(-2.0)],
        },
        Case {
            name: "8k×8 text, first row only",
            array: matrix(ROWS, text_row),
            row_nums: vec![n(1.0)],
        },
        Case {
            name: "8k×8 text, pick 1/8 rows",
            array: matrix(ROWS, text_row),
            row_nums: every_8,
        },
        Case {
            name: "8k×8 text, reverse via negatives",
            array: matrix(ROWS, text_row),
            row_nums: (0..ROWS).map(|i| n(-((i + 1) as f64))).collect(),
        },
        Case {
            name: "8k×8 numeric, last 8 via negatives",
            array: matrix(ROWS, number_row),
            row_nums: (1..=8).map(|i| n(-(i as f64))).collect(),
        },
        Case {
            name: "8k×8 numeric, 0 → #VALUE!",
            array: matrix(ROWS, number_row),
            row_nums: vec![n(0.0)],
        },
    ]
}

fn time_it(iters: u32, mut f: impl FnMut()) -> Duration {
    f();
    let start = Instant::now();
    for _ in 0..iters {
        f();
    }
    start.elapsed() / iters
}

fn fmt_dur(d: Duration) -> String {
    let us = d.as_secs_f64() * 1e6;
    if us >= 1000.0 {
        format!("{:.2} ms", us / 1000.0)
    } else {
        format!("{us:.1} µs")
    }
}

fn kernel_bench() {
    println!("CHOOSEROWS kernel bench (clone-all then pick vs clone-only-picks)");
    println!(
        "{:<42} {:>12} {:>12} {:>8}",
        "case", "naive", "optimized", "speedup"
    );
    println!("{}", "-".repeat(80));
    for c in cases() {
        let naive = time_it(ITERS, || {
            let _ = black_box(excel_chooserows_naive(
                black_box(&c.array),
                black_box(&c.row_nums),
            ));
        });
        let fast = time_it(ITERS, || {
            let _ = black_box(excel_chooserows(
                black_box(&c.array),
                black_box(&c.row_nums),
            ));
        });
        let speedup = naive.as_secs_f64() / fast.as_secs_f64().max(1e-12);
        println!(
            "{:<42} {:>12} {:>12} {:>7.1}x",
            c.name,
            fmt_dur(naive),
            fmt_dur(fast),
            speedup
        );
        let a = excel_chooserows_naive(&c.array, &c.row_nums);
        let b = excel_chooserows(&c.array, &c.row_nums);
        assert_eq!(a, b, "semantic mismatch on {}", c.name);
    }
}

fn workbook_n(n: u32) -> Workbook {
    let mut sheet = Sheet::new("Sheet1");
    for i in 0..n {
        let a1 = format!("A{}", i + 1);
        sheet
            .cells
            .insert(a1, Cell::value(ExcelValue::Number((i + 1) as f64)));
    }
    Workbook {
        sheets: vec![sheet],
        names: vec![],
    }
}

fn evaluate_bench(n: u32, formula: &str, iters: u32) {
    let spec = EvalSpec {
        case_id: "bench.chooserows".into(),
        workbook: workbook_n(n),
        target: EvalTarget::formula(formula.to_string()),
        options: Default::default(),
    };
    let engine = CalcCoreEngine::new();
    let got = engine.evaluate(&spec).expect("evaluate");
    match &got {
        ExcelValue::Array(rows) => {
            assert!(!rows.is_empty(), "expected at least one picked row");
        }
        ExcelValue::Error(_) => {}
        other => panic!("expected array or error, got {other}"),
    }

    let ms = time_it(iters, || {
        black_box(engine.evaluate(black_box(&spec)).unwrap());
    });
    println!(
        "evaluate n={n:>6}  {}  formula={formula}",
        fmt_dur(ms)
    );
}

fn main() {
    println!("CHOOSEROWS bench (calc-core pick kernel vs clone-all)\n");
    kernel_bench();
    println!();
    evaluate_bench(10_000, "=CHOOSEROWS(A1:A10000,-1,-2)", 20);
    evaluate_bench(10_000, "=CHOOSEROWS(A1:A10000,1,5000,10000)", 20);
    evaluate_bench(100_000, "=CHOOSEROWS(A1:A100000,-1)", 8);
}
