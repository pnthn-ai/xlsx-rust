//! Before/after microbench for Excel `IPMT`.
//!
//! Compares the `powf` remaining-balance baseline (`excel_ipmt_naive`) with
//! the `pow_term` production kernel (`excel_ipmt`). Same OpenFormula 6.12.28
//! identity (PMT + FV·rate, type=1 first period is 0).
//!
//! ```text
//! cargo bench -p xlsx-engine-core --bench ipmt
//! ```

use std::hint::black_box;
use std::time::{Duration, Instant};
use xlsx_engine_core::{excel_ipmt, excel_ipmt_naive};

const ITERS_SWEEP: u32 = 4;
const ITERS_LIGHT: u32 = 80;

struct Case {
    name: &'static str,
    rate: f64,
    per: f64,
    nper: f64,
    pv: f64,
    fv: f64,
    typ: f64,
    iters: u32,
    /// When set, sweep `pv` across `sweep` principals (like the 80k unit smoke).
    sweep: u32,
}

fn cases() -> Vec<Case> {
    vec![
        Case {
            name: "Microsoft month 1",
            rate: 0.10 / 12.0,
            per: 1.0,
            nper: 36.0,
            pv: 8_000.0,
            fv: 0.0,
            typ: 0.0,
            iters: ITERS_LIGHT,
            sweep: 0,
        },
        Case {
            name: "Microsoft year 3",
            rate: 0.10,
            per: 3.0,
            nper: 3.0,
            pv: 8_000.0,
            fv: 0.0,
            typ: 0.0,
            iters: ITERS_LIGHT,
            sweep: 0,
        },
        Case {
            name: "30y mortgage mid-life",
            rate: 0.05 / 12.0,
            per: 180.0,
            nper: 360.0,
            pv: 200_000.0,
            fv: 0.0,
            typ: 0.0,
            iters: ITERS_LIGHT,
            sweep: 0,
        },
        Case {
            name: "100y monthly horizon",
            rate: 0.05 / 12.0,
            per: 600.0,
            nper: 1_200.0,
            pv: 100_000.0,
            fv: 0.0,
            typ: 0.0,
            iters: ITERS_LIGHT,
            sweep: 0,
        },
        Case {
            name: "80k swept principal",
            rate: 0.05 / 12.0,
            per: 12.0,
            nper: 360.0,
            pv: 200_000.0,
            fv: 0.0,
            typ: 0.0,
            iters: ITERS_SWEEP,
            sweep: 80_000,
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

fn eval_one(f: fn(f64, f64, f64, f64, f64, f64) -> Result<f64, xlsx_types::ExcelError>, c: &Case) {
    if c.sweep == 0 {
        black_box(
            f(
                black_box(c.rate),
                black_box(c.per),
                black_box(c.nper),
                black_box(c.pv),
                black_box(c.fv),
                black_box(c.typ),
            )
            .unwrap(),
        );
        return;
    }
    let mut acc = 0.0f64;
    for i in 0..c.sweep {
        acc += f(c.rate, c.per, c.nper, c.pv + f64::from(i), c.fv, c.typ).unwrap();
    }
    black_box(acc);
}

fn main() {
    println!("IPMT kernel bench (naive powf vs pow_term)");
    println!(
        "{:<28} {:>12} {:>12} {:>8}",
        "case", "naive", "optimized", "speedup"
    );
    println!("{}", "-".repeat(64));
    for c in cases() {
        let naive = time_it(c.iters, || eval_one(excel_ipmt_naive, &c));
        let fast = time_it(c.iters, || eval_one(excel_ipmt, &c));
        let speedup = naive.as_secs_f64() / fast.as_secs_f64().max(1e-12);
        println!(
            "{:<28} {:>12} {:>12} {:>7.1}x",
            c.name,
            fmt_dur(naive),
            fmt_dur(fast),
            speedup
        );
        if c.sweep == 0 {
            let a = excel_ipmt_naive(c.rate, c.per, c.nper, c.pv, c.fv, c.typ).unwrap();
            let b = excel_ipmt(c.rate, c.per, c.nper, c.pv, c.fv, c.typ).unwrap();
            let scale = a.abs().max(b.abs()).max(1.0);
            assert!(
                (a - b).abs() / scale < 1e-9,
                "semantic mismatch on {}: {a} vs {b}",
                c.name
            );
        }
    }
}
