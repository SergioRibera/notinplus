use super::Inline;
use super::calc::{ColumnRef, Expr, Number};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum ColumnAlign {
    #[default]
    None,
    Left,
    Center,
    Right,
}

/// What a column's total shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Aggregate {
    CountAll,
    CountEmpty,
    CountFilled,
    Sum,
    Average,
    Median,
    Min,
    Max,
    Range,
    /// Rows ticked in the check column.
    Checked,
    Unchecked,
    PercentChecked,
}

impl Aggregate {
    pub const ALL: [Self; 12] = [
        Self::CountAll,
        Self::CountEmpty,
        Self::CountFilled,
        Self::Sum,
        Self::Average,
        Self::Median,
        Self::Min,
        Self::Max,
        Self::Range,
        Self::Checked,
        Self::Unchecked,
        Self::PercentChecked,
    ];

    /// Name in the `notein:table` spec.
    #[must_use]
    pub const fn key(self) -> &'static str {
        match self {
            Self::CountAll => "count",
            Self::CountEmpty => "empty",
            Self::CountFilled => "filled",
            Self::Sum => "sum",
            Self::Average => "average",
            Self::Median => "median",
            Self::Min => "min",
            Self::Max => "max",
            Self::Range => "range",
            Self::Checked => "checked",
            Self::Unchecked => "unchecked",
            Self::PercentChecked => "percent-checked",
        }
    }

    #[must_use]
    pub fn from_key(key: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|op| op.key() == key)
    }

    /// Whether the total needs numbers in the column.
    #[must_use]
    pub const fn numeric(self) -> bool {
        matches!(
            self,
            Self::Sum | Self::Average | Self::Median | Self::Min | Self::Max | Self::Range
        )
    }

    /// Whether the total counts ticks of the check column.
    #[must_use]
    pub const fn checks(self) -> bool {
        matches!(self, Self::Checked | Self::Unchecked | Self::PercentChecked)
    }
}

/// Which rows a total looks at, when the table has a check column.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Scope {
    #[default]
    All,
    Checked,
    Unchecked,
}

impl Scope {
    pub const ALL: [Self; 3] = [Self::All, Self::Checked, Self::Unchecked];

    #[must_use]
    pub const fn key(self) -> &'static str {
        match self {
            Self::All => "all",
            Self::Checked => "checked",
            Self::Unchecked => "unchecked",
        }
    }

    #[must_use]
    pub fn from_key(key: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|scope| scope.key() == key)
    }
}

/// The total shown under a column.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Summary {
    pub op: Aggregate,
    pub scope: Scope,
}

impl From<Aggregate> for Summary {
    fn from(op: Aggregate) -> Self {
        Self {
            op,
            scope: Scope::All,
        }
    }
}

/// A column whose cells come from a formula over the row.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ComputedColumn {
    pub header: String,
    /// Source of an [`Expr`].
    pub formula: String,
    pub summary: Option<Summary>,
}

impl ComputedColumn {
    #[must_use]
    pub fn expr(&self) -> Option<Expr> {
        Expr::parse(&self.formula).ok()
    }
}

/// A column of the table as shown: typed or computed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Column {
    Data(usize),
    Computed(usize),
}

/// A computed cell: the value and, when the formula could not be applied
/// to the row, nothing.
pub type ComputedCell = Option<Number>;

/// A total as shown under a column.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Total {
    pub summary: Summary,
    /// The number, count or percentage as text.
    pub text: String,
}

/// Everything derived from the typed cells: computed columns and totals.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Evaluated {
    /// One entry per computed column, one value per body row.
    pub computed: Vec<Vec<ComputedCell>>,
    /// One entry per data column, then per computed column.
    pub totals: Vec<Option<Total>>,
}

impl Evaluated {
    #[must_use]
    pub fn has_totals(&self) -> bool {
        self.totals.iter().any(Option::is_some)
    }

    #[must_use]
    pub fn computed_text(&self, column: usize, body_row: usize) -> Option<String> {
        self.computed
            .get(column)?
            .get(body_row)?
            .map(|number| Number::format(number.value, number.decimals, number.comma))
    }
}

/// GFM table. Row `0` is the header. Every row holds exactly one cell per
/// column.
///
/// On top of the typed cells a table can have a check column (column `0`
/// holds `[x]` / `[ ]`), a total under any column and computed columns
/// shown after the typed ones. All of that is derived data, see
/// [`Table::evaluate`].
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Table {
    columns: Vec<ColumnAlign>,
    rows: Vec<Vec<Inline>>,
    check_column: bool,
    /// One per data column.
    summaries: Vec<Option<Summary>>,
    computed: Vec<ComputedColumn>,
}

impl Default for Table {
    fn default() -> Self {
        Self::new(2, 2)
    }
}

const CHECKED: &str = "[x]";
const UNCHECKED: &str = "[ ]";

/// Whether `text` is a check mark cell, and whether it is ticked.
fn check_mark(text: &str) -> Option<bool> {
    match text.trim() {
        "" | "[ ]" | "[]" => Some(false),
        "[x]" | "[X]" => Some(true),
        _ => None,
    }
}

impl Table {
    /// Empty table with `columns` columns and `rows` rows (header included).
    #[must_use]
    pub fn new(columns: usize, rows: usize) -> Self {
        let columns = columns.max(1);
        Self {
            columns: vec![ColumnAlign::None; columns],
            rows: vec![vec![Inline::default(); columns]; rows.max(1)],
            check_column: false,
            summaries: vec![None; columns],
            computed: Vec::new(),
        }
    }

    /// Build from parsed rows, padding short rows so the shape stays square.
    #[must_use]
    #[cfg_attr(feature = "sismo", sismo::function("markdown::table_from_rows"))]
    pub fn from_rows(columns: Vec<ColumnAlign>, mut rows: Vec<Vec<Inline>>) -> Self {
        let width = rows
            .iter()
            .map(Vec::len)
            .chain(std::iter::once(columns.len()))
            .max()
            .unwrap_or(1)
            .max(1);
        let mut columns = columns;
        columns.resize(width, ColumnAlign::None);
        if rows.is_empty() {
            rows.push(Vec::new());
        }
        for row in &mut rows {
            row.resize(width, Inline::default());
        }
        Self {
            columns,
            rows,
            check_column: false,
            summaries: vec![None; width],
            computed: Vec::new(),
        }
    }

    #[must_use]
    pub fn columns(&self) -> &[ColumnAlign] {
        &self.columns
    }

    #[must_use]
    pub fn rows(&self) -> &[Vec<Inline>] {
        &self.rows
    }

    #[must_use]
    pub fn cell(&self, row: usize, column: usize) -> Option<&Inline> {
        self.rows.get(row)?.get(column)
    }

    pub fn cell_mut(&mut self, row: usize, column: usize) -> Option<&mut Inline> {
        self.rows.get_mut(row)?.get_mut(column)
    }

    pub fn set_align(&mut self, column: usize, align: ColumnAlign) {
        if let Some(slot) = self.columns.get_mut(column) {
            *slot = align;
        }
    }

    #[cfg_attr(feature = "sismo", sismo::function("markdown::table_insert_row"))]
    pub fn insert_row(&mut self, at: usize) {
        let at = at.clamp(1, self.rows.len());
        let mut row = vec![Inline::default(); self.columns.len()];
        if self.check_column
            && let Some(first) = row.first_mut()
        {
            *first = Inline::from(UNCHECKED);
        }
        self.rows.insert(at, row);
    }

    #[cfg_attr(feature = "sismo", sismo::function("markdown::table_insert_column"))]
    pub fn insert_column(&mut self, at: usize) {
        let at = at.min(self.columns.len());
        self.columns.insert(at, ColumnAlign::None);
        self.summaries.insert(at, None);
        for row in &mut self.rows {
            row.insert(at, Inline::default());
        }
        if at == 0 {
            self.check_column = false;
        }
    }

    /// Remove a body row. The header row is kept.
    #[cfg_attr(feature = "sismo", sismo::function("markdown::table_remove_row"))]
    pub fn remove_row(&mut self, row: usize) -> bool {
        if row == 0 || row >= self.rows.len() {
            return false;
        }
        self.rows.remove(row);
        true
    }

    #[cfg_attr(feature = "sismo", sismo::function("markdown::table_remove_column"))]
    pub fn remove_column(&mut self, column: usize) -> bool {
        if self.columns.len() <= 1 || column >= self.columns.len() {
            return false;
        }
        self.columns.remove(column);
        self.summaries.remove(column);
        for row in &mut self.rows {
            row.remove(column);
        }
        if column == 0 {
            self.check_column = false;
        }
        true
    }

    // Check column.

    /// Whether column `0` holds a check mark per row.
    #[must_use]
    pub const fn has_check_column(&self) -> bool {
        self.check_column
    }

    /// Turn column `0` into check marks (its body cells become `[ ]`) or
    /// back into text (the marks are cleared).
    pub fn set_check_column(&mut self, check: bool) {
        if self.check_column == check {
            return;
        }
        self.check_column = check;
        for row in self.rows.iter_mut().skip(1) {
            if let Some(first) = row.first_mut() {
                *first = if check {
                    Inline::from(match check_mark(first.text()) {
                        Some(true) => CHECKED,
                        _ => UNCHECKED,
                    })
                } else {
                    Inline::default()
                };
            }
        }
        if let Some(summary) = self.summaries.first_mut()
            && !check
            && summary.is_some_and(|summary| summary.op.checks())
        {
            *summary = None;
        }
    }

    /// Whether the cell at `row`, `column` is a check mark rather than text.
    #[must_use]
    pub const fn is_check_cell(&self, row: usize, column: usize) -> bool {
        self.check_column && column == 0 && row > 0
    }

    /// Whether body row `row` is ticked.
    #[must_use]
    pub fn is_checked(&self, row: usize) -> bool {
        self.check_column
            && row > 0
            && self
                .cell(row, 0)
                .is_some_and(|cell| check_mark(cell.text()) == Some(true))
    }

    pub fn toggle_check(&mut self, row: usize) -> bool {
        if !self.is_check_cell(row, 0) {
            return false;
        }
        let checked = self.is_checked(row);
        if let Some(cell) = self.cell_mut(row, 0) {
            *cell = Inline::from(if checked { UNCHECKED } else { CHECKED });
        }
        true
    }

    /// Adopt column `0` as a check column when every body cell reads as a
    /// mark and at least one is written out, as in a table typed by hand.
    pub fn detect_check_column(&mut self) {
        let body = self.rows.iter().skip(1);
        let marks: Option<Vec<bool>> = body
            .map(|row| row.first().and_then(|cell| check_mark(cell.text())))
            .collect();
        let written = self.rows.iter().skip(1).any(|row| {
            row.first()
                .is_some_and(|cell| !cell.text().trim().is_empty())
        });
        if let Some(marks) = marks
            && !marks.is_empty()
            && written
        {
            self.check_column = false;
            self.set_check_column(true);
        }
    }

    // Totals and computed columns.

    /// Total of every data column.
    #[must_use]
    pub fn summaries(&self) -> &[Option<Summary>] {
        &self.summaries
    }

    #[must_use]
    pub fn summary(&self, column: Column) -> Option<Summary> {
        match column {
            Column::Data(index) => self.summaries.get(index).copied().flatten(),
            Column::Computed(index) => self.computed.get(index).and_then(|c| c.summary),
        }
    }

    pub fn set_summary(&mut self, column: Column, summary: Option<Summary>) -> bool {
        let slot = match column {
            Column::Data(index) => self.summaries.get_mut(index),
            Column::Computed(index) => self.computed.get_mut(index).map(|c| &mut c.summary),
        };
        match slot {
            Some(slot) if *slot != summary => {
                *slot = summary;
                true
            }
            _ => false,
        }
    }

    #[must_use]
    pub fn computed(&self) -> &[ComputedColumn] {
        &self.computed
    }

    /// Add a computed column after the existing ones.
    pub fn add_computed(&mut self, header: impl Into<String>, formula: impl Into<String>) {
        self.computed.push(ComputedColumn {
            header: header.into(),
            formula: formula.into(),
            summary: None,
        });
    }

    pub fn set_computed(&mut self, index: usize, header: &str, formula: &str) -> bool {
        let Some(column) = self.computed.get_mut(index) else {
            return false;
        };
        let changed = column.header != header || column.formula != formula;
        header.clone_into(&mut column.header);
        formula.clone_into(&mut column.formula);
        changed
    }

    pub fn remove_computed(&mut self, index: usize) -> bool {
        if index >= self.computed.len() {
            return false;
        }
        self.computed.remove(index);
        true
    }

    /// Header text of a data column, trimmed.
    #[must_use]
    pub fn header(&self, column: usize) -> String {
        self.cell(0, column)
            .map(|cell| cell.plain_text().trim().to_owned())
            .unwrap_or_default()
    }

    /// Point every formula naming data column `column` at its new header.
    pub fn rename_in_formulas(&mut self, column: usize, from: &str, to: &str) {
        let from = from.trim();
        let to = to.trim();
        if from == to {
            return;
        }
        let old = ColumnRef::Header(from.to_owned());
        let new = if to.is_empty() {
            ColumnRef::Index(column + 1)
        } else {
            ColumnRef::Header(to.to_owned())
        };
        for computed in &mut self.computed {
            if let Ok(mut expr) = Expr::parse(&computed.formula)
                && expr.columns().contains(&&old)
            {
                expr.rename_column(&old, &new);
                computed.formula = expr.to_string();
            }
        }
    }

    /// Data column named by `reference`, matching headers case
    /// insensitively.
    fn data_column(&self, reference: &ColumnRef) -> Option<usize> {
        match reference {
            ColumnRef::Index(index) => {
                (*index >= 1 && *index <= self.columns.len()).then(|| index - 1)
            }
            ColumnRef::Header(name) => {
                let name = name.trim().to_lowercase();
                (0..self.columns.len()).find(|column| self.header(*column).to_lowercase() == name)
            }
        }
    }

    fn computed_column(&self, reference: &ColumnRef) -> Option<usize> {
        let ColumnRef::Header(name) = reference else {
            return None;
        };
        let name = name.trim().to_lowercase();
        self.computed
            .iter()
            .position(|column| column.header.trim().to_lowercase() == name)
    }

    /// The number in a typed body cell.
    #[must_use]
    pub fn cell_number(&self, row: usize, column: usize) -> Option<Number> {
        if self.is_check_cell(row, column) {
            return None;
        }
        self.cell(row, column)
            .and_then(|cell| Number::parse(&cell.plain_text()))
    }

    /// Whether any body cell of data column `column` holds a number.
    #[must_use]
    pub fn column_is_numeric(&self, column: usize) -> bool {
        (1..self.rows.len()).any(|row| self.cell_number(row, column).is_some())
    }

    /// Body rows `scope` looks at.
    fn rows_in(&self, scope: Scope) -> Vec<usize> {
        (1..self.rows.len())
            .filter(|row| match scope {
                Scope::All => true,
                Scope::Checked => self.is_checked(*row),
                Scope::Unchecked => !self.is_checked(*row),
            })
            .filter(|_| self.check_column || scope == Scope::All)
            .collect()
    }

    /// Computed columns and totals for the current cells.
    #[must_use]
    #[cfg_attr(feature = "sismo", sismo::function("markdown::table_evaluate"))]
    pub fn evaluate(&self) -> Evaluated {
        let body = self.rows.len().saturating_sub(1);
        let mut computed: Vec<Vec<ComputedCell>> = Vec::with_capacity(self.computed.len());
        for (index, column) in self.computed.iter().enumerate() {
            let expr = column.expr();
            let values: Vec<ComputedCell> = (0..body)
                .map(|body_row| {
                    let row = body_row + 1;
                    let expr = expr.as_ref()?;
                    let operands: Vec<(&ColumnRef, Number)> = expr
                        .columns()
                        .into_iter()
                        .map(|reference| {
                            let number = self.data_column(reference).map_or_else(
                                || {
                                    self.computed_column(reference)
                                        .filter(|other| *other < index)
                                        .and_then(|other| computed[other][body_row])
                                },
                                |data| self.cell_number(row, data),
                            );
                            number.map(|number| (reference, number))
                        })
                        .collect::<Option<Vec<_>>>()?;
                    let value = expr.eval(&|reference| {
                        operands
                            .iter()
                            .find(|(known, _)| *known == reference)
                            .map(|(_, number)| number.value)
                    })?;
                    Some(Number {
                        value,
                        decimals: operands.iter().map(|(_, n)| n.decimals).max().unwrap_or(0),
                        comma: operands.iter().any(|(_, n)| n.comma),
                    })
                })
                .collect();
            computed.push(values);
        }
        let mut totals = Vec::with_capacity(self.columns.len() + self.computed.len());
        for (column, summary) in self.summaries.iter().enumerate() {
            totals.push(summary.map(|summary| Total {
                summary,
                text: self.total_of(
                    summary,
                    |row| self.cell_number(row, column),
                    |row| self.cell_is_filled(row, column),
                ),
            }));
        }
        for (index, column) in self.computed.iter().enumerate() {
            totals.push(column.summary.map(|summary| Total {
                summary,
                text: self.total_of(
                    summary,
                    |row| computed[index][row - 1],
                    |row| computed[index][row - 1].is_some(),
                ),
            }));
        }
        Evaluated { computed, totals }
    }

    #[allow(clippy::cast_precision_loss, reason = "row counts are far below 2^52")]
    fn total_of(
        &self,
        summary: Summary,
        number: impl Fn(usize) -> Option<Number>,
        filled: impl Fn(usize) -> bool,
    ) -> String {
        let rows = self.rows_in(summary.scope);
        let numbers: Vec<Number> = rows.iter().filter_map(|row| number(*row)).collect();
        let decimals = numbers.iter().map(|n| n.decimals).max().unwrap_or(0);
        let comma = numbers.iter().any(|n| n.comma);
        let values: Vec<f64> = numbers.iter().map(|n| n.value).collect();
        let count = |n: usize| n.to_string();
        let percent = |part: usize| {
            if rows.is_empty() {
                "0 %".to_owned()
            } else {
                format!("{} %", (part as f64 / rows.len() as f64 * 100.).round())
            }
        };
        let fixed = |value: f64| Number::format(value, decimals, comma);
        let loose = |value: f64| {
            let text = Number::format(value, decimals.max(2), comma);
            let separator = if comma { ',' } else { '.' };
            if text.contains(separator) {
                text.trim_end_matches('0')
                    .trim_end_matches(separator)
                    .to_owned()
            } else {
                text
            }
        };
        match summary.op {
            Aggregate::CountAll => count(rows.len()),
            Aggregate::CountFilled => count(rows.iter().filter(|row| filled(**row)).count()),
            Aggregate::CountEmpty => count(rows.iter().filter(|row| !filled(**row)).count()),
            Aggregate::Sum => fixed(values.iter().sum()),
            Aggregate::Average | Aggregate::Median if values.is_empty() => String::new(),
            Aggregate::Average => loose(values.iter().sum::<f64>() / values.len() as f64),
            Aggregate::Median => {
                let mut sorted = values;
                sorted.sort_by(f64::total_cmp);
                let middle = sorted.len() / 2;
                loose(if sorted.len() % 2 == 0 {
                    f64::midpoint(sorted[middle - 1], sorted[middle])
                } else {
                    sorted[middle]
                })
            }
            Aggregate::Min => values
                .iter()
                .copied()
                .min_by(f64::total_cmp)
                .map(fixed)
                .unwrap_or_default(),
            Aggregate::Max => values
                .iter()
                .copied()
                .max_by(f64::total_cmp)
                .map(fixed)
                .unwrap_or_default(),
            Aggregate::Range => {
                let min = values.iter().copied().min_by(f64::total_cmp);
                let max = values.iter().copied().max_by(f64::total_cmp);
                min.zip(max)
                    .map(|(min, max)| fixed(max - min))
                    .unwrap_or_default()
            }
            Aggregate::Checked => count(rows.iter().filter(|row| self.is_checked(**row)).count()),
            Aggregate::Unchecked => {
                count(rows.iter().filter(|row| !self.is_checked(**row)).count())
            }
            Aggregate::PercentChecked => {
                percent(rows.iter().filter(|row| self.is_checked(**row)).count())
            }
        }
    }

    /// Whether a typed body cell counts as filled for the count totals.
    #[must_use]
    pub fn cell_is_filled(&self, row: usize, column: usize) -> bool {
        self.cell(row, column)
            .is_some_and(|cell| !cell.plain_text().trim().is_empty())
    }

    // The `notein:table` spec.

    /// Whether the table has anything beyond typed cells.
    #[must_use]
    pub fn has_spec(&self) -> bool {
        self.check_column || self.summaries.iter().any(Option::is_some) || !self.computed.is_empty()
    }

    /// How a data column is named in the spec: its header unless that is
    /// empty, repeated or ambiguous, then `#n`.
    fn spec_ref(&self, column: usize) -> String {
        let header = self.header(column);
        let unique = (0..self.columns.len())
            .filter(|other| self.header(*other).eq_ignore_ascii_case(&header))
            .count()
            == 1;
        let taken_by_computed = self
            .computed
            .iter()
            .any(|computed| computed.header.trim().eq_ignore_ascii_case(&header));
        if header.is_empty() || !unique || taken_by_computed || header.contains('=') {
            format!("#{}", column + 1)
        } else {
            header
        }
    }

    /// Lines of the `notein:table` comment, without the tag.
    #[must_use]
    pub fn spec_lines(&self) -> Vec<String> {
        let mut lines = Vec::new();
        if self.check_column {
            lines.push("check: 1".to_owned());
        }
        for computed in &self.computed {
            lines.push(format!(
                "column: {} = {}",
                computed.header, computed.formula
            ));
        }
        let calc = |name: String, summary: Summary| {
            let scope = match summary.scope {
                Scope::All => String::new(),
                scope => format!(", {}", scope.key()),
            };
            format!("calc: {name} = {}{scope}", summary.op.key())
        };
        for (column, summary) in self.summaries.iter().enumerate() {
            if let Some(summary) = summary {
                lines.push(calc(self.spec_ref(column), *summary));
            }
        }
        for computed in &self.computed {
            if let Some(summary) = computed.summary {
                lines.push(calc(computed.header.clone(), summary));
            }
        }
        lines
    }

    /// Apply the lines of a `notein:table` comment to a table read from
    /// markdown, dropping the cells that were written for the computed
    /// columns and the totals since they are derived again.
    #[cfg_attr(feature = "sismo", sismo::function("markdown::table_apply_spec"))]
    pub fn apply_spec(&mut self, lines: &str) {
        let mut calcs: Vec<(String, Summary)> = Vec::new();
        let mut check = None;
        for line in lines.lines() {
            let Some((key, value)) = line.split_once(':') else {
                continue;
            };
            let value = value.trim();
            match key.trim() {
                "check" => check = Some(matches!(value, "1" | "true" | "yes")),
                "column" => {
                    let Some((header, formula)) = value.split_once('=') else {
                        continue;
                    };
                    let header = header.trim();
                    if let Some(column) = (0..self.columns.len())
                        .rev()
                        .find(|column| self.header(*column).eq_ignore_ascii_case(header))
                        && column > 0
                    {
                        self.remove_column(column);
                    }
                    self.add_computed(header, formula.trim());
                }
                "calc" => {
                    let Some((name, op)) = value.split_once('=') else {
                        continue;
                    };
                    let (op, scope) = op.split_once(',').unwrap_or((op, ""));
                    let Some(op) = Aggregate::from_key(op.trim()) else {
                        continue;
                    };
                    let scope = Scope::from_key(scope.trim()).unwrap_or_default();
                    calcs.push((name.trim().to_owned(), Summary { op, scope }));
                }
                _ => {}
            }
        }
        let reference = |name: &str| {
            name.strip_prefix('#')
                .and_then(|index| index.parse().ok())
                .map_or_else(|| ColumnRef::Header(name.to_owned()), ColumnRef::Index)
        };
        // Totals were written as the last row; it is one when the cells
        // under columns without a total are empty (or check marks).
        let summarised = |column: usize| {
            calcs
                .iter()
                .any(|(name, _)| self.data_column(&reference(name)) == Some(column))
        };
        let looks_like_totals = !calcs.is_empty()
            && self.rows.len() > 1
            && self.rows.last().is_some_and(|row| {
                row.iter().enumerate().all(|(column, cell)| {
                    summarised(column)
                        || cell.text().trim().is_empty()
                        || (column == 0 && check_mark(cell.text()).is_some())
                })
            });
        if looks_like_totals {
            self.rows.pop();
        }
        if let Some(check) = check {
            self.set_check_column(check);
        }
        for (name, summary) in calcs {
            if let Some(column) = self.data_column(&reference(&name)) {
                self.summaries[column] = Some(summary);
            } else if let Some(column) = self.computed_column(&reference(&name)) {
                self.computed[column].summary = Some(summary);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shopping() -> Table {
        let mut table = Table::from_rows(
            vec![ColumnAlign::None; 4],
            vec![
                vec![
                    "".into(),
                    "Artículo".into(),
                    "Presupuesto".into(),
                    "Costo".into(),
                ],
                vec!["[x]".into(), "Pan".into(), "10".into(), "12,50".into()],
                vec!["[ ]".into(), "Leche".into(), "5".into(), "".into()],
                vec!["[x]".into(), "Queso".into(), "20".into(), "18".into()],
            ],
        );
        table.detect_check_column();
        table.add_computed("Diferencia", "[Costo] - [Presupuesto]");
        table.set_summary(Column::Data(2), Some(Aggregate::Sum.into()));
        table.set_summary(
            Column::Data(3),
            Some(Summary {
                op: Aggregate::Sum,
                scope: Scope::Checked,
            }),
        );
        table.set_summary(Column::Computed(0), Some(Aggregate::Sum.into()));
        table.set_summary(Column::Data(0), Some(Aggregate::PercentChecked.into()));
        table
    }

    #[test]
    fn from_rows_pads_shape() {
        let table = Table::from_rows(
            vec![ColumnAlign::Left],
            vec![vec!["a".into(), "b".into()], vec!["c".into()]],
        );
        assert_eq!(table.columns().len(), 2);
        assert!(table.rows().iter().all(|row| row.len() == 2));
    }

    #[test]
    fn header_row_is_permanent() {
        let mut table = Table::new(2, 1);
        assert!(!table.remove_row(0));
        table.insert_row(1);
        assert!(table.remove_row(1));
    }

    #[test]
    fn check_column_is_detected_and_toggled() {
        let mut table = shopping();
        assert!(table.has_check_column());
        assert!(table.is_checked(1));
        assert!(!table.is_checked(2));
        assert!(table.toggle_check(2));
        assert!(table.is_checked(2));
        assert!(!table.toggle_check(0));
        table.insert_row(2);
        assert_eq!(table.cell(2, 0).map(Inline::text), Some("[ ]"));
        table.set_check_column(false);
        assert!(table.cell(1, 0).is_some_and(Inline::is_empty));
        assert!(table.summary(Column::Data(0)).is_none());
    }

    #[test]
    fn evaluation_computes_columns_and_totals() {
        let table = shopping();
        let evaluated = table.evaluate();
        assert_eq!(evaluated.computed_text(0, 0).as_deref(), Some("2,50"));
        assert_eq!(evaluated.computed_text(0, 1), None);
        assert_eq!(evaluated.computed_text(0, 2).as_deref(), Some("-2"));
        let totals: Vec<Option<&str>> = evaluated
            .totals
            .iter()
            .map(|total| total.as_ref().map(|total| total.text.as_str()))
            .collect();
        assert_eq!(
            totals,
            vec![Some("67 %"), None, Some("35"), Some("30,50"), Some("0,50")]
        );
    }

    #[test]
    fn averages_drop_trailing_zeros() {
        let mut table = Table::from_rows(
            vec![ColumnAlign::None],
            vec![
                vec!["n".into()],
                vec!["1".into()],
                vec!["2".into()],
                vec!["4".into()],
            ],
        );
        table.set_summary(Column::Data(0), Some(Aggregate::Average.into()));
        assert_eq!(
            table.evaluate().totals[0].as_ref().map(|t| t.text.as_str()),
            Some("2.33")
        );
        table.set_summary(Column::Data(0), Some(Aggregate::Median.into()));
        assert_eq!(
            table.evaluate().totals[0].as_ref().map(|t| t.text.as_str()),
            Some("2")
        );
        table.set_summary(Column::Data(0), Some(Aggregate::Range.into()));
        assert_eq!(
            table.evaluate().totals[0].as_ref().map(|t| t.text.as_str()),
            Some("3")
        );
    }

    #[test]
    fn spec_round_trips_through_lines() {
        let table = shopping();
        let lines = table.spec_lines();
        assert_eq!(
            lines,
            vec![
                "check: 1",
                "column: Diferencia = [Costo] - [Presupuesto]",
                "calc: #1 = percent-checked",
                "calc: Presupuesto = sum",
                "calc: Costo = sum, checked",
                "calc: Diferencia = sum",
            ]
        );
        // What the serializer writes: the computed column and a totals row.
        let mut written = Table::from_rows(
            vec![ColumnAlign::None; 5],
            vec![
                vec![
                    "".into(),
                    "Artículo".into(),
                    "Presupuesto".into(),
                    "Costo".into(),
                    "Diferencia".into(),
                ],
                vec![
                    "[x]".into(),
                    "Pan".into(),
                    "10".into(),
                    "12,50".into(),
                    "2,50".into(),
                ],
                vec![
                    "[ ]".into(),
                    "Leche".into(),
                    "5".into(),
                    "".into(),
                    "".into(),
                ],
                vec![
                    "[x]".into(),
                    "Queso".into(),
                    "20".into(),
                    "18".into(),
                    "-2".into(),
                ],
                vec![
                    "67 %".into(),
                    "".into(),
                    "35".into(),
                    "30,50".into(),
                    "0,50".into(),
                ],
            ],
        );
        written.apply_spec(&lines.join("\n"));
        assert_eq!(written, table);
    }

    #[test]
    fn renaming_a_header_follows_in_formulas() {
        let mut table = shopping();
        table.rename_in_formulas(3, "Costo", "Gasto");
        assert_eq!(table.computed()[0].formula, "[Gasto] - [Presupuesto]");
        table.rename_in_formulas(3, "Gasto", "");
        assert_eq!(table.computed()[0].formula, "#4 - [Presupuesto]");
    }
}
