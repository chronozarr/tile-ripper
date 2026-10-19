"""E3: discovery (D) for a Tile-Ripper-shaped job: tile 17SPV (Chapel Hill AOI), 2025, cloud <= 20.

Compares, on identical predicates:
  API       Earth Search v1 POST /search (full items, and trimmed with `fields`)
  GP-https  TGE C1 year file through https://data.source.coop (documented door), cold
  GP-s3     same file through s3://us-west-2.opendata.source.coop, cold
  GP-warm   repeat on the same connection
Then checks result-set equality, whether asset hrefs are derivable from the row (skipping the ~6 MB
`assets` chunk), and freezes the scene list for the pixel experiments.
"""

from __future__ import annotations

import hashlib
import json
import statistics
import sys
import time

import httpx

from common import ES_API, RESULTS, USER_AGENT, save, summarize
from ddbprobe import connect, run_cold

AOI = (-79.10, 35.88, -79.02, 35.95)
TILE, YEAR, CC = "17SPV", 2025, 20
S3_SETUP = ["SET s3_region='us-west-2'", "SET s3_url_style='path'"]
P = f"tge-labs/s2-stac-geoparquet/sentinel-2-c1-l2a/year={YEAR}/items.parquet"
HTTPS_URL, S3_URL = (
    f"https://data.source.coop/{P}",
    f"s3://us-west-2.opendata.source.coop/{P}",
)
E84 = "https://e84-earth-search-sentinel-data.s3.us-west-2.amazonaws.com/sentinel-2-c1-l2a"

COLS = """id, datetime::VARCHAR AS datetime, "eo:cloud_cover" AS cc, "s2:nodata_pixel_percentage" AS nodata,
          platform, "s2:processing_baseline" AS baseline"""


ENV = f"ST_MakeEnvelope({AOI[0]}, {AOI[1]}, {AOI[2]}, {AOI[3]})"


def gp_sql(url: str, with_assets: bool = False, footprint: bool = True) -> str:
    extra = (
        ", json_extract_string(assets,'$.red.href') AS red, json_extract_string(assets,'$.nir.href') AS nir, json_extract_string(assets,'$.scl.href') AS scl, json_extract(assets,'$.red') AS red_obj"
        if with_assets
        else ""
    )
    return f"""SELECT {COLS}{extra} FROM read_parquet('{url}')
               WHERE _tile = '{TILE}' AND "eo:cloud_cover" <= {CC}{f" AND ST_Intersects(geometry, {ENV})" if footprint else ""} ORDER BY datetime"""


def api_search(trim: bool) -> dict:
    body: dict = {
        "collections": ["sentinel-2-c1-l2a"],
        "bbox": list(AOI),
        "limit": 200,
        "datetime": f"{YEAR}-01-01T00:00:00Z/{YEAR}-12-31T23:59:59Z",
        "query": {"eo:cloud_cover": {"lte": CC}},
    }
    if trim:
        body["fields"] = {
            "include": [
                "id",
                "properties.datetime",
                "properties.eo:cloud_cover",
                "properties.s2:nodata_pixel_percentage",
                "assets.red",
                "assets.nir",
                "assets.scl",
            ],
            "exclude": ["links"],
        }
    items, nbytes, pages = [], 0, 0
    t0 = time.perf_counter()
    with httpx.Client(timeout=60, headers={"User-Agent": USER_AGENT}) as c:
        url, req_body = f"{ES_API}/search", body
        while url:
            r = c.post(url, json=req_body)
            r.raise_for_status()
            nbytes += len(r.content)
            pages += 1
            j = r.json()
            items += j["features"]
            nxt = next(
                (link for link in j.get("links", []) if link.get("rel") == "next"), None
            )
            url, req_body = (
                (nxt["href"], nxt.get("body", body)) if nxt else (None, None)
            )
    return {
        "wall_s": time.perf_counter() - t0,
        "bytes": nbytes,
        "pages": pages,
        "items": items,
    }


def derive_hrefs(item_id: str, dt: str) -> dict[str, str]:
    tile = item_id.split("_")[1].lstrip("T")  # S2C_T17SPV_20250626T160859_L2A
    zone, band, sq = int(tile[:2]), tile[2], tile[3:5]
    month = int(dt[5:7])
    base = f"{E84}/{zone}/{band}/{sq}/{dt[:4]}/{month}/{item_id}"
    return {
        "red": f"{base}/B04.tif",
        "nir": f"{base}/B08.tif",
        "scl": f"{base}/SCL.tif",
    }


def main() -> None:
    reps = int(sys.argv[1]) if len(sys.argv) > 1 else 5
    out: dict = {"aoi": AOI, "tile": TILE, "year": YEAR, "cc_max": CC}

    print("== API (Earth Search v1)")
    api = {
        "full": [api_search(False) for _ in range(reps)],
        "trimmed": [api_search(True) for _ in range(reps)],
    }
    for k, v in api.items():
        print(
            f"  {k:8s} wall median {statistics.median(x['wall_s'] for x in v):.2f}s  bytes {v[0]['bytes'] / 1024:.0f} KiB  pages {v[0]['pages']}  items {len(v[0]['items'])}"
        )
    out["api"] = {
        k: {
            "wall_s": summarize([x["wall_s"] for x in v]),
            "bytes": v[0]["bytes"],
            "pages": v[0]["pages"],
            "items": len(v[0]["items"]),
        }
        for k, v in api.items()
    }
    api_ids = sorted(f["id"] for f in api["full"][0]["items"])

    print("== GeoParquet cold (fresh connection each)")
    gp = {}
    for label, url, setup in (("https", HTTPS_URL, []), ("s3", S3_URL, S3_SETUP)):
        for with_assets in (False, True):
            key = f"{label}{'+assets' if with_assets else ''}"
            rs = [run_cold(gp_sql(url, with_assets), setup) for _ in range(reps)]
            for r in rs:
                r.pop("requests")
            gp[key] = rs
            print(
                f"  {key:13s} wall median {statistics.median(r['wall_s'] for r in rs):.2f}s  GET {rs[0]['get']}  {rs[0]['bytes_in'] / 2**20:.2f} MiB  waves {rs[0]['waves']}  rows {rs[0]['rows']}  warm {statistics.median(r['warm_wall_s'] for r in rs):.2f}s"
            )
    out["geoparquet"] = {
        k: {
            "wall_s": summarize([r["wall_s"] for r in v]),
            "warm_wall_s": summarize([r["warm_wall_s"] for r in v]),
            "get": v[0]["get"],
            "MiB": v[0]["bytes_in"] / 2**20,
            "waves": sorted({r["waves"] for r in v}),
            "rows": v[0]["rows"],
        }
        for k, v in gp.items()
    }

    tile_only = (
        connect(S3_SETUP, log=False)
        .execute(gp_sql(S3_URL, False, footprint=False))
        .to_arrow_table()
        .num_rows
    )
    out["tile_only_rows"] = tile_only
    print(
        f"== result-set equality and href derivation (tile-only would select {tile_only} rows)"
    )
    con = connect(S3_SETUP, log=False)
    rows = con.execute(gp_sql(S3_URL, True)).fetchall()
    cols = [d[0] for d in con.execute(gp_sql(S3_URL, True)).description]
    recs = [dict(zip(cols, r)) for r in rows]
    gp_ids = sorted(r["id"] for r in recs)
    out["equality"] = {
        "api_n": len(api_ids),
        "gp_n": len(gp_ids),
        "only_in_api": sorted(set(api_ids) - set(gp_ids)),
        "only_in_gp": sorted(set(gp_ids) - set(api_ids)),
        "duplicate_ids_gp": len(gp_ids) - len(set(gp_ids)),
    }
    print(
        "  ",
        {
            k: (v if not isinstance(v, list) else (len(v), v[:3]))
            for k, v in out["equality"].items()
        },
    )
    bad = [
        r["id"]
        for r in recs
        if derive_hrefs(r["id"], r["datetime"])
        != {"red": r["red"], "nir": r["nir"], "scl": r["scl"]}
    ]
    out["href_derivation"] = {
        "checked": len(recs),
        "mismatches": len(bad),
        "examples": bad[:3],
    }
    print(f"   derived hrefs match actual for {len(recs) - len(bad)}/{len(recs)} rows")
    shapes = {
        (
            json.dumps(json.loads(r["red_obj"])["proj:shape"]),
            json.dumps(json.loads(r["red_obj"])["proj:transform"]),
        )
        for r in recs
    }
    out["red_grid_variants"] = [list(x) for x in shapes]
    print(
        "   distinct (proj:shape, proj:transform) for B04 over all scenes:",
        len(shapes),
        list(shapes)[:2],
    )

    print(
        "== freeze scene list (footprint-intersecting, cloud<=20, nodata<=5, first 24 by datetime)"
    )
    chosen = [r for r in recs if (r["nodata"] or 0) <= 5][:24]
    frozen = [
        {
            k: r[k]
            for k in (
                "id",
                "datetime",
                "cc",
                "nodata",
                "platform",
                "baseline",
                "red",
                "nir",
                "scl",
            )
        }
        for r in chosen
    ]
    blob = json.dumps(frozen, sort_keys=True).encode()
    path = RESULTS / "scenes_17SPV_2025.json"
    path.write_text(
        json.dumps(
            {
                "sha256": hashlib.sha256(blob).hexdigest(),
                "rule": "tile 17SPV AND footprint intersects AOI, 2025, cc<=20, nodata<=5, first 24 by datetime",
                "scenes": frozen,
            },
            indent=1,
        )
    )
    out["frozen"] = {
        "n": len(frozen),
        "sha256": hashlib.sha256(blob).hexdigest(),
        "first": frozen[0]["datetime"],
        "last": frozen[-1]["datetime"],
    }
    print("  ", out["frozen"])
    save("e3_discovery", out)


if __name__ == "__main__":
    main()
