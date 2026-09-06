//! Before/after microbench for Excel `MAP`.
//!
//! Compares the Vec-bind-per-cell baseline (`excel_map_naive`) with the
//! specialized zip kernel (`excel_map`) for `x*2` / `x+y` / identity, plus
//! a full `calc-core` evaluate of `MAP(..., LAMBDA(...))`.
//!
//! ```text
//! cargo bench -p xlsx-engine-core --bench map
//! ```

use std::hint::black_box;
use std::time::Instant;
use xlsx_engine_core::{excel_map, excel_map_naive, CalcCoreEngine, MapFast, MapOp};
use xlsx_types::{Candidate, EvalSpec, EvalTarget, ExcelValue, Workbook};

fn n(x: f64) -> ExcelValue {
    ExcelValue::Number(x)
}

fn col(n_rows: usize, f: impl Fn(usize) -> f64) -> ExcelValue {
    ExcelValue::Array((1..=n_rows).map(|i| vec![n(f(i))]).collect())
}

fn times2() -> MapFast {
    MapFast::Op {
        op: MapOp::Mul,
        left: Box::new(MapFast::Param(0)),
        right: Box::new(MapFast::Const(n(2.0))),
    }
}

fn add_xy() -> MapFast {
    MapFast::Op {
        op: MapOp::Add,
        left: Box::new(MapFast::Param(0)),
        right: Box::new(MapFast::Param(1)),
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

fn kernel_bench(label: &str, arrays: &[ExcelValue], plan: &MapFast, iters: u32) {
    let fast = time_ms(iters, || {
        black_box(excel_map(black_box(arrays), black_box(plan)));
    });
    let naive = time_ms(iters, || {
        black_box(excel_map_naive(black_box(arrays), black_box(plan)));
    });
    let speedup = naive / fast.max(1e-9);
    println!("kernel {label:<16}  naive={naive:.4}ms  fast={fast:.4}ms  speedup={speedup:.2}x");
}

fn evaluate_bench(formula: &str, expect_cells: usize, iters: u32) {
    let spec = EvalSpec {
        case_id: "bench.map".into(),
        workbook: Workbook::default(),
        target: EvalTarget::formula(formula.to_string()),
        options: Default::default(),
    };
    let engine = CalcCoreEngine::new();
    let got = engine.evaluate(&spec).expect("evaluate");
    match &got {
        ExcelValue::Array(grid) => {
            let n = grid.iter().map(|r| r.len()).sum::<usize>();
            assert_eq!(n, expect_cells, "{formula}");
        }
        other => panic!("expected array, got {other} for {formula}"),
    }
    let ms = time_ms(iters, || {
        black_box(engine.evaluate(black_box(&spec)).unwrap());
    });
    println!("evaluate {formula:<44}  {ms:.4}ms/call");
}

fn main() {
    println!("MAP bench (specialized zip kernel vs Vec-bind-per-cell)\n");
    let a10k = col(10_000, |i| i as f64);
    let b10k = col(10_000, |i| (i * 2) as f64);
    kernel_bench("x*2 10k", &[a10k.clone()], &times2(), 40);
    kernel_bench("ident 10k", &[a10k.clone()], &MapFast::Param(0), 40);
    kernel_bench("x+y 10k", &[a10k, b10k], &add_xy(), 30);
    println!();
    evaluate_bench("=MAP(SEQUENCE(10000),LAMBDA(x,x*2))", 10_000, 16);
    evaluate_bench(
        "=MAP(SEQUENCE(10000),SEQUENCE(10000,1,10,10),LAMBDA(a,b,a+b))",
        10_000,
        12,
    );
    evaluate_bench("=MAP(SEQUENCE(64,64),LAMBDA(x,IF(x>10,x,0)))", 64 * 64, 8);
}
