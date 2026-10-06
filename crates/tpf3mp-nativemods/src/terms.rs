//! Native mods as a room's terms.
//!
//! A native package that changes the simulation must run in every game of a
//! room in the same version with the same settings, or the worlds part. Its
//! term is its id, its version and a digest of its settings; the room
//! compares the terms of every member as it compares their content
//! (`ContentManifest`, docs/PROTOCOL.md), and a member missing a package,
//! or holding another version of it, installs the room's version from the
//! signed index in one click ([`missing_from_index`]), never from another
//! player. A package that changes only what its player sees is not a term:
//! players may differ in it.
//!
//! The protocol does not carry these terms yet (D29, proposed): until it
//! does, the hook refuses multiplayer when a simulation package is enabled
//! ([`crate::enabled::ROOM_TERMS_CARRIED`]). Each term already has the
//! shape of a mod in a manifest ([`NativeTerm::mod_id`],
//! [`NativeTerm::mod_version`]) so that it can travel either as entries of
//! the manifest or as a field of its own.

use std::collections::BTreeMap;

use ring::digest::{Context, SHA256};

use crate::{
    enabled::Enabled,
    features::FeatureSpec,
    index::{Index, Package, Setting},
    signed,
};

/// Keeps settings digests apart from every other SHA-256 in the project.
const SETTINGS_DOMAIN: &[u8] = b"tpf3mp native settings 1\0";
/// What a native term's mod id starts with. No mod folder can be named so:
/// `:` is not allowed in a Windows file name.
pub const MOD_PREFIX: &str = "native:";
/// Hex digits of the settings digest in a term's mod version.
const SHORT: usize = 10;

/// One simulation-changing package as the room compares it.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct NativeTerm {
    pub id: String,
    pub version: String,
    /// The SHA-256 of its settings, lowercase hex.
    pub settings: String,
}

impl NativeTerm {
    /// Its id as a mod in a manifest: `native:<id>`.
    pub fn mod_id(&self) -> String {
        format!("{MOD_PREFIX}{}", self.id)
    }

    /// Its version as a mod in a manifest: the package's version, `+`, and
    /// the start of the settings digest. At most 31 characters.
    pub fn mod_version(&self) -> String {
        let short = &self.settings[..SHORT.min(self.settings.len())];
        format!("{}+{short}", self.version)
    }
}

/// The digest of `settings`, the same for the same settings on every
/// machine: a `BTreeMap` serializes its keys in order.
pub fn settings_digest(settings: &BTreeMap<String, Setting>) -> String {
    let mut digest = Context::new(&SHA256);
    digest.update(SETTINGS_DOMAIN);
    digest.update(&serde_json::to_vec(settings).unwrap_or_default());
    signed::hex(digest.finish().as_ref())
}

/// The room's terms for the packages in `enabled`: the simulation-changing
/// ones, ordered by id.
pub fn terms(enabled: &Enabled, registry: &[FeatureSpec]) -> Vec<NativeTerm> {
    let mut terms: Vec<NativeTerm> = enabled
        .packages
        .iter()
        .filter(|package| package.changes_simulation(registry))
        .map(|package| NativeTerm {
            id: package.id.clone(),
            version: package.version.clone(),
            settings: settings_digest(&package.settings),
        })
        .collect();
    terms.sort();
    terms
}

/// How a member's terms differ from the room's.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TermsDiff {
    /// The room's packages the member lacks.
    pub missing: Vec<NativeTerm>,
    /// Packages both run, with another version or other settings: the
    /// room's term, then the member's.
    pub changed: Vec<(NativeTerm, NativeTerm)>,
    /// The member's packages the room does not run.
    pub extra: Vec<NativeTerm>,
}

impl TermsDiff {
    pub fn is_empty(&self) -> bool {
        self.missing.is_empty() && self.changed.is_empty() && self.extra.is_empty()
    }
}

/// How `yours` differ from the `room`'s terms.
pub fn compare(room: &[NativeTerm], yours: &[NativeTerm]) -> TermsDiff {
    let mine: BTreeMap<&str, &NativeTerm> = yours.iter().map(|t| (t.id.as_str(), t)).collect();
    let theirs: BTreeMap<&str, &NativeTerm> = room.iter().map(|t| (t.id.as_str(), t)).collect();
    let mut diff = TermsDiff::default();
    for term in room {
        match mine.get(term.id.as_str()) {
            None => diff.missing.push(term.clone()),
            Some(own) if *own != term => diff.changed.push((term.clone(), (*own).clone())),
            Some(_) => {}
        }
    }
    diff.extra = yours
        .iter()
        .filter(|t| !theirs.contains_key(t.id.as_str()))
        .cloned()
        .collect();
    diff
}

/// The packages of the signed `index` that give a member the room's
/// versions of what `diff` says they lack or hold in another version, for
/// the build hashing to `build`. A term the index does not list in that
/// version, or not for this build, is returned as the error: that member
/// cannot join, and nothing is fetched from anywhere else.
pub fn missing_from_index<'a>(
    diff: &TermsDiff,
    index: &'a Index,
    build: &str,
) -> Result<Vec<&'a Package>, NativeTerm> {
    diff.missing
        .iter()
        .chain(diff.changed.iter().map(|(room, _)| room))
        .map(|term| {
            index
                .package(&term.id, &term.version)
                .filter(|package| package.runs_on(build))
                .ok_or_else(|| term.clone())
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::{
        enabled::{EnabledPackage, FORMAT},
        features::example::BIG_MAPS,
        index::tests::{BUILD, OTHER_BUILD, big_maps, index, package},
    };

    fn enabled(settings: &[(&str, Setting)], with_page_only: bool) -> Enabled {
        let mut packages = vec![EnabledPackage {
            id: "bigmap".into(),
            version: "0.3.0".into(),
            simulation: true,
            features: vec!["bigmap.octree".into()],
            settings: settings
                .iter()
                .map(|(k, v)| ((*k).to_owned(), v.clone()))
                .collect(),
            root: PathBuf::new(),
        }];
        if with_page_only {
            packages.push(EnabledPackage {
                id: "pages".into(),
                version: "1.0.0".into(),
                simulation: false,
                features: vec!["bigmap.page".into()],
                settings: BTreeMap::new(),
                root: PathBuf::new(),
            });
        }
        Enabled {
            format: FORMAT,
            build: BUILD.into(),
            packages,
        }
    }

    #[test]
    fn only_simulation_packages_are_terms_and_settings_count() {
        let a = terms(
            &enabled(&[("octree_depth", Setting::Int(10))], true),
            BIG_MAPS,
        );
        assert_eq!(a.len(), 1, "the page-only package is the player's own");
        assert_eq!(a[0].mod_id(), "native:bigmap");
        assert!(a[0].mod_version().starts_with("0.3.0+"));
        assert!(a[0].mod_version().len() <= 32);

        let same = terms(
            &enabled(&[("octree_depth", Setting::Int(10))], false),
            BIG_MAPS,
        );
        assert!(compare(&a, &same).is_empty());

        let deeper = terms(
            &enabled(&[("octree_depth", Setting::Int(11))], false),
            BIG_MAPS,
        );
        let diff = compare(&a, &deeper);
        assert_eq!(diff.changed.len(), 1);
        assert_ne!(a[0].mod_version(), deeper[0].mod_version());

        let diff = compare(&a, &[]);
        assert_eq!(diff.missing, a);
        let diff = compare(&[], &a);
        assert_eq!(diff.extra, a);
    }

    #[test]
    fn a_missing_package_comes_from_the_signed_index_or_not_at_all() {
        let room = terms(&enabled(&[], false), BIG_MAPS);
        let diff = compare(&room, &[]);
        let index = index(vec![big_maps(), package("other", "1.0.0", &[])]);
        let found = missing_from_index(&diff, &index, BUILD).unwrap();
        assert_eq!(found[0].id, "bigmap");
        // Not for this build, or a version the index does not list.
        assert_eq!(
            missing_from_index(&diff, &index, OTHER_BUILD),
            Err(room[0].clone())
        );
        let mut newer = room.clone();
        newer[0].version = "9.0.0".into();
        assert!(missing_from_index(&compare(&newer, &[]), &index, BUILD).is_err());
    }
}
