//! Before/after microbench for Excel `INT`.
//!
//! Compares IEEE `n.floor()` (`excel_int_naive` / slice) with the production
//! kernel (already-integer identity + 15-digit leftover snap).
//!
//! ```text
//! cargo bench -p xlsx-engine-core --bench excel_int
//! ```

use std::hint::black_box;
use std::time::{Duration, Instant};
use xlsx_types::{excel_int, excel_int_naive, excel_int_slice, excel_int_slice_naive};

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

fn fractions() -> Vec<f64> {
    (0..N).map(|i| (i as f64) * 0.013 + 0.17).collect()
}

fn leftovers() -> Vec<f64> {
    let third = 1.0 / 3.0 + 1.0 / 3.0 + 1.0 / 3.0;
    (0..N)
        .map(|i| if i % 2 == 0 { third } else { -third })
        .collect()
}

fn main() {
    println!("INT kernel bench (IEEE floor vs integer identity + 15-digit snap)");
    println!(
        "{:<42} {:>12} {:>12} {:>8}",
        "case", "naive", "optimized", "speedup"
    );
    println!("{}", "-".repeat(78));

    let ns = ints();
    let frac = fractions();
    let left = leftovers();

    {
        let naive = time_it(ITERS, || {
            let mut acc = 0.0;
            for n in &ns {
                acc += excel_int_naive(black_box(*n));
            }
            black_box(acc);
        });
        let fast = time_it(ITERS, || {
            let mut acc = 0.0;
            for n in &ns {
                acc += excel_int(black_box(*n));
            }
            black_box(acc);
        });
        row("200k scalar INT ints", naive, fast);
        for n in ns.iter().step_by(97) {
            assert_eq!(excel_int(*n), excel_int_naive(*n), "INT({n})");
        }
    }

    {
        let mut out_a = vec![0.0; N];
        let mut out_b = vec![0.0; N];
        let naive = time_it(ITERS, || {
            excel_int_slice_naive(black_box(&ns), black_box(&mut out_a));
        });
        let fast = time_it(ITERS, || {
            excel_int_slice(black_box(&ns), black_box(&mut out_b));
        });
        row("200k slice INT ints", naive, fast);
        excel_int_slice_naive(&ns, &mut out_a);
        excel_int_slice(&ns, &mut out_b);
        assert_eq!(out_a, out_b, "slice INT ints mismatch");
    }

    {
        let naive = time_it(ITERS, || {
            let mut acc = 0.0;
            for n in &frac {
                acc += excel_int_naive(black_box(*n));
            }
            black_box(acc);
        });
        let fast = time_it(ITERS, || {
            let mut acc = 0.0;
            for n in &frac {
                acc += excel_int(black_box(*n));
            }
            black_box(acc);
        });
        row("200k scalar INT fractions", naive, fast);
        for n in frac.iter().step_by(97) {
            assert_eq!(excel_int(*n), excel_int_naive(*n), "INT({n})");
        }
    }

    {
        let mut out_a = vec![0.0; N];
        let mut out_b = vec![0.0; N];
        let naive = time_it(ITERS, || {
            excel_int_slice_naive(black_box(&frac), black_box(&mut out_a));
        });
        let fast = time_it(ITERS, || {
            excel_int_slice(black_box(&frac), black_box(&mut out_b));
        });
        row("200k slice INT fractions", naive, fast);
        excel_int_slice_naive(&frac, &mut out_a);
        excel_int_slice(&frac, &mut out_b);
        assert_eq!(out_a, out_b, "slice INT fractions mismatch");
    }

    {
        let naive = time_it(ITERS, || {
            let mut acc = 0.0;
            for n in &left {
                acc += excel_int_naive(black_box(*n));
            }
            black_box(acc);
        });
        let fast = time_it(ITERS, || {
            let mut acc = 0.0;
            for n in &left {
                acc += excel_int(black_box(*n));
            }
            black_box(acc);
        });
        row("200k scalar INT 15-digit leftover", naive, fast);
        // Production snap: 1/3+1/3+1/3 is 1, not IEEE floor 0.
        let third = 1.0 / 3.0 + 1.0 / 3.0 + 1.0 / 3.0;
        assert_eq!(excel_int(third), 1.0);
        assert_eq!(excel_int_naive(third), 0.0);
    }

    {
        let mut out = vec![0.0; N];
        let naive = time_it(ITERS, || {
            excel_int_slice_naive(black_box(&left), black_box(&mut out));
        });
        let fast = time_it(ITERS, || {
            excel_int_slice(black_box(&left), black_box(&mut out));
        });
        row("200k slice INT 15-digit leftover", naive, fast);
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
