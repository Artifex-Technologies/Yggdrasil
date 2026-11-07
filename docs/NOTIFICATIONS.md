# Notifications

Yggdrasil's own notification service lives in the panel (`packages/panel/src/notify.rs`). It replaced Plasma's (KDE is gone since 2026-10-24) and mako's.

## What it does
- Serves `org.freedesktop.Notifications` (Notify, CloseNotification, GetCapabilities, GetServerInformation, and the
  NotificationClosed / ActionInvoked signals). Any app that uses `notify-send` or libnotify just works.
- Keeps up to 200 notifications in `~/.local/state/yggdrasil/notifications.json` until you open or clear them (they survive a
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
  `notify_pane`, `heimdall_alerts` in `~/.config/yggdrasil/panel.conf`).

## Taking over from mako (and, before 2026-10-24, Plasma)
Only one program can own the name `org.freedesktop.Notifications`. What decides it:
- **Plasma** is gone, so it no longer competes. (Under KDE its server lived in the notifications applet of Plasma's system tray,
  and `labyrinth-panel-mode on` removed those panels so Plasma let go of the name; that script no longer exists.)
- **mako** is still installed and ships a D-Bus *activation* file for the name in `/usr/share/dbus-1/services`, so an early
  notification could wake it. Labyrinth OS therefore ships a file that comes first,
  `/usr/local/share/dbus-1/services/org.freedesktop.Notifications.service`, which runs `labyrinth-notify-activate`: it starts
  the panel if it is not running and waits until the panel owns the name. (A per-user copy in
  `~/.local/share/dbus-1/services/`, written by the old `labyrinth-panel-mode`, may still be there on machines that had KDE; it
  does the same.)
- The panel also asks to replace an owner that allows it and retries every 10 s.
- Tested on this machine: with the panel killed, `notify-send` started the panel through D-Bus and the notification landed. Not yet
  tested: a full fresh install from the ISO.

## Added 2026-10-16 (later)
- **Buttons on pop-ups:** an app's own actions (besides "default", which is a click on the pop-up) show as up to two buttons on its pop-up; pressing one tells the app (`ActionInvoked`) and removes the notification. Rendered and tested with `notify-send -A key=Label`; the click path itself was not exercised end to end yet. (This is the piece the future Allow / Deny prompts for apps build on.)
- **Do not disturb:** a switch at the bottom of the list and in Settings > Appearance > Notifications (`notify_dnd`). No pop-ups while on (critical ones still show); everything is still kept.
- **Unread badge:** a purple count beside the clock in the bar for notifications that arrived since the list was last looked at (Fylgja's Notifications section, or the pane, counts as looking). Click it to open the list. Kept in memory only.

## Not built yet (list for later)
- The theme option (macOS look vs a more Windows-like look) in Settings > Appearance.
- Inline reply. (Quiet hours and pop-ups by app: built 2026-11-07, below.)
- A notification sound; images/avatars sent by apps (`image-data`).
- Heimdall alerts de-duplicated when it replays old session alerts after a restart.

## Quiet hours and pop-ups by app (2026-11-07)

Settings > Apps > Notifications: **Quiet hours** (a daily stretch, e.g. 22:00 to 07:00, with no pop-ups, as Do not disturb) and
**Pop-ups by app** (a switch for every app that has sent a notification; a muted app's notifications are only kept in the
Notification Center). A critical notification always pops up, so muting an app never hides a security alarm or a low battery.
The rules are `~/.config/yggdrasil/notifications.conf` (`yggdrasil_conf::notifyrules`), read by the panel at each notification;
the apps seen are in `~/.local/state/yggdrasil/notification-apps`. Not yet seen on screen.

