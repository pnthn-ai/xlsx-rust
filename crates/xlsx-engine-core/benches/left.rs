//! Before/after microbench for Excel `LEFT`.
//!
//! Compares the `Vec<char>` baseline (`excel_left_naive`) with the
//! specialized production kernel (`excel_left`) across string sizes.
//!
//! ```text
//! cargo bench -p xlsx-engine-core --bench left
//! ```

use std::hint::black_box;
use std::time::{Duration, Instant};
use xlsx_engine_core::{excel_left, excel_left_naive};

const ITERS_HEAVY: u32 = 80;
const ITERS_LIGHT: u32 = 400;

struct Case {
    name: &'static str,
    text: String,
    n: u64,
    iters: u32,
    inner: u32,
}

fn cases() -> Vec<Case> {
    let ascii_200k = "x".repeat(200_000);
    vec![
        Case {
            name: "200k ASCII first 1",
            text: ascii_200k.clone(),
            n: 1,
            iters: ITERS_HEAVY,
            inner: 1,
        },
        Case {
            name: "200k ASCII first 16",
            text: ascii_200k.clone(),
            n: 16,
            iters: ITERS_HEAVY,
            inner: 1,
        },
        Case {
            name: "200k ASCII first 100k",
            text: ascii_200k.clone(),
            n: 100_000,
            iters: ITERS_HEAVY,
            inner: 1,
        },
        Case {
            name: "200k ASCII take-all (n=len)",
            text: ascii_200k.clone(),
            n: 200_000,
            iters: ITERS_HEAVY,
            inner: 1,
        },
        Case {
            name: "200k ASCII oversize (n>len)",
            text: ascii_200k,
            n: 200_001,
            iters: ITERS_HEAVY,
            inner: 1,
        },
        Case {
            name: "50k 'é' first 1 scalar",
            text: "é".repeat(50_000),
            n: 1,
            iters: ITERS_LIGHT,
            inner: 200,
        },
        Case {
            name: "50k 'é' first 25k scalars",
            text: "é".repeat(50_000),
            n: 25_000,
            iters: ITERS_HEAVY,
            inner: 1,
        },
        Case {
            name: "10k emoji first 1 scalar",
            text: "😀".repeat(10_000),
            n: 1,
            iters: ITERS_LIGHT,
            inner: 200,
        },
        Case {
            name: "10k emoji first 5k scalars",
            text: "😀".repeat(10_000),
            n: 5_000,
            iters: ITERS_LIGHT,
            inner: 1,
        },
        Case {
            name: "empty n=1",
            text: String::new(),
            n: 1,
            iters: ITERS_LIGHT,
            inner: 200,
        },
        Case {
            name: "n=0 on 200k ASCII",
            text: "x".repeat(200_000),
            n: 0,
            iters: ITERS_LIGHT,
            inner: 20,
        },
        Case {
            name: "Microsoft LEFT(\"Sale Price\", 4)",
            text: "Sale Price".into(),
            n: 4,
            iters: ITERS_LIGHT,
            inner: 200,
        },
    ]
}

fn time_batch(iters: u32, inner: u32, mut f: impl FnMut()) -> Duration {
    f();
    let start = Instant::now();
    for _ in 0..iters {
        for _ in 0..inner {
            f();
        }
    }
    start.elapsed() / (iters * inner)
}

fn fmt_dur(d: Duration) -> String {
    let ns = d.as_secs_f64() * 1e9;
    if ns >= 1_000_000.0 {
        format!("{:.2} ms", ns / 1e6)
    } else if ns >= 1000.0 {
        format!("{:.1} µs", ns / 1000.0)
    } else {
        format!("{ns:.0} ns")
    }
}

fn main() {
    println!("LEFT kernel bench (Vec<char> naive vs specialized)");
    println!(
        "{:<36} {:>12} {:>12} {:>8}",
        "case", "naive", "optimized", "speedup"
    );
    println!("{}", "-".repeat(72));
    for c in cases() {
        let naive = time_batch(c.iters, c.inner, || {
            black_box(excel_left_naive(black_box(&c.text), black_box(c.n)));
        });
        let fast = time_batch(c.iters, c.inner, || {
            black_box(excel_left(black_box(&c.text), black_box(c.n)));
        });
        let speedup = naive.as_secs_f64() / fast.as_secs_f64().max(1e-12);
        println!(
            "{:<36} {:>12} {:>12} {:>7.1}x",
            c.name,
            fmt_dur(naive),
            fmt_dur(fast),
            speedup
        );
        let a = excel_left_naive(&c.text, c.n);
        let b = excel_left(&c.text, c.n);
        assert_eq!(a, b, "semantic mismatch on {}", c.name);
    }
}
