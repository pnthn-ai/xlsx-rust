//! Before/after microbench for Excel `ROUNDDOWN`.
//!
//! Compares the textbook two-`powi` baseline (`rounddown_naive` / slice)
//! with the production kernel (specialised `0` / `±1`…`±4` + table scale).
//!
//! ```text
//! cargo bench -p xlsx-engine-core --bench rounddown
//! ```

use std::hint::black_box;
use std::time::{Duration, Instant};
use xlsx_engine_core::{
    excel_rounddown, excel_rounddown_naive, excel_rounddown_slice, excel_rounddown_slice_digits,
    excel_rounddown_slice_digits_naive, excel_rounddown_slice_naive,
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

fn mixed() -> (Vec<f64>, Vec<i32>) {
    let ns: Vec<f64> = (0..N)
        .map(|i| {
            let s = if i % 2 == 0 { 1.0 } else { -1.0 };
            s * (i as f64) * 0.137 + 0.15
        })
        .collect();
    let ds: Vec<i32> = (0..N)
        .map(|i| match i % 9 {
            0 => 0,
            1 => 1,
            2 => 2,
            3 => 3,
            4 => 4,
            5 => -1,
            6 => -2,
            7 => -3,
            _ => -4,
        })
        .collect();
    (ns, ds)
}

fn fractions() -> Vec<f64> {
    (0..N)
        .map(|i| {
            if i % 3 == 0 {
                -(i as f64) - 0.5
            } else {
                i as f64 + 0.25
            }
        })
        .collect()
}

fn leftovers() -> Vec<f64> {
    (0..N).map(|i| 1.15 + (i as f64) * 1e-6).collect()
}

fn tens() -> Vec<f64> {
    (0..N).map(|i| 1000.0 + i as f64 * 0.37).collect()
}

fn main() {
    println!("ROUNDDOWN kernel bench (two-powi baseline vs specialized)");
    println!(
        "{:<46} {:>12} {:>12} {:>8}",
        "case", "naive", "optimized", "speedup"
    );
    println!("{}", "-".repeat(82));

    let (mixed_n, mixed_d) = mixed();
    let frac = fractions();
    let left = leftovers();
    let tens_n = tens();

    {
        let naive = time_it(ITERS, || {
            let mut acc = 0.0;
            for (n, d) in mixed_n.iter().zip(mixed_d.iter()) {
                acc += excel_rounddown_naive(black_box(*n), black_box(*d));
            }
            black_box(acc);
        });
        let fast = time_it(ITERS, || {
            let mut acc = 0.0;
            for (n, d) in mixed_n.iter().zip(mixed_d.iter()) {
                acc += excel_rounddown(black_box(*n), black_box(*d));
            }
            black_box(acc);
        });
        row("200k scalar mixed signed × mixed digits", naive, fast);
        for (n, d) in mixed_n.iter().zip(mixed_d.iter()).step_by(97) {
            assert_eq!(
                excel_rounddown(*n, *d),
                excel_rounddown_naive(*n, *d),
                "ROUNDDOWN({n}, {d})"
            );
        }
    }

    {
        let mut out_a = vec![0.0; N];
        let mut out_b = vec![0.0; N];
        let naive = time_it(ITERS, || {
            excel_rounddown_slice_naive(
                black_box(&mixed_n),
                black_box(&mixed_d),
                black_box(&mut out_a),
            );
        });
        let fast = time_it(ITERS, || {
            excel_rounddown_slice(
                black_box(&mixed_n),
                black_box(&mixed_d),
                black_box(&mut out_b),
            );
        });
        row("200k slice mixed signed × mixed digits", naive, fast);
        excel_rounddown_slice_naive(&mixed_n, &mixed_d, &mut out_a);
        excel_rounddown_slice(&mixed_n, &mixed_d, &mut out_b);
        assert_eq!(out_a, out_b, "slice mixed mismatch");
    }

    {
        let naive = time_it(ITERS, || {
            let mut acc = 0.0;
            for n in &frac {
                acc += excel_rounddown_naive(black_box(*n), 0);
            }
            black_box(acc);
        });
        let fast = time_it(ITERS, || {
            let mut acc = 0.0;
            for n in &frac {
                acc += excel_rounddown(black_box(*n), 0);
            }
            black_box(acc);
        });
        row("200k scalar ROUNDDOWN digits=0", naive, fast);
    }

    {
        let mut out_a = vec![0.0; N];
        let mut out_b = vec![0.0; N];
        let naive = time_it(ITERS, || {
            excel_rounddown_slice_digits_naive(black_box(&frac), 0, black_box(&mut out_a));
        });
        let fast = time_it(ITERS, || {
            excel_rounddown_slice_digits(black_box(&frac), 0, black_box(&mut out_b));
        });
        row("200k slice ROUNDDOWN digits=0", naive, fast);
        excel_rounddown_slice_digits_naive(&frac, 0, &mut out_a);
        excel_rounddown_slice_digits(&frac, 0, &mut out_b);
        assert_eq!(out_a, out_b, "slice digits=0 mismatch");
    }

    {
        let naive = time_it(ITERS, || {
            let mut acc = 0.0;
            for n in &tens_n {
                acc += excel_rounddown_naive(black_box(*n), -1);
            }
            black_box(acc);
        });
        let fast = time_it(ITERS, || {
            let mut acc = 0.0;
            for n in &tens_n {
                acc += excel_rounddown(black_box(*n), -1);
            }
            black_box(acc);
        });
        row("200k scalar ROUNDDOWN num_digits=-1", naive, fast);
    }

    {
        let mut out = vec![0.0; N];
        let naive = time_it(ITERS, || {
            excel_rounddown_slice_digits_naive(black_box(&tens_n), -1, black_box(&mut out));
        });
        let fast = time_it(ITERS, || {
            excel_rounddown_slice_digits(black_box(&tens_n), -1, black_box(&mut out));
        });
        row("200k slice ROUNDDOWN num_digits=-1", naive, fast);
    }

    {
        let naive = time_it(ITERS, || {
            let mut acc = 0.0;
            for n in &left {
                acc += excel_rounddown_naive(black_box(*n), 2);
            }
            black_box(acc);
        });
        let fast = time_it(ITERS, || {
            let mut acc = 0.0;
            for n in &left {
                acc += excel_rounddown(black_box(*n), 2);
            }
            black_box(acc);
        });
        row("200k scalar ROUNDDOWN 1.15+eps digits=2", naive, fast);
        assert_eq!(excel_rounddown(1.15, 2), 1.15);
        assert_eq!(excel_rounddown_naive(1.15, 2), 1.15);
    }

    {
        let mut out = vec![0.0; N];
        let naive = time_it(ITERS, || {
            excel_rounddown_slice_digits_naive(black_box(&left), 2, black_box(&mut out));
        });
        let fast = time_it(ITERS, || {
            excel_rounddown_slice_digits(black_box(&left), 2, black_box(&mut out));
        });
        row("200k slice ROUNDDOWN 1.15+eps digits=2", naive, fast);
    }
}

fn row(name: &str, naive: Duration, fast: Duration) {
    let speedup = naive.as_secs_f64() / fast.as_secs_f64().max(1e-12);
    println!(
        "{:<46} {:>12} {:>12} {:>7.1}x",
        name,
        fmt_dur(naive),
        fmt_dur(fast),
        speedup
    );
}
