//! Run a candidate against a corpus using an oracle.

use crate::compare::{compare, CompareOptions};
use crate::corpus::Corpus;
use crate::fixture::Fixture;
use crate::report::{CaseVerdict, Report, Status, Timing};
use std::time::Instant;
use xlsx_oracle::{Oracle, OracleRequest};
use xlsx_types::Candidate;

#[derive(Default)]
pub struct Verifier {
    pub compare: CompareOptions,
}

impl Verifier {
    pub fn run<C, O>(&self, candidate: &C, oracle: &O, corpus: &Corpus) -> Report
    where
        C: Candidate + ?Sized,
        O: Oracle + ?Sized,
    {
        self.run_cases(
            candidate,
            oracle,
            &corpus.cases,
            &corpus.root.to_string_lossy(),
        )
    }

    pub fn run_cases<C, O>(
        &self,
        candidate: &C,
        oracle: &O,
        cases: &[Fixture],
        corpus_label: &str,
    ) -> Report
    where
        C: Candidate + ?Sized,
        O: Oracle + ?Sized,
    {
        let verdicts = cases
            .iter()
            .map(|f| self.run_case(candidate, oracle, f))
            .collect();
        Report::new(candidate.id(), oracle.id(), corpus_label, verdicts)
    }

    pub fn run_case<C, O>(&self, candidate: &C, oracle: &O, fixture: &Fixture) -> CaseVerdict
    where
        C: Candidate + ?Sized,
        O: Oracle + ?Sized,
    {
        if let Some(reason) = &fixture.ignore {
            return CaseVerdict {
                id: fixture.id.clone(),
                status: Status::Skip,
                description: fixture.description.clone(),
                tags: fixture.tags.clone(),
                expected: fixture.expected.clone(),
                expected_type: fixture.expected.as_ref().map(|v| v.excel_type()),
                actual: None,
                actual_type: None,
                diffs: Vec::new(),
                message: Some(reason.clone()),
                timing: None,
            };
        }

        let t_oracle = Instant::now();
        let oracle_res = oracle.expect(&OracleRequest {
            spec: &fixture.spec,
            recorded: fixture.expected.as_ref(),
        });
        let oracle_us = t_oracle.elapsed().as_micros();

        let expected = match oracle_res {
            Ok(ans) => ans.value,
            Err(e) => {
                return CaseVerdict {
                    id: fixture.id.clone(),
                    status: Status::Error,
                    description: fixture.description.clone(),
                    tags: fixture.tags.clone(),
                    expected: fixture.expected.clone(),
                    expected_type: fixture.expected.as_ref().map(|v| v.excel_type()),
                    actual: None,
                    actual_type: None,
                    diffs: Vec::new(),
                    message: Some(format!("oracle: {e}")),
                    timing: Some(Timing {
                        candidate_us: 0,
                        oracle_us: Some(oracle_us),
                    }),
                };
            }
        };

        let t_cand = Instant::now();
        let cand_res = candidate.evaluate(&fixture.spec);
        let candidate_us = t_cand.elapsed().as_micros();
        let timing = Some(Timing {
            candidate_us,
            oracle_us: Some(oracle_us),
        });

        match cand_res {
            Ok(actual) => {
                let cmp = compare(&expected, &actual, &self.compare);
                if cmp.equal {
                    CaseVerdict {
                        id: fixture.id.clone(),
                        status: Status::Pass,
                        description: fixture.description.clone(),
                        tags: fixture.tags.clone(),
                        expected: Some(expected.clone()),
                        expected_type: Some(expected.excel_type()),
                        actual: Some(actual.clone()),
                        actual_type: Some(actual.excel_type()),
                        diffs: Vec::new(),
                        message: None,
                        timing,
                    }
                } else {
                    CaseVerdict {
                        id: fixture.id.clone(),
                        status: Status::Fail,
                        description: fixture.description.clone(),
                        tags: fixture.tags.clone(),
                        expected: Some(expected.clone()),
                        expected_type: Some(expected.excel_type()),
                        actual: Some(actual.clone()),
                        actual_type: Some(actual.excel_type()),
                        diffs: cmp.diffs,
                        message: Some(format!(
                            "expected {} ({}) got {} ({})",
                            expected.display_compact(),
                            expected.excel_type(),
                            actual.display_compact(),
                            actual.excel_type()
                        )),
                        timing,
                    }
                }
            }
            Err(e) => CaseVerdict {
                id: fixture.id.clone(),
                status: Status::Error,
                description: fixture.description.clone(),
                tags: fixture.tags.clone(),
                expected: Some(expected.clone()),
                expected_type: Some(expected.excel_type()),
                actual: None,
                actual_type: None,
                diffs: Vec::new(),
                message: Some(format!("candidate: {e}")),
                timing,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixture::load_str;
    use std::path::Path;
    use xlsx_oracle::{FixtureOracle, MockOracle};
    use xlsx_types::{EvalError, EvalSpec, ExcelError, ExcelValue};

    struct ConstCandidate {
        id: &'static str,
        value: ExcelValue,
    }

    impl Candidate for ConstCandidate {
        fn id(&self) -> &str {
            self.id
        }
        fn evaluate(&self, _spec: &EvalSpec) -> Result<ExcelValue, EvalError> {
            Ok(self.value.clone())
        }
    }

    fn fixture() -> Fixture {
        let src = r#"{
            "id": "t.add",
            "formula": "=1+2",
            "expected": { "number": 3 }
        }"#;
        load_str(Path::new("t.json"), src).unwrap().remove(0)
    }

    #[test]
    fn pass_and_fail_and_error() {
        let v = Verifier::default();
        let f = fixture();
        let oracle = FixtureOracle;

        let pass = v.run_case(
            &ConstCandidate {
                id: "ok",
                value: ExcelValue::Number(3.0),
            },
            &oracle,
            &f,
        );
        assert_eq!(pass.status, Status::Pass);

        let fail = v.run_case(
            &ConstCandidate {
                id: "bad",
                value: ExcelValue::Number(4.0),
            },
            &oracle,
            &f,
        );
        assert_eq!(fail.status, Status::Fail);
        assert!(!fail.diffs.is_empty());

        let typo = v.run_case(
            &ConstCandidate {
                id: "err",
                value: ExcelValue::Error(ExcelError::Div0),
            },
            &oracle,
            &f,
        );
        assert_eq!(typo.status, Status::Fail);
        assert!(typo.diffs.iter().any(|d| d.message.contains("type")));

        let mock = MockOracle::new(ExcelValue::Number(99.0));
        let vs_mock = v.run_case(
            &ConstCandidate {
                id: "ok",
                value: ExcelValue::Number(3.0),
            },
            &mock,
            &f,
        );
        assert_eq!(vs_mock.status, Status::Fail);
    }
}
