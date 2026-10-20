import json, os, pathlib, subprocess, ctypes.util, statistics, time, zlib

root = pathlib.Path("/src")
p = pathlib.Path(
    os.environ.get(
        "NATIVE_CONSUMER",
        "/src/prototype-output/native-cog/linux-consumer-target/release/tile-ripper-native-stats",
    )
)
assert all(ctypes.util.find_library(x) is None for x in ["gdal", "proj", "tiff"])
sources = json.loads(
    (root / "prototype-output/production-streaming/real/sources.json").read_text()
)["sources"]
req = dict(
    source=dict(kind="assets"),
    spatial=dict(
        kind="bounds", crs="EPSG:32617", bbox=[673000, 3978060, 673960, 3979020]
    ),
    variables=[dict(kind="band", index=1)],
    grid=dict(kind="native"),
    sink=dict(kind="callback"),
    budget=dict(buffer_bytes=16 * 2**20, max_nodes=100000, chunk_edge=256),
)
req["source"]["assets"] = [
    dict(
        id=a["id"],
        locator=str(
            root / f"prototype-output/production-streaming/real/mirror/{i}.tif"
        ),
        format="cog",
        datetime=a["datetime"],
        etag=a["etag"],
    )
    for i, a in enumerate(sources)
]
path = root / "prototype-output/native-cog/linux-runtime-request.json"
path.write_text(json.dumps(req))
result = subprocess.run([str(p), str(path)], capture_output=True, text=True, check=True)
rows = [json.loads(s) for s in result.stdout.splitlines()]
assert rows[-1]["type"] == "complete" and rows[-1]["chunks"] == 16
expected = (root / "prototype-output/native-cog/exact/many-small.bin").read_bytes()
for i, row in enumerate(rows[1:-1]):
    assert row["asset_id"] == sources[i]["id"]
    assert row["adler32_le"] == zlib.adler32(expected[i * 18432 : (i + 1) * 18432])
    assert row["window"] == dict(x=7300, y=2100, width=96, height=96)
    assert row["crs"] == "EPSG:32617" and row["dtype"] == "uint16"
(root / "prototype-output/native-cog/linux-runtime-stdout.jsonl").write_text(
    result.stdout
)
cli = pathlib.Path(
    os.environ.get(
        "NATIVE_CLI",
        "/src/prototype-output/native-cog/linux-cli-target/release/tile-ripper",
    )
)
startup = []
for _ in range(21):
    t = time.perf_counter()
    subprocess.run([str(cli), "--help"], stdout=subprocess.DEVNULL, check=True)
    startup.append(time.perf_counter() - t)
record = dict(
    platform=os.uname().machine,
    gdal_proj_libtiff_absent=True,
    independent_expected_values_verified=True,
    chunks=rows[-1]["chunks"],
    startup_seconds=dict(
        median=statistics.median(startup), min=min(startup), max=max(startup)
    ),
    dynamic_libraries=subprocess.check_output(["ldd", str(p)], text=True),
    binaries={b.name: b.stat().st_size for b in [p, cli]},
)
(root / "prototype-output/native-cog/linux-runtime.json").write_text(
    json.dumps(record, indent=2)
)
print(json.dumps(record, indent=2))
