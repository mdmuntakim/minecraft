//! The in-game mod manager's model: list, enable/disable, reorder, conflict
//! graph, dependency tree and per-mod logs.
//!
//! Rendering lives in the launcher; this module owns the state the UI shows.

use crate::{
    error::{ErrorCode, LoaderResult, error_with},
    resolve::Resolution,
};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// A row in the mod list.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ManagedMod {
    pub id: String,
    pub version: String,
    pub environment: String,
    pub enabled: bool,
    pub signed: bool,
    pub capabilities: Vec<String>,
    pub dependencies: Vec<String>,
}

/// The mod manager's state.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModManager {
    mods: BTreeMap<String, ManagedMod>,
    /// Display order; enable/disable does not change it.
    order: Vec<String>,
    #[serde(default)]
    logs: BTreeMap<String, Vec<String>>,
}

impl ModManager {
    /// Builds the state from a resolution and the set the player enabled.
    pub fn from_resolution(
        resolution: &Resolution,
        signed: &BTreeSet<String>,
        enabled: &BTreeSet<String>,
        capabilities: &BTreeMap<String, Vec<String>>,
    ) -> Self {
        let mut mods = BTreeMap::new();
        for (id, resolved) in &resolution.mods {
            mods.insert(
                id.clone(),
                ManagedMod {
                    id: id.clone(),
                    version: resolved.version.clone(),
                    environment: resolved.environment.clone(),
                    // A mod not in the enabled set is off; an empty enabled
                    // set means "everything on", the fresh-install state.
                    enabled: enabled.is_empty() || enabled.contains(id),
                    signed: signed.contains(id),
                    capabilities: capabilities.get(id).cloned().unwrap_or_default(),
                    dependencies: resolved.dependencies.clone(),
                },
            );
        }
        Self {
            mods,
            order: resolution.order.clone(),
            logs: BTreeMap::new(),
        }
    }

    pub fn len(&self) -> usize {
        self.mods.len()
    }

    pub fn is_empty(&self) -> bool {
        self.mods.is_empty()
    }

    /// Rows in display order.
    pub fn list(&self) -> Vec<&ManagedMod> {
        self.order
            .iter()
            .filter_map(|id| self.mods.get(id))
            .collect()
    }

    pub fn get(&self, id: &str) -> Option<&ManagedMod> {
        self.mods.get(id)
    }

    pub fn enable(&mut self, id: &str) -> LoaderResult<()> {
        self.mods.get_mut(id).ok_or_else(|| missing(id))?.enabled = true;
        Ok(())
    }

    pub fn disable(&mut self, id: &str) -> LoaderResult<()> {
        self.mods.get_mut(id).ok_or_else(|| missing(id))?.enabled = false;
        // Disabling a dependency disables its dependents, so the next start
        // does not fail on a missing dependency.
        loop {
            let disabled: BTreeSet<String> = self
                .mods
                .values()
                .filter(|m| !m.enabled)
                .map(|m| m.id.clone())
                .collect();
            let mut changed = false;
            for row in self.mods.values_mut() {
                if row.enabled && row.dependencies.iter().any(|dep| disabled.contains(dep)) {
                    row.enabled = false;
                    changed = true;
                }
            }
            if !changed {
                break;
            }
        }
        Ok(())
    }

    /// Moves `id` before `before` (or to the end when `None`).
    pub fn reorder(&mut self, id: &str, before: Option<&str>) -> LoaderResult<()> {
        if !self.mods.contains_key(id) {
            return Err(missing(id));
        }
        let position = self.order.iter().position(|x| x == id);
        if let Some(position) = position {
            self.order.remove(position);
        }
        match before {
            Some(before) => {
                let at = self
                    .order
                    .iter()
                    .position(|x| x == before)
                    .ok_or_else(|| missing(before))?;
                self.order.insert(at, id.to_owned());
            }
            None => self.order.push(id.to_owned()),
        }
        Ok(())
    }

    /// Ids of the mods that are on.
    pub fn enabled_ids(&self) -> BTreeSet<String> {
        self.mods
            .values()
            .filter(|m| m.enabled)
            .map(|m| m.id.clone())
            .collect()
    }

    /// Dependency edges: dependency -> mods that need it.
    pub fn conflict_graph(&self) -> BTreeMap<String, Vec<String>> {
        let mut graph: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for row in self.mods.values() {
            for dependency in &row.dependencies {
                graph
                    .entry(dependency.clone())
                    .or_default()
                    .push(row.id.clone());
            }
        }
        for dependents in graph.values_mut() {
            dependents.sort();
        }
        graph
    }

    /// A text tree of what `id` needs, one level per line.
    pub fn dependency_tree(&self, id: &str) -> LoaderResult<String> {
        if !self.mods.contains_key(id) {
            return Err(missing(id));
        }
        let mut out = String::new();
        let mut seen = BTreeSet::new();
        self.tree_into(id, "", true, &mut seen, &mut out);
        Ok(out)
    }

    fn tree_into(
        &self,
        id: &str,
        prefix: &str,
        last: bool,
        seen: &mut BTreeSet<String>,
        out: &mut String,
    ) {
        let marker = if prefix.is_empty() {
            String::new()
        } else if last {
            format!("{prefix}└── ")
        } else {
            format!("{prefix}├── ")
        };
        let row = self.mods.get(id);
        let version = row.map(|r| r.version.as_str()).unwrap_or("?");
        out.push_str(&format!("{marker}{id} {version}\n"));
        if !seen.insert(id.to_owned()) {
            out.push_str(&format!("{prefix}    (cycle)\n"));
            return;
        }
        let dependencies = row.map(|r| r.dependencies.clone()).unwrap_or_default();
        let next = if prefix.is_empty() {
            String::new()
        } else if last {
            format!("{prefix}    ")
        } else {
            format!("{prefix}│   ")
        };
        for (index, dependency) in dependencies.iter().enumerate() {
            self.tree_into(
                dependency,
                &next,
                index + 1 == dependencies.len(),
                seen,
                out,
            );
        }
    }

    /// Appends a line to a mod's log.
    pub fn log(&mut self, id: &str, line: &str) {
        let lines = self.logs.entry(id.to_owned()).or_default();
        if lines.len() >= 512 {
            lines.remove(0);
        }
        lines.push(line.to_owned());
    }

    pub fn logs(&self, id: &str) -> &[String] {
        self.logs.get(id).map(Vec::as_slice).unwrap_or_default()
    }
}

fn missing(id: &str) -> crate::error::LoaderError {
    error_with(
        ErrorCode::DependencyMissing,
        format!("{id} is not in the mod list"),
        "",
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use fabric_mod_format::container::Container;

    fn resolution() -> Resolution {
        let mods = [
            r#"{"id":"app","version":"1.0.0","depends":{"lib":">=1.0"}}"#,
            r#"{"id":"lib","version":"2.0.0"}"#,
            r#"{"id":"other","version":"0.5.0"}"#,
        ]
        .map(|text| crate::discovery::Discovered {
            container: Container::new(text).unwrap(),
            path: std::path::PathBuf::from("mem.fabricmod"),
            folder: false,
        });
        crate::resolve::resolve_mods(&mods).unwrap()
    }

    #[test]
    fn lists_enables_disables_and_reorders() {
        let mut manager = ModManager::from_resolution(
            &resolution(),
            &BTreeSet::new(),
            &BTreeSet::new(),
            &BTreeMap::new(),
        );
        assert_eq!(
            manager
                .list()
                .iter()
                .map(|m| m.id.as_str())
                .collect::<Vec<_>>(),
            ["lib", "other", "app"]
        );
        assert!(manager.list().iter().all(|m| m.enabled));
        manager.disable("lib").unwrap();
        assert!(!manager.get("lib").unwrap().enabled);
        assert!(
            !manager.get("app").unwrap().enabled,
            "a dependent is disabled too"
        );
        assert!(manager.get("other").unwrap().enabled);
        manager.enable("app").unwrap();
        manager.reorder("other", Some("lib")).unwrap();
        assert_eq!(manager.list()[0].id, "other");
        manager.reorder("other", None).unwrap();
        assert_eq!(manager.list().last().unwrap().id, "other");
        assert_eq!(
            manager.enable("nope").unwrap_err().code,
            ErrorCode::DependencyMissing
        );
    }

    #[test]
    fn shows_a_dependency_tree_and_graph() {
        let manager = ModManager::from_resolution(
            &resolution(),
            &BTreeSet::new(),
            &BTreeSet::new(),
            &BTreeMap::new(),
        );
        let tree = manager.dependency_tree("app").unwrap();
        assert!(tree.contains("app 1.0.0"));
        assert!(tree.contains("lib 2.0.0"));
        let graph = manager.conflict_graph();
        assert_eq!(graph["lib"], ["app"]);
    }

    #[test]
    fn keeps_a_bounded_per_mod_log() {
        let mut manager = ModManager::from_resolution(
            &resolution(),
            &BTreeSet::new(),
            &BTreeSet::new(),
            &BTreeMap::new(),
        );
        for index in 0..600 {
            manager.log("app", &format!("line {index}"));
        }
        let logs = manager.logs("app");
        assert_eq!(logs.len(), 512);
        assert!(logs.last().unwrap().ends_with("599"));
    }
}
