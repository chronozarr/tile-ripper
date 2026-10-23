"""Keep compact milestone receipts; retain full diagnostic JSON locally by hash.

This transforms benchmark evidence only. It is never consulted by acquisition,
source checkpoints, admission or execution. Large pixel/coordinate lists and
repeated source attributes remain in the ignored, hash-indexed full receipts.
"""

import hashlib
import json
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
DOCS = ROOT / "docs/grid-inspection"
RAW = ROOT / "prototype-output/grid-inspection/full-receipts"


def digest(value):
    return hashlib.sha256(
        json.dumps(value, sort_keys=True, separators=(",", ":")).encode()
    ).hexdigest()


def compact(value, key=""):
    if isinstance(value, list):
        if len(value) > 64 and all(not isinstance(v, (list, dict)) for v in value):
            return {
                "items": len(value),
                "diagnostic_json_sha256": digest(value),
                "full_values": "see receipt-index.json",
            }
        return [compact(v) for v in value]
    if not isinstance(value, dict):
        return value
    if key == "attributes" and len(json.dumps(value)) > 1024:
        interpreted = {}
        for name, attr in value.items():
            if isinstance(attr, dict) and "stored_bytes_base64" in attr:
                interpreted[name] = compact(
                    attr.get(
                        "value",
                        attr.get("nonfinite", "uninterpreted; see full receipt"),
                    )
                )
            else:
                interpreted[name] = compact(attr)
        return {
            "full_attributes_sha256": digest(value),
            "interpreted_values": interpreted,
        }
    if key in ["root_attributes", "source_root_attributes"]:
        names = [
            "crid",
            "descriptor_string",
            "title",
            "cycle_number",
            "pass_number",
            "scene_number",
            "time_coverage_start",
            "time_coverage_end",
        ]
        return {
            "full_attributes_sha256": digest(value),
            "identity_attributes": {k: value[k] for k in names if k in value},
        }
    if key in ["plan", "plan_summary"] and ("nodes" in value or "selected" in value):
        selected = value.get("selected", [])
        return {
            "full_plan_sha256": digest(value),
            "nodes": len(value["nodes"])
            if isinstance(value.get("nodes"), list)
            else value.get("nodes"),
            "selected_count": len(selected),
            "estimates": value.get("estimates"),
            "saved_plan_execution_supported": False,
        }
    return {k: compact(v, k) for k, v in value.items()}


def main():
    RAW.mkdir(parents=True, exist_ok=True)
    rows = []
    index_path = DOCS / "receipt-index.json"
    previous = (
        {r["receipt"]: r for r in json.loads(index_path.read_text())["receipts"]}
        if index_path.exists()
        else {}
    )
    excluded = {
        "verification.json",
        "receipt-index.json",
        "summary.json",
        "live-job.json",
        "resolved-controls.json",
        "execution.json",
        "feature-matrix.json",
    }
    for path in sorted(DOCS.rglob("*.json")):
        if path.name in excluded:
            continue
        relative = path.relative_to(DOCS)
        current_sha = hashlib.sha256(path.read_bytes()).hexdigest()
        retained = previous.get(str(relative))
        if retained and current_sha == retained["compact_sha256"]:
            original = Path(retained["full_receipt"])
            assert (
                hashlib.sha256(original.read_bytes()).hexdigest()
                == retained["full_sha256"]
            )
        else:
            original = RAW / current_sha / relative
            original.parent.mkdir(parents=True, exist_ok=True)
            if not original.exists():
                original.write_bytes(path.read_bytes())
        value = json.loads(original.read_text())
        result = compact(value)
        encoded = json.dumps(result, indent=2, allow_nan=False)
        if len(encoded) > 32768:
            encoded = json.dumps(result, separators=(",", ":"), allow_nan=False)
        path.write_text(encoded + "\n")
        rows.append(
            dict(
                receipt=str(relative),
                full_receipt=str(original),
                full_sha256=hashlib.sha256(original.read_bytes()).hexdigest(),
                compact_sha256=hashlib.sha256(path.read_bytes()).hexdigest(),
            )
        )
    (DOCS / "receipt-index.json").write_text(
        json.dumps(
            dict(
                scope="benchmark diagnostics only; not a source-byte or parser cache",
                receipts=rows,
            ),
            indent=2,
        )
        + "\n"
    )
    print(f"Compacted {len(rows)} receipts; full originals retained locally")


if __name__ == "__main__":
    main()
