#!/usr/bin/env python3
"""Fail-closed live fault gate for the three OPERA expansion profiles.

Only cloned requests are changed.  Raw process logs live below ignored
``prototype-output``; the tracked receipt contains no credentials, URLs, or
process output.
"""
from __future__ import annotations

import argparse
import hashlib
import json
import subprocess
from datetime import datetime, timezone
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
EXAMPLES = {
    "ni": ROOT / "examples/opera-dswx-ni-prototype.json",
    "dist_s1": ROOT / "examples/opera-dist-alert-s1.json",
    "dist_ann": ROOT / "examples/opera-dist-ann-hls.json",
}
RECEIPT = ROOT / "docs/products/opera-expansion/failure-qualification.json"
RAW = ROOT / "prototype-output/opera-expansion/faults"
BAD_ETAG = '"00000000000000000000000000000000"'


class FaultGateError(RuntimeError):
    pass


def require(condition, message):
    if not condition:
        raise FaultGateError(message)


def load(name):
    return json.loads(EXAMPLES[name].read_text())


def reduce(request):
    band = request["bands"][0]
    request["bands"] = [band]
    request["quality_policies"] = []
    request["max_observations"] = 1
    observation = request["observations"][0]
    observation["assets"] = {band: observation["assets"][band]}
    return request, band


def no_artifacts(destination):
    staged = list(destination.parent.glob(f".{destination.name}.partial-*"))
    return not destination.exists() and not staged, [path.name for path in staged]


def invoke(binary, netrc, work, name, request, authenticated, required_text, phase, *, conditional_412=False):
    request_path = work / f"{name}.json"
    destination = work / f"{name}.zarr"
    request_path.write_text(json.dumps(request, indent=2) + "\n")
    require(not destination.exists(), f"fault destination already exists: {destination.name}")
    command = [
        str(binary), "fetch", str(request_path), "--output", str(destination),
        "--output-format", "zarr_v2", "--backend", "native", "--workers", "1",
        "--per-host", "1", "--json",
    ]
    if authenticated:
        command += ["--auth", "earthdata", "--netrc", str(netrc)]
    complete = subprocess.run(command, cwd=ROOT, text=True, capture_output=True, timeout=120)
    raw = complete.stdout + complete.stderr
    # No process output is tracked: it may contain provider diagnostic detail.
    (work / f"{name}.log").write_text(raw)
    clean = raw.replace(str(netrc), "PRIVATE_NETRC")
    absent, stages = no_artifacts(destination)
    require(complete.returncode != 0, f"{name} unexpectedly succeeded")
    require(absent, f"{name} left a destination or staging directory: {stages}")
    require(required_text.lower() in clean.lower(), f"{name} did not expose expected failure class {required_text!r}")
    return {
        "name": name,
        "exit_nonzero": True,
        "expected_failure": required_text,
        "rejection_phase": phase,
        "conditional_if_match_412_observed": conditional_412,
        "destination_or_stage_absent": True,
        "stderr_sha256": hashlib.sha256(clean.encode()).hexdigest(),
    }


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, default=ROOT / "target/release/tile-ripper")
    parser.add_argument(
        "--netrc",
        type=Path,
        required=True,
        help="explicit credential file authorized for the authenticated live probes",
    )
    args = parser.parse_args()
    require(args.binary.is_file(), "release binary is required")
    require(args.netrc.is_file(), "an explicit private netrc is required")
    run = datetime.now(timezone.utc).strftime("%Y%m%dT%H%M%SZ")
    work = RAW / run
    work.mkdir(parents=True, exist_ok=False)
    cases = []

    ni, band = reduce(load("ni"))
    ni["observations"][0]["assets"][band]["etag"] = BAD_ETAG
    cases.append(invoke(
        args.binary, args.netrc, work, "ni-unpinned-etag", ni, False,
        "DSWx-NI prototype asset locator or ETag differs from the pinned specimen",
        "product validation before source I/O or staging",
    ))

    s1, band = reduce(load("dist_s1"))
    s1["observations"][0]["assets"][band]["etag"] = BAD_ETAG
    # The frozen release resolves an authenticated object's ETag before it
    # binds the source. A deliberately incorrect pin therefore fails at that
    # earlier fail-closed metadata check, before later conditional reads can
    # carry If-Match. Preserve that observed distinction in the receipt.
    cases.append(invoke(
        args.binary, args.netrc, work, "dist-s1-etag-conflict", s1, True,
        "authenticated native COG resolved ETag conflicts with request pin",
        "authenticated identity HEAD before staging or pixel range reads",
    ))

    annual, band = reduce(load("dist_ann"))
    annual["observations"][0]["assets"][band]["etag"] = BAD_ETAG
    cases.append(invoke(
        args.binary, args.netrc, work, "dist-ann-etag-conflict", annual, True,
        "authenticated native COG resolved ETag conflicts with request pin",
        "authenticated identity HEAD before staging or pixel range reads",
    ))

    annual_bad_time, _ = reduce(load("dist_ann"))
    annual_bad_time["observations"][0]["properties"]["temporal_extent"]["coverage_end"] = "2025-12-26T16:23:46.583Z"
    cases.append(invoke(
        args.binary, args.netrc, work, "dist-ann-cmr-extent", annual_bad_time, True,
        "annual supplied coverage interval conflicts with the exact live CMR granule record",
        "fresh CMR metadata validation before staging",
    ))

    annual_nonoverlap, _ = reduce(load("dist_ann"))
    annual_nonoverlap["start"] = "2024-01-01T00:00:00Z"
    annual_nonoverlap["end"] = "2024-01-02T00:00:00Z"
    cases.append(invoke(
        args.binary, args.netrc, work, "dist-ann-nonoverlap", annual_nonoverlap, True,
        "annual DIST coverage interval does not overlap the requested interval",
        "request interval validation before archive I/O or staging",
    ))

    receipt = {
        "schema": "tile-ripper-opera-expansion-failure-qualification-v1",
        "status": "passed_with_preflight_etag_conflicts",
        "captured_utc": datetime.now(timezone.utc).isoformat(),
        "binary_sha256": hashlib.sha256(args.binary.read_bytes()).hexdigest(),
        "scope": "Fresh cloned requests only. All cases require nonzero exit and no published/staging destination. Raw logs and request copies are ignored under prototype-output. The frozen release rejects deliberately wrong authenticated ETags during preflight identity resolution; this immutable-source probe therefore does not reach conditional If-Match 412.",
        "cases": cases,
    }
    RECEIPT.write_text(json.dumps(receipt, indent=2, sort_keys=True) + "\n")
    print(json.dumps({"status": receipt["status"], "cases": [case["name"] for case in cases]}, indent=2))


if __name__ == "__main__":
    try:
        main()
    except FaultGateError as error:
        raise SystemExit(f"OPERA expansion fault qualification failed: {error}")
