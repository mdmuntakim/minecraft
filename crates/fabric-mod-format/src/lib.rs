//! The `.fabricmod` container format: manifest, constraints, signatures and
//! the one-time migration from `.hmod`/`.hnmod`.
//!
//! The crate is deliberately free of game code so it can be fuzzed and used by
//! the CLI, the launcher and the runtime alike.

#![forbid(unsafe_code)]

pub mod access;
pub mod container;
pub mod error;
pub mod manifest;
pub mod migration;
pub mod mixin;
pub mod signature;
pub mod version;

pub use container::{Container, File, Limits, FORMAT_VERSION, MIME};
pub use error::{ErrorCode, FabricError, Result};
pub use manifest::{Environment, Manifest, Requirement};
pub use signature::{Signature, Trust, TrustStore};
pub use version::{Version, VersionRange};

/// The crate's version, reported by `horizon-ctl fabric --version`.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Checks that every error code has documentation in this crate's README.
#[cfg(test)]
mod doc_tests {
    #[test]
    fn every_code_is_listed_in_the_readme() {
        let readme = include_str!("../README.md");
        for code in crate::error::ALL_CODES {
            assert!(
                readme.contains(code.as_str()),
                "{} is missing from the error table",
                code.as_str()
            );
        }
    }
}
