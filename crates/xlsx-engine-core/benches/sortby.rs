//! Before/after microbench for Excel `SORTBY`.
//!
//! Compares the insertion-sort baseline (`excel_sortby_naive`: clone-all +
//! O(n²) swaps, transpose for column sorts) with the production kernel
//! (`excel_sortby`: extract keys once, stable index permute, assemble).
//!
//! ```text
//! cargo bench -p xlsx-engine-core --bench sortby
//! ```

use std::hint::black_box;
use std::time::{Duration, Instant};
use xlsx_engine_core::{excel_sortby, excel_sortby_naive, CalcCoreEngine};
use xlsx_types::{Candidate, Cell, EvalSpec, EvalTarget, ExcelValue, Sheet, Workbook};

const COLS: usize = 8;
const PAYLOAD: usize = 32;

struct Case {
    name: &'static str,
    array: ExcelValue,
    by: ExcelValue,
    order: Option<ExcelValue>,
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

fn key_col_reverse(n: usize) -> ExcelValue {
    ExcelValue::Array(
        (0..n)
            .map(|i| vec![ExcelValue::Number((n - 1 - i) as f64)])
            .collect(),
    )
}

fn key_col_sorted(n: usize) -> ExcelValue {
    ExcelValue::Array((0..n).map(|i| vec![ExcelValue::Number(i as f64)]).collect())
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

fn key_row_reverse(cols: usize) -> ExcelValue {
    ExcelValue::Array(vec![(0..cols)
        .map(|c| ExcelValue::Number((cols - 1 - c) as f64))
        .collect()])
}

fn cases() -> Vec<Case> {
    vec![
        Case {
            name: "4k×8 text, reverse keys",
            array: reverse_text(4_096),
            by: key_col_reverse(4_096),
            order: None,
            run_naive: true,
            iters: 8,
        },
        Case {
            name: "4k×8 text, already sorted",
            array: already_text(4_096),
            by: key_col_sorted(4_096),
            order: None,
            run_naive: true,
            iters: 10,
        },
        Case {
            name: "4k×8 numeric, reverse keys",
            array: reverse_number(4_096),
            by: key_col_reverse(4_096),
            order: None,
            run_naive: true,
            iters: 10,
        },
        Case {
            name: "4k×8 text, descending",
            array: already_text(4_096),
            by: key_col_sorted(4_096),
            order: Some(ExcelValue::Number(-1.0)),
            run_naive: true,
            iters: 8,
        },
        Case {
            name: "8×4k text, row-key reverse",
            array: wide_text(4_096),
            by: key_row_reverse(4_096),
            order: None,
            run_naive: true,
            iters: 8,
        },
        Case {
            name: "8k×8 text, reverse (fast only)",
            array: reverse_text(8_192),
            by: key_col_reverse(8_192),
            order: None,
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
        let b1 = format!("B{}", i + 1);
        sheet
            .cells
            .insert(a1, Cell::value(ExcelValue::Number((n - 1 - i) as f64)));
        sheet
            .cells
            .insert(b1, Cell::value(ExcelValue::Number((n - 1 - i) as f64)));
    }
    Workbook {
        sheets: vec![sheet],
        names: vec![],
    }
}

fn evaluate_bench(n: u32, iters: u32) {
    let wb = workbook_n(n);
    let formula = format!("=SORTBY(A1:A{n}, B1:B{n})");
    let spec = EvalSpec {
        case_id: "bench.sortby".into(),
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
        "evaluate n={n:>6}  {}/call  formula=SORTBY(A1:A{n}, B1:B{n})",
        fmt_dur(ms)
    );
}

fn main() {
    println!("SORTBY kernel bench (insertion/transpose vs key-extract + index permute)");
    println!(
        "{:<42} {:>12} {:>12} {:>8}",
        "case", "naive", "optimized", "speedup"
    );
    println!("{}", "-".repeat(80));
    for c in cases() {
        let order = c.order.as_ref();
        let keys = [(&c.by, order)];
        let fast = time_it(c.iters, || {
            let _ = black_box(excel_sortby(black_box(&c.array), black_box(&keys)));
        });
        if c.run_naive {
            let naive = time_it(c.iters, || {
                let _ = black_box(excel_sortby_naive(black_box(&c.array), black_box(&keys)));
            });
            let speedup = naive.as_secs_f64() / fast.as_secs_f64().max(1e-12);
            println!(
                "{:<42} {:>12} {:>12} {:>7.1}x",
                c.name,
                fmt_dur(naive),
                fmt_dur(fast),
                speedup
            );
            let a = excel_sortby_naive(&c.array, &keys);
            let b = excel_sortby(&c.array, &keys);
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
