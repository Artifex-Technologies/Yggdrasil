#!/bin/sh
# sandbox.sh WORLD COMMAND...: runs the install program (COMMAND) on a made-up computer, for compare.py. It is started inside
# `unshare -r -m` (a user and mount namespace of its own: "root" there is this user, with no power over the real computer, and
# the mounts below are seen by this process tree only and go away with it). Nothing outside WORLD is changed:
#   /dev, /mnt, /run, /sys/firmware             empty tmpfs (no real device can be named: /dev holds nothing at all)
#   /usr/share/yggdrasil/integrations, /etc/yggdrasil    tmpfs with WORLD's installer integration files
#   /etc/os-release, /etc/passwd, /etc/resolv.conf       WORLD's own
# then WORLD/root/{run,mnt,firmware,share-integrations,etc-yggdrasil} is copied into place. COMMAND runs with PATH=$FAKEPATH
# (the stand-ins in fake.py; nothing else can be run by name). Afterwards what is in /mnt and /run is written down in
# WORLD/snapshot.json (snapshot.py), and the exit code is COMMAND's. With NONROOT=1 it runs as uid 1000 of a namespace inside.
w=$1
shift
here=$(dirname "$0")
PATH=/usr/bin:/bin
for d in /dev /mnt /run /sys/firmware /usr/share/yggdrasil/integrations /etc/yggdrasil; do
  mount -t tmpfs -o mode=755 tmpfs "$d" || exit 99
done
for f in os-release passwd resolv.conf; do
  mount --bind "$w/etc/$f" "/etc/$f" || exit 99
done
cp -a "$w/root/run/." /run/ && cp -a "$w/root/mnt/." /mnt/ && cp -a "$w/root/firmware/." /sys/firmware/ \
  && cp -a "$w/root/share-integrations/." /usr/share/yggdrasil/integrations/ && cp -a "$w/root/etc-yggdrasil/." /etc/yggdrasil/ || exit 99
# the real devices are out of sight: refuse to go on if one is not
[ -z "$(ls -A /dev)" ] || exit 98
if [ "${NONROOT:-}" = 1 ]; then
  PATH="$FAKEPATH" /usr/bin/unshare --map-user=1000 --map-group=1000 -- "$@"
else
  PATH="$FAKEPATH" "$@"
fi
code=$?
/usr/bin/python3 -I "$here/snapshot.py" /mnt /run > "$w/snapshot.json" || exit 97
exit $code
