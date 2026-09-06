//! Before/after microbench for Excel `FIXED`.
//!
//! Compares the first-draft `excel_round_naive` + `format!` + allocating
//! comma-insert baseline with the specialized production kernel (digit
//! 0 / 1 / 2 / 3 fast paths + stack-buffer grouping) across signed values,
//! default two-decimal commas, `no_commas`, negative `decimals`, and IEEE
//! leftover snaps.
//!
//! ```text
//! cargo bench -p xlsx-engine-core --bench fixed
//! ```

use std::hint::black_box;
use std::time::{Duration, Instant};
use xlsx_engine_core::{
    excel_fixed, excel_fixed_naive, excel_fixed_slice, excel_fixed_slice_naive,
};

const ITERS: u32 = 40;
const N: usize = 20_000;

struct Case {
    name: &'static str,
    values: Vec<f64>,
    decimals: i32,
    no_commas: bool,
}

fn cases() -> Vec<Case> {
    let mixed: Vec<f64> = (0..N)
        .map(|i| {
            let s = if i % 2 == 0 { 1.0 } else { -1.0 };
            s * (i as f64) * 13.7 + 0.15
        })
        .collect();
    let millions: Vec<f64> = (0..N)
        .map(|i| 1_000_000.0 + i as f64 * 37.3)
        .collect();
    let leftover: Vec<f64> = (0..N).map(|i| 2.15 + (i as f64) * 1e-4).collect();
    let ones: Vec<f64> = (0..N)
        .map(|i| {
            if i % 3 == 0 {
                -(i as f64) - 0.5
            } else {
                i as f64 + 0.25
            }
        })
        .collect();
    vec![
        Case {
            name: "20k mixed signed × decimals=2 commas",
            values: mixed.clone(),
            decimals: 2,
            no_commas: false,
        },
        Case {
            name: "20k millions × decimals=2 commas",
            values: millions,
            decimals: 2,
            no_commas: false,
        },
        Case {
            name: "20k mixed × decimals=2 no_commas",
            values: mixed,
            decimals: 2,
            no_commas: true,
        },
        Case {
            name: "20k signed × decimals=0 commas",
            values: ones,
            decimals: 0,
            no_commas: false,
        },
        Case {
            name: "20k 2.15+eps × decimals=1 (IEEE snap)",
            values: leftover,
            decimals: 1,
            no_commas: false,
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

fn fold_naive(values: &[f64], decimals: i32, no_commas: bool) -> usize {
    let mut acc = 0usize;
    for n in values {
        acc = acc.wrapping_add(excel_fixed_naive(*n, decimals, no_commas).len());
    }
    acc
}

fn fold_fast(values: &[f64], decimals: i32, no_commas: bool) -> usize {
    let mut acc = 0usize;
    for n in values {
        acc = acc.wrapping_add(excel_fixed(*n, decimals, no_commas).len());
    }
    acc
}

fn main() {
    println!("FIXED kernel bench (format!+commas vs specialized buffer)");
    println!(
        "{:<46} {:>12} {:>12} {:>8}",
        "case", "naive", "optimized", "speedup"
    );
    println!("{}", "-".repeat(82));
    for c in cases() {
        let naive = time_it(ITERS, || {
            black_box(fold_naive(
                black_box(&c.values),
                black_box(c.decimals),
                black_box(c.no_commas),
            ));
        });
        let fast = time_it(ITERS, || {
            black_box(fold_fast(
                black_box(&c.values),
                black_box(c.decimals),
                black_box(c.no_commas),
            ));
        });
        row(&format!("FIXED / {}", c.name), naive, fast);
        let a = fold_naive(&c.values, c.decimals, c.no_commas);
        let b = fold_fast(&c.values, c.decimals, c.no_commas);
        assert_eq!(a, b, "semantic mismatch on {}", c.name);
    }

    let ns: Vec<f64> = (0..N).map(|i| (i as f64) * 13.7 + 1234.15).collect();
    let mut out_a = vec![String::new(); N];
    let mut out_b = vec![String::new(); N];
    let naive = time_it(ITERS, || {
        excel_fixed_slice_naive(black_box(&ns), 2, false, black_box(&mut out_a));
    });
    let fast = time_it(ITERS, || {
        excel_fixed_slice(black_box(&ns), 2, false, black_box(&mut out_b));
    });
    row("20k slice FIXED decimals=2 commas", naive, fast);
    excel_fixed_slice_naive(&ns, 2, false, &mut out_a);
    excel_fixed_slice(&ns, 2, false, &mut out_b);
    assert_eq!(out_a, out_b, "slice FIXED mismatch");
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
