//! Before/after microbench for Excel `SORT`.
//!
//! Compares the insertion-sort baseline (`excel_sort_naive`: clone-all +
//! O(n²) swaps, transpose for `by_col`) with the production kernel
//! (`excel_sort`: extract keys once, stable index permute, assemble).
//!
//! ```text
//! cargo bench -p xlsx-engine-core --bench sort
//! ```

use std::hint::black_box;
use std::time::{Duration, Instant};
use xlsx_engine_core::{excel_sort, excel_sort_naive, CalcCoreEngine};
use xlsx_types::{Candidate, Cell, EvalSpec, EvalTarget, ExcelValue, Sheet, Workbook};

const COLS: usize = 8;
const PAYLOAD: usize = 32;

struct Case {
    name: &'static str,
    array: ExcelValue,
    sort_index: Option<ExcelValue>,
    sort_order: Option<ExcelValue>,
    by_col: Option<ExcelValue>,
    run_naive: bool,
    iters: u32,
}

fn text_row(tag: usize) -> Vec<ExcelValue> {
    (0..COLS)
        .map(|c| ExcelValue::Text(format!("{tag:05}-{c}-{}", "x".repeat(PAYLOAD))))
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

fn reverse_text(n: usize) -> ExcelValue {
    matrix(n, |i| text_row(n - 1 - i))
}

fn reverse_number(n: usize) -> ExcelValue {
    matrix(n, |i| number_row(n - 1 - i))
}

fn already_text(n: usize) -> ExcelValue {
    matrix(n, text_row)
}

fn wide_text(cols: usize) -> ExcelValue {
    ExcelValue::Array(
        (0..8)
            .map(|r| {
                (0..cols)
                    .map(|c| ExcelValue::Text(format!("{r}-{c:05}-{}", "y".repeat(16))))
                    .collect()
            })
            .collect(),
    )
}

fn cases() -> Vec<Case> {
    vec![
        Case {
            name: "4k×8 text, reverse rows",
            array: reverse_text(4_096),
            sort_index: None,
            sort_order: None,
            by_col: None,
            run_naive: true,
            iters: 8,
        },
        Case {
            name: "4k×8 text, already sorted",
            array: already_text(4_096),
            sort_index: None,
            sort_order: None,
            by_col: None,
            run_naive: true,
            iters: 10,
        },
        Case {
            name: "4k×8 numeric, reverse rows",
            array: reverse_number(4_096),
            sort_index: None,
            sort_order: None,
            by_col: None,
            run_naive: true,
            iters: 10,
        },
        Case {
            name: "4k×8 text, descending",
            array: already_text(4_096),
            sort_index: None,
            sort_order: Some(ExcelValue::Number(-1.0)),
            by_col: None,
            run_naive: true,
            iters: 8,
        },
        Case {
            name: "8×4k text, by_col reverse keys",
            array: wide_text(4_096),
            sort_index: None,
            sort_order: Some(ExcelValue::Number(-1.0)),
            by_col: Some(ExcelValue::Bool(true)),
            run_naive: true,
            iters: 8,
        },
        Case {
            name: "8k×8 text, reverse (fast only)",
            array: reverse_text(8_192),
            sort_index: None,
            sort_order: None,
            by_col: None,
            run_naive: false,
            iters: 10,
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
            .insert(a1, Cell::value(ExcelValue::Number((n - 1 - i) as f64)));
    }
    Workbook {
        sheets: vec![sheet],
        names: vec![],
    }
}

fn evaluate_bench(n: u32, iters: u32) {
    let wb = workbook_n(n);
    let formula = format!("=SORT(A1:A{n})");
    let spec = EvalSpec {
        case_id: "bench.sort".into(),
        workbook: wb,
        target: EvalTarget::formula(formula),
        options: Default::default(),
    };
    let engine = CalcCoreEngine::new();
    let got = engine.evaluate(&spec).expect("evaluate");
    match &got {
        ExcelValue::Array(rows) => {
            assert_eq!(rows.len(), n as usize);
            assert_eq!(rows[0][0], ExcelValue::Number(0.0));
            assert_eq!(rows[n as usize - 1][0], ExcelValue::Number((n - 1) as f64));
        }
        other => panic!("expected array, got {other}"),
    }
    let ms = time_it(iters, || {
        black_box(engine.evaluate(black_box(&spec)).unwrap());
    });
    println!(
        "evaluate n={n:>6}  {}/call  formula=SORT(A1:A{n})",
        fmt_dur(ms)
    );
}

fn main() {
    println!("SORT kernel bench (insertion/transpose vs key-extract + index permute)");
    println!(
        "{:<42} {:>12} {:>12} {:>8}",
        "case", "naive", "optimized", "speedup"
    );
    println!("{}", "-".repeat(80));
    for c in cases() {
        let idx = c.sort_index.as_ref();
        let order = c.sort_order.as_ref();
        let by = c.by_col.as_ref();
        let fast = time_it(c.iters, || {
            let _ = black_box(excel_sort(
                black_box(&c.array),
                black_box(idx),
                black_box(order),
                black_box(by),
            ));
        });
        if c.run_naive {
            let naive = time_it(c.iters, || {
                let _ = black_box(excel_sort_naive(
                    black_box(&c.array),
                    black_box(idx),
                    black_box(order),
                    black_box(by),
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
            let a = excel_sort_naive(&c.array, idx, order, by);
            let b = excel_sort(&c.array, idx, order, by);
            assert_eq!(a, b, "semantic mismatch on {}", c.name);
        } else {
            println!(
                "{:<42} {:>12} {:>12} {:>7}",
                c.name,
                "skipped",
                fmt_dur(fast),
                "—"
            );
        }
    }
    println!();
    evaluate_bench(10_000, 8);
    evaluate_bench(100_000, 3);
}
