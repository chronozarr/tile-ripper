"""Record primary registry publication dates for the lock; no package execution."""

import concurrent.futures
import datetime
import json
from pathlib import Path
import tomllib
import urllib.request

CUTOFF = datetime.datetime(2026, 10, 11, tzinfo=datetime.timezone.utc)
PACKAGES = [
    p
    for p in tomllib.loads(Path("Cargo.lock").read_text())["package"]
    if p.get("source", "").startswith("registry")
]
OLD = {}
for file in (
    "docs/dependency-inventory.json",
    "prototype-output/stac-dependency-inventory.json",
):
    if Path(file).exists():
        OLD.update(
            {
                (r["name"], r["version"]): r
                for r in json.loads(Path(file).read_text())
                if "published" in r
            }
        )


def get(url):
    request = urllib.request.Request(
        url, headers={"User-Agent": "tile-ripper-dependency-inventory/0.1"}
    )
    with urllib.request.urlopen(request, timeout=30) as response:
        return json.load(response)


def inspect(p):
    key = (p["name"], p["version"])
    if key in OLD:
        return OLD[key]
    v = get(f"https://crates.io/api/v1/crates/{p['name']}/{p['version']}")["version"]
    row = dict(
        name=p["name"],
        version=p["version"],
        published=v["created_at"],
        license=v.get("license"),
        checksum=p["checksum"],
    )
    if datetime.datetime.fromisoformat(v["created_at"].replace("Z", "+00:00")) > CUTOFF:
        versions = get(f"https://crates.io/api/v1/crates/{p['name']}")["versions"]
        before = [
            x for x in versions if not x["yanked"] and x["created_at"] < "2026-10-11"
        ]
        row["older_candidates"] = [x["num"] for x in before[:8]]
    return row


with concurrent.futures.ThreadPoolExecutor(4) as pool:
    rows = list(pool.map(inspect, PACKAGES))
Path("prototype-output").mkdir(exist_ok=True)
Path("prototype-output/stac-dependency-inventory.json").write_text(
    json.dumps(rows, indent=2) + "\n"
)
young = [
    r
    for r in rows
    if datetime.datetime.fromisoformat(r["published"].replace("Z", "+00:00")) > CUTOFF
]
print(json.dumps(dict(packages=len(rows), younger=young), indent=2))
