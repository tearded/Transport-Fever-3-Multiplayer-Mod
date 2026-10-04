//! The edge watch (logging only; it changes nothing), off unless [`ENV`]
//! names entities (docs/HOOKS.md, "The edge watch").
//!
//! Round of 2026-10-02 on `twomptest`: between steps 12750 and 12800 an
//! existing town street (entity 325514, a `town_old_small` dead end) had its
//! free end moved, to one of two places, which one varying from run to run
//! and game to game, with every town size update alike. The watch finds the
//! update that moves it and what ran then. Within the steps [`STEPS_ENV`]
//! names (every step when unset):
//!
//! - **Each update**, the mod's game script reads every watched entity in
//!   its `postUpdate` (`tpf3mp/lanes.lua`, `lanes.watch`: an edge's nodes,
//!   ends, tangents and its nodes' positions at full precision; a node's
//!   position) and hands the text to the hook (`tpf3mp_native.edgewatch`
//!   and `edgewatched`), which logs it when it differs from the last:
//!
//!   ```text
//!   edge watch: step <s> update <n> entity <e> first|changed <text>
//!   ```
//!
//! - **Every command applied**, from any path: two splices in
//!   `CommandApply::One` (`0x9e1c10`, our name; "Simulation Thread: Apply
//!   Command", `apply_command.cpp`; `rcx` the `GameState`, `rdx` the
//!   `Command`), at its entry (the return address names the path) and at
//!   its epilogue, one line a command:
//!
//!   ```text
//!   apply: step <s>|after <s> update <n> from +<rva> (<path>) kind <k> result <r> entities <n> [<e>,...] -> <n> [<e>,...] entity-ids <before>-><after>[ watched]
//!   ```
//!
//!   The paths: `queue`, the commands `CGame::RunGameSimLoop` drains from
//!   `CommandList` between updates (`0x11eb96`; the GUI's, the street
//!   builder's among them, land here at whatever step the drain falls on);
//!   `script`, a script's `sendCommand` applied at once (`0x1204bf`, the
//!   send lambda at `0x120410`; in a game script's `update` the same lambda
//!   buffers the command per script instead, and the buffer is applied
//!   through `GameState`'s command function, `0x268ed0`, a tail jump, whose
//!   lines name its caller's site); `direct` (`0x120334`, CGame's other
//!   send lambda). `kind` is the payload's variant index (`payload+0x9b8`,
//!   the byte the dispatcher switches on), `result` the byte `One` leaves
//!   at `Command+0x30`, `entities` the command's entity list
//!   (`Command+8`, 16-byte entries, the id in the first four bytes) before
//!   and after, `entity-ids` the engine's entity table's length, `watched`
//!   that a watched entity is listed. `after <s>` is a command applied
//!   between updates, after step `<s>`.
//!
//! Before splicing, the epilogue site must lie at its offset in `One`, the
//! kind's read must be where and what the dispatcher's call expects, and
//! each site's bytes must be the expected ones; otherwise nothing is
//! spliced and the log says why. A panic switches it off.

#![allow(unsafe_code)]
#![cfg_attr(not(all(windows, target_arch = "x86_64")), allow(dead_code))]

use std::cell::RefCell;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};

use tpf3mp_hookcore::detour::{SavedRegs, Splice, SpliceHook};
use tpf3mp_hookcore::profile::ResolvedProfile;

use crate::image::Readable as Probe;
use crate::log;

/// The entities to watch, `325514,220468` (edges or nodes); unset, the
/// watch is off.
pub const ENV: &str = "TPF3MP_HOOK_EDGE_WATCH";
/// `from-to`, the steps watched (both included); unset, every step.
pub const STEPS_ENV: &str = "TPF3MP_HOOK_EDGE_WATCH_STEPS";
/// Most entities watched.
pub const MAX_ENTITIES: usize = 32;
/// Most entity ids a command's line lists, each way.
pub const MAX_LISTED: usize = 8;

pub use crate::build_data::native::edgewatch::APPLY;
pub use crate::build_data::native::edgewatch::KIND_READ;
pub use crate::build_data::native::edgewatch::KIND_READ_AT;
pub use crate::build_data::native::edgewatch::RETURN_SITE;
pub use crate::build_data::native::edgewatch::RETURN_SITE_AT;

pub use crate::build_data::native::edgewatch::ENTRY_EXPECTED;
pub use crate::build_data::native::edgewatch::ENTRY_STEAL;
pub use crate::build_data::native::edgewatch::FRAME;
pub use crate::build_data::native::edgewatch::RETURN_EXPECTED;
pub use crate::build_data::native::edgewatch::RETURN_STEAL;

use crate::build_data::native::edgewatch::ENGINE;
use crate::build_data::native::edgewatch::ENTITIES;
use crate::build_data::native::edgewatch::ENTITY_ENTRY;
use crate::build_data::native::edgewatch::GAME_TIME;
use crate::build_data::native::edgewatch::PAYLOAD_KIND;
use crate::build_data::native::edgewatch::RESULT;

pub use crate::build_data::native::edgewatch::PATHS;

/// What the watch is set to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Watch {
    pub entities: Vec<u32>,
    pub steps: Option<(u64, u64)>,
}

impl Watch {
    /// From [`ENV`]'s and [`STEPS_ENV`]'s values: `Ok(None)` when [`ENV`]
    /// is unset, `Err(why)` for a value that does not read (the watch is
    /// then off).
    pub fn from_env(entities: Option<&str>, steps: Option<&str>) -> Result<Option<Self>, String> {
        let Some(list) = entities.map(str::trim).filter(|v| !v.is_empty()) else {
            return Ok(None);
        };
        let mut parsed = Vec::new();
        for item in list.split(',') {
            let Ok(entity) = item.trim().parse::<u32>() else {
                return Err(format!(
                    "{ENV}={list} is not a list of entities such as 325514,220468; the edge watch is off"
                ));
            };
            if !parsed.contains(&entity) {
                parsed.push(entity);
            }
        }
        if parsed.len() > MAX_ENTITIES {
            return Err(format!(
                "{ENV} names more than {MAX_ENTITIES} entities; the edge watch is off"
            ));
        }
        let steps = match steps.map(str::trim).filter(|v| !v.is_empty()) {
            None => None,
            Some(value) => Some(crate::lanedump::parse_step_range(value).ok_or_else(|| {
                format!(
                    "{STEPS_ENV}={value} is not a step range such as 12750-12800; the edge watch is off"
                )
            })?),
        };
        Ok(Some(Self {
            entities: parsed,
            steps,
        }))
    }

    /// Whether `step` is watched.
    pub fn covers(&self, step: u64) -> bool {
        self.steps
            .is_none_or(|(from, to)| (from..=to).contains(&step))
    }

    pub fn describe(&self) -> String {
        let steps = match self.steps {
            Some((from, to)) => format!("steps {from} to {to}"),
            None => "every step".to_owned(),
        };
        let entities: Vec<String> = self.entities.iter().map(u32::to_string).collect();
        format!("entities {} at {steps}", entities.join(","))
    }
}

/// What each watched entity read last.
#[derive(Debug, Default)]
pub struct Seen {
    last: Vec<(u32, String)>,
}

impl Seen {
    /// The line for `entity` reading `text`, or `None` when it reads as it
    /// did last.
    pub fn line(
        &mut self,
        step: Option<u64>,
        update: Option<u32>,
        entity: u32,
        text: &str,
    ) -> Option<String> {
        let how = match self.last.iter_mut().find(|(e, _)| *e == entity) {
            Some((_, last)) if last == text => return None,
            Some((_, last)) => {
                *last = text.to_owned();
                "changed"
            }
            None => {
                self.last.push((entity, text.to_owned()));
                "first"
            }
        };
        Some(format!(
            "edge watch: step {} update {} entity {entity} {how} {text}",
            opt(step, "-"),
            opt(update, "?"),
        ))
    }
}

fn opt<T: std::fmt::Display>(value: Option<T>, none: &str) -> String {
    value.map_or_else(|| none.to_owned(), |v| v.to_string())
}

/// When a command was applied: in a room's update, or between updates
/// after a step.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum When {
    Step(u64),
    After(u64),
    Unknown,
}

impl When {
    pub fn step(self) -> Option<u64> {
        match self {
            Self::Step(s) | Self::After(s) => Some(s),
            Self::Unknown => None,
        }
    }
}

impl std::fmt::Display for When {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Step(s) => write!(f, "step {s}"),
            Self::After(s) => write!(f, "after {s}"),
            Self::Unknown => write!(f, "step -"),
        }
    }
}

/// What the entry splice read, for the epilogue's.
#[derive(Debug, Clone, PartialEq)]
pub struct Entry {
    /// The entry's `rsp`, to pair it with its epilogue.
    pub rsp: u64,
    pub when: When,
    pub update: Option<u32>,
    /// The call site's offset in the image.
    pub caller: Option<u64>,
    pub kind: Option<i8>,
    pub entities: Option<(usize, Vec<i32>)>,
    pub ids: Option<u64>,
}

/// A command's line.
pub fn apply_line(
    entry: &Entry,
    result: Option<u8>,
    after: Option<&(usize, Vec<i32>)>,
    ids_after: Option<u64>,
    watched: &[u32],
) -> String {
    let path = entry.caller.map_or("?", |site| {
        PATHS
            .iter()
            .find(|(at, _)| *at == site)
            .map_or("other", |(_, name)| name)
    });
    let list = |list: Option<&(usize, Vec<i32>)>| match list {
        None => "?".to_owned(),
        Some((n, ids)) => {
            let shown: Vec<String> = ids.iter().map(i32::to_string).collect();
            let more = if *n > ids.len() { ",..." } else { "" };
            format!("{n} [{}{more}]", shown.join(","))
        }
    };
    let names_watched = [entry.entities.as_ref(), after]
        .into_iter()
        .flatten()
        .flat_map(|(_, ids)| ids.iter())
        .any(|id| u32::try_from(*id).is_ok_and(|id| watched.contains(&id)));
    format!(
        "apply: {} update {} from {} ({path}) kind {} result {} entities {} -> {} entity-ids {}->{}{}",
        entry.when,
        opt(entry.update, "?"),
        entry
            .caller
            .map_or_else(|| "?".to_owned(), |rva| format!("+{rva:#x}")),
        opt(entry.kind, "?"),
        opt(result, "?"),
        list(entry.entities.as_ref()),
        list(after),
        opt(entry.ids, "?"),
        opt(ids_after, "?"),
        if names_watched { " watched" } else { "" },
    )
}

/// The epilogue site's layout in `One`: at its offset, and the kind read
/// (`kind_bytes`) where and what the dispatcher's call needs.
pub fn check_layout(
    apply: u64,
    return_site: u64,
    kind_bytes: Option<[u8; 8]>,
) -> Result<(), String> {
    if return_site != apply.wrapping_add(RETURN_SITE_AT) {
        return Err(format!(
            "{RETURN_SITE} at {return_site:#x} is not {APPLY}+{RETURN_SITE_AT:#x}"
        ));
    }
    if kind_bytes != Some(KIND_READ) {
        return Err(format!(
            "{APPLY}+{KIND_READ_AT:#x} does not read the kind at payload+{PAYLOAD_KIND:#x}"
        ));
    }
    Ok(())
}

static WATCH: OnceLock<Watch> = OnceLock::new();
static SEEN: Mutex<Seen> = Mutex::new(Seen { last: Vec::new() });
static ON: AtomicBool = AtomicBool::new(false);
static BROKEN: AtomicBool = AtomicBool::new(false);
static BASE: AtomicU64 = AtomicU64::new(0);

/// The step of the update running now, or the last step run.
fn when_now() -> When {
    match crate::seeds::current_step() {
        Some(step) => When::Step(step),
        None => crate::install::last_step_run().map_or(When::Unknown, When::After),
    }
}

/// For the game script's `update` (`tpf3mp_native.edgewatch`): the
/// entities to read in this update, when one is watched.
pub fn due_now() -> Option<Vec<u32>> {
    let watch = WATCH.get()?;
    let step = crate::seeds::current_step()?;
    watch.covers(step).then(|| watch.entities.clone())
}

/// For the game script's `postUpdate` (`tpf3mp_native.edgewatched`): what
/// it read of `entity`, logged when it changed.
pub fn watched_now(entity: u32, text: &str) {
    let Some(watch) = WATCH.get() else {
        return;
    };
    if !watch.entities.contains(&entity) {
        return;
    }
    let step = crate::seeds::current_step();
    let update = crate::install::update_count_now();
    let line = SEEN
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .line(step, update, entity, text);
    if let Some(line) = line {
        log::line(&line);
    }
}

/// The command's entity list: its length and the first [`MAX_LISTED`] ids.
fn entity_list(probe: &mut Probe, cmd: u64) -> Option<(usize, Vec<i32>)> {
    let begin = probe.read::<u64>(cmd.checked_add(ENTITIES)?)?;
    let end = probe.read::<u64>(cmd.checked_add(ENTITIES + 8)?)?;
    if end < begin || (end - begin) % ENTITY_ENTRY != 0 || end - begin > 1 << 24 {
        return None;
    }
    let n = usize::try_from((end - begin) / ENTITY_ENTRY).ok()?;
    let ids = (0..n.min(MAX_LISTED) as u64)
        .map(|i| probe.read::<i32>(begin + i * ENTITY_ENTRY))
        .collect::<Option<Vec<_>>>()?;
    Some((n, ids))
}

fn update_of(probe: &mut Probe, game_state: u64) -> Option<u32> {
    probe
        .read::<u64>(game_state.wrapping_add(GAME_TIME))
        .and_then(|gt| crate::ticks::read_counters(gt as usize).ok())
        .map(|c| c.update_count)
}

thread_local! {
    /// The entries of the `One` calls running on this thread, innermost
    /// last (a command's apply can apply another).
    static PENDING: RefCell<Vec<Entry>> = const { RefCell::new(Vec::new()) };
}

/// Most nested calls kept a thread.
const MAX_NESTED: usize = 16;

fn guarded(body: impl FnOnce()) {
    if BROKEN.load(Ordering::Acquire) || !ON.load(Ordering::Acquire) {
        return;
    }
    if std::panic::catch_unwind(std::panic::AssertUnwindSafe(body)).is_err() {
        BROKEN.store(true, Ordering::Release);
        log::line("edge watch: panicked on the game's thread; switched off for this game");
    }
}

/// `One`'s entry: `rcx` the `GameState`, `rdx` the command, the return
/// address at `[rsp]`.
unsafe extern "system" fn entry_hook(regs: *mut SavedRegs) {
    guarded(|| {
        let rsp = SavedRegs::rsp(regs);
        let when = when_now();
        let Some(watch) = WATCH.get() else {
            return;
        };
        if !when.step().is_some_and(|s| watch.covers(s)) {
            return;
        }
        // SAFETY: the stub's block, held until the hook returns.
        let regs = unsafe { &*regs };
        let (game_state, cmd) = (regs.rcx, regs.rdx);
        let mut probe = Probe::new();
        let base = BASE.load(Ordering::Relaxed);
        let caller = probe
            .read::<u64>(rsp)
            .and_then(|ret| ret.checked_sub(base))
            .and_then(|rva| rva.checked_sub(5));
        let kind = probe
            .read::<u64>(cmd)
            .and_then(|payload| probe.read::<i8>(payload.wrapping_add(PAYLOAD_KIND)));
        let engine = probe.read::<u64>(game_state.wrapping_add(ENGINE));
        let entry = Entry {
            rsp,
            when,
            update: update_of(&mut probe, game_state),
            caller,
            kind,
            entities: entity_list(&mut probe, cmd),
            ids: engine.and_then(|e| crate::towntrace::entity_ids(&mut probe, e)),
        };
        PENDING.with(|p| {
            let mut pending = p.borrow_mut();
            if pending.len() < MAX_NESTED {
                pending.push(entry);
            }
        });
    });
}

/// `One`'s epilogue: `r14` the command, `r15` the `GameState`.
unsafe extern "system" fn return_hook(regs: *mut SavedRegs) {
    guarded(|| {
        let entry_rsp = SavedRegs::rsp(regs).wrapping_add(FRAME);
        // This call's entry, and none an unwound call left behind.
        let Some(entry) = PENDING.with(|p| {
            let mut pending = p.borrow_mut();
            while let Some(top) = pending.pop() {
                if top.rsp == entry_rsp {
                    return Some(top);
                }
                if top.rsp > entry_rsp {
                    pending.push(top);
                    return None;
                }
            }
            None
        }) else {
            return;
        };
        // SAFETY: the stub's block, held until the hook returns.
        let regs = unsafe { &*regs };
        let mut probe = Probe::new();
        let (cmd, game_state) = (regs.r14, regs.r15);
        let result = probe.read::<u8>(cmd.wrapping_add(RESULT));
        let after = entity_list(&mut probe, cmd);
        let ids_after = probe
            .read::<u64>(game_state.wrapping_add(ENGINE))
            .and_then(|e| crate::towntrace::entity_ids(&mut probe, e));
        let watched = WATCH.get().map_or(&[][..], |w| w.entities.as_slice());
        log::line(&apply_line(
            &entry,
            result,
            after.as_ref(),
            ids_after,
            watched,
        ));
    });
}

fn splice(at: u64, expected: &[u8], steal: usize, hook: SpliceHook) -> Result<Splice, String> {
    // SAFETY: a site the profile resolved and the layout check placed in
    // its function, installed before any world exists; nothing branches
    // into the stolen bytes past the first (tpfre, noted in the profile);
    // the hooks only read and never unwind (`guarded`).
    unsafe { Splice::install(at as usize as *mut u8, expected, steal, hook) }
        .map_err(|e| format!("{at:#x}: {e}"))
}

/// Starts the watch when [`ENV`] asks for it; `base` the game's image.
/// Returns the lines for hook.log.
pub fn install(resolved: &ResolvedProfile, base: u64) -> Vec<String> {
    let watch = Watch::from_env(
        std::env::var(ENV).ok().as_deref(),
        std::env::var(STEPS_ENV).ok().as_deref(),
    );
    install_with(resolved, base, watch)
}

pub fn install_with(
    resolved: &ResolvedProfile,
    base: u64,
    watch: Result<Option<Watch>, String>,
) -> Vec<String> {
    let watch = match watch {
        Ok(Some(watch)) => watch,
        Ok(None) => {
            return vec![format!(
                "edge watch: off ({ENV} names no entities); edges and applied commands are not logged"
            )];
        }
        Err(why) => return vec![format!("edge watch: {why}")],
    };
    BASE.store(base, Ordering::Relaxed);
    let mut lines = vec![format!(
        "edge watch: {}: one `edge watch:` line when one reads otherwise than in the update before",
        watch.describe()
    )];
    let _ = WATCH.set(watch);
    match (resolved.get(APPLY), resolved.get(RETURN_SITE)) {
        (Some(apply), Some(ret)) => {
            let kind_at = apply.address.wrapping_add(KIND_READ_AT);
            let kind_bytes = crate::image::readable(kind_at as usize, KIND_READ.len()).then(|| {
                // SAFETY: eight readable bytes of the game's code.
                unsafe { std::ptr::read_unaligned(kind_at as usize as *const [u8; 8]) }
            });
            let installed = check_layout(apply.address, ret.address, kind_bytes).and_then(|()| {
                let first = splice(apply.address, &ENTRY_EXPECTED, ENTRY_STEAL, entry_hook)?;
                match splice(ret.address, &RETURN_EXPECTED, RETURN_STEAL, return_hook) {
                    Ok(second) => Ok((first, second)),
                    Err(why) => {
                        // SAFETY: as installed, no world runs yet.
                        let _ = unsafe { first.detach() };
                        Err(why)
                    }
                }
            });
            match installed {
                Ok(splices) => {
                    let _kept = std::mem::ManuallyDrop::new(splices);
                    lines.push(format!(
                        "edge watch: {APPLY} at {:#x} spliced at its entry and +{RETURN_SITE_AT:#x}: one `apply:` line a command in the watched steps (logging only)",
                        apply.address
                    ));
                }
                Err(why) => lines.push(format!(
                    "edge watch: {APPLY} not traced, {why}; edges are still watched"
                )),
            }
        }
        _ => lines.push(format!(
            "edge watch: {APPLY} not traced, the profile lacks {APPLY} or {RETURN_SITE}; edges are still watched"
        )),
    }
    ON.store(true, Ordering::Release);
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_watch_reads_entities_and_steps_or_is_off() {
        assert_eq!(Watch::from_env(None, Some("1-2")), Ok(None));
        assert_eq!(Watch::from_env(Some(" "), None), Ok(None));
        let watch = Watch::from_env(Some("325514, 220468,325514"), Some("12750-12800"))
            .unwrap()
            .unwrap();
        assert_eq!(watch.entities, [325_514, 220_468]);
        assert!(watch.covers(12_750) && watch.covers(12_800));
        assert!(!watch.covers(12_749) && !watch.covers(12_801));
        assert_eq!(
            watch.describe(),
            "entities 325514,220468 at steps 12750 to 12800"
        );
        let always = Watch::from_env(Some("7"), None).unwrap().unwrap();
        assert!(always.covers(0) && always.covers(u64::MAX));
        assert!(
            Watch::from_env(Some("edge"), None)
                .unwrap_err()
                .contains(ENV)
        );
        assert!(
            Watch::from_env(Some("1"), Some("9-1"))
                .unwrap_err()
                .contains(STEPS_ENV)
        );
        let many: Vec<String> = (0..=MAX_ENTITIES).map(|i| i.to_string()).collect();
        assert!(Watch::from_env(Some(&many.join(",")), None).is_err());
    }

    #[test]
    fn an_entity_is_logged_first_and_then_only_when_it_changed() {
        let mut seen = Seen::default();
        assert_eq!(
            seen.line(Some(12_751), Some(113_533), 325_514, "edge p1=1,2,3"),
            Some("edge watch: step 12751 update 113533 entity 325514 first edge p1=1,2,3".into())
        );
        assert_eq!(
            seen.line(Some(12_752), None, 325_514, "edge p1=1,2,3"),
            None
        );
        assert_eq!(
            seen.line(None, None, 325_514, "edge p1=1,2,4"),
            Some("edge watch: step - update ? entity 325514 changed edge p1=1,2,4".into())
        );
        assert!(
            seen.line(None, None, 7, "edge p1=1,2,4")
                .unwrap()
                .contains(" first ")
        );
    }

    #[test]
    fn a_commands_line_names_its_path_kind_and_entities() {
        let entry = Entry {
            rsp: 0x1000,
            when: When::After(12_760),
            update: Some(113_600),
            caller: Some(0x11eb96),
            kind: Some(23),
            entities: Some((2, vec![325_514, 220_468])),
            ids: Some(400_000),
        };
        assert_eq!(
            apply_line(
                &entry,
                Some(1),
                Some(&(1, vec![325_514])),
                Some(400_002),
                &[325_514]
            ),
            "apply: after 12760 update 113600 from +0x11eb96 (queue) kind 23 result 1 entities 2 [325514,220468] -> 1 [325514] entity-ids 400000->400002 watched"
        );
        let other = Entry {
            when: When::Step(12_761),
            caller: Some(0x1234),
            entities: Some((MAX_LISTED + 3, vec![1; MAX_LISTED])),
            ..entry.clone()
        };
        let line = apply_line(&other, None, None, None, &[325_514]);
        assert!(
            line.starts_with("apply: step 12761 update 113600 from +0x1234 (other) "),
            "{line}"
        );
        assert!(
            line.contains("entities 11 [1,1,1,1,1,1,1,1,...] -> ? entity-ids 400000->?"),
            "{line}"
        );
        assert!(!line.ends_with("watched"), "{line}");
        let script = Entry {
            caller: Some(0x1204bf),
            when: When::Unknown,
            ..entry
        };
        assert!(
            apply_line(&script, None, None, None, &[])
                .starts_with("apply: step - update 113600 from +0x1204bf (script) ")
        );
    }

    #[test]
    fn the_layout_must_be_the_release_builds() {
        let apply = 0x0001_409e_1c10;
        let ret = apply + RETURN_SITE_AT;
        assert_eq!(check_layout(apply, ret, Some(KIND_READ)), Ok(()));
        assert!(check_layout(apply, ret + 1, Some(KIND_READ)).is_err());
        assert!(check_layout(apply, ret, None).is_err());
        assert!(check_layout(apply, ret, Some([0x90; 8])).is_err());
        // Five pushes and 0xb0: the epilogue's rsp below the entry's.
        assert_eq!(FRAME, 5 * 8 + 0xb0);
    }

    #[test]
    fn nothing_installs_when_off_or_without_the_targets() {
        let resolved = ResolvedProfile {
            name: "empty".into(),
            targets: Vec::new(),
            absent_optional: Vec::new(),
        };
        let lines = install_with(&resolved, 0, Ok(None));
        assert_eq!(lines.len(), 1);
        assert!(
            lines[0].contains("off") && lines[0].contains(ENV),
            "{lines:?}"
        );
        let lines = install_with(&resolved, 0, Err("bad".into()));
        assert_eq!(lines, ["edge watch: bad"]);
    }

    #[test]
    fn the_profile_states_the_sites_bytes() {
        let profile =
            tpf3mp_hookcore::profile::Profile::from_toml(crate::BUILT_IN_PROFILES[0].1).unwrap();
        let target = |name: &str| {
            profile
                .targets
                .iter()
                .find(|t| t.name == name)
                .unwrap_or_else(|| panic!("{name} in the profile"))
        };
        assert_eq!(
            target(APPLY).prologue,
            ENTRY_EXPECTED[..ENTRY_STEAL].to_vec()
        );
        assert_eq!(
            target(RETURN_SITE).prologue,
            RETURN_EXPECTED[..RETURN_STEAL].to_vec()
        );
        for name in [APPLY, RETURN_SITE] {
            assert!(!target(name).required, "{name} is optional");
        }
    }
}
