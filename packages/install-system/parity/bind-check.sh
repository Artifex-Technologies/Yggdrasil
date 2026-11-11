#!/bin/sh
# bind-check.sh FOLDER: what the install program counts on, checked with this computer's own mount(8), on tmpfs folders under
# FOLDER inside a throwaway namespace (it is started inside `unshare -rm --propagation private`; no real device, nothing of
# the computer's own is bound). A shared "live /dev" with /pts and /shm mounted under it is bound into a "new system" three
# ways, and the new system is unmounted with umount -R each time. It prints how many of the live submounts are left:
#   1. --rbind, then umount -R                                   (the hazard: usually 0, umount -R reached the live ones)
#   2. --rbind --make-rslave in one call, then umount -R          (what the program does: 2)
#   3. --rbind, mount --make-rslave of the new system, umount -R  (the cleanup's guard after a bind that is not a slave: 2)
set -e
f=$1
PATH=/usr/bin:/bin
mkdir -p "$f/live" "$f/new"
mount -t tmpfs live "$f/live"
mount --make-shared "$f/live"
mkdir -p "$f/live/pts" "$f/live/shm"
left() { grep -c " $f/live/" /proc/self/mountinfo || true; }
fresh() {
  mountpoint -q "$f/live/pts" || mount -t tmpfs pts "$f/live/pts"
  mountpoint -q "$f/live/shm" || mount -t tmpfs shm "$f/live/shm"
  mount -t tmpfs new "$f/new"
  mkdir -p "$f/new/dev"
}
fresh; mount --rbind "$f/live" "$f/new/dev"; umount -R "$f/new"; a=$(left)
fresh; mount --rbind --make-rslave "$f/live" "$f/new/dev"; umount -R "$f/new"; b=$(left)
fresh; mount --rbind "$f/live" "$f/new/dev"; mount --make-rslave "$f/new"; umount -R "$f/new"; c=$(left)
echo "$a $b $c"
