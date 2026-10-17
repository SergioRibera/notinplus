use super::Inline;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum ColumnAlign {
    #[default]
    None,
    Left,
    Center,
    Right,
}

/// GFM table. Row `0` is the header. Every row holds exactly one cell per
/// column.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Table {
    columns: Vec<ColumnAlign>,
    rows: Vec<Vec<Inline>>,
}

impl Default for Table {
    fn default() -> Self {
        Self::new(2, 2)
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
        }
    }

    /// Build from parsed rows, padding short rows so the shape stays square.
    #[must_use]
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
        Self { columns, rows }
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

    pub fn insert_row(&mut self, at: usize) {
        let at = at.clamp(1, self.rows.len());
        self.rows
            .insert(at, vec![Inline::default(); self.columns.len()]);
    }

    pub fn insert_column(&mut self, at: usize) {
        let at = at.min(self.columns.len());
        self.columns.insert(at, ColumnAlign::None);
        for row in &mut self.rows {
            row.insert(at, Inline::default());
        }
    }

    /// Remove a body row. The header row is kept.
    pub fn remove_row(&mut self, row: usize) -> bool {
        if row == 0 || row >= self.rows.len() {
            return false;
        }
        self.rows.remove(row);
        true
    }

    pub fn remove_column(&mut self, column: usize) -> bool {
        if self.columns.len() <= 1 || column >= self.columns.len() {
            return false;
        }
        self.columns.remove(column);
        for row in &mut self.rows {
            row.remove(column);
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
