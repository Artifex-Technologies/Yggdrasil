# yggdrasil-portal

Yggdrasil's backend for xdg-desktop-portal: what programs (sandboxed or not) ask the desktop for through the portal.

| Interface | What it does | Sessions |
| --- | --- | --- |
| `FileChooser` (`src/main.rs`) | Minos as every program's open and save dialog (`minos --pick`), opening where the same program last picked from | both |
| `Settings` (`src/settings.rs`) | dark or light, and the accent colour, followed live | both |
| `ScreenCast` (`src/screencast.rs`) | screen sharing: the share picker asks which screens, `yggdrasil-screencast` publishes each as a PipeWire stream ([packages/screencast](../screencast/README.md)) | own compositor |
| `Screenshot` (`src/screenshot.rs`) | a screenshot of the whole desktop (grim) or, when the program asks for it to be interactive, of what the person chooses (`yggdrasil-shot --portal`); a colour the person picks (`yggdrasil-colorpicker --portal`) | own compositor |
| `Request` | a program that gives up closes the dialog or tool it opened | both |

Which backend answers what is set per session: `data/yggdrasil-portals.conf` for the session on Hyprland (screen sharing,
screenshots and global shortcuts: Hyprland's portal) and `data/yggdrasil-compositor-portals.conf` for the session on the own
compositor (its desktop names are `Yggdrasil-Compositor:Yggdrasil`).

Screenshots and colours for programs leave the person's things alone: no copy to the clipboard, nothing in Pictures, the
colour history or the notifications. A screenshot is a file of the portal's own in the runtime folder
(`$XDG_RUNTIME_DIR/yggdrasil-portal/screenshots`, this user's only; the newest ten kept, all gone at log-out). The Screenshot
interface is version 1: the portal frontend asks the person itself before a sandboxed program's screenshot that is not
interactive.

## Tests

`cargo test`: the unit tests, and `tests/screenshot.rs` on a D-Bus of the test's own with stand-ins for grim,
yggdrasil-shot and yggdrasil-colorpicker: a whole-desktop screenshot (in the portal's own folder, the folder this user's
only), an interactive one, a cancel (no file left), only the newest ten kept, a colour, a cancelled pick, and a pick the
program closes while the picker is open. Screen sharing is tested end to end in `packages/screencast/tests`. Skipped where
dbus-daemon is not installed, except in CI (`YGGDRASIL_TEST_PORTAL_REQUIRED`).
