//! Rank-two product output reuses the durable Zarr store and atomic transaction.
use super::*;
use crate::{
    products::{
        array_grid::Inclusion,
        arrays::{self, Prepared},
    },
    Pixels, Spatial, Variable,
};
fn attrs(stored: &Value, dimensions: &[&str]) -> Value {
    let mut out = json!({"_ARRAY_DIMENSIONS":dimensions,"source_attributes":stored});
    for (name, a) in stored.as_object().into_iter().flatten() {
        if let Some(v) = a.get("value").or_else(|| a.get("nonfinite")) {
            out[name] = v.clone();
        }
    }
    out["_ARRAY_DIMENSIONS"] = json!(dimensions);
    out
}
pub(crate) fn fetch(p: &Prepared, path: &str, cancelled: Arc<AtomicBool>) -> Result<Value> {
    check_cancelled(&cancelled)?;
    let started = Instant::now();
    let edge = p.request.chunk_edge;
    let transaction = Transaction::new(Path::new(path))?;
    let mut options = FilesystemStoreOptions::default();
    options.defer_device_flush(true);
    let metrics = Arc::new(Metrics::new(Arc::new(FilesystemStore::new_with_options(
        &transaction.stage,
        options,
    )?)));
    let store = Arc::new(Store::new(metrics.clone()));
    let encoding = ScientificEncoding::Uncompressed;
    group(
        store.clone(),
        "/",
        json!({"status":"incomplete","profile":p.request.product,"Conventions":"CF-1.8"}),
        encoding,
    )?;
    group(
        store.clone(),
        "/observations",
        json!({"layout":"separate native observations"}),
        encoding,
    )?;
    let mut reports = Vec::new();
    let mut objects = 5usize;
    let mut encode_seconds = 0.;
    for o in &p.observations {
        let inclusion = Inclusion::for_grid(&p.request.aoi, &o.grid)?;
        check_cancelled(&cancelled)?;
        let Spatial::ArraySlice {
            start, count, step, ..
        } = &o.native_selection
        else {
            unreachable!()
        };
        let prefix = format!("/observations/{}", o.observation.id);
        let definitions = &o.definitions;
        let first = &definitions[0];
        let tiles = count[0]
            .div_ceil(edge)
            .checked_mul(count[1].div_ceil(edge))
            .ok_or("output object count overflow")?;
        objects = objects
            .checked_add(
                (definitions.len() * 2 + 5 + p.request.quality_policies.len()) * (tiles + 2)
                    + 20
                    + (count[0] + count[1]).div_ceil(edge) * 2,
            )
            .ok_or("output object count overflow")?;
        if objects > 65536 {
            return Err(
                "array output exceeds 65536 object budget; narrow selection or raise chunk_edge"
                    .into(),
            );
        }
        let mut a = attrs(&first["root_attributes"], &[]);
        a["source"] = o.base.explain();
        a["native_grid"] = o.grid.describe();
        a["native_selection"] = json!(o.native_selection);
        a["geographic_selection"] = json!(o.selection);
        a["catalog_observation"] = json!(o.observation);
        a["product_metadata"] = first["product_metadata"].clone();
        group(store.clone(), &prefix, a, encoding)?;
        let chunks = [count[0].min(edge), count[1].min(edge)];
        let opt = serial().with_store_empty_chunks(true);
        let mut data = Vec::new();
        let mut validity = Vec::new();
        for d in definitions {
            let name = d["path"]
                .as_str()
                .ok_or("missing array path")?
                .rsplit('/')
                .next()
                .ok_or("missing array name")?;
            let mut a = attrs(&d["attributes"], &["y", "x"]);
            a["grid_mapping"] = json!("spatial_ref");
            a["coordinates"] = json!(if first["time"].is_object() {
                "t y x latitude longitude"
            } else {
                "y x latitude longitude"
            });
            a["numeric_semantics"]=json!("exact raw stored values; CF unsigned and packing annotations retained without application");
            a["ancillary_variables"] = json!(format!("source_valid_{name} aoi_mask earth_visible"));
            let dtype = d["stored_dtype"].as_str().ok_or("missing array dtype")?;
            let fill = match (&d["storage_fill"], dtype) {
                (Value::Null, _) => json!(0),
                (Value::String(s), _) if s == "NaN" => json!("NaN"),
                (v, "|i1" | "<i2") => {
                    json!(v.as_f64().ok_or("invalid signed storage fill")? as i64)
                }
                (v, "|u1" | "<u2" | "<u4") => {
                    json!(v.as_f64().ok_or("invalid unsigned storage fill")? as u64)
                }
                (v, _) => v.clone(),
            };
            data.push(array(
                store.clone(),
                &format!("{prefix}/{name}"),
                count,
                &chunks,
                dtype,
                fill,
                a,
                encoding,
            )?);
            validity.push(array(store.clone(),&format!("{prefix}/source_valid_{name}"),count,&chunks,"|u1",Value::Null,json!({"_ARRAY_DIMENSIONS":["y","x"],"mask_role":"CF source missingness only; independent of AOI and quality","flag_values":[0,1]}),encoding)?);
        }
        let quality = if let Some(policy) = p.request.quality_policies.first() {
            Some(array(
                store.clone(),
                &format!("{prefix}/quality_{policy}"),
                count,
                &chunks,
                "|u1",
                Value::Null,
                json!({"_ARRAY_DIMENSIONS":["y","x"],"mask_role":"derived raw quality policy only; independent of science validity and AOI","policy":policy,"definition":if policy=="goes_dqf_good"{"DQF == 0 (good_pixel_qf)"}else{"raw retrieval_qual_flag == 0 or 8 (NSIDC v6 recommended retrievals); freeze/thaw bit 3 remains distinct"},"source_variable":if policy=="goes_dqf_good"{"DQF"}else{"retrieval_qual_flag; source PM suffix retained"}}),
                encoding,
            )?)
        } else {
            None
        };
        let aoi = array(
            store.clone(),
            &format!("{prefix}/aoi_mask"),
            count,
            &chunks,
            "|u1",
            Value::Null,
            json!({"_ARRAY_DIMENSIONS":["y","x"],"mask_role":"geographic native-center inclusion; explicit native selection includes every selected index, including space","aoi":p.request.aoi}),
            encoding,
        )?;
        let visible = array(
            store.clone(),
            &format!("{prefix}/earth_visible"),
            count,
            &chunks,
            "|u1",
            Value::Null,
            json!({"_ARRAY_DIMENSIONS":["y","x"],"mask_role":"native center ray intersects source ellipsoid"}),
            encoding,
        )?;
        let lon = array(
            store.clone(),
            &format!("{prefix}/longitude"),
            count,
            &chunks,
            "<f8",
            json!("NaN"),
            json!({"_ARRAY_DIMENSIONS":["y","x"],"standard_name":"longitude","units":"degrees_east","derivation":"native navigation; space is NaN"}),
            encoding,
        )?;
        let lat = array(
            store.clone(),
            &format!("{prefix}/latitude"),
            count,
            &chunks,
            "<f8",
            json!("NaN"),
            json!({"_ARRAY_DIMENSIONS":["y","x"],"standard_name":"latitude","units":"degrees_north","derivation":"native navigation; space is NaN"}),
            encoding,
        )?;
        for row in (0..count[0]).step_by(edge) {
            for col in (0..count[1]).step_by(edge) {
                check_cancelled(&cancelled)?;
                let h = (count[0] - row).min(edge);
                let w = (count[1] - col).min(edge);
                let subset = ArraySubset::new_with_start_shape(
                    vec![row as u64, col as u64],
                    vec![h as u64, w as u64],
                )?;
                let mut longs = Vec::with_capacity(h * w);
                let mut lats = Vec::with_capacity(h * w);
                let mut masks = Vec::with_capacity(h * w);
                let mut earth = Vec::with_capacity(h * w);
                for y in row..row + h {
                    for x in col..col + w {
                        let xy = o
                            .grid
                            .longitude_latitude(o.coordinates[0][y], o.coordinates[1][x]);
                        longs.push(xy.map_or(f64::NAN, |v| v[0]));
                        lats.push(xy.map_or(f64::NAN, |v| v[1]));
                        earth.push(u8::from(xy.is_some()));
                        masks.push(u8::from(inclusion.native_accepts(
                            &o.grid,
                            o.coordinates[0][y],
                            o.coordinates[1][x],
                        )?));
                    }
                }
                for (array, values) in [(&aoi, &masks), (&visible, &earth)] {
                    encode_call(&store, &mut encode_seconds, || {
                        array.store_array_subset_opt(&subset, values, &opt)
                    })?;
                }
                for (array, values) in [(&lon, &longs), (&lat, &lats)] {
                    encode_call(&store, &mut encode_seconds, || {
                        array.store_array_subset_opt(&subset, values, &opt)
                    })?;
                }
            }
        }
        for (i, name) in ["y", "x"].iter().enumerate() {
            let n = count[i];
            let a = &first["axes"][i]["attributes"];
            // GOES axes are exact source Int16 codes. Their original CF packing
            // and radian units reproduce scan angles when explicitly decoded.
            let axis = array(
                store.clone(),
                &format!("{prefix}/{name}"),
                &[n],
                &[n.min(edge)],
                if matches!(
                    o.grid,
                    crate::products::array_grid::NativeGrid::Geostationary { .. }
                ) {
                    "<i2"
                } else {
                    "<f8"
                },
                Value::Null,
                attrs(a, &[name]),
                encoding,
            )?;
            let index = array(
                store.clone(),
                &format!("{prefix}/native_{}", if i == 0 { "row" } else { "column" }),
                &[n],
                &[n.min(edge)],
                "<i8",
                Value::Null,
                json!({"_ARRAY_DIMENSIONS":[name],"semantics":"zero-based source-native index"}),
                encoding,
            )?;
            for at in (0..n).step_by(edge) {
                let len = (n - at).min(edge);
                let subset = ArraySubset::new_with_start_shape(vec![at as u64], vec![len as u64])?;
                if matches!(
                    o.grid,
                    crate::products::array_grid::NativeGrid::Geostationary { .. }
                ) {
                    let values = o.coordinates[i][at..at + len]
                        .iter()
                        .map(|v| *v as i16)
                        .collect::<Vec<_>>();
                    axis.store_array_subset_opt(&subset, &values, &opt)?;
                } else {
                    axis.store_array_subset_opt(&subset, &o.coordinates[i][at..at + len], &opt)?;
                }
                let values = (at..at + len)
                    .map(|v| (start[i] + v * step[i]) as i64)
                    .collect::<Vec<_>>();
                index.store_array_subset_opt(&subset, &values, &opt)?;
            }
        }
        let nisar = matches!(
            o.grid,
            crate::products::array_grid::NativeGrid::NisarUtm16North
        );
        let mapping = array(
            store.clone(),
            &format!("{prefix}/spatial_ref"),
            &[],
            &[],
            if nisar { "<u4" } else { "<i4" },
            Value::Null,
            attrs(&first["grid_mapping"]["attributes"], &[]),
            encoding,
        )?;
        if nisar {
            mapping.store_array_subset_opt(
                &ArraySubset::new_with_shape(vec![]),
                &[32616u32],
                &opt,
            )?;
        } else {
            mapping.store_array_subset_opt(&ArraySubset::new_with_shape(vec![]), &[0i32], &opt)?;
        }
        if first["time"].is_object() {
            let t = &first["time"];
            let mut time_attrs = attrs(&t["attributes"], &[]);
            // The subset does not export the source bounds array. Keep its
            // original reference in source_attributes rather than a dangling CF link.
            time_attrs
                .as_object_mut()
                .ok_or("invalid time attributes")?
                .remove("bounds");
            let axis = array(
                store.clone(),
                &format!("{prefix}/t"),
                &[],
                &[],
                "<f8",
                Value::Null,
                time_attrs,
                encoding,
            )?;
            axis.store_array_subset_opt(
                &ArraySubset::new_with_shape(vec![]),
                &[t["stored_value"].as_f64().ok_or("invalid source time")?],
                &opt,
            )?;
        }
        for (path, definition) in first["ancillary"].as_object().into_iter().flatten() {
            let name = path.trim_start_matches('/');
            let dtype = definition["dtype"]
                .as_str()
                .ok_or("missing ancillary dtype")?;
            let a = array(
                store.clone(),
                &format!("{prefix}/{name}"),
                &[],
                &[],
                dtype,
                Value::Null,
                attrs(&definition["attributes"], &[]),
                encoding,
            )?;
            let subset = ArraySubset::new_with_shape(vec![]);
            match dtype {
                "|i1" => a.store_array_subset_opt(
                    &subset,
                    &[definition["value"].as_i64().ok_or("invalid band id")? as i8],
                    &opt,
                )?,
                "<i4" => a.store_array_subset_opt(
                    &subset,
                    &[definition["value"].as_i64().ok_or("invalid band id")? as i32],
                    &opt,
                )?,
                "<f4" => a.store_array_subset_opt(
                    &subset,
                    &[definition["value"].as_f64().ok_or("invalid wavelength")? as f32],
                    &opt,
                )?,
                _ => return Err("unsupported band metadata".into()),
            }
        }
        let mut coverage: Vec<Vec<[usize; 4]>> = vec![Vec::new(); definitions.len()];
        let mut delivered = vec![0usize; definitions.len()];
        let report = o.base.execute_cancellable(&cancelled, |c| {
            check_cancelled(&cancelled)?;
            let Variable::Dataset { path } = &c.variable else {
                return Err("unexpected array chunk variable".into());
            };
            let i = definitions
                .iter()
                .position(|d| d["path"] == *path)
                .ok_or("unexpected array dataset")?;
            let y = c
                .window
                .y
                .checked_sub(start[0])
                .filter(|v| v % step[0] == 0)
                .ok_or("misaligned native row")?
                / step[0];
            let x = c
                .window
                .x
                .checked_sub(start[1])
                .filter(|v| v % step[1] == 0)
                .ok_or("misaligned native column")?
                / step[1];
            let rect = [y, x, y + c.window.height, x + c.window.width];
            if rect[2] > count[0]
                || rect[3] > count[1]
                || coverage[i].len() >= 4096
                || coverage[i]
                    .iter()
                    .any(|a| rect[0] < a[2] && a[0] < rect[2] && rect[1] < a[3] && a[1] < rect[3])
            {
                return Err("duplicate/out-of-bounds array output coverage".into());
            }
            coverage[i].push(rect);
            delivered[i] += c.pixels.len();
            for row in (0..c.window.height).step_by(edge) {
                for col in (0..c.window.width).step_by(edge) {
                    let h = (c.window.height - row).min(edge);
                    let w = (c.window.width - col).min(edge);
                    let subset = ArraySubset::new_with_start_shape(
                        vec![(y + row) as u64, (x + col) as u64],
                        vec![h as u64, w as u64],
                    )?;
                    macro_rules! write_values {
                        ($v:expr) => {{
                            let mut buffer = Vec::with_capacity(h * w);
                            for r in row..row + h {
                                buffer.extend_from_slice(
                                    &$v[r * c.window.width + col..r * c.window.width + col + w],
                                );
                            }
                            encode_call(&store, &mut encode_seconds, || {
                                data[i].store_array_subset_opt(&subset, &buffer, &opt)
                            })?;
                        }};
                    }
                    match &c.pixels {
                        Pixels::Int8(v) => write_values!(v),
                        Pixels::Int16(v) => write_values!(v),
                        Pixels::UInt8(v) => write_values!(v),
                        Pixels::UInt16(v) => write_values!(v),
                        Pixels::UInt32(v) => write_values!(v),
                        Pixels::Float32(v) => write_values!(v),
                    }
                    let mut valid = Vec::with_capacity(h * w);
                    let mut q = Vec::with_capacity(h * w);
                    for r in row..row + h {
                        for cidx in col..col + w {
                            let at = r * c.window.width + cidx;
                            valid.push(u8::from(c.pixels.valid(at, c.nodata)));
                            if path == "/DQF"
                                || path.ends_with("/retrieval_qual_flag")
                                || path.ends_with("/retrieval_qual_flag_pm")
                            {
                                if let Some(policy) = p.request.quality_policies.first() {
                                    q.push(u8::from(arrays::quality_accepts(
                                        policy,
                                        c.pixels.value(at),
                                    )?));
                                }
                            }
                        }
                    }
                    encode_call(&store, &mut encode_seconds, || {
                        validity[i].store_array_subset_opt(&subset, &valid, &opt)
                    })?;
                    if let Some(quality) = &quality {
                        if !q.is_empty() {
                            encode_call(&store, &mut encode_seconds, || {
                                quality.store_array_subset_opt(&subset, &q, &opt)
                            })?;
                        }
                    }
                }
            }
            Ok(())
        })?;
        if delivered.iter().any(|n| *n != count[0] * count[1]) {
            return Err("incomplete native scientific output coverage".into());
        }
        reports.push(json!({"observation":o.observation.id,"execution":report,"selected_values_per_layer":delivered,"source":o.base.explain()}));
    }
    let output_io = json!({"writes":metrics.writes(),"bytes_written":metrics.bytes_written(),"encode_seconds":encode_seconds,"storage_write_seconds":store.write_seconds(),"object_bookkeeping_bound":objects,"consumer_reservation_bytes":8*1024*1024});
    let manifest = json!({"schema":"tile-ripper-array-output-v1","status":"complete","format":"zarr_v2","profile":p.request.product,"plan":p.explain(),"observations":reports,"output_io":output_io,"seconds_before_publication":started.elapsed().as_secs_f64(),"completion":"source validated before/after native execution; atomic no-replace publication","validity":"source_valid_LAYER, aoi_mask, earth_visible and quality_POLICY are distinct; raw values unchanged"});
    group(
        store.clone(),
        "/",
        json!({"status":"complete","profile":p.request.product,"Conventions":"CF-1.8","completion":"manifest.json"}),
        encoding,
    )?;
    store.set(
        &StoreKey::new("manifest.json")?,
        serde_json::to_vec_pretty(&manifest)?.into(),
    )?;
    drop(store);
    drop(metrics);
    transaction.publish(&cancelled)?;
    Ok(manifest)
}
