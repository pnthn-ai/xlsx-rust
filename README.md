# xlsx-rust

Headless XLSX calculation engine in Rust. The long-term goal is Excel
compatibility (including the quirks) with better time-to-compute than Microsoft
Excel for every function.

The first real calculation candidate is **`calc-core`** (`crates/xlsx-engine-core`):
an AST parser + workbook-backed evaluator with Excel quirk modules. The
verification layer is the gate every candidate — including this one — must pass.

```
                    ┌─────────────┐
   fixtures/*.json  │  Fixture    │  recorded expected
                    │  corpus     │  (value + type + error)
                    └──────┬──────┘
                           │
           ┌───────────────▼────────────────┐
           │          xlsx-verify           │
           │  load → evaluate → compare →   │
           │  structured Report (text/json) │
           └───────┬───────────────┬────────┘
                   │               │
           ┌───────▼──────┐ ┌──────▼───────┐
           │  Candidate   │ │    Oracle    │
           │  (engine)    │ │ fixture/mock │
           │              │ │ (Excel later)│
           └──────────────┘ └──────────────┘
```

CI never requires a live Microsoft Excel install. The default oracle is the
**recorded fixture** (`expected` on each case). A live Excel / LibreOffice
backend can be wired later through `xlsx-oracle` without changing candidates.

## Workspace

| Crate | Role |
|---|---|
| [`crates/xlsx-types`](crates/xlsx-types) | Excel values, error codes, workbook snippets, [`Candidate`](crates/xlsx-types/src/eval.rs) trait |
| [`crates/xlsx-oracle`](crates/xlsx-oracle) | Trusted expected-result source: fixture, mock, recording wrapper for a future live backend |
| [`crates/xlsx-verify`](crates/xlsx-verify) | Corpus loader, comparison, verdict report, `xlsx-verify` CLI |
| [`crates/xlsx-engine-core`](crates/xlsx-engine-core) | Real formula engine (`calc-core`): parser, evaluator, quirk modules |
| [`crates/xlsx-engine`](crates/xlsx-engine) | Stub candidates: `seed-compliant` (seed-scoped pass path) and `naive` (intentional fail path) |

`xlsx-engine` stubs remain so the gate still has an explicit pass/fail demo.
`calc-core` is the serious default; it does **not** read fixture expected
outputs.

## Verification gate (for subagent PRs)

A candidate PR is correct only if:

1. `cargo test --workspace` is green.
2. `xlsx-verify` against the **full** corpus (plus any new fixtures you add)
   exits **0**.
3. The machine-readable report (`--format json`) has `summary.failed == 0`
   and `summary.errored == 0`.

Skipped cases (`ignore` on a fixture) do not fail the gate. Do not use `ignore`
to hide a broken new function — add the case, make the candidate pass it.

```bash
# default candidate is calc-core; fixture oracle; no Excel required
cargo test --workspace
cargo run -p xlsx-verify -- --candidate calc-core --format json
cargo run -p xlsx-verify -- --format text
cargo run -p xlsx-verify -- --format json --output report.json

# prove the gate can fail (intentional)
cargo run -p xlsx-verify -- --candidate naive --format text
# exit code 1, with type/value/error diffs
```

Exit codes: `0` all passed, `1` any fail/error, `2` usage / missing corpus.

## How to add a candidate

1. Create `crates/xlsx-engine-<name>/` (or extend `xlsx-engine`) and add it to
   the workspace in the root `Cargo.toml`.
2. Depend on `xlsx-types` only (keep the DAG one-way: engine → types, never
   engine → verify).
3. Implement [`xlsx_types::Candidate`](crates/xlsx-types/src/eval.rs):

```rust
use xlsx_types::{Candidate, EvalError, EvalSpec, ExcelValue};

pub struct MyEngine;

impl Candidate for MyEngine {
    fn id(&self) -> &str { "my-engine" }

    fn evaluate(&self, spec: &EvalSpec) -> Result<ExcelValue, EvalError> {
        // spec.target: Formula { formula, at } | Cell { cell } | Named { name }
        // spec.workbook: snippet sheets + defined names
        // Return ExcelValue (including ExcelError::* for #DIV/0! etc.)
        // Use EvalError only for infrastructure failures (parse crash, I/O).
        todo!()
    }
}
```

4. Register the id in [`crates/xlsx-verify/src/registry.rs`](crates/xlsx-verify/src/registry.rs)
   so the CLI can load it (`--candidate my-engine`).
5. Add fixtures under `fixtures/` (see below). Keep them next to the behavior
   you are claiming: `fixtures/functions/sum.json`, etc.
6. Run the gate and paste / attach the JSON report:

```bash
cargo run -p xlsx-verify -- --candidate my-engine --format json
```

A candidate that cannot evaluate a case should return `Err(EvalError::…)`
(verdict `error`) or an Excel error value such as `#NAME?` (verdict `fail` if
the oracle expected a number). Do not panic.

Optional hooks on the trait: `load_workbook`, `compute_cell`, `compute_named`.
`evaluate` is the one the verifier calls.

## How to add fixtures

Put JSON under [`fixtures/`](fixtures). Files named `schema.json` are ignored.
A file may be one case, an array of cases, or `{ "cases": [...] }`. (The loader
is a thin serde mapping — a TOML adapter can be added later without changing
the case schema.)

```json
{
  "id": "fn.sum.range-with-blanks",
  "description": "SUM skips blanks",
  "tags": ["sum", "empty"],
  "quirks": ["empty-cell-duality"],
  "formula": "=SUM(A1:A3)",
  "expected": { "number": 3 },
  "workbook": {
    "sheets": [
      {
        "name": "Sheet1",
        "cells": {
          "A1": { "number": 1 },
          "A3": { "number": 2 }
        }
      }
    ]
  }
}
```

**Exactly one** of `formula`, `cell`, or `named` is required.

| Field | Meaning |
|---|---|
| `id` | Stable case id (`area.topic`). Must be unique across the corpus. |
| `expected` | Oracle value for the fixture oracle. Tagged object, see below. |
| `workbook` | Optional snippet. Missing cells are **empty**, not zero. |
| `at` | A1 host cell for `formula` (default `Sheet1!A1`). |
| `cell` / `named` | Compute an existing cell or a defined name instead of a free formula. |
| `tags` | Free-form; CLI `--tag sum` filters on these. |
| `quirks` | From the catalog in [`crates/xlsx-types/src/quirk.rs`](crates/xlsx-types/src/quirk.rs). |
| `ignore` | If set, the case is skipped (reason is the string). |
| `options` | `locale`, `date_system`, `array_mode` (modeled; seed uses defaults). |

**`expected` / cell values:**

```json
{ "number": 3 }
{ "text": "b" }
{ "bool": true }
{ "error": "#DIV/0!" }
{ "empty": null }
{ "array": [[{ "number": 1 }, { "number": 2 }]] }
```

Error aliases: `#DIV/0!`, `DIV0`, `#N/A`, `NA`, `#VALUE!`, `#REF!`, `#NAME?`,
`#NUM!`, `#NULL!`, plus newer codes (`#SPILL!`, `#CALC!`, …).

JSON literals (`3`, `true`, `null`, `"#DIV/0!"`) are also accepted.

Filter while iterating:

```bash
cargo run -p xlsx-verify -- --tag arithmetic --id arith.div
cargo run -p xlsx-verify -- --list-fixtures
cargo run -p xlsx-verify -- --list-candidates
```

JSON Schema: [`fixtures/schema.json`](fixtures/schema.json).

## Verdict report

Text (human) and JSON (subagents / CI) share the same structure:

```json
{
  "candidate": "calc-core",
  "oracle": "fixture",
  "corpus": "/path/to/fixtures",
  "summary": { "total": 42, "passed": 42, "failed": 0, "errored": 0, "skipped": 0 },
  "cases": [
    {
      "id": "arith.div0",
      "status": "pass",
      "expected": { "error": "#DIV/0!" },
      "expected_type": "error",
      "actual": { "error": "#DIV/0!" },
      "actual_type": "error",
      "diffs": [],
      "timing": { "candidate_us": 12, "oracle_us": 1 }
    }
  ]
}
```

On failure, `diffs[]` names what broke:

| `kind` | Meaning |
|---|---|
| `type` | `number` vs `empty` vs `error` vs … |
| `value` | Same type, wrong payload |
| `error` | Wrong Excel error code (`#DIV/0!` vs `#VALUE!`) |
| `shape` | Array dimensions differ |

Numbers compare with Excel’s 15-significant-digit crossover by default (so
`0.1+0.2` matches `0.3`). Timing fields are measured but are **not** a
performance gate.

## Oracle interface (Excel later)

`xlsx-oracle` is deliberately backend-shaped:

- **`FixtureOracle`** (CI default) — returns `expected` from the fixture.
- **`MockOracle`** — unit tests for the verifier itself.
- **`OracleBackend` + `RecordingOracle`** — wrap a live evaluator (Excel COM,
  LibreOffice, a hosted workbook runner). Record answers once, then commit
  them as fixtures so CI stays Excel-free.

A live backend is **out of scope** for this run. When one exists, point
`RecordingOracle::live(backend)` at it locally, dump values into `fixtures/`,
and keep CI on `FixtureOracle`.

## Seed corpus

Under [`fixtures/seed/`](fixtures/seed): arithmetic, comparisons, type-coercion
quirks, `SUM` / `IF` / `VLOOKUP`, error propagation, empty-vs-zero, array
literals, a defined name, and a stored-formula cell.

`calc-core` implements a real parser/evaluator for this corpus (and is the
CLI default). `seed-compliant` is a leftover seed-scoped stub that still
passes. `naive` uses IEEE / weak coercion so the same corpus produces visible
`FAIL` rows (division by zero → `+inf`, `"2"+1` → `#VALUE!`, `TRUE=1` →
`FALSE`, blank `= 0` → `FALSE`, …).

## Calculation core (`calc-core`)

```
formula text ──parse──▶ AST ──eval──▶ ExcelValue
                              │
                    ┌─────────┼─────────┐
                    │         │         │
                 coerce    compare    empty
                 (arith)    (= < >)   (blank duality)
```

| Module | Role |
|---|---|
| [`parse.rs`](crates/xlsx-engine-core/src/parse.rs) | Tokenizer + recursive-descent AST (ops, parens, refs, ranges, names, literals, arrays, calls) |
| [`eval/mod.rs`](crates/xlsx-engine-core/src/eval/mod.rs) | Workbook-snippet walker; cell / named / formula targets; circular detection |
| [`eval/coerce.rs`](crates/xlsx-engine-core/src/eval/coerce.rs) | Arithmetic / `&` / `IF` coercion (`"2"+1` = 3, TRUE → 1, empty → 0) |
| [`eval/compare.rs`](crates/xlsx-engine-core/src/eval/compare.rs) | 15-digit `=`, case-insensitive text, `TRUE=1`, type ranking (`FALSE>100`) |
| [`eval/empty.rs`](crates/xlsx-engine-core/src/eval/empty.rs) | Blank ≠ 0 ≠ `""`, but `A1=0` and `A1=""` when `A1` is blank |
| [`eval/functions.rs`](crates/xlsx-engine-core/src/eval/functions.rs) | `SUM`, `IF` (short-circuit), `VLOOKUP`, `IFERROR`, `ABS`, `N`, `IS*` |

**Implemented for this PR:** arithmetic and comparison operators (unary `+/-`,
`%`, `^`, `&`), cell refs / ranges / defined names, array literals, error
propagation (`#DIV/0!` vs `EvalError`), and the functions above. Workbook
input is the snippet type in `xlsx-types` (no `.xlsx` IO).

**Deferred:** full function library, locale argument separators, implicit
intersection by host row/column (top-left only today), intersection/union
operators, live Excel oracle, performance bakeoff, and mass corpus growth
(another branch may expand `fixtures/` — this crate consumes whatever is
on `main`).

## Excel compatibility notes

Compatibility beats IEEE purity. Empty is a first-class type: a blank cell is
not the number `0` and not the text `""`, even though many operators treat it
as one or the other. Documented quirk categories (not all implemented):

- 15-digit comparison / `0.1+0.2=0.3`
- Empty-cell duality (`A1=0` and `A1=""` when `A1` is blank; `0=""` is false)
- Type ranking for `<`/`>` (logical > text > number; `FALSE>100` is `TRUE`)
- Equality vs arithmetic coercion (`"2"=2` is false, `"2"+1` is `3`)
- Case-insensitive text equality
- `TRUE=1` / `FALSE=0`
- `SUM` skips logicals/text in ranges but coerces scalar arguments
- `VLOOKUP` approximate match on unsorted data
- `IF` short-circuit
- 1900 leap year, 1900 vs 1904 dates, implicit intersection, volatile
  functions, locale separators, circular refs, precision-as-displayed,
  wildcards

See [`crates/xlsx-types/src/quirk.rs`](crates/xlsx-types/src/quirk.rs).

## Development

Requires Rust 1.83+.

```bash
cargo test --workspace
cargo run -p xlsx-verify -- --help
```

Headless: libraries + CLI only. No GUI, no COM automation in CI.
