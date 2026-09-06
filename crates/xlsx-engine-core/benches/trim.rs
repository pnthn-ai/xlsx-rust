//! Before/after microbench for Excel `TRIM`.
//!
//! Compares the `Vec<char>` baseline (`excel_trim_naive`) with the
//! production kernel (`excel_trim`: SWAR space probes + identity / collapse).
//!
//! ```text
//! cargo bench -p xlsx-engine-core --bench trim
//! ```

use std::hint::black_box;
use std::time::{Duration, Instant};
use xlsx_engine_core::{excel_trim, excel_trim_naive};

const ITERS_HEAVY: u32 = 40;
const ITERS_LIGHT: u32 = 80;

struct Case {
    name: &'static str,
    text: String,
    iters: u32,
}

fn cases() -> Vec<Case> {
    let no_space = "x".repeat(200_000);
    let all_spaces = " ".repeat(200_000);
    let doubled = "a  ".repeat(66_667);
    let already = "a b ".repeat(50_000);
    let already_clean = already.trim_end().to_string(); // "a b a b ... a b"
    let lead = format!("{}word", " ".repeat(200_000));
    let unicode = "café  ".repeat(40_000);
    vec![
        Case {
            name: "200k 'x' no spaces (identity)",
            text: no_space,
            iters: ITERS_LIGHT,
        },
        Case {
            name: "200k spaces only → empty",
            text: all_spaces,
            iters: ITERS_LIGHT,
        },
        Case {
            name: "200k 'a  ' collapse + end trim",
            text: doubled,
            iters: ITERS_HEAVY,
        },
        Case {
            name: "200k already-trimmed 'a b …'",
            text: already_clean,
            iters: ITERS_HEAVY,
        },
        Case {
            name: "200k leading spaces + word",
            text: lead,
            iters: ITERS_LIGHT,
        },
        Case {
            name: "240k café-repeat unicode collapse",
            text: unicode,
            iters: ITERS_HEAVY,
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

fn main() {
    println!("TRIM kernel bench (Vec<char> vs SWAR byte collapse)");
    println!(
        "{:<42} {:>12} {:>12} {:>8}",
        "case", "naive", "optimized", "speedup"
    );
    println!("{}", "-".repeat(78));
    for c in cases() {
        let naive = time_it(c.iters, || {
            let _ = black_box(excel_trim_naive(black_box(&c.text)));
        });
        let fast = time_it(c.iters, || {
            let _ = black_box(excel_trim(black_box(&c.text)));
        });
        let speedup = naive.as_secs_f64() / fast.as_secs_f64().max(1e-12);
        println!(
            "{:<42} {:>12} {:>12} {:>7.1}x",
            c.name,
            fmt_dur(naive),
            fmt_dur(fast),
            speedup
        );
        let a = excel_trim_naive(&c.text);
        let b = excel_trim(&c.text);
        assert_eq!(a, b, "semantic mismatch on {}", c.name);
    }
}
