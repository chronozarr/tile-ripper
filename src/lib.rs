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
pub mod swot_discovery;

mod zarr_acquire;

pub mod acquisition;

mod diagnostics;

#[cfg(feature = "authenticated-http")]
pub mod authenticated_http;
#[cfg(all(feature = "native-hdf5", feature = "authenticated-http"))]
pub mod checkpoint;
mod http_transport;
mod storage;
mod zarr_io;

mod zarr_decode;

mod pixels;
pub use pixels::Pixels;

mod scheduler;
mod scientific_output;
pub use scientific_output::{ScientificEncoding, ScientificOutputOptions};
pub mod stream;

#[cfg(feature = "native-cog")]
mod native_cog;

mod zarr_ranges;
mod zarr_v3;

#[cfg(feature = "native-hdf5")]
mod hdf5;

#[cfg(feature = "native-hdf5")]
pub mod swot;
#[cfg(feature = "native-hdf5")]
pub mod swot_job;
