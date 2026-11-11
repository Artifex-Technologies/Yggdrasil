#!/usr/bin/python3 -I
"""snapshot.py FOLDER...: every file, folder and link under the folders, as JSON: path -> [kind, mode, content]. Text is kept
as it is; bytes that are not UTF-8 are kept as hex (the key file's random bytes: their length only)."""
import json
import os
import sys

out = {}
for top in sys.argv[1:]:
    for d, dirs, files in os.walk(top):
        dirs.sort()
        for n in sorted(files) + sorted(x for x in dirs if os.path.islink(os.path.join(d, x))):
            p = os.path.join(d, n)
            st = os.lstat(p)
            if os.path.islink(p):
                out[p] = ["link", 0, os.readlink(p)]
                continue
            with open(p, "rb") as f:
                data = f.read()
            if n == "crypto_keyfile.bin":
                content = f"{len(data)} random bytes"
            else:
                try:
                    content = data.decode()
                except UnicodeDecodeError:
                    content = "hex:" + data.hex()
            out[p] = ["file", st.st_mode & 0o7777, content]
        for n in dirs:
            p = os.path.join(d, n)
            if not os.path.islink(p):
                out[p + "/"] = ["dir", os.lstat(p).st_mode & 0o7777, ""]
json.dump(out, sys.stdout, indent=1, sort_keys=True)
