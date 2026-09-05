//! Before/after microbench for Excel `CHOOSECOLS`.
//!
//! Compares the materializing baseline (`excel_choosecols_naive`: clone +
//! transpose + retain + transpose) with the production kernel
//! (`excel_choosecols`: resolve indices, clone only selected cells), plus a
//! full `calc-core` evaluate of `CHOOSECOLS(A1:Z{n}, 1, -1)` (range-skip).
//!
//! ```text
//! cargo bench -p xlsx-engine-core --bench choosecols
//! ```

use std::hint::black_box;
use std::time::{Duration, Instant};
use xlsx_engine_core::{excel_choosecols, excel_choosecols_naive, CalcCoreEngine};
use xlsx_types::{Candidate, Cell, EvalSpec, EvalTarget, ExcelValue, Sheet, Workbook};

const ITERS: u32 = 30;
const ROWS: usize = 8_192;
const COLS: usize = 32;
const PAYLOAD: usize = 48;

struct Case {
    name: &'static str,
    array: ExcelValue,
    col_nums: Vec<ExcelValue>,
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
            name: "8k×32 text, pick 2 cols",
            array: matrix(ROWS, text_row),
            col_nums: vec![ExcelValue::Number(1.0), ExcelValue::Number(-1.0)],
        },
        Case {
            name: "8k×32 text, pick 1 col",
            array: matrix(ROWS, text_row),
            col_nums: vec![ExcelValue::Number(16.0)],
        },
        Case {
            name: "8k×32 text, reorder 4 cols",
            array: matrix(ROWS, text_row),
            col_nums: vec![
                ExcelValue::Number(-1.0),
                ExcelValue::Number(1.0),
                ExcelValue::Number(8.0),
                ExcelValue::Number(8.0),
            ],
        },
        Case {
            name: "8k×32 numeric, pick 2 cols",
            array: matrix(ROWS, number_row),
            col_nums: vec![ExcelValue::Number(1.0), ExcelValue::Number(32.0)],
        },
        Case {
            name: "8k×32 numeric, array col_nums",
            array: matrix(ROWS, number_row),
            col_nums: vec![ExcelValue::Array(vec![vec![
                ExcelValue::Number(1.0),
                ExcelValue::Number(3.0),
                ExcelValue::Number(5.0),
            ]])],
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

fn workbook_grid(rows: u32, cols: u32) -> Workbook {
    let mut sheet = Sheet::new("Sheet1");
    for r in 0..rows {
        for c in 0..cols {
            let addr = xlsx_types::CellAddr::new(c, r);
            sheet.cells.insert(
                addr.a1(),
                Cell::value(ExcelValue::Number((r * cols + c + 1) as f64)),
            );
        }
    }
    Workbook {
        sheets: vec![sheet],
        names: vec![],
    }
}

fn evaluate_bench(rows: u32, cols: u32, iters: u32) {
    let wb = workbook_grid(rows, cols);
    let end = xlsx_types::CellAddr::new(cols - 1, rows - 1).a1();
    let formula = format!("=CHOOSECOLS(A1:{end}, 1, -1)");
    let spec = EvalSpec {
        case_id: "bench.choosecols".into(),
        workbook: wb,
        target: EvalTarget::formula(formula.clone()),
        options: Default::default(),
    };
    let engine = CalcCoreEngine::new();
    let got = engine.evaluate(&spec).expect("evaluate");
    match &got {
        ExcelValue::Array(out) => {
            assert_eq!(out.len(), rows as usize, "row count");
            assert_eq!(out[0].len(), 2, "picked two columns");
            assert_eq!(out[0][0], ExcelValue::Number(1.0));
            assert_eq!(out[0][1], ExcelValue::Number(cols as f64));
        }
        other => panic!("expected array, got {other}"),
    }

    let ms = time_it(iters, || {
        black_box(engine.evaluate(black_box(&spec)).unwrap());
    });
    println!(
        "evaluate {rows}×{cols}  {}/call  formula={formula}",
        fmt_dur(ms)
    );
}

fn main() {
    println!("CHOOSECOLS kernel bench (transpose-all vs clone-selected)\n");
    println!(
        "{:<42} {:>12} {:>12} {:>8}",
        "case", "naive", "optimized", "speedup"
    );
    println!("{}", "-".repeat(80));
    for c in cases() {
        let naive = time_it(ITERS, || {
            let _ = black_box(excel_choosecols_naive(
                black_box(&c.array),
                black_box(&c.col_nums),
            ));
        });
        let fast = time_it(ITERS, || {
            let _ = black_box(excel_choosecols(
                black_box(&c.array),
                black_box(&c.col_nums),
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
        let a = excel_choosecols_naive(&c.array, &c.col_nums);
        let b = excel_choosecols(&c.array, &c.col_nums);
        assert_eq!(a, b, "semantic mismatch on {}", c.name);
    }
    println!();
    evaluate_bench(10_000, 26, 8);
    evaluate_bench(50_000, 26, 3);
}
