//! Before/after microbench for Excel `TEXTAFTER`.
//!
//! Compares the `Vec<char>` sliding-window baseline (`excel_textafter_naive`)
//! with the production kernel (`excel_textafter`: `str::find` / `rfind`,
//! ASCII last-byte SWAR, early-exit on the first/last instance).
//!
//! ```text
//! cargo bench -p xlsx-engine-core --bench textafter
//! ```

use std::hint::black_box;
use std::time::{Duration, Instant};
use xlsx_engine_core::{excel_textafter, excel_textafter_naive};

const ITERS_HEAVY: u32 = 20;
const ITERS_LIGHT: u32 = 80;

struct Case {
    name: &'static str,
    text: String,
    delim: String,
    instance_num: i64,
    ignore_case: bool,
    match_end: bool,
    iters: u32,
}

fn cases() -> Vec<Case> {
    let miss = "x".repeat(200_000);
    let late = {
        let mut s = "x".repeat(200_000);
        s.push_str("needleMORE");
        s
    };
    let many = {
        let mut s = String::new();
        for i in 0..20_000 {
            s.push_str("tok");
            s.push_str(&i.to_string());
            s.push('-');
        }
        s.push_str("tail");
        s
    };
    let almost = {
        let mut s = "aaa".repeat(80_000);
        s.push_str("aabEND");
        s
    };
    vec![
        Case {
            name: "200k 'x' miss 'needle'",
            text: miss.clone(),
            delim: "needle".into(),
            instance_num: 1,
            ignore_case: false,
            match_end: false,
            iters: ITERS_HEAVY,
        },
        Case {
            name: "200k 'x' + needle at end",
            text: late,
            delim: "needle".into(),
            instance_num: 1,
            ignore_case: false,
            match_end: false,
            iters: ITERS_HEAVY,
        },
        Case {
            name: "200k 'x' miss + match_end",
            text: miss.clone(),
            delim: "needle".into(),
            instance_num: 1,
            ignore_case: false,
            match_end: true,
            iters: ITERS_HEAVY,
        },
        Case {
            name: "20k '-' last instance",
            text: many,
            delim: "-".into(),
            instance_num: -1,
            ignore_case: false,
            match_end: false,
            iters: ITERS_LIGHT,
        },
        Case {
            name: "240k 'aaa' + 'aab' (almost-match)",
            text: almost,
            delim: "aab".into(),
            instance_num: 1,
            ignore_case: false,
            match_end: false,
            iters: ITERS_HEAVY,
        },
        Case {
            name: "200k empty delimiter",
            text: "x".repeat(200_000),
            delim: String::new(),
            instance_num: 1,
            ignore_case: false,
            match_end: false,
            iters: ITERS_LIGHT,
        },
        Case {
            name: "200k café-repeat unicode hit",
            text: "cafe".repeat(50_000) + "caféEND",
            delim: "é".into(),
            instance_num: 1,
            ignore_case: false,
            match_end: false,
            iters: ITERS_HEAVY,
        },
        Case {
            name: "200k miss NEEDLE casefold",
            text: miss,
            delim: "NEEDLE".into(),
            instance_num: 1,
            ignore_case: true,
            match_end: false,
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
    println!("TEXTAFTER kernel bench (Vec<char> window vs find/rfind + ASCII SWAR)");
    println!(
        "{:<42} {:>12} {:>12} {:>8}",
        "case", "naive", "optimized", "speedup"
    );
    println!("{}", "-".repeat(78));
    for c in cases() {
        let delims = [c.delim.as_str()];
        let naive = time_it(c.iters, || {
            let _ = black_box(excel_textafter_naive(
                black_box(&c.text),
                black_box(&delims),
                black_box(c.instance_num),
                black_box(c.ignore_case),
                black_box(c.match_end),
            ));
        });
        let fast = time_it(c.iters, || {
            let _ = black_box(excel_textafter(
                black_box(&c.text),
                black_box(&delims),
                black_box(c.instance_num),
                black_box(c.ignore_case),
                black_box(c.match_end),
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
        let a = excel_textafter_naive(&c.text, &delims, c.instance_num, c.ignore_case, c.match_end);
        let b = excel_textafter(&c.text, &delims, c.instance_num, c.ignore_case, c.match_end);
        assert_eq!(a, b, "semantic mismatch on {}", c.name);
    }
}
