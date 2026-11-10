#!/bin/sh
# sandbox.sh WORLD COMMAND...: for the cancel tests and the parity runs. Started inside `unshare -rm` (a user and mount
# namespace of its own, gone with the process tree): an empty /run/user with WORLD/agora as /run/user/1000/agora, and
# WORLD/cache as pacman's /var/cache/pacman/pkg, so nothing of the real ones is seen or touched.
w=$1
shift
mount -t tmpfs none /run/user || exit 99
mkdir -p /run/user/1000/agora || exit 99
mount --bind "$w/agora" /run/user/1000/agora || exit 99
mount --bind "$w/cache" /var/cache/pacman/pkg || exit 99
exec "$@"
