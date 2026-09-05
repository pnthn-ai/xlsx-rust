//! WRAPROWS hot-path bench: naive flatten/pad/chunk vs one-pass fill,
//! plus a full `calc-core` evaluate of WRAPROWS(A1:An, k).

use std::hint::black_box;
use std::time::Instant;
use xlsx_engine_core::{excel_wraprows, excel_wraprows_naive, CalcCoreEngine};
use xlsx_types::{Candidate, Cell, EvalSpec, EvalTarget, ExcelError, ExcelValue, Sheet, Workbook};

fn column(n: usize) -> ExcelValue {
    ExcelValue::Array((0..n).map(|i| vec![ExcelValue::Number(i as f64)]).collect())
}

fn text_column(n: usize, payload: usize) -> ExcelValue {
    let pad = "x".repeat(payload);
    ExcelValue::Array(
        (0..n)
            .map(|i| vec![ExcelValue::Text(format!("{i:05}-{pad}"))])
            .collect(),
    )
}

fn time_ms(iters: u32, mut f: impl FnMut()) -> f64 {
    f();
    let t0 = Instant::now();
    for _ in 0..iters {
        f();
    }
    t0.elapsed().as_secs_f64() * 1000.0 / f64::from(iters)
}

fn kernel_bench(name: &str, vec: &ExcelValue, wrap: f64, iters: u32) {
    let pad = ExcelValue::Error(ExcelError::Na);
    let naive_ms = time_ms(iters, || {
        black_box(excel_wraprows_naive(
            black_box(vec),
            black_box(wrap),
            black_box(&pad),
        ));
    });
    let fast_ms = time_ms(iters, || {
        black_box(excel_wraprows(
            black_box(vec),
            black_box(wrap),
            black_box(&pad),
        ));
    });
    let speedup = naive_ms / fast_ms.max(1e-9);
    println!(
        "kernel {name:<28} wrap={wrap:<5}  naive={naive_ms:.4}ms  fast={fast_ms:.4}ms  speedup={speedup:.2}x"
    );
}

fn workbook_n(n: u32) -> Workbook {
    let mut sheet = Sheet::new("Sheet1");
    for i in 0..n {
        let a1 = format!("A{}", i + 1);
        sheet
            .cells
            .insert(a1, Cell::value(ExcelValue::Number(i as f64)));
    }
    Workbook {
        sheets: vec![sheet],
        names: vec![],
    }
}

fn evaluate_bench(n: u32, wrap: u32, iters: u32) {
    let wb = workbook_n(n);
    let formula = format!("=WRAPROWS(A1:A{n}, {wrap})");
    let spec = EvalSpec {
        case_id: "bench.wraprows".into(),
        workbook: wb,
        target: EvalTarget::formula(formula),
        options: Default::default(),
    };
    let engine = CalcCoreEngine::new();
    let got = engine.evaluate(&spec).expect("evaluate");
    let expected_rows = (n as usize).div_ceil(wrap as usize);
    match &got {
        ExcelValue::Array(rows) => {
            assert_eq!(
                rows.len(),
                expected_rows,
                "WRAPROWS(A1:A{n}, {wrap}) got {} rows",
                rows.len()
            );
            assert_eq!(rows[0].len(), wrap as usize);
        }
        other => panic!("expected array, got {other}"),
    }

    let ms = time_ms(iters, || {
        black_box(engine.evaluate(black_box(&spec)).unwrap());
    });
    println!("evaluate n={n:>6} wrap={wrap:<5}  {ms:.4}ms/call  formula=WRAPROWS(A1:A{n}, {wrap})");
}

fn main() {
    println!("WRAPROWS bench (calc-core one-pass fill vs flatten/pad/chunk)\n");
    kernel_bench("10k numbers", &column(10_000), 8.0, 40);
    kernel_bench("10k numbers", &column(10_000), 1.0, 40);
    kernel_bench("10k numbers wide pad", &column(10_000), 64.0, 24);
    kernel_bench("8k×32-char text", &text_column(8_000, 32), 8.0, 20);
    kernel_bench("100k numbers", &column(100_000), 16.0, 8);
    println!();
    evaluate_bench(10_000, 8, 8);
    evaluate_bench(100_000, 16, 3);
}
