//! Crash attribution and safe mode.
//!
//! The loader records which mod is being initialised. A panic or abort is
//! attributed to the innermost mod on the stack, and the launcher offers to
//! start without mods. The marker is a file beside the mods so the offer
//! survives the crash.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// The marker's name.
pub const SAFE_MODE_FILE: &str = ".safe-mode.json";

/// A crash, attributed when possible.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CrashReport {
    /// The mod that was initialising, when the stack had one.
    pub mod_id: Option<String>,
    pub message: String,
    /// Every mod on the stack, innermost last.
    pub stack: Vec<String>,
    pub offer_start_without_mods: bool,
}

impl CrashReport {
    pub fn summary(&self) -> String {
        match &self.mod_id {
            Some(id) => format!("{id} crashed: {}", self.message),
            None => format!("the game crashed outside a mod: {}", self.message),
        }
    }
}

/// Tracks which mod is initialising.
#[derive(Clone, Debug, Default)]
pub struct CrashGuard {
    stack: Vec<String>,
    safe_mode: bool,
}

impl CrashGuard {
    pub fn new() -> Self {
        Self::default()
    }

    /// Starts initialising `mod_id`.
    pub fn begin(&mut self, mod_id: &str) {
        self.stack.push(mod_id.to_owned());
    }

    /// Finishes the innermost mod.
    pub fn end(&mut self) {
        self.stack.pop();
    }

    /// The mod currently initialising.
    pub fn current(&self) -> Option<&str> {
        self.stack.last().map(String::as_str)
    }

    /// Attributes a panic. `safe_mode` is true when this run already started
    /// without mods, in which case there is nothing left to disable.
    pub fn attribute(&mut self, message: &str, safe_mode: bool) -> CrashReport {
        self.safe_mode = safe_mode;
        CrashReport {
            mod_id: self.stack.last().cloned(),
            message: message.to_owned(),
            stack: self.stack.clone(),
            offer_start_without_mods: !safe_mode && !self.stack.is_empty(),
        }
    }

    pub fn is_safe_mode(&self) -> bool {
        self.safe_mode
    }
}

/// Writes the safe-mode marker.
pub fn mark_safe_mode(dir: &Path, report: &CrashReport) -> std::io::Result<PathBuf> {
    let path = dir.join(SAFE_MODE_FILE);
    let text = serde_json::to_string_pretty(report).unwrap_or_else(|_| "{}".into());
    std::fs::write(&path, text)?;
    Ok(path)
}

/// Reads the marker, if a previous run crashed.
pub fn safe_mode_report(dir: &Path) -> Option<CrashReport> {
    let text = std::fs::read_to_string(dir.join(SAFE_MODE_FILE)).ok()?;
    serde_json::from_str(&text).ok()
}

/// Clears the marker after a successful start.
pub fn clear_safe_mode(dir: &Path) -> std::io::Result<()> {
    match std::fs::remove_file(dir.join(SAFE_MODE_FILE)) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e),
    }
}

/// The mods a safe-mode start keeps. Code mods are disabled; data and
/// resource mods have no code to crash and stay on if `keep_data`.
pub fn safe_mode_set(discovery: &crate::discovery::Discovery, keep_data: bool) -> Vec<String> {
    discovery
        .mods
        .iter()
        .filter(|m| {
            if keep_data {
                m.container.native_library().is_none()
            } else {
                false
            }
        })
        .map(|m| m.id().to_owned())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use fabric_mod_format::container::Container;
    use std::path::PathBuf;

    #[test]
    fn a_panic_is_attributed_to_the_innermost_mod() {
        let mut guard = CrashGuard::new();
        guard.begin("outer");
        guard.begin("inner");
        let report = guard.attribute("index out of bounds", false);
        assert_eq!(report.mod_id.as_deref(), Some("inner"));
        assert_eq!(report.stack, ["outer", "inner"]);
        assert!(report.offer_start_without_mods);
        assert!(report.summary().contains("inner"));
        guard.end();
        assert_eq!(guard.current(), Some("outer"));
        guard.end();
        assert!(guard.current().is_none());
    }

    #[test]
    fn a_crash_outside_a_mod_offers_nothing() {
        let mut guard = CrashGuard::new();
        let report = guard.attribute("boom", false);
        assert!(report.mod_id.is_none());
        assert!(!report.offer_start_without_mods);
    }

    #[test]
    fn safe_mode_marker_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        assert!(safe_mode_report(dir.path()).is_none());
        let report = CrashReport {
            mod_id: Some("a".into()),
            message: "boom".into(),
            stack: vec!["a".into()],
            offer_start_without_mods: true,
        };
        mark_safe_mode(dir.path(), &report).unwrap();
        assert_eq!(safe_mode_report(dir.path()).unwrap(), report);
        clear_safe_mode(dir.path()).unwrap();
        assert!(safe_mode_report(dir.path()).is_none());
        clear_safe_mode(dir.path()).unwrap();
    }

    #[test]
    fn safe_mode_keeps_only_data_mods_when_asked() {
        let mut code = Container::new(
            r#"{"id":"code","version":"1.0.0","custom":{"horizon":{"native":"lib/arm64-v8a/libcode.so"}}}"#,
        )
        .unwrap();
        code.insert("lib/arm64-v8a/libcode.so", vec![0; 8]).unwrap();
        let data = Container::new(r#"{"id":"data","version":"1.0.0"}"#).unwrap();
        let discovery = crate::discovery::Discovery {
            mods: vec![
                crate::discovery::Discovered {
                    container: code,
                    path: PathBuf::from("code.fabricmod"),
                    folder: false,
                },
                crate::discovery::Discovered {
                    container: data,
                    path: PathBuf::from("data.fabricmod"),
                    folder: false,
                },
            ],
            ..crate::discovery::Discovery::default()
        };
        assert_eq!(safe_mode_set(&discovery, true), ["data"]);
        assert!(safe_mode_set(&discovery, false).is_empty());
    }
}
