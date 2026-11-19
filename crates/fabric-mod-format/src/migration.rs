//! One-time migration of legacy `.hmod` and `.hnmod` files to `.fabricmod`.
//!
//! The originals are moved to `mods/.legacy-backup/` and are never deleted.
//! After the migration marker exists this build does not write a legacy
//! extension again; the marker is checked before the directory is scanned.

use crate::{
    container::{self, Container, MANIFEST_ENTRY, SIGNATURE_ENTRY},
    error::{ErrorCode, FabricError, Result},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    fs,
    io::{Cursor, Read},
    path::{Path, PathBuf},
};

/// The legacy extensions this build migrates, and only reads for that.
pub const LEGACY_EXTENSIONS: [&str; 2] = ["hmod", "hnmod"];
/// The marker that records the one migration.
pub const MARKER: &str = ".fabricmod-migration-v1";
/// Where originals are kept.
pub const BACKUP_DIR: &str = ".legacy-backup";
/// The biggest legacy file read.
const MAX_LEGACY: u64 = 512 << 20;

/// One migrated file.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Migrated {
    pub from: PathBuf,
    pub to: PathBuf,
    pub legacy_kind: String,
}

/// The result of a migration pass.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MigrationReport {
    pub migrated: Vec<Migrated>,
    pub skipped: Vec<PathBuf>,
    pub failed: Vec<String>,
    pub already_done: bool,
}

impl MigrationReport {
    /// One line for the launcher's log.
    pub fn summary(&self) -> String {
        format!(
            "migrated {} mod(s), skipped {}, failed {}{}",
            self.migrated.len(),
            self.skipped.len(),
            self.failed.len(),
            if self.already_done {
                " (migration already done)"
            } else {
                ""
            }
        )
    }
}

/// Migrates every legacy file in `mods`. `force` re-runs a pass that has
/// already been recorded, for tests and for a manual repair.
pub fn migrate_dir(mods: &Path, force: bool) -> MigrationReport {
    let mut report = MigrationReport::default();
    let marker = mods.join(MARKER);
    if marker.is_file() && !force {
        report.already_done = true;
        return report;
    }
    let Ok(read) = fs::read_dir(mods) else {
        return report;
    };
    let mut paths: Vec<PathBuf> = read
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.is_file() && extension(p).is_some_and(|e| LEGACY_EXTENSIONS.contains(&e.as_str()))
        })
        .collect();
    paths.sort();
    if paths.is_empty() {
        return report;
    }
    let backup = mods.join(BACKUP_DIR);
    if let Err(e) = fs::create_dir_all(&backup) {
        report.failed.push(format!("{}: {e}", backup.display()));
        return report;
    }
    for path in paths {
        match migrate_file(&path, mods, &backup) {
            Ok(migrated) => report.migrated.push(migrated),
            Err(err) => report
                .failed
                .push(format!("{}: {}", path.display(), err.log_line())),
        }
    }
    if report.failed.is_empty() {
        let _ = fs::write(&marker, b"v1\n");
    }
    report
}

/// Migrates one file. The original is moved, never removed.
pub fn migrate_file(path: &Path, mods: &Path, backup: &Path) -> Result<Migrated> {
    let legacy_kind = extension(path).ok_or_else(|| {
        FabricError::with_detail(
            ErrorCode::MigrationFailed,
            "not a legacy mod",
            path.display().to_string(),
        )
    })?;
    let metadata = fs::metadata(path).map_err(|e| {
        FabricError::with_detail(
            ErrorCode::MigrationFailed,
            "the legacy mod cannot be read",
            format!("{}: {e}", path.display()),
        )
    })?;
    if metadata.len() > MAX_LEGACY {
        return Err(FabricError::new(
            ErrorCode::ZipLimit,
            "the legacy mod is too large",
        ));
    }
    let bytes = fs::read(path).map_err(|e| {
        FabricError::with_detail(
            ErrorCode::MigrationFailed,
            "the legacy mod cannot be read",
            format!("{}: {e}", path.display()),
        )
    })?;
    let fallback = sanitize_id(
        path.file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("legacy-mod"),
    );
    let (manifest_text, files) = migrate_bytes(&bytes, &fallback, &legacy_kind)?;
    let id = Container::new(&manifest_text)?.manifest.id;
    let to = mods.join(format!(
        "{id}.{}.{}",
        short_hash(&bytes),
        container::EXTENSION
    ));
    let container = {
        let mut c = Container::new(&manifest_text)?;
        for (name, data) in files {
            c.insert(&name, data)?;
        }
        c
    };
    container.write(&to)?;
    let original = backup.join(path.file_name().unwrap_or_default());
    fs::rename(path, &original).map_err(|e| {
        let _ = fs::remove_file(&to);
        FabricError::with_detail(
            ErrorCode::MigrationFailed,
            "the migrated file was written but the original could not be moved",
            format!("{}: {e}", path.display()),
        )
    })?;
    Ok(Migrated {
        from: original,
        to,
        legacy_kind,
    })
}

/// Converts legacy bytes into a manifest and a set of files. Public so tests
/// and the CLI can inspect a conversion without touching the disk.
pub fn migrate_bytes(
    bytes: &[u8],
    fallback_id: &str,
    legacy_kind: &str,
) -> Result<(String, BTreeMap<String, Vec<u8>>)> {
    if bytes.len() < 4 || &bytes[..4] != b"PK\x03\x04" {
        return Err(FabricError::with_detail(
            ErrorCode::MigrationFailed,
            "the legacy mod is not a zip archive",
            legacy_kind.to_owned(),
        ));
    }
    let mut zip = zip::ZipArchive::new(Cursor::new(bytes)).map_err(|e| {
        FabricError::with_detail(
            ErrorCode::MigrationFailed,
            "the legacy mod is damaged",
            e.to_string(),
        )
    })?;
    if zip.len() > 4096 {
        return Err(FabricError::new(
            ErrorCode::ZipLimit,
            "the legacy mod has too many entries",
        ));
    }
    let mut entries: BTreeMap<String, Vec<u8>> = BTreeMap::new();
    for index in 0..zip.len() {
        let mut entry = zip.by_index(index).map_err(|e| {
            FabricError::with_detail(
                ErrorCode::MigrationFailed,
                "a legacy entry cannot be read",
                e.to_string(),
            )
        })?;
        if entry.is_dir() {
            continue;
        }
        let raw_name = entry.name().map_err(|e| {
            FabricError::with_detail(
                ErrorCode::MigrationFailed,
                "a legacy entry name is not valid UTF-8",
                e.to_string(),
            )
        })?;
        let name = container::canonical_name(&raw_name)?;
        let mut data = Vec::new();
        entry
            .by_ref()
            .take((256u64 << 20) + 1)
            .read_to_end(&mut data)
            .map_err(|e| {
                FabricError::with_detail(
                    ErrorCode::MigrationFailed,
                    "a legacy entry is damaged",
                    format!("{name}: {e}"),
                )
            })?;
        entries.insert(name, data);
    }
    if entries.contains_key(MANIFEST_ENTRY) {
        let text = String::from_utf8(entries.remove(MANIFEST_ENTRY).unwrap_or_default()).map_err(
            |_| FabricError::new(ErrorCode::MigrationFailed, "fabric.mod.json is not UTF-8"),
        )?;
        // Already a fabric container with a legacy name: only the extension
        // changes. Drop a stale signature, since the bytes may have changed.
        entries.remove(SIGNATURE_ENTRY);
        return Ok((text, entries));
    }

    let mut files = BTreeMap::new();
    let (id, name, version, description, authors, api) = match entries.remove("horizon-mod.toml") {
        Some(toml_bytes) => {
            let text = String::from_utf8(toml_bytes).map_err(|_| {
                FabricError::new(ErrorCode::MigrationFailed, "horizon-mod.toml is not UTF-8")
            })?;
            let value: toml::Value =
                text.parse::<toml::Value>().map_err(|e: toml::de::Error| {
                    FabricError::with_detail(
                        ErrorCode::MigrationFailed,
                        "horizon-mod.toml is not valid TOML",
                        e.to_string(),
                    )
                })?;
            let id = value
                .get("id")
                .and_then(toml::Value::as_str)
                .map(sanitize_id)
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| fallback_id.to_owned());
            let name = value
                .get("name")
                .and_then(toml::Value::as_str)
                .unwrap_or(&id)
                .to_owned();
            let version = value
                .get("version")
                .and_then(toml::Value::as_str)
                .filter(|v| crate::version::Version::parse(v).is_ok())
                .unwrap_or("0.0.0")
                .to_owned();
            let description = value
                .get("description")
                .and_then(toml::Value::as_str)
                .unwrap_or("Migrated from a .hmod")
                .to_owned();
            let authors = value
                .get("authors")
                .and_then(toml::Value::as_array)
                .map(|a| {
                    a.iter()
                        .filter_map(toml::Value::as_str)
                        .map(str::to_owned)
                        .collect()
                })
                .unwrap_or_default();
            let api = value
                .get("api")
                .and_then(toml::Value::as_str)
                .unwrap_or("0.1")
                .to_owned();
            (id, name, version, description, authors, api)
        }
        None => (
            fallback_id.to_owned(),
            fallback_id.to_owned(),
            "0.0.0".to_owned(),
            flag(legacy_kind),
            Vec::new(),
            "0.1".to_owned(),
        ),
    };

    let mut native = None;
    for (path, data) in entries {
        // Content keeps its own path; code and anything else the loader cannot
        // run is preserved under `legacy/`, so nothing is lost.
        if path.starts_with("assets/") || path.starts_with("data/") || path.starts_with("mixins/") {
            files.insert(path, data);
        } else if path.ends_with(".so") && native.is_none() {
            let target = format!("lib/{}/{path}", container::ABI);
            native = Some(target.clone());
            files.insert(target, data);
        } else {
            files.insert(format!("legacy/{path}"), data);
        }
    }
    let native = native.or_else(|| {
        files
            .iter()
            .find(|(name, _)| name.starts_with("lib/") && name.ends_with(".so"))
            .map(|(name, _)| name.clone())
    });
    if native.is_none() && !files.is_empty() {
        // A legacy code mod without a native library has no way to run, but
        // its files are still preserved; the loader will explain why.
    }

    let custom = json!({
        "horizon": {
            "migratedFrom": legacy_kind,
            "legacyApi": api,
            "native": native,
            "legacy": true,
        }
    });
    let manifest = json!({
        "schemaVersion": 2,
        "formatVersion": 1,
        "id": id,
        "version": version,
        "name": name,
        "description": description,
        "environment": "*",
        "authors": authors,
        "license": "Unknown",
        "custom": custom,
        "extra": Value::Null,
    });
    // The `extra` key above is not part of the manifest schema; remove it.
    let mut manifest = manifest;
    if let Some(object) = manifest.as_object_mut() {
        object.remove("extra");
    }
    let text = serde_json::to_string_pretty(&manifest).map_err(|e| {
        FabricError::with_detail(
            ErrorCode::MigrationFailed,
            "the migrated manifest cannot be written",
            e.to_string(),
        )
    })?;
    // Never produce a manifest the reader would reject.
    crate::manifest::Manifest::parse(&text)?;
    Ok((text, files))
}

fn flag(legacy_kind: &str) -> String {
    format!("Migrated from a .{legacy_kind}")
}

fn sanitize_id(text: &str) -> String {
    let mut out = String::new();
    for c in text.chars() {
        let c = c.to_ascii_lowercase();
        if c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' {
            out.push(c);
        } else if !out.ends_with('-') {
            out.push('-');
        }
    }
    let trimmed = out.trim_matches('-');
    let mut id: String = trimmed.chars().take(64).collect();
    if id.is_empty() {
        id = "legacy-mod".into();
    }
    if id.starts_with(|c: char| c.is_ascii_digit()) {
        id.insert_str(0, "mod-");
    }
    id
}

fn extension(path: &Path) -> Option<String> {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
}

fn short_hash(bytes: &[u8]) -> String {
    use sha2::Digest;
    let digest = sha2::Sha256::digest(bytes);
    crate::signature::hex(&digest[..4])
}

/// Deletes nothing: a legacy folder that this build does not migrate is
/// reported, not touched.
pub fn legacy_files(dir: &Path) -> Vec<PathBuf> {
    let Ok(read) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut found: Vec<PathBuf> = read
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.is_file() && extension(p).is_some_and(|e| LEGACY_EXTENSIONS.contains(&e.as_str()))
        })
        .collect();
    found.sort();
    found
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn legacy_zip(entries: &[(&str, &[u8])]) -> Vec<u8> {
        let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
        let options = zip::write::SimpleFileOptions::default();
        for (name, data) in entries {
            zip.start_file(*name, options).unwrap();
            zip.write_all(data).unwrap();
        }
        zip.finish().unwrap().into_inner()
    }

    #[test]
    fn migrates_an_hmod_with_content() {
        let toml = br#"
id = "coords_hud"
name = "Coordinates HUD"
version = "0.1.0"
api = "0.1"
authors = ["Someone"]
"#;
        let bytes = legacy_zip(&[
            ("horizon-mod.toml", toml),
            ("mod.wasm", b"\0asm"),
            ("assets/coords_hud/lang/en_us.json", b"{}"),
        ]);
        let (text, files) = migrate_bytes(&bytes, "fallback", "hmod").unwrap();
        let manifest = crate::manifest::Manifest::parse(&text).unwrap();
        assert_eq!(manifest.id, "coords_hud");
        assert_eq!(manifest.version, "0.1.0");
        assert_eq!(manifest.authors, ["Someone"]);
        assert!(files.contains_key("assets/coords_hud/lang/en_us.json"));
        assert!(files.contains_key("legacy/mod.wasm"));
        // The generated manifest is a valid container.
        let mut c = Container::new(&text).unwrap();
        for (name, data) in files {
            c.insert(&name, data).unwrap();
        }
        let round = Container::read_bytes(&c.to_bytes().unwrap()).unwrap();
        assert_eq!(round.manifest.id, "coords_hud");
    }

    #[test]
    fn migrates_an_hnmod_with_a_native_library() {
        let bytes = legacy_zip(&[
            ("libarm64.so", b"\x7fELF-not-really"),
            ("assets/x/a.txt", b"a"),
        ]);
        let (text, files) = migrate_bytes(&bytes, "native-thing", "hnmod").unwrap();
        let manifest = crate::manifest::Manifest::parse(&text).unwrap();
        assert_eq!(manifest.id, "native-thing");
        assert!(files.contains_key("lib/arm64-v8a/libarm64.so"));
        assert_eq!(
            manifest.native_library().as_deref(),
            Some("lib/arm64-v8a/libarm64.so")
        );
    }

    #[test]
    fn a_fabric_container_under_a_legacy_name_only_changes_extension() {
        let text = r#"{"id":"already","version":"2.0.0","schemaVersion":2,"formatVersion":1}"#;
        let bytes = legacy_zip(&[
            (MANIFEST_ENTRY, text.as_bytes()),
            ("icon.png", b"\x89PNG\r\n\x1a\n"),
            (SIGNATURE_ENTRY, b"stale"),
        ]);
        let (out, files) = migrate_bytes(&bytes, "x", "hmod").unwrap();
        assert_eq!(out, text);
        assert!(files.contains_key("icon.png"));
        assert!(!files.contains_key(SIGNATURE_ENTRY));
    }

    #[test]
    fn hostile_legacy_files_fail_without_panicking() {
        for bytes in [&b""[..], b"not a zip", b"PK\x03\x04garbage", b"PK\x03\x04"] {
            let err = migrate_bytes(bytes, "x", "hmod").unwrap_err();
            assert_eq!(err.code, ErrorCode::MigrationFailed);
        }
    }

    #[test]
    fn directory_migration_moves_originals_and_sets_a_marker() {
        let dir = tempfile::tempdir().unwrap();
        let mods = dir.path();
        let toml = b"id = \"old\"\nname = \"Old\"\nversion = \"1.0.0\"\n";
        fs::write(
            mods.join("old.hmod"),
            legacy_zip(&[("horizon-mod.toml", toml)]),
        )
        .unwrap();
        fs::write(
            mods.join("n.hnmod"),
            legacy_zip(&[("lib.so", b"\x7fELF"), ("assets/a.txt", b"a")]),
        )
        .unwrap();
        let report = migrate_dir(mods, false);
        assert_eq!(report.migrated.len(), 2, "{report:?}");
        assert!(report.failed.is_empty(), "{report:?}");
        assert!(mods.join(MARKER).is_file());
        assert!(mods.join(BACKUP_DIR).join("old.hmod").is_file());
        assert!(legacy_files(mods).is_empty());
        let fabric: Vec<_> = fs::read_dir(mods)
            .unwrap()
            .flatten()
            .filter(|e| {
                e.path()
                    .extension()
                    .is_some_and(|x| x == container::EXTENSION)
            })
            .collect();
        assert_eq!(fabric.len(), 2);
        let again = migrate_dir(mods, false);
        assert!(again.already_done);
        assert!(again.migrated.is_empty());
    }
}
