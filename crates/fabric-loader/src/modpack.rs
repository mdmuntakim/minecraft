//! `.fabricpack`: a modpack that carries mods, configs and the lockfile, so a
//! one-tap import reproduces a working set.

use crate::{
    discovery::discover,
    error::{ErrorCode, LoaderResult, error_with},
    lockfile::{LOCKFILE, LockFile, LockedMod},
    resolve::resolve,
};
use fabric_mod_format::container::{EXTENSION, canonical_name};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    fs,
    io::{Cursor, Read, Write},
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

/// The pack's manifest.
pub const PACK_MANIFEST: &str = "fabricpack.json";
/// The pack's extension.
pub const PACK_EXTENSION: &str = "fabricpack";
/// The pack format.
pub const PACK_FORMAT: u32 = 1;

/// `fabricpack.json`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackManifest {
    pub format_version: u32,
    pub name: String,
    pub version: String,
    pub created: u64,
    pub mods: Vec<LockedMod>,
}

/// What packing produced.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PackReport {
    pub path: PathBuf,
    pub mods: usize,
    pub configs: usize,
    pub bytes: u64,
}

/// Packs `mods_dir` (and `config_dir`, when present) into `out`.
pub fn pack(
    mods_dir: &Path,
    config_dir: Option<&Path>,
    out: &Path,
    name: &str,
    version: &str,
) -> LoaderResult<PackReport> {
    let discovery = discover(mods_dir);
    if let Some(failure) = discovery.failures.first() {
        return Err(failure.error.clone());
    }
    if discovery.mods.is_empty() {
        return Err(error_with(
            ErrorCode::ModpackInvalid,
            "there are no .fabricmod files to pack",
            mods_dir.display().to_string(),
        ));
    }
    let resolution = resolve(&discovery)?;
    let lock = LockFile::from_resolution(&resolution);
    let manifest = PackManifest {
        format_version: PACK_FORMAT,
        name: name.to_owned(),
        version: version.to_owned(),
        created: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0),
        mods: lock.mods.clone(),
    };
    let manifest_text = serde_json::to_string_pretty(&manifest).map_err(|e| {
        error_with(
            ErrorCode::ModpackInvalid,
            "the pack manifest cannot be written",
            e.to_string(),
        )
    })?;

    let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
    let options = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Stored)
        .last_modified_time(zip::DateTime::default());
    let err = |e: zip::result::ZipError| {
        error_with(
            ErrorCode::ModpackInvalid,
            "the pack cannot be written",
            e.to_string(),
        )
    };
    let mut put = |name: &str, data: &[u8]| -> LoaderResult<()> {
        canonical_name(name)?;
        zip.start_file(name, options).map_err(err)?;
        zip.write_all(data).map_err(|e| {
            error_with(
                ErrorCode::ModpackInvalid,
                "the pack cannot be written",
                e.to_string(),
            )
        })
    };
    put(PACK_MANIFEST, manifest_text.as_bytes())?;
    put(LOCKFILE, lock.to_json()?.as_bytes())?;
    let mut mods = 0;
    for discovered in &discovery.mods {
        let bytes = if discovered.folder {
            discovered.container.to_bytes()?
        } else {
            fs::read(&discovered.path).map_err(|e| {
                error_with(
                    ErrorCode::ModpackInvalid,
                    "a mod cannot be read",
                    e.to_string(),
                )
            })?
        };
        put(&format!("mods/{}.{}", discovered.id(), EXTENSION), &bytes)?;
        mods += 1;
    }
    let mut configs = 0;
    if let Some(config_dir) = config_dir
        && config_dir.is_dir()
    {
        for (name, data) in read_tree(config_dir)? {
            put(&format!("config/{name}"), &data)?;
            configs += 1;
        }
    }
    let bytes = zip.finish().map_err(err)?.into_inner();
    if let Some(parent) = out.parent() {
        fs::create_dir_all(parent).map_err(|e| {
            error_with(
                ErrorCode::ModpackInvalid,
                "the pack folder cannot be created",
                e.to_string(),
            )
        })?;
    }
    fs::write(out, &bytes).map_err(|e| {
        error_with(
            ErrorCode::ModpackInvalid,
            "the pack cannot be written",
            e.to_string(),
        )
    })?;
    Ok(PackReport {
        path: out.to_path_buf(),
        mods,
        configs,
        bytes: bytes.len() as u64,
    })
}

/// A read pack.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ModpackArchive {
    pub manifest: Option<PackManifest>,
    pub lock: Option<String>,
    /// File name inside `mods/` -> bytes.
    pub mods: BTreeMap<String, Vec<u8>>,
    /// Path inside `config/` -> bytes.
    pub configs: BTreeMap<String, Vec<u8>>,
}

impl ModpackArchive {
    pub fn read(path: &Path) -> LoaderResult<Self> {
        let bytes = fs::read(path).map_err(|e| {
            error_with(
                ErrorCode::ModpackInvalid,
                "the pack cannot be read",
                e.to_string(),
            )
        })?;
        Self::read_bytes(&bytes)
    }

    pub fn read_bytes(bytes: &[u8]) -> LoaderResult<Self> {
        if bytes.len() < 4 || &bytes[..4] != b"PK\x03\x04" {
            return Err(error_with(
                ErrorCode::ModpackInvalid,
                "this is not a .fabricpack",
                "",
            ));
        }
        let mut zip = zip::ZipArchive::new(Cursor::new(bytes)).map_err(|e| {
            error_with(
                ErrorCode::ModpackInvalid,
                "the pack is damaged",
                e.to_string(),
            )
        })?;
        if zip.len() > 8192 {
            return Err(error_with(
                ErrorCode::ModpackInvalid,
                "the pack has too many entries",
                "",
            ));
        }
        let mut archive = Self::default();
        let mut total = 0u64;
        for index in 0..zip.len() {
            let mut entry = zip.by_index(index).map_err(|e| {
                error_with(
                    ErrorCode::ModpackInvalid,
                    "a pack entry cannot be read",
                    e.to_string(),
                )
            })?;
            if entry.is_dir() {
                continue;
            }
            let name = entry
                .name()
                .map_err(|e| {
                    error_with(
                        ErrorCode::ModpackInvalid,
                        "a pack entry name is invalid",
                        e.to_string(),
                    )
                })?
                .into_owned();
            let name = canonical_name(&name)?;
            let mut data = Vec::new();
            entry
                .by_ref()
                .take((256u64 << 20) + 1)
                .read_to_end(&mut data)
                .map_err(|e| {
                    error_with(
                        ErrorCode::ModpackInvalid,
                        "a pack entry is damaged",
                        e.to_string(),
                    )
                })?;
            total = total.saturating_add(data.len() as u64);
            if total > 512 << 20 {
                return Err(error_with(
                    ErrorCode::ModpackInvalid,
                    "the pack is too large",
                    "",
                ));
            }
            match name.as_str() {
                PACK_MANIFEST => {
                    archive.manifest = Some(serde_json::from_slice(&data).map_err(|e| {
                        error_with(
                            ErrorCode::ModpackInvalid,
                            "fabricpack.json is not valid",
                            e.to_string(),
                        )
                    })?);
                }
                LOCKFILE => {
                    archive.lock = String::from_utf8(data).ok();
                }
                _ if name.starts_with("mods/") => {
                    if !name.ends_with(&format!(".{EXTENSION}")) {
                        return Err(error_with(
                            ErrorCode::ModpackInvalid,
                            "a pack holds a mod that is not a .fabricmod",
                            name,
                        ));
                    }
                    archive.mods.insert(name["mods/".len()..].to_owned(), data);
                }
                _ if name.starts_with("config/") => {
                    archive
                        .configs
                        .insert(name["config/".len()..].to_owned(), data);
                }
                _ => {
                    // Unknown top-level entries are preserved, not rejected.
                }
            }
        }
        if archive
            .manifest
            .as_ref()
            .is_some_and(|m| m.format_version > PACK_FORMAT)
        {
            return Err(error_with(
                ErrorCode::FormatTooNew,
                "the pack is newer than this build",
                "",
            ));
        }
        Ok(archive)
    }

    /// Writes the mods to `mods_dir` and the configs to `config_dir`.
    pub fn import(&self, mods_dir: &Path, config_dir: &Path) -> LoaderResult<ImportReport> {
        let same_mods = same_path(mods_dir, config_dir);
        let mut report = ImportReport::default();
        for (name, data) in &self.mods {
            let target = mods_dir.join(name);
            if target.exists() {
                let existing = fs::read(&target).unwrap_or_default();
                if existing == *data {
                    report.unchanged += 1;
                    continue;
                }
                report.skipped.push(name.clone());
                continue;
            }
            fs::create_dir_all(mods_dir).map_err(|e| {
                error_with(
                    ErrorCode::ModpackInvalid,
                    "the mods folder cannot be created",
                    e.to_string(),
                )
            })?;
            fs::write(&target, data).map_err(|e| {
                error_with(
                    ErrorCode::ModpackInvalid,
                    "a mod cannot be written",
                    e.to_string(),
                )
            })?;
            report.written += 1;
        }
        if !same_mods {
            for (name, data) in &self.configs {
                let target = config_dir.join(name);
                if let Some(parent) = target.parent() {
                    fs::create_dir_all(parent).map_err(|e| {
                        error_with(
                            ErrorCode::ModpackInvalid,
                            "the config folder cannot be created",
                            e.to_string(),
                        )
                    })?;
                }
                fs::write(&target, data).map_err(|e| {
                    error_with(
                        ErrorCode::ModpackInvalid,
                        "a config cannot be written",
                        e.to_string(),
                    )
                })?;
                report.configs += 1;
            }
        }
        Ok(report)
    }
}

/// What an import wrote.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ImportReport {
    pub written: usize,
    pub unchanged: usize,
    pub configs: usize,
    /// Files already present with different contents; never overwritten.
    pub skipped: Vec<String>,
}

fn same_path(a: &Path, b: &Path) -> bool {
    a == b
}

fn read_tree(root: &Path) -> LoaderResult<Vec<(String, Vec<u8>)>> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let read = fs::read_dir(&dir).map_err(|e| {
            error_with(
                ErrorCode::ModpackInvalid,
                "a config folder cannot be read",
                e.to_string(),
            )
        })?;
        for entry in read.flatten() {
            let path = entry.path();
            let file_type = entry.file_type().map_err(|e| {
                error_with(
                    ErrorCode::ModpackInvalid,
                    "a config file cannot be read",
                    e.to_string(),
                )
            })?;
            if file_type.is_symlink() {
                return Err(error_with(
                    ErrorCode::ModpackInvalid,
                    "symbolic links are not allowed in a pack",
                    path.display().to_string(),
                ));
            }
            if file_type.is_dir() {
                stack.push(path);
            } else if file_type.is_file() {
                let name = path
                    .strip_prefix(root)
                    .map_err(|_| {
                        error_with(ErrorCode::ModpackInvalid, "a file is outside the pack", "")
                    })?
                    .to_string_lossy()
                    .replace('\\', "/");
                out.push((
                    name,
                    fs::read(&path).map_err(|e| {
                        error_with(
                            ErrorCode::ModpackInvalid,
                            "a config file cannot be read",
                            e.to_string(),
                        )
                    })?,
                ));
            }
        }
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use fabric_mod_format::container::{Container, EXTENSION};

    fn write_mods(dir: &Path) {
        for (id, version) in [("a", "1.0.0"), ("b", "2.0.0")] {
            let container = Container::new(&format!(
                r#"{{"id":"{id}","version":"{version}","depends":{{"minecraft":">=1.21"}}}}"#
            ))
            .unwrap();
            fs::write(
                dir.join(format!("{id}.{EXTENSION}")),
                container.to_bytes().unwrap(),
            )
            .unwrap();
        }
    }

    #[test]
    fn packs_and_imports_a_working_set() {
        let dir = tempfile::tempdir().unwrap();
        let mods = dir.path().join("mods");
        let configs = dir.path().join("config");
        fs::create_dir_all(&mods).unwrap();
        fs::create_dir_all(configs.join("nested")).unwrap();
        write_mods(&mods);
        fs::write(configs.join("nested/options.json"), b"{}").unwrap();
        let out = dir.path().join("pack.fabricpack");
        let report = pack(&mods, Some(&configs), &out, "Test Pack", "1.0.0").unwrap();
        assert_eq!(report.mods, 2);
        assert_eq!(report.configs, 1);
        let archive = ModpackArchive::read(&out).unwrap();
        assert_eq!(archive.manifest.as_ref().unwrap().name, "Test Pack");
        assert_eq!(archive.mods.len(), 2);
        assert!(archive.lock.as_ref().unwrap().contains("\"a\""));

        let target = dir.path().join("imported");
        let imported = dir.path().join("imported-config");
        let report = archive.import(&target, &imported).unwrap();
        assert_eq!(report.written, 2);
        assert_eq!(report.configs, 1);
        assert!(target.join("a.fabricmod").is_file());
        assert!(imported.join("nested/options.json").is_file());
        // Importing again changes nothing and overwrites nothing.
        let report = archive.import(&target, &imported).unwrap();
        assert_eq!(report.unchanged, 2);
        assert_eq!(report.written, 0);
    }

    #[test]
    fn hostile_packs_are_refused() {
        for bytes in [&b""[..], b"not a pack", b"PK\x03\x04garbage"] {
            assert_eq!(
                ModpackArchive::read_bytes(bytes).unwrap_err().code,
                ErrorCode::ModpackInvalid
            );
        }
    }
}
