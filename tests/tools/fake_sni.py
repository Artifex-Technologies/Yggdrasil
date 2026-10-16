#!/usr/bin/env python3
"""A fake StatusNotifierItem for testing the tray: an item with a red square icon sent as pixels and a small menu (dbusmenu); logs clicks.

  fake_sni.py [seconds] [passive]     passive: Status "Passive" (shown only in the tray's hidden-icons list)"""
import dbus, dbus.service, dbus.mainloop.glib, sys
from gi.repository import GLib

dbus.mainloop.glib.DBusGMainLoop(set_as_default=True)
bus = dbus.SessionBus()
passive = len(sys.argv) > 2 and sys.argv[2] == "passive"
unique = "fake-p" if passive else "fake-1"
name = dbus.service.BusName("org.kde.StatusNotifierItem-%s" % unique, bus)

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
            "Id": "fake", "Title": "Fake tray item" + (" (hidden)" if passive else ""), "Status": "Passive" if passive else "Active", "IconName": "", "Menu": dbus.ObjectPath("/Menu"),
            "IconThemePath": "", "Category": "ApplicationStatus",
            "IconPixmap": dbus.Array([dbus.Struct((dbus.Int32(w), dbus.Int32(h), dbus.Array([dbus.Byte(b) for b in px], signature="y")), signature="iiay")], signature="(iiay)"),
        }


class Menu(dbus.service.Object):
    """A tiny dbusmenu: Play, a separator, a ticked Repeat, a disabled entry, and a submenu with one entry."""
    def entry(self, i, props, kids=()):
        return dbus.Struct((dbus.Int32(i), dbus.Dictionary(props, signature="sv"), dbus.Array([dbus.Struct(k, signature="ia{sv}av") if False else k for k in kids], signature="v")), signature="ia{sv}av")

    @dbus.service.method("com.canonical.dbusmenu", in_signature="i", out_signature="b")
    def AboutToShow(self, i):
        return False

    @dbus.service.method("com.canonical.dbusmenu", in_signature="iias", out_signature="u(ia{sv}av)")
    def GetLayout(self, parent, depth, props):
        e = self.entry
        kids = [
            e(1, {"label": "_Play"}),
            e(2, {"type": "separator"}),
            e(3, {"label": "Repeat", "toggle-type": "checkmark", "toggle-state": dbus.Int32(1)}),
            e(4, {"label": "Unavailable", "enabled": False}),
            e(5, {"label": "More", "children-display": "submenu"}, [e(6, {"label": "Inner entry"})]),
        ]
        return dbus.UInt32(1), e(0, {}, kids)

    @dbus.service.method("com.canonical.dbusmenu", in_signature="isvu", out_signature="")
    def Event(self, i, kind, data, ts):
        print("MenuEvent", int(i), str(kind), flush=True)


Menu(bus, "/Menu")
Item(bus, "/StatusNotifierItem")
watcher = bus.get_object("org.kde.StatusNotifierWatcher", "/StatusNotifierWatcher")
watcher.RegisterStatusNotifierItem("org.kde.StatusNotifierItem-%s" % unique, dbus_interface="org.kde.StatusNotifierWatcher")
print("registered", flush=True)
GLib.timeout_add_seconds(int(sys.argv[1]) if len(sys.argv) > 1 else 60, lambda: sys.exit(0))
GLib.MainLoop().run()
