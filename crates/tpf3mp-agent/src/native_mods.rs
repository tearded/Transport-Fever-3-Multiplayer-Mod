//! Native mods the player enabled (proposed D29, docs/NATIVE_MODS.md), handed
//! to the game the launcher starts.
//!
//! Before the game starts, the launcher writes the enabled packages for the
//! game's build (the executable's SHA-256) to `native-mods/enabled.json` in
//! its data folder and names that file in the game's environment
//! (`TPF3MP_NATIVE_MODS`); the hook enables nothing else. With nothing
//! enabled, or no native mod installed, the game gets no variable and the
//! executable is not even hashed.

use std::path::Path;

use tpf3mp_nativemods::{
    FOLDER, enabled,
    fetch::sha256_of_file,
    store::{REGISTRY, Store},
};
use tracing::{info, warn};

/// The variable for the game's environment naming the native mods enabled
/// for `exe`, with the launcher's data folder `data_dir`; `None` when none
/// are. A registry that cannot be read stops the start: what it enables
/// cannot be told (fail closed).
pub fn game_env(data_dir: &Path, exe: &Path) -> Result<Option<(String, String)>, String> {
    let root = data_dir.join(FOLDER);
    if !root.join(REGISTRY).exists() {
        return Ok(None);
    }
    let store = Store::open(&root).map_err(|error| format!("native mods: {error}"))?;
    if !store.registry().packages.values().any(|i| i.enabled) {
        return Ok(None);
    }
    let build = sha256_of_file(exe)
        .map_err(|error| format!("native mods: cannot hash {}: {error}", exe.display()))?;
    let (path, left_out) = store
        .write_enabled(&build)
        .map_err(|error| format!("native mods: {error}"))?;
    for id in left_out {
        warn!(%id, "the native mod is enabled but not for this build of the game; left out");
    }
    info!(file = %path.display(), "the game gets the enabled native mods");
    Ok(Some((
        enabled::ENV.to_owned(),
        path.to_string_lossy().into_owned(),
    )))
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    #[test]
    fn nothing_installed_or_enabled_passes_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let exe = dir.path().join("game.exe");
        fs::write(&exe, b"not really a game").unwrap();
        assert_eq!(game_env(dir.path(), &exe).unwrap(), None);

        // Installed, not enabled.
        let root = dir.path().join(FOLDER);
        fs::create_dir_all(&root).unwrap();
        fs::write(
            root.join(REGISTRY),
            br#"{"format":1,"serial":3,"packages":{}}"#,
        )
        .unwrap();
        assert_eq!(game_env(dir.path(), &exe).unwrap(), None);

        // Enabled for this build: the game is told where the list is.
        let package = tpf3mp_nativemods::index::Package {
            id: "pages".into(),
            version: "1.0.0".into(),
            name: "Pages".into(),
            description: String::new(),
            simulation: false,
            builds: vec![sha256_of_file(&exe).unwrap()],
            features: vec!["bigmap.page".into()],
            settings: Default::default(),
            depends: Vec::new(),
            conflicts: Vec::new(),
            files: Vec::new(),
            plugins: Vec::new(),
        };
        let registry = tpf3mp_nativemods::store::Registry {
            format: 1,
            serial: 3,
            packages: [(
                "pages".to_owned(),
                tpf3mp_nativemods::store::Installed {
                    current: "1.0.0".into(),
                    previous: None,
                    enabled: true,
                    settings: Default::default(),
                    versions: [("1.0.0".to_owned(), package)].into(),
                },
            )]
            .into(),
        };
        fs::write(root.join(REGISTRY), serde_json::to_vec(&registry).unwrap()).unwrap();
        let (name, path) = game_env(dir.path(), &exe).unwrap().unwrap();
        assert_eq!(name, enabled::ENV);
        let list = enabled::read(Path::new(&path)).unwrap();
        assert_eq!(list.packages[0].id, "pages");

        // Another build of the game: the list is empty.
        fs::write(&exe, b"another build").unwrap();
        let (_, path) = game_env(dir.path(), &exe).unwrap().unwrap();
        assert!(enabled::read(Path::new(&path)).unwrap().packages.is_empty());

        // A registry that cannot be read stops the start.
        fs::write(root.join(REGISTRY), b"{ broken").unwrap();
        assert!(game_env(dir.path(), &exe).is_err());
    }
}
