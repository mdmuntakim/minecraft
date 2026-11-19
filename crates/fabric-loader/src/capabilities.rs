//! Declared capabilities and one-time consent. Undeclared use fails closed.

use crate::error::{ErrorCode, LoaderResult, error_with};
use fabric_mod_format::Manifest;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// A capability a mod can ask for.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Capability {
    Fs,
    Net,
    Gfx,
    Input,
    Clipboard,
    Accounts,
}

impl Capability {
    pub const ALL: [Capability; 6] = [
        Capability::Fs,
        Capability::Net,
        Capability::Gfx,
        Capability::Input,
        Capability::Clipboard,
        Capability::Accounts,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Fs => "fs",
            Self::Net => "net",
            Self::Gfx => "gfx",
            Self::Input => "input",
            Self::Clipboard => "clipboard",
            Self::Accounts => "accounts",
        }
    }

    pub fn parse(text: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|c| c.as_str() == text)
    }

    /// What the consent prompt says.
    pub fn description(self) -> &'static str {
        match self {
            Self::Fs => "read and write files in its own data folder",
            Self::Net => "open network connections",
            Self::Gfx => "draw on the screen",
            Self::Input => "read keyboard, mouse and controller input",
            Self::Clipboard => "read and write the clipboard",
            Self::Accounts => "see the signed-in account's name",
        }
    }
}

/// The capabilities a manifest declares, checked for unknown names.
pub fn declared(manifest: &Manifest) -> LoaderResult<BTreeSet<Capability>> {
    let mut out = BTreeSet::new();
    for name in manifest.capabilities() {
        let capability = Capability::parse(&name).ok_or_else(|| {
            error_with(
                ErrorCode::CapabilityMissing,
                format!("{} declares the unknown capability {name:?}", manifest.id),
                format!(
                    "known: {}",
                    Capability::ALL.map(Capability::as_str).join(", ")
                ),
            )
        })?;
        out.insert(capability);
    }
    Ok(out)
}

/// Grants remembered per mod, persisted by the launcher.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConsentStore {
    /// mod id -> granted capabilities.
    pub granted: BTreeMap<String, BTreeSet<Capability>>,
}

impl ConsentStore {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn is_granted(&self, mod_id: &str, capability: Capability) -> bool {
        self.granted
            .get(mod_id)
            .is_some_and(|set| set.contains(&capability))
    }

    pub fn grant(&mut self, mod_id: &str, capability: Capability) {
        self.granted
            .entry(mod_id.to_owned())
            .or_default()
            .insert(capability);
    }

    pub fn revoke(&mut self, mod_id: &str, capability: Capability) {
        if let Some(set) = self.granted.get_mut(mod_id) {
            set.remove(&capability);
        }
    }

    pub fn revoke_all(&mut self, mod_id: &str) {
        self.granted.remove(mod_id);
    }

    pub fn read(path: &std::path::Path) -> LoaderResult<Self> {
        let text = std::fs::read_to_string(path).map_err(|e| {
            error_with(
                ErrorCode::CapabilityMissing,
                "the consent store cannot be read",
                e.to_string(),
            )
        })?;
        serde_json::from_str(&text).map_err(|e| {
            error_with(
                ErrorCode::CapabilityMissing,
                "the consent store is not valid",
                e.to_string(),
            )
        })
    }

    pub fn write(&self, path: &std::path::Path) -> LoaderResult<()> {
        let text = serde_json::to_string_pretty(self).map_err(|e| {
            error_with(
                ErrorCode::CapabilityMissing,
                "the consent store cannot be written",
                e.to_string(),
            )
        })?;
        std::fs::write(path, text).map_err(|e| {
            error_with(
                ErrorCode::CapabilityMissing,
                "the consent store cannot be written",
                e.to_string(),
            )
        })
    }
}

/// Asks for every declared capability once, then checks a use.
///
/// `prompt` is called with the capability, its description and the mod's
/// display name. Returning `false` refuses the capability; a refused or
/// undeclared capability always fails closed.
pub fn ensure(
    manifest: &Manifest,
    store: &mut ConsentStore,
    mut prompt: impl FnMut(Capability, &str) -> bool,
) -> LoaderResult<()> {
    let declared = declared(manifest)?;
    for capability in declared {
        if store.is_granted(&manifest.id, capability) {
            continue;
        }
        if prompt(capability, manifest.display_name()) {
            store.grant(&manifest.id, capability);
        } else {
            return Err(error_with(
                ErrorCode::CapabilityMissing,
                format!("{} was not granted {}", manifest.id, capability.as_str()),
                "the mod is disabled until the capability is accepted",
            ));
        }
    }
    Ok(())
}

/// Checks one use. It must be declared and granted; there is no implicit
/// grant and no "allow once".
pub fn require(
    manifest: &Manifest,
    store: &ConsentStore,
    capability: Capability,
) -> LoaderResult<()> {
    let declared = declared(manifest)?;
    if !declared.contains(&capability) {
        return Err(error_with(
            ErrorCode::CapabilityMissing,
            format!(
                "{} used {} without declaring it",
                manifest.id,
                capability.as_str()
            ),
            "add it to custom.horizon.capabilities",
        ));
    }
    if !store.is_granted(&manifest.id, capability) {
        return Err(error_with(
            ErrorCode::CapabilityMissing,
            format!(
                "{} has not been granted {}",
                manifest.id,
                capability.as_str()
            ),
            "the player declined the consent prompt",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use fabric_mod_format::Manifest;

    fn manifest(capabilities: &str) -> Manifest {
        Manifest::parse(&format!(
            r#"{{"id":"a","version":"1.0.0","custom":{{"horizon":{{"capabilities":{capabilities}}}}}}}"#
        ))
        .unwrap()
    }

    #[test]
    fn declared_capabilities_are_typed() {
        let m = manifest(r#"["fs","net"]"#);
        assert_eq!(
            declared(&m).unwrap().into_iter().collect::<Vec<_>>(),
            [Capability::Fs, Capability::Net]
        );
        let err = declared(&manifest(r#"["root"]"#)).unwrap_err();
        assert_eq!(err.code, ErrorCode::CapabilityMissing);
    }

    #[test]
    fn consent_is_one_time_and_fail_closed() {
        let m = manifest(r#"["fs"]"#);
        let mut store = ConsentStore::new();
        let mut asked = 0;
        ensure(&m, &mut store, |_, _| {
            asked += 1;
            true
        })
        .unwrap();
        assert_eq!(asked, 1);
        ensure(&m, &mut store, |_, _| {
            asked += 1;
            true
        })
        .unwrap();
        assert_eq!(asked, 1, "a granted capability must not prompt again");
        require(&m, &store, Capability::Fs).unwrap();
        let err = require(&m, &store, Capability::Net).unwrap_err();
        assert_eq!(err.code, ErrorCode::CapabilityMissing);
        store.revoke("a", Capability::Fs);
        assert!(require(&m, &store, Capability::Fs).is_err());
    }

    #[test]
    fn a_decline_disables_the_mod() {
        let m = manifest(r#"["gfx"]"#);
        let mut store = ConsentStore::new();
        let err = ensure(&m, &mut store, |_, _| false).unwrap_err();
        assert_eq!(err.code, ErrorCode::CapabilityMissing);
        assert!(!store.is_granted("a", Capability::Gfx));
    }

    #[test]
    fn the_store_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("consent.json");
        let mut store = ConsentStore::new();
        store.grant("a", Capability::Accounts);
        store.write(&path).unwrap();
        let read = ConsentStore::read(&path).unwrap();
        assert!(read.is_granted("a", Capability::Accounts));
    }
}
