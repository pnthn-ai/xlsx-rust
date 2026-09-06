//! Before/after microbench for Excel `SEQUENCE`.
//!
//! Compares the growing-`Vec` baseline (`excel_sequence_naive`) with the
//! production kernel (`excel_sequence`: pre-sized rows + `i64` increment
//! when `start`/`step` are exact integers).
//!
//! ```text
//! cargo bench -p xlsx-engine-core --bench sequence
//! ```

use std::hint::black_box;
use std::time::{Duration, Instant};
use xlsx_engine_core::{excel_sequence, excel_sequence_naive, CalcCoreEngine};
use xlsx_types::{Candidate, EvalSpec, EvalTarget, ExcelValue, Workbook};

const KERNEL_ITERS: u32 = 40;
const EVAL_ITERS: u32 = 12;

struct Case {
    name: &'static str,
    rows: f64,
    cols: f64,
    start: f64,
    step: f64,
}

fn cases() -> Vec<Case> {
    vec![
        Case {
            name: "64k column, start 1 step 1",
            rows: 65_536.0,
            cols: 1.0,
            start: 1.0,
            step: 1.0,
        },
        Case {
            name: "8k×16 grid, start 1 step 1",
            rows: 8_192.0,
            cols: 16.0,
            start: 1.0,
            step: 1.0,
        },
        Case {
            name: "4k×32 grid, start 100 step 10",
            rows: 4_096.0,
            cols: 32.0,
            start: 100.0,
            step: 10.0,
        },
        Case {
            name: "32k column, start 1e6 step -3",
            rows: 32_768.0,
            cols: 1.0,
            start: 1_000_000.0,
            step: -3.0,
        },
        Case {
            name: "16k column, start 0.5 step 0.25",
            rows: 16_384.0,
            cols: 1.0,
            start: 0.5,
            step: 0.25,
        },
        Case {
            name: "1×64k row, start 1 step 1",
            rows: 1.0,
            cols: 65_536.0,
            start: 1.0,
            step: 1.0,
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

fn evaluate_bench(rows: u32, cols: u32) {
    let formula = format!("=SEQUENCE({rows},{cols})");
    let spec = EvalSpec {
        case_id: "bench.sequence".into(),
        workbook: Workbook {
            sheets: vec![],
            names: vec![],
        },
        target: EvalTarget::formula(formula),
        options: Default::default(),
    };
    let engine = CalcCoreEngine::new();
    let got = engine.evaluate(&spec).expect("evaluate");
    match &got {
        ExcelValue::Array(out) => {
            assert_eq!(out.len(), rows as usize, "SEQUENCE rows");
            assert_eq!(out[0].len(), cols as usize, "SEQUENCE cols");
            assert_eq!(out[0][0], ExcelValue::Number(1.0));
        }
        other => panic!("expected array, got {other}"),
    }
    let ms = time_it(EVAL_ITERS, || {
        black_box(engine.evaluate(black_box(&spec)).unwrap());
    });
    println!("evaluate SEQUENCE({rows},{cols})  {:>12}/call", fmt_dur(ms));
}

fn main() {
    println!("SEQUENCE bench (calc-core pre-size + i64 increment vs growing Vec)\n");
    println!(
        "{:<42} {:>12} {:>12} {:>8}",
        "case", "naive", "optimized", "speedup"
    );
    println!("{}", "-".repeat(80));
    for c in cases() {
        let naive = time_it(KERNEL_ITERS, || {
            let _ = black_box(excel_sequence_naive(
                black_box(c.rows),
                black_box(c.cols),
                black_box(c.start),
                black_box(c.step),
            ));
        });
        let fast = time_it(KERNEL_ITERS, || {
            let _ = black_box(excel_sequence(
                black_box(c.rows),
                black_box(c.cols),
                black_box(c.start),
                black_box(c.step),
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
        let a = excel_sequence_naive(c.rows, c.cols, c.start, c.step);
        let b = excel_sequence(c.rows, c.cols, c.start, c.step);
        assert_eq!(a, b, "semantic mismatch on {}", c.name);
    }
    println!();
    evaluate_bench(10_000, 1);
    evaluate_bench(1_000, 16);
    evaluate_bench(65_536, 1);
}
