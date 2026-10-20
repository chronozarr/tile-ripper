"""Derive compact profile/check receipts from the retained output experiments."""
import hashlib
import json
from pathlib import Path
import platform
import re
import statistics
import subprocess
import tomllib

ROOT=Path(__file__).resolve().parents[1]
DOCS=ROOT/"docs/durable-output"
WORK=ROOT/"prototype-output/durable-output"


def save(name,value):
    (DOCS/name).write_text(json.dumps(value,indent=2)+"\n")


def main():
    benchmark=json.loads((DOCS/"benchmark.json").read_text())
    assert benchmark["completed"] and benchmark["stable_modes_passed"]
    assert len(benchmark["runs"])==144
    profile=[]
    for size in ["small","medium","large"]:
        for mode in ["old","raw","gzip","blosc_zstd","raw128","raw512","sharded","stream"]:
            rows=[r for r in benchmark["runs"] if r["workload"]==size and r["link"]["latency_ms"]==0 and r["mode"]==mode and r["passed"]]
            if not rows:continue
            def med(f):return statistics.median(f(r) for r in rows)
            stages={s["stage"] for r in rows for s in r["stages"]}
            if mode not in ["old","stream"]:assert "publication_total" in stages
            row=dict(workload=size,mode=mode,wall_seconds=med(lambda r:r["process"]["wall_seconds"]),
                source_http_seconds=med(lambda r:r["observed"]["source_retrieval_seconds"]),
                source_read_decode_seconds=med(lambda r:r["observed"]["read_decode_seconds"]),
                native_decode_copy_residual_seconds=med(lambda r:max(0,r["observed"]["read_decode_seconds"]-r["observed"]["source_retrieval_seconds"])),
                sync_seconds=med(lambda r:sum(r["filesystem"][k][f] for k in ["output","manifest","directories"] for f in ["fsync_seconds","fullsync_seconds"])),
                file_open_create_seconds=med(lambda r:sum(r["filesystem"][k]["open_seconds"] for k in ["output","manifest"])),
                file_write_seconds=med(lambda r:sum(r["filesystem"][k]["write_seconds"] for k in ["output","manifest"])),
                mkdir_seconds=med(lambda r:r["filesystem"]["mkdir_seconds"]),
                stages={s:med(lambda r:sum(v["seconds"] for v in r["stages"] if v["stage"]==s)) for s in sorted(stages)},
                writer_reservation_bytes=med(lambda r:(r["observed"].get("output_io") or {}).get("buffered_bytes_peak",0)),
                scheduler_reservation_bytes=med(lambda r:r["observed"]["scheduler"]["reserved_bytes_peak"]))
            # The old field was left at its default zero. Do not present it as
            # measured zero encoding cost. New calls explicitly exclude store I/O.
            row["encode_library_seconds"]=None if mode=="old" else med(lambda r:(r["observed"].get("output_io") or {}).get("encode_seconds",0))
            if rows[0]["reading"]:
                row["warm_independent_reads"]={k:dict(wall_seconds=med(lambda r:r["reading"][k]["wall_seconds"]),cpu_seconds=med(lambda r:r["reading"][k]["cpu_seconds"])) for k in ["full","subset"]}
            profile.append(row)
    save("profile.json",dict(profiles=profile,
        scope="Three matched warm-local repetitions per successful mode. Source worker durations overlap sink wall time; decode residual includes copying and reader bookkeeping. File open/create, pwrite/write and sync timers are syscall-facing. Directory walks and unwrapped stat work remain within setup/final-directory-stage and residual costs, not a claimed exhaustive syscall trace. Original encode-only field is unmeasured (null), not zero. Original writer bottleneck is independently timed by its actual sync calls.",
        baseline_commit="82e21b15352471471900e03806dd6d7b22003afc"))
    results=[]
    for name in ["tests-default-final","tests-native-final","tests-experimental-final","tests-all-features"]:
        data=(WORK/f"{name}.log").read_text()
        counts=re.findall(r"test result: ok\. (\d+) passed; (\d+) failed; (\d+) ignored",data)
        assert counts and all(int(f)==0 for _,f,_ in counts) and "error:" not in data
        results.append(dict(check=name,passed=sum(int(p) for p,_,_ in counts),ignored=sum(int(i) for _,_,i in counts),failed=0))
    for name in ["clippy-default-final","clippy-native-final","clippy-all-features","check-zarr-only-final","consumer-build-final"]:
        data=(WORK/f"{name}.log").read_text()
        assert "Finished" in data and "error:" not in data
        results.append(dict(check=name,passed=True))
    base=tomllib.loads(subprocess.check_output(["git","show","82e21b1:Cargo.lock"],cwd=ROOT,text=True))
    now=tomllib.loads((ROOT/"Cargo.lock").read_text())
    def registry(lock):return {(p["name"],p["version"],p.get("checksum")) for p in lock["package"] if p.get("source","").startswith("registry+")}
    assert registry(base)==registry(now)
    files=sorted([ROOT/"Cargo.toml",ROOT/"Cargo.lock",*ROOT.glob("src/**/*.rs"),*ROOT.glob("vendor/zarrs_filesystem/src/*.rs")])
    digest=hashlib.sha256()
    for path in files:
        digest.update(str(path.relative_to(ROOT)).encode());digest.update(path.read_bytes())
    receipts=["candidates.json","faults.json","interoperability-v2.json","live-default/live.json","live-candidates.json","memory.json","regressions.json","linux.json"]
    assert all(json.loads((DOCS/p).read_text())["passed"] for p in receipts)
    save("verification.json",dict(stable_improvement_gate_passed=True,experimental_sharding_gate_passed=False,
        checks=results,independent_receipts=receipts,platform=platform.platform(),architecture=platform.machine(),
        rustc=subprocess.check_output(["rustc","--version"],text=True).strip(),
        baseline_commit="82e21b15352471471900e03806dd6d7b22003afc",
        production_source_sha256=digest.hexdigest(),registry_packages_and_checksums_unchanged=True,
        binaries=benchmark["binaries"],
        experimental_failure_count=sum(not r["passed"] for r in benchmark["runs"]),
        scope="Rust test logs, strict clippy, Zarr-only compile, separate locked consumer build, macOS and Linux scientific/fault receipts. cargo fmt --check and git diff --check are separate closeout commands. Ignored external-fixture tests remain explicitly ignored; inherited independent harnesses are replayed in regressions.json."))
    failures=[r for r in benchmark["runs"] if not r["passed"]]
    save("sharding-failures.json",dict(candidate="experimental_v3_sharded_gzip",passed=False,failures=failures,
        interpretation="Pinned zarrs 0.22.10's bounded sharding encoder reports an insufficient allocation. The error originates in encode_bounded, which documents an incorrect inner-codec size bound as the cause of this guard. The gzip encoded-representation bound is a lead, not a completed upstream root-cause fix. All failed attempts clean their staging directory and publish nothing; no failed duration is a successful performance result.",
        next_milestone="Reproduce and resolve the upstream bound failure; test raw and compressed shards and the existing partial-encoding path, CRC32C index configuration, write amplification and independent readers before any production promotion."))
    print(json.dumps(dict(stable_gate=True,experimental_gate=False,rust_checks=results,benchmark_runs=144,experimental_failures=len(failures))))


if __name__=="__main__":main()
