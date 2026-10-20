"""Inspect real Mach-O executables, dependency closure and process startup."""

import hashlib
import json
from pathlib import Path
import platform
import random
import statistics
import subprocess
import time

ROOT = Path(__file__).resolve().parents[1]
WORK = ROOT / "prototype-output/native-cog"


def dependencies(path):
    output = subprocess.check_output(["otool", "-L", str(path)], text=True)
    return output, [
        s.strip().split(" (", 1)[0] for s in output.splitlines()[1:] if s.strip()
    ]


def closure(path):
    seen = {}
    pending = [path]
    while pending:
        _, libs = dependencies(pending.pop())
        for name in libs:
            p = Path(name)
            if name.startswith("/opt/homebrew/") and p.exists():
                p = p.resolve()
                if str(p) not in seen:
                    seen[str(p)] = p.stat().st_size
                    pending.append(p)
    return seen


def main():
    report = dict(platform=platform.platform(), binaries={})
    rng = random.Random(20261019)
    starts = {k: [] for k in ["native", "baseline"]}
    # Alternate randomized fresh processes. Help opens no data/network connection.
    for _ in range(21):
        modes = list(starts)
        rng.shuffle(modes)
        for mode in modes:
            p = WORK / mode / "tile-ripper"
            t = time.perf_counter()
            subprocess.run([str(p), "--help"], stdout=subprocess.DEVNULL, check=True)
            starts[mode].append(time.perf_counter() - t)
    for mode in starts:
        cli = WORK / mode / "tile-ripper"
        c = closure(cli)
        report["binaries"][mode] = dict(
            non_system_dynamic_files=len(c),
            non_system_dynamic_bytes=sum(c.values()),
            non_system_closure=c,
            startup_seconds=dict(
                median=statistics.median(starts[mode]),
                min=min(starts[mode]),
                max=max(starts[mode]),
            ),
        )
        for name in ["tile-ripper", "tile-ripper-native-stats"]:
            p = WORK / mode / name
            output, libs = dependencies(p)
            report["binaries"][mode][name] = dict(
                file_bytes=p.stat().st_size,
                sha256=hashlib.sha256(p.read_bytes()).hexdigest(),
                dynamic_libraries=libs,
            )
    report["native_tree"] = subprocess.check_output(
        [
            "cargo",
            "tree",
            "--locked",
            "--no-default-features",
            "--features",
            "native-cog",
        ],
        cwd=ROOT,
        text=True,
    )
    for lib in ["gdal v", "gdal-sys v", "proj-sys v", "libtiff"]:
        assert lib not in report["native_tree"]
    assert report["binaries"]["native"]["non_system_dynamic_files"] == 0
    (WORK / "deployment-macos.json").write_text(json.dumps(report, indent=2))
    print(
        json.dumps(
            {
                k: {x: v for x, v in row.items() if x not in ["non_system_closure"]}
                for k, row in report["binaries"].items()
            },
            indent=2,
        )
    )


if __name__ == "__main__":
    main()
