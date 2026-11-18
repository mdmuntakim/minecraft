//! `fabric.lock`: a reproducible record of the resolved mod set.
//!
//! The lockfile stores every mod's id, version, digest and aliases, sorted by
//! id. `--frozen` refuses a set that differs in any of them, so a build or a
//! server cannot silently pick up a changed file.

use crate::{
    discovery::Discovery,
    error::{ErrorCode, LoaderResult, error_with},
    resolve::Resolution,
};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// The lockfile's name beside the mods.
pub const LOCKFILE: &str = "fabric.lock";
/// The lockfile format this build writes.
pub const LOCK_VERSION: u32 = 1;

/// One locked mod.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LockedMod {
    pub id: String,
    pub version: String,
    pub sha256: String,
    #[serde(default)]
    pub provides: Vec<String>,
}

/// The whole lockfile.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LockFile {
    pub version: u32,
    pub loader: String,
    pub mods: Vec<LockedMod>,
}

impl LockFile {
    /// Builds the lockfile for a resolution. Sorted by id, no duplicates.
    pub fn from_resolution(resolution: &Resolution) -> Self {
        let mods = resolution
            .mods
            .values()
            .map(|mod_| LockedMod {
                id: mod_.id.clone(),
                version: mod_.version.clone(),
                sha256: mod_.sha256.clone(),
                provides: mod_.provides.clone(),
            })
            .collect();
        Self {
            version: LOCK_VERSION,
            loader: fabric_mod_format::VERSION.to_owned(),
            mods,
        }
    }

    /// Builds the lockfile for an already resolved discovery.
    pub fn for_discovery(discovery: &Discovery) -> LoaderResult<Self> {
        Ok(Self::from_resolution(&crate::resolve::resolve(discovery)?))
    }

    /// Identical mod sets, regardless of file names.
    pub fn check(&self, resolution: &Resolution) -> LoaderResult<()> {
        if self.version != LOCK_VERSION {
            return Err(error_with(
                ErrorCode::LockfileMismatch,
                "fabric.lock was written by a different loader",
                format!("lock version {}, expected {LOCK_VERSION}", self.version),
            ));
        }
        let locked: std::collections::BTreeMap<&str, &LockedMod> =
            self.mods.iter().map(|m| (m.id.as_str(), m)).collect();
        if locked.len() != self.mods.len() {
            return Err(error_with(
                ErrorCode::LockfileMismatch,
                "fabric.lock lists the same mod twice",
                "",
            ));
        }
        for (id, resolved) in &resolution.mods {
            let Some(entry) = locked.get(id.as_str()) else {
                return Err(error_with(
                    ErrorCode::LockfileMismatch,
                    format!("{id} is installed but is not in fabric.lock"),
                    resolved.path.display().to_string(),
                ));
            };
            if entry.version != resolved.version || entry.sha256 != resolved.sha256 {
                return Err(error_with(
                    ErrorCode::LockfileMismatch,
                    format!("{id} differs from fabric.lock"),
                    format!(
                        "locked {} {} ({}), installed {} {} ({})",
                        id, entry.version, entry.sha256, id, resolved.version, resolved.sha256
                    ),
                ));
            }
        }
        for id in locked.keys() {
            if !resolution.mods.contains_key(*id) {
                return Err(error_with(
                    ErrorCode::LockfileMismatch,
                    format!("{id} is in fabric.lock but is not installed"),
                    "",
                ));
            }
        }
        Ok(())
    }

    pub fn to_json(&self) -> LoaderResult<String> {
        serde_json::to_string_pretty(self).map_err(|e| {
            error_with(
                ErrorCode::LockfileMismatch,
                "fabric.lock cannot be written",
                e.to_string(),
            )
        })
    }

    pub fn read(path: &Path) -> LoaderResult<Self> {
        let text = std::fs::read_to_string(path).map_err(|e| {
            error_with(
                ErrorCode::LockfileMismatch,
                "fabric.lock cannot be read",
                format!("{}: {e}", path.display()),
            )
        })?;
        serde_json::from_str(&text).map_err(|e| {
            error_with(
                ErrorCode::LockfileMismatch,
                "fabric.lock is not valid",
                e.to_string(),
            )
        })
    }

    pub fn write(&self, path: &Path) -> LoaderResult<()> {
        std::fs::write(path, self.to_json()?).map_err(|e| {
            error_with(
                ErrorCode::LockfileMismatch,
                "fabric.lock cannot be written",
                format!("{}: {e}", path.display()),
            )
        })
    }

    /// Checks a lockfile beside the mods, or fails when `--frozen` needs one.
    pub fn enforce(dir: &Path, resolution: &Resolution, frozen: bool) -> LoaderResult<()> {
        let path = dir.join(LOCKFILE);
        if path.is_file() {
            return Self::read(&path)?.check(resolution);
        }
        if frozen {
            return Err(error_with(
                ErrorCode::LockfileMismatch,
                "--frozen was requested but there is no fabric.lock",
                PathBuf::from(&path).display().to_string(),
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::discovery::discover;
    use fabric_mod_format::container::{Container, EXTENSION};
    use std::fs;

    fn mods(dir: &Path, id: &str, version: &str, extra: &str) {
        let text = format!(
            r#"{{"schemaVersion":2,"formatVersion":1,"id":"{id}","version":"{version}"{extra}}}"#
        );
        let container = Container::new(&text).unwrap();
        fs::write(
            dir.join(format!("{id}.{EXTENSION}")),
            container.to_bytes().unwrap(),
        )
        .unwrap();
    }

    #[test]
    fn a_lockfile_round_trips_and_catches_changes() {
        let dir = tempfile::tempdir().unwrap();
        mods(dir.path(), "a", "1.0.0", "");
        mods(dir.path(), "b", "2.0.0", r#","provides":["alias"]"#);
        let discovery = discover(dir.path());
        let lock = LockFile::for_discovery(&discovery).unwrap();
        assert_eq!(
            lock.mods.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(),
            ["a", "b"]
        );
        assert_eq!(lock.mods[1].provides, ["alias"]);
        let path = dir.path().join(LOCKFILE);
        lock.write(&path).unwrap();
        let read = LockFile::read(&path).unwrap();
        assert_eq!(read, lock);
        let resolution = crate::resolve::resolve(&discovery).unwrap();
        read.check(&resolution).unwrap();
        LockFile::enforce(dir.path(), &resolution, true).unwrap();

        // Change one byte of one mod: the lockfile notices.
        let mut container = Container::read(&dir.path().join("a.fabricmod")).unwrap();
        container.insert("assets/new.txt", b"x".to_vec()).unwrap();
        container.write(&dir.path().join("a.fabricmod")).unwrap();
        let changed = discover(dir.path());
        let resolution = crate::resolve::resolve(&changed).unwrap();
        assert_eq!(
            read.check(&resolution).unwrap_err().code,
            ErrorCode::LockfileMismatch
        );
    }

    #[test]
    fn frozen_without_a_lockfile_fails() {
        let dir = tempfile::tempdir().unwrap();
        mods(dir.path(), "a", "1.0.0", "");
        let discovery = discover(dir.path());
        let resolution = crate::resolve::resolve(&discovery).unwrap();
        assert_eq!(
            LockFile::enforce(dir.path(), &resolution, true)
                .unwrap_err()
                .code,
            ErrorCode::LockfileMismatch
        );
        LockFile::enforce(dir.path(), &resolution, false).unwrap();
    }

    #[test]
    fn a_missing_or_extra_mod_is_named() {
        let dir = tempfile::tempdir().unwrap();
        mods(dir.path(), "a", "1.0.0", "");
        let discovery = discover(dir.path());
        let lock = LockFile::for_discovery(&discovery).unwrap();
        let empty =
            crate::resolve::resolve(&discover(tempfile::tempdir().unwrap().path())).unwrap();
        let err = lock.check(&empty).unwrap_err();
        assert!(err.message.contains('a'));
    }
}
