# Device offers (`yggdrasil-device-offers`)

The software for this computer's devices, offered by the installed system itself. A computer installed without internet, or with
the device software switched off in the installer, gets it later: once it is online, when such a device is plugged in, and at
sign-in, one notification names what is missing.

- **Install** runs the operating system's component command (`labyrinth-apps install razer ...` on Labyrinth). Root steps go
  through Agora's password box. A notification says when it is done or what failed (details in
  `~/.local/state/yggdrasil/device-offers.log`).
- **Don't ask again** is kept in `~/.local/state/yggdrasil/device-offers-never`. Agora still installs it.
- Closing the notification means not now. Each offer is asked once a session (`$XDG_RUNTIME_DIR/yggdrasil/device-offers-asked`).

Nothing is installed without the click. Offline nothing is asked at all.

## What is offered

The OS's own device list, named by `devices` in its `packages` integration (docs/INTEGRATIONS.md). On Labyrinth that is the
installer's list, `installer-extras.json`, so the vendors and devices are written down in one place. An entry is offered when
this computer asks for it, by the same code the installer ticks it with (this crate's library):

- `chosen_if_usb`: a USB device plugged in, by exact `vendor:product`, or by vendor. A vendor alone counts only for a device of the
  entry's `usb_kind` when it gives one:
  - `keyboard`, `mouse`, or `input` (either);
  - `printer`;
  - `gamepad`.
  These are read from the device's USB interfaces under /sys. So a Corsair fan controller is not a Corsair keyboard, and a HyperX
  mouse (HP's vendor id) is not an HP printer.
- `chosen_if_machine`: what the computer is (processor maker, battery, tablet-mode switch, a path that must not exist).

An entry is also skipped when:

- its components are all installed (every package of each, as Agora counts them);
- a component is unknown;
- it is in the OS's `first_login` list for this user, since the OS installs those by itself.

## What starts it

Nothing stays running and nothing polls. systemd starts the check as a short user service in the signed-in user's session; it
looks once and exits.

| Event | How |
| --- | --- |
| Sign-in | `graphical-session.target.wants/yggdrasil-device-offers.service` |
| A USB device plugged in | `71-yggdrasil-device-offers.rules`: `SYSTEMD_USER_WANTS` on a USB device's `add` |
| Online | NetworkManager's dispatcher runs `device-offers-online` (as root) on "up", or "connectivity-change" to FULL. It starts `yggdrasil-online.target` in each signed-in user's service manager, as that user (`--no-block`). The target stops again at once (`StopWhenUnneeded`, like sleep.target), so the next start starts what it wants again. |

`yggdrasil-online.target` is also there for the OS. Labyrinth links its first-login installer
(`labyrinth-first-components.service`) into `yggdrasil-online.target.wants`, so parts chosen in the installer are installed once
the computer gets online, not only at the next sign-in.

### Online without waiting

"Online" is decided without waiting:

- NetworkManager's last answer (`nmcli -t networking connectivity`: `full`);
- without NetworkManager, a default route in this network namespace's routing table.

## Files

| Installed as | From |
| --- | --- |
| /usr/lib/yggdrasil/device-offers | `yggdrasil-device-offers` (src/main.rs) |
| /usr/lib/yggdrasil/device-offers-online, linked as /usr/lib/NetworkManager/dispatcher.d/90-yggdrasil-online | src/bin/online.rs |
| /usr/lib/udev/rules.d/71-yggdrasil-device-offers.rules | data/ |
| /usr/lib/systemd/user/yggdrasil-device-offers.service, yggdrasil-online.target | data/ |

They are part of the `yggdrasil-agora` package. `yggdrasil-device-offers --print` lists what would be offered now: id and
components, one per line. It asks nothing and does not check for internet.

## Tests

`cargo test` runs these, with no real network, notification, package or service:

- the library's matching on made-up /sys trees;
- both programs on a made-up computer with stand-ins for nmcli, pacman, notify-send (which "clicks" a button), the component command,
  the password box and systemctl;
- the no-network case once more inside `unshare -rn`, skipped where user namespaces are not allowed;
- the udev rule, checked with `udevadm verify`.

## Not done

- Bluetooth-only devices are not seen: nothing is plugged in by USB.
- Settings > Peripherals does not list the offers yet, and "Don't ask again" cannot be undone from Settings: Agora installs them
  anyway, or delete the line from the file above.
- Not yet tried on a real installed system. The udev and dispatcher triggers need root to install, so only the programs and files
  are tested here.
