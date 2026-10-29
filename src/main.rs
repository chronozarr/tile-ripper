mod cli;
use std::{env, path::Path};
use tile_ripper::{acquisition, cog::LayoutCache, Format, Request, Result, Sink, Source};
fn run() -> Result<()> {
    let mut args: Vec<_> = env::args().collect();
    if let Some(command) = args.get_mut(1) {
        match command.as_str() {
            "stac-plan" => *command = "plan".into(),
            "stac-fetch" => *command = "fetch".into(),
            _ => {}
        }
    }
    if args.get(1).is_some_and(|s| s == "products") {
        if args.len() != 2 {
            return Err(
                "products accepts no options; use discover --product or a product request".into(),
            );
        }
        return cli::products::list();
    }
    if args.get(1).is_some_and(|s| s == "discover") && args.iter().any(|s| s == "--product") {
        return cli::products::discover(&args[2..]);
    }
    if args.get(1).is_some_and(|s| s == "discover") {
        return cli::discovery::run(&args[2..]);
    }
    if args.get(1).is_some_and(|s| s == "stack") {
        return cli::stack::run(&args[2..]);
    }
    if args.get(1).is_some_and(|s| s == "discover-job") {
        #[cfg(all(feature = "native-hdf5", feature = "authenticated-http"))]
        return cli::discovery::job(&args[2..]);
        #[cfg(not(all(feature = "native-hdf5", feature = "authenticated-http")))]
        return Err("discover-job requires native-hdf5,authenticated-http features".into());
    }
    let options = cli::parse(&args[1..])?;
    let Some(options) = options else {
        return Ok(());
    };
    if options.input.get("product").is_some() {
        return cli::products::run(options);
    }
    if options.input.get("job_schema").is_some() {
        #[cfg(feature = "native-hdf5")]
        return run_job(options);
        #[cfg(not(feature = "native-hdf5"))]
        return Err("SWOT jobs require the native-hdf5 feature".into());
    }
    let cli::Options {
        command,
        input,
        discover_only,
        human,
        cache_path,
        output,
        workers,
        per_host,
        backend,
        gdal_free,
        output_format,
        access,
        checkpoint,
        quality_policy,
        values,
    } = options;
    if let Some(backend) = backend {
        env::set_var("TILE_RIPPER_COG_BACKEND", backend.name());
    } else if gdal_free && cfg!(feature = "native-cog") {
        // Explicit GDAL-free operation selects native regardless of inherited settings.
        env::set_var("TILE_RIPPER_COG_BACKEND", "native");
    }
    if gdal_free {
        tile_ripper::cog::require_gdal_free()?;
    }
    if let Some(n) = workers {
        env::set_var("TILE_RIPPER_WORKERS", n.to_string());
    }
    if let Some(n) = per_host {
        env::set_var("TILE_RIPPER_PER_HOST", n.to_string());
    }
    // Quiet native driver diagnostics; they can include signed locators.
    #[cfg(feature = "gdal")]
    gdal::config::set_error_handler(|_, _, _| {});
    #[cfg(feature = "gdal")]
    for (key, value) in [
        ("GDAL_DISABLE_READDIR_ON_OPEN", "EMPTY_DIR"),
        ("GDAL_INGESTED_BYTES_AT_OPEN", "32768"),
        ("GDAL_NUM_THREADS", "1"),
        ("GDAL_CACHEMAX", "32"),
        ("GDAL_HTTP_MAX_RETRY", "3"),
        ("GDAL_HTTP_TIMEOUT", "45"),
        ("CPL_CURL_VERBOSE", "NO"),
        ("CPL_DEBUG", "OFF"),
    ] {
        // Explicit process settings take precedence over the CLI defaults.
        if env::var_os(key).is_none() {
            gdal::config::set_config_option(key, value)?;
        }
    }
    let mut cache = if let Some(path) = cache_path.as_deref().filter(|p| Path::new(p).exists()) {
        LayoutCache::load(Path::new(path))?
    } else {
        LayoutCache::default()
    };
    let mut request: Request = if input.get("source").is_some() {
        serde_json::from_value(input)
            .map_err(|_| "invalid request schema; see docs/unified-workflow.md")?
    } else {
        acquisition::from_stac(
            serde_json::from_value(input)
                .map_err(|_| "invalid request schema; see docs/unified-workflow.md")?,
        )
    };
    if let Some(path) = output {
        request.sink = match request.sink {
            Sink::ZarrDirectory { .. } => Sink::ZarrDirectory { path },
            Sink::GeoTiffDirectory { .. } => Sink::GeoTiffDirectory { path },
            _ => Sink::Directory { path },
        };
    }
    if command == "stream" {
        request.sink = Sink::Callback;
    } else {
        let path = match &request.sink {
            Sink::Directory { path }
            | Sink::ZarrDirectory { path }
            | Sink::GeoTiffDirectory { path } => Some(path.clone()),
            Sink::Callback => None,
        };
        if let Some(path) = path {
            request.sink = match output_format.as_deref() {
                Some("zarr_v2") => Sink::ZarrDirectory { path },
                Some("geotiff") => Sink::GeoTiffDirectory { path },
                Some("native") => Sink::Directory { path },
                None if gdal_free
                    && cog_source(&request.source)
                    && matches!(request.sink, Sink::Directory { .. }) =>
                {
                    Sink::ZarrDirectory { path }
                }
                _ => request.sink,
            };
        } else if output_format.is_some() {
            return Err(
                "--output-format requires a directory sink; supply --output NEW_DIRECTORY".into(),
            );
        }
    }
    if cog_source(&request.source) {
        // Resolve the selected reader before discovery or source inspection.
        tile_ripper::cog::backend()?;
    }
    if command == "fetch"
        && !cfg!(feature = "gdal")
        && cog_source(&request.source)
        && !matches!(request.sink, Sink::ZarrDirectory { .. })
    {
        return Err("GeoTIFF output requires GDAL; use --gdal-free or --output-format zarr_v2 for scientific Zarr output".into());
    }
    if discover_only && command != "plan" {
        return Err("--discover-only requires plan".into());
    }
    let destination = match &request.sink {
        Sink::Directory { path }
        | Sink::ZarrDirectory { path }
        | Sink::GeoTiffDirectory { path } => Some(std::path::absolute(path)?),
        Sink::Callback => None,
    };
    if command == "fetch" && destination.as_ref().is_some_and(|path| path.exists()) {
        return Err("output directory already exists; choose --output NEW_DIRECTORY".into());
    }
    if human {
        eprintln!("Inspecting source metadata and native selection…");
    }
    let cancelled = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    // Fields are still parsed/validated in builds without the optional readers.
    let _ = (&access, &checkpoint, &quality_policy, values);
    #[cfg(feature = "native-hdf5")]
    if quality_policy.is_some() {
        return run_quality(
            &request,
            &mut cache,
            &command,
            human,
            access,
            checkpoint,
            quality_policy.as_deref().unwrap(),
            values,
            cancelled,
        );
    }
    if command == "fetch"
        && cog_source(&request.source)
        && matches!(request.sink, Sink::ZarrDirectory { .. })
    {
        let flag = cancelled.clone();
        ctrlc::set_handler(move || flag.store(true, std::sync::atomic::Ordering::Release))?;
    }
    let value = if discover_only {
        acquisition::discover(&request)?
    } else {
        #[cfg(all(feature = "native-hdf5", feature = "authenticated-http"))]
        let session = access
            .as_ref()
            .map(|a| authenticated_session(&request, a))
            .transpose()?;
        #[cfg(all(feature = "native-hdf5", feature = "authenticated-http"))]
        let prepared = if let Some(session) = session {
            if let Some(options) = checkpoint {
                acquisition::prepare_authenticated_checkpointed(
                    &request,
                    &mut cache,
                    session,
                    checkpoint_options(options),
                )?
            } else {
                acquisition::prepare_authenticated(&request, &mut cache, session)?
            }
        } else {
            acquisition::prepare(&request, &mut cache)?
        };
        #[cfg(not(all(feature = "native-hdf5", feature = "authenticated-http")))]
        let prepared = acquisition::prepare(&request, &mut cache)?;
        match command.as_str() {
            "plan" => {
                if human {
                    let result = prepared.human();
                    if let Some(path) = cache_path {
                        cache.save(Path::new(&path))?;
                    }
                    println!("{result}");
                    return Ok(());
                }
                prepared.explain()
            }
            "stream" => {
                use std::io::Write;
                let started = std::time::Instant::now();
                let mut stream = prepared.into_stream(Default::default())?;
                let stdout = std::io::stdout();
                let mut out = stdout.lock();
                for chunk in stream.by_ref() {
                    let c = chunk?;
                    writeln!(
                        out,
                        "{}",
                        serde_json::json!({"type":"chunk","asset_id":c.asset_id,"node_id":c.node_id,"variable":c.variable,"datetime":c.datetime,"indices":c.indices,"window":c.window,"crs":c.crs,"transform":c.transform,"nodata":c.nodata,"scale":c.scale,"offset":c.offset,"dtype":match &c.pixels {tile_ripper::Pixels::Int8(_)=>"int8",tile_ripper::Pixels::UInt8(_)=>"uint8",tile_ripper::Pixels::UInt16(_)=>"uint16",tile_ripper::Pixels::Int16(_)=>"int16",tile_ripper::Pixels::UInt32(_)=>"uint32",tile_ripper::Pixels::Float32(_)=>"float32",tile_ripper::Pixels::Float64(_)=>"float64"},"bytes":c.pixels.byte_len(),"valid_pixels":c.validity().filter(|v|*v).count(),"seconds":started.elapsed().as_secs_f64()})
                    )?;
                }
                let report = stream.finish()?;
                writeln!(
                    out,
                    "{}",
                    serde_json::json!({"type":"complete","observed":report})
                )?;
                return Ok(());
            }
            "fetch" => {
                if human {
                    eprintln!("Fetching native pixels; validating source identities…");
                }
                if prepared.scientific_output() {
                    prepared.fetch_cancellable(cancelled)?
                } else {
                    prepared.fetch()?
                }
            }
            _ => return Err("expected plan, fetch or stream".into()),
        }
    };
    if let Some(path) = cache_path {
        cache.save(Path::new(&path))?;
    }
    if human && command == "fetch" {
        print!(
            "{}",
            cli::fetch_summary(
                value["outputs"].as_array().map_or(0, |a| a.len()),
                &destination
                    .as_ref()
                    .map_or_else(|| "callback".into(), |p| p.display().to_string()),
                cli::transport_totals(&value["observed"]["transport"]),
            )
        );
    } else {
        println!("{}", serde_json::to_string_pretty(&value)?);
    }
    Ok(())
}
#[cfg(all(feature = "native-hdf5", feature = "authenticated-http"))]
fn authenticated_session(
    request: &Request,
    access: &cli::Access,
) -> Result<std::sync::Arc<tile_ripper::authenticated_http::AuthenticatedHttp>> {
    use tile_ripper::authenticated_http::{AuthenticatedHttp, Config};
    let Source::Assets { assets } = &request.source else {
        return Err("authentication requires one explicit native HDF5 asset".into());
    };
    if assets.len() != 1 || assets[0].format != Format::Hdf5 {
        return Err("authentication requires one explicit native HDF5 asset".into());
    }
    let session = if access.profile == "earthdata" {
        AuthenticatedHttp::earthdata(&assets[0].locator, &access.netrc)?
    } else {
        let mut config: Config = serde_json::from_slice(
            &std::fs::read(
                access
                    .policy
                    .as_ref()
                    .ok_or("missing public access policy")?,
            )
            .map_err(|_| "cannot read public access policy")?,
        )
        .map_err(|_| "invalid public access policy")?;
        if let Some(ca) = &access.ca {
            config.additional_roots.push(
                reqwest::Certificate::from_pem(
                    &std::fs::read(ca).map_err(|_| "cannot read trust root")?,
                )
                .map_err(|_| "invalid trust root")?,
            );
        }
        AuthenticatedHttp::new(config, &access.netrc)?
    };
    Ok(std::sync::Arc::new(session))
}
#[cfg(all(feature = "native-hdf5", feature = "authenticated-http"))]
fn checkpoint_options(flags: cli::Checkpoint) -> tile_ripper::checkpoint::Options {
    let mut options = tile_ripper::checkpoint::Options::new(flags.directory);
    options.max_bytes = flags.mib as u64 * 1024 * 1024;
    options.max_entry_bytes = flags.entry_mib * 1024 * 1024;
    options.max_entries = flags.entries;
    options
}
#[cfg(feature = "native-hdf5")]
#[allow(clippy::too_many_arguments)]
fn run_quality(
    request: &Request,
    cache: &mut LayoutCache,
    command: &str,
    human: bool,
    access: Option<cli::Access>,
    checkpoint: Option<cli::Checkpoint>,
    policy_path: &str,
    values: bool,
    cancelled: std::sync::Arc<std::sync::atomic::AtomicBool>,
) -> Result<()> {
    use tile_ripper::swot;
    let started = std::time::Instant::now();
    let policy: swot::Policy = serde_json::from_slice(
        &std::fs::read(policy_path).map_err(|_| "cannot read quality policy")?,
    )
    .map_err(|_| "invalid quality policy")?;
    let _ = (&access, &checkpoint);
    #[cfg(feature = "authenticated-http")]
    let session = access
        .as_ref()
        .map(|a| authenticated_session(request, a))
        .transpose()?;
    #[cfg(feature = "authenticated-http")]
    let prepared = if let Some(session) = &session {
        if let Some(options) = checkpoint {
            swot::prepare_authenticated_checkpointed(
                request,
                policy,
                cache,
                session.clone(),
                checkpoint_options(options),
            )
        } else {
            swot::prepare_authenticated(request, policy, cache, session.clone())
        }
    } else {
        swot::prepare(request, policy, cache)
    };
    #[cfg(not(feature = "authenticated-http"))]
    let prepared = swot::prepare(request, policy, cache);
    #[cfg(feature = "authenticated-http")]
    let prepared = prepared.inspect_err(|_| {
        if let Some(s) = &session {
            eprintln!(
                "{}",
                serde_json::json!({"authenticated_http":s.statistics()})
            );
        }
    })?;
    #[cfg(not(feature = "authenticated-http"))]
    let prepared = prepared?;
    if command == "plan" {
        if human {
            println!("SWOT aligned five-layer inspection (all stream values provisional until finish).\n{}", prepared.explain())
        } else {
            println!("{}", serde_json::to_string_pretty(&prepared.explain())?)
        }
        return Ok(());
    }
    let result = stream_quality(prepared, values, cancelled, started);
    #[cfg(feature = "authenticated-http")]
    if result.is_err() {
        if let Some(session) = &session {
            eprintln!(
                "{}",
                serde_json::json!({"authenticated_http":session.statistics()})
            );
        }
    }
    result
}
#[cfg(feature = "native-hdf5")]
fn stream_quality(
    prepared: tile_ripper::swot::Prepared,
    values: bool,
    cancelled: std::sync::Arc<std::sync::atomic::AtomicBool>,
    started: std::time::Instant,
) -> Result<()> {
    use std::io::Write;
    let flag = cancelled.clone();
    ctrlc::set_handler(move || flag.store(true, std::sync::atomic::Ordering::Release))?;
    let preparation = started.elapsed().as_secs_f64();
    println!(
        "{}",
        serde_json::json!({"type":"prepared","inspection":prepared.explain(),"prepare_seconds":preparation})
    );
    std::io::stdout().flush()?;
    let mut stream = prepared.into_stream_cancellable(cancelled)?;
    let stdout = std::io::stdout();
    let mut out = stdout.lock();
    let mut summary = Summary::default();
    for (index, tile) in stream.by_ref().enumerate() {
        let tile = tile?;
        summary.add(&tile)?;
        let layers = tile.layers.iter().map(|c| {
            let mut v = serde_json::json!({"asset_id":c.asset_id,"variable":c.variable,"window":c.window,
                "indices":c.indices,"coordinates":c.coordinates,"crs":c.crs,"transform":c.transform,
                "nodata":c.nodata,"scale":c.scale,"offset":c.offset,"bytes":c.pixels.byte_len()});
            if values {
                use base64::Engine;
                let bytes: Vec<u8> = match &c.pixels {
                    tile_ripper::Pixels::Int8(v) => v.iter().map(|v|*v as u8).collect(),
                    tile_ripper::Pixels::UInt8(v) => v.clone(),
                    tile_ripper::Pixels::UInt16(v) => v.iter().flat_map(|v| v.to_le_bytes()).collect(),
                    tile_ripper::Pixels::Int16(v) => v.iter().flat_map(|v| v.to_le_bytes()).collect(),
                    tile_ripper::Pixels::UInt32(v) => v.iter().flat_map(|v| v.to_le_bytes()).collect(),
                    tile_ripper::Pixels::Float32(v) => v.iter().flat_map(|v| v.to_le_bytes()).collect(),
                    tile_ripper::Pixels::Float64(v) => v.iter().flat_map(|v| v.to_le_bytes()).collect(),
                };
                v["encoding"] = "base64-little-endian".into();
                v["dtype"] = match &c.pixels {tile_ripper::Pixels::Int8(_)=>"int8",tile_ripper::Pixels::UInt8(_)=>"uint8",tile_ripper::Pixels::UInt16(_)=>"uint16",tile_ripper::Pixels::Int16(_)=>"int16",tile_ripper::Pixels::UInt32(_)=>"uint32",tile_ripper::Pixels::Float32(_)=>"float32",tile_ripper::Pixels::Float64(_)=>"float64"}.into();
                v["values"] = base64::engine::general_purpose::STANDARD.encode(bytes).into();
                v["source_mask"] = base64::engine::general_purpose::STANDARD.encode(c.validity().map(u8::from).collect::<Vec<_>>()).into();
            }
            v
        }).collect::<Vec<_>>();
        writeln!(
            out,
            "{}",
            serde_json::json!({"type":"tile","provisional":true,"index":index,"layers":layers,
            "validity":tile.validity,"seconds":started.elapsed().as_secs_f64(),"replay_contract":"stream restarts; earlier tiles may repeat"})
        )?;
        out.flush()?;
    }
    let report = stream.finish()?;
    writeln!(
        out,
        "{}",
        serde_json::json!({"type":"complete","observed":report,"summary":summary.report(),
        "prepare_seconds":preparation,"completion_seconds":started.elapsed().as_secs_f64()})
    )?;
    Ok(())
}
#[cfg(feature = "native-hdf5")]
#[derive(Default, serde::Serialize)]
struct Summary {
    selected_count: u64,
    wse_sum_m: f64,
    uncertainty_count: u64,
    uncertainty_sum_m: f64,
    contributing_count: u64,
    contributing_pixel_sum: u64,
}
#[cfg(feature = "native-hdf5")]
impl Summary {
    fn report(&self) -> serde_json::Value {
        let mut value = serde_json::to_value(self).expect("finite qualified summary");
        value["wse_mean_m"] = serde_json::json!(if self.selected_count > 0 {
            Some(self.wse_sum_m / self.selected_count as f64)
        } else {
            None
        });
        value["mean_pixel_uncertainty_m"] = serde_json::json!(if self.uncertainty_count > 0 {
            Some(self.uncertainty_sum_m / self.uncertainty_count as f64)
        } else {
            None
        });
        value
    }
    fn add(&mut self, tile: &tile_ripper::swot::Tile) -> Result<()> {
        let wse = tile.layers[0].pixels.as_f32().ok_or("WSE dtype")?;
        let u = tile.layers[3].pixels.as_f32().ok_or("uncertainty dtype")?;
        let n = tile.layers[4].pixels.as_u32().ok_or("count dtype")?;
        for (i, (um, nm)) in tile.layers[3]
            .validity()
            .zip(tile.layers[4].validity())
            .enumerate()
        {
            if tile.validity[i] != 0 {
                self.selected_count += 1;
                self.wse_sum_m += f64::from(wse[i]);
                if um && u[i].is_finite() {
                    self.uncertainty_count += 1;
                    self.uncertainty_sum_m += f64::from(u[i]);
                }
                if nm {
                    self.contributing_count += 1;
                    self.contributing_pixel_sum += u64::from(n[i]);
                }
            }
        }
        Ok(())
    }
}
#[cfg(feature = "native-hdf5")]
fn run_job(options: cli::Options) -> Result<()> {
    use std::sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    };
    use tile_ripper::swot_job::{Job, Manifest};
    if options.command == "stream"
        || options.discover_only
        || options.cache_path.is_some()
        || options.quality_policy.is_some()
        || options.values
        || options.backend.is_some()
        || options.workers.is_some_and(|n| n != 1)
        || options.per_host.is_some_and(|n| n != 1)
        || options
            .output_format
            .as_deref()
            .is_some_and(|v| v != "zarr_v2")
    {
        return Err("SWOT jobs support serial plan/fetch with inline quality policies and Zarr v2; no discovery, metadata cache, COG backend or stream flags".into());
    }
    if options.gdal_free {
        tile_ripper::cog::require_gdal_free()?;
    }
    let manifest: Manifest = serde_json::from_value(options.input)
        .map_err(|_| "invalid SWOT job schema; see docs/swot-job/workflow.md")?;
    manifest.validate()?;
    if options.command == "fetch" && options.output.is_none() {
        return Err("job fetch requires --output NEW_DIRECTORY".into());
    }
    let cancelled = Arc::new(AtomicBool::new(false));
    if options.command == "fetch" {
        let flag = cancelled.clone();
        ctrlc::set_handler(move || flag.store(true, Ordering::Release))?;
    }
    #[cfg(feature = "authenticated-http")]
    let mut factory = |asset: &tile_ripper::Asset| {
        job_session(
            asset,
            options
                .access
                .as_ref()
                .ok_or("missing explicit authentication opt-in")?,
            manifest.observations.len(),
        )
    };
    #[cfg(feature = "authenticated-http")]
    let job = if options.access.is_some() {
        Job::authenticated(
            &manifest,
            &mut factory,
            options.checkpoint.map(checkpoint_options),
        )?
    } else {
        Job::new(&manifest)?
    };
    #[cfg(not(feature = "authenticated-http"))]
    let job = Job::new(&manifest)?;
    let value = if options.command == "plan" {
        job.plan()?
    } else {
        match job.fetch(options.output.as_ref().unwrap(), cancelled) {
            Ok(value) => value,
            Err(error) => {
                if let Some(failure) = error.downcast_ref::<tile_ripper::swot_job::Failure>() {
                    println!("{}", serde_json::to_string_pretty(&failure.receipt)?);
                }
                return Err(error);
            }
        }
    };
    if options.human {
        println!(
            "SWOT job {}: {} explicit native-grid observation(s), manifest order.",
            if options.command == "plan" {
                "inspection"
            } else {
                "complete"
            },
            manifest.observations.len()
        );
        for entry in &manifest.observations {
            println!(
                "  {} -> observations/{}; {:?}",
                entry.id, entry.id, entry.request.spatial
            );
        }
        println!("Aggregate working admission: {} bytes (limit {}); one active source, synchronous writer. Process RSS is separate.",value["admission"]["aggregate_admitted_cap_bytes"],manifest.limits.memory_bytes);
        if let Some(output) = options.output {
            println!("Output: {output}");
        }
        println!("Full source/grid/variable, memory, checkpoint and completion details: repeat with --json.");
    } else {
        println!("{}", serde_json::to_string_pretty(&value)?);
    }
    Ok(())
}
fn cog_source(source: &Source) -> bool {
    match source {
        Source::Stac { .. } | Source::Catalog { .. } => true,
        Source::Assets { assets } => assets.iter().any(|asset| asset.format == Format::Cog),
    }
}
fn main() {
    if let Err(e) = run() {
        eprintln!("tile-ripper: {e}");
        std::process::exit(1);
    }
}

#[cfg(all(feature = "native-hdf5", feature = "authenticated-http"))]
fn job_session(
    asset: &tile_ripper::Asset,
    access: &cli::Access,
    source_count: usize,
) -> Result<std::sync::Arc<tile_ripper::authenticated_http::AuthenticatedHttp>> {
    use std::sync::Arc;
    let session = if access.profile == "earthdata" {
        tile_ripper::authenticated_http::AuthenticatedHttp::earthdata(
            &asset.locator,
            &access.netrc,
        )?
    } else {
        use std::io::Read;
        let mut bytes = Vec::new();
        std::fs::File::open(access.policy.as_ref().ok_or("missing job access policy")?)
            .map_err(|_| "cannot read public job access policy")?
            .take(256 * 1024 + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| "cannot read public job access policy")?;
        if bytes.len() > 256 * 1024 {
            return Err("public job access policy exceeds 256 KiB".into());
        }
        #[derive(serde::Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Policies {
            sources: Vec<serde_json::Value>,
        }
        let policies: Policies = serde_json::from_slice(&bytes).map_err(|_| {
            "job --auth-policy requires a sources array of public exact-object policies"
        })?;
        if policies.sources.len() != source_count {
            return Err("job access policies must match the explicit source count".into());
        }
        let matching: Vec<_> = policies
            .sources
            .iter()
            .filter(|p| p["canonical_url"] == asset.locator)
            .collect();
        if matching.len() != 1 {
            return Err(
                "each job source requires exactly one matching public access policy".into(),
            );
        }
        let mut config: tile_ripper::authenticated_http::Config =
            serde_json::from_value(matching[0].clone())
                .map_err(|_| "invalid public job access policy")?;
        if let Some(ca) = &access.ca {
            config.additional_roots.push(
                reqwest::Certificate::from_pem(
                    &std::fs::read(ca).map_err(|_| "cannot read trust root")?,
                )
                .map_err(|_| "invalid trust root")?,
            );
        }
        tile_ripper::authenticated_http::AuthenticatedHttp::new(config, &access.netrc)?
    };
    Ok(Arc::new(session))
}
