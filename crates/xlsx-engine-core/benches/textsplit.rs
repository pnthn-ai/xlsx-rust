//! Before/after microbench for Excel `TEXTSPLIT`.
//!
//! Compares the `Vec<char>` / try-every-index baseline (`excel_textsplit_naive`)
//! with the production kernel (`excel_textsplit`: ASCII-byte / `str::find`
//! / earliest-of-N delimiters).
//!
//! ```text
//! cargo bench -p xlsx-engine-core --bench textsplit
//! ```

use std::hint::black_box;
use std::time::{Duration, Instant};
use xlsx_engine_core::{excel_textsplit, excel_textsplit_naive, CalcCoreEngine};
use xlsx_types::{Candidate, Cell, EvalSpec, EvalTarget, ExcelError, ExcelValue, Sheet, Workbook};

const ITERS: u32 = 40;
const PAD: ExcelValue = ExcelValue::Error(ExcelError::Na);

struct Case {
    name: &'static str,
    text: String,
    col: Vec<String>,
    row: Vec<String>,
    ignore_empty: bool,
    case_insensitive: bool,
}

fn csv_line(n: usize, delim: &str) -> String {
    (0..n)
        .map(|i| format!("tok{i:04}"))
        .collect::<Vec<_>>()
        .join(delim)
}

fn csv_grid(rows: usize, cols: usize) -> String {
    (0..rows)
        .map(|r| {
            (0..cols)
                .map(|c| format!("r{r:03}c{c}"))
                .collect::<Vec<_>>()
                .join(",")
        })
        .collect::<Vec<_>>()
        .join(";")
}

fn cases() -> Vec<Case> {
    vec![
        Case {
            name: "8k comma cols",
            text: csv_line(8_192, ","),
            col: vec![",".into()],
            row: vec![],
            ignore_empty: false,
            case_insensitive: false,
        },
        Case {
            name: "8k comma cols, ignore empty",
            text: csv_line(8_192, ",,"),
            col: vec![",".into()],
            row: vec![],
            ignore_empty: true,
            case_insensitive: false,
        },
        Case {
            name: "256×16 2-D pad",
            text: csv_grid(256, 16),
            col: vec![",".into()],
            row: vec![";".into()],
            ignore_empty: false,
            case_insensitive: false,
        },
        Case {
            name: "4k multi-delim {,;}",
            text: (0..4_096)
                .map(|i| {
                    if i % 2 == 0 {
                        format!("a{i},")
                    } else {
                        format!("b{i};")
                    }
                })
                .collect::<String>(),
            col: vec![",".into(), ";".into()],
            row: vec![],
            ignore_empty: false,
            case_insensitive: false,
        },
        Case {
            name: "4k case-insensitive ' x '",
            text: (0..4_096)
                .map(|i| {
                    if i % 2 == 0 {
                        format!("w{i} x ")
                    } else {
                        format!("w{i} X ")
                    }
                })
                .collect::<String>(),
            col: vec![" x ".into()],
            row: vec![],
            ignore_empty: false,
            case_insensitive: true,
        },
        Case {
            name: "8k no-match (original text)",
            text: csv_line(8_192, ","),
            col: vec!["|".into()],
            row: vec![],
            ignore_empty: false,
            case_insensitive: false,
        },
    ]
}

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

fn workbook_csv(n: u32) -> Workbook {
    let mut sheet = Sheet::new("Sheet1");
    sheet.cells.insert(
        "A1".into(),
        Cell::value(ExcelValue::Text(csv_line(n as usize, ","))),
    );
    Workbook {
        sheets: vec![sheet],
        names: vec![],
    }
}

fn evaluate_bench(n: u32, iters: u32) {
    let wb = workbook_csv(n);
    let spec = EvalSpec {
        case_id: "bench.textsplit".into(),
        workbook: wb,
        target: EvalTarget::formula("=TEXTSPLIT(A1,\",\")"),
        options: Default::default(),
    };
    let engine = CalcCoreEngine::new();
    let got = engine.evaluate(&spec).expect("evaluate");
    match &got {
        ExcelValue::Array(rows) => {
            assert_eq!(rows.len(), 1);
            assert_eq!(rows[0].len(), n as usize);
        }
        other => panic!("expected array, got {other}"),
    }
    let ms = time_it(iters, || {
        black_box(engine.evaluate(black_box(&spec)).unwrap());
    });
    println!(
        "evaluate n={n:>6}  {}/call  formula=TEXTSPLIT(A1,\",\")",
        fmt_dur(ms)
    );
}

fn main() {
    println!("TEXTSPLIT kernel bench (Vec<char> scan vs find / ASCII-byte)");
    println!(
        "{:<36} {:>12} {:>12} {:>8}",
        "case", "naive", "optimized", "speedup"
    );
    println!("{}", "-".repeat(72));
    for c in cases() {
        let naive = time_it(ITERS, || {
            let _ = black_box(excel_textsplit_naive(
                black_box(&c.text),
                black_box(&c.col),
                black_box(&c.row),
                black_box(c.ignore_empty),
                black_box(c.case_insensitive),
                black_box(&PAD),
            ));
        });
        let fast = time_it(ITERS, || {
            let _ = black_box(excel_textsplit(
                black_box(&c.text),
                black_box(&c.col),
                black_box(&c.row),
                black_box(c.ignore_empty),
                black_box(c.case_insensitive),
                black_box(&PAD),
            ));
        });
        let speedup = naive.as_secs_f64() / fast.as_secs_f64().max(1e-12);
        println!(
            "{:<36} {:>12} {:>12} {:>7.1}x",
            c.name,
            fmt_dur(naive),
            fmt_dur(fast),
            speedup
        );
        let a = excel_textsplit_naive(
            &c.text,
            &c.col,
            &c.row,
            c.ignore_empty,
            c.case_insensitive,
            &PAD,
        );
        let b = excel_textsplit(
            &c.text,
            &c.col,
            &c.row,
            c.ignore_empty,
            c.case_insensitive,
            &PAD,
        );
        assert_eq!(a, b, "semantic mismatch on {}", c.name);
    }
    println!();
    evaluate_bench(10_000, 8);
    evaluate_bench(50_000, 4);
}
