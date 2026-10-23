"""Small independent synthetic grid over the existing raw SWOT policy fixture."""

from pathlib import Path
import shutil
import json
import h5py
import numpy as np

ROOT = Path(__file__).resolve().parents[1]
source = ROOT / "tests/data/swot/quality-le.h5"
target = ROOT / "tests/data/swot/grid-inspection.h5"
shutil.copyfile(source, target)
review = json.loads((ROOT / "docs/swot-aoi/live-review.json").read_text())
attrs = review["observations"][1]["native_crs"]["attributes"]
with h5py.File(target, "r+") as f:
    f.attrs["descriptor_string"] = np.bytes_("100m_UTM19T_N_x_x_x")
    for name, attr in attrs.items():
        if "value" not in attr or name == "_FillValue":
            continue
        value = attr["value"]
        f["crs"].attrs[name] = np.bytes_(value) if isinstance(value, str) else value
    for name, standard in [
        ("y", "projection_y_coordinate"),
        ("x", "projection_x_coordinate"),
    ]:
        f[name].attrs["standard_name"] = np.bytes_(standard)
print(target)
