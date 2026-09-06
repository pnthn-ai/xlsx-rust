//! Before/after microbench for Excel classic `CEILING`.
//!
//! Compares the first-draft snap-both-args then IEEE `ceil` path
//! (`excel_ceiling_naive` / slice) with the production kernel (safe-integer
//! path + cheap 15-digit multiple snap).
//!
//! ```text
//! cargo bench -p xlsx-engine-core --bench excel_ceiling
//! ```

use std::hint::black_box;
use std::time::{Duration, Instant};
use xlsx_types::{
    excel_ceiling, excel_ceiling_naive, excel_ceiling_slice, excel_ceiling_slice_naive,
};

const N: usize = 200_000;
const ITERS: u32 = 40;

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

fn ints() -> Vec<f64> {
    (0..N).map(|i| (i as i64 % 997) as f64 - 400.0).collect()
}

fn decimals() -> Vec<f64> {
    (0..N).map(|i| (i as f64) * 0.013 + 0.17).collect()
}

fn multiples() -> Vec<f64> {
    (0..N).map(|i| (i as f64) * 0.1 + 0.2).collect()
}

fn main() {
    println!("CEILING kernel bench (snap-then-ceil vs integer path + cheap multiple)");
    println!(
        "{:<42} {:>12} {:>12} {:>8}",
        "case", "naive", "optimized", "speedup"
    );
    println!("{}", "-".repeat(78));

    let ns = ints();
    let dec = decimals();
    let mult = multiples();

    {
        let naive = time_it(ITERS, || {
            let mut acc = 0.0;
            for n in &ns {
                acc += excel_ceiling_naive(black_box(*n), black_box(7.0)).unwrap_or(0.0);
            }
            black_box(acc);
        });
        let fast = time_it(ITERS, || {
            let mut acc = 0.0;
            for n in &ns {
                acc += excel_ceiling(black_box(*n), black_box(7.0)).unwrap_or(0.0);
            }
            black_box(acc);
        });
        row("200k scalar CEILING(n, 7) ints", naive, fast);
        for n in ns.iter().step_by(97) {
            assert_eq!(
                excel_ceiling(*n, 7.0),
                excel_ceiling_naive(*n, 7.0),
                "CEILING({n}, 7)"
            );
        }
    }

    {
        let mut out_a = vec![0.0; N];
        let mut out_b = vec![0.0; N];
        let naive = time_it(ITERS, || {
            excel_ceiling_slice_naive(black_box(&ns), black_box(7.0), black_box(&mut out_a));
        });
        let fast = time_it(ITERS, || {
            excel_ceiling_slice(black_box(&ns), black_box(7.0), black_box(&mut out_b));
        });
        row("200k slice CEILING(n, 7) ints", naive, fast);
        excel_ceiling_slice_naive(&ns, 7.0, &mut out_a);
        excel_ceiling_slice(&ns, 7.0, &mut out_b);
        assert_eq!(out_a, out_b, "slice CEILING ints mismatch");
    }

    {
        let naive = time_it(ITERS, || {
            let mut acc = 0.0;
            for n in &dec {
                acc += excel_ceiling_naive(black_box(*n), black_box(0.01)).unwrap_or(0.0);
            }
            black_box(acc);
        });
        let fast = time_it(ITERS, || {
            let mut acc = 0.0;
            for n in &dec {
                acc += excel_ceiling(black_box(*n), black_box(0.01)).unwrap_or(0.0);
            }
            black_box(acc);
        });
        row("200k scalar CEILING(n, 0.01) dec", naive, fast);
    }

    {
        let mut out_a = vec![0.0; N];
        let mut out_b = vec![0.0; N];
        let naive = time_it(ITERS, || {
            excel_ceiling_slice_naive(black_box(&dec), black_box(0.01), black_box(&mut out_a));
        });
        let fast = time_it(ITERS, || {
            excel_ceiling_slice(black_box(&dec), black_box(0.01), black_box(&mut out_b));
        });
        row("200k slice CEILING(n, 0.01) dec", naive, fast);
    }

    {
        let naive = time_it(ITERS, || {
            let mut acc = 0.0;
            for n in &mult {
                acc += excel_ceiling_naive(black_box(*n), black_box(0.1)).unwrap_or(0.0);
            }
            black_box(acc);
        });
        let fast = time_it(ITERS, || {
            let mut acc = 0.0;
            for n in &mult {
                acc += excel_ceiling(black_box(*n), black_box(0.1)).unwrap_or(0.0);
            }
            black_box(acc);
        });
        row("200k scalar CEILING(n, 0.1) near-mult", naive, fast);
        assert_eq!(excel_ceiling(1.2, 0.1).unwrap(), 1.2);
    }

    {
        let mut out = vec![0.0; N];
        let naive = time_it(ITERS, || {
            excel_ceiling_slice_naive(black_box(&mult), black_box(0.1), black_box(&mut out));
        });
        let fast = time_it(ITERS, || {
            excel_ceiling_slice(black_box(&mult), black_box(0.1), black_box(&mut out));
        });
        row("200k slice CEILING(n, 0.1) near-mult", naive, fast);
    }

    {
        let naive = time_it(ITERS * 4, || {
            let mut acc = 0u32;
            for n in ns.iter().step_by(8) {
                if excel_ceiling_naive(black_box(n.abs().max(1.0)), black_box(-2.0)).is_err() {
                    acc += 1;
                }
                if excel_ceiling_naive(black_box(*n), black_box(0.0)).is_err() {
                    acc += 1;
                }
            }
            black_box(acc);
        });
        let fast = time_it(ITERS * 4, || {
            let mut acc = 0u32;
            for n in ns.iter().step_by(8) {
                if excel_ceiling(black_box(n.abs().max(1.0)), black_box(-2.0)).is_err() {
                    acc += 1;
                }
                if excel_ceiling(black_box(*n), black_box(0.0)).is_err() {
                    acc += 1;
                }
            }
            black_box(acc);
        });
        row("25k sign/zero-sig error checks", naive, fast);
    }
}

fn row(name: &str, naive: Duration, fast: Duration) {
    let speedup = naive.as_secs_f64() / fast.as_secs_f64().max(1e-12);
    println!(
        "{:<42} {:>12} {:>12} {:>7.1}x",
        name,
        fmt_dur(naive),
        fmt_dur(fast),
        speedup
    );
}
