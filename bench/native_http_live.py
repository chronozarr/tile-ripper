"""Repeat the inherited live Sentinel oracle without replacing its baseline receipt."""

import json
import os
from pathlib import Path
import shutil

import native_sentinel_live as live


ROOT = Path(__file__).resolve().parents[1]
WORK = ROOT / "prototype-output/native-http/live"


def main():
    receipt_root = WORK / "receipt-root"
    destination = receipt_root / "docs/native-sentinel"
    destination.mkdir(parents=True, exist_ok=True)
    shutil.copyfile(ROOT / "docs/native-sentinel/request.json", destination / "request.json")
    live.ROOT = receipt_root
    live.WORK = WORK
    live.TARGET = Path(os.environ.get("TILE_RIPPER_NATIVE_TARGET", ROOT / "target/native-http/release"))
    live.main()
    receipt = json.loads((destination / "live.json").read_text())
    assert receipt["passed"] and receipt["samples_compared_bit_for_bit"] == 20566
    (ROOT / "docs/native-http/live.json").write_text(json.dumps(receipt, indent=2) + "\n")


if __name__ == "__main__":
    main()
