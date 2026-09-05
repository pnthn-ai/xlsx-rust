//! Stub calculation candidates for the verification gate.
//!
//! The real engine lives in `xlsx-engine-core` (`calc-core`). This crate keeps
//! a seed-scoped pass path (`seed-compliant`) and an intentional fail path
//! (`naive`) so the gate remains demonstrable.

mod dates;
mod eval;
mod parse;

pub use eval::{eval_formula_in, Interpreter, Semantics};
pub use parse::{parse, Expr};

use xlsx_types::{Candidate, EvalError, EvalSpec, ExcelValue};

/// Excel-compatible stub that passes the expanded fixture corpus.
#[derive(Clone, Debug, Default)]
pub struct SeedCompliantEngine;

impl SeedCompliantEngine {
    pub fn new() -> Self {
        Self
    }
}

impl Candidate for SeedCompliantEngine {
    fn id(&self) -> &str {
        "seed-compliant"
    }

    fn evaluate(&self, spec: &EvalSpec) -> Result<ExcelValue, EvalError> {
        Interpreter::new(Semantics::ExcelSeed).eval_spec(spec)
    }
}

/// Intentionally naive candidate: IEEE arithmetic, weak coercion.
///
/// Used to prove the gate reports actionable failures. A real engine PR
/// should **not** use this as the thing under test.
#[derive(Clone, Debug, Default)]
pub struct NaiveEngine;

impl NaiveEngine {
    pub fn new() -> Self {
        Self
    }
}

impl Candidate for NaiveEngine {
    fn id(&self) -> &str {
        "naive"
    }

    fn evaluate(&self, spec: &EvalSpec) -> Result<ExcelValue, EvalError> {
        Interpreter::new(Semantics::Naive).eval_spec(spec)
    }
}
