//! Structured pass/fail report.

use crate::compare::Diff;
use serde::{Deserialize, Serialize};
use xlsx_types::{ExcelType, ExcelValue};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Pass,
    Fail,
    Error,
    Skip,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Timing {
    pub candidate_us: u128,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub oracle_us: Option<u128>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct CaseVerdict {
    pub id: String,
    pub status: Status,
    pub description: String,
    pub tags: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expected: Option<ExcelValue>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expected_type: Option<ExcelType>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub actual: Option<ExcelValue>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub actual_type: Option<ExcelType>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub diffs: Vec<Diff>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timing: Option<Timing>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Summary {
    pub total: usize,
    pub passed: usize,
    pub failed: usize,
    pub errored: usize,
    pub skipped: usize,
}

impl Summary {
    pub fn from_cases(cases: &[CaseVerdict]) -> Self {
        let mut s = Self {
            total: cases.len(),
            passed: 0,
            failed: 0,
            errored: 0,
            skipped: 0,
        };
        for c in cases {
            match c.status {
                Status::Pass => s.passed += 1,
                Status::Fail => s.failed += 1,
                Status::Error => s.errored += 1,
                Status::Skip => s.skipped += 1,
            }
        }
        s
    }

    pub fn ok(&self) -> bool {
        self.failed == 0 && self.errored == 0
    }
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Report {
    pub candidate: String,
    pub oracle: String,
    pub corpus: String,
    pub summary: Summary,
    pub cases: Vec<CaseVerdict>,
}

impl Report {
    pub fn new(
        candidate: impl Into<String>,
        oracle: impl Into<String>,
        corpus: impl Into<String>,
        cases: Vec<CaseVerdict>,
    ) -> Self {
        Self {
            candidate: candidate.into(),
            oracle: oracle.into(),
            corpus: corpus.into(),
            summary: Summary::from_cases(&cases),
            cases,
        }
    }

    pub fn ok(&self) -> bool {
        self.summary.ok()
    }

    pub fn to_json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string_pretty(self)
    }

    pub fn to_text(&self) -> String {
        let mut out = String::new();
        out.push_str(&format!(
            "xlsx-verify\ncandidate: {}\noracle:    {}\ncorpus:    {}\n\n",
            self.candidate, self.oracle, self.corpus
        ));
        for c in &self.cases {
            let mark = match c.status {
                Status::Pass => "PASS",
                Status::Fail => "FAIL",
                Status::Error => "ERROR",
                Status::Skip => "SKIP",
            };
            let exp = c
                .expected
                .as_ref()
                .map(|v| v.display_compact())
                .unwrap_or_else(|| "-".into());
            let act = c
                .actual
                .as_ref()
                .map(|v| v.display_compact())
                .unwrap_or_else(|| "-".into());
            out.push_str(&format!("  {mark:<5} {:<36}  {exp}  vs  {act}\n", c.id));
            if let Some(msg) = &c.message {
                out.push_str(&format!("        {msg}\n"));
            }
            for d in &c.diffs {
                out.push_str(&format!(
                    "        [{}] {}: {}\n",
                    d.kind_label(),
                    d.path,
                    d.message
                ));
            }
        }
        out.push('\n');
        out.push_str(&format!(
            "{} cases  {} passed  {} failed  {} errored  {} skipped\n",
            self.summary.total,
            self.summary.passed,
            self.summary.failed,
            self.summary.errored,
            self.summary.skipped
        ));
        if self.ok() {
            out.push_str("OK\n");
        } else {
            out.push_str("FAILED\n");
        }
        out
    }
}

impl crate::compare::Diff {
    fn kind_label(&self) -> &'static str {
        match self.kind {
            crate::compare::DiffKind::Type => "type",
            crate::compare::DiffKind::Value => "value",
            crate::compare::DiffKind::ErrorCode => "error",
            crate::compare::DiffKind::ArrayShape => "shape",
        }
    }
}
