//! Narrow SWOT adapter over the existing Zarr arrays, timed store and transaction.
use super::*;
use crate::{swot, swot_job::Observation, Pixels, Spatial};

pub(crate) fn reservation(edge: usize) -> Result<usize> {
    edge.checked_mul(edge)
        .and_then(|n| n.checked_mul(32))
        .and_then(|n| n.checked_add(edge.checked_mul(32)?))
        .and_then(|n| n.checked_add(65536))
        .ok_or_else(|| "SWOT writer reservation overflow".into())
}

fn attributes(stored: &Value, dimensions: &[&str]) -> Value {
    let mut attrs = json!({});
    for (name, attr) in stored.as_object().into_iter().flatten() {
        if let Some(value) = attr.get("value") {
            attrs[name] = value.clone();
        } else if let Some(value) = attr.get("nonfinite") {
            attrs[name] = value.clone();
        }
    }
    // Storage annotations belong to the destination. Preserve same-named
    // source attributes inside the raw provenance map without overwriting them.
    attrs["_ARRAY_DIMENSIONS"] = json!(dimensions);
    attrs["source_attributes"] = stored.clone();
    attrs
}

pub(crate) struct Writer {
    transaction: Transaction,
    store: Arc<Store>,
    metrics: Arc<Metrics>,
    edge: usize,
    encode_seconds: f64,
    started: Instant,
    first_observation: Option<f64>,
}
impl Writer {
    pub(crate) fn new(path: &Path, edge: usize, cancelled: &AtomicBool) -> Result<Self> {
        check_cancelled(cancelled)?;
        let transaction = Transaction::new(path)?;
        let mut options = FilesystemStoreOptions::default();
        options.defer_device_flush(true);
        let metrics = Arc::new(Metrics::new(Arc::new(FilesystemStore::new_with_options(
            &transaction.stage,
            options,
        )?)));
        let store = Arc::new(Store::new(metrics.clone()));
        group(
            store.clone(),
            "/",
            json!({"status":"incomplete","Conventions":"CF-1.8","profile":"swot_raster_d_pgd0","layout":"independent native-grid observations"}),
            ScientificEncoding::Uncompressed,
        )?;
        group(
            store.clone(),
            "/observations",
            json!({"ordering":"manifest order; no implicit temporal alignment"}),
            ScientificEncoding::Uncompressed,
        )?;
        Ok(Self {
            transaction,
            store,
            metrics,
            edge,
            encode_seconds: 0.,
            started: Instant::now(),
            first_observation: None,
        })
    }
    pub(crate) fn observation(
        &mut self,
        entry: &Observation,
        prepared: &swot::Prepared,
        inspection: &Value,
        cancelled: &AtomicBool,
    ) -> Result<Value> {
        let setup = Instant::now();
        let Spatial::ArraySlice {
            start, count, step, ..
        } = &entry.request.spatial
        else {
            unreachable!()
        };
        let prefix = format!("/observations/{}", entry.id);
        let datasets = inspection["sources"][0]["object_identities"]["datasets"]
            .as_array()
            .ok_or("missing source definitions")?;
        let definitions: Vec<_> = swot::PATHS
            .iter()
            .map(|path| {
                datasets
                    .iter()
                    .find(|d| d["path"] == *path)
                    .ok_or("missing source definition")
            })
            .collect::<std::result::Result<_, _>>()?;
        let first = definitions[0];
        let source = &inspection["sources"][0];
        let mut attrs = attributes(&first["root_attributes"], &[]);
        attrs["observation_id"] = json!(entry.id);
        attrs["source"] = source.clone();
        attrs["selection"] = json!(entry.request.spatial);
        attrs["quality_policy"] = entry.quality_policy.provenance();
        if let Some(catalog) = &entry.catalog {
            attrs["catalog_provenance"] = json!(catalog);
            attrs["source_datetime_origin"] = json!("catalog granule acquisition_start; native granule and coverage times remain in source_attributes");
        }
        attrs["native_grid"] = json!({"source_shape":first["shape"],"selected_shape":count,"grid_mapping":first["grid_mapping"],"axes":first["axes"],"index_transform_is_geolocation":false});
        attrs["source_datetime"] = source["datetime"].clone();
        group(
            self.store.clone(),
            &prefix,
            attrs,
            ScientificEncoding::Uncompressed,
        )?;
        let mut values = Vec::new();
        let mut masks = Vec::new();
        let chunks = [count[0].clamp(1, self.edge), count[1].clamp(1, self.edge)];
        let options = serial().with_store_empty_chunks(true);
        for definition in &definitions {
            check_cancelled(cancelled)?;
            let name = definition["path"]
                .as_str()
                .ok_or("missing dataset path")?
                .trim_start_matches('/');
            let mut attrs = attributes(&definition["attributes"], &["y", "x"]);
            attrs["grid_mapping"] = json!("spatial_ref");
            attrs["coordinates"] = json!(if first["time"].is_object() {
                "t y x"
            } else {
                "y x"
            });
            attrs["ancillary_variables"] = json!(format!("source_valid_{name} quality_mask"));
            attrs["numeric_semantics"] =
                json!("raw stored values; no calibration or quality masking");
            let mut fill = match &definition["storage_fill"] {
                Value::Null => json!(0),
                Value::String(s) if s == "NaN" => json!("NaN"),
                Value::String(s) if s == "inf" => json!("Infinity"),
                Value::String(s) if s == "-inf" => json!("-Infinity"),
                v => v.clone(),
            };
            if matches!(definition["stored_dtype"].as_str(), Some("|u1" | "<u4")) {
                fill = json!(fill
                    .as_f64()
                    .ok_or("invalid unsigned source storage fill")?
                    as u64);
            }
            values.push(array(
                self.store.clone(),
                &format!("{prefix}/{name}"),
                count,
                &chunks,
                definition["stored_dtype"]
                    .as_str()
                    .ok_or("missing stored type")?,
                fill,
                attrs,
                ScientificEncoding::Uncompressed,
            )?);
            masks.push(array(self.store.clone(), &format!("{prefix}/source_valid_{name}"), count, &chunks, "|u1", Value::Null,
                json!({"_ARRAY_DIMENSIONS":["y","x"],"flag_values":[0,1],"flag_meanings":"missing valid","mask_role":"source validity only: CF missing markers and NaNs","source_variable":name}), ScientificEncoding::Uncompressed)?);
        }
        let quality = array(
            self.store.clone(),
            &format!("{prefix}/quality_mask"),
            count,
            &chunks,
            "|u1",
            Value::Null,
            json!({"_ARRAY_DIMENSIONS":["y","x"],"flag_values":[0,1],"flag_meanings":"excluded accepted","mask_role":"derived quality policy; not source validity or an AOI mask","quality_policy":entry.quality_policy.provenance()}),
            ScientificEncoding::Uncompressed,
        )?;
        for (axis, name) in ["y", "x"].iter().enumerate() {
            let mut attrs = attributes(&first["axes"][axis]["attributes"], &[name]);
            attrs["source_path"] = first["axes"][axis]["path"].clone();
            attrs["source_dtype"] = first["axes"][axis]["dtype"].clone();
            let a = array(
                self.store.clone(),
                &format!("{prefix}/{name}"),
                &[count[axis]],
                &[count[axis].clamp(1, self.edge)],
                "<f8",
                Value::Null,
                attrs,
                ScientificEncoding::Uncompressed,
            )?;
            prepared.with_coordinates(|y, x| {
                let coordinates = if axis == 0 { y } else { x };
                if coordinates.len() != count[axis] {
                    return Err("incomplete prepared coordinate coverage".into());
                }
                for at in (0..coordinates.len()).step_by(self.edge) {
                    check_cancelled(cancelled)?;
                    let end = (at + self.edge).min(coordinates.len());
                    let subset = ArraySubset::new_with_start_shape(
                        vec![at as u64],
                        vec![(end - at) as u64],
                    )?;
                    encode_call(&self.store, &mut self.encode_seconds, || {
                        a.store_array_subset_opt(&subset, &coordinates[at..end], &options)
                    })?;
                }
                Ok(())
            })?;
        }
        let mut attrs = attributes(&first["grid_mapping"]["attributes"], &[]);
        attrs["source_path"] = first["grid_mapping"]["path"].clone();
        attrs["scalar_role"] =
            json!("CRS metadata container; scalar zero is not a source measurement");
        let crs = array(
            self.store.clone(),
            &format!("{prefix}/spatial_ref"),
            &[],
            &[],
            "<i4",
            Value::Null,
            attrs,
            ScientificEncoding::Uncompressed,
        )?;
        crs.store_array_subset_opt(&ArraySubset::new_with_shape(vec![]), &[0i32], &options)?;
        if first["time"].is_object() {
            let time = &first["time"];
            let value = time["stored_value"]
                .as_f64()
                .filter(|v| v.is_finite())
                .ok_or("source time must be a finite stored Float64 scalar")?;
            let a = array(
                self.store.clone(),
                &format!("{prefix}/t"),
                &[],
                &[],
                "<f8",
                Value::Null,
                attributes(&time["attributes"], &[]),
                ScientificEncoding::Uncompressed,
            )?;
            a.store_array_subset_opt(&ArraySubset::new_with_shape(vec![]), &[value], &options)?;
        }
        let setup_seconds = setup.elapsed().as_secs_f64();
        let mut windows: Vec<(usize, usize, usize, usize)> = Vec::new();
        let mut delivered = 0usize;
        let report = prepared.execute_tiles(cancelled, |tile| {
            check_cancelled(cancelled)?;
            let c = &tile.layers[0];
            let window = &c.window;
            let y = window
                .y
                .checked_sub(start[0])
                .filter(|n| n % step[0] == 0)
                .ok_or("misaligned output row")?
                / step[0];
            let x = window
                .x
                .checked_sub(start[1])
                .filter(|n| n % step[1] == 0)
                .ok_or("misaligned output column")?
                / step[1];
            let end_y = y
                .checked_add(window.height)
                .filter(|n| *n <= count[0])
                .ok_or("output row outside selection")?;
            let end_x = x
                .checked_add(window.width)
                .filter(|n| *n <= count[1])
                .ok_or("output column outside selection")?;
            if windows.len() >= entry.request.budget.max_nodes
                || windows
                    .iter()
                    .any(|(a, b, z, w)| y < *z && *a < end_y && x < *w && *b < end_x)
            {
                return Err("duplicate output coverage or output tile bookkeeping limit".into());
            }
            windows.push((y, x, end_y, end_x));
            delivered = delivered
                .checked_add(tile.validity.len())
                .ok_or("output coverage overflow")?;
            for row in (0..window.height).step_by(self.edge) {
                for col in (0..window.width).step_by(self.edge) {
                    check_cancelled(cancelled)?;
                    let height = (window.height - row).min(self.edge);
                    let width = (window.width - col).min(self.edge);
                    let subset = ArraySubset::new_with_start_shape(
                        vec![(y + row) as u64, (x + col) as u64],
                        vec![height as u64, width as u64],
                    )?;
                    for (i, layer) in tile.layers.iter().enumerate() {
                        macro_rules! write_values {
                            ($v:expr) => {{
                                let mut buffer = Vec::with_capacity(height * width);
                                for r in row..row + height {
                                    buffer.extend_from_slice(
                                        &$v[r * window.width + col..r * window.width + col + width],
                                    );
                                }
                                encode_call(&self.store, &mut self.encode_seconds, || {
                                    values[i].store_array_subset_opt(&subset, &buffer, &options)
                                })?;
                            }};
                        }
                        match &layer.pixels {
                            Pixels::Float32(v) => write_values!(v),
                            Pixels::UInt32(v) => write_values!(v),
                            Pixels::UInt8(v) => write_values!(v),
                            _ => return Err("unqualified SWOT output type".into()),
                        }
                        let mask: Vec<_> = (row..row + height)
                            .flat_map(|r| {
                                (col..col + width).map(move |c| {
                                    u8::from(layer.pixels.valid(r * window.width + c, layer.nodata))
                                })
                            })
                            .collect();
                        encode_call(&self.store, &mut self.encode_seconds, || {
                            masks[i].store_array_subset_opt(&subset, &mask, &options)
                        })?;
                    }
                    let mask: Vec<_> = (row..row + height)
                        .flat_map(|r| {
                            (col..col + width).map({
                                let validity = &tile.validity;
                                move |c| validity[r * window.width + c]
                            })
                        })
                        .collect();
                    encode_call(&self.store, &mut self.encode_seconds, || {
                        quality.store_array_subset_opt(&subset, &mask, &options)
                    })?;
                }
            }
            Ok(())
        })?;
        if delivered
            != count[0]
                .checked_mul(count[1])
                .ok_or("selection size overflow")?
            || report.chunks != windows.len() * 5
        {
            return Err("incomplete scientific output coverage".into());
        }
        check_cancelled(cancelled)?;
        self.first_observation
            .get_or_insert_with(|| self.started.elapsed().as_secs_f64());
        Ok(
            json!({"id":entry.id,"group":prefix.trim_start_matches('/'),"status":"source_complete_pending_job_validation","pixels_per_variable":delivered,
            "source":source,"catalog":entry.catalog,"quality_policy":entry.quality_policy.provenance(),"selection":entry.request.spatial,
            "working_memory":inspection["aligned_execution"]["working_memory"],"observed":report,"output_setup_seconds":setup_seconds}),
        )
    }
    pub(crate) fn statistics(&self) -> Value {
        json!({"reads":self.metrics.reads(),"writes":self.metrics.writes(),"bytes_read":self.metrics.bytes_read(),"bytes_written":self.metrics.bytes_written(),
            "storage_read_seconds":self.store.read_seconds(),"storage_write_seconds":self.store.write_seconds(),"encode_seconds":self.encode_seconds,
            "working_reservation_bytes":reservation(self.edge).expect("admitted writer"),"first_observation_seconds":self.first_observation,
            "scope":"Zarr library operations and bytes, including coordinates and metadata; excludes final root metadata/manifest and publication synchronization; not device traffic"})
    }
    pub(crate) fn publish(self, manifest: &Value, cancelled: &AtomicBool) -> Result<()> {
        check_cancelled(cancelled)?;
        group(
            self.store.clone(),
            "/",
            json!({"status":"complete","Conventions":"CF-1.8","profile":"swot_raster_d_pgd0",
            "observation_ids":manifest["observation_ids"],"layout":"independent native-grid observations","completion":"see manifest.json"}),
            ScientificEncoding::Uncompressed,
        )?;
        self.store.set(
            &StoreKey::new("manifest.json")?,
            serde_json::to_vec_pretty(manifest)?.into(),
        )?;
        drop(self.store);
        drop(self.metrics);
        self.transaction.publish(cancelled)
    }
}
