"""E4: layout index (L). One ranged GET of the file prefix per asset, parsed into tile offsets/byte counts.

This is the piece no STAC catalog or stac-geoparquet file carries. Measures what it costs to build,
how big a prefix is actually needed, and writes the index as Parquet for the DuckDB planner (E5).
"""

from __future__ import annotations

import concurrent.futures as cf
import json
import re
import sys
import time

import httpx
import pyarrow as pa
import pyarrow.parquet as pq

import tiffscan
from common import RESULTS, USER_AGENT, save, summarize

PREFIX = 32768  # tuned GDAL uses GDAL_INGESTED_BYTES_AT_OPEN=32768; default is 16384
BANDS = {"red": "B04", "nir": "B08", "scl": "SCL"}


def fetch_layout(client: httpx.Client, href: str) -> dict:
    t0 = time.perf_counter()
    r = client.get(href, headers={"Range": f"bytes=0-{PREFIX - 1}"})
    r.raise_for_status()
    m = re.match(r"bytes (\d+)-(\d+)/(\d+)", r.headers["Content-Range"])
    size = int(m.group(3))
    extra: list[tuple[int, int]] = []

    def more(off: int, n: int) -> bytes:
        extra.append((off, n))
        rr = client.get(href, headers={"Range": f"bytes={off}-{off + n - 1}"})
        rr.raise_for_status()
        return rr.content

    ifds, buf, big = tiffscan.parse(r.content, more)
    return {
        "href": href,
        "size": size,
        "etag": r.headers.get("ETag", "").strip('"'),
        "last_modified": r.headers.get("Last-Modified"),
        "bigtiff": big,
        "ifds": ifds,
        "gets": 1 + len(extra),
        "bytes": len(r.content) + sum(n for _, n in extra),
        "extra_fetches": extra,
        "header_needed": buf.high,
        "wall_s": time.perf_counter() - t0,
    }


def main() -> None:
    conc = int(sys.argv[1]) if len(sys.argv) > 1 else 16
    scenes = json.loads((RESULTS / "scenes_17SPV_2025.json").read_text())["scenes"]
    hrefs = [(s["id"], b, s[b]) for s in scenes for b in BANDS]
    print(f"{len(hrefs)} assets, prefix {PREFIX} B, concurrency {conc}")
    limits = httpx.Limits(max_connections=conc, max_keepalive_connections=conc)
    t0 = time.perf_counter()
    with httpx.Client(
        http2=False, timeout=60, limits=limits, headers={"User-Agent": USER_AGENT}
    ) as c:
        with cf.ThreadPoolExecutor(conc) as ex:
            res = list(ex.map(lambda h: fetch_layout(c, h[2]), hrefs))
    wall = time.perf_counter() - t0

    rows, facts = [], []
    for (sid, band, href), r in zip(hrefs, res):
        i0 = r["ifds"][0]
        ex = i0.extra
        facts.append(
            {
                "band": band,
                "tile": (i0.tile_w, i0.tile_h),
                "comp": i0.compression,
                "pred": i0.predictor,
                "n_ifd": len(r["ifds"]),
                "ntiles": len(i0.offsets),
                "bigtiff": r["bigtiff"],
                "order": ex["byte_order"],
                "tiepoint": ex["tiepoint"],
                "scale": ex["pixel_scale"],
                "nodata": ex["nodata"],
                "extra": r["extra_fetches"],
                "dtype_bits": i0.bits,
                "gap8": all(
                    b - (a + n) == 8
                    for a, n, b in zip(i0.offsets[:-1], i0.counts[:-1], i0.offsets[1:])
                )
                if len(i0.offsets) > 1
                else True,
            }
        )
        for ifd in r["ifds"]:
            rows.append(
                {
                    "scene_id": sid,
                    "band": band,
                    "href": href,
                    "file_size": r["size"],
                    "etag": r["etag"],
                    "level": ifd.level,
                    "width": ifd.width,
                    "height": ifd.height,
                    "tile_w": ifd.tile_w,
                    "tile_h": ifd.tile_h,
                    "compression": ifd.compression,
                    "predictor": ifd.predictor,
                    "bits": ifd.bits,
                    "spp": ifd.spp,
                    "planar": ifd.planar,
                    "offsets": ifd.offsets,
                    "counts": ifd.counts,
                    "tiepoint": ex["tiepoint"] if ifd.level == 0 else None,
                    "pixel_scale": ex["pixel_scale"] if ifd.level == 0 else None,
                    "nodata": ex["nodata"] if ifd.level == 0 else None,
                }
            )
    schema = pa.schema(
        [
            ("scene_id", pa.string()),
            ("band", pa.string()),
            ("href", pa.string()),
            ("file_size", pa.int64()),
            ("etag", pa.string()),
            ("level", pa.int32()),
            ("width", pa.int32()),
            ("height", pa.int32()),
            ("tile_w", pa.int32()),
            ("tile_h", pa.int32()),
            ("compression", pa.int32()),
            ("predictor", pa.int32()),
            ("bits", pa.int32()),
            ("spp", pa.int32()),
            ("planar", pa.int32()),
            ("offsets", pa.list_(pa.uint64())),
            ("counts", pa.list_(pa.uint64())),
            ("tiepoint", pa.list_(pa.float64())),
            ("pixel_scale", pa.list_(pa.float64())),
            ("nodata", pa.string()),
        ]
    )
    path = RESULTS / "layout_17SPV_2025.parquet"
    pq.write_table(pa.Table.from_pylist(rows, schema=schema), path, compression="zstd")

    by_band: dict[str, dict] = {}
    for f in facts:
        by_band.setdefault(
            f["band"],
            {
                "tile": set(),
                "comp": set(),
                "pred": set(),
                "n_ifd": set(),
                "ntiles": set(),
                "bigtiff": set(),
                "order": set(),
                "tiepoint": set(),
                "extra": 0,
                "gap8_all": True,
                "bits": set(),
            },
        )
        b = by_band[f["band"]]
        for key, field in (
            ("tile", "tile"),
            ("comp", "comp"),
            ("pred", "pred"),
            ("n_ifd", "n_ifd"),
            ("ntiles", "ntiles"),
            ("bigtiff", "bigtiff"),
            ("order", "order"),
            ("bits", "dtype_bits"),
        ):
            b[key].add(f[field])
        b["tiepoint"].add(tuple(f["tiepoint"] or ()))
        b["extra"] += len(f["extra"])
        b["gap8_all"] &= f["gap8"]
    summary = {
        "assets": len(res),
        "concurrency": conc,
        "prefix_bytes": PREFIX,
        "wall_s": wall,
        "gets": sum(r["gets"] for r in res),
        "bytes": sum(r["bytes"] for r in res),
        "extra_fetches_needed": sum(len(r["extra_fetches"]) for r in res),
        "per_asset_wall_s": summarize([r["wall_s"] for r in res]),
        "header_bytes_needed": {
            b: summarize(
                [r["header_needed"] for (_, bb, _), r in zip(hrefs, res) if bb == b]
            )
            for b in BANDS
        },
        "etags_unique": len({r["etag"] for r in res}) == len(res),
        "by_band": {
            k: {
                kk: (sorted(map(str, vv)) if isinstance(vv, set) else vv)
                for kk, vv in v.items()
            }
            for k, v in by_band.items()
        },
        "file_sizes_MB": {
            b: [
                round(r["size"] / 1e6, 1)
                for (_, bb, _), r in zip(hrefs, res)
                if bb == b
            ][:3]
            for b in BANDS
        },
    }
    print(json.dumps(summary, indent=1, default=str)[:3500])
    save("e4_layout", summary)


if __name__ == "__main__":
    main()
