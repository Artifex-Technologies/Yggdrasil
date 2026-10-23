# Agora (`agora`)

An app store laid out like the macOS one: **Discover** (featured cards, then picks for Create, Work, Play and Develop), the four tabs, **Categories** (every sub-category with its count), **Updates**, search, and an app page with a Get / Open button, the source, review notes and what installing will run.

## How installing works
No hidden installs. **Get** opens a terminal (Ghostty) and runs the command shown on the app's page: `sudo pacman -S --needed ...` (Arch repositories), `yay -S --needed ...` (the AUR: `yay` is the AUR helper installed on this system), `flatpak install -y flathub ...`, or the operating system's component command (its own components). The password prompt and all output are visible there; when the window closes the app's button changes to Open. **Remove** and **Update everything** (`yay -Syu`) work the same way. Passwords never pass through the Marketplace.

## Sources
- **The operating system's catalog** (reviewed): the file named by `catalog` in its `packages` integration (see INTEGRATIONS.md; Labyrinth's is `/usr/share/labyrinth/agora/catalog.json` in its own repository), with apps in Create, Work, Play, Develop, Security, Internet and Utilities, each with a sub-category. Components can only appear in this catalog, and only when the integration names a `component` command. Without an integration Agora lists only the sources you add.
- **Repositories you add** (Sources, bottom of the sidebar): an https address to a catalog file, or the Omarchy plugin registry. Adding shows a warning and needs a tick: *(the system) has not tested or reviewed anything from this source; it is up to you to decide what to trust.* Their apps are marked "Not reviewed by" the system (an amber dot in lists, a notice on the page). A repository cannot mark itself reviewed, cannot define components, and package names that are not plain names are dropped (they end up in a shell command).
- **Omarchy plugins** (`omacom/omarchy-plugin-marketplace` registry.json, ~5,600 listings): browse and search only. They are QML for Omarchy's own shell and run unsandboxed there, so there is no install button; **View** opens the plugin's repository. Plugins tagged `games` show under Play. See `OMARCHY_PLUGINS.md`.

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
Installing Omarchy plugins (needs the sandboxed host), in-app progress instead of a terminal, ratings, screenshots, the Artifex apps (waiting to learn which), a dock/start-menu pin, remembering search history.

## Descriptions and screenshots (added 2026-10-17)
An app's page can show a fuller description, screenshots, the licence, the maker and the website. The data is AppStream, the standard store metadata:
- **The app's own** (`appstreamcli dump`, from the installed app: no network). The ArtCraft apps ship good metadata with screenshots.
- **Flathub's public AppStream service** (`https://flathub.org/api/v2/appstream/<id>`) for the apps in the system's catalog that are on Flathub (`flathub` field). Cached in `~/.cache/yggdrasil/market/details`.
- Pictures are downloaded from where that data points (Flathub's media server, the app's repository) into `~/.cache/yggdrasil/market/img`, HTTPS only, 8 MB each.

**Nothing is fetched until you say yes.** The first app page with remote data asks ("Show screenshots and a fuller description?"); the answer is kept in `~/.config/yggdrasil/market/settings.json` and can be changed under Sources. Added repositories cannot set these fields (they are read only from the system's own catalog), so a repository cannot make the Marketplace contact an address of its choosing.

## Install progress, reinstall (2026-10-17)
Installs, removals and updates of **reviewed** apps now run inside the Marketplace: the Get pill fills as it goes, the app's page shows a progress card (what it is doing, a bar, and "Show details" for the output), and **Activity** in the sidebar lists everything running or finished (the badge counts running ones). A desktop notification says when it is done. **Reinstall** (next to Remove, on an installed reviewed app) runs the install again without `--needed`.
- Commands: `pkexec pacman -S/-Rs --noconfirm` (repositories), `yay -S --noconfirm --sudo /usr/lib/agora/sudo` (AUR), `flatpak install --user` (Flathub), the integration's `component` command with `install`/`remove` (components, with `AGORA_SUDO=/usr/lib/agora/sudo`), `yay -Syu` for "Update everything".
- Administrator rights are asked for through the desktop's own password box (polkit); `/usr/lib/agora/sudo` is a stand-in for `sudo` that yay and the component command use. A dismissed box shows as "Cancelled".
- Apps from repositories you added are not run this way: they still open a terminal, so their prompts (an AUR build file, for instance) can be read before saying yes.
- Not done: queueing several installs, per-package progress for AUR builds (the bar moves on gently while it builds).

## Cancel, history, smoother bar (2026-10-17)
- **Cancel** (on the progress card): the user-level programs (yay, makepkg, flatpak) are stopped through their process group; the administrator part runs inside `/usr/lib/agora/pkg-root` (started with pkexec, one password box), which watches a cancel file in `/run/user/<uid>/agora/` and, on cancel, stops pacman, removes the packages that run had added (the set of installed packages before and after) and the package files it downloaded. The Marketplace removes an AUR build's folders in `~/.cache/yay` and uninstalls a Flatpak it had just added. A cancelled install is not put in the history. `pkg-root` (packages/market/helpers) accepts only `pacman` install and remove commands with a short list of options, plain package names, and a cancel file inside the invoking user's runtime folder (`tests/test_agora_pkg_root.py`).
- **History** (sidebar, above Sources): apps whose install (or reinstall) *finished*, newest first, with the date; Open or Get beside each one, "Clear history". Kept in `~/.local/share/yggdrasil/market/history.json`. Cancelled and failed installs, removals and updates are not added.
- **Bar:** downloading and installing are two stretches of one bar (each half, when there was a download; pacman's per-file download bars and its `(n/m)` install counts are read), it never goes backwards, it starts moving the moment the job begins, moves on gently while a build gives no numbers, and is eased toward each new value so it glides.

## Editions: Labyrinth and Chymaera (2026-10-17)
The Marketplace is shared by Labyrinth OS and Chymaera OS (the Artifex ecosystem). The integration's `edition`, else `/etc/os-release` `ID=`, picks the edition (`AGORA_EDITION=chymaera` tries the other one). An app in the catalog can carry `"editions": ["chymaera"]` and is then listed only there; no `editions` means both. Chymaera-only today: Ghidra and reverse engineering, Wireshark. Penetration-testing tools such as Argus go in the same way (add the entry with `"editions": ["chymaera"]` once it has a package or a source). Labyrinth's own listing and descriptions say nothing about penetration testing: Sarina is described as a helper and companion. Repositories users add cannot set `editions`. Not changed: the Labyrinth image still contains the reverse-engineering component and its preset (`components.json`, `labyrinth-rea`); say if Chymaera-only components should leave the Labyrinth image too.

## Updates (2026-10-17)
The Updates page used to read pacman's local database, which is only as fresh as the last `pacman -Sy`, so it said "up to date" while updates existed. It now looks properly, in the background and again every half hour while the window is open: the repositories through a private copy of the sync databases (like `checkupdates`, refreshed under `fakeroot` with a copy of pacman.conf that does not check signatures, because without root the keyring cannot be read; the list is only names and versions, the update itself still checks everything), and the AUR with `yay -Qua`. **The system's own builds** (the `local_builds` list in its catalog and the folders in the integration's `own_builds`, on Labyrinth `/usr/share/labyrinth/pkgbuilds`: Sarina, Heimdall, Limbo, the WhiteSur themes ...) are left out, because the AUR has unrelated packages with the same names (a different `heimdall-git`, a different WhiteSur version). "Update everything" runs `pkexec /usr/lib/agora/pkg-root ... pacman -Syu --noconfirm --ignore <those>` and then `yay -Sua --noconfirm --ignore <those>`; it has no Cancel (cancelling a system upgrade half-way is not safe).
