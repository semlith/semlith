#!/usr/bin/env python3
"""Generate the image class's pinned corpus.

The retrieval harness measures text over `tests/fixtures/retrieval/corpus`,
whose hash may not move: every figure the repository publishes is read against
it. So the image class gets a corpus of its own, here, and this script is how it
is made — the same rule `tests/fixtures/generate.py` follows for the document
fixtures. Regenerate rather than edit, and re-pin `corpus.yaml` afterwards.

The pictures are drawn from arithmetic rather than fetched, so the corpus is
reproducible byte for byte on any machine and carries nobody else's licence. No
Pillow: a PNG is a zlib stream in four chunks, and writing it directly is fewer
moving parts than a dependency the rest of the fixtures do not need.

    python3 tests/fixtures/retrieval/images/generate.py

Every image is 256x256 RGB. CLIP resizes to 224, so a shape smaller than about
a fifth of the frame survives the resize as a smudge; each of these fills most
of it on purpose.
"""

from __future__ import annotations

import pathlib
import struct
import zlib

SIZE = 256
HERE = pathlib.Path(__file__).resolve().parent
CORPUS = HERE / "corpus"

WHITE = (255, 255, 255)
RED = (220, 38, 38)
BLUE = (37, 99, 235)
GREEN = (22, 163, 74)
YELLOW = (250, 204, 21)
PURPLE = (147, 51, 234)
ORANGE = (234, 88, 12)
BLACK = (17, 17, 17)


def png(path: pathlib.Path, pixel) -> None:
    """Write a 256x256 RGB PNG, one `pixel(x, y) -> (r, g, b)` call per pixel."""
    raw = bytearray()
    for y in range(SIZE):
        raw.append(0)  # filter type 0 (None) for every scanline
        for x in range(SIZE):
            raw.extend(pixel(x, y))

    def chunk(kind: bytes, data: bytes) -> bytes:
        body = kind + data
        return struct.pack(">I", len(data)) + body + struct.pack(">I", zlib.crc32(body))

    header = struct.pack(">IIBBBBB", SIZE, SIZE, 8, 2, 0, 0, 0)
    path.write_bytes(
        b"\x89PNG\r\n\x1a\n"
        + chunk(b"IHDR", header)
        # Fixed level, so two runs of this script produce identical bytes and
        # the manifest below stays true.
        + chunk(b"IDAT", zlib.compress(bytes(raw), 9))
        + chunk(b"IEND", b"")
    )


def disc(cx, cy, r):
    return lambda x, y: (x - cx) ** 2 + (y - cy) ** 2 <= r * r


def main() -> None:
    CORPUS.mkdir(parents=True, exist_ok=True)
    mid = SIZE // 2

    inside = disc(mid, mid, 96)
    png(CORPUS / "red-circle.png", lambda x, y: RED if inside(x, y) else WHITE)

    png(
        CORPUS / "blue-square.png",
        lambda x, y: BLUE if 40 <= x < 216 and 40 <= y < 216 else WHITE,
    )

    # An upright triangle: the half-width grows linearly with the distance down
    # from the apex.
    def triangle(x, y):
        if y < 32 or y >= 224:
            return False
        half = (y - 32) * 112 // 192
        return abs(x - mid) <= half

    png(CORPUS / "green-triangle.png", lambda x, y: GREEN if triangle(x, y) else WHITE)

    png(
        CORPUS / "black-white-checkerboard.png",
        lambda x, y: BLACK if (x // 32 + y // 32) % 2 == 0 else WHITE,
    )

    png(
        CORPUS / "yellow-horizontal-stripes.png",
        lambda x, y: YELLOW if (y // 16) % 2 == 0 else WHITE,
    )

    png(
        CORPUS / "purple-diagonal-line.png",
        lambda x, y: PURPLE if abs(x - y) <= 14 else WHITE,
    )

    outer, inner = disc(mid, mid, 104), disc(mid, mid, 58)
    png(
        CORPUS / "orange-ring.png",
        lambda x, y: ORANGE if outer(x, y) and not inner(x, y) else WHITE,
    )

    # Dark on the left, light on the right, so "fading" is a direction rather
    # than a wash.
    png(
        CORPUS / "grey-gradient.png",
        lambda x, y: (20 + x * 215 // (SIZE - 1),) * 3,
    )

    total = 0
    for image in sorted(CORPUS.glob("*.png")):
        size = image.stat().st_size
        total += size
        print(f"  {image.name:<34} {size:>7} bytes")
    print(f"  {len(list(CORPUS.glob('*.png')))} images, {total} bytes")


if __name__ == "__main__":
    main()
