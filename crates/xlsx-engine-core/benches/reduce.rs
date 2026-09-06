//! Before/after microbench for Excel `REDUCE`.
//!
//! Compares the HashMap-per-step baseline (`excel_reduce_naive`) with the
//! specialized acc/value kernel (`excel_reduce`) for `acc+val` / `acc*val`
//! / `acc&val` plans, plus a full `calc-core` evaluate of
//! `REDUCE(..., LAMBDA(...))`.
//!
//! ```text
//! cargo bench -p xlsx-engine-core --bench reduce
//! ```

use std::hint::black_box;
use std::time::Instant;
use xlsx_engine_core::{excel_reduce, excel_reduce_naive, CalcCoreEngine, ReduceOp, ReducePlan};
use xlsx_types::{Candidate, EvalSpec, EvalTarget, ExcelValue, Workbook};

fn add_plan() -> ReducePlan {
    ReducePlan::Op {
        op: ReduceOp::Add,
        left: Box::new(ReducePlan::Acc),
        right: Box::new(ReducePlan::Val),
    }
}

fn mul_plan() -> ReducePlan {
    ReducePlan::Op {
        op: ReduceOp::Mul,
        left: Box::new(ReducePlan::Acc),
        right: Box::new(ReducePlan::Val),
    }
}

fn concat_plan() -> ReducePlan {
    ReducePlan::Op {
        op: ReduceOp::Concat,
        left: Box::new(ReducePlan::Acc),
        right: Box::new(ReducePlan::Val),
    }
}

fn number_row(n: usize) -> ExcelValue {
    ExcelValue::Array(vec![(1..=n)
        .map(|i| ExcelValue::Number(i as f64))
        .collect()])
}

fn text_row(n: usize) -> ExcelValue {
    ExcelValue::Array(vec![(0..n)
        .map(|i| ExcelValue::Text(format!("{}", (b'A' + (i % 26) as u8) as char)))
        .collect()])
}

fn time_ms(iters: u32, mut f: impl FnMut()) -> f64 {
    f();
    let t0 = Instant::now();
    for _ in 0..iters {
        f();
    }
    t0.elapsed().as_secs_f64() * 1000.0 / f64::from(iters)
}

fn kernel_bench(
    n: usize,
    array: &ExcelValue,
    init: &ExcelValue,
    plan: &ReducePlan,
    label: &str,
    iters: u32,
) {
    let fast = time_ms(iters, || {
        black_box(excel_reduce(
            black_box(Some(init)),
            black_box(array),
            black_box(plan),
        ));
    });
    let naive = time_ms(iters, || {
        black_box(excel_reduce_naive(
            black_box(Some(init)),
            black_box(array),
            black_box(plan),
        ));
    });
    let speedup = naive / fast.max(1e-9);
    println!(
        "kernel {label:<12} n={n:<6}  naive={naive:.4}ms  fast={fast:.4}ms  speedup={speedup:.2}x"
    );
}

fn evaluate_bench(n: usize, init: &str, body: &str, iters: u32) {
    let seq = (1..=n).map(|i| i.to_string()).collect::<Vec<_>>().join(",");
    let formula = format!("=REDUCE({init},{{{seq}}},LAMBDA(a,b,{body}))");
    let spec = EvalSpec {
        case_id: "bench.reduce".into(),
        workbook: Workbook::default(),
        target: EvalTarget::formula(formula.clone()),
        options: Default::default(),
    };
    let engine = CalcCoreEngine::new();
    let got = engine.evaluate(&spec).expect("evaluate");
    if matches!(got, ExcelValue::Error(_)) {
        panic!("unexpected error for {formula}: {got}");
    }
    let ms = time_ms(iters, || {
        black_box(engine.evaluate(black_box(&spec)).unwrap());
    });
    println!("evaluate n={n:<6} body={body:<8}  {ms:.4}ms/call");
}

fn main() {
    println!("REDUCE bench (specialized acc/value kernel vs HashMap-per-step)\n");
    let add = add_plan();
    let mul = mul_plan();
    let cat = concat_plan();
    let nums_8k = number_row(8_192);
    let nums_32k = number_row(32_768);
    let texts_4k = text_row(4_096);
    let zero = ExcelValue::Number(0.0);
    let one = ExcelValue::Number(1.0);
    let empty = ExcelValue::Text(String::new());
    kernel_bench(8_192, &nums_8k, &zero, &add, "acc+val", 40);
    kernel_bench(32_768, &nums_32k, &zero, &add, "acc+val", 12);
    kernel_bench(8_192, &nums_8k, &one, &mul, "acc*val", 40);
    kernel_bench(4_096, &texts_4k, &empty, &cat, "acc&val", 20);
    println!();
    evaluate_bench(1_024, "0", "a+b", 20);
    evaluate_bench(4_096, "0", "a+b", 12);
    evaluate_bench(8_192, "0", "a+b", 8);
    evaluate_bench(1_024, "0", "IF(b>0,a+b,a)", 8);
}
