//! Before/after microbench for Excel `MID`.
//!
//! Compares the `Vec<char>` baseline (`excel_mid_naive` /
//! `excel_mid_value_naive`) with the specialized production kernel
//! (`excel_mid`: ASCII O(1) slice / UTF-8 span walk, no full-string
//! `Vec<char>`).
//!
//! ```text
//! cargo bench -p xlsx-engine-core --bench mid
//! ```

use std::hint::black_box;
use std::time::{Duration, Instant};
use xlsx_engine_core::{excel_mid, excel_mid_naive, excel_mid_value, excel_mid_value_naive};
use xlsx_types::ExcelValue;

const ITERS_HEAVY: u32 = 16;
const ITERS_LIGHT: u32 = 40;

struct Case {
    name: &'static str,
    text: String,
    start: u64,
    num_chars: u64,
    iters: u32,
}

fn cases() -> Vec<Case> {
    let ascii_200k = "a".repeat(200_000);
    let ascii_mid = 100_000u64;
    vec![
        Case {
            name: "200k ASCII mid 1 char",
            text: ascii_200k.clone(),
            start: ascii_mid,
            num_chars: 1,
            iters: ITERS_HEAVY,
        },
        Case {
            name: "200k ASCII mid 50k remainder-ish",
            text: ascii_200k.clone(),
            start: 75_000,
            num_chars: 50_000,
            iters: ITERS_HEAVY,
        },
        Case {
            name: "200k ASCII prefix (start 1, 8)",
            text: ascii_200k.clone(),
            start: 1,
            num_chars: 8,
            iters: ITERS_HEAVY,
        },
        Case {
            name: "200k ASCII start past end",
            text: ascii_200k.clone(),
            start: 200_001,
            num_chars: 1,
            iters: ITERS_LIGHT,
        },
        Case {
            name: "200k ASCII num_chars 0",
            text: ascii_200k,
            start: 1,
            num_chars: 0,
            iters: ITERS_LIGHT,
        },
        Case {
            name: "50k 'é' mid scalar",
            text: "é".repeat(50_000),
            start: 25_000,
            num_chars: 1,
            iters: ITERS_HEAVY,
        },
        Case {
            name: "10k emoji mid one scalar",
            text: "😀".repeat(10_000),
            start: 5_000,
            num_chars: 1,
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
    println!("MID kernel bench (Vec<char> naive vs specialized)");
    println!(
        "{:<42} {:>12} {:>12} {:>8}",
        "case", "naive", "optimized", "speedup"
    );
    println!("{}", "-".repeat(78));
    for c in cases() {
        let naive = time_it(c.iters, || {
            black_box(excel_mid_naive(
                black_box(&c.text),
                black_box(c.start),
                black_box(c.num_chars),
            ));
        });
        let fast = time_it(c.iters, || {
            black_box(excel_mid(
                black_box(&c.text),
                black_box(c.start),
                black_box(c.num_chars),
            ));
        });
        let speedup = naive.as_secs_f64() / fast.as_secs_f64().max(1e-12);
        println!(
            "{:<42} {:>12} {:>12} {:>7.1}x",
            c.name,
            fmt_dur(naive),
            fmt_dur(fast),
            speedup
        );
        let a = excel_mid_naive(&c.text, c.start, c.num_chars);
        let b = excel_mid(&c.text, c.start, c.num_chars);
        assert_eq!(a, b, "semantic mismatch on {}", c.name);
    }

    println!();
    println!("MID value bench (to_text+Vec<char> vs borrow / slice)");
    println!(
        "{:<42} {:>12} {:>12} {:>8}",
        "case", "naive", "optimized", "speedup"
    );
    println!("{}", "-".repeat(78));
    let long = ExcelValue::Text("x".repeat(200_000));
    let values = [
        ("200k Text mid 1", long, 100_000u64, 1u64, ITERS_HEAVY),
        (
            "Number 12345 mid 2,3",
            ExcelValue::Number(12345.0),
            2,
            3,
            ITERS_LIGHT,
        ),
        ("TRUE mid 1,1", ExcelValue::Bool(true), 1, 1, ITERS_LIGHT),
        ("Empty → \"\"", ExcelValue::Empty, 1, 1, ITERS_LIGHT),
    ];
    for (name, value, start, num, iters) in values {
        let naive = time_it(iters, || {
            let _ = black_box(excel_mid_value_naive(
                black_box(&value),
                black_box(start),
                black_box(num),
            ));
        });
        let fast = time_it(iters, || {
            let _ = black_box(excel_mid_value(
                black_box(&value),
                black_box(start),
                black_box(num),
            ));
        });
        let speedup = naive.as_secs_f64() / fast.as_secs_f64().max(1e-12);
        println!(
            "{:<42} {:>12} {:>12} {:>7.1}x",
            name,
            fmt_dur(naive),
            fmt_dur(fast),
            speedup
        );
        let a = excel_mid_value_naive(&value, start, num);
        let b = excel_mid_value(&value, start, num);
        assert_eq!(a, b, "value semantic mismatch on {name}");
    }
}
