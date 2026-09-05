//! Before/after microbench for Excel `XLOOKUP`.
//!
//! Compares the flatten-both baseline (`excel_xlookup_naive`) with the
//! production kernel (`excel_xlookup`: in-place scan / binary search, clone
//! only the matched return slice).
//!
//! ```text
//! cargo bench -p xlsx-engine-core --bench xlookup
//! ```

use std::hint::black_box;
use std::time::{Duration, Instant};
use xlsx_engine_core::{excel_xlookup, excel_xlookup_naive, CalcCoreEngine};
use xlsx_types::{Candidate, Cell, EvalSpec, EvalTarget, ExcelValue, Sheet, Workbook};

const ITERS: u32 = 40;
const N: usize = 8_192;
const WIDE: usize = 8;
const PAYLOAD: usize = 32;

struct Case {
    name: &'static str,
    lookup: ExcelValue,
    keys: ExcelValue,
    ret: ExcelValue,
    if_not_found: Option<ExcelValue>,
    match_mode: Option<ExcelValue>,
    search_mode: Option<ExcelValue>,
}

fn n(x: f64) -> ExcelValue {
    ExcelValue::Number(x)
}
fn t(s: impl Into<String>) -> ExcelValue {
    ExcelValue::Text(s.into())
}

fn col_num(len: usize, f: impl Fn(usize) -> f64) -> ExcelValue {
    ExcelValue::Array((0..len).map(|i| vec![n(f(i))]).collect())
}

fn col_text(len: usize, f: impl Fn(usize) -> String) -> ExcelValue {
    ExcelValue::Array((0..len).map(|i| vec![t(f(i))]).collect())
}

fn matrix_text(rows: usize, cols: usize) -> ExcelValue {
    ExcelValue::Array(
        (0..rows)
            .map(|r| {
                (0..cols)
                    .map(|c| t(format!("{r:04}-{c}-{}", "x".repeat(PAYLOAD))))
                    .collect()
            })
            .collect(),
    )
}

fn cases() -> Vec<Case> {
    let sorted_keys = col_num(N, |i| i as f64);
    let sorted_ret = col_num(N, |i| (i * 3) as f64);
    let fat_ret = matrix_text(N, WIDE);
    let text_keys = col_text(N, |i| format!("k{i:05}"));
    let text_ret = col_text(N, |i| format!("v{i:05}-{}", "y".repeat(PAYLOAD)));
    vec![
        Case {
            name: "8k exact, hit last (linear)",
            lookup: n((N - 1) as f64),
            keys: sorted_keys.clone(),
            ret: sorted_ret.clone(),
            if_not_found: Some(t("miss")),
            match_mode: Some(n(0.0)),
            search_mode: Some(n(1.0)),
        },
        Case {
            name: "8k exact, hit first, search -1",
            lookup: n(0.0),
            keys: sorted_keys.clone(),
            ret: sorted_ret.clone(),
            if_not_found: Some(t("miss")),
            match_mode: Some(n(0.0)),
            search_mode: Some(n(-1.0)),
        },
        Case {
            name: "8k exact miss (linear)",
            lookup: n(-1.0),
            keys: sorted_keys.clone(),
            ret: sorted_ret.clone(),
            if_not_found: Some(t("miss")),
            match_mode: Some(n(0.0)),
            search_mode: Some(n(1.0)),
        },
        Case {
            name: "8k exact, binary search mid",
            lookup: n((N / 2) as f64),
            keys: sorted_keys.clone(),
            ret: sorted_ret.clone(),
            if_not_found: Some(t("miss")),
            match_mode: Some(n(0.0)),
            search_mode: Some(n(2.0)),
        },
        Case {
            name: "8k next-smaller, binary",
            lookup: n((N / 2) as f64 + 0.5),
            keys: sorted_keys.clone(),
            ret: sorted_ret.clone(),
            if_not_found: Some(t("miss")),
            match_mode: Some(n(-1.0)),
            search_mode: Some(n(2.0)),
        },
        Case {
            name: "8k×8 text return, exact last",
            lookup: n((N - 1) as f64),
            keys: sorted_keys.clone(),
            ret: fat_ret,
            if_not_found: Some(t("miss")),
            match_mode: Some(n(0.0)),
            search_mode: Some(n(1.0)),
        },
        Case {
            name: "8k text wildcard last a*",
            lookup: t("k081*"),
            keys: text_keys,
            ret: text_ret,
            if_not_found: Some(t("miss")),
            match_mode: Some(n(2.0)),
            search_mode: Some(n(1.0)),
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

fn evaluate_bench(n: u32, iters: u32) {
    let mut sheet = Sheet::new("Sheet1");
    for i in 0..n {
        sheet.cells.insert(
            format!("A{}", i + 1),
            Cell::value(ExcelValue::Number(i as f64)),
        );
        sheet.cells.insert(
            format!("B{}", i + 1),
            Cell::value(ExcelValue::Number((i * 3) as f64)),
        );
    }
    let wb = Workbook {
        sheets: vec![sheet],
        names: vec![],
    };
    let hit = n / 2;
    let formula = format!("=XLOOKUP({hit}, A1:A{n}, B1:B{n}, \"miss\", 0, 2)");
    let spec = EvalSpec {
        case_id: "bench.xlookup".into(),
        workbook: wb,
        target: EvalTarget::formula(formula),
        options: Default::default(),
    };
    let engine = CalcCoreEngine::new();
    let got = engine.evaluate(&spec).expect("evaluate");
    assert_eq!(got, ExcelValue::Number((hit * 3) as f64), "binary XLOOKUP");
    let ms = time_it(iters, || {
        let _ = black_box(engine.evaluate(black_box(&spec)).unwrap());
    });
    println!(
        "evaluate n={n:>6}  {}/call  formula=XLOOKUP(mid, A1:A{n}, B1:B{n}, …, 0, 2)",
        fmt_dur(ms)
    );
}

fn main() {
    println!("XLOOKUP kernel bench (flatten-both vs in-place / binary)");
    println!(
        "{:<42} {:>12} {:>12} {:>8}",
        "case", "naive", "optimized", "speedup"
    );
    println!("{}", "-".repeat(80));
    for c in cases() {
        let inf = c.if_not_found.as_ref();
        let mm = c.match_mode.as_ref();
        let sm = c.search_mode.as_ref();
        let naive = time_it(ITERS, || {
            let _ = black_box(excel_xlookup_naive(
                black_box(&c.lookup),
                black_box(&c.keys),
                black_box(&c.ret),
                black_box(inf),
                black_box(mm),
                black_box(sm),
            ));
        });
        let fast = time_it(ITERS, || {
            let _ = black_box(excel_xlookup(
                black_box(&c.lookup),
                black_box(&c.keys),
                black_box(&c.ret),
                black_box(inf),
                black_box(mm),
                black_box(sm),
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
        let a = excel_xlookup_naive(&c.lookup, &c.keys, &c.ret, inf, mm, sm);
        let b = excel_xlookup(&c.lookup, &c.keys, &c.ret, inf, mm, sm);
        assert_eq!(a, b, "semantic mismatch on {}", c.name);
    }
    println!();
    evaluate_bench(10_000, 8);
    evaluate_bench(100_000, 4);
}
