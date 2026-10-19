"""Additional independent output checks; use the isolated Zarr environment."""

import copy
import hashlib
import json
import shutil

import output_write as bench
from zarr_workflow import check, fixture


def fingerprint(path):
    return {
        str(p.relative_to(path)): hashlib.sha256(p.read_bytes()).hexdigest()
        for p in path.rglob("*")
        if p.is_file() and p.name != "manifest.json"
    }


def main():
    r = json.loads(fixture().read_text())
    cases = [
        ("upper-left", [500000, 3999500, 500450, 4000000], None),
        ("lower-right", [501450, 3998720, 501600, 3998870], None),
        ("exact-boundary", [500320, 3999360, 500640, 3999680], None),
        (
            "single-instant",
            None,
            {"kind": "instants", "values": ["2025-01-02T00:00:00Z"]},
        ),
        (
            "irregular-instants",
            None,
            {
                "kind": "instants",
                "values": ["2025-01-04T00:00:00Z", "2025-01-01T00:00:00Z"],
            },
        ),
        ("empty-time", None, {"kind": "instants", "values": ["2024-01-01T00:00:00Z"]}),
        ("empty-space", [600000, 3990000, 600100, 3990100], None),
    ]
    rows = []
    for label, bbox, times in cases:
        request = copy.deepcopy(r)
        if bbox is not None:
            request["spatial"]["bbox"] = bbox
        if times is not None:
            request["time"] = times
        file = bench.WORK / f"check-{label}.json"
        file.write_text(json.dumps(request))
        out = bench.WORK / f"check-{label}"
        shutil.rmtree(out, ignore_errors=True)
        row = bench.process(
            [
                str(bench.ROOT / "target/release/tile-ripper"),
                "fetch",
                str(file),
                "--output",
                str(out),
            ],
            request,
            out,
            "fixed",
            False,
        )
        minimum = bench.unified.minimum_keys(request)
        actual = row["report"]["source_reads"]
        assert sorted(v["key"] for v in actual) == minimum
        assert all(v["requests"] == 1 for v in actual)
        if minimum:
            values = check(request, out)
        else:
            assert json.loads((out / "manifest.json").read_text())["outputs"] == []
            values = 0
        rows.append(dict(case=label, minimum_keys=minimum, validated_values=values))
    determinism = []
    for size in ["small", "medium"]:
        a = bench.WORK / f"after-{size}-fixed-0-0"
        b = bench.WORK / f"after-{size}-fixed-0-1"
        assert fingerprint(a) == fingerprint(b)
        before = bench.WORK / f"after-{size}-b3ceb70-0-0"
        assert fingerprint(a) == fingerprint(before)
        determinism.append(dict(workload=size, identical_objects=len(fingerprint(a))))
    (bench.ROOT / "docs/output-write-validation.json").write_text(
        json.dumps(dict(edge_cases=rows, determinism=determinism), indent=2)
    )


if __name__ == "__main__":
    main()
