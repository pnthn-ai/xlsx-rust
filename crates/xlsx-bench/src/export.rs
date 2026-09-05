//! Compact JSON/CSV timing snapshots.
//!
//! Criterion already emits `target/criterion/**/estimates.json`. These types
//! are a smaller, stable schema for later Excel-oracle comparison. Nothing
//! here talks to live Excel.

use serde::{Deserialize, Serialize};
use std::fs::File;
use std::io::{self, Write};
use std::path::Path;
use std::time::{Duration, Instant};
use xlsx_types::{Candidate, EvalSpec, ExcelValue};

/// One timed scenario (one function, one workbook size).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BenchRecord {
    pub id: String,
    pub function: String,
    pub formula: String,
    pub cells: u64,
    pub warmup: u32,
    pub iters: u32,
    pub mean_ns: f64,
    pub min_ns: f64,
    pub max_ns: f64,
    pub stddev_ns: f64,
    /// Candidate result after the last iteration (for a later oracle diff).
    pub result: ExcelValue,
}

/// File-level envelope. `oracle` is reserved; CI stays fixture-only.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TimingReport {
    pub candidate: String,
    /// Always `"none"` in this harness — live Excel is out of scope.
    pub oracle: String,
    pub generated_unix_s: u64,
    pub benches: Vec<BenchRecord>,
}

impl TimingReport {
    pub fn new(candidate: impl Into<String>, benches: Vec<BenchRecord>) -> Self {
        Self {
            candidate: candidate.into(),
            oracle: "none".into(),
            generated_unix_s: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0),
            benches,
        }
    }
}

pub fn write_json(path: &Path, report: &TimingReport) -> io::Result<()> {
    let mut f = File::create(path)?;
    serde_json::to_writer_pretty(&mut f, report)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    f.write_all(b"\n")?;
    Ok(())
}

pub fn write_json_stdout(report: &TimingReport) -> io::Result<()> {
    let mut out = io::stdout().lock();
    serde_json::to_writer_pretty(&mut out, report)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    out.write_all(b"\n")
}

pub fn write_csv(path: &Path, report: &TimingReport) -> io::Result<()> {
    let mut f = File::create(path)?;
    write_csv_to(&mut f, report)
}

pub fn write_csv_to<W: Write>(mut w: W, report: &TimingReport) -> io::Result<()> {
    writeln!(
        w,
        "candidate,oracle,id,function,formula,cells,warmup,iters,mean_ns,min_ns,max_ns,stddev_ns,result"
    )?;
    for b in &report.benches {
        writeln!(
            w,
            "{},{},{},{},{},{},{},{},{:.3},{:.3},{:.3},{:.3},{}",
            csv_escape(&report.candidate),
            csv_escape(&report.oracle),
            csv_escape(&b.id),
            csv_escape(&b.function),
            csv_escape(&b.formula),
            b.cells,
            b.warmup,
            b.iters,
            b.mean_ns,
            b.min_ns,
            b.max_ns,
            b.stddev_ns,
            csv_escape(&b.result.display_compact()),
        )?;
    }
    Ok(())
}

fn csv_escape(s: &str) -> String {
    if s.contains([',', '"', '\n']) {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s.to_string()
    }
}

/// Warm up, then time `iters` evaluations. Returns stats plus the last value.
pub fn measure_evaluate(
    candidate: &dyn Candidate,
    spec: &EvalSpec,
    function: impl Into<String>,
    formula: impl Into<String>,
    cells: u64,
    warmup: u32,
    iters: u32,
) -> Result<BenchRecord, xlsx_types::EvalError> {
    let function = function.into();
    let formula = formula.into();
    let mut last = ExcelValue::Empty;
    for _ in 0..warmup {
        last = candidate.evaluate(spec)?;
    }
    let iters = iters.max(1);
    let mut samples = Vec::with_capacity(iters as usize);
    for _ in 0..iters {
        let t0 = Instant::now();
        last = candidate.evaluate(spec)?;
        samples.push(t0.elapsed());
    }
    Ok(summarize(
        format!("{}.{}", function.to_ascii_lowercase(), cells),
        function,
        formula,
        cells,
        warmup,
        iters,
        &samples,
        last,
    ))
}

#[allow(clippy::too_many_arguments)]
fn summarize(
    id: String,
    function: String,
    formula: String,
    cells: u64,
    warmup: u32,
    iters: u32,
    samples: &[Duration],
    result: ExcelValue,
) -> BenchRecord {
    let ns: Vec<f64> = samples.iter().map(|d| d.as_secs_f64() * 1e9).collect();
    let mean = ns.iter().sum::<f64>() / ns.len() as f64;
    let var = ns.iter().map(|x| (x - mean).powi(2)).sum::<f64>() / ns.len() as f64;
    BenchRecord {
        id,
        function,
        formula,
        cells,
        warmup,
        iters,
        mean_ns: mean,
        min_ns: ns.iter().copied().fold(f64::INFINITY, f64::min),
        max_ns: ns.iter().copied().fold(0.0, f64::max),
        stddev_ns: var.sqrt(),
        result,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::snippet::numeric_column;
    use xlsx_engine_core::CalcCoreEngine;

    #[test]
    fn measure_sum_is_finite_and_correct() {
        let range = numeric_column(200, |i| (i + 1) as f64);
        let formula = range.call("SUM");
        let spec = range.call_spec("sum.200", "SUM");
        let rec = measure_evaluate(
            &CalcCoreEngine::new(),
            &spec,
            "SUM",
            formula,
            range.cell_count,
            1,
            3,
        )
        .unwrap();
        assert!(rec.mean_ns.is_finite() && rec.mean_ns > 0.0);
        match rec.result {
            ExcelValue::Number(v) => assert!((v - 20100.0).abs() < 1e-6),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn csv_and_json_roundtrip_shape() {
        let rec = BenchRecord {
            id: "sum.10".into(),
            function: "SUM".into(),
            formula: "=SUM(A1:A10)".into(),
            cells: 10,
            warmup: 0,
            iters: 1,
            mean_ns: 1.0,
            min_ns: 1.0,
            max_ns: 1.0,
            stddev_ns: 0.0,
            result: ExcelValue::Number(55.0),
        };
        let report = TimingReport::new("calc-core", vec![rec]);
        let json = serde_json::to_string(&report).unwrap();
        assert!(json.contains("\"oracle\":\"none\""));
        let mut csv = Vec::new();
        write_csv_to(&mut csv, &report).unwrap();
        let s = String::from_utf8(csv).unwrap();
        assert!(s.contains("calc-core"));
        assert!(s.contains("SUM"));
    }
}
