#!/usr/bin/env python3
"""One bounded, credential-free Landsat signing/HEAD timing; no science GET.

The temporary SAS remains in memory. This complements original-provider CLI
receipts, whose complete execution includes per-asset signing and inspection.
"""
import json
from pathlib import Path
import time
from urllib.parse import urlsplit

import requests

ROOT = Path(__file__).resolve().parents[1]
receipt = json.loads((ROOT / "docs/products/wave2/landsat-le07-validation.json").read_text())
asset = next(source for source in receipt["sources"] if source["id"] == "SR_B3")
canonical = asset["href"]
session = requests.Session()
session.trust_env = False  # No ambient netrc or proxy credentials.
report = {"canonical_href": canonical, "scope": "one accountless sign plus conditional identity HEAD; no imagery body"}
started = time.perf_counter()
try:
    response = session.get("https://planetarycomputer.microsoft.com/api/sas/v1/sign", params={"href": canonical}, timeout=30)
    report["signing"] = {"status": response.status_code, "seconds": time.perf_counter() - started}
    response.raise_for_status()
    signed = response.json()["href"]
    original, temporary = urlsplit(canonical), urlsplit(signed)
    if (original.scheme, original.netloc, original.path) != (temporary.scheme, temporary.netloc, temporary.path):
        raise ValueError("signing changed canonical object")
    started = time.perf_counter()
    headers = {"If-Match": asset["etag"]} if asset.get("etag") else {}
    response = session.head(signed, headers=headers, timeout=30, allow_redirects=False)
    report["source_identity_head"] = {"status": response.status_code, "seconds": time.perf_counter() - started, "etag": response.headers.get("ETag"), "size": response.headers.get("Content-Length")}
    response.raise_for_status()
    if asset.get("etag") and response.headers.get("ETag") != asset["etag"]:
        raise ValueError("source changed")
    report["passed"] = True
except (requests.RequestException, KeyError, ValueError):
    report["passed"] = False
    report["failure"] = "selected source signing/HEAD failed; no fallback or temporary URL retained"
(ROOT / "docs/products/wave2/access-timing.json").write_text(json.dumps(report, indent=2) + "\n")
print(json.dumps(report))
