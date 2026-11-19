//! Reading and writing the `.fabricmod` container.
//!
//! A container is a zip archive with a `fabric.mod.json`, optional native
//! entrypoints, resources, data, mixin configs and an Ed25519 signature. The
//! reader is deliberately paranoid: entry names are canonicalised, symlinks
//! and zip slips are refused, duplicates are refused, and both the entry count
//! and the uncompressed size are capped before anything is allocated.

use crate::{
    error::{ErrorCode, FabricError, Result},
    manifest::{MAX_MANIFEST, Manifest},
    signature::{Signature, TrustStore},
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::{Cursor, Read, Write},
    path::Path,
};

/// The container format this build writes and the newest it reads.
pub const FORMAT_VERSION: u32 = 1;
/// The manifest's path inside a container.
pub const MANIFEST_ENTRY: &str = "fabric.mod.json";
/// The signature's path inside a container.
pub const SIGNATURE_ENTRY: &str = "META-INF/fabricmod.sig";
/// The canonical extension and the only one this build reads or writes.
pub const EXTENSION: &str = "fabricmod";
/// The MIME type Android's picker and the registry use.
pub const MIME: &str = "application/vnd.fabricmod+zip";
/// The native ABI this build supports.
pub const ABI: &str = "arm64-v8a";
/// The largest icon edge.
pub const MAX_ICON_EDGE: u32 = 512;

/// Caps applied before a byte is allocated for an entry.
#[derive(Clone, Copy, Debug)]
pub struct Limits {
    pub entries: usize,
    pub entry: u64,
    pub total: u64,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            entries: 4096,
            entry: 256 << 20,
            total: 512 << 20,
        }
    }
}

/// One file in a container.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct File {
    pub name: String,
    pub data: Vec<u8>,
    pub executable: bool,
}

/// A parsed `.fabricmod`.
#[derive(Clone, Debug)]
pub struct Container {
    pub manifest: Manifest,
    /// The manifest's exact bytes, so a signature covers what was written.
    pub manifest_text: String,
    /// Every entry except the manifest and signature, keyed by canonical path.
    pub files: BTreeMap<String, File>,
    /// The container digest.
    pub digest: [u8; 32],
    /// The signature, when the container has one.
    pub signature: Option<Signature>,
}

impl Container {
    /// Reads an archive from disk.
    pub fn read(path: &Path) -> Result<Self> {
        let bytes = fs::read(path).map_err(|e| {
            FabricError::with_detail(
                ErrorCode::FormatInvalid,
                "the mod file cannot be read",
                format!("{}: {e}", path.display()),
            )
        })?;
        Self::read_bytes(&bytes)
    }

    /// Reads an archive from memory. Never panics on hostile bytes.
    pub fn read_bytes(bytes: &[u8]) -> Result<Self> {
        Self::read_with(bytes, Limits::default())
    }

    pub fn read_with(bytes: &[u8], limits: Limits) -> Result<Self> {
        if bytes.len() < 4 || &bytes[..4] != b"PK\x03\x04" {
            return Err(FabricError::with_detail(
                ErrorCode::FormatInvalid,
                "this is not a .fabricmod container",
                format!("{} bytes, no PK header", bytes.len()),
            ));
        }
        let mut zip = zip::ZipArchive::new(Cursor::new(bytes)).map_err(|e| {
            FabricError::with_detail(
                ErrorCode::ZipInvalid,
                "the zip archive is damaged",
                e.to_string(),
            )
        })?;
        if zip.len() > limits.entries {
            return Err(FabricError::with_detail(
                ErrorCode::ZipLimit,
                format!("the mod has more than {} entries", limits.entries),
                zip.len().to_string(),
            ));
        }

        let mut manifest_text: Option<String> = None;
        let mut signature: Option<Signature> = None;
        let mut files = BTreeMap::new();
        let mut seen = BTreeSet::new();
        let mut seen_folded = BTreeSet::new();
        let mut total = 0u64;

        for index in 0..zip.len() {
            let mut entry = zip.by_index(index).map_err(|e| {
                FabricError::with_detail(
                    ErrorCode::ZipInvalid,
                    "an entry cannot be read",
                    e.to_string(),
                )
            })?;
            let raw_name = entry
                .name()
                .map_err(|e| {
                    FabricError::with_detail(
                        ErrorCode::ZipInvalid,
                        "an entry name is not valid UTF-8",
                        e.to_string(),
                    )
                })?
                .into_owned();
            if entry.encrypted() {
                return Err(FabricError::with_detail(
                    ErrorCode::ZipInvalid,
                    "encrypted container entries are not supported",
                    raw_name,
                ));
            }
            let is_dir = entry.is_dir();
            if let Some(mode) = entry.unix_mode()
                && mode & 0o170000 == 0o120000
            {
                return Err(FabricError::with_detail(
                    ErrorCode::ZipSymlink,
                    "symbolic links are not allowed in a mod",
                    raw_name,
                ));
            }
            let name = canonical_name(&raw_name)?;
            if is_dir {
                continue;
            }
            if !seen.insert(name.clone()) {
                return Err(FabricError::with_detail(
                    ErrorCode::ZipDuplicate,
                    "two entries have the same path",
                    name,
                ));
            }
            if !seen_folded.insert(name.to_ascii_lowercase()) {
                return Err(FabricError::with_detail(
                    ErrorCode::ZipDuplicate,
                    "two entries differ only in case",
                    name,
                ));
            }
            let declared = entry.size();
            if declared > limits.entry {
                return Err(FabricError::with_detail(
                    ErrorCode::ZipLimit,
                    format!("an entry is larger than {} MiB", limits.entry >> 20),
                    name,
                ));
            }
            let cap = limits.entry.saturating_add(1);
            let mut data = Vec::new();
            entry
                .by_ref()
                .take(cap)
                .read_to_end(&mut data)
                .map_err(|e| {
                    FabricError::with_detail(
                        ErrorCode::ZipInvalid,
                        "an entry is damaged",
                        format!("{name}: {e}"),
                    )
                })?;
            if data.len() as u64 > limits.entry {
                return Err(FabricError::with_detail(
                    ErrorCode::ZipLimit,
                    "an entry decompressed past its limit",
                    name,
                ));
            }
            total = total.saturating_add(data.len() as u64);
            if total > limits.total {
                return Err(FabricError::with_detail(
                    ErrorCode::ZipLimit,
                    format!(
                        "the mod is larger than {} MiB uncompressed",
                        limits.total >> 20
                    ),
                    name,
                ));
            }

            match name.as_str() {
                MANIFEST_ENTRY => {
                    if data.len() > MAX_MANIFEST {
                        return Err(FabricError::with_detail(
                            ErrorCode::ManifestTooLarge,
                            "fabric.mod.json is over 1 MiB",
                            data.len().to_string(),
                        ));
                    }
                    let text = String::from_utf8(data).map_err(|_| {
                        FabricError::new(ErrorCode::ManifestInvalid, "fabric.mod.json is not UTF-8")
                    })?;
                    manifest_text = Some(text);
                }
                SIGNATURE_ENTRY => {
                    if data.len() > 16 << 10 {
                        return Err(FabricError::new(
                            ErrorCode::SignatureInvalid,
                            "the signature document is too large",
                        ));
                    }
                    let parsed: Signature = serde_json::from_slice(&data).map_err(|e| {
                        FabricError::with_detail(
                            ErrorCode::SignatureInvalid,
                            "the signature document is not valid",
                            e.to_string(),
                        )
                    })?;
                    signature = Some(parsed);
                }
                _ => {
                    files.insert(
                        name.clone(),
                        File {
                            name,
                            data,
                            executable: matches!(
                                entry.unix_mode(),
                                Some(mode) if mode & 0o111 != 0
                            ),
                        },
                    );
                }
            }
        }

        let manifest_text = manifest_text.ok_or_else(|| {
            FabricError::new(
                ErrorCode::FormatInvalid,
                "the container has no fabric.mod.json",
            )
        })?;
        let manifest = Manifest::parse(&manifest_text)?;
        if manifest.format_version > FORMAT_VERSION {
            return Err(FabricError::with_detail(
                ErrorCode::FormatTooNew,
                format!(
                    "formatVersion {} is newer than this build's {FORMAT_VERSION}",
                    manifest.format_version
                ),
                manifest.id,
            ));
        }
        for file in files.values() {
            if file.name.starts_with("lib/") && file.name.ends_with(".so") {
                check_abi(&file.name)?;
            }
        }
        let digest = digest_of(&manifest_text, &files);
        if let Some(sig) = &signature {
            sig.verify(&digest)?;
        }
        check_java(&manifest, &files)?;
        check_icon(&manifest, &files)?;

        Ok(Self {
            manifest,
            manifest_text,
            files,
            digest,
            signature,
        })
    }

    /// Reads an unpacked mod folder (development layout, and the template the
    /// CLI writes). The result is identical to the container a pack produces.
    pub fn read_folder(path: &Path) -> Result<Self> {
        let manifest_text = fs::read_to_string(path.join(MANIFEST_ENTRY)).map_err(|e| {
            FabricError::with_detail(
                ErrorCode::ManifestInvalid,
                "the folder has no fabric.mod.json",
                format!("{}: {e}", path.display()),
            )
        })?;
        if manifest_text.len() > MAX_MANIFEST {
            return Err(FabricError::new(
                ErrorCode::ManifestTooLarge,
                "fabric.mod.json is over 1 MiB",
            ));
        }
        let manifest = Manifest::parse(&manifest_text)?;
        if manifest.format_version > FORMAT_VERSION {
            return Err(FabricError::new(
                ErrorCode::FormatTooNew,
                "the folder uses a newer format",
            ));
        }
        let limits = Limits::default();
        let mut files = BTreeMap::new();
        let mut total = 0u64;
        let mut stack = vec![path.to_path_buf()];
        while let Some(dir) = stack.pop() {
            let read = fs::read_dir(&dir).map_err(|e| {
                FabricError::with_detail(
                    ErrorCode::FormatInvalid,
                    "a mod folder cannot be read",
                    format!("{}: {e}", dir.display()),
                )
            })?;
            for entry in read.flatten() {
                let file_type = entry
                    .file_type()
                    .map_err(|e| FabricError::new(ErrorCode::FormatInvalid, e.to_string()))?;
                if file_type.is_symlink() {
                    return Err(FabricError::with_detail(
                        ErrorCode::ZipSymlink,
                        "symbolic links are not allowed in a mod",
                        entry.path().display().to_string(),
                    ));
                }
                if file_type.is_dir() {
                    stack.push(entry.path());
                    continue;
                }
                if !file_type.is_file() {
                    continue;
                }
                let relative = entry
                    .path()
                    .strip_prefix(path)
                    .map_err(|_| {
                        FabricError::new(ErrorCode::ZipSlip, "a file is outside the folder")
                    })?
                    .to_string_lossy()
                    .replace('\\', "/");
                let name = canonical_name(&relative)?;
                if name == MANIFEST_ENTRY || name == SIGNATURE_ENTRY {
                    continue;
                }
                let data = fs::read(entry.path()).map_err(|e| {
                    FabricError::with_detail(
                        ErrorCode::FormatInvalid,
                        "a mod file cannot be read",
                        e.to_string(),
                    )
                })?;
                total = total.saturating_add(data.len() as u64);
                if files.len() >= limits.entries
                    || data.len() as u64 > limits.entry
                    || total > limits.total
                {
                    return Err(FabricError::new(
                        ErrorCode::ZipLimit,
                        "the mod folder is too large",
                    ));
                }
                files.insert(
                    name.clone(),
                    File {
                        executable: entry
                            .metadata()
                            .map(|m| {
                                #[cfg(unix)]
                                {
                                    use std::os::unix::fs::PermissionsExt;
                                    m.permissions().mode() & 0o111 != 0
                                }
                                #[cfg(not(unix))]
                                {
                                    let _ = m;
                                    false
                                }
                            })
                            .unwrap_or(false),
                        name,
                        data,
                    },
                );
            }
        }
        for file in files.values() {
            if file.name.starts_with("lib/") && file.name.ends_with(".so") {
                check_abi(&file.name)?;
            }
        }
        let digest = digest_of(&manifest_text, &files);
        check_java(&manifest, &files)?;
        check_icon(&manifest, &files)?;
        Ok(Self {
            manifest,
            manifest_text,
            files,
            digest,
            signature: None,
        })
    }

    /// The native library the manifest points at, if the container holds one.
    pub fn native_library(&self) -> Option<&File> {
        if let Some(path) = self.manifest.native_library() {
            return self.files.get(path.trim_start_matches('/'));
        }
        self.files
            .values()
            .find(|f| f.name.starts_with("lib/") && f.name.ends_with(".so"))
    }

    /// The icon bytes, when the manifest names one.
    pub fn icon(&self) -> Option<&[u8]> {
        let path = self.manifest.icon.as_deref()?;
        self.files.get(path).map(|f| f.data.as_slice())
    }

    /// Verifies the signature against a trust store, asking for consent on a
    /// first-seen key.
    pub fn verify_signature(
        &self,
        store: &mut TrustStore,
        consent: impl FnMut(&str) -> bool,
    ) -> Result<crate::signature::Trust> {
        let Some(signature) = &self.signature else {
            return Err(FabricError::new(
                ErrorCode::SignatureUntrusted,
                "this mod is not signed",
            ));
        };
        crate::signature::verify_with_consent(store, signature, &self.digest, consent)
    }

    /// Serialises the container deterministically.
    pub fn to_bytes(&self) -> Result<Vec<u8>> {
        let mut files = self.files.clone();
        if let Some(signature) = &self.signature {
            files.insert(
                SIGNATURE_ENTRY.into(),
                File {
                    name: SIGNATURE_ENTRY.into(),
                    data: serde_json::to_vec(signature).map_err(|e| {
                        FabricError::new(ErrorCode::SignatureInvalid, e.to_string())
                    })?,
                    executable: false,
                },
            );
        }
        write_container(&self.manifest_text, &files)
    }

    /// Writes the container to `path`.
    pub fn write(&self, path: &Path) -> Result<()> {
        let bytes = self.to_bytes()?;
        fs::write(path, bytes).map_err(|e| {
            FabricError::with_detail(
                ErrorCode::FormatInvalid,
                "the mod file cannot be written",
                format!("{}: {e}", path.display()),
            )
        })
    }

    /// A manifest for a new container.
    pub fn new(manifest_text: &str) -> Result<Self> {
        let manifest = Manifest::parse(manifest_text)?;
        let files = BTreeMap::new();
        let digest = digest_of(manifest_text, &files);
        Ok(Self {
            manifest,
            manifest_text: manifest_text.to_owned(),
            digest,
            signature: None,
            files,
        })
    }

    /// Adds a file, rejecting paths outside the container.
    pub fn insert(&mut self, name: &str, data: Vec<u8>) -> Result<()> {
        let name = canonical_name(name)?;
        if name == MANIFEST_ENTRY || name == SIGNATURE_ENTRY {
            return Err(FabricError::with_detail(
                ErrorCode::FormatInvalid,
                "that path is reserved",
                name,
            ));
        }
        self.files.insert(
            name.clone(),
            File {
                name,
                data,
                executable: false,
            },
        );
        self.digest = digest_of(&self.manifest_text, &self.files);
        Ok(())
    }

    /// Signs the container with an Ed25519 key and stores the signature.
    pub fn sign(&mut self, key: &ed25519_dalek::SigningKey) {
        self.digest = digest_of(&self.manifest_text, &self.files);
        self.signature = Some(Signature::sign(&self.digest, key));
    }

    /// Recomputes the digest after a manifest edit.
    pub fn set_manifest_text(&mut self, text: &str) -> Result<()> {
        self.manifest = Manifest::parse(text)?;
        self.manifest_text = text.to_owned();
        self.digest = digest_of(text, &self.files);
        Ok(())
    }
}

fn check_abi(name: &str) -> Result<()> {
    let mut parts = name.split('/');
    let _lib = parts.next();
    let abi = parts.next().unwrap_or_default();
    if abi != ABI {
        return Err(FabricError::with_detail(
            ErrorCode::AbiUnsupported,
            format!("this build only runs {ABI} native code"),
            format!("{name} is for {abi}"),
        ));
    }
    Ok(())
}

fn check_java(manifest: &Manifest, files: &BTreeMap<String, File>) -> Result<()> {
    let has_native = files
        .values()
        .any(|f| f.name.starts_with("lib/") && f.name.ends_with(".so"))
        || manifest.native_library().is_some();
    if files.keys().any(|name| name.ends_with(".class")) && !has_native {
        return Err(FabricError::with_detail(
            ErrorCode::UnsupportedJava,
            format!(
                "{} ships Java classes only; this build runs Rust mods (no JVM)",
                manifest.id
            ),
            "build a .fabricmod with lib/arm64-v8a/lib<id>.so, or use a data-only mod",
        ));
    }
    for mixin in &manifest.mixins {
        if mixin.config().ends_with(".class") {
            return Err(FabricError::with_detail(
                ErrorCode::MixinJavaUnsupported,
                "Java mixin classes cannot run in this build",
                mixin.config().to_owned(),
            ));
        }
    }
    Ok(())
}

fn check_icon(manifest: &Manifest, files: &BTreeMap<String, File>) -> Result<()> {
    let Some(path) = &manifest.icon else {
        return Ok(());
    };
    let Some(file) = files.get(path) else {
        return Err(FabricError::with_detail(
            ErrorCode::IconInvalid,
            "the manifest names an icon that is not in the container",
            path.clone(),
        ));
    };
    let (width, height) = png_size(&file.data).ok_or_else(|| {
        FabricError::with_detail(
            ErrorCode::IconInvalid,
            "the icon is not a PNG",
            path.clone(),
        )
    })?;
    if width > MAX_ICON_EDGE || height > MAX_ICON_EDGE || width == 0 || height == 0 {
        return Err(FabricError::with_detail(
            ErrorCode::IconInvalid,
            "the icon must be at most 512x512",
            format!("{width}x{height}"),
        ));
    }
    Ok(())
}

/// Reads a PNG's dimensions from its IHDR chunk.
pub fn png_size(bytes: &[u8]) -> Option<(u32, u32)> {
    if bytes.len() < 24 || &bytes[..8] != b"\x89PNG\r\n\x1a\n" || &bytes[12..16] != b"IHDR" {
        return None;
    }
    let width = u32::from_be_bytes(bytes[16..20].try_into().ok()?);
    let height = u32::from_be_bytes(bytes[20..24].try_into().ok()?);
    Some((width, height))
}

/// The container digest: SHA-256 over the manifest and every payload entry,
/// sorted by path, with lengths so that concatenation is unambiguous.
pub fn digest_of(manifest_text: &str, files: &BTreeMap<String, File>) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(b"fabricmod\0");
    hasher.update((manifest_text.len() as u64).to_be_bytes());
    hasher.update(manifest_text.as_bytes());
    for (name, file) in files {
        if name == SIGNATURE_ENTRY {
            continue;
        }
        hasher.update((name.len() as u64).to_be_bytes());
        hasher.update(name.as_bytes());
        hasher.update((file.data.len() as u64).to_be_bytes());
        hasher.update(&file.data);
    }
    hasher.finalize().into()
}

/// Rejects a path that is not a plain relative path inside the container.
pub fn canonical_name(raw: &str) -> Result<String> {
    if raw.is_empty() || raw.len() > 512 {
        return Err(FabricError::with_detail(
            ErrorCode::ZipSlip,
            "an entry has no usable path",
            raw.to_owned(),
        ));
    }
    if raw.contains('\\') || raw.contains('\0') {
        return Err(FabricError::with_detail(
            ErrorCode::ZipSlip,
            "an entry path contains a forbidden character",
            raw.to_owned(),
        ));
    }
    if raw.starts_with('/') {
        return Err(FabricError::with_detail(
            ErrorCode::ZipSlip,
            "an entry path is absolute",
            raw.to_owned(),
        ));
    }
    let mut parts = Vec::new();
    for part in raw.split('/') {
        match part {
            "" | "." => continue,
            ".." => {
                return Err(FabricError::with_detail(
                    ErrorCode::ZipSlip,
                    "an entry path escapes the container",
                    raw.to_owned(),
                ));
            }
            part if part.ends_with(':') || part.contains(':') => {
                return Err(FabricError::with_detail(
                    ErrorCode::ZipSlip,
                    "an entry path names a drive",
                    raw.to_owned(),
                ));
            }
            part => parts.push(part),
        }
    }
    if parts.is_empty() {
        return Err(FabricError::with_detail(
            ErrorCode::ZipSlip,
            "an entry has no usable path",
            raw.to_owned(),
        ));
    }
    Ok(parts.join("/"))
}

/// Writes a deterministic zip: fixed timestamp, sorted names, stored already
/// compressed payloads.
pub fn write_container(manifest_text: &str, files: &BTreeMap<String, File>) -> Result<Vec<u8>> {
    use zip::write::SimpleFileOptions;
    use zip::{CompressionMethod, DateTime};
    if manifest_text.len() > MAX_MANIFEST {
        return Err(FabricError::new(
            ErrorCode::ManifestTooLarge,
            "fabric.mod.json is over 1 MiB",
        ));
    }
    let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
    let timestamp = DateTime::from_date_and_time(1980, 1, 1, 0, 0, 0).unwrap_or_default();
    let err = |e: zip::result::ZipError| {
        FabricError::with_detail(
            ErrorCode::ZipInvalid,
            "the container cannot be written",
            e.to_string(),
        )
    };
    let mut put = |name: &str, bytes: &[u8], executable: bool| -> Result<()> {
        let method = if is_compressed(name) {
            CompressionMethod::Stored
        } else {
            CompressionMethod::Deflated
        };
        let mut options = SimpleFileOptions::default()
            .compression_method(method)
            .last_modified_time(timestamp)
            .unix_permissions(if executable { 0o755 } else { 0o644 });
        if !is_compressed(name) {
            options = options.compression_level(Some(9));
        }
        zip.start_file(name, options).map_err(err)?;
        zip.write_all(bytes).map_err(|e| {
            FabricError::with_detail(
                ErrorCode::ZipInvalid,
                "the container cannot be written",
                e.to_string(),
            )
        })
    };
    put(MANIFEST_ENTRY, manifest_text.as_bytes(), false)?;
    for (name, file) in files {
        if name == MANIFEST_ENTRY || name == SIGNATURE_ENTRY {
            continue;
        }
        canonical_name(name)?;
        put(name, &file.data, file.executable)?;
    }
    if let Some(file) = files.get(SIGNATURE_ENTRY) {
        put(SIGNATURE_ENTRY, &file.data, false)?;
    }
    Ok(zip.finish().map_err(err)?.into_inner())
}

/// Extensions whose payload is already compressed.
pub fn is_compressed(name: &str) -> bool {
    matches!(
        name.rsplit('.')
            .next()
            .unwrap_or_default()
            .to_ascii_lowercase()
            .as_str(),
        "zip"
            | "jar"
            | "ogg"
            | "png"
            | "jpg"
            | "jpeg"
            | "webp"
            | "gz"
            | "xz"
            | "zst"
            | "mp3"
            | "mp4"
    )
}

/// A summary row for `horizon-ctl fabric list`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModSummary {
    pub id: String,
    pub name: String,
    pub version: String,
    pub environment: String,
    pub native: bool,
    pub signed: bool,
}

impl Container {
    pub fn summary(&self) -> ModSummary {
        ModSummary {
            id: self.manifest.id.clone(),
            name: self.manifest.display_name().to_owned(),
            version: self.manifest.version.clone(),
            environment: self.manifest.environment.as_str().to_owned(),
            native: self.native_library().is_some(),
            signed: self.signature.is_some(),
        }
    }
}
