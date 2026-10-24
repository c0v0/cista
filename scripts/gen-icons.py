#!/usr/bin/env python3
"""生成 cista 桌面壳图标（无第三方依赖：纯 zlib + struct 写 PNG）。

用法：python3 scripts/gen-icons.py
输出：apps/desktop/src-tauri/icons/{32x32.png,128x128.png,icon.png}
图形：深色圆角方块 + 白色「匣」边框 + 青色钥匙孔；纯几何，可重复生成。
"""
from __future__ import annotations

import struct
import zlib
from pathlib import Path

OUT = Path(__file__).resolve().parent.parent / "apps" / "desktop" / "src-tauri" / "icons"

BG = (31, 41, 55, 255)
FG = (248, 250, 252, 255)
ACCENT = (56, 189, 248, 255)
CLEAR = (0, 0, 0, 0)


def png(size: int, rows: list[list[tuple[int, int, int, int]]]) -> bytes:
    raw = b"".join(b"\x00" + bytes(v for px in row for v in px) for row in rows)

    def chunk(tag: bytes, data: bytes) -> bytes:
        return (
            struct.pack(">I", len(data))
            + tag
            + data
            + struct.pack(">I", zlib.crc32(tag + data) & 0xFFFFFFFF)
        )

    header = struct.pack(">IIBBBBB", size, size, 8, 6, 0, 0, 0)
    return (
        b"\x89PNG\r\n\x1a\n"
        + chunk(b"IHDR", header)
        + chunk(b"IDAT", zlib.compress(raw, 9))
        + chunk(b"IEND", b"")
    )


def icon(size: int) -> bytes:
    center = size / 2.0
    radius = size * 0.22
    margin = size * 0.18
    band = max(1.0, size * 0.055)
    key_r = size * 0.09
    stem_w = max(1.0, size * 0.035)

    rows: list[list[tuple[int, int, int, int]]] = []
    for y in range(size):
        row: list[tuple[int, int, int, int]] = []
        for x in range(size):
            fx, fy = x + 0.5, y + 0.5
            dx = max(abs(fx - center) - (center - radius), 0.0)
            dy = max(abs(fy - center) - (center - radius), 0.0)
            if dx * dx + dy * dy > radius * radius:
                row.append(CLEAR)
                continue

            color = BG
            inside_box = margin <= fx <= size - margin and margin <= fy <= size - margin
            inside_hole = (
                margin + band <= fx <= size - margin - band
                and margin + band <= fy <= size - margin - band
            )
            if inside_box and not inside_hole:
                color = FG

            kx, ky = fx - center, fy - size * 0.44
            if kx * kx + ky * ky <= key_r * key_r:
                color = ACCENT
            if abs(kx) <= stem_w and size * 0.50 <= fy <= size * 0.70:
                color = ACCENT
            row.append(color)
        rows.append(row)
    return png(size, rows)


def main() -> None:
    OUT.mkdir(parents=True, exist_ok=True)
    for name, size in (("32x32.png", 32), ("128x128.png", 128), ("icon.png", 512)):
        path = OUT / name
        path.write_bytes(icon(size))
        print(f"{path.relative_to(OUT.parent.parent.parent.parent.parent)} {size}x{size} {path.stat().st_size}B")


if __name__ == "__main__":
    main()
