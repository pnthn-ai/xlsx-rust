//! Before/after microbench for Excel `UNICODE`.
//!
//! Compares the `to_text` + `Vec<char>` baseline (`excel_unicode_naive` /
//! `excel_unicode_value_naive`) with the production kernel (`excel_unicode`:
//! O(1) first UTF-8 scalar, no `Text` clone).
//!
//! ```text
//! cargo bench -p xlsx-engine-core --bench unicode
//! ```

use std::hint::black_box;
use std::time::{Duration, Instant};
use xlsx_engine_core::{
    excel_unicode, excel_unicode_naive, excel_unicode_value, excel_unicode_value_naive,
};
use xlsx_types::ExcelValue;

const ITERS_HEAVY: u32 = 40;
const ITERS_LIGHT: u32 = 200;

struct Case {
    name: &'static str,
    text: String,
    iters: u32,
}

fn cases() -> Vec<Case> {
    vec![
        Case {
            name: "200k ASCII 'x' (first byte)",
            text: "x".repeat(200_000),
            iters: ITERS_HEAVY,
        },
        Case {
            name: "200k 'A' + suffix",
            text: format!("A{}", "x".repeat(199_999)),
            iters: ITERS_HEAVY,
        },
        Case {
            name: "200k café-repeat (2-byte lead)",
            text: "é".repeat(100_000),
            iters: ITERS_HEAVY,
        },
        Case {
            name: "200k 中-repeat (3-byte lead)",
            text: "中".repeat(66_667),
            iters: ITERS_HEAVY,
        },
        Case {
            name: "50k 😀-repeat (4-byte lead)",
            text: "😀".repeat(50_000),
            iters: ITERS_HEAVY,
        },
        Case {
            name: "empty text → #VALUE!",
            text: String::new(),
            iters: ITERS_LIGHT,
        },
        Case {
            name: "single ASCII",
            text: "A".into(),
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
    println!("UNICODE kernel bench (Vec<char> walk vs O(1) first scalar)");
    println!(
        "{:<42} {:>12} {:>12} {:>8}",
        "case", "naive", "optimized", "speedup"
    );
    println!("{}", "-".repeat(78));
    for c in cases() {
        let naive = time_it(c.iters, || {
            let _ = black_box(excel_unicode_naive(black_box(&c.text)));
        });
        let fast = time_it(c.iters, || {
            let _ = black_box(excel_unicode(black_box(&c.text)));
        });
        let speedup = naive.as_secs_f64() / fast.as_secs_f64().max(1e-12);
        println!(
            "{:<42} {:>12} {:>12} {:>7.1}x",
            c.name,
            fmt_dur(naive),
            fmt_dur(fast),
            speedup
        );
        let a = excel_unicode_naive(&c.text);
        let b = excel_unicode(&c.text);
        assert_eq!(a, b, "semantic mismatch on {}", c.name);
    }

    println!();
    println!("UNICODE value bench (to_text+Vec<char> vs borrow / first byte)");
    println!(
        "{:<42} {:>12} {:>12} {:>8}",
        "case", "naive", "optimized", "speedup"
    );
    println!("{}", "-".repeat(78));
    let long = ExcelValue::Text("x".repeat(200_000));
    let values = [
        ("200k Text clone vs borrow", long, ITERS_HEAVY),
        ("Number 65 → '6'", ExcelValue::Number(65.0), ITERS_LIGHT),
        ("TRUE → 'T'", ExcelValue::Bool(true), ITERS_LIGHT),
        ("Empty → #VALUE!", ExcelValue::Empty, ITERS_LIGHT),
    ];
    for (name, value, iters) in values {
        let naive = time_it(iters, || {
            let _ = black_box(excel_unicode_value_naive(black_box(&value)));
        });
        let fast = time_it(iters, || {
            let _ = black_box(excel_unicode_value(black_box(&value)));
        });
        let speedup = naive.as_secs_f64() / fast.as_secs_f64().max(1e-12);
        println!(
            "{:<42} {:>12} {:>12} {:>7.1}x",
            name,
            fmt_dur(naive),
            fmt_dur(fast),
            speedup
        );
        let a = excel_unicode_value_naive(&value);
        let b = excel_unicode_value(&value);
        assert_eq!(a, b, "value semantic mismatch on {name}");
    }
}
