//! Before/after microbench for Excel `MAKEARRAY`.
//!
//! Compares the HashMap-per-cell baseline (`excel_makearray_naive`) with the
//! specialized index kernel (`excel_makearray`) for `r*c` / `r+c` / constant
//! plans, plus a full `calc-core` evaluate of `MAKEARRAY(..., LAMBDA(...))`.
//!
//! ```text
//! cargo bench -p xlsx-engine-core --bench makearray
//! ```

use std::hint::black_box;
use std::time::Instant;
use xlsx_engine_core::{excel_makearray, excel_makearray_naive, CalcCoreEngine, FastBody, FastOp};
use xlsx_types::{Candidate, EvalSpec, EvalTarget, ExcelValue, Workbook};

fn mul_plan() -> FastBody {
    FastBody::Op {
        op: FastOp::Mul,
        left: Box::new(FastBody::Row),
        right: Box::new(FastBody::Col),
    }
}

fn add_plan() -> FastBody {
    FastBody::Op {
        op: FastOp::Add,
        left: Box::new(FastBody::Row),
        right: Box::new(FastBody::Col),
    }
}

fn time_ms(iters: u32, mut f: impl FnMut()) -> f64 {
    f();
    let t0 = Instant::now();
    for _ in 0..iters {
        f();
    }
    t0.elapsed().as_secs_f64() * 1000.0 / f64::from(iters)
}

fn kernel_bench(rows: usize, cols: usize, plan: &FastBody, label: &str, iters: u32) {
    let fast = time_ms(iters, || {
        black_box(excel_makearray(
            black_box(rows),
            black_box(cols),
            black_box(plan),
        ));
    });
    let naive = time_ms(iters, || {
        black_box(excel_makearray_naive(
            black_box(rows),
            black_box(cols),
            black_box(plan),
        ));
    });
    let speedup = naive / fast.max(1e-9);
    println!(
        "kernel {label:<12} {rows}×{cols}  naive={naive:.4}ms  fast={fast:.4}ms  speedup={speedup:.2}x"
    );
}

fn evaluate_bench(rows: usize, cols: usize, body: &str, iters: u32) {
    let formula = format!("=MAKEARRAY({rows},{cols},LAMBDA(r,c,{body}))");
    let spec = EvalSpec {
        case_id: "bench.makearray".into(),
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
    println!("MAKEARRAY bench (specialized index kernel vs HashMap-per-cell)\n");
    let mul = mul_plan();
    let add = add_plan();
    let konst = FastBody::Const(ExcelValue::Number(1.0));
    kernel_bench(256, 256, &mul, "r*c", 20);
    kernel_bench(512, 128, &mul, "r*c", 12);
    kernel_bench(256, 256, &add, "r+c", 20);
    kernel_bench(256, 256, &konst, "const", 20);
    println!();
    evaluate_bench(64, 64, "r*c", 16);
    evaluate_bench(128, 128, "r*c", 8);
    evaluate_bench(256, 256, "r*c", 4);
    evaluate_bench(64, 64, "IF(r=c,1,0)", 8);
}
