//! Before/after microbench for Excel `NPER`.
//!
//! Compares the `ln(num/den)/ln(1+r)` baseline (`excel_nper_naive`) with the
//! production kernel (`excel_nper`: `ln1p` + `rate=0` shortcut).
//!
//! ```text
//! cargo bench -p xlsx-engine-core --bench nper
//! ```

use std::hint::black_box;
use std::time::{Duration, Instant};
use xlsx_engine_core::{excel_nper, excel_nper_naive};

const ITERS_HEAVY: u32 = 80;
const ITERS_LIGHT: u32 = 200;

struct Case {
    name: &'static str,
    rate: f64,
    pmt: f64,
    pv: f64,
    fv: f64,
    typ: f64,
    count: u32,
    iters: u32,
}

fn cases() -> Vec<Case> {
    vec![
        Case {
            name: "Microsoft begin-of-period",
            rate: 0.12 / 12.0,
            pmt: -100.0,
            pv: -1000.0,
            fv: 10_000.0,
            typ: 1.0,
            count: 80_000,
            iters: ITERS_LIGHT,
        },
        Case {
            name: "30y mortgage sweep (pv varies)",
            rate: 0.05 / 12.0,
            pmt: -1_100.0,
            pv: 200_000.0,
            fv: 0.0,
            typ: 0.0,
            count: 80_000,
            iters: ITERS_HEAVY,
        },
        Case {
            name: "tiny rate 1e-8",
            rate: 1e-8,
            pmt: -300.0,
            pv: 100_000.0,
            fv: 0.0,
            typ: 0.0,
            count: 80_000,
            iters: ITERS_LIGHT,
        },
        Case {
            name: "zero rate straight-line",
            rate: 0.0,
            pmt: -100.0,
            pv: 1000.0,
            fv: 0.0,
            typ: 0.0,
            count: 80_000,
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

fn run_kernel(f: fn(f64, f64, f64, f64, f64) -> Result<f64, xlsx_types::ExcelError>, c: &Case) {
    for i in 0..c.count {
        let pv = c.pv + f64::from(i);
        let _ = black_box(f(
            black_box(c.rate),
            black_box(c.pmt),
            black_box(pv),
            black_box(c.fv),
            black_box(c.typ),
        ));
    }
}

fn main() {
    println!("NPER kernel bench (naive ln/ln vs ln1p)");
    println!(
        "{:<36} {:>12} {:>12} {:>8}",
        "case", "naive", "optimized", "speedup"
    );
    println!("{}", "-".repeat(72));
    for c in cases() {
        let naive = time_it(c.iters, || run_kernel(excel_nper_naive, &c));
        let fast = time_it(c.iters, || run_kernel(excel_nper, &c));
        let speedup = naive.as_secs_f64() / fast.as_secs_f64().max(1e-12);
        println!(
            "{:<36} {:>12} {:>12} {:>7.1}x",
            c.name,
            fmt_dur(naive),
            fmt_dur(fast),
            speedup
        );
        // Semantic check on the unshifted inputs. Tiny-rate is allowed to
        // differ in ULP; ordinary rates must match at 15 digits.
        let a = excel_nper_naive(c.rate, c.pmt, c.pv, c.fv, c.typ);
        let b = excel_nper(c.rate, c.pmt, c.pv, c.fv, c.typ);
        assert_eq!(a.is_ok(), b.is_ok(), "domain mismatch on {}", c.name);
        if c.rate.abs() >= 1e-6 {
            if let (Ok(x), Ok(y)) = (a, b) {
                let slop = 1e-9 * x.abs().max(y.abs()).max(1.0);
                assert!(
                    (x - y).abs() <= slop,
                    "semantic mismatch on {}: {x} vs {y}",
                    c.name
                );
            }
        }
    }
}
