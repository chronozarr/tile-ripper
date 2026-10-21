import io
import json
import urllib.request

import h5py
import numpy as np

URL = "https://noaa-goes19.s3.amazonaws.com/ABI-L2-RRQPEF/2025/100/12/OR_ABI-L2-RRQPEF-M6_G19_s20251001200211_e20251001209519_c20251001210009.nc"


class Remote(io.RawIOBase):
    def __init__(self, url):
        self.url = url
        self.p = 0
        self.reads = []
        with urllib.request.urlopen(
            urllib.request.Request(url, method="HEAD"), timeout=30
        ) as r:
            self.size = int(r.headers["Content-Length"])
            self.etag = r.headers["ETag"]

    def readable(self):
        return True

    def seekable(self):
        return True

    def tell(self):
        return self.p

    def seek(self, n, w=0):
        self.p = n if w == 0 else self.p + n if w == 1 else self.size + n
        return self.p

    def readinto(self, b):
        n = len(b)
        if n > 4 * 1024 * 1024:
            raise ValueError("unbounded reference read")
        req = urllib.request.Request(
            self.url,
            headers={
                "Range": f"bytes={self.p}-{self.p + n - 1}",
                "If-Match": self.etag,
            },
        )
        with urllib.request.urlopen(req, timeout=30) as r:
            assert r.status == 206 and r.headers["ETag"] == self.etag
            raw = r.read(n + 1)
            assert len(raw) == n
        self.reads.append([self.p, n])
        self.p += n
        b[:] = raw
        return n


def main():
    r = Remote(URL)
    with h5py.File(r, "r") as f:
        out = {
            "url": URL,
            "size": r.size,
            "etag": r.etag,
            "superblock": f.id.get_create_plist().get_version(),
            "keys": list(f),
            "datasets": {},
        }
        for name in ["RRQPE", "x", "y", "t", "goes_imager_projection"]:
            d = f[name]
            attrs = {
                k: (v.tolist() if hasattr(v, "tolist") else v)
                for k, v in d.attrs.items()
                if k not in ["DIMENSION_LIST", "REFERENCE_LIST"]
            }
            out["datasets"][name] = {
                "shape": d.shape,
                "dtype": str(d.dtype),
                "chunks": d.chunks,
                "compression": d.compression,
                "filters": [
                    d.id.get_create_plist().get_filter(i)
                    for i in range(d.id.get_create_plist().get_nfilters())
                ],
                "attrs": attrs,
            }
            if name == "RRQPE":
                out["datasets"][name]["chunks_total"] = d.id.get_num_chunks()
                out["datasets"][name]["selected_chunk"] = str(
                    d.id.get_chunk_info_by_coord((2500, 2500))
                )
                a = d[2500:2564, 2500:2564]
                out["datasets"][name]["subset"] = {
                    "min": float(np.min(a)),
                    "max": float(np.max(a)),
                    "nonzero": int(np.count_nonzero(a)),
                }
        out["reads"] = r.reads
        out["bytes"] = sum(x[1] for x in r.reads)
        print(
            json.dumps(
                out,
                default=lambda x: x.decode() if isinstance(x, bytes) else str(x),
                indent=2,
            )
        )


if __name__ == "__main__":
    main()
