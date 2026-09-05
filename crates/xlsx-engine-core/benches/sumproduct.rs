//! SUMPRODUCT hot-path bench: naive ExcelValue walk vs packed f64 kernel,
//! plus a full `calc-core` evaluate of large ranges (10k / 100k).

use std::hint::black_box;
use std::time::Instant;
use xlsx_engine_core::{product_sum, product_sum_naive, product_sum_packed, CalcCoreEngine};
use xlsx_types::{
    Candidate, Cell, CellAddr, EvalSpec, EvalTarget, ExcelValue, Sheet, Workbook,
};

fn fill_row(n: usize, start: f64, step: f64) -> ExcelValue {
    ExcelValue::Array(vec![(0..n)
        .map(|i| ExcelValue::Number(start + i as f64 * step))
        .collect()])
}

fn time_ms(iters: u32, mut f: impl FnMut()) -> f64 {
    // Warmup
    f();
    let t0 = Instant::now();
    for _ in 0..iters {
        f();
    }
    t0.elapsed().as_secs_f64() * 1000.0 / f64::from(iters)
}

fn kernel_bench(n: usize, iters: u32) {
    let a = fill_row(n, 1.0, 0.001);
    let b = fill_row(n, 2.0, 0.002);
    let arrays = [a.clone(), b.clone()];

    let naive_ms = time_ms(iters, || {
        black_box(product_sum_naive(black_box(&arrays)));
    });
    let packed_ms = time_ms(iters, || {
        black_box(product_sum(black_box(&arrays)));
    });

    let a_f: Vec<f64> = (0..n).map(|i| 1.0 + i as f64 * 0.001).collect();
    let b_f: Vec<f64> = (0..n).map(|i| 2.0 + i as f64 * 0.002).collect();
    let packed_only = [a_f, b_f];
    let raw_ms = time_ms(iters, || {
        black_box(product_sum_packed(black_box(&packed_only)));
    });

    let speedup = naive_ms / packed_ms.max(1e-9);
    println!(
        "kernel n={n:>6}  naive={naive_ms:.4}ms  packed={packed_ms:.4}ms  raw_f64={raw_ms:.4}ms  speedup={speedup:.2}x"
    );
}

fn workbook_n(n: u32) -> Workbook {
    let mut sheet = Sheet::new("Sheet1");
    for i in 0..n {
        let a = format!("A{}", i + 1);
        let b = format!("B{}", i + 1);
        sheet.cells.insert(
            a,
            Cell::value(ExcelValue::Number(1.0 + i as f64 * 0.001)),
        );
        sheet.cells.insert(
            b,
            Cell::value(ExcelValue::Number(2.0 + i as f64 * 0.002)),
        );
    }
    Workbook {
        sheets: vec![sheet],
        names: vec![],
    }
}

fn evaluate_bench(n: u32, iters: u32) {
    let wb = workbook_n(n);
    let formula = format!("=SUMPRODUCT(A1:A{n},B1:B{n})");
    let spec = EvalSpec {
        case_id: "bench.sumproduct".into(),
        workbook: wb,
        target: EvalTarget::formula(formula),
        options: Default::default(),
    };
    let engine = CalcCoreEngine::new();
    let expected = {
        let mut acc = 0.0;
        for i in 0..n {
            acc += (1.0 + i as f64 * 0.001) * (2.0 + i as f64 * 0.002);
        }
        acc
    };
    let got = engine.evaluate(&spec).expect("evaluate");
    match got {
        ExcelValue::Number(v) => {
            assert!(
                (v - expected).abs() / expected.max(1.0) < 1e-9,
                "got {v} expected {expected}"
            );
        }
        other => panic!("expected number, got {other}"),
    }

    let ms = time_ms(iters, || {
        black_box(engine.evaluate(black_box(&spec)).unwrap());
    });
    println!("evaluate n={n:>6}  {ms:.4}ms/call  formula=SUMPRODUCT(A1:A{n},B1:B{n})");
    let _ = CellAddr::new(0, 0);
}

fn main() {
    println!("SUMPRODUCT bench (calc-core packed kernel vs naive ExcelValue walk)\n");
    kernel_bench(10_000, 40);
    kernel_bench(100_000, 12);
    println!();
    evaluate_bench(10_000, 8);
    evaluate_bench(100_000, 3);
}
