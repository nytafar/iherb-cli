//! The concrete things [`crate::fetch::fetch`] knows how to fetch.
//!
//! Each target is a descriptor: cache identity, URLs, extraction, validation.
//! A new command adds a module here; it does not repeat the pipeline.

pub mod product;
pub mod search;

pub use product::ProductTarget;
pub use search::SearchTarget;
