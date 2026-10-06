//! Native mods on disk: the launcher's own folder for them, and the
//! registry of every file it put there.
//!
//! ```text
//! <launcher data>/native-mods/
//!   native-mods.json, native-mods.json.sig   the last index accepted
//!   registry.json                            every installed package and file
//!   enabled.json                             what the next game runs (crate::enabled)
//!   <id>/<version>/…                         a package's files
//!   <id>/.partial-<version>/                 a download in progress
//! ```
//!
//! Nothing goes into the game's folder. An install downloads every file
//! into the package's `.partial-` folder, checking each against the signed
//! index's size and SHA-256, and only when all of them check out renames
//! the folder into place and records it: a download cut short or tampered
//! with leaves the installed version as it was. An upgrade keeps the
//! version before it, for [`Store::rollback`], and deletes the one before
//! that. Uninstalling deletes exactly the files the registry names, then
//! the folders they leave empty; a file someone else put there stays.
//!
//! One launcher at a time uses the folder (the launcher's instance lock,
//! `tpf3mp-agent`'s `launcher::instance`).

#[cfg(feature = "install")]
use std::time::Duration;
use std::{
    collections::BTreeMap,
    fs, io,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::{
    enabled::{self, Enabled, EnabledPackage},
    fetch::{self, FetchError},
    index::{self, FileEntry, Index, IndexError, Package, Setting},
    resolve::{self, ResolveError},
};

/// The registry's file name.
pub const REGISTRY: &str = "registry.json";
const REGISTRY_FORMAT: u32 = 1;
const PARTIAL: &str = ".partial-";

/// Where packages come from.
pub trait Source {
    /// The index and its signature.
    fn index(&self) -> Result<(Vec<u8>, Vec<u8>), FetchError>;
    /// Downloads `file` into `path`, checked against its size and SHA-256
    /// on the way ([`fetch::copy_verified`]).
    fn download(&self, file: &FileEntry, path: &Path) -> Result<(), FetchError>;
}

/// The index and packages over HTTPS (the `install` feature): `<base>/native-mods.json` and its
/// signature, and each file at the address the signed index gives.
#[cfg(feature = "install")]
pub struct Http {
    base: String,
    https_only: bool,
    user_agent: String,
}

#[cfg(feature = "install")]
impl Http {
    pub fn new(base: impl Into<String>, user_agent: impl Into<String>) -> Self {
        Self {
            base: base.into().trim_end_matches('/').to_owned(),
            https_only: true,
            user_agent: user_agent.into(),
        }
    }

    /// The project's index on GitHub: assets of the `native-mods` release
    /// of `repository` (`owner/name`).
    pub fn github(repository: &str, user_agent: impl Into<String>) -> Self {
        Self::new(
            format!("https://github.com/{repository}/releases/download/native-mods"),
            user_agent,
        )
    }

    /// Allows plain HTTP, for tests on loopback.
    pub fn allow_http(mut self) -> Self {
        self.https_only = false;
        self
    }
}

#[cfg(feature = "install")]
impl Source for Http {
    fn index(&self) -> Result<(Vec<u8>, Vec<u8>), FetchError> {
        let agent = fetch::agent(self.https_only, Duration::from_secs(60));
        let json = fetch::get(
            &agent,
            &format!("{}/{}", self.base, index::INDEX_FILE),
            index::MAX_INDEX,
            &self.user_agent,
        )?;
        let signature = fetch::get(
            &agent,
            &format!("{}/{}", self.base, index::SIGNATURE_FILE),
            256,
            &self.user_agent,
        )?;
        Ok((json, signature))
    }

    fn download(&self, file: &FileEntry, path: &Path) -> Result<(), FetchError> {
        let agent = fetch::agent(self.https_only, Duration::from_secs(3 * 60 * 60));
        fetch::download(
            &agent,
            &file.url,
            path,
            file.size,
            &file.sha256,
            &self.user_agent,
            |_| {},
        )
    }
}

#[derive(Debug, Error)]
pub enum StoreError {
    #[error("{0}")]
    Io(#[from] io::Error),
    #[error(transparent)]
    Fetch(#[from] FetchError),
    #[error(transparent)]
    Index(#[from] IndexError),
    #[error(transparent)]
    Resolve(#[from] ResolveError),
    #[error(
        "the native-mods index offered ({offered}) is older than one already accepted ({seen})"
    )]
    OlderIndex { offered: u64, seen: u64 },
    #[error("no native-mods index has been accepted yet")]
    NoIndex,
    #[error("the native-mods registry cannot be read: {0}")]
    Registry(String),
    #[error("the native mod {0} is not installed")]
    NotInstalled(String),
    #[error("the native mod {0} has no earlier version to go back to")]
    NoPrevious(String),
    #[error("files of {id} {version} are missing or changed: {}", .files.join(", "))]
    Damaged {
        id: String,
        version: String,
        files: Vec<String>,
    },
    #[error("{0} holds files the registry does not name; move them away first")]
    Occupied(PathBuf),
    #[error("{by} needs {id}")]
    Needed { id: String, by: String },
    #[error("{0}")]
    Setting(String),
}

/// Every installed package.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Registry {
    pub format: u32,
    /// The highest index serial accepted.
    pub serial: u64,
    pub packages: BTreeMap<String, Installed>,
}

impl Default for Registry {
    fn default() -> Self {
        Self {
            format: REGISTRY_FORMAT,
            serial: 0,
            packages: BTreeMap::new(),
        }
    }
}

/// One installed package.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Installed {
    /// The version in use.
    pub current: String,
    /// The version before it, kept for a rollback.
    pub previous: Option<String>,
    /// Whether the player switched it on; installing does not.
    pub enabled: bool,
    /// The player's settings, over the package's defaults.
    pub settings: BTreeMap<String, Setting>,
    /// Each version on disk, as the signed index described it: its files
    /// are exactly these.
    pub versions: BTreeMap<String, Package>,
}

impl Installed {
    /// The package in use.
    pub fn package(&self) -> Option<&Package> {
        self.versions.get(&self.current)
    }
}

/// The native-mods folder and its registry.
pub struct Store {
    root: PathBuf,
    registry: Registry,
}

impl Store {
    /// The store in `root` (`<launcher data>/native-mods`). A registry that
    /// cannot be read is an error, never replaced by an empty one.
    pub fn open(root: impl Into<PathBuf>) -> Result<Self, StoreError> {
        let root = root.into();
        let registry = match fs::read(root.join(REGISTRY)) {
            Ok(bytes) => {
                let registry: Registry = serde_json::from_slice(&bytes)
                    .map_err(|error| StoreError::Registry(error.to_string()))?;
                if registry.format != REGISTRY_FORMAT {
                    return Err(StoreError::Registry(format!("format {}", registry.format)));
                }
                registry
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => Registry::default(),
            Err(error) => return Err(error.into()),
        };
        Ok(Self { root, registry })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn registry(&self) -> &Registry {
        &self.registry
    }

    /// The folder of `id` in `version`.
    pub fn folder(&self, id: &str, version: &str) -> PathBuf {
        self.root.join(id).join(version)
    }

    /// Fetches the index from `source` and accepts it ([`Self::accept_index`]).
    pub fn refresh(&mut self, source: &dyn Source, keys: &[Vec<u8>]) -> Result<Index, StoreError> {
        let (json, signature) = source.index()?;
        self.accept_index(&json, &signature, keys)
    }

    /// Accepts `json` as the index if `signature` is one of `keys`' and its
    /// serial is not older than one accepted before, and keeps it.
    pub fn accept_index(
        &mut self,
        json: &[u8],
        signature: &[u8],
        keys: &[Vec<u8>],
    ) -> Result<Index, StoreError> {
        let index = Index::verified(json, signature, keys)?;
        if index.serial < self.registry.serial {
            return Err(StoreError::OlderIndex {
                offered: index.serial,
                seen: self.registry.serial,
            });
        }
        fs::create_dir_all(&self.root)?;
        write_atomic(&self.root.join(index::INDEX_FILE), json)?;
        write_atomic(&self.root.join(index::SIGNATURE_FILE), signature)?;
        self.registry.serial = index.serial;
        self.save()?;
        Ok(index)
    }

    /// The index accepted last, verified again.
    pub fn cached_index(&self, keys: &[Vec<u8>]) -> Result<Index, StoreError> {
        let (Ok(json), Ok(signature)) = (
            fs::read(self.root.join(index::INDEX_FILE)),
            fs::read(self.root.join(index::SIGNATURE_FILE)),
        ) else {
            return Err(StoreError::NoIndex);
        };
        Ok(Index::verified(&json, &signature, keys)?)
    }

    /// Installs `id` (the newest version for the build) and what it needs.
    /// Upgrades it when it is installed. Returns what was installed.
    pub fn install(
        &mut self,
        context: resolve::Context<'_>,
        id: &str,
        source: &dyn Source,
    ) -> Result<Vec<String>, StoreError> {
        let installed: Vec<&Package> = self
            .registry
            .packages
            .values()
            .filter_map(Installed::package)
            .collect();
        let order: Vec<Package> = resolve::resolve(context, &[id], &installed)?
            .into_iter()
            .cloned()
            .collect();
        let mut done = Vec::new();
        for package in &order {
            self.install_one(package, source)?;
            done.push(format!("{} {}", package.id, package.version));
        }
        Ok(done)
    }

    fn install_one(&mut self, package: &Package, source: &dyn Source) -> Result<(), StoreError> {
        let folder = self.folder(&package.id, &package.version);
        let registered = self
            .registry
            .packages
            .get(&package.id)
            .is_some_and(|i| i.versions.contains_key(&package.version));
        if registered {
            // The version kept for a rollback: in use again if it is whole,
            // downloaded again if it is not.
            if self.damaged(&folder, package)?.is_empty() {
                return self.make_current(package);
            }
            remove_registered(&folder, package)?;
            if let Some(installed) = self.registry.packages.get_mut(&package.id) {
                installed.versions.remove(&package.version);
            }
        } else if folder.exists() {
            // Left by an install cut short after the rename: taken if it is
            // exactly the package, refused otherwise.
            if !self.damaged(&folder, package)?.is_empty() {
                return Err(StoreError::Occupied(folder));
            }
            return self.make_current(package);
        }
        let partial = self
            .root
            .join(&package.id)
            .join(format!("{PARTIAL}{}", package.version));
        if partial.exists() {
            fs::remove_dir_all(&partial)?;
        }
        fs::create_dir_all(&partial)?;
        let downloaded = package.files.iter().try_for_each(|file| {
            let path = partial.join(&file.path);
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent)?;
            }
            source.download(file, &path).map_err(StoreError::from)
        });
        if let Err(error) = downloaded {
            let _ = fs::remove_dir_all(&partial);
            return Err(error);
        }
        if let Err(error) = fs::rename(&partial, &folder) {
            let _ = fs::remove_dir_all(&partial);
            return Err(error.into());
        }
        self.make_current(package)
    }

    /// Records `package`, on disk, as its id's version in use, keeping the
    /// one before and deleting older ones.
    fn make_current(&mut self, package: &Package) -> Result<(), StoreError> {
        let entry = self
            .registry
            .packages
            .entry(package.id.clone())
            .or_insert_with(|| Installed {
                current: package.version.clone(),
                previous: None,
                enabled: false,
                settings: BTreeMap::new(),
                versions: BTreeMap::new(),
            });
        entry
            .versions
            .insert(package.version.clone(), package.clone());
        if entry.current != package.version {
            entry.previous = Some(std::mem::replace(
                &mut entry.current,
                package.version.clone(),
            ));
        }
        // The player's settings that the new version still has, with the
        // same type.
        entry.settings.retain(|key, value| {
            package
                .settings
                .get(key)
                .is_some_and(|default| default.same_type(value))
        });
        let keep = [Some(entry.current.clone()), entry.previous.clone()];
        let old: Vec<Package> = entry
            .versions
            .values()
            .filter(|p| !keep.contains(&Some(p.version.clone())))
            .cloned()
            .collect();
        for package in &old {
            entry.versions.remove(&package.version);
        }
        self.save()?;
        for package in old {
            remove_registered(&self.folder(&package.id, &package.version), &package)?;
        }
        Ok(())
    }

    /// Deletes every file of `id` the registry names, and the folders that
    /// leaves empty. Refused while another installed package needs it.
    pub fn uninstall(&mut self, id: &str) -> Result<(), StoreError> {
        let installed = self
            .registry
            .packages
            .get(id)
            .ok_or_else(|| StoreError::NotInstalled(id.to_owned()))?;
        if let Some(by) = self
            .registry
            .packages
            .values()
            .filter_map(Installed::package)
            .find(|p| p.id != id && p.depends.iter().any(|d| d.id == id))
        {
            return Err(StoreError::Needed {
                id: id.to_owned(),
                by: by.id.clone(),
            });
        }
        let versions: Vec<Package> = installed.versions.values().cloned().collect();
        self.registry.packages.remove(id);
        self.save()?;
        for package in &versions {
            remove_registered(&self.folder(id, &package.version), package)?;
        }
        let folder = self.root.join(id);
        if let Ok(entries) = fs::read_dir(&folder) {
            for entry in entries.flatten() {
                if entry.file_name().to_string_lossy().starts_with(PARTIAL) {
                    let _ = fs::remove_dir_all(entry.path());
                }
            }
        }
        let _ = fs::remove_dir(&folder);
        Ok(())
    }

    /// Goes back to the version in use before the last upgrade, if its
    /// files are still whole.
    pub fn rollback(&mut self, id: &str) -> Result<(), StoreError> {
        let installed = self
            .registry
            .packages
            .get(id)
            .ok_or_else(|| StoreError::NotInstalled(id.to_owned()))?;
        let previous = installed
            .previous
            .as_ref()
            .and_then(|v| installed.versions.get(v))
            .ok_or_else(|| StoreError::NoPrevious(id.to_owned()))?
            .clone();
        let damaged = self.damaged(&self.folder(id, &previous.version), &previous)?;
        if !damaged.is_empty() {
            return Err(StoreError::Damaged {
                id: id.to_owned(),
                version: previous.version,
                files: damaged,
            });
        }
        self.make_current(&previous)
    }

    /// Checks the files of the version of `id` in use.
    pub fn verify(&self, id: &str) -> Result<(), StoreError> {
        let package = self
            .registry
            .packages
            .get(id)
            .and_then(Installed::package)
            .ok_or_else(|| StoreError::NotInstalled(id.to_owned()))?;
        let damaged = self.damaged(&self.folder(id, &package.version), package)?;
        if damaged.is_empty() {
            Ok(())
        } else {
            Err(StoreError::Damaged {
                id: id.to_owned(),
                version: package.version.clone(),
                files: damaged,
            })
        }
    }

    /// Switches `id` on or off for the next game.
    pub fn set_enabled(&mut self, id: &str, on: bool) -> Result<(), StoreError> {
        self.registry
            .packages
            .get_mut(id)
            .ok_or_else(|| StoreError::NotInstalled(id.to_owned()))?
            .enabled = on;
        self.save()
    }

    /// Sets one of `id`'s settings: one its package has, with the same type.
    pub fn set_setting(&mut self, id: &str, key: &str, value: Setting) -> Result<(), StoreError> {
        let installed = self
            .registry
            .packages
            .get_mut(id)
            .ok_or_else(|| StoreError::NotInstalled(id.to_owned()))?;
        let package = installed
            .versions
            .get(&installed.current)
            .ok_or_else(|| StoreError::NotInstalled(id.to_owned()))?;
        match package.settings.get(key) {
            Some(default) if default.same_type(&value) => {
                installed.settings.insert(key.to_owned(), value);
                self.save()
            }
            Some(_) => Err(StoreError::Setting(format!(
                "{id}'s setting {key} has another type"
            ))),
            None => Err(StoreError::Setting(format!("{id} has no setting {key}"))),
        }
    }

    /// The enabled packages for the game build hashing to `build`, with
    /// their settings. An enabled package not for this build is left out,
    /// with its id in the second list.
    pub fn enabled_for(&self, build: &str) -> (Enabled, Vec<String>) {
        let mut packages = Vec::new();
        let mut left_out = Vec::new();
        for (id, installed) in &self.registry.packages {
            let Some(package) = installed.package().filter(|_| installed.enabled) else {
                continue;
            };
            if !package.runs_on(build) {
                left_out.push(id.clone());
                continue;
            }
            let mut settings = package.settings.clone();
            settings.extend(installed.settings.clone());
            packages.push(EnabledPackage {
                id: id.clone(),
                version: package.version.clone(),
                simulation: package.simulation,
                features: package.features.clone(),
                settings,
                root: self.folder(id, &package.version),
            });
        }
        (
            Enabled {
                format: enabled::FORMAT,
                build: build.to_owned(),
                packages,
            },
            left_out,
        )
    }

    /// Writes what the next game on `build` runs to `enabled.json`, for
    /// the launcher to name in the game's environment ([`enabled::ENV`]).
    pub fn write_enabled(&self, build: &str) -> Result<(PathBuf, Vec<String>), StoreError> {
        let (enabled, left_out) = self.enabled_for(build);
        fs::create_dir_all(&self.root)?;
        let path = self.root.join(enabled::FILE);
        let json =
            serde_json::to_vec_pretty(&enabled).map_err(|e| StoreError::Registry(e.to_string()))?;
        write_atomic(&path, &json)?;
        Ok((path, left_out))
    }

    /// The files of `package` in `folder` that are missing or changed.
    fn damaged(&self, folder: &Path, package: &Package) -> Result<Vec<String>, StoreError> {
        let mut damaged = Vec::new();
        for file in &package.files {
            if !fetch::file_matches(&folder.join(&file.path), file.size, &file.sha256)? {
                damaged.push(file.path.clone());
            }
        }
        Ok(damaged)
    }

    fn save(&self) -> Result<(), StoreError> {
        fs::create_dir_all(&self.root)?;
        let json = serde_json::to_vec_pretty(&self.registry)
            .map_err(|error| StoreError::Registry(error.to_string()))?;
        write_atomic(&self.root.join(REGISTRY), &json)?;
        Ok(())
    }
}

/// Deletes the files `package` names in `folder`, then the folders that
/// leaves empty, `folder` last. Nothing else.
fn remove_registered(folder: &Path, package: &Package) -> io::Result<()> {
    let mut dirs = Vec::new();
    for file in &package.files {
        let path = folder.join(&file.path);
        match fs::remove_file(&path) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
        let mut parent = path.parent();
        while let Some(dir) = parent.filter(|dir| dir.starts_with(folder) && *dir != folder) {
            dirs.push(dir.to_owned());
            parent = dir.parent();
        }
    }
    // Deepest first; a folder that is not empty stays.
    dirs.sort_by_key(|dir| std::cmp::Reverse(dir.components().count()));
    dirs.dedup();
    for dir in dirs {
        let _ = fs::remove_dir(dir);
    }
    let _ = fs::remove_dir(folder);
    Ok(())
}

/// Writes `bytes` to `path` through a file beside it, so that a reader
/// sees the old contents or the new, never part of them.
fn write_atomic(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let mut temporary = path.as_os_str().to_owned();
    temporary.push(".new");
    let temporary = PathBuf::from(temporary);
    {
        use io::Write;
        let mut file = fs::File::create(&temporary)?;
        file.write_all(bytes)?;
        file.sync_all()?;
    }
    fs::rename(&temporary, path)
}

#[cfg(test)]
mod tests {
    use std::{cell::RefCell, collections::HashMap};

    use super::*;
    use crate::{
        enabled,
        features::example::BIG_MAPS,
        index::{
            Dependency,
            tests::{BUILD, OTHER_BUILD, big_maps, index, keys, package, signed, test_key, url_of},
        },
        resolve::Context,
    };

    /// Serves files from memory, as a server would: bytes by address, some
    /// cut short.
    #[derive(Default)]
    struct Served {
        files: RefCell<HashMap<String, Vec<u8>>>,
        index: RefCell<Option<(Vec<u8>, Vec<u8>)>>,
    }

    impl Served {
        fn serve(&self, id: &str, version: &str, files: &[(&str, &[u8])]) {
            for (path, bytes) in files {
                self.files
                    .borrow_mut()
                    .insert(url_of(id, version, path), bytes.to_vec());
            }
        }
    }

    impl Source for Served {
        fn index(&self) -> Result<(Vec<u8>, Vec<u8>), FetchError> {
            self.index
                .borrow()
                .clone()
                .ok_or_else(|| FetchError::Http("404".into()))
        }

        fn download(&self, file: &FileEntry, path: &Path) -> Result<(), FetchError> {
            let files = self.files.borrow();
            let bytes = files
                .get(&file.url)
                .ok_or_else(|| FetchError::Http("404".into()))?;
            fetch::copy_verified(&bytes[..], path, file.size, &file.sha256, |_| {})
        }
    }

    const V1: &[(&str, &[u8])] = &[
        ("mod/tpf3mp_bigmap_1/mod.lua", b"-- version 1"),
        ("data/sizes.toml", b"sizes = [128]"),
    ];
    const V2: &[(&str, &[u8])] = &[
        ("mod/tpf3mp_bigmap_1/mod.lua", b"-- version 2"),
        ("data/sizes.toml", b"sizes = [128, 256]"),
        ("data/new.toml", b"new = true"),
    ];

    fn context(index: &Index) -> Context<'_> {
        Context {
            index,
            build: BUILD,
            registry: BIG_MAPS,
        }
    }

    /// Every file under `dir`, relative, sorted.
    fn tree(dir: &Path) -> Vec<String> {
        let mut out = Vec::new();
        let mut stack = vec![dir.to_owned()];
        while let Some(at) = stack.pop() {
            let Ok(entries) = fs::read_dir(&at) else {
                continue;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    stack.push(path.clone());
                }
                out.push(
                    path.strip_prefix(dir)
                        .unwrap()
                        .to_string_lossy()
                        .replace('\\', "/"),
                );
            }
        }
        out.sort();
        out
    }

    #[test]
    fn an_install_puts_verified_files_in_the_launchers_folder_and_registers_them() {
        let dir = tempfile::tempdir().unwrap();
        let served = Served::default();
        served.serve("bigmap", "1.0.0", V1);
        let index = index(vec![package("bigmap", "1.0.0", V1)]);
        let mut store = Store::open(dir.path().join("native-mods")).unwrap();
        let done = store.install(context(&index), "bigmap", &served).unwrap();
        assert_eq!(done, ["bigmap 1.0.0"]);
        let folder = store.folder("bigmap", "1.0.0");
        assert_eq!(
            fs::read(folder.join("mod/tpf3mp_bigmap_1/mod.lua")).unwrap(),
            b"-- version 1"
        );
        store.verify("bigmap").unwrap();
        // The registry survives a restart; installing is not enabling.
        let again = Store::open(dir.path().join("native-mods")).unwrap();
        let installed = &again.registry().packages["bigmap"];
        assert_eq!(installed.current, "1.0.0");
        assert!(!installed.enabled);
        assert_eq!(installed.package().unwrap().files.len(), 2);
    }

    #[test]
    fn a_wrong_hash_or_a_partial_download_installs_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("native-mods");
        let index = index(vec![package("bigmap", "1.0.0", V1)]);
        for body in [&b"-- version X"[..], &b"-- ver"[..]] {
            let served = Served::default();
            served.serve("bigmap", "1.0.0", V1);
            served
                .files
                .borrow_mut()
                .insert(url_of("bigmap", "1.0.0", "data/sizes.toml"), body.to_vec());
            let mut store = Store::open(&root).unwrap();
            assert!(matches!(
                store.install(context(&index), "bigmap", &served),
                Err(StoreError::Fetch(FetchError::Mismatch))
            ));
            assert!(store.registry().packages.is_empty());
            // No package folder, no partial download left.
            assert!(tree(&root.join("bigmap")).is_empty(), "{:?}", tree(&root));
        }
    }

    #[test]
    fn a_package_not_pinned_to_this_build_is_not_installed() {
        let dir = tempfile::tempdir().unwrap();
        let served = Served::default();
        served.serve("bigmap", "1.0.0", V1);
        let mut other = package("bigmap", "1.0.0", V1);
        other.builds = vec![OTHER_BUILD.into()];
        let index = index(vec![other]);
        let mut store = Store::open(dir.path()).unwrap();
        assert!(matches!(
            store.install(context(&index), "bigmap", &served),
            Err(StoreError::Resolve(ResolveError::NotForBuild(_)))
        ));
        assert!(tree(dir.path()).is_empty());
    }

    #[test]
    fn only_a_signed_index_not_older_than_the_last_is_accepted() {
        let dir = tempfile::tempdir().unwrap();
        let pair = test_key();
        let mut store = Store::open(dir.path()).unwrap();
        let mut newer = index(vec![package("bigmap", "1.0.0", V1)]);
        newer.serial = 5;
        let (json, signature) = signed(&newer, &pair);
        let served = Served::default();
        *served.index.borrow_mut() = Some((json.clone(), signature.clone()));
        assert_eq!(store.refresh(&served, &keys(&pair)).unwrap(), newer);
        assert_eq!(store.cached_index(&keys(&pair)).unwrap(), newer);
        // Unsigned, tampered, another key.
        assert!(matches!(
            store.accept_index(&json, b"", &keys(&pair)),
            Err(StoreError::Index(IndexError::BadSignature))
        ));
        assert!(matches!(
            store.accept_index(&json, &signature, &keys(&test_key())),
            Err(StoreError::Index(IndexError::BadSignature))
        ));
        assert!(matches!(
            store.cached_index(&keys(&test_key())),
            Err(StoreError::Index(IndexError::BadSignature))
        ));
        // An older index, signed, is a replay.
        let mut older = newer.clone();
        older.serial = 4;
        let (json, signature) = signed(&older, &pair);
        assert!(matches!(
            store.accept_index(&json, &signature, &keys(&pair)),
            Err(StoreError::OlderIndex {
                offered: 4,
                seen: 5
            })
        ));
        assert_eq!(store.cached_index(&keys(&pair)).unwrap().serial, 5);
    }

    #[test]
    fn uninstalling_leaves_nothing_of_the_package() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("native-mods");
        let served = Served::default();
        served.serve("bigmap", "1.0.0", V1);
        served.serve("bigmap", "2.0.0", V2);
        let mut store = Store::open(&root).unwrap();
        let first = index(vec![package("bigmap", "1.0.0", V1)]);
        store.install(context(&first), "bigmap", &served).unwrap();
        let second = index(vec![
            package("bigmap", "1.0.0", V1),
            package("bigmap", "2.0.0", V2),
        ]);
        store.install(context(&second), "bigmap", &served).unwrap();
        store.set_enabled("bigmap", true).unwrap();
        store.write_enabled(BUILD).unwrap();
        // A file someone else put there.
        fs::write(root.join("bigmap/2.0.0/data/mine.txt"), b"mine").unwrap();
        store.uninstall("bigmap").unwrap();
        assert!(store.registry().packages.is_empty());
        assert_eq!(
            tree(&root.join("bigmap")),
            ["2.0.0", "2.0.0/data", "2.0.0/data/mine.txt"]
        );
        fs::remove_dir_all(root.join("bigmap")).unwrap();

        // Without it, nothing of the package is left at all.
        store.install(context(&second), "bigmap", &served).unwrap();
        store.uninstall("bigmap").unwrap();
        assert!(!root.join("bigmap").exists(), "{:?}", tree(&root));
        assert!(matches!(
            store.uninstall("bigmap"),
            Err(StoreError::NotInstalled(_))
        ));
    }

    #[test]
    fn an_upgrade_keeps_the_old_version_until_the_new_one_verifies() {
        let dir = tempfile::tempdir().unwrap();
        let served = Served::default();
        served.serve("bigmap", "1.0.0", V1);
        let mut store = Store::open(dir.path()).unwrap();
        let first = index(vec![package("bigmap", "1.0.0", V1)]);
        store.install(context(&first), "bigmap", &served).unwrap();
        store
            .set_setting("bigmap", "octree_depth", Setting::Int(9))
            .unwrap_err();

        // Version 2's server hands out a damaged file.
        let second = index(vec![
            package("bigmap", "1.0.0", V1),
            package("bigmap", "2.0.0", V2),
        ]);
        served.serve("bigmap", "2.0.0", V2);
        served.files.borrow_mut().insert(
            url_of("bigmap", "2.0.0", "data/new.toml"),
            b"new = 1!!!".to_vec(),
        );
        assert!(store.install(context(&second), "bigmap", &served).is_err());
        assert_eq!(store.registry().packages["bigmap"].current, "1.0.0");
        store.verify("bigmap").unwrap();
        assert!(!store.folder("bigmap", "2.0.0").exists());

        // Fixed: 2 in use, 1 kept for a rollback.
        served.serve("bigmap", "2.0.0", V2);
        store.install(context(&second), "bigmap", &served).unwrap();
        let installed = &store.registry().packages["bigmap"];
        assert_eq!(
            (installed.current.as_str(), installed.previous.as_deref()),
            ("2.0.0", Some("1.0.0"))
        );
        assert!(store.folder("bigmap", "1.0.0").exists());

        store.rollback("bigmap").unwrap();
        let installed = &store.registry().packages["bigmap"];
        assert_eq!(
            (installed.current.as_str(), installed.previous.as_deref()),
            ("1.0.0", Some("2.0.0"))
        );
        store.verify("bigmap").unwrap();

        // A third version: the one before the previous goes.
        const V3: &[(&str, &[u8])] = &[("mod/tpf3mp_bigmap_1/mod.lua", b"-- version 3")];
        served.serve("bigmap", "3.0.0", V3);
        let third = index(vec![
            package("bigmap", "1.0.0", V1),
            package("bigmap", "2.0.0", V2),
            package("bigmap", "3.0.0", V3),
        ]);
        store.install(context(&third), "bigmap", &served).unwrap();
        let installed = &store.registry().packages["bigmap"];
        assert_eq!(
            (installed.current.as_str(), installed.previous.as_deref()),
            ("3.0.0", Some("1.0.0"))
        );
        assert!(!store.folder("bigmap", "2.0.0").exists());
    }

    #[test]
    fn a_rollback_to_damaged_files_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let served = Served::default();
        served.serve("bigmap", "1.0.0", V1);
        served.serve("bigmap", "2.0.0", V2);
        let mut store = Store::open(dir.path()).unwrap();
        let both = index(vec![
            package("bigmap", "1.0.0", V1),
            package("bigmap", "2.0.0", V2),
        ]);
        let first = index(vec![package("bigmap", "1.0.0", V1)]);
        assert!(matches!(
            store.rollback("bigmap"),
            Err(StoreError::NotInstalled(_))
        ));
        store.install(context(&first), "bigmap", &served).unwrap();
        assert!(matches!(
            store.rollback("bigmap"),
            Err(StoreError::NoPrevious(_))
        ));
        store.install(context(&both), "bigmap", &served).unwrap();
        fs::write(
            store.folder("bigmap", "1.0.0").join("data/sizes.toml"),
            b"x",
        )
        .unwrap();
        assert!(matches!(
            store.rollback("bigmap"),
            Err(StoreError::Damaged { .. })
        ));
        assert_eq!(store.registry().packages["bigmap"].current, "2.0.0");
    }

    #[test]
    fn a_needed_package_stays_and_dependencies_install_first() {
        let dir = tempfile::tempdir().unwrap();
        let served = Served::default();
        served.serve("base", "1.0.0", V1);
        served.serve("addon", "1.0.0", &[("addon.lua", b"--")]);
        let mut addon = package("addon", "1.0.0", &[("addon.lua", b"--")]);
        addon.depends.push(Dependency {
            id: "base".into(),
            version: "^1".into(),
        });
        let index = index(vec![package("base", "1.0.0", V1), addon]);
        let mut store = Store::open(dir.path()).unwrap();
        let done = store.install(context(&index), "addon", &served).unwrap();
        assert_eq!(done, ["base 1.0.0", "addon 1.0.0"]);
        assert!(matches!(
            store.uninstall("base"),
            Err(StoreError::Needed { .. })
        ));
        store.uninstall("addon").unwrap();
        store.uninstall("base").unwrap();
        assert!(tree(dir.path()).iter().all(|f| f.starts_with(REGISTRY)));
    }

    #[test]
    fn a_folder_left_by_a_cut_install_is_taken_only_if_it_is_the_package() {
        let dir = tempfile::tempdir().unwrap();
        let served = Served::default();
        let index = index(vec![package("bigmap", "1.0.0", V1)]);
        let mut store = Store::open(dir.path()).unwrap();
        let folder = store.folder("bigmap", "1.0.0");
        for (path, bytes) in V1 {
            let file = folder.join(path);
            fs::create_dir_all(file.parent().unwrap()).unwrap();
            fs::write(file, bytes).unwrap();
        }
        // Nothing is served: the folder is taken as it is.
        store.install(context(&index), "bigmap", &served).unwrap();
        assert_eq!(store.registry().packages["bigmap"].current, "1.0.0");

        let dir = tempfile::tempdir().unwrap();
        let mut store = Store::open(dir.path()).unwrap();
        let folder = store.folder("bigmap", "1.0.0");
        fs::create_dir_all(&folder).unwrap();
        fs::write(folder.join("other.txt"), b"?").unwrap();
        assert!(matches!(
            store.install(context(&index), "bigmap", &served),
            Err(StoreError::Occupied(_))
        ));
        assert!(folder.join("other.txt").exists());
    }

    #[test]
    fn the_hook_gets_the_enabled_packages_for_its_build_with_settings() {
        let dir = tempfile::tempdir().unwrap();
        let served = Served::default();
        let files: &[(&str, &[u8])] = &[("mod/tpf3mp_bigmap_1/mod.lua", b"-- ")];
        served.serve("bigmap", "0.3.0", files);
        let index = index(vec![big_maps()]);
        let mut store = Store::open(dir.path()).unwrap();
        store.install(context(&index), "bigmap", &served).unwrap();

        let (path, _) = store.write_enabled(BUILD).unwrap();
        assert!(
            enabled::read(&path).unwrap().packages.is_empty(),
            "not enabled yet"
        );

        store.set_enabled("bigmap", true).unwrap();
        store
            .set_setting("bigmap", "octree_depth", Setting::Int(11))
            .unwrap();
        assert!(matches!(
            store.set_setting("bigmap", "octree_depth", Setting::Bool(true)),
            Err(StoreError::Setting(_))
        ));
        assert!(matches!(
            store.set_setting("bigmap", "nonsense", Setting::Int(1)),
            Err(StoreError::Setting(_))
        ));
        let (path, left_out) = store.write_enabled(BUILD).unwrap();
        assert!(left_out.is_empty());
        let list = enabled::read(&path).unwrap();
        let package = &list.packages[0];
        assert_eq!(package.settings["octree_depth"], Setting::Int(11));
        assert_eq!(package.settings["street_raster"], Setting::Bool(true));
        assert_eq!(package.root, store.folder("bigmap", "0.3.0"));
        let plan = enabled::plan(&list, BUILD, BIG_MAPS, |_| true, true);
        assert!(plan.feature("bigmap.octree").is_some());

        // Another build: left out.
        let (path, left_out) = store.write_enabled(OTHER_BUILD).unwrap();
        assert_eq!(left_out, ["bigmap"]);
        assert!(enabled::read(&path).unwrap().packages.is_empty());
    }

    #[test]
    fn an_unreadable_registry_is_not_replaced() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join(REGISTRY), b"{ broken").unwrap();
        assert!(matches!(
            Store::open(dir.path()),
            Err(StoreError::Registry(_))
        ));
        assert_eq!(fs::read(dir.path().join(REGISTRY)).unwrap(), b"{ broken");
    }
}
