//! Before/after microbench for Excel `UPPER`.
//!
//! Compares the `Vec<char>` baseline (`excel_upper_naive`) with the
//! production kernel (`excel_upper`: SWAR ASCII XOR + Unicode walk).
//!
//! ```text
//! cargo bench -p xlsx-engine-core --bench upper
//! ```

use std::hint::black_box;
use std::time::{Duration, Instant};
use xlsx_engine_core::{excel_upper, excel_upper_naive};

const ITERS_HEAVY: u32 = 40;
const ITERS_LIGHT: u32 = 80;

struct Case {
    name: &'static str,
    text: String,
    iters: u32,
}

fn cases() -> Vec<Case> {
    let already = "X".repeat(200_000);
    let lower = "x".repeat(200_000);
    let mixed = "aB".repeat(100_000);
    let digits = "7".repeat(200_000);
    let unicode = "café".repeat(50_000);
    let sharp = "straße".repeat(33_334);
    vec![
        Case {
            name: "200k 'X' already upper (identity)",
            text: already,
            iters: ITERS_LIGHT,
        },
        Case {
            name: "200k 'x' all lowercase ASCII",
            text: lower,
            iters: ITERS_HEAVY,
        },
        Case {
            name: "200k 'aB' mixed ASCII",
            text: mixed,
            iters: ITERS_HEAVY,
        },
        Case {
            name: "200k digits (identity, no letters)",
            text: digits,
            iters: ITERS_LIGHT,
        },
        Case {
            name: "200k café-repeat unicode",
            text: unicode,
            iters: ITERS_HEAVY,
        },
        Case {
            name: "200k straße-repeat (ß kept)",
            text: sharp,
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
    let us = d.as_secs_f64() * 1e6;
    if us >= 1000.0 {
        format!("{:.2} ms", us / 1000.0)
    } else {
        format!("{us:.1} µs")
    }
}

fn main() {
    println!("UPPER kernel bench (Vec<char> vs SWAR ASCII / Unicode walk)");
    println!(
        "{:<42} {:>12} {:>12} {:>8}",
        "case", "naive", "optimized", "speedup"
    );
    println!("{}", "-".repeat(78));
    for c in cases() {
        let naive = time_it(c.iters, || {
            let _ = black_box(excel_upper_naive(black_box(&c.text)));
        });
        let fast = time_it(c.iters, || {
            let _ = black_box(excel_upper(black_box(&c.text)));
        });
        let speedup = naive.as_secs_f64() / fast.as_secs_f64().max(1e-12);
        println!(
            "{:<42} {:>12} {:>12} {:>7.1}x",
            c.name,
            fmt_dur(naive),
            fmt_dur(fast),
            speedup
        );
        let a = excel_upper_naive(&c.text);
        let b = excel_upper(&c.text);
        assert_eq!(a, b, "semantic mismatch on {}", c.name);
    }
}
