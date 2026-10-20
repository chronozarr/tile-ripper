//! CLI convenience syntax translates into the existing public Request schema.
use serde_json::{json, Value};
use std::{collections::BTreeMap, fs};
use tile_ripper::Result;
const HELP: &str = "tile-ripper plan|fetch|stream [request.json] [options]
  --source URL_OR_PATH --format zarr_v2|stac
  --collection NAME                 required for STAC
  --bbox WEST,SOUTH,EAST,NORTH       EPSG:4326; or --aoi polygon.geojson
  --start RFC3339 --end RFC3339      inclusive endpoints
  --variables NAME[,NAME]           native array names or STAC bands
  --output NEW_DIRECTORY            required for direct fetch
  --buffer-mib N                    payload budget (default 32; not RSS)
  --workers N --per-host N           concurrent reader operations (1..64)
  --human | --json                  JSON default, stable for scripts
  --dry-run                        fetch behaves as plan; metadata may download
  --cache FILE --discover-only      discover-only requires plan
JSON requests retain the full advanced schema. Do not mix selection flags and JSON.
stream emits NDJSON descriptors and completion; Rust streaming yields native values.
Supported profiles and installation: README.md. No reprojection or compositing.";
pub struct Options {
    pub command: String,
    pub input: Value,
    pub discover_only: bool,
    pub human: bool,
    pub cache_path: Option<String>,
    pub output: Option<String>,
    pub workers: Option<usize>,
    pub per_host: Option<usize>,
}
pub fn parse(args: &[String]) -> Result<Option<Options>> {
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
    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--human" => human = true,
            "--json" => human = false,
            "--dry-run" => dry = true,
            "--discover-only" => discover_only = true,
            key @ ("--source" | "--format" | "--collection" | "--bbox" | "--aoi" | "--start"
            | "--end" | "--variables" | "--buffer-mib" | "--output" | "--cache"
            | "--workers" | "--per-host") => {
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
    let input = if let Some(file) = file {
        if !flags.is_empty() {
            return Err("do not mix request JSON with source/selection flags; edit JSON or use direct flags".into());
        }
        let bytes =
            fs::read(file).map_err(|_| "cannot read request file; check path and permissions")?;
        serde_json::from_slice(&bytes)
            .map_err(|_| "invalid request JSON syntax; see examples/weatherbench2.json")?
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
        json!({"source":source,"spatial":spatial,"time":{"kind":"interval","start":start,"end":end},"variables":variables,"grid":{"kind":"native"},"sink":{"kind":"directory","path":output.as_deref().unwrap_or("subset")},"budget":{"buffer_bytes":bytes,"max_nodes":100000,"chunk_edge":512}})
    };
    Ok(Some(Options {
        command,
        input,
        discover_only,
        human,
        cache_path,
        output,
        workers,
        per_host,
    }))
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
