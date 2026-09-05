//! Large-join hill-climb for Excel `TEXTJOIN`.
//!
//! Compares the materializing baseline (`eval_expr` → 2-D `Array` → collect →
//! join) with the production walk (sheet-direct read, stream append).
//!
//! ```text
//! cargo bench -p xlsx-engine-core --bench textjoin
//! ```
//!
//! A faster TEXTJOIN that fails `xlsx-verify --candidate calc-core` is not a win.

use std::hint::black_box;
use std::time::{Duration, Instant};
use xlsx_engine_core::{eval_textjoin_formula, textjoin_naive_join};
use xlsx_types::{Cell, CellAddr, ExcelValue, Sheet, Workbook};

const N_10K: u32 = 10_000;
const N_50K: u32 = 50_000;

fn col_a(n: u32, every_fifth_blank: bool) -> Workbook {
    let mut sheet = Sheet::new("Sheet1");
    for i in 0..n {
        if every_fifth_blank && i % 5 == 0 {
            continue;
        }
        sheet.insert(
            CellAddr::new(0, i),
            Cell::value(ExcelValue::Text(format!("v{i}"))),
        );
    }
    Workbook {
        sheets: vec![sheet],
        names: vec![],
    }
}

fn cycle_delims() -> Workbook {
    let mut sheet = Sheet::new("Sheet1");
    for i in 0..N_10K {
        sheet.insert(
            CellAddr::new(0, i),
            Cell::value(ExcelValue::Text(format!("v{i}"))),
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
    let walk = eval_textjoin_formula(wb, formula, false).expect("walk");
    let mat = eval_textjoin_formula(wb, formula, true).expect("materialize");
    assert_eq!(walk, mat, "walk and materialize must agree for {formula}");
}

fn main() {
    let dense = col_a(N_10K, false);
    let mixed = col_a(N_10K, true);
    let dense_50k = col_a(N_50K, false);
    let cyc = cycle_delims();

    let cases: &[(&str, &str, &Workbook, u32)] = &[
        (
            "10k dense ignore TRUE",
            "=TEXTJOIN(\",\",TRUE,A1:A10000)",
            &dense,
            8,
        ),
        (
            "10k mixed ignore TRUE",
            "=TEXTJOIN(\",\",TRUE,A1:A10000)",
            &mixed,
            8,
        ),
        (
            "10k mixed ignore FALSE",
            "=TEXTJOIN(\",\",FALSE,A1:A10000)",
            &mixed,
            8,
        ),
        (
            "10k cycling delimiters",
            "=TEXTJOIN(B1:C1,TRUE,A1:A10000)",
            &cyc,
            8,
        ),
        (
            "50k dense ignore TRUE",
            "=TEXTJOIN(\",\",TRUE,A1:A50000)",
            &dense_50k,
            4,
        ),
    ];

    println!("TEXTJOIN eval bench (materialize 2-D array vs walk+stream)");
    println!(
        "{:<28} {:>12} {:>12} {:>8}",
        "case", "materialize", "walk", "speedup"
    );
    println!("{}", "-".repeat(64));

    for (name, formula, wb, iters) in cases {
        assert_same(formula, wb);
        let mat = time_it(*iters, || {
            black_box(eval_textjoin_formula(wb, formula, true).unwrap());
        });
        let walk = time_it(*iters, || {
            black_box(eval_textjoin_formula(wb, formula, false).unwrap());
        });
        let speedup = mat.as_secs_f64() / walk.as_secs_f64();
        println!(
            "{:<28} {:>12} {:>12} {:>7.2}×",
            name,
            fmt_dur(mat),
            fmt_dur(walk),
            speedup
        );
    }

    // Kernel-only: collect+join vs streaming builder (single delimiter).
    let parts: Vec<String> = (0..10_000).map(|i| format!("v{i}")).collect();
    let refs: Vec<&str> = parts.iter().map(String::as_str).collect();
    let naive = time_it(40, || {
        black_box(textjoin_naive_join(",", &refs, true).unwrap());
    });
    let mut opt = time_it(40, || {
        let mut b = xlsx_engine_core::TextJoinBuilder::new(vec![",".into()]);
        for p in &refs {
            b.push(p, true).unwrap();
        }
        black_box(b.finish());
    });
    // Recompute opt cleanly (the first bind is used).
    let _ = opt;
    opt = time_it(40, || {
        let mut b = xlsx_engine_core::TextJoinBuilder::new(vec![",".into()]);
        for p in &refs {
            b.push(p, true).unwrap();
        }
        black_box(b.finish());
    });
    println!();
    println!(
        "kernel 10k single-delim   naive {}  builder {}  {:.2}×",
        fmt_dur(naive),
        fmt_dur(opt),
        naive.as_secs_f64() / opt.as_secs_f64()
    );
}
