# Accessibility audit (2026-10-16)

**Status 2026-11-19:** Atkinson Hyperlegible is the system font by default, and Atkinson Hyperlegible Mono for code and
terminals (yggdrasil-theme's fontconfig file, `58-yggdrasil-default-font.conf`; Noto Sans stays a choice). Settings depends on
all six reading fonts and the theme on the two defaults, so a system installed before gets them with its next update.

**Status 2026-11-18:** fonts made for reading: Atkinson Hyperlegible (and Next, and Mono) for low vision, Lexend, OpenDyslexic
(and Mono), chosen in the installer's Font page and in Settings > Appearance > Fonts, for the whole system through fontconfig
(the login screen too). Details: APPS_SETTINGS.md, INSTALLER.md.

**Status 2026-11-13:** Settings has an Accessibility page (`packages/settings/src/accessibility.rs`, near the top of the
sidebar): text and control size and reduce motion (moved from Appearance; reduce motion now also reloads Hyprland so its window
animations follow), the pointer's size (`ygg-theme cursor-size 24|32|48|64|96`: Hyprland at once, programs as they start,
`cursor_size` in appearance.conf), Hyprland's zoom around the pointer with the Super+Alt+= / - / 0 keys beside it, and colour
filters (greyscale, protanopia, deuteranopia, tritanopia: a screen shader Settings writes to ~/.config/yggdrasil/shaders, named
in accessibility.conf, which the session's yggdrasil.lua gives Hyprland at every start; `packages/session/tests/color-filter.lua`).
Colour filters are Hyprland's: Yggdrasil's own compositor does not offer them yet; since 2026-11-24 it zooms (the same keys
and Settings' Zoom, through yggdrasil-wm; packages/compositor/README.md, "The magnifier"). Since 2026-11-24 a High contrast
switch there gives the egui apps a black-and-white palette (`yggdrasil_ui::theme::palette_with`): text and dim text at least
7:1 on every background, lines 3:1, status colours 4.5:1, the chosen accent moved toward white or black until it reads at 7:1,
buttons and fields outlined; `the_high_contrast_palettes_meet_the_contrast_levels` checks every pair on both styles with six
accents. It follows the switch without a restart. Still missing: sticky, slow and bounce keys, mono audio. The shaders and the live zoom were not tried on a screen yet.

**Status 2026-11-04:** KDE is gone (removed from the owner's PC on 2026-10-24), and with it everything this audit said was still
KDE's: `kaccess` (sticky, slow and bounce keys, mouse keys), KWin's zoom and high-contrast effects, KDE's Orca hook and
`qt6-virtualkeyboard` are not installed now. The Yggdrasil session runs on Hyprland 0.57, and **nothing replaces them yet**: there
are no sticky, slow or bounce keys, no mouse keys, no screen magnifier, no high-contrast mode and no on-screen keyboard. Details in
the table and in items 7 and 8 below.

Why now (2026-10-16): my inventory of what KDE did on this machine (in my private plans) says `kaccess` and the screen reader hook must not be dropped without a plan. Labyrinth's own desktop
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

## Known limitations (2026-11-19)

1. **Screen readers and the panel:** the bar, dock, pop-ups and kept notifications are described to screen readers
   (`packages/panel/src/a11y.rs`, AccessKit over AT-SPI; checked by reading the tree in a nested session,
   `tests/a11y/nested_panel_a11y.sh`); the start menu, calendar, Fylgja and the lock screen are not.
2. **The lock screen is silent:** it announces nothing ("Screen locked", "Wrong password") and gives no audible cue.
3. **No keyboard path to the dock, bar, tray and toast buttons:** they are mouse-only, the Allow / Deny prompts of "Ask before
   connecting" included.
4. **Text size, reduce motion and high contrast** (Settings > Accessibility) apply to the egui apps (Settings, Minos, the
   password box, Calculator, the keyring tool); the bar, dock, start menu, toasts and lock screen follow only the screen scale.
5. **High contrast reaches only the egui apps** (item 4); GTK and Qt programs keep their own themes. The pointer's size is in
   Settings > Accessibility.
6. **Orca, Speech Dispatcher and eSpeak NG come as the optional component `accessibility`** (and in the "everything" preset), not
   by default. Super+Alt+S starts or stops Orca (`yggdrasil-screen-reader`) and says how to install it when it is missing. The
   live ISO has no boot entry that starts with speech, and there is no braille support.
7. **No sticky, slow or bounce keys, no mouse keys and no mono audio:** the keys came from KDE, which is gone, and Hyprland 0.57
   has no setting for them. The magnifier zooms around the pointer (Super+Alt+= / - / 0, and Zoom in Settings >
   Accessibility) on Hyprland and on Yggdrasil's own compositor; on the own compositor the window switcher's cards and a full
   screen game drawn opaque are not magnified.
8. **On-screen keyboard:** wvkbd (Agora's "On-screen keyboard") shows in tablet mode on a 2-in-1 while no keyboard is plugged in
   (DESKTOP.md, "Tablet mode and rotation"). It cannot be opened on demand, and the lock screen and password box, which draw
   their own fields, have none.

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
