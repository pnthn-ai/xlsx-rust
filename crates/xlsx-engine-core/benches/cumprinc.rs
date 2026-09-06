//! Before/after microbench for Excel `CUMPRINC`.
//!
//! Compares the period-loop baseline (`excel_cumprinc_naive`) with the
//! closed-form production kernel (`excel_cumprinc`). Same domain rules
//! (Σ PPMT via `PMT` + `FV`).
//!
//! ```text
//! cargo bench -p xlsx-engine-core --bench cumprinc
//! ```

use std::hint::black_box;
use std::time::{Duration, Instant};
use xlsx_types::{excel_cumprinc, excel_cumprinc_naive};

const ITERS_HEAVY: u32 = 8;
const ITERS_LIGHT: u32 = 40;

struct Case {
    name: &'static str,
    rate: f64,
    nper: f64,
    pv: f64,
    start: f64,
    end: f64,
    typ: f64,
    iters: u32,
}

fn cases() -> Vec<Case> {
    vec![
        Case {
            name: "Microsoft year 2 (13–24, type 0)",
            rate: 0.09 / 12.0,
            nper: 360.0,
            pv: 125_000.0,
            start: 13.0,
            end: 24.0,
            typ: 0.0,
            iters: ITERS_LIGHT,
        },
        Case {
            name: "30-year full term (1–360, type 0)",
            rate: 0.05 / 12.0,
            nper: 360.0,
            pv: 200_000.0,
            start: 1.0,
            end: 360.0,
            typ: 0.0,
            iters: ITERS_LIGHT,
        },
        Case {
            name: "30-year full term (1–360, type 1)",
            rate: 0.05 / 12.0,
            nper: 360.0,
            pv: 200_000.0,
            start: 1.0,
            end: 360.0,
            typ: 1.0,
            iters: ITERS_LIGHT,
        },
        Case {
            name: "100-year monthly (1–1200, type 0)",
            rate: 0.05 / 12.0,
            nper: 1200.0,
            pv: 100_000.0,
            start: 1.0,
            end: 1200.0,
            typ: 0.0,
            iters: ITERS_HEAVY,
        },
        Case {
            name: "100-year monthly (1–1200, type 1)",
            rate: 0.05 / 12.0,
            nper: 1200.0,
            pv: 100_000.0,
            start: 1.0,
            end: 1200.0,
            typ: 1.0,
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
    println!("CUMPRINC kernel bench (naive period loop vs closed form)");
    println!(
        "{:<40} {:>12} {:>12} {:>8}",
        "case", "naive", "optimized", "speedup"
    );
    println!("{}", "-".repeat(76));
    for c in cases() {
        let naive = time_it(c.iters, || {
            black_box(
                excel_cumprinc_naive(
                    black_box(c.rate),
                    black_box(c.nper),
                    black_box(c.pv),
                    black_box(c.start),
                    black_box(c.end),
                    black_box(c.typ),
                )
                .unwrap(),
            );
        });
        let fast = time_it(c.iters, || {
            black_box(
                excel_cumprinc(
                    black_box(c.rate),
                    black_box(c.nper),
                    black_box(c.pv),
                    black_box(c.start),
                    black_box(c.end),
                    black_box(c.typ),
                )
                .unwrap(),
            );
        });
        let speedup = naive.as_secs_f64() / fast.as_secs_f64().max(1e-12);
        println!(
            "{:<40} {:>12} {:>12} {:>7.1}x",
            c.name,
            fmt_dur(naive),
            fmt_dur(fast),
            speedup
        );
        let a = excel_cumprinc_naive(c.rate, c.nper, c.pv, c.start, c.end, c.typ).unwrap();
        let b = excel_cumprinc(c.rate, c.nper, c.pv, c.start, c.end, c.typ).unwrap();
        let scale = a.abs().max(b.abs()).max(1.0);
        assert!(
            (a - b).abs() / scale < 1e-9,
            "semantic mismatch on {}: {a} vs {b}",
            c.name
        );
    }
}
