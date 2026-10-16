import os, sys, dbus
addr = os.environ["AT_SPI_BUS_ADDRESS"]
bus = dbus.bus.BusConnection(addr)
def acc(name, path): return dbus.Interface(bus.get_object(name, path, introspect=False), "org.a11y.atspi.Accessible")
def props(name, path): return dbus.Interface(bus.get_object(name, path, introspect=False), "org.freedesktop.DBus.Properties")
def walk(name, path, d=0):
    p = props(name, path)
    n = str(p.Get("org.a11y.atspi.Accessible", "Name")); desc = str(p.Get("org.a11y.atspi.Accessible", "Description"))
    try: role = str(acc(name, path).GetRoleName())
    except Exception: role = "role" + str(int(acc(name, path).GetRole()))
    actions = ""
    try:
        a = dbus.Interface(bus.get_object(name, path, introspect=False), "org.a11y.atspi.Action")
        k = int(props(name, path).Get("org.a11y.atspi.Action", "NActions"))
        actions = " actions=" + ",".join(str(a.GetName(i)) for i in range(k))
    except Exception: pass
    print("%s%s \"%s\"%s%s" % (" " * d, role, n, (" - " + desc) if desc else "", actions))
    for (cn, cp) in acc(name, path).GetChildren():
        walk(str(cn), str(cp), d + 2)
names = [str(n) for n in bus.list_names() if str(n).startswith(":")]
print("connections:", names)
dbusif = dbus.Interface(bus.get_object("org.freedesktop.DBus", "/org/freedesktop/DBus", introspect=False), "org.freedesktop.DBus")
for n in names:
    try:
        pid = int(dbusif.GetConnectionUnixProcessID(n)); print("  ", n, pid, open("/proc/%d/comm" % pid).read().strip())
    except Exception as e: print("  ", n, "?", e)
for n in names:
    try:
        pass
        nm = str(props(n, "/org/a11y/atspi/accessible/root").Get("org.a11y.atspi.Accessible", "Name"))
    except Exception:
        continue
    print("app", n, repr(nm), "pid", dbus.Interface(bus.get_object("org.freedesktop.DBus", "/org/freedesktop/DBus", introspect=False), "org.freedesktop.DBus").GetConnectionUnixProcessID(n))
    if "panel" in nm.lower() or "labyrinth" in nm.lower():
        walk(n, "/org/a11y/atspi/accessible/root")
