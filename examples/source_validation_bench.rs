//! Local-fixture consumer for matched validation lifecycle measurements.
//! Uses only existing public preparation/stream/checkpoint APIs.
#[cfg(all(feature = "native-hdf5", feature = "authenticated-http"))]
mod enabled {
    use serde_json::{json, Value};
    use std::{fs, sync::Arc, time::Instant};
    use tile_ripper::{
        acquisition, authenticated_http, checkpoint, cog::LayoutCache, swot, Pixels, Request,
        Result,
    };

    fn session(input: &Value) -> Result<Arc<authenticated_http::AuthenticatedHttp>> {
        let mut config: authenticated_http::Config =
            serde_json::from_value(input["policy"].clone())?;
        config
            .additional_roots
            .push(reqwest::Certificate::from_pem(&fs::read(
                input["ca"].as_str().ok_or("missing CA")?,
            )?)?);
        Ok(Arc::new(authenticated_http::AuthenticatedHttp::new(
            config,
            input["netrc"].as_str().ok_or("missing netrc")?,
        )?))
    }
    fn control(input: &Value, stage: &str) -> Result<()> {
        if let Some(url) = input["control"].as_str() {
            reqwest::blocking::get(format!("{url}/{stage}"))?.error_for_status()?;
        }
        Ok(())
    }
    fn fingerprint(chunk: &tile_ripper::Chunk) -> Value {
        let bytes: Vec<u8> = match &chunk.pixels {
            Pixels::Int8(v) => v.iter().map(|v| *v as u8).collect(),
            Pixels::UInt8(v) => v.clone(),
            Pixels::UInt16(v) => v.iter().flat_map(|v| v.to_le_bytes()).collect(),
            Pixels::Int16(v) => v.iter().flat_map(|v| v.to_le_bytes()).collect(),
            Pixels::UInt32(v) => v.iter().flat_map(|v| v.to_le_bytes()).collect(),
            Pixels::Float32(v) => v.iter().flat_map(|v| v.to_le_bytes()).collect(),
        };
        let mask: Vec<u8> = chunk.validity().map(u8::from).collect();
        let digest = |b: &[u8]| {
            ring::digest::digest(&ring::digest::SHA256, b)
                .as_ref()
                .iter()
                .map(|v| format!("{v:02x}"))
                .collect::<String>()
        };
        json!({"variable":chunk.variable,"indices":chunk.indices,"window":chunk.window,
            "coordinates":chunk.coordinates,"bytes":bytes.len(),"sha256":digest(&bytes),"mask_sha256":digest(&mask)})
    }
    pub fn run() -> Result<()> {
        let input: Value = serde_json::from_slice(&fs::read(
            std::env::args().nth(1).ok_or("input JSON required")?,
        )?)?;
        let request: Request = serde_json::from_value(input["request"].clone())?;
        let started = Instant::now();
        let mut cache = LayoutCache::default();
        let mut chunks = Vec::new();
        let mut first = None;
        if input["swot"].as_bool() == Some(true) {
            let policy = swot::Policy::cookbook();
            let prepared = if input["policy"].is_object() {
                let transport = session(&input)?;
                if let Some(path) = input["checkpoint"].as_str() {
                    swot::prepare_authenticated_checkpointed(
                        &request,
                        policy,
                        &mut cache,
                        transport,
                        checkpoint::Options::new(path),
                    )?
                } else if input["inspect_grid"].as_bool() == Some(true) {
                    let grid = swot::GridInspection::inspect_authenticated(&request, transport)?;
                    control(&input, "inspected")?;
                    grid.prepare(&request, policy)?
                } else {
                    swot::prepare_authenticated(&request, policy, &mut cache, transport)?
                }
            } else {
                swot::prepare(&request, policy, &mut cache)?
            };
            let prepare_seconds = started.elapsed().as_secs_f64();
            let inspection = prepared.explain();
            control(&input, "prepared")?;
            if input["sticky"].as_bool() == Some(true) {
                let first = prepared.clone().into_stream()?.finish();
                control(&input, "restore")?;
                let second = prepared.into_stream()?.finish();
                println!(
                    "{}",
                    json!({"first_failed":first.is_err(),"retry_failed":second.is_err()})
                );
                return Ok(());
            }
            let mut stream = prepared.into_stream()?;
            for tile in stream.by_ref() {
                let tile = tile?;
                first.get_or_insert_with(|| started.elapsed().as_secs_f64());
                for chunk in &tile.layers {
                    chunks.push(fingerprint(chunk));
                }
                if chunks.len() == 5 {
                    control(&input, "delivered")?;
                    if input["cancel"].as_bool() == Some(true) {
                        stream.cancel();
                        break;
                    }
                }
            }
            let report = stream.finish()?;
            println!(
                "{}",
                json!({"status":"complete","prepare_seconds":prepare_seconds,
                "first_chunk_seconds":first,"seconds":started.elapsed().as_secs_f64(),
                "chunks":chunks,"inspection":inspection,"observed":report})
            );
        } else {
            let prepared = acquisition::prepare(&request, &mut cache)?;
            let prepare_seconds = started.elapsed().as_secs_f64();
            let inspection = prepared.explain();
            control(&input, "prepared")?;
            if input["sticky"].as_bool() == Some(true) {
                let first = prepared.execute(|_| Ok(()));
                control(&input, "restore")?;
                let second = prepared.execute(|_| Ok(()));
                println!(
                    "{}",
                    json!({"first_failed":first.is_err(),"retry_failed":second.is_err()})
                );
                return Ok(());
            }
            let mut stream =
                prepared.into_stream(tile_ripper::stream::StreamOptions { capacity: 0 })?;
            for chunk in stream.by_ref() {
                let chunk = chunk?;
                first.get_or_insert_with(|| started.elapsed().as_secs_f64());
                chunks.push(fingerprint(&chunk));
                if chunks.len() == 1 {
                    control(&input, "delivered")?;
                    if input["cancel"].as_bool() == Some(true) {
                        stream.cancel();
                        break;
                    }
                }
            }
            let report = stream.finish()?;
            println!(
                "{}",
                json!({"status":"complete","prepare_seconds":prepare_seconds,
                "first_chunk_seconds":first,"seconds":started.elapsed().as_secs_f64(),
                "chunks":chunks,"inspection":inspection,"observed":report})
            );
        }
        Ok(())
    }
}
fn main() -> tile_ripper::Result<()> {
    #[cfg(all(feature = "native-hdf5", feature = "authenticated-http"))]
    {
        enabled::run()
    }
    #[cfg(not(all(feature = "native-hdf5", feature = "authenticated-http")))]
    {
        Err("requires native-hdf5,authenticated-http".into())
    }
}
