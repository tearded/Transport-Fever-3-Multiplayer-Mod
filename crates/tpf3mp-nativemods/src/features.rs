//! The features built into TPF3-MP's hook, by id: what a native package can
//! switch on.
//!
//! In this first version a native package carries no code of its own. It
//! names features already compiled into the hook, which the hook switches on
//! with the package's settings only when the launcher enabled the package
//! and the running build's profile has every target the feature patches
//! ([`crate::enabled`]). The launcher reads the same registry to refuse a
//! package this TPF3-MP cannot run before downloading it.
//!
//! [`BUILT_IN`] is empty on `dev`: no native feature is built into the hook
//! yet. Big Maps, on its own branches, would add its features here; the
//! shape it would take is [`example::BIG_MAPS`], which no hook runs.

/// What a feature changes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Effect {
    /// Only what one player sees or may do on their own machine: a game
    /// may run without it.
    Ui,
    /// How the world runs: every game of a room runs it alike, or none.
    Simulation,
}

/// A feature compiled into the hook.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FeatureSpec {
    /// Ids joined by dots: the package's id, then the feature's.
    pub id: &'static str,
    pub effect: Effect,
    /// The hook-profile targets it patches, all of them (docs/HOOKS.md).
    pub targets: &'static [&'static str],
}

/// The features this hook has. None yet.
pub const BUILT_IN: &[FeatureSpec] = &[];

/// The feature `id` in `registry`.
pub fn find<'a>(registry: &'a [FeatureSpec], id: &str) -> Option<&'a FeatureSpec> {
    registry.iter().find(|feature| feature.id == id)
}

/// Feature registries no hook carries, for the docs and the tests.
pub mod example {
    use super::{Effect, FeatureSpec};

    /// Big Maps (docs/BIGMAPS.md, `crates/tpf3mp-bigmap`) as a native
    /// package's features. Not built into the hook: Big Maps lives on its
    /// own branches.
    pub const BIG_MAPS: &[FeatureSpec] = &[
        FeatureSpec {
            // The octree's root box past ±32,768 m.
            id: "bigmap.octree",
            effect: Effect::Simulation,
            targets: &["bigmap::octree_root"],
        },
        FeatureSpec {
            // The street raster's 32-bit cell count (180 tiles).
            id: "bigmap.street_raster",
            effect: Effect::Simulation,
            targets: &["bigmap::street_raster"],
        },
        FeatureSpec {
            // Town and industry placement attempts.
            id: "bigmap.placement",
            effect: Effect::Simulation,
            targets: &["bigmap::placement_attempts"],
        },
        FeatureSpec {
            // The New Game page with the added sizes, served by the hook.
            id: "bigmap.page",
            effect: Effect::Ui,
            targets: &["bigmap::tile_count", "bigmap::size_list"],
        },
        FeatureSpec {
            // Density levels for towns and industries.
            id: "bigmap.density",
            effect: Effect::Simulation,
            targets: &["bigmap::density_levels"],
        },
        FeatureSpec {
            // Refuses a size this PC lacks the memory to generate; changes
            // nothing in a world.
            id: "bigmap.memory_gate",
            effect: Effect::Ui,
            targets: &[],
        },
    ];
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::index::is_feature_id;

    #[test]
    fn feature_ids_are_well_formed_and_unique() {
        for registry in [BUILT_IN, example::BIG_MAPS] {
            let mut ids: Vec<&str> = registry.iter().map(|f| f.id).collect();
            assert!(ids.iter().all(|id| is_feature_id(id)));
            ids.sort_unstable();
            ids.dedup();
            assert_eq!(ids.len(), registry.len());
        }
        assert_eq!(
            find(example::BIG_MAPS, "bigmap.octree").map(|f| f.effect),
            Some(Effect::Simulation)
        );
        assert!(find(BUILT_IN, "bigmap.octree").is_none());
    }
}
