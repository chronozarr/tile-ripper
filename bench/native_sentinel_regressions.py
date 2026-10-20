"""Repeat existing native COG and COG/Zarr public-consumer regression gates."""

import hashlib
import json
import os
from pathlib import Path

import consumer_validation as consumer
import native_cog_checks as faults
import native_cog_exact as exact

ROOT = Path(__file__).resolve().parents[1]
WORK = ROOT / "prototype-output/native-sentinel/regressions"
TARGET = Path(os.environ.get("TILE_RIPPER_NATIVE_TARGET", ROOT / "target/native-sentinel/release"))


def main():
    WORK.mkdir(parents=True, exist_ok=True)
    binary = TARGET / "tile-ripper-native-stats"
    digest = hashlib.sha256(binary.read_bytes()).hexdigest()
    faults.BIN, faults.WORK = binary, WORK / "cog-faults"
    consumer.BIN = binary
    faults.main()
    os.environ["TILE_RIPPER_ORACLE_BINARY"] = str(TARGET / "examples/native_cog_oracle")
    exact.main()
    consumer.WORK = WORK / "consumer"
    consumer.main()
    assert hashlib.sha256(binary.read_bytes()).hexdigest() == digest
    fault_receipt = json.loads((faults.WORK / "report.json").read_text())
    exact_receipt = json.loads((ROOT / "prototype-output/native-cog/exact/report.json").read_text())
    consumer_receipt = json.loads((consumer.WORK / "receipt.json").read_text())
    record = dict(passed=True, consumer_sha256=digest,
        native_cog_fault_checks=len(fault_receipt["checks"]),
        native_cog_exact_samples=sum(r["samples_compared_bit_for_bit"] for r in exact_receipt["rows"]),
        cog_zarr_consumer_checks=consumer_receipt["results"],
        fault_harness_sha256=fault_receipt["harness_sha256"],
        exact=exact_receipt,
        source_sha256=consumer_receipt["rust_source_sha256"],
        max_fault_rss_bytes=max(r["peak_rss_bytes"] for r in fault_receipt["checks"]))
    assert record["native_cog_fault_checks"] == 61
    assert record["native_cog_exact_samples"] == 32391168
    (ROOT / "docs/native-sentinel/regressions.json").write_text(json.dumps(record, indent=2) + "\n")
    print(json.dumps(record, indent=2))


if __name__ == "__main__":
    main()
