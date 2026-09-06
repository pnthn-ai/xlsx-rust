//! Before/after microbench for Excel `TEXT`.
//!
//! Compares the no-cache generic parser (`excel_text_naive`) with the
//! production kernel (`excel_text`: literal fast paths + interned plans).
//!
//! ```text
//! cargo bench -p xlsx-engine-core --bench text
//! ```

use std::hint::black_box;
use std::time::{Duration, Instant};
use xlsx_engine_core::{excel_text, excel_text_naive};
use xlsx_types::{DateSystem, ExcelValue};

const ITERS: u32 = 80_000;

struct Case {
    name: &'static str,
    value: ExcelValue,
    fmt: &'static str,
}

fn cases() -> Vec<Case> {
    vec![
        Case {
            name: "0.00 1234.567",
            value: ExcelValue::Number(1234.567),
            fmt: "0.00",
        },
        Case {
            name: "#,##0 million",
            value: ExcelValue::Number(1_234_567.0),
            fmt: "#,##0",
        },
        Case {
            name: "$#,##0.00",
            value: ExcelValue::Number(1234.567),
            fmt: "$#,##0.00",
        },
        Case {
            name: "0.00% 0.285",
            value: ExcelValue::Number(0.285),
            fmt: "0.00%",
        },
        Case {
            name: "0000000 pad",
            value: ExcelValue::Number(1234.0),
            fmt: "0000000",
        },
        Case {
            name: "yyyy-mm-dd",
            value: ExcelValue::Number(45366.0),
            fmt: "yyyy-mm-dd",
        },
        Case {
            name: "mm/dd/yyyy",
            value: ExcelValue::Number(45366.0),
            fmt: "mm/dd/yyyy",
        },
        Case {
            name: "#.# omit zero",
            value: ExcelValue::Number(0.5),
            fmt: "#.#",
        },
        Case {
            name: "@ general",
            value: ExcelValue::Number(1234.5),
            fmt: "@",
        },
        Case {
            name: "non-numeric text",
            value: ExcelValue::Text("abc".into()),
            fmt: "0.00",
        },
        Case {
            name: "quoted USD group",
            value: ExcelValue::Number(1234.0),
            fmt: "\"USD \"#,##0",
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
    let ns = d.as_secs_f64() * 1e9;
    if ns >= 1_000_000.0 {
        format!("{:.2} ms", ns / 1e6)
    } else if ns >= 1000.0 {
        format!("{:.1} µs", ns / 1000.0)
    } else {
        format!("{ns:.1} ns")
    }
}

fn main() {
    println!("TEXT kernel bench (reparse naive vs fast-path / interned plan)");
    println!(
        "{:<22} {:>12} {:>12} {:>8}",
        "case", "naive", "optimized", "speedup"
    );
    println!("{}", "-".repeat(58));
    for c in cases() {
        let naive = time_it(ITERS, || {
            let _ = black_box(excel_text_naive(
                black_box(&c.value),
                black_box(c.fmt),
                black_box(DateSystem::Excel1900),
            ));
        });
        let fast = time_it(ITERS, || {
            let _ = black_box(excel_text(
                black_box(&c.value),
                black_box(c.fmt),
                black_box(DateSystem::Excel1900),
            ));
        });
        let speedup = naive.as_secs_f64() / fast.as_secs_f64().max(1e-12);
        println!(
            "{:<22} {:>12} {:>12} {:>7.1}x",
            c.name,
            fmt_dur(naive),
            fmt_dur(fast),
            speedup
        );
        let a = excel_text_naive(&c.value, c.fmt, DateSystem::Excel1900).unwrap();
        let b = excel_text(&c.value, c.fmt, DateSystem::Excel1900).unwrap();
        assert_eq!(a, b, "semantic mismatch on {}", c.name);
    }
}
