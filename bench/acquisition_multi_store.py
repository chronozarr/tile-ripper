import copy, json, shutil, sys
from pathlib import Path
ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "bench"))
import remote_zarr, zarr_workflow

WORK = ROOT / "prototype-output/acquisition-multi-store"
remote_zarr.WORK = WORK
zarr_workflow.WORK = WORK / "fixture-base"
remote_zarr.ROOT = ROOT
remote_zarr.CURRENT = ROOT / "target/release/tile-ripper"
shutil.rmtree(WORK, ignore_errors=True)
WORK.mkdir(parents=True, exist_ok=True)
r, source1 = remote_zarr.fixture_setup()
source2 = WORK / "source-second.zarr"
shutil.copytree(source1, source2)
s1 = remote_zarr.start(source1)
s2 = remote_zarr.start(source2)
try:
    single = copy.deepcopy(r)
    single["source"]["assets"][0]["locator"] = s1.url
    single["sink"] = {"kind": "directory", "path": str(WORK / "single-out")}
    one_req = WORK / "single-request.json"
    one_req.write_text(json.dumps(single))
    one = remote_zarr.process([str(remote_zarr.CURRENT), "fetch", str(one_req)])
    assert one["code"] == 0, one["error"]
    single_wire_records = copy.deepcopy(s1.records)
    one_obs = one["result"]["observed"]
    one_reads = sorted((v["key"], v["requests"], v["bytes"]) for v in one_obs["source_reads"])
    assert one_reads and all(n == 1 for _, n, _ in one_reads)
    # Exclude the single-store baseline when counting the subsequent two-store fetch.
    with s1.lock:
        s1.records.clear()

    both = copy.deepcopy(r)
    a1 = copy.deepcopy(both["source"]["assets"][0])
    a1["id"] = "cube-a"
    a1["locator"] = s1.url
    a2 = copy.deepcopy(a1)
    a2["id"] = "cube-b"
    a2["locator"] = s2.url
    both["source"]["assets"] = [a1, a2]
    both["sink"] = {"kind": "directory", "path": str(WORK / "both-out")}
    both_req = WORK / "both-request.json"
    both_req.write_text(json.dumps(both))
    two = remote_zarr.process([str(remote_zarr.CURRENT), "fetch", str(both_req)])
    assert two["code"] == 0, two["error"]
    combined_wire_records = {"cube-a": copy.deepcopy(s1.records), "cube-b": copy.deepcopy(s2.records)}
    obs = two["result"]["observed"]
    reads = obs["source_reads"]
    for asset_id in ["cube-a", "cube-b"]:
        actual = sorted((v["key"], v["requests"], v["bytes"]) for v in reads if v["asset_id"] == asset_id)
        assert actual == one_reads, (asset_id, actual, one_reads)
    assert len(reads) == 2 * len(one_reads)
    for key in ["chunks", "decoded_bytes", "non_nodata_pixels", "decoded_source_chunks", "filled_source_chunks", "source_data_requests", "source_data_bytes"]:
        expected = one_obs.get(key, 0)
        assert obs.get(key, 0) == 2 * expected, (key, obs.get(key), expected)
    metadata_keys = {".zgroup", ".zattrs", ".zmetadata", "zarr.json"}
    def is_metadata(record):
        key = record["key"]
        return key in metadata_keys or key.endswith("/.zarray") or key.endswith("/.zattrs") or key.endswith("/.zgroup") or key.endswith("/zarr.json") or key.startswith(("x/", "y/", "time/"))
    metadata_bytes = sum(x["bytes"] for server in [s1, s2] for x in server.records if x["method"] == "GET" and is_metadata(x))
    buffer_bytes = both["budget"]["buffer_bytes"]
    assert metadata_bytes <= buffer_bytes // 2, (metadata_bytes, buffer_bytes // 2)
    wire_data_gets = {}
    for asset_id, server in [("cube-a", s1), ("cube-b", s2)]:
        data_keys = {key for key, _, _ in one_reads}
        data_gets = [x for x in server.records if x["method"] == "GET" and x["key"] in data_keys]
        counts = {}
        for item in data_gets: counts[item["key"]] = counts.get(item["key"], 0) + 1
        wire_data_gets[asset_id] = {"total_gets": len(data_gets), "keys": len(counts), "gets_per_key": sorted(set(counts.values())), "counts": counts}
    (WORK / "wire-evidence.json").write_text(json.dumps({"single": {"records": single_wire_records, "transport": one_obs.get("transport")}, "combined": {k: {"records": v, "transport": obs.get("transport", {}).get(k)} for k, v in combined_wire_records.items()}, "acquisition_source_reads": reads}, indent=2))
    assert all(x["gets_per_key"] == [1] for x in wire_data_gets.values()), wire_data_gets

    # Exercise the aggregate metadata cap: each padded store fits the per-store
    # budget, while the pair exceeds the shared half-buffer reservation.
    reject1 = WORK / "reject-source-a.zarr"
    reject2 = WORK / "reject-source-b.zarr"
    shutil.copytree(source1, reject1)
    shutil.copytree(source2, reject2)
    for path in [reject1 / ".zattrs", reject2 / ".zattrs"]:
        attrs = json.loads(path.read_text())
        attrs["metadata_budget_probe"] = "x" * 16000
        path.write_text(json.dumps(attrs))
    s3 = remote_zarr.start(reject1)
    s4 = remote_zarr.start(reject2)
    try:
        small_single = copy.deepcopy(r)
        small_single["budget"]["buffer_bytes"] = 65536
        small_single["source"]["assets"][0]["locator"] = s3.url
        small_single["sink"] = {"kind": "callback"}
        single_plan = WORK / "small-single.json"
        single_plan.write_text(json.dumps(small_single))
        single_plan_result = remote_zarr.process([str(remote_zarr.CURRENT), "plan", str(single_plan)])
        assert single_plan_result["code"] == 0, single_plan_result["error"]

        small_both = copy.deepcopy(small_single)
        b1 = copy.deepcopy(small_both["source"]["assets"][0])
        b1["id"] = "padded-a"
        b2 = copy.deepcopy(b1)
        b2["id"] = "padded-b"
        b2["locator"] = s4.url
        small_both["source"]["assets"] = [b1, b2]
        both_plan = WORK / "small-both.json"
        both_plan.write_text(json.dumps(small_both))
        both_plan_result = remote_zarr.process([str(remote_zarr.CURRENT), "plan", str(both_plan)])
        assert both_plan_result["code"] != 0, both_plan_result
        assert "aggregate HTTP metadata/coordinate cache exceeds half buffer_bytes" in both_plan_result["error"], both_plan_result["error"]
        aggregate_budget_rejection = True
    finally:
        s3.shutdown(); s3.server_close()
        s4.shutdown(); s4.server_close()
    print(json.dumps({"passed": True, "single_chunks": one_obs["chunks"], "combined_chunks": obs["chunks"], "single_source_reads": len(one_reads), "combined_source_reads": len(reads), "metadata_get_bytes": metadata_bytes, "buffer_bytes": buffer_bytes, "shared_metadata_cap_bytes": buffer_bytes // 2, "per_asset_reads_once": True, "raw_wire_data_gets_diagnostic": wire_data_gets, "aggregate_metadata_budget_rejection": aggregate_budget_rejection, "rejection_buffer_bytes": 65536}))
finally:
    s1.shutdown(); s1.server_close()
    s2.shutdown(); s2.server_close()
