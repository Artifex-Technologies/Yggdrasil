#!/usr/bin/env python3
"""Makes Saga MD's icon (../saga-md.svg) from icon/saga-md-original.jpg, the raven I had Gemini draw (2026-11-08), kept as it came.

    python3 packages/saga-md/icon/gen-icon.py

Gemini drew the raven perched on a branch; Saga's raven holds a scroll. So:
  1. the raven is found: everything brighter than the tile behind it (the tile's soft gradient is estimated first, so its light
     top does not count as bird); the dark lines inside the bird stay open and show the tile through, as in the drawing;
  2. the branch is taken away: the pieces that lie (almost) wholly along it go, the toes wrapped round it stay;
  3. it is traced with potrace into smooth outlines;
  4. a rolled scroll is drawn where the branch was, under the toes, and everything is put on Liki's dark tile (Minos's).
Needs Pillow, numpy and potrace.
"""
import math
import os
import re
import subprocess
import sys
import tempfile
from collections import deque

import numpy as np
from PIL import Image

HERE = os.path.dirname(os.path.abspath(__file__))
ORIGINAL = os.path.join(HERE, "saga-md-original.jpg")
OUT = os.path.join(os.path.dirname(HERE), "saga-md.svg")
TILE = os.environ.get("LIKI_TILE", "/usr/share/liki/minos-icon/tile-dark.svg")

INSIDE = (110, 914)  # the drawing's tile, less its edge: nothing outside this is the bird
LIFT = 9  # how much brighter than the tile behind it a pixel must be to be bird
# the band the branch lies in, as Gemini drew it (its axis and half its width, in the picture's pixels, with room to spare):
# pieces wholly inside it are the branch
ROD = ((206, 627), (476, 871))
ROD_HALF = 32
# the scroll drawn in its place: the branch's own line and thickness, measured from the pieces taken away, so the toes still wrap it
SCROLL = ((212, 619), (461, 879))
SCROLL_HALF = 25
BIRD = "#80858f"
PAPER = "#b8bcc4"
RIBBON = "#7c54d6"  # Yggdrasil's purple: the tie round the scroll
EDGE_GAP = 70  # pieces wholly this close to the drawing's tile edge are its corners' highlights, not bird
INK = "#15171d"


def background(lum):
    """The tile's gradient: a smooth surface fitted to the picture, refitted without what stands out above it (the bird)."""
    ys, xs = np.mgrid[INSIDE[0]:INSIDE[1]:4, INSIDE[0]:INSIDE[1]:4]
    x, y = xs.ravel() / 1024.0, ys.ravel() / 1024.0
    v = lum[ys.ravel(), xs.ravel()].astype(float)
    terms = lambda x, y: np.stack([x**i * y**j for i in range(4) for j in range(4 - i)], axis=1)
    keep = np.ones_like(v, bool)
    for _ in range(6):
        coef, *_ = np.linalg.lstsq(terms(x[keep], y[keep]), v[keep], rcond=None)
        fit = terms(x, y) @ coef
        keep = v < fit + 4
    gy, gx = np.mgrid[0:lum.shape[0], 0:lum.shape[1]]
    return (terms(gx.ravel() / 1024.0, gy.ravel() / 1024.0) @ coef).reshape(lum.shape)


def components(mask):
    """Each connected piece of the mask: a label picture and the pieces' pixel lists."""
    h, w = mask.shape
    label = np.zeros(mask.shape, np.int32)
    pieces = []
    for y0, x0 in zip(*np.nonzero(mask)):
        if label[y0, x0]:
            continue
        n = len(pieces) + 1
        q, pix = deque([(y0, x0)]), []
        label[y0, x0] = n
        while q:
            y, x = q.popleft()
            pix.append((y, x))
            for dy, dx in ((1, 0), (-1, 0), (0, 1), (0, -1)):
                ny, nx = y + dy, x + dx
                if 0 <= ny < h and 0 <= nx < w and mask[ny, nx] and not label[ny, nx]:
                    label[ny, nx] = n
                    q.append((ny, nx))
        pieces.append(pix)
    return label, pieces


def along_rod(y, x, half):
    """True where (x, y) lies on the branch's band."""
    (ax, ay), (bx, by) = ROD
    ux, uy = bx - ax, by - ay
    L = math.hypot(ux, uy)
    ux, uy = ux / L, uy / L
    t = (x - ax) * ux + (y - ay) * uy
    d = abs(-(x - ax) * uy + (y - ay) * ux)
    return -half <= t <= L + half and d <= half


def scroll_svg():
    """A rolled scroll lying where the branch was: the roll, its spiral ends, the edge of the outer turn of paper, a ribbon."""
    (ax, ay), (bx, by) = SCROLL
    L = math.hypot(bx - ax, by - ay)
    ux, uy = (bx - ax) / L, (by - ay) / L
    nx, ny = -uy, ux
    r = SCROLL_HALF
    ang = math.degrees(math.atan2(uy, ux))
    p = lambda t, s: (ax + ux * t + nx * s, ay + uy * t + ny * s)
    f = lambda pt: f"{pt[0]:.1f} {pt[1]:.1f}"
    out = []
    body = [p(0, -r), p(L, -r), p(L, r), p(0, r)]
    out.append(f'<path d="M{f(body[0])} L{f(body[1])} L{f(body[2])} L{f(body[3])} Z" fill="{PAPER}" stroke="{INK}" stroke-width="7" stroke-linejoin="round"/>')
    # the far end's round, then the near end's, with the paper's spiral showing
    for t, inner in ((L, False), (0, True)):
        cx, cy = p(t, 0)
        out.append(f'<ellipse cx="{cx:.1f}" cy="{cy:.1f}" rx="{r * 0.42:.1f}" ry="{r:.1f}" transform="rotate({ang:.1f} {cx:.1f} {cy:.1f})" fill="{PAPER}" stroke="{INK}" stroke-width="7"/>')
        if inner:
            # the turns of paper inside, as rings
            for k in (0.66, 0.34):
                out.append(f'<ellipse cx="{cx:.1f}" cy="{cy:.1f}" rx="{r * 0.42 * k:.1f}" ry="{r * k:.1f}" transform="rotate({ang:.1f} {cx:.1f} {cy:.1f})" fill="none" stroke="{INK}" stroke-width="4.5"/>')
    # the edge of the outer turn of paper, running along the roll and stepping down near the end
    e0, e1, e2 = p(L - 6, -r * 0.35), p(34, -r * 0.35), p(22, -r * 0.05)
    out.append(f'<path d="M{f(e0)} L{f(e1)} L{f(e2)}" fill="none" stroke="{INK}" stroke-width="5" stroke-linecap="round" stroke-linejoin="round"/>')
    # the ribbon tied round the middle, its two ends hanging down
    t0, t1 = L * 0.5 - 13, L * 0.5 + 13
    band = [p(t0, -r - 3), p(t1, -r - 3), p(t1, r + 3), p(t0, r + 3)]
    tail_a = [p(t0 + 4, r), p(t0 - 14, r + 46), p(t0 - 2, r + 38), p(t0 + 8, r + 50), p(t0 + 16, r)]
    tail_b = [p(t1 - 6, r), p(t1 + 18, r + 38), p(t1 + 22, r + 26), p(t1 + 32, r + 34), p(t1 + 8, r)]
    for poly in (tail_a, tail_b, band):
        out.append('<path d="M' + " L".join(f(q) for q in poly) + f' Z" fill="{RIBBON}" stroke="{INK}" stroke-width="6" stroke-linejoin="round"/>')
    return "\n  ".join(out)


def main():
    rgb = Image.open(ORIGINAL).convert("RGB")
    lum = np.asarray(rgb.convert("L")).astype(float)
    bird = (lum - background(lum)) > LIFT
    edge = np.zeros_like(bird)
    edge[INSIDE[0]:INSIDE[1], INSIDE[0]:INSIDE[1]] = True
    bird &= edge
    label, pieces = components(bird)
    dropped = 0
    for i, pix in enumerate(pieces, 1):
        on_rod = sum(along_rod(y, x, ROD_HALF) for y, x in pix)
        ys, xs = [q[0] for q in pix], [q[1] for q in pix]
        near_edge = min(ys) > INSIDE[1] - EDGE_GAP or max(ys) < INSIDE[0] + EDGE_GAP or min(xs) > INSIDE[1] - EDGE_GAP or max(xs) < INSIDE[0] + EDGE_GAP
        if len(pix) < 25 or on_rod >= 0.8 * len(pix) or (near_edge and len(pix) < 2000):
            for y, x in pix:
                bird[y, x] = False
            dropped += 1
    with tempfile.TemporaryDirectory() as tmp:
        pbm, traced = os.path.join(tmp, "bird.pbm"), os.path.join(tmp, "bird.svg")
        Image.fromarray(np.where(bird, 0, 255).astype(np.uint8)).convert("1").save(pbm)
        subprocess.run(["potrace", "-s", "-o", traced, "--turdsize", "20", "--opttolerance", "0.3", pbm], check=True)
        text = open(traced).read()
    g = re.search(r"<g transform=\"([^\"]*)\"[^>]*>(.*?)</g>", text, re.S)
    src = open(TILE).read()
    defs = re.search(r"<defs>.*?</defs>", src).group(0)
    tile_d = re.findall(r'<path d="([^"]*)" fill="url\(#t\)"/>', src)[0]
    tile_g = re.search(r'<g transform="[^"]*">', src).group(0)
    svg = f'''<svg xmlns="http://www.w3.org/2000/svg" width="256" height="256" viewBox="0 0 1024 1024">
  <!-- Saga MD: Saga's raven with a scroll in its claws. Made by icon/gen-icon.py from icon/saga-md-original.jpg (drawn with Gemini);
       the tile is Liki's (Minos's). -->
  {defs}
  {tile_g}<path d="{tile_d}" fill="url(#t)"/><path d="{tile_d}" fill="none" stroke="#ffffff" stroke-opacity=".10" stroke-width="5"/></g>
  {scroll_svg()}
  <g transform="{g.group(1)}" fill="{BIRD}" stroke="none">{g.group(2)}</g>
</svg>
'''
    open(OUT, "w").write(svg)
    print(f"{len(pieces)} pieces, {dropped} dropped (the branch and specks); wrote {OUT}")


if __name__ == "__main__":
    sys.exit(main())
