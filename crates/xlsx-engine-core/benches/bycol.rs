//! Before/after microbench for Excel `BYCOL`.
//!
//! Compares the clone-each-column baseline (`excel_bycol_naive`) with the
//! in-place column walk (`excel_bycol`) for `SUM` / `AVERAGE` / constant
//! plans, plus a full `calc-core` evaluate of `BYCOL(..., LAMBDA(...))`.
//!
//! ```text
//! cargo bench -p xlsx-engine-core --bench bycol
//! ```

use std::hint::black_box;
use std::time::Instant;
use xlsx_engine_core::{excel_bycol, excel_bycol_naive, BycolOp, CalcCoreEngine};
use xlsx_types::{Candidate, EvalSpec, EvalTarget, ExcelValue, Workbook};

fn n(x: f64) -> ExcelValue {
    ExcelValue::Number(x)
}

fn numeric_rows(rows: usize, cols: usize) -> Vec<Vec<ExcelValue>> {
    let mut grid = Vec::with_capacity(rows);
    for r in 0..rows {
        let mut row = Vec::with_capacity(cols);
        for c in 0..cols {
            row.push(n(((r * cols + c + 1) % 17) as f64));
        }
        grid.push(row);
    }
    grid
}

fn time_ms(iters: u32, mut f: impl FnMut()) -> f64 {
    f();
    let t0 = Instant::now();
    for _ in 0..iters {
        f();
    }
    t0.elapsed().as_secs_f64() * 1000.0 / f64::from(iters)
}

fn kernel_bench(rows: usize, cols: usize, op: &BycolOp, label: &str, iters: u32) {
    let grid = numeric_rows(rows, cols);
    let fast = time_ms(iters, || {
        black_box(excel_bycol(black_box(&grid), black_box(op)));
    });
    let naive = time_ms(iters, || {
        black_box(excel_bycol_naive(black_box(&grid), black_box(op)));
    });
    let speedup = naive / fast.max(1e-9);
    println!(
        "kernel {label:<12} {rows}×{cols}  naive={naive:.4}ms  fast={fast:.4}ms  speedup={speedup:.2}x"
    );
}

fn evaluate_bench(rows: usize, cols: usize, body: &str, iters: u32) {
    let mut cells = std::collections::BTreeMap::new();
    for r in 0..rows {
        for c in 0..cols {
            let addr = xlsx_types::CellAddr::new(c as u32, r as u32);
            cells.insert(
                addr.a1(),
                xlsx_types::Cell::value(n(((r * cols + c + 1) % 17) as f64)),
            );
        }
    }
    let end = xlsx_types::CellAddr::new((cols - 1) as u32, (rows - 1) as u32);
    let formula = format!("=BYCOL(A1:{},LAMBDA(c,{body}))", end.a1());
    let spec = EvalSpec {
        case_id: "bench.bycol".into(),
        workbook: Workbook {
            sheets: vec![xlsx_types::Sheet {
                name: "Sheet1".into(),
                cells,
            }],
            names: vec![],
        },
        target: EvalTarget::formula(formula.clone()),
        options: Default::default(),
    };
    let engine = CalcCoreEngine::new();
    let got = engine.evaluate(&spec).expect("evaluate");
    match &got {
        ExcelValue::Array(grid) => {
            assert_eq!(grid.len(), 1, "{formula}");
            assert_eq!(grid[0].len(), cols, "{formula}");
        }
        other => panic!("expected array, got {other} for {formula}"),
    }
    let ms = time_ms(iters, || {
        black_box(engine.evaluate(black_box(&spec)).unwrap());
    });
    println!("evaluate {rows}×{cols} body={body:<12}  {ms:.4}ms/call  {formula}");
}

fn main() {
    println!("BYCOL bench (in-place column reduce vs clone-each-column)\n");
    kernel_bench(256, 256, &BycolOp::Sum, "SUM", 20);
    kernel_bench(512, 128, &BycolOp::Sum, "SUM", 12);
    kernel_bench(256, 256, &BycolOp::Average, "AVERAGE", 20);
    kernel_bench(256, 256, &BycolOp::Const(n(1.0)), "const", 20);
    println!();
    evaluate_bench(64, 64, "SUM(c)", 16);
    evaluate_bench(128, 128, "SUM(c)", 8);
    evaluate_bench(256, 64, "SUM(c)", 8);
    evaluate_bench(64, 64, "IF(SUM(c)>8,1,0)", 8);
}
