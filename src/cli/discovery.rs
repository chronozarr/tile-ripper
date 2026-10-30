use std::{collections::BTreeMap, fs::OpenOptions, io::Write};
use tile_ripper::{
    swot_discovery::{self as discovery, CmrClient, Completion, Query},
    Result,
};
const HELP: &str = r#"tile-ripper discover --collection SWOT_L2_HR_Raster_D --bbox WEST,SOUTH,EAST,NORTH
  --start RFC3339 --end RFC3339 --limit N [--output NEW_FILE.json]
  --concept-id EXPECTED_CMR_COLLECTION_ID     optional assertion, freshly resolved
  --page-size N --max-pages N --max-records N
  --response-kib N --total-kib N --timeout-seconds N --request-seconds N --retries N
  --cmr-fixture http://127.0.0.1:PORT/search/  frozen local catalog tests only
No credentials, archive access, science pixels or implicit native windows.
One bbox is limited to 30 degrees per axis / 400 square degrees; interval <=31 days.
Incomplete/failed searches write explicit status JSON and return nonzero.
discover-job SELECTION.json --auth earthdata --netrc PRIVATE_FILE --output NEW_JOB.json
  Uses the normal job planner and source-version checks; writes review JSON to stdout.
  Selection file: {"catalogs":["candidates.json"],"selection":{...}}.
  Per granule: exactly one explicit window or WGS84 aoi (bounds / Polygon).
  Geographic requests inspect actual source axes, preserve aoi_mask and pin the resolved indices.
See docs/swot-aoi/workflow.md, examples/swot-aoi-selection.json and the native-index
workflow in docs/swot-discovery/workflow.md."#;
pub fn run(args: &[String]) -> Result<()> {
    if args.is_empty() || args.iter().any(|s| s == "--help" || s == "-h") {
        println!("{HELP}");
        return Ok(());
    }
    let mut flags = BTreeMap::new();
    let mut chunks = args.chunks_exact(2);
    for pair in &mut chunks {
        if ![
            "--collection",
            "--concept-id",
            "--bbox",
            "--start",
            "--end",
            "--limit",
            "--output",
            "--page-size",
            "--max-pages",
            "--max-records",
            "--response-kib",
            "--total-kib",
            "--timeout-seconds",
            "--request-seconds",
            "--retries",
            "--cmr-fixture",
        ]
        .contains(&pair[0].as_str())
            || flags.insert(pair[0].as_str(), pair[1].as_str()).is_some()
        {
            return Err("unknown/duplicate discovery option; use discover --help (discovery accepts no authentication flags)".into());
        }
    }
    if !chunks.remainder().is_empty() {
        return Err("discovery option requires value".into());
    }
    let required = |k| {
        flags
            .get(k)
            .copied()
            .ok_or_else(|| format!("missing {k}; use discover --help"))
    };
    let bbox: Vec<f64> = required("--bbox")?
        .split(',')
        .map(str::parse)
        .collect::<std::result::Result<_, _>>()
        .map_err(|_| "invalid WGS84 bbox")?;
    let mut q = Query {
        collection: required("--collection")?.into(),
        collection_concept_id: flags.get("--concept-id").map(|s| s.to_string()),
        bbox: bbox
            .try_into()
            .map_err(|_| "bbox requires exactly four numbers")?,
        start: required("--start")?.into(),
        end: required("--end")?.into(),
        limit: required("--limit")?
            .parse()
            .map_err(|_| "limit requires an integer")?,
        limits: Default::default(),
    };
    for (name, value) in [
        ("--page-size", &mut q.limits.page_size),
        ("--max-pages", &mut q.limits.max_pages),
        ("--max-records", &mut q.limits.max_records),
        ("--response-kib", &mut q.limits.max_response_bytes),
        ("--total-kib", &mut q.limits.max_total_bytes),
        ("--retries", &mut q.limits.retries),
    ] {
        if let Some(v) = flags.get(name) {
            *value = v
                .parse::<usize>()
                .map_err(|_| "invalid discovery limit")?
                .checked_mul(if name.ends_with("kib") { 1024 } else { 1 })
                .ok_or("discovery limit overflow")?;
        }
    }
    if let Some(v) = flags.get("--timeout-seconds") {
        q.limits.timeout_seconds = v.parse().map_err(|_| "invalid query timeout")?;
    }
    if let Some(v) = flags.get("--request-seconds") {
        q.limits.request_timeout_seconds = v.parse().map_err(|_| "invalid request timeout")?;
    }
    q.validate()?;
    let client = if let Some(url) = flags.get("--cmr-fixture") {
        CmrClient::loopback_fixture(url)?
    } else {
        CmrClient::new()?
    };
    let report = client.discover(&q)?;
    let bytes = serde_json::to_vec_pretty(&report)?;
    if bytes.len() > discovery::MAX_REPORT_BYTES {
        return Err("discovery report exceeds bounded output allowance".into());
    }
    if let Some(path) = flags.get("--output") {
        write_new(path, &bytes)?;
    } else {
        std::io::stdout().lock().write_all(&bytes)?;
        println!();
    }
    if report.status != Completion::Complete {
        return Err("discovery was incomplete; review status/reasons and narrow or explicitly adjust bounded query; results are not a complete catalog set".into());
    }
    Ok(())
}
fn write_new(path: &str, bytes: &[u8]) -> Result<()> {
    let mut f = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|_| "cannot create new output file; choose a fresh path")?;
    if let Err(e) = f.write_all(bytes).and_then(|_| f.sync_all()) {
        drop(f);
        let _ = std::fs::remove_file(path);
        return Err(e.into());
    }
    Ok(())
}
#[cfg(all(feature = "native-hdf5", feature = "authenticated-http"))]
pub fn job(args: &[String]) -> Result<()> {
    use std::{fs::File, io::Read};
    use tile_ripper::swot_discovery::bridge::{self, JobSelection};
    if args.is_empty() || args.iter().any(|s| s == "--help" || s == "-h") {
        println!("{HELP}");
        return Ok(());
    }
    let mut translated = vec!["plan".into()];
    translated.extend_from_slice(args);
    let options = super::parse_with_input_limit(&translated, 64 * 1024)
        .map_err(crate::argument_error)?
        .ok_or("selection required")?;
    if options.discover_only
        || options.human
        || options.cache_path.is_some()
        || options.checkpoint.is_some()
        || options.quality_policy.is_some()
        || options.values
        || options.workers.is_some()
        || options.per_host.is_some()
        || options.backend.is_some()
        || options.output_format.is_some()
        || options.gdal_free
    {
        return Err("discover-job accepts only selection JSON, explicit authentication flags and --output NEW_JOB.json; use normal plan/fetch for checkpoints".into());
    }
    if serde_json::to_vec(&options.input)?.len() > 64 * 1024 {
        return Err("discovery selection exceeds 64 KiB".into());
    }
    #[derive(serde::Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Input {
        catalogs: Vec<String>,
        selection: JobSelection,
    }
    let input: Input = serde_json::from_value(options.input).map_err(|_| {
        "invalid explicit discovery selection; see examples/swot-discovery-selection.json"
    })?;
    if !(1..=4).contains(&input.catalogs.len()) {
        return Err("choose one to four catalog files".into());
    }
    let output = options
        .output
        .ok_or("discover-job requires --output NEW_JOB.json")?;
    if std::fs::symlink_metadata(&output).is_ok() {
        return Err("job output exists; choose a fresh path".into());
    }
    let access=options.access.ok_or("discover-job requires explicit --auth and --netrc (discovery alone does not authorize source inspection)")?;
    let mut reports = vec![];
    for file in input.catalogs {
        let mut bytes = vec![];
        File::open(file)
            .map_err(|_| "cannot read discovery report")?
            .take(discovery::MAX_REPORT_BYTES as u64 + 1)
            .read_to_end(&mut bytes)?;
        reports.push(discovery::Report::from_json(&bytes)?);
    }
    let draft = bridge::draft(&reports, &input.selection)?;
    // Native inspection retains only the small selected manifest/provenance,
    // never the caller's candidate lists alongside an active decoder.
    drop(reports);
    let mut sessions = |asset: &tile_ripper::Asset| {
        crate::job_session(asset, &access, input.selection.observations.len())
    };
    let ready = draft.inspect_authenticated(&mut sessions)?;
    let Some(manifest) = &ready.manifest else {
        let review = serde_json::to_vec_pretty(&ready.review)?;
        std::io::stdout().lock().write_all(&review)?;
        println!();
        return Err("selected candidate has zero native AOI centers; review emitted, no job manifest written".into());
    };
    let bytes = serde_json::to_vec_pretty(manifest)?;
    if bytes.len() > 64 * 1024 {
        return Err("generated job exceeds manifest input allowance".into());
    }
    let review = serde_json::to_vec_pretty(&ready.review)?;
    if review.len() > 3 * 1024 * 1024 {
        return Err("job review exceeds 3 MiB bounded output allowance".into());
    }
    write_new(&output, &bytes)?;
    std::io::stdout().lock().write_all(&review)?;
    println!();
    Ok(())
}
