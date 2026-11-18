//! Hot reload of data and resource mods.
//!
//! A `.fabricmod` with only `assets/` and `data/` is swapped in place. A mod
//! with a native library cannot be reloaded without a restart, and the result
//! says so instead of pretending.

use crate::{discovery::Discovery, error::LoaderResult};
use fabric_mod_format::Container;
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

/// What a reload needs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reload {
    /// Nothing in the file changed.
    None,
    /// Data and resources can be reloaded now.
    Resources,
    /// The native library changed; a restart is required.
    RestartRequired,
}

/// A change seen on disk.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Change {
    pub path: PathBuf,
    pub reload: Reload,
}

/// Watches the mods folder for changed files by digest.
#[derive(Clone, Debug, Default)]
pub struct Watcher {
    seen: BTreeMap<PathBuf, [u8; 32]>,
}

impl Watcher {
    pub fn new() -> Self {
        Self::default()
    }

    /// The initial snapshot: changes reported after this are real.
    pub fn prime(&mut self, discovery: &Discovery) {
        for discovered in &discovery.mods {
            self.seen
                .insert(discovered.path.clone(), hash(&discovered.container));
        }
    }

    /// Scans for new, changed and removed mods. Code changes require a
    /// restart; everything else can reload in place.
    pub fn poll(&mut self, dir: &Path) -> Vec<Change> {
        let discovery = crate::discovery::discover(dir);
        let mut changes = Vec::new();
        let mut present = BTreeMap::new();
        for discovered in &discovery.mods {
            let digest = hash(&discovered.container);
            present.insert(discovered.path.clone(), digest);
            match self.seen.get(&discovered.path) {
                Some(previous) if previous == &digest => continue,
                Some(_) => {
                    // The same path changed. A native library means restart;
                    // a data-only change can be applied live.
                    let code = discovered.container.native_library().is_some();
                    changes.push(Change {
                        path: discovered.path.clone(),
                        reload: if code {
                            Reload::RestartRequired
                        } else {
                            Reload::Resources
                        },
                    });
                }
                None => {
                    // A new mod: a code mod loads at the next restart, a data
                    // mod can be applied now.
                    let code = discovered.container.native_library().is_some();
                    changes.push(Change {
                        path: discovered.path.clone(),
                        reload: if code {
                            Reload::RestartRequired
                        } else {
                            Reload::Resources
                        },
                    });
                }
            }
        }
        for path in self.seen.keys() {
            if !present.contains_key(path) {
                changes.push(Change {
                    path: path.clone(),
                    reload: Reload::Resources,
                });
            }
        }
        self.seen = present;
        changes.sort_by(|a, b| a.path.cmp(&b.path));
        changes
    }

    /// Whether a specific container can be reloaded live.
    pub fn classify(container: &Container) -> Reload {
        if container.native_library().is_some() {
            Reload::RestartRequired
        } else {
            Reload::Resources
        }
    }
}

fn hash(container: &Container) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(container.digest);
    hasher.finalize().into()
}

/// Reads a data mod's resources for a live reload, in a deterministic order.
pub fn resources(container: &Container) -> Vec<(&str, &[u8])> {
    let mut files: Vec<(&str, &[u8])> = container
        .files
        .values()
        .filter(|file| file.name.starts_with("assets/") || file.name.starts_with("data/"))
        .map(|file| (file.name.as_str(), file.data.as_slice()))
        .collect();
    files.sort_by(|a, b| a.0.cmp(b.0));
    files
}

/// Reloads every data mod and reports what needs a restart.
pub fn reload(discovery: &Discovery) -> LoaderResult<ReloadBatch> {
    let mut batch = ReloadBatch::default();
    for discovered in &discovery.mods {
        if discovered.container.native_library().is_some() {
            batch.restart.push(discovered.id().to_owned());
        } else {
            batch.reloaded.push((
                discovered.id().to_owned(),
                resources(&discovered.container).len(),
            ));
        }
    }
    Ok(batch)
}

/// The result of one reload pass.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ReloadBatch {
    /// (mod id, resource count)
    pub reloaded: Vec<(String, usize)>,
    pub restart: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use fabric_mod_format::container::{Container, EXTENSION};
    use std::fs;

    fn write(dir: &Path, id: &str, extra: &str) -> PathBuf {
        let text = format!(r#"{{"id":"{id}","version":"1.0.0"{extra}}}"#);
        let mut container = Container::new(&text).unwrap();
        container.insert("assets/a.txt", b"a".to_vec()).unwrap();
        let path = dir.join(format!("{id}.{EXTENSION}"));
        fs::write(&path, container.to_bytes().unwrap()).unwrap();
        path
    }

    #[test]
    fn a_data_change_reloads_and_a_code_change_restarts() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "data", "");
        let mut watcher = Watcher::new();
        watcher.prime(&crate::discovery::discover(dir.path()));
        assert!(watcher.poll(dir.path()).is_empty());

        let mut container = Container::read(&dir.path().join("data.fabricmod")).unwrap();
        container
            .insert("assets/a.txt", b"changed".to_vec())
            .unwrap();
        container.write(&dir.path().join("data.fabricmod")).unwrap();
        let changes = watcher.poll(dir.path());
        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].reload, Reload::Resources);

        let mut code = Container::new(
            r#"{"id":"code","version":"1.0.0","custom":{"horizon":{"native":"lib/arm64-v8a/libcode.so"}}}"#,
        )
        .unwrap();
        code.insert("lib/arm64-v8a/libcode.so", vec![0; 4]).unwrap();
        fs::write(dir.path().join("code.fabricmod"), code.to_bytes().unwrap()).unwrap();
        let changes = watcher.poll(dir.path());
        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].reload, Reload::RestartRequired);
    }

    #[test]
    fn removed_mods_are_reported() {
        let dir = tempfile::tempdir().unwrap();
        let path = write(dir.path(), "a", "");
        let mut watcher = Watcher::new();
        watcher.prime(&crate::discovery::discover(dir.path()));
        fs::remove_file(path).unwrap();
        let changes = watcher.poll(dir.path());
        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].reload, Reload::Resources);
    }

    #[test]
    fn the_batch_keeps_code_mods_for_a_restart() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "data", "");
        let mut code = Container::new(
            r#"{"id":"code","version":"1.0.0","custom":{"horizon":{"native":"lib/arm64-v8a/libcode.so"}}}"#,
        )
        .unwrap();
        code.insert("lib/arm64-v8a/libcode.so", vec![0; 4]).unwrap();
        code.write(&dir.path().join("code.fabricmod")).unwrap();
        let batch = reload(&crate::discovery::discover(dir.path())).unwrap();
        assert_eq!(batch.reloaded, [("data".to_owned(), 1)]);
        assert_eq!(batch.restart, ["code"]);
    }
}
