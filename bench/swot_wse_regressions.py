"""Repeat inherited gates in fresh SWOT receipt directories, without old-doc edits."""

import argparse
import hashlib
import json
import os
import re
import shutil
import subprocess
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
WORK = ROOT / "prototype-output/swot-wse/inherited"
DOCS = ROOT / "docs/swot-wse/inherited"
WORK.mkdir(parents=True, exist_ok=True)
DOCS.mkdir(parents=True, exist_ok=True)


def matrix(only=None):
    rows = json.loads((DOCS / "rust.json").read_text()) if only else []
    for name, flags in [
        ("default", []),
        ("none", ["--no-default-features"]),
        ("cog", ["--no-default-features", "--features", "native-cog"]),
        ("hdf5", ["--no-default-features", "--features", "native-hdf5"]),
        ("all", ["--all-features"]),
        (
            "hdf5-reader",
            [
                "--manifest-path",
                "vendor/hdf5-reader/Cargo.toml",
                "--no-default-features",
            ],
        ),
    ]:
        if only and name not in only:
            continue
        for action in ["test", "clippy"]:
            cmd = ["cargo", action, "--locked", *flags]
            cmd += (
                (["--lib"] if name == "hdf5-reader" else [])
                if action == "test"
                else ["--all-targets", "--", "-D", "warnings"]
            )
            log = WORK / f"{name}-{action}.log"
            with log.open("w") as f:
                result = subprocess.run(
                    cmd, cwd=ROOT, stdout=f, stderr=subprocess.STDOUT, check=False
                )
            raw = log.read_bytes()
            text = raw.decode()
            row = {
                "name": f"{name}-{action}",
                "command": cmd,
                "returncode": result.returncode,
                "log_sha256": hashlib.sha256(raw).hexdigest(),
                "passed": sum(
                    map(int, re.findall(r"test result: ok\. (\d+) passed", text))
                ),
                "ignored": sum(map(int, re.findall(r"; (\d+) ignored", text))),
            }
            rows = [r for r in rows if r["name"] != row["name"]] + [row]
            (DOCS / "rust.json").write_text(json.dumps(rows, indent=2) + "\n")
            assert result.returncode == 0, str(log)
            print(row["name"], "PASS", row["passed"], flush=True)


def hdf5(action):
    import hdf5_input as h
    import hdf5_selective as selected

    if action.startswith("selective-"):
        suffix = action.removeprefix("selective-")
        h.WORK = WORK / "hdf5-selective"
        h.DOCS = DOCS / "hdf5-selective"
        selected.WORK, selected.DOCS = h.WORK, h.DOCS
        h.WORK.mkdir(parents=True, exist_ok=True)
        h.DOCS.mkdir(parents=True, exist_ok=True)
        if suffix in ["science", "faults"]:
            selected.inherited_hdf5(suffix)
        else:
            getattr(selected, suffix)()
    else:
        h.WORK = WORK / "hdf5"
        h.DOCS = DOCS / "hdf5"
        h.WORK.mkdir(parents=True, exist_ok=True)
        h.DOCS.mkdir(parents=True, exist_ok=True)
        if action == "resources":
            import hdf5_resources

            shutil.copyfile(
                ROOT / "prototype-output/native-hdf5/goes19-reference-control.nc",
                h.WORK / "goes19-reference-control.nc",
            )
            hdf5_resources.main()
        else:
            getattr(h, action)()


def inherited(action):
    os.environ["TILE_RIPPER_HDF5_REGRESSION_WORK"] = str(WORK / "formats")
    os.environ["TILE_RIPPER_HDF5_REGRESSION_DOCS"] = str(DOCS / "formats")
    import hdf5_regressions

    hdf5_regressions.main(action)


def coordinates():
    import hdf5_input as h
    import hdf5_selective as selected
    from hdf5_selective_mirror import Mirror, server

    h.WORK = WORK / "hdf5-selective"
    h.DOCS = DOCS / "hdf5-selective"
    selected.WORK, selected.DOCS = h.WORK, h.DOCS
    original = ROOT / "prototype-output/hdf5-selective/large-mirror"
    mirror = Mirror(original)
    mirror.verify()
    alias = h.WORK / "large-mirror"
    alias.mkdir(exist_ok=True)
    if not (alias / "source.nc").exists():
        (alias / "source.nc").symlink_to(mirror.path)
    rows = []
    with server(mirror) as srv:
        for name, selection in selected.SELECTIONS.items():
            req = h.request(srv.url, "/TEMP", *selection)
            for policy in ["full", "selective"]:
                label = (
                    f"0-{name}-0-selective-exact"
                    if policy == "selective"
                    else f"goes-{name}-full"
                )
                with selected.policies(policy):
                    out = h.run_native(req, label)
                rows.append(
                    {
                        "selection": name,
                        "index": policy,
                        **h.validate(out, mirror.path, req),
                        **h.compact(out),
                    }
                )
        assert not srv.errors, srv.errors
    h.save(
        "goes-regression",
        {
            "cases": rows,
            "original_source": {k: mirror.info[k] for k in ["url", "etag", "size"]},
            "new_provider_bytes": 0,
        },
    )
    selected.coordinates()


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("action")
    args = parser.parse_args()
    if args.action == "coordinates":
        coordinates()
    elif args.action == "refresh-rust":
        matrix({"hdf5", "all"})
    elif args.action == "matrix":
        matrix()
    elif args.action.startswith("hdf5-"):
        hdf5(args.action.removeprefix("hdf5-"))
    else:
        inherited(args.action)
