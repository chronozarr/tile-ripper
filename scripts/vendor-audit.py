#!/usr/bin/env python3
"""Compare the local forks with cached, checksum-pinned upstream crates."""
import argparse
import difflib
import hashlib
import json
import os
from pathlib import Path
import tarfile

ROOT = Path(__file__).resolve().parents[1]
CRATES = {
    "tiff-reader": ("0.9.0", "5f5f0c80d1e2039b4cf5218b4fb55c93fbc1a466a1db893936407a00c3d59de2"),
    "hdf5-reader": ("0.9.3", "c50e22fc5c53e8e23edd603050ac342ea4cde1898b64bf86266d60f6a9cc3963"),
    "zarrs_filesystem": ("0.3.13", "a63fc5f551c941a98a5942814030c937ec979effb35e5edbf7f6334323aefd78"),
}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=False)
    cache = Path(os.environ.get("CARGO_HOME", Path.home() / ".cargo")) / "registry/cache"
    result = {"status": "passed", "forks": []}
    for name, (version, expected) in CRATES.items():
        archives = list(cache.glob("*/" + name + "-" + version + ".crate"))
        if not archives:
            raise RuntimeError(f"Missing cached upstream crate: {name} {version}")
        archive = archives[0]
        assert hashlib.sha256(archive.read_bytes()).hexdigest() == expected, name
        prefix = name + "-" + version + "/"
        with tarfile.open(archive) as source:
            original = {p.name[len(prefix):]: source.extractfile(p).read()
                        for p in source.getmembers() if p.isfile() and p.name.startswith(prefix)}
        local = ROOT / "vendor" / name
        files, patch = [], []
        for p in sorted(local.rglob("*")):
            if not p.is_file() or "target" in p.relative_to(local).parts:
                continue
            relative = p.relative_to(local).as_posix()
            current = p.read_bytes()
            previous = original.get(relative)
            row = {"path": relative, "sha256": hashlib.sha256(current).hexdigest(),
                   "status": "added" if previous is None else "unchanged" if current == previous else "modified"}
            if previous is not None:
                row["upstream_sha256"] = hashlib.sha256(previous).hexdigest()
            files.append(row)
            if previous is not None and current != previous:
                patch.extend(difflib.unified_diff(previous.decode().splitlines(True), current.decode().splitlines(True),
                                                 fromfile="a/" + relative, tofile="b/" + relative))
        patch_path = output / (name + ".patch")
        patch_path.write_text("".join(patch))
        result["forks"].append({"name": name, "version": version, "crate_sha256": expected,
                                "patch": patch_path.name, "files": files,
                                "omitted_upstream_files": sorted(set(original) - {r["path"] for r in files})})
    (output / "vendor-audit.json").write_text(json.dumps(result, indent=2) + "\n")
    print(json.dumps({r["name"]: [f["path"] for f in r["files"] if f["status"] == "modified"]
                      for r in result["forks"]}))


if __name__ == "__main__":
    main()
