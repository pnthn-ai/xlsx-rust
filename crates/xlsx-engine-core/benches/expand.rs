//! EXPAND hot-path bench: naive clone/rebuild vs one-pass fill,
//! plus a full `calc-core` evaluate of EXPAND(A1:An, rows, cols, pad).

use std::hint::black_box;
use std::time::Instant;
use xlsx_engine_core::{excel_expand, excel_expand_naive, CalcCoreEngine};
use xlsx_types::{Candidate, Cell, EvalSpec, EvalTarget, ExcelError, ExcelValue, Sheet, Workbook};

fn column(n: usize) -> ExcelValue {
    ExcelValue::Array((0..n).map(|i| vec![ExcelValue::Number(i as f64)]).collect())
}

fn grid(rows: usize, cols: usize) -> ExcelValue {
    ExcelValue::Array(
        (0..rows)
            .map(|r| {
                (0..cols)
                    .map(|c| ExcelValue::Number((r * cols + c) as f64))
                    .collect()
            })
            .collect(),
    )
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

fn kernel_bench(name: &str, arr: &ExcelValue, rows: Option<f64>, cols: Option<f64>, iters: u32) {
    let pad = ExcelValue::Error(ExcelError::Na);
    let naive_ms = time_ms(iters, || {
        black_box(excel_expand_naive(
            black_box(arr),
            black_box(rows),
            black_box(cols),
            black_box(&pad),
        ));
    });
    let fast_ms = time_ms(iters, || {
        black_box(excel_expand(
            black_box(arr),
            black_box(rows),
            black_box(cols),
            black_box(&pad),
        ));
    });
    let speedup = naive_ms / fast_ms.max(1e-9);
    println!(
        "kernel {name:<28} rows={rows:?} cols={cols:?}  naive={naive_ms:.4}ms  fast={fast_ms:.4}ms  speedup={speedup:.2}x"
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

fn evaluate_bench(n: u32, rows: u32, cols: u32, iters: u32) {
    let wb = workbook_n(n);
    let formula = format!("=EXPAND(A1:A{n}, {rows}, {cols}, 0)");
    let spec = EvalSpec {
        case_id: "bench.expand".into(),
        workbook: wb,
        target: EvalTarget::formula(formula),
        options: Default::default(),
    };
    let engine = CalcCoreEngine::new();
    let got = engine.evaluate(&spec).expect("evaluate");
    match &got {
        ExcelValue::Array(out) => {
            assert_eq!(
                out.len(),
                rows as usize,
                "EXPAND(A1:A{n}, {rows}, {cols}) got {} rows",
                out.len()
            );
            assert_eq!(out[0].len(), cols as usize);
        }
        other => panic!("expected array, got {other}"),
    }

    let ms = time_ms(iters, || {
        black_box(engine.evaluate(black_box(&spec)).unwrap());
    });
    println!(
        "evaluate n={n:>6} -> {rows}x{cols}  {ms:.4}ms/call  formula=EXPAND(A1:A{n}, {rows}, {cols}, 0)"
    );
}

fn main() {
    println!("EXPAND bench (calc-core one-pass fill vs clone/rebuild)\n");
    kernel_bench(
        "10k col → 10k×8",
        &column(10_000),
        Some(10_000.0),
        Some(8.0),
        20,
    );
    kernel_bench("10k col → 12k×1", &column(10_000), Some(12_000.0), None, 20);
    kernel_bench("2k×2 → 4k×8", &grid(2_000, 2), Some(4_000.0), Some(8.0), 16);
    kernel_bench(
        "8k×32-char → 8k×4",
        &text_column(8_000, 32),
        Some(8_000.0),
        Some(4.0),
        12,
    );
    kernel_bench(
        "100k col → 100k×4",
        &column(100_000),
        Some(100_000.0),
        Some(4.0),
        6,
    );
    println!();
    evaluate_bench(10_000, 10_000, 4, 6);
    evaluate_bench(100_000, 100_000, 2, 3);
}
