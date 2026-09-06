//! Before/after microbench for Excel classic `FLOOR`.
//!
//! Compares the first-draft IEEE `s * (n/s).floor()` then `excel_round_15`
//! path (`excel_floor_naive` / slice) with the production kernel
//! (`excel_int` at significance 1, safe-integer path, cheap multiple snap).
//!
//! ```text
//! cargo bench -p xlsx-engine-core --bench excel_floor
//! ```

use std::hint::black_box;
use std::time::{Duration, Instant};
use xlsx_types::{excel_floor, excel_floor_naive, excel_floor_slice, excel_floor_slice_naive};

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

fn leftovers() -> Vec<f64> {
    let tenths = (0..10).fold(0.0, |a, _| a + 0.1);
    let sub = 0.3 - 0.1 - 0.2;
    (0..N)
        .map(|i| if i % 2 == 0 { tenths } else { sub })
        .collect()
}

fn main() {
    println!("FLOOR kernel bench (IEEE snap-then-floor vs INT/int-path + leftover snap)");
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
                acc += excel_floor_naive(black_box(*n), black_box(7.0)).unwrap_or(0.0);
            }
            black_box(acc);
        });
        let fast = time_it(ITERS, || {
            let mut acc = 0.0;
            for n in &ns {
                acc += excel_floor(black_box(*n), black_box(7.0)).unwrap_or(0.0);
            }
            black_box(acc);
        });
        row("200k scalar FLOOR(n, 7) ints", naive, fast);
        for n in ns.iter().step_by(97) {
            assert_eq!(
                excel_floor(*n, 7.0),
                excel_floor_naive(*n, 7.0),
                "FLOOR({n}, 7)"
            );
        }
    }

    {
        let mut out_a = vec![0.0; N];
        let mut out_b = vec![0.0; N];
        let naive = time_it(ITERS, || {
            excel_floor_slice_naive(black_box(&ns), black_box(7.0), black_box(&mut out_a));
        });
        let fast = time_it(ITERS, || {
            excel_floor_slice(black_box(&ns), black_box(7.0), black_box(&mut out_b));
        });
        row("200k slice FLOOR(n, 7) ints", naive, fast);
        excel_floor_slice_naive(&ns, 7.0, &mut out_a);
        excel_floor_slice(&ns, 7.0, &mut out_b);
        assert_eq!(out_a, out_b, "slice FLOOR(n, 7) mismatch");
    }

    {
        let naive = time_it(ITERS, || {
            let mut acc = 0.0;
            for n in &ns {
                acc += excel_floor_naive(black_box(*n), black_box(1.0)).unwrap_or(0.0);
            }
            black_box(acc);
        });
        let fast = time_it(ITERS, || {
            let mut acc = 0.0;
            for n in &ns {
                acc += excel_floor(black_box(*n), black_box(1.0)).unwrap_or(0.0);
            }
            black_box(acc);
        });
        row("200k scalar FLOOR(n, 1) ints", naive, fast);
        for n in ns.iter().step_by(97) {
            assert_eq!(
                excel_floor(*n, 1.0),
                excel_floor_naive(*n, 1.0),
                "FLOOR({n}, 1)"
            );
        }
    }

    {
        let mut out_a = vec![0.0; N];
        let mut out_b = vec![0.0; N];
        let naive = time_it(ITERS, || {
            excel_floor_slice_naive(black_box(&ns), black_box(1.0), black_box(&mut out_a));
        });
        let fast = time_it(ITERS, || {
            excel_floor_slice(black_box(&ns), black_box(1.0), black_box(&mut out_b));
        });
        row("200k slice FLOOR(n, 1) ints", naive, fast);
        excel_floor_slice_naive(&ns, 1.0, &mut out_a);
        excel_floor_slice(&ns, 1.0, &mut out_b);
        assert_eq!(out_a, out_b, "slice FLOOR(n, 1) mismatch");
    }

    {
        let naive = time_it(ITERS, || {
            let mut acc = 0.0;
            for n in &dec {
                acc += excel_floor_naive(black_box(*n), black_box(0.01)).unwrap_or(0.0);
            }
            black_box(acc);
        });
        let fast = time_it(ITERS, || {
            let mut acc = 0.0;
            for n in &dec {
                acc += excel_floor(black_box(*n), black_box(0.01)).unwrap_or(0.0);
            }
            black_box(acc);
        });
        row("200k scalar FLOOR(n, 0.01) dec", naive, fast);
    }

    {
        let mut out = vec![0.0; N];
        let naive = time_it(ITERS, || {
            excel_floor_slice_naive(black_box(&dec), black_box(0.1), black_box(&mut out));
        });
        let fast = time_it(ITERS, || {
            excel_floor_slice(black_box(&dec), black_box(0.1), black_box(&mut out));
        });
        row("200k slice FLOOR(n, 0.1) dec", naive, fast);
        assert_eq!(excel_floor(1.2, 0.1).unwrap(), 1.2);
    }

    {
        let naive = time_it(ITERS, || {
            let mut acc = 0.0;
            for n in &left {
                acc += excel_floor_naive(black_box(*n), black_box(1.0)).unwrap_or(0.0);
            }
            black_box(acc);
        });
        let fast = time_it(ITERS, || {
            let mut acc = 0.0;
            for n in &left {
                acc += excel_floor(black_box(*n), black_box(1.0)).unwrap_or(0.0);
            }
            black_box(acc);
        });
        row("200k scalar FLOOR leftover s=1", naive, fast);
        let tenths = (0..10).fold(0.0, |a, _| a + 0.1);
        assert_eq!(excel_floor(tenths, 1.0).unwrap(), 1.0);
        let sub = 0.3 - 0.1 - 0.2;
        assert_eq!(excel_floor(sub, 1.0).unwrap(), 0.0);
    }

    {
        let mut out = vec![0.0; N];
        let naive = time_it(ITERS, || {
            excel_floor_slice_naive(black_box(&left), black_box(1.0), black_box(&mut out));
        });
        let fast = time_it(ITERS, || {
            excel_floor_slice(black_box(&left), black_box(1.0), black_box(&mut out));
        });
        row("200k slice FLOOR leftover s=1", naive, fast);
    }

    {
        let naive = time_it(ITERS * 4, || {
            let mut acc = 0u32;
            for n in ns.iter().step_by(8) {
                if excel_floor_naive(black_box(n.abs().max(1.0)), black_box(-2.0)).is_err() {
                    acc += 1;
                }
                if excel_floor_naive(black_box(*n), black_box(0.0)).is_err() {
                    acc += 1;
                }
            }
            black_box(acc);
        });
        let fast = time_it(ITERS * 4, || {
            let mut acc = 0u32;
            for n in ns.iter().step_by(8) {
                if excel_floor(black_box(n.abs().max(1.0)), black_box(-2.0)).is_err() {
                    acc += 1;
                }
                if excel_floor(black_box(*n), black_box(0.0)).is_err() {
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
