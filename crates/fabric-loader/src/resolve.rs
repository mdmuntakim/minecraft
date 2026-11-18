//! Deterministic dependency resolution.
//!
//! The solver is a topological sort over a `BTreeMap`/`BTreeSet`, so the same
//! mod set yields the same order on every run and on every machine. Every
//! failure names the mod that asked, the mod that provides and both ranges.

use crate::{
    discovery::{Discovered, Discovery},
    error::{ErrorCode, LoaderResult, error_with},
};
use fabric_mod_format::{Manifest, Version, VersionRange};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    path::PathBuf,
};

/// A resolved mod.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Resolved {
    pub id: String,
    pub version: String,
    pub provides: Vec<String>,
    pub sha256: String,
    pub path: PathBuf,
    /// The ids this mod depends on, sorted and deduplicated.
    pub dependencies: Vec<String>,
    pub environment: String,
}

/// The result of resolution: a deterministic order and per-mod facts.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Resolution {
    /// Dependencies before dependents, ties broken by id.
    pub order: Vec<String>,
    pub mods: BTreeMap<String, Resolved>,
}

impl Resolution {
    /// The discovered mods in load order.
    pub fn ordered<'a>(&self, discovery: &'a Discovery) -> Vec<&'a Discovered> {
        self.order
            .iter()
            .filter_map(|id| discovery.by_id(id))
            .collect()
    }

    pub fn is_empty(&self) -> bool {
        self.order.is_empty()
    }
}

/// Resolves a discovered set.
pub fn resolve(discovery: &Discovery) -> LoaderResult<Resolution> {
    if !discovery.failures.is_empty() {
        // A broken mod is not silently dropped from a resolved set.
        return Err(discovery.failures[0].error.clone());
    }
    resolve_mods(&discovery.mods)
}

/// Resolves a list of mods.
pub fn resolve_mods(mods: &[Discovered]) -> LoaderResult<Resolution> {
    // 1. Real ids and aliases.
    let mut by_id: BTreeMap<String, usize> = BTreeMap::new();
    let mut aliases: BTreeMap<String, usize> = BTreeMap::new();
    for (index, discovered) in mods.iter().enumerate() {
        let id = discovered.id();
        if let Some(previous) = by_id.insert(id.to_owned(), index) {
            return Err(error_with(
                ErrorCode::DuplicateMod,
                format!("two mods have the id {id}"),
                format!(
                    "{} and {}",
                    mods[previous].path.display(),
                    discovered.path.display()
                ),
            ));
        }
    }
    for (index, discovered) in mods.iter().enumerate() {
        for alias in &discovered.container.manifest.provides {
            if by_id.contains_key(alias) {
                return Err(error_with(
                    ErrorCode::DuplicateMod,
                    format!("{alias} is both a mod id and an alias"),
                    discovered.path.display().to_string(),
                ));
            }
            if let Some(previous) = aliases.insert(alias.clone(), index) {
                return Err(error_with(
                    ErrorCode::DuplicateMod,
                    format!("two mods provide {alias}"),
                    format!("{} and {}", mods[previous].id(), discovered.id()),
                ));
            }
        }
    }

    let find =
        |id: &str| -> Option<usize> { by_id.get(id).copied().or_else(|| aliases.get(id).copied()) };

    // 2. Dependencies exist and their version satisfies every range.
    let mut edges: BTreeMap<&str, BTreeSet<&str>> = BTreeMap::new();
    for discovered in mods {
        edges.entry(discovered.id()).or_default();
    }
    for (self_index, discovered) in mods.iter().enumerate() {
        let manifest = &discovered.container.manifest;
        for (dependency, requirement) in &manifest.depends {
            // `minecraft` and `fabric-loader` are the runtime, not mods; the
            // compatibility checker compares them with the build's versions.
            if is_builtin(dependency) {
                continue;
            }
            let Some(index) = find(dependency) else {
                return Err(error_with(
                    ErrorCode::DependencyMissing,
                    format!(
                        "{} needs {dependency}, which is not installed",
                        discovered.id()
                    ),
                    format!("required range: {}", requirement.text()),
                ));
            };
            let provider = &mods[index];
            let version = provider.container.manifest.semver()?;
            if !satisfies(requirement, &version) {
                return Err(error_with(
                    ErrorCode::VersionConflict,
                    format!(
                        "{} needs {dependency} {}, but {} {} is installed",
                        discovered.id(),
                        requirement.text(),
                        provider.id(),
                        provider.version()
                    ),
                    format!(
                        "{}: {}; {}: {}",
                        discovered.id(),
                        requirement.text(),
                        provider.id(),
                        provider.version()
                    ),
                ));
            }
            if index != self_index {
                edges
                    .entry(discovered.id())
                    .or_default()
                    .insert(provider.id());
            }
        }
        // Pairwise requirement conflicts, even when the installed version
        // happens to satisfy one side: they are what a player must resolve.
        for other in mods {
            if other.id() == discovered.id() {
                continue;
            }
            for (dependency, ours) in &manifest.depends {
                let Some(theirs) = other.container.manifest.depends.get(dependency) else {
                    continue;
                };
                if !requirements_intersect(ours, theirs) {
                    return Err(error_with(
                        ErrorCode::VersionConflict,
                        format!(
                            "{} and {} need incompatible versions of {dependency}",
                            discovered.id(),
                            other.id()
                        ),
                        format!(
                            "{}: {}; {}: {}",
                            discovered.id(),
                            ours.text(),
                            other.id(),
                            theirs.text()
                        ),
                    ));
                }
            }
        }
        // 3. Breaks.
        for (broken, requirement) in &manifest.breaks {
            if let Some(index) = find(broken) {
                let provider = &mods[index];
                let version = provider.container.manifest.semver()?;
                if satisfies(requirement, &version) {
                    return Err(error_with(
                        ErrorCode::DependencyBroken,
                        format!(
                            "{} cannot be used with {broken} {}",
                            discovered.id(),
                            provider.version()
                        ),
                        format!("{} breaks {broken} {}", discovered.id(), requirement.text()),
                    ));
                }
            }
        }
    }

    // 4. Cycle detection and a deterministic topological order.
    let mut indegree: BTreeMap<&str, usize> = BTreeMap::new();
    for discovered in mods {
        indegree.insert(discovered.id(), 0);
    }
    for (dependent, dependencies) in &edges {
        indegree.insert(dependent, dependencies.len());
    }
    let queue: BTreeSet<&str> = indegree
        .iter()
        .filter(|(_, degree)| **degree == 0)
        .map(|(id, _)| *id)
        .collect();
    let mut queue: VecDeque<&str> = {
        let mut sorted: Vec<&str> = queue.into_iter().collect();
        sorted.sort();
        sorted.into()
    };
    let mut order = Vec::new();
    let mut reverse: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for (dependent, dependencies) in &edges {
        for dependency in dependencies {
            reverse.entry(dependency).or_default().push(dependent);
        }
    }
    while let Some(id) = queue.pop_front() {
        order.push(id.to_owned());
        if let Some(dependents) = reverse.get(id) {
            let mut next: Vec<&str> = dependents.clone();
            next.sort();
            for dependent in next {
                if let Some(degree) = indegree.get_mut(dependent) {
                    *degree = degree.saturating_sub(1);
                    if *degree == 0 {
                        queue.push_back(dependent);
                    }
                }
            }
        }
    }
    if order.len() != mods.len() {
        let remaining: Vec<&str> = indegree
            .iter()
            .filter(|(_, degree)| **degree > 0)
            .map(|(id, _)| *id)
            .collect();
        return Err(error_with(
            ErrorCode::DependencyCycle,
            "the mods depend on each other in a circle",
            remaining.join(" -> "),
        ));
    }

    let mut resolved = BTreeMap::new();
    for discovered in mods {
        let manifest = &discovered.container.manifest;
        let dependencies = edges
            .get(discovered.id())
            .map(|set| set.iter().map(|s| (*s).to_owned()).collect())
            .unwrap_or_default();
        resolved.insert(
            discovered.id().to_owned(),
            Resolved {
                id: discovered.id().to_owned(),
                version: discovered.version().to_owned(),
                provides: manifest.provides.clone(),
                sha256: discovered.digest_hex(),
                path: discovered.path.clone(),
                dependencies,
                environment: manifest.environment.as_str().to_owned(),
            },
        );
    }
    Ok(Resolution {
        order,
        mods: resolved,
    })
}

/// Dependencies the runtime itself provides.
pub fn is_builtin(id: &str) -> bool {
    matches!(id, "minecraft" | "fabric-loader" | "java" | "fabric")
}

fn satisfies(requirement: &fabric_mod_format::Requirement, version: &Version) -> bool {
    // Several ranges on one dependency are all required (AND).
    requirement
        .ranges
        .iter()
        .all(|range| range.contains(version))
}

fn requirements_intersect(
    a: &fabric_mod_format::Requirement,
    b: &fabric_mod_format::Requirement,
) -> bool {
    if a.ranges.is_empty() || b.ranges.is_empty() {
        return true;
    }
    a.ranges.iter().all(|left| {
        b.ranges
            .iter()
            .all(|right| left.intersects(right) || same_range(left, right))
    })
}

fn same_range(a: &VersionRange, b: &VersionRange) -> bool {
    a.as_str() == b.as_str()
}

/// A conflict diagnostic with the witness version, for the UI.
pub fn explain_conflict(
    first_mod: &str,
    first_range: &VersionRange,
    second_mod: &str,
    second_range: &VersionRange,
) -> String {
    match first_range.intersection_witness(second_range) {
        Some(witness) => format!(
            "{first_mod} needs {} and {second_mod} needs {}; both would accept {witness}",
            first_range.as_str(),
            second_range.as_str()
        ),
        None => format!(
            "{first_mod} needs {} and {second_mod} needs {}; no version satisfies both",
            first_range.as_str(),
            second_range.as_str()
        ),
    }
}

/// A manifest field used by the compatibility checker.
pub fn game_version(manifest: &Manifest) -> Option<&fabric_mod_format::Requirement> {
    manifest.depends.get("minecraft")
}

#[cfg(test)]
mod tests {
    use super::*;
    use fabric_mod_format::container::{Container, EXTENSION};
    use std::fs;

    fn mod_with(manifest: &str) -> Discovered {
        let container = Container::new(manifest).unwrap();
        Discovered {
            container,
            path: PathBuf::from("mem.fabricmod"),
            folder: false,
        }
    }

    fn write(dir: &std::path::Path, manifest: &str) -> Discovered {
        let container = Container::new(manifest).unwrap();
        let path = dir.join(format!("{}.{}", container.manifest.id, EXTENSION));
        fs::write(&path, container.to_bytes().unwrap()).unwrap();
        Discovered {
            path,
            container: Container::read_bytes(&container.to_bytes().unwrap()).unwrap(),
            folder: false,
        }
    }

    #[test]
    fn sorts_dependencies_before_dependents_deterministically() {
        let a = mod_with(r#"{"id":"a","version":"1.0.0","depends":{"c":">=1.0"}}"#);
        let b = mod_with(r#"{"id":"b","version":"1.0.0","depends":{"c":">=1.0"}}"#);
        let c = mod_with(r#"{"id":"c","version":"2.0.0"}"#);
        let first = resolve_mods(&[a.clone(), b.clone(), c.clone()]).unwrap();
        assert_eq!(first.order, ["c", "a", "b"]);
        let second = resolve_mods(&[c, b, a]).unwrap();
        assert_eq!(first.order, second.order);
    }

    #[test]
    fn missing_dependency_is_named() {
        let a = mod_with(r#"{"id":"a","version":"1.0.0","depends":{"nope":">=1.0"}}"#);
        let err = resolve_mods(&[a]).unwrap_err();
        assert_eq!(err.code, ErrorCode::DependencyMissing);
        assert!(err.message.contains("nope"));
    }

    #[test]
    fn conflict_names_both_mods_and_both_ranges() {
        let a = mod_with(r#"{"id":"a","version":"1.0.0","depends":{"lib":"^1.0"}}"#);
        let b = mod_with(r#"{"id":"b","version":"1.0.0","depends":{"lib":"^2.0"}}"#);
        let lib = mod_with(r#"{"id":"lib","version":"1.5.0"}"#);
        let err = resolve_mods(&[a, b, lib]).unwrap_err();
        assert_eq!(err.code, ErrorCode::VersionConflict);
        let all = format!("{err}");
        assert!(all.contains("a") && all.contains("b"));
        assert!(all.contains("^1.0") && all.contains("^2.0"), "{all}");
    }

    #[test]
    fn aliases_satisfy_dependencies() {
        let a = mod_with(r#"{"id":"a","version":"1.0.0","depends":{"lib":">=1.0"}}"#);
        let lib = mod_with(r#"{"id":"real-lib","version":"1.2.0","provides":["lib"]}"#);
        let resolution = resolve_mods(&[a, lib]).unwrap();
        assert_eq!(resolution.order, ["real-lib", "a"]);
    }

    #[test]
    fn cycles_are_refused() {
        let a = mod_with(r#"{"id":"a","version":"1.0.0","depends":{"b":"*"}}"#);
        let b = mod_with(r#"{"id":"b","version":"1.0.0","depends":{"a":"*"}}"#);
        let err = resolve_mods(&[a, b]).unwrap_err();
        assert_eq!(err.code, ErrorCode::DependencyCycle);
        assert!(err.detail.unwrap().contains("a"));
    }

    #[test]
    fn duplicates_and_breaks_are_refused() {
        let a = mod_with(r#"{"id":"a","version":"1.0.0"}"#);
        let a2 = mod_with(r#"{"id":"a","version":"2.0.0"}"#);
        assert_eq!(
            resolve_mods(&[a, a2]).unwrap_err().code,
            ErrorCode::DuplicateMod
        );
        let a = mod_with(r#"{"id":"a","version":"1.0.0","breaks":{"b":"<2.0"}}"#);
        let b = mod_with(r#"{"id":"b","version":"1.0.0"}"#);
        assert_eq!(
            resolve_mods(&[a, b]).unwrap_err().code,
            ErrorCode::DependencyBroken
        );
    }

    #[test]
    fn multiple_ranges_are_all_required() {
        let a = mod_with(r#"{"id":"a","version":"1.0.0","depends":{"lib":[">=1.0","<2.0"]}}"#);
        let lib = mod_with(r#"{"id":"lib","version":"2.5.0"}"#);
        let err = resolve_mods(&[a, lib]).unwrap_err();
        assert_eq!(err.code, ErrorCode::VersionConflict);
    }

    #[test]
    fn a_broken_mod_fails_the_scan_strictly() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("bad.fabricmod"), b"nope").unwrap();
        let discovery = crate::discovery::discover(dir.path());
        assert_eq!(
            resolve(&discovery).unwrap_err().code,
            ErrorCode::FormatInvalid
        );
    }

    #[test]
    fn explain_conflict_shows_a_witness() {
        let a = VersionRange::parse(">=1.0 <2.0").unwrap();
        let b = VersionRange::parse("[1.5,3.0)").unwrap();
        let text = explain_conflict("a", &a, "b", &b);
        assert!(text.contains("1.5.0"), "{text}");
    }

    #[test]
    fn write_helper_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let discovered = write(dir.path(), r#"{"id":"a","version":"1.0.0"}"#);
        assert_eq!(discovered.id(), "a");
        assert!(discovered.path.ends_with("a.fabricmod"));
    }
}
