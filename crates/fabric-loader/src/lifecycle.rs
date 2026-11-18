//! The lifecycle: discover, validate, resolve, sort, `preLaunch`, `main`,
//! `client`/`server`, custom keys, `ready`, `dispose`.
//!
//! The plan is a list, built from sorted maps and the resolver's stable order,
//! so two runs of the same set produce byte-identical traces.

use crate::{
    discovery::Discovery,
    error::{ErrorCode, LoaderResult},
    resolve::Resolution,
};
use fabric_mod_format::Environment;
use serde::{Deserialize, Serialize};

/// A lifecycle phase.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Phase {
    Discover,
    Validate,
    Resolve,
    Sort,
    PreLaunch,
    Main,
    Client,
    Server,
    /// A manifest key this build does not know; dispatched in sorted key order.
    Custom,
    Ready,
    Dispose,
}

impl Phase {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Discover => "discover",
            Self::Validate => "validate",
            Self::Resolve => "resolve",
            Self::Sort => "sort",
            Self::PreLaunch => "preLaunch",
            Self::Main => "main",
            Self::Client => "client",
            Self::Server => "server",
            Self::Custom => "custom",
            Self::Ready => "ready",
            Self::Dispose => "dispose",
        }
    }

    /// The system phases happen once, not per mod.
    pub fn is_system(self) -> bool {
        matches!(
            self,
            Self::Discover | Self::Validate | Self::Resolve | Self::Sort
        )
    }
}

/// Which side the game is running.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
    Client,
    Server,
}

impl Side {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Client => "client",
            Self::Server => "server",
        }
    }
}

/// One entry in the plan.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Step {
    /// 1-based position, for the trace.
    pub order: usize,
    pub phase: Phase,
    /// The manifest key (`main`, `preLaunch`, a custom key).
    pub key: String,
    /// The entrypoint value.
    pub entrypoint: String,
    /// `None` for a system step.
    pub mod_id: Option<String>,
    pub reason: String,
}

impl Step {
    pub fn line(&self) -> String {
        match &self.mod_id {
            Some(id) => format!(
                "{:>3}. [{}] {id} -> {} ({})",
                self.order,
                self.phase.as_str(),
                self.entrypoint,
                self.reason
            ),
            None => format!(
                "{:>3}. [{}] {}",
                self.order,
                self.phase.as_str(),
                self.reason
            ),
        }
    }
}

/// The ordered plan.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Plan {
    pub side: String,
    pub steps: Vec<Step>,
}

impl Plan {
    /// A dump for `--trace`, one line per step.
    pub fn trace(&self) -> String {
        self.steps
            .iter()
            .map(Step::line)
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// Only the steps a host is called for.
    pub fn calls(&self) -> impl Iterator<Item = &Step> {
        self.steps.iter().filter(|step| !step.phase.is_system())
    }
}

/// Builds the plan for `side`.
pub fn plan(resolution: &Resolution, discovery: &Discovery, side: Side) -> LoaderResult<Plan> {
    let mut steps = Vec::new();
    let mut push =
        |phase: Phase, key: &str, entrypoint: &str, mod_id: Option<String>, reason: String| {
            steps.push(Step {
                order: steps.len() + 1,
                phase,
                key: key.to_owned(),
                entrypoint: entrypoint.to_owned(),
                mod_id,
                reason,
            });
        };

    push(
        Phase::Discover,
        "",
        "",
        None,
        format!("{} mod(s) from the bundled folder", discovery.mods.len()),
    );
    for discovered in resolution.ordered(discovery) {
        push(
            Phase::Validate,
            "",
            "",
            Some(discovered.id().to_owned()),
            format!("manifest {} is valid", discovered.version()),
        );
    }
    push(
        Phase::Resolve,
        "",
        "",
        None,
        "dependency graph resolved".into(),
    );
    push(
        Phase::Sort,
        "",
        "",
        None,
        format!("{} mod(s) in deterministic order", resolution.order.len()),
    );

    // Phase-major order: every preLaunch, then every main, then the side's
    // entrypoints, then every custom key (sorted), then ready, then dispose
    // in reverse. Each phase keeps the resolver's deterministic mod order.
    let ordered = resolution.ordered(discovery);
    let side_env = match side {
        Side::Client => Environment::Client,
        Side::Server => Environment::Server,
    };
    let runs = |discovered: &crate::discovery::Discovered| {
        discovered.container.manifest.environment.runs_on(side_env)
    };
    for (phase, key) in [(Phase::PreLaunch, "preLaunch"), (Phase::Main, "main")] {
        for discovered in &ordered {
            if !runs(discovered) {
                continue;
            }
            if let Some(entries) = discovered.container.manifest.entrypoints.get(key) {
                for entry in entries {
                    push(
                        phase,
                        key,
                        entry.value(),
                        Some(discovered.id().to_owned()),
                        format!("declared {key}"),
                    );
                }
            }
        }
    }
    let (phase, side_key) = match side {
        Side::Client => (Phase::Client, "client"),
        Side::Server => (Phase::Server, "server"),
    };
    for discovered in &ordered {
        if !runs(discovered) {
            continue;
        }
        if let Some(entries) = discovered.container.manifest.entrypoints.get(side_key) {
            for entry in entries {
                push(
                    phase,
                    side_key,
                    entry.value(),
                    Some(discovered.id().to_owned()),
                    format!("declared {side_key}"),
                );
            }
        }
    }
    let mut custom: std::collections::BTreeSet<&str> = std::collections::BTreeSet::new();
    for discovered in &ordered {
        for key in discovered.container.manifest.entrypoints.keys() {
            if !matches!(key.as_str(), "preLaunch" | "main" | "client" | "server") {
                custom.insert(key);
            }
        }
    }
    for key in custom {
        for discovered in &ordered {
            if !runs(discovered) {
                continue;
            }
            if let Some(entries) = discovered.container.manifest.entrypoints.get(key) {
                for entry in entries {
                    push(
                        Phase::Custom,
                        key,
                        entry.value(),
                        Some(discovered.id().to_owned()),
                        format!("custom entrypoint key {key}"),
                    );
                }
            }
        }
    }
    for discovered in &ordered {
        if runs(discovered) {
            push(
                Phase::Ready,
                "",
                "",
                Some(discovered.id().to_owned()),
                "loaded".into(),
            );
        }
    }
    for discovered in ordered.into_iter().rev() {
        push(
            Phase::Dispose,
            "",
            "",
            Some(discovered.id().to_owned()),
            "released in reverse order".into(),
        );
    }
    Ok(Plan {
        side: side.as_str().to_owned(),
        steps,
    })
}

/// The runtime the plan calls into.
pub trait Host {
    /// Called once for system phases.
    fn system(&mut self, phase: Phase) -> LoaderResult<()> {
        let _ = phase;
        Ok(())
    }

    /// Called for each per-mod step.
    fn call(&mut self, step: &Step) -> LoaderResult<()>;
}

/// Runs a plan in order, stopping at the first failure.
pub fn run(plan: &Plan, host: &mut impl Host) -> LoaderResult<()> {
    for step in &plan.steps {
        if step.phase.is_system() {
            host.system(step.phase)?;
        } else {
            host.call(step)?;
        }
    }
    Ok(())
}

/// Checks that the plan names an executable order and no step twice.
pub fn verify(plan: &Plan) -> LoaderResult<()> {
    let mut last_phase: Option<Phase> = None;
    for (index, step) in plan.steps.iter().enumerate() {
        if step.order != index + 1 {
            return Err(crate::error::error(
                ErrorCode::FormatInvalid,
                "the lifecycle plan has a gap",
            ));
        }
        if let Some(last) = last_phase
            && step.phase < last
        {
            return Err(crate::error::error(
                ErrorCode::FormatInvalid,
                "the lifecycle plan is out of phase order",
            ));
        }
        last_phase = Some(step.phase);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::discovery::{Discovered, discover};
    use fabric_mod_format::container::Container;
    use std::path::PathBuf;

    fn discovered(manifest: &str) -> Discovered {
        Discovered {
            container: Container::new(manifest).unwrap(),
            path: PathBuf::from("mem.fabricmod"),
            folder: false,
        }
    }

    #[test]
    fn the_plan_is_deterministic_and_ordered() {
        let mods = vec![
            discovered(
                r#"{"id":"z","version":"1.0.0","entrypoints":{"main":"zmain","preLaunch":"zpre"}}"#,
            ),
            discovered(r#"{"id":"a","version":"1.0.0","entrypoints":{"main":"amain"}}"#),
            discovered(
                r#"{"id":"m","version":"1.0.0","environment":"client","entrypoints":{"client":"mclient","custom-key":"mcustom"}}"#,
            ),
        ];
        let discovery = Discovery {
            mods: mods.clone(),
            ..Discovery::default()
        };
        let resolution = crate::resolve::resolve_mods(&mods).unwrap();
        let first_plan = plan(&resolution, &discovery, Side::Client).unwrap();
        let plan_ref = &first_plan;
        verify(plan_ref).unwrap();
        let trace = plan_ref.trace();
        assert!(trace.contains("[discover]"));
        assert!(trace.contains("[preLaunch] z -> zpre"));
        assert!(trace.contains("[custom] m -> mcustom"));
        assert!(trace.contains("[ready] a"));
        let again = plan(&resolution, &discovery, Side::Client).unwrap();
        assert_eq!(*plan_ref, again);
        // Main comes before custom, which comes before ready and dispose.
        let position = |needle: &str| {
            plan_ref
                .steps
                .iter()
                .position(|s| format!("{} {}", s.phase.as_str(), s.entrypoint) == needle)
                .unwrap_or(usize::MAX)
        };
        assert!(position("main amain") < position("custom mcustom"));
        assert!(position("custom mcustom") < position("ready "));
    }

    #[test]
    fn server_side_skips_client_mods_and_entrypoints() {
        let mods = vec![
            discovered(
                r#"{"id":"c","version":"1.0.0","environment":"client","entrypoints":{"main":"c"}}"#,
            ),
            discovered(
                r#"{"id":"s","version":"1.0.0","environment":"server","entrypoints":{"server":"s"}}"#,
            ),
            discovered(
                r#"{"id":"b","version":"1.0.0","entrypoints":{"client":"bc","server":"bs","main":"bm"}}"#,
            ),
        ];
        let discovery = Discovery {
            mods: mods.clone(),
            ..Discovery::default()
        };
        let resolution = crate::resolve::resolve_mods(&mods).unwrap();
        let server = plan(&resolution, &discovery, Side::Server).unwrap();
        assert!(!server.trace().contains("[main] c"));
        assert!(server.trace().contains("[server] s -> s"));
        assert!(!server.trace().contains("bc"));
        assert!(server.trace().contains("bs"));
        let client = plan(&resolution, &discovery, Side::Client).unwrap();
        assert!(client.trace().contains("bc"));
        assert!(!client.trace().contains("bs"));
    }

    #[test]
    fn a_host_is_called_in_order_and_a_failure_stops_the_run() {
        struct Recorder(Vec<String>);
        impl Host for Recorder {
            fn call(&mut self, step: &Step) -> LoaderResult<()> {
                self.0
                    .push(format!("{}:{}", step.phase.as_str(), step.entrypoint));
                if step.entrypoint == "boom" {
                    return Err(crate::error::error(ErrorCode::ManifestInvalid, "boom"));
                }
                Ok(())
            }
        }
        let mods = vec![discovered(
            r#"{"id":"a","version":"1.0.0","entrypoints":{"main":"first","client":"boom","ready":"never"}}"#,
        )];
        let discovery = Discovery {
            mods: mods.clone(),
            ..Discovery::default()
        };
        let resolution = crate::resolve::resolve_mods(&mods).unwrap();
        let plan = plan(&resolution, &discovery, Side::Client).unwrap();
        let mut host = Recorder(Vec::new());
        assert!(run(&plan, &mut host).is_err());
        assert_eq!(host.0, ["main:first", "client:boom"]);
    }

    #[test]
    fn discovery_helper_is_not_needed_for_memory_mods() {
        let dir = tempfile::tempdir().unwrap();
        assert!(discover(dir.path()).mods.is_empty());
    }
}
