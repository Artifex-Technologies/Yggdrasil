#!/usr/bin/env python3
"""A fake StatusNotifierItem for testing the tray: an Active item with a red square icon sent as pixels; logs clicks."""
import dbus, dbus.service, dbus.mainloop.glib, sys
from gi.repository import GLib

dbus.mainloop.glib.DBusGMainLoop(set_as_default=True)
bus = dbus.SessionBus()
name = dbus.service.BusName("org.kde.StatusNotifierItem-fake-1", bus)

# 22x22 ARGB32: opaque red square with a white dot
w = h = 22
px = bytearray()
for y in range(h):
    for x in range(w):
        dot = (x - 11) ** 2 + (y - 11) ** 2 < 16
        px += bytes([255, 255, 255, 255]) if dot else bytes([255, 220, 40, 40])


class Item(dbus.service.Object):
    @dbus.service.method("org.kde.StatusNotifierItem", in_signature="ii")
    def Activate(self, x, y):
        print("Activate", x, y, flush=True)

    @dbus.service.method("org.kde.StatusNotifierItem", in_signature="ii")
    def ContextMenu(self, x, y):
        print("ContextMenu", x, y, flush=True)

    @dbus.service.method(dbus.PROPERTIES_IFACE, in_signature="ss", out_signature="v")
    def Get(self, iface, prop):
        return self.GetAll(iface)[prop]

    @dbus.service.method(dbus.PROPERTIES_IFACE, in_signature="s", out_signature="a{sv}")
    def GetAll(self, iface):
        return {
            "Id": "fake", "Title": "Fake tray item", "Status": "Active", "IconName": "",
            "IconThemePath": "", "Category": "ApplicationStatus",
            "IconPixmap": dbus.Array([dbus.Struct((dbus.Int32(w), dbus.Int32(h), dbus.Array([dbus.Byte(b) for b in px], signature="y")), signature="iiay")], signature="(iiay)"),
        }


Item(bus, "/StatusNotifierItem")
watcher = bus.get_object("org.kde.StatusNotifierWatcher", "/StatusNotifierWatcher")
watcher.RegisterStatusNotifierItem("org.kde.StatusNotifierItem-fake-1", dbus_interface="org.kde.StatusNotifierWatcher")
print("registered", flush=True)
GLib.timeout_add_seconds(int(sys.argv[1]) if len(sys.argv) > 1 else 60, lambda: sys.exit(0))
GLib.MainLoop().run()
