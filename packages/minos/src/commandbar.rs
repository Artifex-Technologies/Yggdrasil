//! The command bar above the list (the third row of the window's top, as in the Files app): which commands apply to what is
//! selected and where, what their buttons are called, and their keyboard shortcuts (shown in each button's tooltip).

/// A command on the bar's left side.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Command {
    NewFolder,
    NewFile,
    Cut,
    Copy,
    Paste,
    Rename,
    Share,
    Delete,
    Properties,
}

/// What the commands depend on.
#[derive(Clone, Copy, Debug, Default)]
pub struct Facts {
    /// How many items are selected.
    pub selected: usize,
    /// Minos holds files to paste (its own copy or cut, or another program's files taken from the clipboard).
    pub clipboard: bool,
    /// The folder is one Minos makes up (This PC, a disk's partitions): nothing in it is a file.
    pub virtual_folder: bool,
    /// The folder is inside an archive (read-only here).
    pub in_archive: bool,
}

/// Why Share is greyed: no program on the system shares files (the share picker that exists asks which screen to share).
pub const SHARE_UNAVAILABLE: &str = "Share: no file sharing service is installed";

/// Whether a command can be used now.
pub fn enabled(c: Command, f: &Facts) -> bool {
    let files = !f.virtual_folder;
    let writable = files && !f.in_archive;
    match c {
        Command::NewFolder | Command::NewFile => writable,
        // cut, rename and delete change the folder; copying out of an archive extracts, so Copy works there too
        Command::Cut | Command::Delete => writable && f.selected > 0,
        Command::Copy => files && f.selected > 0,
        Command::Paste => writable && f.clipboard,
        Command::Rename => writable && f.selected == 1,
        Command::Share => false,
        // nothing selected: the folder itself
        Command::Properties => writable,
    }
}

/// The button's tooltip: its name and, when it has one, its keyboard shortcut.
pub fn tooltip(c: Command) -> &'static str {
    match c {
        Command::NewFolder => "New folder (Ctrl+Shift+N)",
        Command::NewFile => "New file",
        Command::Cut => "Cut (Ctrl+X)",
        Command::Copy => "Copy (Ctrl+C)",
        Command::Paste => "Paste (Ctrl+V)",
        Command::Rename => "Rename (F2)",
        Command::Share => SHARE_UNAVAILABLE,
        Command::Delete => "Move to trash (Delete)",
        Command::Properties => "Properties (Alt+Enter)",
    }
}

/// The left side of the bar, in order (New is a menu of the first two).
pub const LEFT: [Command; 7] =
    [Command::Cut, Command::Copy, Command::Paste, Command::Rename, Command::Share, Command::Delete, Command::Properties];

/// The quick filter: whether a name has `filter` (given in lower case) in it, in any case. An empty filter lets everything through.
pub fn filter_matches(name: &str, filter: &str) -> bool {
    filter.is_empty() || name.to_lowercase().contains(filter)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn facts(selected: usize, clipboard: bool) -> Facts {
        Facts { selected, clipboard, ..Default::default() }
    }

    #[test]
    fn paste_needs_something_to_paste_and_rename_exactly_one() {
        assert!(!enabled(Command::Paste, &facts(0, false)));
        assert!(enabled(Command::Paste, &facts(0, true)));
        assert!(!enabled(Command::Rename, &facts(0, false)));
        assert!(enabled(Command::Rename, &facts(1, false)));
        assert!(!enabled(Command::Rename, &facts(2, false)));
        for c in [Command::Cut, Command::Copy, Command::Delete] {
            assert!(!enabled(c, &facts(0, true)), "{c:?} with nothing selected");
            assert!(enabled(c, &facts(3, false)), "{c:?} with several selected");
        }
        assert!(enabled(Command::NewFolder, &facts(0, false)) && enabled(Command::Properties, &facts(0, false)));
    }

    #[test]
    fn this_pc_and_archives_allow_less() {
        let pc = Facts { selected: 1, clipboard: true, virtual_folder: true, in_archive: false };
        for c in [Command::NewFolder, Command::NewFile, Command::Cut, Command::Copy, Command::Paste, Command::Rename, Command::Delete] {
            assert!(!enabled(c, &pc), "{c:?} in This PC");
        }
        let arc = Facts { selected: 1, clipboard: true, virtual_folder: false, in_archive: true };
        assert!(enabled(Command::Copy, &arc), "copying out of an archive extracts");
        for c in [Command::Cut, Command::Paste, Command::Rename, Command::Delete, Command::NewFolder, Command::Properties] {
            assert!(!enabled(c, &arc), "{c:?} in an archive");
        }
    }

    #[test]
    fn share_is_greyed_with_the_reason_and_every_tooltip_names_its_shortcut() {
        assert!(!enabled(Command::Share, &facts(1, true)));
        assert_eq!(tooltip(Command::Share), SHARE_UNAVAILABLE);
        for (c, key) in [
            (Command::Cut, "Ctrl+X"),
            (Command::Copy, "Ctrl+C"),
            (Command::Paste, "Ctrl+V"),
            (Command::Rename, "F2"),
            (Command::Delete, "Delete"),
            (Command::Properties, "Alt+Enter"),
            (Command::NewFolder, "Ctrl+Shift+N"),
        ] {
            assert!(tooltip(c).contains(key), "{c:?}");
        }
    }

    #[test]
    fn the_filter_matches_any_case_anywhere_in_the_name() {
        assert!(filter_matches("Holiday Photos", ""));
        assert!(filter_matches("Holiday Photos", "photo"));
        assert!(filter_matches("REPORT.PDF", "port.p"), "the name in any case");
        assert!(!filter_matches("notes.txt", "pdf"));
    }
}
