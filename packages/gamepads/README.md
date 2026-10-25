# yggdrasil-gamepads

A small Rust library for game controllers, used by Settings (Bluetooth > Controllers) and the Controllers top bar plugin. It reads
what the kernel's own drivers publish in sysfs and writes the LED files they offer; it never talks to a controller in its own
protocol.

- `list()`: the connected controllers (input devices with a `js` node, or that udev marks as joysticks), with model (DualShock 4,
  DualSense, DualSense Edge, Xbox, Switch Pro, other), USB or Bluetooth, address, driver, event device, battery (percentage or
  level, and charging state) and lights (light bar, player lights, and whether they can be written).
- `set_lightbar`, `set_player` (the PS5's player light patterns), `rumble` (a force-feedback test, the way games make it rumble).
- `disconnect`, `paired_gamepads` (Bluetooth, through `bluetoothctl`).
- `Settings` in `~/.config/yggdrasil/controllers.json`: a light bar colour for all, or per controller, a player number, notifications
  and the low-battery level.

Drivers: the kernel drives the DualShock 4 and DualSense itself (`hid-playstation`, or `hid-sony` on older kernels), with battery
and lights, so [ds4drv](https://github.com/chrippa/ds4drv) (a userspace driver from before that) is not used. Xbox controllers work
over USB with `xpad`; over Bluetooth [xpadneo](https://github.com/atar-axis/xpadneo) (a separate kernel driver, the
`xpadneo-dkms` package) adds rumble and the battery level.

Ideas and sysfs details from [Bluechip](https://github.com/chizzler/Bluechip), [bt-battery](https://github.com/JosephDubon993/bt-battery)
and [omarchy-plugin-dualsense](https://github.com/atoslins/omarchy-plugin-dualsense) (all MIT); the code here is written anew.
