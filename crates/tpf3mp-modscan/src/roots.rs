//! Where Transport Fever 3 keeps the mods a player can activate (build
//! 40408, Windows; the other platforms' places are to confirm):
//!
//! - Mod Hub (mod.io) downloads: `%PUBLIC%\mod.io\10640\mods\<mod.io id>`
//!   (58 mods on one PC, 2026-10-03, while `%LOCALAPPDATA%\mod.io\10640`
//!   held only its user's file), and `%LOCALAPPDATA%\mod.io\10640\mods`,
//!   where they were found on 2026-09-30; each `mod.json` names the mod
//!   (`revyn112_towns_de` in `...\6414521`);
//! - local mods: `<Steam>\userdata\<account>\3493540\local\staging_area\<modId>`
//!   (investigation/TF3_MODS_2026-09-27.md), and `...\local\mods`;
//! - the game's own: `<game>\mods`, `<game>\mods\release` and
//!   `<game>\dlcs`. `release` holds the game's built-in mods, the ones a
//!   save lists as `urbangames_no_costs_1`, `urbangames_sandbox_1` and so
//!   on, and the campaign's (21 mods on one PC, 2026-10-04; the `mw_*` among
//!   them were also in `<game>\mods`, with the same files).
//!
//! A mod is found by the id its `mod.json` gives, or else by its folder's
//! name. A save lists a Mod Hub mod by the `modId` (`revyn112_towns_de`,
//! its mod.io number `6414521` only as the hub id; docs/MODS.md).

use std::{
    fs,
    path::{Path, PathBuf},
};

/// Transport Fever 3's game id on mod.io.
pub const MODIO_GAME: u32 = 10640;
/// Transport Fever 3's Steam app id.
pub const STEAM_APP: u32 = 3_493_540;

/// The folders mods are kept in, those that exist: the game's own (in
/// `game`, its install folder), each Steam account's local ones (under
/// each of `steam_roots`), and Mod Hub's downloads (under each of `data`,
/// the folders that may hold a `mod.io` folder, in order).
pub fn roots(game: Option<&Path>, steam_roots: &[PathBuf], data: &[PathBuf]) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for data in data {
        out.push(
            data.join("mod.io")
                .join(MODIO_GAME.to_string())
                .join("mods"),
        );
    }
    for steam in steam_roots {
        let Ok(accounts) = fs::read_dir(steam.join("userdata")) else {
            continue;
        };
        let mut accounts: Vec<PathBuf> =
            accounts.filter_map(Result::ok).map(|e| e.path()).collect();
        accounts.sort();
        for account in accounts {
            let local = account.join(STEAM_APP.to_string()).join("local");
            out.push(local.join("staging_area"));
            out.push(local.join("mods"));
        }
    }
    if let Some(game) = game {
        out.push(game.join("mods"));
        out.push(game.join("mods").join("release"));
        out.push(game.join("dlcs"));
    }
    out.retain(|root| root.is_dir());
    out
}

/// This player's roots, as [`roots`] with the folders mod.io keeps its
/// downloads in.
pub fn default_roots(game: Option<&Path>, steam_roots: &[PathBuf]) -> Vec<PathBuf> {
    roots(game, steam_roots, &modio_data())
}

/// The folders that may hold mod.io's `mod.io` folder, the first copy of a
/// mod counting: on Windows `%PUBLIC%` (the users' shared folder, where
/// Mod Hub downloads its mods for every user of the PC), then
/// `%LOCALAPPDATA%`; elsewhere the per-user local data folder.
fn modio_data() -> Vec<PathBuf> {
    #[cfg(windows)]
    let found = windows_modio_data(|var| std::env::var_os(var));
    #[cfg(not(windows))]
    let found = dirs_local().into_iter().collect();
    found
}

/// [`modio_data`] on Windows, with `env` reading the environment: those of
/// the two folders that are set.
#[cfg_attr(not(windows), allow(dead_code))]
fn windows_modio_data(env: impl Fn(&str) -> Option<std::ffi::OsString>) -> Vec<PathBuf> {
    ["PUBLIC", "LOCALAPPDATA"]
        .into_iter()
        .filter_map(|var| env(var).map(PathBuf::from))
        .collect()
}

#[cfg(not(windows))]
fn dirs_local() -> Option<PathBuf> {
    // $XDG_DATA_HOME or ~/.local/share on Linux; ~/Library/Application
    // Support on macOS.
    std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME").map(|home| {
                let home = PathBuf::from(home);
                if cfg!(target_os = "macos") {
                    home.join("Library/Application Support")
                } else {
                    home.join(".local/share")
                }
            })
        })
}

/// One mod found in a root.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Found {
    /// Its id, from `mod.json`, else the folder's name.
    pub id: String,
    /// Its folder's name.
    pub folder: String,
    pub path: PathBuf,
}

/// Every mod in `roots`: each folder with a `mod.json`, in the roots'
/// order, then by folder name.
pub fn installed(roots: &[PathBuf]) -> Vec<Found> {
    let mut out = Vec::new();
    for root in roots {
        let Ok(entries) = fs::read_dir(root) else {
            continue;
        };
        let mut dirs: Vec<PathBuf> = entries
            .filter_map(Result::ok)
            .map(|e| e.path())
            .filter(|p| p.join("mod.json").is_file())
            .collect();
        dirs.sort();
        for path in dirs {
            let folder = path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            let id = fs::read_to_string(path.join("mod.json"))
                .ok()
                .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok())
                .and_then(|v| v.get("modId").and_then(|id| id.as_str()).map(str::to_owned))
                .unwrap_or_else(|| folder.clone());
            out.push(Found { id, folder, path });
        }
    }
    out
}

/// The folder of the mod named `name`, by id first, then by folder name.
pub fn find<'a>(found: &'a [Found], name: &str) -> Option<&'a Found> {
    found
        .iter()
        .find(|f| f.id == name)
        .or_else(|| found.iter().find(|f| f.folder == name))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(path: &Path, text: &str) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }

    #[test]
    fn mods_are_found_in_mod_hub_steam_and_the_game_folders() {
        let dir = tempfile::tempdir().unwrap();
        let public = dir.path().join("public");
        let data = dir.path().join("data");
        let steam = dir.path().join("steam");
        let game = dir.path().join("game");
        write(
            &public.join("mod.io/10640/mods/6414521/mod.json"),
            r#"{"modId": "revyn112_towns_de"}"#,
        );
        write(
            &data.join("mod.io/10640/mods/6037864/mod.json"),
            r#"{"modId": "celmi_timetables"}"#,
        );
        write(
            &steam.join("userdata/42/3493540/local/staging_area/gw_big_city_1/mod.json"),
            "{}",
        );
        write(
            &game.join("dlcs/urbangames_preorder_pack/mod.json"),
            r#"{"modId": "urbangames_preorder_pack"}"#,
        );
        // The game's built-in mods, in a folder of their own; once taken
        // for not a mod, as it has no mod.json itself (2026-10-04).
        write(
            &game.join("mods/release/urbangames_no_costs/mod.json"),
            r#"{"modId": "urbangames_no_costs_1"}"#,
        );

        let roots = roots(
            Some(&game),
            std::slice::from_ref(&steam),
            &[public, data, dir.path().join("none")],
        );
        assert_eq!(roots.len(), 6, "{roots:?}");
        let found = installed(&roots);
        let ids: Vec<&str> = found.iter().map(|f| f.id.as_str()).collect();
        assert_eq!(
            ids,
            [
                "revyn112_towns_de",
                "celmi_timetables",
                "gw_big_city_1",
                "urbangames_no_costs_1",
                "urbangames_preorder_pack"
            ]
        );
        assert_eq!(find(&found, "celmi_timetables").unwrap().folder, "6037864");
        assert_eq!(find(&found, "6037864").unwrap().id, "celmi_timetables");
        assert!(find(&found, "missing").is_none());
    }

    /// Mod Hub's downloads were missed on a PC that keeps them in the
    /// users' shared folder, so a room's start save listed mods its owner
    /// had as missing (2026-10-03).
    #[test]
    fn mod_hub_downloads_are_looked_for_in_the_shared_folder_first() {
        let env = |public: bool, local: bool| {
            move |var: &str| match var {
                "PUBLIC" if public => Some(r"C:\Users\Public".into()),
                "LOCALAPPDATA" if local => Some(r"C:\Users\p\AppData\Local".into()),
                _ => None,
            }
        };
        let public = PathBuf::from(r"C:\Users\Public");
        let local = PathBuf::from(r"C:\Users\p\AppData\Local");
        assert_eq!(
            windows_modio_data(env(true, true)),
            [public.clone(), local.clone()]
        );
        assert_eq!(windows_modio_data(env(false, true)), [local]);
        assert_eq!(windows_modio_data(env(true, false)), [public]);
        assert!(windows_modio_data(env(false, false)).is_empty());
    }
}
