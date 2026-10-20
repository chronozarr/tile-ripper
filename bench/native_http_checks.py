"""Inherited independent science/fault gates plus prepared snapshot isolation.

Candidate binaries must be built separately from frozen 1473c4c executables.
This harness redirects receipts; historical native-Sentinel receipts are intact.
"""

import hashlib
import json
import os
from pathlib import Path
import subprocess
import sys
import threading

import consumer_validation as consumer
import native_cog_checks as faults
import native_cog_exact as exact
import native_http_faults as selection_faults
import native_sentinel_workflow as workflow
import stac_workflow as stac

ROOT = Path(__file__).resolve().parents[1]
WORK = ROOT / "prototype-output/native-http/checks"
DOCS = ROOT / "docs/native-http"
TARGET = Path(os.environ.get("TILE_RIPPER_NATIVE_TARGET", ROOT / "target/native-http/release"))


def sha(path):
    return hashlib.sha256(Path(path).read_bytes()).hexdigest()


def main():
    WORK.mkdir(parents=True, exist_ok=True)
    DOCS.mkdir(parents=True, exist_ok=True)
    binary = TARGET / "tile-ripper-native-stats"
    digest = sha(binary)
    subprocess.run([sys.executable, str(ROOT / "bench/native_sentinel_checks.py"),
        "--binary", str(TARGET / "examples/native_sentinel_oracle"),
        "--receipt", str(DOCS / "science.json")], check=True, cwd=ROOT)
    faults.BIN, faults.WORK = binary, WORK / "cog-faults"
    consumer.BIN = binary
    faults.main()
    selection_faults.main()
    os.environ["TILE_RIPPER_ORACLE_BINARY"] = str(TARGET / "examples/native_cog_oracle")
    exact.main()
    consumer.WORK = WORK / "consumer"
    consumer.main()
    # ROOT is used only as the workflow receipt destination at runtime; all
    # fixture/binary paths are explicitly redirected below.
    workflow.ROOT = WORK / "receipt-root"
    (workflow.ROOT / "docs/native-sentinel").mkdir(parents=True, exist_ok=True)
    workflow.WORK, workflow.BIN = WORK / "workflow", binary
    workflow.main()
    workflow_receipt = json.loads((workflow.ROOT / "docs/native-sentinel/workflow-checks.json").read_text())
    (DOCS / "workflow-checks.json").write_text(json.dumps(workflow_receipt, indent=2) + "\n")

    server = stac.Server(("127.0.0.1", 0), stac.Handler)
    server.root = WORK / "snapshot-fixtures"
    server.files, server.features = stac.create(server.root, server.server_port)
    server.queries, server.seen_conditions = [], []
    server.reset()
    server.advance_on_second, server.advanced_searches = True, 0
    threading.Thread(target=server.serve_forever, daemon=True).start()
    try:
        request = workflow.unified(stac.request(server.server_port, "unused", large=True))
        path = WORK / "snapshot-isolation.json"
        path.write_text(json.dumps(request))
        env = {**os.environ, "TILE_RIPPER_NATIVE_HTTP_SNAPSHOT_REQUEST": str(path)}
        completed = subprocess.run(["cargo", "test", "--locked", "--no-default-features", "--features", "native-cog",
            "--test", "native_http_snapshot", "--", "--ignored"], cwd=ROOT, env=env, capture_output=True, text=True)
        (WORK / "snapshot-test.log").write_text(completed.stdout + completed.stderr)
        assert completed.returncode == 0, completed.stdout + completed.stderr
    finally:
        server.shutdown()
        server.server_close()
    assert sha(binary) == digest
    fault_receipt = json.loads((faults.WORK / "report.json").read_text())
    exact_receipt = json.loads((ROOT / "prototype-output/native-cog/exact/report.json").read_text())
    consumer_receipt = json.loads((consumer.WORK / "receipt.json").read_text())
    science = json.loads((DOCS / "science.json").read_text())
    selection_receipt = json.loads((DOCS / "selection-faults.json").read_text())
    assert science["failed"] == 0 and science["accepted"] == 95
    assert len(fault_receipt["checks"]) == 61
    assert sum(r["samples_compared_bit_for_bit"] for r in exact_receipt["rows"]) == 32391168
    record = dict(passed=True, consumer_sha256=digest, source_sha256=consumer_receipt["rust_source_sha256"],
        geometry_checks=science["accepted"], geometry_samples=science["samples_compared_bit_for_bit"],
        native_profile_fault_checks=len(fault_receipt["checks"]), exact_frozen_samples=32391168,
        bounded_selection_payload_checks=len(selection_receipt["checks"]),
        bounded_selection_exact_samples=selection_receipt["samples_compared_bit_for_bit"],
        consumer_checks=consumer_receipt["results"], workflow_checks=len(workflow_receipt["checks"]),
        independent_prepared_snapshot_isolation_and_repeatability=True,
        fault_max_rss_bytes=max(r["peak_rss_bytes"] for r in fault_receipt["checks"]),
        exact=exact_receipt, harness_sha256=sha(__file__))
    (DOCS / "validation.json").write_text(json.dumps(record, indent=2) + "\n")
    print(json.dumps(record, indent=2))


if __name__ == "__main__":
    main()
