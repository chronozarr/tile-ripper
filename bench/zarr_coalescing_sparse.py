"""Paired sparse follow-up; identical frozen sources, transport and completion boundary."""

import json
import statistics
import zarr_coalescing as c


def main(repetitions=9):
    work = c.WORK / "sparse-followup"
    work.mkdir(parents=True, exist_ok=True)
    rows = []
    groups = []
    for layout, source in c.layouts():
        for link in ["local-http", "8ms-100Mbps"]:
            server = c.Server(
                source,
                latency=0.008 if link.startswith("8") else 0,
                bandwidth=12_500_000 if link.startswith("8") else 0,
            ).start()
            r = c.h.request(server.url, work / "unused", shape=c.SHAPE)
            r["budget"]["chunk_edge"] = 32
            r["spatial"]["bbox"] = [10, 10, 95, 95]
            request = work / "request.json"
            request.write_text(json.dumps(r))
            pairs = []
            try:
                for rep in range(repetitions):
                    pair = {}
                    for reader in (
                        ["baseline", "cost"] if rep % 2 == 0 else ["cost", "baseline"]
                    ):
                        server.records.clear()
                        server.activity.clear()
                        server.next_byte = 0
                        binary = (
                            c.BASE / "zarr_subset_oracle"
                            if reader == "baseline"
                            else c.BINARY
                        )
                        result = c.measure(
                            [binary, request, "stream"],
                            {**c.ENV, "TILE_RIPPER_ZARR_COALESCE": "cost"},
                        )
                        assert "error" not in result, result
                        label = f"{layout}-{link}-{rep}-{reader}"
                        (work / (label + ".json")).write_text(json.dumps(result))
                        (work / (label + "-http.json")).write_text(
                            json.dumps(server.records)
                        )
                        c.h.verify(r, source, result)
                        row = dict(
                            layout=layout,
                            link=link,
                            rep=rep,
                            reader=reader,
                            seconds=result["seconds"],
                            prepare_seconds=result["prepare_seconds"],
                            first_seconds=result["runs"][0]["first_chunk_seconds"],
                            **c.traffic(server, source, layout != "ordinary"),
                        )
                        assert (
                            row["payload_requests"] == 6
                            and row["duplicate_ranges"] == 0
                        )
                        pair[reader] = row
                        rows.append(row)
                    assert (
                        pair["baseline"]["payload_bytes"]
                        == pair["cost"]["payload_bytes"]
                    )
                    pairs.append(pair["cost"]["seconds"] - pair["baseline"]["seconds"])
                groups.append(
                    dict(
                        layout=layout,
                        link=link,
                        baseline_median=statistics.median(
                            v["seconds"]
                            for v in rows
                            if v["layout"] == layout
                            and v["link"] == link
                            and v["reader"] == "baseline"
                        ),
                        cost_median=statistics.median(
                            v["seconds"]
                            for v in rows
                            if v["layout"] == layout
                            and v["link"] == link
                            and v["reader"] == "cost"
                        ),
                        paired_delta_median=statistics.median(pairs),
                        paired_delta_min=min(pairs),
                        paired_delta_max=max(pairs),
                        paired_deltas=pairs,
                    )
                )
                print(groups[-1], flush=True)
            finally:
                server.shutdown()
                server.server_close()
    (c.DOCS / "sparse-followup.json").write_text(
        json.dumps(dict(repetitions=repetitions, rows=rows, groups=groups), indent=2)
    )


if __name__ == "__main__":
    main()
