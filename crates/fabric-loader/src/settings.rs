//! Settings a mod declares in `custom.horizon.settings`, and the UI model the
//! launcher renders for them.
//!
//! Types: `bool`, `int` (with `min`/`max`), `enum` (with `values`), `string`,
//! `keybind` and `color`.

use crate::error::{ErrorCode, LoaderResult, error_with};
use fabric_mod_format::Manifest;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

/// A setting's type and constraints.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum Kind {
    Bool,
    Int {
        #[serde(default)]
        min: Option<i64>,
        #[serde(default)]
        max: Option<i64>,
    },
    Enum {
        values: Vec<String>,
    },
    String {
        #[serde(default)]
        max_length: Option<usize>,
    },
    Keybind,
    Color,
}

/// One declared setting.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Setting {
    pub key: String,
    #[serde(default)]
    pub label: Option<String>,
    #[serde(flatten)]
    pub kind: Kind,
    #[serde(default)]
    pub default: Value,
    #[serde(default)]
    pub description: Option<String>,
}

impl Setting {
    pub fn label(&self) -> &str {
        self.label.as_deref().unwrap_or(&self.key)
    }

    /// Checks a value against the type's constraints.
    pub fn validate(&self, value: &Value) -> LoaderResult<()> {
        let fail = |reason: &str| {
            error_with(
                ErrorCode::ManifestInvalid,
                format!("setting {} is invalid", self.key),
                reason.to_owned(),
            )
        };
        match (&self.kind, value) {
            (Kind::Bool, Value::Bool(_)) => Ok(()),
            (Kind::Int { min, max }, Value::Number(number)) => {
                let Some(value) = number.as_i64() else {
                    return Err(fail("an int setting needs an integer"));
                };
                if let Some(min) = min
                    && value < *min
                {
                    return Err(fail("below the minimum"));
                }
                if let Some(max) = max
                    && value > *max
                {
                    return Err(fail("above the maximum"));
                }
                Ok(())
            }
            (Kind::Enum { values }, Value::String(text)) if values.contains(text) => Ok(()),
            (Kind::String { max_length }, Value::String(text)) => match max_length {
                Some(max) if text.chars().count() > *max => Err(fail("too long")),
                _ => Ok(()),
            },
            (Kind::Keybind, Value::String(text)) if !text.is_empty() && text.len() <= 128 => Ok(()),
            (Kind::Color, Value::String(text)) if is_color(text) => Ok(()),
            _ => Err(fail("the value does not match the declared type")),
        }
    }

    /// A value the UI can start from.
    pub fn initial(&self) -> Value {
        if self.default.is_null() {
            match &self.kind {
                Kind::Bool => Value::Bool(false),
                Kind::Int { .. } => Value::from(0),
                Kind::Enum { values } => values
                    .first()
                    .cloned()
                    .map(Value::String)
                    .unwrap_or(Value::String(String::new())),
                Kind::String { .. } | Kind::Keybind => Value::String(String::new()),
                Kind::Color => Value::String("#ffffff".into()),
            }
        } else {
            self.default.clone()
        }
    }
}

fn is_color(text: &str) -> bool {
    let hex = text.strip_prefix('#').unwrap_or(text);
    (hex.len() == 6 || hex.len() == 8) && hex.chars().all(|c| c.is_ascii_hexdigit())
}

/// All settings a mod declares, in sorted key order.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct SettingsSchema {
    pub mod_id: String,
    pub settings: Vec<Setting>,
}

impl SettingsSchema {
    /// Reads `custom.horizon.settings`, as an array or as an object keyed by
    /// setting name.
    pub fn parse(manifest: &Manifest) -> LoaderResult<Self> {
        let Some(horizon) = manifest.horizon_custom() else {
            return Ok(Self {
                mod_id: manifest.id.clone(),
                settings: Vec::new(),
            });
        };
        let Some(raw) = horizon.get("settings") else {
            return Ok(Self {
                mod_id: manifest.id.clone(),
                settings: Vec::new(),
            });
        };
        let mut settings: Vec<Setting> = match raw {
            Value::Array(items) => {
                let mut out = Vec::with_capacity(items.len());
                for item in items {
                    out.push(serde_json::from_value(item.clone()).map_err(|e| {
                        error_with(
                            ErrorCode::ManifestInvalid,
                            format!("{} has an invalid setting", manifest.id),
                            e.to_string(),
                        )
                    })?);
                }
                out
            }
            Value::Object(map) => {
                let mut out = Vec::with_capacity(map.len());
                for (key, value) in map {
                    let mut object = value.as_object().cloned().ok_or_else(|| {
                        error_with(
                            ErrorCode::ManifestInvalid,
                            format!("{} setting {key} is not an object", manifest.id),
                            "",
                        )
                    })?;
                    object.insert("key".into(), Value::String(key.clone()));
                    out.push(serde_json::from_value(Value::Object(object)).map_err(|e| {
                        error_with(
                            ErrorCode::ManifestInvalid,
                            format!("{} setting {key} is invalid", manifest.id),
                            e.to_string(),
                        )
                    })?);
                }
                out
            }
            _ => {
                return Err(error_with(
                    ErrorCode::ManifestInvalid,
                    format!("{} settings must be an array or an object", manifest.id),
                    "",
                ));
            }
        };
        settings.sort_by(|a, b| a.key.cmp(&b.key));
        let mut seen = std::collections::BTreeSet::new();
        for setting in &settings {
            if setting.key.is_empty() || !seen.insert(setting.key.clone()) {
                return Err(error_with(
                    ErrorCode::ManifestInvalid,
                    format!("{} has an empty or duplicate setting key", manifest.id),
                    setting.key.clone(),
                ));
            }
        }
        let schema = Self {
            mod_id: manifest.id.clone(),
            settings,
        };
        // Defaults must satisfy their own type.
        for setting in &schema.settings {
            let initial = setting.initial();
            setting.validate(&initial)?;
        }
        Ok(schema)
    }

    pub fn get(&self, key: &str) -> Option<&Setting> {
        self.settings.iter().find(|s| s.key == key)
    }

    /// Values merged over the defaults, with validation.
    pub fn values(
        &self,
        stored: &BTreeMap<String, Value>,
    ) -> LoaderResult<BTreeMap<String, Value>> {
        let mut out = BTreeMap::new();
        for setting in &self.settings {
            let value = stored
                .get(&setting.key)
                .cloned()
                .unwrap_or_else(|| setting.initial());
            setting.validate(&value)?;
            out.insert(setting.key.clone(), value);
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fabric_mod_format::Manifest;

    fn manifest(settings: &str) -> Manifest {
        Manifest::parse(&format!(
            r#"{{"id":"a","version":"1.0.0","custom":{{"horizon":{{"settings":{settings}}}}}}}"#
        ))
        .unwrap()
    }

    #[test]
    fn parses_every_type_and_generates_defaults() {
        let m = manifest(
            r##"[
                {"key":"enabled","type":"bool","label":"Enabled","default":true},
                {"key":"speed","type":"int","min":0,"max":10,"default":4},
                {"key":"mode","type":"enum","values":["a","b"],"default":"b"},
                {"key":"name","type":"string","max_length":8,"default":"hi"},
                {"key":"sprint","type":"keybind","default":"LeftShift"},
                {"key":"tint","type":"color","default":"#aabbcc"}
            ]"##,
        );
        let schema = SettingsSchema::parse(&m).unwrap();
        assert_eq!(schema.settings.len(), 6);
        assert_eq!(schema.get("speed").unwrap().label(), "speed");
        let values = schema.values(&BTreeMap::new()).unwrap();
        assert_eq!(values["enabled"], Value::Bool(true));
        assert_eq!(values["mode"], Value::String("b".into()));
    }

    #[test]
    fn object_form_is_accepted_and_sorted() {
        let m = manifest(r#"{"z":{"type":"bool"},"a":{"type":"int","default":2,"min":0,"max":5}}"#);
        let schema = SettingsSchema::parse(&m).unwrap();
        assert_eq!(schema.settings[0].key, "a");
        assert_eq!(schema.settings[1].key, "z");
    }

    #[test]
    fn invalid_values_are_refused() {
        let m = manifest(r#"[{"key":"speed","type":"int","min":0,"max":10,"default":4}]"#);
        let schema = SettingsSchema::parse(&m).unwrap();
        let mut bad = BTreeMap::new();
        bad.insert("speed".to_owned(), Value::from(99));
        assert_eq!(
            schema.values(&bad).unwrap_err().code,
            ErrorCode::ManifestInvalid
        );
        let mut bad = BTreeMap::new();
        bad.insert("speed".to_owned(), Value::String("fast".into()));
        assert!(schema.values(&bad).is_err());
    }

    #[test]
    fn hostile_declarations_never_panic() {
        for settings in [
            "null",
            "3",
            r#"[{"key":"","type":"bool"}]"#,
            r#"[{"key":"a","type":"bool"},{"key":"a","type":"bool"}]"#,
            r#"[{"key":"a","type":"nope"}]"#,
            r#"[{"key":"a","type":"int","min":10,"max":0,"default":20}]"#,
            r#"{"a":"not an object"}"#,
            r#"[{"key":"a","type":"color","default":"red"}]"#,
        ] {
            let _ = SettingsSchema::parse(&manifest(settings));
        }
    }
}
