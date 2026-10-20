"""Replay existing gates into a separate receipt directory for the zarrs upgrade."""
import argparse
import os
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
WORK = ROOT / "prototype-output/zarr-v3-upgrade"
DOCS = ROOT / "docs/zarr-v3-upgrade"
os.environ["TILE_RIPPER_VALIDATION_DOCS"] = str(DOCS)


def main(action):
    DOCS.mkdir(parents=True, exist_ok=True)
    if action == "benchmark":
        import durable_output_benchmark as gate
        gate.WORK = WORK / "benchmark"
        gate.DOCS = DOCS
        gate.OLD = WORK / "baseline-f885e50"
        gate.main("benchmark", 3)
    elif action in ["checks", "faults", "memory", "regressions", "reopen-v2"]:
        import durable_output_validation as gate
        gate.DOCS = DOCS
        gate.WORK = WORK / ("checks" if action == "reopen-v2" else action)
        gate.WORK.mkdir(parents=True, exist_ok=True)
        getattr(gate, action.replace("-", "_"))()
    elif action == "remote":
        import remote_zarr as gate
        import zarr_workflow
        gate.CURRENT = ROOT / "target/scientific-output/release/tile-ripper"
        gate.EXAMPLE = gate.CURRENT.parent / "examples/remote_zarr"
        gate.WORK = WORK / "remote"
        zarr_workflow.WORK = WORK / "v2-fixture"
        gate.checks()
    elif action == "cf":
        import cf_zarr as gate
        gate.WORK = WORK / "cf"
        gate.h.CURRENT = ROOT / "target/scientific-output/release/tile-ripper"
        gate.checks()


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("action", choices=["checks", "faults", "memory", "regressions", "reopen-v2", "benchmark", "remote", "cf"])
    main(parser.parse_args().action)
