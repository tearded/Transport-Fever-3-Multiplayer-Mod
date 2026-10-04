//! The mods a Transport Fever 3 save lists, read from the file without the
//! game (build 40408).
//!
//! A save is one zstd frame. Near its start (after the `tf**` magic and a
//! few settings) is the list of the game's mods as `GameSaveCommandData`
//! writes them (`api/tealdef/api/cmd.d.tl`, `modDescs : {Mod.GameModDesc}`):
//! a little-endian `u32` count, then per mod five `u32`-length strings and
//! an `i32`:
//!
//! ```text
//! modId.name   "tpf3mp_1"
//! modSource    "StagingArea"            ("DLC", ...)
//! modhubModId  "StagingArea,tpf3mp_1"   (the source, a comma, an id;
//!                                        a mod.io mod's is its mod.io
//!                                        number, "6414521")
//! name         "TPF3-MP"
//! url          "https://github.com/..." (often empty)
//! severityRemove  0, 1 or 2
//! ```
//!
//! SEEN in saves of build 40408 with the two DLCs, TPF3-MP, local mods and
//! mod.io ones. The fields before the list vary, so the list is found by
//! trying each offset in the first [`HEAD_BYTES`] and taking the first where
//! a whole list parses and every entry holds together (a mod id, a hub id
//! that is the source and a comma or a number, a severity of 0 to 2). The
//! list may run on past [`HEAD_BYTES`], up to [`LIST_BYTES`].
//!
//! Where the real list does not hold together, its tail does: an entry's
//! severity of 1 reads as a count of one, and the entries behind it as a
//! list (2026-10-03: a save listing a mod.io mod, whose hub id was not yet
//! known, read as the Pre-Order Pack alone, so the launcher refused it for
//! lacking TPF3-MP). So a list found right behind something shaped like an
//! entry (an id and four more strings) is taken for such a tail, and
//! the save is refused, never guessed at.

use std::{
    collections::{HashMap, HashSet},
    fs::File,
    io::Read,
    path::Path,
};

/// How far into the decompressed save the list may start.
pub const HEAD_BYTES: usize = 64 * 1024;
/// How much of the decompressed save is read: the list may run on past
/// [`HEAD_BYTES`], a few hundred mods with links being more than that.
pub const LIST_BYTES: usize = 4 << 20;
/// Most mods a list may hold.
const MAX_MODS: u32 = 4096;
/// Longest string of an entry.
const MAX_STRING: u32 = 4096;

/// One mod a save lists.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SaveMod {
    /// The mod's id, as `app.loadGame`'s `info.mods` names it.
    pub id: String,
    /// Where the game had it from: `StagingArea`, `DLC`, `mod.io`, ...
    pub source: String,
    /// Its hub id: `<source>,<id>`, or a mod.io mod's mod.io number.
    pub hub: String,
    /// Its name for players.
    pub name: String,
}

impl SaveMod {
    /// The mod's mod.io number, for a mod the save had from Mod Hub.
    pub fn modio_id(&self) -> Option<u64> {
        (self.source == "mod.io")
            .then(|| self.hub.parse().ok())
            .flatten()
    }
}

/// The mods the save at `path` lists, in its order.
pub fn mods(path: &Path) -> Result<Vec<SaveMod>, String> {
    let file = File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut decoder =
        zstd::stream::read::Decoder::new(file).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut head = Vec::with_capacity(HEAD_BYTES);
    let mut buf = [0u8; 8192];
    while head.len() < LIST_BYTES {
        match decoder.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => head.extend_from_slice(&buf[..n.min(LIST_BYTES - head.len())]),
            Err(e) => {
                return Err(format!(
                    "{}: not a save the game wrote: {e}",
                    path.display()
                ));
            }
        }
    }
    mods_in(&head).ok_or_else(|| format!("{}: no list of mods found in the save", path.display()))
}

/// The mods listed in the start of a decompressed save, if a list is found
/// starting in its first [`HEAD_BYTES`].
pub fn mods_in(head: &[u8]) -> Option<Vec<SaveMod>> {
    if !head.starts_with(b"tf**") {
        return None;
    }
    let (at, list) = (4..head.len().min(HEAD_BYTES).saturating_sub(4))
        .find_map(|at| list_at(head, at).map(|list| (at, list)))?;
    (!follows_an_entry(head, at)).then_some(list)
}

/// Whether the strings of an entry, an id and four more, end at `end`,
/// with room for a number before them: what is at `end` is then the
/// entry's severity, not a list's count. Strings are matched by their
/// lengths alone, backwards from `end`, and the id only as text that is
/// not empty: an entry that does not hold together still counts. Each
/// place a string can start is looked at once per string, so a save made
/// to match many lengths costs no more than one that matches none.
fn follows_an_entry(bytes: &[u8], end: usize) -> bool {
    // Where a string ending at a place can start, by that place.
    let from = end.saturating_sub(5 * (MAX_STRING as usize + 4));
    let mut starts: HashMap<usize, Vec<usize>> = HashMap::new();
    for start in from..end {
        if let Some(len) = u32_at(bytes, start).filter(|&len| len <= MAX_STRING) {
            let string_end = start + 4 + len as usize;
            if string_end <= end {
                starts.entry(string_end).or_default().push(start);
            }
        }
    }
    let before = |ends: &HashSet<usize>| -> HashSet<usize> {
        ends.iter()
            .filter_map(|string_end| starts.get(string_end))
            .flatten()
            .copied()
            .collect()
    };
    // url, name, hub and source, then the id.
    let mut ends = HashSet::from([end]);
    for _ in 0..4 {
        ends = before(&ends);
    }
    before(&ends).into_iter().any(|start| {
        let mut at = start;
        start >= 4 && string_at(bytes, &mut at).is_some_and(|id| !id.is_empty())
    })
}

fn u32_at(bytes: &[u8], at: usize) -> Option<u32> {
    let b = bytes.get(at..at + 4)?;
    Some(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
}

fn string_at(bytes: &[u8], at: &mut usize) -> Option<String> {
    let len = u32_at(bytes, *at)?;
    if len > MAX_STRING {
        return None;
    }
    let start = *at + 4;
    let end = start + len as usize;
    let text = std::str::from_utf8(bytes.get(start..end)?).ok()?;
    if text.chars().any(char::is_control) {
        return None;
    }
    *at = end;
    Some(text.to_owned())
}

fn is_mod_id(id: &str) -> bool {
    !id.is_empty()
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'))
}

/// The entry at `pos` and where the next one starts, if it holds together.
fn entry_at(bytes: &[u8], mut pos: usize) -> Option<(SaveMod, usize)> {
    let id = string_at(bytes, &mut pos)?;
    let source = string_at(bytes, &mut pos)?;
    let hub = string_at(bytes, &mut pos)?;
    let name = string_at(bytes, &mut pos)?;
    let _url = string_at(bytes, &mut pos)?;
    let severity = u32_at(bytes, pos)?;
    pos += 4;
    let hub_holds = hub.starts_with(&format!("{source},"))
        || (!hub.is_empty() && hub.bytes().all(|b| b.is_ascii_digit()));
    if !is_mod_id(&id) || source.is_empty() || !hub_holds || severity > 2 {
        return None;
    }
    Some((
        SaveMod {
            id,
            source,
            hub,
            name,
        },
        pos,
    ))
}

fn list_at(bytes: &[u8], at: usize) -> Option<Vec<SaveMod>> {
    let count = u32_at(bytes, at)?;
    if count == 0 || count > MAX_MODS {
        return None;
    }
    let mut pos = at + 4;
    let mut out = Vec::new();
    for _ in 0..count {
        let (entry, next) = entry_at(bytes, pos)?;
        out.push(entry);
        pos = next;
    }
    Some(out)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    fn string(out: &mut Vec<u8>, text: &str) {
        out.extend_from_slice(&u32::try_from(text.len()).unwrap().to_le_bytes());
        out.extend_from_slice(text.as_bytes());
    }

    /// The start of a save as build 40408 writes one: settings, then the
    /// mods, then the rest.
    pub(crate) fn head(mods: &[(&str, &str, &str)]) -> Vec<u8> {
        let mut out = b"tf**\x5c\x02\x00\x00".to_vec();
        // A setting that looks like nothing: "company" = 4.
        string(&mut out, "company");
        out.extend_from_slice(&[4, 0, 0, 0, 1, 1, 0, 0, 0, 3, 0, 0, 0]);
        out.extend_from_slice(&u32::try_from(mods.len()).unwrap().to_le_bytes());
        for (id, source, name) in mods {
            string(&mut out, id);
            string(&mut out, source);
            string(&mut out, &format!("{source},{id}"));
            string(&mut out, name);
            string(&mut out, "");
            out.extend_from_slice(&0u32.to_le_bytes());
        }
        out.extend_from_slice(&[1, 0x80, 2, 0, 0, 0x68, 1, 0, 0]);
        out
    }

    #[test]
    fn a_saves_mods_are_read_in_order() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("room.sav");
        let bytes = head(&[
            ("urbangames_deluxe_upgrade_pack", "DLC", "Deluxe Upgrade"),
            ("tpf3mp_1", "StagingArea", "TPF3-MP"),
            ("celmi_timetables", "mod.io", "Timetables"),
        ]);
        std::fs::write(&file, zstd::encode_all(&bytes[..], 3).unwrap()).unwrap();
        let mods = mods(&file).unwrap();
        let ids: Vec<&str> = mods.iter().map(|m| m.id.as_str()).collect();
        assert_eq!(
            ids,
            [
                "urbangames_deluxe_upgrade_pack",
                "tpf3mp_1",
                "celmi_timetables"
            ]
        );
        assert_eq!(mods[1].source, "StagingArea");
        assert_eq!(mods[2].name, "Timetables");
    }

    /// A save's start with its entries written field by field: id, source,
    /// hub id, name, url, severity.
    fn head_of(entries: &[(&str, &str, &str, &str, &str, u32)]) -> Vec<u8> {
        let mut out = b"tf**\x5c\x02\x00\x00".to_vec();
        string(&mut out, "company");
        out.extend_from_slice(&[4, 0, 0, 0, 1, 1, 0, 0, 0, 3, 0, 0, 0]);
        out.extend_from_slice(&u32::try_from(entries.len()).unwrap().to_le_bytes());
        for (id, source, hub, name, url, severity) in entries {
            for text in [id, source, hub, name, url] {
                string(&mut out, text);
            }
            out.extend_from_slice(&severity.to_le_bytes());
        }
        out.extend_from_slice(&[1, 0x80, 2, 0, 0, 0x68, 1, 0, 0]);
        out
    }

    /// The mods of a player's save of 2026-10-03, as it wrote them: a mod.io
    /// mod's hub id is its mod.io number. The list once failed on it, and
    /// the reader took the Pre-Order Pack's entry, behind the Deluxe
    /// Upgrade's severity of 1, for a list of one.
    const LIVE: [(&str, &str, &str, &str, &str, u32); 5] = [
        (
            "urbangames_deluxe_upgrade_pack",
            "DLC",
            "DLC,urbangames_deluxe_upgrade_pack",
            "Deluxe Upgrade",
            "",
            1,
        ),
        (
            "urbangames_preorder_pack",
            "DLC",
            "DLC,urbangames_preorder_pack",
            "Pre-Order Pack",
            "",
            1,
        ),
        (
            "auto_signals_1",
            "StagingArea",
            "StagingArea,auto_signals",
            "Auto Signals",
            "",
            0,
        ),
        (
            "tpf3mp_1",
            "StagingArea",
            "StagingArea,tpf3mp_1",
            "TPF3-MP",
            "https://github.com/Juliansgith/Transport-Fever-3-Multiplayer-Mod",
            0,
        ),
        (
            "revyn112_towns_de",
            "mod.io",
            "6414521",
            "Deutsche Städte und Gemeinden",
            "https://mod.io/g/transportfever3/m/german-cities-and-municipalities5",
            2,
        ),
    ];

    #[test]
    fn a_mod_io_mod_is_read_with_the_rest() {
        let mods = mods_in(&head_of(&LIVE)).unwrap();
        let ids: Vec<&str> = mods.iter().map(|m| m.id.as_str()).collect();
        assert_eq!(
            ids,
            [
                "urbangames_deluxe_upgrade_pack",
                "urbangames_preorder_pack",
                "auto_signals_1",
                "tpf3mp_1",
                "revyn112_towns_de"
            ]
        );
        assert_eq!(mods[4].source, "mod.io");
        assert_eq!(mods[4].name, "Deutsche Städte und Gemeinden");
        assert_eq!(mods[4].modio_id(), Some(6414521));
        assert_eq!(mods[3].hub, "StagingArea,tpf3mp_1");
        assert_eq!(mods[3].modio_id(), None);
    }

    #[test]
    fn a_list_that_breaks_off_is_refused_not_cut_short() {
        // An entry of a shape not seen yet, last, first or anywhere: the
        // list does not hold together, and no tail of it is taken for the
        // list (behind the Deluxe Upgrade's severity of 1, a list of one).
        for at in 0..LIVE.len() {
            let mut entries = LIVE;
            entries[at].2 = "mod.io:6414521";
            assert_eq!(mods_in(&head_of(&entries)), None, "hub of entry {at}");
            let mut entries = LIVE;
            entries[at].5 = 3;
            assert_eq!(mods_in(&head_of(&entries)), None, "severity of entry {at}");
        }
        // TPF3-MP behind a broken first entry is not read as the only mod,
        // however the entry is broken.
        for (field, broken) in [
            (0, "urbangames deluxe"),
            (1, "D\nLC"),
            (2, "DLC:urbangames_deluxe_upgrade_pack"),
            (2, "DLC\n"),
            (3, "Deluxe\tUpgrade"),
            (4, "\u{7}"),
        ] {
            let mut entries = LIVE;
            entries.swap(1, 3);
            match field {
                0 => entries[0].0 = broken,
                1 => entries[0].1 = broken,
                2 => entries[0].2 = broken,
                3 => entries[0].3 = broken,
                _ => entries[0].4 = broken,
            }
            assert_eq!(mods_in(&head_of(&entries)), None, "{broken:?}");
        }
    }

    #[test]
    fn a_list_may_run_on_past_the_head() {
        let url = "https://mod.io/".to_owned() + &"x".repeat(1000);
        let ids: Vec<String> = (0..100).map(|n| format!("mod_{n}")).collect();
        let mut entries: Vec<_> = ids
            .iter()
            .map(|id| (id.as_str(), "mod.io", "123", "A mod", url.as_str(), 0))
            .collect();
        entries.push(LIVE[3]);
        let bytes = head_of(&entries);
        assert!(bytes.len() > HEAD_BYTES);
        let mods = mods_in(&bytes).unwrap();
        assert_eq!(mods.len(), 101);
        assert_eq!(mods[100].id, "tpf3mp_1");
        // Cut short, as a save larger than what is read: refused.
        assert_eq!(mods_in(&bytes[..HEAD_BYTES]), None);
    }

    #[test]
    fn what_is_not_a_save_is_refused() {
        assert_eq!(mods_in(b"not a save at all"), None);
        assert_eq!(mods_in(b"tf**\x01\x00\x00\x00"), None);
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("x.sav");
        std::fs::write(&file, b"plain bytes").unwrap();
        assert!(mods(&file).is_err());
    }
}
