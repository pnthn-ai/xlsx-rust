//! Before/after microbench for Excel `BYROW`.
//!
//! Compares the HashMap-per-row baseline (`excel_byrow_naive`) with the
//! specialized row-aggregator kernel (`excel_byrow`) for `SUM` / `MAX` /
//! constant plans, plus a full `calc-core` evaluate of
//! `BYROW(..., LAMBDA(row, SUM(row)))`.
//!
//! ```text
//! cargo bench -p xlsx-engine-core --bench byrow
//! ```

use std::hint::black_box;
use std::time::Instant;
use xlsx_engine_core::{excel_byrow, excel_byrow_naive, CalcCoreEngine, RowAgg, RowPlan};
use xlsx_types::{
    Candidate, Cell, EvalSpec, EvalTarget, ExcelValue, Sheet, Workbook,
};

fn numeric_grid(rows: usize, cols: usize) -> Vec<Vec<ExcelValue>> {
    (0..rows)
        .map(|r| {
            (0..cols)
                .map(|c| ExcelValue::Number((r * cols + c + 1) as f64))
                .collect()
        })
        .collect()
}

fn time_ms(iters: u32, mut f: impl FnMut()) -> f64 {
    f();
    let t0 = Instant::now();
    for _ in 0..iters {
        f();
    }
    t0.elapsed().as_secs_f64() * 1000.0 / f64::from(iters)
}

fn kernel_bench(rows: usize, cols: usize, plan: &RowPlan, label: &str, iters: u32) {
    let grid = numeric_grid(rows, cols);
    let fast = time_ms(iters, || {
        black_box(excel_byrow(black_box(&grid), black_box(plan)));
    });
    let naive = time_ms(iters, || {
        black_box(excel_byrow_naive(black_box(&grid), black_box(plan)));
    });
    let speedup = naive / fast.max(1e-9);
    println!(
        "kernel {label:<10} {rows}×{cols}  naive={naive:.4}ms  fast={fast:.4}ms  speedup={speedup:.2}x"
    );
}

fn workbook_grid(rows: usize, cols: usize) -> Workbook {
    let mut sheet = Sheet::new("Sheet1");
    for r in 0..rows {
        for c in 0..cols {
            let addr = xlsx_types::CellAddr::new(c as u32, r as u32);
            sheet.cells.insert(
                addr.a1(),
                Cell::value(ExcelValue::Number((r * cols + c + 1) as f64)),
            );
        }
    }
    Workbook {
        sheets: vec![sheet],
        names: vec![],
    }
}

fn evaluate_bench(rows: usize, cols: usize, body: &str, iters: u32) {
    let end = xlsx_types::CellAddr::new((cols - 1) as u32, (rows - 1) as u32);
    let formula = format!("=BYROW(A1:{},LAMBDA(row,{body}))", end.a1());
    let spec = EvalSpec {
        case_id: "bench.byrow".into(),
        workbook: workbook_grid(rows, cols),
        target: EvalTarget::formula(formula.clone()),
        options: Default::default(),
    };
    let engine = CalcCoreEngine::new();
    let got = engine.evaluate(&spec).expect("evaluate");
    match &got {
        ExcelValue::Array(grid) => {
            assert_eq!(grid.len(), rows, "{formula}");
            assert_eq!(grid[0].len(), 1, "{formula}");
        }
        other => panic!("expected array, got {other} for {formula}"),
    }
    let ms = time_ms(iters, || {
        black_box(engine.evaluate(black_box(&spec)).unwrap());
    });
    println!("evaluate {rows}×{cols} body={body:<10}  {ms:.4}ms/call  {formula}");
}

fn main() {
    println!("BYROW bench (specialized row-agg kernel vs HashMap-per-row)\n");
    let sum = RowPlan::Agg(RowAgg::Sum);
    let max = RowPlan::Agg(RowAgg::Max);
    let konst = RowPlan::Const(ExcelValue::Number(1.0));
    kernel_bench(4_096, 16, &sum, "SUM", 40);
    kernel_bench(1_024, 64, &sum, "SUM", 40);
    kernel_bench(4_096, 16, &max, "MAX", 40);
    kernel_bench(4_096, 16, &konst, "const", 60);
    println!();
    evaluate_bench(1_024, 8, "SUM(row)", 20);
    evaluate_bench(2_048, 8, "SUM(row)", 12);
    evaluate_bench(1_024, 8, "MAX(row)", 16);
    evaluate_bench(512, 8, "IF(SUM(row)>10,1,0)", 12);
}
