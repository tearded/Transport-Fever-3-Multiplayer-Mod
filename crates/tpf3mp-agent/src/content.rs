//! What this player's game runs, until the game's hook reports it: the
//! game's build, and its mods from a list the player keeps.
//!
//! The list names the active mods in load order, one per line: the mod's
//! name, then its version if it has one. Blank lines and lines starting
//! with `#` are skipped.
//!
//! ```text
//! # my mods, in the order the game loads them
//! urbangames_vehicles 1.4
//! more_stations 2
//! ```

use std::{fs, path::Path};

use thiserror::Error;
use tpf3mp_bridge::{ModLists, ModName};
use tpf3mp_modscan::{Class, roots};
use tpf3mp_proto::{BoundedVec, ContentManifest, ModRef, Text};

/// Longest mod list read, in bytes: far more than any real list.
const MAX_LIST_BYTES: u64 = 4 << 20;

#[derive(Debug, Error)]
pub enum ContentError {
    #[error("reading {path}: {source}")]
    Read {
        path: String,
        source: std::io::Error,
    },
    #[error("{path} is larger than a list of mods can be")]
    TooLarge { path: String },
    #[error("{path}, line {line}: {problem}")]
    Line {
        path: String,
        line: usize,
        problem: &'static str,
    },
    #[error("the game build name is too long")]
    Build,
}

/// The manifest of `game_build` running the mods listed in `mods`, or none.
pub fn manifest(game_build: &str, mods: Option<&Path>) -> Result<ContentManifest, ContentError> {
    let game = Text::new(game_build.trim()).map_err(|_| ContentError::Build)?;
    let mods = match mods {
        Some(path) => read_mods(path)?,
        None => Vec::new(),
    };
    Ok(ContentManifest::new(game, mods))
}

/// This player's mods sorted for the room (docs/MODS.md, proposed D25).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Split {
    /// What the player declares to the room: the build and the shared
    /// mods, which every player of the room must run alike.
    pub manifest: ContentManifest,
    /// The lists the hook loads the room's worlds with, or none when they do
    /// not fit the bridge's bounds (a world then loads with its save's list).
    pub lists: Option<ModLists>,
    /// Each listed mod, whether it is shared, and why: for the log.
    pub verdicts: Vec<Verdict>,
}

/// What became of one listed mod.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Verdict {
    pub listed: ModRef,
    pub class: Class,
    /// Why, in a line: where it was found and the first reason, or that it
    /// was not found.
    pub why: String,
}

/// The manifest of `game_build` running the mods listed in `mods`, with
/// the personal ones left out of it: each listed mod is looked for among
/// `installed` (`tpf3mp_modscan::roots::installed`) and scanned. A mod the
/// scan calls personal is the player's own; one it calls shared, one it
/// cannot find, and TPF3-MP itself are the room's (fail closed). One it
/// calls carried (a game-script mod whose commands the room carries,
/// `tpf3mp/modguard.lua`) is the player's own only with `carried_personal`,
/// an opt-in until the room lets game-script mods differ (docs/MODS.md).
pub fn split(
    game_build: &str,
    mods: Option<&Path>,
    installed: &[roots::Found],
    carried_personal: bool,
) -> Result<Split, ContentError> {
    let game = Text::new(game_build.trim()).map_err(|_| ContentError::Build)?;
    let Some(path) = mods else {
        return Ok(Split {
            manifest: ContentManifest::new(game, Vec::new()),
            lists: None,
            verdicts: Vec::new(),
        });
    };
    let listed = read_mods(path)?;
    let mut shared = Vec::new();
    let mut personal = Vec::new();
    let mut verdicts = Vec::new();
    for mut listed in listed {
        let (class, why) = if listed.id.as_str() == tpf3mp_bridge::mods::OWN_MOD {
            // Its version is the listed one and the fingerprint of the files
            // the game loads (`crate::own_mod`), where it is installed.
            match roots::find(installed, listed.id.as_str()) {
                Some(found) => {
                    listed.version = Text::lossy(&crate::own_mod::version(
                        listed.version.as_str(),
                        &found.path,
                    ));
                    (
                        Class::Shared,
                        format!("TPF3-MP itself, in {}", found.path.display()),
                    )
                }
                None => (
                    Class::Shared,
                    "TPF3-MP itself, not found among the installed mods: its files are not compared"
                        .to_owned(),
                ),
            }
        } else {
            match roots::find(installed, listed.id.as_str()) {
                None => (
                    Class::Shared,
                    "not found among the installed mods, so shared".to_owned(),
                ),
                Some(found) => {
                    let report = tpf3mp_modscan::scan(&found.path);
                    let why = report.sharing().next().map_or_else(
                        || format!("{}: nothing in it reaches the world", found.path.display()),
                        |reason| format!("{}: {reason}", found.path.display()),
                    );
                    (report.class, why)
                }
            }
        };
        let own = match class {
            Class::Personal => true,
            Class::Carried => carried_personal,
            Class::Shared => false,
        };
        if !own {
            shared.push(listed.clone());
        } else {
            personal.push(listed.clone());
        }
        verdicts.push(Verdict { listed, class, why });
    }
    let names = |list: &[ModRef]| -> Option<Vec<ModName>> {
        list.iter()
            .map(|m| ModName::new(m.id.as_str()).ok())
            .collect()
    };
    let lists = (|| {
        Some(ModLists {
            shared: BoundedVec::new(names(&shared)?).ok()?,
            personal: BoundedVec::new(names(&personal)?).ok()?,
            params: Vec::new(),
        })
    })();
    Ok(Split {
        manifest: ContentManifest::new(game, shared),
        lists,
        verdicts,
    })
}

/// The mods listed in `path`, in order.
pub fn read_mods(path: &Path) -> Result<Vec<ModRef>, ContentError> {
    let shown = path.display().to_string();
    let read = |source| ContentError::Read {
        path: shown.clone(),
        source,
    };
    if fs::metadata(path).map_err(read)?.len() > MAX_LIST_BYTES {
        return Err(ContentError::TooLarge { path: shown });
    }
    let text = fs::read_to_string(path).map_err(read)?;
    parse_mods(&text).map_err(|(line, problem)| ContentError::Line {
        path: shown,
        line,
        problem,
    })
}

fn parse_mods(text: &str) -> Result<Vec<ModRef>, (usize, &'static str)> {
    let mut mods = Vec::new();
    for (index, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let mut words = line.split_whitespace();
        let id = words.next().unwrap_or_default();
        let version = words.next().unwrap_or_default();
        if words.next().is_some() {
            return Err((index + 1, "expected a mod's name and at most its version"));
        }
        mods.push(ModRef {
            id: Text::new(id).map_err(|_| (index + 1, "the mod's name is too long"))?,
            version: Text::new(version).map_err(|_| (index + 1, "the version is too long"))?,
        });
    }
    Ok(mods)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_list_names_mods_in_load_order() {
        let mods =
            parse_mods("# my mods\n\nurbangames_vehicles 1.4\n  more_stations 2  \nno_version\n")
                .unwrap();
        let names: Vec<(&str, &str)> = mods
            .iter()
            .map(|listed| (listed.id.as_str(), listed.version.as_str()))
            .collect();
        assert_eq!(
            names,
            [
                ("urbangames_vehicles", "1.4"),
                ("more_stations", "2"),
                ("no_version", "")
            ]
        );
    }

    #[test]
    fn a_line_that_is_not_a_mod_is_refused_with_its_number() {
        assert_eq!(
            parse_mods("a 1\nb 2 extra\n").unwrap_err(),
            (2, "expected a mod's name and at most its version")
        );
        let long = "x".repeat(200);
        assert_eq!(
            parse_mods(&format!("{long} 1\n")).unwrap_err(),
            (1, "the mod's name is too long")
        );
    }

    /// A mod folder with `files`, as `roots::installed` finds it.
    fn installed_mod(root: &Path, folder: &str, files: &[(&str, &str)]) {
        for (path, text) in files {
            let path = root.join(folder).join(path);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, text).unwrap();
        }
    }

    #[test]
    fn personal_mods_stay_out_of_what_the_room_compares() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("mods");
        installed_mod(
            &root,
            "overlay_1",
            &[
                ("mod.json", r#"{"modId": "overlay_1"}"#),
                (
                    "content/gui/o.res.lua",
                    "function data() return { type = \"react-plugin ::GameBarInfoDisplayExtension\" } end",
                ),
            ],
        );
        installed_mod(
            &root,
            "6037864",
            &[
                ("mod.json", r#"{"modId": "timetables"}"#),
                ("content/tt.gs.lua", "function data() return {} end"),
            ],
        );
        let list = dir.path().join("mods.txt");
        fs::write(
            &list,
            "timetables 8\noverlay_1 1\ntpf3mp_1 1\nnot_installed 2\n",
        )
        .unwrap();
        let found = roots::installed(std::slice::from_ref(&root));

        let split = split("40408", Some(&list), &found, false).unwrap();
        let declared: Vec<&str> = split.manifest.mods.iter().map(|m| m.id.as_str()).collect();
        assert_eq!(declared, ["timetables", "tpf3mp_1", "not_installed"]);
        // TPF3-MP is not installed here: its listed version stands.
        assert_eq!(split.manifest.mods[1].version.as_str(), "1");
        let lists = split.lists.unwrap();
        assert_eq!(
            lists.shared.iter().map(|m| m.as_str()).collect::<Vec<_>>(),
            declared
        );
        assert_eq!(
            lists
                .personal
                .iter()
                .map(|m| m.as_str())
                .collect::<Vec<_>>(),
            ["overlay_1"]
        );
        let whys: Vec<(&str, Class)> = split
            .verdicts
            .iter()
            .map(|v| (v.listed.id.as_str(), v.class))
            .collect();
        assert_eq!(
            whys,
            [
                ("timetables", Class::Carried),
                ("overlay_1", Class::Personal),
                ("tpf3mp_1", Class::Shared),
                ("not_installed", Class::Shared),
            ]
        );
        assert!(split.verdicts[0].why.contains("game script"));
        assert!(split.verdicts[3].why.contains("not found"));

        // With game-script mods let differ, the timetable mod is this
        // player's own too.
        let opted = super::split("40408", Some(&list), &found, true).unwrap();
        let opted: Vec<&str> = opted
            .lists
            .as_ref()
            .unwrap()
            .personal
            .iter()
            .map(|m| m.as_str())
            .collect();
        assert_eq!(opted, ["timetables", "overlay_1"]);

        // Two players who differ only in personal mods declare the same.
        fs::write(&list, "timetables 8\ntpf3mp_1 1\nnot_installed 2\n").unwrap();
        let other = super::split("40408", Some(&list), &found, false).unwrap();
        assert_eq!(other.manifest.fingerprint(), split.manifest.fingerprint());
        // Without a list, no lists: worlds load with their saves' mods.
        let bare = super::split("40408", None, &found, false).unwrap();
        assert_eq!(bare.lists, None);
        assert_eq!(bare.manifest, manifest("40408", None).unwrap());
    }

    #[test]
    fn the_same_list_gives_the_same_fingerprint() {
        let dir = std::env::temp_dir().join(format!("tpf3mp-mods-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let file = dir.join("mods.txt");
        fs::write(&file, "trains 1\nstations 2\n").unwrap();
        let one = manifest("35924", Some(&file)).unwrap();
        let two = manifest("35924", Some(&file)).unwrap();
        assert_eq!(one.fingerprint(), two.fingerprint());
        assert_ne!(
            one.fingerprint(),
            manifest("35924", None).unwrap().fingerprint()
        );
        fs::remove_dir_all(&dir).unwrap();
    }
}
