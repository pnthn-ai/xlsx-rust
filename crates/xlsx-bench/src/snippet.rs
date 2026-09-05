//! Cheap builders for large workbook snippets used as bench inputs.
//!
//! Fixture JSON is the wrong tool for 10k–100k cells. These helpers fill a
//! [`Workbook`](xlsx_types::Workbook) in-process: one A1-key allocation per
//! cell, no serde, no corpus files.

use xlsx_types::{Cell, CellAddr, EvalSpec, EvalTarget, ExcelValue, Sheet, Workbook};

/// A filled rectangle plus the A1 range string to feed a function call.
#[derive(Clone, Debug)]
pub struct FilledRange {
    pub workbook: Workbook,
    pub sheet: String,
    pub start: CellAddr,
    pub end: CellAddr,
    /// Populated (non-blank) cells, not the geometric area.
    pub cell_count: u64,
}

impl FilledRange {
    pub fn a1_range(&self) -> String {
        format!("{}:{}", self.start.a1(), self.end.a1())
    }

    /// `=SUM(A1:A10000)` — `function` is the Excel name without `=`.
    pub fn call(&self, function: &str) -> String {
        format!("={}({})", function.trim(), self.a1_range())
    }

    pub fn spec(&self, case_id: impl Into<String>, formula: impl Into<String>) -> EvalSpec {
        formula_spec(case_id, formula, self.workbook.clone())
    }

    pub fn call_spec(&self, case_id: impl Into<String>, function: &str) -> EvalSpec {
        self.spec(case_id, self.call(function))
    }
}

/// Incremental sheet filler. Prefer [`numeric_column`] / [`numeric_grid`] /
/// [`mixed_column`] for the common cases.
pub struct SnippetBuilder {
    sheet: String,
    cells: Vec<(CellAddr, Cell)>,
    min: Option<CellAddr>,
    max: Option<CellAddr>,
}

impl SnippetBuilder {
    pub fn new(sheet: impl Into<String>) -> Self {
        Self {
            sheet: sheet.into(),
            cells: Vec::new(),
            min: None,
            max: None,
        }
    }

    pub fn with_capacity(sheet: impl Into<String>, cells: usize) -> Self {
        let mut s = Self::new(sheet);
        s.cells.reserve(cells);
        s
    }

    pub fn set(&mut self, col: u32, row: u32, value: ExcelValue) -> &mut Self {
        self.put(CellAddr::new(col, row), Cell::value(value));
        self
    }

    pub fn put(&mut self, addr: CellAddr, cell: Cell) {
        self.min = Some(match self.min {
            Some(m) => CellAddr::new(m.col.min(addr.col), m.row.min(addr.row)),
            None => addr,
        });
        self.max = Some(match self.max {
            Some(m) => CellAddr::new(m.col.max(addr.col), m.row.max(addr.row)),
            None => addr,
        });
        self.cells.push((addr, cell));
    }

    /// Fill `[start, start+(cols,rows))`. `f` returning `None` leaves a blank.
    pub fn fill_rect(
        &mut self,
        start: CellAddr,
        rows: u32,
        cols: u32,
        mut f: impl FnMut(u32, u32) -> Option<ExcelValue>,
    ) -> &mut Self {
        self.cells.reserve(rows.saturating_mul(cols) as usize);
        for r in 0..rows {
            for c in 0..cols {
                if let Some(v) = f(r, c) {
                    self.put(CellAddr::new(start.col + c, start.row + r), Cell::value(v));
                }
            }
        }
        // Still record the geometric end so A1 ranges cover blanks.
        let end = CellAddr::new(
            start.col + cols.saturating_sub(1),
            start.row + rows.saturating_sub(1),
        );
        self.min = Some(match self.min {
            Some(m) => CellAddr::new(m.col.min(start.col), m.row.min(start.row)),
            None => start,
        });
        self.max = Some(match self.max {
            Some(m) => CellAddr::new(m.col.max(end.col), m.row.max(end.row)),
            None => end,
        });
        self
    }

    pub fn finish(self) -> FilledRange {
        let mut sheet = Sheet::new(&self.sheet);
        let cell_count = self.cells.len() as u64;
        // A1 keys are required by the snippet map; generate them once here
        // (setup), never on the timed evaluate path.
        let mut a1 = String::with_capacity(8);
        for (addr, cell) in self.cells {
            write_a1(addr.col, addr.row, &mut a1);
            sheet.cells.insert(a1.clone(), cell);
        }
        FilledRange {
            workbook: Workbook {
                sheets: vec![sheet],
                names: vec![],
            },
            sheet: self.sheet,
            start: self.min.unwrap_or(CellAddr::new(0, 0)),
            end: self.max.unwrap_or(CellAddr::new(0, 0)),
            cell_count,
        }
    }
}

/// Column `A1:A{rows}` filled with `f(row_index)` (0-based).
pub fn numeric_column(rows: u32, mut f: impl FnMut(u32) -> f64) -> FilledRange {
    let mut b = SnippetBuilder::with_capacity("Sheet1", rows as usize);
    b.fill_rect(CellAddr::new(0, 0), rows, 1, |r, _| {
        Some(ExcelValue::Number(f(r)))
    });
    b.finish()
}

/// `A1:{last}{rows}` grid of numbers. `f(row, col)` is 0-based.
pub fn numeric_grid(rows: u32, cols: u32, mut f: impl FnMut(u32, u32) -> f64) -> FilledRange {
    let mut b = SnippetBuilder::with_capacity("Sheet1", (rows as usize) * (cols as usize));
    b.fill_rect(CellAddr::new(0, 0), rows, cols, |r, c| {
        Some(ExcelValue::Number(f(r, c)))
    });
    b.finish()
}

/// Alias for [`numeric_grid`].
pub fn grid(rows: u32, cols: u32, f: impl FnMut(u32, u32) -> f64) -> FilledRange {
    numeric_grid(rows, cols, f)
}

/// Column that cycles number / blank / text / bool — the `SUM` skip path at scale.
pub fn mixed_column(rows: u32) -> FilledRange {
    let mut b = SnippetBuilder::with_capacity("Sheet1", rows as usize);
    b.fill_rect(CellAddr::new(0, 0), rows, 1, |r, _| match r % 4 {
        0 => Some(ExcelValue::Number((r + 1) as f64)),
        1 => None,
        2 => Some(ExcelValue::Text("x".into())),
        _ => Some(ExcelValue::Bool(true)),
    });
    b.finish()
}

/// Build an [`EvalSpec`] that evaluates `formula` against `workbook`.
pub fn formula_spec(
    case_id: impl Into<String>,
    formula: impl Into<String>,
    workbook: Workbook,
) -> EvalSpec {
    EvalSpec {
        case_id: case_id.into(),
        workbook,
        target: EvalTarget::formula(formula),
        options: Default::default(),
    }
}

/// Write `A1` / `AA10` into `buf` without `format!` (setup-path hot helper).
fn write_a1(col: u32, row: u32, buf: &mut String) {
    buf.clear();
    let mut c = col + 1;
    let mut tmp = [0u8; 3];
    let mut n = 0usize;
    while c > 0 {
        c -= 1;
        tmp[n] = b'A' + (c % 26) as u8;
        c /= 26;
        n += 1;
    }
    for i in (0..n).rev() {
        buf.push(tmp[i] as char);
    }
    let mut x = row + 1;
    let mut digits = [0u8; 10];
    let mut i = 10usize;
    loop {
        i -= 1;
        digits[i] = b'0' + (x % 10) as u8;
        x /= 10;
        if x == 0 {
            break;
        }
    }
    buf.push_str(std::str::from_utf8(&digits[i..]).unwrap());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn write_a1_matches_celladdr() {
        for (col, row) in [(0, 0), (25, 0), (26, 9), (701, 99), (16383, 1_048_575)] {
            let mut buf = String::new();
            write_a1(col, row, &mut buf);
            assert_eq!(buf, CellAddr::new(col, row).a1());
        }
    }

    #[test]
    fn builder_capacity_and_blanks() {
        let mut b = SnippetBuilder::with_capacity("Data", 4);
        b.fill_rect(CellAddr::new(0, 0), 4, 1, |r, _| {
            if r % 2 == 0 {
                Some(ExcelValue::Number(r as f64))
            } else {
                None
            }
        });
        let filled = b.finish();
        assert_eq!(filled.sheet, "Data");
        assert_eq!(filled.cell_count, 2);
        assert_eq!(filled.a1_range(), "A1:A4");
        assert!(filled.workbook.sheets[0].cells.contains_key("A1"));
        assert!(!filled.workbook.sheets[0].cells.contains_key("A2"));
    }
}
