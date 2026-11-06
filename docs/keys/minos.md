# Minos keyboard shortcuts

Not tried by hand on a keyboard yet; the build and unit tests pass.

**Tabs:** Ctrl+T new, Ctrl+W close, Ctrl+Shift+T reopen the last closed tab (20 are remembered), Ctrl+Tab / Ctrl+Shift+Tab next / previous (wraps around), Ctrl+1..8 that tab, Ctrl+9 the last, Ctrl+Shift+W close the window.

**With the mouse:** drag a tab along the tab strip to move it; drag it out of the window (let go outside it) and it opens in a new Minos window of its own (`minos --window FOLDER`; that window keeps no tabs for next time, so the main window's stay). Favorites in the sidebar are dragged up or down to a new place (a line shows where); right-click still has Move up / Move down / Remove. The well-known folders (Desktop, Documents, Downloads, Music, Pictures, Videos, Home, and a "3D Objects" folder in home) have their own pictures in Favorites (icons.rs `paint_special`); a folder given a colour keeps the coloured folder.

**Moving:** arrows, Home, End, PageUp, PageDown move the selection (Shift extends it). In the details list Left goes to the parent folder and Right enters the selected folder; in the gallery the arrows move across the grid. Enter opens. Alt+Left / Alt+Right back and forward, Alt+Up or Backspace up. Tab switches between the two panes.

**Finding:** just start typing a name to jump to it (the same letter again cycles); Ctrl+F searches, Ctrl+L edits the address.

**Files:** Ctrl+C / X / V, Ctrl+A, F2 rename, Delete trash, Ctrl+Shift+N new folder, F3 preview, F5 reload, F11 full screen.

**Where it opens:** Settings > Apps > Minos sets the first window (Home or where you left off), a window opened while another is open (Home or a copy of that window's folder) and a new tab (Home or a copy of the current tab). Open windows list their folder in $XDG_RUNTIME_DIR/minos/.
