//! The signed index of native mods.
//!
//! `native-mods.json` lists every package the launcher may install, and
//! `native-mods.json.sig` is an Ed25519 signature of its exact bytes with
//! the project's native-mods key ([`crate::signed`]). An index that does not
//! verify, names a format this launcher does not read, or holds one entry
//! that does not check out is refused whole: the project signs it, so a
//! malformed entry is a mistake to fix there, never something to guess
//! around.
//!
//! ```json
//! {
//!   "format": 1,
//!   "serial": 12,
//!   "packages": [{
//!     "id": "bigmap", "version": "0.3.0", "name": "Big Maps",
//!     "simulation": true,
//!     "builds": ["<the game executable's SHA-256>"],
//!     "features": ["bigmap.octree", "bigmap.street_raster", "bigmap.page"],
//!     "settings": {"octree_depth": 10, "street_raster": true},
//!     "depends": [], "conflicts": [],
//!     "files": [{"path": "mod/tpf3mp_bigmap_1/mod.lua",
//!                "url": "https://…", "size": 1234, "sha256": "…"}]
//!   }]
//! }
//! ```
//!
//! `serial` only grows: the launcher remembers the highest it accepted and
//! refuses an older index, so an old signed index cannot be replayed to
//! bring back a withdrawn package.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::signed;

/// The index format this launcher reads.
pub const FORMAT: u32 = 1;
/// The index's file name.
pub const INDEX_FILE: &str = "native-mods.json";
/// Its signature's file name.
pub const SIGNATURE_FILE: &str = "native-mods.json.sig";
/// Largest index read.
pub const MAX_INDEX: u64 = 1 << 20;
/// Largest file a package may ship.
pub const MAX_FILE: u64 = 1 << 30;
/// Most files one package may ship.
pub const MAX_FILES: usize = 4096;
/// Longest package version, so that the room's terms fit a mod version
/// ([`crate::terms`]).
pub const MAX_VERSION: usize = 20;

/// The signed list of native mods.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Index {
    pub format: u32,
    /// Grows with every index published; an older one is refused.
    pub serial: u64,
    pub packages: Vec<Package>,
}

/// One version of one native mod.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Package {
    /// Lowercase letters, digits and `_`.
    pub id: String,
    /// Semantic version.
    pub version: String,
    /// What players are shown.
    pub name: String,
    #[serde(default)]
    pub description: String,
    /// Whether it changes how the world runs. If so, every game of a room
    /// must run it in the same version with the same settings
    /// ([`crate::terms`]). A feature the hook knows to change the
    /// simulation makes the package count as changing it too.
    pub simulation: bool,
    /// The game builds it runs on, by the executable's SHA-256, as hook
    /// profiles pin them. On any other build it is neither installed nor
    /// enabled.
    pub builds: Vec<String>,
    /// The hook features it enables ([`crate::features`]).
    #[serde(default)]
    pub features: Vec<String>,
    /// Its settings and their defaults. A player's settings must name
    /// these and keep their types.
    #[serde(default)]
    pub settings: BTreeMap<String, Setting>,
    #[serde(default)]
    pub depends: Vec<Dependency>,
    /// Packages that must not be installed beside it.
    #[serde(default)]
    pub conflicts: Vec<String>,
    /// Its data, Lua and settings files.
    #[serde(default)]
    pub files: Vec<FileEntry>,
    /// Native libraries the hook would load. Not built: this launcher
    /// refuses a package that has any ([`ResolveError::NeedsPlugins`]).
    ///
    /// [`ResolveError::NeedsPlugins`]: crate::resolve::ResolveError::NeedsPlugins
    #[serde(default)]
    pub plugins: Vec<Plugin>,
}

/// A setting's value: what a package's defaults and a player's choices
/// hold.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Setting {
    Bool(bool),
    Int(i64),
    Text(String),
}

impl Setting {
    /// Whether `other` is a value of the same type.
    pub fn same_type(&self, other: &Self) -> bool {
        std::mem::discriminant(self) == std::mem::discriminant(other)
    }
}

/// A package another needs, in versions matching `version` (a semver
/// requirement such as `^1.2`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Dependency {
    pub id: String,
    pub version: String,
}

/// One file of a package: where it goes in the package's folder, where it
/// comes from, and what it must be.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FileEntry {
    /// Plain names separated by `/`, inside the package's folder.
    pub path: String,
    pub url: String,
    pub size: u64,
    /// Lowercase hex.
    pub sha256: String,
}

/// A native library a later launcher may load into the hook: signed in the
/// index like every file, and built against the hook's plugin interface
/// `abi`. Reserved; nothing loads one yet.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Plugin {
    pub path: String,
    pub url: String,
    pub size: u64,
    pub sha256: String,
    pub abi: u32,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum IndexError {
    #[error("the native-mods index is not signed with the project's key")]
    BadSignature,
    #[error("the native-mods index is malformed: {0}")]
    Malformed(String),
    #[error("the native-mods index has format {0}; this launcher reads format {FORMAT}")]
    Format(u32),
}

impl Index {
    /// The index in `json`, if `signature` is one of `keys`' signatures of
    /// it and every entry checks out.
    pub fn verified(json: &[u8], signature: &[u8], keys: &[Vec<u8>]) -> Result<Self, IndexError> {
        if !signed::verify(json, signature, keys) {
            return Err(IndexError::BadSignature);
        }
        let index: Self = serde_json::from_slice(json)
            .map_err(|error| IndexError::Malformed(error.to_string()))?;
        if index.format != FORMAT {
            return Err(IndexError::Format(index.format));
        }
        index.validate().map_err(IndexError::Malformed)?;
        Ok(index)
    }

    fn validate(&self) -> Result<(), String> {
        let mut seen = BTreeSet::new();
        for package in &self.packages {
            package
                .validate()
                .map_err(|problem| format!("{} {}: {problem}", package.id, package.version))?;
            if !seen.insert((package.id.as_str(), package.version.as_str())) {
                return Err(format!(
                    "{} {} is listed twice",
                    package.id, package.version
                ));
            }
        }
        Ok(())
    }

    /// The package `id` in `version`.
    pub fn package(&self, id: &str, version: &str) -> Option<&Package> {
        self.packages
            .iter()
            .find(|p| p.id == id && p.version == version)
    }

    /// Every version of `id`, newest first.
    pub fn versions(&self, id: &str) -> Vec<&Package> {
        let mut versions: Vec<&Package> = self.packages.iter().filter(|p| p.id == id).collect();
        versions.sort_by_key(|p| std::cmp::Reverse(p.semver()));
        versions
    }
}

impl Package {
    /// Its version, parsed; validated packages always have one.
    pub fn semver(&self) -> semver::Version {
        semver::Version::parse(&self.version).unwrap_or(semver::Version::new(0, 0, 0))
    }

    /// Whether it is pinned to the game build whose executable hashes to
    /// `build`.
    pub fn runs_on(&self, build: &str) -> bool {
        self.builds.iter().any(|pinned| pinned == build)
    }

    fn validate(&self) -> Result<(), String> {
        if !is_id(&self.id) {
            return Err("the id is not lowercase letters, digits and _".into());
        }
        if self.version.len() > MAX_VERSION || semver::Version::parse(&self.version).is_err() {
            return Err("the version is not a short semantic version".into());
        }
        if self.name.trim().is_empty() || self.name.len() > 64 {
            return Err("the name is empty or too long".into());
        }
        if self.builds.is_empty() || !self.builds.iter().all(|b| signed::is_sha256(b)) {
            return Err("it pins no game build, or one that is not a SHA-256".into());
        }
        if let Some(bad) = self.features.iter().find(|f| !is_feature_id(f)) {
            return Err(format!("the feature id {bad:?} is malformed"));
        }
        if let Some(bad) = self.settings.keys().find(|k| !is_id(k)) {
            return Err(format!("the setting {bad:?} is malformed"));
        }
        for dependency in &self.depends {
            if !is_id(&dependency.id) || semver::VersionReq::parse(&dependency.version).is_err() {
                return Err(format!("the dependency {:?} is malformed", dependency.id));
            }
        }
        if let Some(bad) = self.conflicts.iter().find(|c| !is_id(c)) {
            return Err(format!("the conflict {bad:?} is malformed"));
        }
        if self.files.len() > MAX_FILES {
            return Err("too many files".into());
        }
        let mut paths = BTreeSet::new();
        let entries = self
            .files
            .iter()
            .map(|f| (&f.path, &f.url, f.size, &f.sha256))
            .chain(
                self.plugins
                    .iter()
                    .map(|p| (&p.path, &p.url, p.size, &p.sha256)),
            );
        for (path, url, size, sha256) in entries {
            if !is_safe_path(path) {
                return Err(format!("the file path {path:?} is not a plain path"));
            }
            // Windows folds case: two names differing only in case are one
            // file there.
            if !paths.insert(path.to_ascii_lowercase()) {
                return Err(format!("the file {path:?} is listed twice"));
            }
            if !url.starts_with("https://") && !url.starts_with("http://") {
                return Err(format!("the file {path:?} has no web address"));
            }
            if size > MAX_FILE || !signed::is_sha256(sha256) {
                return Err(format!("the file {path:?} is too large or has no SHA-256"));
            }
        }
        Ok(())
    }
}

/// A package or setting id: 1 to 40 lowercase letters, digits and `_`.
pub fn is_id(text: &str) -> bool {
    (1..=40).contains(&text.len())
        && text
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
}

/// A feature id: ids joined by dots, such as `bigmap.octree`.
pub fn is_feature_id(text: &str) -> bool {
    text.len() <= 64 && text.split('.').all(is_id)
}

/// A path inside a package's folder: plain names joined by `/`, none of
/// them hidden, relative, or holding a drive or another separator.
pub fn is_safe_path(path: &str) -> bool {
    !path.is_empty()
        && path.len() <= 240
        && path.split('/').all(|part| {
            !part.is_empty()
                && !part.starts_with('.')
                && !part.ends_with('.')
                && !part.ends_with(' ')
                && part
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-' | b' '))
        })
}

#[cfg(test)]
pub(crate) mod tests {
    use ring::{
        rand::SystemRandom,
        signature::{Ed25519KeyPair, KeyPair},
    };

    use super::*;

    /// A TEST key, made fresh for each test; never a real one.
    pub(crate) fn test_key() -> Ed25519KeyPair {
        let pkcs8 = Ed25519KeyPair::generate_pkcs8(&SystemRandom::new()).unwrap();
        Ed25519KeyPair::from_pkcs8(pkcs8.as_ref()).unwrap()
    }

    pub(crate) fn keys(pair: &Ed25519KeyPair) -> Vec<Vec<u8>> {
        vec![pair.public_key().as_ref().to_vec()]
    }

    pub(crate) const BUILD: &str =
        "1111111111111111111111111111111111111111111111111111111111111111";
    pub(crate) const OTHER_BUILD: &str =
        "2222222222222222222222222222222222222222222222222222222222222222";

    /// A package of `files` (path, bytes), served from `https://test/`.
    pub(crate) fn package(id: &str, version: &str, files: &[(&str, &[u8])]) -> Package {
        Package {
            id: id.into(),
            version: version.into(),
            name: id.into(),
            description: String::new(),
            simulation: false,
            builds: vec![BUILD.into()],
            features: Vec::new(),
            settings: BTreeMap::new(),
            depends: Vec::new(),
            conflicts: Vec::new(),
            files: files
                .iter()
                .map(|(path, bytes)| FileEntry {
                    path: (*path).into(),
                    url: url_of(id, version, path),
                    size: bytes.len() as u64,
                    sha256: signed::sha256_hex(bytes),
                })
                .collect(),
            plugins: Vec::new(),
        }
    }

    pub(crate) fn url_of(id: &str, version: &str, path: &str) -> String {
        format!("https://test/{id}/{version}/{path}")
    }

    pub(crate) fn index(packages: Vec<Package>) -> Index {
        Index {
            format: FORMAT,
            serial: 1,
            packages,
        }
    }

    pub(crate) fn signed(index: &Index, pair: &Ed25519KeyPair) -> (Vec<u8>, Vec<u8>) {
        let json = serde_json::to_vec(index).unwrap();
        let signature = pair.sign(&json).as_ref().to_vec();
        (json, signature)
    }

    /// The example in docs/NATIVE_MODS.md, Big Maps as a package.
    pub(crate) fn big_maps() -> Package {
        let mut package = package(
            "bigmap",
            "0.3.0",
            &[("mod/tpf3mp_bigmap_1/mod.lua", b"-- ")],
        );
        package.name = "Big Maps".into();
        package.simulation = true;
        package.features = [
            "bigmap.octree",
            "bigmap.street_raster",
            "bigmap.placement",
            "bigmap.page",
            "bigmap.density",
            "bigmap.memory_gate",
        ]
        .map(String::from)
        .to_vec();
        package.settings = BTreeMap::from([
            ("octree_depth".into(), Setting::Int(10)),
            ("street_raster".into(), Setting::Bool(true)),
        ]);
        package
    }

    #[test]
    fn a_signed_index_verifies_and_reads() {
        let pair = test_key();
        let (json, signature) = signed(&index(vec![big_maps()]), &pair);
        let read = Index::verified(&json, &signature, &keys(&pair)).unwrap();
        assert_eq!(read.packages[0].features.len(), 6);
        assert!(read.package("bigmap", "0.3.0").unwrap().runs_on(BUILD));
        assert!(
            !read
                .package("bigmap", "0.3.0")
                .unwrap()
                .runs_on(OTHER_BUILD)
        );
    }

    #[test]
    fn a_tampered_index_is_refused() {
        let pair = test_key();
        let (json, signature) = signed(&index(vec![big_maps()]), &pair);
        let tampered = String::from_utf8(json)
            .unwrap()
            .replace("\"simulation\":true", "\"simulation\":false")
            .into_bytes();
        assert_eq!(
            Index::verified(&tampered, &signature, &keys(&pair)),
            Err(IndexError::BadSignature)
        );
    }

    #[test]
    fn an_index_signed_with_another_key_is_refused() {
        let pair = test_key();
        let (json, signature) = signed(&index(vec![big_maps()]), &pair);
        assert_eq!(
            Index::verified(&json, &signature, &keys(&test_key())),
            Err(IndexError::BadSignature)
        );
        assert_eq!(
            Index::verified(&json, &signature, &[]),
            Err(IndexError::BadSignature)
        );
    }

    #[test]
    fn another_format_is_refused() {
        let pair = test_key();
        let mut future = index(vec![big_maps()]);
        future.format = 2;
        let (json, signature) = signed(&future, &pair);
        assert_eq!(
            Index::verified(&json, &signature, &keys(&pair)),
            Err(IndexError::Format(2))
        );
    }

    #[test]
    fn one_bad_entry_refuses_the_whole_index() {
        let pair = test_key();
        let bad: [fn(&mut Package); 9] = [
            |p| p.builds.clear(),
            |p| p.builds = vec!["not a hash".into()],
            |p| p.files[0].path = "../escape.lua".into(),
            |p| p.files[0].path = "C:/Windows/x.dll".into(),
            |p| p.files[0].path = "mod\\x.lua".into(),
            |p| p.files[0].sha256 = "ABC".into(),
            |p| p.version = "latest".into(),
            |p| p.id = "Big Maps".into(),
            |p| p.features = vec!["bigmap..octree".into()],
        ];
        for (n, spoil) in bad.iter().enumerate() {
            let mut package = big_maps();
            spoil(&mut package);
            let (json, signature) = signed(&index(vec![package]), &pair);
            assert!(
                matches!(
                    Index::verified(&json, &signature, &keys(&pair)),
                    Err(IndexError::Malformed(_))
                ),
                "case {n}"
            );
        }
        let (json, signature) = signed(&index(vec![big_maps(), big_maps()]), &pair);
        assert!(matches!(
            Index::verified(&json, &signature, &keys(&pair)),
            Err(IndexError::Malformed(_))
        ));
    }

    #[test]
    fn an_unknown_field_is_refused() {
        let pair = test_key();
        let json = serde_json::to_string(&index(vec![big_maps()]))
            .unwrap()
            .replacen("\"serial\"", "\"run_this\":\"x\",\"serial\"", 1)
            .into_bytes();
        let signature = pair.sign(&json).as_ref().to_vec();
        assert!(matches!(
            Index::verified(&json, &signature, &keys(&pair)),
            Err(IndexError::Malformed(_))
        ));
    }

    #[test]
    fn versions_come_newest_first() {
        let index = index(vec![
            package("a", "1.2.0", &[]),
            package("a", "1.10.0", &[]),
            package("a", "1.9.1", &[]),
        ]);
        let order: Vec<&str> = index
            .versions("a")
            .iter()
            .map(|p| p.version.as_str())
            .collect();
        assert_eq!(order, ["1.10.0", "1.9.1", "1.2.0"]);
    }
}
