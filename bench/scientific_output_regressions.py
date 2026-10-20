"""Repeat existing COG, direct Zarr and native streaming regression harnesses."""
import hashlib
import json
import os
from pathlib import Path
import native_cog_checks as faults
import native_cog_exact as exact
import consumer_validation as consumer

ROOT = Path(__file__).resolve().parents[1]
WORK = Path(os.environ.get("TILE_RIPPER_OUTPUT_REGRESSION_WORK",ROOT/"prototype-output/scientific-output/regressions"))
TARGET = ROOT/"target/scientific-output/release"


def main():
    WORK.mkdir(parents=True,exist_ok=True)
    binary = TARGET/"tile-ripper-native-stats"
    before = hashlib.sha256(binary.read_bytes()).hexdigest()
    faults.BIN, faults.WORK = binary, WORK/"cog-faults"
    consumer.BIN = binary
    faults.main()
    os.environ["TILE_RIPPER_ORACLE_BINARY"] = str(TARGET/"examples/native_cog_oracle")
    exact.main()
    consumer.WORK = WORK/"consumer"
    consumer.main()
    assert hashlib.sha256(binary.read_bytes()).hexdigest() == before
    record = dict(passed=True,
        native_cog_faults=json.loads((faults.WORK/"report.json").read_text()),
        native_cog_exact=json.loads((ROOT/"prototype-output/native-cog/exact/report.json").read_text()),
        streaming_consumer=json.loads((consumer.WORK/"receipt.json").read_text()))
    (ROOT/"docs/scientific-output/regressions.json").write_text(json.dumps(record,indent=2)+"\n")


if __name__=="__main__":main()
