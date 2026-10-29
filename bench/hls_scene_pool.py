"""Bounded cross-scene acquisition for the unchanged HLS science recipe.

Replace only its executor: read_asset still does the original fresh identity,
grid/type/calibration checks and conditional GDAL reads. Sixteen workers each
use one HTTP/1.1 connection. At most thirty-two asset futures are retained; results
are delivered in the original scene/band order to the original QA and encoder.
"""

from collections import deque
from concurrent.futures import ThreadPoolExecutor

from hls_river_30day import BANDS


class ScenePool:
    def __init__(self, protocol, max_workers=4):
        assert max_workers == 4, "original recipe changed"
        self.pins = [
            next(
                p
                for p in protocol["pins"]
                if p["item"] == item["id"] and p["band"] == band
            )
            for item in protocol["items"]
            for band in [*BANDS, "Fmask"]
        ]
        assert len(self.pins) == len(protocol["pins"])
        assert len({(p["item"], p["band"]) for p in self.pins}) == len(self.pins)
        self.executor = ThreadPoolExecutor(max_workers=16)
        self.pending = deque()
        self.submitted = 0
        self.delivered = 0
        self.read_asset = None
        self.peak_pending = 0

    def __enter__(self):
        self.executor.__enter__()
        return self

    def __exit__(self, kind, value, traceback):
        self.executor.shutdown(wait=True, cancel_futures=True)
        self.pending.clear()
        if kind is None:
            assert self.delivered == self.submitted == len(self.pins)

    def fill(self):
        while len(self.pending) < 32 and self.submitted < len(self.pins):
            self.pending.append(
                self.executor.submit(self.read_asset, self.pins[self.submitted])
            )
            self.submitted += 1
        self.peak_pending = max(self.peak_pending, len(self.pending))

    def map(self, function, pins):
        pins = list(pins)
        assert len(pins) == 4 and pins == self.pins[self.delivered : self.delivered + 4]
        if self.read_asset is None:
            self.read_asset = function
            self.fill()
        else:
            assert self.read_asset is function
        for _ in pins:
            future = self.pending.popleft()
            result = future.result()
            self.delivered += 1
            del future
            # Drop this iterator's reference before submitting the next asset.
            # The caller retains the current scene's four result arrays.
            yield result
            del result
            self.fill()

    def stats(self):
        return {
            "workers": 16,
            "max_pending_futures": self.peak_pending,
            "submitted_assets": self.submitted,
            "delivered_assets": self.delivered,
            "order": "original chronological scene and B03/B04/B8A/Fmask order",
        }
