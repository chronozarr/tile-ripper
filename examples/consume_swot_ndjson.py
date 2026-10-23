"""Restartable consumer for `tile-ripper stream --quality-policy ... --values`.

Starts its calculation at zero every invocation. Emits a result only after
the successful native completion record. Never append attempts to one file.
"""

import base64
import json
import math
import struct
import sys

PATHS = ["/wse", "/wse_qual", "/wse_qual_bitwise", "/wse_uncert", "/n_wse_pix"]
TYPES = [
    ("float32", "f"),
    ("uint8", "B"),
    ("uint32", "I"),
    ("float32", "f"),
    ("uint32", "I"),
]


def consume(lines):
    summary = dict(
        selected_count=0,
        wse_sum_m=0.0,
        uncertainty_count=0,
        uncertainty_sum_m=0.0,
        contributing_count=0,
        contributing_pixel_sum=0,
    )
    tiles = 0
    completion = None
    for line in lines:
        record = json.loads(line)
        if completion is not None:
            raise ValueError("records after completion; do not concatenate attempts")
        if record["type"] == "prepared":
            continue
        if record["type"] == "complete":
            completion = record
            continue
        if record["type"] != "tile" or record["index"] != tiles:
            raise ValueError("expected a fresh ordered stream starting at tile zero")
        tiles += 1
        arrays, masks = [], []
        valid = record["validity"]
        if len(record["layers"]) != 5 or any(v not in [0, 1] for v in valid):
            raise ValueError("invalid five-layer tile")
        for layer, path, (dtype, code) in zip(record["layers"], PATHS, TYPES):
            if (
                layer["variable"] != {"kind": "dataset", "path": path}
                or layer["dtype"] != dtype
            ):
                raise ValueError("unexpected scientific variable/type")
            if layer["encoding"] != "base64-little-endian":
                raise ValueError("unexpected native value encoding")
            window = layer["window"]
            if window["width"] * window["height"] != len(valid):
                raise ValueError("misaligned scientific tile")
            raw = base64.b64decode(layer["values"], validate=True)
            arrays.append(struct.unpack("<" + str(len(valid)) + code, raw))
            mask = base64.b64decode(layer["source_mask"], validate=True)
            if len(mask) != len(valid) or any(v not in [0, 1] for v in mask):
                raise ValueError("invalid source mask")
            masks.append(mask)
        for i, keep in enumerate(valid):
            if keep:
                summary["selected_count"] += 1
                summary["wse_sum_m"] += arrays[0][i]
                if masks[3][i] and math.isfinite(arrays[3][i]):
                    summary["uncertainty_count"] += 1
                    summary["uncertainty_sum_m"] += arrays[3][i]
                if masks[4][i]:
                    summary["contributing_count"] += 1
                    summary["contributing_pixel_sum"] += arrays[4][i]
    if completion is None:
        raise ValueError(
            "no successful finish; discard this attempt's provisional calculation"
        )
    summary["wse_mean_m"] = (
        summary["wse_sum_m"] / summary["selected_count"]
        if summary["selected_count"]
        else None
    )
    summary["mean_pixel_uncertainty_m"] = (
        summary["uncertainty_sum_m"] / summary["uncertainty_count"]
        if summary["uncertainty_count"]
        else None
    )
    if summary != completion["summary"]:
        raise ValueError("consumer calculation disagrees with completed native stream")
    return summary


if __name__ == "__main__":
    with open(sys.argv[1], encoding="utf-8") as source:
        print(json.dumps(consume(source), allow_nan=False))
