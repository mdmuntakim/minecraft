//! Fabric loader for Rust `.fabricmod` files. See `README.md`.

#![forbid(unsafe_code)]

pub mod capabilities;
pub mod catalog;
pub mod compat;
pub mod crash;
pub mod discovery;
pub mod entrypoints;
pub mod error;
pub mod hot_reload;
pub mod lifecycle;
pub mod lockfile;
pub mod manager;
pub mod modpack;
pub mod perf;
pub mod resolve;
pub mod settings;
pub mod snapshot;

pub use error::{LoaderError, LoaderResult};
