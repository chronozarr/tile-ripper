//! Qualified, source-bound rank-two scientific products over the native HDF5 reader.
//! Product navigation and policies do not change raw samples or their source masks.
use super::{
    array_grid::{self, Inclusion, NativeGrid},
    routing::Provider,
    Observation, ProductAsset, ProductRequest,
};
use crate::{
    acquisition, fail, Asset, Budget, Chunk, ExecutionReport, Format, Grid, Request, Result, Sink,
    Source, Spatial, Variable,
};
use serde_json::{json, Value};
use std::{
    io::Read,
    sync::{atomic::AtomicBool, Arc},
    time::Instant,
};
mod nisar;
mod smap;
const RESERVATION: usize = 8 * 1024 * 1024;
pub fn supports(id: &str) -> bool {
    matches!(
        id,
        "goes19-abi-cmipc-c13" | "goes19-abi-cmipc-c01" | "goes19-abi-cmipm1-c02"
    ) || smap::supports(id)
        || nisar::supports(id)
}
pub fn profiles() -> Value {
    json!([
     {"id":"goes19-abi-cmipc-c13","family":"GOES ABI CMIPC","spectral_band":13,"bands":["CMI","DQF"],"coordinate_model":"geostationary scanning angles","stored_dtype":["int16","int8"],"science":"brightness temperature; raw packed codes and CF unsigned annotations retained","provider":"noaa","qualification":"only inspected GOES-19 Mode 6 CONUS profile"},
     {"id":"goes19-abi-cmipc-c01","family":"GOES ABI CMIPC","spectral_band":1,"bands":["CMI","DQF"],"coordinate_model":"geostationary scanning angles","stored_dtype":["int16","int8"],"science":"reflectance factor; raw packed codes and CF unsigned annotations retained","provider":"noaa","qualification":"separate 1 km source specimen"},
     {"id":"goes19-abi-cmipm1-c02","family":"GOES ABI CMIPM1","spectral_band":2,"bands":["CMI","DQF"],"coordinate_model":"geostationary scanning angles","stored_dtype":["int16","int8"],"science":"reflectance factor; original raw codes and calibration","provider":"noaa","qualification":"independently inspected Mode 6 mesoscale-1 0.5km grid"},
     {"id":"smap-spl3smp-e-v6-am","family":"SMAP SPL3SMP_E v006/R19240","pass":"descending AM","bands":["soil_moisture","retrieval_qual_flag","surface_flag","freeze_thaw_fraction"],"coordinate_model":"EPSG:6933 EASE2 M09 native centers","provider":"earthdata","qualification":"global grid only; polar grids excluded"},
     {"id":"smap-spl3smp-e-v6-pm","family":"SMAP SPL3SMP_E v006/R19240","pass":"ascending PM","bands":["soil_moisture","retrieval_qual_flag","surface_flag","freeze_thaw_fraction"],"coordinate_model":"EPSG:6933 EASE2 M09 native centers","provider":"earthdata","qualification":"separate PM source fields; no averaging"},
     {"id":"smap-spl3smp-e-v6-north-am","family":"SMAP SPL3SMP_E v006/R19240","pass":"descending AM","bands":["soil_moisture","retrieval_qual_flag","surface_flag","freeze_thaw_fraction"],"coordinate_model":"EPSG:6931 EASE2 N09 northern native centers","provider":"earthdata","qualification":"distinct 2000x2000 polar source group"},
     {"id":"smap-spl3smp-e-v6-north-pm","family":"SMAP SPL3SMP_E v006/R19240","pass":"ascending PM","bands":["soil_moisture","retrieval_qual_flag","surface_flag","freeze_thaw_fraction"],"coordinate_model":"EPSG:6931 EASE2 N09 northern native centers","provider":"earthdata","qualification":"separate polar PM fields; no averaging"},
     {"id":"nisar-gcov-p1-fa-hhhh","family":"NISAR provisional GCOV v1","frequency":"A","bands":["HHHH"],"science":"already calibrated RTC gamma0 linear power","coordinate_model":"source Float64 UTM16N centers","selection":"WGS84 AOI or native indices","qualification":"inspected P05023/productVersion1.0.11; no other covariance terms"}
    ])
}
fn band_token(id: &str) -> Result<&'static str> {
    match id {
        "goes19-abi-cmipc-c13" => Ok("C13"),
        "goes19-abi-cmipc-c01" => Ok("C01"),
        "goes19-abi-cmipm1-c02" => Ok("C02"),
        _ => fail("unknown native scientific array product"),
    }
}
fn goes_scene(id: &str) -> (&'static str, &'static str, &'static str) {
    if id == "goes19-abi-cmipm1-c02" {
        ("ABI-L2-CMIPM1", "ABI-L2-CMIPM", "Mesoscale")
    } else {
        ("ABI-L2-CMIPC", "ABI-L2-CMIPC", "CONUS")
    }
}
fn validate(r: &ProductRequest) -> Result<()> {
    band_token(&r.product)?;
    let start = chrono::DateTime::parse_from_rfc3339(&r.start)?;
    let end = chrono::DateTime::parse_from_rfc3339(&r.end)?;
    if end < start
        || end - start > chrono::Duration::hours(1)
        || r.max_observations == 0
        || r.max_observations > 32
        || r.buffer_bytes < 16 * 1024 * 1024
        || r.buffer_bytes > 1024 * 1024 * 1024
        || r.chunk_edge == 0
        || r.chunk_edge > 256
    {
        return fail("array products require <=1 hour, 1..32 observations, 16..1024 MiB and chunk_edge 1..256");
    }
    if r.bands.is_empty()
        || r.bands.len() > 2
        || r.bands.iter().any(|b| !matches!(b.as_str(), "CMI" | "DQF"))
        || r.bands
            .iter()
            .enumerate()
            .any(|(i, b)| r.bands[..i].contains(b))
    {
        return fail("GOES bands must be distinct CMI and/or DQF");
    }
    if r.quality_policies.len() > 1
        || r.quality_policies.iter().any(|p| p != "goes_dqf_good")
        || (!r.quality_policies.is_empty() && !r.bands.iter().any(|b| b == "DQF"))
    {
        return fail("GOES policy goes_dqf_good requires raw DQF");
    }
    Inclusion::new(&r.aoi)?;
    if let Some(o) = &r.observations {
        if o.len() > r.max_observations || serde_json::to_vec(o)?.len() > 1024 * 1024 {
            return fail("explicit array observations exceed count/metadata limit");
        }
    }
    Ok(())
}
fn filename_time(name: &str) -> Result<String> {
    let token = name
        .split('_')
        .find_map(|v| v.strip_prefix('s'))
        .ok_or("missing NOAA acquisition token")?;
    if token.len() != 14 || !token.bytes().all(|v| v.is_ascii_digit()) {
        return fail("invalid NOAA decisecond acquisition token");
    }
    let date = chrono::NaiveDateTime::parse_from_str(&token[..13], "%Y%j%H%M%S")?;
    Ok(format!(
        "{}{}Z",
        date.format("%Y-%m-%dT%H:%M:%S."),
        &token[13..]
    ))
}
fn validate_observation(r: &ProductRequest, o: &Observation) -> Result<()> {
    let token = band_token(&r.product)?;
    if o.platform.as_deref().is_some_and(|p| p != "G19")
        || o.processing.as_deref().is_some_and(|p| p != "L2 M6")
    {
        return fail("GOES catalog platform/processing conflicts with profile");
    }
    if o.id.len() > 200
        || !o
            .id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
    {
        return fail("unsafe GOES observation identifier");
    }
    let filename = format!("{}.nc", o.id);
    if !filename.starts_with(&format!("OR_{}-M6{token}_G19_s", goes_scene(&r.product).0))
        || filename_time(&filename)? != o.datetime
        || o.collection != goes_scene(&r.product).1
    {
        return fail("GOES collection, scene/band or acquisition identity mismatch");
    }
    let time = chrono::DateTime::parse_from_rfc3339(&o.datetime)?;
    if time < chrono::DateTime::parse_from_rfc3339(&r.start)?
        || time > chrono::DateTime::parse_from_rfc3339(&r.end)?
    {
        return fail("explicit observation outside requested time interval");
    }
    let mut first = None;
    for b in &r.bands {
        let asset = o
            .assets
            .get(b)
            .ok_or("array observation missing selected layer")?;
        if first.is_some_and(|a: &ProductAsset| a.href != asset.href || a.etag != asset.etag) {
            return fail("array layers must refer to the same exact source version");
        }
        first = Some(asset);
        let url = reqwest::Url::parse(&asset.href);
        if let Ok(u) = &url {
            if u.query().is_some()
                || u.fragment().is_some()
                || !u.username().is_empty()
                || u.password().is_some()
            {
                return fail("array sources require canonical unsigned exact-object locators");
            }
        }
        let noaa = url.as_ref().is_ok_and(|u| {
            u.scheme() == "https"
                && u.host_str() == Some("noaa-goes19.s3.amazonaws.com")
                && u.port().is_none()
                && u.path().ends_with(&format!("/{filename}"))
        });
        match r.provider {
            Provider::Auto if noaa=>(),
            Provider::Pinned if r.observations.is_some()=>(),
            _=>return fail("GOES requires anonymous NOAA source or explicitly pinned reference; no provider substitution"),
        }
    }
    Ok(())
}
/// Catalog-only NOAA ListObjectsV2 search. It never prepares or fetches science.
pub fn discover(r: &ProductRequest) -> Result<Value> {
    if nisar::supports(&r.product) {
        return nisar::discover(r);
    }
    if smap::supports(&r.product) {
        return smap::discover(r);
    }
    validate(r)?;
    let started = Instant::now();
    let observations = if let Some(o) = &r.observations {
        o.clone()
    } else {
        if r.provider != Provider::Auto {
            return fail("GOES discovery requires automatic anonymous NOAA provider");
        }
        use chrono::{Datelike, Timelike};
        let start = chrono::DateTime::parse_from_rfc3339(&r.start)?.with_timezone(&chrono::Utc);
        let end = chrono::DateTime::parse_from_rfc3339(&r.end)?.with_timezone(&chrono::Utc);
        let mut hour = start
            .with_minute(0)
            .and_then(|d| d.with_second(0))
            .and_then(|d| d.with_nanosecond(0))
            .ok_or("invalid discovery hour")?;
        let client = reqwest::blocking::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(std::time::Duration::from_secs(30))
            .build()?;
        let mut out = Vec::new();
        while hour <= end {
            let prefix = format!(
                "{}/{}/{:03}/{:02}/OR_{}-M6{}",
                goes_scene(&r.product).1,
                hour.year(),
                hour.ordinal(),
                hour.hour(),
                goes_scene(&r.product).0,
                band_token(&r.product)?
            );
            let response = client
                .get("https://noaa-goes19.s3.amazonaws.com/")
                .query(&[
                    ("list-type", "2"),
                    ("prefix", &prefix),
                    ("max-keys", "1000"),
                ])
                .send()?;
            if response.status() != 200 {
                return fail("NOAA bounded catalog HTTP failure");
            }
            let mut bytes = Vec::new();
            response.take(1024 * 1024 + 1).read_to_end(&mut bytes)?;
            if bytes.len() > 1024 * 1024 {
                return fail("NOAA listing exceeds one MiB");
            }
            let xml = std::str::from_utf8(&bytes)?;
            let doc = roxmltree::Document::parse(xml)?;
            if doc
                .descendants()
                .any(|n| n.has_tag_name("IsTruncated") && n.text() == Some("true"))
            {
                return fail("NOAA discovery listing truncated; narrow interval");
            }
            for node in doc.descendants().filter(|n| n.has_tag_name("Key")) {
                let key = node.text().ok_or("missing NOAA object key")?;
                if !key.starts_with(&prefix)
                    || !key.contains(&format!("M6{}_G19_", band_token(&r.product)?))
                {
                    continue;
                }
                let name = key.rsplit('/').next().ok_or("missing NOAA filename")?;
                let datetime = filename_time(name)?;
                let dt = chrono::DateTime::parse_from_rfc3339(&datetime)?;
                if dt < start || dt > end {
                    continue;
                }
                let a = ProductAsset {
                    href: format!("https://noaa-goes19.s3.amazonaws.com/{key}"),
                    etag: None,
                    metadata: json!({"physical_format":"NetCDF-4/HDF5","catalog":"NOAA ListObjectsV2","version":"validated at physical preparation"}),
                };
                out.push(Observation {
                    id: name.trim_end_matches(".nc").into(),
                    datetime,
                    collection: goes_scene(&r.product).1.into(),
                    platform: Some("G19".into()),
                    processing: Some("L2 M6".into()),
                    assets: r.bands.iter().map(|b| (b.clone(), a.clone())).collect(),
                    properties: json!({"spectral_band":band_token(&r.product)?,"object_key":key}),
                });
                if out.len() > r.max_observations {
                    return fail(
                        "NOAA observation cap exceeded; narrow interval (no silent truncation)",
                    );
                }
            }
            hour += chrono::Duration::hours(1);
        }
        out
    };
    let mut seen = std::collections::BTreeSet::new();
    for o in &observations {
        validate_observation(r, o)?;
        if !seen.insert(&o.id) {
            return fail("duplicate array observation identity");
        }
    }
    Ok(
        json!({"schema":"tile-ripper-array-products-v1","product":r.product,"provider":if r.provider==Provider::Pinned{"pinned"}else{"noaa"},"complete":true,"observations":observations,"seconds":started.elapsed().as_secs_f64(),"routing":{"selection":"before physical preparation","fallback":"none","prepared_source":"exact locator and source version; never substituted"}}),
    )
}
fn admit_retained(
    r: &ProductRequest,
    catalog: &Value,
    definitions: &[Value],
    coordinates: &[Vec<f64>; 2],
) -> Result<()> {
    let metadata = serde_json::to_vec(r)?
        .len()
        .checked_add(serde_json::to_vec(catalog)?.len())
        .and_then(|n| n.checked_add(serde_json::to_vec(definitions).ok()?.len()))
        .and_then(|n| n.checked_mul(4))
        .and_then(|n| n.checked_add(coordinates.iter().map(|v| v.len() * 32).sum::<usize>()))
        .ok_or("array retained metadata reservation overflow")?;
    if metadata > RESERVATION / 2 {
        return fail("array product metadata/coordinate retention exceeds reserved half; narrow supplied metadata or native selection");
    }
    Ok(())
}
#[derive(Clone, Debug, serde::Serialize)]
pub struct PreparationTimings {
    pub inspection_seconds: f64,
    pub selection_seconds: f64,
    pub physical_plan_seconds: f64,
}
/// Immutable prepared observation over its actual native grid.
#[derive(Clone)]
pub struct PreparedObservation {
    pub observation: Observation,
    pub preparation_timings: PreparationTimings,
    pub native_selection: Spatial,
    pub selection: array_grid::ResolvedSelection,
    pub grid: NativeGrid,
    pub definitions: Vec<Value>,
    pub coordinates: [Vec<f64>; 2],
    pub(crate) base: acquisition::Prepared,
}
#[derive(Clone)]
pub struct Prepared {
    pub(crate) request: ProductRequest,
    pub(crate) catalog: Value,
    pub(crate) observations: Vec<PreparedObservation>,
}
impl Prepared {
    pub fn explain(&self) -> Value {
        json!({"schema":"tile-ripper-array-products-v1","product":self.request.product,"catalog":self.catalog,"observations":self.observations.iter().map(|o|json!({"id":o.observation.id,"grid":o.grid.describe(),"native_selection":o.native_selection,"selection":o.selection,"preparation_timings":o.preparation_timings,"physical":o.base.explain()})).collect::<Vec<_>>(),"working_memory":{"requested_bytes":self.request.buffer_bytes,"consumer_and_output_reservation_bytes":RESERVATION,"scope":"reserved in addition to native reader; caller retention and process runtime outside budget"},"quality_policies":self.request.quality_policies,"numeric_semantics":"raw native values and dtype; no unsigned reinterpretation, scale/offset application, reprojection or resampling"})
    }
    pub fn human(&self) -> String {
        let mut out=format!("Tile-Ripper native scientific product: {}\nProvider: {}\nWorking-memory budget: {} bytes (includes {} reserved for product/output work)\n",self.request.product,self.catalog["provider"].as_str().unwrap_or("unknown"),self.request.buffer_bytes,RESERVATION);
        for o in &self.observations {
            out+=&format!("\nObservation: {}\nNative coordinate model: {}\nSelection: {}\nSource identity: {}\nQuality policies: {}\n",o.observation.id,serde_json::to_string(&o.grid).unwrap_or_default(),serde_json::to_string(&o.native_selection).unwrap_or_default(),o.base.explain()["sources"][0]["object_identities"]["identity"],self.request.quality_policies.join(", "));
            out += &format!("Actual native grid/CRS: {}\n", o.grid.describe());
            let physical = o.base.explain();
            for d in physical["sources"][0]["object_identities"]["datasets"]
                .as_array()
                .into_iter()
                .flatten()
            {
                let chunks = d["chunks"].as_array().map_or(0, |chunks| {
                    chunks
                        .iter()
                        .filter(|c| !c["byte_offset"].is_null())
                        .count()
                });
                let scalar_bytes = match d["stored_dtype"].as_str() {
                    Some("|i1" | "|u1") => 1,
                    Some("<i2" | "<u2") => 2,
                    Some("<f4" | "<u4") => 4,
                    _ => 0,
                };
                let decoded = d["chunk_shape"].as_array().map_or(0, |shape| {
                    shape.iter().filter_map(Value::as_u64).product::<u64>()
                }) * scalar_bytes
                    * chunks as u64;
                out += &format!("Source shape {} / {}: required allocated physical chunks {}; decoded source-chunk byte estimate {}\n",d["shape"],d["path"],chunks,decoded);
            }
            out += &format!("Requested AOI: {}\nIncluded native centers: {}\nMembership: {}\nPhysical plan and resource admission: {}\n", serde_json::to_string(&o.selection.requested_aoi).unwrap_or_default(), o.selection.included_centers, o.selection.membership, o.base.human());
            out += &format!(
                "Unsupported operations: {}\n",
                o.grid.describe()["unsupported"]
            );
        }
        out
    }
    pub fn observations(&self) -> &[PreparedObservation] {
        &self.observations
    }
    pub fn fetch(&self, path: &str, cancelled: Arc<AtomicBool>) -> Result<Value> {
        crate::scientific_output::arrays::fetch(self, path, cancelled)
    }
    pub fn execute(
        &self,
        cancelled: &AtomicBool,
        mut consume: impl FnMut(&PreparedObservation, Chunk) -> Result<()>,
    ) -> Result<Vec<ExecutionReport>> {
        let mut reports = Vec::new();
        for o in &self.observations {
            reports.push(o.base.execute_cancellable(cancelled, |c| consume(o, c))?);
        }
        Ok(reports)
    }
}
pub fn prepare(r: &ProductRequest) -> Result<Prepared> {
    if nisar::supports(&r.product) {
        return nisar::prepare(r, None);
    }
    if smap::supports(&r.product) {
        return smap::prepare(r, None);
    }
    let catalog = discover(r)?;
    let observations: Vec<Observation> = serde_json::from_value(catalog["observations"].clone())?;
    let mut prepared = Vec::new();
    if observations.len() > 1 {
        return fail("array acquisition admits one observation per job; discover candidates then explicitly select one");
    }
    for o in observations {
        let source = o.assets.get(&r.bands[0]).ok_or("missing array source")?;
        let axes = ["/y".into(), "/x".into()];
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
                axes: axes.clone(),
            },
            time: None,
            quality: vec![],
            operations: vec![],
            variables: r
                .bands
                .iter()
                .map(|b| Variable::Dataset {
                    path: format!("/{b}"),
                })
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
        let inspection = crate::hdf5::ArrayInspection::inspect(&request, Default::default())?;
        let inspection_seconds = t.elapsed().as_secs_f64();
        let first = inspection
            .definitions
            .first()
            .ok_or("missing native product definition")?;
        let root = &first["root_attributes"];
        if root["dataset_name"]["value"] != format!("{}.nc", o.id)
            || root["platform_ID"]["value"] != "G19"
            || root["scene_id"]["value"] != goes_scene(&r.product).2
            || root["title"]["value"] != "ABI L2 Cloud and Moisture Imagery"
            || root["time_coverage_start"]["value"] != o.datetime
        {
            return fail("physical GOES product/time metadata conflicts with selected observation");
        }
        for axis in first["axes"].as_array().ok_or("missing GOES axes")? {
            if axis["stored_dtype"] != "<i2" {
                return fail("GOES source axes must retain original Int16 packing");
            }
        }
        let spectral = band_token(&r.product)?[1..].parse::<u32>()?;
        if first["ancillary"]["/band_id"]["value"] != spectral {
            return fail("physical ABI band identity conflicts with profile");
        }
        let grid = NativeGrid::goes(first)?;
        for d in &inspection.definitions {
            if d["grid_mapping"] != first["grid_mapping"] || d["axes"] != first["axes"] {
                return fail("GOES layers have different coordinate models");
            }
            let a = &d["attributes"];
            match d["path"].as_str(){
                Some("/CMI")=>{let band=band_token(&r.product)?;let unit=if band=="C13"{"K"}else{"1"};if d["stored_dtype"]!="<i2" || a["units"]["value"]!=unit || a["_Unsigned"]["value"]!="true" || a["_FillValue"]["value"]!= -1. || a["scale_factor"]["value"].as_f64().is_none() || a["add_offset"]["value"].as_f64().is_none(){return fail("unqualified GOES CMI dtype/calibration/fill");}},
                Some("/DQF")=>{if d["stored_dtype"]!="|i1" || a["_FillValue"]["value"]!= -1. || a["flag_meanings"]["value"]!="good_pixel_qf conditionally_usable_pixel_qf out_of_range_pixel_qf no_value_pixel_qf focal_plane_temperature_threshold_exceeded_qf"{return fail("unqualified GOES DQF definitions");}},
                _=>return fail("unqualified GOES variable"),
            }
        }
        let selection_started = Instant::now();
        let selection = array_grid::resolve(
            &grid,
            &inspection.coordinates[0],
            &inspection.coordinates[1],
            &r.aoi,
            axes,
        )?;
        request.spatial = selection.native_selection.clone();
        let selection_seconds = selection_started.elapsed().as_secs_f64();
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
                        .ok_or("native selection outside source axes")?,
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
/// Closed policy on raw DQF only. Source fill and geographic inclusion are separate.
pub fn quality_accepts(policy: &str, value: f64) -> Result<bool> {
    match policy {
        "goes_dqf_good" => Ok(value == 0.),
        "smap_retrieval_recommended" => Ok(value == 0. || value == 8.),
        _ => fail("unknown array quality policy"),
    }
}

#[cfg(feature = "authenticated-http")]
pub fn prepare_earthdata(r: &ProductRequest, netrc: &std::path::Path) -> Result<Prepared> {
    if smap::supports(&r.product) {
        smap::prepare(r, Some(netrc))
    } else if nisar::supports(&r.product) {
        nisar::prepare(r, Some(netrc))
    } else {
        fail("this array product has no qualified Earthdata route")
    }
}
