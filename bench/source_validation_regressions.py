"""Re-run existing qualified gates with this milestone's evidence destination.

Use the existing legacy HDF5 reference runtime, and the existing Zarr v3 runtime
for v3 actions. Raw captures remain in their original verified locations.
"""

import json
import os
import sys
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
DOCS = ROOT / "docs/source-validation/regressions"
WORK = ROOT / "prototype-output/source-validation/regressions"
BIN = ROOT / "target/release"


def configure(module, name, work=False):
    module.DOCS = DOCS / name
    module.DOCS.mkdir(parents=True, exist_ok=True)
    if work:
        WORK.mkdir(parents=True, exist_ok=True)
        module.WORK = Path(tempfile.mkdtemp(prefix=name + "-", dir=WORK))


def run(action):
    if action == "hdf5":
        import hdf5_input as gate

        configure(gate, "hdf5", True)
        gate.BINARY = BIN / "examples/hdf5_subset_oracle"
        gate.science()
        gate.faults()
    elif action == "quality":
        import swot_quality as gate

        configure(gate, "quality")
        gate.BINARY = BIN / "examples/swot_quality"
        gate.science()
        gate.faults()
    elif action == "auth":
        import earthdata_auth as gate

        configure(gate, "auth")
        gate.q.BINARY = BIN / "examples/swot_quality"
        gate.fixtures()
    elif action == "checkpoint":
        import transfer_resume as gate

        configure(gate, "checkpoint")
        gate.BINARY = BIN / "tile-ripper"
        gate.fixtures()
    elif action == "job":
        import swot_job as gate

        configure(gate, "job")
        gate.BINARY = BIN / "tile-ripper"
        gate.offline()
    elif action == "geography":
        import swot_aoi as gate

        configure(gate, "geography")
        configure(gate.job, "geographic-job")
        gate.ORACLE = BIN / "examples/swot_aoi_oracle"
        gate.geometry_oracles()
        gate.offline()
    elif action == "inspection":
        import grid_inspection as gate

        configure(gate, "inspection")
        gate.faults()
    elif action in ["remote", "cf", "checks", "faults"]:
        import zarr_v3_regressions as gate

        configure(gate, "formats", True)
        gate.BIN_DIR = BIN
        gate.inherited(action)
    elif action.startswith("v3-"):
        import zarr_v3_input as gate

        configure(gate, "v3", True)
        gate.BINARY = BIN / "examples/zarr_subset_oracle"
        gate.CLI = BIN / "tile-ripper"
        getattr(gate, action.removeprefix("v3-"))()
    elif action == "cog":
        import consumer_validation as consumer
        import native_cog_checks as faults
        import native_cog_exact as exact

        binary = ROOT / "consumers/native-stats/target/release/tile-ripper-native-stats"
        consumer.BIN = faults.BIN = binary
        consumer.WORK, faults.WORK = WORK / "consumer", WORK / "cog-faults"
        os.environ["TILE_RIPPER_ORACLE_BINARY"] = str(
            BIN / "examples/native_cog_oracle"
        )
        faults.main()
        exact.main()
        consumer.main()
        destination = DOCS / "formats/cog-stream.json"
        destination.parent.mkdir(parents=True, exist_ok=True)
        destination.write_text(
            json.dumps(
                {
                    "cog_faults": json.loads((faults.WORK / "report.json").read_text()),
                    "cog_exact": json.loads(
                        (
                            ROOT / "prototype-output/native-cog/exact/report.json"
                        ).read_text()
                    ),
                    "stream_consumer": json.loads(
                        (consumer.WORK / "receipt.json").read_text()
                    ),
                },
                indent=2,
            )
            + "\n"
        )
    else:
        raise ValueError(action)


if __name__ == "__main__":
    run(sys.argv[1])
