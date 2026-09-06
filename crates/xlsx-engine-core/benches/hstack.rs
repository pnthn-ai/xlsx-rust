//! Before/after microbench for Excel `HSTACK`.
//!
//! Compares the grow-by-`extend` baseline (`excel_hstack_naive`) with the
//! production kernel (`excel_hstack`: one pre-sized allocation + copy), plus
//! a full `calc-core` evaluate of `HSTACK(A1:An, B1:Bn)`.
//!
//! ```text
//! cargo bench -p xlsx-engine-core --bench hstack
//! ```

use std::hint::black_box;
use std::time::{Duration, Instant};
use xlsx_engine_core::{excel_hstack, excel_hstack_naive, CalcCoreEngine};
use xlsx_types::{Candidate, Cell, EvalSpec, EvalTarget, ExcelValue, Sheet, Workbook};

const ITERS_KERNEL: u32 = 30;
const ITERS_EVAL: u32 = 8;

struct Case {
    name: &'static str,
    args: Vec<ExcelValue>,
}

fn col(n: usize, offset: f64) -> ExcelValue {
    ExcelValue::Array(
        (0..n)
            .map(|i| vec![ExcelValue::Number(offset + i as f64)])
            .collect(),
    )
}

fn wide_row(n: usize, offset: f64) -> ExcelValue {
    ExcelValue::Array(vec![(0..n)
        .map(|i| ExcelValue::Number(offset + i as f64))
        .collect()])
}

fn matrix(rows: usize, cols: usize, offset: f64) -> ExcelValue {
    ExcelValue::Array(
        (0..rows)
            .map(|r| {
                (0..cols)
                    .map(|c| ExcelValue::Number(offset + (r * cols + c) as f64))
                    .collect()
            })
            .collect(),
    )
}

fn cases() -> Vec<Case> {
    vec![
        Case {
            name: "8k×1 + 8k×1 equal height",
            args: vec![col(8_192, 0.0), col(8_192, 10_000.0)],
        },
        Case {
            name: "8k×8 + 8k×8 matrices",
            args: vec![matrix(8_192, 8, 0.0), matrix(8_192, 8, 100_000.0)],
        },
        Case {
            name: "8k×1 + 128×1 pad #N/A",
            args: vec![col(8_192, 0.0), col(128, 50_000.0)],
        },
        Case {
            name: "128×1 + 8k×1 pad left",
            args: vec![col(128, 0.0), col(8_192, 50_000.0)],
        },
        Case {
            // 512×513 with a large #N/A triangle — not 8k×8k (67M pad cells).
            name: "1×512 + 512×1 mixed axes",
            args: vec![wide_row(512, 0.0), col(512, 1.0)],
        },
        Case {
            name: "16 columns of 4k (many args)",
            args: (0..16).map(|i| col(4_096, (i * 10_000) as f64)).collect(),
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

fn workbook_two_cols(n: u32, short: u32) -> Workbook {
    let mut sheet = Sheet::new("Sheet1");
    for i in 0..n {
        sheet.cells.insert(
            format!("A{}", i + 1),
            Cell::value(ExcelValue::Number((i + 1) as f64)),
        );
        if i < short {
            sheet.cells.insert(
                format!("B{}", i + 1),
                Cell::value(ExcelValue::Number((i + 1_000) as f64)),
            );
        }
    }
    Workbook {
        sheets: vec![sheet],
        names: vec![],
    }
}

fn evaluate_bench(n: u32, short: u32, formula: &str, iters: u32) {
    let wb = workbook_two_cols(n, short);
    let spec = EvalSpec {
        case_id: "bench.hstack".into(),
        workbook: wb,
        target: EvalTarget::formula(formula),
        options: Default::default(),
    };
    let engine = CalcCoreEngine::new();
    let got = engine.evaluate(&spec).expect("evaluate");
    match &got {
        ExcelValue::Array(rows) => {
            assert_eq!(rows.len(), n as usize, "{formula} height");
            assert_eq!(rows[0].len(), 2, "{formula} width");
        }
        other => panic!("expected array, got {other}"),
    }
    let ms = time_it(iters, || {
        black_box(engine.evaluate(black_box(&spec)).unwrap());
    });
    println!(
        "evaluate n={n:>6} short={short:<5}  {}  formula={formula}",
        fmt_dur(ms)
    );
}

fn main() {
    println!("HSTACK kernel bench (grow-by-extend vs pre-size + copy)");
    println!(
        "{:<36} {:>12} {:>12} {:>8}",
        "case", "naive", "optimized", "speedup"
    );
    println!("{}", "-".repeat(72));
    for c in cases() {
        let naive = time_it(ITERS_KERNEL, || {
            let _ = black_box(excel_hstack_naive(black_box(&c.args)));
        });
        let fast = time_it(ITERS_KERNEL, || {
            let _ = black_box(excel_hstack(black_box(&c.args)));
        });
        let speedup = naive.as_secs_f64() / fast.as_secs_f64().max(1e-12);
        println!(
            "{:<36} {:>12} {:>12} {:>7.1}x",
            c.name,
            fmt_dur(naive),
            fmt_dur(fast),
            speedup
        );
        assert_eq!(
            excel_hstack(&c.args),
            excel_hstack_naive(&c.args),
            "semantic mismatch on {}",
            c.name
        );
    }
    println!();
    evaluate_bench(10_000, 10_000, "=HSTACK(A1:A10000,B1:B10000)", ITERS_EVAL);
    evaluate_bench(10_000, 100, "=HSTACK(A1:A10000,B1:B100)", ITERS_EVAL);
}
