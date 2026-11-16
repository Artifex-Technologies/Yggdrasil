# Minos keyboard shortcuts

Not tried by hand on a keyboard yet; the build and unit tests pass.

**Tabs:** Ctrl+T new, Ctrl+W close, Ctrl+Shift+T reopen the last closed tab (20 are remembered), Ctrl+Tab / Ctrl+Shift+Tab next / previous (wraps around), Ctrl+1..8 that tab, Ctrl+9 the last, Ctrl+Shift+W close the window.

**With the mouse:** drag a tab along the tab strip to move it; drag it out of the window (let go outside it) and it opens in a new Minos window of its own (`minos --window FOLDER`; that window keeps no tabs for next time, so the main window's stay). Favorites in the sidebar are dragged up or down to a new place (a line shows where); right-click still has Move up / Move down / Remove. The well-known folders (Desktop, Documents, Downloads, Music, Pictures, Videos, Home, and a "3D Objects" folder in home) have their own pictures in Favorites (icons.rs `paint_special`); a folder given a colour keeps the coloured folder.

**Moving:** arrows, Home, End, PageUp, PageDown move the selection (Shift extends it). In the details list Left goes to the parent folder and Right enters the selected folder; in the gallery the arrows move across the grid. Enter opens. Alt+Left / Alt+Right back and forward, Alt+Up or Backspace up. Tab switches between the two panes.

**Finding:** just start typing a name to jump to it (the same letter again cycles); Ctrl+F searches, Ctrl+L edits the address.

**Files:** Ctrl+C / X / V, Ctrl+A, F2 rename, Delete trash, Ctrl+Shift+N new folder, F3 preview, F5 reload, F11 full screen.

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
