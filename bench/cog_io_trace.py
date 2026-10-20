"""Account frozen COG HTTP responses against every physical TIFF block.

Benchmark-only: read the preserved sparse mirrors and their range logs. No
network, provider access, decoder, production cache, or image output is added.
Metadata intervals are TIFF header/IFD/tag storage; all image IFDs are included
so read-ahead into unselected native tiles cannot be mistaken for metadata.
"""

from __future__ import annotations

import argparse
from collections import Counter
import hashlib
import json
from pathlib import Path
import struct

ROOT = Path(__file__).resolve().parents[1]
REAL = ROOT / "prototype-output/production-streaming/real"
WINDOWS = {
    "many-small": (7300, 2100, 96, 96),
    "several-large": (6144, 1024, 2048, 2048),
    "overlap-a": (6868, 1748, 768, 768),
    "overlap-b": (7124, 2004, 768, 768),
}


def merged(intervals):
    result = []
    for left, right in sorted(intervals):
        if result and left <= result[-1][1]:
            result[-1] = (result[-1][0], max(right, result[-1][1]))
        else:
            result.append((left, right))
    return result


def nbytes(intervals):
    return sum(b - a for a, b in merged(intervals))


def overlap(a, b):
    left, right = max(a[0], b[0]), min(a[1], b[1])
    return (left, right) if right > left else None


def tiff_layout(path):
    """Read TIFF offsets/counts only; never decode or interpret sample values."""
    sizes = {
        1: 1,
        2: 1,
        3: 2,
        4: 4,
        5: 8,
        6: 1,
        7: 1,
        8: 2,
        9: 4,
        10: 8,
        11: 4,
        12: 8,
        13: 4,
        16: 8,
        17: 8,
        18: 8,
    }
    integer = {1: "B", 3: "H", 4: "I", 13: "I", 16: "Q", 18: "Q"}
    metadata, images, seen = [], [], set()
    with path.open("rb") as source:

        def read(offset, size):
            source.seek(offset)
            data = source.read(size)
            assert len(data) == size, "truncated TIFF metadata"
            return data

        header = read(0, 16)
        endian = {b"II": "<", b"MM": ">"}[header[:2]]
        version = struct.unpack(endian + "H", header[2:4])[0]
        if version == 42:
            address, count_fmt, entry_size, inline = "I", "H", 12, 4
            pending = [struct.unpack(endian + "I", header[4:8])[0]]
            metadata.append((0, 8))
        else:
            assert version == 43 and header[4:8] == struct.pack(endian + "HH", 8, 0)
            address, count_fmt, entry_size, inline = "Q", "Q", 20, 8
            pending = [struct.unpack(endian + "Q", header[8:16])[0]]
            metadata.append((0, 16))
        address_size, count_size = struct.calcsize(address), struct.calcsize(count_fmt)
        while pending:
            at = pending.pop(0)
            if not at or at in seen:
                continue
            seen.add(at)
            count = struct.unpack(endian + count_fmt, read(at, count_size))[0]
            assert count < 65536
            raw = read(at + count_size, count * entry_size + address_size)
            metadata.append((at, at + count_size + len(raw)))
            tags = {}
            for i in range(count):
                entry = raw[i * entry_size : (i + 1) * entry_size]
                tag, kind = struct.unpack(endian + "HH", entry[:4])
                n = struct.unpack(endian + address, entry[4 : 4 + address_size])[0]
                size = n * sizes[kind]
                value = entry[-inline:]
                if size > inline:
                    offset = struct.unpack(endian + address, value)[0]
                    metadata.append((offset, offset + size))
                    value = read(offset, size)
                else:
                    value = value[:size]
                if kind in integer:
                    tags[tag] = list(struct.unpack(endian + integer[kind] * n, value))
            following = struct.unpack(endian + address, raw[-address_size:])[0]
            pending.extend([following] + tags.get(330, []))
            if 324 not in tags:
                continue
            width, height = tags[256][0], tags[257][0]
            bx, by = tags[322][0], tags[323][0]
            nx, ny = (width + bx - 1) // bx, (height + by - 1) // by
            offsets, counts = tags[324], tags[325]
            assert len(offsets) == len(counts) == nx * ny
            images.append(
                dict(
                    ifd=at,
                    width=width,
                    height=height,
                    block_width=bx,
                    block_height=by,
                    blocks=[
                        dict(x=i % nx, y=i // nx, offset=lo, bytes=size)
                        for i, (lo, size) in enumerate(zip(offsets, counts))
                    ],
                )
            )
    assert images
    return dict(metadata=merged(metadata), images=images, file_size=path.stat().st_size)


def analyze(records, layouts, workload):
    x, y, width, height = WINDOWS[workload]
    requests, blocks, sources = [], [], []
    totals = Counter()
    for key, layout in sorted(layouts.items()):
        events = sorted(
            (r for r in records if r.get("method") == "GET" and r["source"] == key),
            key=lambda r: r.get("started", 0),
        )
        if not events:
            continue
        native = max(
            layout["images"], key=lambda image: image["width"] * image["height"]
        )
        all_blocks = []
        for image in layout["images"]:
            for block in image["blocks"]:
                selected = (
                    image is native
                    and block["x"] * image["block_width"] < x + width
                    and (block["x"] + 1) * image["block_width"] > x
                    and block["y"] * image["block_height"] < y + height
                    and (block["y"] + 1) * image["block_height"] > y
                )
                all_blocks.append(
                    {
                        **block,
                        "ifd": image["ifd"],
                        "selected": selected,
                        "native": image is native,
                        "intersections": [],
                    }
                )
        metadata = layout["metadata"]
        covered = []
        source_counts = Counter()
        for index, event in enumerate(events):
            region = (event["start"], event["end"] + 1)
            size = region[1] - region[0]
            repeated = nbytes(
                [
                    found
                    for previous in merged(covered)
                    if (found := overlap(region, previous))
                ]
            )
            counts = Counter(body_bytes=size, duplicate_response_bytes=repeated)
            hits = []
            for block in all_blocks:
                found = overlap(
                    region, (block["offset"], block["offset"] + block["bytes"])
                )
                if found:
                    block["intersections"].append(found)
                    category = (
                        "selected_payload_bytes"
                        if block["selected"]
                        else "unselected_payload_bytes"
                    )
                    counts[category] += found[1] - found[0]
                    hits.append(
                        dict(
                            ifd=block["ifd"],
                            x=block["x"],
                            y=block["y"],
                            selected=block["selected"],
                            bytes=found[1] - found[0],
                        )
                    )
            counts["exact_metadata_bytes"] = nbytes(
                [found for interval in metadata if (found := overlap(region, interval))]
            )
            counts["padding_or_other_bytes"] = (
                size
                - counts["selected_payload_bytes"]
                - counts["unselected_payload_bytes"]
                - counts["exact_metadata_bytes"]
            )
            if region[0] == 0:
                counts["header_response_bytes"] += size
            else:
                counts["imagery_response_bytes"] += size
            covered.append(region)
            source_counts.update(counts)
            requests.append(
                dict(
                    source=key,
                    index=index,
                    start=region[0],
                    end=region[1] - 1,
                    **counts,
                    blocks=hits,
                )
            )
        for block in all_blocks:
            pieces = block.pop("intersections")
            if not pieces:
                continue
            unique = nbytes(pieces)
            total = sum(right - left for left, right in pieces)
            blocks.append(
                dict(
                    source=key,
                    **block,
                    requests=len(pieces),
                    transferred_payload_bytes=total,
                    unique_payload_bytes=unique,
                    duplicate_payload_bytes=total - unique,
                    complete_fetch_equivalents=total / block["bytes"],
                )
            )
        source_counts["get_requests"] = len(events)
        source_counts["unique_response_bytes"] = nbytes(covered)
        source_counts["selected_blocks"] = sum(b["selected"] for b in all_blocks)
        source_counts["selected_compressed_bytes"] = sum(
            b["bytes"] for b in all_blocks if b["selected"]
        )
        source_counts["requested_blocks"] = sum(b["source"] == key for b in blocks)
        source_counts["blocks_with_repeated_payload"] = sum(
            b["source"] == key and b["duplicate_payload_bytes"] > 0 for b in blocks
        )
        source_counts["duplicate_selected_payload_bytes"] = sum(
            b["duplicate_payload_bytes"]
            for b in blocks
            if b["source"] == key and b["selected"]
        )
        source_counts["duplicate_unselected_payload_bytes"] = sum(
            b["duplicate_payload_bytes"]
            for b in blocks
            if b["source"] == key and not b["selected"]
        )
        source_counts["selected_payload_unique_bytes"] = sum(
            b["unique_payload_bytes"]
            for b in blocks
            if b["source"] == key and b["selected"]
        )
        sources.append(dict(source=key, **source_counts))
        totals.update(source_counts)
    totals["head_requests"] = sum(r.get("method") == "HEAD" for r in records)
    assert totals["body_bytes"] == sum(
        r["end"] - r["start"] + 1 for r in records if r.get("method") == "GET"
    )
    assert (
        totals["body_bytes"]
        == totals["selected_payload_bytes"]
        + totals["unselected_payload_bytes"]
        + totals["exact_metadata_bytes"]
        + totals["padding_or_other_bytes"]
    )
    assert (
        totals["body_bytes"]
        == totals["unique_response_bytes"] + totals["duplicate_response_bytes"]
    )
    assert (
        totals["selected_payload_unique_bytes"] == totals["selected_compressed_bytes"]
    ), "selected block not completely captured by response trace"
    return dict(totals=dict(totals), sources=sources, requests=requests, blocks=blocks)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--runs",
        type=Path,
        nargs="+",
        default=[REAL / "matched-latency-8ms-100mbit/runs.jsonl"],
    )
    parser.add_argument(
        "--output",
        type=Path,
        default=ROOT / "docs/cog-efficiency/trace-accounting.json",
    )
    parser.add_argument(
        "--raw-output",
        type=Path,
        default=ROOT / "prototype-output/cog-efficiency/trace-accounting-full.json",
    )
    args = parser.parse_args()
    args.runs = [path.resolve() for path in args.runs]
    manifest = json.loads((REAL / "sources.json").read_text())
    layouts = {
        f"/{i}.tif": tiff_layout(REAL / f"mirror/{i}.tif")
        for i in range(len(manifest["sources"]))
    }
    rows = []
    for path in args.runs:
        for line in path.read_text().splitlines():
            run = json.loads(line)
            if not run.get("request_records"):
                continue
            analysis = analyze(run["request_records"], layouts, run["workload"])
            assert analysis["totals"]["body_bytes"] == run["http"]["body_bytes"]
            rows.append(
                dict(
                    input=str(path.relative_to(ROOT)),
                    workload=run["workload"],
                    mode=run["mode"],
                    rep=run.get("rep"),
                    workers=run.get("workers"),
                    passed=run.get("passed"),
                    **analysis,
                )
            )
    common = dict(
        schema_version=1,
        scope="all HTTP response bodies accounted exactly by TIFF block offsets; no decoder inference",
        interval_convention="request end is inclusive; metadata source intervals are half-open",
        mirror_index_sha256=hashlib.sha256(
            (REAL / "mirror/index.json").read_bytes()
        ).hexdigest(),
        inputs={
            str(p.relative_to(ROOT)): hashlib.sha256(p.read_bytes()).hexdigest()
            for p in args.runs
        },
        source_layouts={
            k: dict(
                metadata_intervals=v["metadata"],
                metadata_bytes=nbytes(v["metadata"]),
                images=[
                    {name: value for name, value in image.items() if name != "blocks"}
                    for image in v["images"]
                ],
            )
            for k, v in layouts.items()
        },
    )
    args.raw_output.parent.mkdir(parents=True, exist_ok=True)
    args.raw_output.write_text(json.dumps({**common, "rows": rows}, indent=2) + "\n")
    details = []
    for row in rows:
        if (
            row["workload"] == "several-large"
            and row["rep"] == 0
            and row["mode"].endswith("stream")
        ):
            details.append(
                {
                    **row,
                    "requests": [
                        {k: v for k, v in request.items() if k != "blocks"}
                        for request in row["requests"]
                    ],
                }
            )
    compact = {
        **common,
        "rows": [
            {k: v for k, v in row.items() if k not in ("requests", "blocks", "sources")}
            for row in rows
        ],
        "several_large_first_rep": details,
    }
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(compact, indent=2) + "\n")
    for row in rows:
        if row["rep"] == 0 and row["mode"].endswith("stream"):
            print(
                row["workload"], row["mode"], json.dumps(row["totals"], sort_keys=True)
            )


if __name__ == "__main__":
    main()
