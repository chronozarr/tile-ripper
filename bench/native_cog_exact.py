"""Compare every native UInt16 sample directly with independent rasterio bytes."""

import hashlib
import json
import os
from pathlib import Path
import subprocess

import rasterio
from rasterio.windows import Window
import real_multiscene as m
import cog_io_trace as trace


def main():
    dest = m.ROOT / "prototype-output/native-cog/exact"
    dest.mkdir(exist_ok=True)
    sources = json.loads((m.WORK / "sources.json").read_text())["sources"]
    mirror = m.Mirror(m.WORK / "mirror", sources)
    digest = mirror.verify_pages()
    rows = []
    binary = Path(os.environ.get("TILE_RIPPER_ORACLE_BINARY", m.ROOT / "target/release/examples/native_cog_oracle"))
    for name, (x, y, width, height) in trace.WINDOWS.items():
        assets = sources if name == "many-small" else sources[:6]
        req = dict(
            source=dict(
                kind="assets",
                assets=[
                    dict(
                        id=a["id"],
                        locator=str(mirror.path(f"/{i}.tif")),
                        format="cog",
                        datetime=a["datetime"],
                    )
                    for i, a in enumerate(assets)
                ],
            ),
            spatial=dict(
                kind="bounds",
                crs="EPSG:32617",
                bbox=[
                    600000 + x * 10,
                    4000020 - (y + height) * 10,
                    600000 + (x + width) * 10,
                    4000020 - y * 10,
                ],
            ),
            variables=[dict(kind="band", index=1)],
            grid=dict(kind="native"),
            sink=dict(kind="callback"),
            budget=dict(buffer_bytes=16 * 2**20, max_nodes=100000, chunk_edge=256),
        )
        path = dest / f"{name}.json"
        path.write_text(json.dumps(req))
        expected = dest / f"{name}.bin"
        with expected.open("wb") as output:
            for a in req["source"]["assets"]:
                with rasterio.open(a["locator"]) as ds:
                    for row in range(y, y + height, 256):
                        for col in range(x, x + width, 256):
                            output.write(
                                ds.read(
                                    1,
                                    window=Window(
                                        col,
                                        row,
                                        min(256, x + width - col),
                                        min(256, y + height - row),
                                    ),
                                )
                                .astype("<u2", copy=False)
                                .tobytes()
                            )
        response = subprocess.check_output(
            [str(binary), str(path), str(expected)], text=True
        )
        result = json.loads(response)
        assert result["samples_compared_bit_for_bit"] == len(assets) * width * height
        rows.append(
            dict(
                workload=name,
                **result,
                expected_sha256=hashlib.sha256(expected.read_bytes()).hexdigest(),
            )
        )
        print(name, result, flush=True)
    report = dict(
        passed=True,
        rows=rows,
        mirror_index_sha256=digest,
        binary_sha256=hashlib.sha256(binary.read_bytes()).hexdigest(),
        harness_sha256=hashlib.sha256(Path(__file__).read_bytes()).hexdigest(),
    )
    (dest / "report.json").write_text(json.dumps(report, indent=2))


if __name__ == "__main__":
    main()
