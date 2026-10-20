#[cfg(feature = "gdal")]
#[path = "diagnostics_gdal.rs"]
mod gdal_impl;
#[cfg(feature = "gdal")]
pub(crate) use gdal_impl::QuietNative;
#[cfg(not(feature = "gdal"))]
pub(crate) struct QuietNative;
#[cfg(not(feature = "gdal"))]
impl QuietNative {
    pub(crate) fn enter() -> Self {
        Self
    }
}
/// Opt-in benchmark stage trace, outside the output directory. Diagnostic failure
/// cannot change acquisition success. No source references or credentials logged.
pub(crate) fn profile_stage(stage: &str, seconds: f64) {
    use std::io::Write;
    if let Some(path) = std::env::var_os("TILE_RIPPER_STAGE_TRACE") {
        if let Ok(mut file) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
        {
            let _ = writeln!(
                file,
                "{}",
                serde_json::json!({"stage":stage,"seconds":seconds})
            );
        }
    }
}
