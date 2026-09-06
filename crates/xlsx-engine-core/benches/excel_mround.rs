//! Before/after microbench for Excel `MROUND`.
//!
//! Compares the first-draft snap-both-args then rem-based half-away path
//! (`excel_mround_naive` / slice) with the production kernel (`ROUND` at
//! `|multiple| == 1`, safe-integer path, cheap multiple snap).
//!
//! ```text
//! cargo bench -p xlsx-engine-core --bench excel_mround
//! ```

use std::hint::black_box;
use std::time::{Duration, Instant};
use xlsx_types::{excel_mround, excel_mround_naive, excel_mround_slice, excel_mround_slice_naive};

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
    (0..N)
        .map(|i| (i as i64 % 997) as f64 - 50.0)
        .map(|n| n.abs() + 1.0)
        .collect()
}

fn decimals() -> Vec<f64> {
    (0..N).map(|i| (i as f64) * 0.013 + 0.17).collect()
}

fn leftovers() -> Vec<f64> {
    (0..N)
        .map(|i| if i % 2 == 0 { 6.05 } else { 7.05 })
        .collect()
}

fn main() {
    println!("MROUND kernel bench (snap-then-rem vs ROUND-share / int-path + leftover)");
    println!(
        "{:<42} {:>12} {:>12} {:>8}",
        "case", "naive", "optimized", "speedup"
    );
    println!("{}", "-".repeat(78));

    let ns = ints();
    let dec = decimals();
    let left = leftovers();

    {
        let naive = time_it(ITERS, || {
            let mut acc = 0.0;
            for n in &ns {
                acc += excel_mround_naive(black_box(*n), black_box(7.0)).unwrap_or(0.0);
            }
            black_box(acc);
        });
        let fast = time_it(ITERS, || {
            let mut acc = 0.0;
            for n in &ns {
                acc += excel_mround(black_box(*n), black_box(7.0)).unwrap_or(0.0);
            }
            black_box(acc);
        });
        row("200k scalar MROUND(n, 7) ints", naive, fast);
        for n in ns.iter().step_by(97) {
            assert_eq!(
                excel_mround(*n, 7.0),
                excel_mround_naive(*n, 7.0),
                "MROUND({n}, 7)"
            );
        }
    }

    {
        let mut out_a = vec![0.0; N];
        let mut out_b = vec![0.0; N];
        let naive = time_it(ITERS, || {
            excel_mround_slice_naive(black_box(&ns), black_box(7.0), black_box(&mut out_a));
        });
        let fast = time_it(ITERS, || {
            excel_mround_slice(black_box(&ns), black_box(7.0), black_box(&mut out_b));
        });
        row("200k slice MROUND(n, 7) ints", naive, fast);
        excel_mround_slice_naive(&ns, 7.0, &mut out_a);
        excel_mround_slice(&ns, 7.0, &mut out_b);
        assert_eq!(out_a, out_b, "slice MROUND(n, 7) mismatch");
    }

    {
        let naive = time_it(ITERS, || {
            let mut acc = 0.0;
            for n in &ns {
                acc += excel_mround_naive(black_box(*n), black_box(1.0)).unwrap_or(0.0);
            }
            black_box(acc);
        });
        let fast = time_it(ITERS, || {
            let mut acc = 0.0;
            for n in &ns {
                acc += excel_mround(black_box(*n), black_box(1.0)).unwrap_or(0.0);
            }
            black_box(acc);
        });
        row("200k scalar MROUND(n, 1) ints", naive, fast);
        for n in ns.iter().step_by(97) {
            assert_eq!(
                excel_mround(*n, 1.0),
                excel_mround_naive(*n, 1.0),
                "MROUND({n}, 1)"
            );
        }
    }

    {
        let mut out_a = vec![0.0; N];
        let mut out_b = vec![0.0; N];
        let naive = time_it(ITERS, || {
            excel_mround_slice_naive(black_box(&ns), black_box(1.0), black_box(&mut out_a));
        });
        let fast = time_it(ITERS, || {
            excel_mround_slice(black_box(&ns), black_box(1.0), black_box(&mut out_b));
        });
        row("200k slice MROUND(n, 1) ints", naive, fast);
        excel_mround_slice_naive(&ns, 1.0, &mut out_a);
        excel_mround_slice(&ns, 1.0, &mut out_b);
        assert_eq!(out_a, out_b, "slice MROUND(n, 1) mismatch");
    }

    {
        let naive = time_it(ITERS, || {
            let mut acc = 0.0;
            for n in &dec {
                acc += excel_mround_naive(black_box(*n), black_box(0.01)).unwrap_or(0.0);
            }
            black_box(acc);
        });
        let fast = time_it(ITERS, || {
            let mut acc = 0.0;
            for n in &dec {
                acc += excel_mround(black_box(*n), black_box(0.01)).unwrap_or(0.0);
            }
            black_box(acc);
        });
        row("200k scalar MROUND(n, 0.01) dec", naive, fast);
    }

    {
        let mut out = vec![0.0; N];
        let naive = time_it(ITERS, || {
            excel_mround_slice_naive(black_box(&dec), black_box(0.1), black_box(&mut out));
        });
        let fast = time_it(ITERS, || {
            excel_mround_slice(black_box(&dec), black_box(0.1), black_box(&mut out));
        });
        row("200k slice MROUND(n, 0.1) dec", naive, fast);
        assert_eq!(excel_mround(1.2, 0.1).unwrap(), 1.2);
    }

    {
        let naive = time_it(ITERS, || {
            let mut acc = 0.0;
            for n in &left {
                acc += excel_mround_naive(black_box(*n), black_box(0.1)).unwrap_or(0.0);
            }
            black_box(acc);
        });
        let fast = time_it(ITERS, || {
            let mut acc = 0.0;
            for n in &left {
                acc += excel_mround(black_box(*n), black_box(0.1)).unwrap_or(0.0);
            }
            black_box(acc);
        });
        row("200k scalar MROUND leftover 0.1", naive, fast);
        assert_eq!(excel_mround(6.05, 0.1).unwrap(), 6.0);
        assert!((excel_mround(7.05, 0.1).unwrap() - 7.1).abs() < 1e-12);
    }

    {
        let mut out = vec![0.0; N];
        let naive = time_it(ITERS, || {
            excel_mround_slice_naive(black_box(&left), black_box(0.1), black_box(&mut out));
        });
        let fast = time_it(ITERS, || {
            excel_mround_slice(black_box(&left), black_box(0.1), black_box(&mut out));
        });
        row("200k slice MROUND leftover 0.1", naive, fast);
    }

    {
        let naive = time_it(ITERS * 4, || {
            let mut acc = 0u32;
            for n in ns.iter().step_by(8) {
                if excel_mround_naive(black_box(*n), black_box(-2.0)).is_err() {
                    acc += 1;
                }
                let _ = excel_mround_naive(black_box(*n), black_box(0.0));
            }
            black_box(acc);
        });
        let fast = time_it(ITERS * 4, || {
            let mut acc = 0u32;
            for n in ns.iter().step_by(8) {
                if excel_mround(black_box(*n), black_box(-2.0)).is_err() {
                    acc += 1;
                }
                let _ = excel_mround(black_box(*n), black_box(0.0));
            }
            black_box(acc);
        });
        row("25k sign/zero-mult checks", naive, fast);
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
