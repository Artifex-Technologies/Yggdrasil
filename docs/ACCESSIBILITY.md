# Accessibility audit (2026-10-16)

**Status 2026-11-04:** KDE is gone (removed from the owner's PC on 2026-10-24), and with it everything this audit said was still
KDE's: `kaccess` (sticky, slow and bounce keys, mouse keys), KWin's zoom and high-contrast effects, KDE's Orca hook and
`qt6-virtualkeyboard` are not installed now. The Yggdrasil session runs on Hyprland 0.57, and **nothing replaces them yet**: there
are no sticky, slow or bounce keys, no mouse keys, no screen magnifier, no high-contrast mode and no on-screen keyboard. Details in
the table and in items 7 and 8 below.

Why now (2026-10-16): `docs/KDE_INVENTORY.md` says `kaccess` and the screen reader hook must not be dropped without a plan. Labyrinth's own desktop
pieces (panel, lock screen, polkit box, Settings, Minos) are replacing KDE's, so each one has to keep, or provide, what KDE gave.
This is a read-only audit of the code and the machine, plus a plan. Nothing was tested with a real screen reader or by a person who relies
on one: those results are listed as "unverified".

## What each piece does today

| Piece | Toolkit | Screen reader (AT-SPI) | Keyboard only | Notes |
| --- | --- | --- | --- | --- |
| Settings, Minos, admin password box (polkit), keyring-sync | egui/eframe 0.36 | **Yes in principle, but probably not active on this system (see below)**: eframe builds with AccessKit, and `accesskit_unix` / `accesskit_atspi_common` are in their Cargo.lock | Tab and arrows work in egui widgets; some custom-drawn rows (Settings rows, Minos grid) may be skipped | unverified with Orca (not installed here) |
| Top bar, dock, start menu, notification toasts, notification pane, tray | custom tiny-skia on layer-shell (`yggdrasil-panel`) | **No**: not in the accessibility tree at all (no accesskit in its lock) | Start menu: Super, typing, Tab/arrows/Enter/Escape. Dock, bar, tray and toast buttons: no keyboard path | the biggest gap |
| Lock screen | custom (`yggdrasil-lock`) | **No** | Password typing and Enter only | a screen-reader user cannot tell that it is locked or whether a password failed |
| Calculator | egui | as above | as above | |
| Boot menu / installer | GRUB / `yggdrasil-installer` (egui, eframe 0.36) | installer: as the egui apps above (AccessKit is in its lock, unverified); GRUB: none | installer: Tab and arrows in egui widgets | the live ISO has no speech boot entry; Calamares is no longer used (removed from Labyrinth's ISO on 2026-11-07) |
| Compositor features (zoom, high contrast colours, sticky keys) | were KWin and kaccess; today Hyprland | **Missing since KDE left (2026-10-24)** | | Hyprland 0.57 has a zoom around the pointer (`cursor:zoom_factor`) that Yggdrasil does not switch on or offer anywhere, and no sticky, slow or bounce keys setting; no high-contrast mode |

On this machine (rechecked 2026-11-04): the AT-SPI bus is running (`at-spi2-core` is installed); **Orca and Speech Dispatcher are not
installed**; eSpeak NG is. `kaccess` is not installed; the file `~/.config/kaccessrc` (KDE's hook that started Orca when asked) is
left over, and nothing reads it.

## Found while building the panel's support: the egui apps are probably silent on this system

`at-spi2-core` 2.62 (what Arch ships) announces that a screen reader is running with the `IsEnabled` property of `org.a11y.Status`; it has no
`ScreenReaderEnabled` property. `accesskit_unix` 0.21, the version egui/eframe 0.36 pulls in, waits for `ScreenReaderEnabled`, so Settings,
Minos and the other egui apps most likely never switch their accessibility on, even with Orca running. `accesskit_unix` 0.24 (used by the
panel) watches `IsEnabled` and does work (see the test above). The fix is upstream (a newer egui/eframe) or carrying a patch; **recheck
with Orca before trusting the table below**, and again when eframe is upgraded.

## What is missing (in the order that hurts most)

1. **Panel and screen readers: built 2026-10-16 (bar, dock, pop-ups, kept notifications; not the start menu, calendar, Fylgja or lock screen yet).** `packages/panel/src/a11y.rs` describes the surfaces through AccessKit (accesskit 0.25 + accesskit_unix 0.24); a reader's click on a node is sent back as the same action as a real click. Verified by reading the tree over AT-SPI in a nested session (`tests/a11y/nested_panel_a11y.sh`): "Top bar" (Start menu, Desktops, Fylgja, Network, Bluetooth, Clock), "Dock" (every app and the trash), "Kept notifications", each button offering `click`. **Not verified:** a real screen reader reading it, and the click from a reader actually opening the menu. Original finding: **Panel not exposed to screen readers.** A blind user cannot open an app, read a notification, or see the clock. Fix: give
   `yggdrasil-panel` an AT-SPI tree with `accesskit_unix` (it supports toolkits that are not winit): one node per button, the start menu list,
   toasts (with their action buttons) and the notification list, with roles and names. Announce new notifications (live region).
2. **Lock screen silent.** Announce "Screen locked. Enter your password" and "Wrong password" (the same AccessKit route, plus a
   short audible cue for people without a reader). Keep the keyboard path exactly as it is.
3. **No keyboard path to the dock, bar, tray and toast buttons.** Add a focus ring and a shortcut to move into the panel (for example
   Super+Tab for the dock, Super+N for the newest notification's buttons: Allow / Deny prompts from "Ask before connecting" are otherwise mouse-only).
4. **Text size and reduce motion: built for the egui apps (2026-10-16), not yet for the panel.** Settings > Appearance > Accessibility sets 100/125/150/200 % for Settings, Minos, the admin password box, Calculator and the keyring tool (egui zoom, applied within a second), and "Reduce motion" for their animations. The bar, dock, start menu, toasts and lock screen still follow only the screen scale. Original finding: **No text size setting.** Settings > Appearance has display scale (fractional) but no "text size" for our apps. Add a 100/125/150/200 % setting
   read by the panel and every egui app (`pixels_per_point` multiplier), so low-vision users need not enlarge the whole desktop.
5. **No high-contrast or reduced-motion setting, no big-cursor setting.** Add a high-contrast palette variant in `yggdrasil-ui`, a "reduce
   motion" switch (animations in the panel are short, but there are some), and cursor size in Settings.
6. **Not shipped:** Orca, Speech Dispatcher, eSpeak NG, braille. Added now as the optional component `accessibility` (and in the
   "everything" preset). Still to do: install it on every machine by default, a boot-menu entry that starts the live ISO with speech
   (archiso's `accessibility=on` / espeakup), and a Super+Alt+S binding (KDE's shortcut went with KDE; nothing in the Yggdrasil session starts a screen reader from the keyboard yet).
7. **Sticky/slow/bounce keys, mouse keys, the magnifier: missing now.** They came from KDE (kaccess and KWin effects), which is gone.
   Hyprland 0.57 has no setting for sticky, slow or bounce keys or mouse keys (checked in its option list, `hyprctl descriptions`).
   It does have a zoom around the pointer (`cursor:zoom_factor`, 1 to 10), but Yggdrasil binds no key to it and Settings has no
   switch for it, so for a user there is no magnifier today. To do: a magnifier switch and keys on that zoom, and sticky keys in
   Yggdrasil itself or in the compositor (libinput supports sticky/bounce keys only partly). Yggdrasil's own compositor must offer
   them too when it runs the real session.
8. **On-screen keyboard: missing now.** `qt6-virtualkeyboard` went with Plasma and no other on-screen keyboard is installed; our own
   lock screen and password box have none either. Needed for tablets and for some motor impairments.

## Already fine

- Status is not shown by colour alone: Minos's Proton Drive marks differ in shape (cloud, check, arrows, cross), the sync badges likewise.
- The start menu is fully keyboard operable and opens with the Super key.
- Notifications that need a decision stay on screen until answered (critical ones ignore Do Not Disturb).

## Do not remove until replaced (2026-10-16; overtaken)

The list was: `kaccess`, `orca`'s KDE hook, KWin zoom/high-contrast effects, `qt6-virtualkeyboard` (it is also a Plasma dependency).
They were removed with KDE on 2026-10-24 before replacements existed, so the items above are now gaps to fill, not things to keep.

## How to verify (needs a person, about 30 minutes)

1. `sudo pacman -S orca speech-dispatcher espeak-ng` (or install the `accessibility` component in Settings > Apps > Optional software).
2. Start Orca (`orca`), open Settings and Minos with the keyboard only; check that rows, switches and buttons are announced by name.
3. Open the start menu with Super; check that the list is read. Expect the dock, bar and notifications to be silent (known gap 1).
4. Lock the screen (Super+L) and unlock it with the keyboard; expect silence (known gap 2).
