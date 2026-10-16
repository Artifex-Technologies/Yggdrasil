# Notifications

Labyrinth's own notification service lives in the panel (`packages/panel/src/notify.rs`). It replaces mako/Plasma's.

## What it does
- Serves `org.freedesktop.Notifications` (Notify, CloseNotification, GetCapabilities, GetServerInformation, and the
  NotificationClosed / ActionInvoked signals). Any app that uses `notify-send` or libnotify just works.
- Keeps up to 200 notifications in `~/.local/state/labyrinth/notifications.json` until you open or clear them (they survive a
  panel restart or a reboot).
- Shows them **grouped by app, the groups ordered by their newest notification**. A group with more than one has a count badge;
  clicking it opens it out to list them all (newest first); clicking again folds it. Clicking one notification (or a group of one)
  **goes to the app**: it sends the app its `default` action (apps like chat and mail clients use that to open the right
  conversation or message), then brings the app's window forward, or starts the app. The notification is then removed.
- **Pop-ups**: a new notification slides in at the bottom right of the main screen for 7 s (14 s if critical, and it stays while the
  pointer is on it); up to three stack. Low-urgency ones skip the pop-up. Clicking the pop-up opens it; the x puts it away but
  keeps it in the list.
- **Heimdall alerts** arrive as notifications from "Heimdall" (polled from 127.0.0.1:8930; warnings and critical by default).
  A blocked app that connected opens its page in Settings > Apps when clicked. What is already in Heimdall's list when the
  panel starts is not announced.
- **Clear all** (and a small x on a group or a notification when the pointer is on it).

## Where it shows (Settings > Appearance > Notifications)
- Fylgja's **Notifications** section (replaces the old Apps section; the dock and Start menu already list apps).
- Or, with "Notification Center pane" on, a **pane down the right edge** that opens with the calendar (click the date): the
  calendar on top, the list below, flush with the screen edge.
- Settings: Pop-ups on/off; the pane on/off; Heimdall alerts: Off / Critical / Warnings / Everything (`notify_toasts`,
  `notify_pane`, `heimdall_alerts` in `~/.config/labyrinth/panel.conf`).

## Taking over from Plasma and mako (fresh installs)
Only one program can own the name `org.freedesktop.Notifications`. What decides it:
- **Plasma's server only exists while its panels do** (it lives in the notifications applet in Plasma's system tray).
  `labyrinth-panel-mode on` removes those panels, so Plasma lets go of the name.
- **mako** (kept for the Hyprland session) and Plasma each ship a D-Bus *activation* file for the name, so whichever one an early
  notification wakes would hold it. `labyrinth-panel-mode on` therefore writes a per-user file that comes first,
  `~/.local/share/dbus-1/services/org.freedesktop.Notifications.service`, which runs `labyrinth-notify-activate`: it starts
  the panel if it is not running and waits until the panel owns the name. `panel-mode off` removes the file (Plasma's applies again).
  It asks the bus to reload its service files, so it works on the first login without a restart.
- The panel also asks to replace an owner that allows it and retries every 10 s.
- Tested on this machine: with the panel killed, `notify-send` started the panel through D-Bus and the notification landed. Not yet
  tested: a full fresh install from the ISO.

## Not built yet (list for later)
- The theme option (macOS look vs a more Windows-like look) in Settings > Appearance.
- Do Not Disturb / quiet hours; per-app mute.
- Action buttons on the pop-up (apps send actions besides "default"); inline reply.
- A notification sound; images/avatars sent by apps (`image-data`).
- Heimdall alerts de-duplicated when it replays old session alerts after a restart.
- A badge on Fylgja's tab or the bar when there are unread notifications.
