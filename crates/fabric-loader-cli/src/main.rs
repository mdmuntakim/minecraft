//! `horizon-ctl fabric ...`: the `.fabricmod` toolchain.
//!
//! ```text
//! horizon-ctl fabric list    [--dir DIR] [--json]
//! horizon-ctl fabric verify  [--dir DIR] [--game VER] [--loader VER] [--frozen]
//! horizon-ctl fabric pack    --out FILE --name NAME --version VER [--dir DIR] [--config DIR]
//! horizon-ctl fabric resolve [--dir DIR] [--write-lock] [--frozen]
//! horizon-ctl fabric tree    [--dir DIR] [--mod ID]
//! horizon-ctl fabric migrate [--dir DIR] [--force]
//! horizon-ctl fabric new     --id ID [--name NAME] [--version VER] [--out DIR]
//! horizon-ctl fabric catalog [--file JSON] [--query TEXT]
//! ```

use fabric_loader::{
    catalog::{Catalog, Filter},
    compat, discovery,
    entrypoints::EntrypointTable,
    lockfile::LockFile,
    modpack, resolve,
};
use fabric_mod_format::{
    Manifest,
    container::{Container, EXTENSION, MANIFEST_ENTRY},
    migration,
};
use std::{path::PathBuf, process::ExitCode};

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match run(&args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(code) => {
            eprintln!("{code}");
            ExitCode::FAILURE
        }
    }
}

fn run(args: &[String]) -> Result<(), String> {
    if args.is_empty() || args[0] != "fabric" {
        return Err(usage());
    }
    let args = &args[1..];
    let Some(command) = args.first() else {
        return Err(usage());
    };
    let options = Options::parse(&args[1..])?;
    match command.as_str() {
        "list" => list(&options),
        "verify" => verify(&options),
        "pack" => pack(&options),
        "modpack" => modpack_cmd(&options),
        "resolve" => resolve_cmd(&options),
        "tree" => tree(&options),
        "migrate" => migrate(&options),
        "new" => new(&options),
        "catalog" => catalog(&options),
        "help" | "--help" | "-h" => {
            println!("{}", usage());
            Ok(())
        }
        other => Err(format!("unknown command {other:?}\n{}", usage())),
    }
}

fn usage() -> String {
    "usage: horizon-ctl fabric <list|verify|pack|modpack|resolve|tree|migrate|new|catalog> [options]\n\
     \n\
     global: --dir DIR   the mods folder (default: ./mods)\n\
             --json      machine-readable output\n\
     verify: --game VER --loader VER --frozen --hash\n\
     pack:   --dir MOD_FOLDER --out FILE\n\
     modpack:--out FILE --name NAME --version VER [--dir MODS] [--config DIR]\n\
     resolve:--write-lock --frozen\n\
     tree:   --mod ID\n\
     migrate:--force\n\
     new:    --id ID [--name NAME] [--version VER] [--out DIR]
     pack:   --sign HEX (optional Ed25519 seed)\n\
     catalog:--file JSON [--query TEXT] [--tag TAG]"
        .into()
}

struct Options {
    dir: PathBuf,
    json: bool,
    values: std::collections::BTreeMap<String, String>,
    flags: std::collections::BTreeSet<String>,
}

impl Options {
    fn parse(args: &[String]) -> Result<Self, String> {
        let mut options = Self {
            dir: PathBuf::from("mods"),
            json: false,
            values: Default::default(),
            flags: Default::default(),
        };
        let mut index = 0;
        while index < args.len() {
            let arg = &args[index];
            if arg == "--json" {
                options.json = true;
                index += 1;
                continue;
            }
            if let Some(name) = arg.strip_prefix("--") {
                if name == "dir" {
                    let value = args
                        .get(index + 1)
                        .ok_or("--dir needs a value")?;
                    options.dir = PathBuf::from(value);
                    index += 2;
                    continue;
                }
                if matches!(name, "frozen" | "force" | "write-lock" | "hash") {
                    options.flags.insert(name.to_owned());
                    index += 1;
                    continue;
                }
                let value = args
                    .get(index + 1)
                    .ok_or_else(|| format!("--{name} needs a value"))?;
                options.values.insert(name.to_owned(), value.clone());
                index += 2;
                continue;
            }
            return Err(format!("unexpected argument {arg:?}"));
        }
        Ok(options)
    }

    fn value(&self, name: &str) -> Option<&str> {
        self.values.get(name).map(String::as_str)
    }

    fn flag(&self, name: &str) -> bool {
        self.flags.contains(name)
    }
}

fn print_json(value: &serde_json::Value) -> Result<(), String> {
    println!(
        "{}",
        serde_json::to_string_pretty(value).map_err(|e| e.to_string())?
    );
    Ok(())
}

fn load(options: &Options) -> Result<(discovery::Discovery, resolve::Resolution), String> {
    let found = discovery::discover(&options.dir);
    if let Some(failure) = found.failures.first() {
        return Err(format!(
            "{}: {}",
            failure.path.display(),
            failure.error.log_line()
        ));
    }
    let resolution = resolve::resolve(&found).map_err(|e| e.log_line())?;
    Ok((found, resolution))
}

fn list(options: &Options) -> Result<(), String> {
    let (found, resolution) = load(options)?;
    if options.json {
        let rows: Vec<_> = found
            .mods
            .iter()
            .map(|m| serde_json::to_value(m.container.summary()).unwrap_or_default())
            .collect();
        return print_json(&serde_json::Value::Array(rows));
    }
    if found.mods.is_empty() {
        println!("no .{EXTENSION} mods in {}", options.dir.display());
    }
    for mod_ in &found.mods {
        let summary = mod_.container.summary();
        println!(
            "{} {}  [{}]{}",
            summary.id,
            summary.version,
            summary.environment,
            if summary.signed { "  signed" } else { "" }
        );
        if let Some(name) = &mod_.container.manifest.name {
            if name != &summary.id {
                println!("  {name}");
            }
        }
    }
    if !found.legacy.is_empty() {
        println!(
            "{} legacy file(s) can be migrated with `horizon-ctl fabric migrate`",
            found.legacy.len()
        );
    }
    println!(
        "{} mod(s), load order: {}",
        resolution.order.len(),
        resolution.order.join(" -> ")
    );
    Ok(())
}

fn verify(options: &Options) -> Result<(), String> {
    let found = discovery::discover(&options.dir);
    let game = options.value("game").unwrap_or("1.21.4");
    let loader = options
        .value("loader")
        .unwrap_or(fabric_mod_format::VERSION);
    let report = compat::check(&found, game, loader);
    let mut failed = false;
    if options.json {
        print_json(&serde_json::to_value(&report).map_err(|e| e.to_string())?)?;
    } else {
        for line in report.lines() {
            println!("{line}");
        }
    }
    failed |= !report.is_compatible();
    if let Some(store_path) = options.value("trust-store") {
        let mut store = fabric_mod_format::TrustStore::read(std::path::Path::new(store_path))
            .map_err(|e| e.log_line())?;
        for mod_ in &found.mods {
            match mod_.container.verify_signature(&mut store, |_| false) {
                Ok(trust) => {
                    if !options.json {
                        println!("{}: signed ({trust:?})", mod_.id());
                    }
                }
                Err(error) => {
                    failed = true;
                    if !options.json {
                        println!("{}: {}", mod_.id(), error.log_line());
                    }
                }
            }
        }
    }
    if let Ok(resolution) = resolve::resolve(&found) {
        if options.flag("frozen") {
            if let Err(error) = LockFile::enforce(&options.dir, &resolution, true) {
                failed = true;
                if !options.json {
                    println!("{}", error.log_line());
                }
            }
        }
        for mod_ in &found.mods {
            if let Err(error) = EntrypointTable::inspect(&mod_.container) {
                failed = true;
                if !options.json {
                    println!("{}: {}", mod_.id(), error.log_line());
                }
            }
        }
    }
    if failed {
        return Err("verification failed".into());
    }
    if !options.json {
        println!("{} mod(s) verified", found.mods.len());
    }
    Ok(())
}

fn resolve_cmd(options: &Options) -> Result<(), String> {
    let (found, resolution) = load(options)?;
    if options.flag("frozen") || options.flag("write-lock") {
        let lock = LockFile::from_resolution(&resolution);
        if options.flag("frozen") {
            LockFile::enforce(&options.dir, &resolution, true).map_err(|e| e.log_line())?;
        }
        if options.flag("write-lock") {
            lock.write(&options.dir.join(fabric_loader::lockfile::LOCKFILE))
                .map_err(|e| e.log_line())?;
        }
    }
    if options.json {
        return print_json(&serde_json::to_value(&resolution).map_err(|e| e.to_string())?);
    }
    for (index, id) in resolution.order.iter().enumerate() {
        let mod_ = &resolution.mods[id];
        println!(
            "{:>3}. {} {}  ({})",
            index + 1,
            id,
            mod_.version,
            mod_.sha256
        );
        for dependency in &mod_.dependencies {
            println!("     needs {dependency}");
        }
    }
    if resolution.order.len() != found.mods.len() {
        return Err("not every mod resolved".into());
    }
    Ok(())
}

fn tree(options: &Options) -> Result<(), String> {
    let (found, resolution) = load(options)?;
    let signed = Default::default();
    let enabled = Default::default();
    let capabilities = Default::default();
    let manager = fabric_loader::manager::ModManager::from_resolution(
        &resolution,
        &signed,
        &enabled,
        &capabilities,
    );
    let ids: Vec<String> = match options.value("mod") {
        Some(id) => vec![id.to_owned()],
        None => resolution.order.clone(),
    };
    for id in ids {
        println!(
            "{}",
            manager.dependency_tree(&id).map_err(|e| e.log_line())?
        );
    }
    if found.mods.is_empty() {
        println!("no mods");
    }
    Ok(())
}

fn migrate(options: &Options) -> Result<(), String> {
    let report = migration::migrate_dir(&options.dir, options.flag("force"));
    if options.json {
        print_json(&serde_json::to_value(&report).map_err(|e| e.to_string())?)?;
    } else {
        for migrated in &report.migrated {
            println!(
                "{} -> {} ({})",
                migrated.from.display(),
                migrated.to.display(),
                migrated.legacy_kind
            );
        }
        for failed in &report.failed {
            println!("{failed}");
        }
        println!("{}", report.summary());
    }
    if !report.failed.is_empty() {
        return Err(format!(
            "{} legacy file(s) could not be migrated",
            report.failed.len()
        ));
    }
    Ok(())
}

fn pack(options: &Options) -> Result<(), String> {
    let out = PathBuf::from(options.value("out").ok_or("--out is required")?);
    if !options.dir.join(MANIFEST_ENTRY).is_file() {
        return Err(format!(
            "{} is not a mod folder (no {MANIFEST_ENTRY}); use `fabric modpack` for a set of mods",
            options.dir.display()
        ));
    }
    let mut container = Container::read_folder(&options.dir).map_err(|e| e.log_line())?;
    if let Some(seed) = options.value("sign") {
        let bytes = parse_seed(seed)?;
        container.sign(&fabric_mod_format::signature::signing_key(bytes));
    }
    container.write(&out).map_err(|e| e.log_line())?;
    let summary = container.summary();
    if options.json {
        print_json(&serde_json::json!({
            "path": out,
            "id": summary.id,
            "version": summary.version,
            "signed": summary.signed,
            "sha256": fabric_mod_format::signature::hex(&container.digest),
        }))?;
    } else {
        println!(
            "packed {} {} -> {}",
            summary.id,
            summary.version,
            out.display()
        );
    }
    Ok(())
}

fn parse_seed(text: &str) -> Result<[u8; 32], String> {
    let bytes = text.as_bytes();
    if bytes.len() != 64 {
        return Err("--sign takes a 64-character hex Ed25519 seed".into());
    }
    let mut seed = [0u8; 32];
    for (index, pair) in bytes.chunks_exact(2).enumerate() {
        let hi = (pair[0] as char)
            .to_digit(16)
            .ok_or("the seed is not hex")?;
        let lo = (pair[1] as char)
            .to_digit(16)
            .ok_or("the seed is not hex")?;
        seed[index] = (hi * 16 + lo) as u8;
    }
    Ok(seed)
}

fn modpack_cmd(options: &Options) -> Result<(), String> {
    let out = PathBuf::from(options.value("out").ok_or("--out is required")?);
    let name = options.value("name").ok_or("--name is required")?;
    let version = options.value("version").ok_or("--version is required")?;
    let config = options.value("config").map(PathBuf::from);
    let report = modpack::pack(&options.dir, config.as_deref(), &out, name, version)
        .map_err(|e| e.log_line())?;
    if options.json {
        print_json(&serde_json::json!({
            "path": report.path,
            "mods": report.mods,
            "configs": report.configs,
            "bytes": report.bytes,
        }))?;
    } else {
        println!(
            "wrote {} ({} mods, {} configs, {} bytes)",
            report.path.display(),
            report.mods,
            report.configs,
            report.bytes
        );
    }
    Ok(())
}

fn new(options: &Options) -> Result<(), String> {
    let id = options.value("id").ok_or("--id is required")?;
    let name = options.value("name").unwrap_or(id);
    let version = options.value("version").unwrap_or("0.1.0");
    let out = PathBuf::from(options.value("out").unwrap_or(".")).join(id);
    if out.exists() {
        return Err(format!("{} already exists", out.display()));
    }
    let manifest = serde_json::json!({
        "schemaVersion": 2,
        "formatVersion": 1,
        "id": id,
        "version": version,
        "name": name,
        "description": "A Rust Fabric mod",
        "environment": "*",
        "authors": [],
        "license": "MIT",
        "entrypoints": {"main": "main"},
        "depends": {"fabric-loader": ">=0.1.0"},
        "custom": {"horizon": {"capabilities": [], "native": format!("lib/arm64-v8a/lib{id}.so")}},
    });
    let text = serde_json::to_string_pretty(&manifest).map_err(|e| e.to_string())?;
    // Refuse to write a template the loader would reject.
    Manifest::parse(&text).map_err(|e| e.log_line())?;
    std::fs::create_dir_all(out.join("lib/arm64-v8a")).map_err(|e| e.to_string())?;
    std::fs::create_dir_all(out.join("assets").join(id)).map_err(|e| e.to_string())?;
    std::fs::create_dir_all(out.join("data").join(id)).map_err(|e| e.to_string())?;
    std::fs::write(out.join(MANIFEST_ENTRY), &text).map_err(|e| e.to_string())?;
    std::fs::write(
        out.join("src.rs"),
        "// Build a cdylib exporting `fabric_main`.\n#[no_mangle]\npub extern \"C\" fn fabric_main() {\n    // register blocks, items and data here\n}\n",
    )
    .map_err(|e| e.to_string())?;
    std::fs::write(
        out.join("README.md"),
        format!(
            "# {name}\n\nA Rust Fabric mod. Build `lib/arm64-v8a/lib{id}.so` from `src.rs`, then:\n\n```bash\nhorizon-ctl fabric pack --dir {} --out {id}.{EXTENSION} --name \"{name}\" --version {version}\n```\n",
            out.parent().unwrap_or(&out).display()
        ),
    )
    .map_err(|e| e.to_string())?;
    println!("created {}", out.display());
    Ok(())
}

fn catalog(options: &Options) -> Result<(), String> {
    let path = options.value("file").ok_or("--file is required")?;
    let catalog = Catalog::read(std::path::Path::new(path)).map_err(|e| e.log_line())?;
    let filter = Filter {
        query: options.value("query").unwrap_or("").to_owned(),
        tag: options.value("tag").map(str::to_owned),
        ..Filter::default()
    };
    let rows: Vec<_> = catalog
        .filter(&filter)
        .into_iter()
        .map(|entry| serde_json::to_value(entry).unwrap_or_default())
        .collect();
    if options.json {
        return print_json(&serde_json::Value::Array(rows));
    }
    for entry in catalog.filter(&filter) {
        println!(
            "{} {}  {}{}",
            entry.id,
            entry.version,
            entry.name,
            if entry.tags.is_empty() {
                String::new()
            } else {
                format!("  [{}]", entry.tags.join(", "))
            }
        );
    }
    Ok(())
}

// Keep the container type in this binary's public surface: the CLI reads and
// writes containers through the format crate's public API.
#[allow(dead_code)]
fn read_container(path: &std::path::Path) -> fabric_mod_format::Result<Container> {
    Container::read(path)
}
