//! Compact JSON/CSV timings without Criterion HTML (and without live Excel).
//!
//! Example:
//! ```text
//! cargo run -p xlsx-bench -- --function SUM --rows 10000 --format json
//! cargo run -p xlsx-bench -- --function SUM --rows 10000 --format csv -o /tmp/sum.csv
//! ```
//!
//! This is a snapshot, not a substitute for `xlsx-verify`. Correctness stays
//! the hard gate.

use clap::{Parser, ValueEnum};
use std::io::{self, Write};
use std::path::PathBuf;
use xlsx_bench::export::{
    measure_evaluate, write_csv, write_csv_to, write_json, write_json_stdout, TimingReport,
};
use xlsx_bench::snippet::{mixed_column, numeric_column};
use xlsx_engine_core::CalcCoreEngine;

#[derive(Clone, Copy, Debug, ValueEnum)]
enum Format {
    Json,
    Csv,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum Shape {
    /// `A1:A{rows}` filled with `1..=rows`.
    Numeric,
    /// Number / blank / text / bool cycle (SUM skip path).
    Mixed,
}

#[derive(Parser, Debug)]
#[command(
    name = "xlsx-bench-snapshot",
    about = "Write JSON/CSV timings for one calc-core function over a generated range"
)]
struct Args {
    /// Excel function name (`SUM`, `AVERAGE`, …). Formula becomes `=FN(range)`.
    #[arg(long, default_value = "SUM")]
    function: String,

    /// Rows in column A (10k–100k is the intended scale).
    #[arg(long, default_value_t = 10_000)]
    rows: u32,

    #[arg(long, value_enum, default_value_t = Shape::Numeric)]
    shape: Shape,

    #[arg(long, default_value_t = 2)]
    warmup: u32,

    #[arg(long, default_value_t = 8)]
    iters: u32,

    #[arg(long, value_enum, default_value_t = Format::Json)]
    format: Format,

    /// Output path. Omit or `-` for stdout.
    #[arg(short, long)]
    output: Option<PathBuf>,
}

fn main() {
    let args = Args::parse();
    if args.rows == 0 {
        eprintln!("--rows must be > 0");
        std::process::exit(2);
    }

    let range = match args.shape {
        Shape::Numeric => numeric_column(args.rows, |i| (i + 1) as f64),
        Shape::Mixed => mixed_column(args.rows),
    };
    let formula = range.call(&args.function);
    let spec = range.call_spec(
        format!(
            "{}.{}",
            args.function.to_ascii_lowercase(),
            range.cell_count
        ),
        &args.function,
    );

    let rec = match measure_evaluate(
        &CalcCoreEngine::new(),
        &spec,
        args.function.clone(),
        formula,
        range.cell_count,
        args.warmup,
        args.iters,
    ) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("evaluate failed: {e}");
            std::process::exit(1);
        }
    };

    let report = TimingReport::new("calc-core", vec![rec]);
    if let Err(e) = emit(&args, &report) {
        eprintln!("write failed: {e}");
        std::process::exit(1);
    }
}

fn emit(args: &Args, report: &TimingReport) -> io::Result<()> {
    let to_stdout = args
        .output
        .as_ref()
        .map(|p| p.as_os_str() == "-")
        .unwrap_or(true);
    match (args.format, to_stdout) {
        (Format::Json, true) => write_json_stdout(report),
        (Format::Json, false) => write_json(args.output.as_ref().unwrap(), report),
        (Format::Csv, true) => write_csv_to(io::stdout().lock(), report),
        (Format::Csv, false) => write_csv(args.output.as_ref().unwrap(), report),
    }?;
    if !to_stdout {
        let _ = writeln!(
            io::stderr(),
            "wrote {} ({} bench(es))",
            args.output.as_ref().unwrap().display(),
            report.benches.len()
        );
    }
    Ok(())
}
