# Game time

I play games and run a local AI model on the same computer, and I want every bit of performance for them. So while a game
runs, background updates and chores wait and catch up afterwards. Security that watches keeps running: "leak watch and virus
scans should still work during a game. Definition updates can wait till after or before a game."

How it works, and what counts as a game (Steam's launcher, gamescope, Lutris, and names I add in
`/etc/yggdrasil/gametime.conf`): [packages/gametime](../packages/gametime/README.md). In short, systemd asks
`yggdrasil-gametime no-game` before each chore below starts. During a game the chore is skipped and noted, and a timer starts
it within 5 minutes of the game ending. Nothing stays running.

## What waits

| Chore | Manager | What it is | Why it can wait |
| --- | --- | --- | --- |
| `clamav-freshclam-once.service` | system | new virus definitions (every 2 hours; replaces the freshclam daemon) | the definitions from 2 hours ago still catch nearly everything; scanning itself keeps running |
| `heimdall-intel-update.service` | system | Heimdall's IP databases, threat lists and the security tracker | daily lists; a few hours late changes nothing |
| `heimdall-dns-blocklist.service` | system | the DNS block list (malware, phishing, scams) | the list in use keeps blocking; only the refresh waits. It also reloads NetworkManager's DNS, which I don't want mid-game |
| `heimdall-inventory.service` | system | the daily inventory of programs outside the packages | reads many files: disk and page cache a game wants |
| `fstrim.service` | system | the weekly SSD trim | heavy disk work, can stall loading |
| `archlinux-keyring-wkd-sync.service` | system | the weekly refresh of the Arch packagers' keys | only matters at the next update |
| `snapper-cleanup.service` | system | deleting old restore points | btrfs deletions are heavy disk work |
| `yggdrasil-weather-record.service` | user | the hourly forecast recording | a late record is still a record |
| `yggdrasil-weather-nowcast.service` | user | the rain warning, every 10 minutes | no rain pop-ups over a game; it checks again right after |
| `labyrinth-publish.service` | user | building and publishing Labyrinth's packages | hours of compiling. Only its start waits: a build already running when a game starts keeps going (at nice 15 and idle disk priority) |
| `labyrinth-sarina-checkup.service` | user | Sarina's daily check-up | it would load the model onto the graphics card |

## What never waits

| Unit | Why |
| --- | --- |
| `yggdrasil-leakwatch` | the leak watch: a leak in a game or the desktop matters most during a long session |
| `labyrinth-scan`, `labyrinth-scan-watch`, `labyrinth-scan-full` | the virus scans: a file downloaded during a game is checked at once |
| `heimdall` (heimdalld) | the security watcher itself |
| `heimdall-lan-scan` | who is on the home network; an intruder doesn't wait for my game to end |
| `labyrinth-sarina-held` | tells me when a program is broken because Heimdall holds its file |
| `snapper-timeline` | the hourly restore points: small, and the thing that saves me after a bad change |
| `sarina-night`, `labyrinth-sarina-reflect` | they already wait for games themselves |

## Virus definitions

ClamAV's `clamav-freshclam.service` is a daemon that keeps running and checks on its own clock, so it can't wait for a game.
Game time replaces it with ClamAV's own `clamav-freshclam-once.timer`, made to run every 2 hours (the daemon's default) with up
to 10 minutes of random delay, and its run waits during games. Installing or upgrading the package makes the switch; removing
the package puts the daemon back. ClamAV's settings in `/etc/clamav` are unchanged.

## Adding a chore

A drop-in `/etc/systemd/system/UNIT.d/gametime.conf` (or `/etc/systemd/user/…` for a user unit), then `systemctl daemon-reload`:

    [Service]
    ExecCondition=+/usr/bin/yggdrasil-gametime no-game %n

Only for a chore whose run can be late. Never for something that protects the computer while I play.
