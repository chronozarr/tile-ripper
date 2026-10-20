"""Feature-matrix verification with explicit fixture-dependent convenience coverage."""

import hashlib
import json
import os
from pathlib import Path
import re
import subprocess
import time


ROOT = Path(__file__).resolve().parents[1]
WORK = ROOT / "prototype-output/native-http/verification"


def main():
    WORK.mkdir(parents=True, exist_ok=True)
    runs = []
    commands = [
        ("default", ["cargo", "test", "--locked"]),
        ("all-features", ["cargo", "test", "--locked", "--all-features"]),
        ("native-only", ["cargo", "test", "--locked", "--no-default-features", "--features", "native-cog"]),
        ("zarr-only", ["cargo", "test", "--locked", "--no-default-features"]),
        ("native-clippy", ["cargo", "clippy", "--locked", "--no-default-features", "--features", "native-cog", "--all-targets", "--", "-D", "warnings"]),
        ("all-features-clippy", ["cargo", "clippy", "--locked", "--all-features", "--all-targets", "--", "-D", "warnings"]),
        ("format", ["cargo", "fmt", "--all", "--check"]),
        ("convenience-late-mutation", ["cargo", "test", "--locked", "--no-default-features", "--features", "native-cog",
                                       "--test", "native_cog", "--", "--ignored"]),
    ]
    for name, command in commands:
        env = dict(os.environ)
        if name == "convenience-late-mutation":
            env["TILE_RIPPER_NATIVE_REQUEST"] = str(ROOT / "prototype-output/native-http/checks/cog-faults/profile-deflate-2-NO-LITTLE.json")
        started = time.monotonic()
        done = subprocess.run(command, cwd=ROOT, env=env, capture_output=True, text=True)
        log = WORK / f"{name}.log"
        log.write_text(done.stdout + done.stderr)
        assert done.returncode == 0, f"{name}: {log}\n{done.stdout}\n{done.stderr}"
        counts = re.findall(r"test result: ok\. (\d+) passed; (\d+) failed; (\d+) ignored", done.stdout)
        runs.append(dict(name=name, command=command, passed=True, seconds=time.monotonic()-started,
                         tests_passed=sum(int(a) for a,_,_ in counts),
                         tests_ignored=sum(int(c) for _,_,c in counts), log=str(log.relative_to(ROOT))))
        print(name, "PASS", flush=True)
    files = sorted([*ROOT.glob("src/**/*.rs"), *ROOT.glob("consumers/native-stats/src/**/*.rs"),
                    ROOT/"Cargo.toml", ROOT/"Cargo.lock", ROOT/"consumers/native-stats/Cargo.toml",
                    ROOT/"consumers/native-stats/Cargo.lock"])
    source_hashes = {str(p.relative_to(ROOT)):hashlib.sha256(p.read_bytes()).hexdigest() for p in files}
    candidate = json.loads((ROOT / "docs/native-http/candidate.json").read_text())
    assert source_hashes == candidate["files_sha256"], "production source changed after candidate freeze"
    record = dict(passed=True, runs=runs, source_sha256=source_hashes,
                  source_fingerprint_sha256=candidate["source_fingerprint_sha256"],
                  candidate_source_files_identical=True,
                  harness_sha256=hashlib.sha256(Path(__file__).read_bytes()).hexdigest())
    (ROOT / "docs/native-http/verification.json").write_text(json.dumps(record, indent=2) + "\n")


if __name__ == "__main__":
    main()
