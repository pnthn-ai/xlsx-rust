//! Before/after microbench for Excel `CHAR`.
//!
//! Compares the CP1252 `match` + `to_string` baseline (`excel_char_naive`)
//! with the production static-table lookup (`excel_char`).
//!
//! ```text
//! cargo bench -p xlsx-engine-core --bench excel_char
//! ```

use std::hint::black_box;
use std::time::{Duration, Instant};
use xlsx_engine_core::{excel_char, excel_char_naive};

const ITERS_HEAVY: u32 = 40;
const ITERS_LIGHT: u32 = 80;

struct Case {
    name: &'static str,
    numbers: Vec<f64>,
    iters: u32,
}

fn cases() -> Vec<Case> {
    let ascii: Vec<f64> = (0..200_000).map(|i| 32.0 + (i % 95) as f64).collect();
    let ints_1_255: Vec<f64> = (0..200_000).map(|i| 1.0 + (i % 255) as f64).collect();
    let fracs: Vec<f64> = (0..200_000).map(|i| 1.1 + (i % 255) as f64 + 0.7).collect();
    let cp1252: Vec<f64> = (0..200_000).map(|i| 128.0 + (i % 128) as f64).collect();
    let mixed_domain: Vec<f64> = (0..200_000)
        .map(|i| match i % 7 {
            0 => 65.0,
            1 => 128.4,
            2 => 0.0,
            3 => 256.0,
            4 => -1.0,
            5 => 255.9,
            _ => 1e20,
        })
        .collect();
    vec![
        Case {
            name: "200k CHAR(65) integer hot path",
            numbers: vec![65.0; 200_000],
            iters: ITERS_HEAVY,
        },
        Case {
            name: "200k printable ASCII 32..=126",
            numbers: ascii,
            iters: ITERS_HEAVY,
        },
        Case {
            name: "200k sweep 1..=255",
            numbers: ints_1_255,
            iters: ITERS_HEAVY,
        },
        Case {
            name: "200k fractional (trunc)",
            numbers: fracs,
            iters: ITERS_HEAVY,
        },
        Case {
            name: "200k CP1252 128..=255",
            numbers: cp1252,
            iters: ITERS_HEAVY,
        },
        Case {
            name: "200k mixed in-range / #VALUE!",
            numbers: mixed_domain,
            iters: ITERS_LIGHT,
        },
        Case {
            name: "Microsoft CHAR(65) × 50k",
            numbers: vec![65.0; 50_000],
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
    println!("CHAR kernel bench (CP1252 match+to_string vs static table)");
    println!(
        "{:<40} {:>12} {:>12} {:>8}",
        "case", "naive", "optimized", "speedup"
    );
    println!("{}", "-".repeat(76));
    for c in cases() {
        let naive = time_it(c.iters, || {
            for &n in &c.numbers {
                let _ = black_box(excel_char_naive(black_box(n)));
            }
        });
        let fast = time_it(c.iters, || {
            for &n in &c.numbers {
                let _ = black_box(excel_char(black_box(n)));
            }
        });
        let speedup = naive.as_secs_f64() / fast.as_secs_f64().max(1e-12);
        println!(
            "{:<40} {:>12} {:>12} {:>7.1}x",
            c.name,
            fmt_dur(naive),
            fmt_dur(fast),
            speedup
        );
        for &n in &c.numbers {
            let a = excel_char_naive(n);
            let b = excel_char(n).map(str::to_owned);
            assert_eq!(a, b, "semantic mismatch CHAR({n})");
        }
    }
}
