//! Before/after microbench for Excel `FV`.
//!
//! Compares the `powf` baseline (`excel_fv_naive`) with the production
//! kernel (`excel_fv`: `rate=0` skip, `powi` / `expm1` via `pow_term`).
//!
//! ```text
//! cargo bench -p xlsx-engine-core --bench fv
//! ```

use std::hint::black_box;
use std::time::{Duration, Instant};
use xlsx_types::{excel_fv, excel_fv_naive};

const ITERS_HEAVY: u32 = 80;
const ITERS_LIGHT: u32 = 400;

struct Case {
    name: &'static str,
    rate: f64,
    nper: f64,
    pmt: f64,
    pv: f64,
    typ: f64,
    sweep: u32,
    iters: u32,
}

fn cases() -> Vec<Case> {
    vec![
        Case {
            name: "Microsoft 10-mo begin",
            rate: 0.06 / 12.0,
            nper: 10.0,
            pmt: -200.0,
            pv: -500.0,
            typ: 1.0,
            sweep: 1,
            iters: 4_000,
        },
        Case {
            name: "30y mortgage, type 0",
            rate: 0.05 / 12.0,
            nper: 360.0,
            pmt: -1_073.64,
            pv: 200_000.0,
            typ: 0.0,
            sweep: 1,
            iters: ITERS_LIGHT,
        },
        Case {
            name: "80k swept principal",
            rate: 0.05 / 12.0,
            nper: 360.0,
            pmt: -1_000.0,
            pv: -200_000.0,
            typ: 0.0,
            sweep: 80_000,
            iters: ITERS_HEAVY,
        },
        Case {
            name: "zero rate (no pow)",
            rate: 0.0,
            nper: 360.0,
            pmt: -100.0,
            pv: -1_000.0,
            typ: 0.0,
            sweep: 1,
            iters: 8_000,
        },
        Case {
            name: "100y monthly annuity",
            rate: 0.05 / 12.0,
            nper: 1_200.0,
            pmt: -100.0,
            pv: 0.0,
            typ: 0.0,
            sweep: 1,
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

fn eval_sweep(f: fn(f64, f64, f64, f64, f64) -> Result<f64, xlsx_types::ExcelError>, c: &Case) {
    for i in 0..c.sweep {
        black_box(
            f(
                black_box(c.rate),
                black_box(c.nper),
                black_box(c.pmt),
                black_box(c.pv + f64::from(i)),
                black_box(c.typ),
            )
            .unwrap(),
        );
    }
}

fn main() {
    println!("FV kernel bench (naive powf vs pow_term)");
    println!(
        "{:<32} {:>12} {:>12} {:>8}",
        "case", "naive", "optimized", "speedup"
    );
    println!("{}", "-".repeat(68));
    for c in cases() {
        let naive = time_it(c.iters, || eval_sweep(excel_fv_naive, &c));
        let fast = time_it(c.iters, || eval_sweep(excel_fv, &c));
        let speedup = naive.as_secs_f64() / fast.as_secs_f64().max(1e-12);
        println!(
            "{:<32} {:>12} {:>12} {:>7.1}x",
            c.name,
            fmt_dur(naive),
            fmt_dur(fast),
            speedup
        );
        let a = excel_fv_naive(c.rate, c.nper, c.pmt, c.pv, c.typ).unwrap();
        let b = excel_fv(c.rate, c.nper, c.pmt, c.pv, c.typ).unwrap();
        let scale = a.abs().max(b.abs()).max(1.0);
        assert!(
            (a - b).abs() / scale < 1e-9,
            "semantic mismatch on {}: {a} vs {b}",
            c.name
        );
    }
}
