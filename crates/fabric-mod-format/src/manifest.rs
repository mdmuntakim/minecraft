//! `fabric.mod.json`, schema versions 1 and 2.
//!
//! Unknown top-level keys are preserved in [`Manifest::extra`] so that a mod
//! written for a newer loader survives a round trip through this one. A
//! `schemaVersion` above [`MAX_SCHEMA_VERSION`] is refused with
//! [`ErrorCode::FormatTooNew`] rather than ignored.

use crate::{
    error::{ErrorCode, FabricError, Result},
    version::VersionRange,
};
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

/// The newest manifest schema this build understands.
pub const MAX_SCHEMA_VERSION: u32 = 2;
/// The largest manifest a container may carry.
pub const MAX_MANIFEST: usize = 1 << 20;
/// Namespaces no mod may claim.
pub const RESERVED_NAMESPACES: [&str; 2] = ["minecraft", "fabric"];

fn default_schema() -> u32 {
    1
}

fn default_format() -> u32 {
    1
}

fn one_or_many<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> std::result::Result<Vec<String>, D::Error> {
    let value = Value::deserialize(deserializer)?;
    match value {
        Value::String(s) => Ok(vec![s]),
        Value::Array(items) => items
            .into_iter()
            .map(|item| match item {
                Value::String(s) => Ok(s),
                _ => Err(serde::de::Error::custom("expected a string")),
            })
            .collect(),
        _ => Err(serde::de::Error::custom(
            "expected a string or an array of strings",
        )),
    }
}

/// Fabric allows `"main": "value"`, `"main": { ... }` and arrays of either.
fn entrypoints<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> std::result::Result<Entrypoints, D::Error> {
    let value = BTreeMap::<String, Value>::deserialize(deserializer)?;
    let mut out = Entrypoints::new();
    for (name, value) in value {
        let items = match value {
            Value::String(s) => vec![Entrypoint::Plain(s)],
            Value::Object(_) => {
                vec![serde_json::from_value(value).map_err(serde::de::Error::custom)?]
            }
            Value::Array(items) => {
                let mut out = Vec::with_capacity(items.len());
                for item in items {
                    out.push(serde_json::from_value(item).map_err(serde::de::Error::custom)?);
                }
                out
            }
            _ => {
                return Err(serde::de::Error::custom(
                    "an entrypoint is a string, an object or an array of both",
                ));
            }
        };
        out.insert(name, items);
    }
    Ok(out)
}

fn license<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> std::result::Result<Option<String>, D::Error> {
    let value = Value::deserialize(deserializer)?;
    match value {
        Value::Null => Ok(None),
        Value::String(s) => Ok(Some(s)),
        Value::Array(items) => {
            let mut names = Vec::new();
            for item in items {
                match item {
                    Value::String(s) => names.push(s),
                    _ => return Err(serde::de::Error::custom("expected a license name")),
                }
            }
            Ok(Some(names.join(", ")))
        }
        _ => Err(serde::de::Error::custom(
            "expected a string or an array of strings",
        )),
    }
}

fn icon<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> std::result::Result<Option<String>, D::Error> {
    let value = Value::deserialize(deserializer)?;
    match value {
        Value::Null => Ok(None),
        Value::String(s) => Ok(Some(s)),
        Value::Object(map) => {
            // Fabric allows {"1x": "icon.png", "2x": "icon@2x.png"}; the base
            // size is the one the container stores.
            for key in ["1x", "2x", "minecraft:1x"] {
                if let Some(Value::String(s)) = map.get(key) {
                    return Ok(Some(s.clone()));
                }
            }
            map.values()
                .find_map(Value::as_str)
                .map(|s| Ok(Some(s.to_owned())))
                .unwrap_or(Ok(None))
        }
        _ => Err(serde::de::Error::custom(
            "icon must be a path or a size map",
        )),
    }
}

/// Which side of a game a mod loads on.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Environment {
    /// Both sides.
    #[default]
    #[serde(rename = "*")]
    Any,
    Client,
    Server,
}

impl Environment {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Any => "*",
            Self::Client => "client",
            Self::Server => "server",
        }
    }

    pub fn runs_on(self, side: Environment) -> bool {
        self == Self::Any || side == Self::Any || self == side
    }
}

/// One entrypoint, with an optional adapter (`"kotlin"`, `"rust"`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Entrypoint {
    Plain(String),
    Detailed {
        adapter: Option<String>,
        value: String,
    },
}

impl Entrypoint {
    pub fn value(&self) -> &str {
        match self {
            Self::Plain(value) => value,
            Self::Detailed { value, .. } => value,
        }
    }

    pub fn adapter(&self) -> Option<&str> {
        match self {
            Self::Plain(_) => None,
            Self::Detailed { adapter, .. } => adapter.as_deref(),
        }
    }
}

/// A named group of entrypoints. The standard names are `preLaunch`, `main`,
/// `client`, `server`, `modmenu` and `junit`; any other key is preserved and
/// dispatched like a custom phase.
pub type Entrypoints = BTreeMap<String, Vec<Entrypoint>>;

/// A mixin configuration reference: either a bare path or an object.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum MixinRef {
    Plain(String),
    Detailed {
        config: String,
        #[serde(default)]
        environment: Option<Environment>,
    },
}

impl MixinRef {
    pub fn config(&self) -> &str {
        match self {
            Self::Plain(config) => config,
            Self::Detailed { config, .. } => config,
        }
    }
}

/// A dependency requirement as written in the manifest, with the parsed
/// ranges kept beside it so a conflict can quote the original text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Requirement {
    pub raw: Vec<String>,
    pub ranges: Vec<VersionRange>,
}

impl Serialize for Requirement {
    fn serialize<S: serde::Serializer>(
        &self,
        serializer: S,
    ) -> std::result::Result<S::Ok, S::Error> {
        match self.raw.as_slice() {
            [one] => serializer.serialize_str(one),
            many => many.serialize(serializer),
        }
    }
}

impl Requirement {
    pub fn parse(raw: Vec<String>) -> Result<Self> {
        let mut ranges = Vec::new();
        for text in &raw {
            ranges.push(VersionRange::parse(text).map_err(|e| {
                FabricError::with_detail(
                    ErrorCode::VersionInvalid,
                    format!("version range {text:?} cannot be parsed"),
                    e.log_line(),
                )
            })?);
        }
        Ok(Self { raw, ranges })
    }

    /// Whether `version` satisfies any of the ranges (an empty requirement
    /// accepts any version).
    pub fn matches(&self, version: &crate::version::Version) -> bool {
        self.ranges.is_empty() || self.ranges.iter().any(|r| r.contains(version))
    }

    /// The requirement text as the mod wrote it, for diagnostics.
    pub fn text(&self) -> String {
        if self.raw.is_empty() {
            "*".into()
        } else {
            self.raw.join(" || ")
        }
    }
}

impl<'de> Deserialize<'de> for Requirement {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        let value = Value::deserialize(deserializer)?;
        let raw = match value {
            Value::String(s) => vec![s],
            Value::Array(items) => {
                let mut out = Vec::new();
                for item in items {
                    match item {
                        Value::String(s) => out.push(s),
                        _ => {
                            return Err(serde::de::Error::custom(
                                "a version requirement is a string or an array of strings",
                            ));
                        }
                    }
                }
                out
            }
            _ => {
                return Err(serde::de::Error::custom(
                    "a version requirement is a string or an array of strings",
                ));
            }
        };
        Requirement::parse(raw).map_err(serde::de::Error::custom)
    }
}

/// A dependency group (`depends`, `breaks`, `recommends`, `suggests`).
pub type DependencyMap = BTreeMap<String, Requirement>;

/// The manifest of a `.fabricmod`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Manifest {
    #[serde(default = "default_schema", rename = "schemaVersion")]
    pub schema_version: u32,
    /// The container format (`1`). A newer value is refused instead of
    /// silently misread.
    #[serde(default = "default_format", rename = "formatVersion")]
    pub format_version: u32,
    pub id: String,
    pub version: String,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub environment: Environment,
    #[serde(default, deserialize_with = "one_or_many")]
    pub authors: Vec<String>,
    #[serde(default)]
    pub contact: BTreeMap<String, String>,
    #[serde(default, deserialize_with = "license")]
    pub license: Option<String>,
    #[serde(default, deserialize_with = "icon")]
    pub icon: Option<String>,
    #[serde(default, deserialize_with = "entrypoints")]
    pub entrypoints: Entrypoints,
    #[serde(default)]
    pub depends: DependencyMap,
    #[serde(default)]
    pub breaks: DependencyMap,
    #[serde(default)]
    pub recommends: DependencyMap,
    #[serde(default)]
    pub suggests: DependencyMap,
    #[serde(default, rename = "nestedJars")]
    pub nested_jars: Vec<String>,
    #[serde(default)]
    pub jars: Vec<String>,
    #[serde(default)]
    pub mixins: Vec<MixinRef>,
    #[serde(default, rename = "accessWidener")]
    pub access_widener: Option<String>,
    /// Alternative ids this mod satisfies.
    #[serde(default)]
    pub provides: Vec<String>,
    /// Mod-owned, namespaced data, preserved verbatim.
    #[serde(default)]
    pub custom: Value,
    /// Top-level keys this build does not know; kept for a round trip.
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl Manifest {
    /// Parses and validates a manifest. Never panics, whatever the bytes say.
    pub fn parse(text: &str) -> Result<Self> {
        if text.len() > MAX_MANIFEST {
            return Err(FabricError::with_detail(
                ErrorCode::ManifestTooLarge,
                "fabric.mod.json is over 1 MiB",
                format!("{} bytes", text.len()),
            ));
        }
        // A UTF-8 byte-order mark is legal in the wild and not part of JSON.
        let text = text.strip_prefix('\u{feff}').unwrap_or(text);
        let manifest: Self = serde_json::from_str(text).map_err(|e| {
            FabricError::with_detail(
                ErrorCode::ManifestInvalid,
                "fabric.mod.json is not valid",
                e.to_string(),
            )
        })?;
        manifest.validate()?;
        Ok(manifest)
    }

    /// Checks every invariant a loader relies on.
    pub fn validate(&self) -> Result<()> {
        if self.format_version > crate::container::FORMAT_VERSION {
            return Err(FabricError::with_detail(
                ErrorCode::FormatTooNew,
                format!(
                    "fabric.mod.json formatVersion {} is newer than this build's {}",
                    self.format_version,
                    crate::container::FORMAT_VERSION
                ),
                self.id.clone(),
            ));
        }
        if self.schema_version > MAX_SCHEMA_VERSION {
            return Err(FabricError::with_detail(
                ErrorCode::FormatTooNew,
                format!(
                    "fabric.mod.json schemaVersion {} is newer than this build's {}",
                    self.schema_version, MAX_SCHEMA_VERSION
                ),
                self.id.clone(),
            ));
        }
        if self.schema_version == 0 {
            return Err(FabricError::with_detail(
                ErrorCode::FormatInvalid,
                "schemaVersion must be 1 or 2",
                self.id.clone(),
            ));
        }
        validate_id(&self.id)?;
        crate::version::Version::parse(&self.version).map_err(|e| {
            FabricError::with_detail(
                ErrorCode::VersionInvalid,
                format!("{} declares an invalid version", self.id),
                e.log_line(),
            )
        })?;
        for alias in &self.provides {
            validate_id(alias)?;
        }
        for (name, entrypoint) in self
            .entrypoints
            .iter()
            .flat_map(|(k, v)| v.iter().map(move |e| (k, e)))
        {
            if entrypoint.value().trim().is_empty() {
                return Err(FabricError::with_detail(
                    ErrorCode::ManifestInvalid,
                    format!("entrypoint {name:?} is empty"),
                    self.id.clone(),
                ));
            }
        }
        if let Some(icon) = &self.icon {
            if icon.is_empty() || icon.starts_with('/') || icon.contains("..") {
                return Err(FabricError::with_detail(
                    ErrorCode::IconInvalid,
                    "the icon path must be relative and stay inside the container",
                    icon.clone(),
                ));
            }
        }
        if let Some(widener) = &self.access_widener {
            if !matches!(
                widener.as_str(),
                "accessWidener" | "accessWidener-v1" | "accessWidener-v2"
            ) && widener.is_empty()
            {
                return Err(FabricError::with_detail(
                    ErrorCode::FormatInvalid,
                    "accessWidener is empty",
                    self.id.clone(),
                ));
            }
        }
        Ok(())
    }

    /// The display name (`name` or the id).
    pub fn display_name(&self) -> &str {
        self.name.as_deref().unwrap_or(&self.id)
    }

    /// Every id this mod answers to: its own and `provides`.
    pub fn ids(&self) -> impl Iterator<Item = &str> {
        std::iter::once(self.id.as_str()).chain(self.provides.iter().map(String::as_str))
    }

    /// The parsed version.
    pub fn semver(&self) -> Result<crate::version::Version> {
        crate::version::Version::parse(&self.version)
    }

    /// `custom.horizon` as an object, if the mod carries one.
    pub fn horizon_custom(&self) -> Option<&serde_json::Map<String, Value>> {
        self.custom.get("horizon")?.as_object()
    }

    /// The capabilities the mod declares, as stable strings.
    pub fn capabilities(&self) -> Vec<String> {
        let Some(horizon) = self.horizon_custom() else {
            return Vec::new();
        };
        horizon
            .get("capabilities")
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::to_owned)
                    .collect()
            })
            .unwrap_or_default()
    }

    /// The native entrypoint library path, when the mod is a code mod.
    pub fn native_library(&self) -> Option<String> {
        let horizon = self.horizon_custom()?;
        horizon
            .get("native")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .or_else(|| {
                horizon
                    .get("library")
                    .and_then(Value::as_str)
                    .map(str::to_owned)
            })
    }

    /// Serialises the manifest with stable key order (for hashing and pack).
    pub fn to_pretty_json(&self) -> Result<String> {
        // serde_json's map order for BTreeMap is sorted; `Value` objects keep
        // insertion order only with preserve_order, which this build does not
        // enable, so the output is deterministic.
        serde_json::to_string_pretty(self).map_err(|e| {
            FabricError::with_detail(
                ErrorCode::ManifestInvalid,
                "manifest cannot be serialised",
                e.to_string(),
            )
        })
    }
}

fn validate_id(id: &str) -> Result<()> {
    let valid = !id.is_empty()
        && id.len() <= 64
        && id
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-')
        && !id.starts_with('-')
        && !id.ends_with('-');
    if !valid {
        return Err(FabricError::with_detail(
            ErrorCode::InvalidId,
            "a mod id is 1-64 characters of a-z, 0-9, _ and -, not starting or ending with -",
            id.to_owned(),
        ));
    }
    if RESERVED_NAMESPACES.contains(&id) {
        return Err(FabricError::with_detail(
            ErrorCode::InvalidId,
            "that id is reserved",
            id.to_owned(),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const V2: &str = r#"{
        "schemaVersion": 2,
        "id": "example-mod",
        "version": "1.4.2-beta.1",
        "name": "Example",
        "description": "A mod",
        "environment": "client",
        "authors": ["A", "B"],
        "contact": {"homepage": "https://example.invalid"},
        "license": "MIT",
        "icon": "icon.png",
        "entrypoints": {
            "preLaunch": ["com.example.Pre", {"adapter": "rust", "value": "example_pre"}],
            "main": "example_main",
            "custom-thing": [{"value": "custom"}]
        },
        "depends": {"fabric-loader": ">=0.15", "other": [">=1.0 <2.0", "[3.0,4.0)"]},
        "breaks": {"bad": "<1.0"},
        "recommends": {"nice": "*"},
        "suggests": {"maybe": "1.x"},
        "nestedJars": ["META-INF/jars/inner.jar"],
        "jars": [],
        "mixins": ["example.mixins.json", {"config": "client.mixins.json", "environment": "client"}],
        "accessWidener": "example.accesswidener",
        "provides": ["example-alias"],
        "custom": {"horizon": {"capabilities": ["fs", "net"], "native": "lib/arm64-v8a/libexample.so"}, "other-mod": {"x": 1}},
        "futureField": {"kept": true}
    }"#;

    #[test]
    fn parses_every_field_and_keeps_unknown_ones() {
        let m = Manifest::parse(V2).unwrap();
        assert_eq!(m.schema_version, 2);
        assert_eq!(m.environment, Environment::Client);
        assert_eq!(m.entrypoints["preLaunch"].len(), 2);
        assert_eq!(m.entrypoints["preLaunch"][1].adapter(), Some("rust"));
        assert_eq!(m.depends["other"].ranges.len(), 2);
        assert_eq!(m.provides, ["example-alias"]);
        assert_eq!(m.capabilities(), ["fs", "net"]);
        assert_eq!(
            m.native_library().as_deref(),
            Some("lib/arm64-v8a/libexample.so")
        );
        assert_eq!(m.extra["futureField"]["kept"], true);
        assert_eq!(m.custom["other-mod"]["x"], 1);
        assert_eq!(m.mixins[1].config(), "client.mixins.json");
    }

    #[test]
    fn round_trips_unknown_top_level_keys() {
        let m = Manifest::parse(V2).unwrap();
        let text = m.to_pretty_json().unwrap();
        let again = Manifest::parse(&text).unwrap();
        assert_eq!(m, again);
    }

    #[test]
    fn schema_one_defaults_and_any_environment() {
        let m = Manifest::parse(r#"{"id":"a","version":"1.0.0"}"#).unwrap();
        assert_eq!(m.schema_version, 1);
        assert_eq!(m.environment, Environment::Any);
        assert!(m.name.is_none());
    }

    #[test]
    fn newer_schema_is_refused() {
        let err = Manifest::parse(r#"{"schemaVersion":3,"id":"a","version":"1.0.0"}"#).unwrap_err();
        assert_eq!(err.code, ErrorCode::FormatTooNew);
    }

    #[test]
    fn malformed_input_never_panics() {
        for text in [
            "",
            "null",
            "[]",
            "{}",
            r#"{"id":"","version":"1.0.0"}"#,
            r#"{"id":"A","version":"1.0.0"}"#,
            r#"{"id":"minecraft","version":"1.0.0"}"#,
            r#"{"id":"a","version":"nope"}"#,
            r#"{"id":"a","version":"1.0.0","depends":{"x":">=wat"}}"#,
            r#"{"id":"a","version":"1.0.0","environment":"both"}"#,
            r#"{"id":"a","version":"1.0.0","entrypoints":{"main":[""]}}"#,
            r#"{"id":"a","version":"1.0.0","icon":"../x.png"}"#,
            &("{\"id\":\"a\",\"version\":\"1.0.0\",\"x\":\"".to_owned()
                + &"y".repeat(MAX_MANIFEST)
                + "\"}"),
        ] {
            let _ = Manifest::parse(text);
        }
    }

    #[test]
    fn requirements_accept_strings_and_arrays() {
        let m = Manifest::parse(
            r#"{"id":"a","version":"1.0.0","depends":{"x":">=1.0","y":["<2.0",">=1.0"]}}"#,
        )
        .unwrap();
        let v = crate::version::Version::parse("1.5.0").unwrap();
        assert!(m.depends["x"].matches(&v));
        assert!(m.depends["y"].matches(&v));
        assert_eq!(m.depends["y"].text(), "<2.0 || >=1.0");
    }
}
