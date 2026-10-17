#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""生成带透明通道的彩色托盘图标 tray.ico（蓝色圆角方块 + 白色 A 字形）。

纯标准库实现，不依赖 PIL。输出 16/32/48 三档 32-bit BGRA 图像，
供 WinOCR-Html 的 tray.rs 用 LoadImageW(LR_LOADFROMFILE) 加载。
"""
import struct

# 调色板（BGR 顺序，直接写入 ICO 的 BGRA 字节流）
BLUE = (0xEB, 0x63, 0x25)    # #2563EB 品牌蓝
BLUE_D = (0xAF, 0x40, 0x1E)  # #1E40AF 深蓝（描边）
WHITE = (0xFF, 0xFF, 0xFF)


def rounded_bg(size):
    """返回 size×size 的 BGRA 像素表，蓝色圆角方块。"""
    px = [[(0, 0, 0, 0) for _ in range(size)] for _ in range(size)]  # 透明底
    r = max(2, size // 6)  # 圆角半径
    for y in range(size):
        for x in range(size):
            # 圆角：四角超出半径的圆形区域设为透明
            cx = min(x, size - 1 - x)
            cy = min(y, size - 1 - y)
            if cx < r and cy < r:
                # 圆心在 (r, r) 方向；判断是否在圆外
                if (r - cx) ** 2 + (r - cy) ** 2 > r * r:
                    continue
            # 描边：靠近边缘一圈用深蓝
            edge = max(x, size - 1 - x, y, size - 1 - y)
            if edge < max(1, size // 16):
                color = BLUE_D
            else:
                color = BLUE
            px[y][x] = (color[0], color[1], color[2], 255)
    return px


# 5x7 大写 A 字形（# 为笔画）
A_GLYPH = [
    "01110",
    "10001",
    "10001",
    "11111",
    "10001",
    "10001",
    "10001",
]


def stamp_a(px, size):
    """把白色 A 字形盖在背景中央。"""
    gw, gh = 5, 7
    ox = (size - gw) // 2
    oy = (size - gh) // 2
    for gy, row in enumerate(A_GLYPH):
        for gx, ch in enumerate(row):
            if ch == "1":
                x = ox + gx
                y = oy + gy
                if 0 <= x < size and 0 <= y < size:
                    px[y][x] = (WHITE[0], WHITE[1], WHITE[2], 255)


def make_image(size):
    px = rounded_bg(size)
    stamp_a(px, size)
    # 转 BMP 行（自下而上，BGRA）
    bgra = bytearray()
    for y in range(size - 1, -1, -1):
        for x in range(size):
            b, g, r, a = px[y][x]
            bgra += bytes((b, g, r, a))
    # AND 掩码：32-bit alpha 下全 0（每行按 4 字节对齐）
    and_row_bytes = ((size + 31) // 32) * 4
    and_mask = b"\x00" * (and_row_bytes * size)
    return bgra + and_mask


def build_ico(path, sizes=(16, 32, 48)):
    images = []
    for s in sizes:
        images.append(make_image(s))
    # ICONDIR
    out = bytearray()
    out += struct.pack("<HHH", 0, 1, len(images))
    offset = 6 + 16 * len(images)
    for img, s in zip(images, sizes):
        bpp = 32
        out += struct.pack(
            "<BBBBHHII",
            s, s, 0, 0, 1, bpp, len(img), offset,
        )
        offset += len(img)
    for img in images:
        out += img
    with open(path, "wb") as f:
        f.write(out)
    print("wrote", path, len(out), "bytes")


if __name__ == "__main__":
    import os
    here = os.path.dirname(os.path.abspath(__file__))
    root = os.path.dirname(here)
    target = os.path.join(root, "tray.ico")
    build_ico(target)
