//! Before/after microbench for Excel `IFS`.
//!
//! Compares the materializing baseline (`excel_ifs_naive`) with the
//! production single-pass kernel (`excel_ifs`). Both evaluate every pair
//! (Excel does not short-circuit `IFS`); the optimized path clones only
//! the winning value.
//!
//! ```text
//! cargo bench -p xlsx-engine-core --bench ifs
//! ```

use std::hint::black_box;
use std::time::{Duration, Instant};
use xlsx_engine_core::{excel_ifs, excel_ifs_naive};
use xlsx_types::{ExcelError, ExcelValue};

const ITERS: u32 = 80;
const PAIRS: usize = 127;
const PAYLOAD: usize = 4096;

struct Case {
    name: &'static str,
    args: Vec<ExcelValue>,
}

fn payload(tag: u32) -> ExcelValue {
    ExcelValue::Text(format!("{tag:04}-{}", "x".repeat(PAYLOAD)))
}

fn first_true() -> Vec<ExcelValue> {
    let mut args = Vec::with_capacity(PAIRS * 2);
    args.push(ExcelValue::Bool(true));
    args.push(payload(0));
    for i in 1..PAIRS {
        args.push(ExcelValue::Bool(false));
        args.push(payload(i as u32));
    }
    args
}

fn last_true() -> Vec<ExcelValue> {
    let mut args = Vec::with_capacity(PAIRS * 2);
    for i in 0..PAIRS - 1 {
        args.push(ExcelValue::Bool(false));
        args.push(payload(i as u32));
    }
    args.push(ExcelValue::Bool(true));
    args.push(payload((PAIRS - 1) as u32));
    args
}

fn no_match() -> Vec<ExcelValue> {
    let mut args = Vec::with_capacity(PAIRS * 2);
    for i in 0..PAIRS {
        args.push(ExcelValue::Bool(false));
        args.push(payload(i as u32));
    }
    args
}

fn trailing_error() -> Vec<ExcelValue> {
    let mut args = first_true();
    let n = args.len();
    args[n - 2] = ExcelValue::Bool(false);
    args[n - 1] = ExcelValue::Error(ExcelError::Div0);
    args
}

fn number_first_true() -> Vec<ExcelValue> {
    let mut args = Vec::with_capacity(PAIRS * 2);
    args.push(ExcelValue::Number(1.0));
    args.push(ExcelValue::Number(9.0));
    for i in 1..PAIRS {
        args.push(ExcelValue::Number(0.0));
        args.push(ExcelValue::Number(i as f64));
    }
    args
}

fn cases() -> Vec<Case> {
    vec![
        Case {
            name: "127 pairs, 4KiB text, first TRUE",
            args: first_true(),
        },
        Case {
            name: "127 pairs, 4KiB text, last TRUE",
            args: last_true(),
        },
        Case {
            name: "127 pairs, 4KiB text, no match #N/A",
            args: no_match(),
        },
        Case {
            name: "127 pairs, first TRUE, trailing #DIV/0!",
            args: trailing_error(),
        },
        Case {
            name: "127 pairs, numeric, first TRUE",
            args: number_first_true(),
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

fn main() {
    println!("IFS kernel bench (materialize-all-pairs vs single-pass select)");
    println!(
        "{:<44} {:>12} {:>12} {:>8}",
        "case", "naive", "optimized", "speedup"
    );
    println!("{}", "-".repeat(80));
    for c in cases() {
        let naive = time_it(ITERS, || {
            let _ = black_box(excel_ifs_naive(black_box(&c.args)));
        });
        let fast = time_it(ITERS, || {
            let _ = black_box(excel_ifs(black_box(&c.args)));
        });
        let speedup = naive.as_secs_f64() / fast.as_secs_f64().max(1e-12);
        println!(
            "{:<44} {:>12} {:>12} {:>7.1}x",
            c.name,
            fmt_dur(naive),
            fmt_dur(fast),
            speedup
        );
        let a = excel_ifs_naive(&c.args);
        let b = excel_ifs(&c.args);
        assert_eq!(a, b, "semantic mismatch on {}", c.name);
    }
}
