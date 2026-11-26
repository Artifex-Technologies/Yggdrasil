# Minos keyboard shortcuts

Not tried by hand on a keyboard yet; the build and unit tests pass.

**Tabs:** Ctrl+T new, Ctrl+W close, Ctrl+Shift+T reopen the last closed tab (20 are remembered), Ctrl+Tab / Ctrl+Shift+Tab next / previous (wraps around), Ctrl+1..8 that tab, Ctrl+9 the last, Ctrl+Shift+W close the window.

**With the mouse:** drag a tab along the tab strip to move it; drag it out of the window (let go outside it) and it opens in a new Minos window of its own (`minos --window FOLDER`; that window keeps no tabs for next time, so the main window's stay). Favorites in the sidebar are dragged up or down to a new place (a line shows where); right-click still has Move up / Move down / Remove. The well-known folders (Desktop, Documents, Downloads, Music, Pictures, Videos, Home, and a "3D Objects" folder in home) have their own pictures in Favorites (icons.rs `paint_special`); a folder given a colour keeps the coloured folder.

**Selecting several:** a click selects one, Ctrl+click adds or removes one, Shift+click takes everything from the last click.
A drag from empty space (between or below the files, in both views) draws a box and selects what it touches as it moves;
with Ctrl held it adds to what was selected. With a finger: tap twice and drag on the second tap to draw the box (one finger
dragging scrolls, as before). A drag that starts on a file still moves or copies it.

**Moving:** arrows, Home, End, PageUp, PageDown move the selection (Shift extends it). In the details list Left goes to the parent folder and Right enters the selected folder; in the gallery the arrows move across the grid. Enter opens (a file opens in its default app; on Labyrinth OS through its app launcher, so the app's own rules, such as a viewer's network on "ask", apply as they do from the dock). Alt+Left / Alt+Right back and forward, Alt+Up or Backspace up. Tab switches between the two panes.

**Finding:** just start typing a name to jump to it (the same letter again cycles); Ctrl+F searches, Ctrl+L edits the address.

**Files:** Ctrl+C / X / V, Ctrl+A, F2 rename, Delete trash, Ctrl+Shift+N new folder, Alt+Enter properties, F3 preview, F5 reload, F11 full screen.

**The top of the window** (laid out like the Files app's): the tabs in the title strip (+ for a new tab); under them the
navigation row: back, forward, up, refresh, the address bar (click it to type a path: Enter goes there, Esc cancels), the
favorite star, the sidebar button (folds the sidebar away; also Menu > Sidebar) and search. Over the list, the command bar: New
(a menu: Folder, File), Cut, Copy, Paste, Rename, Share, Delete and Properties on the left; Filter (only names with the letters
typed are shown, until you leave the folder; Esc clears it), Sort (name, date modified, size; up or down), Group (by type, date
modified or size, as Windows groups them: Today, Yesterday, Earlier this week ...; or none), the layout (details or gallery) and
the preview pane on the right. A button that does not apply is greyed (Paste with nothing to paste, Rename unless exactly one
item is selected, everything that changes files in This PC or inside an archive); every button's tooltip names its shortcut.
Share is greyed: the system has no file sharing service. In split view each pane has its own tabs, navigation row and command
bar, and a narrow pane puts Sort, Group, the layout and the preview pane in one menu. Code: `src/topbar.rs`, the rules in
`src/commandbar.rs`, Group by in `src/grouping.rs`.

**Properties** (Alt+Enter, or the wrench): name, type, size, where it is, when it was made and changed, its permissions in
letters and words, and its owner; with nothing selected, the folder's. A folder's size (and several items') is counted on a
thread and fills in as it goes; on a network or cloud drive it is not counted (it would fetch every file). Code: `src/props.rs`.

**Cloud Storage** (the sidebar): Proton Drive as before, and every cloud drive added in Settings > Apps > Minos > Cloud drives
(packages/clouddrives). A mounted drive, or a sync app's folder that is there, opens on a click; a drive that is not mounted is
greyed and mounts on a click or with right-click > Mount (and opens once it is up); right-click > Unmount, and Manage cloud
drives opens the Settings page. A green dot is a mounted drive, a red one an error (the tooltip says which). The list follows
Settings without a restart; whether a drive is mounted is read from the mount table every few seconds on a thread. Code:
`src/cloudside.rs`.

**Where it opens:** Settings > Apps > Minos sets the first window (Home or where you left off), a window opened while another is open (Home or a copy of that window's folder) and a new tab (Home or a copy of the current tab). Open windows list their folder in $XDG_RUNTIME_DIR/minos/.

**Right-click > Scrub location sounds** (on one file with sound in it: an audio file, or a video with an audio track): Heimdall's
`heimdall-scrub` writes a copy beside it, "name (scrubbed).ext" ("name (scrubbed 2).ext" when that is taken; the original is never
changed or replaced), without the sounds that can place or date the recording (aircraft overhead, mains hum) and without the
file's hidden information (dates, places, the device). The picture of a video is copied untouched. A small window shows how far it
is, with Stop; then Done (Show opens the folder with the copy selected) or Failed with the reason, and the status line says the
same. A stopped or failed scrub leaves no copy behind. Whether a file has sound is read from its content, not its name: its first
bytes, and for a video ffprobe (3 seconds at most), only once the menu is opened on it; while that runs the entry is greyed. The
entry is there only when heimdall-scrub is installed, for one file at a time, and not inside an archive or for a cloud file not
downloaded yet. Code: `src/privacy.rs` (the menu's Privacy entries; the place for more of them) and `start_scrub` / `poll_scrubs`
in `src/main.rs`.

**Right-click > Remove hidden info** (2026-11-15; on one file Heimdall's `heimdall-strip` can clean: photos, videos, music, PDFs,
Office and LibreOffice documents, ZIP files): it writes a copy beside it, "name (clean).ext" ("name (clean 2).ext" when that is
taken; never over a file, the original never changed), without what is hidden in it: the place a photo or video was taken (GPS),
the camera or phone, the author, the company, who changed it last, the template's path, comments and dates. The window lists what
was removed, with Show; when nothing was hidden it says so and no copy is made; a stopped or failed one leaves no copy. Whether
a file can be cleaned is asked of `heimdall-strip --can FILE` (it reads the first bytes) once the menu is opened on it; while that
runs the entry is greyed. The entry is there when heimdall-strip is installed and Settings > Security > Privacy > "Remove hidden
info in Minos" is on (the default; `remove_hidden_info` in `~/.config/heimdall/privacy.json`). Minos runs `heimdall-strip --json
IN OUT` (argv, never a shell) and reads its one JSON line. Code: `src/privacy.rs` and `start_strip` / `poll_strips` in
`src/main.rs`.
