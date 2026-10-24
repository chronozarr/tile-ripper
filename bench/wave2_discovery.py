#!/usr/bin/env python3
"""Measure the ordinary bounded catalog CLI for five frozen qualification AOIs."""
import json
from pathlib import Path
import sys

from scientific_output import invoke

ROOT = Path(__file__).resolve().parents[1]
binary = Path(sys.argv[1]).resolve() if len(sys.argv) > 1 else ROOT / "prototype-output/wave2/tile-ripper-native"
cases = []
for name in ["landsat-lt04", "landsat-lt05", "landsat-le07", "dswx-s1", "dist-alert"]:
    request = json.loads((ROOT / f"docs/products/wave2/{name}-request.json").read_text())
    args = ["discover", "--product", request["product"], "--bbox", ",".join(map(str, request["aoi"]["bbox"])), "--start", request["start"], "--end", request["end"], "--bands", ",".join(request["bands"]), "--limit", "8"]
    result = invoke(args, binary=binary)
    case = {"case": name, "command": args, "process": {k: v for k, v in result.items() if k not in ["stdout", "stderr"]}}
    if result["code"] == 0:
        catalog = json.loads(result["stdout"])
        case.update(complete=catalog["complete"], catalog_seconds=catalog["seconds"], provider=catalog["provider"], observation_ids=[o["id"] for o in catalog["observations"]])
        expected = request.get("observations", [{}])[0].get("id")
        if expected:
            assert expected in case["observation_ids"]
    else:
        case["failure"] = "catalog CLI failed; no alternate provider or science request"
    cases.append(case)
report = {"scope": "catalog-only public discovery; no authentication, signing or science reads", "cases": cases}
(ROOT / "docs/products/wave2/discovery-timing.json").write_text(json.dumps(report, indent=2) + "\n")
print(json.dumps(report))
assert all(c["process"]["code"] == 0 and c["complete"] for c in cases)
