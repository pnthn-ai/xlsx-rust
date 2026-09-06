//! Before/after microbench for Excel `EXACT`.
//!
//! Compares the `to_text` + `Vec<char>` baseline (`excel_exact_naive`)
//! with the production kernel (`excel_exact`: borrowed text, length
//! reject, integer itoa, memcmp).
//!
//! ```text
//! cargo bench -p xlsx-engine-core --bench exact
//! ```

use std::hint::black_box;
use std::time::{Duration, Instant};
use xlsx_engine_core::{excel_exact, excel_exact_naive};
use xlsx_types::ExcelValue;

const ITERS_HEAVY: u32 = 40;
const ITERS_LIGHT: u32 = 80;

struct Case {
    name: &'static str,
    left: ExcelValue,
    right: ExcelValue,
    iters: u32,
}

fn cases() -> Vec<Case> {
    let eq = "x".repeat(200_000);
    let first = {
        let mut s = eq.clone();
        s.replace_range(0..1, "y");
        s
    };
    let last = {
        let mut s = eq.clone();
        let n = s.len();
        s.replace_range(n - 1..n, "y");
        s
    };
    let short = "x".repeat(199_999);
    let cafe = "café".repeat(50_000);
    vec![
        Case {
            name: "200k ASCII equal (identity)",
            left: ExcelValue::Text(eq.clone()),
            right: ExcelValue::Text(eq.clone()),
            iters: ITERS_LIGHT,
        },
        Case {
            name: "200k ASCII differ at first byte",
            left: ExcelValue::Text(eq.clone()),
            right: ExcelValue::Text(first),
            iters: ITERS_LIGHT,
        },
        Case {
            name: "200k ASCII differ at last byte",
            left: ExcelValue::Text(eq.clone()),
            right: ExcelValue::Text(last),
            iters: ITERS_HEAVY,
        },
        Case {
            name: "200k vs 199999 length reject",
            left: ExcelValue::Text(eq.clone()),
            right: ExcelValue::Text(short),
            iters: ITERS_LIGHT,
        },
        Case {
            name: "200k text vs number 1",
            left: ExcelValue::Text(eq),
            right: ExcelValue::Number(1.0),
            iters: ITERS_LIGHT,
        },
        Case {
            name: "200k café-repeat unicode equal",
            left: ExcelValue::Text(cafe.clone()),
            right: ExcelValue::Text(cafe),
            iters: ITERS_HEAVY,
        },
        Case {
            name: "integer 42 vs \"42\"",
            left: ExcelValue::Number(42.0),
            right: ExcelValue::Text("42".into()),
            iters: ITERS_LIGHT,
        },
        Case {
            name: "TRUE vs TRUE (bool)",
            left: ExcelValue::Bool(true),
            right: ExcelValue::Bool(true),
            iters: ITERS_LIGHT,
        },
        Case {
            name: "empty vs empty",
            left: ExcelValue::Empty,
            right: ExcelValue::Empty,
            iters: ITERS_LIGHT,
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
    println!("EXACT kernel bench (to_text+Vec<char> vs borrow/memcmp/itoa)");
    println!(
        "{:<42} {:>12} {:>12} {:>8}",
        "case", "naive", "optimized", "speedup"
    );
    println!("{}", "-".repeat(78));
    for c in cases() {
        let naive = time_it(c.iters, || {
            let _ = black_box(excel_exact_naive(black_box(&c.left), black_box(&c.right)));
        });
        let fast = time_it(c.iters, || {
            let _ = black_box(excel_exact(black_box(&c.left), black_box(&c.right)));
        });
        let speedup = naive.as_secs_f64() / fast.as_secs_f64().max(1e-12);
        println!(
            "{:<42} {:>12} {:>12} {:>7.1}x",
            c.name,
            fmt_dur(naive),
            fmt_dur(fast),
            speedup
        );
        let a = excel_exact_naive(&c.left, &c.right);
        let b = excel_exact(&c.left, &c.right);
        assert_eq!(a, b, "semantic mismatch on {}", c.name);
    }
}
