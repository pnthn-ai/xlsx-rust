//! Stub calculation candidates for the verification gate.
//!
//! This is **not** a full formula engine. It exists so the verification layer
//! has something to load, and so a subagent can see a pass path
//! (`seed-compliant`) and a fail path (`naive`) before writing a real engine.

mod eval;
mod parse;

pub use eval::{eval_formula_in, Interpreter, Semantics};
pub use parse::{parse, Expr};

use xlsx_types::{Candidate, EvalError, EvalSpec, ExcelValue};

/// Excel-compatible (seed corpus only) candidate. Default CLI target.
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
