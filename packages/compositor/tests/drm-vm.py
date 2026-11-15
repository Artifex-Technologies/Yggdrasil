#!/usr/bin/env python3
"""The test machine's screen and input, for drm-vm.sh (the standard library only):
  drm-vm.py VMDIR shot OUT.png        the screen through QEMU's VNC socket (RFB 3.8, no auth, raw): QEMU reads the virgl scanout
                                      back for it, where QMP's screendump has no surface to give
  drm-vm.py VMDIR keys COMBO...       press key combinations through QMP (QEMU key names: ctrl-alt-f2, ret, meta_l)
  drm-vm.py VMDIR type TEXT           type text (US layout) on the USB keyboard
  drm-vm.py VMDIR move X Y            move the USB tablet's pointer to screen pixel X,Y
  drm-vm.py VMDIR click X Y [right]   move there and click
  drm-vm.py VMDIR powerdown           ask the guest to power off
"""
import json
import os
import socket
import struct
import sys
import time
import zlib

SHIFTED = {"!": "1", "@": "2", "#": "3", "$": "4", "%": "5", "^": "6", "&": "7", "*": "8", "(": "9", ")": "0", "_": "minus",
           "+": "equal", ":": "semicolon", '"': "apostrophe", "<": "comma", ">": "dot", "?": "slash", "|": "backslash", "~": "grave_accent"}
PLAIN = {" ": "spc", "\n": "ret", "-": "minus", "=": "equal", ";": "semicolon", "'": "apostrophe", ",": "comma", ".": "dot",
         "/": "slash", "\\": "backslash", "`": "grave_accent"}


def recv(s, n):
    b = bytearray()
    while len(b) < n:
        c = s.recv(n - len(b))
        if not c:
            raise RuntimeError("VNC closed")
        b += c
    return bytes(b)


def write_png(path, w, h, rgb):
    raw = b"".join(b"\x00" + rgb[y * w * 3:(y + 1) * w * 3] for y in range(h))

    def chunk(t, d):
        return struct.pack(">I", len(d)) + t + d + struct.pack(">I", zlib.crc32(t + d) & 0xFFFFFFFF)

    with open(path, "wb") as out:
        out.write(b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", w, h, 8, 2, 0, 0, 0))
                  + chunk(b"IDAT", zlib.compress(raw, 6)) + chunk(b"IEND", b""))


def screen(vm):
    """The whole screen: (width, height, RGB bytes)."""
    s = socket.socket(socket.AF_UNIX)
    s.settimeout(20)
    s.connect(os.path.join(vm, "vnc.sock"))
    recv(s, 12)
    s.sendall(b"RFB 003.008\n")
    if 1 not in recv(s, recv(s, 1)[0]):
        raise RuntimeError("the VNC server wants a password")
    s.sendall(b"\x01")
    if struct.unpack(">I", recv(s, 4))[0] != 0:
        raise RuntimeError("VNC security failed")
    s.sendall(b"\x01")
    w, h = struct.unpack(">HH", recv(s, 4))
    recv(s, 16)
    recv(s, struct.unpack(">I", recv(s, 4))[0])
    # 32 bits a pixel, little endian, red at 16, green at 8, blue at 0; raw encoding; the whole screen
    s.sendall(struct.pack(">BxxxBBBBHHHBBBxxx", 0, 32, 24, 0, 1, 255, 255, 255, 16, 8, 0))
    s.sendall(struct.pack(">BxHi", 2, 1, 0))
    s.sendall(struct.pack(">BBHHHH", 3, 0, 0, 0, w, h))
    fb = bytearray(w * h * 3)
    covered = 0
    while covered < w * h:
        t = recv(s, 1)[0]
        if t == 0:
            recv(s, 1)
            for _ in range(struct.unpack(">H", recv(s, 2))[0]):
                x, y, rw, rh, enc = struct.unpack(">HHHHi", recv(s, 12))
                if enc != 0:
                    raise RuntimeError(f"VNC encoding {enc}")
                data = recv(s, rw * rh * 4)
                for row in range(rh):
                    line = data[row * rw * 4:(row + 1) * rw * 4]
                    rgb = bytearray(rw * 3)
                    rgb[0::3], rgb[1::3], rgb[2::3] = line[2::4], line[1::4], line[0::4]
                    off = ((y + row) * w + x) * 3
                    fb[off:off + rw * 3] = rgb
                covered += rw * rh
        elif t == 3:
            recv(s, 3)
            recv(s, struct.unpack(">I", recv(s, 4))[0])
        elif t != 2:
            raise RuntimeError(f"VNC message {t}")
    return w, h, bytes(fb)


def qmp(vm):
    s = socket.socket(socket.AF_UNIX)
    s.connect(os.path.join(vm, "qmp.sock"))
    f = s.makefile("rw")
    f.readline()
    cmd(f, "qmp_capabilities")
    return f


def cmd(f, name, **args):
    f.write(json.dumps({"execute": name, "arguments": args}) + "\n")
    f.flush()
    while True:
        m = json.loads(f.readline())
        if "return" in m:
            return m["return"]
        if "error" in m:
            raise RuntimeError(m["error"])


def keys(f, combo):
    cmd(f, "send-key", keys=[{"type": "qcode", "data": k} for k in combo.split("-")])
    time.sleep(0.08)


def move(vm, f, x, y):
    w, h = screen(vm)[:2]
    cmd(f, "input-send-event", events=[{"type": "abs", "data": {"axis": "x", "value": round(x * 32767 / (w - 1))}},
                                       {"type": "abs", "data": {"axis": "y", "value": round(y * 32767 / (h - 1))}}])


def main(vm, what, args):
    if what == "shot":
        w, h, rgb = screen(vm)
        write_png(args[0], w, h, rgb)
        print(f"saved {args[0]} ({w}x{h})")
        return
    f = qmp(vm)
    if what == "keys":
        for k in args:
            keys(f, k)
            time.sleep(0.3)
    elif what == "type":
        for ch in " ".join(args):
            if ch in SHIFTED:
                keys(f, "shift-" + SHIFTED[ch])
            elif ch in PLAIN:
                keys(f, PLAIN[ch])
            else:
                keys(f, "shift-" + ch.lower() if ch.isupper() else ch)
    elif what in ("move", "click"):
        move(vm, f, int(args[0]), int(args[1]))
        time.sleep(0.3)
        if what == "click":
            b = args[2] if len(args) > 2 else "left"
            for down in (True, False):
                cmd(f, "input-send-event", events=[{"type": "btn", "data": {"down": down, "button": b}}])
                time.sleep(0.1)
    elif what == "powerdown":
        cmd(f, "system_powerdown")
    else:
        print(__doc__)
        sys.exit(2)


if __name__ == "__main__":
    if len(sys.argv) < 3:
        print(__doc__)
        sys.exit(2)
    main(sys.argv[1], sys.argv[2], sys.argv[3:])
