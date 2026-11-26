# Cloud drives (yggdrasil-clouddrives)

The cloud drives that Settings > Cloud Storage adds, mounts and removes, and that Minos lists under Cloud
Storage in its sidebar. A library shared by both; it has no window of its own.

## What a drive is

Linux has no single cloud API, so a drive is one of two things:

- **An rclone remote mounted as a folder.** rclone signs in to the provider and `rclone mount NAME: FOLDER --vfs-cache-mode
  full --dir-cache-time 5m` shows its files as a folder (`~/Cloud/NAME` unless another one is chosen). The mount runs as a
  systemd *user* service, `~/.config/systemd/user/yggdrasil-cloud-NAME.service`, so it stays up when Minos and Settings are
  closed. That file is written only when Mount or "Mount at sign-in" is used for the first time, and removed with the drive.
  Proton Drive's remote `proton` is mounted by the service that ships with Minos (`proton-drive.service`, at `~/ProtonDrive`), so
  Minos's own Proton Drive sign-in and this list agree.
- **A folder a sync app keeps** (the Nextcloud or Proton Drive desktop app's folder): listed, never mounted.

## Signing in

The sign-in is always the provider's own, through rclone; no account of ours is involved.

| Provider | rclone type | How |
| --- | --- | --- |
| Google Drive, OneDrive, Dropbox, Box | `drive`, `onedrive`, `dropbox`, `box` | `rclone config create NAME TYPE`: the provider's page opens in the browser; Settings shows what rclone prints while it waits, and Cancel stops it |
| Proton Drive, WebDAV / Nextcloud, S3 | `protondrive`, `webdav`, `s3` | `rclone config create NAME TYPE --all` in a terminal: rclone asks for the user name, password, two-factor code or keys itself, so they never pass through Settings or a command line; Settings waits until the remote is there |
| An existing rclone remote | any | added to the list as it is |

A remote's name is 1 to 32 letters, digits, `-` and `_` (it also names the unit and the folder).

## The list

`~/.config/yggdrasil/cloud-drives.conf`, in the folder Minos already watches:

```
[remote GoogleDrive]
type = drive
mount = /home/me/Cloud/GoogleDrive

[folder Nextcloud]
path = /home/me/Nextcloud
```

Proton Drive is not in the file: Settings shows it whenever rclone has the `proton` remote, and Minos has its own row for it.
A section with a name or folder that is not allowed (a relative path, `..`, quotes, backslashes) is skipped when read.

## State

Mounted (the folder is in `/proc/self/mountinfo`), Mounting (the unit is starting), Error (the unit failed), Not signed in (a
browser sign-in that was not finished: the remote has no token), Not mounted; a sync app's folder is Ready or Folder missing.
Settings reads the mount table and one `systemctl --user show` every 3 seconds while its page is open and in front, and
rclone's remotes when the page opens, after a change and while a sign-in is awaited. Minos reads only the mount table, on a
thread every 3 seconds while it has a remote to watch, and draws again only when something changed; it never starts rclone
for the sidebar.

## Removing

Remove asks first, then stops and disables the unit, deletes the unit file (never Minos's Proton Drive unit), deletes the rclone
remote (`rclone config delete NAME`), removes the mount folder only if it is empty (`rmdir`), and takes the drive off the list.
Files in the cloud, and files in a sync app's folder, are never touched. A drive still mounted (a program has a file open in
it) is not removed; it says so.

## When rclone is missing

The Settings page says so and shows the install command (`sudo pacman -S rclone` on Arch-based systems); nothing is installed
for you. Sync app folders can still be added.

## Tests

`cargo test` here: the list read back as written and bad entries left out, `rclone config dump` and `rclone listremotes`
(short and long) parsed, the mount unit's text (quoting, systemd's `%` and `$`), the state from the mount table, the unit and
the token, `systemctl show` read per unit, and mount, "at sign-in" and remove run against a stand-in system in a temporary
home: they never start rclone or systemctl and never touch the real `~/.config`.
