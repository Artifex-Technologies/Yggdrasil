//! The details list's columns: which are shown (the layout menu's Columns, and a right-click on the list's header), and how
//! wide each is in the room a pane has. Name is always shown, and Status wherever it means something (Proton Drive folders);
//! the others are chosen and keep their order, at any width (the preview pane open or not, split view or not).

use serde::{Deserialize, Serialize};

/// A column after Name, in the list's order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ListColumn {
    /// Proton Drive's marks (on this computer, or in the cloud only): not chosen, there in Proton Drive folders while
    /// Settings shows the marks
    Status,
    Modified,
    Created,
    Type,
    Size,
}

impl ListColumn {
    /// The columns the menus offer, in the list's order.
    pub const CHOOSABLE: [ListColumn; 4] = [ListColumn::Modified, ListColumn::Created, ListColumn::Type, ListColumn::Size];

    pub fn label(self) -> &'static str {
        match self {
            ListColumn::Status => "Status",
            ListColumn::Modified => "Date modified",
            ListColumn::Created => "Date created",
            ListColumn::Type => "Type",
            ListColumn::Size => "Size",
        }
    }
}

/// Which of the chosen columns are shown: all but Date created, until changed (as Windows starts).
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct Columns {
    pub modified: bool,
    pub created: bool,
    #[serde(rename = "type")]
    pub kind: bool,
    pub size: bool,
}

impl Default for Columns {
    fn default() -> Self {
        Self { modified: true, created: false, kind: true, size: true }
    }
}

impl Columns {
    /// Whether a chosen column is shown (Status is not chosen: false).
    pub fn shows(&self, c: ListColumn) -> bool {
        match c {
            ListColumn::Status => false,
            ListColumn::Modified => self.modified,
            ListColumn::Created => self.created,
            ListColumn::Type => self.kind,
            ListColumn::Size => self.size,
        }
    }

    /// Switches a chosen column (Status is not one: nothing).
    pub fn toggle(&mut self, c: ListColumn) {
        let s = match c {
            ListColumn::Status => return,
            ListColumn::Modified => &mut self.modified,
            ListColumn::Created => &mut self.created,
            ListColumn::Type => &mut self.kind,
            ListColumn::Size => &mut self.size,
        };
        *s = !*s;
    }

    /// The columns shown, in order: Status first where `status_here` (a Proton Drive folder, with Settings' marks on), then
    /// the chosen ones.
    pub fn shown(&self, status_here: bool) -> Vec<ListColumn> {
        status_here.then_some(ListColumn::Status).into_iter().chain(ListColumn::CHOOSABLE.into_iter().filter(|c| self.shows(*c))).collect()
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
        use ListColumn::*;
        let mut c = Columns::default();
        assert_eq!(c.shown(false), [Modified, Type, Size], "Date created only when chosen");
        assert_eq!(c.shown(true), [Status, Modified, Type, Size], "Status in Proton Drive folders, not chosen");
        c.toggle(Created);
        assert_eq!(c.shown(false), [Modified, Created, Type, Size]);
        c.toggle(Type);
        assert!(!c.shows(Type));
        assert_eq!(c.shown(false), [Modified, Created, Size]);
        c.toggle(Status);
        assert_eq!(c.shown(true), [Status, Modified, Created, Size], "Status cannot be switched");
        c.toggle(Type);
        c.toggle(Created);
        assert_eq!(c, Columns::default());
        assert!(!ListColumn::CHOOSABLE.contains(&Status));
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
        // a file saved while Status was a choice
        let old = r#"{"status":false,"modified":true,"type":false,"size":true}"#;
        assert_eq!(serde_json::from_str::<Columns>(old).unwrap(), Columns { kind: false, ..Columns::default() });
    }
}
