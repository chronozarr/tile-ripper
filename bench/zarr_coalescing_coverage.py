"""Exact science and physical coverage when logical windows interleave in a shard."""

import json
import numpy as np
from zarr.codecs.sharding import _ShardIndex
from zarr.core.buffer.cpu import Buffer
from zarr.core.sync import sync
import zarr_coalescing as c


def main():
    work = c.WORK / "physical-coverage"
    work.mkdir(parents=True, exist_ok=True)
    shape = (16, 32, 32)
    source = c.h.fixture(
        work / "source",
        True,
        "raw",
        shape=shape,
        inner=(2, 8, 8),
        factor=(4, 4, 4),
        consolidate=True,
    )
    group = c.zarr.open_group(str(source), mode="r")
    for name in c.VARS:
        array = group[name]
        codec = array.metadata.codecs[0]
        cps = tuple(o // i for o, i in zip(array.shards, array.chunks))
        n = int(np.prod(cps)) * 16 + 4
        for path in sorted((source / name / "c").rglob("*")):
            if not path.is_file():
                continue
            data = path.read_bytes()
            decoded = sync(codec._decode_shard_index(Buffer.from_bytes(data[-n:]), cps))
            index = _ShardIndex(cps, decoded.offsets_and_lengths.copy())
            members = [
                i for i in np.ndindex(cps) if index.get_chunk_slice(i) is not None
            ]
            members.sort(
                key=lambda i: (
                    int(np.ravel_multi_index(i, cps)) % 32,
                    int(np.ravel_multi_index(i, cps)) // 32,
                )
            )
            spans = {i: index.get_chunk_slice(i) for i in members}
            body = bytearray()
            for i in members:
                start = len(body)
                a, b = spans[i]
                body.extend(data[a:b])
                index.set_chunk_slice(i, slice(start, len(body)))
            path.write_bytes(body + sync(codec._encode_shard_index(index)).to_bytes())
    server = c.Server(source).start()
    r = c.h.request(server.url, work / "unused", shape=shape, dense=True)
    r["budget"]["chunk_edge"] = 8
    r["time"] = {
        "kind": "instants",
        "values": [
            str(
                np.datetime64("2025-01-01T00:00:00.123456789", "ns")
                + np.timedelta64(i * 3600, "s")
            )
            + "Z"
            for i in range(16)
        ],
    }
    try:
        result = c.h.run(
            r, work / "request.json", mode="dump", dump=work / "chunks", env=c.ENV
        )
        science = c.h.verify(r, source, result, work / "chunks")
        plans = {
            v["key"]: v
            for array in result["inspection"]["sources"][0]["zarr_v3"].values()
            for v in array["physical_read_plan"]
        }
        rows = []
        for key, plan in sorted(plans.items()):
            size = len(server.cache[key])
            spans = [
                tuple(v["span"])
                for v in server.records
                if v["method"] == "GET"
                and v["key"] == key
                and tuple(v["span"]) != (size - 1028, size)
            ]
            unique = 0
            end = 0
            for a, b in sorted(spans):
                unique += max(0, b - max(a, end))
                end = max(end, b)
            total = sum(b - a for a, b in spans)
            rows.append(
                dict(
                    key=key,
                    identity=plan["identity"],
                    selected_plan=plan,
                    actual_payload_ranges=spans,
                    selected_unique_bytes=plan["unique_selected_bytes"],
                    actual_body_bytes=total,
                    actual_unique_bytes=unique,
                    repeated_body_bytes=total - unique,
                )
            )
        selected = sum(v["selected_unique_bytes"] for v in rows)
        total = sum(v["actual_body_bytes"] for v in rows)
        repeated = sum(v["repeated_body_bytes"] for v in rows)
        assert repeated > 0 and total <= 2 * selected
        record = dict(
            science=science,
            shards=rows,
            selected_unique_bytes=selected,
            actual_payload_bytes=total,
            repeated_payload_bytes=repeated,
            traffic=c.traffic(server, source, True),
            scheduler_reserved_peak=result["runs"][0]["observed"]["scheduler"][
                "reserved_bytes_peak"
            ],
        )
        (work / "result.json").write_text(json.dumps(result))
        (work / "http.json").write_text(json.dumps(server.records))
        c.h.BINARY = c.BASE / "zarr_subset_oracle"
        baseline = c.h.run(r, work / "baseline-request.json", env=c.ENV)
        assert baseline["runs"][0]["chunks"] == result["runs"][0]["chunks"]
        record["chunk_order_and_digests_match_baseline"] = True
        (c.DOCS / "physical-coverage.json").write_text(json.dumps(record, indent=2))
        print(
            dict(
                selected=selected,
                total=total,
                repeated=repeated,
                requests=record["traffic"]["payload_requests"],
            )
        )
    finally:
        server.shutdown()
        server.server_close()


if __name__ == "__main__":
    main()
