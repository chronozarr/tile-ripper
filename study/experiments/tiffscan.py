"""Minimal TIFF/BigTIFF IFD scanner for COG headers. A probe, not a reader: it extracts exactly what a
request manifest needs (tile grid, TileOffsets, TileByteCounts, compression, predictor, geo tags).

Input is a byte prefix of the file plus a callback to fetch more bytes if a tag array lies beyond it.
"""

from __future__ import annotations

import struct
from dataclasses import dataclass, field
from typing import Callable

# TIFF tag ids
IMAGE_WIDTH, IMAGE_LENGTH, BITS, COMPRESSION, PHOTOMETRIC = 256, 257, 258, 259, 262
SPP, PLANAR, PREDICTOR, TILE_W, TILE_H, TILE_OFFSETS, TILE_COUNTS = (
    277,
    284,
    317,
    322,
    323,
    324,
    325,
)
STRIP_OFFSETS, SUBFILE, SAMPLE_FORMAT = 273, 254, 339
PIXEL_SCALE, TIEPOINT, GEOKEYS, GDAL_METADATA, GDAL_NODATA = (
    33550,
    33922,
    34735,
    42112,
    42113,
)

# (struct char, size) by TIFF field type
TYPES = {
    1: ("B", 1),
    2: ("c", 1),
    3: ("H", 2),
    4: ("I", 4),
    5: ("II", 8),
    6: ("b", 1),
    7: ("B", 1),
    8: ("h", 2),
    9: ("i", 4),
    10: ("ii", 8),
    11: ("f", 4),
    12: ("d", 8),
    16: ("Q", 8),
    17: ("q", 8),
    18: ("Q", 8),
}


@dataclass
class Ifd:
    level: int
    width: int
    height: int
    tile_w: int
    tile_h: int
    compression: int
    predictor: int
    bits: int
    spp: int
    planar: int
    sample_format: int
    offsets: list[int]
    counts: list[int]
    subfile: int = 0
    extra: dict = field(default_factory=dict)  # geo tags (IFD0 only)


class Buffer:
    """Prefix bytes with on-demand extension through `fetch(start, length)`; counts extra fetches."""

    def __init__(self, prefix: bytes, fetch: Callable[[int, int], bytes] | None):
        self.data = bytearray(prefix)
        self.fetch = fetch
        self.extra_fetches: list[tuple[int, int]] = []
        self.high = 0  # furthest byte the parser needed (header bytes really required)

    def read(self, off: int, n: int) -> bytes:
        self.high = max(self.high, off + n)
        if off + n <= len(self.data):
            return bytes(self.data[off : off + n])
        if self.fetch is None:
            raise EOFError(
                f"need bytes {off}-{off + n} but prefix has {len(self.data)}"
            )
        self.extra_fetches.append((off, n))
        return self.fetch(off, n)


def parse(
    prefix: bytes, fetch: Callable[[int, int], bytes] | None = None
) -> tuple[list[Ifd], Buffer, bool]:
    """Return (ifds, buffer, bigtiff). Raises on anything that is not little/big-endian classic or BigTIFF."""
    buf = Buffer(prefix, fetch)
    order = {b"II": "<", b"MM": ">"}[bytes(prefix[:2])]
    magic = struct.unpack(order + "H", prefix[2:4])[0]
    big = magic == 43
    if magic not in (42, 43):
        raise ValueError(f"not a TIFF (magic {magic})")
    first = struct.unpack(
        order + ("Q" if big else "I"), prefix[8:16] if big else prefix[4:8]
    )[0]
    entry_size, count_fmt, count_size, ptr_fmt, ptr_size = (
        (20, "Q", 8, "Q", 8) if big else (12, "H", 2, "I", 4)
    )
    inline = 8 if big else 4

    ifds: list[Ifd] = []
    off, level = first, 0
    while off:
        n = struct.unpack(order + count_fmt, buf.read(off, count_size))[0]
        raw = buf.read(off + count_size, n * entry_size)
        next_off = struct.unpack(
            order + ptr_fmt, buf.read(off + count_size + n * entry_size, ptr_size)
        )[0]
        tags: dict[int, list] = {}
        for i in range(n):
            e = raw[i * entry_size : (i + 1) * entry_size]
            tag, typ = struct.unpack(order + "HH", e[:4])
            cnt = struct.unpack(
                order + ("Q" if big else "I"), e[4 : 4 + (8 if big else 4)]
            )[0]
            val = e[4 + (8 if big else 4) :]
            fmt, size = TYPES.get(typ, ("B", 1))
            total = cnt * size
            data = (
                val[:total]
                if total <= inline
                else buf.read(struct.unpack(order + ptr_fmt, val[:ptr_size])[0], total)
            )
            if typ in (5, 10):
                nums = struct.unpack(order + fmt[0] * (2 * cnt), data)
                tags[tag] = [nums[j] / nums[j + 1] for j in range(0, len(nums), 2)]
            elif typ == 2:
                tags[tag] = [data.rstrip(b"\0").decode("latin1")]
            else:
                tags[tag] = list(struct.unpack(order + fmt * cnt, data))
        g = lambda t, d=None: tags.get(t, [d])[0]  # noqa: E731
        if TILE_OFFSETS not in tags:
            raise ValueError(
                f"IFD{level} is not tiled (no TileOffsets): not a COG for tile planning"
            )
        ifd = Ifd(
            level,
            g(IMAGE_WIDTH),
            g(IMAGE_LENGTH),
            g(TILE_W),
            g(TILE_H),
            g(COMPRESSION),
            g(PREDICTOR, 1),
            g(BITS, 8),
            g(SPP, 1),
            g(PLANAR, 1),
            g(SAMPLE_FORMAT, 1),
            tags[TILE_OFFSETS],
            tags[TILE_COUNTS],
            g(SUBFILE, 0),
        )
        if level == 0:
            ifd.extra = {
                "pixel_scale": tags.get(PIXEL_SCALE),
                "tiepoint": tags.get(TIEPOINT),
                "nodata": g(GDAL_NODATA),
                "byte_order": "little" if order == "<" else "big",
            }
        ifds.append(ifd)
        off, level = next_off, level + 1
    return ifds, buf, big
