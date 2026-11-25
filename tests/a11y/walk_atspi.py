#!/usr/bin/env python3
"""An application's accessibility tree as a screen reader finds it: from the AT-SPI registry's desktop down, each node's role,
name, description and actions, read with plain D-Bus calls (python-dbus) on the accessibility bus that AT_SPI_BUS_ADDRESS names.

  walk_atspi.py APP               the tree of the application called APP, one node a line, indented by depth:
                                  role "name" - description actions=click
  walk_atspi.py APP --press NAME  do the first action of APP's first node called NAME, as a screen reader's "click" does

Exit status: 0 done, 1 no application (or no node with an action) of that name."""
import os
import sys

import dbus

ACCESSIBLE = "org.a11y.atspi.Accessible"
ACTION = "org.a11y.atspi.Action"
DESKTOP = ("org.a11y.atspi.Registry", "/org/a11y/atspi/accessible/root")
# libatspi's names for the roles of the panel's nodes; over D-Bus a role is only its number
ROLES = {23: "frame", 31: "list", 32: "list item", 43: "button", 63: "tool bar", 75: "application", 111: "log"}

bus = dbus.bus.BusConnection(os.environ["AT_SPI_BUS_ADDRESS"])


def call(ref, interface, method, *args):
    return bus.get_object(ref[0], ref[1], introspect=False).get_dbus_method(method, interface)(*args)


def prop(ref, interface, name):
    return call(ref, "org.freedesktop.DBus.Properties", "Get", interface, name)


def children(ref):
    return [(str(name), str(path)) for name, path in call(ref, ACCESSIBLE, "GetChildren")]


def actions(ref):
    try:
        count = int(prop(ref, ACTION, "NActions"))
    except dbus.DBusException:
        return []  # a node without the Action interface
    return [str(call(ref, ACTION, "GetName", i)) for i in range(count)]


def nodes(ref, depth=0):
    """(depth, ref, role, name, description, actions) of a node and of everything under it, in reading order."""
    role = int(call(ref, ACCESSIBLE, "GetRole"))
    name, description = str(prop(ref, ACCESSIBLE, "Name")), str(prop(ref, ACCESSIBLE, "Description"))
    yield depth, ref, ROLES.get(role, "role %d" % role), name, description, actions(ref)
    for child in children(ref):
        yield from nodes(child, depth + 1)


def name_of(ref):
    try:
        return str(prop(ref, ACCESSIBLE, "Name"))
    except dbus.DBusException:
        return None  # an application that went away


def main():
    if len(sys.argv) not in (2, 4) or (len(sys.argv) == 4 and sys.argv[2] != "--press"):
        sys.exit(__doc__)
    app = next((ref for ref in children(DESKTOP) if name_of(ref) == sys.argv[1]), None)
    if app is None:
        print("walk_atspi: the registry knows no application called %r" % sys.argv[1], file=sys.stderr)
        sys.exit(1)
    if len(sys.argv) == 4:
        for _, ref, _, name, _, acts in nodes(app):
            if name == sys.argv[3] and acts:
                call(ref, ACTION, "DoAction", 0)
                return
        print("walk_atspi: %s has no node called %r with an action" % (sys.argv[1], sys.argv[3]), file=sys.stderr)
        sys.exit(1)
    for depth, _, role, name, description, acts in nodes(app):
        description = " - " + description if description else ""
        print('%s%s "%s"%s%s' % ("  " * depth, role, name, description, " actions=" + ",".join(acts) if acts else ""))


main()
