"""Draws assets/qasd.ico (and qasd.png): a bold Q on a blue rounded square,
its tail cutting through the ring (the name's letter, and a lens), and the
tray's one-colour versions: qasd-tray-dark.ico (white, for a dark taskbar)
and qasd-tray-light.ico (black, for a light one).
Pure Python, no dependencies: signed-distance shapes, 4x4 supersampling."""
import math, struct, zlib, os

def clamp(x, a=0.0, b=1.0): return max(a, min(b, x))

def sd_round_rect(px, py, cx, cy, hw, hh, r):
    qx, qy = abs(px - cx) - hw + r, abs(py - cy) - hh + r
    return math.hypot(max(qx, 0), max(qy, 0)) + min(max(qx, qy), 0) - r

def sd_segment(px, py, ax, ay, bx, by):
    pax, pay, bax, bay = px - ax, py - ay, bx - ax, by - ay
    h = clamp((pax * bax + pay * bay) / (bax * bax + bay * bay))
    return math.hypot(pax - bax * h, pay - bay * h)

def q_mark(u, v, n):
    """Inside the Q (ring with a gap around its tail, and the tail)?"""
    w = .1 if n > 24 else .13
    cx, cy, r = .48, .47, .215
    tail = sd_segment(u, v, .57, .57, .77, .77) - w / 2
    ring = abs(math.hypot(u - cx, v - cy) - r) - w / 2
    ring = max(ring, -(tail - w * .55))
    return min(ring, tail) < 0

def render(n, tray=None):
    """The app icon, or (tray = (r, g, b)) the bare Q in one colour, a bit larger."""
    ss = 4
    px = []
    for y in range(n):
        row = []
        for x in range(n):
            acc = [0.0, 0.0, 0.0, 0.0]
            for sy in range(ss):
                for sx in range(ss):
                    u = (x + (sx + .5) / ss) / n  # 0..1
                    v = (y + (sy + .5) / ss) / n
                    if tray:
                        # No tile: the Q fills the square, as tray glyphs do.
                        if not q_mark(.5 + (u - .5) * .78, .5 + (v - .5) * .78, n):
                            continue
                        col = list(tray)
                    else:
                        if sd_round_rect(u, v, .5, .5, .46, .46, .22) > 0:
                            continue
                        t = clamp((u + v) / 2)
                        c1, c2 = (37, 99, 235), (6, 182, 212)
                        col = [c1[i] + (c2[i] - c1[i]) * t for i in range(3)]
                        if q_mark(u, v, n):
                            col = [255, 255, 255]
                    for i in range(3): acc[i] += col[i]
                    acc[3] += 1
            k = ss * ss
            a = acc[3] / k
            if acc[3]:
                row.append(tuple(int(acc[i] / acc[3]) for i in range(3)) + (int(a * 255),))
            else:
                row.append((0, 0, 0, 0))
        px.append(row)
    return px

def png(px):
    n = len(px)
    raw = b"".join(b"\0" + bytes(c for p in row for c in p) for row in px)
    def chunk(t, d): return struct.pack(">I", len(d)) + t + d + struct.pack(">I", zlib.crc32(t + d) & 0xffffffff)
    return b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", n, n, 8, 6, 0, 0, 0)) + chunk(b"IDAT", zlib.compress(raw, 9)) + chunk(b"IEND", b"")

here = os.path.dirname(os.path.abspath(__file__))

def ico(name, sizes, tray=None):
    imgs = [png(render(s, tray)) for s in sizes]
    head = struct.pack("<HHH", 0, 1, len(sizes))
    off = 6 + 16 * len(sizes)
    dirs = b""
    for s, d in zip(sizes, imgs):
        dirs += struct.pack("<BBBBHHII", s % 256, s % 256, 0, 0, 1, 32, len(d), off)
        off += len(d)
    open(os.path.join(here, name), "wb").write(head + dirs + b"".join(imgs))
    return imgs

imgs = ico("qasd.ico", [16, 20, 24, 32, 40, 48, 64, 256])
open(os.path.join(here, "qasd.png"), "wb").write(imgs[-1])
ico("qasd-tray-dark.ico", [16, 20, 24, 32, 40, 48], (255, 255, 255))
ico("qasd-tray-light.ico", [16, 20, 24, 32, 40, 48], (28, 28, 28))
print("ok")
