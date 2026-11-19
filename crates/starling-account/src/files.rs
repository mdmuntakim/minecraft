//! The game's official files, fetched from Mojang's servers, each checked against the hash its
//! manifest gives, as the official launcher does. Starling pins the version JSON itself (`research-lock.json`), so the
//! chain of hashes starts from Starling's own files. Nothing of the game ships with Starling:
//! the files land on the player's device, in the official launcher's layout
//! (`versions/<v>/<v>.jar`, `libraries/…`, `assets/indexes|objects`) that horizon-prepare reads.
use serde_json::Value;
#[cfg(not(target_os = "android"))]
use sha1::{Digest, Sha1};
#[cfg(not(target_os = "android"))]
use std::io::{Read, Write};
#[cfg(not(target_os = "android"))]
use std::path::Path;
use std::path::PathBuf;
#[cfg(not(target_os = "android"))]
use std::sync::Mutex;
#[cfg(not(target_os = "android"))]
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
#[cfg(not(target_os = "android"))]
use std::time::Duration;

/// What `research-lock.json` pins about the game files.
#[derive(Clone, Debug)]
pub struct Pins {
    pub version: String,
    /// The version JSON's sha1, then the earlier ones Mojang republished.
    pub manifests: Vec<String>,
    pub client_sha1: String,
    pub server_sha1: String,
}

impl Pins {
    /// Starling's own pins, built in.
    pub fn starling() -> Pins {
        let lock: Value = serde_json::from_str(include_str!("../../../research-lock.json"))
            .expect("research-lock.json is JSON");
        Pins::read(&lock).expect("research-lock.json pins the game")
    }
    pub fn read(lock: &Value) -> Option<Pins> {
        let game = &lock["minecraft"];
        let text = |key: &str| game[key].as_str().map(String::from);
        let mut manifests = vec![text("manifest_sha1")?];
        if let Some(list) = game["accepted_manifest_sha1"].as_array() {
            manifests.extend(list.iter().filter_map(Value::as_str).map(String::from));
        }
        Some(Pins {
            version: text("version")?,
            manifests,
            client_sha1: text("client_sha1")?,
            server_sha1: text("server_sha1")?,
        })
    }
}

/// Mojang's servers, or a mock in tests.
#[derive(Clone, Debug)]
pub struct Sources {
    /// piston-meta: the version JSONs by their sha1.
    pub meta: String,
    /// The asset objects by hash.
    pub resources: String,
}

/// The base this run downloads the game's files from: the Mojang host the file is on, or the
/// owner's `mdmuntakim/minecraft-assets` repository when `HORIZON_MINECRAFT_DOWNLOAD_BASE` holds
/// the base to download from instead. The files keep the paths they have on Mojang's hosts, so
/// one base serves the version JSONs, the jars and the asset objects.
#[cfg(not(target_os = "android"))]
pub fn download_base(host: &str) -> String {
    std::env::var("HORIZON_MINECRAFT_DOWNLOAD_BASE")
        .ok()
        .filter(|b| !b.is_empty())
        .unwrap_or_else(|| host.to_owned())
}

/// The version files' base: Mojang's version list, or the owner's `minecraft-assets`.
#[cfg(not(target_os = "android"))]
pub fn version_base() -> String {
    download_base("https://piston-meta.mojang.com")
}

/// The game jars' base: Mojang's version data, or the owner's `minecraft-assets`.
#[cfg(not(target_os = "android"))]
pub fn jar_base() -> String {
    download_base("https://piston-data.mojang.com")
}

/// The asset objects' base: Mojang's assets, or the owner's `minecraft-assets`.
#[cfg(not(target_os = "android"))]
pub fn objects_base() -> String {
    download_base("https://resources.download.minecraft.net")
}

/// One Mojang URL of this run, from the base this run downloads from: the host the file is on
/// picks the base, the rest of the path comes along (`/v1/packages/<sha1>/<version>.json`,
/// `/assets/<xx>/<hash>`, `/client.jar`, ...). A URL on another host is the player's own and is
/// taken as it is.
#[cfg(not(target_os = "android"))]
pub fn mojang_url(url: &str) -> String {
    let version = version_base();
    let jar = jar_base();
    let objects = objects_base();
    let (base, rest) = if url.starts_with("https://piston-meta.mojang.com") {
        (
            version.as_str(),
            &url["https://piston-meta.mojang.com".len()..],
        )
    } else if url.starts_with("https://piston-data.mojang.com") {
        (jar.as_str(), &url["https://piston-data.mojang.com".len()..])
    } else if url.starts_with("https://resources.download.minecraft.net") {
        (
            objects.as_str(),
            &url["https://resources.download.minecraft.net".len()..],
        )
    } else {
        return url.to_owned();
    };
    format!("{}{rest}", base.trim_end_matches('/'))
}

/// Mojang's servers, or a mock in tests: the base this run downloads the game's files
/// from, which the owner's `mdmuntakim/minecraft-assets` repository answers when
/// `HORIZON_MINECRAFT_DOWNLOAD_BASE` holds the base to download from instead. The offline
/// APK never calls it: its files come out of the APK (crates/horizon-client/src/bundled.rs).
#[cfg(not(target_os = "android"))]
impl Sources {
    pub fn live() -> Self {
        Self {
            meta: version_base(),
            resources: objects_base(),
        }
    }
}

/// Which files to fetch beyond the version JSON and the client jar.
#[derive(Clone, Copy, Debug)]
pub struct Wanted {
    pub server: bool,
    pub libraries: bool,
    pub assets: bool,
}

/// How far a download is: what the progress bar shows.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Progress {
    pub files: usize,
    pub files_total: usize,
    pub bytes: u64,
    pub bytes_total: u64,
}

/// Where the fetched files are.
#[derive(Clone, Debug)]
pub struct Installed {
    /// The launcher-layout root (holding `versions`, `libraries`, `assets`).
    pub root: PathBuf,
    pub client_jar: PathBuf,
    pub server_jar: Option<PathBuf>,
}

/// One file to fetch: where from, where to, and what it must hash to.
#[cfg(not(target_os = "android"))]
struct Item {
    url: String,
    path: PathBuf,
    sha1: String,
    size: u64,
}

/// One agent for every download of the run: its connections stay open and are used again, so
/// the thousands of small asset files go over the download workers' few connections instead of
/// a connection and a TLS handshake each, which a phone's network (and its router) does not
/// keep up with for long.
#[cfg(not(target_os = "android"))]
pub(crate) fn agent() -> ureq::Agent {
    static AGENT: std::sync::OnceLock<ureq::Agent> = std::sync::OnceLock::new();
    AGENT
        .get_or_init(|| {
            ureq::AgentBuilder::new()
                .timeout_connect(Duration::from_secs(20))
                .timeout_read(Duration::from_secs(60))
                // A connection kept for each download worker, and some to spare.
                .max_idle_connections_per_host(8)
                .user_agent(&format!("starling/{}", env!("CARGO_PKG_VERSION")))
                .build()
        })
        .clone()
}

#[cfg(not(target_os = "android"))]
pub(crate) fn sha1_hex(data: &[u8]) -> String {
    let digest = Sha1::digest(data);
    digest.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(not(target_os = "android"))]
fn sha1_file(path: &Path) -> Option<String> {
    let mut file = std::fs::File::open(path).ok()?;
    let mut hasher = Sha1::new();
    let mut buf = vec![0; 1 << 16];
    loop {
        let n = file.read(&mut buf).ok()?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Some(
        hasher
            .finalize()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect(),
    )
}

/// How a try at a download failed: worth another (the network dropped for a moment, as a
/// phone's does when it changes networks, or the server was busy), or not.
#[cfg(not(target_os = "android"))]
pub(crate) enum Failure {
    Again(String),
    Final(String),
}

/// Tries before a download's error is the player's.
#[cfg(not(target_os = "android"))]
const TRIES: u32 = 5;
/// The wait before the second try; it doubles before each one after.
#[cfg(not(target_os = "android"))]
const FIRST_WAIT: Duration = Duration::from_secs(1);

/// Whether a server's refusal may pass with time: its own trouble, or its limit on this address.
/// A mobile network shares one address among many phones, and GitHub answers those with 403 or 429.
#[cfg(not(target_os = "android"))]
pub(crate) fn may_pass(code: u16) -> bool {
    code >= 500 || matches!(code, 403 | 408 | 429)
}

/// `attempt` until it succeeds, a final failure, or [`TRIES`] tries.
#[cfg(not(target_os = "android"))]
pub(crate) fn retrying<T>(mut attempt: impl FnMut() -> Result<T, Failure>) -> Result<T, String> {
    let mut wait = FIRST_WAIT;
    for _ in 1..TRIES {
        match attempt() {
            Ok(done) => return Ok(done),
            Err(Failure::Final(e)) => return Err(e),
            Err(Failure::Again(_)) => {
                std::thread::sleep(wait);
                wait *= 2;
            }
        }
    }
    attempt().map_err(|(Failure::Again(e) | Failure::Final(e))| e)
}

/// `url`'s body, which must hash to `sha1`; the error names the file, never more of the URL.
#[cfg(not(target_os = "android"))]
fn fetch_checked(url: &str, sha1: &str, name: &str) -> Result<Vec<u8>, String> {
    let body = retrying(|| {
        let response = agent().get(url).call().map_err(|e| match e {
            ureq::Error::Status(code, _) => {
                if may_pass(code) {
                    Failure::Again(format!(
                        "Mojang's server refused {name} ({code}). Try again in a few minutes."
                    ))
                } else {
                    Failure::Final(format!("Mojang's server refused {name} ({code})."))
                }
            }
            ureq::Error::Transport(t) => Failure::Again(format!(
                "Could not download {name} ({}). Check your internet connection.",
                crate::transport_detail(&t)
            )),
        })?;
        let mut body = Vec::new();
        response
            .into_reader()
            .read_to_end(&mut body)
            .map_err(|_| Failure::Again(format!("The download of {name} was cut off.")))?;
        Ok(body)
    })?;
    match sha1_hex(&body) {
        actual if actual == sha1 => Ok(body),
        _ => Err(format!(
            "{name} is not the file Mojang lists (its hash differs)."
        )),
    }
}

/// Write through a `.part` file, so a cut-off write never looks finished.
#[cfg(not(target_os = "android"))]
fn write_file(path: &Path, data: &[u8]) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    let mut part = path.as_os_str().to_owned();
    part.push(".part");
    let part = PathBuf::from(part);
    let mut file = std::fs::File::create(&part).map_err(|e| format!("{}: {e}", part.display()))?;
    file.write_all(data)
        .and_then(|()| file.sync_all())
        .map_err(|e| format!("{}: {e}", part.display()))?;
    drop(file);
    std::fs::rename(&part, path).map_err(|e| format!("{}: {e}", path.display()))
}

/// A library applies on the device: Mojang's rules for the OS (a phone counts as Linux, as
/// horizon-prepare reads it), and no natives classifiers (artifacts only).
#[cfg(not(target_os = "android"))]
fn library_applies(library: &Value) -> bool {
    let Some(rules) = library["rules"].as_array() else {
        return true;
    };
    let mut allowed = false;
    for rule in rules {
        let matches = rule["os"]["name"]
            .as_str()
            .is_none_or(|name| name == "linux");
        if matches {
            allowed = rule["action"].as_str() == Some("allow");
        }
    }
    allowed
}

/// Fetch the pinned version's files into `root`, in the official launcher's layout: what is
/// there already with the right hash is kept, so an interrupted download resumes. The version
/// JSON must be one Starling pins; every other file is checked against the hash it lists.
#[cfg(not(target_os = "android"))]
pub fn download(
    sources: &Sources,
    pins: &Pins,
    root: &Path,
    wanted: Wanted,
    progress: &(dyn Fn(Progress) + Sync),
    cancelled: &(dyn Fn() -> bool + Sync),
) -> Result<Installed, String> {
    let v = &pins.version;
    let version_dir = root.join("versions").join(v);
    let manifest_path = version_dir.join(format!("{v}.json"));
    let manifest = match std::fs::read(&manifest_path) {
        Ok(data) if pins.manifests.contains(&sha1_hex(&data)) => data,
        _ => {
            let sha1 = &pins.manifests[0];
            let data = fetch_checked(
                &format!("{}/v1/packages/{sha1}/{v}.json", sources.meta),
                sha1,
                &format!("the {v} version file"),
            )?;
            write_file(&manifest_path, &data)?;
            data
        }
    };
    let json: Value = serde_json::from_slice(&manifest)
        .map_err(|_| format!("The {v} version file is not valid JSON."))?;
    let download = |key: &str, path: PathBuf, pinned: &str| -> Result<Item, String> {
        let d = &json["downloads"][key];
        let (Some(url), Some(sha1)) = (d["url"].as_str(), d["sha1"].as_str()) else {
            return Err(format!("The {v} version file lists no {key} download."));
        };
        if sha1 != pinned {
            return Err(format!(
                "The {v} version file lists another {key} jar than Starling pins."
            ));
        }
        Ok(Item {
            url: mojang_url(url),
            path,
            sha1: sha1.into(),
            size: d["size"].as_u64().unwrap_or(0),
        })
    };
    let client_jar = version_dir.join(format!("{v}.jar"));
    let mut items = vec![download("client", client_jar.clone(), &pins.client_sha1)?];
    let server_jar = wanted
        .server
        .then(|| version_dir.join(format!("{v}-server.jar")));
    if let Some(path) = &server_jar {
        items.push(download("server", path.clone(), &pins.server_sha1)?);
    }
    if wanted.libraries {
        for library in json["libraries"].as_array().into_iter().flatten() {
            let artifact = &library["downloads"]["artifact"];
            let (Some(path), Some(url), Some(sha1)) = (
                artifact["path"].as_str(),
                artifact["url"].as_str(),
                artifact["sha1"].as_str(),
            ) else {
                continue;
            };
            if !library_applies(library) || path.contains("..") {
                continue;
            }
            items.push(Item {
                url: mojang_url(url),
                path: root.join("libraries").join(path),
                sha1: sha1.into(),
                size: artifact["size"].as_u64().unwrap_or(0),
            });
        }
    }
    if wanted.assets {
        let index = &json["assetIndex"];
        let (Some(id), Some(url), Some(sha1)) = (
            index["id"].as_str(),
            index["url"].as_str(),
            index["sha1"].as_str(),
        ) else {
            return Err(format!("The {v} version file lists no asset index."));
        };
        let index_path = root.join("assets/indexes").join(format!("{id}.json"));
        let data = match std::fs::read(&index_path) {
            Ok(data) if sha1_hex(&data) == sha1 => data,
            _ => {
                let data = fetch_checked(&mojang_url(url), sha1, "the asset index")?;
                write_file(&index_path, &data)?;
                data
            }
        };
        let objects: Value =
            serde_json::from_slice(&data).map_err(|_| "The asset index is not valid JSON.")?;
        for object in objects["objects"]
            .as_object()
            .into_iter()
            .flat_map(|o| o.values())
        {
            let Some(hash) = object["hash"]
                .as_str()
                .filter(|h| h.len() == 40 && h.bytes().all(|b| b.is_ascii_hexdigit()))
            else {
                continue;
            };
            items.push(Item {
                url: format!("{}/{}/{hash}", sources.resources, &hash[..2]),
                path: root.join("assets/objects").join(&hash[..2]).join(hash),
                sha1: hash.into(),
                size: object["size"].as_u64().unwrap_or(0),
            });
        }
        items.sort_by(|a, b| a.path.cmp(&b.path));
        items.dedup_by(|a, b| a.path == b.path);
    }
    fetch_all(items, progress, cancelled)?;
    Ok(Installed {
        root: root.to_path_buf(),
        client_jar,
        server_jar,
    })
}

/// Every item, a few at a time; what is already there with its hash counts as done.
#[cfg(not(target_os = "android"))]
fn fetch_all(
    items: Vec<Item>,
    progress: &(dyn Fn(Progress) + Sync),
    cancelled: &(dyn Fn() -> bool + Sync),
) -> Result<(), String> {
    let total = Progress {
        files_total: items.len(),
        bytes_total: items.iter().map(|i| i.size).sum(),
        ..Progress::default()
    };
    let (files, bytes) = (AtomicU64::new(0), AtomicU64::new(0));
    let report = |size: u64| {
        let files = files.fetch_add(1, Ordering::Relaxed) + 1;
        let bytes = bytes.fetch_add(size, Ordering::Relaxed) + size;
        progress(Progress {
            files: files as usize,
            bytes,
            ..total
        });
    };
    progress(total);
    let queue = Mutex::new(items);
    let failed: Mutex<Option<String>> = Mutex::new(None);
    let stop = AtomicBool::new(false);
    std::thread::scope(|scope| {
        for _ in 0..4 {
            scope.spawn(|| {
                loop {
                    if stop.load(Ordering::Relaxed) || cancelled() {
                        stop.store(true, Ordering::Relaxed);
                        return;
                    }
                    let Some(item) = queue.lock().unwrap().pop() else {
                        return;
                    };
                    let name = item
                        .path
                        .file_name()
                        .map_or_else(String::new, |n| n.to_string_lossy().into_owned());
                    let result = if sha1_file(&item.path).as_deref() == Some(item.sha1.as_str()) {
                        Ok(())
                    } else {
                        fetch_checked(&item.url, &item.sha1, &name)
                            .and_then(|data| write_file(&item.path, &data))
                    };
                    match result {
                        Ok(()) => report(item.size),
                        Err(e) => {
                            failed.lock().unwrap().get_or_insert(e);
                            stop.store(true, Ordering::Relaxed);
                            return;
                        }
                    }
                }
            });
        }
    });
    if let Some(e) = failed.into_inner().unwrap() {
        return Err(e);
    }
    if cancelled() {
        return Err("The download was cancelled.".into());
    }
    Ok(())
}

#[cfg(all(test, not(target_os = "android")))]
mod tests;
