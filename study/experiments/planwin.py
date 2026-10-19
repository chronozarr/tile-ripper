"""Window definitions shared by the planner and the pixel workers (no heavy dependencies: also imported in the rasteret env)."""

X0, Y0, RES = (
    600000.0,
    4000020.0,
    10.0,
)  # tile 17SPV origin / 10 m grid (identical for every scene: E3, E4)

# pixel windows on the 10 m grid: W_A sits inside ONE 1024 block, W_B spans a 4-block corner
WINDOWS = {
    "W_A_chip400": dict(c0=7300, r0=2100, w=400, h=400),
    "W_B_corner600": dict(c0=6868, r0=1748, w=600, h=600),
}


def bounds(win: dict) -> tuple[float, float, float, float]:
    xmin = X0 + win["c0"] * RES
    ymax = Y0 - win["r0"] * RES
    return xmin, ymax - win["h"] * RES, xmin + win["w"] * RES, ymax
