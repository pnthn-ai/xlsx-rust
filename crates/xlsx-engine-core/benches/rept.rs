//! Before/after microbench for Excel `REPT`.
//!
//! Compares the unreserved `push_str` loop (`excel_rept_naive`) with the
//! production kernel (`excel_rept`: UTF-16 overflow check, then `str::repeat`
//! / single-byte ASCII fill). Workloads stay at or under Excel’s 32,767
//! UTF-16-unit result cap so the bench measures a completed repeat, except
//! the dedicated overflow-reject case.
//!
//! ```text
//! cargo bench -p xlsx-engine-core --bench rept
//! ```
//!
//! A faster REPT that fails `xlsx-verify --candidate calc-core` is not a win.

use std::hint::black_box;
use std::time::{Duration, Instant};
use xlsx_engine_core::{excel_rept, excel_rept_naive};

const ITERS_HEAVY: u32 = 40;
const ITERS_LIGHT: u32 = 80;

struct Case {
    name: &'static str,
    text: String,
    times: u64,
    iters: u32,
    expect_err: bool,
}

fn cases() -> Vec<Case> {
    vec![
        Case {
            name: "1-byte ASCII × 32767 (cap)",
            text: "x".into(),
            times: 32767,
            iters: ITERS_HEAVY,
            expect_err: false,
        },
        Case {
            name: "1-byte ASCII × 8000",
            text: "-".into(),
            times: 8000,
            iters: ITERS_HEAVY,
            expect_err: false,
        },
        Case {
            name: "8-byte ASCII × 4000",
            text: "abcdefgh".into(),
            times: 4000,
            iters: ITERS_HEAVY,
            expect_err: false,
        },
        Case {
            name: "100-byte ASCII × 300",
            text: "0123456789".repeat(10),
            times: 300,
            iters: ITERS_HEAVY,
            expect_err: false,
        },
        Case {
            name: "empty × 1e6 (no alloc)",
            text: String::new(),
            times: 1_000_000,
            iters: ITERS_LIGHT,
            expect_err: false,
        },
        Case {
            name: "ASCII overflow reject 1e9",
            text: "a".into(),
            times: 1_000_000_000,
            iters: ITERS_LIGHT,
            expect_err: true,
        },
        Case {
            name: "'é' × 10000 (2-byte UTF-8)",
            text: "é".into(),
            times: 10_000,
            iters: ITERS_HEAVY,
            expect_err: false,
        },
        Case {
            name: "emoji × 16383 (UTF-16 cap-1)",
            text: "😀".into(),
            times: 16383,
            iters: ITERS_HEAVY,
            expect_err: false,
        },
        Case {
            name: "emoji overflow 16384",
            text: "😀".into(),
            times: 16384,
            iters: ITERS_LIGHT,
            expect_err: true,
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
    println!("REPT kernel bench (unreserved push_str vs repeat/fill)");
    println!(
        "{:<36} {:>12} {:>12} {:>8}",
        "case", "naive", "optimized", "speedup"
    );
    println!("{}", "-".repeat(72));
    for c in cases() {
        let naive_v = excel_rept_naive(&c.text, c.times);
        let fast_v = excel_rept(&c.text, c.times);
        assert_eq!(naive_v, fast_v, "semantic mismatch on {}", c.name);
        if c.expect_err {
            assert!(naive_v.is_err(), "{} should be #VALUE!", c.name);
        } else {
            assert!(naive_v.is_ok(), "{} should complete under the cap", c.name);
        }
        let naive = time_it(c.iters, || {
            let _ = black_box(excel_rept_naive(black_box(&c.text), black_box(c.times)));
        });
        let fast = time_it(c.iters, || {
            let _ = black_box(excel_rept(black_box(&c.text), black_box(c.times)));
        });
        let speedup = naive.as_secs_f64() / fast.as_secs_f64().max(1e-12);
        println!(
            "{:<36} {:>12} {:>12} {:>7.1}x",
            c.name,
            fmt_dur(naive),
            fmt_dur(fast),
            speedup
        );
    }
}
