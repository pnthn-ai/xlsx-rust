//! Before/after microbench for Excel `SWITCH` vs nested `IF`.
//!
//! 1. **Match kernel** — specialized type dispatch (`excel_switch_first_match`)
//!    vs nested-`IF` style (`expr=value` + clone on every pair).
//! 2. **Formula** — `SWITCH` evaluates the expression once; the equivalent
//!    nested `IF(expr=v1, r1, IF(expr=v2, …))` re-evaluates it on every pair.
//!    Unused `SWITCH` results are not evaluated (eager path vs short-circuit).
//!
//! ```text
//! cargo bench -p xlsx-engine-core --bench switch
//! ```

use std::hint::black_box;
use std::time::{Duration, Instant};
use xlsx_engine_core::{
    excel_switch_first_match, excel_switch_first_match_naive, excel_switch_pick_evaluated,
    CalcCoreEngine,
};
use xlsx_types::{Candidate, Cell, CellAddr, EvalSpec, EvalTarget, ExcelValue, Sheet, Workbook};

const ITERS_KERNEL: u32 = 80;
const ITERS_FORMULA: u32 = 40;

fn time_it(iters: u32, mut f: impl FnMut()) -> Duration {
    f();
    let start = Instant::now();
    for _ in 0..iters {
        f();
    }
    start.elapsed() / iters
}

fn fmt_dur(d: Duration) -> String {
    let us = d.as_secs_f64() * 1e6;
    if us >= 1000.0 {
        format!("{:.2} ms", us / 1000.0)
    } else {
        format!("{us:.1} µs")
    }
}

fn numeric_values(n: usize) -> Vec<ExcelValue> {
    (1..=n).map(|i| ExcelValue::Number(i as f64)).collect()
}

fn sum_range_wb(rows: u32) -> Workbook {
    let mut sheet = Sheet::new("Sheet1");
    for i in 0..rows {
        sheet.insert(
            CellAddr::new(0, i),
            Cell::value(ExcelValue::Number((i + 1) as f64)),
        );
    }
    // Stored formula: expensive expression used as SWITCH / IF input.
    sheet.insert(
        CellAddr::new(1, 0),
        Cell::formula(format!("=SUM(A1:A{rows})"), None),
    );
    Workbook {
        sheets: vec![sheet],
        names: vec![],
    }
}

fn spec(wb: Workbook, formula: &str) -> EvalSpec {
    EvalSpec {
        case_id: "bench.switch".into(),
        workbook: wb,
        target: EvalTarget::formula(formula),
        options: Default::default(),
    }
}

fn switch_pairs(expr: &str, n: usize, hit: usize, miss_result: &str) -> String {
    let mut args = expr.to_string();
    for i in 1..=n {
        args.push_str(&format!(", {i}, "));
        if i == hit {
            args.push_str(&format!("{i}"));
        } else {
            args.push_str(miss_result);
        }
    }
    format!("=SWITCH({args})")
}

fn nested_if_pairs(expr: &str, n: usize, hit: usize) -> String {
    // IF(expr=1, …, IF(expr=2, …)) re-evaluates `expr` (and thus SUM) each level.
    // Fallback is NA() so a miss matches SWITCH's #N/A (not IF's FALSE).
    let mut s = String::from("NA()");
    for i in (1..=n).rev() {
        let result = if i == hit { format!("{i}") } else { "0".into() };
        s = format!("IF({expr}={i}, {result}, {s})");
    }
    format!("={s}")
}

fn main() {
    println!("SWITCH kernel + formula bench (exact-match scan / vs nested IF)");
    println!(
        "{:<48} {:>12} {:>12} {:>8}",
        "case", "naive", "optimized", "speedup"
    );
    println!("{}", "-".repeat(84));

    let pairs_late = numeric_values(126);
    let expr_late = ExcelValue::Number(126.0);
    let pairs_miss = numeric_values(126);
    let expr_miss = ExcelValue::Number(0.0);
    let pairs_text: Vec<ExcelValue> = (0..126)
        .map(|i| ExcelValue::Text(format!("key-{i:03}")))
        .collect();
    let expr_text = ExcelValue::Text("key-125".into());

    let kernel_cases: [(&str, ExcelValue, Vec<ExcelValue>); 3] = [
        ("126 numeric pairs, last hits", expr_late, pairs_late),
        ("126 numeric pairs, miss", expr_miss, pairs_miss),
        (
            "126 text pairs, last hits (casefold)",
            expr_text,
            pairs_text,
        ),
    ];
    for (name, expr, values) in kernel_cases {
        let naive = time_it(ITERS_KERNEL, || {
            let _ = black_box(excel_switch_first_match_naive(
                black_box(&expr),
                black_box(&values),
            ));
        });
        let fast = time_it(ITERS_KERNEL, || {
            let _ = black_box(excel_switch_first_match(
                black_box(&expr),
                black_box(&values),
            ));
        });
        let speedup = naive.as_secs_f64() / fast.as_secs_f64().max(1e-12);
        println!(
            "{:<48} {:>12} {:>12} {:>7.1}x",
            name,
            fmt_dur(naive),
            fmt_dur(fast),
            speedup
        );
        assert_eq!(
            excel_switch_first_match_naive(&expr, &values),
            excel_switch_first_match(&expr, &values),
            "kernel mismatch on {name}"
        );
    }

    let engine = CalcCoreEngine::new();
    let rows = 4000u32;
    let wb = sum_range_wb(rows);
    let closed_form: f64 = (rows * (rows + 1) / 2) as f64;

    // SWITCH vs nested IF: expression is SUM/divisor (= hit). SWITCH evals it
    // once; 16-deep IF evals it once per level (stack stays under the 64 cap).
    let n_if = 16usize;
    let divisor = closed_form / n_if as f64;
    let expensive = format!("B1/{divisor}");
    let switch_late = switch_pairs(&expensive, n_if, n_if, "0");
    let if_late = nested_if_pairs(&expensive, n_if, n_if);
    let switch_spec = spec(wb.clone(), &switch_late);
    let if_spec = spec(wb.clone(), &if_late);

    let if_t = time_it(ITERS_FORMULA, || {
        let _ = black_box(engine.evaluate(black_box(&if_spec)).unwrap());
    });
    let sw_t = time_it(ITERS_FORMULA, || {
        let _ = black_box(engine.evaluate(black_box(&switch_spec)).unwrap());
    });
    let speedup = if_t.as_secs_f64() / sw_t.as_secs_f64().max(1e-12);
    println!(
        "{:<48} {:>12} {:>12} {:>7.1}x",
        "16-pair late hit: nested IF vs SWITCH",
        fmt_dur(if_t),
        fmt_dur(sw_t),
        speedup
    );
    let sw_v = engine.evaluate(&switch_spec).unwrap();
    let if_v = engine.evaluate(&if_spec).unwrap();
    assert_eq!(sw_v, ExcelValue::Number(n_if as f64));
    assert_eq!(if_v, ExcelValue::Number(n_if as f64));

    // Eager vs short-circuit: unused results are SUM(A1:A4000). SWITCH(1, 1, 1,
    // 2, SUM(...), …) must not evaluate those SUMs.
    let n_eager = 32usize;
    let short_f = switch_pairs("1", n_eager, 1, "SUM(A1:A4000)");
    let short_spec = spec(wb.clone(), &short_f);
    let short_t = time_it(ITERS_FORMULA, || {
        let _ = black_box(engine.evaluate(black_box(&short_spec)).unwrap());
    });

    // Eager: evaluate every unused result (`SUM` of 4k cells) then pick.
    let unused_sum = spec(wb, "=SUM(A1:A4000)");
    let eager_t = time_it(ITERS_FORMULA, || {
        let mut args = Vec::with_capacity(1 + n_eager * 2);
        args.push(ExcelValue::Number(1.0));
        for i in 1..=n_eager {
            args.push(ExcelValue::Number(i as f64));
            if i == 1 {
                args.push(ExcelValue::Number(1.0));
            } else {
                args.push(engine.evaluate(black_box(&unused_sum)).unwrap());
            }
        }
        let _ = black_box(excel_switch_pick_evaluated(black_box(&args)));
    });
    let eager_speedup = eager_t.as_secs_f64() / short_t.as_secs_f64().max(1e-12);
    println!(
        "{:<48} {:>12} {:>12} {:>7.1}x",
        "32-pair early hit: eager results vs SWITCH",
        fmt_dur(eager_t),
        fmt_dur(short_t),
        eager_speedup
    );
    assert_eq!(
        engine.evaluate(&short_spec).unwrap(),
        ExcelValue::Number(1.0)
    );
    let mut check = vec![ExcelValue::Number(1.0)];
    for i in 1..=n_eager {
        check.push(ExcelValue::Number(i as f64));
        check.push(if i == 1 {
            ExcelValue::Number(1.0)
        } else {
            ExcelValue::Number(closed_form)
        });
    }
    assert_eq!(excel_switch_pick_evaluated(&check), ExcelValue::Number(1.0));
}
