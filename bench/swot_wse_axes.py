"""Independent chunked-axis, fault, mutation and reservation gates; loopback only."""

import argparse
import hashlib
import itertools
import json
import struct
import zlib

import h5py
import hdf5_input as h
import numpy as np
import swot_wse as w

WORK = w.WORK / "axes"
WORK.mkdir(parents=True, exist_ok=True)
h.WORK = WORK


def fixture(
    file_name,
    dtype="<f8",
    compression="gzip",
    shuffle=True,
    fletcher=False,
    shape=(37, 53),
    blocks=(5, 7),
    holes=False,
    latest=False,
    science_blocks=(9, 11),
):
    p, ds = h.fixture(
        file_name, dtype="<f4", shape=shape, chunks=science_blocks, holes=False
    )
    with h5py.File(p, "a") as f:
        g = f["science"]
        for i, name in enumerate(["y", "x"]):
            # Remove the old dimension scale reference before replacing the axis.
            g["signal"].dims[i].detach_scale(g[name])
            del g[name]
            n = shape[i]
            values = (
                10000 - np.arange(n) * 10 if i == 0 else 500 + np.arange(n) * 5
            ).astype(dtype)
            if np.dtype(dtype).kind == "f":
                values += np.sin(np.arange(n)).astype(dtype) * 0.03125
            if latest:
                # A modern-index axis in an otherwise legacy admitted file.
                modern = WORK / "modern-axis.h5"
                with h5py.File(modern, "w", libver="latest") as m:
                    m.create_dataset(name, data=values, chunks=(blocks[i],))
                with h5py.File(modern, "r") as m:
                    m.copy(name, g)
                axis = g[name]
            else:
                axis = g.create_dataset(
                    name,
                    shape=(n,),
                    dtype=dtype,
                    chunks=(blocks[i],),
                    compression=compression,
                    shuffle=shuffle,
                    fletcher32=fletcher,
                    fillvalue=-999 if np.dtype(dtype).kind != "u" else 65535,
                )
                for at in range(0, n, blocks[i]):
                    if not holes or at != blocks[i]:
                        axis[at : at + blocks[i]] = values[at : at + blocks[i]]
            axis.make_scale(name)
            axis.attrs["units"] = np.bytes_("m")
            axis.attrs["scale_factor"] = np.float64(0.5)
            axis.attrs["add_offset"] = np.float64(10)
            g["signal"].dims[i].attach_scale(axis)
    return p, ds


def science():
    rows = []
    selections = [((2, 2), (20, 25), (1, 1)), ((1, 2), (6, 7), (7, 8))]
    for dtype, pipeline in itertools.product(
        [end + t for end in ["<", ">"] for t in ["i2", "u2", "i4", "u4", "f4", "f8"]],
        ["raw", "shuffle-gzip", "checksum"],
    ):
        p, ds = fixture(
            dtype.replace("<", "le").replace(">", "be") + "-" + pipeline,
            dtype,
            None if pipeline == "raw" else "gzip",
            pipeline != "raw",
            pipeline == "checksum",
        )
        with h.server(p, ds) as srv:
            for selection, index in itertools.product(
                selections, ["full", "selective"]
            ):
                srv.reset()
                r = h.request(srv.url, ds, *selection)
                with w.policies(index):
                    out = h.run_native(r, f"{p.stem}-{selection[2][0]}-{index}")
                checked = h.validate(out, p, r)
                binding = out["result"]["inspection"]["sources"][0][
                    "object_identities"
                ]["datasets"][0]
                expected_ranges = []
                with h5py.File(p, "r") as f:
                    for i, axis in enumerate(binding["axes"]):
                        d = f[axis["path"]]
                        a, n, s = (
                            r["spatial"][key][i] for key in ["start", "count", "step"]
                        )
                        indices = sorted(
                            {(a + k * s) // d.chunks[0] * d.chunks[0] for k in range(n)}
                        )
                        assert [
                            c["chunk_offset"] for c in axis["storage"]["chunks"]
                        ] == indices
                        for offset in indices:
                            c = d.id.get_chunk_info_by_coord((offset,))
                            expected_ranges.append(
                                (c.byte_offset, c.byte_offset + c.size)
                            )
                observed = out["result"]["runs"][0]["observed"]["transport"]["science"]
                phase = observed["phases"]["hdf5_coordinate_chunks"]
                assert sum(v["bytes"] for v in phase.values()) == sum(
                    b - a for a, b in expected_ranges
                )
                for a, b in expected_ranges:
                    matches = [
                        e
                        for e in srv.events
                        if e["method"] == "GET" and (e["start"], e["end"]) == (a, b)
                    ]
                    assert len(matches) == 1, (a, b, matches)
                rows.append(
                    {
                        "dtype": dtype,
                        "pipeline": pipeline,
                        "index": index,
                        "selection": r["spatial"],
                        "agreement": checked,
                        "coordinate_ranges": expected_ranges,
                        "coordinate_reserved_peak": max(
                            a["storage"]["prepare_reserved_bytes_peak"]
                            for a in binding["axes"]
                        ),
                        **h.compact(out),
                    }
                )
    # Preserve the old compact path as well as the inherited contiguous matrix.
    p, ds = fixture("compact-coordinates")
    with h5py.File(p, "a") as f:
        g = f["science"]
        for i, name in enumerate(["y", "x"]):
            values = g[name][:]
            g["signal"].dims[i].detach_scale(g[name])
            del g[name]
            props = h5py.h5p.create(h5py.h5p.DATASET_CREATE)
            props.set_layout(h5py.h5d.COMPACT)
            axis_id = h5py.h5d.create(
                g.id,
                name.encode(),
                h5py.h5t.py_create(values.dtype),
                h5py.h5s.create_simple(values.shape),
                dcpl=props,
            )
            axis_id.write(h5py.h5s.ALL, h5py.h5s.ALL, values)
            axis = g[name]
            axis.make_scale(name)
            axis.attrs["units"] = np.bytes_("m")
            g["signal"].dims[i].attach_scale(axis)
    for index in ["full", "selective"]:
        with w.policies(index):
            req = h.request(p, ds, (2, 2), (20, 25))
            out = h.run_native(req, "compact-" + index)
        rows.append(
            {
                "layout": "compact",
                "index": index,
                "agreement": h.validate(out, p, req),
                **h.compact(out),
            }
        )
    w.save(
        "axes-science",
        {
            "cells": rows,
            "all_selected_coordinate_chunks_once": True,
            "includes_descending_order_partial_edges_and_skipped_chunks": True,
        },
    )
    print(json.dumps({"axis_scientific_cells": len(rows)}))


def faults():
    rows = []

    def reject(p, ds, label, selection=((0, 0), (12, 15), (1, 1)), error=None):
        for index in ["full", "selective"]:
            with w.policies(index):
                out = h.run_native(
                    h.request(p, ds, *selection), f"fault-{label}-{index}", "stream"
                )
            assert out["returncode"] != 0, (label, out)
            if error:
                assert error in out["error"], (label, out["error"])
            rows.append({"fault": label, "index": index, **h.compact(out)})

    p, ds = fixture("missing-coordinate", holes=True)
    reject(p, ds, "absent-coordinate-chunk", error="absent or invalid")
    # Missing chunks outside the selection are allowed; both policies still
    # validate all visible index structure under their documented scopes.
    for index in ["full", "selective"]:
        with w.policies(index):
            r = h.request(p, ds, (0, 0), (2, 2))
            out = h.run_native(r, f"unselected-coordinate-hole-{index}")
        h.validate(out, p, r)
        rows.append(
            {
                "fault": "unselected-hole-allowed",
                "index": index,
                "expected_success": True,
                **h.compact(out),
            }
        )

    for label in [
        "nonfinite",
        "cf-fill",
        "implicit-cf-fill",
        "compressed-truncated",
        "compressed-corrupt",
        "deflate-corrupt",
        "inflate-bomb",
        "bad-chunk-pointer",
        "bad-index-pointer",
        "rank2-axis",
        "unsupported-filter",
        "huge-axis-chunk",
    ]:
        p, ds = fixture(
            label,
            fletcher=label == "compressed-corrupt",
            blocks=(37, 53) if label == "huge-axis-chunk" else (5, 7),
        )
        with h5py.File(p, "a") as f:
            axis = f["science/y"]
            c = axis.id.get_chunk_info_by_coord((0,))
            offset, length = c.byte_offset, c.size
            if label == "nonfinite":
                axis[0] = np.nan
            elif label == "cf-fill":
                axis.attrs["_FillValue"] = np.float64(10000)
            elif label == "implicit-cf-fill":
                g = f["science"]
                g["signal"].dims[0].detach_scale(axis)
                del g["y"]
                axis = g.create_dataset(
                    "y", data=np.arange(37, dtype="f8"), chunks=(5,)
                )
                axis.attrs["_FillValue"] = np.float64(np.nan)
            elif label == "compressed-truncated":
                _, raw = axis.id.read_direct_chunk((0,))
                axis.id.write_direct_chunk((0,), raw[: len(raw) // 2])
            elif label == "inflate-bomb":
                axis.id.write_direct_chunk(
                    (0,), zlib.compress(b"\0" * (16 * 1024 * 1024)), filter_mask=1
                )
            elif label in ["rank2-axis", "unsupported-filter"]:
                g = f["science"]
                g["signal"].dims[0].detach_scale(axis)
                del g["y"]
                if label == "rank2-axis":
                    g.create_dataset("y", data=np.zeros((37, 1)), chunks=(5, 1))
                else:
                    g.create_dataset(
                        "y",
                        data=np.arange(37, dtype="f8"),
                        chunks=(5,),
                        compression="lzf",
                    )
        if label in [
            "compressed-corrupt",
            "deflate-corrupt",
            "bad-chunk-pointer",
            "bad-index-pointer",
        ]:
            raw = bytearray(p.read_bytes())
            if label == "compressed-corrupt":
                raw[offset + length - 1] ^= 1
            elif label == "deflate-corrupt":
                raw[offset + length // 2] ^= 128
            else:
                needle = struct.pack("<Q", offset)
                found = raw.find(needle)
                assert found >= 0
                if label == "bad-chunk-pointer":
                    raw[found : found + 8] = struct.pack("<Q", len(raw) + 8192)
                else:
                    root = raw.rfind(b"TREE\x01", 0, found)
                    assert root >= 0
                    # Corrupt the actual layout's index pointer, located by its
                    # independently written root address (test fixture only).
                    pointer = struct.pack("<Q", root)
                    position = raw.find(pointer)
                    assert 0 <= position < root
                    raw[position : position + 8] = struct.pack("<Q", len(raw) + 8192)
            p.write_bytes(raw)
        if label == "huge-axis-chunk":
            # Declared huge native chunk, tiny selected output: early admission
            # must fail instead of allowing the decoder to allocate that chunk.
            with h5py.File(p, "a") as f:
                g = f["science"]
                g["signal"].dims[0].detach_scale(g["y"])
                del g["y"]
                g.create_dataset(
                    "y",
                    shape=(37,),
                    maxshape=(None,),
                    dtype="f8",
                    chunks=(1048576,),
                    compression="gzip",
                )
        reject(p, ds, label)

    # Unsupported modern chunk indexes on an axis; copy preserves v4 layout.
    p, ds = fixture("modern-axis", latest=True)
    reject(p, ds, "modern-axis-index", error="v1 B-tree")

    # Exact mutations to both dependency types through version-bound HTTP.
    p, ds = fixture("mutations")
    with h5py.File(p, "r") as f:
        offsets = {
            name: f[path]
            .id.get_chunk_info_by_coord((0,) if name == "axis" else (0, 0))
            .byte_offset
            for name, path in [("axis", "science/y"), ("data", "science/signal")]
        }
    original = h.Handler.serve
    for dependency, mode in itertools.product(["axis", "data"], ["before", "during"]):

        def mutate(self, head, dependency=dependency):
            if self.path == "/control":
                raw = bytearray(self.server.raw)
                raw[offsets[dependency]] ^= 1
                self.server.raw = bytes(raw)
                self.server.etag = '"' + hashlib.sha256(raw).hexdigest() + '"'
            return original(self, head)

        h.Handler.serve = mutate
        try:
            with h.server(p, ds) as srv:
                out = h.run_native(
                    h.request(srv.url, ds, (0, 0), (30, 40)),
                    f"mutation-{dependency}-{mode}",
                    mode,
                    control=srv.url.replace("/data.nc", "/control"),
                )
                assert out["returncode"] != 0, "mutation returned successful completion"
                rows.append(
                    {"fault": f"{dependency}-mutation-{mode}", **h.compact(out)}
                )
        finally:
            h.Handler.serve = original
    for mode in ["cancel", "cancel-inflight"]:
        with h.server(p, ds, latency=0.01) as srv:
            out = h.run_native(
                h.request(srv.url, ds, (0, 0), (30, 40)),
                mode,
                mode,
                control=srv.url.replace("/data.nc", "/control/wait-payload"),
            )
            assert out["returncode"] != 0
            rows.append({"fault": mode, **h.compact(out)})
    w.save("axes-faults", {"cells": rows})
    print(json.dumps({"axis_fault_cells": len(rows)}))


def resources():
    rows = []
    for edge, budget in itertools.product(
        [512, 4096], [4 * 1024 * 1024, 8 * 1024 * 1024]
    ):
        p, ds = fixture(
            f"memory-{edge}",
            shape=(edge, edge),
            blocks=(128, 128),
            science_blocks=(256, 256),
        )
        r = h.request(p, ds, (0, 0), (edge, edge), budget=budget)
        out = h.run_native(r, f"memory-{edge}-{budget}", "discard")
        assert out["returncode"] == 0, out["error"]
        report = out["result"]["runs"][0]["observed"]
        assert report["decoded_bytes"] == edge * edge * 4
        assert report["scheduler"]["reserved_bytes_peak"] <= budget
        rows.append(
            {
                "edge": edge,
                "budget": budget,
                "scheduler": report["scheduler"],
                **h.compact(out),
            }
        )
    # A malicious encoded extent is bounded before network/local payload reads.
    p, ds = fixture("encoded-extent")
    with h5py.File(p, "a") as f:
        f.create_dataset(
            "unrelated_padding", data=np.zeros(8 * 1024 * 1024, dtype="u1")
        )
        c = f["science/y"].id.get_chunk_info_by_coord((0,))
    raw = bytearray(p.read_bytes())
    pointer = raw.find(struct.pack("<Q", c.byte_offset))
    assert pointer >= 0
    # Rank-one key: size/mask + two u64 coordinates, then address.
    struct.pack_into("<I", raw, pointer - 24, 2 * 1024 * 1024)
    p.write_bytes(raw)
    out = h.run_native(
        h.request(p, ds, (0, 0), (1, 1), budget=4 * 1024 * 1024),
        "encoded-extent",
        "stream",
    )
    assert out["returncode"] != 0
    assert "coordinate decoding exceeds working memory" in out["error"]
    rows.append(
        {"fault": "encoded-extent", "budget": 4 * 1024 * 1024, **h.compact(out)}
    )
    w.save("axes-resources", {"cells": rows})
    print(json.dumps({"resource_cells": len(rows)}))


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("action", choices=["science", "faults", "resources"])
    globals()[parser.parse_args().action]()
