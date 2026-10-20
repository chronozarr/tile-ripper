//! Explainable acquisition planning. Locators are runtime inputs, never plan fields.
pub mod cog;
pub mod execute;
pub mod model;
pub mod planner;
pub mod zarr;
pub use execute::{execute, fetch, Chunk, ExecutionReport};
pub use model::*;
pub use planner::{plan, FormatPlanner};
pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;
pub(crate) fn fail<T>(message: &str) -> Result<T> {
    Err(message.to_owned().into())
}

pub mod spatial;
pub mod stac_workflow;

mod zarr_acquire;

pub mod acquisition;

mod diagnostics;

mod storage;
mod zarr_io;

mod zarr_decode;

mod pixels;
pub use pixels::Pixels;

mod scheduler;
pub mod stream;

#[cfg(feature = "native-cog")]
mod native_cog;
