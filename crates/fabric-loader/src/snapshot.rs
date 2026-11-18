//! Mod set snapshots: save a working set and restore it in one tap.

use crate::{
    error::{ErrorCode, LoaderResult, error_with},
    lockfile::{LockFile, LockedMod},
    resolve::Resolution,
};
use serde::{Deserialize, Serialize};
use std::{
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

/// A saved set.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Snapshot {
    pub name: String,
    /// Unix seconds.
    pub created: u64,
    pub mods: Vec<LockedMod>,
}

impl Snapshot {
    pub fn from_resolution(name: &str, resolution: &Resolution) -> Self {
        Self {
            name: name.to_owned(),
            created: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0),
            mods: LockFile::from_resolution(resolution).mods,
        }
    }

    pub fn ids(&self) -> Vec<&str> {
        self.mods.iter().map(|m| m.id.as_str()).collect()
    }
}

/// The snapshots next to the mods.
#[derive(Clone, Debug)]
pub struct SnapshotStore {
    dir: PathBuf,
}

impl SnapshotStore {
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    fn path(&self, name: &str) -> LoaderResult<PathBuf> {
        let safe = sanitize(name);
        if safe.is_empty() {
            return Err(error_with(
                ErrorCode::SnapshotMissing,
                "a snapshot needs a name",
                name.to_owned(),
            ));
        }
        Ok(self.dir.join(format!("{safe}.json")))
    }

    pub fn save(&self, snapshot: &Snapshot) -> LoaderResult<PathBuf> {
        let path = self.path(&snapshot.name)?;
        std::fs::create_dir_all(&self.dir).map_err(|e| {
            error_with(
                ErrorCode::SnapshotMissing,
                "the snapshot folder cannot be created",
                e.to_string(),
            )
        })?;
        let text = serde_json::to_string_pretty(snapshot).map_err(|e| {
            error_with(
                ErrorCode::SnapshotMissing,
                "the snapshot cannot be written",
                e.to_string(),
            )
        })?;
        std::fs::write(&path, text).map_err(|e| {
            error_with(
                ErrorCode::SnapshotMissing,
                "the snapshot cannot be written",
                e.to_string(),
            )
        })?;
        Ok(path)
    }

    pub fn list(&self) -> Vec<Snapshot> {
        let Ok(read) = std::fs::read_dir(&self.dir) else {
            return Vec::new();
        };
        let mut out: Vec<Snapshot> = read
            .flatten()
            .filter(|e| e.path().extension().is_some_and(|x| x == "json"))
            .filter_map(|e| std::fs::read_to_string(e.path()).ok())
            .filter_map(|text| serde_json::from_str(&text).ok())
            .collect();
        out.sort_by(|a, b| b.created.cmp(&a.created).then_with(|| a.name.cmp(&b.name)));
        out
    }

    pub fn load(&self, name: &str) -> LoaderResult<Snapshot> {
        let path = self.path(name)?;
        let text = std::fs::read_to_string(&path).map_err(|e| {
            error_with(
                ErrorCode::SnapshotMissing,
                "that snapshot does not exist",
                format!("{}: {e}", path.display()),
            )
        })?;
        serde_json::from_str(&text).map_err(|e| {
            error_with(
                ErrorCode::SnapshotMissing,
                "the snapshot is damaged",
                e.to_string(),
            )
        })
    }

    /// The mods a restore keeps enabled, and whether each is present.
    pub fn plan_restore(&self, name: &str, installed: &[String]) -> LoaderResult<RestorePlan> {
        let snapshot = self.load(name)?;
        let installed: std::collections::BTreeSet<&str> =
            installed.iter().map(String::as_str).collect();
        let enable: Vec<String> = snapshot
            .mods
            .iter()
            .filter(|m| installed.contains(m.id.as_str()))
            .map(|m| m.id.clone())
            .collect();
        let missing: Vec<String> = snapshot
            .mods
            .iter()
            .filter(|m| !installed.contains(m.id.as_str()))
            .map(|m| m.id.clone())
            .collect();
        Ok(RestorePlan {
            enable,
            disable: installed
                .iter()
                .filter(|id| !snapshot.mods.iter().any(|m| m.id == **id))
                .map(|id| (*id).to_owned())
                .collect(),
            missing,
        })
    }
}

/// What a rollback changes.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RestorePlan {
    pub enable: Vec<String>,
    pub disable: Vec<String>,
    pub missing: Vec<String>,
}

impl RestorePlan {
    pub fn is_complete(&self) -> bool {
        self.missing.is_empty()
    }
}

fn sanitize(name: &str) -> String {
    name.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == ' ' {
                c
            } else {
                '-'
            }
        })
        .collect::<String>()
        .trim()
        .to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::discovery::discover;
    use fabric_mod_format::container::{Container, EXTENSION};
    use std::fs;

    fn write_mods(dir: &Path) -> Resolution {
        for (id, version) in [("a", "1.0.0"), ("b", "2.0.0")] {
            let container =
                Container::new(&format!(r#"{{"id":"{id}","version":"{version}"}}"#)).unwrap();
            fs::write(
                dir.join(format!("{id}.{EXTENSION}")),
                container.to_bytes().unwrap(),
            )
            .unwrap();
        }
        let discovery = discover(dir);
        crate::resolve::resolve(&discovery).unwrap()
    }

    #[test]
    fn saves_lists_loads_and_plans_a_restore() {
        let dir = tempfile::tempdir().unwrap();
        let resolution = write_mods(dir.path());
        let store = SnapshotStore::new(dir.path().join("snapshots"));
        let snapshot = Snapshot::from_resolution("working", &resolution);
        store.save(&snapshot).unwrap();
        let list = store.list();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].ids(), ["a", "b"]);
        let loaded = store.load("working").unwrap();
        assert_eq!(loaded, snapshot);
        let plan = store
            .plan_restore("working", &["a".into(), "c".into()])
            .unwrap();
        assert_eq!(plan.enable, ["a"]);
        assert_eq!(plan.disable, ["c"]);
        assert_eq!(plan.missing, ["b"]);
        assert!(!plan.is_complete());
    }

    #[test]
    fn missing_snapshots_are_named() {
        let dir = tempfile::tempdir().unwrap();
        let store = SnapshotStore::new(dir.path());
        assert_eq!(
            store.load("nope").unwrap_err().code,
            ErrorCode::SnapshotMissing
        );
        let err = store.load("../../etc/passwd").unwrap_err();
        assert_eq!(err.code, ErrorCode::SnapshotMissing);
    }

    #[test]
    fn listing_an_absent_folder_is_empty() {
        let store = SnapshotStore::new("/definitely/not/here");
        assert!(store.list().is_empty());
    }
}
