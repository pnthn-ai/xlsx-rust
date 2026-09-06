//! Before/after microbench for Excel `FIND`.
//!
//! Compares the `Vec<char>` sliding-window baseline (`excel_find_naive` /
//! `excel_find_value_naive`) with the production kernel (`excel_find`:
//! 1-byte ASCII memchr + last-byte SWAR + `str::find` + ASCII index;
//! `excel_find_value` borrows `Text` instead of `to_text` clone).
//!
//! ```text
//! cargo bench -p xlsx-engine-core --bench find
//! ```

use std::hint::black_box;
use std::time::{Duration, Instant};
use xlsx_engine_core::{excel_find, excel_find_naive, excel_find_value, excel_find_value_naive};
use xlsx_types::ExcelValue;

const ITERS_HEAVY: u32 = 20;
const ITERS_LIGHT: u32 = 80;

struct Case {
    name: &'static str,
    needle: String,
    haystack: String,
    start_num: i64,
    iters: u32,
}

fn cases() -> Vec<Case> {
    let miss = "x".repeat(200_000);
    let late = {
        let mut s = "x".repeat(200_000);
        s.push_str("needle");
        s
    };
    let late_start = {
        let mut s = "x".repeat(200_000);
        s.push('z');
        s
    };
    let almost = {
        let mut s = "aaa".repeat(80_000);
        s.push_str("aab");
        s
    };
    vec![
        Case {
            name: "200k 'x' miss 'needle'",
            needle: "needle".into(),
            haystack: miss,
            start_num: 1,
            iters: ITERS_HEAVY,
        },
        Case {
            name: "200k 'x' + needle at end",
            needle: "needle".into(),
            haystack: late,
            start_num: 1,
            iters: ITERS_HEAVY,
        },
        Case {
            name: "200k 'x' start_num near end",
            needle: "z".into(),
            haystack: late_start,
            start_num: 199_000,
            iters: ITERS_LIGHT,
        },
        Case {
            name: "200k 'x' + 'z' (1-byte ASCII)",
            needle: "z".into(),
            haystack: {
                let mut s = "x".repeat(200_000);
                s.push('z');
                s
            },
            start_num: 1,
            iters: ITERS_HEAVY,
        },
        Case {
            name: "240k 'aaa' + 'aab' (almost-match)",
            needle: "aab".into(),
            haystack: almost,
            start_num: 1,
            iters: ITERS_HEAVY,
        },
        Case {
            name: "200k empty find_text",
            needle: String::new(),
            haystack: "x".repeat(200_000),
            start_num: 50_000,
            iters: ITERS_LIGHT,
        },
        Case {
            name: "200k café-repeat unicode hit",
            needle: "é".into(),
            haystack: "cafe".repeat(50_000) + "café",
            start_num: 1,
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
    println!("FIND kernel bench (Vec<char> window vs str::find / ASCII memchr)");
    println!(
        "{:<42} {:>12} {:>12} {:>8}",
        "case", "naive", "optimized", "speedup"
    );
    println!("{}", "-".repeat(78));
    for c in cases() {
        let naive = time_it(c.iters, || {
            let _ = black_box(excel_find_naive(
                black_box(&c.needle),
                black_box(&c.haystack),
                black_box(c.start_num),
            ));
        });
        let fast = time_it(c.iters, || {
            let _ = black_box(excel_find(
                black_box(&c.needle),
                black_box(&c.haystack),
                black_box(c.start_num),
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
        let a = excel_find_naive(&c.needle, &c.haystack, c.start_num);
        let b = excel_find(&c.needle, &c.haystack, c.start_num);
        assert_eq!(a, b, "semantic mismatch on {}", c.name);
    }

    println!();
    println!("FIND value path (to_text clone vs Text borrow)");
    println!(
        "{:<42} {:>12} {:>12} {:>8}",
        "case", "naive", "optimized", "speedup"
    );
    println!("{}", "-".repeat(78));
    let long = "x".repeat(200_000) + "needle";
    let value_cases: [(&str, ExcelValue, ExcelValue, i64, u32); 4] = [
        (
            "200k Text borrow hit",
            ExcelValue::Text("needle".into()),
            ExcelValue::Text(long),
            1,
            ITERS_HEAVY,
        ),
        (
            "TRUE in TRUEBLUE (bool borrow)",
            ExcelValue::Bool(true),
            ExcelValue::Text("TRUEBLUE".into()),
            1,
            ITERS_LIGHT,
        ),
        (
            "empty needle vs 200k Text",
            ExcelValue::Empty,
            ExcelValue::Text("x".repeat(200_000)),
            50_000,
            ITERS_LIGHT,
        ),
        (
            "int 2 in 12321 (format_plain)",
            ExcelValue::Number(2.0),
            ExcelValue::Number(12321.0),
            1,
            ITERS_LIGHT,
        ),
    ];
    for (name, needle, hay, start, iters) in value_cases {
        let naive = time_it(iters, || {
            let _ = black_box(excel_find_value_naive(
                black_box(&needle),
                black_box(&hay),
                black_box(start),
            ));
        });
        let fast = time_it(iters, || {
            let _ = black_box(excel_find_value(
                black_box(&needle),
                black_box(&hay),
                black_box(start),
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
        let a = excel_find_value_naive(&needle, &hay, start);
        let b = excel_find_value(&needle, &hay, start);
        assert_eq!(a, b, "value-path semantic mismatch on {name}");
    }
}
