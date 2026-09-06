//! Before/after microbench for Excel `PDURATION`.
//!
//! Compares the textbook two-`ln` baseline (`excel_pduration_naive`) with the
//! production kernel (`excel_pduration`: `pv==fv` / one-period identities,
//! `ln1p` on the rate and on a near-unity `fv/pv`).
//!
//! ```text
//! cargo bench -p xlsx-engine-core --bench pduration
//! ```

use std::hint::black_box;
use std::time::{Duration, Instant};
use xlsx_engine_core::{excel_pduration, excel_pduration_naive};

const ITERS_LIGHT: u32 = 80;
const ITERS_HEAVY: u32 = 20;
const N: usize = 50_000;

struct Case {
    name: &'static str,
    rates: Vec<f64>,
    pvs: Vec<f64>,
    fvs: Vec<f64>,
    iters: u32,
}

fn cases() -> Vec<Case> {
    let rates: Vec<f64> = (0..N).map(|i| 0.01 + (i as f64) * 1e-6).collect();
    let pvs: Vec<f64> = (0..N).map(|i| 1_000.0 + (i as f64)).collect();
    let fvs: Vec<f64> = pvs.iter().map(|&p| p * 1.2).collect();
    let near: Vec<f64> = pvs.iter().map(|&p| p + 1.0).collect();
    let same = pvs.clone();
    let one_period: Vec<f64> = rates
        .iter()
        .zip(pvs.iter())
        .map(|(&r, &p)| p * (1.0 + r))
        .collect();
    let tiny_rates: Vec<f64> = (0..N).map(|i| 1e-12 * (1 + i % 97) as f64).collect();
    vec![
        Case {
            name: "50k growth × 20% (general)",
            rates: rates.clone(),
            pvs: pvs.clone(),
            fvs: fvs.clone(),
            iters: ITERS_LIGHT,
        },
        Case {
            name: "50k pv==fv (identity 0)",
            rates: rates.clone(),
            pvs: pvs.clone(),
            fvs: same,
            iters: ITERS_LIGHT,
        },
        Case {
            name: "50k one-period identity",
            rates: rates.clone(),
            pvs: pvs.clone(),
            fvs: one_period,
            iters: ITERS_LIGHT,
        },
        Case {
            name: "50k near-equal fv (ln1p num)",
            rates: rates.clone(),
            pvs: pvs.clone(),
            fvs: near,
            iters: ITERS_LIGHT,
        },
        Case {
            name: "50k tiny rates (ln1p den)",
            rates: tiny_rates,
            pvs: pvs.clone(),
            fvs,
            iters: ITERS_HEAVY,
        },
        Case {
            name: "Microsoft PDURATION(2.5%, 2000, 2200)",
            rates: vec![0.025; 10_000],
            pvs: vec![2000.0; 10_000],
            fvs: vec![2200.0; 10_000],
            iters: 400,
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

fn fold(
    f: fn(f64, f64, f64) -> Result<f64, xlsx_types::ExcelError>,
    rates: &[f64],
    pvs: &[f64],
    fvs: &[f64],
) -> f64 {
    let mut acc = 0.0;
    for i in 0..rates.len() {
        if let Ok(v) = f(rates[i], pvs[i], fvs[i]) {
            acc += v;
        }
    }
    acc
}

fn main() {
    println!("PDURATION kernel bench (two-ln baseline vs ln1p / identities)");
    println!(
        "{:<42} {:>12} {:>12} {:>8}",
        "case", "naive", "optimized", "speedup"
    );
    println!("{}", "-".repeat(78));
    for c in cases() {
        let naive = time_it(c.iters, || {
            black_box(fold(
                excel_pduration_naive,
                black_box(&c.rates),
                black_box(&c.pvs),
                black_box(&c.fvs),
            ));
        });
        let fast = time_it(c.iters, || {
            black_box(fold(
                excel_pduration,
                black_box(&c.rates),
                black_box(&c.pvs),
                black_box(&c.fvs),
            ));
        });
        let speedup = naive.as_secs_f64() / fast.as_secs_f64().max(1e-12);
        println!(
            "{:<42} {:>12} {:>12} {:>7.1}x",
            c.name,
            fmt_dur(naive),
            fmt_dur(fast),
            speedup
        );
        let step = (c.rates.len() / 16).max(1);
        for i in (0..c.rates.len()).step_by(step) {
            let a = excel_pduration_naive(c.rates[i], c.pvs[i], c.fvs[i]);
            let b = excel_pduration(c.rates[i], c.pvs[i], c.fvs[i]);
            match (a, b) {
                (Ok(a), Ok(b)) => {
                    let scale = a.abs().max(b.abs()).max(1e-18);
                    if c.rates[i] >= 1e-8 {
                        assert!(
                            (a - b).abs() / scale < 1e-9,
                            "semantic mismatch r={} pv={} fv={}: {a} vs {b}",
                            c.rates[i],
                            c.pvs[i],
                            c.fvs[i]
                        );
                    }
                }
                (Err(ea), Err(eb)) => assert_eq!(ea, eb, "error mismatch i={i}"),
                // Tiny rates: naive ln(1+ε) cancels; optimized keeps the term.
                (Err(_), Ok(_)) if c.rates[i] < 1e-8 => {}
                other => panic!(
                    "domain mismatch r={} pv={} fv={}: {other:?}",
                    c.rates[i], c.pvs[i], c.fvs[i]
                ),
            }
        }
    }
}
