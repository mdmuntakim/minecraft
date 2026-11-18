//! Cold discovery: finding `.fabricmod` files and development folders.
//!
//! Discovery is deliberately total: a broken mod becomes a row in
//! [`Discovery::failures`], it never stops the other mods from loading. The
//! budget is 150 ms for 100 mods on a mid-range arm64 device; the reader does
//! one linear pass over the directory and no range checks.

use crate::error::{ErrorCode, LoaderError, LoaderResult};
use fabric_mod_format::{
    Container, Limits,
    container::{EXTENSION, MANIFEST_ENTRY},
    migration,
};
use std::{collections::BTreeSet, path::PathBuf, time::Instant};

/// The budget for 100 cold mods, in milliseconds.
pub const COLD_BUDGET_MS: u128 = 150;

/// A mod found and read, ready for resolution.
#[derive(Clone, Debug)]
pub struct Discovered {
    pub container: Container,
    pub path: PathBuf,
    /// True when the mod is an unpacked folder rather than a `.fabricmod`.
    pub folder: bool,
}

impl Discovered {
    pub fn id(&self) -> &str {
        &self.container.manifest.id
    }

    pub fn version(&self) -> &str {
        &self.container.manifest.version
    }

    pub fn digest_hex(&self) -> String {
        fabric_mod_format::signature::hex(&self.container.digest)
    }
}

/// A mod that could not be read.
#[derive(Clone, Debug)]
pub struct DiscoveryFailure {
    pub path: PathBuf,
    pub error: LoaderError,
}

/// The result of a scan.
#[derive(Clone, Debug, Default)]
pub struct Discovery {
    pub mods: Vec<Discovered>,
    pub failures: Vec<DiscoveryFailure>,
    /// `.hmod` / `.hnmod` files waiting for the one-time migration.
    pub legacy: Vec<PathBuf>,
    pub elapsed_ms: f64,
}

impl Discovery {
    /// Whether the scan kept every mod.
    pub fn is_clean(&self) -> bool {
        self.failures.is_empty() && self.legacy.is_empty()
    }

    /// A one-line summary for the launcher's log.
    pub fn summary(&self) -> String {
        format!(
            "{} mod(s) discovered in {:.1} ms, {} failed, {} legacy",
            self.mods.len(),
            self.elapsed_ms,
            self.failures.len(),
            self.legacy.len()
        )
    }

    pub fn by_id(&self, id: &str) -> Option<&Discovered> {
        self.mods.iter().find(|m| m.id() == id)
    }

    /// The set of ids a mod set answers to, including `provides`.
    pub fn provided_ids(&self) -> BTreeSet<String> {
        self.mods
            .iter()
            .flat_map(|m| m.container.manifest.ids().map(str::to_owned))
            .collect()
    }
}

/// Scans `dir` for `.fabricmod` files and mod folders.
pub fn discover(dir: &std::path::Path) -> Discovery {
    discover_with(dir, Limits::default())
}

/// Scans with explicit caps, for the fuzz and limit tests.
pub fn discover_with(dir: &std::path::Path, limits: Limits) -> Discovery {
    let start = Instant::now();
    let mut discovery = Discovery::default();
    let Ok(read) = std::fs::read_dir(dir) else {
        discovery.elapsed_ms = start.elapsed().as_secs_f64() * 1000.0;
        return discovery;
    };
    let mut candidates: Vec<PathBuf> = read
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.extension()
                .and_then(|e| e.to_str())
                .is_some_and(|e| e.eq_ignore_ascii_case(EXTENSION))
                || (path.is_dir() && path.join(MANIFEST_ENTRY).is_file())
        })
        .collect();
    candidates.sort();
    for path in candidates {
        let folder = path.is_dir();
        let read = if folder {
            Container::read_folder(&path)
        } else {
            std::fs::read(&path)
                .map_err(|e| {
                    crate::error::error_with(
                        ErrorCode::FormatInvalid,
                        "the mod file cannot be read",
                        format!("{}: {e}", path.display()),
                    )
                })
                .and_then(|bytes| Container::read_with(&bytes, limits))
        };
        match read {
            Ok(container) => discovery.mods.push(Discovered {
                container,
                path,
                folder,
            }),
            Err(error) => discovery.failures.push(DiscoveryFailure { path, error }),
        }
    }
    discovery.mods.sort_by(|a, b| {
        a.id()
            .cmp(b.id())
            .then_with(|| a.version().cmp(b.version()))
    });
    discovery.legacy = migration::legacy_files(dir);
    discovery.elapsed_ms = start.elapsed().as_secs_f64() * 1000.0;
    discovery
}

/// Scans and fails when any mod is broken, for `--strict`.
pub fn discover_strict(dir: &std::path::Path) -> LoaderResult<Discovery> {
    let discovery = discover(dir);
    if let Some(failure) = discovery.failures.first() {
        return Err(failure.error.clone());
    }
    Ok(discovery)
}

#[cfg(test)]
mod tests {
    use super::*;
    use fabric_mod_format::{Container, File};
    use std::fs;

    fn manifest(id: &str, version: &str) -> String {
        format!(r#"{{"schemaVersion":2,"formatVersion":1,"id":"{id}","version":"{version}"}}"#)
    }

    fn write_mod(dir: &std::path::Path, id: &str, version: &str) -> PathBuf {
        let mut container = Container::new(&manifest(id, version)).unwrap();
        container.insert("assets/x.txt", b"x".to_vec()).unwrap();
        let path = dir.join(format!("{id}.{EXTENSION}"));
        fs::write(&path, container.to_bytes().unwrap()).unwrap();
        path
    }

    #[test]
    fn discovers_mods_and_sorts_them() {
        let dir = tempfile::tempdir().unwrap();
        write_mod(dir.path(), "zebra", "1.0.0");
        write_mod(dir.path(), "alpha", "1.0.0");
        let discovery = discover(dir.path());
        assert!(discovery.failures.is_empty(), "{:?}", discovery.failures);
        assert_eq!(
            discovery.mods.iter().map(|m| m.id()).collect::<Vec<_>>(),
            ["alpha", "zebra"]
        );
        assert!(discovery.summary().contains("2 mod(s)"));
    }

    #[test]
    fn a_broken_mod_does_not_hide_the_good_ones() {
        let dir = tempfile::tempdir().unwrap();
        write_mod(dir.path(), "good", "1.0.0");
        fs::write(dir.path().join("bad.fabricmod"), b"not a zip").unwrap();
        fs::write(dir.path().join("old.hmod"), b"legacy").unwrap();
        let discovery = discover(dir.path());
        assert_eq!(discovery.mods.len(), 1);
        assert_eq!(discovery.failures.len(), 1);
        assert_eq!(discovery.legacy.len(), 1);
        assert!(!discovery.is_clean());
        assert_eq!(
            discover_strict(dir.path()).unwrap_err().code,
            ErrorCode::FormatInvalid
        );
    }

    #[test]
    fn discovers_a_development_folder() {
        let dir = tempfile::tempdir().unwrap();
        let folder = dir.path().join("dev-mod");
        fs::create_dir_all(folder.join("assets")).unwrap();
        fs::write(folder.join(MANIFEST_ENTRY), manifest("dev", "0.1.0")).unwrap();
        fs::write(folder.join("assets/a.txt"), b"a").unwrap();
        let discovery = discover(dir.path());
        assert_eq!(discovery.mods.len(), 1);
        assert!(discovery.mods[0].folder);
        assert!(
            discovery.mods[0]
                .container
                .files
                .contains_key("assets/a.txt")
        );
    }

    #[test]
    fn a_missing_directory_is_an_empty_scan() {
        let discovery = discover(std::path::Path::new("/definitely/not/here"));
        assert!(discovery.mods.is_empty());
        assert!(discovery.failures.is_empty());
    }

    #[test]
    fn one_hundred_mods_are_one_linear_pass() {
        let dir = tempfile::tempdir().unwrap();
        for index in 0..100 {
            write_mod(dir.path(), &format!("mod-{index:03}"), "1.0.0");
        }
        let discovery = discover(dir.path());
        assert_eq!(discovery.mods.len(), 100);
        // A debug build on shared CI is several times slower than a release
        // build on the phone; the documented 150 ms budget is measured there.
        assert!(
            discovery.elapsed_ms < 5_000.0,
            "discovery took {:.1} ms",
            discovery.elapsed_ms
        );
    }

    #[test]
    fn file_caps_apply_to_discovery() {
        let dir = tempfile::tempdir().unwrap();
        write_mod(dir.path(), "big", "1.0.0");
        let limits = Limits {
            entries: 1,
            entry: 1,
            total: 1,
        };
        let discovery = discover_with(dir.path(), limits);
        assert!(discovery.mods.is_empty());
        assert_eq!(discovery.failures[0].error.code, ErrorCode::ZipLimit);
    }

    // Keep the unused import warning honest: File is part of the public API
    // this test touches through Container::insert.
    #[allow(dead_code)]
    fn _file_type(_: File) {}
}
