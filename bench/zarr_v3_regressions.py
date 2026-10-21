"""Post-integration feature gates and inherited scientific/fault regressions."""

import json
import os
import re
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
WORK = Path(
    os.environ.get(
        "TILE_RIPPER_V3_REGRESSION_WORK",
        ROOT / "prototype-output/zarr-v3-input/regressions",
    )
)
DOCS = Path(
    os.environ.get(
        "TILE_RIPPER_V3_REGRESSION_DOCS", ROOT / "docs/zarr-v3-input/regressions"
    )
)
BIN_DIR = Path(
    os.environ.get(
        "TILE_RIPPER_V3_REGRESSION_BIN_DIR", ROOT / "target/scientific-output/release"
    )
)


def matrix():
    WORK.mkdir(parents=True, exist_ok=True)
    DOCS.mkdir(parents=True, exist_ok=True)
    rows = []
    env = {**os.environ, "CARGO_INCREMENTAL": "0"}
    for label, features in [
        ("default", []),
        ("native", ["--no-default-features", "--features", "native-cog"]),
        ("zarr-only", ["--no-default-features"]),
        ("all", ["--all-features"]),
    ]:
        cmd = ["cargo", "test", "--locked", *features]
        log = WORK / (label + ".log")
        with log.open("w") as f:
            p = subprocess.run(
                cmd, cwd=ROOT, env=env, stdout=f, stderr=subprocess.STDOUT
            )
        assert p.returncode == 0, str(log)
        counts = re.findall(
            r"test result: ok\. (\d+) passed; (\d+) failed; (\d+) ignored",
            log.read_text(),
        )
        rows.append(
            {
                "configuration": label,
                "command": cmd,
                "passed": sum(int(v[0]) for v in counts),
                "ignored": sum(int(v[2]) for v in counts),
                "code": p.returncode,
            }
        )
        print(rows[-1], flush=True)
    for label, features in [
        ("default", []),
        ("native", ["--no-default-features", "--features", "native-cog"]),
        ("zarr-only", ["--no-default-features"]),
        ("all", ["--all-features"]),
    ]:
        cmd = [
            "cargo",
            "clippy",
            "--locked",
            *features,
            "--all-targets",
            "--",
            "-D",
            "warnings",
        ]
        log = WORK / (label + "-clippy.log")
        with log.open("w") as f:
            p = subprocess.run(
                cmd, cwd=ROOT, env=env, stdout=f, stderr=subprocess.STDOUT
            )
        assert p.returncode == 0, str(log)
        rows.append({"configuration": label, "command": cmd, "code": p.returncode})
        print(label, "clippy PASS", flush=True)
    # Refresh the path-dependent public consumer's root dependency edges first.
    p = subprocess.run(
        ["cargo", "check", "--manifest-path", "consumers/native-stats/Cargo.toml"],
        env=env,
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
        text=True,
    )
    (WORK / "consumer.log").write_text(p.stdout)
    assert p.returncode == 0
    subprocess.run(
        [
            "cargo",
            "check",
            "--locked",
            "--manifest-path",
            "consumers/native-stats/Cargo.toml",
        ],
        env=env,
        check=True,
        stdout=subprocess.DEVNULL,
    )
    (DOCS / "rust.json").write_text(json.dumps(rows, indent=2))


def inherited(action):
    DOCS.mkdir(parents=True, exist_ok=True)
    os.environ["TILE_RIPPER_VALIDATION_DOCS"] = str(DOCS)
    if action in ["remote", "cf"]:
        import zarr_workflow
        import remote_zarr

        remote_zarr.CURRENT = BIN_DIR / "tile-ripper"
        remote_zarr.EXAMPLE = remote_zarr.CURRENT.parent / "examples/remote_zarr"
        remote_zarr.WORK = WORK / "remote"
        zarr_workflow.WORK = WORK / "v2-fixture"
        if action == "remote":
            remote_zarr.checks()
        else:
            import cf_zarr

            cf_zarr.WORK = WORK / "cf"
            cf_zarr.h.CURRENT = remote_zarr.CURRENT
            cf_zarr.checks()
    else:
        import durable_output_validation as gate

        gate.WORK = WORK / action
        gate.DOCS = DOCS
        gate.BIN = BIN_DIR / "tile-ripper"
        gate.WORK.mkdir(parents=True, exist_ok=True)
        if action == "checks":
            gate.checks([("default", 256)])
        else:
            getattr(gate, action)()


if __name__ == "__main__":
    action = sys.argv[1]
    matrix() if action == "matrix" else inherited(action)
