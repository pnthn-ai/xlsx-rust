//! Before/after microbench for Excel `PV`.
//!
//! Compares the `powf` baseline (`excel_pv_naive`) with the production
//! kernel (`excel_pv`: `rate=0` divide, integer `powi`, tiny-rate `expm1`).
//!
//! ```text
//! cargo bench -p xlsx-engine-core --bench pv
//! ```

use std::hint::black_box;
use std::time::{Duration, Instant};
use xlsx_types::{excel_pv, excel_pv_naive};

const ITERS_HEAVY: u32 = 8;
const ITERS_LIGHT: u32 = 40;

struct Case {
    name: &'static str,
    rate: f64,
    nper: f64,
    pmt: f64,
    fv: f64,
    typ: f64,
    reps: u32,
    iters: u32,
}

fn cases() -> Vec<Case> {
    vec![
        Case {
            name: "MS annuity 8%/12 × 240",
            rate: 0.08 / 12.0,
            nper: 240.0,
            pmt: 500.0,
            fv: 0.0,
            typ: 0.0,
            reps: 80_000,
            iters: ITERS_LIGHT,
        },
        Case {
            name: "30y mortgage inverse, type 1",
            rate: 0.05 / 12.0,
            nper: 360.0,
            pmt: -1073.6432460242763,
            fv: 0.0,
            typ: 1.0,
            reps: 80_000,
            iters: ITERS_LIGHT,
        },
        Case {
            name: "tiny rate 1e-8 × 360 (expm1)",
            rate: 1e-8,
            nper: 360.0,
            pmt: 100.0,
            fv: 0.0,
            typ: 0.0,
            reps: 80_000,
            iters: ITERS_LIGHT,
        },
        Case {
            name: "rate=0 straight-line",
            rate: 0.0,
            nper: 360.0,
            pmt: 100.0,
            fv: 0.0,
            typ: 0.0,
            reps: 80_000,
            iters: ITERS_HEAVY,
        },
        Case {
            name: "100-year monthly horizon",
            rate: 0.05 / 12.0,
            nper: 1200.0,
            pmt: 500.0,
            fv: 0.0,
            typ: 0.0,
            reps: 40_000,
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
    println!("PV kernel bench (naive powf vs pow_term)");
    println!(
        "{:<40} {:>12} {:>12} {:>8}",
        "case", "naive", "optimized", "speedup"
    );
    println!("{}", "-".repeat(76));
    for c in cases() {
        let naive = time_it(c.iters, || {
            let mut acc = 0.0;
            for i in 0..c.reps {
                acc += excel_pv_naive(
                    black_box(c.rate),
                    black_box(c.nper),
                    black_box(c.pmt + f64::from(i)),
                    black_box(c.fv),
                    black_box(c.typ),
                )
                .unwrap_or(0.0);
            }
            black_box(acc);
        });
        let fast = time_it(c.iters, || {
            let mut acc = 0.0;
            for i in 0..c.reps {
                acc += excel_pv(
                    black_box(c.rate),
                    black_box(c.nper),
                    black_box(c.pmt + f64::from(i)),
                    black_box(c.fv),
                    black_box(c.typ),
                )
                .unwrap_or(0.0);
            }
            black_box(acc);
        });
        let speedup = naive.as_secs_f64() / fast.as_secs_f64().max(1e-12);
        println!(
            "{:<40} {:>12} {:>12} {:>7.1}x",
            c.name,
            fmt_dur(naive),
            fmt_dur(fast),
            speedup
        );
        let a = excel_pv_naive(c.rate, c.nper, c.pmt, c.fv, c.typ);
        let b = excel_pv(c.rate, c.nper, c.pmt, c.fv, c.typ);
        match (a, b) {
            (Ok(a), Ok(b)) => {
                let scale = a.abs().max(b.abs()).max(1.0);
                assert!(
                    (a - b).abs() / scale < 1e-9 || c.rate.abs() < 1e-6,
                    "semantic mismatch on {}: {a} vs {b}",
                    c.name
                );
            }
            (Err(ea), Err(eb)) => assert_eq!(ea, eb, "error mismatch on {}", c.name),
            other => panic!("result kind mismatch on {}: {other:?}", c.name),
        }
    }
}
