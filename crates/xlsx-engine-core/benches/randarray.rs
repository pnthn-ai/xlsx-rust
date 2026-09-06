//! RANDARRAY hot-path bench: naive push-per-cell vs pre-sized fill,
//! plus a full `calc-core` evaluate of RANDARRAY(rows, columns, …).
//!
//! ```text
//! cargo bench -p xlsx-engine-core --bench randarray
//! ```

use std::hint::black_box;
use std::time::Instant;
use xlsx_engine_core::{
    excel_randarray_fill, excel_randarray_fill_naive, CalcCoreEngine, XorShift64,
};
use xlsx_types::{Candidate, EvalSpec, ExcelValue};

fn time_ms(iters: u32, mut f: impl FnMut()) -> f64 {
    f();
    let t0 = Instant::now();
    for _ in 0..iters {
        f();
    }
    t0.elapsed().as_secs_f64() * 1000.0 / f64::from(iters)
}

fn kernel_bench(rows: u32, cols: u32, integer: bool, iters: u32) {
    let cells = rows as u64 * cols as u64;
    let (min, max) = if integer { (1.0, 100.0) } else { (0.0, 1.0) };

    let fast_ms = time_ms(iters, || {
        let mut rng = XorShift64::new(1);
        black_box(excel_randarray_fill(
            black_box(rows),
            black_box(cols),
            min,
            max,
            integer,
            &mut rng,
        ));
    });
    let naive_ms = time_ms(iters, || {
        let mut rng = XorShift64::new(1);
        black_box(excel_randarray_fill_naive(
            black_box(rows),
            black_box(cols),
            min,
            max,
            integer,
            &mut rng,
        ));
    });
    let const_ms = time_ms(iters, || {
        let mut rng = XorShift64::new(1);
        black_box(excel_randarray_fill(
            black_box(rows),
            black_box(cols),
            7.0,
            7.0,
            integer,
            &mut rng,
        ));
    });
    let speedup = naive_ms / fast_ms.max(1e-9);
    let kind = if integer { "int" } else { "dec" };
    println!(
        "kernel {rows:>5}×{cols:<5} {kind}  cells={cells:<7}  naive={naive_ms:.4}ms  fast={fast_ms:.4}ms  const={const_ms:.4}ms  speedup={speedup:.2}x"
    );
}

fn evaluate_bench(rows: u32, cols: u32, integer: bool, iters: u32) {
    let flag = if integer { ",1,100,TRUE" } else { "" };
    let formula = format!("=RANDARRAY({rows},{cols}{flag})");
    let spec = EvalSpec::formula("bench.randarray", formula.clone());
    let engine = CalcCoreEngine::new();
    let got = engine.evaluate(&spec).expect("evaluate");
    match &got {
        ExcelValue::Array(grid) => {
            assert_eq!(grid.len(), rows as usize, "{formula} row count");
            assert_eq!(grid[0].len(), cols as usize, "{formula} col count");
        }
        other => panic!("expected array, got {other}"),
    }
    let ms = time_ms(iters, || {
        black_box(engine.evaluate(black_box(&spec)).unwrap());
    });
    println!("evaluate {rows:>5}×{cols:<5}  {ms:.4}ms/call  formula={formula}");
}

fn main() {
    println!("RANDARRAY bench (pre-sized fill vs naive push; calc-core evaluate)\n");
    kernel_bench(10_000, 1, false, 40);
    kernel_bench(100, 100, false, 40);
    kernel_bench(10_000, 1, true, 40);
    kernel_bench(100, 100, true, 40);
    kernel_bench(1_000, 100, false, 12);
    println!();
    evaluate_bench(10_000, 1, false, 20);
    evaluate_bench(100, 100, true, 20);
    evaluate_bench(1_000, 100, false, 8);
}
