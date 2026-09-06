//! Before/after microbench for Excel `ISOMITTED`.
//!
//! 1. **Kernel** — reverse-scan [`excel_isomitted`] vs HashMap-per-call
//!    [`excel_isomitted_naive`].
//! 2. **Formula** — IIFE `LAMBDA(...)(args)` and `MAKEARRAY` bodies that
//!    call `ISOMITTED` on a bound parameter.
//!
//! ```text
//! cargo bench -p xlsx-engine-core --bench isomitted
//! ```

use std::hint::black_box;
use std::time::Instant;
use xlsx_engine_core::{excel_isomitted, excel_isomitted_naive, parse, CalcCoreEngine, Local};
use xlsx_types::{Candidate, EvalSpec, EvalTarget, ExcelValue, Workbook};

fn time_ms(iters: u32, mut f: impl FnMut()) -> f64 {
    f();
    let t0 = Instant::now();
    for _ in 0..iters {
        f();
    }
    t0.elapsed().as_secs_f64() * 1000.0 / f64::from(iters)
}

fn kernel_bench(label: &str, locals: &[Local], arg: &xlsx_engine_core::Expr, n: u32) {
    let fast = time_ms(n, || {
        black_box(excel_isomitted(black_box(arg), black_box(locals)));
    });
    let naive = time_ms(n, || {
        black_box(excel_isomitted_naive(black_box(arg), black_box(locals)));
    });
    let speedup = naive / fast.max(1e-12);
    println!(
        "kernel {label:<16}  naive={naive:.4}ms  fast={fast:.4}ms  speedup={speedup:.2}x  n={n}"
    );
}

fn evaluate_bench(label: &str, formula: &str, iters: u32) {
    let spec = EvalSpec {
        case_id: format!("bench.isomitted.{label}"),
        workbook: Workbook::default(),
        target: EvalTarget::formula(formula),
        options: Default::default(),
    };
    let engine = CalcCoreEngine::new();
    let got = engine.evaluate(&spec).expect("evaluate");
    assert!(!matches!(got, ExcelValue::Error(_)), "{formula} → {got}");
    let ms = time_ms(iters, || {
        black_box(engine.evaluate(black_box(&spec)).unwrap());
    });
    println!("evaluate {label:<24}  {ms:.4}ms/call  {formula}");
}

fn main() {
    println!("ISOMITTED bench (reverse-scan kernel vs HashMap-per-call)\n");
    let locals = vec![
        Local::provided("x", ExcelValue::Number(1.0)),
        Local::missing("y"),
        Local::provided("z", ExcelValue::Number(3.0)),
    ];
    let y = parse("y").unwrap();
    let x = parse("x").unwrap();
    let lit = parse("1").unwrap();
    kernel_bench("omitted y", &locals, &y, 200_000);
    kernel_bench("provided x", &locals, &x, 200_000);
    kernel_bench("literal", &locals, &lit, 200_000);
    println!();
    evaluate_bench("iife_omitted", "=LAMBDA(x,y,ISOMITTED(y))(1,)", 8_000);
    evaluate_bench("iife_provided", "=LAMBDA(x,y,ISOMITTED(y))(1,2)", 8_000);
    evaluate_bench(
        "iife_if_default",
        "=LAMBDA(x,y,IF(ISOMITTED(y),10,y)+x)(5)",
        6_000,
    );
    evaluate_bench(
        "makearray_provided",
        "=MAKEARRAY(64,64,LAMBDA(r,c,ISOMITTED(c)))",
        40,
    );
}
