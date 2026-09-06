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
| [`crates/xlsx-engine`](crates/xlsx-engine) | Stub candidates: `seed-compliant` (expanded-corpus pass path) and `naive` (intentional fail path) |
| [`crates/xlsx-bench`](crates/xlsx-bench) | Shared Criterion harness: large-snippet builders, one-file-per-function benches, JSON/CSV snapshots |

`xlsx-engine` stubs remain so the gate still has an explicit pass/fail demo.
`calc-core` is the serious default; it does **not** read fixture expected
outputs. `seed-compliant` implements just enough Excel-like semantics to pass
every non-ignored case on the expanded corpus.

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
   you are claiming: `fixtures/functions/lookup.json`, `fixtures/quirks/…`, etc.
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

Group new cases by what they prove, not by when they were added:

| Directory | What belongs there |
|---|---|
| [`fixtures/seed/`](fixtures/seed) | Original gate seed (kept stable; do not duplicate) |
| [`fixtures/quirks/`](fixtures/quirks) | Excel oddities: type rank, empty duality, coercion, dates, VLOOKUP approx, … |
| [`fixtures/functions/`](fixtures/functions) | Function families (`SUM`/`AVERAGE`/`COUNT`/`COUNTIF`, logicals, text, lookup, `TYPE`) |
| [`fixtures/functions/`](fixtures/functions) | Function families (`SUM`/`AVERAGE`/`COUNT`, logicals, text, lookup, `TYPE`, `PMT`/`FV`) |
| [`fixtures/functions/`](fixtures/functions) | Function families (`SUM`/`AVERAGE`/`COUNT`/`COUNTIF`, logicals, text, lookup, `TYPE`, `PMT`, `PV`) |
| [`fixtures/functions/`](fixtures/functions) | Function families (`SUM`/`AVERAGE`/`COUNT`/`COUNTIF`, logicals, text, lookup, `TYPE`, `PMT`/`NPER`) |
| [`fixtures/functions/`](fixtures/functions) | Function families (`SUM`/`AVERAGE`/`COUNT`, logicals, text, lookup, `TYPE`, `PMT`, `RATE`) |
| [`fixtures/functions/`](fixtures/functions) | Function families (`SUM`/`AVERAGE`/`COUNT`/`COUNTIF`, logicals, text, lookup, `TYPE`, `PMT`/`IPMT`) |
| [`fixtures/functions/`](fixtures/functions) | Function families (`SUM`/`AVERAGE`/`COUNT`, logicals, text, lookup, `TYPE`, `PMT`/`PPMT`) |
| [`fixtures/functions/`](fixtures/functions) | Function families (`SUM`/`AVERAGE`/`COUNT`/`COUNTIF`, logicals, text, lookup, `TYPE`, `PMT`, `CUMPRINC`) |
| [`fixtures/functions/`](fixtures/functions) | Function families (`SUM`/`AVERAGE`/`COUNT`, logicals, text, lookup, `TYPE`, `PMT`, `CUMIPMT`) |
| [`fixtures/functions/`](fixtures/functions) | Function families (`SUM`/`AVERAGE`/`COUNT`/`COUNTIF`, logicals, text, lookup, `TYPE`, `EFFECT`) |
| [`fixtures/functions/`](fixtures/functions) | Function families (`SUM`/`AVERAGE`/`COUNT`, logicals, text, lookup, `TYPE`, `PMT`, `EFFECT`) |
| [`fixtures/functions/`](fixtures/functions) | Function families (`SUM`/`AVERAGE`/`COUNT`/`COUNTIF`, logicals, text, lookup, `TYPE`, `NOMINAL`) |
| [`fixtures/functions/`](fixtures/functions) | Function families (`SUM`/`AVERAGE`/`COUNT`, logicals, text, lookup, `TYPE`, `PMT`, `NOMINAL`) |
| [`fixtures/functions/`](fixtures/functions) | Function families (`SUM`/`AVERAGE`/`COUNT`/`COUNTIF`, logicals, text, lookup, `TYPE`, `PDURATION`) |
| [`fixtures/functions/`](fixtures/functions) | Function families (`SUM`/`AVERAGE`/`COUNT`, logicals, text, lookup, `TYPE`, `PMT`, `PDURATION`) |
| [`fixtures/functions/`](fixtures/functions) | Function families (`SUM`/`AVERAGE`/`COUNT`, logicals, text, lookup, `TYPE`, `PMT`, `RRI`) |
| [`fixtures/functions/`](fixtures/functions) | Function families (`SUM`/`AVERAGE`/`COUNT`/`COUNTIF`, logicals, text, lookup, `TYPE`, `PMT`, `TOCOL`) |
| [`fixtures/functions/`](fixtures/functions) | Function families (`SUM`/`AVERAGE`/`COUNT`/`COUNTIF`/`COUNTIFS`, logicals, text, lookup, `TYPE`) |
| [`fixtures/functions/`](fixtures/functions) | Function families (`SUM`/`AVERAGE`/`COUNT`, logicals, text, lookup, `TYPE`, `PMT`) |
| [`fixtures/operators/`](fixtures/operators) | Unary/`%`, intersection / union-shaped ranges |
| [`fixtures/ignored/`](fixtures/ignored) | Documented `ignore` (volatile, locale, precision-as-displayed, hidden rows) |

If a result is not something you can justify from documented Excel behavior,
set `ignore` with a reason. Do not invent a golden.

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
| `options` | `locale`, `date_system`, `array_mode` (modeled; seed uses defaults). Optional `rng_seed` is a **test hook** for volatile RNG — not an Excel argument. |

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

## Corpus

The seed files under [`fixtures/seed/`](fixtures/seed) stay as the original
gate (arithmetic, comparisons, `SUM` / `IF` / `VLOOKUP`, empty-vs-zero, array
literals, a defined name, and a stored-formula cell). Additional cases live
beside them by category (see *How to add fixtures*).

`calc-core` implements a real parser/evaluator for this corpus (and is the
CLI default). `seed-compliant` is a stub that implements just enough Excel-like
semantics to pass every non-ignored case. `naive` uses IEEE / weak coercion so
the same corpus still produces visible `FAIL` rows (division by zero → `+inf`,
`"2"+1` → `#VALUE!`, `TRUE=1` → `FALSE`, blank `= 0` → `FALSE`, first-cell
instead of implicit intersection, …). Ignored cases (volatile / locale / …)
are skipped by all candidates.

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
| [`eval/functions.rs`](crates/xlsx-engine-core/src/eval/functions.rs) | Dispatch: aggregators (`SUM`/`SUMIF`/`SUMIFS`/`AVERAGEIF`/`AVERAGEIFS`/`COUNTIF`/`COUNTIFS`/`SUMPRODUCT`), logicals (`IF`/`IFS`/`SWITCH`), lookup (`VLOOKUP`/`HLOOKUP`/`XLOOKUP`/`INDEX`/`MATCH`/`FILTER`/`UNIQUE`/`SORT`/`SORTBY`/`TOCOL`/`TOROW`/`SEQUENCE`/`VSTACK`/`HSTACK`/`WRAPCOLS`/`WRAPROWS`/`TAKE`/`DROP`/`EXPAND`/`CHOOSECOLS`/`CHOOSEROWS`/`MAKEARRAY`), dates (`DATE`/`EOMONTH`/`NETWORKDAYS`/`NETWORKDAYS.INTL`/`WEEKDAY`/`WORKDAY`/`WORKDAY.INTL`/`YEARFRAC`), math (`ROUND`/`ROUNDUP`/`ROUNDDOWN`/`FLOOR`/`CEILING`/`RANDARRAY`), text (`LEFT`/`SUBSTITUTE`/`REPLACE`/`FIND`/`SEARCH`/`TEXT`/`TEXTJOIN`/`TEXTSPLIT`/`TEXTAFTER`/`TEXTBEFORE`/`CONCAT`), financial (`NPV`/`XNPV`/`PMT`/`FV`/`PV`/`NPER`/`RATE`/`IPMT`/`PPMT`/`CUMPRINC`/`CUMIPMT`/`IRR`/`XIRR`/`MIRR`/`EFFECT`/`NOMINAL`/`PDURATION`/`RRI`), `TYPE` / `IS*` |
| [`eval/sumif.rs`](crates/xlsx-engine-core/src/eval/sumif.rs) | Excel `SUMIF` kernel (criteria walk, reshape `sum_range`, no array literals) |
| [`eval/sumifs.rs`](crates/xlsx-engine-core/src/eval/sumifs.rs) | Excel `SUMIFS`: multi-criteria AND, same-shape ranges |
| [`eval/countifs.rs`](crates/xlsx-engine-core/src/eval/countifs.rs) | Excel `COUNTIFS`: multi-criteria AND, same-shape ranges, COUNTIF matcher |
| [`eval/averageif.rs`](crates/xlsx-engine-core/src/eval/averageif.rs) | Excel `AVERAGEIF` kernel (reshape `average_range`, `#DIV/0!` when empty) |
| [`eval/averageifs.rs`](crates/xlsx-engine-core/src/eval/averageifs.rs) | Excel `AVERAGEIFS`: multi-criteria AND, same-shape ranges, `#DIV/0!` when empty |
| [`eval/sumproduct.rs`](crates/xlsx-engine-core/src/eval/sumproduct.rs) | `SUMPRODUCT`: array-context args, boolean 0/1 via `--`/`*`, packed f64 hot path |
| [`eval/substitute.rs`](crates/xlsx-engine-core/src/eval/substitute.rs) | Excel `SUBSTITUTE` kernel (case-sensitive, nth instance, empty `old_text` no-op; ASCII SWAR / last-byte probe) |
| [`eval/replace.rs`](crates/xlsx-engine-core/src/eval/replace.rs) | Excel `REPLACE` kernel (1-based span, Unicode scalars / Compat v2) |
| [`eval/find.rs`](crates/xlsx-engine-core/src/eval/find.rs) | Excel `FIND` kernel (case-sensitive, `start_num`, empty `find_text`) |
| [`eval/search.rs`](crates/xlsx-engine-core/src/eval/search.rs) | Excel `SEARCH` kernel (case-insensitive, `*`/`?`/`~` wildcards, `start_num`) |
| [`eval/textafter.rs`](crates/xlsx-engine-core/src/eval/textafter.rs) | Excel `TEXTAFTER` kernel (nth delimiter, `match_mode` / `match_end` / `if_not_found`) |
| [`eval/textbefore.rs`](crates/xlsx-engine-core/src/eval/textbefore.rs) | Excel `TEXTBEFORE` (nth delimiter, `match_mode` / `match_end` / `if_not_found`) |
| [`eval/textjoin.rs`](crates/xlsx-engine-core/src/eval/textjoin.rs) | `TEXTJOIN` with cycling delimiters and `ignore_empty` |
| [`eval/textsplit.rs`](crates/xlsx-engine-core/src/eval/textsplit.rs) | `TEXTSPLIT` col/row split, `ignore_empty`, `match_mode`, `pad_with` |
| [`eval/concat.rs`](crates/xlsx-engine-core/src/eval/concat.rs) | Excel `CONCAT`: row-major flatten, blanks/`""` add nothing, 32,767 UTF-16 cap |
| [`eval/round.rs`](crates/xlsx-engine-core/src/eval/round.rs) | Excel `ROUNDUP` / `ROUNDDOWN` (away / toward zero, negative `num_digits`) |
| [`eval/switch.rs`](crates/xlsx-engine-core/src/eval/switch.rs) | Excel `SWITCH` exact-match kernel (first hit, default / `#N/A`) |
| [`eval/ifs.rs`](crates/xlsx-engine-core/src/eval/ifs.rs) | `IFS` pair-selection kernel (eager eval, first TRUE, no-match `#N/A`) |
| [`eval/unique.rs`](crates/xlsx-engine-core/src/eval/unique.rs) | `UNIQUE(array, [by_col], [exactly_once])` hash distinctness |
| [`eval/filter.rs`](crates/xlsx-engine-core/src/eval/filter.rs) | `FILTER` mask/select kernel (`#CALC!` / `if_empty`, row vs column) |
| [`eval/sort.rs`](crates/xlsx-engine-core/src/eval/sort.rs) | `SORT(array, [sort_index], [sort_order], [by_col])` key-extract / index permute |
| [`eval/xlookup.rs`](crates/xlsx-engine-core/src/eval/xlookup.rs) | `XLOOKUP` match/search kernel (`match_mode` / `search_mode` / `if_not_found`) |
| [`eval/sortby.rs`](crates/xlsx-engine-core/src/eval/sortby.rs) | `SORTBY(array, by_array1, [sort_order1], …)` key-extract / index permute |
| [`eval/tocol.rs`](crates/xlsx-engine-core/src/eval/tocol.rs) | `TOCOL(array, [ignore], [scan_by_col])`: flatten to a column |
| [`eval/torow.rs`](crates/xlsx-engine-core/src/eval/torow.rs) | `TOROW(array, [ignore], [scan_by_col])` flatten-to-row kernel |
| [`eval/sequence.rs`](crates/xlsx-engine-core/src/eval/sequence.rs) | `SEQUENCE(rows, [columns], [start], [step])` row-major generator |
| [`eval/vstack.rs`](crates/xlsx-engine-core/src/eval/vstack.rs) | `VSTACK(array1, [array2], …)` vertical append; `#N/A` width pad |
| [`eval/wrapcols.rs`](crates/xlsx-engine-core/src/eval/wrapcols.rs) | `WRAPCOLS` column-wrap kernel (1-D vector, `#N/A` pad, `#NUM!` / `#VALUE!`) |
| [`eval/wraprows.rs`](crates/xlsx-engine-core/src/eval/wraprows.rs) | `WRAPROWS(vector, wrap_count, [pad_with])` row-wrap + pad |
| [`eval/hstack.rs`](crates/xlsx-engine-core/src/eval/hstack.rs) | `HSTACK(array1, [array2], …)`: left-to-right append, `#N/A` height pad |
| [`eval/take.rs`](crates/xlsx-engine-core/src/eval/take.rs) | `TAKE(array, rows, [cols])` window slice (negative = from end; `0` → `#CALC!`) |
| [`eval/choosecols.rs`](crates/xlsx-engine-core/src/eval/choosecols.rs) | `CHOOSECOLS(array, col_num1, …)` column pick (neg index / `#VALUE!`) |
| [`eval/drop.rs`](crates/xlsx-engine-core/src/eval/drop.rs) | `DROP(array, rows, [cols])` rectangle slice (negative = from end) |
| [`eval/expand.rs`](crates/xlsx-engine-core/src/eval/expand.rs) | `EXPAND(array, rows, [columns], [pad_with])` grow/pad (`#N/A` / shrink `#VALUE!`) |
| [`eval/chooserows.rs`](crates/xlsx-engine-core/src/eval/chooserows.rs) | `CHOOSEROWS(array, row_num1, …)` pick kernel (negative index / `#VALUE!`) |
| [`eval/randarray.rs`](crates/xlsx-engine-core/src/eval/randarray.rs) | `RANDARRAY([rows],[columns],[min],[max],[integer])` fill (xorshift64*; not Excel's RNG) |
| [`eval/makearray.rs`](crates/xlsx-engine-core/src/eval/makearray.rs) | `MAKEARRAY(rows, cols, LAMBDA(r, c, body))` index kernel (`r*c` / `r+c` specialized) |
| [`eval/npv.rs`](crates/xlsx-engine-core/src/eval/npv.rs) | Excel `NPV` kernel (period-1 discount, range skip of blanks/text/logicals) |
| [`eval/irr.rs`](crates/xlsx-engine-core/src/eval/irr.rs) | Excel `IRR` Newton / secant kernel (20 tries, `1e-7` rate, `#NUM!` on failure) |
| [`eval/xnpv.rs`](crates/xlsx-engine-core/src/eval/xnpv.rs) | Excel `XNPV` kernel (365-day year, serial day counts, blank date → 0) |
| [`eval/xirr.rs`](crates/xlsx-engine-core/src/eval/xirr.rs) | Excel `XIRR` Newton / bisection kernel (100 tries, `1e-8` rate, 365-day serials) |
| [`eval/mirr.rs`](crates/xlsx-engine-core/src/eval/mirr.rs) | Excel `MIRR` kernel (finance / reinvest NPV closed form; streaming factors) |
| [`text_format.rs`](crates/xlsx-engine-core/src/text_format.rs) | Excel `TEXT` for a documented number/date format subset |
| [`dates.rs`](crates/xlsx-engine-core/src/dates.rs) | 1900/1904 serials, leap-year bug, `EOMONTH` / `NETWORKDAYS` / `NETWORKDAYS.INTL` / `WEEKDAY` / `WORKDAY` / `WORKDAY.INTL` / `YEARFRAC` |

**Implemented:** arithmetic and comparison operators (unary `+/-`, `%`, `^`,
`&`, space intersection), host-aware implicit intersection, cell refs /
ranges / defined names, array literals, error propagation, and the function
families above. Criterion matching for `SUMIF` / `SUMIFS` / `AVERAGEIF` /
`AVERAGEIFS` / `COUNTIF` / `COUNTIFS` lives in
[`xlsx_types::Criterion`](crates/xlsx-types/src/criterion.rs)
(`compile` vs `parse`). TVM helpers (`PMT` / `FV` / `PV` / `NPER` / `RATE` /
`IPMT` / `PPMT` / `CUMPRINC` / `CUMIPMT` / `EFFECT` / `NOMINAL` / `PDURATION`
/ `RRI`) live in
[`xlsx-types/src/financial.rs`](crates/xlsx-types/src/financial.rs). Workbook
input is the snippet type in `xlsx-types` (no `.xlsx` IO). Kernels do **not**
read fixture goldens.

**`TEXT` subset** (see [`text_format.rs`](crates/xlsx-engine-core/src/text_format.rs)):
`0` / `#` / `.` / grouping `,` / `%` / `$` and other literals; dates
`yyyy`/`yy`/`mm`/`m`/`dd`/`d`; `General`. Not implemented (no goldens;
those codes return `#VALUE!`): scientific, fractions, sections `;`,
colors/conditions, `*`/`_`/`?`, trailing-comma scaling, time (`h`/`s`),
month/day names. Non-numeric text is returned unchanged.

**Deferred / in progress:** full function library, locale argument separators,
live Excel oracle, and performance bakeoff. The fixture corpus is expanded
beyond the original seed; `calc-core` is expected to pass every non-ignored
case with real Excel-compatible semantics (not by reading fixture goldens).

## Excel compatibility notes

Compatibility beats IEEE purity. Empty is a first-class type: a blank cell is
not the number `0` and not the text `""`, even though many operators treat it
as one or the other. Documented quirk categories:

- 15-digit comparison / `0.1+0.2=0.3` / `1/3+1/3+1/3=1`
- Empty-cell duality (`A1=0` and `A1=""` when `A1` is blank; `0=""` is false;
  stored `""` is *not* blank: `ISBLANK` false, `COUNTA` 1, `""+1` is `#VALUE!`)
- Type ranking for `<`/`>` (logical > text > number). Signature split:
  `FALSE=0` is `TRUE` but `FALSE>0` / `FALSE<=0` use ranking (`TRUE` / `FALSE`)
- Equality vs arithmetic coercion (`"2"=2` is false, `"2"+1` is `3`, `--"2"=2`)
- Case-insensitive text equality (`"A"="a"`) vs case-sensitive `EXACT` / `FIND`; `SEARCH` is case-insensitive; `TEXTAFTER` / `TEXTBEFORE` are case-sensitive unless `match_mode` is TRUE
- `TEXTBEFORE(text, delimiter, [instance_num], [match_mode], [match_end], [if_not_found])`:
  Nth non-overlapping delimiter (negative counts from the end); `match_mode`
  0/1; `match_end` 1 treats the unmatched end (or start, when counting
  backward) as one extra delimiter; empty delimiter matches immediately
  (`""` from the front, the whole text from the end); miss is `#N/A` /
  `if_not_found`; `|instance_num| > LEN(text)` or `instance_num = 0` is
  `#VALUE!`. Array delimiters take the leftmost / longest match. `TEXTAFTER`
  / `TEXTSPLIT` share the same delimiter / instance conventions.
- Classic `FLOOR` / `CEILING`: same-sign multiples; positive number + negative significance is `#NUM!`; significance `0` is `#DIV/0!` except `(0, 0)` → `0`. Negative number + positive significance is allowed (Excel 2010+). `FLOOR.MATH` / `CEILING.MATH` ignore significance sign, treat significance `0` as `0`, and take an optional mode.
- `TRUE=1` / `FALSE=0` in `=` and in arithmetic; `ISNUMBER(TRUE)` is still false
- `SUM` / `AVERAGE` / `COUNT` / `PRODUCT` / `MIN` / `MAX`: skip logicals/text
  in ranges and array literals; coerce scalar arguments (`SUM(TRUE)` is 1,
  `SUM(A1)` of `TRUE` is 0)
- `SUMPRODUCT`: element-wise multiply then sum; uncoerced logicals/text/empty
  are 0; `--` or `*` turns TRUE/FALSE into 1/0; mismatched dimensions are
  `#VALUE!`; arguments evaluate in array context (`(A1:A3>1)*B1:B3`)
- `NPV(rate, values…)`: discounts from period 1; range/array blanks, text, and
  logicals are skipped and do **not** consume a period; scalar logicals/text
  numbers coerce (`NPV(1,TRUE,1)` is 0.75, `NPV(1,A1)` of `TRUE` is 0);
  `rate = -1` with a kept cash flow is `#DIV/0!`
- `XNPV(rate, values, dates)`: irregular-date NPV on a 365-day year,
  `Σ P_i / (1+rate)^((d_i − d_1) / 365)`. Day counts are **serial
  differences** (the 1900 leap-year bug is included when the span crosses
  serial 60). Dates truncate toward zero; the first date is the origin and
  later dates may be unsorted but must not precede it (`#NUM!`). Range /
  array blanks are **zeros**, not skips — a blank date is serial 0 and
  typically `#NUM!`. Text / logicals in a range are `#VALUE!`. Mixed signs
  are not required. `rate = -1` with a later flow is `#DIV/0!`. `NPV` /
  `IRR` are separate functions.
- `VLOOKUP` approximate match binary-searches (wrong answers on unsorted data);
  omitted `range_lookup` defaults to approximate. `XLOOKUP` defaults to exact
  (`match_mode` 0): `*` / `?` are literal unless `match_mode` is 2; exact match
  is type-strict (`1` ≠ `"1"` ≠ `TRUE`, blank ≠ `0` ≠ `""`). `if_not_found` is
  used only on a miss (omitted → `#N/A`). `match_mode` `-1` / `1` are next
  smaller / next larger (linear scan finds the globally closest key; `search_mode`
  `2` / `-2` binary-search and can return the wrong row on unsorted data, or
  miss a present key in exact mode). `search_mode` `-1` takes the last duplicate.
  Wildcard + binary search is `#VALUE!`. A 2-D `return_array` yields a row or
  column array; a 2-D `lookup_array` is `#VALUE!`.
- `FILTER(array, include, [if_empty])`: no matches without `if_empty` is
  `#CALC!`; `if_empty` is used only when the filtered set is empty. `include`
  must be a vector matching height (row filter) or width (column filter), or
  a scalar broadcast. An error inside `include` wins. The result is an
  `ExcelValue::Array` — see spill / model limits below
- `VSTACK(array1, [array2], …)`: row-wise append. Height is the sum of
  argument heights; width is the **max** argument width. A narrower array is
  padded on the right with `#N/A` (not blank). A blank **source** cell stays
  empty. Scalars are 1×1. A computed scalar error (`#DIV/0!` literal, `1/0`,
  `FILTER` → `#CALC!`) surfaces as the whole result; a cell-stored error is
  stacked as a 1×1 (Microsoft’s mixed-width example). A 0-row array is
  ignored; if nothing remains, `#CALC!`. Result is an `ExcelValue::Array` —
  see VSTACK spill / pad limits below
- `SWITCH(expression, value1, result1, …, [default])` uses Excel `=` (not `IF`
  truthiness: `IF(2, …)` is true, `SWITCH(2, TRUE, …)` does not match). First
  hit wins; unused values/results are not evaluated. No match and no default
  is `#N/A` (a nested `IF` missing an else is `FALSE`). `*` / `?` are literal.
- `IF` short-circuits; `AND` / `OR` / `IFS` do not (`AND(FALSE, 1/0)` is `#DIV/0!`;
  `IFS(TRUE, 1, FALSE, 1/0)` is `#DIV/0!`). Unmatched `IFS` is `#N/A` (use a
  final `TRUE` pair as the default).
- Error precedence is left-to-right (`#DIV/0!+#VALUE!` keeps `#DIV/0!`)
- 1900 leap-year bug (`DATE(1900,2,29)` is serial 60); 1904 date system.
  `EOMONTH` inherits it (`EOMONTH(59,0)` / `EOMONTH(60,0)` are both 60).
  `NETWORKDAYS` treats serial 60 as a Wednesday workday and weekends as Sat/Sun.
  `NETWORKDAYS.INTL` uses the same inclusive / reverse-sign / holiday rules with
  weekend codes 1–7 / 11–17 (`#NUM!` otherwise) or a 7-character Mon→Sun `0`/`1`
  string (`#VALUE!` if the length or characters are wrong). `"1111111"` is valid
  and always returns 0. Omitted weekend is Sat/Sun (same as `NETWORKDAYS`).
  `WEEKDAY` is O(1) on the serial (`serial % 7`); 1900-01-01 is Sunday in
  Excel (historically Monday). `return_type` 1/2/3/11–17; anything else is `#NUM!`.
  `WORKDAY` skips Sat/Sun (and optional holidays); `days=0` returns the start
  even on a weekend/holiday; serial 60 is a Wednesday workday.
  `YEARFRAC` day-count bases 0–4 (US 30/360, actual/actual, actual/360,
  actual/365, EU 30/360). Dates swap so the result is ≥ 0. 1900 is a leap
  year (serial 60 is last-of-February; 59 is not). Basis 0 keeps the Excel
  last-day-of-February quirk (Mar 31 is not pulled down to 30).
  `WORKDAY.INTL` adds weekend codes 1–7 / 11–17 and a Monday-first 7-character
  `0`/`1` string (`"0000011"` = Sat/Sun). Invalid codes are `#NUM!`; invalid
  strings (wrong length, non-`0`/`1`, or `1111111`) are `#VALUE!`. Text `"1"`
  is not code 1. The weekly pattern is an O(1) inversion (same holiday adjust
  as `WORKDAY`); `WORKDAY.INTL(start, days)` matches `WORKDAY`.
- Unary `+`/`-` and postfix `%` (`50%` is 0.5, `5%%` is 0.0005)
- Space intersection (`A1:B2 B2`); non-overlap is `#NULL!`
- Implicit intersection of a range in a scalar host cell (`A1:A3` at `B2` → `A2`)
- Wildcards in exact `VLOOKUP` / `MATCH` / `COUNTIF` / `COUNTIFS` (`*` / `?` / `~`) and in `SEARCH` (`*` / `?` / `~`)
- `SUMIF` criteria strings (`">5"`, `"*a*"`, `"="` / `"<>"` blanks), text `"5"` dual-matching numbers, range vs `sum_range` reshape from the top-left, array literals → `#VALUE!`
- `COUNTIF` criteria: operators (`= <> > < >= <=`), numeric text matching both
  number and `"2"`, `"TRUE"` coerced to the logical (use `"TRUE*"` for text),
  `""` / `"="` vs `"<>"` blank duality, errors ignored unless the criterion is
  that error
- `WRAPROWS(vector, wrap_count, [pad_with])`: wraps a row or column into a
  2-D array by rows after every `wrap_count` elements. A 2-D block is
  `#VALUE!`. `wrap_count` truncates toward zero; `< 1` is `#NUM!`. Omitted
  `pad_with` is `#N/A`. Result is always an array value — see spill / size
  limits below.
- `EXPAND(array, rows, [columns], [pad_with])`: grows an array; original
  values stay top-left. Omitted `pad_with` fills new cells with `#N/A`
  (not `0` / blank) — `SUM(EXPAND(…))` is then `#N/A`. A blank pad cell
  writes empty. **Cannot shrink:** `rows` / `columns` below the source
  (after truncate-toward-zero), including `0` and negatives, is `#VALUE!`
  — use `TAKE` / `DROP`. Omitted **or empty** `rows` / `columns` keep the
  current size (`FALSE` → `0` → `#VALUE!`). Result is always an array
  value — see spill / size limits below.
- `COUNTIFS` is `COUNTIF` matching with `SUMIFS` range geometry: every
  `criteria_range` must share rows **and** columns (3×1 vs 1×3 is `#VALUE!`;
  a 1×1 first range does not extend). Pairs AND together by offset. Number
  `5` matches numeric text `"5"`; `"TRUE"` is the logical; `NA()` counts
  `#N/A` cells instead of propagating. Array literals are `#VALUE!` (unlike
  `COUNTIF`). No matches → `0`
- `UNIQUE(array, [by_col], [exactly_once])`: first-occurrence distinct rows
  (or columns when `by_col` is TRUE); case-insensitive text; type-strict
  (`1` ≠ `"1"` ≠ `TRUE`); blanks collapse to one empty; `exactly_once` with
  no survivors is `#CALC!`. Result is always an array value.
- `SORT(array, [sort_index], [sort_order], [by_col])`: stable sort of rows
  (or columns when `by_col` is TRUE) by a 1-based `sort_index` (default 1).
- `SORTBY(array, by_array1, [sort_order1], [by_array2, sort_order2], …)`:
  stable sort of rows (or columns) by one or more **vector** keys. A column
  key matching `array` height sorts rows; a row key matching `array` width
  sorts columns. A 1-D transpose (column + same-length row key) is accepted.
  `sort_order` is `1` ascending (default) or `-1` descending; anything else
  is `#VALUE!`. Type groups follow Excel Data Sort — numbers, then text,
  then FALSE/TRUE, then errors — **not** `<`/`>` ranking (`FALSE>100`).
  Text is case-insensitive ASCII. `1`, `"1"`, and `TRUE` stay in different
  groups. **Blanks are last in both directions.** Out-of-range `sort_index`
  is `#VALUE!`. Result is always an array value.
- `WRAPCOLS(vector, wrap_count, [pad_with])`: wraps a **one-dimensional**
  row or column by filling down each column of height `wrap_count`. A 2-D
  `vector` is `#VALUE!`. `wrap_count` is truncated toward zero; `< 1` is
  `#NUM!`. If `wrap_count >= n` the vector is returned as a single column
  (no pad). Remainder cells default to `#N/A`. Blanks stay empty; errors
  inside the vector are data. Result is always an array value — see
  spill / size limits below. `WRAPROWS` / `TOCOL` / `TOROW` are separate
  workstreams.
- `CHOOSECOLS(array, col_num1, [col_num2], …)`: listed columns, listed
  order, always an array value. **Negative** `col_num` counts from the
  right (`-1` last). **Zero** and out-of-range (abs exceeds width) are
  `#VALUE!`, not INDEX `#REF!`. Fractions truncate toward zero (CHOOSE
  family): `1.9` → 1, `-1.9` → `-1`, `0.9` → 0 → `#VALUE!`. Array
  `col_num` args flatten row-major. Duplicates / reorder allowed. A
  worksheet range evaluates only the selected columns.
- `RANDARRAY([rows], [columns], [min], [max], [integer])`: omitted args
  default to 1 / 1 / 0 / 1 / FALSE. A **blank cell** is 0, not omitted, so
  `RANDARRAY(A1)` of a blank `A1` is `#CALC!`. Decimal dimensions truncate
  toward zero; `< 1` is `#CALC!`. `min > max` is `#VALUE!`; `min == max` is
  a constant fill. `integer=TRUE` requires whole `min`/`max` else `#VALUE!`.
  Integers are inclusive; decimals use `[min, max)` (`u * (max-min)`).
  Result is always an array value (including 1×1). **Volatile:** each
  `evaluate` is a recalc with a new stream. The kernel is xorshift64*,
  **not** Excel's undocumented generator — do not invent sequence goldens.
  `EvalOptions.rng_seed` is a test/bench hook only (not a sixth argument).
  Unseeded `RANDARRAY()` is ignored in the corpus. `RAND()` is a later
  workstream.
- `MAKEARRAY(rows, cols, LAMBDA(r, c, body))`: 1-based indexes relative to
  the result array. `rows`/`cols` truncate toward zero; `< 1` or non-numeric
  is `#VALUE!`; sizes above the worksheet grid are `#NUM!`. The LAMBDA must
  have exactly two name parameters (inline or a defined name that refers to
  one). A body error stays in that cell. A body that returns an array is
  `#CALC!` in that cell. Bare `LAMBDA(...)` (not consumed by `MAKEARRAY`) is
  `#CALC!` — this engine has no first-class function value.
- **Spill limitation:** `evaluate` returns that array. The engine does **not**
  write spilled values into neighboring cells, so occupied destinations never
  yield `#SPILL!`. Scalar operators (`UNIQUE(...)+1`, `SORT(...)+1`) take the
  top-left element (`scalarize`), not a host-aware intersection of a written
  spill. Use `INDEX` / `SUM` / `COUNTA` to consume the array without a grid
  write. The parser does not accept omitted middle arguments (`SORT(a,,-1)`).
  Text order is ASCII case-fold, not locale collation. Excel's ~1,048,576-row
  array cap is not enforced.
  groups. **Blanks are last in both directions.** A 2-D `by_array` or a
  length mismatch is `#VALUE!`. Arguments after `array` are `(by, order)`
  pairs (skipping an order shifts the next key into that slot). At most 64
  keys. Result is always an array value.
- **Spill limitation:** `evaluate` returns that array. The engine does **not**
  write spilled values into neighboring cells, so occupied destinations never
  yield `#SPILL!`. Scalar operators (`UNIQUE(...)+1`, `SORTBY(...)+1`) take
  the top-left element (`scalarize`), not a host-aware intersection of a
  written spill. Use `INDEX` / `SUM` / `COUNTA` to consume the array without
  a grid write. The parser does not accept omitted middle arguments
  (`SORTBY(a, by1,, by2)`). Text order is ASCII case-fold, not locale
  collation. Excel's ~1,048,576-row array cap is not enforced.
- `TOCOL(array, [ignore], [scan_by_col])`: flatten to an n×1 array.
  `ignore` 0 keeps all (default), 1 drops blanks, 2 drops errors, 3 drops
  both; other values (after numeric coerce + trunc toward zero) are
  `#VALUE!`. Stored `""` is text, not a blank. `scan_by_col` FALSE / omitted
  is row-major; TRUE walks columns. No survivors after ignore → `#CALC!`.
  Errors in the source are data unless ignored. Nested array cells unnest.
  Result longer than 1,048,576 rows is `#NUM!`.
- **Spill limitation:** `evaluate` returns that array. The engine does **not**
  write spilled values into neighboring cells, so occupied destinations never
  yield `#SPILL!`. Scalar operators (`UNIQUE(...)+1` / `TOCOL(...)+1`) take
- `TOROW(array, [ignore], [scan_by_col])`: flatten to one row. Default scan
  is row-major (left-to-right, then down); `scan_by_col` TRUE walks
  top-to-bottom then across. `ignore` is a whole number `0` keep all /
  `1` drop blanks / `2` drop errors / `3` drop both (`TRUE` → `1`).
  `""` is text, not blank; `0` and `FALSE` are kept under `ignore=1`.
  All values filtered out → `#CALC!`. Fractional / out-of-range `ignore`
  is `#VALUE!`. Result is always an array value.
- **Spill limitation:** `evaluate` returns that array. The engine does **not**
  write spilled values into neighboring cells, so occupied destinations never
  yield `#SPILL!`. Scalar operators (`UNIQUE(...)+1`, `TOROW(...)+1`) take
  the top-left element (`scalarize`), not a host-aware intersection of a
  written spill. Use `INDEX` / `SUM` / `COUNTA` to consume the array without
  a grid write.
  yield `#SPILL!`. Scalar operators (`UNIQUE(...)+1`, `SEQUENCE(...)+1`) take
  the top-left element (`scalarize`), not a host-aware intersection of a
  written spill. Use `INDEX` / `SUM` / `COUNTA` to consume the array without
  a grid write.
- `SEQUENCE(rows, [columns], [start], [step])`: omitted optionals default to
  `1`; `rows` is required (`SEQUENCE()` is `#VALUE!`). Fill is row-major
  (`SEQUENCE(2,3)` is `{1,2,3;4,5,6}`). `rows` / `columns` truncate toward
  zero; `0` after truncation is `#CALC!`; a negative size is `#VALUE!`.
  `start` / `step` may be any finite number (including `0` and negatives).
  Result is always an array value, including `1×1`. The parser does not
  accept empty argument slots (`SEQUENCE(,5)` / `SEQUENCE(4,,10)`), so
  write the default `1` explicitly.
- `HSTACK(array1, [array2], …)`: appends arguments left-to-right. Result
  height is the max row count; result width is the sum of column counts.
  A shorter argument is padded with `#N/A` in the extra rows (not blank,
  not `0`). In-bounds blank cells stay `Empty` — this engine does not
  invent the `0` that Microsoft’s published example table sometimes shows
  for a source blank. A 0-row / 0-column array is ignored; if every
  argument is ignored the result is `#CALC!` (Excel cannot return a 0×0
  array). `HSTACK()` is `#VALUE!`. Scalars, including scalar errors, are
  1×1 and are stacked (a leading `#DIV/0!` does not abort the call).
  Result is always an array value.
- **Spill limitation:** `evaluate` returns that array. The engine does **not**
  write spilled values into neighboring cells, so occupied destinations never
  yield `#SPILL!`. Scalar operators (`UNIQUE(...)+1`, `HSTACK(...)+1`) take
- `TAKE(array, rows, [cols])`: positive counts take from the start (top /
  left); **negative counts take from the end** (bottom / right), they are
  not a `DROP` of `|n|`. `|n|` larger than the axis returns the whole axis
  (no padding). `0` after toward-zero truncate is `#CALC!` (Excel cannot
  return an empty array) — so `0.9` / `-0.9` / `FALSE` / a blank count cell
  are also `#CALC!`. `TRUE` → 1. `TAKE(array)` with neither count is
  `#VALUE!`. Result is always an array value.
- **Spill limitation:** `evaluate` returns that array. The engine does **not**
  write spilled values into neighboring cells, so occupied destinations never
  yield `#SPILL!`. Scalar operators (`UNIQUE(...)+1` / `TAKE(...)+1`) take
  the top-left element (`scalarize`), not a host-aware intersection of a
  written spill. Use `INDEX` / `SUM` / `COUNTA` to consume the array without
  a grid write.
- `DROP(array, rows, [cols])`: positive counts drop from the start (top /
  left); negative counts drop from the end (bottom / right). Omitted `cols`
  is `0`. `0` on an axis is a **no-op** (Microsoft's DROP page says `#CALC!`
  "when rows or columns is 0" — that contradicts their own examples
  `=DROP(A2:C4,2)` / `=DROP(A2:C4,,2)` and the useful `=DROP(data,0)`).
  `#CALC!` is an **empty result**: `|rows| >= height` or `|cols| >= width`
  (DROP does not cap the way some sources describe `TAKE`). Counts truncate
  toward zero (`1.9` → 1, `-1.9` → −1). `DROP(array)` with neither count is
  `#VALUE!`. Result is always an array value.
- **Spill limitation:** `evaluate` returns that array. The engine does **not**
  write spilled values into neighboring cells, so occupied destinations never
  yield `#SPILL!`. Scalar operators (`UNIQUE(...)+1`, `DROP(...)+1`) take the
  top-left element (`scalarize`), not a host-aware intersection of a written
  spill. Use `INDEX` / `SUM` / `COUNTA` to consume the array without a grid
  write.
- `CHOOSEROWS(array, row_num1, [row_num2], …)`: pick listed rows in listed
  order. Positive `row_num` is 1-based from the top; **negative** counts from
  the bottom (`-1` = last row). `0` or `|row_num|` past the height is
  `#VALUE!` (not `INDEX`'s `#REF!`). `FALSE` / blank coerce to `0` →
  `#VALUE!`; `TRUE` is row 1. Fractional `row_num` is truncated toward zero
  (`1.9` → 1, `-1.9` → `-1`; modeled `TRUNC`, live Excel not in CI). Each
  `row_num` may be a scalar or an array (row-major flatten). Duplicates are
  kept. Result is always an array value.
- **Spill limitation:** `evaluate` returns that array. The engine does **not**
  write spilled values into neighboring cells, so occupied destinations never
  yield `#SPILL!`. Scalar operators (`UNIQUE(...)+1` / `CHOOSEROWS(...)+1`) take the top-left
  element (`scalarize`), not a host-aware intersection of a written spill.
  Use `INDEX` / `SUM` / `COUNTA` to consume the array without a grid write.
- `TEXTSPLIT(text, col_delimiter, [row_delimiter], [ignore_empty], [match_mode], [pad_with])`:
  inverse of `TEXTJOIN`. Omitted `col_delimiter` (`TEXTSPLIT(text,,row)`) is
  a row-only split; omitted `row_delimiter` is a column-only split; both
  omitted is `#VALUE!`. An empty-string delimiter is `#VALUE!` (not the same
  as omitted). `ignore_empty` TRUE drops empty tokens from consecutive
  delimiters; if that leaves no rows the result is `#CALC!`. `match_mode` 0
  is case-sensitive, 1 is ASCII case-insensitive; anything else is `#VALUE!`.
  Uneven 2-D rows pad with `pad_with` (default `#N/A`). Pieces stay text.
  Result is always an array value — see spill / pad limits below.
- `AVERAGEIF` criteria strings (`">5"`, `"*a*"`, `"="` / `"<>"` blanks), text `"5"` dual-matching numbers, range vs `average_range` reshape from the top-left, no matches / no numeric average cells → `#DIV/0!`, empty criteria cell treated as `0`
- `AVERAGEIFS` multi-criteria AND, same-shape ranges (no `AVERAGEIF` reshape; mismatch is `#VALUE!`), `Criterion::compile` like `SUMIFS`, no matches / no numeric average cells → `#DIV/0!`
- `PMT(rate, nper, pv, [fv], [type])`: Excel cash-flow sign (pay out is
  negative); `rate=0` is `-(pv+fv)/nper` (`#DIV/0!` if `nper=0`);
  `rate=-1` / overflow / negative^non-integer `nper` are `#NUM!`; omitted
  `fv`/`type` default to 0; `type` is the OpenFormula PayType multiplier
- `FV(rate, nper, pmt, [pv], [type])`: same cash-flow sign and PayType
  multiplier; `rate=0` is `-pv - pmt*nper`; `nper=0` is `-pv` (not a
  domain error); `rate=-1` and `nper≤0` / overflow / negative^non-integer
  are `#NUM!`; omitted `pv`/`type` default to 0
- `PV(rate, nper, pmt, [fv], [type])`: same identity solved for present
  value; `rate=0` is `-(fv+pmt*nper)` (finite even when `nper=0`: `-fv`);
  `rate=-1` divides by `(1+rate)^nper=0` → `#NUM!`; overflow /
  negative^non-integer `nper` are `#NUM!`; omitted `fv`/`type` default to 0
- `NPER(rate, pmt, pv, [fv], [type])`: inverse of `PMT` (OpenFormula 6.12.29);
  `rate=0` is `-(pv+fv)/pmt` (`#DIV/0!` if `pmt=0`); `rate ≤ -1`, a
  non-positive log argument, or a payment that never reaches `fv` are
  `#NUM!`; omitted `fv`/`type` default to 0; `type` is the same PayType
  multiplier. Tiny rates use `ln1p` so `ln(ratio)/ln(1+r)` does not cancel.
- `RATE(nper, pmt, pv, [fv], [type], [guess])`: Newton-Raphson with secant
  fallback on the same TVM identity as `PMT`, default guess `0.1`, 20
  iterations, successive-rate tolerance `1e-7`. A step to `r <= -1`, a
  guess `<= -1`, `nper=0`, or no settle in 20 tries → `#NUM!`. Long-horizon
  monthly loans often need an explicit guess (default 10% is too far from
  ~0.4% per month). `type` is the OpenFormula PayType multiplier; omitted
  `fv`/`type`/`guess` default to `0` / `0` / `0.1`
- `IPMT(rate, per, nper, pv, [fv], [type])`: interest portion of `PMT` for
  period `per` (OpenFormula 6.12.28: `FV(rate, per-1, PMT, pv, type)*rate`,
  annuity-due divides by `1+rate`; `type≠0` and `per=1` is `0`). `per` must
  be in `1..=nper` (`per < 1` or `per ≥ nper+1` → `#NUM!`). Reuses `PMT`
  and `pow_term`; does not export worksheet `FV`
- `PPMT(rate, per, nper, pv, [fv], [type])`: principal portion of a period
  payment (`PMT − IPMT`). `per` must be in `1…nper` (`#NUM!` otherwise).
  `type=1` and `per=1` is all principal (no interest yet). Remaining
  balance uses the same `pow_term` closed form as `PMT`. Worksheet `IPMT`
  is a sibling TVM helper.
- `CUMPRINC(rate, nper, pv, start_period, end_period, type)`: cumulative
  principal (same cash-flow sign). `rate ≤ 0` / `nper ≤ 0` / `pv ≤ 0`,
  `start < 1` / `end < 1` / `start > end` / `end > nper`, or `type` not
  0 or 1 → `#NUM!`. Periods and `type` truncate toward 0. All six args
  required. Closed form: `type=0` is `FV(start-1) − FV(end)`; `type=1`
  is a geometric sum of the annuity-due PPMT loop. Does not read goldens.
- `CUMIPMT(rate, nper, pv, start_period, end_period, type)`: cumulative
  interest (OpenFormula 6.12.12 = Σ IPMT, `fv` always 0). All six args
  required. `nper` / `start` / `end` / `type` truncate toward 0.
  `rate ≤ 0`, `nper ≤ 0`, `pv ≤ 0`, `start < 1`, `end < 1`,
  `start > end`, `end > nper`, or `type` not 0/1 → `#NUM!`. Period 1 /
  type 0 is `−pv·rate`; type 1 / period 1 is 0. Closed form (not a
  period loop). Does **not** register worksheet `IPMT`.
- `EFFECT(nominal_rate, npery)`: OpenFormula 6.12.19
  `(1 + nominal/npery)^npery − 1`; `npery` is `TRUNC`'d toward zero;
  `nominal_rate ≤ 0` or truncated `npery < 1` is `#NUM!`; `npery = 1` is
  the identity; overflow is `#NUM!`.
- `NOMINAL(effect_rate, npery)`: OpenFormula 6.12.32
  `npery · ((1 + effect)^(1/npery) − 1)`; `npery` is `TRUNC`'d toward zero;
  `effect_rate ≤ 0` or truncated `npery < 1` is `#NUM!`; `npery = 1` is
  the identity; overflow is `#NUM!`.
- `PDURATION(rate, pv, fv)`: OpenFormula 6.12.32
  `log(fv/pv) / log(1+rate)`; all three arguments must be positive
  (`≤ 0` or non-finite is `#NUM!`); `pv = fv` is `0`; `fv = pv·(1+rate)`
  is `1`; `fv < pv` is a signed (negative) period count. Tiny rates use
  `ln1p`.
- `RRI(nper, pv, fv)`: equivalent rate `(fv/pv)^(1/nper)−1` (OpenFormula
  6.12.45). Production uses `expm1(ln1p((fv−pv)/pv)/nper)` so tiny growth
  does not cancel. `nper≤0` or `pv=0` is `#NUM!`; opposite signs with
  `nper≠1` are `#NUM!` (`nper=1` is the simple return). Overflow follows
  `POWER`. Exactly three arguments; omitted extras are `#VALUE!`.
- Circular refs modeled as `#CIRCULAR!`
- `IRR(values, [guess])`: Newton-Raphson with secant fallback, default guess
  `0.1`, 20 iterations, rate tolerance `1e-7` (0.00001 percent). Needs at
  least one inflow and one outflow. Text / logicals / empty cells in a
  range or array are skipped (they do **not** occupy a period; store `0`
  for a quiet period). Convergence failure, no sign change, guess `-1`,
  or a Newton step to `r <= -1` → `#NUM!`. `NPV` is a separate function.
- `XIRR(values, dates, [guess])`: root of `XNPV(r, values, dates) = 0` on a
  365-day year. Day counts are **serial differences** (1900 leap-year bug
  included; 2008-01-01 → 2009-01-01 is 366 serial days). First date is the
  origin; a preceding date, a length mismatch, or no sign change is `#NUM!`.
  Invalid dates (negative / past 9999-12-31) are `#VALUE!`. Range / array
  blanks are **zeros** (a blank date is serial 0), not skips; text / logicals
  in a range or array are `#VALUE!`. Newton-Raphson from `guess` (default
  `0.1`) with bisection fallback, 100 iterations, rate tolerance `1e-8`
  (0.000001 percent). Guess `-1` → `#NUM!`. `XNPV` is a separate function.
- `MIRR(values, finance_rate, reinvest_rate)`: Microsoft closed form
  `((−NPV(rrate, values⁺)·(1+rrate)^n) / (NPV(frate, values⁻)·(1+frate)))^(1/(n−1))−1`.
  Needs at least one inflow and one outflow (`#DIV/0!` otherwise). Range /
  array blanks, text, and logicals are skipped (same compaction as `IRR`);
  stored `0` occupies a period. `finance_rate` / `reinvest_rate` of `-1`
  with a cash flow of that sign is `#DIV/0!` (Excel `NPV`). Rates coerce
  like other scalars (`TRUE`→1, `"0.1"`→0.1).
- Volatile / locale / precision-as-displayed / hidden-row `SUBTOTAL` are
  catalogued as `ignore` until they can be evaluated honestly. `RANDARRAY`
  is implemented; unseeded `RANDARRAY()` / `RAND()` stay ignored because
  there is no Excel-matching sequence to record.

**`FILTER` / `WRAPCOLS` spill / model limits** (honest, not hidden behind a broken case):
**`CHOOSECOLS` spill / model limits** (honest, not hidden behind a broken case):

- CHOOSECOLS returns an array **value**. The snippet workbook has no spill
  grid, so a blocked cell below/right of the host never yields `#SPILL!`.
- Scalar operators (`CHOOSECOLS(...)+1`) take the top-left element
  (`scalarize`), not a host-aware intersection of a written spill.
- When `array` is a range, only selected columns are walked. An error or
  circular formula that exists *only* in a dropped column is not observed.
- Excel's ~16,384-column cap is not enforced; size is memory-bounded.
**`EXPAND` spill / size limits** (honest, not hidden behind a broken case):

- EXPAND returns an array **value**. Occupied neighbors never yield
  `#SPILL!` — the snippet workbook has no spill grid. Pick cells with
  `INDEX` / `SUM` / `COUNTA`.
- Scalar operators (`EXPAND(...)+1`) take the top-left element, not a
  host-aware intersection of a written spill.
- Worksheet caps **are** enforced as `#NUM!` before allocate: output
  height `> 1,048,576` or width `> 16,384`. That is a size error, not
  occupancy `#SPILL!`. We do not invent a `#SPILL!` golden.
- The parser does not accept empty commas (`EXPAND(A1:B2,,4)`). Omit a
  trailing `columns`, or pass a blank cell for an empty dimension.
- `pad_with` is a scalar (top-left of an array). An error used as pad is
  a pad **value**; it does not fail the call.

**`MAKEARRAY` / `LAMBDA` limits** (honest, not hidden behind a broken case):

- MAKEARRAY returns an array **value**. The snippet workbook has no spill
  grid, so a blocked cell below/right of the host never yields `#SPILL!`.
- Immediately-invoked `LAMBDA(...)(args)` is not parsed. Optional LAMBDA
  parameters and `LET` helpers are out of scope. Parameter names that
  tokenize as A1 refs are `#VALUE!`.
- Excel's worksheet array-size cap is enforced (`1,048,576` rows /
  `16,384` columns); larger dimensions are `#NUM!`.

**`FILTER` spill / model limits** (honest, not hidden behind a broken case):

- These functions return an array **value**. The snippet workbook has no spill
  grid, so a blocked cell below/right of the host never yields `#SPILL!`.
- Comparison / arithmetic operators still scalarize. `FILTER(A1:A3, A1:A3>1)`
  is not a boolean-array include — pass a logical/numeric vector (literal or
  range). `*` / `+` criteria broadcasting is not modeled. `WRAPCOLS(...)+1`
  takes the top-left element, not a host-aware intersection of a written spill.
  Consume with `INDEX` / `SUM` / `COUNTA` / `TYPE`.
- Excel's worksheet array caps (~1,048,576 rows / 16,384 columns) are not
  enforced; size is memory-bounded. Live Excel would `#NUM!` an oversized
  `WRAPCOLS` result (for example `wrap_count = 1` on a 20,000-cell row).

**`SORT` spill / model limits** (honest, not hidden behind a broken case):

- SORT returns an array **value**. Occupied neighbors never yield `#SPILL!`.
- Scalar operators take the top-left sorted cell (`SORT({10;20;5})+1` is 6).
- Text collation is ASCII case-insensitive, not locale-aware.
- Omitted middle arguments (`SORT(array,,-1)`) do not parse; pass explicit
  `sort_index` / `sort_order`.
**`SORTBY` spill / model limits** (honest, not hidden behind a broken case):

- SORTBY returns an array **value**. Occupied neighbors never yield `#SPILL!`.
- Scalar operators take the top-left sorted cell
  (`SORTBY({10;20;5},{10;20;5})+1` is 6).
- Text collation is ASCII case-insensitive, not locale-aware.
- Omitted middle arguments (`SORTBY(array, by1,, by2)`) do not parse; pass
  explicit `sort_order` when chaining keys.
- A `by_array` must be one row or one column; a matrix is `#VALUE!`.
- Excel's ~1,048,576-row array cap is not enforced; size is memory-bounded.
**`TOCOL` spill / model limits** (honest, not hidden behind a broken case):

- TOCOL returns an array **value** (n×1). The snippet workbook has no spill
  grid, so a blocked cell below the host never yields `#SPILL!`.
- Excel's 1,048,576-row result cap **is** enforced (`#NUM!`). Occupancy of
  neighboring cells is not modeled — there is no `#SPILL!` path.
- Kept blanks stay `Empty` (not coerced to `0`). Microsoft's "blank values
  return a 0" example is display / arithmetic, not the stored type.
- `TOROW` / `WRAPCOLS` / `WRAPROWS` are out of scope.
**`TOROW` spill / model limits** (honest, not hidden behind a broken case):

- TOROW returns an array **value** (one row, including 1×1). The snippet
  workbook has no spill grid, so a blocked cell to the right of the host
  never yields `#SPILL!`.
- Excel's worksheet column cap (16,384 / `XFD`) is **not** enforced. A
  result wider than that is memory-bounded here; live Excel would `#NUM!`.
  The ~1,048,576-row cap does not apply (the result is one row).
- Scalar operators still take the top-left element. Consume the row with
  `INDEX` / `SUM` / `COUNTA` / `TYPE` instead of relying on a written spill.
**`SEQUENCE` spill / size limits** (honest, not hidden behind a broken case):

- SEQUENCE returns an array **value**. Occupied cells in what would be the
  Excel spill zone never yield `#SPILL!` — see `fn.sequence.no-grid-write`.
- Excel would `#SPILL!` if the sequence could not fit from the host cell to
  the sheet edge (`1,048,576` rows × `16,384` columns). That sheet-edge cap
  is **not** enforced; there is no spill grid.
- A safety cap of **16,777,216** cells (`2^24`, `SEQUENCE_MAX_CELLS`) rejects
  `SEQUENCE(16777217)` / `SEQUENCE(5000,5000)` / `SEQUENCE(1E20)` as
  `#VALUE!` without allocating. This is a model limit, not Excel's
  sheet-edge `#SPILL!`.
**`VSTACK` spill / pad / width** (honest):

- VSTACK returns the array that **would** spill. Occupied neighbors never
  yield `#SPILL!` (`fn.vstack.no-grid-write`). `VSTACK(...)+1` takes the
  top-left via `scalarize`, not a host-aware intersection of a written spill.
- Width pad is `#N/A` inside that array (`fn.vstack.pad-*`). It is **not**
  empty: `COUNTA` counts it, `COUNTBLANK` does not, `SUM` surfaces `#N/A`.
- Excel's `IFNA(VSTACK(...), "")` rewrites each pad cell. This engine's
  `IFNA` / `IFERROR` only replace a **scalar** error (`fn.vstack.ifna-does-not-rewrite-pads`).
  Pick a pad with `INDEX`.
- Omitted middle arguments (`VSTACK(A1,,B1)`) are not modeled — the parser
  requires an expression after each comma.
- Excel's ~1,048,576-row cap is not enforced.

**`WRAPROWS` spill / size limits** (honest, not hidden behind a broken case):

- WRAPROWS returns an array **value**. Occupied neighbors never yield
  `#SPILL!` — the snippet workbook has no spill grid. Pick cells with
  `INDEX` / `SUM` / `COUNTA`.
- Scalar operators (`WRAPROWS(...)+1`) take the top-left element, not a
  host-aware intersection of a written spill.
- Worksheet caps **are** enforced as `#NUM!` before allocate: `wrap_count`
  (output width) `> 16,384`, or `ceil(n / wrap_count)` (output height)
  `> 1,048,576`. That is a size error, not occupancy `#SPILL!`. We do not
  invent a `#SPILL!` golden for a blocked spill zone.
- `pad_with` is a scalar (top-left of an array). An error used as pad is a
  pad **value**; it does not fail the call.

**`HSTACK` spill / pad / `#N/A` height quirks** (honest, not hidden behind a
broken case):

- HSTACK returns an array **value**. Occupied cells in the Excel spill zone
  never yield `#SPILL!`.
- Height padding is `#N/A`: `ISNA` true, `COUNTA` counts the pad, `COUNTBLANK`
  does not, `SUM` surfaces `#N/A`. That is different from an in-bounds blank
  (`Empty`), which `COUNTBLANK` does count.
- `IFNA` / `IFERROR` here unwrap only a **scalar** error. `IFNA(HSTACK(...),"")`
  therefore does **not** blank pad cells the way Excel’s dynamic-array `IFNA`
  would after a spill. Pick a pad with `INDEX`.
- Omitted arguments (`HSTACK(a,,b)`) do not parse. Excel would treat the hole
  as a missing optional; this parser requires an expression.
- The 254-argument cap and worksheet array-size caps are not enforced.

**`TAKE` negative-count / spill / model limits** (honest, not hidden):

- Negative `rows` / `cols` mean “from the end”, not “drop that many from
  the start”. `TAKE({1;2;3}, -2)` is `{2;3}`.
- `0` (and anything that truncates to `0`) is `#CALC!`, matching Microsoft,
  not `#VALUE!`. A blank count cell is `0`, which is **not** the same as an
  omitted argument (Excel’s `TAKE(a,,2)` keeps all rows).
- The parser does not accept omitted middle arguments (`TAKE(a,,2)`). Use
  an oversize `rows` count to keep every row while slicing columns.
- TAKE returns an array **value**. The snippet workbook has no spill grid,
  so a blocked cell below/right of the host never yields `#SPILL!`.
- Excel's `#NUM!` when an array exceeds ~1,048,576 rows is not enforced;
  size is memory-bounded.

**`DROP` spill / model limits** (honest, not hidden behind a broken case):

- DROP returns an array **value**. Occupied neighbors never yield `#SPILL!`.
- Microsoft's `#CALC!` "when rows or columns is 0" wording is not followed;
  `0` is a no-op. Empty leftover (`|count| >= axis`) is `#CALC!`.
- The parser does not accept omitted-middle arguments (`DROP(a,,2)`). Use
  `DROP(a,0,2)`.
- A range first argument evaluates **only the kept rectangle**. Dropped
  formula cells are not computed (a circular ref in a dropped header does
  not fire). Stored values and errors in the kept region match Excel.
- Excel's worksheet array-size cap (and the documented `#NUM!` for a
  too-large array) is not enforced; allocation is memory-bounded.

**`CHOOSEROWS` index / `#VALUE!` / spill notes** (honest):

- Negative indices are from the end; they are **not** `INDEX` (`INDEX(..., -1)`
  is `#REF!`). Out-of-range and `0` are `#VALUE!`, not `#REF!`.
- Fractional truncation is toward zero (`TRUNC`), matching `CHOOSE` / `INDEX`
  in this engine. That is a modeled rule — CI has no live Excel oracle.
- `SEQUENCE` is not implemented. Reverse a column with explicit negatives
  (`CHOOSEROWS(a, -1, -2, -3)`), not `CHOOSEROWS(a, SEQUENCE(...))`.
- Same spill limit as `UNIQUE` / `FILTER`: evaluate returns the array that
  would spill; occupied neighbors never yield `#SPILL!`. Scalar operators
  take the top-left pick.

**`TEXTSPLIT` spill / pad / model limits** (honest):

- TEXTSPLIT returns an array **value**. Occupied neighbors never yield
  `#SPILL!`. Scalar operators take the top-left token (`scalarize`).
- `IFNA` / `IFERROR` wrap a scalar error. They do **not** rewrite pad `#N/A`
  cells inside the array (Excel's dynamic-array `IFNA` does).
- `text` is a scalar (implicit intersection / top-left). Excel's
  "array of arrays" `TEXTSPLIT` of a range of strings is not modeled.
- Pad cells are `#N/A` (or `pad_with`), not blank: `COUNTA` counts them,
  `ISNA` is TRUE. 1-D results are never padded.
- Excel's ~1,048,576-row array cap is not enforced; size is memory-bounded.

See [`crates/xlsx-types/src/quirk.rs`](crates/xlsx-types/src/quirk.rs). The
catalog also names `error-precedence`, `percent-unary`, and `range-operators`.

## Development

Requires Rust 1.83+.

```bash
cargo test --workspace
cargo run -p xlsx-verify -- --help
cargo run -p xlsx-engine-core --release --example text_bench
```

Headless: libraries + CLI only. No GUI, no COM automation in CI.

## Performance harness (per-function hill-climbing)

Correctness is the **hard gate**. Benches are advisory: they measure time-to-compute
so parallel agents can hill-climb a single function, but a faster result that
fails `xlsx-verify` is not a win.

```bash
# 1. Correctness first (must stay exit 0)
cargo test --workspace
cargo run -p xlsx-verify -- --candidate calc-core

# 2. Then measure (Criterion; not part of cargo test)
cargo bench -p xlsx-bench --bench fn_sum

# Optional 100k-cell case (slower)
XLSX_BENCH_LARGE=1 cargo bench -p xlsx-bench --bench fn_sum

# Compact JSON/CSV snapshot for a later Excel-oracle comparison.
# Does NOT call live Excel; records calc-core wall time + the computed value.
cargo run -p xlsx-bench -- --function SUM --rows 10000 --format json
cargo run -p xlsx-bench -- --function SUM --rows 10000 --format csv -o /tmp/sum.csv
```

Criterion also writes `target/criterion/fn_sum/**/estimates.json` (and HTML
under `target/criterion/report/`). The snapshot schema is a smaller envelope
(`candidate`, `oracle: "none"`, per-row `mean_ns` / `result`) so a future
oracle bakeoff can land without changing bench files.

### How to add a function bench

Convention: **one Excel function → one bench file → one Criterion group**.

1. Copy [`crates/xlsx-bench/benches/fn_sum.rs`](crates/xlsx-bench/benches/fn_sum.rs)
   to `crates/xlsx-bench/benches/fn_<name>.rs` (`<name>` = lowercase function
   id: `average`, `vlookup`, `xlookup`, `error_type`, …).
2. Register it in [`crates/xlsx-bench/Cargo.toml`](crates/xlsx-bench/Cargo.toml):

   ```toml
   [[bench]]
   name = "fn_average"
   harness = false
   ```

3. Call `xlsx_bench::bench_fn(c, "AVERAGE", |g| { ... })`. That helper forces
   the group name `fn_average` so agents do not fight over Criterion ids.
4. Build 10k–100k cell inputs with the snippet helpers — do **not** hand-write
   fixture JSON at this scale:

   ```rust
   use xlsx_bench::prelude::*;

   let range = numeric_column(10_000, |i| (i + 1) as f64);
   let spec = range.call_spec("average.10k", "AVERAGE");
   // workbook/spec constructed once, outside iter
   g.throughput(Throughput::Elements(range.cell_count));
   g.bench_function("range_10k", |b| {
       b.iter(|| black_box(eval_calc_core(black_box(&spec))))
   });
   ```

   Helpers: `numeric_column`, `numeric_grid` / `grid`, `mixed_column` (number /
   blank / text / bool cycle), `SnippetBuilder` for custom shapes.
5. Time **only** `Candidate::evaluate`. Setup (range fill, `EvalSpec`) stays
   outside `iter`.
6. Do not expand the quirk corpus for a bench-only PR. Do not rewrite
   calc-core except tiny hooks the bench actually needs.

`cargo bench` is **not** a correctness signal. Before claiming a performance
win:

1. `cargo test --workspace` exits 0.
2. `xlsx-verify --candidate calc-core` exits 0.
3. The JSON report has `summary.failed == 0` and `summary.errored == 0`.

Skipped (`ignore`) fixtures still do not count as a pass. Hill-climb under
that gate, not around it.
