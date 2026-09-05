//! Correctness verification layer for xlsx-rust engine candidates.
//!
//! A candidate implements [`xlsx_types::Candidate`]. The verifier loads a
//! fixture corpus, asks an [`xlsx_oracle::Oracle`] for the Excel-compatible
//! expected result, and emits a structured [`report::Report`].

pub mod compare;
pub mod corpus;
pub mod fixture;
pub mod registry;
pub mod report;
pub mod verifier;

pub use compare::{compare, CompareOptions, Comparison, Diff, DiffKind, NumberMode};
pub use corpus::{discover_fixtures, resolve_corpus_root, Corpus, CorpusError};
pub use fixture::{load_path, load_str, Fixture, FixtureError};
pub use registry::{all_candidates, builtin, default_candidate_id, known_ids};
pub use report::{CaseVerdict, Report, Status, Summary, Timing};
pub use verifier::Verifier;
