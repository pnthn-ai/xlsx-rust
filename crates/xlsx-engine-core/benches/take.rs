//! Before/after microbench for Excel `TAKE`.
//!
//! Compares the materializing baseline (`excel_take_naive`: clone-all +
//! retain / transpose) with the production kernel (`excel_take`: clone only
//! the selected window).
//!
//! ```text
//! cargo bench -p xlsx-engine-core --bench take
//! ```

use std::hint::black_box;
use std::time::{Duration, Instant};
use xlsx_engine_core::{excel_take, excel_take_naive, CalcCoreEngine};
use xlsx_types::{Candidate, Cell, EvalSpec, EvalTarget, ExcelValue, Sheet, Workbook};

const ITERS: u32 = 40;
const ROWS: usize = 8_192;
const COLS: usize = 8;
const PAYLOAD: usize = 64;

struct Case {
    name: &'static str,
    array: ExcelValue,
    rows: Option<ExcelValue>,
    cols: Option<ExcelValue>,
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

fn cases() -> Vec<Case> {
    vec![
        Case {
            name: "8k×8 text, first 16 rows",
            array: matrix(ROWS, text_row),
            rows: Some(ExcelValue::Number(16.0)),
            cols: None,
        },
        Case {
            name: "8k×8 text, last 16 rows",
            array: matrix(ROWS, text_row),
            rows: Some(ExcelValue::Number(-16.0)),
            cols: None,
        },
        Case {
            name: "8k×8 text, first 2 cols",
            array: matrix(ROWS, text_row),
            rows: None,
            cols: Some(ExcelValue::Number(2.0)),
        },
        Case {
            name: "8k×8 text, last 2 cols",
            array: matrix(ROWS, text_row),
            rows: None,
            cols: Some(ExcelValue::Number(-2.0)),
        },
        Case {
            name: "8k×8 text, last 16 × last 2",
            array: matrix(ROWS, text_row),
            rows: Some(ExcelValue::Number(-16.0)),
            cols: Some(ExcelValue::Number(-2.0)),
        },
        Case {
            name: "8k×8 text, take all rows",
            array: matrix(ROWS, text_row),
            rows: Some(ExcelValue::Number(ROWS as f64)),
            cols: None,
        },
        Case {
            name: "8k×8 numeric, last 64 rows",
            array: matrix(ROWS, number_row),
            rows: Some(ExcelValue::Number(-64.0)),
            cols: None,
        },
        Case {
            name: "8k×8 text, rows=0 → #CALC!",
            array: matrix(ROWS, text_row),
            rows: Some(ExcelValue::Number(0.0)),
            cols: None,
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

fn evaluate_bench(n: u32, rows: i32, iters: u32) {
    let wb = workbook_n(n);
    let formula = format!("=TAKE(A1:A{n}, {rows})");
    let spec = EvalSpec {
        case_id: "bench.take".into(),
        workbook: wb,
        target: EvalTarget::formula(formula),
        options: Default::default(),
    };
    let engine = CalcCoreEngine::new();
    let got = engine.evaluate(&spec).expect("evaluate");
    let expected_len = (rows.unsigned_abs() as u32).min(n) as usize;
    match &got {
        ExcelValue::Array(out) => {
            assert_eq!(
                out.len(),
                expected_len,
                "TAKE(A1:A{n}, {rows}) got {} rows",
                out.len()
            );
        }
        other => panic!("expected array, got {other}"),
    }

    let ms = time_it(iters, || {
        black_box(engine.evaluate(black_box(&spec)).unwrap());
    });
    println!(
        "evaluate n={n:>6} rows={rows:<5}  {}/call  formula=TAKE(A1:A{n}, {rows})",
        fmt_dur(ms)
    );
}

fn main() {
    println!("TAKE kernel bench (clone-all/transpose vs window clone)");
    println!(
        "{:<42} {:>12} {:>12} {:>8}",
        "case", "naive", "optimized", "speedup"
    );
    println!("{}", "-".repeat(80));
    for c in cases() {
        let rows = c.rows.as_ref();
        let cols = c.cols.as_ref();
        let naive = time_it(ITERS, || {
            let _ = black_box(excel_take_naive(
                black_box(&c.array),
                black_box(rows),
                black_box(cols),
            ));
        });
        let fast = time_it(ITERS, || {
            let _ = black_box(excel_take(
                black_box(&c.array),
                black_box(rows),
                black_box(cols),
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
        let a = excel_take_naive(&c.array, rows, cols);
        let b = excel_take(&c.array, rows, cols);
        assert_eq!(a, b, "semantic mismatch on {}", c.name);
    }
    println!();
    evaluate_bench(10_000, 16, 8);
    evaluate_bench(10_000, -16, 8);
    evaluate_bench(100_000, -64, 3);
}
