#!/bin/sh
# sandbox.sh WORLD COMMAND...: runs COMMAND with a few of the system's fixed files replaced by WORLD's, for compare.py. It is
# started inside `unshare -rm` (a user and mount namespace of its own: "root" there is this user, and the mounts are seen by
# this process tree only and go away with it), so nothing outside WORLD is changed:
#   WORLD/mkinitcpio.conf -> /etc/mkinitcpio.conf,  WORLD/cmdline -> /proc/cmdline,  WORLD/sysmodule/ -> /sys/module/
w=$1
shift
[ -f "$w/mkinitcpio.conf" ] && { mount --bind "$w/mkinitcpio.conf" /etc/mkinitcpio.conf || exit 99; }
[ -f "$w/cmdline" ] && { mount --bind "$w/cmdline" /proc/cmdline || exit 99; }
[ -d "$w/sysmodule" ] && { mount --bind "$w/sysmodule" /sys/module || exit 99; }
exec "$@"
