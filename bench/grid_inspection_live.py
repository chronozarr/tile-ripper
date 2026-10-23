"""One bounded production discover-job and checkpoint rebuild on three pinned products.

Uses the existing authoritative catalog receipts and geographic inputs. No new
CMR search, source, spatial selection, capture, or fallback is introduced. Native
archive authentication is exercised by ordinary CLI invocations. Existing
version-bound checkpoint bytes are copied into a private fresh run directory.
"""

import argparse
import json
from pathlib import Path
import shutil
import tempfile

import swot_aoi as aoi
import swot_job as job
from swot_aoi_profile import traffic

ROOT = job.ROOT
DOCS = ROOT / "docs/grid-inspection"
WORK = ROOT / "prototype-output/grid-inspection"


def aggregate(transports):
    records = [traffic(t) for t in transports]
    return {k: sum(t[k] for t in records) for k in records[0]}


def main(netrc, resume=None):
    baseline = json.loads((ROOT / "docs/swot-aoi/live-workflow.json").read_text())
    prior = Path(baseline["directory"])
    directory = resume if resume else Path(tempfile.mkdtemp(prefix="live-", dir=WORK))
    caps = dict(
        source_response_body_bytes=2 * 1024 * 1024,
        wire_requests=240,
        observations=3,
        phases=[
            "ordinary production discover-job",
            "ordinary checkpoint-assisted fetch and final sweep",
        ],
        existing_inputs=str(prior / "selection.json"),
        new_catalog_queries=0,
        new_science_captures=0,
    )
    job.write(directory / "declared-bounds.json", caps)
    selection = json.loads((prior / "selection.json").read_text())
    job.write(directory / "selection.json", selection)
    if resume:
        generated = json.loads((directory / "generation-measurements.json").read_text())
    else:
        generated = aoi.measured_cli(
            [
                "discover-job",
                directory / "selection.json",
                "--auth",
                "earthdata",
                "--netrc",
                netrc,
                "--output",
                directory / "job.json",
            ],
            directory,
            "review",
            timeout=180,
        )
        generated["generation_binary_sha256"] = job.digest(job.BINARY)
        job.write(directory / "generation-measurements.json", generated)
    assert generated["returncode"] == 0, Path(generated["stderr"]).read_text()[-2000:]
    review = json.loads(Path(generated["stdout"]).read_text())
    manifest = json.loads((directory / "job.json").read_text())
    expected = json.loads((ROOT / "docs/swot-aoi/generated-job.json").read_text())
    assert manifest == expected, "native source/grid/AOI/window identity changed"
    generation_traffic = aggregate(
        s["inspection_transport"] for s in review["observations"]
    )
    assert (
        generation_traffic["source_response_body_bytes"]
        <= caps["source_response_body_bytes"]
    )
    assert generation_traffic["actual_wire_http_requests"] <= caps["wire_requests"]
    checkpoint = directory / "checkpoint"
    if not checkpoint.exists():
        shutil.copytree(prior / "checkpoint", checkpoint)
    fetched = job.run(
        "inspection-live-checkpoint-reconstruction",
        manifest,
        checkpoint=checkpoint,
        extra=["--auth", "earthdata", "--netrc", str(netrc)],
    )
    assert fetched["returncode"] == 0, fetched["error"]
    coords = [
        aoi.axes(p, "grid-live-" + str(i))[0]
        for i, p in enumerate(
            [job.HELD_REFERENCE, job.ORIGINAL, aoi.WORK / "additional/reference"]
        )
    ]
    sources = [job.HELD_REFERENCE, job.ORIGINAL, aoi.WORK / "additional/reference"]
    science = aoi.verify_aoi(fetched, manifest, coords, sources=sources)
    acquisition = []
    for i, o in enumerate(fetched["receipt"]["observations"]):
        acquisition.append(o["observed"]["transport"])
        acquisition.append(o["job_final_validation"]["transport"])
    fetch_traffic = aggregate(acquisition)
    total = {k: generation_traffic[k] + fetch_traffic[k] for k in generation_traffic}
    assert total["source_response_body_bytes"] <= caps["source_response_body_bytes"], (
        total
    )
    assert total["actual_wire_http_requests"] <= caps["wire_requests"], total
    job.write(DOCS / "live-review.json", review)
    job.write(DOCS / "live-job.json", manifest)
    job.write(
        DOCS / "live.json",
        dict(
            declared_bounds=caps,
            generation=generated,
            generation_traffic=generation_traffic,
            reconstruction=job.compact(fetched),
            reconstruction_traffic=fetch_traffic,
            total=total,
            geographic_science=science,
            raw_directory=str(directory),
            all_passed=True,
        ),
    )
    print(
        "production three-source discovery inspection and scientific rebuild passed",
        total,
        flush=True,
    )


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("--netrc", type=Path, required=True)
    parser.add_argument("--resume", type=Path)
    args = parser.parse_args()
    main(args.netrc, args.resume)
