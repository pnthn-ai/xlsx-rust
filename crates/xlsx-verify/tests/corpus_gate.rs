//! End-to-end gate: calc-core / seed-compliant must pass; naive must fail quirks.

use std::path::PathBuf;
use xlsx_engine::{NaiveEngine, SeedCompliantEngine};
use xlsx_engine_core::CalcCoreEngine;
use xlsx_oracle::FixtureOracle;
use xlsx_verify::{Corpus, Status, Verifier};

fn fixtures_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures")
}

fn load_corpus() -> Corpus {
    Corpus::load(fixtures_root()).expect("fixtures/ must load")
}

#[test]
fn calc_core_passes_entire_corpus() {
    let corpus = load_corpus();
    assert!(
        !corpus.cases.is_empty(),
        "seed corpus should contain fixtures"
    );
    let report = Verifier::default().run(&CalcCoreEngine::new(), &FixtureOracle, &corpus);
    assert!(
        report.ok(),
        "calc-core must pass the corpus:\n{}",
        report.to_text()
    );
}

#[test]
fn seed_compliant_passes_entire_corpus() {
    let corpus = load_corpus();
    assert!(
        !corpus.cases.is_empty(),
        "seed corpus should contain fixtures"
    );
    let report = Verifier::default().run(&SeedCompliantEngine::new(), &FixtureOracle, &corpus);
    assert!(
        report.ok(),
        "seed-compliant must pass the corpus:\n{}",
        report.to_text()
    );
}

#[test]
fn naive_fails_excel_quirks() {
    let corpus = load_corpus();
    let report = Verifier::default().run(&NaiveEngine::new(), &FixtureOracle, &corpus);
    assert!(
        !report.ok(),
        "naive must fail at least one quirk case so the gate is observable"
    );

    let must_fail = [
        "arith.div0",
        "arith.text-plus-one",
        "cmp.text-eq-casefold",
        "cmp.num-eq-text",
        "cmp.text-gt-number",
        "cmp.false-gt-number",
        "cmp.true-eq-one",
        "cmp.ieee-fuzzy",
        "empty.eq-zero",
        "fn.sum-bool-cell",
    ];
    for id in must_fail {
        let v = report
            .cases
            .iter()
            .find(|c| c.id == id)
            .unwrap_or_else(|| panic!("missing case {id}"));
        assert_eq!(
            v.status,
            Status::Fail,
            "{id} should FAIL on naive (got {:?}: {:?})",
            v.status,
            v.message
        );
        assert!(
            !v.diffs.is_empty() || v.message.is_some(),
            "{id} should include an actionable diff"
        );
    }
}

#[test]
fn report_json_is_machine_readable() {
    let corpus = load_corpus();
    let report = Verifier::default().run(&SeedCompliantEngine::new(), &FixtureOracle, &corpus);
    let json = report.to_json().unwrap();
    let v: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert_eq!(v["candidate"], "seed-compliant");
    assert_eq!(v["oracle"], "fixture");
    assert!(v["summary"]["passed"].as_u64().unwrap() > 0);
    assert!(v["cases"].as_array().unwrap().len() == report.cases.len());
}
