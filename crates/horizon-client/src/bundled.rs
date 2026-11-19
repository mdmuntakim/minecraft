//! The APK's offline payload: the game files, the Java runtime and Bedrock's control textures,
//! packed into the APK's `assets/bundled/` at build time by
//! `packaging/android/offline_bundle.py` and described by its `manifest.json`.
//!
//! A fresh install unpacks them into the app's private folder and never downloads the game:
//! the setup's "game files" step is this extraction, and the preparer runs from the carried
//! Java runtime. The desktop game (and a phone build without the payload) keeps its own way:
//! `starling_account::files::download` and `starling_account::java::install`.
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

/// The manifest the build writes beside the payload.
pub const MANIFEST: &str = "bundled/manifest.json";
/// The launcher layout (`versions/`, `libraries/`, `assets/`) the setup unpacks.
pub const GAME_FILES: &str = "game-files.zip";
/// The Java runtime that prepares the game files on the phone.
pub const JAVA_RUNTIME: &str = "jre25-android-arm64.tar.xz";
/// Bedrock's control textures and the raw touch-button sources.
pub const ASSETS: &str = "assets.zip";

/// Whether this build carries the offline payload (the Android APK does; the desktop and a
/// build made without `--offline` do not). Read once: the setup asks every frame.
pub fn available() -> bool {
    manifest().is_some()
}

/// The payload's manifest, when the app carries one: read once from the APK's assets.
pub fn manifest() -> Option<Value> {
    static CACHED: std::sync::OnceLock<Value> = std::sync::OnceLock::new();
    if let Some(manifest) = CACHED.get() {
        return Some(manifest.clone());
    }
    if !crate::platform::registered() {
        return None;
    }
    let manifest: Value = serde_json::from_slice(&crate::platform::asset(MANIFEST)?).ok()?;
    let _ = CACHED.set(manifest.clone());
    Some(manifest)
}

/// An archive's size and SHA-256, as the manifest describes it.
fn archive(manifest: &Value, name: &str) -> Option<(u64, String)> {
    let archives = manifest["archives"].as_array()?;
    let entry = archives.iter().find(|a| a["path"].as_str() == Some(name))?;
    let hash = entry["sha256"].as_str().filter(|h| h.len() == 64)?;
    Some((entry["size"].as_u64().unwrap_or(0), hash.to_owned()))
}

/// How many game files the payload holds: the setup's progress total.
pub fn game_file_count() -> usize {
    manifest()
        .as_ref()
        .and_then(|m| m["game_files"].as_array().map(Vec::len))
        .unwrap_or(0)
}

/// The pinned game version the payload was made from.
pub fn version() -> Option<String> {
    manifest()?.get("version")?.as_str().map(str::to_owned)
}

/// Where the payload's archives are unpacked: the app's own folder.
fn cache() -> PathBuf {
    crate::logging::private_home().join("bundled")
}

fn sha256_file(path: &Path) -> Option<String> {
    let mut file = std::fs::File::open(path).ok()?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; 1 << 20];
    loop {
        let n = std::io::Read::read(&mut file, &mut buffer).ok()?;
        if n == 0 {
            break;
        }
        hasher.update(&buffer[..n]);
    }
    Some(
        hasher
            .finalize()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect(),
    )
}

/// `name` (an archive of the payload) streamed out of the APK into the app's folder, checked
/// against the manifest's SHA-256: the archive path, or an error that names what is missing.
fn copied(name: &str, progress: &mut dyn FnMut(u64, u64)) -> Result<PathBuf, String> {
    let manifest = manifest().ok_or("This build carries no offline game files.")?;
    let (size, hash) = archive(&manifest, name)
        .ok_or_else(|| format!("The offline payload does not describe {name}."))?;
    let folder = cache();
    std::fs::create_dir_all(&folder).map_err(|e| format!("{}: {e}", folder.display()))?;
    let path = folder.join(name);
    if path.is_file() && sha256_file(&path).as_deref() == Some(hash.as_str()) {
        return Ok(path);
    }
    let _ = std::fs::remove_file(&path);
    let written = crate::platform::asset_to_file(&format!("bundled/{name}"), &path, progress)
        .ok_or_else(|| format!("The offline payload has no {name}."))?;
    if written != size {
        let _ = std::fs::remove_file(&path);
        return Err(format!("{name} was cut off ({written} of {size} bytes)."));
    }
    if sha256_file(&path).as_deref() != Some(hash.as_str()) {
        let _ = std::fs::remove_file(&path);
        return Err(format!("{name} is damaged (its hash differs)."));
    }
    Ok(path)
}

/// The archive's entries into `dir`, never outside it; the number of files written.
fn unzip(archive: &Path, dir: &Path) -> Result<usize, String> {
    let file = std::fs::File::open(archive).map_err(|e| format!("{}: {e}", archive.display()))?;
    let mut zip = zip::ZipArchive::new(file).map_err(|e| format!("{}: {e}", archive.display()))?;
    let mut written = 0;
    for index in 0..zip.len() {
        let mut entry = zip.by_index(index).map_err(|e| e.to_string())?;
        let Some(name) = entry.enclosed_name() else {
            return Err(format!(
                "{archive:?} names {}",
                entry
                    .name()
                    .map_or_else(|e| e.to_string(), |n| n.into_owned())
            ));
        };
        let path = dir.join(name);
        if entry.is_dir() {
            continue;
        }
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
        }
        let mut file =
            std::fs::File::create(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        std::io::copy(&mut entry, &mut file).map_err(|e| format!("{}: {e}", path.display()))?;
        written += 1;
    }
    Ok(written)
}

/// The payload's game files into `root` (`versions/`, `libraries/`, `assets/`), the layout the
/// preparer reads; what is there already is overwritten, so an interrupted extraction resumes.
/// `progress` gets the archive's bytes as they are copied. The number of files written.
pub fn extract_game_files(
    root: &Path,
    progress: &mut dyn FnMut(u64, u64),
) -> Result<usize, String> {
    let archive = copied(GAME_FILES, progress)?;
    std::fs::create_dir_all(root).map_err(|e| format!("{}: {e}", root.display()))?;
    let written = unzip(&archive, root)?;
    let _ = std::fs::remove_file(&archive);
    Ok(written)
}

/// `name` unpacked in the app's folder once for each version of it the app carries (a marker
/// holds its SHA-256); the folder, or None when the build carries no payload.
pub fn folder(name: &str) -> Result<Option<PathBuf>, String> {
    let Some(manifest) = manifest() else {
        return Ok(None);
    };
    let (_, hash) = archive(&manifest, name)
        .ok_or_else(|| format!("The offline payload does not describe {name}."))?;
    let dir = cache().join(name.trim_end_matches(".zip"));
    let marker = dir.join(".sha256");
    if std::fs::read_to_string(&marker).ok().as_deref() == Some(hash.as_str()) {
        return Ok(Some(dir));
    }
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let archive = copied(name, &mut |_, _| {})?;
    unzip(&archive, &dir)?;
    let _ = std::fs::remove_file(&archive);
    std::fs::write(&marker, &hash).map_err(|e| format!("{}: {e}", marker.display()))?;
    Ok(Some(dir))
}

/// `name` streamed out of the APK into `dir` (the Java runtime: the preparer unpacks it), or
/// None when the build carries no payload.
pub fn archive_file(
    name: &str,
    dir: &Path,
    progress: &mut dyn FnMut(u64, u64),
) -> Result<Option<PathBuf>, String> {
    if manifest().is_none() {
        return Ok(None);
    }
    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let path = copied(name, progress)?;
    let target = dir.join(name);
    if path != target {
        std::fs::rename(&path, &target).map_err(|e| format!("{}: {e}", target.display()))?;
    }
    Ok(Some(target))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_archive_lookup_reads_the_manifest() {
        let manifest = serde_json::json!({
            "archives": [
                {"path": "game-files.zip", "size": 12, "sha256": "a".repeat(64)},
                {"path": "assets.zip", "size": 3, "sha256": "short"},
            ]
        });
        assert_eq!(
            archive(&manifest, "game-files.zip"),
            Some((12, "a".repeat(64)))
        );
        // An entry without a whole SHA-256 is not usable.
        assert_eq!(archive(&manifest, "assets.zip"), None);
        assert_eq!(archive(&manifest, "jre25-android-arm64.tar.xz"), None);
    }

    #[test]
    fn unzip_writes_the_entries_and_refuses_to_escape() {
        let root = std::env::temp_dir().join(format!("starling-bundled-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let archive = root.join("payload.zip");
        {
            let mut zip = zip::ZipWriter::new(std::fs::File::create(&archive).unwrap());
            zip.start_file(
                "versions/26.3/26.3.json",
                zip::write::SimpleFileOptions::default(),
            )
            .unwrap();
            std::io::Write::write_all(&mut zip, b"{}").unwrap();
            zip.start_file(
                "assets/objects/ab/abcd",
                zip::write::SimpleFileOptions::default(),
            )
            .unwrap();
            std::io::Write::write_all(&mut zip, b"asset").unwrap();
            zip.finish().unwrap();
        }
        let out = root.join("out");
        std::fs::create_dir_all(&out).unwrap();
        assert_eq!(unzip(&archive, &out).unwrap(), 2);
        assert_eq!(
            std::fs::read(out.join("versions/26.3/26.3.json")).unwrap(),
            b"{}"
        );
        assert_eq!(
            std::fs::read(out.join("assets/objects/ab/abcd")).unwrap(),
            b"asset"
        );
        std::fs::remove_dir_all(root).unwrap();
    }
}
