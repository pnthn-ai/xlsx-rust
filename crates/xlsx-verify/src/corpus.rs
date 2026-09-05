//! Discover and load a fixture corpus from disk.

use crate::fixture::{load_path, Fixture, FixtureError};
use std::path::{Path, PathBuf};
use thiserror::Error;

#[derive(Clone, Debug)]
pub struct Corpus {
    pub root: PathBuf,
    pub cases: Vec<Fixture>,
}

#[derive(Debug, Error)]
pub enum CorpusError {
    #[error(transparent)]
    Fixture(#[from] FixtureError),
    #[error("corpus path does not exist: {0}")]
    Missing(PathBuf),
    #[error("duplicate fixture id '{id}' in {} and {}", a.display(), b.display())]
    Duplicate { id: String, a: PathBuf, b: PathBuf },
}

impl Corpus {
    pub fn load(root: impl AsRef<Path>) -> Result<Self, CorpusError> {
        let root = root.as_ref().to_path_buf();
        if !root.exists() {
            return Err(CorpusError::Missing(root));
        }
        let mut files = Vec::new();
        collect_files(&root, &mut files);
        files.sort();
        let mut cases = Vec::new();
        for f in files {
            cases.extend(load_path(&f)?);
        }
        let mut seen = std::collections::BTreeMap::<String, PathBuf>::new();
        for c in &cases {
            if let Some(prev) = seen.insert(c.id.clone(), c.source.clone()) {
                return Err(CorpusError::Duplicate {
                    id: c.id.clone(),
                    a: prev,
                    b: c.source.clone(),
                });
            }
        }
        Ok(Self { root, cases })
    }

    pub fn filter(&self, tag: Option<&str>, id_prefix: Option<&str>) -> Vec<&Fixture> {
        self.cases
            .iter()
            .filter(|c| {
                if let Some(tag) = tag {
                    if !c.tags.iter().any(|t| t.eq_ignore_ascii_case(tag)) {
                        return false;
                    }
                }
                if let Some(prefix) = id_prefix {
                    if !c.id.starts_with(prefix) {
                        return false;
                    }
                }
                true
            })
            .collect()
    }
}

fn collect_files(path: &Path, out: &mut Vec<PathBuf>) {
    if path.is_file() {
        if is_fixture(path) {
            out.push(path.to_path_buf());
        }
        return;
    }
    let Ok(rd) = std::fs::read_dir(path) else {
        return;
    };
    for ent in rd.flatten() {
        let p = ent.path();
        if p.is_dir() {
            collect_files(&p, out);
        } else if is_fixture(&p) {
            out.push(p);
        }
    }
}

fn is_fixture(path: &Path) -> bool {
    matches!(path.extension().and_then(|e| e.to_str()), Some("json"))
        && path.file_name().and_then(|n| n.to_str()) != Some("schema.json")
}

/// Walk up from `start` looking for a workspace `fixtures/` directory.
pub fn discover_fixtures(start: &Path) -> Option<PathBuf> {
    let mut cur = start.to_path_buf();
    loop {
        let candidate = cur.join("fixtures");
        if candidate.is_dir() {
            return Some(candidate);
        }
        if cur.join("Cargo.toml").is_file() && candidate.is_dir() {
            return Some(candidate);
        }
        if !cur.pop() {
            return None;
        }
    }
}

/// Resolve the corpus path used by the CLI.
///
/// Order: explicit `--corpus`, `XLSX_FIXTURES`, `fixtures/` upward from cwd
/// and from `CARGO_MANIFEST_DIR`.
pub fn resolve_corpus_root(explicit: Option<&Path>) -> Option<PathBuf> {
    if let Some(p) = explicit {
        return Some(p.to_path_buf());
    }
    if let Ok(env) = std::env::var("XLSX_FIXTURES") {
        return Some(PathBuf::from(env));
    }
    let cwd = std::env::current_dir().ok()?;
    if let Some(found) = discover_fixtures(&cwd) {
        return Some(found);
    }
    if let Ok(manifest) = std::env::var("CARGO_MANIFEST_DIR") {
        let p = PathBuf::from(manifest);
        if let Some(found) = discover_fixtures(&p) {
            return Some(found);
        }
        if let Some(found) = p.parent().and_then(discover_fixtures) {
            return Some(found);
        }
    }
    None
}
