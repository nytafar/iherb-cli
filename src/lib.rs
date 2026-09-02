//! iHerb CLI, as a library.
//!
//! The binary in `src/main.rs` is a thin wrapper: it parses arguments and calls
//! [`run`]. Everything else lives here so integration tests under `tests/` can
//! exercise the code directly instead of shelling out to the built executable.

pub mod app;
pub mod batch;
pub mod browser;
pub mod cache;
pub mod cli;
pub mod config;
pub mod error;
pub mod fetch;
pub mod model;
pub mod output;
pub mod scraper;
pub mod targets;

pub use app::run;
pub use config::AppConfig;
pub use error::IherbError;
