//! Before/after microbench for Excel `CUMIPMT`.
//!
//! Compares the per-period IPMT + `powf` baseline (`excel_cumipmt_naive`)
//! with the closed-form production kernel (`excel_cumipmt`). Same
//! OpenFormula 6.12.12 identity (sum of IPMT, `fv = 0`).
//!
//! ```text
//! cargo bench -p xlsx-engine-core --bench cumipmt
//! ```

use std::hint::black_box;
use std::time::{Duration, Instant};
use xlsx_engine_core::{excel_cumipmt, excel_cumipmt_naive};

const ITERS_SWEEP: u32 = 4;
const ITERS_LIGHT: u32 = 80;

struct Case {
    name: &'static str,
    rate: f64,
    nper: f64,
    pv: f64,
    start: f64,
    end: f64,
    typ: f64,
    iters: u32,
    /// When set, sweep `pv` across `sweep` principals (like the 80k unit smoke).
    sweep: u32,
}

fn cases() -> Vec<Case> {
    vec![
        Case {
            name: "Microsoft month 1",
            rate: 0.09 / 12.0,
            nper: 360.0,
            pv: 125_000.0,
            start: 1.0,
            end: 1.0,
            typ: 0.0,
            iters: ITERS_LIGHT,
            sweep: 0,
        },
        Case {
            name: "Microsoft year 2",
            rate: 0.09 / 12.0,
            nper: 360.0,
            pv: 125_000.0,
            start: 13.0,
            end: 24.0,
            typ: 0.0,
            iters: ITERS_LIGHT,
            sweep: 0,
        },
        Case {
            name: "30y mortgage full life",
            rate: 0.05 / 12.0,
            nper: 360.0,
            pv: 200_000.0,
            start: 1.0,
            end: 360.0,
            typ: 0.0,
            iters: ITERS_LIGHT,
            sweep: 0,
        },
        Case {
            name: "100y monthly first year",
            rate: 0.05 / 12.0,
            nper: 1_200.0,
            pv: 100_000.0,
            start: 1.0,
            end: 12.0,
            typ: 0.0,
            iters: ITERS_LIGHT,
            sweep: 0,
        },
        Case {
            name: "80k swept principal (y2)",
            rate: 0.05 / 12.0,
            nper: 360.0,
            pv: 200_000.0,
            start: 13.0,
            end: 24.0,
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
                black_box(c.nper),
                black_box(c.pv),
                black_box(c.start),
                black_box(c.end),
                black_box(c.typ),
            )
            .unwrap(),
        );
        return;
    }
    let mut acc = 0.0f64;
    for i in 0..c.sweep {
        acc += f(c.rate, c.nper, c.pv + f64::from(i), c.start, c.end, c.typ).unwrap();
    }
    black_box(acc);
}

fn main() {
    println!("CUMIPMT kernel bench (naive IPMT-sum/powf vs closed-form pow_term)");
    println!(
        "{:<28} {:>12} {:>12} {:>8}",
        "case", "naive", "optimized", "speedup"
    );
    println!("{}", "-".repeat(64));
    for c in cases() {
        let naive = time_it(c.iters, || eval_one(excel_cumipmt_naive, &c));
        let fast = time_it(c.iters, || eval_one(excel_cumipmt, &c));
        let speedup = naive.as_secs_f64() / fast.as_secs_f64().max(1e-12);
        println!(
            "{:<28} {:>12} {:>12} {:>7.1}x",
            c.name,
            fmt_dur(naive),
            fmt_dur(fast),
            speedup
        );
        if c.sweep == 0 {
            let a = excel_cumipmt_naive(c.rate, c.nper, c.pv, c.start, c.end, c.typ).unwrap();
            let b = excel_cumipmt(c.rate, c.nper, c.pv, c.start, c.end, c.typ).unwrap();
            let scale = a.abs().max(b.abs()).max(1.0);
            assert!(
                (a - b).abs() / scale < 1e-9,
                "semantic mismatch on {}: {a} vs {b}",
                c.name
            );
        }
    }
}
