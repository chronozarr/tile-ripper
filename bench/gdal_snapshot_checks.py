"""Small loopback tests for the independently implemented GDAL snapshot control."""

import hashlib
import json
import subprocess
import sys
import threading

import numpy as np
import rasterio
from rasterio.transform import from_origin

import acquisition_engine as engine
from consumer_validation import verify
import real_multiscene as m

ROOT = m.ROOT
WORK = ROOT / "prototype-output/production-streaming/gdal-snapshot-checks"


class Handler(engine.Handler):
    def serve(self, body):
        # Serial asset acquisition guarantees that source 0's complete native
        # plane has already been consumed when source 1's metadata is opened.
        if body and self.path == "/1.tif" and self.server.mutate:
            self.server.mutate = False
            source = self.server.objects["/0.tif"]
            data = bytearray(source.read_bytes())
            data[-1] ^= 1
            source.write_bytes(data)
            self.server.etags["/0.tif"] = '"changed-after-source-zero-read"'
        super().serve(body)


def main():
    WORK.mkdir(parents=True, exist_ok=True)
    objects = {}
    refs = {}
    for i in range(2):
        path = WORK / f"{i}.tif"
        values = (np.arange(64 * 64).reshape(64, 64) + i).astype("uint16")
        with rasterio.open(
            path,
            "w",
            driver="GTiff",
            width=64,
            height=64,
            count=1,
            dtype="uint16",
            crs="EPSG:32617",
            transform=from_origin(600000, 4000000, 10, 10),
            nodata=0,
            tiled=True,
            blockxsize=32,
            blockysize=32,
        ) as ds:
            ds.write(values, 1)
            ds.scales = (0.0001,)
            ds.offsets = (-0.1,)
        objects[f"/{i}.tif"] = path
        refs[f"asset-{i}"] = dict(
            local=path, format="cog", datetime="2025-01-01T00:00:00Z"
        )
    server = engine.Server(("127.0.0.1", 0), Handler)
    server.configure(objects, 0, None)
    server.mutate = False
    threading.Thread(target=server.serve_forever, daemon=True).start()
    req = dict(
        source=dict(
            kind="assets",
            assets=[
                dict(
                    id=f"asset-{i}",
                    format="cog",
                    datetime="2025-01-01T00:00:00Z",
                    locator=f"http://127.0.0.1:{server.server_port}/{i}.tif",
                    etag=server.etags[f"/{i}.tif"],
                )
                for i in range(2)
            ],
        ),
        spatial=dict(
            kind="bounds", crs="EPSG:32617", bbox=[600000, 3999360, 600640, 4000000]
        ),
        variables=[dict(kind="band", index=1)],
        grid=dict(kind="native"),
        sink=dict(kind="callback"),
        budget=dict(chunk_edge=64, buffer_bytes=1048576, max_nodes=1000),
    )
    path = WORK / "request.json"
    path.write_text(json.dumps(req))
    command = [
        sys.executable,
        str(ROOT / "bench/gdal_snapshot_worker.py"),
        str(path),
        "1",
    ]
    results = []
    try:
        good = subprocess.run(command, capture_output=True, text=True, timeout=60)
        assert good.returncode == 0, good.stderr
        value = json.loads(good.stdout)
        for key, entry in server.counts["paths"].items():
            if key.startswith("GET "):
                expected = server.etags[key.split()[1]]
                assert all(
                    signature.endswith(expected)
                    for signature in entry["request_signatures"]
                )
        checked = verify(value["rows"], refs)
        assert checked["chunks"] == 2 and value["rows"][-1]["type"] == "complete"
        results.append(dict(case="conditional-native-statistics", passed=True))
        for output in [None, WORK / "mutated-output"]:
            server.etags = {
                k: '"' + hashlib.sha256(v.read_bytes()).hexdigest() + '"'
                for k, v in objects.items()
            }
            for i, a in enumerate(req["source"]["assets"]):
                a["etag"] = server.etags[f"/{i}.tif"]
            path.write_text(json.dumps(req))
            server.mutate = True
            cmd = command + ([str(output)] if output else [])
            failed = subprocess.run(cmd, capture_output=True, text=True, timeout=60)
            assert failed.returncode != 0 and not failed.stdout
            assert not server.mutate, "mutation hook did not run"
            if output is not None:
                assert not output.exists()
            results.append(
                dict(
                    case="late-mutation-file-cleanup"
                    if output
                    else "late-mutation-no-completion",
                    passed=True,
                )
            )
    finally:
        server.shutdown()
        server.server_close()
    (WORK / "receipt.json").write_text(json.dumps(results, indent=2))
    print(json.dumps(results, indent=2))


if __name__ == "__main__":
    main()
