//! Large-join hill-climb for Excel `TEXTJOIN`.
//!
//! Compares the materializing baseline (`eval_expr` → 2-D `Array` → collect →
//! join), a dense rectangle walk, and the production walk (occupied-cell
//! gather on sparse ranges when `ignore_empty` is TRUE, stream append).
//! Workloads stay under Excel’s 32,767 UTF-16-unit result cap so the bench
//! measures a completed join, not the `#VALUE!` overflow path.
//!
//! ```text
//! cargo bench -p xlsx-engine-core --bench textjoin
//! ```
//!
//! A faster TEXTJOIN that fails `xlsx-verify --candidate calc-core` is not a win.

use std::hint::black_box;
use std::time::{Duration, Instant};
use xlsx_engine_core::{
    eval_textjoin_formula, textjoin_naive_join, TextJoinBuilder, TextJoinWalk, TEXTJOIN_MAX_CHARS,
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

fn cycle_delims() -> Workbook {
    let mut sheet = Sheet::new("Sheet1");
    // 4000 × "ab" + cycling ","/";" stays well under 32767.
    for i in 0..4000u32 {
        sheet.insert(
            CellAddr::new(0, i),
            Cell::value(ExcelValue::Text("ab".into())),
        );
    }
    sheet.insert(
        CellAddr::new(1, 0),
        Cell::value(ExcelValue::Text(",".into())),
    );
    sheet.insert(
        CellAddr::new(2, 0),
        Cell::value(ExcelValue::Text(";".into())),
    );
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
    let auto = eval_textjoin_formula(wb, formula, TextJoinWalk::Auto).expect("auto");
    let dense = eval_textjoin_formula(wb, formula, TextJoinWalk::Dense).expect("dense");
    let mat = eval_textjoin_formula(wb, formula, TextJoinWalk::Materialize).expect("materialize");
    assert_eq!(auto, dense, "auto and dense must agree for {formula}");
    assert_eq!(auto, mat, "auto and materialize must agree for {formula}");
    match auto {
        ExcelValue::Text(s) => assert!(
            s.encode_utf16().count() <= TEXTJOIN_MAX_CHARS,
            "{formula} overflowed the Excel cap ({} UTF-16 units)",
            s.encode_utf16().count()
        ),
        ExcelValue::Error(_) => panic!("{formula} returned an error; bench must complete the join"),
        other => panic!("unexpected {other:?}"),
    }
}

fn main() {
    // 8000 × "ab" + empty delim = 16000 chars (full join).
    let dense_join = col_a_fixed(8_000, "ab", |_| true);
    // 10k walk, every 5th blank, empty delim, "x" → 8000 chars.
    let mixed_10k = col_a_fixed(N_10K, "x", |i| i % 5 != 0);
    // 4k × 2-col "a"/"b" + empty delim = 8000 chars; exercises row-major flatten.
    let grid_4k = two_cols(4_000, "a", "b");
    // 50k sparse walk: 2% filled ("x"), empty delim → 1000 chars.
    let sparse_50k = col_a_fixed(N_50K, "x", |i| i % 50 == 0);
    // 200k very sparse: 0.5% filled ("x") → 1000 chars. Occupied gather
    // should beat a 200k dense lookup / 2-D materialize when ignore TRUE.
    let sparse_200k = col_a_fixed(N_200K, "x", |i| i % 200 == 0);
    let cyc = cycle_delims();

    let cases: &[(&str, &str, &Workbook, u32)] = &[
        (
            "8k dense empty-delim",
            "=TEXTJOIN(\"\",TRUE,A1:A8000)",
            &dense_join,
            10,
        ),
        (
            "10k mixed ignore TRUE",
            "=TEXTJOIN(\"\",TRUE,A1:A10000)",
            &mixed_10k,
            10,
        ),
        (
            "10k mixed ignore FALSE",
            "=TEXTJOIN(\"\",FALSE,A1:A10000)",
            &mixed_10k,
            10,
        ),
        (
            "4k×2 row-major",
            "=TEXTJOIN(\"\",TRUE,A1:B4000)",
            &grid_4k,
            10,
        ),
        (
            "4k cycling delimiters",
            "=TEXTJOIN(B1:C1,TRUE,A1:A4000)",
            &cyc,
            10,
        ),
        (
            "50k sparse ignore TRUE",
            "=TEXTJOIN(\"\",TRUE,A1:A50000)",
            &sparse_50k,
            6,
        ),
        (
            "50k sparse ignore FALSE",
            "=TEXTJOIN(\"\",FALSE,A1:A50000)",
            &sparse_50k,
            6,
        ),
        (
            "200k sparse ignore TRUE",
            "=TEXTJOIN(\"\",TRUE,A1:A200000)",
            &sparse_200k,
            4,
        ),
    ];

    println!("TEXTJOIN eval bench (materialize / dense walk / occupied auto)");
    println!(
        "{:<28} {:>12} {:>12} {:>12} {:>8}",
        "case", "materialize", "dense", "auto", "vs mat"
    );
    println!("{}", "-".repeat(76));

    for (name, formula, wb, iters) in cases {
        assert_same(formula, wb);
        let mat = time_it(*iters, || {
            black_box(eval_textjoin_formula(wb, formula, TextJoinWalk::Materialize).unwrap());
        });
        let dense = time_it(*iters, || {
            black_box(eval_textjoin_formula(wb, formula, TextJoinWalk::Dense).unwrap());
        });
        let auto = time_it(*iters, || {
            black_box(eval_textjoin_formula(wb, formula, TextJoinWalk::Auto).unwrap());
        });
        let speedup = mat.as_secs_f64() / auto.as_secs_f64();
        println!(
            "{:<28} {:>12} {:>12} {:>12} {:>7.2}×",
            name,
            fmt_dur(mat),
            fmt_dur(dense),
            fmt_dur(auto),
            speedup
        );
    }

    // Kernel-only: collect+join vs streaming builder (2000 parts, under the cap).
    let parts: Vec<String> = (0..2_000).map(|i| format!("v{i:03}")).collect();
    let refs: Vec<&str> = parts.iter().map(String::as_str).collect();
    let naive = time_it(80, || {
        black_box(textjoin_naive_join(",", &refs, true).unwrap());
    });
    let opt = time_it(80, || {
        let mut b = TextJoinBuilder::new(vec![",".into()]);
        b.reserve(2_000 * 5);
        for p in &refs {
            b.push(p, true).unwrap();
        }
        black_box(b.finish());
    });
    println!();
    println!(
        "kernel 2k single-delim    naive {}  builder {}  {:.2}×",
        fmt_dur(naive),
        fmt_dur(opt),
        naive.as_secs_f64() / opt.as_secs_f64()
    );
}
