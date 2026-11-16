# Agora (`agora`)

An app store laid out like the macOS one: **Discover** (featured cards, then picks for Create, Work, Play and Develop), the four tabs, **Categories** (every sub-category with its count), **Updates**, search, and an app page with a Get / Open button, the source, review notes and what installing will run.

## How installing works
No hidden installs. **Get** opens a terminal (Ghostty) and runs the command shown on the app's page: `sudo pacman -S --needed ...` (Arch repositories), `yay -S --needed ...` (the AUR: `yay` is the AUR helper installed on this system), `flatpak install -y flathub ...`, or the operating system's component command (its own components). The password prompt and all output are visible there; when the window closes the app's button changes to Open. **Remove** and **Update everything** (`yay -Syu`) work the same way. Passwords never pass through the Marketplace.

## Sources
- **The operating system's catalog** (reviewed): the file named by `catalog` in its `packages` integration (see INTEGRATIONS.md; Labyrinth's is `/usr/share/labyrinth/agora/catalog.json` in its own repository), with apps in Create, Work, Play, Develop, Security, Internet and Utilities, each with a sub-category. Components can only appear in this catalog, and only when the integration names a `component` command. Without an integration Agora lists only the sources you add.
- **Repositories you add** (Sources, bottom of the sidebar): an https address to a catalog file, or the Omarchy plugin registry. Adding shows a warning and needs a tick: *(the system) has not tested or reviewed anything from this source; it is up to you to decide what to trust.* Their apps are marked "Not reviewed by" the system (an amber dot in lists, a notice on the page). A repository cannot mark itself reviewed, cannot define components, and package names that are not plain names are dropped (they end up in a shell command).
- **Omarchy plugins** (`omacom/omarchy-plugin-marketplace` registry.json, ~5,600 listings): browse and search only. They are QML for Omarchy's own shell and run unsandboxed there, so there is no install button; **View** opens the plugin's repository. Plugins tagged `games` show under Play. See `OMARCHY_PLUGINS.md`. The plugins I wanted (timer, health, browser warm-up, agent usage, Proton Authenticator, controllers, 2048, Sudoku) were instead rewritten in Rust as native apps; they are in the system's catalog (Plugins, or Play for the games) and install like any reviewed app (THIRD_PARTY_NOTICES.md names each original).

## Look-alike names (2026-11-15)
A listing whose name imitates another is marked "Look-alike name" (amber, where the subtitle is) in every list, and its name is
drawn with hidden characters written out (`<U+200B>`). That is a name with letters from another alphabet ("Vivаldi" with a
Cyrillic а), fullwidth or mathematical letters, invisible or direction-changing characters (`yggdrasil-ui/src/lookalike.rs`, the
same check as Heimdall's and Labyrinth's), and an unreviewed listing that looks like an app of the system's reviewed list ("G1MP"
against "GIMP") or takes its very name from another source. Its page says why in plain words and what it really is: where it
installs from and the package or Flatpak id, the source it was listed by, and, once installed, the program its menu entry starts.
On while Settings > Security > Privacy > "Warn about look-alike names" is (`lookalike_warnings` in
`~/.config/heimdall/privacy.json`; read when the lists are built). Code: `src/names.rs`. Agora has no Flathub or AUR search of
its own, so the names it lists come from the catalogs only; those are the ones checked.

## Device software, offered by itself (2026-11-13)
Software for this computer's devices (Razer, Corsair, Logitech, gaming mice, printers, a laptop's fingerprint reader) is offered
without opening Agora: once the computer is online, when such a USB device is plugged in, and at sign-in, one notification names
what is missing, with **Install** (the component command, root steps through the password box) and **Don't ask again**. Closing it
means not now (asked again next session). Nothing installs without the click. The device list is the OS's (its `packages`
integration's `devices`, the installer's own list); see packages/device-offers/README.md.

## Writing a catalog file
```json
{ "name": "My apps",
  "apps": [
    { "id": "myapp", "name": "My App", "subtitle": "One line", "description": "A paragraph.",
      "category": "Work", "subcategory": "Office",
      "source": { "type": "pacman", "packages": ["myapp"] },
      "icon": "myapp", "tags": ["notes"], "launch": "myapp" } ] }
```
`source.type` is `pacman`, `aur` (packages) or `flatpak` (`app`). `category` is any text (Create, Work, Play and Develop get their own tabs).

## Not done yet
Installing plugins from the Omarchy registry (not planned: the ones wanted were rewritten in Rust, above), in-app progress instead of a terminal, ratings, screenshots, the Artifex apps (waiting to learn which), a start-menu pin, remembering search history. (Pin to dock: built 2026-11-07, on an installed app's page when it has a menu
entry; `yggdrasil_conf::dock`, which the panel re-reads within a second.)

## Descriptions and screenshots (added 2026-10-17)
An app's page can show a fuller description, screenshots, the licence, the maker and the website. The data is AppStream, the standard store metadata:
- **The app's own** (`appstreamcli dump`, from the installed app: no network). The ArtCraft apps ship good metadata with screenshots.
- **Flathub's public AppStream service** (`https://flathub.org/api/v2/appstream/<id>`) for the apps in the system's catalog that are on Flathub (`flathub` field). Cached in `~/.cache/yggdrasil/market/details`.
- Pictures are downloaded from where that data points (Flathub's media server, the app's repository) into `~/.cache/yggdrasil/market/img`, HTTPS only, 8 MB each.
- **Icons** (added 2026-11-15): the vendor's own, from this computer, else Flathub's, else the address in the system catalog's `icon_url` (for apps on neither, such as the ArtCraft apps from the AUR: their repository's icon, pinned to a commit). That address is fetched once in the background, HTTPS only (redirects too), at most 1 MB, and kept in `~/.cache/yggdrasil/market/icons` under the app's id only if its content is a PNG (up to 1024 px a side) or an SVG that draws only itself (no scripts, pictures or links outside the file). Then the brand logo, the icon pack's, and the letter tile.

**Nothing is fetched until you say yes.** The first app page with remote data asks ("Show screenshots and a fuller description?"); the answer is kept in `~/.config/yggdrasil/market/settings.json` and can be changed under Sources. Added repositories cannot set these fields (they are read only from the system's own catalog), so a repository cannot make the Marketplace contact an address of its choosing.

## Install progress, reinstall (2026-10-17)
Installs, removals and updates of **reviewed** apps now run inside the Marketplace: the Get pill fills as it goes, the app's page shows a progress card (what it is doing, a bar, and "Show details" for the output), and **Activity** in the sidebar lists everything running or finished (the badge counts running ones). A desktop notification says when it is done. **Reinstall** (next to Remove, on an installed reviewed app) runs the install again without `--needed`.
- Commands: `pkexec pacman -S/-Rs --noconfirm` (repositories), `yay -S --noconfirm --sudo /usr/lib/agora/sudo` (AUR), `flatpak install --user` (Flathub), the integration's `component` command with `install`/`remove` (components, with `AGORA_SUDO=/usr/lib/agora/sudo`), `yay -Syu` for "Update everything".
- Administrator rights are asked for through the desktop's own password box (polkit); `/usr/lib/agora/sudo` is a stand-in for `sudo` that yay and the component command use. A dismissed box shows as "Cancelled".
- Apps from repositories you added are not run this way: they still open a terminal, so their prompts (an AUR build file, for instance) can be read before saying yes.
- Not done: queueing several installs, per-package progress for AUR builds (the bar moves on gently while it builds).

## Cancel, history, smoother bar (2026-10-17)
- **Cancel** (on the progress card): the user-level programs (yay, makepkg, flatpak) are stopped through their process group; the administrator part runs inside `/usr/lib/agora/pkg-root` (started with pkexec, one password box), which watches a cancel file in `/run/user/<uid>/agora/` and, on cancel, stops pacman, removes the packages that run had added (the set of installed packages before and after) and the package files it downloaded. The Marketplace removes an AUR build's folders in `~/.cache/yay` and uninstalls a Flatpak it had just added. A cancelled install is not put in the history. `pkg-root` (packages/agora-helpers) accepts only `pacman` install and remove commands with a short list of options, plain package names, and a cancel file inside the invoking user's runtime folder (`packages/agora-helpers/tests/pkg_root.rs`).
- **History** (sidebar, above Sources): apps whose install (or reinstall) *finished*, newest first, with the date; Open or Get beside each one, "Clear history". Kept in `~/.local/share/yggdrasil/market/history.json`. Cancelled and failed installs, removals and updates are not added.
- **Bar:** downloading and installing are two stretches of one bar (each half, when there was a download; pacman's per-file download bars and its `(n/m)` install counts are read), it never goes backwards, it starts moving the moment the job begins, moves on gently while a build gives no numbers, and is eased toward each new value so it glides.

## Editions: Labyrinth and LykanOS (2026-10-17)
The Marketplace is shared by Labyrinth OS and LykanOS, formerly Chymaera OS (the Artifex ecosystem; [LykanOS](https://github.com/Artifex-Technologies/LykanOS)). The integration's `edition`, else `/etc/os-release` `ID=`, picks the edition (`AGORA_EDITION=lykanos` tries the other one). An app in the catalog can carry `"editions": ["lykanos"]` and is then listed only there; no `editions` means both. LykanOS's `ID=` is `lykanos`; Labyrinth's catalog still marks its LykanOS-only entries with the old name `chymaera`, so they have to be renamed there to show on LykanOS. LykanOS-only today: Ghidra and reverse engineering, Wireshark. Penetration-testing tools such as Argus go in the same way (add the entry with `"editions": ["lykanos"]` once it has a package or a source). Labyrinth's own listing and descriptions say nothing about penetration testing: Sarina is described as a helper and companion. Repositories users add cannot set `editions`. Not changed: the Labyrinth image still contains the reverse-engineering component and its preset (`components.json`, `labyrinth-rea`); say if LykanOS-only components should leave the Labyrinth image too.

## Updates (2026-10-17)
The Updates page used to read pacman's local database, which is only as fresh as the last `pacman -Sy`, so it said "up to date" while updates existed. It now looks properly, in the background and again every half hour while the window is open: the repositories through a private copy of the sync databases (like `checkupdates`, refreshed under `fakeroot` with a copy of pacman.conf that does not check signatures, because without root the keyring cannot be read; the list is only names and versions, the update itself still checks everything), and the AUR with `yay -Qua`. **The system's own builds** (the `local_builds` list in its catalog and the folders in the integration's `own_builds`, on Labyrinth `/usr/share/labyrinth/pkgbuilds`: Sarina, Heimdall, Limbo, the WhiteSur themes ...) are left out, because the AUR has unrelated packages with the same names (a different `heimdall-git`, a different WhiteSur version). "Update everything" is one run of `yay -Syu --noconfirm --sudo /usr/lib/agora/sudo --ignore <those>`: yay's root steps go through
`pkg-root`, which turns yay's refresh (`pacman -S -y`) into a full repository upgrade (a refresh alone would leave the system half
updated), then installs the AUR builds (`pacman -U`, package files only, `--config /etc/pacman.conf` only). Because one process asks
for every step, the password box asks once: the polkit action `org.yggdrasil.agora.packages` (`auth_admin_keep`, for
`/usr/lib/agora/pkg-root` only) remembers it for polkit's few minutes. An update whose AUR builds take longer than that asks again.
It has no Cancel (cancelling a system upgrade half-way is not safe).
