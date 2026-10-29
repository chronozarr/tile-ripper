"""Check bounded cross-scene overlap, ordered delivery and failure propagation."""

from threading import Barrier, Event, Lock
import time

from hls_river_30day import BANDS
from hls_scene_pool import ScenePool


def protocol():
    items = [{"id": f"scene{i}"} for i in range(8)]
    pins = [
        {"item": item["id"], "band": band, "index": index * 4 + b}
        for index, item in enumerate(items)
        for b, band in enumerate([*BANDS, "Fmask"])
    ]
    return {"items": items, "pins": pins}


def main():
    p = protocol()
    gate, lock = Barrier(16, timeout=5), Lock()
    next_scene_started = Event()
    active = peak = 0
    started = set()

    def read(pin):
        nonlocal active, peak
        with lock:
            active += 1
            peak = max(peak, active)
            started.add(pin["index"])
        if pin["index"] < 16:
            gate.wait()
        if pin["index"] == 16:
            next_scene_started.set()
        if pin["index"] == 0:
            assert next_scene_started.wait(5), (
                "work must advance beyond the first four scenes before ordered delivery"
            )
        time.sleep((4 - pin["index"] % 4) * 0.001)
        with lock:
            active -= 1
        return pin["index"]

    with ScenePool(p) as pool:
        results = []
        for scene in range(8):
            values = list(pool.map(read, p["pins"][scene * 4 : scene * 4 + 4]))
            if scene == 0:
                assert set(range(16)) <= started, "four scenes must overlap"
            results.extend(values)
            assert len(pool.pending) <= 32
    assert results == list(range(32))
    assert started == set(range(32)) and active == 0 and peak == 16
    assert pool.stats()["max_pending_futures"] == 32

    calls = []

    def bad_read(pin):
        with lock:
            calls.append(pin["index"])
        if pin["index"] == 0:
            raise ValueError("source identity failure")
        time.sleep(0.001)
        return pin["index"]

    try:
        with ScenePool(p) as pool:
            list(pool.map(bad_read, p["pins"][:4]))
    except ValueError as error:
        assert str(error) == "source identity failure"
    else:
        raise AssertionError("source failure must propagate")
    assert pool.delivered == 0 and pool.submitted == 32 and not pool.pending
    assert set(calls) <= set(range(32))
    print(
        "PASS: sixteen-way cross-scene overlap, bounded retention, chronology and terminal errors"
    )


if __name__ == "__main__":
    main()
