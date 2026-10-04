//! Which mods the room's world loads with in this player's game, and with
//! which settings (docs/MODS.md, proposed D25 and D28).
//!
//! The room's players share the room's mods and may each have personal ones
//! (`tpf3mp-modscan`). The agent knows both lists: the room's are those its
//! owner declared and the room told every player, in the room's order, with
//! the settings the owner picked; the personal ones are this player's alone.
//! It hands them to the hook when the room's game begins
//! ([`crate::ToHook::Begin`]).
//!
//! A save lists the mods of the game that wrote it, that player's personal
//! ones included, and the game loads a save with its own list and settings
//! unless told otherwise (`app.loadGame(id, isMapEditor, info)`, where
//! `info.mods` and `info.modParams` replace them: the game's own Load Game
//! page does exactly this when the player changes a save's mods or their
//! settings, `gui/menu/savegame_react_util.tl`, build 40408). [`plan`] says
//! what to load instead: the room's mods in the room's order, then this
//! player's personal mods; [`settings`] which settings.

use serde::{Deserialize, Serialize};
use tpf3mp_proto::{BoundedVec, ModParams, Text};

/// A mod's name as the game lists it (`Mod.ModId.name`).
pub type ModName = Text<96>;
/// Most shared mods the lists carry: as many as a room runs.
pub const MAX_SHARED_MODS: usize = tpf3mp_proto::MAX_ROOM_MODS;
/// Most personal mods the lists carry.
pub const MAX_PERSONAL_MODS: usize = 64;
/// TPF3-MP's own mod, which every game of a room runs, listed or not.
pub const OWN_MOD: &str = tpf3mp_proto::OWN_MOD;

/// This player's mods for the room's world.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModLists {
    /// The room's mods, which every player of the room runs, in load order.
    pub shared: BoundedVec<ModName, MAX_SHARED_MODS>,
    /// This player's personal mods, in load order.
    pub personal: BoundedVec<ModName, MAX_PERSONAL_MODS>,
    /// The settings of the room's mods the room's owner picked; a room's
    /// mod without any loads with the save's settings, or the game's
    /// defaults.
    pub params: Vec<ModParams>,
}

/// What [`plan`] made of a save's mods.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plan {
    /// The mods to load the save with, in load order.
    pub mods: Vec<String>,
    /// The save's mods left out: other players' personal mods, mods the
    /// room's owner left out, in no game of the room.
    pub dropped: Vec<String>,
    /// The mods the save did not have: the room's that its owner added,
    /// and this player's personal ones.
    pub added: Vec<String>,
}

/// The mods to load a save that lists `save` with: the room's mods in the
/// room's order, TPF3-MP's own among them (last, unless the room lists it
/// elsewhere), then this player's personal mods not among them, in the
/// order of `lists.personal`. Every game of the room loads the same room's
/// mods in the same order, whatever its save lists: a mod the room's owner
/// added is added in every game alike, one they left out is left out.
pub fn plan(save: &[String], lists: &ModLists) -> Plan {
    let mut mods: Vec<String> = Vec::new();
    for name in lists.shared.iter() {
        let name = name.as_str().to_owned();
        if !mods.contains(&name) {
            mods.push(name);
        }
    }
    if !mods.iter().any(|name| name == OWN_MOD) {
        mods.push(OWN_MOD.to_owned());
    }
    for own in lists.personal.iter() {
        let own = own.as_str().to_owned();
        if !mods.contains(&own) {
            mods.push(own);
        }
    }
    let mut dropped = Vec::new();
    for name in save {
        if !mods.contains(name) && !dropped.contains(name) {
            dropped.push(name.clone());
        }
    }
    let added = mods
        .iter()
        .filter(|name| !save.contains(name))
        .cloned()
        .collect();
    Plan {
        mods,
        dropped,
        added,
    }
}

/// The settings to load the room's world with, as the game keeps them
/// (`modParams`: by mod, by setting): the save's, with each of the room's
/// mods that has settings in `lists.params` taking exactly those, and the
/// game's own entry (`""`, `GAME_SETTINGS`) too when the room carries it.
/// Other mods' stay the save's.
pub fn settings(
    save: &[(String, Vec<(String, i64)>)],
    lists: &ModLists,
) -> Vec<(String, Vec<(String, i64)>)> {
    let mut out: Vec<(String, Vec<(String, i64)>)> = save
        .iter()
        .filter(|(id, _)| !lists.params.iter().any(|of| of.id.as_str() == id))
        .cloned()
        .collect();
    for of in &lists.params {
        out.push((
            of.id.as_str().to_owned(),
            of.params
                .iter()
                .map(|param| (param.key.as_str().to_owned(), param.value))
                .collect(),
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    use tpf3mp_proto::ModParam;

    use super::*;

    fn names(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| (*s).to_owned()).collect()
    }

    fn lists(shared: &[&str], personal: &[&str]) -> ModLists {
        let to = |l: &[&str]| l.iter().map(|s| Text::new(*s).unwrap()).collect::<Vec<_>>();
        ModLists {
            shared: BoundedVec::new(to(shared)).unwrap(),
            personal: BoundedVec::new(to(personal)).unwrap(),
            params: Vec::new(),
        }
    }

    #[test]
    fn the_owners_personal_mod_is_left_out_and_mine_added() {
        // The owner saved the start world with their minimap; this player
        // runs line colours instead.
        let save = names(&["vehicles_pack", "tpf3mp_1", "owner_minimap"]);
        let plan = plan(
            &save,
            &lists(&["vehicles_pack", "tpf3mp_1"], &["my_line_colours"]),
        );
        assert_eq!(plan.mods, ["vehicles_pack", "tpf3mp_1", "my_line_colours"]);
        assert_eq!(plan.dropped, ["owner_minimap"]);
        assert_eq!(plan.added, ["my_line_colours"]);
    }

    #[test]
    fn a_personal_mod_both_have_loads_once() {
        let save = names(&["overlay", "tpf3mp_1"]);
        let plan = plan(&save, &lists(&["tpf3mp_1"], &["overlay"]));
        assert_eq!(plan.mods, ["tpf3mp_1", "overlay"]);
        assert!(plan.dropped.is_empty());
        assert!(plan.added.is_empty());
    }

    /// The room's list decides, in every game alike: a mod its owner added
    /// loads though the save lacks it, one they left out does not, and the
    /// room's order holds whatever order the save had.
    #[test]
    fn the_rooms_list_and_order_decide_whatever_the_save_lists() {
        let save = names(&["b", "a", "a", "left_out", "tpf3mp_1"]);
        let plan = plan(&save, &lists(&["a", "b", "added", "tpf3mp_1"], &[]));
        assert_eq!(plan.mods, ["a", "b", "added", "tpf3mp_1"]);
        assert_eq!(plan.dropped, ["left_out"]);
        assert_eq!(plan.added, ["added"]);
    }

    #[test]
    fn tpf3mp_itself_is_kept_unlisted() {
        let plan = plan(&names(&["tpf3mp_1"]), &ModLists::default());
        assert_eq!(plan.mods, ["tpf3mp_1"]);
    }

    #[test]
    fn the_rooms_settings_replace_the_saves_for_its_mods_only() {
        let save = vec![
            (String::new(), vec![("seed".to_owned(), 7)]),
            ("signals".to_owned(), vec![("distance".to_owned(), 1)]),
            ("towns".to_owned(), vec![("size".to_owned(), 2)]),
        ];
        let mut room = lists(&["signals", "towns", "tpf3mp_1"], &[]);
        room.params = vec![ModParams {
            id: Text::lossy("signals"),
            params: vec![ModParam {
                key: Text::lossy("distance"),
                value: 3,
            }],
        }];
        let mut got = settings(&save, &room);
        got.sort();
        assert_eq!(
            got,
            [
                (String::new(), vec![("seed".to_owned(), 7)]),
                ("signals".to_owned(), vec![("distance".to_owned(), 3)]),
                ("towns".to_owned(), vec![("size".to_owned(), 2)]),
            ]
        );
    }
}
