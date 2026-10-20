"""Independent acceptance and phase faults for explicit experimental v3 outputs."""
import argparse
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
WORK = ROOT / "prototype-output/zarr-v3-output"
DOCS = ROOT / "docs/zarr-v3-output"
MODES = ["v3_raw", "v3_sharded_raw", "sharded", "v3_sharded_blosc", "v3_sharded_zstd"]


def main(action):
    WORK.mkdir(parents=True, exist_ok=True)
    DOCS.mkdir(parents=True, exist_ok=True)
    if action == "matrix":
        import durable_output_benchmark as gate
        gate.WORK, gate.DOCS = WORK / "matrix", DOCS
        gate.main("benchmark", 3, MODES)
    else:
        import durable_output_validation as gate
        gate.DOCS = DOCS
        if action == "checks":
            gate.WORK = WORK / "checks"
            gate.WORK.mkdir(parents=True, exist_ok=True)
            gate.checks([(mode, 256) for mode in MODES])
        elif action == "faults":
            for mode in MODES:
                gate.WORK, gate.DOCS = WORK / ("faults-" + mode), DOCS / ("faults-" + mode)
                gate.WORK.mkdir(parents=True, exist_ok=True)
                gate.DOCS.mkdir(parents=True, exist_ok=True)
                gate.faults(mode)
        elif action == "memory":
            gate.WORK = WORK / "memory"
            gate.WORK.mkdir(parents=True, exist_ok=True)
            gate.memory(MODES)


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("action", choices=["checks", "matrix", "faults", "memory"])
    main(parser.parse_args().action)
