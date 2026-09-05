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
| [`eval/functions.rs`](crates/xlsx-engine-core/src/eval/functions.rs) | Dispatch: aggregators (`SUM`/`SUMIF`/`SUMIFS`/`AVERAGEIF`/`COUNTIF`/`SUMPRODUCT`), logicals (`IF`/`IFS`/`SWITCH`), lookup (`VLOOKUP`/`HLOOKUP`/`XLOOKUP`/`INDEX`/`MATCH`/`FILTER`/`UNIQUE`), dates (`DATE`/`EOMONTH`/`NETWORKDAYS`/`WEEKDAY`/`WORKDAY`), math (`ROUND`/`ROUNDUP`/`ROUNDDOWN`/`FLOOR`/`CEILING`), text (`LEFT`/`SUBSTITUTE`/`REPLACE`/`FIND`/`SEARCH`/`TEXT`/`TEXTJOIN`/`CONCAT`), financial (`NPV`/`PMT`/`IRR`), `TYPE` / `IS*` |
| [`eval/sumif.rs`](crates/xlsx-engine-core/src/eval/sumif.rs) | Excel `SUMIF` kernel (criteria walk, reshape `sum_range`, no array literals) |
| [`eval/sumifs.rs`](crates/xlsx-engine-core/src/eval/sumifs.rs) | Excel `SUMIFS`: multi-criteria AND, same-shape ranges |
| [`eval/averageif.rs`](crates/xlsx-engine-core/src/eval/averageif.rs) | Excel `AVERAGEIF` kernel (reshape `average_range`, `#DIV/0!` when empty) |
| [`eval/sumproduct.rs`](crates/xlsx-engine-core/src/eval/sumproduct.rs) | `SUMPRODUCT`: array-context args, boolean 0/1 via `--`/`*`, packed f64 hot path |
| [`eval/substitute.rs`](crates/xlsx-engine-core/src/eval/substitute.rs) | Excel `SUBSTITUTE` kernel (case-sensitive, nth instance, empty `old_text` no-op) |
| [`eval/replace.rs`](crates/xlsx-engine-core/src/eval/replace.rs) | Excel `REPLACE` kernel (1-based span, Unicode scalars / Compat v2) |
| [`eval/find.rs`](crates/xlsx-engine-core/src/eval/find.rs) | Excel `FIND` kernel (case-sensitive, `start_num`, empty `find_text`) |
| [`eval/search.rs`](crates/xlsx-engine-core/src/eval/search.rs) | Excel `SEARCH` kernel (case-insensitive, `*`/`?`/`~` wildcards, `start_num`) |
| [`eval/textjoin.rs`](crates/xlsx-engine-core/src/eval/textjoin.rs) | `TEXTJOIN` with cycling delimiters and `ignore_empty` |
| [`eval/concat.rs`](crates/xlsx-engine-core/src/eval/concat.rs) | Excel `CONCAT`: row-major flatten, blanks/`""` add nothing, 32,767 UTF-16 cap |
| [`eval/round.rs`](crates/xlsx-engine-core/src/eval/round.rs) | Excel `ROUNDUP` / `ROUNDDOWN` (away / toward zero, negative `num_digits`) |
| [`eval/switch.rs`](crates/xlsx-engine-core/src/eval/switch.rs) | Excel `SWITCH` exact-match kernel (first hit, default / `#N/A`) |
| [`eval/ifs.rs`](crates/xlsx-engine-core/src/eval/ifs.rs) | `IFS` pair-selection kernel (eager eval, first TRUE, no-match `#N/A`) |
| [`eval/unique.rs`](crates/xlsx-engine-core/src/eval/unique.rs) | `UNIQUE(array, [by_col], [exactly_once])` hash distinctness |
| [`eval/filter.rs`](crates/xlsx-engine-core/src/eval/filter.rs) | `FILTER` mask/select kernel (`#CALC!` / `if_empty`, row vs column) |
| [`eval/npv.rs`](crates/xlsx-engine-core/src/eval/npv.rs) | Excel `NPV` kernel (period-1 discount, range skip of blanks/text/logicals) |
| [`eval/irr.rs`](crates/xlsx-engine-core/src/eval/irr.rs) | Excel `IRR` Newton / secant kernel (20 tries, `1e-7` rate, `#NUM!` on failure) |
| [`text_format.rs`](crates/xlsx-engine-core/src/text_format.rs) | Excel `TEXT` for a documented number/date format subset |
| [`dates.rs`](crates/xlsx-engine-core/src/dates.rs) | 1900/1904 serials, leap-year bug, `EOMONTH` / `NETWORKDAYS` / `WEEKDAY` / `WORKDAY` |

**Implemented:** arithmetic and comparison operators (unary `+/-`, `%`, `^`,
`&`, space intersection), host-aware implicit intersection, cell refs /
ranges / defined names, array literals, error propagation, and the function
families above. Criterion matching for `SUMIF` / `SUMIFS` / `AVERAGEIF` /
`COUNTIF` lives in [`xlsx_types::Criterion`](crates/xlsx-types/src/criterion.rs)
(`compile` vs `parse`). `PMT` lives in
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
- Case-insensitive text equality (`"A"="a"`) vs case-sensitive `EXACT` / `FIND`; `SEARCH` is case-insensitive
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
- `VLOOKUP` approximate match binary-searches (wrong answers on unsorted data);
  omitted `range_lookup` defaults to approximate. `XLOOKUP` defaults to exact
- `FILTER(array, include, [if_empty])`: no matches without `if_empty` is
  `#CALC!`; `if_empty` is used only when the filtered set is empty. `include`
  must be a vector matching height (row filter) or width (column filter), or
  a scalar broadcast. An error inside `include` wins. The result is an
  `ExcelValue::Array` — see spill / model limits below
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
  `WEEKDAY` is O(1) on the serial (`serial % 7`); 1900-01-01 is Sunday in
  Excel (historically Monday). `return_type` 1/2/3/11–17; anything else is `#NUM!`.
  `WORKDAY` skips Sat/Sun (and optional holidays); `days=0` returns the start
  even on a weekend/holiday; serial 60 is a Wednesday workday.
- Unary `+`/`-` and postfix `%` (`50%` is 0.5, `5%%` is 0.0005)
- Space intersection (`A1:B2 B2`); non-overlap is `#NULL!`
- Implicit intersection of a range in a scalar host cell (`A1:A3` at `B2` → `A2`)
- Wildcards in exact `VLOOKUP` / `MATCH` / `COUNTIF` (`*` / `?` / `~`) and in `SEARCH` (`*` / `?` / `~`)
- `SUMIF` criteria strings (`">5"`, `"*a*"`, `"="` / `"<>"` blanks), text `"5"` dual-matching numbers, range vs `sum_range` reshape from the top-left, array literals → `#VALUE!`
- `COUNTIF` criteria: operators (`= <> > < >= <=`), numeric text matching both
  number and `"2"`, `"TRUE"` coerced to the logical (use `"TRUE*"` for text),
  `""` / `"="` vs `"<>"` blank duality, errors ignored unless the criterion is
  that error
- `UNIQUE(array, [by_col], [exactly_once])`: first-occurrence distinct rows
  (or columns when `by_col` is TRUE); case-insensitive text; type-strict
  (`1` ≠ `"1"` ≠ `TRUE`); blanks collapse to one empty; `exactly_once` with
  no survivors is `#CALC!`. Result is always an array value.
- **Spill limitation:** `evaluate` returns that array. The engine does **not**
  write spilled values into neighboring cells, so occupied destinations never
  yield `#SPILL!`. Scalar operators (`UNIQUE(...)+1`) take the top-left
  element (`scalarize`), not a host-aware intersection of a written spill.
  Use `INDEX` / `SUM` / `COUNTA` to consume the array without a grid write.
- `AVERAGEIF` criteria strings (`">5"`, `"*a*"`, `"="` / `"<>"` blanks), text `"5"` dual-matching numbers, range vs `average_range` reshape from the top-left, no matches / no numeric average cells → `#DIV/0!`, empty criteria cell treated as `0`
- `PMT(rate, nper, pv, [fv], [type])`: Excel cash-flow sign (pay out is
  negative); `rate=0` is `-(pv+fv)/nper` (`#DIV/0!` if `nper=0`);
  `rate=-1` / overflow / negative^non-integer `nper` are `#NUM!`; omitted
  `fv`/`type` default to 0; `type` is the OpenFormula PayType multiplier
- Circular refs modeled as `#CIRCULAR!`
- `IRR(values, [guess])`: Newton-Raphson with secant fallback, default guess
  `0.1`, 20 iterations, rate tolerance `1e-7` (0.00001 percent). Needs at
  least one inflow and one outflow. Text / logicals / empty cells in a
  range or array are skipped (they do **not** occupy a period; store `0`
  for a quiet period). Convergence failure, no sign change, guess `-1`,
  or a Newton step to `r <= -1` → `#NUM!`. `NPV` is a separate function.
- Volatile / locale / precision-as-displayed / hidden-row `SUBTOTAL` are
  catalogued as `ignore` until they can be evaluated honestly

**`FILTER` spill / model limits** (honest, not hidden behind a broken case):

- FILTER returns an array **value**. The snippet workbook has no spill grid,
  so a blocked cell below/right of the host never yields `#SPILL!`.
- Comparison / arithmetic operators still scalarize. `FILTER(A1:A3, A1:A3>1)`
  is not a boolean-array include — pass a logical/numeric vector (literal or
  range). `*` / `+` criteria broadcasting is not modeled.
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
