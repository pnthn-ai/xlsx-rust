//! UNIQUE hot-path bench: naive pairwise distinctness vs the hash kernel,
//! plus a full `calc-core` evaluate of UNIQUE(A1:An).

use std::hint::black_box;
use std::time::Instant;
use xlsx_engine_core::{unique_apply, unique_apply_naive, CalcCoreEngine};
use xlsx_types::{Candidate, Cell, EvalSpec, EvalTarget, ExcelValue, Sheet, Workbook};

fn column(n: usize, period: usize) -> Vec<Vec<ExcelValue>> {
    (0..n)
        .map(|i| vec![ExcelValue::Number((i % period) as f64)])
        .collect()
}

fn time_ms(iters: u32, mut f: impl FnMut()) -> f64 {
    f();
    let t0 = Instant::now();
    for _ in 0..iters {
        f();
    }
    t0.elapsed().as_secs_f64() * 1000.0 / f64::from(iters)
}

fn kernel_bench(n: usize, period: usize, iters: u32, run_naive: bool) {
    let grid = column(n, period);

    let hash_ms = time_ms(iters, || {
        black_box(unique_apply(black_box(&grid), false, false));
    });
    let once_ms = time_ms(iters, || {
        black_box(unique_apply(black_box(&grid), false, true));
    });

    if run_naive {
        let naive_ms = time_ms(iters, || {
            black_box(unique_apply_naive(black_box(&grid), false, false));
        });
        let speedup = naive_ms / hash_ms.max(1e-9);
        println!(
            "kernel n={n:>6} period={period:<5}  naive={naive_ms:.4}ms  hash={hash_ms:.4}ms  exactly_once={once_ms:.4}ms  speedup={speedup:.2}x"
        );
    } else {
        println!(
            "kernel n={n:>6} period={period:<5}  naive=skipped  hash={hash_ms:.4}ms  exactly_once={once_ms:.4}ms"
        );
    }
}

fn workbook_n(n: u32, period: u32) -> Workbook {
    let mut sheet = Sheet::new("Sheet1");
    for i in 0..n {
        let a1 = format!("A{}", i + 1);
        sheet.cells.insert(
            a1,
            Cell::value(ExcelValue::Number((i % period) as f64)),
        );
    }
    Workbook {
        sheets: vec![sheet],
        names: vec![],
    }
}

fn evaluate_bench(n: u32, period: u32, iters: u32) {
    let wb = workbook_n(n, period);
    let formula = format!("=UNIQUE(A1:A{n})");
    let spec = EvalSpec {
        case_id: "bench.unique".into(),
        workbook: wb,
        target: EvalTarget::formula(formula),
        options: Default::default(),
    };
    let engine = CalcCoreEngine::new();
    let got = engine.evaluate(&spec).expect("evaluate");
    let expected_len = period.min(n) as usize;
    match &got {
        ExcelValue::Array(rows) => {
            assert_eq!(
                rows.len(),
                expected_len,
                "UNIQUE(A1:A{n}) period={period} got {} rows",
                rows.len()
            );
        }
        other => panic!("expected array, got {other}"),
    }

    let ms = time_ms(iters, || {
        black_box(engine.evaluate(black_box(&spec)).unwrap());
    });
    println!("evaluate n={n:>6} period={period:<5}  {ms:.4}ms/call  formula=UNIQUE(A1:A{n})");
}

fn main() {
    println!("UNIQUE bench (calc-core hash kernel vs naive pairwise)\n");
    kernel_bench(10_000, 100, 30, true);
    kernel_bench(10_000, 10_000, 12, true);
    kernel_bench(100_000, 1_000, 6, true);
    // All-distinct 100k is O(n²) on the naive path — hash only.
    kernel_bench(100_000, 100_000, 8, false);
    println!();
    evaluate_bench(10_000, 100, 8);
    evaluate_bench(100_000, 1_000, 3);
}
