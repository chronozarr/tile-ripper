//! Exact SMAP SPL3SMP_E v006/R19240 global and northern polar AM/PM profiles.
use super::*;
pub(super) const AM: &str = "smap-spl3smp-e-v6-am";
pub(super) const PM: &str = "smap-spl3smp-e-v6-pm";
pub(super) const NORTH_AM: &str = "smap-spl3smp-e-v6-north-am";
pub(super) const NORTH_PM: &str = "smap-spl3smp-e-v6-north-pm";
const COLLECTION: &str = "SPL3SMP_E";
const CONCEPT: &str = "C2938664763-NSIDC_CPRD";
const ARCHIVE: &str = "data.nsidc.earthdatacloud.nasa.gov";
pub(super) fn supports(id: &str) -> bool {
    [AM, PM, NORTH_AM, NORTH_PM].contains(&id)
}
fn north(id: &str) -> bool {
    [NORTH_AM, NORTH_PM].contains(&id)
}
fn pm(id: &str) -> bool {
    [PM, NORTH_PM].contains(&id)
}
fn group(id: &str) -> &'static str {
    match (north(id), pm(id)) {
        (true, true) => "Soil_Moisture_Retrieval_Data_Polar_PM",
        (true, false) => "Soil_Moisture_Retrieval_Data_Polar_AM",
        (false, true) => "Soil_Moisture_Retrieval_Data_PM",
        (false, false) => "Soil_Moisture_Retrieval_Data_AM",
    }
}
fn layer(id: &str, name: &str) -> String {
    format!("{name}{}", if pm(id) { "_pm" } else { "" })
}
fn axes(id: &str) -> [String; 2] {
    let grid = if north(id) { "n09" } else { "m09" };
    [
        format!("/@smap_ease2_{grid}/y"),
        format!("/@smap_ease2_{grid}/x"),
    ]
}
fn validate(r: &ProductRequest) -> Result<()> {
    if !supports(&r.product) {
        return fail("unknown SMAP product");
    }
    let start = chrono::DateTime::parse_from_rfc3339(&r.start)?;
    let end = chrono::DateTime::parse_from_rfc3339(&r.end)?;
    if end < start
        || end - start > chrono::Duration::days(31)
        || !(1..=32).contains(&r.max_observations)
        || r.buffer_bytes < 32 * 1024 * 1024
        || r.buffer_bytes > 1024 * 1024 * 1024
        || r.chunk_edge == 0
        || r.chunk_edge > 256
    {
        return fail(
            "SMAP requires <=31 days, 1..32 observations, 32..1024 MiB and chunk_edge 1..256",
        );
    }
    if r.bands.is_empty()
        || r.bands.len() > 4
        || r.bands.iter().any(|b| {
            ![
                "soil_moisture",
                "retrieval_qual_flag",
                "surface_flag",
                "freeze_thaw_fraction",
            ]
            .contains(&b.as_str())
        })
        || r.bands
            .iter()
            .enumerate()
            .any(|(i, b)| r.bands[..i].contains(b))
    {
        return fail("SMAP selects distinct soil_moisture, retrieval_qual_flag, surface_flag and/or freeze_thaw_fraction");
    }
    if r.quality_policies.len() > 1
        || r.quality_policies
            .iter()
            .any(|p| p != "smap_retrieval_recommended")
        || (!r.quality_policies.is_empty() && !r.bands.iter().any(|b| b == "retrieval_qual_flag"))
    {
        return fail(
            "SMAP policy smap_retrieval_recommended requires original retrieval_qual_flag",
        );
    }
    Inclusion::new(&r.aoi)?;
    if north(&r.product) && !matches!(r.aoi, Spatial::ArraySlice { .. }) {
        let inclusion = Inclusion::new(&r.aoi)?;
        if inclusion.geometry_bounds().is_some_and(|b| b.min().y < 0.) {
            return fail("SMAP northern product requires northern-hemisphere AOIs; longitude wrapping unsupported");
        }
    }
    if let Some(o) = &r.observations {
        if o.len() > r.max_observations || serde_json::to_vec(o)?.len() > 1024 * 1024 {
            return fail("SMAP observation count/metadata cap exceeded");
        }
    }
    Ok(())
}
fn day(id: &str) -> Result<chrono::NaiveDate> {
    let parts = id.split('_').collect::<Vec<_>>();
    if parts.len() != 8
        || parts[..5] != ["SMAP", "L3", "SM", "P", "E"]
        || parts[6] != "R19240"
        || parts[7] != "001"
        || parts[5].len() != 8
        || !parts[5].bytes().all(|b| b.is_ascii_digit())
    {
        return fail("SMAP requires exact daily R19240_001 identity");
    }
    Ok(chrono::NaiveDate::parse_from_str(parts[5], "%Y%m%d")?)
}
fn validate_observation(r: &ProductRequest, o: &Observation) -> Result<()> {
    let date = day(&o.id)?;
    if o.platform.as_deref().is_some_and(|p| p != "SMAP")
        || o.processing.as_deref().is_some_and(|p| p != "006/R19240")
    {
        return fail("SMAP catalog platform/processing conflicts with profile");
    }
    if o.collection != COLLECTION
        || chrono::DateTime::parse_from_rfc3339(&o.datetime)?.date_naive() != date
    {
        return fail("SMAP collection/acquisition identity conflict");
    }
    let dt = chrono::DateTime::parse_from_rfc3339(&o.datetime)?;
    if dt < chrono::DateTime::parse_from_rfc3339(&r.start)?
        || dt > chrono::DateTime::parse_from_rfc3339(&r.end)?
    {
        return fail("SMAP observation outside requested dates");
    }
    let mut first: Option<&ProductAsset> = None;
    for b in &r.bands {
        let a = o
            .assets
            .get(b)
            .ok_or("SMAP observation missing selected layer")?;
        if first.is_some_and(|f| f.href != a.href || f.etag != a.etag) {
            return fail("SMAP variables must share one exact HDF5 source version");
        }
        first = Some(a);
        let u = reqwest::Url::parse(&a.href);
        let protected = u.as_ref().is_ok_and(|u| u.host_str() == Some(ARCHIVE));
        if let Ok(u) = &u {
            if u.query().is_some()
                || u.fragment().is_some()
                || !u.username().is_empty()
                || u.password().is_some()
            {
                return fail("SMAP requires canonical unsigned locators");
            }
        }
        match r.provider {
            Provider::Auto | Provider::Earthdata if protected => {
                use chrono::Datelike;
                let expected = format!(
                    "/nsidc-cumulus-prod-protected/SMAP/SPL3SMP_E/006/{}/{:02}/{:02}/{}.h5",
                    date.year(),
                    date.month(),
                    date.day(),
                    o.id
                );
                let u = u?;
                if u.scheme() != "https" || u.port().is_some() || u.path() != expected {
                    return fail("SMAP exact NSIDC object route mismatch");
                }
            }
            Provider::Pinned if r.observations.is_some() && !protected => (),
            _ => return fail("SMAP provider conflicts with supplied source; no fallback"),
        }
    }
    Ok(())
}
pub(super) fn discover(r: &ProductRequest) -> Result<Value> {
    validate(r)?;
    let started = Instant::now();
    let observations = if let Some(o) = &r.observations {
        o.clone()
    } else {
        if !matches!(r.provider, Provider::Auto | Provider::Earthdata) {
            return fail("SMAP discovery requires NSIDC Earthdata catalog");
        }
        let c = reqwest::blocking::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(std::time::Duration::from_secs(45))
            .build()?;
        let mut q = vec![
            ("collection_concept_id".to_string(), CONCEPT.into()),
            ("temporal".into(), format!("{},{}", r.start, r.end)),
            ("page_size".into(), r.max_observations.to_string()),
            ("sort_key".into(), "start_date".into()),
        ];
        if let Some(bounds) = Inclusion::new(&r.aoi)?.geometry_bounds() {
            let bbox = [
                bounds.min().x,
                bounds.min().y,
                bounds.max().x,
                bounds.max().y,
            ];
            q.push((
                "bounding_box".into(),
                bbox.iter()
                    .map(f64::to_string)
                    .collect::<Vec<_>>()
                    .join(","),
            ));
        }
        let response = c
            .get("https://cmr.earthdata.nasa.gov/search/granules.json")
            .query(&q)
            .send()?;
        if response.status() != 200 {
            return fail("SMAP CMR discovery HTTP failure");
        }
        let hits = response
            .headers()
            .get("CMR-Hits")
            .ok_or("SMAP CMR hit count absent")?
            .to_str()?
            .parse::<usize>()?;
        if hits > r.max_observations {
            return fail("SMAP catalog exceeds observation cap; narrow date interval");
        }
        let mut b = Vec::new();
        response.take(4 * 1024 * 1024 + 1).read_to_end(&mut b)?;
        if b.len() > 4 * 1024 * 1024 {
            return fail("SMAP CMR JSON cap exceeded");
        }
        let value: Value = serde_json::from_slice(&b)?;
        let entries = value["feed"]["entry"]
            .as_array()
            .ok_or("SMAP CMR entries absent")?;
        if entries.len() != hits {
            return fail("SMAP CMR incomplete listing");
        }
        let mut out = Vec::new();
        for e in entries {
            let id = e["producer_granule_id"]
                .as_str()
                .ok_or("SMAP granule id absent")?
                .trim_end_matches(".h5");
            day(id)?;
            let links = e["links"].as_array().ok_or("SMAP data links absent")?;
            let hrefs = links
                .iter()
                .filter_map(|v| v["href"].as_str())
                .filter(|u| {
                    reqwest::Url::parse(u).is_ok_and(|u| {
                        u.host_str() == Some(ARCHIVE) && u.path().ends_with(&format!("/{id}.h5"))
                    })
                })
                .collect::<Vec<_>>();
            if hrefs.len() != 1 {
                return fail("SMAP exact NSIDC HDF5 asset absent or ambiguous");
            }
            let asset = ProductAsset {
                href: hrefs[0].into(),
                etag: None,
                metadata: json!({"collection_concept":CONCEPT,"collection_version":"006","cmr_granule_id":e["id"]}),
            };
            out.push(Observation{id:id.into(),datetime:e["time_start"].as_str().ok_or("SMAP acquisition time absent")?.into(),collection:COLLECTION.into(),platform:Some("SMAP".into()),processing:Some("006/R19240".into()),assets:r.bands.iter().map(|b|(b.clone(),asset.clone())).collect(),properties:json!({"cmr":e,"pass":if pm(&r.product){"ascending PM"}else{"descending AM"}})});
        }
        out
    };
    let mut seen = std::collections::BTreeSet::new();
    for o in &observations {
        validate_observation(r, o)?;
        if !seen.insert(&o.id) {
            return fail("duplicate SMAP observation");
        }
    }
    Ok(
        json!({"schema":"tile-ripper-array-products-v1","product":r.product,"provider":if r.provider==Provider::Pinned{"pinned"}else{"earthdata"},"complete":true,"observations":observations,"seconds":started.elapsed().as_secs_f64(),"routing":{"collection_concept":CONCEPT,"version":"006","selection":"before preparation","authentication":"explicit exact-object NSIDC session required for live acquisition","fallback":"none"}}),
    )
}
pub(super) fn prepare(r: &ProductRequest, netrc: Option<&std::path::Path>) -> Result<Prepared> {
    let catalog = discover(r)?;
    let entries: Vec<Observation> = serde_json::from_value(catalog["observations"].clone())?;
    if entries.len() != 1 {
        return fail("SMAP acquisition requires exactly one selected daily granule");
    }
    let mut prepared = Vec::new();
    for o in entries {
        let source = &o.assets[&r.bands[0]];
        let transport = if catalog["provider"] == "earthdata" {
            #[cfg(feature = "authenticated-http")]
            {
                let n = netrc
                    .ok_or("live SMAP requires explicit --auth earthdata --netrc PRIVATE_FILE")?;
                crate::http_transport::Transport::Authenticated(Arc::new(
                    crate::authenticated_http::AuthenticatedHttp::earthdata_smap(&source.href, n)?,
                ))
            }
            #[cfg(not(feature = "authenticated-http"))]
            {
                return fail("live SMAP requires authenticated-http feature");
            }
        } else {
            if netrc.is_some() {
                return fail("SMAP mirror acquisition does not accept authentication");
            }
            Default::default()
        };
        let paths = r
            .bands
            .iter()
            .map(|b| format!("/{}/{}", group(&r.product), layer(&r.product, b)))
            .collect::<Vec<_>>();
        let mut request = Request {
            source: Source::Assets {
                assets: vec![Asset {
                    id: o.id.clone(),
                    locator: source.href.clone(),
                    format: Format::Hdf5,
                    datetime: Some(o.datetime.clone()),
                    etag: source.etag.clone(),
                }],
            },
            spatial: Spatial::ArraySlice {
                start: [0, 0],
                count: [1, 1],
                step: [1, 1],
                axes: axes(&r.product),
            },
            time: None,
            quality: vec![],
            operations: vec![],
            variables: paths
                .iter()
                .map(|p| Variable::Dataset { path: p.clone() })
                .collect(),
            grid: Grid::Native,
            sink: Sink::Callback,
            budget: Budget {
                buffer_bytes: r.buffer_bytes - RESERVATION,
                max_nodes: 65536,
                chunk_edge: r.chunk_edge,
            },
        };
        let t = Instant::now();
        let inspection = crate::hdf5::ArrayInspection::inspect(&request, transport)?;
        let inspection_seconds = t.elapsed().as_secs_f64();
        let first = &inspection.definitions[0];
        let meta = &first["product_metadata"];
        if meta["/Metadata/DatasetIdentification"]["fileName"]["value"] != format!("{}.h5", o.id)
            || chrono::DateTime::parse_from_rfc3339(
                meta["/Metadata/Extent"]["rangeBeginningDateTime"]["value"]
                    .as_str()
                    .ok_or("SMAP extent start missing")?,
            )? != chrono::DateTime::parse_from_rfc3339(&o.datetime)?
        {
            return fail("physical SMAP acquisition/granule identity conflicts with selection");
        }
        for (b, d) in r.bands.iter().zip(&inspection.definitions) {
            let a = &d["attributes"];
            let (dtype, fill) = if ["retrieval_qual_flag", "surface_flag"].contains(&b.as_str()) {
                ("<u2", 65534.)
            } else {
                ("<f4", -9999.)
            };
            if d["shape"]
                != if north(&r.product) {
                    json!([2000, 2000])
                } else {
                    json!([1624, 3856])
                }
                || d["stored_dtype"] != dtype
                || a["_FillValue"]["value"] != fill
                || a.get("scale_factor").is_some()
                || a.get("add_offset").is_some()
            {
                return fail("unqualified SMAP grid/type/fill/packing");
            }
            if b == "soil_moisture" && a["units"]["value"] != "cm**3/cm**3" {
                return fail("SMAP soil moisture units conflict");
            }
            if b=="retrieval_qual_flag"&&(a["flag_masks"]["value"]!="1s, 2s, 4s, 8s"||a["flag_meanings"]["value"]!="Retrieval_recommended Retrieval_attempted Retrieval_success FT_retrieval_success"){return fail("unqualified SMAP retrieval flag definitions");}
        }
        let grid = if north(&r.product) {
            NativeGrid::EaseGridNorth { epsg: 6931 }
        } else {
            NativeGrid::EaseGridGlobal { epsg: 6933 }
        };
        let t = Instant::now();
        let selection = array_grid::resolve(
            &grid,
            &inspection.coordinates[0],
            &inspection.coordinates[1],
            &r.aoi,
            axes(&r.product),
        )?;
        request.spatial = selection.native_selection.clone();
        let selection_seconds = t.elapsed().as_secs_f64();
        crate::planner::validate(&request)?;
        let Spatial::ArraySlice {
            start, count, step, ..
        } = &request.spatial
        else {
            unreachable!()
        };
        let mut coordinates = [Vec::new(), Vec::new()];
        for i in 0..2 {
            for n in 0..count[i] {
                coordinates[i].push(
                    *inspection.coordinates[i]
                        .get(start[i] + n * step[i])
                        .ok_or("SMAP selection outside native grid")?,
                );
            }
        }
        admit_retained(r, &catalog, &inspection.definitions, &coordinates)?;
        let t = Instant::now();
        let base = acquisition::prepare_array_inspected(&request, &inspection)?;
        let physical_plan_seconds = t.elapsed().as_secs_f64();
        prepared.push(PreparedObservation {
            observation: o,
            preparation_timings: PreparationTimings {
                inspection_seconds,
                selection_seconds,
                physical_plan_seconds,
            },
            native_selection: request.spatial,
            selection,
            grid,
            definitions: inspection.definitions.clone(),
            coordinates,
            base,
        });
    }
    Ok(Prepared {
        request: r.clone(),
        catalog,
        observations: prepared,
    })
}
