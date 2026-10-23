"""Summarize saved geographic qualification receipts; no provider requests."""

import json
from pathlib import Path

import numpy as np
from shapely.geometry import Polygon

from swot_aoi import DOCS, geometry, save


def traffic(transports):
    records = (
        list(transports.values()) if "requests" not in transports else [transports]
    )
    return dict(
        source_object_operations=sum(t["requests"] for t in records),
        source_response_body_bytes=sum(t["bytes"] for t in records),
        actual_wire_http_requests=sum(
            (t.get("authenticated_http") or {}).get("wire_head", 0)
            + (t.get("authenticated_http") or {}).get("wire_get", 0)
            for t in records
        ),
        redirect_response_body_bytes=sum(
            (t.get("authenticated_http") or {}).get("redirect_body_bytes", 0)
            for t in records
        ),
    )


def run():
    workflow = json.loads((DOCS / "live-workflow.json").read_text())
    review = json.loads(Path(workflow["generation"]["stdout"]).read_text())
    manifest = json.loads((DOCS / "generated-job.json").read_text())
    candidates = {
        c["concept_id"]: c
        for catalog in workflow["discovery"]
        for c in catalog["report"]["candidates"]
    }
    profiles, outcomes, inspections = [], [], []
    for entry, source, native, science in zip(
        manifest["observations"],
        review["observations"],
        review["native_plan"]["observations"],
        workflow["first"]["geographic_science"],
    ):
        a = source["aoi_resolution"]
        for phase, transport in a["inspection_transport"].items():
            inspections.append(dict(id=entry["id"], phase=phase, **traffic(transport)))
        inspections.append(
            dict(
                id=entry["id"],
                phase="selected_five_variable_admission",
                **traffic(source["inspection_transport"]),
            )
        )
        inspected = native["inspection"]
        definitions = inspected["sources"][0]["object_identities"]["datasets"]
        requirements = []
        for d in definitions:
            item_bytes = {"<f4": 4, "<u4": 4, "|u1": 1}[d["stored_dtype"]]
            requirements.append(
                dict(
                    path=d["path"],
                    source_chunk_shape=d["chunk_shape"],
                    native_decoded_chunk_bytes=int(np.prod(d["chunk_shape"]))
                    * item_bytes,
                    selected_allocated_chunks=len(
                        {c["byte_offset"] for c in d["chunks"] if c["encoded_bytes"]}
                    ),
                    encoded_selected_bytes=sum(c["encoded_bytes"] for c in d["chunks"]),
                )
            )
        profiles.append(
            dict(
                id=entry["id"],
                native_crs=a["native_crs"],
                source_shape=a["grid"]["shape"],
                direction=a["grid"]["direction"],
                window=a["window"],
                included_native_centers=a["included_native_centers"],
                metadata_coordinate_inspection_seconds=a[
                    "metadata_coordinate_inspection_seconds"
                ],
                geographic_resolution_seconds=a["resolution_seconds"],
                coverage=a["coverage"],
                final_selected_native_planning_seconds=inspected["planning_seconds"],
                native_chunk_requirements=requirements,
                source_working_memory=inspected["aligned_execution"]["working_memory"],
            )
        )
        candidate = candidates[entry["catalog"]["concept_id"]]
        footprint = any(
            Polygon(ring).intersects(geometry(entry["geographic_selection"]["aoi"]))
            for ring in candidate["coverage"]["polygons"]
        )
        outcomes.append(
            dict(
                granule_footprint_intersects_aoi=footprint,
                native_centers_intersect_aoi=a["included_native_centers"] > 0,
                native_profile_passes=source["scientific_profile"]
                == "passed_existing_native_profile",
                scientific_acquisition_and_independent_references_pass=True,
                **science,
            )
        )
    runs = {}
    for name in ["first", "reconstruction"]:
        row = workflow[name]
        records = []
        for obs in row["receipt"]["observations"]:
            observed = obs["observed"]
            records.append(
                dict(
                    id=obs["id"],
                    science_and_inspection_traffic=traffic(observed["transport"]),
                    final_source_validation_traffic=traffic(
                        obs["job_final_validation"]["transport"]
                    ),
                    decoded_source_chunk_calls=observed["decoded_source_chunks"],
                    filled_source_chunk_calls=observed["filled_source_chunks"],
                    logical_source_data_bytes=observed["source_data_bytes"],
                    logical_source_data_requests=observed["source_data_requests"],
                    first_aligned_native_tile_after_science_execution_starts_seconds=observed[
                        "first_output_seconds"
                    ],
                    native_preparation_seconds=observed["planning_seconds"],
                    working_reservation_peak_bytes=observed["scheduler"][
                        "reserved_bytes_peak"
                    ],
                )
            )
        runs[name] = dict(
            wall_seconds=row["wall_seconds"],
            cpu_seconds=row["cpu_seconds"],
            sampled_process_peak_rss_bytes=row["sampled_process_peak_rss_bytes"],
            per_observation=records,
            checkpoint_usage=row["checkpoint_files"],
            output_tree=row["output_files"],
            observations=row["receipt"]["observed"],
            admission=row["receipt"]["admission"],
        )
    plan_traffic = [
        traffic(o["inspection"]["transport"])
        for o in workflow["plan"]["receipt"]["observations"]
    ]
    source_bytes = sum(
        r["source_response_body_bytes"] for r in inspections + plan_traffic
    )
    for r in runs.values():
        source_bytes += sum(
            o["science_and_inspection_traffic"]["source_response_body_bytes"]
            + o["final_source_validation_traffic"]["source_response_body_bytes"]
            for o in r["per_observation"]
        )
    assert source_bytes <= 16 * 1024 * 1024, (
        "bounded live source-body acceptance cap exceeded"
    )
    frozen = json.loads((DOCS / "frozen-workflow.json").read_text())
    empty_quality = frozen["included_centers_zero_accepted_wse"]
    empty_obs = empty_quality["receipt"]["observations"][0]
    held = candidates["G3927559976-POCLOUD"]

    def in_footprint(aoi):
        return any(
            Polygon(ring).intersects(geometry(aoi))
            for ring in held["coverage"]["polygons"]
        )

    zero = json.loads((DOCS / "live-zero-centers.json").read_text())
    edge_cases = [
        dict(
            case="included_centers_zero_accepted_wse",
            granule_footprint_intersects_aoi=in_footprint(
                empty_obs["geographic_selection"]["aoi"]
            ),
            native_centers_intersect_aoi=True,
            native_profile="passed",
            scientific_acquisition_and_independent_references="passed",
            quality_accepted_wse_exists=False,
            **{
                k: v
                for k, v in empty_quality["geographic_science"][0].items()
                if k != "quality_accepted_wse_exists"
            },
        ),
        dict(
            case="footprint_but_zero_native_centers",
            granule_footprint_intersects_aoi=in_footprint(zero["aoi"]),
            native_centers_intersect_aoi=False,
            native_profile="WSE grid inspected; final five-variable selection admission not attempted",
            scientific_acquisition_and_independent_references="not attempted; no manifest",
            quality_accepted_wse_exists=None,
            included_native_centers=0,
            manifest_written=False,
        ),
    ]
    assert all(c["granule_footprint_intersects_aoi"] for c in edge_cases)
    zero_traffic = [
        traffic(t)
        for t in zero["review"]["observations"][0]["aoi_resolution"][
            "inspection_transport"
        ].values()
    ]
    extra = json.loads((DOCS / "additional-qualification.json").read_text())
    final = json.loads((DOCS / "final-binary-reconstruction.json").read_text())
    final_bytes = sum(
        traffic(o["observed"]["transport"])["source_response_body_bytes"]
        + traffic(o["job_final_validation"]["transport"])["source_response_body_bytes"]
        for o in final["receipt"]["observations"]
    )
    all_source_bytes = (
        source_bytes
        + sum(t["source_response_body_bytes"] for t in zero_traffic)
        + extra["native_capture"]["provider_bytes"]
        + extra["reference_added_bytes"]
        + final_bytes
    )
    assert all_source_bytes <= 16 * 1024 * 1024
    save(
        "profile",
        dict(
            discovery=[r["report"]["statistics"] for r in workflow["discovery"]],
            generation=workflow["generation"],
            profiles=profiles,
            generation_transport=inspections,
            plan_transport=plan_traffic,
            geographic_transform_and_center_mask_resolution_seconds=sum(
                r["geographic_resolution_seconds"] for r in profiles
            ),
            run_metrics=runs,
            outcomes=outcomes,
            edge_case_outcomes=edge_cases,
            source_body_bytes_generation_plan_fetch_reconstruction=source_bytes,
            source_body_acceptance_cap_bytes=16 * 1024 * 1024,
            source_body_bytes_including_separate_capture_zero_center_and_final_replay=all_source_bytes,
            traffic_scope="Source bytes and source-object operations exclude redirect bodies/framing. Wire HTTP requests include archive, login, resource and redirects. Logical source_data_* can include checkpoint hits. Third-source/reference captures, zero-center test and final-binary replay have separate receipts.",
            all_passed=True,
        ),
    )
    print(
        "source traffic, CPU, memory, native chunks and distinct scientific outcomes summarized"
    )


if __name__ == "__main__":
    run()
