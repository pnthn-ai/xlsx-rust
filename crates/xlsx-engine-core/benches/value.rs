//! Before/after microbench for Excel `VALUE`.
//!
//! Compares the allocating cleanup baseline (`excel_value_naive`) with the
//! production kernel (`excel_value`: no-alloc byte walk + stack comma strip).
//!
//! ```text
//! cargo bench -p xlsx-engine-core --bench value
//! ```

use std::hint::black_box;
use std::time::{Duration, Instant};
use xlsx_engine_core::{excel_value, excel_value_naive};
use xlsx_types::DateSystem;

const ITERS_HEAVY: u32 = 80;
const ITERS_LIGHT: u32 = 160;

struct Case {
    name: &'static str,
    text: String,
    iters: u32,
}

fn cases() -> Vec<Case> {
    vec![
        Case {
            name: "plain integer \"123456789\"",
            text: "123456789".into(),
            iters: ITERS_LIGHT,
        },
        Case {
            name: "trimmed decimal \"   123.45   \"",
            text: "   123.45   ".into(),
            iters: ITERS_LIGHT,
        },
        Case {
            name: "currency \"$1,234,567.89\"",
            text: "$1,234,567.89".into(),
            iters: ITERS_HEAVY,
        },
        Case {
            name: "percent \"12.5%\"",
            text: "12.5%".into(),
            iters: ITERS_LIGHT,
        },
        Case {
            name: "parens+currency \"($1,234.50)\"",
            text: "($1,234.50)".into(),
            iters: ITERS_HEAVY,
        },
        Case {
            name: "scientific \"1.23456789E+10\"",
            text: "1.23456789E+10".into(),
            iters: ITERS_LIGHT,
        },
        Case {
            name: "time \"16:48:00\"",
            text: "16:48:00".into(),
            iters: ITERS_HEAVY,
        },
        Case {
            name: "date \"12/31/2024\"",
            text: "12/31/2024".into(),
            iters: ITERS_HEAVY,
        },
        Case {
            name: "reject \"not-a-number\"",
            text: "not-a-number".into(),
            iters: ITERS_LIGHT,
        },
        Case {
            name: "64-digit grouped currency",
            text: "$123,456,789,012.34".into(),
            iters: ITERS_HEAVY,
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
    if ns >= 1000.0 {
        format!("{:.1} µs", ns / 1000.0)
    } else {
        format!("{ns:.1} ns")
    }
}

fn main() {
    println!("VALUE kernel bench (allocating cleanup vs no-alloc scan)");
    println!(
        "{:<42} {:>12} {:>12} {:>8}",
        "case", "naive", "optimized", "speedup"
    );
    println!("{}", "-".repeat(78));
    let system = DateSystem::Excel1900;
    for c in cases() {
        let naive = time_it(c.iters, || {
            let _ = black_box(excel_value_naive(black_box(&c.text), system));
        });
        let fast = time_it(c.iters, || {
            let _ = black_box(excel_value(black_box(&c.text), system));
        });
        let speedup = naive.as_secs_f64() / fast.as_secs_f64().max(1e-12);
        println!(
            "{:<42} {:>12} {:>12} {:>7.1}x",
            c.name,
            fmt_dur(naive),
            fmt_dur(fast),
            speedup
        );
        let a = excel_value_naive(&c.text, system);
        let b = excel_value(&c.text, system);
        assert_eq!(a, b, "semantic mismatch on {}", c.name);
    }
}
