//! An actionable compatibility report: every reason a mod cannot load, with
//! one sentence and one action each.

use crate::{
    discovery::Discovery,
    entrypoints::EntrypointTable,
    error::{ErrorCode, LoaderResult},
};
use fabric_mod_format::{Version, container::ABI, manifest::Manifest};
use serde::{Deserialize, Serialize};

/// One incompatibility.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Issue {
    pub code: ErrorCode,
    pub mod_id: String,
    pub reason: String,
    pub action: String,
}

/// The whole report.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompatibilityReport {
    pub issues: Vec<Issue>,
}

impl CompatibilityReport {
    pub fn is_compatible(&self) -> bool {
        self.issues.is_empty()
    }

    /// Lines for the launcher's "why won't it load" sheet.
    pub fn lines(&self) -> Vec<String> {
        self.issues
            .iter()
            .map(|issue| format!("{}: {} — {}", issue.mod_id, issue.reason, issue.action))
            .collect()
    }
}

/// Checks a discovered set against this build.
pub fn check(
    discovery: &Discovery,
    game_version: &str,
    loader_version: &str,
) -> CompatibilityReport {
    let mut report = CompatibilityReport::default();
    for failure in &discovery.failures {
        report.issues.push(Issue {
            code: failure.error.code,
            mod_id: failure
                .path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("?")
                .to_owned(),
            reason: failure.error.message.clone(),
            action: failure
                .error
                .detail
                .clone()
                .unwrap_or_else(|| "replace or remove the file".into()),
        });
    }
    let Ok(game) = Version::parse(game_version) else {
        report.issues.push(Issue {
            code: ErrorCode::VersionInvalid,
            mod_id: "game".into(),
            reason: format!("the game version {game_version:?} is not a version"),
            action: "build with a semantic game version".into(),
        });
        return report;
    };
    let loader = Version::parse(loader_version).ok();

    for discovered in &discovery.mods {
        let manifest = &discovered.container.manifest;
        let id = manifest.id.clone();
        if let Err(error) = EntrypointTable::inspect(&discovered.container) {
            report.issues.push(Issue {
                code: error.code,
                mod_id: id.clone(),
                reason: error.message.clone(),
                action: error
                    .detail
                    .clone()
                    .unwrap_or_else(|| "ask the author for an arm64 Rust build".into()),
            });
        }
        if let Err(error) = check_manifest(manifest, &game, loader.as_ref()) {
            report.issues.push(Issue {
                code: error.code,
                mod_id: id.clone(),
                reason: error.message.clone(),
                action: error.detail.clone().unwrap_or_default(),
            });
        }
        if let Some(native) = discovered.container.native_library()
            && !native.name.contains(&format!("/{ABI}/"))
        {
            report.issues.push(Issue {
                code: ErrorCode::AbiUnsupported,
                mod_id: id.clone(),
                reason: format!("native code is not {ABI}"),
                action: "install the arm64-v8a build".into(),
            });
        }
        // A `.class` file is the clearest sign of a Java mod.
        if discovered
            .container
            .files
            .keys()
            .any(|name| name.ends_with(".class"))
        {
            report.issues.push(Issue {
                code: ErrorCode::UnsupportedJava,
                mod_id: id.clone(),
                reason: "the mod ships Java classes".into(),
                action: "use a Rust-native or data-only .fabricmod".into(),
            });
        }
    }

    // Missing and incompatible dependencies, through the resolver.
    if let Err(error) = crate::resolve::resolve(discovery) {
        if !report.issues.iter().any(|issue| issue.code == error.code) {
            report.issues.push(Issue {
                code: error.code,
                mod_id: "mod set".into(),
                reason: error.message.clone(),
                action: error.detail.clone().unwrap_or_default(),
            });
        }
    }
    report
}

fn check_manifest(
    manifest: &Manifest,
    game: &Version,
    loader: Option<&Version>,
) -> LoaderResult<()> {
    if let Some(requirement) = manifest.depends.get("minecraft")
        && !matches_all(requirement, game)
    {
        return Err(crate::error::error_with(
            ErrorCode::VersionConflict,
            format!(
                "{} needs Minecraft {}, this build is {game}",
                manifest.id,
                requirement.text()
            ),
            "update the mod or the game".to_owned(),
        ));
    }
    if let Some(requirement) = manifest.depends.get("fabric-loader")
        && let Some(loader) = loader
        && !matches_all(requirement, loader)
    {
        return Err(crate::error::error_with(
            ErrorCode::VersionConflict,
            format!(
                "{} needs fabric-loader {}, this build is {loader}",
                manifest.id,
                requirement.text()
            ),
            "update the loader or the mod".to_owned(),
        ));
    }
    Ok(())
}

fn matches_all(requirement: &fabric_mod_format::Requirement, version: &Version) -> bool {
    requirement
        .ranges
        .iter()
        .all(|range| range.contains(version))
}

#[cfg(test)]
mod tests {
    use super::*;
    use fabric_mod_format::container::Container;
    use std::path::PathBuf;

    fn discovered(text: &str) -> crate::discovery::Discovered {
        crate::discovery::Discovered {
            container: Container::new(text).unwrap(),
            path: PathBuf::from("mem.fabricmod"),
            folder: false,
        }
    }

    #[test]
    fn a_clean_set_is_compatible() {
        let discovery = Discovery {
            mods: vec![discovered(
                r#"{"id":"a","version":"1.0.0","depends":{"minecraft":">=1.21"}}"#,
            )],
            ..Discovery::default()
        };
        let report = check(&discovery, "1.21.4", "0.16.0");
        assert!(report.is_compatible(), "{:?}", report.lines());
    }

    #[test]
    fn a_game_version_mismatch_names_both() {
        let discovery = Discovery {
            mods: vec![discovered(
                r#"{"id":"a","version":"1.0.0","depends":{"minecraft":"~1.20"}}"#,
            )],
            ..Discovery::default()
        };
        let report = check(&discovery, "1.21.4", "0.16.0");
        assert!(!report.is_compatible());
        let line = &report.lines()[0];
        assert!(
            line.contains("Minecraft ~1.20") && line.contains("1.21.4"),
            "{line}"
        );
    }

    #[test]
    fn java_and_missing_dependencies_are_explained() {
        let mut java = Container::new(
            r#"{"id":"java","version":"1.0.0","entrypoints":{"main":"bootstrap"},"custom":{"horizon":{"native":"lib/arm64-v8a/libjava.so"}}}"#,
        )
        .unwrap();
        // A real class file with a native library avoids the container's own
        // Java-only rejection, so the checker sees it.
        java.insert("lib/arm64-v8a/libjava.so", vec![0; 4]).unwrap();
        java.insert("com/example/Mod.class", vec![0xca, 0xfe])
            .unwrap();
        let discovery = Discovery {
            mods: vec![
                crate::discovery::Discovered {
                    container: java,
                    path: PathBuf::from("java.fabricmod"),
                    folder: false,
                },
                discovered(r#"{"id":"b","version":"1.0.0","depends":{"missing":">=1.0"}}"#),
            ],
            ..Discovery::default()
        };
        let report = check(&discovery, "1.21.4", "0.16.0");
        let text = report.lines().join("\n");
        assert!(text.contains("Java"), "{text}");
        assert!(text.contains("missing"), "{text}");
    }
}
