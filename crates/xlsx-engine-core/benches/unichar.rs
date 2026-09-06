//! Before/after microbench for Excel `UNICHAR`.
//!
//! Compares the UTF-16 `Vec` + `from_utf16` baseline (`excel_unichar_naive`)
//! with the production kernel (`excel_unichar`: range check, then a 1–4
//! byte UTF-8 write). Each case is a batch of already-truncated `f64`
//! code points so the timer measures conversion, not formula parse.
//!
//! ```text
//! cargo bench -p xlsx-engine-core --bench unichar
//! ```
//!
//! A faster UNICHAR that fails `xlsx-verify --candidate calc-core` is not a win.

use std::hint::black_box;
use std::time::{Duration, Instant};
use xlsx_engine_core::{excel_unichar, excel_unichar_naive};

const ITERS: u32 = 80;
const BATCH: usize = 50_000;

struct Case {
    name: &'static str,
    nums: Vec<f64>,
    expect_err: bool,
}

fn cases() -> Vec<Case> {
    vec![
        Case {
            name: "ASCII 'A'..'Z' cycle",
            nums: (0..BATCH).map(|i| 65.0 + (i % 26) as f64).collect(),
            expect_err: false,
        },
        Case {
            name: "C0 / DEL (1..=127)",
            nums: (0..BATCH).map(|i| 1.0 + (i % 127) as f64).collect(),
            expect_err: false,
        },
        Case {
            name: "2-byte UTF-8 (é = 233)",
            nums: vec![233.0; BATCH],
            expect_err: false,
        },
        Case {
            name: "3-byte UTF-8 (中 = 20013)",
            nums: vec![20013.0; BATCH],
            expect_err: false,
        },
        Case {
            name: "4-byte UTF-8 (😀 = 128512)",
            nums: vec![128512.0; BATCH],
            expect_err: false,
        },
        Case {
            name: "U+10FFFF max scalar",
            nums: vec![1_114_111.0; BATCH],
            expect_err: false,
        },
        Case {
            name: "reject 0 (#VALUE!)",
            nums: vec![0.0; BATCH],
            expect_err: true,
        },
        Case {
            name: "reject surrogate D800 (#N/A)",
            nums: vec![0xD800 as f64; BATCH],
            expect_err: true,
        },
        Case {
            name: "reject 1114112 (#VALUE!)",
            nums: vec![1_114_112.0; BATCH],
            expect_err: true,
        },
        Case {
            name: "mixed BMP + emoji + reject",
            nums: (0..BATCH)
                .map(|i| match i % 5 {
                    0 => 65.0,
                    1 => 233.0,
                    2 => 20013.0,
                    3 => 128512.0,
                    _ => 0.0,
                })
                .collect(),
            expect_err: false,
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

fn run_batch(f: fn(f64) -> Result<String, xlsx_types::ExcelError>, nums: &[f64]) {
    for &n in nums {
        let _ = black_box(f(black_box(n)));
    }
}

fn main() {
    println!("UNICHAR kernel bench (UTF-16 collect vs direct UTF-8), {BATCH} / case");
    println!(
        "{:<32} {:>12} {:>12} {:>8}",
        "case", "naive", "optimized", "speedup"
    );
    println!("{}", "-".repeat(68));
    for c in cases() {
        let sample = c.nums[0];
        let naive_v = excel_unichar_naive(sample);
        let fast_v = excel_unichar(sample);
        assert_eq!(naive_v, fast_v, "semantic mismatch on {}", c.name);
        if c.expect_err {
            assert!(naive_v.is_err(), "{} should error", c.name);
        } else if c.name.starts_with("mixed") {
            // mixed includes a 0; first element is ASCII and ok
        } else {
            assert!(naive_v.is_ok(), "{} should encode", c.name);
        }
        let naive = time_it(ITERS, || {
            run_batch(excel_unichar_naive, black_box(&c.nums));
        });
        let fast = time_it(ITERS, || {
            run_batch(excel_unichar, black_box(&c.nums));
        });
        let speedup = naive.as_secs_f64() / fast.as_secs_f64().max(1e-12);
        println!(
            "{:<32} {:>12} {:>12} {:>7.1}x",
            c.name,
            fmt_dur(naive),
            fmt_dur(fast),
            speedup
        );
    }
}
