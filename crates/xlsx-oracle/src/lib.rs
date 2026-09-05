//! Trusted Excel-compatible oracle.
//!
//! The default CI path is [`FixtureOracle`]: it returns the recorded expected
//! value from a fixture. That does **not** require Microsoft Excel.
//!
//! A live backend (Excel COM, LibreOffice, a hosted evaluator) implements
//! [`OracleBackend`] and can be wrapped with [`RecordingOracle`] to mint
//! golden files for later fixture-only runs.

use thiserror::Error;
use xlsx_types::{EvalError, EvalSpec, ExcelValue};

/// How an expected value was produced.
#[derive(Clone, Debug, PartialEq)]
pub struct OracleAnswer {
    pub value: ExcelValue,
    /// `fixture`, `mock`, `excel`, `libreoffice`, …
    pub source: String,
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum OracleError {
    #[error("no recorded expectation for case {0}")]
    MissingRecorded(String),
    #[error("oracle backend failed: {0}")]
    Backend(String),
    #[error(transparent)]
    Eval(#[from] EvalError),
}

/// Request sent to an oracle. `recorded` is the fixture's `expected` field
/// when present; live oracles may ignore it.
#[derive(Clone, Debug)]
pub struct OracleRequest<'a> {
    pub spec: &'a EvalSpec,
    pub recorded: Option<&'a ExcelValue>,
}

/// Source of the Excel-compatible expected result.
pub trait Oracle: Send + Sync {
    fn id(&self) -> &str;
    fn expect(&self, request: &OracleRequest<'_>) -> Result<OracleAnswer, OracleError>;
}

/// A live evaluator that can stand in for Excel later (COM, LibreOffice, …).
pub trait OracleBackend: Send + Sync {
    fn id(&self) -> &str;
    fn evaluate(&self, spec: &EvalSpec) -> Result<ExcelValue, OracleError>;
}

/// Default CI oracle: use the fixture's recorded `expected` value.
#[derive(Clone, Debug, Default)]
pub struct FixtureOracle;

impl Oracle for FixtureOracle {
    fn id(&self) -> &str {
        "fixture"
    }

    fn expect(&self, request: &OracleRequest<'_>) -> Result<OracleAnswer, OracleError> {
        let value = request
            .recorded
            .cloned()
            .ok_or_else(|| OracleError::MissingRecorded(request.spec.case_id.clone()))?;
        Ok(OracleAnswer {
            value,
            source: "fixture".to_string(),
        })
    }
}

/// Programmable oracle for verifier unit tests.
#[derive(Clone, Debug)]
pub struct MockOracle {
    pub id: String,
    pub value: ExcelValue,
}

impl MockOracle {
    pub fn new(value: ExcelValue) -> Self {
        Self {
            id: "mock".to_string(),
            value,
        }
    }
}

impl Oracle for MockOracle {
    fn id(&self) -> &str {
        &self.id
    }

    fn expect(&self, _request: &OracleRequest<'_>) -> Result<OracleAnswer, OracleError> {
        Ok(OracleAnswer {
            value: self.value.clone(),
            source: self.id.clone(),
        })
    }
}

/// Live-backend oracle with an optional golden cache.
///
/// When `prefer_recorded` is true (CI default if goldens exist), the recorded
/// fixture value is returned and the backend is not contacted. When false,
/// the backend is queried; callers can persist the answer as a new fixture.
pub struct RecordingOracle<B> {
    backend: B,
    prefer_recorded: bool,
}

impl<B: OracleBackend> RecordingOracle<B> {
    pub fn live(backend: B) -> Self {
        Self {
            backend,
            prefer_recorded: false,
        }
    }

    pub fn prefer_recorded(backend: B) -> Self {
        Self {
            backend,
            prefer_recorded: true,
        }
    }
}

impl<B: OracleBackend> Oracle for RecordingOracle<B> {
    fn id(&self) -> &str {
        self.backend.id()
    }

    fn expect(&self, request: &OracleRequest<'_>) -> Result<OracleAnswer, OracleError> {
        if self.prefer_recorded {
            if let Some(v) = request.recorded {
                return Ok(OracleAnswer {
                    value: v.clone(),
                    source: "fixture".to_string(),
                });
            }
        }
        let value = self.backend.evaluate(request.spec)?;
        Ok(OracleAnswer {
            value,
            source: self.backend.id().to_string(),
        })
    }
}

impl<T: Oracle + ?Sized> Oracle for &T {
    fn id(&self) -> &str {
        (**self).id()
    }
    fn expect(&self, request: &OracleRequest<'_>) -> Result<OracleAnswer, OracleError> {
        (**self).expect(request)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use xlsx_types::EvalSpec;

    #[test]
    fn fixture_oracle_uses_recorded() {
        let spec = EvalSpec::formula("c", "=1+1");
        let recorded = ExcelValue::Number(2.0);
        let ans = FixtureOracle
            .expect(&OracleRequest {
                spec: &spec,
                recorded: Some(&recorded),
            })
            .unwrap();
        assert_eq!(ans.value, recorded);
        assert_eq!(ans.source, "fixture");
    }

    #[test]
    fn fixture_oracle_requires_recorded() {
        let spec = EvalSpec::formula("c", "=1+1");
        let err = FixtureOracle
            .expect(&OracleRequest {
                spec: &spec,
                recorded: None,
            })
            .unwrap_err();
        assert!(matches!(err, OracleError::MissingRecorded(_)));
    }
}
