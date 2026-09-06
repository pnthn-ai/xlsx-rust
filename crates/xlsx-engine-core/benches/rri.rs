//! Before/after microbench for Excel `RRI`.
//!
//! Compares the `powf` baseline (`excel_rri_naive`) with the production
//! `expm1(ln1p)` kernel (`excel_rri`). Same domain errors.
//!
//! ```text
//! cargo bench -p xlsx-engine-core --bench rri
//! ```

use std::hint::black_box;
use std::time::{Duration, Instant};
use xlsx_engine_core::{excel_rri, excel_rri_naive};

const ITERS_HEAVY: u32 = 8;
const ITERS_LIGHT: u32 = 40;

struct Case {
    name: &'static str,
    nper: f64,
    pv: f64,
    fv: f64,
    count: u32,
    iters: u32,
}

fn cases() -> Vec<Case> {
    vec![
        Case {
            name: "Microsoft 96-month",
            nper: 96.0,
            pv: 10_000.0,
            fv: 11_000.0,
            count: 80_000,
            iters: ITERS_LIGHT,
        },
        Case {
            name: "tiny growth (fv≈pv)",
            nper: 360.0,
            pv: 100_000.0,
            fv: 100_001.0,
            count: 80_000,
            iters: ITERS_LIGHT,
        },
        Case {
            name: "nper=1 simple return",
            nper: 1.0,
            pv: 100.0,
            fv: 110.0,
            count: 200_000,
            iters: ITERS_LIGHT,
        },
        Case {
            name: "sweep 80k future values",
            nper: 96.0,
            pv: 10_000.0,
            fv: 11_000.0,
            count: 80_000,
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
    println!("RRI kernel bench (naive powf vs expm1/ln1p)");
    println!(
        "{:<32} {:>12} {:>12} {:>8}",
        "case", "naive", "optimized", "speedup"
    );
    println!("{}", "-".repeat(68));
    for c in cases() {
        let naive = time_it(c.iters, || {
            let mut acc = 0.0;
            for i in 0..c.count {
                let fv = c.fv + f64::from(i);
                acc += excel_rri_naive(black_box(c.nper), black_box(c.pv), black_box(fv)).unwrap();
            }
            black_box(acc);
        });
        let fast = time_it(c.iters, || {
            let mut acc = 0.0;
            for i in 0..c.count {
                let fv = c.fv + f64::from(i);
                acc += excel_rri(black_box(c.nper), black_box(c.pv), black_box(fv)).unwrap();
            }
            black_box(acc);
        });
        let speedup = naive.as_secs_f64() / fast.as_secs_f64().max(1e-12);
        println!(
            "{:<32} {:>12} {:>12} {:>7.1}x",
            c.name,
            fmt_dur(naive),
            fmt_dur(fast),
            speedup
        );
        let a = excel_rri_naive(c.nper, c.pv, c.fv).unwrap();
        let b = excel_rri(c.nper, c.pv, c.fv).unwrap();
        let scale = a.abs().max(b.abs()).max(1.0);
        assert!(
            (a - b).abs() / scale < 1e-9,
            "semantic mismatch on {}: {a} vs {b}",
            c.name
        );
    }
}
