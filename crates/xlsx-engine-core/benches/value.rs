//! Before/after microbench for Excel `VALUE`.
//!
//! Compares the allocating cleanup baseline (`excel_value_naive`) with the
//! production kernel (`excel_value`: no-alloc byte walk + stack comma strip).
//!
//! ```text
//! cargo bench -p xlsx-engine-core --bench value
//! ```

use std::hint::black_box;
use std::time::{Duration, Instant};
use xlsx_engine_core::{excel_value, excel_value_naive};
use xlsx_types::DateSystem;

/// Inner-loop count. A single `VALUE` parse is tens of nanoseconds, so
/// `Instant` granularity would dominate without a batch.
const BATCH: u32 = 80_000;

struct Case {
    name: &'static str,
    text: String,
}

fn cases() -> Vec<Case> {
    vec![
        Case {
            name: "plain integer \"123456789\"",
            text: "123456789".into(),
        },
        Case {
            name: "trimmed decimal \"   123.45   \"",
            text: "   123.45   ".into(),
        },
        Case {
            name: "currency \"$1,234,567.89\"",
            text: "$1,234,567.89".into(),
        },
        Case {
            name: "percent \"12.5%\"",
            text: "12.5%".into(),
        },
        Case {
            name: "parens+currency \"($1,234.50)\"",
            text: "($1,234.50)".into(),
        },
        Case {
            name: "scientific \"1.23456789E+10\"",
            text: "1.23456789E+10".into(),
        },
        Case {
            name: "time \"16:48:00\"",
            text: "16:48:00".into(),
        },
        Case {
            name: "date \"12/31/2024\"",
            text: "12/31/2024".into(),
        },
        Case {
            name: "reject \"not-a-number\"",
            text: "not-a-number".into(),
        },
        Case {
            name: "grouped currency \"$123,456,789,012.34\"",
            text: "$123,456,789,012.34".into(),
        },
    ]
}

fn time_it(mut f: impl FnMut()) -> Duration {
    for _ in 0..BATCH {
        f();
    }
    let start = Instant::now();
    for _ in 0..BATCH {
        f();
    }
    start.elapsed() / BATCH
}

fn fmt_dur(d: Duration) -> String {
    let ns = d.as_secs_f64() * 1e9;
    if ns >= 1000.0 {
        format!("{:.1} µs", ns / 1000.0)
    } else {
        format!("{ns:.1} ns")
    }
}

fn main() {
    println!("VALUE kernel bench (allocating cleanup vs no-alloc scan)");
    println!(
        "{:<42} {:>12} {:>12} {:>8}",
        "case", "naive", "optimized", "speedup"
    );
    println!("{}", "-".repeat(78));
    let system = DateSystem::Excel1900;
    for c in cases() {
        let naive = time_it(|| {
            let _ = black_box(excel_value_naive(black_box(&c.text), system));
        });
        let fast = time_it(|| {
            let _ = black_box(excel_value(black_box(&c.text), system));
        });
        let speedup = naive.as_secs_f64() / fast.as_secs_f64().max(1e-12);
        println!(
            "{:<42} {:>12} {:>12} {:>7.1}x",
            c.name,
            fmt_dur(naive),
            fmt_dur(fast),
            speedup
        );
        let a = excel_value_naive(&c.text, system);
        let b = excel_value(&c.text, system);
        assert_eq!(a, b, "semantic mismatch on {}", c.name);
    }
}
