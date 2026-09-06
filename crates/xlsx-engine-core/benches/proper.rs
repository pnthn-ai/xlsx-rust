//! Before/after microbench for Excel `PROPER`.
//!
//! Compares the `Vec<char>` + per-scalar `to_string` baseline
//! (`excel_proper_naive`) with the in-place ASCII byte walk (`excel_proper`).
//!
//! ```text
//! cargo bench -p xlsx-engine-core --bench proper
//! ```

use std::hint::black_box;
use std::time::{Duration, Instant};
use xlsx_engine_core::{excel_proper, excel_proper_naive};

const ITERS_HEAVY: u32 = 16;
const ITERS_LIGHT: u32 = 40;

struct Case {
    name: &'static str,
    text: String,
    iters: u32,
}

fn cases() -> Vec<Case> {
    let mixed = "aB-cD'eF 76x ".repeat(20_000);
    let already = "Ab-Cd'Ef 76X ".repeat(20_000);
    let caps = "HELLO WORLD ".repeat(20_000);
    let lower = "hello world ".repeat(20_000);
    let punct = "...hello_world--2nd ".repeat(16_000);
    let accented = "école café straße ".repeat(10_000);
    vec![
        Case {
            name: "260k mixed ASCII title-case",
            text: mixed,
            iters: ITERS_HEAVY,
        },
        Case {
            name: "260k already-PROPER ASCII",
            text: already,
            iters: ITERS_HEAVY,
        },
        Case {
            name: "240k ALLCAPS words",
            text: caps,
            iters: ITERS_HEAVY,
        },
        Case {
            name: "240k lowercase words",
            text: lower,
            iters: ITERS_HEAVY,
        },
        Case {
            name: "320k punct / digit breaks",
            text: punct,
            iters: ITERS_HEAVY,
        },
        Case {
            name: "180k accented + ASCII mix",
            text: accented,
            iters: ITERS_LIGHT,
        },
        Case {
            name: "empty no-op",
            text: String::new(),
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
    println!("PROPER kernel bench (Vec<char> naive vs in-place ASCII walk)");
    println!(
        "{:<42} {:>12} {:>12} {:>8}",
        "case", "naive", "optimized", "speedup"
    );
    println!("{}", "-".repeat(78));
    for c in cases() {
        let naive = time_it(c.iters, || {
            black_box(excel_proper_naive(black_box(&c.text)));
        });
        let fast = time_it(c.iters, || {
            black_box(excel_proper(black_box(&c.text)));
        });
        let speedup = naive.as_secs_f64() / fast.as_secs_f64().max(1e-12);
        println!(
            "{:<42} {:>12} {:>12} {:>7.1}x",
            c.name,
            fmt_dur(naive),
            fmt_dur(fast),
            speedup
        );
        let a = excel_proper_naive(&c.text);
        let b = excel_proper(&c.text);
        assert_eq!(a, b, "semantic mismatch on {}", c.name);
    }
}
