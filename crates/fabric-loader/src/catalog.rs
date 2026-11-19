//! The bundled offline mod catalog: search and filter with no network.

use crate::error::{ErrorCode, LoaderResult, error_with};
use serde::{Deserialize, Serialize};
use std::path::Path;

/// A catalog row.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CatalogEntry {
    pub id: String,
    pub name: String,
    pub version: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default = "any_environment")]
    pub environment: String,
    /// The file inside `assets/bundled/mods/`, or the player's own path.
    #[serde(default)]
    pub path: String,
    #[serde(default)]
    pub sha256: String,
    #[serde(default)]
    pub capabilities: Vec<String>,
}

fn any_environment() -> String {
    "*".into()
}

/// The catalog document, `assets/bundled/mods/catalog.json`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Catalog {
    #[serde(default = "format_version")]
    pub format_version: u32,
    pub mods: Vec<CatalogEntry>,
}

fn format_version() -> u32 {
    1
}

/// A filter for the manager's search box.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Filter {
    /// Free text over id, name, description and tags.
    pub query: String,
    pub tag: Option<String>,
    /// `client`, `server` or `*`.
    pub environment: Option<String>,
    /// Only mods declaring every one of these.
    pub capabilities: Vec<String>,
}

impl Catalog {
    pub fn parse(text: &str) -> LoaderResult<Self> {
        if text.len() > 16 << 20 {
            return Err(error_with(
                ErrorCode::ManifestTooLarge,
                "the mod catalog is larger than 16 MiB",
                text.len().to_string(),
            ));
        }
        let catalog: Self = serde_json::from_str(text).map_err(|e| {
            error_with(
                ErrorCode::ManifestInvalid,
                "the mod catalog is not valid",
                e.to_string(),
            )
        })?;
        if catalog.format_version > 1 {
            return Err(error_with(
                ErrorCode::FormatTooNew,
                "the mod catalog is newer than this build",
                catalog.format_version.to_string(),
            ));
        }
        Ok(catalog)
    }

    pub fn read(path: &Path) -> LoaderResult<Self> {
        let text = std::fs::read_to_string(path).map_err(|e| {
            error_with(
                ErrorCode::OfflineAssetMissing,
                "the bundled mod catalog is missing",
                format!("{}: {e}", path.display()),
            )
        })?;
        Self::parse(&text)
    }

    pub fn find(&self, id: &str) -> Option<&CatalogEntry> {
        self.mods.iter().find(|m| m.id == id)
    }

    pub fn search(&self, query: &str) -> Vec<&CatalogEntry> {
        self.filter(&Filter {
            query: query.to_owned(),
            ..Filter::default()
        })
    }

    /// Sorted, deterministic results.
    pub fn filter(&self, filter: &Filter) -> Vec<&CatalogEntry> {
        let query = filter.query.trim().to_lowercase();
        let mut out: Vec<&CatalogEntry> = self
            .mods
            .iter()
            .filter(|entry| {
                if !query.is_empty() {
                    let haystack = format!(
                        "{} {} {} {}",
                        entry.id,
                        entry.name,
                        entry.description,
                        entry.tags.join(" ")
                    )
                    .to_lowercase();
                    if !haystack.contains(&query) {
                        return false;
                    }
                }
                if let Some(tag) = &filter.tag
                    && !entry.tags.iter().any(|t| t == tag)
                {
                    return false;
                }
                if let Some(environment) = &filter.environment
                    && environment != "*"
                    && entry.environment != "*"
                    && entry.environment != *environment
                {
                    return false;
                }
                filter
                    .capabilities
                    .iter()
                    .all(|wanted| entry.capabilities.contains(wanted))
            })
            .collect();
        out.sort_by(|a, b| {
            a.name
                .to_lowercase()
                .cmp(&b.name.to_lowercase())
                .then(a.id.cmp(&b.id))
        });
        out
    }

    /// Every tag, sorted.
    pub fn tags(&self) -> Vec<&str> {
        let mut tags: Vec<&str> = self
            .mods
            .iter()
            .flat_map(|m| m.tags.iter().map(String::as_str))
            .collect();
        tags.sort_unstable();
        tags.dedup();
        tags
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CATALOG: &str = r#"{
        "formatVersion": 1,
        "mods": [
            {"id":"coords","name":"Coordinates","version":"1.0.0","description":"HUD coordinates","tags":["hud","client"],"environment":"client","capabilities":["gfx"]},
            {"id":"mapper","name":"Mapper","version":"2.0.0","description":"A world map","tags":["map","hud"],"environment":"*","capabilities":["gfx","fs"]},
            {"id":"server-tools","name":"Server Tools","version":"0.3.0","tags":["server"],"environment":"server"}
        ]
    }"#;

    #[test]
    fn searches_without_network() {
        let catalog = Catalog::parse(CATALOG).unwrap();
        assert_eq!(catalog.search("coord").len(), 1);
        assert_eq!(catalog.search("HUD").len(), 2);
        assert_eq!(catalog.search("map").len(), 1);
        assert!(catalog.search("nothing").is_empty());
        assert_eq!(catalog.find("mapper").unwrap().version, "2.0.0");
    }

    #[test]
    fn filters_by_tag_environment_and_capability() {
        let catalog = Catalog::parse(CATALOG).unwrap();
        let filtered = catalog.filter(&Filter {
            tag: Some("hud".into()),
            environment: Some("client".into()),
            capabilities: vec!["gfx".into()],
            ..Filter::default()
        });
        assert_eq!(
            filtered.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(),
            ["coords", "mapper"]
        );
        let server = catalog.filter(&Filter {
            environment: Some("server".into()),
            ..Filter::default()
        });
        assert_eq!(
            server.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(),
            ["mapper", "server-tools"]
        );
        assert_eq!(catalog.tags(), ["client", "hud", "map", "server"]);
    }

    #[test]
    fn hostile_catalogs_are_refused() {
        for text in [
            "",
            "{}",
            "null",
            "[]",
            r#"{"formatVersion":2,"mods":[]}"#,
            r#"{"mods":[{"id":1}]}"#,
        ] {
            let _ = Catalog::parse(text);
        }
    }

    #[test]
    fn a_missing_bundled_catalog_has_an_offline_code() {
        let err = Catalog::read(Path::new("/definitely/missing/catalog.json")).unwrap_err();
        assert_eq!(err.code, ErrorCode::OfflineAssetMissing);
    }
}
