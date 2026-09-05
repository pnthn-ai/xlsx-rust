//! Before/after microbench for Excel `SUBSTITUTE`.
//!
//! Compares the quadratic `replace_range` baseline (`excel_substitute_naive`)
//! with the single-allocation production kernel (`excel_substitute`).
//!
//! ```text
//! cargo bench -p xlsx-engine-core --bench substitute
//! ```

use std::hint::black_box;
use std::time::{Duration, Instant};
use xlsx_engine_core::{excel_substitute, excel_substitute_naive};

const ITERS_HEAVY: u32 = 8;
const ITERS_LIGHT: u32 = 40;

struct Case {
    name: &'static str,
    text: String,
    old: &'static str,
    new: &'static str,
    instance: Option<u32>,
    iters: u32,
}

fn cases() -> Vec<Case> {
    let many_a = "a".repeat(200_000);
    let rare = {
        let mut s = "xyz".repeat(80_000);
        s.push_str("needle");
        s.push_str(&"xyz".repeat(80_000));
        s
    };
    let foos = "foo-".repeat(50_000);
    let overlapping = "a".repeat(200_000);
    vec![
        Case {
            name: "200k 'a' → 'b' (many replacements)",
            text: many_a,
            old: "a",
            new: "b",
            instance: None,
            iters: ITERS_HEAVY,
        },
        Case {
            name: "160k 'xyz' + one needle (rare hit)",
            text: rare,
            old: "needle",
            new: "pin",
            instance: None,
            iters: ITERS_LIGHT,
        },
        Case {
            name: "50k 'foo-' replace-all",
            text: foos.clone(),
            old: "foo",
            new: "bar",
            instance: None,
            iters: ITERS_HEAVY,
        },
        Case {
            name: "50k 'foo-' replace 25000th only",
            text: foos,
            old: "foo",
            new: "bar",
            instance: Some(25_000),
            iters: ITERS_LIGHT,
        },
        Case {
            name: "200k 'a' overlapping 'aa' → 'b'",
            text: overlapping,
            old: "aa",
            new: "b",
            instance: None,
            iters: ITERS_HEAVY,
        },
        Case {
            name: "200k empty-old no-op",
            text: "x".repeat(200_000),
            old: "",
            new: "y",
            instance: None,
            iters: ITERS_LIGHT,
        },
    ]
}

fn time_it(iters: u32, mut f: impl FnMut()) -> Duration {
    // Warmup
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
    println!("SUBSTITUTE kernel bench (naive replace_range vs specialized)");
    println!(
        "{:<42} {:>12} {:>12} {:>8}",
        "case", "naive", "optimized", "speedup"
    );
    println!("{}", "-".repeat(78));
    for c in cases() {
        let naive = time_it(c.iters, || {
            black_box(excel_substitute_naive(
                black_box(&c.text),
                black_box(c.old),
                black_box(c.new),
                black_box(c.instance),
            ));
        });
        let fast = time_it(c.iters, || {
            black_box(excel_substitute(
                black_box(&c.text),
                black_box(c.old),
                black_box(c.new),
                black_box(c.instance),
            ));
        });
        let speedup = naive.as_secs_f64() / fast.as_secs_f64().max(1e-12);
        println!(
            "{:<42} {:>12} {:>12} {:>7.1}x",
            c.name,
            fmt_dur(naive),
            fmt_dur(fast),
            speedup
        );
        let a = excel_substitute_naive(&c.text, c.old, c.new, c.instance);
        let b = excel_substitute(&c.text, c.old, c.new, c.instance);
        assert_eq!(a, b, "semantic mismatch on {}", c.name);
    }
}
