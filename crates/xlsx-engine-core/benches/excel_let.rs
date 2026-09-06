//! Before/after microbench for Excel `LET`.
//!
//! 1. **Calc kernel** — indexed walk (`excel_let_eval_fast`) vs HashMap clone
//!    on every name leaf (`excel_let_eval_naive`).
//! 2. **Formula** — `LET(s, SUM(range), s+s+s+s)` evaluates the range once;
//!    the equivalent `SUM+SUM+SUM+SUM` walks it four times.
//!
//! ```text
//! cargo bench -p xlsx-engine-core --bench excel_let
//! ```

use std::hint::black_box;
use std::time::{Duration, Instant};
use xlsx_engine_core::{
    excel_let_eval_fast, excel_let_eval_naive, parse, CalcCoreEngine, FastCalc, LetFastOp,
};
use xlsx_types::{Candidate, Cell, CellAddr, EvalSpec, EvalTarget, ExcelValue, Sheet, Workbook};

const ITERS_KERNEL: u32 = 200;
const ITERS_FORMULA: u32 = 40;

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

fn sum_range_wb(rows: u32) -> Workbook {
    let mut sheet = Sheet::new("Sheet1");
    for i in 0..rows {
        sheet.insert(
            CellAddr::new(0, i),
            Cell::value(ExcelValue::Number((i + 1) as f64)),
        );
    }
    Workbook {
        sheets: vec![sheet],
        names: vec![],
    }
}

fn spec(wb: Workbook, formula: &str) -> EvalSpec {
    EvalSpec {
        case_id: "bench.let".into(),
        workbook: wb,
        target: EvalTarget::formula(formula),
        options: Default::default(),
    }
}

fn reuse_calc() -> FastCalc {
    // s+s+s+s
    let add = |l, r| FastCalc::Op {
        op: LetFastOp::Add,
        left: Box::new(l),
        right: Box::new(r),
    };
    add(
        add(FastCalc::Name(0), FastCalc::Name(0)),
        add(FastCalc::Name(0), FastCalc::Name(0)),
    )
}

fn main() {
    println!("LET kernel + formula bench (bind-once vs recompute / HashMap)\n");
    println!(
        "{:<48} {:>12} {:>12} {:>8}",
        "case", "naive", "optimized", "speedup"
    );
    println!("{}", "-".repeat(84));

    let names = vec!["s".to_string()];
    let values = vec![ExcelValue::Number(12345.0)];
    let calc = reuse_calc();
    assert_eq!(
        excel_let_eval_fast(&calc, &values),
        excel_let_eval_naive(&calc, &names, &values)
    );
    assert_eq!(
        excel_let_eval_fast(&calc, &values),
        ExcelValue::Number(49380.0)
    );

    let naive = time_it(ITERS_KERNEL, || {
        let _ = black_box(excel_let_eval_naive(
            black_box(&calc),
            black_box(&names),
            black_box(&values),
        ));
    });
    let fast = time_it(ITERS_KERNEL, || {
        let _ = black_box(excel_let_eval_fast(black_box(&calc), black_box(&values)));
    });
    let speedup = naive.as_secs_f64() / fast.as_secs_f64().max(1e-12);
    println!(
        "{:<48} {:>12} {:>12} {:>7.1}x",
        "s+s+s+s kernel (HashMap vs index)",
        fmt_dur(naive),
        fmt_dur(fast),
        speedup
    );

    let body = parse("s*s+s").unwrap();
    let classified = xlsx_engine_core::eval::excel_let::classify(&body, &names).unwrap();
    let naive2 = time_it(ITERS_KERNEL, || {
        let _ = black_box(excel_let_eval_naive(
            black_box(&classified),
            black_box(&names),
            black_box(&values),
        ));
    });
    let fast2 = time_it(ITERS_KERNEL, || {
        let _ = black_box(excel_let_eval_fast(
            black_box(&classified),
            black_box(&values),
        ));
    });
    let speedup2 = naive2.as_secs_f64() / fast2.as_secs_f64().max(1e-12);
    println!(
        "{:<48} {:>12} {:>12} {:>7.1}x",
        "s*s+s kernel (HashMap vs index)",
        fmt_dur(naive2),
        fmt_dur(fast2),
        speedup2
    );

    let engine = CalcCoreEngine::new();
    let rows = 4000u32;
    let wb = sum_range_wb(rows);
    let closed = (rows * (rows + 1) / 2) as f64;
    let let_spec = spec(wb.clone(), "=LET(s, SUM(A1:A4000), s+s+s+s)");
    let dup_spec = spec(
        wb,
        "=SUM(A1:A4000)+SUM(A1:A4000)+SUM(A1:A4000)+SUM(A1:A4000)",
    );

    let dup_t = time_it(ITERS_FORMULA, || {
        let _ = black_box(engine.evaluate(black_box(&dup_spec)).unwrap());
    });
    let let_t = time_it(ITERS_FORMULA, || {
        let _ = black_box(engine.evaluate(black_box(&let_spec)).unwrap());
    });
    let speedup_f = dup_t.as_secs_f64() / let_t.as_secs_f64().max(1e-12);
    println!(
        "{:<48} {:>12} {:>12} {:>7.1}x",
        "4× SUM(4k): duplicated vs LET",
        fmt_dur(dup_t),
        fmt_dur(let_t),
        speedup_f
    );
    assert_eq!(
        engine.evaluate(&let_spec).unwrap(),
        ExcelValue::Number(closed * 4.0)
    );
    assert_eq!(
        engine.evaluate(&dup_spec).unwrap(),
        ExcelValue::Number(closed * 4.0)
    );
}
