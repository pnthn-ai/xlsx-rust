//! Before/after microbench for Excel `SCAN`.
//!
//! Compares the HashMap-per-step baseline (`excel_scan_naive`) with the
//! specialized running-sum / product / concat kernels (`excel_scan`), plus
//! a full `calc-core` evaluate of `SCAN(..., LAMBDA(...))`.
//!
//! ```text
//! cargo bench -p xlsx-engine-core --bench scan
//! ```

use std::hint::black_box;
use std::time::Instant;
use xlsx_engine_core::{excel_scan, excel_scan_naive, CalcCoreEngine, FastOp, FastScan};
use xlsx_types::{Candidate, EvalSpec, EvalTarget, ExcelValue, Workbook};

fn add_plan() -> FastScan {
    FastScan::Op {
        op: FastOp::Add,
        left: Box::new(FastScan::Acc),
        right: Box::new(FastScan::Val),
    }
}

fn mul_plan() -> FastScan {
    FastScan::Op {
        op: FastOp::Mul,
        left: Box::new(FastScan::Acc),
        right: Box::new(FastScan::Val),
    }
}

fn concat_plan() -> FastScan {
    FastScan::Concat(Box::new(FastScan::Acc), Box::new(FastScan::Val))
}

fn numeric_row(n: usize) -> Vec<Vec<ExcelValue>> {
    vec![(1..=n).map(|i| ExcelValue::Number(i as f64)).collect()]
}

fn time_ms(iters: u32, mut f: impl FnMut()) -> f64 {
    f();
    let t0 = Instant::now();
    for _ in 0..iters {
        f();
    }
    t0.elapsed().as_secs_f64() * 1000.0 / f64::from(iters)
}

fn kernel_bench(n: usize, plan: &FastScan, label: &str, initial: ExcelValue, iters: u32) {
    let grid = numeric_row(n);
    let fast = time_ms(iters, || {
        black_box(excel_scan(
            black_box(Some(&initial)),
            black_box(&grid),
            black_box(plan),
        ));
    });
    let naive = time_ms(iters, || {
        black_box(excel_scan_naive(
            black_box(Some(&initial)),
            black_box(&grid),
            black_box(plan),
        ));
    });
    let speedup = naive / fast.max(1e-9);
    println!(
        "kernel {label:<12} n={n:<6}  naive={naive:.4}ms  fast={fast:.4}ms  speedup={speedup:.2}x"
    );
}

fn evaluate_bench(rows: usize, cols: usize, body: &str, iters: u32) {
    let formula = format!("=SCAN(0,SEQUENCE({rows},{cols}),LAMBDA(a,v,{body}))");
    let spec = EvalSpec {
        case_id: "bench.scan".into(),
        workbook: Workbook::default(),
        target: EvalTarget::formula(formula.clone()),
        options: Default::default(),
    };
    let engine = CalcCoreEngine::new();
    let got = engine.evaluate(&spec).expect("evaluate");
    match &got {
        ExcelValue::Array(grid) => {
            assert_eq!(grid.len(), rows, "{formula}");
            assert_eq!(grid[0].len(), cols, "{formula}");
        }
        other => panic!("expected array, got {other} for {formula}"),
    }
    let ms = time_ms(iters, || {
        black_box(engine.evaluate(black_box(&spec)).unwrap());
    });
    println!("evaluate {rows}×{cols} body={body:<6}  {ms:.4}ms/call  {formula}");
}

fn main() {
    println!("SCAN bench (specialized running kernel vs HashMap-per-step)\n");
    let add = add_plan();
    let mul = mul_plan();
    let concat = concat_plan();
    let zero = ExcelValue::Number(0.0);
    let one = ExcelValue::Number(1.0);
    kernel_bench(4096, &add, "acc+val", zero.clone(), 40);
    kernel_bench(16384, &add, "acc+val", zero, 16);
    kernel_bench(4096, &mul, "acc*val", one, 40);
    kernel_bench(
        4096,
        &concat,
        "acc&val",
        ExcelValue::Text(String::new()),
        20,
    );
    println!();
    evaluate_bench(64, 64, "a+v", 16);
    evaluate_bench(128, 128, "a+v", 8);
    evaluate_bench(256, 256, "a+v", 4);
    evaluate_bench(64, 64, "IF(v>0,a+v,a)", 8);
}
