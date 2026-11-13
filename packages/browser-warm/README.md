# yggdrasil-browser-warm

Keeps your default browser running in the background with no windows open. A new window or a clicked link then opens
almost at once, because it goes to the browser that is already running instead of starting a new one. Linux builds of
browsers have no "startup boost" of their own; this provides one. It is based on
[omarchy-speedup-browser-startup](https://github.com/AgentRBY/omarchy-speedup-browser-startup), which measured windows
opening in about 0.08 s instead of 0.5 s (0.15 s for Vivaldi).

**What it costs:** memory. The browser stays loaded even with every window closed, usually 0.5 to 0.8 GB.

## Which browsers

Chromium-based ones: Chromium, Google Chrome, Brave, Microsoft Edge, Vivaldi, Helium and Thorium. The browser starts with
`--no-startup-window --keep-alive-for-test`. The first flag starts it with no window. The second keeps it running:
without it, Chromium quits as soon as its extensions have loaded, because nothing is holding it open.

**Background mode must be on.** `--keep-alive-for-test` works through the browser's background mode (Chromium's
"Continue running background apps" setting, stored as `background_mode` in the profile's `Local State` file). With
background mode off, the browser quits within a second of starting with no window, whatever the flags. I tested this
with Vivaldi 8.3: `--keep-alive-for-test`, `--silent-launch` and the two together all quit at once with background mode
off, and `--keep-alive-for-test` keeps it running with background mode on. I found no other flag or setting that keeps it
running with no window. So when background mode is off, the browser is not started at all, and `status` says why.
Background mode also shows the browser's icon in the tray while it runs in the background, so warm start comes with
that icon. To use warm start, turn background mode back on. I found no switch for it in Vivaldi's settings: with
Vivaldi closed, set `"background_mode":{"enabled":true}` in `~/.config/vivaldi/Local State`, or delete that entry.

Firefox-based browsers (Firefox, LibreWolf, Zen, Floorp, Waterfox, Mullvad Browser) cannot run with no window, so this
does nothing for them. It also does nothing for Flatpak browsers. It keeps checking, so it starts working if you later
switch to a Chromium-based browser.

## How it works

The `yggdrasil-browser-warm` user service checks every minute.

- **Starting:** when none of the browser is running, it starts the browser as a user unit of its own,
  `yggdrasil-browser-warm-instance`. If the browser is already open, it waits until you close it, then starts it in the
  background.
- **Restarting:** it restarts the background browser only when the browser was updated (its program on disk was
  replaced) or your default browser changed. Even then, it only restarts it when it has no windows open and pacman is
  not installing anything. It finds open windows through the compositor (yggdrasil-wm: Hyprland's
  window list on its socket, or Yggdrasil's own compositor's). When it cannot tell whether
  windows are open, it does not restart anything.
- **A browser that will not stay running:** if the background browser quits on its own right after it was started
  (gone by the next check), it is tried again after 5 minutes, then after 15. After the third such exit it is not
  started again until the browser is updated (its program on disk is replaced) or your default browser changes, also
  after a restart or reboot (the note is kept in `~/.local/state/yggdrasil/browser-warm-gave-up`).
  `yggdrasil-browser-warm status` says so, and `yggdrasil-browser-warm start` tries again at once.
- **Quitting:** if you quit the browser from its menu, it is started again in the background within a minute. To turn
  this off, run `systemctl --user disable --now yggdrasil-browser-warm`. Stopping the service also stops the background
  browser, unless it has windows open; it then ends when they are closed or when you log out.

Session restore ("continue where you left off") keeps working. Your tabs come back with the first window.

## The desktop's app rules and the sandbox

On a desktop with an app launcher that applies per-app rules (the `apps` integration in Yggdrasil's
`docs/INTEGRATIONS.md`, such as `labyrinth-launch`):

- **Network rules:** the background browser runs in the same slice as the launcher's copy (`lapp_<id>.slice`).
  That app's network rule, such as blocked or Wi-Fi only, covers it the same way.
- **Sandbox:** a browser that the launcher runs in a sandbox is never warmed. The launcher has no way to start an app
  with extra arguments. A background copy started outside the sandbox would take over every window opened later, so the
  sandbox would have no effect. Turning the sandbox on also stops the background copy, once it has no windows open.

## Commands

    yggdrasil-browser-warm status    # the default browser, the background copy, open windows, why it is not started
    yggdrasil-browser-warm start     # start it now (the service does this by itself); also a new try after it gave up
    yggdrasil-browser-warm stop      # stop it now
    yggdrasil-browser-warm run       # what the service runs

To pass extra flags to the background browser, put them in `~/.config/yggdrasil/browser-warm-flags.conf`.
Separate them with spaces or new lines; lines starting with `#` are comments.

## Files

    /usr/bin/yggdrasil-browser-warm
    /usr/lib/systemd/user/yggdrasil-browser-warm.service
