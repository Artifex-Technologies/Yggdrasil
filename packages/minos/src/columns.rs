//! The details list's columns: which are shown (the layout menu's Columns, and a right-click on the list's header), and how
//! wide each is in the room a pane has. Name is always shown; the others are chosen and keep their order, at any width (the
//! preview pane open or not, split view or not).

use serde::{Deserialize, Serialize};

/// A column after Name, in the list's order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ListColumn {
    /// Proton Drive's marks (on this computer, or in the cloud only), in Proton Drive folders while Settings shows them
    Status,
    Modified,
    Type,
    Size,
}

impl ListColumn {
    pub const ALL: [ListColumn; 4] = [ListColumn::Status, ListColumn::Modified, ListColumn::Type, ListColumn::Size];

    pub fn label(self) -> &'static str {
        match self {
            ListColumn::Status => "Status",
            ListColumn::Modified => "Date modified",
            ListColumn::Type => "Type",
            ListColumn::Size => "Size",
        }
    }
}

/// Which columns after Name are shown. Every one is, until it is switched off.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct Columns {
    pub status: bool,
    pub modified: bool,
    #[serde(rename = "type")]
    pub kind: bool,
    pub size: bool,
}

impl Default for Columns {
    fn default() -> Self {
        Self { status: true, modified: true, kind: true, size: true }
    }
}

impl Columns {
    pub fn shows(&self, c: ListColumn) -> bool {
        match c {
            ListColumn::Status => self.status,
            ListColumn::Modified => self.modified,
            ListColumn::Type => self.kind,
            ListColumn::Size => self.size,
        }
    }

    pub fn toggle(&mut self, c: ListColumn) {
        let s = match c {
            ListColumn::Status => &mut self.status,
            ListColumn::Modified => &mut self.modified,
            ListColumn::Type => &mut self.kind,
            ListColumn::Size => &mut self.size,
        };
        *s = !*s;
    }

    /// The shown columns in order; Status only where `status_here` (a Proton Drive folder, with Settings' marks on).
    pub fn shown(&self, status_here: bool) -> Vec<ListColumn> {
        ListColumn::ALL.into_iter().filter(|c| self.shows(*c) && (*c != ListColumn::Status || status_here)).collect()
    }
}

/// The narrowest Name gets while the other columns can still give room, and the narrowest it ever gets.
pub const NAME_MIN: f32 = 140.0;
pub const NAME_FLOOR: f32 = 60.0;

/// The widths of the columns after Name in `avail` points, `spacing` between columns; each column is (the width its widest
/// text wants, its narrowest). Each gets what it wants when Name keeps at least NAME_MIN; otherwise all give up the same
/// share of what they can (down to their narrowest), and Name goes under NAME_MIN only once they are all at their narrowest.
/// Name takes the rest: `name_floor` says how narrow it may then be.
pub fn fit(avail: f32, spacing: f32, cols: &[(f32, f32)]) -> Vec<f32> {
    let room = avail - spacing * cols.len() as f32 - NAME_MIN;
    let want: f32 = cols.iter().map(|c| c.0).sum();
    if want <= room {
        return cols.iter().map(|c| c.0).collect();
    }
    let mins: Vec<f32> = cols.iter().map(|c| c.1.min(c.0)).collect();
    let min: f32 = mins.iter().sum();
    if room <= min {
        return mins;
    }
    let share = (room - min) / (want - min);
    cols.iter().zip(&mins).map(|(c, m)| m + (c.0 - m) * share).collect()
}

/// How narrow Name may be beside columns of these `widths`: NAME_MIN, less where they leave less, never under NAME_FLOOR.
pub fn name_floor(avail: f32, spacing: f32, widths: &[f32]) -> f32 {
    (avail - spacing * widths.len() as f32 - widths.iter().sum::<f32>()).clamp(NAME_FLOOR, NAME_MIN)
}

#[cfg(test)]
mod tests {
    use super::*;

    const COLS: [(f32, f32); 3] = [(130.0, 56.0), (100.0, 56.0), (70.0, 56.0)];

    #[test]
    fn every_chosen_column_is_shown_in_its_order() {
        let mut c = Columns::default();
        assert_eq!(c.shown(true), ListColumn::ALL);
        assert_eq!(c.shown(false), [ListColumn::Modified, ListColumn::Type, ListColumn::Size], "Status only in Proton Drive");
        c.toggle(ListColumn::Type);
        assert!(!c.shows(ListColumn::Type));
        assert_eq!(c.shown(false), [ListColumn::Modified, ListColumn::Size]);
        c.toggle(ListColumn::Type);
        assert_eq!(c, Columns::default());
    }

    #[test]
    fn a_wide_pane_gives_each_column_its_text() {
        let w = fit(900.0, 8.0, &COLS);
        assert_eq!(w, [130.0, 100.0, 70.0]);
        assert_eq!(name_floor(900.0, 8.0, &w), NAME_MIN);
    }

    #[test]
    fn a_narrow_pane_shrinks_every_column_alike_and_keeps_them_all() {
        // the preview pane open: 400 points for the list
        let w = fit(400.0, 8.0, &COLS);
        assert_eq!(w.len(), 3);
        let room = 400.0 - 24.0 - NAME_MIN;
        assert!((w.iter().sum::<f32>() - room).abs() < 0.01, "{w:?} fill exactly the room Name leaves");
        // each gave up the same share of what it could give
        let gave: Vec<f32> = w.iter().zip(COLS).map(|(w, c)| (c.0 - w) / (c.0 - c.1)).collect();
        assert!(gave.windows(2).all(|g| (g[0] - g[1]).abs() < 0.001), "{gave:?}");
        assert!(w.iter().zip(COLS).all(|(w, c)| *w >= c.1 && *w <= c.0));
        assert_eq!(name_floor(400.0, 8.0, &w), NAME_MIN);
    }

    #[test]
    fn a_very_narrow_pane_keeps_the_columns_at_their_narrowest_and_narrows_name() {
        let w = fit(300.0, 8.0, &COLS);
        assert_eq!(w, [56.0, 56.0, 56.0]);
        assert_eq!(name_floor(300.0, 8.0, &w), 300.0 - 24.0 - 168.0, "Name has what the columns leave");
        assert_eq!(name_floor(200.0, 8.0, &w), NAME_FLOOR, "and never less than its floor");
    }

    #[test]
    fn a_column_narrower_than_its_minimum_keeps_its_own_width() {
        assert_eq!(fit(200.0, 8.0, &[(40.0, 56.0)]), [40.0]);
    }

    #[test]
    fn saved_choices_read_back_and_an_old_file_shows_every_column() {
        let mut c = Columns::default();
        c.toggle(ListColumn::Modified);
        let text = serde_json::to_string(&c).unwrap();
        assert!(text.contains("\"type\":true"), "{text}");
        assert_eq!(serde_json::from_str::<Columns>(&text).unwrap(), c);
        assert_eq!(serde_json::from_str::<Columns>("{}").unwrap(), Columns::default());
    }
}
