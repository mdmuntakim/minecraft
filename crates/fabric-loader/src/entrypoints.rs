//! Entrypoints are Rust symbols in an arm64 ELF, never Java classes.
//!
//! A code mod exports `fabric_<name>` for each entrypoint it declares, or a
//! `fabric_register` table. The symbol scan is a bounds-checked ELF64 reader:
//! a hostile library yields an error, never a panic.

use crate::error::{ErrorCode, LoaderResult, error_with};
use fabric_mod_format::{Container, Manifest};
use std::collections::BTreeSet;

/// The prefix a Rust Fabric entrypoint exports.
pub const SYMBOL_PREFIX: &str = "fabric_";
/// A registration table instead of one symbol per entrypoint.
pub const REGISTER_SYMBOL: &str = "fabric_register";

/// The symbols found in a native library.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Symbols {
    pub names: BTreeSet<String>,
}

impl Symbols {
    /// Reads `SHT_SYMTAB` and `SHT_DYNSYM` from a little-endian ELF64.
    pub fn scan(elf: &[u8]) -> LoaderResult<Self> {
        if elf.len() < 64 || &elf[..4] != b"\x7fELF" {
            return Err(error_with(
                ErrorCode::AbiUnsupported,
                "the native library is not an ELF file",
                format!("{} bytes", elf.len()),
            ));
        }
        if elf[4] != 2 {
            return Err(error_with(
                ErrorCode::AbiUnsupported,
                "the native library is not 64-bit ELF",
                format!("ELF class {}", elf[4]),
            ));
        }
        if elf[5] != 1 {
            return Err(error_with(
                ErrorCode::AbiUnsupported,
                "the native library is not little-endian",
                format!("ELF data {}", elf[5]),
            ));
        }
        let machine = u16(elf, 18)?;
        if machine != 183 {
            return Err(error_with(
                ErrorCode::AbiUnsupported,
                "the native library is not AArch64",
                format!("ELF machine {machine}"),
            ));
        }
        let shoff = u64(elf, 0x28)?;
        let shentsize = u16(elf, 0x3a)? as usize;
        let shnum = u16(elf, 0x3c)? as usize;
        if shnum == 0 || shentsize < 64 {
            return Err(error_with(
                ErrorCode::AbiUnsupported,
                "the native library has no section table",
                format!("{shnum} sections of {shentsize} bytes"),
            ));
        }
        let table_end = (shoff as usize)
            .checked_add(shnum.saturating_mul(shentsize))
            .ok_or_else(|| {
                error_with(
                    ErrorCode::AbiUnsupported,
                    "the ELF section table overflows",
                    "",
                )
            })?;
        if table_end > elf.len() {
            return Err(error_with(
                ErrorCode::AbiUnsupported,
                "the ELF section table is outside the file",
                format!("{table_end} > {}", elf.len()),
            ));
        }
        let mut names = BTreeSet::new();
        for index in 0..shnum {
            let base = shoff as usize + index * shentsize;
            let kind = u32(elf, base + 4)?;
            if kind != 2 && kind != 11 {
                continue;
            }
            let offset = u64(elf, base + 24)? as usize;
            let size = u64(elf, base + 32)? as usize;
            let link = u32(elf, base + 40)? as usize;
            let entry_size = u64(elf, base + 56)? as usize;
            let entry_size = if entry_size == 0 { 24 } else { entry_size };
            if link >= shnum {
                continue;
            }
            let str_base = shoff as usize + link * shentsize;
            let str_offset = u64(elf, str_base + 24)? as usize;
            let str_size = u64(elf, str_base + 32)? as usize;
            let Some(strings) = elf.get(str_offset..str_offset.saturating_add(str_size)) else {
                continue;
            };
            if entry_size < 24 {
                continue;
            }
            let Some(table) = elf.get(offset..offset.saturating_add(size)) else {
                continue;
            };
            let count = table.len() / entry_size;
            for symbol in 0..count {
                let at = symbol * entry_size;
                let name_at = u32(table, at)? as usize;
                if name_at == 0 || name_at >= strings.len() {
                    continue;
                }
                let tail = &strings[name_at..];
                let end = tail.iter().position(|b| *b == 0).unwrap_or(tail.len());
                if let Ok(name) = std::str::from_utf8(&tail[..end])
                    && !name.is_empty()
                {
                    names.insert(name.to_owned());
                }
            }
        }
        Ok(Self { names })
    }

    pub fn has(&self, name: &str) -> bool {
        self.names.contains(name)
    }

    /// The entrypoint names the library exports.
    pub fn entrypoints(&self) -> BTreeSet<String> {
        self.names
            .iter()
            .filter_map(|name| name.strip_prefix(SYMBOL_PREFIX))
            .filter(|name| *name != "register")
            .map(str::to_owned)
            .collect()
    }
}

fn u16(bytes: &[u8], at: usize) -> LoaderResult<u16> {
    let slice = bytes.get(at..at + 2).ok_or_else(|| {
        error_with(
            ErrorCode::AbiUnsupported,
            "an ELF field is outside the file",
            at.to_string(),
        )
    })?;
    Ok(u16::from_le_bytes([slice[0], slice[1]]))
}

fn u32(bytes: &[u8], at: usize) -> LoaderResult<u32> {
    let slice = bytes.get(at..at + 4).ok_or_else(|| {
        error_with(
            ErrorCode::AbiUnsupported,
            "an ELF field is outside the file",
            at.to_string(),
        )
    })?;
    Ok(u32::from_le_bytes([slice[0], slice[1], slice[2], slice[3]]))
}

fn u64(bytes: &[u8], at: usize) -> LoaderResult<u64> {
    let slice = bytes.get(at..at + 8).ok_or_else(|| {
        error_with(
            ErrorCode::AbiUnsupported,
            "an ELF field is outside the file",
            at.to_string(),
        )
    })?;
    Ok(u64::from_le_bytes(slice.try_into().map_err(|_| {
        error_with(
            ErrorCode::AbiUnsupported,
            "an ELF field is truncated",
            at.to_string(),
        )
    })?))
}

/// A mod's declared entrypoints, checked against its native library.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct EntrypointTable {
    pub declared: BTreeSet<String>,
    pub exported: BTreeSet<String>,
    pub registration_table: bool,
    pub data_only: bool,
}

impl EntrypointTable {
    /// Reads the manifest and library, and refuses Java-only entrypoints.
    pub fn inspect(container: &Container) -> LoaderResult<Self> {
        let manifest = &container.manifest;
        let declared: BTreeSet<String> = manifest
            .entrypoints
            .values()
            .flatten()
            .map(|entry| entry.value().to_owned())
            .collect();
        let data_only = is_data_only(manifest);
        for name in &declared {
            // `com.example.Mod` is a Java class, not a Rust symbol.
            if name.contains('.') || name.contains('/') || name.ends_with(".class") {
                return Err(error_with(
                    ErrorCode::UnsupportedJava,
                    format!("{} declares the Java entrypoint {name}", manifest.id),
                    "this build has no JVM; export fabric_<name> from a Rust library",
                ));
            }
        }
        if declared.is_empty() {
            return Ok(Self {
                declared,
                data_only,
                ..Self::default()
            });
        }
        let Some(library) = container.native_library() else {
            if data_only {
                return Ok(Self {
                    declared,
                    data_only,
                    ..Self::default()
                });
            }
            return Err(error_with(
                ErrorCode::EntrypointMissing,
                format!(
                    "{} declares entrypoints but has no native library",
                    manifest.id
                ),
                "expected lib/arm64-v8a/lib<id>.so",
            ));
        };
        let symbols = Symbols::scan(&library.data)?;
        let exported = symbols.entrypoints();
        let registration_table =
            symbols.has(REGISTER_SYMBOL) || symbols.has("horizon_fabric_entrypoints");
        for name in &declared {
            if !exported.contains(name) && !registration_table {
                return Err(error_with(
                    ErrorCode::EntrypointMissing,
                    format!(
                        "{} declares {name} but its library does not export it",
                        manifest.id
                    ),
                    format!("expected {SYMBOL_PREFIX}{name} or {REGISTER_SYMBOL}"),
                ));
            }
        }
        Ok(Self {
            declared,
            exported,
            registration_table,
            data_only,
        })
    }

    pub fn has(&self, name: &str) -> bool {
        self.declared.contains(name) && (self.exported.contains(name) || self.registration_table)
    }
}

fn is_data_only(manifest: &Manifest) -> bool {
    manifest
        .horizon_custom()
        .and_then(|h| h.get("dataOnly"))
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use fabric_mod_format::container::Container;

    /// A minimal little-endian ELF64 with a `.dynsym` and a `.dynstr`.
    fn elf_with(symbols: &[&str]) -> Vec<u8> {
        let mut strings = vec![0u8];
        let mut names = Vec::new();
        for symbol in symbols {
            names.push(strings.len() as u32);
            strings.extend_from_slice(symbol.as_bytes());
            strings.push(0);
        }
        let shoff = 64usize;
        let shentsize = 64usize;
        let shnum = 3usize;
        let dynsym_off = shoff + shentsize * shnum;
        let dynsym_size = names.len() * 24;
        let dynstr_off = dynsym_off + dynsym_size;
        let total = dynstr_off + strings.len();
        let mut elf = vec![0u8; total];
        elf[..4].copy_from_slice(b"\x7fELF");
        elf[4] = 2;
        elf[5] = 1;
        elf[6] = 1;
        elf[16..18].copy_from_slice(&3u16.to_le_bytes()); // ET_DYN
        elf[18..20].copy_from_slice(&183u16.to_le_bytes()); // AArch64
        elf[0x28..0x30].copy_from_slice(&(shoff as u64).to_le_bytes());
        elf[0x3a..0x3c].copy_from_slice(&(shentsize as u16).to_le_bytes());
        elf[0x3c..0x3e].copy_from_slice(&(shnum as u16).to_le_bytes());
        // Section 1: .dynsym
        let sym = shoff + shentsize;
        elf[sym + 4..sym + 8].copy_from_slice(&11u32.to_le_bytes());
        elf[sym + 24..sym + 32].copy_from_slice(&(dynsym_off as u64).to_le_bytes());
        elf[sym + 32..sym + 40].copy_from_slice(&(dynsym_size as u64).to_le_bytes());
        elf[sym + 40..sym + 44].copy_from_slice(&2u32.to_le_bytes()); // link to .dynstr
        elf[sym + 56..sym + 64].copy_from_slice(&24u64.to_le_bytes());
        // Section 2: .dynstr
        let str_section = shoff + shentsize * 2;
        elf[str_section + 4..str_section + 8].copy_from_slice(&3u32.to_le_bytes());
        elf[str_section + 24..str_section + 32].copy_from_slice(&(dynstr_off as u64).to_le_bytes());
        elf[str_section + 32..str_section + 40]
            .copy_from_slice(&(strings.len() as u64).to_le_bytes());
        for (index, name_at) in names.iter().enumerate() {
            let at = dynsym_off + index * 24;
            elf[at..at + 4].copy_from_slice(&name_at.to_le_bytes());
            elf[at + 4] = 0x12; // GLOBAL FUNC
            elf[at + 6..at + 8].copy_from_slice(&1u16.to_le_bytes());
        }
        elf[dynstr_off..dynstr_off + strings.len()].copy_from_slice(&strings);
        elf
    }

    #[test]
    fn scans_symbols_and_entrypoints() {
        let elf = elf_with(&[
            "fabric_main",
            "fabric_preLaunch",
            "fabric_register",
            "other",
        ]);
        let symbols = Symbols::scan(&elf).unwrap();
        assert!(symbols.has("fabric_main"));
        assert!(symbols.has(REGISTER_SYMBOL));
        assert_eq!(
            symbols.entrypoints().into_iter().collect::<Vec<_>>(),
            ["main", "preLaunch"]
        );
    }

    #[test]
    fn hostile_libraries_never_panic() {
        for elf in [
            &b""[..],
            b"not elf",
            b"\x7fELF\x02\x01\x01",
            &[0u8; 128],
            b"\x7fELF\x01\x01\x01\x00\x00\x00\x00\x00\x00\x00\x00\x00",
        ] {
            let _ = Symbols::scan(elf);
        }
        // An AArch64 header that claims a section table past the end.
        let mut elf = vec![0u8; 64];
        elf[..4].copy_from_slice(b"\x7fELF");
        elf[4] = 2;
        elf[5] = 1;
        elf[18..20].copy_from_slice(&183u16.to_le_bytes());
        elf[0x28..0x30].copy_from_slice(&u64::MAX.to_le_bytes());
        elf[0x3a..0x3c].copy_from_slice(&64u16.to_le_bytes());
        elf[0x3c..0x3e].copy_from_slice(&1u16.to_le_bytes());
        assert!(Symbols::scan(&elf).is_err());
    }

    #[test]
    fn java_entrypoints_are_refused() {
        let text = r#"{"id":"a","version":"1.0.0","entrypoints":{"main":"com.example.Mod"}}"#;
        let container = Container::new(text).unwrap();
        let err = EntrypointTable::inspect(&container).unwrap_err();
        assert_eq!(err.code, ErrorCode::UnsupportedJava);
    }

    #[test]
    fn data_only_mods_without_a_library_are_accepted() {
        let text = r#"{"id":"a","version":"1.0.0","entrypoints":{"main":"bootstrap"},"custom":{"horizon":{"dataOnly":true}}}"#;
        let container = Container::new(text).unwrap();
        let table = EntrypointTable::inspect(&container).unwrap();
        assert!(table.data_only);
        assert!(table.declared.contains("bootstrap"));
    }

    #[test]
    fn a_declared_but_missing_symbol_is_named() {
        let mut container = Container::new(
            r#"{"id":"a","version":"1.0.0","entrypoints":{"main":"bootstrap"},"custom":{"horizon":{"native":"lib/arm64-v8a/liba.so"}}}"#,
        )
        .unwrap();
        container
            .insert("lib/arm64-v8a/liba.so", elf_with(&["fabric_other"]))
            .unwrap();
        let err = EntrypointTable::inspect(&container).unwrap_err();
        assert_eq!(err.code, ErrorCode::EntrypointMissing);
        assert!(err.detail.unwrap().contains("fabric_bootstrap"));
    }

    #[test]
    fn a_registration_table_covers_every_entrypoint() {
        let mut container = Container::new(
            r#"{"id":"a","version":"1.0.0","entrypoints":{"main":"bootstrap"},"custom":{"horizon":{"native":"lib/arm64-v8a/liba.so"}}}"#,
        )
        .unwrap();
        container
            .insert("lib/arm64-v8a/liba.so", elf_with(&["fabric_register"]))
            .unwrap();
        let table = EntrypointTable::inspect(&container).unwrap();
        assert!(table.registration_table);
        assert!(table.has("bootstrap"));
    }
}
