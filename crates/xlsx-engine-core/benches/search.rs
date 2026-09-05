//! Before/after microbench for Excel `SEARCH`.
//!
//! Compares the `Vec<char>` try-every-index baseline (`excel_search_naive`)
//! with the production kernel (`excel_search`: ASCII case-insensitive last-byte
//! SWAR, leading-`*` shortcut, first-literal skip).
//!
//! ```text
//! cargo bench -p xlsx-engine-core --bench search
//! ```

use std::hint::black_box;
use std::time::{Duration, Instant};
use xlsx_engine_core::{excel_search, excel_search_naive};

const ITERS_HEAVY: u32 = 20;
const ITERS_LIGHT: u32 = 80;
const ITERS_WILD: u32 = 8;

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
    let wild_miss = "x".repeat(20_000);
    let wild_late = {
        let mut s = "x".repeat(20_000);
        s.push_str("needle");
        s
    };
    vec![
        Case {
            name: "200k 'x' miss 'NEEDLE' (ci)",
            needle: "NEEDLE".into(),
            haystack: miss,
            start_num: 1,
            iters: ITERS_HEAVY,
        },
        Case {
            name: "200k 'x' + needle at end (ci)",
            needle: "NEEDLE".into(),
            haystack: late,
            start_num: 1,
            iters: ITERS_HEAVY,
        },
        Case {
            name: "200k 'x' start_num near end",
            needle: "Z".into(),
            haystack: late_start,
            start_num: 199_000,
            iters: ITERS_LIGHT,
        },
        Case {
            name: "240k 'aaa' + 'AAB' (almost-match)",
            needle: "AAB".into(),
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
            needle: "É".into(),
            haystack: "cafe".repeat(50_000) + "café",
            start_num: 1,
            iters: ITERS_HEAVY,
        },
        Case {
            name: "20k 'x' miss '*NEEDLE' (lead *)",
            needle: "*NEEDLE".into(),
            haystack: wild_miss.clone(),
            start_num: 1,
            iters: ITERS_WILD,
        },
        Case {
            name: "20k 'x' + '*NEEDLE' at end",
            needle: "*NEEDLE".into(),
            haystack: wild_late,
            start_num: 1,
            iters: ITERS_WILD,
        },
        Case {
            name: "20k 'x' miss 'a*b' wildcard",
            needle: "a*b".into(),
            haystack: wild_miss,
            start_num: 1,
            iters: ITERS_WILD,
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
    println!("SEARCH kernel bench (Vec<char> scan vs ci SWAR + leading-*)");
    println!(
        "{:<42} {:>12} {:>12} {:>8}",
        "case", "naive", "optimized", "speedup"
    );
    println!("{}", "-".repeat(78));
    for c in cases() {
        let naive = time_it(c.iters, || {
            let _ = black_box(excel_search_naive(
                black_box(&c.needle),
                black_box(&c.haystack),
                black_box(c.start_num),
            ));
        });
        let fast = time_it(c.iters, || {
            let _ = black_box(excel_search(
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
        let a = excel_search_naive(&c.needle, &c.haystack, c.start_num);
        let b = excel_search(&c.needle, &c.haystack, c.start_num);
        assert_eq!(a, b, "semantic mismatch on {}", c.name);
    }
}
