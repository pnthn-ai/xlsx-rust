//! Before/after microbench for Excel `RIGHT`.
//!
//! Compares the `Vec<char>` baseline (`excel_right_naive`) with the
//! production kernel (`excel_right`: ASCII suffix slice / UTF-8 walk from
//! the end).
//!
//! ```text
//! cargo bench -p xlsx-engine-core --bench right
//! ```

use std::hint::black_box;
use std::time::{Duration, Instant};
use xlsx_engine_core::{excel_right, excel_right_naive};

const ITERS_HEAVY: u32 = 40;
const ITERS_LIGHT: u32 = 80;

struct Case {
    name: &'static str,
    text: String,
    n: u64,
    iters: u32,
}

fn cases() -> Vec<Case> {
    let ascii = "a".repeat(200_000);
    let mixed = "Ab".repeat(100_000);
    let cafe = "café".repeat(50_000);
    let emoji = "😀".repeat(50_000);
    vec![
        Case {
            name: "200k ASCII last 8",
            text: ascii.clone(),
            n: 8,
            iters: ITERS_HEAVY,
        },
        Case {
            name: "200k ASCII last 1 (default)",
            text: ascii.clone(),
            n: 1,
            iters: ITERS_HEAVY,
        },
        Case {
            name: "200k ASCII take-all (n = len)",
            text: ascii.clone(),
            n: 200_000,
            iters: ITERS_LIGHT,
        },
        Case {
            name: "200k ASCII take-all (n > len)",
            text: ascii.clone(),
            n: 200_001,
            iters: ITERS_LIGHT,
        },
        Case {
            name: "200k ASCII n = 0",
            text: ascii,
            n: 0,
            iters: ITERS_LIGHT,
        },
        Case {
            name: "200k mixed ASCII last 100",
            text: mixed,
            n: 100,
            iters: ITERS_HEAVY,
        },
        Case {
            name: "200k café last 2",
            text: cafe,
            n: 2,
            iters: ITERS_HEAVY,
        },
        Case {
            name: "200k emoji last 1",
            text: emoji,
            n: 1,
            iters: ITERS_HEAVY,
        },
        Case {
            name: "Microsoft Sale Price last 5",
            text: "Sale Price".into(),
            n: 5,
            iters: 8_000,
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
    println!("RIGHT kernel bench (Vec<char> vs ASCII slice / UTF-8 suffix walk)");
    println!(
        "{:<42} {:>12} {:>12} {:>8}",
        "case", "naive", "optimized", "speedup"
    );
    println!("{}", "-".repeat(78));
    for c in cases() {
        let naive = time_it(c.iters, || {
            let _ = black_box(excel_right_naive(black_box(&c.text), black_box(c.n)));
        });
        let fast = time_it(c.iters, || {
            let _ = black_box(excel_right(black_box(&c.text), black_box(c.n)));
        });
        let speedup = naive.as_secs_f64() / fast.as_secs_f64().max(1e-12);
        println!(
            "{:<42} {:>12} {:>12} {:>7.1}x",
            c.name,
            fmt_dur(naive),
            fmt_dur(fast),
            speedup
        );
        let a = excel_right_naive(&c.text, c.n);
        let b = excel_right(&c.text, c.n);
        assert_eq!(a, b, "semantic mismatch on {}", c.name);
    }
}
