//! Before/after microbench for Excel `PPMT`.
//!
//! Compares the `powf` baseline (`excel_ppmt_naive`) with the production
//! kernel (`excel_ppmt`, via shared `pow_term`). An amortization walk on
//! late periods shows the closed form’s O(1) vs O(per) win.
//!
//! Tiny rates are timed but not required to match at 1e-9 — `powf`
//! cancels; `expm1`/`ln1p` does not.
//!
//! ```text
//! cargo bench -p xlsx-engine-core --bench ppmt
//! ```

use std::hint::black_box;
use std::time::{Duration, Instant};
use xlsx_engine_core::{excel_ppmt, excel_ppmt_naive};
use xlsx_types::excel_pmt;

const SWEEP: u32 = 80_000;

struct Case {
    name: &'static str,
    rate: f64,
    per: f64,
    nper: f64,
    pv: f64,
    fv: f64,
    typ: f64,
    walk: bool,
    /// Ordinary rates must match `powf` to 1e-12 relative; tiny rates skip that.
    match_powf: bool,
}

fn cases() -> Vec<Case> {
    vec![
        Case {
            name: "Microsoft 2y loan, month 1",
            rate: 0.10 / 12.0,
            per: 1.0,
            nper: 24.0,
            pv: 2_000.0,
            fv: 0.0,
            typ: 0.0,
            walk: false,
            match_powf: true,
        },
        Case {
            name: "Microsoft 10y annual, year 10",
            rate: 0.08,
            per: 10.0,
            nper: 10.0,
            pv: 200_000.0,
            fv: 0.0,
            typ: 0.0,
            walk: false,
            match_powf: true,
        },
        Case {
            name: "30y mortgage, month 180",
            rate: 0.05 / 12.0,
            per: 180.0,
            nper: 360.0,
            pv: 200_000.0,
            fv: 0.0,
            typ: 0.0,
            walk: true,
            match_powf: true,
        },
        Case {
            name: "100y monthly, month 600",
            rate: 0.05 / 12.0,
            per: 600.0,
            nper: 1200.0,
            pv: 100_000.0,
            fv: 0.0,
            typ: 0.0,
            walk: true,
            match_powf: true,
        },
        Case {
            name: "tiny rate 1e-8, period 1",
            rate: 1e-8,
            per: 1.0,
            nper: 360.0,
            pv: 100_000.0,
            fv: 0.0,
            typ: 0.0,
            walk: false,
            match_powf: false,
        },
    ]
}

/// Period-by-period remaining-balance walk (integer `per` only).
fn ppmt_amortize(rate: f64, per: f64, nper: f64, pv: f64, fv: f64, typ: f64) -> f64 {
    let payment = excel_pmt(rate, nper, pv, fv, typ).unwrap();
    let last = per as i32;
    let mut bal = pv;
    for k in 1..=last {
        let interest = if typ != 0.0 && k == 1 {
            0.0
        } else {
            -bal * rate
        };
        let principal = payment - interest;
        if k == last {
            return principal;
        }
        bal += principal;
    }
    payment
}

fn sweep(
    f: fn(f64, f64, f64, f64, f64, f64) -> Result<f64, xlsx_types::ExcelError>,
    c: &Case,
) {
    for i in 0..SWEEP {
        black_box(
            f(
                black_box(c.rate),
                black_box(c.per),
                black_box(c.nper),
                black_box(c.pv + f64::from(i)),
                black_box(c.fv),
                black_box(c.typ),
            )
            .unwrap(),
        );
    }
}

fn sweep_amortize(c: &Case) {
    for i in 0..SWEEP {
        black_box(ppmt_amortize(
            black_box(c.rate),
            black_box(c.per),
            black_box(c.nper),
            black_box(c.pv + f64::from(i)),
            black_box(c.fv),
            black_box(c.typ),
        ));
    }
}

fn time_it(mut f: impl FnMut()) -> Duration {
    f();
    let start = Instant::now();
    f();
    start.elapsed()
}

fn fmt_dur(d: Duration) -> String {
    let ns = d.as_secs_f64() * 1e9;
    if ns >= 1e6 {
        format!("{:.2} ms", ns / 1e6)
    } else if ns >= 1000.0 {
        format!("{:.1} µs", ns / 1000.0)
    } else {
        format!("{ns:.0} ns")
    }
}

fn main() {
    println!("PPMT kernel bench ({SWEEP} swept principals; powf vs pow_term; walk vs closed form)");
    println!(
        "{:<36} {:>12} {:>12} {:>8} {:>12} {:>8}",
        "case", "naive", "optimized", "vs powf", "amortize", "vs walk"
    );
    println!("{}", "-".repeat(96));
    for c in cases() {
        let naive = time_it(|| sweep(excel_ppmt_naive, &c));
        let fast = time_it(|| sweep(excel_ppmt, &c));
        let vs_powf = naive.as_secs_f64() / fast.as_secs_f64().max(1e-12);
        let (walk_s, vs_walk) = if c.walk {
            let walk = time_it(|| sweep_amortize(&c));
            let speedup = walk.as_secs_f64() / fast.as_secs_f64().max(1e-12);
            (fmt_dur(walk), format!("{speedup:.1}x"))
        } else {
            ("—".into(), "—".into())
        };
        println!(
            "{:<36} {:>12} {:>12} {:>7.1}x {:>12} {:>8}",
            c.name,
            fmt_dur(naive),
            fmt_dur(fast),
            vs_powf,
            walk_s,
            vs_walk
        );
        let a = excel_ppmt_naive(c.rate, c.per, c.nper, c.pv, c.fv, c.typ).unwrap();
        let b = excel_ppmt(c.rate, c.per, c.nper, c.pv, c.fv, c.typ).unwrap();
        if c.match_powf {
            let scale = a.abs().max(b.abs()).max(1.0);
            assert!(
                (a - b).abs() / scale < 1e-12,
                "semantic mismatch on {}: {a} vs {b}",
                c.name
            );
        }
        if c.walk {
            let w = ppmt_amortize(c.rate, c.per, c.nper, c.pv, c.fv, c.typ);
            let scale_w = b.abs().max(w.abs()).max(1.0);
            assert!(
                (w - b).abs() / scale_w < 1e-9,
                "amortize mismatch on {}: {w} vs {b}",
                c.name
            );
        }
    }
}
