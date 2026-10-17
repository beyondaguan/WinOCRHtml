#!/usr/bin/env python3
"""probe_px.py — 量测离屏渲染 PNG 的内容边界，定位「被窗口裁切」类布局问题。

为什么需要它：Slint 的裁切（父容器 clip:true）不会报错、语法检查也过，
只能靠「内容是否触及图像右/下边缘」判断。人工看图容易漏，本脚本给出数值。

用法：
    python probe_px.py <png> [--bg 255,255,255] [--rows 560,590] [--cols all]
"""
import sys
import struct
import zlib


def read_png(path):
    f = open(path, "rb").read()
    assert f[:8] == b"\x89PNG\r\n\x1a\n", "not a png"
    pos, idat = 8, b""
    w = h = None
    while pos < len(f):
        ln = struct.unpack(">I", f[pos : pos + 4])[0]
        typ = f[pos + 4 : pos + 8]
        data = f[pos + 8 : pos + 8 + ln]
        if typ == b"IHDR":
            w, h, bd, ct = struct.unpack(">IIBB", data[:10])
            assert bd == 8 and ct in (2, 6), f"unsupported png bd={bd} ct={ct}"
            bpp = 3 if ct == 2 else 4
        elif typ == b"IDAT":
            idat += data
        pos += 12 + ln
    raw = zlib.decompress(idat)
    stride = w * bpp
    prev = bytearray(stride)
    rows = []
    i = 0
    for _ in range(h):
        ft = raw[i]
        i += 1
        line = bytearray(raw[i : i + stride])
        i += stride
        for x in range(stride):
            a = line[x - bpp] if x >= bpp else 0
            b = prev[x]
            c = prev[x - bpp] if x >= bpp else 0
            if ft == 1:
                line[x] = (line[x] + a) & 255
            elif ft == 2:
                line[x] = (line[x] + b) & 255
            elif ft == 3:
                line[x] = (line[x] + (a + b) // 2) & 255
            elif ft == 4:
                p = a + b - c
                pa, pb, pc = abs(p - a), abs(p - b), abs(p - c)
                pr = a if (pa <= pb and pa <= pc) else (b if pb <= pc else c)
                line[x] = (line[x] + pr) & 255
        rows.append(bytes(line))
        prev = line
    return w, h, bpp, rows


def main():
    args = sys.argv[1:]
    if not args:
        print(__doc__)
        sys.exit(2)
    path = args[0]
    bg = b"\xff\xff\xff"
    probe_rows = None
    for a in args[1:]:
        if a.startswith("--bg="):
            bg = bytes(int(v) for v in a[5:].split(","))
        elif a.startswith("--rows="):
            probe_rows = [int(v) for v in a[7:].split(",")]

    w, h, bpp, rows = read_png(path)
    print(f"[probe_px] {path}  {w}x{h}")

    # 全图内容边界
    maxx = maxy = -1
    for y in range(h):
        r = rows[y]
        nz = [x for x in range(w) if r[x * bpp : x * bpp + 3] != bg]
        if nz:
            maxx = max(maxx, max(nz))
            maxy = y
    print(f"  内容最右 x = {maxx} / 宽度 {w}  →  {'⚠ 触边(可能被裁)' if maxx >= w - 2 else 'ok'}")
    print(f"  内容最下 y = {maxy} / 高度 {h}")

    # 指定行（默认状态条区域）的边界
    ys = probe_rows if probe_rows else [h - 25, h - 20, h - 15, h - 10, h - 5]
    for y in ys:
        if not (0 <= y < h):
            continue
        r = rows[y]
        nz = [x for x in range(w) if r[x * bpp : x * bpp + 3] != bg]
        if nz:
            print(f"  y={y:4d} 最右非背景 x={max(nz):4d}  {'⚠ 触边' if max(nz) >= w - 2 else ''}")


if __name__ == "__main__":
    main()