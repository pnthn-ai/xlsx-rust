//! Before/after microbench for Excel `CLEAN`.
//!
//! Compares the `Vec<char>` baseline (`excel_clean_naive`) with the
//! specialized production kernel (`excel_clean`) across string sizes.
//!
//! ```text
//! cargo bench -p xlsx-engine-core --bench clean
//! ```

use std::hint::black_box;
use std::time::{Duration, Instant};
use xlsx_engine_core::{excel_clean, excel_clean_naive};

const ITERS_HEAVY: u32 = 24;
const ITERS_LIGHT: u32 = 48;

struct Case {
    name: &'static str,
    text: String,
    iters: u32,
}

fn cases() -> Vec<Case> {
    let ascii_200k = "a".repeat(200_000);
    let mut sparse = ascii_200k.clone();
    // One C0 every 1_000 bytes, plus edges.
    for i in (0..200_000).step_by(1_000) {
        sparse.replace_range(i..i + 1, "\u{0007}");
    }
    let dense: String = (0..200_000)
        .map(|i| if i % 2 == 0 { 'x' } else { '\u{0001}' })
        .collect();
    let mut edges = ascii_200k.clone();
    edges.replace_range(
        0..8,
        "\u{0009}\u{0009}\u{0009}\u{0009}\u{0009}\u{0009}\u{0009}\u{0009}",
    );
    edges.push('\n');
    let utf8_clean = "é".repeat(50_000);
    let mut utf8_dirty = utf8_clean.clone();
    utf8_dirty.insert(utf8_clean.len() / 2, '\u{0007}');
    vec![
        Case {
            name: "200k clean ASCII (no-op)",
            text: ascii_200k,
            iters: ITERS_HEAVY,
        },
        Case {
            name: "200k ASCII sparse BEL / 1k",
            text: sparse,
            iters: ITERS_HEAVY,
        },
        Case {
            name: "200k ASCII every-other C0",
            text: dense,
            iters: ITERS_HEAVY,
        },
        Case {
            name: "200k ASCII C0 only at edges",
            text: edges,
            iters: ITERS_LIGHT,
        },
        Case {
            name: "50k 'é' already clean",
            text: utf8_clean,
            iters: ITERS_HEAVY,
        },
        Case {
            name: "50k 'é' one BEL mid",
            text: utf8_dirty,
            iters: ITERS_HEAVY,
        },
        Case {
            name: "10k emoji already clean",
            text: "😀".repeat(10_000),
            iters: ITERS_LIGHT,
        },
        Case {
            name: "32-byte all C0 → empty",
            text: (0u8..=31).map(|n| n as char).collect(),
            iters: ITERS_LIGHT,
        },
        Case {
            name: "empty string",
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
    } else {
        format!("{us:.1} µs")
    }
}

fn main() {
    println!("CLEAN kernel bench (Vec<char> naive vs specialized)");
    println!(
        "{:<36} {:>12} {:>12} {:>8}",
        "case", "naive", "optimized", "speedup"
    );
    println!("{}", "-".repeat(72));
    for c in cases() {
        let naive = time_it(c.iters, || {
            black_box(excel_clean_naive(black_box(&c.text)));
        });
        let fast = time_it(c.iters, || {
            black_box(excel_clean(black_box(&c.text)));
        });
        let speedup = naive.as_secs_f64() / fast.as_secs_f64().max(1e-12);
        println!(
            "{:<36} {:>12} {:>12} {:>7.1}x",
            c.name,
            fmt_dur(naive),
            fmt_dur(fast),
            speedup
        );
        let a = excel_clean_naive(&c.text);
        let b = excel_clean(&c.text);
        assert_eq!(a, b, "semantic mismatch on {}", c.name);
    }
}
