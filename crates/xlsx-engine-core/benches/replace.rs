//! Before/after microbench for Excel `REPLACE`.
//!
//! Compares the `Vec<char>` baseline (`excel_replace_naive`) with the
//! specialized production kernel (`excel_replace`) across string sizes,
//! plus the value-level `to_text` clone vs borrow-`Text` path.
//!
//! ```text
//! cargo bench -p xlsx-engine-core --bench replace
//! ```

use std::hint::black_box;
use std::time::{Duration, Instant};
use xlsx_engine_core::{
    excel_replace, excel_replace_naive, excel_replace_value, excel_replace_value_naive,
};
use xlsx_types::ExcelValue;

const ITERS_HEAVY: u32 = 16;
const ITERS_LIGHT: u32 = 40;

struct Case {
    name: &'static str,
    old: String,
    start: u64,
    num_chars: u64,
    new: String,
    iters: u32,
}

fn cases() -> Vec<Case> {
    let ascii_200k = "a".repeat(200_000);
    let ascii_mid = 100_000u64;
    vec![
        Case {
            name: "200k ASCII replace 1 mid (same width)",
            old: ascii_200k.clone(),
            start: ascii_mid,
            num_chars: 1,
            new: "b".into(),
            iters: ITERS_HEAVY,
        },
        Case {
            name: "200k ASCII shrink mid 50k → 8",
            old: ascii_200k.clone(),
            start: 75_000,
            num_chars: 50_000,
            new: "REPLACED".into(),
            iters: ITERS_HEAVY,
        },
        Case {
            name: "200k ASCII grow mid 8 → 50k",
            old: ascii_200k.clone(),
            start: ascii_mid,
            num_chars: 8,
            new: "x".repeat(50_000),
            iters: ITERS_HEAVY,
        },
        Case {
            name: "200k ASCII delete mid 50k",
            old: ascii_200k.clone(),
            start: 75_000,
            num_chars: 50_000,
            new: String::new(),
            iters: ITERS_HEAVY,
        },
        Case {
            name: "200k ASCII insert 1k at mid",
            old: ascii_200k.clone(),
            start: ascii_mid,
            num_chars: 0,
            new: "y".repeat(1_000),
            iters: ITERS_HEAVY,
        },
        Case {
            name: "200k ASCII append (start past end)",
            old: ascii_200k.clone(),
            start: 200_001,
            num_chars: 1,
            new: "Z".into(),
            iters: ITERS_LIGHT,
        },
        Case {
            name: "200k ASCII no-op (num=0, empty new)",
            old: ascii_200k.clone(),
            start: 1,
            num_chars: 0,
            new: String::new(),
            iters: ITERS_LIGHT,
        },
        Case {
            name: "50k 'é' replace mid scalar",
            old: "é".repeat(50_000),
            start: 25_000,
            num_chars: 1,
            new: "e".into(),
            iters: ITERS_HEAVY,
        },
        Case {
            name: "10k emoji replace one scalar",
            old: "😀".repeat(10_000),
            start: 5_000,
            num_chars: 1,
            new: "X".into(),
            iters: ITERS_LIGHT,
        },
        Case {
            name: "200k ASCII whole replace (start 1)",
            old: ascii_200k.clone(),
            start: 1,
            num_chars: 200_000,
            new: "Z".into(),
            iters: ITERS_LIGHT,
        },
        Case {
            name: "200k ASCII prefix insert (start 1, n=0)",
            old: ascii_200k,
            start: 1,
            num_chars: 0,
            new: "Y".into(),
            iters: ITERS_LIGHT,
        },
        Case {
            name: "10k emoji equal-width overwrite",
            old: "😀".repeat(10_000),
            start: 5_000,
            num_chars: 1,
            new: "🎉".into(),
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
    println!("REPLACE kernel bench (Vec<char> naive vs specialized)");
    println!(
        "{:<42} {:>12} {:>12} {:>8}",
        "case", "naive", "optimized", "speedup"
    );
    println!("{}", "-".repeat(78));
    for c in cases() {
        let naive = time_it(c.iters, || {
            black_box(excel_replace_naive(
                black_box(&c.old),
                black_box(c.start),
                black_box(c.num_chars),
                black_box(&c.new),
            ));
        });
        let fast = time_it(c.iters, || {
            black_box(excel_replace(
                black_box(&c.old),
                black_box(c.start),
                black_box(c.num_chars),
                black_box(&c.new),
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
        let a = excel_replace_naive(&c.old, c.start, c.num_chars, &c.new);
        let b = excel_replace(&c.old, c.start, c.num_chars, &c.new);
        assert_eq!(a, b, "semantic mismatch on {}", c.name);
    }

    println!();
    println!("REPLACE value bench (to_text+Vec<char> vs borrow-Text kernel)");
    println!(
        "{:<42} {:>12} {:>12} {:>8}",
        "case", "naive", "optimized", "speedup"
    );
    println!("{}", "-".repeat(78));
    let long = ExcelValue::Text("a".repeat(200_000));
    let start = ExcelValue::Number(100_000.0);
    let one = ExcelValue::Number(1.0);
    let new_b = ExcelValue::Text("b".into());
    let values = [
        (
            "200k Text mid 1 (borrow)",
            long,
            start,
            one,
            new_b,
            ITERS_HEAVY,
        ),
        (
            "Number 2009 → 2010",
            ExcelValue::Number(2009.0),
            ExcelValue::Number(3.0),
            ExcelValue::Number(2.0),
            ExcelValue::Number(10.0),
            ITERS_LIGHT,
        ),
        (
            "TRUE first char",
            ExcelValue::Bool(true),
            ExcelValue::Number(1.0),
            ExcelValue::Number(1.0),
            ExcelValue::Text("X".into()),
            ITERS_LIGHT,
        ),
    ];
    for (name, old, start, num, new, iters) in values {
        let naive = time_it(iters, || {
            let _ = black_box(excel_replace_value_naive(
                black_box(&old),
                black_box(&start),
                black_box(&num),
                black_box(&new),
            ));
        });
        let fast = time_it(iters, || {
            let _ = black_box(excel_replace_value(
                black_box(&old),
                black_box(&start),
                black_box(&num),
                black_box(&new),
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
        let a = excel_replace_value_naive(&old, &start, &num, &new);
        let b = excel_replace_value(&old, &start, &num, &new);
        assert_eq!(a, b, "value semantic mismatch on {name}");
    }
}
