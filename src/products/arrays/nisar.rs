//! Real provisional NISAR GCOV frequency-A HHHH, source-defined gamma0 linear power.
use super::*;
const PRODUCT: &str = "nisar-gcov-p1-fa-hhhh";
const COLLECTION: &str = "NISAR_L2_GCOV_PROVISIONAL_V1";
const CONCEPT: &str = "C2854338529-ASF";
const ARCHIVE: &str = "nisar.asf.earthdatacloud.nasa.gov";
const ROOT: &str = "/science/LSAR/GCOV/grids/frequencyA";
pub(super) fn supports(id: &str) -> bool {
    id == PRODUCT
}
fn axes() -> [String; 2] {
    [
        format!("{ROOT}/yCoordinates"),
        format!("{ROOT}/xCoordinates"),
    ]
}
fn validate(r: &ProductRequest) -> Result<()> {
    let start = chrono::DateTime::parse_from_rfc3339(&r.start)?;
    let end = chrono::DateTime::parse_from_rfc3339(&r.end)?;
    if !supports(&r.product)
        || end < start
        || end - start > chrono::Duration::days(31)
        || !(1..=32).contains(&r.max_observations)
        || r.buffer_bytes < 64 * 1024 * 1024
        || r.buffer_bytes > 1024 * 1024 * 1024
        || r.chunk_edge == 0
        || r.chunk_edge > 256
        || r.bands != ["HHHH"]
        || !r.quality_policies.is_empty()
    {
        return fail("NISAR profile requires HHHH only, raw quality, <=31 days, 1..32 observations, 64..1024 MiB and chunk_edge 1..256");
    }
    Inclusion::for_grid(&r.aoi, &NativeGrid::NisarUtm16North)?;
    if let Some(o) = &r.observations {
        if o.len() > r.max_observations || serde_json::to_vec(o)?.len() > 1024 * 1024 {
            return fail("NISAR observation metadata/count cap exceeded");
        }
    }
    Ok(())
}
fn validate_observation(r: &ProductRequest, o: &Observation) -> Result<()> {
    if o.platform.as_deref().is_some_and(|p| p != "NISAR")
        || o.processing.as_deref().is_some_and(|p| p != "P05023")
    {
        return fail("NISAR catalog platform/processing conflicts with profile");
    }
    if o.id.len() != 91
        || !o.id.starts_with("NISAR_L2_PR_GCOV_")
        || !o.id.ends_with("_P05023_N_F_J_001")
        || !o
            .id
            .bytes()
            .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'_')
        || o.collection != COLLECTION
    {
        return fail("unqualified NISAR provisional collection/CRID identity");
    }
    let dt = chrono::DateTime::parse_from_rfc3339(&o.datetime)?;
    if dt < chrono::DateTime::parse_from_rfc3339(&r.start)?
        || dt > chrono::DateTime::parse_from_rfc3339(&r.end)?
    {
        return fail("NISAR observation outside dates");
    }
    let a = o.assets.get("HHHH").ok_or("NISAR HHHH source missing")?;
    let u = reqwest::Url::parse(&a.href);
    let protected = u.as_ref().is_ok_and(|u| u.host_str() == Some(ARCHIVE));
    if let Ok(u) = &u {
        if u.query().is_some()
            || u.fragment().is_some()
            || !u.username().is_empty()
            || u.password().is_some()
        {
            return fail("NISAR requires canonical unsigned exact-object locator");
        }
    }
    match r.provider {
        Provider::Auto | Provider::Earthdata if protected => {
            let u = u?;
            let path = format!("/NISAR/{COLLECTION}/{0}/{0}.h5", o.id);
            if u.scheme() != "https" || u.port().is_some() || u.path() != path {
                return fail("NISAR exact archive object conflict");
            }
        }
        Provider::Pinned if r.observations.is_some() && !protected => (),
        _ => return fail("NISAR provider conflicts with supplied source; no fallback"),
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
            return fail("NISAR discovery requires explicit ASF CMR collection");
        }
        let c = reqwest::blocking::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(std::time::Duration::from_secs(45))
            .build()?;
        let mut q = vec![
            ("collection_concept_id".to_string(), CONCEPT.into()),
            ("temporal".into(), format!("{},{}", r.start, r.end)),
            ("page_size".into(), r.max_observations.to_string()),
            (
                "producer_granule_id".into(),
                "NISAR_L2_PR_GCOV_*_P05023_N_F_J_001".into(),
            ),
            (
                "options[producer_granule_id][pattern]".into(),
                "true".into(),
            ),
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
            return fail("NISAR CMR HTTP failure");
        }
        let hits = response
            .headers()
            .get("CMR-Hits")
            .ok_or("NISAR CMR hit count missing")?
            .to_str()?
            .parse::<usize>()?;
        if hits > r.max_observations {
            return fail("NISAR discovery exceeds observation cap; narrow dates/AOI");
        }
        let mut b = Vec::new();
        response.take(4 * 1024 * 1024 + 1).read_to_end(&mut b)?;
        if b.len() > 4 * 1024 * 1024 {
            return fail("NISAR CMR response cap exceeded");
        }
        let v: Value = serde_json::from_slice(&b)?;
        let e = v["feed"]["entry"]
            .as_array()
            .ok_or("NISAR CMR entries missing")?;
        if e.len() != hits {
            return fail("NISAR catalog incomplete");
        }
        let mut out = Vec::new();
        for item in e {
            let id = item["producer_granule_id"]
                .as_str()
                .ok_or("NISAR CMR identity missing")?
                .trim_end_matches(".h5");
            let href = format!("https://{ARCHIVE}/NISAR/{COLLECTION}/{id}/{id}.h5");
            if !item["links"]
                .as_array()
                .is_some_and(|a| a.iter().any(|l| l["href"] == href))
            {
                return fail(
                    "NISAR exact HDF5 source absent from catalog; no sidecar substitution",
                );
            }
            let a = ProductAsset {
                href,
                etag: None,
                metadata: json!({"cmr_granule_id":item["id"],"collection_concept":CONCEPT,"version":"1"}),
            };
            out.push(Observation {
                id: id.into(),
                datetime: item["time_start"]
                    .as_str()
                    .ok_or("NISAR acquisition time missing")?
                    .into(),
                collection: COLLECTION.into(),
                platform: Some("NISAR".into()),
                processing: Some("P05023".into()),
                assets: std::collections::BTreeMap::from([("HHHH".into(), a)]),
                properties: item.clone(),
            });
        }
        out
    };
    let mut seen = std::collections::BTreeSet::new();
    for o in &observations {
        validate_observation(r, o)?;
        if !seen.insert(&o.id) {
            return fail("duplicate NISAR observation");
        }
    }
    Ok(
        json!({"schema":"tile-ripper-array-products-v1","product":PRODUCT,"provider":if r.provider==Provider::Pinned{"pinned"}else{"earthdata"},"complete":true,"observations":observations,"seconds":started.elapsed().as_secs_f64(),"routing":{"collection":COLLECTION,"concept":CONCEPT,"selection":"before preparation","physical_source":"original HDF5 only; derived COGs are not substitutes","fallback":"none"}}),
    )
}
pub(super) fn prepare(r: &ProductRequest, netrc: Option<&std::path::Path>) -> Result<Prepared> {
    let catalog = discover(r)?;
    let entries: Vec<Observation> = serde_json::from_value(catalog["observations"].clone())?;
    if entries.len() != 1 {
        return fail("NISAR acquisition requires exactly one selected observation");
    }
    let mut prepared = Vec::new();
    for o in entries {
        let source = &o.assets["HHHH"];
        let transport = if catalog["provider"] == "earthdata" {
            #[cfg(feature = "authenticated-http")]
            {
                let n = netrc
                    .ok_or("live NISAR requires explicit --auth earthdata --netrc PRIVATE_FILE")?;
                crate::http_transport::Transport::Authenticated(Arc::new(
                    crate::authenticated_http::AuthenticatedHttp::earthdata_nisar(&source.href, n)?,
                ))
            }
            #[cfg(not(feature = "authenticated-http"))]
            {
                return fail("live NISAR requires authenticated-http feature");
            }
        } else {
            if netrc.is_some() {
                return fail("NISAR pinned mirror cannot inherit credentials");
            }
            Default::default()
        };
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
                axes: axes(),
            },
            time: None,
            variables: vec![Variable::Dataset {
                path: format!("{ROOT}/HHHH"),
            }],
            quality: vec![],
            operations: vec![],
            grid: Grid::Native,
            sink: Sink::Callback,
            budget: Budget {
                buffer_bytes: r.buffer_bytes - RESERVATION,
                max_nodes: 65536,
                chunk_edge: r.chunk_edge,
            },
        };
        let t = Instant::now();
        let inspected = crate::hdf5::ArrayInspection::inspect(&request, transport)?;
        let inspection_seconds = t.elapsed().as_secs_f64();
        let d = &inspected.definitions[0];
        let m = &d["product_metadata"];
        let id = "/science/LSAR/identification";
        let p = "/science/LSAR/GCOV/metadata/processingInformation/parameters";
        for (path, expected) in [
            (format!("{id}/granuleId"), o.id.as_str()),
            (format!("{id}/productVersion"), "1.0.11"),
            (format!("{id}/productSpecificationVersion"), "1.5.0"),
            (format!("{id}/compositeReleaseId"), "P05023"),
            (format!("{id}/productType"), "GCOV"),
            (format!("{id}/radarBand"), "L"),
            (format!("{p}/radiometricTerrainCorrectionApplied"), "True"),
            (
                format!("{p}/rtc/outputBackscatterNormalizationConvention"),
                "gamma0",
            ),
            (
                format!("{p}/rtc/outputBackscatterExpressionConvention"),
                "Linear power",
            ),
        ] {
            if m[&path]["value"] != expected {
                return fail(
                    "NISAR source identity/radiometric definition outside inspected profile",
                );
            }
        }
        let source_start = m[format!("{id}/zeroDopplerStartTime")]["value"]
            .as_str()
            .ok_or("missing NISAR acquisition start")?;
        if chrono::DateTime::parse_from_rfc3339(&format!("{source_start}Z"))?
            != chrono::DateTime::parse_from_rfc3339(&o.datetime)?
        {
            return fail("NISAR native acquisition start conflicts with catalog");
        }
        if d["stored_dtype"] != "<f4"
            || d["attributes"]["units"]["value"] != "1"
            || d["attributes"]["_FillValue"]["nonfinite"] != "NaN"
            || d["storage_fill"] != 0.
        {
            return fail("NISAR exact Float32/CF NaN/default storage zero contract mismatch");
        }
        if m[format!("{ROOT}/projection")]["value"] != 32616
            || m[format!("{ROOT}/listOfCovarianceTerms")]["value"] != json!(["HHHH", "HVHV"])
            || m[format!("{ROOT}/listOfPolarizations")]["value"] != json!(["HH", "HV"])
        {
            return fail(
                "NISAR actual projection/frequency/polarization declarations conflict with profile",
            );
        }
        let mapping = &d["grid_mapping"]["attributes"];
        if mapping["spatial_ref"]["value"] != crate::swot_aoi::canonical_wkt(16, false)
            || mapping["epsg_code"]["value"] != 32616.
        {
            return fail("NISAR requires inspected WGS84 UTM16N grid");
        }
        for (i, a) in d["axes"]
            .as_array()
            .ok_or("missing NISAR axes")?
            .iter()
            .enumerate()
        {
            if a["stored_dtype"] != "<f8"
                || a["attributes"]["units"]["value"] != "meters"
                || a["attributes"]["pixel_coordinate_convention"]["value"] != "center"
                || inspected.coordinates[i].iter().any(|v| !v.is_finite())
                || inspected.coordinates[i].windows(2).any(|w| {
                    w[1] - w[0]
                        != (if i == 0 { -1. } else { 1. })
                            * if d["shape"] == json!([34776, 35352]) {
                                10.
                            } else {
                                20.
                            }
                })
            {
                return fail("NISAR requires inspected source Float64 10m/20m pixel-center axes");
            }
        }
        let t = Instant::now();
        let selection = array_grid::resolve(
            &NativeGrid::NisarUtm16North,
            &inspected.coordinates[0],
            &inspected.coordinates[1],
            &r.aoi,
            axes(),
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
                    *inspected.coordinates[i]
                        .get(start[i] + n * step[i])
                        .ok_or("NISAR native selection outside source")?,
                );
            }
        }
        admit_retained(r, &catalog, &inspected.definitions, &coordinates)?;
        let t = Instant::now();
        let base = acquisition::prepare_array_inspected(&request, &inspected)?;
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
            grid: NativeGrid::NisarUtm16North,
            definitions: inspected.definitions.clone(),
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
