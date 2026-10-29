#!/usr/bin/env python3
"""Run the local locked build matrix; never overwrite an earlier result directory."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import platform
import re
import subprocess
import sys
import time

ROOT = Path(__file__).resolve().parents[1]
MATRIX = {
    "default": [],
    "zarr-only": ["--no-default-features"],
    "native-cog": ["--no-default-features", "--features", "native-cog"],
    "native-hdf5": ["--no-default-features", "--features", "native-hdf5"],
    "native-full": ["--no-default-features", "--features", "native-cog,native-hdf5,authenticated-http"],
    "all-features": ["--all-features"],
}


def documentation_checks(source_receipt=None):
    missing, local_evidence = [], []
    for p in [ROOT / "README.md", ROOT / "ALPHA.md", *ROOT.glob("docs/**/*.md")]:
        if "proposed-licensing" in p.parts:
            continue
        for ref in re.findall(r"!?\[[^\]]*\]\(([^\s)]+)", p.read_text()):
            if ref.startswith(("https:", "http:", "mailto:", "#")):
                continue
            ref = ref.split("#")[0]
            target = (p.parent / ref).resolve()
            if (target.is_relative_to(ROOT / "prototype-output") or
                    (target.parent == ROOT / "docs/acquisition-engine" and target.suffix == ".gz")):
                local_evidence.append([str(p.relative_to(ROOT)), ref])
                continue
            if ref and not target.exists():
                missing.append([str(p.relative_to(ROOT)), ref])
    source_receipt = source_receipt or ROOT / "docs/products/wave4/acceptance.json"
    receipt = json.loads(source_receipt.read_text())
    approval = json.loads((ROOT / "docs/consolidation/license-decision.json").read_text())
    mismatches, metadata_changes = [], []
    for p, digest in receipt["source_sha256"].items():
        actual = hashlib.sha256((ROOT / p).read_bytes()).hexdigest()
        if actual == digest:
            continue
        if (p == "Cargo.toml" and digest == approval["baseline_manifest_sha256"]
                and actual == approval["approved_manifest_sha256"]):
            metadata_changes.append("package.license = Apache-2.0; production dependencies unchanged")
        else:
            mismatches.append(p)
    if missing or mismatches:
        raise RuntimeError(json.dumps({"missing_links": missing, "source_receipt": str(source_receipt),
                                       "baseline_source_mismatches": mismatches}))
    return {"missing_links": missing, "wave4_source_mismatches": mismatches,
            "source_receipt": str(source_receipt),
            "approved_manifest_metadata_changes": metadata_changes,
            "local_evidence_links": local_evidence}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--toolchain", default="1.92.0")
    parser.add_argument("--source-receipt", type=Path,
                        help="Exact source-hash receipt to verify; defaults to the frozen Wave 4 receipt")
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=False)
    cargo = ["cargo", "+" + args.toolchain]
    report = {"status": "running", "toolchain": args.toolchain,
              "platform": platform.platform(), "checks": []}
    env = {**os.environ, "CARGO_BUILD_JOBS": "2"}

    def run(name, command):
        print(name, flush=True)
        start = time.monotonic()
        path = output / (name + ".log")
        with path.open("w") as log:
            result = subprocess.run(command, cwd=ROOT, env=env, stdout=log, stderr=subprocess.STDOUT)
        text = path.read_text()
        totals = re.findall(r"test result: ok\. (\d+) passed; (\d+) failed; (\d+) ignored", text)
        row = {"name": name, "command": command, "exit_code": result.returncode,
               "seconds": time.monotonic() - start,
               "log": path.name, "sha256": hashlib.sha256(path.read_bytes()).hexdigest()}
        if totals:
            row["tests"] = dict(zip(("passed", "failed", "ignored"),
                                    (sum(int(t[i]) for t in totals) for i in range(3))))
        report["checks"].append(row)
        if result.returncode:
            raise RuntimeError(f"{name} failed; see {path}")

    try:
        report["documentation"] = documentation_checks(args.source_receipt)
        run("format", cargo + ["fmt", "--all", "--", "--check"])
        run("clippy", cargo + ["clippy", "--locked", "--offline", "--all-features", "--all-targets", "--", "-D", "warnings"])
        for name, features in MATRIX.items():
            run("tests-" + name, cargo + ["test", "--locked", "--offline", *features,
                                        "--all-targets", "--", "--test-threads=1"])
        for name in ("hdf5-reader", "zarrs_filesystem"):
            manifest = str(ROOT / "vendor" / name / "Cargo.toml")
            run("vendor-tests-" + name, cargo + ["test", "--locked", "--offline", "--manifest-path", manifest,
                                                "--all-targets", "--", "--test-threads=1"])
        for name, features in (("default", []), ("native", ["--no-default-features", "--features", "native-cog"])):
            run("consumer-" + name, cargo + ["check", "--locked", "--offline", "--manifest-path",
                                             "consumers/native-stats/Cargo.toml", *features])
        run("python-syntax", [sys.executable, "-m", "compileall", "-q", "scripts", "bench"])
        report["status"] = "passed"
    except Exception as error:
        report["status"] = "failed"
        report["error"] = str(error)
    finally:
        (output / "report.json").write_text(json.dumps(report, indent=2) + "\n")
    return 0 if report["status"] == "passed" else 1


if __name__ == "__main__":
    sys.exit(main())
