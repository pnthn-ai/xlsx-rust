//! Before/after microbench for Excel `LOWER`.
//!
//! Compares the `Vec<char>` baseline (`excel_lower_naive`) with the
//! production kernel (`excel_lower`: SWAR A–Z fold + Unicode fallback).
//!
//! ```text
//! cargo bench -p xlsx-engine-core --bench lower
//! ```

use std::hint::black_box;
use std::time::{Duration, Instant};
use xlsx_engine_core::{excel_lower, excel_lower_naive};

const ITERS_HEAVY: u32 = 40;
const ITERS_LIGHT: u32 = 80;

struct Case {
    name: &'static str,
    text: String,
    iters: u32,
}

fn cases() -> Vec<Case> {
    let already = "x".repeat(200_000);
    let all_upper = "A".repeat(200_000);
    let mixed = "Ab".repeat(100_000);
    let digits = "7".repeat(200_000);
    let punct = "@[`{".repeat(50_000);
    let unicode_upper = "CAFÉ".repeat(50_000);
    let unicode_ident = "café".repeat(50_000);
    vec![
        Case {
            name: "200k already-lower ASCII (identity)",
            text: already,
            iters: ITERS_LIGHT,
        },
        Case {
            name: "200k 'A' → 'a'",
            text: all_upper,
            iters: ITERS_HEAVY,
        },
        Case {
            name: "200k 'Ab' mixed ASCII",
            text: mixed,
            iters: ITERS_HEAVY,
        },
        Case {
            name: "200k digits (no letters)",
            text: digits,
            iters: ITERS_LIGHT,
        },
        Case {
            name: "200k punct around A-Z edges",
            text: punct,
            iters: ITERS_LIGHT,
        },
        Case {
            name: "200k CAFÉ unicode fold",
            text: unicode_upper,
            iters: ITERS_HEAVY,
        },
        Case {
            name: "200k café already-lower unicode",
            text: unicode_ident,
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
    println!("LOWER kernel bench (Vec<char> vs SWAR ASCII fold)");
    println!(
        "{:<42} {:>12} {:>12} {:>8}",
        "case", "naive", "optimized", "speedup"
    );
    println!("{}", "-".repeat(78));
    for c in cases() {
        let naive = time_it(c.iters, || {
            let _ = black_box(excel_lower_naive(black_box(&c.text)));
        });
        let fast = time_it(c.iters, || {
            let _ = black_box(excel_lower(black_box(&c.text)));
        });
        let speedup = naive.as_secs_f64() / fast.as_secs_f64().max(1e-12);
        println!(
            "{:<42} {:>12} {:>12} {:>7.1}x",
            c.name,
            fmt_dur(naive),
            fmt_dur(fast),
            speedup
        );
        let a = excel_lower_naive(&c.text);
        let b = excel_lower(&c.text);
        assert_eq!(a, b, "semantic mismatch on {}", c.name);
    }
}
