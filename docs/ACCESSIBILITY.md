# Accessibility audit (2026-10-16)

Why now: `docs/KDE_INVENTORY.md` says `kaccess` and the screen reader hook must not be dropped without a plan. Labyrinth's own desktop
pieces (panel, lock screen, polkit box, Settings, Minos) are replacing KDE's, so each one has to keep, or provide, what KDE gave.
This is a read-only audit of the code and the machine, plus a plan. Nothing was tested with a real screen reader or by a person who relies
on one: those results are listed as "unverified".

## What each piece does today

| Piece | Toolkit | Screen reader (AT-SPI) | Keyboard only | Notes |
| --- | --- | --- | --- | --- |
| Settings, Minos, admin password box (polkit), keyring-sync | egui/eframe 0.36 | **Yes in principle**: eframe builds with AccessKit, and `accesskit_unix` / `accesskit_atspi_common` are in their Cargo.lock | Tab and arrows work in egui widgets; some custom-drawn rows (Settings rows, Minos grid) may be skipped | unverified with Orca (not installed here) |
| Top bar, dock, start menu, notification toasts, notification pane, tray | custom tiny-skia on layer-shell (`labyrinth-panel`) | **No**: not in the accessibility tree at all (no accesskit in its lock) | Start menu: Super, typing, Tab/arrows/Enter/Escape. Dock, bar, tray and toast buttons: no keyboard path | the biggest gap |
| Lock screen | custom (`labyrinth-lock`) | **No** | Password typing and Enter only | a screen-reader user cannot tell that it is locked or whether a password failed |
| Calculator | egui | as above | as above | |
| Boot menu / installer | GRUB / Calamares (Qt) | Calamares: Qt exposes AT-SPI; GRUB: none | | the live ISO has no speech boot entry |
| Compositor features (zoom, high contrast colours, sticky keys) | KWin, kaccess | Still KDE's | | stay until a replacement exists |

On this machine: the AT-SPI bus is running; **Orca and Speech Dispatcher are not installed**; `~/.config/kaccessrc` has `[ScreenReader] SystemdManaged=true`
(KDE's own hook that starts Orca when asked).

## What is missing (in the order that hurts most)

1. **Panel not exposed to screen readers.** A blind user cannot open an app, read a notification, or see the clock. Fix: give
   `labyrinth-panel` an AT-SPI tree with `accesskit_unix` (it supports toolkits that are not winit): one node per button, the start menu list,
   toasts (with their action buttons) and the notification list, with roles and names. Announce new notifications (live region).
2. **Lock screen silent.** Announce "Screen locked. Enter your password" and "Wrong password" (the same AccessKit route, plus a
   short audible cue for people without a reader). Keep the keyboard path exactly as it is.
3. **No keyboard path to the dock, bar, tray and toast buttons.** Add a focus ring and a shortcut to move into the panel (for example
   Super+Tab for the dock, Super+N for the newest notification's buttons: Allow / Deny prompts from "Ask before connecting" are otherwise mouse-only).
4. **Text size and reduce motion: built for the egui apps (2026-10-16), not yet for the panel.** Settings > Appearance > Accessibility sets 100/125/150/200 % for Settings, Minos, the admin password box, Calculator and the keyring tool (egui zoom, applied within a second), and "Reduce motion" for their animations. The bar, dock, start menu, toasts and lock screen still follow only the screen scale. Original finding: **No text size setting.** Settings > Appearance has display scale (fractional) but no "text size" for our apps. Add a 100/125/150/200 % setting
   read by the panel and every egui app (`pixels_per_point` multiplier), so low-vision users need not enlarge the whole desktop.
5. **No high-contrast or reduced-motion setting, no big-cursor setting.** Add a high-contrast palette variant in `labyrinth-ui`, a "reduce
   motion" switch (animations in the panel are short, but there are some), and cursor size in Settings.
6. **Not shipped:** Orca, Speech Dispatcher, eSpeak NG, braille. Added now as the optional component `accessibility` (and in the
   "everything" preset). Still to do: install it on every machine by default, a boot-menu entry that starts the live ISO with speech
   (archiso's `accessibility=on` / espeakup), and a Super+Alt+S binding (KDE's shortcut disappears with KDE).
7. **Sticky/slow/bounce keys, mouse keys, the magnifier** come from KDE (kaccess and KWin effects). Keep `kaccess` and the KWin zoom while
   KWin is our compositor; when it is replaced, the new compositor must offer them (libinput supports sticky/bounce keys only partly).
8. **On-screen keyboard:** `qt6-virtualkeyboard` is installed (a Plasma dependency); our own lock screen and password box have no on-screen
   keyboard. Needed for tablets and for some motor impairments.

## Already fine

- Status is not shown by colour alone: Minos's Proton Drive marks differ in shape (cloud, check, arrows, cross), the sync badges likewise.
- The start menu is fully keyboard operable and opens with the Super key.
- Notifications that need a decision stay on screen until answered (critical ones ignore Do Not Disturb).

## Do not remove until replaced

`kaccess`, `orca`'s KDE hook, KWin zoom/high-contrast effects, `qt6-virtualkeyboard` (it is also a Plasma dependency). The KDE inventory
row stays "drop later, with a plan"; this document is the plan.

## How to verify (needs a person, about 30 minutes)

1. `sudo pacman -S orca speech-dispatcher espeak-ng` (or install the `accessibility` component in Settings > Apps > Optional software).
2. Start Orca (`orca`), open Settings and Minos with the keyboard only; check that rows, switches and buttons are announced by name.
3. Open the start menu with Super; check that the list is read. Expect the dock, bar and notifications to be silent (known gap 1).
4. Lock the screen (Super+L) and unlock it with the keyboard; expect silence (known gap 2).
