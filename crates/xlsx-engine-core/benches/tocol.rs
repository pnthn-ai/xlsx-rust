//! Before/after microbench for Excel `TOCOL`.
//!
//! Compares the materializing baseline (`tocol_apply_naive`: clone-all +
//! transpose + retain) with the production kernel (`tocol_apply`: scan-order
//! walk, clone only kept cells).
//!
//! ```text
//! cargo bench -p xlsx-engine-core --bench tocol
//! ```

use std::hint::black_box;
use std::time::{Duration, Instant};
use xlsx_engine_core::{tocol_apply, tocol_apply_naive, CalcCoreEngine};
use xlsx_types::{Candidate, Cell, EvalSpec, EvalTarget, ExcelError, ExcelValue, Sheet, Workbook};

const ITERS: u32 = 30;
const ROWS: usize = 4_096;
const COLS: usize = 16;

struct Case {
    name: &'static str,
    array: ExcelValue,
    ignore: u8,
    scan_by_col: bool,
}

fn number_row(tag: usize) -> Vec<ExcelValue> {
    (0..COLS)
        .map(|c| ExcelValue::Number((tag * COLS + c) as f64))
        .collect()
}

fn mixed_row(tag: usize) -> Vec<ExcelValue> {
    (0..COLS)
        .map(|c| {
            let i = tag * COLS + c;
            match i % 8 {
                0 => ExcelValue::Empty,
                1 => ExcelValue::Error(ExcelError::Na),
                2 => ExcelValue::Text(format!("t{i}")),
                _ => ExcelValue::Number(i as f64),
            }
        })
        .collect()
}

fn matrix(rows: usize, cell: impl Fn(usize) -> Vec<ExcelValue>) -> ExcelValue {
    ExcelValue::Array((0..rows).map(cell).collect())
}

fn cases() -> Vec<Case> {
    vec![
        Case {
            name: "4k×16 numeric, keep all, by row",
            array: matrix(ROWS, number_row),
            ignore: 0,
            scan_by_col: false,
        },
        Case {
            name: "4k×16 numeric, keep all, by col",
            array: matrix(ROWS, number_row),
            ignore: 0,
            scan_by_col: true,
        },
        Case {
            name: "4k×16 mixed, ignore blanks",
            array: matrix(ROWS, mixed_row),
            ignore: 1,
            scan_by_col: false,
        },
        Case {
            name: "4k×16 mixed, ignore blanks+errors, by col",
            array: matrix(ROWS, mixed_row),
            ignore: 3,
            scan_by_col: true,
        },
        Case {
            name: "4k×16 mixed, keep none → #CALC!",
            array: ExcelValue::Array((0..ROWS).map(|_| vec![ExcelValue::Empty; COLS]).collect()),
            ignore: 1,
            scan_by_col: false,
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

fn evaluate_bench(rows: u32, cols: u32, ignore: u8, scan_by_col: bool, iters: u32) {
    let wb = workbook_grid(rows, cols);
    let end = xlsx_types::CellAddr::new(cols - 1, rows - 1).a1();
    let formula = if scan_by_col {
        format!("=TOCOL(A1:{end},{ignore},TRUE)")
    } else {
        format!("=TOCOL(A1:{end},{ignore})")
    };
    let spec = EvalSpec {
        case_id: "bench.tocol".into(),
        workbook: wb,
        target: EvalTarget::formula(formula.clone()),
        options: Default::default(),
    };
    let engine = CalcCoreEngine::new();
    let got = engine.evaluate(&spec).expect("evaluate");
    let expected = (rows as usize) * (cols as usize);
    match &got {
        ExcelValue::Array(out) => {
            assert_eq!(out.len(), expected, "{formula} rows");
            assert_eq!(out[0].len(), 1, "{formula} must be a column");
        }
        other => panic!("expected array, got {other}"),
    }
    let ms = time_it(iters, || {
        black_box(engine.evaluate(black_box(&spec)).unwrap());
    });
    println!(
        "evaluate {rows}×{cols} ignore={ignore} scan_by_col={scan_by_col}  {}/call  {formula}",
        fmt_dur(ms)
    );
}

fn main() {
    println!("TOCOL kernel bench (clone-all/transpose vs scan + clone-kept)");
    println!(
        "{:<48} {:>12} {:>12} {:>8}",
        "case", "naive", "optimized", "speedup"
    );
    println!("{}", "-".repeat(86));
    for c in cases() {
        let naive = time_it(ITERS, || {
            let _ = black_box(tocol_apply_naive(
                black_box(&c.array),
                black_box(c.ignore),
                black_box(c.scan_by_col),
            ));
        });
        let fast = time_it(ITERS, || {
            let _ = black_box(tocol_apply(
                black_box(&c.array),
                black_box(c.ignore),
                black_box(c.scan_by_col),
            ));
        });
        let speedup = naive.as_secs_f64() / fast.as_secs_f64().max(1e-12);
        println!(
            "{:<48} {:>12} {:>12} {:>7.1}x",
            c.name,
            fmt_dur(naive),
            fmt_dur(fast),
            speedup
        );
        let a = tocol_apply_naive(&c.array, c.ignore, c.scan_by_col);
        let b = tocol_apply(&c.array, c.ignore, c.scan_by_col);
        assert_eq!(a, b, "semantic mismatch on {}", c.name);
    }
    println!();
    evaluate_bench(256, 16, 0, false, 8);
    evaluate_bench(256, 16, 0, true, 8);
    evaluate_bench(1_024, 8, 0, false, 5);
}
