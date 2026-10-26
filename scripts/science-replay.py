#!/usr/bin/env python3
"""Replay Wave 3/4 science and faults into a fresh namespace using sealed captures."""
import argparse
import hashlib
import json
from pathlib import Path
import shutil
import sys
import time

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "bench"))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--captures", type=Path, required=True, help="Existing prototype-output root")
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    captures = args.captures.resolve()
    output = args.output.resolve()
    if output.exists():
        parser.error("output must be a new directory")
    required = {"wave3": ["goes-C13", "goes-C01", "smap/global", "nisar"],
                "wave4": ["nisar-geographic", "nisar-additional", "goes-m1-c02", "smap-polar"]}
    for wave, directories in required.items():
        for directory in directories:
            for filename in ("source.nc", "manifest.json"):
                if not (captures / wave / directory / filename).is_file():
                    parser.error(f"missing capture: {wave}/{directory}/{filename}; see docs/consolidation/validation.md")
    binary = ROOT / "target/release/tile-ripper"
    if not binary.is_file() or not (ROOT / "target/release/examples/array_faults").is_file():
        parser.error("build the documented native release binary and array_faults example first")
    output.mkdir(parents=True)
    import requests
    import wave3_arrays as w
    import wave3_acceptance as old
    import wave4_geographic as geo
    import wave4_acceptance as new
    from hdf5_selective_mirror import Mirror

    # Python references may only read the guarded loopback mirrors in this mode.
    send = requests.adapters.HTTPAdapter.send

    def loopback_only(adapter, request, **kwargs):
        from urllib.parse import urlparse
        if urlparse(request.url).hostname not in ("127.0.0.1", "localhost", "::1"):
            raise RuntimeError("external HTTP is disabled during frozen replay")
        return send(adapter, request, **kwargs)

    requests.adapters.HTTPAdapter.send = loopback_only
    sandbox = output / "sandbox"
    manifests = {}
    for wave, directories in required.items():
        work = sandbox / "prototype-output" / wave
        work.mkdir(parents=True)
        for directory in directories:
            link = work / directory
            link.parent.mkdir(parents=True, exist_ok=True)
            link.symlink_to(captures / wave / directory, target_is_directory=True)
            mirror = Mirror(link)
            mirror.verify()
            manifests[str(link)] = hashlib.sha256(mirror.manifest.read_bytes()).hexdigest()
        for p in (captures / wave).glob("*-inspection.json"):
            shutil.copy2(p, work / p.name)
        shutil.copytree(ROOT / "docs/products" / wave, sandbox / "docs/products" / wave)
    (sandbox / "examples").mkdir()

    original_save = w.save

    def save_new(path, value):
        path = Path(path)
        if not path.resolve().is_relative_to(output):
            raise RuntimeError(f"replay refused to overwrite source evidence: {path}")
        original_save(path, value)

    w.save = save_new
    w.ROOT = sandbox
    w.WORK = sandbox / "prototype-output/wave3"
    w.DOC = sandbox / "docs/products/wave3"
    w.BIN = binary
    old.ROOT = ROOT  # read-only example binaries
    old.WORK, old.DOC, old.BIN = w.WORK, w.DOC, binary
    geo.ROOT = sandbox
    geo.WORK = sandbox / "prototype-output/wave4"
    geo.DOC = sandbox / "docs/products/wave4"
    new.ROOT, new.WORK, new.DOC = ROOT, geo.WORK, geo.DOC
    report = {"status": "running", "binary_sha256": hashlib.sha256(binary.read_bytes()).hexdigest(), "checks": []}
    try:
        actions = [("wave3-goes", w.goes), ("wave3-smap", w.smap), ("wave3-nisar", w.nisar),
                   ("wave3-faults", old.faults), ("wave4-nisar", geo.nisar), ("wave4-goes", geo.goes),
                   ("wave4-smap", geo.smap), ("wave4-additional", geo.additional), ("wave4-faults", new.faults),
                   ("wave4-nisar-full-coordinate-oracle", geo.full_oracle),
                   ("wave4-smap-full-coordinate-oracle", geo.polar_oracle)]
        for name, action in actions:
            print(name, flush=True)
            start = time.monotonic()
            action()
            report["checks"].append({"name": name, "seconds": time.monotonic() - start, "status": "passed"})
        report["status"] = "passed"
    except Exception as error:
        report["status"], report["error"] = "failed", str(error)
    finally:
        for path, digest in manifests.items():
            mirror = Mirror(path)
            mirror.verify()
            if hashlib.sha256(mirror.manifest.read_bytes()).hexdigest() != digest:
                report["status"], report["error"] = "failed", "original capture manifest changed"
        (output / "report.json").write_text(json.dumps(report, indent=2) + "\n")
    return 0 if report["status"] == "passed" else 1


if __name__ == "__main__":
    sys.exit(main())
