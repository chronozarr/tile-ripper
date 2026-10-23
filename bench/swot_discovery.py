"""Bounded CMR workflow acceptance; frozen catalog tests never contact NASA.

Run with the existing Python 3.10 scientific-reference environment. Live mode
requires an explicit private Earthdata credential path. Raw artifacts remain
under ignored prototype-output; compact evidence and exact identities are saved.
"""

import argparse
import copy
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
import os
from pathlib import Path
import subprocess
import tempfile
import threading
import time
import urllib.parse

import swot_job as job

ROOT = Path(__file__).resolve().parents[1]
WORK = ROOT / "prototype-output/swot-discovery"
DOCS = ROOT / "docs/swot-discovery"
BINARY = ROOT / "target/release/tile-ripper"
WORK.mkdir(parents=True, exist_ok=True)


def write(path, value):
    path.write_text(json.dumps(value, indent=2) + "\n")


def call(args, destination):
    env = os.environ.copy()
    env.update(
        TILE_RIPPER_HDF5_METADATA_POLICY="page16k",
        TILE_RIPPER_WORKERS="1",
        TILE_RIPPER_PER_HOST="1",
    )
    start = time.perf_counter()
    process = subprocess.run(
        [str(BINARY), *map(str, args)], capture_output=True, timeout=120, env=env
    )
    destination.write_bytes(process.stdout)
    destination.with_suffix(".stderr.txt").write_bytes(process.stderr)
    return {
        "returncode": process.returncode,
        "wall_seconds": time.perf_counter() - start,
        "error": process.stderr.decode(),
        "stdout": str(destination),
    }


class FrozenCatalog:
    def __init__(self):
        self.requests = []
        owner = self

        class Handler(BaseHTTPRequestHandler):
            def log_message(self, *_):
                pass

            def do_GET(self):
                assert not self.headers.get("Authorization") and not self.headers.get(
                    "Cookie"
                )
                owner.requests.append(self.path)
                url = urllib.parse.urlparse(self.path)
                params = urllib.parse.parse_qs(url.query)
                if url.path.endswith("collections.umm_json"):
                    data = json.loads(
                        (ROOT / "tests/data/cmr/collection.json").read_text()
                    )
                    after = None
                else:
                    assert params["collection_concept_id"] == ["C3233944993-POCLOUD"]
                    assert params["sort_key[]"] == ["start_date", "producer_granule_id"]
                    assert "temporal" in params and "bounding_box" in params
                    name = (
                        "held"
                        if params["bounding_box"][0].startswith("-179")
                        else "original"
                    )
                    data = json.loads(
                        (ROOT / f"tests/data/cmr/{name}.json").read_text()
                    )
                    size = int(params["page_size"][0])
                    offset = json.loads(self.headers.get("CMR-Search-After", "[0]"))[0]
                    data["items"] = data["items"][offset : offset + size]
                    after = json.dumps([offset + size]) if data["items"] else None
                body = json.dumps(data, separators=(",", ":")).encode()
                self.send_response(200)
                self.send_header("Content-Length", str(len(body)))
                if after:
                    self.send_header("CMR-Search-After", after)
                self.end_headers()
                self.wfile.write(body)

        self.server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
        self.thread = threading.Thread(target=self.server.serve_forever, daemon=True)
        self.thread.start()
        self.url = f"http://127.0.0.1:{self.server.server_port}/search/"

    def close(self):
        self.server.shutdown()
        self.server.server_close()
        self.thread.join()


def catalog(directory, fixture=None):
    results, reports = [], []
    protocol = json.loads((DOCS / "protocol.json").read_text())
    for row in protocol["known_controls"]:
        path = directory / (row["name"] + ".json")
        args = [
            "discover",
            "--collection",
            "SWOT_L2_HR_Raster_D",
            "--bbox",
            ",".join(map(str, row["bbox"])),
            "--start",
            row["start"],
            "--end",
            row["end"],
            "--limit",
            "20",
            "--page-size",
            "2",
            "--output",
            path,
        ]
        if fixture:
            args += ["--cmr-fixture", fixture.url]
        result = call(args, directory / (row["name"] + "-stdout.json"))
        assert result["returncode"] == 0, result
        report = json.loads(path.read_text())
        assert report["status"] == "complete" and len(report["candidates"]) == 6
        assert all(
            c["scientific_profile"] == "not_inspected"
            and c["native_selection"] == "unresolved"
            for c in report["candidates"]
        )
        assert report["statistics"]["http_requests"] == 5
        reports.append(path)
        results.append({**result, "report": report})
    return reports, results


def selection(catalogs, template=None):
    template = template or job.manifest()
    ids = ["G3927559976-POCLOUD", "G3927359490-POCLOUD"]
    observations = []
    for concept_id, entry in zip(ids, template["observations"]):
        spatial = entry["request"]["spatial"]
        observations.append(
            {
                "concept_id": concept_id,
                "observation_id": entry["id"],
                "window": {k: spatial[k] for k in ["start", "count", "step"]},
                "quality_policy": entry["quality_policy"],
                "expected_etag": entry["request"]["source"]["assets"][0].get("etag"),
            }
        )
    return {
        "catalogs": list(map(str, catalogs)),
        "selection": {
            "observations": observations,
            "source_budget": template["observations"][0]["request"]["budget"],
            "limits": template["limits"],
        },
    }


def offline():
    directory = Path(tempfile.mkdtemp(prefix="frozen-", dir=WORK))
    fixture = FrozenCatalog()
    try:
        _, results = catalog(directory, fixture)
        receipt = {
            "catalogs": results,
            "public_provider_requests": 0,
            "fixture_requests": len(fixture.requests),
            "all_passed": True,
        }
        write(DOCS / "frozen-workflow.json", receipt)
        print("frozen scientist catalog workflow passed", flush=True)
    finally:
        fixture.close()


def inspect(directory, selected, netrc, name):
    path = directory / (name + "-selection.json")
    manifest = directory / (name + "-job.json")
    write(path, selected)
    result = call(
        [
            "discover-job",
            path,
            "--auth",
            "earthdata",
            "--netrc",
            netrc,
            "--output",
            manifest,
        ],
        directory / (name + "-review.json"),
    )
    if result["returncode"]:
        return result, None
    result["manifest"] = json.loads(manifest.read_text())
    review = json.loads(Path(result["stdout"]).read_text())
    result["inspection_seconds"] = review["inspection_seconds"]
    result["construction_admission_seconds"] = review.get(
        "construction_admission_seconds"
    )
    result["admission"] = review["admission"]
    result["native_preparation"] = native_preparation(review)
    result["inspected"] = [
        {
            k: source[k]
            for k in [
                "concept_id",
                "scientific_profile",
                "selection",
                "source_identity",
                "source_shape",
                "explicit_selection",
                "quality_policy",
                "catalog",
            ]
        }
        for source in review["observations"]
    ]
    return result, result["manifest"]


def native_preparation(review):
    rows = []
    for observation in review["native_plan"]["observations"]:
        inspection = observation["inspection"]
        transports = inspection["transport"].values()
        rows.append(
            {
                "observation_id": observation["id"],
                "native_planning_seconds": inspection["planning_seconds"],
                "http_requests": sum(t["requests"] for t in transports),
                "source_body_bytes": sum(
                    t["bytes"] for t in inspection["transport"].values()
                ),
            }
        )
    return rows


def source_bytes(row):
    return sum(
        sum(t["bytes"] for t in source["observed"]["transport"].values())
        + source["job_final_validation"]["transport"]["bytes"]
        for source in row["receipt"]["observations"]
    )


def live(netrc):
    directory = Path(tempfile.mkdtemp(prefix="live-", dir=WORK))
    catalogs, discovery = catalog(directory)
    for path in catalogs:
        (DOCS / ("known-" + path.name)).write_bytes(path.read_bytes())
    selected = selection(catalogs)
    generated, manifest = inspect(directory, selected, netrc, "qualified")
    assert manifest is not None, generated
    checkpoint = directory / "checkpoint"
    auth = ["--auth", "earthdata", "--netrc", str(netrc)]
    plan = job.run(
        "discovered-plan", manifest, command="plan", checkpoint=checkpoint, extra=auth
    )
    assert plan["returncode"] == 0, plan["error"]
    first = job.run("discovered-fetch", manifest, checkpoint=checkpoint, extra=auth)
    first["science"] = job.verify(first, manifest)
    replay = job.run(
        "discovered-reconstruction", manifest, checkpoint=checkpoint, extra=auth
    )
    replay["science"] = job.verify(replay, manifest)
    for row in [first, replay]:
        for entry, result in zip(
            manifest["observations"], row["receipt"]["observations"]
        ):
            assert result["catalog"] == entry["catalog"]
            assert (
                result["source"]["object_identities"]["identity"]["url"]
                == entry["request"]["source"]["assets"][0]["locator"]
            )
    # Same catalog footprint and inspected grid; this alternate explicit native
    # window is known to have zero accepted cells. No footprint-derived indices.
    empty_manifest = copy.deepcopy(manifest)
    empty_manifest["observations"] = empty_manifest["observations"][:1]
    empty_manifest["observations"][0]["request"]["spatial"]["start"] = [600, 400]
    empty = job.run("discovered-footprint-empty-window", empty_manifest, extra=auth)
    empty["science"] = job.verify(empty, empty_manifest)
    import zarr

    group = zarr.open_group(empty["output"], mode="r")["observations/held_out_20240217"]
    assert int(group["quality_mask"][:].sum()) == 0
    write(
        directory / "measured-runs.json",
        {"plan": plan, "first": first, "replay": replay, "empty": empty},
    )
    finish_live(directory, discovery, generated, manifest, plan, first, replay, empty)


def finish_live(directory, discovery, generated, manifest, plan, first, replay, empty):
    total = sum(source_bytes(row) for row in [first, replay, empty])
    assert total <= 16 * 1024 * 1024
    receipt = {
        "directory": str(directory),
        "discovery": discovery,
        "generated": generated,
        "plan": job.compact(plan),
        "first": job.compact(first),
        "reconstruction": job.compact(replay),
        "footprint_intersects_but_selected_quality_empty": job.compact(empty),
        "source_body_bytes_fetch_reconstruction_empty": total,
        "source_body_acceptance_cap_bytes": 16 * 1024 * 1024,
        "all_passed": True,
    }
    write(directory / "workflow-full.json", receipt)
    write(DOCS / "live-workflow.json", receipt)
    example = selection(
        [
            "docs/swot-discovery/known-original.json",
            "docs/swot-discovery/known-held.json",
        ]
    )
    write(ROOT / "examples/swot-discovery-selection.json", example)
    write(DOCS / "generated-job.json", manifest)
    print(
        "live catalog -> inspection -> manifest -> plan -> fetch/reconstruction -> guarded references/Zarr/xarray passed",
        flush=True,
    )


def additional(netrc):
    directory = Path(tempfile.mkdtemp(prefix="additional-", dir=WORK))
    path = directory / "candidates.json"
    result = call(
        [
            "discover",
            "--collection",
            "SWOT_L2_HR_Raster_D",
            "--bbox",
            "-73,41,-70,44",
            "--start",
            "2024-02-16T00:00:00Z",
            "--end",
            "2024-02-18T00:00:00Z",
            "--limit",
            "20",
            "--output",
            path,
        ],
        directory / "catalog-stdout.json",
    )
    report = json.loads(path.read_text())
    assert report["status"] in ["complete", "truncated"], result
    known = {"G3927559976-POCLOUD", "G3927359490-POCLOUD"}
    candidates = [
        c
        for c in report["candidates"]
        if c["concept_id"] not in known
        and not c["exclusions"]
        and c["product"]["resolution_m"] == 100
    ][:3]
    assert len(candidates) == 3
    rows = []
    for c in candidates:
        selected = selection([path])
        s = selected["selection"]["observations"][0]
        selected["selection"]["observations"] = [
            {
                **s,
                "concept_id": c["concept_id"],
                "observation_id": "additional",
                "window": {"start": [0, 0], "count": [16, 16], "step": [1, 1]},
                "expected_etag": None,
            }
        ]
        result, manifest = inspect(directory, selected, netrc, c["concept_id"])
        rows.append(
            {
                "candidate": c,
                "compatibility": "passed_existing_profile" if manifest else "refused",
                **result,
            }
        )
    write(
        DOCS / "additional-inspections.json",
        {
            "protocol": "protocol.json",
            "catalog": report,
            "inspections": rows,
            "science_pixels_streamed": 0,
            "scientific_support_expanded": False,
        },
    )
    print("three predeclared additional compatibility inspections recorded", flush=True)


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("mode", choices=["offline", "live", "additional"])
    parser.add_argument("--netrc", type=Path)
    args = parser.parse_args()
    if args.mode != "offline" and args.netrc is None:
        parser.error(
            "live archive inspection/acquisition requires explicit --netrc PRIVATE_FILE"
        )
    if args.mode == "offline":
        offline()
    else:
        {"live": live, "additional": additional}[args.mode](args.netrc)
