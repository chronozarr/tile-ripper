"""Re-run inherited local format and durable-output gates without old-doc edits.
Use the existing Python reference environment plus psutil, or the documented
pinned reference packages. All server traffic is loopback.
"""

import json
import os
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
WORK = Path(
    os.environ.get(
        "TILE_RIPPER_HDF5_REGRESSION_WORK",
        ROOT / "prototype-output/native-hdf5/regressions-final",
    )
).resolve()
DOCS = Path(
    os.environ.get(
        "TILE_RIPPER_HDF5_REGRESSION_DOCS", ROOT / "docs/native-hdf5/inherited"
    )
).resolve()
WORK.mkdir(parents=True, exist_ok=True)
DOCS.mkdir(parents=True, exist_ok=True)
os.environ["TILE_RIPPER_VALIDATION_DOCS"] = str(DOCS)
os.environ["TILE_RIPPER_ORACLE_BINARY"] = str(
    ROOT / "target/hdf5-regression/release/examples/native_cog_oracle"
)
BIN = ROOT / "target/hdf5-regression/release"


def main(action):
    if action == "cog-stream":
        import consumer_validation as consumer
        import native_cog_checks as faults
        import native_cog_exact as exact

        stats = ROOT / "target/hdf5-regression/release/tile-ripper-native-stats"
        consumer.BIN = stats
        faults.BIN = stats
        faults.WORK = WORK / "cog-faults"
        faults.main()
        exact.main()
        consumer.WORK = WORK / "consumer"
        consumer.main()
        summary = {
            "cog_faults": json.loads((faults.WORK / "report.json").read_text()),
            "cog_exact": json.loads(
                (ROOT / "prototype-output/native-cog/exact/report.json").read_text()
            ),
            "stream_consumer": json.loads((consumer.WORK / "receipt.json").read_text()),
        }
        (DOCS / "cog-stream.json").write_text(json.dumps(summary, indent=2) + "\n")
    elif action in ["remote", "cf", "checks", "faults"]:
        import zarr_v3_regressions as gates

        gates.WORK = WORK
        gates.DOCS = DOCS
        gates.BIN_DIR = BIN
        gates.inherited(action)
    elif action.startswith("v3-"):
        import zarr_v3_input as v3

        v3.WORK = WORK / "v3"
        v3.DOCS = DOCS / "v3"
        v3.BINARY = BIN / "examples/zarr_subset_oracle"
        v3.CLI = BIN / "tile-ripper"
        v3.WORK.mkdir(parents=True, exist_ok=True)
        v3.DOCS.mkdir(parents=True, exist_ok=True)
        getattr(v3, action.removeprefix("v3-"))()
    else:
        raise ValueError(action)


if __name__ == "__main__":
    main(sys.argv[1])
