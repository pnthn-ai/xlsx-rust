//! Built-in candidate inventory used by the CLI.
//!
//! Subagents: add your crate to the workspace, implement
//! [`xlsx_types::Candidate`], then register it here (and in
//! [`all_candidates`]) so `xlsx-verify --candidate <id>` can load it.

use xlsx_engine::{NaiveEngine, SeedCompliantEngine};
use xlsx_engine_core::CalcCoreEngine;
use xlsx_types::Candidate;

pub fn builtin(id: &str) -> Result<Box<dyn Candidate>, String> {
    match id {
        "calc-core" | "engine" | "core" => Ok(Box::new(CalcCoreEngine::new())),
        "seed-compliant" | "compliant" | "reference" => Ok(Box::new(SeedCompliantEngine::new())),
        "naive" | "stub" => Ok(Box::new(NaiveEngine::new())),
        other => Err(format!(
            "unknown candidate '{other}'. known: {}",
            known_ids().join(", ")
        )),
    }
}

pub fn known_ids() -> Vec<&'static str> {
    vec!["calc-core", "seed-compliant", "naive"]
}

pub fn all_candidates() -> Vec<Box<dyn Candidate>> {
    vec![
        Box::new(CalcCoreEngine::new()),
        Box::new(SeedCompliantEngine::new()),
        Box::new(NaiveEngine::new()),
    ]
}

pub fn default_candidate_id() -> &'static str {
    "calc-core"
}
