#!/bin/sh
# gpuunlock-sandbox.sh ROOT USER COMMAND...: runs COMMAND with ROOT as its whole file system, for gpuunlock.py. It is started
# inside `unshare -rmnpf` (user, mount, network and process namespaces of its own: "root" there is this user, the mounts are
# seen by this process tree only and go away with it, the network is a loopback of its own and nothing else), so nothing
# outside ROOT is read or changed: not /boot, /etc/default/grub, /sys, the graphics card or the network.
#
# ROOT has the made-up /etc, /var, /boot, /sys, /opt and /home; the system's /usr comes in read-only (the programs, Python and
# the libraries), with ROOT/.usr's yggdrasil folders and firmware over its own; /proc is a new one with ROOT/.proc's command
# line, boot id and driver files over it; /dev is the system's. Then ROOT becomes / (pivot_root) and the rest of the system
# is let go. USER is root (uid 0 of the namespace) or user (uid 1000 of one more namespace inside it).
set -e
r=$1
user=$2
shift 2
mount --bind "$r" "$r"
mount --rbind /usr "$r/usr"
mount -o remount,bind,ro "$r/usr"
for d in share/yggdrasil lib/yggdrasil lib/firmware; do
  mount --bind "$r/.usr/$d" "$r/usr/$d"
done
mount --rbind /dev "$r/dev"
mount -t proc proc "$r/proc"
mount --bind "$r/.proc/cmdline" "$r/proc/cmdline"
mount --bind "$r/.proc/boot_id" "$r/proc/sys/kernel/random/boot_id"
mount --bind "$r/.proc/driver" "$r/proc/driver"
mount -t tmpfs tmpfs "$r/tmp"
ip link set lo up
cd "$r"
pivot_root . .old
umount -l /.old
cd /
umask 022
# a stand-in for Ollama on its port, when the case has an answer for it
if [ -f /case/ollama.answer ]; then
  python3 -I /standins/ollama-server.py &
  i=0
  while [ ! -e /tmp/ollama-ready ] && [ $i -lt 100 ]; do sleep 0.05; i=$((i + 1)); done
fi
if [ "$user" = user ]; then
  exec unshare --user --map-user=1000 --map-group=1000 -- "$@"
fi
exec "$@"
