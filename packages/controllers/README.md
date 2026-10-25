# yggdrasil-controllers

Game controllers in Yggdrasil's top bar (an Agora plugin). While a controller is connected the bar shows its battery (the lowest,
with several), amber when low and red when nearly empty. Its card lists each controller (model, USB or Bluetooth, battery with a
meter) and has a rumble test, Disconnect (Bluetooth), light bar colours (Purple, Blue, Red, Green, White, Off; one colour for every
controller, kept and put back each time a controller connects), and a link to Settings > Bluetooth.

    yggdrasil-controllers daemon      the user service: keeps the bar's file current, restores light bar colours, and notifies
                                      when a controller connects or disconnects, runs low or is charged
    yggdrasil-controllers --bar       prints what the bar shows
    yggdrasil-controllers press ID    the card's buttons (rumble:N, disconnect:MAC, light:NAME, settings)

The daemon only reads sysfs: every 5 seconds while a controller is connected, every 10 otherwise.

`70-yggdrasil-controllers.rules` lets the people using the computer set the lights of controllers on the PlayStation drivers (the
light bar and player lights' brightness and colour files only). The rumble test uses the controller's event device, which the
logged-in user may already use (udev's `uaccess`).

The kernel drives the DualShock 4 and DualSense itself, so [ds4drv](https://github.com/chrippa/ds4drv) is not used. For Xbox
controllers over Bluetooth, [xpadneo](https://github.com/atar-axis/xpadneo) (the optional `xpadneo-dkms` driver) adds rumble and
the battery level. Built on `yggdrasil-gamepads` (packages/gamepads). Ideas from
[Bluechip](https://github.com/chizzler/Bluechip), [bt-battery](https://github.com/JosephDubon993/bt-battery) and
[omarchy-plugin-dualsense](https://github.com/atoslins/omarchy-plugin-dualsense) (MIT).

| file | installed as |
|---|---|
| `target/release/yggdrasil-controllers` | `/usr/bin/yggdrasil-controllers` |
| `controllers.conf` | `/usr/share/yggdrasil/integrations/controllers.conf` |
| `yggdrasil-controllers.service` | `/usr/lib/systemd/user/yggdrasil-controllers.service` (enabled for every user) |
| `70-yggdrasil-controllers.rules` | `/usr/lib/udev/rules.d/70-yggdrasil-controllers.rules` |
| `README.md` | `/usr/share/doc/yggdrasil-controllers/README.md` |
