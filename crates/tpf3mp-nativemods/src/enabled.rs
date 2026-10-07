//! What the launcher hands the game, and what the hook makes of it.
//!
//! The launcher writes the packages the player enabled, for the build it
//! starts, to `native-mods/enabled.json` in its data folder, and names that
//! file in the game's environment ([`ENV`]), beside the game's link (D11).
//! No variable, nothing enabled: a game started any other way runs no
//! native mod. No other switch turns a feature on (Big Maps' own branches
//! read `TPF3MP_BIGMAP_*` variables; as a package it would not).
//!
//! The hook then switches a package's features on only when all of this
//! holds ([`plan`]):
//!
//! - the file names the build the hook is running in (the executable's
//!   SHA-256, the same one its profile matched);
//! - the hook has every feature the package names ([`crate::features`]),
//!   and the matched profile has every target those features patch;
//! - for a package that changes the simulation, the room compares native
//!   mods ([`ROOM_TERMS_CARRIED`]).
//!
//! When one does not, a package that changes only what its player sees is
//! left off with the reason; one that changes the simulation **refuses
//! multiplayer** for that game: the hook installs nothing, as for a build
//! it has no profile for. A file that names a package but cannot be read
//! refuses too, since what it holds cannot be told.

use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};

use crate::{
    features::{self, Effect, FeatureSpec},
    index::Setting,
};

/// The variable naming the enabled packages' file in the game's
/// environment.
pub const ENV: &str = "TPF3MP_NATIVE_MODS";
/// The file's name, in the native-mods folder.
pub const FILE: &str = "enabled.json";
/// The file's format.
pub const FORMAT: u32 = 1;
/// Largest file the hook reads.
pub const MAX_BYTES: u64 = 256 * 1024;

/// Whether rooms compare native mods yet. They do not: the protocol does
/// not carry them ([`crate::terms`] says how it would), so a package that
/// changes the simulation would let one game of a room run a different
/// world. Until it does, such a package refuses multiplayer (PLAN.md,
/// Part 3: a channel not checked yet is refused, never used unchecked).
pub const ROOM_TERMS_CARRIED: bool = false;

/// The packages the launcher enabled for one build.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Enabled {
    pub format: u32,
    /// The game build they are enabled for, by the executable's SHA-256.
    pub build: String,
    pub packages: Vec<EnabledPackage>,
}

/// One enabled package, as installed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EnabledPackage {
    pub id: String,
    pub version: String,
    /// As the index says; the hook also counts a simulation feature.
    pub simulation: bool,
    pub features: Vec<String>,
    /// The package's defaults with the player's choices over them.
    pub settings: BTreeMap<String, Setting>,
    /// Its folder, `native-mods/<id>/<version>/`.
    pub root: PathBuf,
}

impl EnabledPackage {
    /// Whether it changes the simulation: the index says so, or a feature
    /// it names does (an unknown feature counts as one, fail closed).
    pub fn changes_simulation(&self, registry: &[FeatureSpec]) -> bool {
        self.simulation
            || self.features.iter().any(|id| {
                features::find(registry, id).is_none_or(|f| f.effect == Effect::Simulation)
            })
    }
}

/// A feature the hook switches on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Active {
    pub feature: String,
    pub package: String,
    pub version: String,
    pub settings: BTreeMap<String, Setting>,
    pub root: PathBuf,
}

/// What the hook does with the enabled packages.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Plan {
    /// The features to switch on.
    pub active: Vec<Active>,
    /// Packages left off, with why: they change only what a player sees.
    pub left_off: Vec<String>,
    /// Why this game must not play in a room, if it must not.
    pub refusal: Option<String>,
}

impl Plan {
    /// The active feature `id`, if the plan switches it on.
    pub fn feature(&self, id: &str) -> Option<&Active> {
        self.active.iter().find(|active| active.feature == id)
    }
}

/// What the hook does with `enabled`, running in the build hashing to
/// `build`, with the features in `registry`, where `has_target` says
/// whether the matched profile has a target.
pub fn plan(
    enabled: &Enabled,
    build: &str,
    registry: &[FeatureSpec],
    has_target: impl Fn(&str) -> bool,
    terms_carried: bool,
) -> Plan {
    let mut plan = Plan::default();
    let mut refusals = Vec::new();
    for package in &enabled.packages {
        let name = format!("{} {}", package.id, package.version);
        let problem = if enabled.format != FORMAT {
            Some(format!("the enabled list has format {}", enabled.format))
        } else if !build.eq_ignore_ascii_case(&enabled.build) {
            Some("it was enabled for another build of the game".to_owned())
        } else {
            package
                .features
                .iter()
                .find_map(|id| match features::find(registry, id) {
                    None => Some(format!("this hook has no feature {id}")),
                    Some(feature) => feature
                        .targets
                        .iter()
                        .find(|target| !has_target(target))
                        .map(|target| {
                            format!("{id} needs {target}, which this build's profile lacks")
                        }),
                })
        };
        let simulation = package.changes_simulation(registry);
        let problem = problem.or_else(|| {
            (simulation && !terms_carried).then(|| {
                "it changes the simulation, and rooms do not compare native mods yet".to_owned()
            })
        });
        match (problem, simulation) {
            (Some(why), true) => refusals.push(format!("{name}: {why}")),
            (Some(why), false) => plan.left_off.push(format!("{name}: {why}")),
            (None, _) => plan
                .active
                .extend(package.features.iter().map(|feature| Active {
                    feature: feature.clone(),
                    package: package.id.clone(),
                    version: package.version.clone(),
                    settings: package.settings.clone(),
                    root: package.root.clone(),
                })),
        }
    }
    if !refusals.is_empty() {
        plan.active.clear();
        plan.refusal = Some(format!(
            "native mods that change the simulation cannot run: {}",
            refusals.join("; ")
        ));
    }
    plan
}

/// The enabled packages in the file `get` names under [`ENV`], read, and
/// the hook's plan for them. No variable: nothing enabled. A file that
/// cannot be read refuses multiplayer.
pub fn plan_from_env(
    get: impl Fn(&str) -> Option<String>,
    build: &str,
    registry: &[FeatureSpec],
    has_target: impl Fn(&str) -> bool,
) -> Plan {
    let Some(path) = get(ENV).filter(|path| !path.is_empty()) else {
        return Plan::default();
    };
    match read(Path::new(&path)) {
        Ok(enabled) => plan(&enabled, build, registry, has_target, ROOM_TERMS_CARRIED),
        Err(error) => Plan {
            refusal: Some(format!("the enabled native mods cannot be read: {error}")),
            ..Plan::default()
        },
    }
}

/// Reads an enabled list.
pub fn read(path: &Path) -> Result<Enabled, String> {
    let bytes = std::fs::read(path).map_err(|error| format!("{}: {error}", path.display()))?;
    if bytes.len() as u64 > MAX_BYTES {
        return Err(format!("{} is too large", path.display()));
    }
    serde_json::from_slice(&bytes).map_err(|error| format!("{}: {error}", path.display()))
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;
    use crate::{
        features::example::BIG_MAPS,
        index::tests::{BUILD, OTHER_BUILD},
    };

    fn enabled(packages: Vec<EnabledPackage>) -> Enabled {
        Enabled {
            format: FORMAT,
            build: BUILD.into(),
            packages,
        }
    }

    fn package(id: &str, simulation: bool, features: &[&str]) -> EnabledPackage {
        EnabledPackage {
            id: id.into(),
            version: "1.0.0".into(),
            simulation,
            features: features.iter().map(|f| (*f).to_owned()).collect(),
            settings: BTreeMap::from([("depth".into(), Setting::Int(10))]),
            root: PathBuf::from(format!("native-mods/{id}/1.0.0")),
        }
    }

    fn every_target(_: &str) -> bool {
        true
    }

    #[test]
    fn a_matching_build_with_every_target_switches_features_on() {
        let list = enabled(vec![package(
            "bigmap",
            true,
            &["bigmap.octree", "bigmap.page"],
        )]);
        let plan = plan(&list, BUILD, BIG_MAPS, every_target, true);
        assert_eq!(plan.refusal, None);
        assert_eq!(
            plan.feature("bigmap.octree").map(|a| &a.settings["depth"]),
            Some(&Setting::Int(10))
        );
        assert!(plan.feature("bigmap.page").is_some());
        assert!(plan.feature("bigmap.density").is_none(), "not enabled");
    }

    #[test]
    fn simulation_packages_refuse_until_rooms_compare_them() {
        let list = enabled(vec![package("bigmap", true, &["bigmap.octree"])]);
        let plan = plan(&list, BUILD, BIG_MAPS, every_target, ROOM_TERMS_CARRIED);
        assert!(plan.active.is_empty());
        assert!(plan.refusal.unwrap().contains("do not compare native mods"));
    }

    #[test]
    fn another_build_enables_nothing() {
        let mut list = enabled(vec![
            package("bigmap", true, &["bigmap.octree"]),
            package("pages", false, &["bigmap.page"]),
        ]);
        list.build = OTHER_BUILD.into();
        let plan = plan(&list, BUILD, BIG_MAPS, every_target, true);
        assert!(plan.active.is_empty());
        assert!(plan.refusal.is_some(), "the simulation package refuses");
        assert_eq!(plan.left_off.len(), 1, "the page is only left off");
    }

    #[test]
    fn a_missing_target_or_feature_refuses_or_leaves_off() {
        let list = enabled(vec![
            package("bigmap", true, &["bigmap.octree"]),
            package("pages", false, &["bigmap.page"]),
        ]);
        // The profile lacks the page's targets: the page is left off.
        let plan_a = plan(
            &list,
            BUILD,
            BIG_MAPS,
            |target| !target.starts_with("bigmap::size"),
            true,
        );
        assert_eq!(plan_a.refusal, None);
        assert!(plan_a.feature("bigmap.octree").is_some());
        assert!(plan_a.feature("bigmap.page").is_none());
        assert_eq!(plan_a.left_off.len(), 1);
        // It lacks the octree's: no game in a room.
        let plan_b = plan(&list, BUILD, BIG_MAPS, |t| t != "bigmap::octree_root", true);
        assert!(plan_b.refusal.is_some());
        assert!(plan_b.active.is_empty());
        // A hook without the features at all: an unknown feature counts as
        // changing the simulation, even in a package that says it does not.
        let quiet = enabled(vec![package("pages", false, &["bigmap.page"])]);
        let plan_c = plan(&quiet, BUILD, crate::features::BUILT_IN, every_target, true);
        assert!(plan_c.refusal.is_some());
    }

    #[test]
    fn the_environment_names_the_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(FILE);
        let list = enabled(vec![package("pages", false, &["bigmap.page"])]);
        std::fs::write(&path, serde_json::to_vec(&list).unwrap()).unwrap();
        let env = HashMap::from([(ENV, path.to_string_lossy().into_owned())]);
        let plan = plan_from_env(|key| env.get(key).cloned(), BUILD, BIG_MAPS, every_target);
        assert!(plan.feature("bigmap.page").is_some());

        // No variable: nothing at all.
        assert_eq!(
            plan_from_env(|_| None, BUILD, BIG_MAPS, every_target),
            Plan::default()
        );
        // A file that cannot be read refuses.
        std::fs::write(&path, b"{ not json").unwrap();
        let plan = plan_from_env(|key| env.get(key).cloned(), BUILD, BIG_MAPS, every_target);
        assert!(plan.refusal.is_some());
        let gone = HashMap::from([(
            ENV,
            dir.path().join("gone.json").to_string_lossy().into_owned(),
        )]);
        assert!(
            plan_from_env(|key| gone.get(key).cloned(), BUILD, BIG_MAPS, every_target)
                .refusal
                .is_some()
        );
    }
}
