"""HDF5 working reservations, stream backpressure and bounded bad chunks."""

import time
import zlib

import h5py
import hdf5_input as h
import numpy as np


def main():
    rows = []
    # Source output grows 64-fold; the consumer drops each chunk immediately.
    for edge in [512, 4096]:
        path, ds = h.fixture(
            "memory-" + str(edge), shape=(edge, edge), chunks=(256, 256), holes=False
        )
        for budget in [4 * 1024 * 1024, 8 * 1024 * 1024]:
            req = h.request(path, ds, (0, 0), (edge, edge), budget=budget)
            result = h.run_native(
                req, f"memory-{edge}-{budget}", "discard", "adjacent", workers=2
            )
            assert result["returncode"] == 0, result["error"]
            report = result["result"]["runs"][0]["observed"]
            assert report["decoded_bytes"] == edge * edge * 2
            assert report["scheduler"]["reserved_bytes_peak"] <= budget
            rows.append(
                {
                    "shape": [edge, edge],
                    "output_bytes": report["decoded_bytes"],
                    "buffer_bytes": budget,
                    **h.compact(result),
                    "scheduler": report["scheduler"],
                }
            )
            print("MEMORY PASS", edge, budget, result["peak_rss_bytes"], flush=True)
    for budget in [4 * 1024 * 1024, 8 * 1024 * 1024]:
        small, big = [r for r in rows if r["buffer_bytes"] == budget]
        assert big["peak_rss_bytes"] - small["peak_rss_bytes"] < 32 * 1024 * 1024
    # Decode limits are enforced by hdf5-reader/flate2, before output allocation
    # can follow the malicious stream's larger decoded length.
    p, ds = h.fixture("inflate-bomb")
    with h5py.File(p, "a") as f:
        f[ds].id.write_direct_chunk(
            (0, 0), zlib.compress(b"\0" * (16 * 1024 * 1024)), filter_mask=1
        )
    result = h.run_native(h.request(p, ds), "inflate-bomb", "stream")
    assert result["returncode"] != 0, result
    bomb = h.compact(result)
    # A delayed in-flight GET cannot be interrupted inside reqwest's blocking
    # send. Cancellation remains terminal once that bounded call returns.
    p, ds = h.fixture("delayed-cancel")
    with h.server(p, ds) as s:
        # Delay only payload responses, leaving preparation cheap.
        original = h.Handler.serve

        def delayed(self, head):
            requested = self.headers.get("Range")
            if requested and not head:
                a, b = map(int, requested.removeprefix("bytes=").split("-"))
                if any(
                    a <= offset and offset + size <= b + 1
                    for offset, size in self.server.payload
                ):
                    self.server.payload_started.set()
                    time.sleep(0.25)
            return original(self, head)

        h.Handler.serve = delayed
        try:
            result = h.run_native(
                h.request(s.url, ds),
                "delayed-cancel",
                "cancel-inflight",
                control=s.url.replace("/data.nc", "/control/wait-payload"),
            )
            assert result["returncode"] != 0
            cancel = h.compact(result)
        finally:
            h.Handler.serve = original
    # Same real-product bytes over loopback: inspect one selected logical chunk
    # versus the entire logical grid without acquiring scientific payloads.
    real = h.WORK / "goes19-reference-control.nc"
    index = []
    for label, start, count in [
        ("one", (2500, 2500), (64, 64)),
        ("full-grid", (0, 0), (5424, 5424)),
    ]:
        with h.server(real, "/RRQPE") as s:
            result = h.run_native(
                h.request(s.url, "/RRQPE", start, count), "index-" + label, "inspect"
            )
            assert result["returncode"] == 0, result["error"]
            stats = s.stats()
            assert stats["payload_gets"] == 0
            phase = result["result"]["transport"]["science"]["phases"][
                "hdf5_chunk_index_discovery"
            ]
            index.append(
                {
                    "selection": label,
                    "logical_chunks": len(result["result"]["plan"]["nodes"]) // 2,
                    "index_phase": phase,
                    "http": stats,
                }
            )
    assert sum(v["bytes"] for v in index[0]["index_phase"].values()) == sum(
        v["bytes"] for v in index[1]["index_phase"].values()
    )
    # An independent HDF5 writer places small unrelated allocations between
    # compressed scientific chunks. This exercises bounded-gap overfetch.
    p = h.WORK / "gapped.nc"
    with h5py.File(p, "w", libver="earliest") as f:
        f.create_dataset("y", data=np.arange(128, dtype="f8"))
        f.create_dataset("x", data=np.arange(256, dtype="f8"))
        d = f.create_dataset(
            "signal",
            shape=(128, 256),
            dtype="u2",
            chunks=(64, 64),
            compression="gzip",
            fillvalue=65535,
        )
        d.attrs["_FillValue"] = np.uint16(65535)
        values = np.random.default_rng(49).integers(
            0, 50000, size=(128, 256), dtype="u2"
        )
        for i, (y, x) in enumerate((y, x) for y in [0, 64] for x in [0, 64, 128, 192]):
            d[y : y + 64, x : x + 64] = values[y : y + 64, x : x + 64]
            f.flush()
            f.create_dataset("padding" + str(i), data=np.ones(512, dtype="u1"))
            f.flush()
    gap = []
    for policy in ["individual", "adjacent", "bounded"]:
        with h.server(p, "/signal", latency=0.04) as s:
            req = h.request(s.url, "/signal", (0, 0), (128, 256))
            result = h.run_native(req, "gapped-" + policy, "dump", policy, workers=1)
            checked = h.validate(result, p, req)
            observed = result["result"]["runs"][0]["observed"]
            selected = result["result"]["inspection"]["plan"]["estimates"][
                "selected_compressed_payload_bytes"
            ]
            gap.append(
                {
                    "policy": policy,
                    **h.compact(result),
                    "science": checked,
                    "selected_compressed_bytes": selected,
                    "payload_bytes": observed["source_data_bytes"],
                    "payload_gets": observed["source_data_requests"],
                }
            )
    assert gap[2]["payload_gets"] < gap[0]["payload_gets"]
    assert (
        0
        < gap[2]["payload_bytes"] - gap[2]["selected_compressed_bytes"]
        <= gap[2]["selected_compressed_bytes"] / 4
    )

    # Large sparse layouts must be rejected before allocating a job per cell.
    p = h.WORK / "all-fill-plan.nc"
    with h5py.File(p, "w", libver="earliest") as f:
        g = f.create_group("science")
        g.create_dataset("y", data=np.arange(1024, dtype="f8"))
        g.create_dataset("x", data=np.arange(1024, dtype="f8"))
        g.create_dataset(
            "signal", shape=(1024, 1024), dtype="u2", chunks=(1, 1), fillvalue=7
        )
    req = h.request(p, "/science/signal", (0, 0), (1024, 1024), budget=1024 * 1024)
    req["budget"]["max_nodes"] = 3_000_000
    rejected = h.run_native(req, "all-fill-plan", "stream")
    assert rejected["returncode"] != 0 and "chunk-plan bookkeeping" in rejected["error"]
    assert rejected["peak_rss_bytes"] < 32 * 1024 * 1024

    h.save(
        "resources",
        {
            "memory": rows,
            "all_fill_plan_rejection": h.compact(rejected),
            "gapped_physical_layout": gap,
            "real_product_index_discovery": index,
            "index_discovery_selective": False,
            "index_discovery_limit": "hdf5-reader 0.9.3 traverses all v1 B-tree leaves; metadata cache and buffer cap bound this scan",
            "inflate_bomb": bomb,
            "delayed_inflight_cancellation": cancel,
            "scope": "scheduler working reservations plus separately sampled process RSS; process runtime and consumer retention are outside buffer_bytes",
        },
    )


if __name__ == "__main__":
    main()
