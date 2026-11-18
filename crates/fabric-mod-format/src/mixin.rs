//! Fabric mixin configuration parsing.
//!
//! This build has a Rust mixin pipeline; Java mixin classes are parsed so the
//! user gets a precise reason, never a crash.

use crate::error::{ErrorCode, FabricError, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// A mixin configuration, `mixins/<id>.json`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MixinConfig {
    #[serde(default)]
    pub required: bool,
    #[serde(default)]
    pub package: Option<String>,
    #[serde(default)]
    pub compatibility_level: Option<String>,
    #[serde(default)]
    pub min_version: Option<String>,
    #[serde(default)]
    pub refmap: Option<String>,
    #[serde(default)]
    pub plugin: Option<String>,
    #[serde(default)]
    pub mixins: Vec<String>,
    #[serde(default)]
    pub client: Vec<String>,
    #[serde(default)]
    pub server: Vec<String>,
    /// Keys this build does not know, preserved verbatim.
    #[serde(flatten)]
    pub extra: std::collections::BTreeMap<String, Value>,
}

impl MixinConfig {
    /// Parses a config from bytes.
    pub fn parse(bytes: &[u8]) -> Result<Self> {
        if bytes.len() > 1 << 20 {
            return Err(FabricError::new(
                ErrorCode::ManifestTooLarge,
                "the mixin config is larger than 1 MiB",
            ));
        }
        let text = std::str::from_utf8(bytes).map_err(|_| {
            FabricError::new(ErrorCode::ManifestInvalid, "the mixin config is not UTF-8")
        })?;
        let config: Self = serde_json::from_str(text).map_err(|e| {
            FabricError::with_detail(
                ErrorCode::ManifestInvalid,
                "the mixin config is not valid JSON",
                e.to_string(),
            )
        })?;
        config.validate()?;
        Ok(config)
    }

    pub fn validate(&self) -> Result<()> {
        for class in self.classes() {
            if class.is_empty()
                || class.contains("..")
                || class.contains('/')
                || class.ends_with(".class")
                || !class
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '$' || c == '.')
            {
                return Err(FabricError::with_detail(
                    ErrorCode::MixinJavaUnsupported,
                    "the mixin config names a class this build cannot run",
                    class.to_owned(),
                ));
            }
        }
        Ok(())
    }

    /// Every mixin class named, from all three lists.
    pub fn classes(&self) -> impl Iterator<Item = &str> {
        self.mixins
            .iter()
            .chain(&self.client)
            .chain(&self.server)
            .map(String::as_str)
    }

    pub fn is_empty(&self) -> bool {
        self.mixins.is_empty() && self.client.is_empty() && self.server.is_empty()
    }

    /// Routes the config: Java classes are collected so the loader can reject
    /// them with [`ErrorCode::MixinJavaUnsupported`], and any Rust adapter
    /// declarations under `custom.horizon.rustMixins` pass through.
    pub fn route(&self) -> RoutedMixins {
        RoutedMixins {
            java_classes: self.classes().map(str::to_owned).collect(),
            rust: self
                .extra
                .get("custom")
                .and_then(|c| c.get("horizon"))
                .and_then(|h| h.get("rustMixins"))
                .and_then(Value::as_array)
                .map(|items| items.iter().filter_map(Value::as_str).map(str::to_owned).collect())
                .unwrap_or_default(),
        }
    }
}

/// What a mixin config routes to.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RoutedMixins {
    /// Java mixin classes, which this build refuses.
    pub java_classes: Vec<String>,
    /// Rust mixin names, which the existing pipeline runs.
    pub rust: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    const CONFIG: &str = r#"{
        "required": true,
        "package": "com.example.mixin",
        "compatibilityLevel": "JAVA_21",
        "refmap": "example.refmap.json",
        "plugin": "com.example.Plugin",
        "mixins": ["CommonMixin"],
        "client": ["ClientMixin"],
        "server": [],
        "injectors": {"defaultRequire": 1},
        "custom": {"horizon": {"rustMixins": ["tick_hook"]}}
    }"#;

    #[test]
    fn parses_and_routes() {
        let config = MixinConfig::parse(CONFIG.as_bytes()).unwrap();
        assert!(config.required);
        assert_eq!(config.classes().count(), 2);
        let routed = config.route();
        assert_eq!(routed.java_classes, ["CommonMixin", "ClientMixin"]);
        assert_eq!(routed.rust, ["tick_hook"]);
        assert!(config.extra.contains_key("injectors"));
    }

    #[test]
    fn java_classes_are_reported_not_crashed() {
        for text in [
            r#"{"mixins":["a..b"]}"#,
            r#"{"mixins":["com/example/Mixin"]}"#,
            r#"{"mixins":["Thing.class"]}"#,
            r#"{"mixins":[""]}"#,
            r#"{"mixins":"not-an-array"}"#,
            "not json",
            "",
        ] {
            let _ = MixinConfig::parse(text.as_bytes());
        }
        let err = MixinConfig::parse(br#"{"mixins":["bad/name"]}"#).unwrap_err();
        assert_eq!(err.code, ErrorCode::MixinJavaUnsupported);
    }
}
