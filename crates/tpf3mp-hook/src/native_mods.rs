//! Native mods in the game (proposed D29, docs/NATIVE_MODS.md): the hook
//! switches on only the built-in features the launcher enabled, for the
//! build it enabled them for, where the matched profiles have every target
//! they patch.
//!
//! The launcher names the enabled packages' file in the game's environment
//! (`TPF3MP_NATIVE_MODS`, [`tpf3mp_nativemods::enabled::ENV`]); without it
//! nothing is enabled, and no other switch (such as the `TPF3MP_BIGMAP_*`
//! variables of Big Maps' own branches) turns a feature on. The plan is made once,
//! at bootstrap, before any hook is installed; a feature's code asks
//! [`feature`] for its settings. A simulation-changing package that cannot
//! run refuses multiplayer: [`decide`] returns why, and the hook then
//! installs nothing, as for a build it has no profile for.
//!
//! No feature is built in yet ([`tpf3mp_nativemods::features::BUILT_IN`]).

use std::sync::OnceLock;

use tpf3mp_hookcore::profile::Profile;
use tpf3mp_nativemods::{
    enabled::{self, Active, Plan},
    features::BUILT_IN,
};

use crate::Logger;

static PLAN: OnceLock<Plan> = OnceLock::new();

/// Makes the plan for the game running `build` (the executable's SHA-256)
/// with `profiles`, the profiles matching it, logs it, and returns why the
/// game must not play in a room, if it must not.
pub(crate) fn decide(profiles: &[Profile], build: &str, log: &mut Logger) -> Option<String> {
    let plan = enabled::plan_from_env(
        |key| std::env::var(key).ok(),
        build,
        BUILT_IN,
        |target| has_target(profiles, target),
    );
    for active in &plan.active {
        log.line(&format!(
            "native mod {} {}: feature {} on",
            active.package, active.version, active.feature
        ));
    }
    for why in &plan.left_off {
        log.line(&format!("native mod left off: {why}"));
    }
    let refusal = plan.refusal.clone();
    let _ = PLAN.set(plan);
    refusal
}

/// Whether one of `profiles` has the target `name`.
fn has_target(profiles: &[Profile], name: &str) -> bool {
    profiles
        .iter()
        .any(|profile| profile.targets.iter().any(|target| target.name == name))
}

/// The feature `id`, with its package's settings and folder, if the
/// launcher enabled it and it runs here.
pub fn feature(id: &str) -> Option<&'static Active> {
    PLAN.get()?.feature(id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_target_counts_from_any_matching_profile() {
        let profile = |target: &str| {
            Profile::from_toml(&format!(
                r#"
name = "p"
[build]
sha256 = "00"
[[target]]
name = "{target}"
signature = "40 53"
prologue = "40 53"
"#
            ))
            .unwrap()
        };
        let profiles = [profile("step"), profile("bigmap::octree_root")];
        assert!(has_target(&profiles, "bigmap::octree_root"));
        assert!(has_target(&profiles, "step"));
        assert!(!has_target(&profiles, "bigmap::street_raster"));
        assert!(!has_target(&[], "step"));
        // Nothing enabled until the launcher's plan is made.
        assert!(feature("bigmap.octree").is_none());
    }
}
