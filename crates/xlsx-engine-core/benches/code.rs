//! Before/after microbench for Excel `CODE`.
//!
//! Compares the `Vec<char>` baseline (`excel_code_naive`) with the
//! production kernel (`excel_code`: first UTF-8 sequence only).
//!
//! ```text
//! cargo bench -p xlsx-engine-core --bench code
//! ```

use std::hint::black_box;
use std::time::{Duration, Instant};
use xlsx_engine_core::{excel_code, excel_code_naive};

const ITERS_HEAVY: u32 = 80;
const ITERS_LIGHT: u32 = 160;

struct Case {
    name: &'static str,
    text: String,
    iters: u32,
}

fn cases() -> Vec<Case> {
    vec![
        Case {
            name: "1-byte ASCII 'A'",
            text: "A".into(),
            iters: ITERS_LIGHT,
        },
        Case {
            name: "200k ASCII, first 'Z'",
            text: format!("Z{}", "x".repeat(199_999)),
            iters: ITERS_HEAVY,
        },
        Case {
            name: "200k ASCII, first C0 BEL",
            text: format!("\u{0007}{}", "x".repeat(199_999)),
            iters: ITERS_HEAVY,
        },
        Case {
            name: "50k 'é' (Latin-1 first)",
            text: "é".repeat(50_000),
            iters: ITERS_HEAVY,
        },
        Case {
            name: "10k euro then ASCII",
            text: format!("€{}", "x".repeat(10_000)),
            iters: ITERS_HEAVY,
        },
        Case {
            name: "10k CJK first (reject)",
            text: format!("中{}", "x".repeat(10_000)),
            iters: ITERS_HEAVY,
        },
        Case {
            name: "10k emoji first (reject)",
            text: format!("😀{}", "x".repeat(10_000)),
            iters: ITERS_LIGHT,
        },
        Case {
            name: "empty → #VALUE!",
            text: String::new(),
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
    } else if us >= 1.0 {
        format!("{us:.1} µs")
    } else {
        format!("{:.1} ns", us * 1000.0)
    }
}

fn main() {
    println!("CODE kernel bench (Vec<char> vs first UTF-8 sequence)");
    println!(
        "{:<36} {:>12} {:>12} {:>8}",
        "case", "naive", "optimized", "speedup"
    );
    println!("{}", "-".repeat(72));
    for c in cases() {
        let naive = time_it(c.iters, || {
            let _ = black_box(excel_code_naive(black_box(&c.text)));
        });
        let fast = time_it(c.iters, || {
            let _ = black_box(excel_code(black_box(&c.text)));
        });
        let speedup = naive.as_secs_f64() / fast.as_secs_f64().max(1e-12);
        println!(
            "{:<36} {:>12} {:>12} {:>7.1}x",
            c.name,
            fmt_dur(naive),
            fmt_dur(fast),
            speedup
        );
        let a = excel_code_naive(&c.text);
        let b = excel_code(&c.text);
        assert_eq!(a, b, "semantic mismatch on {}", c.name);
    }
}
