# Labyrinth Marketplace (`labyrinth-market`)

An app store laid out like the macOS one: **Discover** (featured cards, then picks for Create, Work, Play and Develop), the four tabs, **Categories** (every sub-category with its count), **Updates**, search, and an app page with a Get / Open button, the source, review notes and what installing will run.

## How installing works
No hidden installs. **Get** opens a terminal (Ghostty) and runs the command shown on the app's page: `sudo pacman -S --needed ...` (Arch repositories), `yay -S --needed ...` (the AUR: `yay` is the AUR helper installed on this system), `flatpak install -y flathub ...`, or `labyrinth-apps install ...` (Labyrinth's own components). The password prompt and all output are visible there; when the window closes the app's button changes to Open. **Remove** and **Update everything** (`yay -Syu`) work the same way. Passwords never pass through the Marketplace.

## Sources
- **Labyrinth** (built in, reviewed): `packages/market/catalog.json`, 68 apps in Create, Work, Play, Develop, Security, Internet and Utilities, each with a sub-category. The ArtCraft apps carry the findings of `docs/ARTCRAFT_REVIEW.md`. Components (steam, sarina, heimdall ...) can only appear in this catalog.
- **Repositories you add** (Sources, bottom of the sidebar): an https address to a Labyrinth catalog file, or the Omarchy plugin registry. Adding shows a warning and needs a tick: *Labyrinth has not tested or reviewed anything from this source; it is up to you to decide what to trust.* Their apps are marked "Not reviewed by Labyrinth" (an amber dot in lists, a notice on the page). A repository cannot mark itself reviewed, cannot define components, and package names that are not plain names are dropped (they end up in a shell command).
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
