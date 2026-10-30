//! Supported command routes, compiled reader availability and playback boundaries.
//!
//! These descriptors perform no discovery or authorization. Named-product policy
//! stays owned by `products::routing`; physical source inspection remains required.
use crate::{products, Result};
use serde_json::{json, Value};

/// The exact collection admitted by the generic Earth Search adapter.
pub const EARTH_SEARCH_SENTINEL2_COLLECTION: &str = "sentinel-2-c1-l2a";

/// Describe supported routes without reading credentials, catalogs or source data.
pub fn describe() -> Result<Value> {
    let native_cog = cfg!(feature = "native-cog");
    let authenticated_http = cfg!(feature = "authenticated-http");
    let cog_reader = native_cog || cfg!(feature = "gdal");
    let mut named_products = Vec::new();
    for profile in products::profile::profiles() {
        let mut provider_routes = Vec::new();
        for route in products::routing::routes(profile.id)? {
            let protected = route.provider == products::routing::Provider::Earthdata;
            let discoverable = route.provider != products::routing::Provider::Pinned;
            let mut descriptor = serde_json::to_value(route)?;
            descriptor["available_in_build"] =
                json!(native_cog && (!protected || authenticated_http));
            descriptor["required_features"] = if protected {
                json!(["native-cog", "authenticated-http"])
            } else {
                json!(["native-cog"])
            };
            descriptor["availability_scope"] = if protected {
                json!("authenticated_production_acquisition")
            } else {
                json!("listed_source_route_acquisition")
            };
            descriptor["discovery_available_in_build"] = json!(discoverable);
            descriptor["credential_status"] = json!("not_checked");
            provider_routes.push(descriptor);
        }
        named_products.push(json!({
            "product":profile.id,
            "profile_catalog":profile.catalog,
            "provider_routes":provider_routes,
            "available_in_build":native_cog,
            "required_features":["native-cog"],
            "supplied_observations":{
                "available_in_build":native_cog,
                "required_features":["native-cog"],
                "discovery":false,
                "scope":"Qualified explicitly supplied public reference objects or local mirrors remain subject to the profile and existing source/provider rules. This does not establish access to protected production archives."
            },
            "chronozarr_bridge":"conditional_completed_cog_observation_output",
            "limitations":"Provider descriptions retain the existing routing policy. Availability describes compiled acquisition features, not configured authorization, catalog coverage or physical source qualification. Explicit supplied observations remain subject to their existing provider/source rules."
        }));
    }
    Ok(json!({
        "schema":"tile-ripper-routes-v1",
        "scope":"Supported routes only; not a universal catalog. Listing does not inspect sources, obtain credentials, sign assets or select a provider.",
        "build_features":{
            "gdal":cfg!(feature="gdal"),
            "native_cog":native_cog,
            "native_hdf5":cfg!(feature="native-hdf5"),
            "authenticated_http":authenticated_http,
            "icechunk":cfg!(feature="icechunk")
        },
        "generic":[
            {
                "id":"earth-search-sentinel-2-c1-l2a",
                "request_format":"stac",
                "provider":"earth_search",
                "endpoint":"https://earth-search.aws.element84.com/v1",
                "collection":EARTH_SEARCH_SENTINEL2_COLLECTION,
                "discovery":true,
                "source_selection":"Bounded Earth Search-compatible STAC discovery for this exact collection; discovered native band assets remain explicit source observations.",
                "access":"Public HTTPS catalog and anonymous HTTPS COG assets; no account required for the qualified Earth Search route.",
                "automatic_planetary_computer_signing":false,
                "available_in_build":cog_reader,
                "required_features_any":[["native-cog"],["gdal"]],
                "native_profile_available_in_build":native_cog,
                "native_build":"--no-default-features --features native-cog",
                "playback_output_requirement":"Request --output-format zarr_v2 for the ordinary per-observation COG acquisition_result consumed by the direct bridge.",
                "chronozarr_bridge":"conditional_completed_cog_observation_output",
                "limitations":"Exact Sentinel-2 Collection 1 Level-2A adapter; qualified native UInt16 UTM bands only. This does not admit Planetary Computer sentinel-1-rtc, arbitrary STAC collections, resampling or automatic provider substitution. Alternate endpoints remain subject to the same adapter/schema and HTTPS restrictions."
            },
            {
                "id":"explicit-cog",
                "request_source_kind":"assets",
                "asset_format":"cog",
                "request_file_required":true,
                "cli_format_flag":null,
                "example_request":"examples/cog-request.json",
                "provider":"supplied_source",
                "collection":null,
                "discovery":false,
                "source_selection":"Caller supplies each exact asset URL or local path; no catalog search or inferred mission/product/provider route.",
                "access":"Local files or anonymous HTTPS sources accepted by the selected reader. This generic CLI route does not supply protected-object authentication; an unsigned URL does not grant access.",
                "automatic_planetary_computer_signing":false,
                "available_in_build":cog_reader,
                "required_features_any":[["native-cog"],["gdal"]],
                "native_profile_available_in_build":native_cog,
                "native_build":"--no-default-features --features native-cog",
                "explicit_authentication":{
                    "cli_supported":false,
                    "scope":"The generic CLI does not route supplied COGs through the separate authenticated COG library API. --auth/--netrc do not add that route; use a qualified named-product route where one exists."
                },
                "chronozarr_bridge":"conditional_on_required_source_metadata",
                "limitations":"The generic native reader admits its narrow self-contained tiled single-band UInt16 profile. Other scientific types belong to separately qualified named profiles. Even ordinary Zarr v2 output is not bridge-eligible unless every selected observation includes the required collection, item_id and requested_band source metadata."
            }
        ],
        "named_products":named_products,
        "array_products":{
            "descriptors":"See the existing array_profiles field when native-hdf5 is compiled; its unavailable-build descriptor is preserved otherwise.",
            "required_features":["native-hdf5"],
            "available_in_build":cfg!(feature="native-hdf5"),
            "chronozarr_bridge":"unsupported_input_schema",
            "limitations":"Native HDF5/array acquisition outputs are outside the direct per-observation COG bridge; product-specific access and qualification still apply."
        },
        "chronozarr_bridge":{
            "status":"checkout_only",
            "entrypoint":"chronozarr.convert_tile_ripper",
            "provided_by":"separate Chronozarr source checkout",
            "installed_by_tile_ripper":false,
            "published_in_chronozarr_0_5_1":false,
            "input_schema":"acquisition_result",
            "input_writer":"per_observation_cog",
            "input_format":"zarr_v2",
            "input_encoding":"uncompressed_c_order_unfiltered",
            "required_source_metadata":["source.collection","source.item_id","source.requested_band"],
            "eligibility":[
                "Completed local acquisition_result with its per-observation groups and source metadata.",
                "One exact collection and native grid; explicitly select groups when other grids/series are present.",
                "Unique millisecond timestamps, every selected band at every time, one science dtype, and fixed calibration per science band.",
                "Source-valid and AOI masks must satisfy the bridge checks. An optional quality policy must already have been written by the fetch and join unambiguously to its science scene."
            ],
            "unsupported_inputs":[
                "generic Zarr or native HDF5/array writer schemas",
                "compressed, filtered, sharded or Zarr v3 observation outputs",
                "a persistent tile-ripper stack as direct-bridge input",
                "observations without required source identity or band metadata"
            ],
            "limitations":"Eligibility is checked by the existing Python bridge, not guaranteed by a product name or build feature. It performs no resampling, mosaicking, compositing, cloud inference or automatic orbit/grid alignment. Other Chronozarr conversion APIs have separate input contracts.",
            "documentation":"docs/tile-ripper.md in the Chronozarr checkout"
        }
    }))
}
