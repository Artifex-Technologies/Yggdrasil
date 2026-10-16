#!/bin/bash
# Run inside a private session bus: dbus-run-session -- tests/test_keyring_sync.sh  (uses its own keyring and runtime folders; the real keyring is not touched)
# a private session bus, private keyring folder and runtime dir: nothing here touches the real keyring
T=${TMPDIR:-/tmp}/kr-test-$$; rm -rf $T; mkdir -p $T/data $T/run; chmod 700 $T/run
export XDG_DATA_HOME=$T/data XDG_RUNTIME_DIR=$T/run HOME=$T/home; mkdir -p $HOME
unset GNOME_KEYRING_CONTROL SSH_AUTH_SOCK
SYNC=${SYNC:-$(dirname "$0")/../packages/keyring-sync/target/debug/labyrinth-keyring-sync}
start() { setsid -f gnome-keyring-daemon --start --components=secrets >/dev/null 2>&1; sleep 2; }
stopd() { pid=$(pgrep -x -u $UID gnome-keyring-d | while read p; do grep -q "kr-test" /proc/$p/environ 2>/dev/null && echo $p; done); [ -n "$pid" ] && kill $pid; sleep 1; }
echo "--- 1: a keyring locked with the OLD password, with one item in it"
printf '%s' 'oldpass' | gnome-keyring-daemon --login >/dev/null 2>&1; sleep 1; start
printf 'item-secret' | secret-tool store --label="test item" application krtest 2>&1 | head -2
secret-tool lookup application krtest >/dev/null && echo "item stored"
echo "--- 2: restart without unlocking: the keyring is locked"
stopd; start
echo "state: $($SYNC --check)"
echo "--- 3: wrong previous password"
LABYRINTH_KEYRING_OLD=wrong LABYRINTH_KEYRING_CURRENT=newpass $SYNC --cli; echo "rc=$?"
echo "--- 4: the right previous password"
LABYRINTH_KEYRING_OLD=oldpass LABYRINTH_KEYRING_CURRENT=newpass $SYNC --cli; echo "rc=$?"
echo "--- 5: restart; unlock with the NEW password; the item is still there"
stopd; printf '%s' 'newpass' | gnome-keyring-daemon --login >/dev/null 2>&1; sleep 1; start
echo "state: $($SYNC --check)"
printf 'item-secret' | cmp -s - <(secret-tool lookup application krtest) && echo "item intact after the change"
echo "--- 6: the OLD password no longer opens it"
stopd; start
LABYRINTH_KEYRING_OLD=oldpass LABYRINTH_KEYRING_CURRENT=zzz $SYNC --cli; echo "rc=$? (1 means the old password was refused)"
stopd
