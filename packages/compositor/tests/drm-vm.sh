#!/bin/sh
# The hardware backend's test machine: an Arch guest with a virtual GPU, keyboard and tablet, where `yggdrasil-compositor --drm`
# runs on a real (virtual) screen and virtual terminal. Never on the desktop's own GPU: the guest has its own.
#
# Runs as you with plain QEMU (no root, no libvirt; /dev/kvm through the kvm group), memory capped (QEMU -m 4G inside a 6 GB
# scope). The disk is an overlay on Limbo's Arch cloud image (read only), with cloud-init making the user `ygg` (ssh key of its
# own, sudo). Everything lives in $YGG_DRM_VM (default ~/.cache/yggdrasil-drm-vm).
#
#   drm-vm.sh boot [gl|2d]     start the guest in the background (gl: virtio-vga-gl, virgl through the host's render node;
#                              2d: virtio-vga with no 3D, so the compositor draws in software)
#   drm-vm.sh provision        once, with the guest's network: mesa, seatd (libseat), libinput, polkit (logind's VT switch),
#                              foot, XWayland, fonts, wlr-randr; tty1 logs `ygg` in by itself and starts ~/drm-start
#   drm-vm.sh deploy           copy the release builds (compositor, panel, Settings) into the guest's ~/bin
#   drm-vm.sh start [ARGS]     start the compositor on tty1 through logind: --drm ARGS (default: the panel and Settings);
#                              YGG_DRM_ENV="VAR=value ..." is set for it (YGGDRASIL_COMPOSITOR_RENDERER=pixman, ..._DEBUG=1)
#   drm-vm.sh ctl COMMAND      the running compositor's control socket (outputs, windows, quit, ...)
#   drm-vm.sh shot NAME [KEYS [WAIT]]   press KEYS (QEMU names: ctrl-alt-f2), wait, and save the screen as $YGG_DRM_VM/shots/NAME.png
#   drm-vm.sh type TEXT | click X Y | move X Y   keyboard and tablet input, as a person gives it
#   drm-vm.sh measure [SECONDS]  the compositor's CPU (all threads), wake-ups and memory while idle
#   drm-vm.sh ssh CMD          run a command in the guest;  drm-vm.sh stop: power it off
set -eu
HERE=$(cd "$(dirname "$0")" && pwd)
VM=${YGG_DRM_VM:-$HOME/.cache/yggdrasil-drm-vm}
BASE=${YGG_DRM_VM_BASE:-/var/lib/limbo/images/arch-cloudimg.qcow2}
TARGET=${CARGO_TARGET_DIR:-$HERE/../../../target}/release
mkdir -p "$VM/shots"

ssh_() { ssh -q -i "$VM/key" -p 2222 -o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null -o ConnectTimeout=5 ygg@127.0.0.1 "$@"; }

make_disk() {
  [ -f "$VM/disk.qcow2" ] && return
  [ -r "$BASE" ] || { echo "no base image $BASE (Limbo's Arch cloud image; YGG_DRM_VM_BASE names another)" >&2; exit 1; }
  ssh-keygen -q -t ed25519 -N '' -C ygg-drm-vm -f "$VM/key"
  qemu-img create -q -f qcow2 -F qcow2 -b "$BASE" "$VM/disk.qcow2" 20G
  mkdir -p "$VM/seed"
  printf 'instance-id: ygg-drm-1\nlocal-hostname: yggvm\n' > "$VM/seed/meta-data"
  printf '#cloud-config\nusers:\n  - name: ygg\n    groups: [wheel, video, input]\n    shell: /bin/bash\n    sudo: "ALL=(ALL) NOPASSWD:ALL"\n    lock_passwd: true\n    ssh_authorized_keys:\n      - %s\nssh_pwauth: false\n' \
    "$(cat "$VM/key.pub")" > "$VM/seed/user-data"
  xorriso -as mkisofs -quiet -o "$VM/seed.iso" -V cidata -J -r "$VM/seed/"
}

case "${1:-}" in
boot)
  make_disk
  if [ "${2:-gl}" = gl ]; then
    gpu="-device virtio-vga-gl,xres=1920,yres=1080 -display egl-headless,rendernode=${YGG_DRM_VM_RENDERNODE:-/dev/dri/renderD128}"
  else
    gpu="-device virtio-vga,xres=1920,yres=1080 -display none"
  fi
  rm -f "$VM/qmp.sock" "$VM/vnc.sock"
  # shellcheck disable=SC2086
  systemd-run --user --scope -q -p MemoryMax=6G -p MemorySwapMax=0 -p CPUQuota=400% \
    nice -n 10 qemu-system-x86_64 -enable-kvm -cpu host -smp 4 -m 4G -machine q35 \
    -drive file="$VM/disk.qcow2",if=virtio,format=qcow2 -drive file="$VM/seed.iso",media=cdrom,readonly=on \
    $gpu -vnc unix:"$VM/vnc.sock" -device qemu-xhci -device usb-kbd -device usb-tablet \
    -nic user,model=virtio-net-pci,hostfwd=tcp:127.0.0.1:2222-:22 \
    -qmp unix:"$VM/qmp.sock",server,nowait -serial file:"$VM/serial.log" -name ygg-drm-vm \
    > "$VM/qemu.log" 2>&1 &
  echo "booting; ssh answers in about a minute (drm-vm.sh ssh true)"
  ;;
provision)
  ssh_ 'sudo pacman-key --init >/dev/null 2>&1; sudo pacman-key --populate archlinux >/dev/null 2>&1
    sudo pacman -Syu --noconfirm --needed mesa seatd libinput libxkbcommon pixman wayland fontconfig ttf-dejavu foot \
      xorg-xwayland libglvnd polkit wlr-randr procps-ng >/dev/null
    sudo mkdir -p /etc/systemd/system/getty@tty1.service.d
    printf "[Service]\nExecStart=\nExecStart=-/sbin/agetty -o \"-p -f -- \\\\\\\\u\" --noclear --autologin ygg %%I \$TERM\n" |
      sudo tee /etc/systemd/system/getty@tty1.service.d/autologin.conf >/dev/null
    grep -q drm-start ~/.bash_profile 2>/dev/null ||
      printf "\nif [ \"\$(tty)\" = /dev/tty1 ] && [ -x ~/drm-start ]; then ~/drm-start; fi\n" >> ~/.bash_profile
    printf "#!/bin/sh\nexport PATH=\"\$HOME/bin:\$PATH\" LIBSEAT_BACKEND=logind XDG_SESSION_TYPE=wayland\ncd ~\n[ -f ~/drm-env ] && . ~/drm-env\nyggdrasil-compositor --drm \$(cat ~/drm-args 2>/dev/null) > ~/drm.log 2>&1\necho \"compositor ended: \$?\" >> ~/drm.log\n" > ~/drm-start
    chmod +x ~/drm-start; mkdir -p ~/bin; sudo systemctl daemon-reload; echo provisioned'
  ;;
deploy)
  scp -q -i "$VM/key" -P 2222 -o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null \
    "$TARGET/yggdrasil-compositor" "$TARGET/yggdrasil-panel" "$TARGET/yggdrasil-settings" ygg@127.0.0.1:bin/
  ;;
start)
  shift
  args=${*:---run dbus-run-session -- yggdrasil-panel --run yggdrasil-settings}
  env=""
  for kv in ${YGG_DRM_ENV:-}; do env="${env}export $kv
"; done
  ssh_ "XDG_RUNTIME_DIR=/run/user/1000 bin/yggdrasil-compositor ctl quit 2>/dev/null; sleep 1
    printf '%s' '$env' > ~/drm-env; printf '%s\n' '$args' > ~/drm-args
    sudo systemctl restart getty@tty1; sleep 8; head -4 ~/drm.log"
  ;;
ctl)
  shift
  ssh_ "XDG_RUNTIME_DIR=/run/user/1000 bin/yggdrasil-compositor ctl $*"
  ;;
shot)
  if [ -n "${3:-}" ]; then
    python3 -I "$HERE/drm-vm.py" "$VM" keys "$3"
    sleep "${4:-1.5}"
  fi
  python3 -I "$HERE/drm-vm.py" "$VM" shot "$VM/shots/$2.png"
  ;;
type | click | move)
  python3 -I "$HERE/drm-vm.py" "$VM" "$@"
  ;;
measure)
  ssh_ "pid=\$(pgrep -x yggdrasil-compo); s=${2:-60}
    cpu() { cat /proc/\$pid/task/*/schedstat | awk '{s+=\$1} END{print s}'; }
    sw() { cat /proc/\$pid/task/*/status | awk '/ctxt_switches/{s+=\$2} END{print s}'; }
    c0=\$(cpu); w0=\$(sw); sleep \$s; c1=\$(cpu); w1=\$(sw)
    awk -v a=\$c0 -v b=\$c1 -v s=\$s -v w=\$((w1 - w0)) 'BEGIN{printf \"%.2f ms of CPU in %ss = %.4f %% of a core, %d context switches\\n\", (b-a)/1e6, s, 100*(b-a)/1e9/s, w}'
    grep -E '^(VmRSS|RssAnon|RssFile|RssShmem)' /proc/\$pid/status | tr -s ' \t' ' ' | tr '\n' ';'; echo"
  ;;
ssh)
  shift
  ssh_ "$@"
  ;;
stop)
  python3 -I "$HERE/drm-vm.py" "$VM" powerdown
  ;;
*)
  sed -n '2,22p' "$0" >&2
  exit 2
  ;;
esac
