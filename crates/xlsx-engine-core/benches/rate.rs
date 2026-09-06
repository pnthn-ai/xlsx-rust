//! Before/after microbench for Excel `RATE`.
//!
//! Compares the `powf` baseline (`excel_rate_naive`) with the production
//! kernel (`excel_rate`: closed forms, integer `powi`, tiny-rate `expm1`).
//! Same Newton / secant rules (20 tries, `1e-7`, `r <= -1` → `#NUM!`).
//!
//! ```text
//! cargo bench -p xlsx-engine-core --bench rate
//! ```

use std::hint::black_box;
use std::time::{Duration, Instant};
use xlsx_types::{excel_pmt, excel_rate, excel_rate_naive};

const ITERS_HEAVY: u32 = 8;
const ITERS_LIGHT: u32 = 40;

struct Case {
    name: &'static str,
    nper: f64,
    pmt: f64,
    pv: f64,
    fv: f64,
    typ: f64,
    guess: f64,
    reps: u32,
    iters: u32,
}

fn cases() -> Vec<Case> {
    let mortgage_pmt = excel_pmt(0.05 / 12.0, 360.0, 200_000.0, 0.0, 0.0).unwrap();
    vec![
        Case {
            name: "MS loan 4y monthly (guess 0.1)",
            nper: 48.0,
            pmt: -200.0,
            pv: 8_000.0,
            fv: 0.0,
            typ: 0.0,
            guess: 0.1,
            reps: 40_000,
            iters: ITERS_LIGHT,
        },
        Case {
            name: "10-period invert (guess 0.1)",
            nper: 10.0,
            pmt: excel_pmt(0.1, 10.0, 1_000.0, 0.0, 0.0).unwrap(),
            pv: 1_000.0,
            fv: 0.0,
            typ: 0.0,
            guess: 0.1,
            reps: 40_000,
            iters: ITERS_LIGHT,
        },
        Case {
            name: "30y mortgage (guess 0.01)",
            nper: 360.0,
            pmt: mortgage_pmt,
            pv: 200_000.0,
            fv: 0.0,
            typ: 0.0,
            guess: 0.01,
            reps: 20_000,
            iters: ITERS_HEAVY,
        },
        Case {
            name: "nper=1 closed form",
            nper: 1.0,
            pmt: -110.0,
            pv: 100.0,
            fv: 0.0,
            typ: 0.0,
            guess: 0.1,
            reps: 80_000,
            iters: ITERS_LIGHT,
        },
        Case {
            name: "pmt=0 compound closed form",
            nper: 10.0,
            pmt: 0.0,
            pv: -1_000.0,
            fv: 2_000.0,
            typ: 0.0,
            guess: 0.1,
            reps: 80_000,
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
    println!("RATE kernel bench (naive powf vs pow_term Newton)");
    println!(
        "{:<40} {:>12} {:>12} {:>8}",
        "case", "naive", "optimized", "speedup"
    );
    println!("{}", "-".repeat(76));
    for c in cases() {
        let naive = time_it(c.iters, || {
            let mut acc = 0.0;
            for i in 0..c.reps {
                acc += excel_rate_naive(
                    black_box(c.nper),
                    black_box(c.pmt),
                    black_box(c.pv + f64::from(i)),
                    black_box(c.fv),
                    black_box(c.typ),
                    black_box(c.guess),
                )
                .unwrap_or(0.0);
            }
            black_box(acc);
        });
        let fast = time_it(c.iters, || {
            let mut acc = 0.0;
            for i in 0..c.reps {
                acc += excel_rate(
                    black_box(c.nper),
                    black_box(c.pmt),
                    black_box(c.pv + f64::from(i)),
                    black_box(c.fv),
                    black_box(c.typ),
                    black_box(c.guess),
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
        let a = excel_rate_naive(c.nper, c.pmt, c.pv, c.fv, c.typ, c.guess);
        let b = excel_rate(c.nper, c.pmt, c.pv, c.fv, c.typ, c.guess);
        match (a, b) {
            (Ok(a), Ok(b)) => {
                let scale = a.abs().max(b.abs()).max(1e-12);
                assert!(
                    (a - b).abs() / scale < 1e-9,
                    "semantic mismatch on {}: {a} vs {b}",
                    c.name
                );
            }
            (Err(ea), Err(eb)) => assert_eq!(ea, eb, "error mismatch on {}", c.name),
            other => panic!("result kind mismatch on {}: {other:?}", c.name),
        }
    }
}
