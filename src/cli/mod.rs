//! CLI convenience syntax translates into the existing public Request schema.
use serde_json::{json, Value};
use std::{collections::BTreeMap, fs};
use tile_ripper::Result;
pub mod discovery;
pub mod products;
const HELP: &str = r#"tile-ripper plan|fetch|stream [request.json] [options]
  tile-ripper products
  tile-ripper discover --product FAMILY --bbox W,S,E,N --start RFC3339 --end RFC3339 --bands NAMES --limit 8
  Product JSON: docs/products/workflow.md
  tile-ripper discover --collection SWOT_L2_HR_Raster_D --bbox W,S,E,N \
      --start RFC3339 --end RFC3339 --limit 20 --output candidates.json
  tile-ripper discover-job selection.json --auth earthdata --netrc PRIVATE_FILE \
      --output NEW_JOB.json > job-review.json
  Bounded CMR discovery and explicit native selection: docs/swot-discovery/workflow.md
  --source URL_OR_PATH --format zarr_v2|stac
  --collection NAME                 required for STAC
  --bbox WEST,SOUTH,EAST,NORTH       EPSG:4326; or --aoi polygon.geojson
  --start RFC3339 --end RFC3339      inclusive endpoints
  --variables NAME[,NAME]           native array names or STAC bands
  --output NEW_DIRECTORY            required for direct fetch
  --output-format zarr_v2|geotiff|native   GDAL-free COG defaults to zarr_v2
  --buffer-mib N                    payload budget (default 32; not RSS)
  --workers N --per-host N           concurrent reader operations (1..64)
  --backend native|gdal             COG reader; overrides TILE_RIPPER_COG_BACKEND
  --gdal-free                      require a build without GDAL; native COG if compiled
  --provider auto|planetary_computer|earthdata|pinned   product route override
  --human | --json                  JSON default, stable for scripts
  --dry-run                        fetch behaves as plan; metadata may download
  --cache FILE --discover-only      discover-only requires plan
  --auth earthdata|configured --netrc PRIVATE_FILE   explicit native HDF5 auth
  --auth-policy PUBLIC_JSON --auth-ca PEM           configured exact-object policy
  --quality-policy POLICY_JSON      aligned five-layer SWOT plan/stream
  --checkpoint PRIVATE_DIRECTORY    opt-in authenticated source-range resume
  --checkpoint-mib N --checkpoint-entry-mib N --checkpoint-entries N
  --values                         stream native bits as base64 LE plus masks/axes
SWOT job JSON: plan/fetch with one to four explicit observations and inline policies.
  tile-ripper plan job.json --auth earthdata --netrc PRIVATE_FILE
  tile-ripper fetch job.json --auth earthdata --netrc PRIVATE_FILE \
      --checkpoint PRIVATE_DIRECTORY --output NEW.zarr
Job checkpoint byte/entry flags are AGGREGATE limits, divided into fixed source quotas.
Configured job --auth-policy JSON contains a sources array of exact-object policies.
Requires native-hdf5 (plus authenticated-http for opt-in auth/checkpoints).
Job contract, aggregate memory limits and native groups: docs/swot-job/workflow.md.
JSON requests retain the full advanced schema. Do not mix selection flags and JSON.
stream emits NDJSON descriptors and completion; Rust streaming yields native values.
GDAL remains the default. --backend native in a dual-feature build still links GDAL.
GDAL-free Sentinel-2: build --no-default-features --features native-cog, then:
  tile-ripper stream docs/native-sentinel/request.json --gdal-free
  tile-ripper plan --source https://earth-search.aws.element84.com/v1 --format stac \
    --collection sentinel-2-c1-l2a --aoi polygon.geojson \
    --start 2025-07-28T00:00:00Z --end 2025-07-28T23:59:59Z --variables B04,B08 --gdal-free
Native supports narrow tiled single-band UInt8/UInt16/Int16/Float32 COGs; unsupported profiles fail.
GDAL-free fetch writes native scientific values to per-observation Zarr v2 groups.
Use --output-format zarr_v2 in a GDAL build; GeoTIFF output still needs GDAL.
Install, supported profiles, Rust examples: docs/native-http/cli.md. No resampling or compositing."#;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Backend {
    Native,
    Gdal,
}
impl Backend {
    pub fn name(self) -> &'static str {
        match self {
            Self::Native => "native",
            Self::Gdal => "gdal",
        }
    }
}
pub struct Options {
    pub command: String,
    pub input: Value,
    pub discover_only: bool,
    pub human: bool,
    pub cache_path: Option<String>,
    pub output: Option<String>,
    pub workers: Option<usize>,
    pub per_host: Option<usize>,
    pub backend: Option<Backend>,
    pub gdal_free: bool,
    pub output_format: Option<String>,
    pub access: Option<Access>,
    pub checkpoint: Option<Checkpoint>,
    pub quality_policy: Option<String>,
    pub values: bool,
}
#[cfg_attr(
    not(all(feature = "native-hdf5", feature = "authenticated-http")),
    allow(dead_code)
)]
pub struct Access {
    pub profile: String,
    pub netrc: String,
    pub policy: Option<String>,
    pub ca: Option<String>,
}
#[cfg_attr(
    not(all(feature = "native-hdf5", feature = "authenticated-http")),
    allow(dead_code)
)]
pub struct Checkpoint {
    pub directory: String,
    pub mib: usize,
    pub entry_mib: usize,
    pub entries: usize,
}
pub fn parse(args: &[String]) -> Result<Option<Options>> {
    parse_with_input_limit(args, 16 * 1024 * 1024)
}
pub fn parse_with_input_limit(args: &[String], input_limit: usize) -> Result<Option<Options>> {
    if args.is_empty() || args.iter().any(|s| s == "--help" || s == "-h") {
        println!("{HELP}");
        return Ok(None);
    }
    let mut command = args[0].clone();
    if !matches!(command.as_str(), "plan" | "fetch" | "stream") {
        return Err("expected plan, fetch or stream; use --help".into());
    }
    let mut flags = BTreeMap::new();
    let mut file = None;
    let mut human = false;
    let mut discover_only = false;
    let mut dry = false;
    let mut gdal_free = false;
    let mut values = false;
    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--human" => human = true,
            "--json" => human = false,
            "--dry-run" => dry = true,
            "--discover-only" => discover_only = true,
            "--gdal-free" => gdal_free = true,
            "--values" => values = true,
            key @ ("--source"
            | "--format"
            | "--collection"
            | "--bbox"
            | "--aoi"
            | "--start"
            | "--end"
            | "--variables"
            | "--buffer-mib"
            | "--output"
            | "--cache"
            | "--workers"
            | "--per-host"
            | "--backend"
            | "--output-format"
            | "--provider"
            | "--auth"
            | "--netrc"
            | "--auth-policy"
            | "--auth-ca"
            | "--quality-policy"
            | "--checkpoint"
            | "--checkpoint-mib"
            | "--checkpoint-entry-mib"
            | "--checkpoint-entries") => {
                i += 1;
                let value = args
                    .get(i)
                    .filter(|v| !v.starts_with("--"))
                    .ok_or("option requires a value; use --help")?;
                if flags.insert(key.to_owned(), value.clone()).is_some() {
                    return Err("duplicate option; specify each option once".into());
                }
            }
            key if key.starts_with('-') => return Err("unknown option; use --help".into()),
            _ if file.is_none() => file = Some(args[i].clone()),
            _ => return Err("expected only one request file; use --help".into()),
        }
        i += 1;
    }
    if dry {
        command = "plan".into();
    }
    if discover_only && command != "plan" {
        return Err("--discover-only requires plan".into());
    }
    let backend = flags
        .remove("--backend")
        .map(|name| match name.as_str() {
            "native" => Ok(Backend::Native),
            "gdal" => Ok(Backend::Gdal),
            _ => Err("--backend requires native or gdal"),
        })
        .transpose()?;
    validate_backend(backend, gdal_free)?;
    let output_format = flags.remove("--output-format");
    if output_format
        .as_deref()
        .is_some_and(|s| !["zarr_v2", "geotiff", "native"].contains(&s))
    {
        return Err("--output-format requires zarr_v2, geotiff or native".into());
    }
    let mut resource = |key: &str| -> Result<Option<usize>> {
        flags
            .remove(key)
            .map(|value| {
                value
                    .parse::<usize>()
                    .ok()
                    .filter(|n| (1..=64).contains(n))
                    .ok_or_else(|| format!("{key} requires an integer from 1 to 64").into())
            })
            .transpose()
    };
    let workers = resource("--workers")?;
    let per_host = resource("--per-host")?;
    let output = flags.remove("--output");
    let cache_path = flags.remove("--cache");
    let provider = flags.remove("--provider");
    let profile = flags.remove("--auth");
    let netrc = flags.remove("--netrc");
    let policy = flags.remove("--auth-policy");
    let ca = flags.remove("--auth-ca");
    let access = match (profile, netrc) {
        (Some(profile), Some(netrc)) => {
            if !cfg!(all(any(feature = "native-hdf5", feature = "native-cog"), feature = "authenticated-http")) {
                return Err("authenticated CLI requires native-cog or native-hdf5 plus authenticated-http features".into());
            }
            if !matches!(profile.as_str(), "earthdata" | "configured")
                || (profile == "configured") != policy.is_some()
                || ca.is_some() && policy.is_none()
            {return Err("use --auth earthdata --netrc FILE, or --auth configured --netrc FILE --auth-policy FILE [--auth-ca PEM]".into())}
            Some(Access {profile, netrc, policy, ca})
        }
        (None, None) if policy.is_none() && ca.is_none() => None,
        _ => return Err("authentication requires explicit --auth and --netrc; credential environment variables are not used".into()),
    };
    let quality_policy = flags.remove("--quality-policy");
    if quality_policy.is_some() && !cfg!(feature = "native-hdf5") {
        return Err("--quality-policy requires native-hdf5".into());
    }
    let directory = flags.remove("--checkpoint");
    let has_limits = [
        "--checkpoint-mib",
        "--checkpoint-entry-mib",
        "--checkpoint-entries",
    ]
    .iter()
    .any(|k| flags.contains_key(*k));
    let checkpoint = if let Some(directory) = directory {
        if access.is_none() {
            return Err("--checkpoint requires explicit authenticated native HDF5 access".into());
        }
        let mut limit = |key: &str, default: usize, maximum: usize| -> Result<usize> {
            let value = flags
                .remove(key)
                .map(|v| v.parse::<usize>())
                .transpose()
                .map_err(|_| "checkpoint limits require positive integers")?
                .unwrap_or(default);
            if value == 0 || value > maximum {
                return Err("checkpoint limit exceeds supported bounds".into());
            }
            Ok(value)
        };
        Some(Checkpoint {
            directory,
            mib: limit("--checkpoint-mib", 64, 1048576)?,
            entry_mib: limit("--checkpoint-entry-mib", 8, 16)?,
            entries: limit("--checkpoint-entries", 2048, 16384)?,
        })
    } else {
        if has_limits {
            return Err("checkpoint limits require --checkpoint DIRECTORY".into());
        }
        None
    };
    let mut input = if let Some(file) = file {
        if !flags.is_empty() {
            return Err("do not mix request JSON with source/selection flags; edit JSON or use direct flags".into());
        }
        use std::io::Read;
        let mut bytes = Vec::new();
        fs::File::open(file)
            .map_err(|_| "cannot read request file; check path and permissions")?
            .take(input_limit as u64 + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| "cannot read request JSON")?;
        if bytes.len() > input_limit {
            return Err(format!("request JSON exceeds input bound ({input_limit} bytes)").into());
        }
        let input: Value = serde_json::from_slice(&bytes)
            .map_err(|_| "invalid request JSON syntax; see examples/weatherbench2.json")?;
        if input.get("job_schema").is_some() && bytes.len() > 64 * 1024 {
            return Err("job manifest exceeds 64 KiB input bound".into());
        }
        input
    } else {
        let required = |name: &str| {
            flags
                .get(name)
                .cloned()
                .ok_or_else(|| format!("missing {name}; use --help"))
        };
        let source = required("--source")?;
        let format = required("--format")?;
        let variables = required("--variables")?;
        let source = match format.as_str() {
            "zarr_v2" => {
                if flags.contains_key("--collection") {
                    return Err("--collection is only valid with --format stac".into());
                }
                json!({"kind":"assets","assets":[{"id":"subset","locator":source,"format":"zarr_v2"}]})
            }
            "stac" => {
                json!({"kind":"stac","endpoint":source,"collection":required("--collection")?,"limits":{"max_items":8,"page_size":2,"max_assets":8,"max_output_pixels":1000000}})
            }
            _ => {
                return Err(
                    "unsupported --format; use stac or zarr_v2 (specific profiles in README.md)"
                        .into(),
                )
            }
        };
        let spatial = match (flags.get("--bbox"), flags.get("--aoi")) {
            (Some(bbox), None) => {
                let values: Vec<f64> = bbox
                    .split(',')
                    .map(str::parse)
                    .collect::<std::result::Result<_, _>>()
                    .map_err(|_| "--bbox requires four finite numbers WEST,SOUTH,EAST,NORTH")?;
                if values.len() != 4
                    || values.iter().any(|n| !n.is_finite())
                    || values[0] >= values[2]
                    || values[1] >= values[3]
                    || values[0] < -180.
                    || values[2] > 360.
                    || values[1] < -90.
                    || values[3] > 90.
                {
                    return Err(
                        "--bbox requires ordered geographic bounds; no antimeridian crossing"
                            .into(),
                    );
                }
                json!({"kind":"bounds","crs":"EPSG:4326","bbox":values})
            }
            (None, Some(path)) => {
                let geo: Value =
                    serde_json::from_slice(&fs::read(path).map_err(|_| "cannot read --aoi file")?)
                        .map_err(|_| "--aoi must contain a GeoJSON Polygon geometry")?;
                if geo["type"] != "Polygon" {
                    return Err("--aoi requires a GeoJSON Polygon geometry, not a Feature/collection; Zarr supports bounds only".into());
                }
                json!({"kind":"polygon","crs":"EPSG:4326","coordinates":geo["coordinates"]})
            }
            _ => return Err("provide exactly one of --bbox or --aoi".into()),
        };
        let start = required("--start")?;
        let end = required("--end")?;
        let t0 = chrono::DateTime::parse_from_rfc3339(&start)
            .map_err(|_| "--start requires RFC3339, e.g. 2020-06-01T00:00:00Z")?;
        let t1 = chrono::DateTime::parse_from_rfc3339(&end)
            .map_err(|_| "--end requires RFC3339, e.g. 2020-06-01T23:59:59Z")?;
        if t0 > t1 {
            return Err("--start must be no later than --end".into());
        }
        let variables: Vec<_> = variables
            .split(',')
            .map(str::trim)
            .map(|name| {
                if format == "stac" {
                    json!({"kind":"named","name":name})
                } else {
                    json!({"kind":"dataset","path":name})
                }
            })
            .collect();
        if variables.iter().any(|v| v["name"] == "" || v["path"] == "") {
            return Err("--variables requires nonempty comma-separated names".into());
        }
        let mib: usize = flags
            .get("--buffer-mib")
            .map(|v| v.parse())
            .transpose()
            .map_err(|_| "--buffer-mib requires a positive integer")?
            .unwrap_or(32);
        let bytes = mib
            .checked_mul(1024 * 1024)
            .filter(|b| *b > 0)
            .ok_or("--buffer-mib overflows or is zero")?;
        if command == "fetch" && output.is_none() {
            return Err(
                "fetch requires --output NEW_DIRECTORY; existing directories are never overwritten"
                    .into(),
            );
        }
        let sink = if command == "plan" && gdal_free && output.is_none() {
            json!({"kind":"callback"})
        } else {
            json!({"kind":"directory","path":output.as_deref().unwrap_or("subset")})
        };
        json!({"source":source,"spatial":spatial,"time":{"kind":"interval","start":start,"end":end},"variables":variables,"grid":{"kind":"native"},"sink":sink,"budget":{"buffer_bytes":bytes,"max_nodes":100000,"chunk_edge":512}})
    };
    let product = input.get("product").is_some();
    if values && (command != "stream" || (!product && quality_policy.is_none())) {
        return Err("--values requires stream --quality-policy FILE, or a product stream".into());
    }
    if access.is_some() && !product && !cfg!(feature = "native-hdf5") {
        return Err("ordinary authenticated requests require native-hdf5".into());
    }
    if !product
        && input.get("job_schema").is_none()
        && (access.is_some() || quality_policy.is_some())
        && (command == "fetch" || discover_only)
    {
        return Err("authenticated/quality single-source CLI supports plan and stream; use a SWOT job manifest for scientific Zarr fetch".into());
    }
    if let Some(provider) = provider {
        if input.get("product").is_none() {
            return Err("--provider applies only to named product requests".into());
        }
        let parsed: tile_ripper::products::routing::Provider =
            serde_json::from_value(Value::String(provider))
                .map_err(|_| "--provider requires auto, planetary_computer, earthdata or pinned")?;
        input["provider"] = serde_json::to_value(parsed)?;
    }
    Ok(Some(Options {
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
    }))
}

/// Validate explicit choices before opening a request file or contacting sources.
pub fn validate_backend(backend: Option<Backend>, gdal_free: bool) -> Result<()> {
    if gdal_free && backend == Some(Backend::Gdal) {
        return Err("--gdal-free cannot be combined with --backend gdal".into());
    }
    if gdal_free && cfg!(feature = "gdal") {
        return Err("--gdal-free requires a binary built without GDAL; rebuild with --no-default-features --features native-cog (or --no-default-features for Zarr only); selecting --backend native does not remove GDAL linkage".into());
    }
    if backend == Some(Backend::Native) && !cfg!(feature = "native-cog") {
        return Err("native COG backend is not compiled; rebuild with --no-default-features --features native-cog for GDAL-free acquisition, or add --features native-cog to a GDAL build".into());
    }
    if backend == Some(Backend::Gdal) && !cfg!(feature = "gdal") {
        return Err("GDAL COG backend is not compiled; build with --features gdal, or use --backend native with the native-cog feature".into());
    }
    Ok(())
}

/// Transport observations are keyed by asset, unlike the format-independent report.
pub fn transport_totals(transport: &Value) -> Option<(u64, u64)> {
    let objects = transport.as_object()?;
    if objects.is_empty() {
        return None;
    }
    objects
        .values()
        .try_fold((0u64, 0u64), |(requests, bytes), value| {
            Some((
                requests.checked_add(value["requests"].as_u64()?)?,
                bytes.checked_add(value["bytes"].as_u64()?)?,
            ))
        })
}

/// `--human` fetch report; the manifest in the output directory has the detail.
pub fn fetch_summary(records: usize, output: &str, transport: Option<(u64, u64)>) -> String {
    let http = if let Some((requests, bytes)) = transport {
        format!("Observed HTTP: {requests} requests, {bytes} response-body bytes (metadata and validation included).")
    } else {
        "Observed HTTP totals unavailable for this backend; manifest records measured stages and planned reads separately.".into()
    };
    format!("Acquisition complete: {records} output records.\nOutput: {output}\nCompletion and provenance: manifest.json\n{http}\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    fn arguments(extra: &[&str]) -> Vec<String> {
        let mut args = vec![
            "fetch",
            "--source",
            "https://example.org/test.zarr/",
            "--format",
            "zarr_v2",
            "--bbox",
            "10,30,25,50",
            "--start",
            "1959-01-02T00:00:00Z",
            "--end",
            "1959-01-02T06:00:00Z",
            "--variables",
            "2m_temperature",
        ];
        args.extend(extra);
        args.into_iter().map(str::to_owned).collect()
    }
    #[test]
    fn direct_flags_produce_shared_request_and_dry_run_never_fetches() {
        let options = parse(&arguments(&["--dry-run"])).unwrap().unwrap();
        assert_eq!(options.command, "plan");
        let request: tile_ripper::Request = serde_json::from_value(options.input).unwrap();
        assert!(matches!(request.source, tile_ripper::Source::Assets { .. }));
        assert_eq!(request.budget.buffer_bytes, 32 * 1024 * 1024);
        assert!(parse(&arguments(&[])).is_err());
        assert!(parse(&arguments(&["--output", "test", "--buffer-mib", "0"])).is_err());
        assert!(parse(&arguments(&["--output", "test", "--variables", "other"])).is_err());
    }
    #[test]
    fn streaming_and_resource_overrides_use_the_shared_request() {
        let mut args = arguments(&["--workers", "8", "--per-host", "2"]);
        args[0] = "stream".into();
        let options = parse(&args).unwrap().unwrap();
        assert_eq!(options.command, "stream");
        assert_eq!(options.workers, Some(8));
        assert_eq!(options.per_host, Some(2));
        assert!(parse(&arguments(&["--dry-run", "--workers", "0"])).is_err());
        assert!(parse(&arguments(&["--dry-run", "--per-host", "65"])).is_err());
    }
    #[test]
    #[cfg(not(feature = "gdal"))]
    fn gdal_free_direct_plan_defaults_to_callback_and_preserves_explicit_output() {
        let mut args = arguments(&["--gdal-free", "--collection", "sentinel-2-c1-l2a"]);
        args[0] = "plan".into();
        let index = args.iter().position(|v| v == "--format").unwrap();
        args[index + 1] = "stac".into();
        let options = parse(&args).unwrap().unwrap();
        let request: tile_ripper::Request = serde_json::from_value(options.input).unwrap();
        assert!(matches!(request.sink, tile_ripper::Sink::Callback));
        args.extend(["--output".into(), "explicit-directory".into()]);
        let options = parse(&args).unwrap().unwrap();
        let request: tile_ripper::Request = serde_json::from_value(options.input).unwrap();
        assert!(
            matches!(request.sink, tile_ripper::Sink::Directory { path } if path == "explicit-directory")
        );
        let options =
            parse(&["plan", "examples/unified-stac.json", "--gdal-free"].map(str::to_owned))
                .unwrap()
                .unwrap();
        let request: tile_ripper::Request = serde_json::from_value(options.input).unwrap();
        assert!(matches!(request.sink, tile_ripper::Sink::Directory { .. }));
    }
    #[test]
    fn catalog_flags_and_observed_transport_are_format_independent() {
        let mut args = arguments(&["--dry-run", "--collection", "sentinel-2-c1-l2a"]);
        let index = args.iter().position(|v| v == "--format").unwrap();
        args[index + 1] = "stac".into();
        let options = parse(&args).unwrap().unwrap();
        let request: tile_ripper::Request = serde_json::from_value(options.input).unwrap();
        assert!(matches!(request.source, tile_ripper::Source::Stac { .. }));
        assert!(matches!(
            request.variables[0],
            tile_ripper::Variable::Named { .. }
        ));
        assert_eq!(
            transport_totals(
                &json!({"one":{"requests":2,"bytes":100},"two":{"requests":3,"bytes":200}})
            ),
            Some((5, 300))
        );
        assert_eq!(transport_totals(&Value::Null), None);
    }
    #[test]
    fn malformed_selection_is_rejected_before_preparation() {
        for (key, value) in [
            ("--bbox", "NaN,2,3,4"),
            ("--bbox", "3,2,1,4"),
            ("--start", "yesterday"),
            ("--end", "1950-01-01T00:00:00Z"),
            ("--variables", "x,,y"),
            ("--format", "zarr_v3"),
        ] {
            let mut args = arguments(&["--dry-run"]);
            let pos = args.iter().position(|s| s == key).unwrap();
            args[pos + 1] = value.into();
            assert!(parse(&args).is_err(), "{key}");
        }
    }
}
