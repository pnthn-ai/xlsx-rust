//! VSTACK hot-path bench: grow-and-repad baseline vs the prealloc kernel,
//! plus a full `calc-core` evaluate of VSTACK(A1:An, B1:Bn).

use std::hint::black_box;
use std::time::Instant;
use xlsx_engine_core::{excel_vstack, excel_vstack_naive, excel_vstack_owned, CalcCoreEngine};
use xlsx_types::{Candidate, Cell, EvalSpec, EvalTarget, ExcelValue, Sheet, Workbook};

fn time_ms(iters: u32, mut f: impl FnMut()) -> f64 {
    f();
    let t0 = Instant::now();
    for _ in 0..iters {
        f();
    }
    t0.elapsed().as_secs_f64() * 1000.0 / f64::from(iters)
}

fn matrix(rows: usize, cols: usize, tag: f64) -> ExcelValue {
    ExcelValue::Array(
        (0..rows)
            .map(|r| {
                (0..cols)
                    .map(|c| ExcelValue::Number(tag + (r * cols + c) as f64))
                    .collect()
            })
            .collect(),
    )
}

fn kernel_case(name: &str, args: Vec<ExcelValue>, iters: u32) {
    let got = excel_vstack(&args);
    let naive = excel_vstack_naive(&args);
    let owned = excel_vstack_owned(args.clone());
    assert_eq!(got, naive, "kernel mismatch: {name}");
    assert_eq!(got, owned, "owned mismatch: {name}");

    let fast_ms = time_ms(iters, || {
        black_box(excel_vstack(black_box(&args)));
    });
    let owned_ms = time_ms(iters, || {
        black_box(excel_vstack_owned(black_box(args.clone())));
    });
    let naive_ms = time_ms(iters, || {
        black_box(excel_vstack_naive(black_box(&args)));
    });
    let speedup = naive_ms / fast_ms.max(1e-9);
    println!(
        "kernel {name:<28}  naive={naive_ms:.4}ms  prealloc={fast_ms:.4}ms  owned={owned_ms:.4}ms  speedup={speedup:.2}x"
    );
}

fn workbook_two_cols(n: u32) -> Workbook {
    let mut sheet = Sheet::new("Sheet1");
    for i in 0..n {
        let r = i + 1;
        sheet
            .cells
            .insert(format!("A{r}"), Cell::value(ExcelValue::Number(i as f64)));
        sheet.cells.insert(
            format!("B{r}"),
            Cell::value(ExcelValue::Number((i + 1_000) as f64)),
        );
        sheet.cells.insert(
            format!("C{r}"),
            Cell::value(ExcelValue::Number((i + 2_000) as f64)),
        );
    }
    Workbook {
        sheets: vec![sheet],
        names: vec![],
    }
}

fn evaluate_bench(n: u32, formula: &str, expect_rows: usize, expect_cols: usize, iters: u32) {
    let spec = EvalSpec {
        case_id: "bench.vstack".into(),
        workbook: workbook_two_cols(n),
        target: EvalTarget::formula(formula.to_string()),
        options: Default::default(),
    };
    let engine = CalcCoreEngine::new();
    match engine.evaluate(&spec).expect("evaluate") {
        ExcelValue::Array(rows) => {
            assert_eq!(rows.len(), expect_rows, "{formula} row count");
            assert_eq!(rows[0].len(), expect_cols, "{formula} col count");
        }
        other => panic!("expected array, got {other}"),
    }
    let ms = time_ms(iters, || {
        black_box(engine.evaluate(black_box(&spec)).unwrap());
    });
    println!("evaluate n={n:>6}  {ms:.4}ms/call  formula={formula}");
}

fn main() {
    println!("VSTACK bench (calc-core prealloc kernel vs immutable-append naive)\n");

    kernel_case(
        "2× (8k×4 equal width)",
        vec![matrix(8_192, 4, 0.0), matrix(8_192, 4, 10_000.0)],
        20,
    );
    kernel_case(
        "wide then 32 narrow 4k",
        {
            let mut args = vec![matrix(4_096, 16, 0.0)];
            for i in 0..32 {
                args.push(matrix(4_096, 2, 1_000.0 * (i + 1) as f64));
            }
            args
        },
        8,
    );
    kernel_case(
        "32 narrow then wide 4k",
        {
            let mut args = Vec::new();
            for i in 0..32 {
                args.push(matrix(4_096, 2, 1_000.0 * (i + 1) as f64));
            }
            args.push(matrix(4_096, 16, 0.0));
            args
        },
        8,
    );
    kernel_case(
        "3 scalars + 2k×1",
        vec![
            ExcelValue::Number(1.0),
            ExcelValue::Number(2.0),
            ExcelValue::Number(3.0),
            matrix(2_048, 1, 10.0),
        ],
        40,
    );
    kernel_case(
        "64 × 2k×2 equal width",
        (0..64)
            .map(|i| matrix(2_048, 2, 1_000.0 * (i + 1) as f64))
            .collect(),
        6,
    );

    println!();
    evaluate_bench(10_000, "=VSTACK(A1:A10000, B1:B10000)", 20_000, 1, 6);
    evaluate_bench(10_000, "=VSTACK(A1:C5000, A5001:B10000)", 10_000, 3, 6);
}
