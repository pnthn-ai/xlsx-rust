//! CLI gate: run the corpus against a candidate and exit non-zero on failure.

use clap::{Parser, ValueEnum};
use std::path::PathBuf;
use std::process::ExitCode;
use xlsx_oracle::FixtureOracle;
use xlsx_verify::{
    builtin, default_candidate_id, known_ids, resolve_corpus_root, Corpus, Verifier,
};

#[derive(Clone, Copy, ValueEnum)]
enum Format {
    Text,
    Json,
}

#[derive(Parser)]
#[command(
    name = "xlsx-verify",
    about = "Run the Excel-compatibility corpus against a calculation candidate",
    version
)]
struct Args {
    /// Candidate id (`seed-compliant`, `naive`, or a registered custom id).
    #[arg(short, long)]
    candidate: Option<String>,

    /// Fixture directory or file (default: workspace `fixtures/`).
    #[arg(long)]
    corpus: Option<PathBuf>,

    /// Report format.
    #[arg(short, long, value_enum, default_value_t = Format::Text)]
    format: Format,

    /// Optional JSON/text output path (stdout is always used when omitted).
    #[arg(short, long)]
    output: Option<PathBuf>,

    /// Only cases whose tags include this value.
    #[arg(long)]
    tag: Option<String>,

    /// Only cases whose id starts with this prefix.
    #[arg(long)]
    id: Option<String>,

    /// List registered candidates and exit.
    #[arg(long)]
    list_candidates: bool,

    /// List fixture ids and exit.
    #[arg(long)]
    list_fixtures: bool,
}

fn main() -> ExitCode {
    match run() {
        Ok(code) => code,
        Err(e) => {
            eprintln!("xlsx-verify: {e}");
            ExitCode::from(2)
        }
    }
}

fn run() -> Result<ExitCode, String> {
    let args = Args::parse();

    if args.list_candidates {
        for id in known_ids() {
            println!("{id}");
        }
        return Ok(ExitCode::SUCCESS);
    }

    let corpus_root = resolve_corpus_root(args.corpus.as_deref())
        .ok_or_else(|| "could not find fixtures/; pass --corpus".to_string())?;
    let corpus = Corpus::load(&corpus_root).map_err(|e| e.to_string())?;

    if args.list_fixtures {
        for c in &corpus.cases {
            println!("{}\t{}", c.id, c.description);
        }
        return Ok(ExitCode::SUCCESS);
    }

    let selected: Vec<_> = corpus
        .filter(args.tag.as_deref(), args.id.as_deref())
        .into_iter()
        .cloned()
        .collect();
    if selected.is_empty() {
        return Err("no fixtures matched the given filters".into());
    }

    let candidate_id = args
        .candidate
        .unwrap_or_else(|| default_candidate_id().to_string());
    let candidate = builtin(&candidate_id)?;
    let oracle = FixtureOracle;
    let report = Verifier::default().run_cases(
        candidate.as_ref(),
        &oracle,
        &selected,
        &corpus.root.display().to_string(),
    );

    let rendered = match args.format {
        Format::Text => report.to_text(),
        Format::Json => report.to_json().map_err(|e| e.to_string())?,
    };
    print!("{rendered}");
    if let Some(path) = args.output {
        std::fs::write(&path, &rendered).map_err(|e| format!("{}: {e}", path.display()))?;
    }

    if report.ok() {
        Ok(ExitCode::SUCCESS)
    } else {
        Ok(ExitCode::from(1))
    }
}
