//! Large-concat hill-climb for Excel `CONCAT`.
//!
//! Compares the materializing baseline (`eval_expr` → 2-D `Array` → collect →
//! join), a dense rectangle walk, and the production walk (occupied-cell
//! gather on sparse ranges, stream append). Workloads stay under Excel’s
//! 32,767 UTF-16-unit result cap so the bench measures a completed concat,
//! not the `#VALUE!` overflow path.
//!
//! ```text
//! cargo bench -p xlsx-engine-core --bench concat
//! ```
//!
//! A faster CONCAT that fails `xlsx-verify --candidate calc-core` is not a win.

use std::hint::black_box;
use std::time::{Duration, Instant};
use xlsx_engine_core::{
    concat_naive_join, eval_concat_formula, ConcatBuilder, ConcatWalk, CONCAT_MAX_CHARS,
};
use xlsx_types::{Cell, CellAddr, ExcelValue, Sheet, Workbook};

const N_10K: u32 = 10_000;
const N_50K: u32 = 50_000;
const N_200K: u32 = 200_000;

fn col_a_fixed(n: u32, text: &str, keep: impl Fn(u32) -> bool) -> Workbook {
    let mut sheet = Sheet::new("Sheet1");
    for i in 0..n {
        if !keep(i) {
            continue;
        }
        sheet.insert(
            CellAddr::new(0, i),
            Cell::value(ExcelValue::Text(text.to_string())),
        );
    }
    Workbook {
        sheets: vec![sheet],
        names: vec![],
    }
}

fn two_cols(n: u32, a: &str, b: &str) -> Workbook {
    let mut sheet = Sheet::new("Sheet1");
    for i in 0..n {
        sheet.insert(
            CellAddr::new(0, i),
            Cell::value(ExcelValue::Text(a.to_string())),
        );
        sheet.insert(
            CellAddr::new(1, i),
            Cell::value(ExcelValue::Text(b.to_string())),
        );
    }
    Workbook {
        sheets: vec![sheet],
        names: vec![],
    }
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

fn assert_same(formula: &str, wb: &Workbook) {
    let auto = eval_concat_formula(wb, formula, ConcatWalk::Auto).expect("auto");
    let dense = eval_concat_formula(wb, formula, ConcatWalk::Dense).expect("dense");
    let mat = eval_concat_formula(wb, formula, ConcatWalk::Materialize).expect("materialize");
    assert_eq!(auto, dense, "auto and dense must agree for {formula}");
    assert_eq!(auto, mat, "auto and materialize must agree for {formula}");
    match auto {
        ExcelValue::Text(s) => assert!(
            s.encode_utf16().count() <= CONCAT_MAX_CHARS,
            "{formula} overflowed the Excel cap ({} UTF-16 units)",
            s.encode_utf16().count()
        ),
        ExcelValue::Error(_) => {
            panic!("{formula} returned an error; bench must complete the concat")
        }
        other => panic!("unexpected {other:?}"),
    }
}

fn main() {
    // 8000 × "ab" = 16000 chars (full concat).
    let dense_8k = col_a_fixed(8_000, "ab", |_| true);
    // 10k walk, every 5th blank, "x" → 8000 chars.
    let mixed_10k = col_a_fixed(N_10K, "x", |i| i % 5 != 0);
    // 4k × 2-col "a"/"b" = 8000 chars; exercises row-major flatten.
    let grid_4k = two_cols(4_000, "a", "b");
    // 50k sparse walk: 2% filled ("x") → 1000 chars.
    let sparse_50k = col_a_fixed(N_50K, "x", |i| i % 50 == 0);
    // 200k very sparse: 0.5% filled ("x") → 1000 chars. Occupied gather
    // should beat a 200k dense lookup / 2-D materialize.
    let sparse_200k = col_a_fixed(N_200K, "x", |i| i % 200 == 0);

    let cases: &[(&str, &str, &Workbook, u32)] = &[
        ("8k dense", "=CONCAT(A1:A8000)", &dense_8k, 10),
        ("10k mixed blanks", "=CONCAT(A1:A10000)", &mixed_10k, 10),
        ("4k×2 row-major", "=CONCAT(A1:B4000)", &grid_4k, 10),
        (
            "4k two-range args",
            "=CONCAT(A1:A4000,B1:B4000)",
            &grid_4k,
            10,
        ),
        ("50k sparse", "=CONCAT(A1:A50000)", &sparse_50k, 6),
        ("200k sparse", "=CONCAT(A1:A200000)", &sparse_200k, 4),
    ];

    println!("CONCAT eval bench (materialize / dense walk / occupied auto)");
    println!(
        "{:<24} {:>12} {:>12} {:>12} {:>8}",
        "case", "materialize", "dense", "auto", "vs mat"
    );
    println!("{}", "-".repeat(72));

    for (name, formula, wb, iters) in cases {
        assert_same(formula, wb);
        let mat = time_it(*iters, || {
            black_box(eval_concat_formula(wb, formula, ConcatWalk::Materialize).unwrap());
        });
        let dense = time_it(*iters, || {
            black_box(eval_concat_formula(wb, formula, ConcatWalk::Dense).unwrap());
        });
        let auto = time_it(*iters, || {
            black_box(eval_concat_formula(wb, formula, ConcatWalk::Auto).unwrap());
        });
        let speedup = mat.as_secs_f64() / auto.as_secs_f64();
        println!(
            "{:<24} {:>12} {:>12} {:>12} {:>7.2}×",
            name,
            fmt_dur(mat),
            fmt_dur(dense),
            fmt_dur(auto),
            speedup
        );
    }

    // Kernel-only: collect+concat vs streaming builder (2000 parts, under the cap).
    let parts: Vec<String> = (0..2_000).map(|i| format!("v{i:03}")).collect();
    let refs: Vec<&str> = parts.iter().map(String::as_str).collect();
    let naive = time_it(80, || {
        black_box(concat_naive_join(&refs).unwrap());
    });
    let opt = time_it(80, || {
        let mut b = ConcatBuilder::new();
        b.reserve(2_000 * 5);
        for p in &refs {
            b.push(p).unwrap();
        }
        black_box(b.finish());
    });
    println!();
    println!(
        "kernel 2k parts          naive {}  builder {}  {:.2}×",
        fmt_dur(naive),
        fmt_dur(opt),
        naive.as_secs_f64() / opt.as_secs_f64()
    );
}
