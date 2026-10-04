//! A read-only probe of where the engine keeps its player
//! (investigation/TF3_LOCAL_PLAYER_2026-10-01.md), off unless
//! `TPF3MP_PROBE_PLAYER=1`.
//!
//! `api.engine.util.getPlayer` answers from a `GameState` each Lua state's
//! setup hands it: the game scripts' from one of two buffers under
//! `CGame+0x1f0` (the getter [`SIM_TARGET`]), the GUI's from `CGame+0x1e0`
//! (the getter [`GUI_TARGET`], through `CMenuUI::m_game`). Whether the GUI's
//! is a state of its own, or one of the simulation's two buffers, decides
//! whether the GUI and the game's tools can act as the player's company
//! while the simulation keeps the save's player. This probe says so in
//! `hook.log`, from a real game:
//!
//! - each pointer, and whether the GUI's equals either buffer;
//! - where in each state the save's player entity (which the mod's game
//!   script notes, `tpf3mp.player`) stands, as a dword or a qword, within
//!   the first [`SCAN_BYTES`].
//!
//! It writes nothing: the two targets are only read for their field
//! offsets, every pointer is checked readable before it is read, and
//! nothing is ever written to the game. A missing target leaves it off.
//! It runs in `CMenuUI::DoStep`'s detour, on the main thread, after the
//! game's own frame: a few frames after the world changes, then once every
//! [`EVERY_MS`].

#![allow(unsafe_code)]
#![cfg_attr(not(all(windows, target_arch = "x86_64")), allow(dead_code))]

use std::sync::{
    Mutex, PoisonError,
    atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering},
};

use tpf3mp_hookcore::profile::ResolvedProfile;

/// The environment variable that turns the probe on (`1`).
pub const ENV: &str = "TPF3MP_PROBE_PLAYER";
/// The note that tells the GUI's Lua the probe is on (`1`).
pub const LUA_NOTE: &str = "tpf3mp.probe";
pub use crate::build_data::native::probe::GUI_TARGET;
pub use crate::build_data::native::probe::SIM_TARGET;
/// How far into each state the player is looked for.
pub const SCAN_BYTES: usize = 0x2000;
/// How often the probe looks, once the frames after a change are done.
pub const EVERY_MS: u64 = 3_000;
/// Frames looked at in a row after the game's pointer changes.
pub const FRAMES_AFTER_CHANGE: u32 = 5;
/// Places listed per state, at most.
const MAX_HITS: usize = 16;

/// The offsets the GUI's getter reads: `CMenuUI::m_game`, then the
/// `GameState` in the game.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GuiFields {
    pub game: usize,
    pub state: usize,
}

/// The offsets the game scripts' getter reads: the double buffer in the
/// game, the index word in it, and the first of the two pointers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SimFields {
    pub states: usize,
    pub index: usize,
    pub base: usize,
}

fn disp32(code: &[u8], at: usize) -> Option<usize> {
    let disp = i32::from_le_bytes(code.get(at..at + 4)?.try_into().ok()?);
    let offset = usize::try_from(disp).ok()?;
    (offset < 0x1_0000).then_some(offset)
}

/// The GUI getter's fields, if `code` is that getter.
pub fn gui_fields(code: &[u8]) -> Option<GuiFields> {
    let shape = code.get(..19)?;
    let fixed = [
        (0, 0x48),
        (1, 0x8B),
        (2, 0x41),
        (3, 0x08),
        (4, 0x48),
        (5, 0x8B),
        (6, 0x80),
    ]
    .iter()
    .chain([(11, 0x48), (12, 0x8B), (13, 0x80), (18, 0xC3)].iter())
    .all(|&(i, b)| shape[i] == b);
    if !fixed {
        return None;
    }
    let fields = GuiFields {
        game: disp32(code, 7)?,
        state: disp32(code, 14)?,
    };
    (fields.game.is_multiple_of(8) && fields.state.is_multiple_of(8)).then_some(fields)
}

/// The engine getter's fields, if `code` is that getter.
pub fn sim_fields(code: &[u8]) -> Option<SimFields> {
    let code = code.get(..37)?;
    let fixed: &[(usize, u8)] = &[
        (0, 0x80),
        (1, 0x79),
        (3, 0x00),
        (4, 0x48),
        (5, 0x8B),
        (6, 0x41),
        (7, 0x08),
        (8, 0x48),
        (9, 0x8B),
        (10, 0x80),
        (15, 0x74),
        (17, 0xB9),
        (18, 0x01),
        (22, 0x2B),
        (23, 0x88),
        (28, 0x48),
        (29, 0x63),
        (30, 0xD1),
        (31, 0x48),
        (32, 0x8B),
        (33, 0x44),
        (34, 0xD0),
        (36, 0xC3),
    ];
    if !fixed.iter().all(|&(i, b)| code[i] == b) {
        return None;
    }
    let fields = SimFields {
        states: disp32(code, 11)?,
        index: disp32(code, 24)?,
        base: usize::from(code[35]),
    };
    (fields.states.is_multiple_of(8) && fields.base.is_multiple_of(8)).then_some(fields)
}

/// Where `value` stands in `bytes`, at 4-byte steps: as a dword (`d`) or a
/// qword (`q`, when the dword after it is 0), at most [`MAX_HITS`].
pub fn scan(bytes: &[u8], value: u32) -> Vec<(usize, char)> {
    let mut hits = Vec::new();
    let mut at = 0;
    while at + 4 <= bytes.len() && hits.len() < MAX_HITS {
        let word = u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap_or_default());
        if word == value {
            let high = bytes
                .get(at + 4..at + 8)
                .map(|b| u32::from_le_bytes(b.try_into().unwrap_or_default()));
            hits.push((
                at,
                if high == Some(0) && at.is_multiple_of(8) {
                    'q'
                } else {
                    'd'
                },
            ));
        }
        at += 4;
    }
    hits
}

fn hits_text(hits: &[(usize, char)]) -> String {
    if hits.is_empty() {
        return "none".into();
    }
    hits.iter()
        .map(|(at, kind)| format!("+{at:#x}{kind}"))
        .collect::<Vec<_>>()
        .join(" ")
}

static ON: AtomicBool = AtomicBool::new(false);
static GUI_GAME: AtomicUsize = AtomicUsize::new(0);
static GUI_STATE: AtomicUsize = AtomicUsize::new(0);
static SIM_STATES: AtomicUsize = AtomicUsize::new(0);
static SIM_INDEX: AtomicUsize = AtomicUsize::new(0);
static SIM_BASE: AtomicUsize = AtomicUsize::new(0);

pub use crate::build_data::native::probe::CALLER_TARGET;
/// Return addresses counted apart; the rest go to one overflow row.
const SLOTS: usize = 64;

/// Where a call came from: inside the simulation's step on the main thread,
/// on another thread (the simulation's pool, during the step), or on the
/// main thread outside the step (the GUI and its tools).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    Step = 0,
    Pool = 1,
    Gui = 2,
}

/// One row: a return address and its counts by [`Side`].
struct Row {
    key: AtomicU64,
    counts: [AtomicU64; 3],
}

#[allow(clippy::declare_interior_mutable_const)]
const ROW: Row = Row {
    key: AtomicU64::new(0),
    counts: [const { AtomicU64::new(0) }; 3],
};

/// The callers seen since the last flush, fixed size: no allocation and no
/// lock on the hot path.
static ROWS: [Row; SLOTS] = [ROW; SLOTS];
/// Calls whose return address found no free row.
static OVERFLOW: [AtomicU64; 3] = [const { AtomicU64::new(0) }; 3];
/// The original function (the trampoline), 0 while not detoured.
static CALLER_ORIGINAL: AtomicUsize = AtomicUsize::new(0);
/// The executable's base, for return addresses as RVAs.
static IMAGE_BASE: AtomicUsize = AtomicUsize::new(0);
/// The main thread's id, as the menu's frame runs on it; 0 while unknown.
static MAIN_THREAD: AtomicU32 = AtomicU32::new(0);
/// When the callers were last flushed to the log.
static FLUSHED_MS: AtomicU64 = AtomicU64::new(0);

/// Counts one call from `ret` on `side`. Lock-free: a row is claimed by a
/// compare-and-swap of its key, then counted with one add.
pub fn count(ret: u64, side: Side) {
    let start = usize::try_from((ret ^ (ret >> 7)) % SLOTS as u64).unwrap_or(0);
    for i in 0..SLOTS {
        let row = &ROWS[(start + i) % SLOTS];
        let key = row.key.load(Ordering::Relaxed);
        let mine = key == ret
            || (key == 0
                && match row
                    .key
                    .compare_exchange(0, ret, Ordering::Relaxed, Ordering::Relaxed)
                {
                    Ok(_) => true,
                    Err(now) => now == ret,
                });
        if mine {
            row.counts[side as usize].fetch_add(1, Ordering::Relaxed);
            return;
        }
    }
    OVERFLOW[side as usize].fetch_add(1, Ordering::Relaxed);
}

/// The counts since the last call, busiest first, and the overflow. Each
/// count is taken and zeroed at once, so a call counted meanwhile is in this
/// flush or the next, never lost.
pub fn take_counts() -> (Vec<(u64, [u64; 3])>, [u64; 3]) {
    let mut rows = Vec::new();
    for row in &ROWS {
        let key = row.key.load(Ordering::Relaxed);
        if key == 0 {
            continue;
        }
        let counts = [0, 1, 2].map(|i| row.counts[i].swap(0, Ordering::Relaxed));
        if counts.iter().any(|&n| n > 0) {
            rows.push((key, counts));
        }
    }
    rows.sort_by_key(|(key, counts)| (std::cmp::Reverse(counts.iter().sum::<u64>()), *key));
    let overflow = [0, 1, 2].map(|i| OVERFLOW[i].swap(0, Ordering::Relaxed));
    (rows, overflow)
}

/// The flush's lines: one per caller, at most 12, as RVAs.
pub fn callers_text(rows: &[(u64, [u64; 3])], overflow: [u64; 3], base: u64) -> Vec<String> {
    if rows.is_empty() && overflow.iter().all(|&n| n == 0) {
        return vec!["probe: owner reads: none in 3 s".into()];
    }
    let mut lines: Vec<String> = rows
        .iter()
        .take(12)
        .map(|(ret, [step, pool, gui])| {
            format!(
                "probe: owner read from rva {:#x}: in the step {step}, sim pool {pool}, GUI {gui}",
                ret.saturating_sub(base)
            )
        })
        .collect();
    if rows.len() > 12 || overflow.iter().any(|&n| n > 0) {
        lines.push(format!(
            "probe: owner reads elsewhere: {} more callers, overflow step {} pool {} GUI {}",
            rows.len().saturating_sub(12),
            overflow[0],
            overflow[1],
            overflow[2]
        ));
    }
    lines
}

#[cfg(windows)]
fn thread_id() -> u32 {
    // SAFETY: reads the calling thread's id; no arguments, no failure.
    unsafe { windows_sys::Win32::System::Threading::GetCurrentThreadId() }
}

#[cfg(not(windows))]
fn thread_id() -> u32 {
    0
}

/// Which side a call on this thread is on now.
fn side_now() -> Side {
    if crate::order::in_step() {
        return Side::Step;
    }
    let main = MAIN_THREAD.load(Ordering::Relaxed);
    if main != 0 && thread_id() != main {
        Side::Pool
    } else {
        Side::Gui
    }
}

/// The detour's body: counts the caller, then calls the original with the
/// same two arguments and returns its answer unchanged.
extern "C" fn owner_read(this: usize, entity: usize, ret: u64) -> usize {
    count(ret, side_now());
    let original = CALLER_ORIGINAL.load(Ordering::Acquire);
    // SAFETY: the trampoline of the function this detours, stored before
    // the detour could be reached; it takes `this` and the entity in rcx and
    // rdx, as the original was called, and returns its pointer in rax.
    let original: extern "C" fn(usize, usize) -> usize =
        unsafe { std::mem::transmute::<usize, extern "C" fn(usize, usize) -> usize>(original) };
    original(this, entity)
}

/// The detour's entry: hands the return address (at `[rsp]` on entry) to
/// [`owner_read`] as its third argument and jumps there, so the stack is the
/// caller's own and `owner_read` returns straight to it. r8 is no argument
/// of the original (`this` in rcx, the entity in edx).
#[cfg(all(windows, target_arch = "x86_64"))]
#[unsafe(naked)]
extern "C" fn owner_read_entry() {
    core::arch::naked_asm!("mov r8, [rsp]", "jmp {body}", body = sym owner_read);
}

/// Detours [`CALLER_TARGET`] for counting; returns its log line.
#[cfg(all(windows, target_arch = "x86_64"))]
fn install_callers(resolved: &ResolvedProfile) -> String {
    use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
    let Some(target) = resolved.get(CALLER_TARGET) else {
        return format!("probe: owner reads not counted: the profile has no {CALLER_TARGET}");
    };
    // SAFETY: a null name asks for the executable's own base.
    let base = unsafe { GetModuleHandleW(std::ptr::null()) } as usize;
    IMAGE_BASE.store(base, Ordering::Release);
    // SAFETY: a function the profile resolved and prologue-checked in this
    // build, detoured while the game starts, before any world exists; the
    // entry forwards every argument register untouched but r8, which the
    // original does not take.
    let installed = unsafe {
        tpf3mp_hookcore::detour::InlineDetour::install(
            target.address as usize as *mut u8,
            owner_read_entry as *const u8,
        )
    };
    match installed {
        Ok(detoured) => {
            CALLER_ORIGINAL.store(detoured.trampoline() as usize, Ordering::Release);
            let _kept = std::mem::ManuallyDrop::new(detoured);
            format!(
                "probe: counting the callers of {CALLER_TARGET} at {:#x}, its answer unchanged; flushed every {} s",
                target.address,
                EVERY_MS / 1000
            )
        }
        Err(error) => format!("probe: owner reads not counted: detouring failed: {error:?}"),
    }
}

#[cfg(not(all(windows, target_arch = "x86_64")))]
fn install_callers(_resolved: &ResolvedProfile) -> String {
    "probe: owner reads not counted: Windows x86-64 only".into()
}

/// The callers' lines when due (every [`EVERY_MS`]); none while not counting.
fn flush_callers(now_ms: u64) -> Vec<String> {
    if CALLER_ORIGINAL.load(Ordering::Acquire) == 0 {
        return Vec::new();
    }
    MAIN_THREAD.store(thread_id(), Ordering::Relaxed);
    let last = FLUSHED_MS.load(Ordering::Relaxed);
    if now_ms.saturating_sub(last) < EVERY_MS {
        return Vec::new();
    }
    FLUSHED_MS.store(now_ms, Ordering::Relaxed);
    let (rows, overflow) = take_counts();
    callers_text(&rows, overflow, IMAGE_BASE.load(Ordering::Acquire) as u64)
}

pub use crate::build_data::native::probe::OTHER_OWNER_TARGET;
/// Edges (with the tool's player) logged apart; the rest are counted.
const OWNER_SLOTS: usize = 64;

/// What the owner of an edge the test called another player's turned out to
/// be, found by asking the original again with each candidate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Owner {
    /// Not asked yet (the row is being claimed).
    Unknown = 0,
    /// The company this player plays for (the GUI's note).
    Company = 1,
    /// The save's own player (the mod's game script's note).
    SavePlayer = 2,
    /// Neither: another company, or no company was noted.
    Other = 3,
}

impl Owner {
    fn from_u8(v: u8) -> Owner {
        match v {
            1 => Owner::Company,
            2 => Owner::SavePlayer,
            3 => Owner::Other,
            _ => Owner::Unknown,
        }
    }
}

struct OwnerRow {
    /// `(player << 32) | entity`, both as u32; 0 while free.
    key: AtomicU64,
    /// The first caller's return address.
    ret: AtomicU64,
    owner: std::sync::atomic::AtomicU8,
    count: AtomicU64,
}

#[allow(clippy::declare_interior_mutable_const)]
const OWNER_ROW: OwnerRow = OwnerRow {
    key: AtomicU64::new(0),
    ret: AtomicU64::new(0),
    owner: std::sync::atomic::AtomicU8::new(0),
    count: AtomicU64::new(0),
};

static OWNER_ROWS: [OwnerRow; OWNER_SLOTS] = [OWNER_ROW; OWNER_SLOTS];
static OWNER_OVERFLOW: AtomicU64 = AtomicU64::new(0);
/// The test's original (the trampoline), 0 while not detoured.
static OWNER_ORIGINAL: AtomicUsize = AtomicUsize::new(0);
/// The company this player plays for and the save's player, as noted; -1
/// while unknown. Refreshed from the notes on the main thread.
static NOTED_COMPANY: std::sync::atomic::AtomicI64 = std::sync::atomic::AtomicI64::new(-1);
static NOTED_SAVE_PLAYER: std::sync::atomic::AtomicI64 = std::sync::atomic::AtomicI64::new(-1);

fn owner_key(player: i32, entity: i32) -> u64 {
    // A key of 0 marks a free row; (0, 0) is no edge a tool asks about.
    (u64::from(player as u32) << 32) | u64::from(entity as u32)
}

/// Counts one "another player's" answer for `entity` under the tool's
/// `player`, from `ret`. When the row is new, `classify` says whose the
/// edge is. Lock-free, as [`count`].
pub fn note_other_owner(player: i32, entity: i32, ret: u64, classify: impl FnOnce() -> Owner) {
    let key = owner_key(player, entity);
    if key == 0 {
        return;
    }
    let start = usize::try_from((key ^ (key >> 29)) % OWNER_SLOTS as u64).unwrap_or(0);
    for i in 0..OWNER_SLOTS {
        let row = &OWNER_ROWS[(start + i) % OWNER_SLOTS];
        let now = row.key.load(Ordering::Acquire);
        if now == key {
            row.count.fetch_add(1, Ordering::Relaxed);
            return;
        }
        if now == 0 {
            match row
                .key
                .compare_exchange(0, key, Ordering::AcqRel, Ordering::Acquire)
            {
                Ok(_) => {
                    row.ret.store(ret, Ordering::Relaxed);
                    row.owner.store(classify() as u8, Ordering::Release);
                    row.count.fetch_add(1, Ordering::Relaxed);
                    return;
                }
                Err(other) if other == key => {
                    row.count.fetch_add(1, Ordering::Relaxed);
                    return;
                }
                Err(_) => {}
            }
        }
    }
    OWNER_OVERFLOW.fetch_add(1, Ordering::Relaxed);
}

/// One logged edge: the tool's player, the edge, whose it is, the first
/// caller and the answers since the last flush.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OtherOwned {
    pub player: i32,
    pub entity: i32,
    pub owner: Owner,
    pub ret: u64,
    pub count: u64,
}

/// The edges answered since the last call, most asked first, and the
/// overflow. Rows keep their edge, so an edge is described once and counted
/// again later.
pub fn take_other_owned() -> (Vec<OtherOwned>, u64) {
    let mut rows = Vec::new();
    for row in &OWNER_ROWS {
        let key = row.key.load(Ordering::Acquire);
        if key == 0 {
            continue;
        }
        let count = row.count.swap(0, Ordering::Relaxed);
        if count == 0 {
            continue;
        }
        rows.push(OtherOwned {
            player: (key >> 32) as u32 as i32,
            entity: key as u32 as i32,
            owner: Owner::from_u8(row.owner.load(Ordering::Acquire)),
            ret: row.ret.load(Ordering::Relaxed),
            count,
        });
    }
    rows.sort_by_key(|r| (std::cmp::Reverse(r.count), r.entity));
    (rows, OWNER_OVERFLOW.swap(0, Ordering::Relaxed))
}

/// The flush's lines for the edges, at most 12, the caller as an RVA.
pub fn other_owned_text(
    rows: &[OtherOwned],
    overflow: u64,
    base: u64,
    company: i64,
    save: i64,
) -> Vec<String> {
    let mut lines: Vec<String> = rows
        .iter()
        .take(12)
        .map(|r| {
            let owner = match r.owner {
                Owner::Company => format!("owned by this player's company {company}"),
                Owner::SavePlayer => format!("owned by the save's player {save}"),
                Owner::Other if company < 0 => {
                    "owned by another player (no company noted for this player)".to_owned()
                }
                Owner::Other => format!(
                    "owned by neither this player's company {company} nor the save's player {save}"
                ),
                Owner::Unknown => "owner not read yet".to_owned(),
            };
            format!(
                "probe: a native tool took entity {} for another player's: the tool acts as player {}, the entity is {owner}; {} time(s), first from rva {:#x}",
                r.entity,
                r.player,
                r.count,
                r.ret.saturating_sub(base)
            )
        })
        .collect();
    if rows.len() > 12 || overflow > 0 {
        lines.push(format!(
            "probe: a native tool took {} more entities for another player's, {overflow} answer(s) uncounted",
            rows.len().saturating_sub(12)
        ));
    }
    lines
}

/// The detour's body: asks the original, and when it answers "another
/// player's", counts the edge and, the first time, asks the original again
/// with the noted company and the save's player to say whose it is. Returns
/// the original's first answer unchanged.
extern "C" fn other_owner(engine: usize, player: i32, entity: i32, ret: u64) -> bool {
    let original = OWNER_ORIGINAL.load(Ordering::Acquire);
    // SAFETY: the trampoline of the function this detours, stored before
    // the detour could be reached; it takes the engine, the player and the
    // entity in rcx, edx and r8d, as the original was called, and returns a
    // bool in al. It reads the engine's components only.
    let original: extern "C" fn(usize, i32, i32) -> bool =
        unsafe { std::mem::transmute::<usize, extern "C" fn(usize, i32, i32) -> bool>(original) };
    let answer = original(engine, player, entity);
    if answer {
        note_other_owner(player, entity, ret, || {
            let company = NOTED_COMPANY.load(Ordering::Relaxed);
            let save = NOTED_SAVE_PLAYER.load(Ordering::Relaxed);
            // Owned, by not `who`: the original answers false for its owner.
            let owned_by = |who: i64| {
                i32::try_from(who).is_ok_and(|who| who >= 0 && !original(engine, who, entity))
            };
            if owned_by(company) {
                Owner::Company
            } else if owned_by(save) {
                Owner::SavePlayer
            } else {
                Owner::Other
            }
        });
    }
    answer
}

/// The detour's entry: hands the return address to [`other_owner`] in r9,
/// which the original does not take (engine in rcx, player in edx, entity
/// in r8d), and jumps there on the caller's stack.
#[cfg(all(windows, target_arch = "x86_64"))]
#[unsafe(naked)]
extern "C" fn other_owner_entry() {
    core::arch::naked_asm!("mov r9, [rsp]", "jmp {body}", body = sym other_owner);
}

/// Detours [`OTHER_OWNER_TARGET`] for logging; returns its log line.
#[cfg(all(windows, target_arch = "x86_64"))]
fn install_other_owner(resolved: &ResolvedProfile) -> String {
    use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
    let Some(target) = resolved.get(OTHER_OWNER_TARGET) else {
        return format!(
            "probe: the native tools' ownership test is not logged: the profile has no {OTHER_OWNER_TARGET}"
        );
    };
    // SAFETY: a null name asks for the executable's own base.
    let base = unsafe { GetModuleHandleW(std::ptr::null()) } as usize;
    IMAGE_BASE.store(base, Ordering::Release);
    // SAFETY: a leaf function the profile resolved and prologue-checked in
    // this build, detoured while the game starts; the entry forwards every
    // argument register untouched but r9, which the original does not take.
    let installed = unsafe {
        tpf3mp_hookcore::detour::InlineDetour::install(
            target.address as usize as *mut u8,
            other_owner_entry as *const u8,
        )
    };
    match installed {
        Ok(detoured) => {
            OWNER_ORIGINAL.store(detoured.trampoline() as usize, Ordering::Release);
            let _kept = std::mem::ManuallyDrop::new(detoured);
            format!(
                "probe: logging what {OTHER_OWNER_TARGET} at {:#x} takes for another player's, its answer unchanged; flushed every {} s",
                target.address,
                EVERY_MS / 1000
            )
        }
        Err(error) => {
            format!(
                "probe: the native tools' ownership test is not logged: detouring failed: {error:?}"
            )
        }
    }
}

#[cfg(not(all(windows, target_arch = "x86_64")))]
fn install_other_owner(_resolved: &ResolvedProfile) -> String {
    "probe: the native tools' ownership test is not logged: Windows x86-64 only".into()
}

fn noted_entity(key: &str) -> i64 {
    crate::lua::noted(key)
        .and_then(|v| v.trim().parse::<i64>().ok())
        .filter(|&v| v >= 0)
        .unwrap_or(-1)
}

/// When the ownership lines were last flushed.
static OWNER_FLUSHED_MS: AtomicU64 = AtomicU64::new(0);

/// The ownership lines when due (every [`EVERY_MS`]), and the notes read
/// afresh for the next answers; none while not logging.
fn flush_other_owned(now_ms: u64) -> Vec<String> {
    if OWNER_ORIGINAL.load(Ordering::Acquire) == 0 {
        return Vec::new();
    }
    NOTED_COMPANY.store(
        noted_entity(crate::toolplayer::COMPANY_NOTE),
        Ordering::Relaxed,
    );
    NOTED_SAVE_PLAYER.store(noted_entity("tpf3mp.player"), Ordering::Relaxed);
    let last = OWNER_FLUSHED_MS.load(Ordering::Relaxed);
    if now_ms.saturating_sub(last) < EVERY_MS {
        return Vec::new();
    }
    OWNER_FLUSHED_MS.store(now_ms, Ordering::Relaxed);
    let (rows, overflow) = take_other_owned();
    if rows.is_empty() && overflow == 0 {
        return Vec::new();
    }
    other_owned_text(
        &rows,
        overflow,
        IMAGE_BASE.load(Ordering::Acquire) as u64,
        NOTED_COMPANY.load(Ordering::Relaxed),
        NOTED_SAVE_PLAYER.load(Ordering::Relaxed),
    )
}

pub use crate::build_data::native::probe::BULLDOZE_EDGE_TARGET;
pub use crate::build_data::native::probe::BULLDOZE_OWNER_TARGET;
/// Rows kept between flushes; the rest are counted.
const BULLDOZE_ROWS: usize = 48;

/// One answer: which test, the entity, the first players of the list (and
/// how many), the answer, the caller (its return address), and how often.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BulldozeRow {
    pub owner_test: bool,
    pub caller: u64,
    pub entity: i32,
    pub players: Vec<i32>,
    pub listed: usize,
    pub answer: bool,
    pub count: u64,
}

struct BulldozeLog {
    rows: Vec<BulldozeRow>,
    dropped: u64,
    flushed_ms: u64,
}

static BULLDOZE_LOG: Mutex<BulldozeLog> = Mutex::new(BulldozeLog {
    rows: Vec::new(),
    dropped: 0,
    flushed_ms: 0,
});
static BULLDOZE_EDGE_ORIGINAL: AtomicUsize = AtomicUsize::new(0);
static BULLDOZE_OWNER_ORIGINAL: AtomicUsize = AtomicUsize::new(0);

/// Counts one answer, merging it with an equal one since the last flush.
pub fn note_bulldoze(
    owner_test: bool,
    caller: u64,
    entity: i32,
    players: Vec<i32>,
    listed: usize,
    answer: bool,
) {
    let mut log = BULLDOZE_LOG.lock().unwrap_or_else(PoisonError::into_inner);
    if let Some(row) = log.rows.iter_mut().find(|r| {
        r.owner_test == owner_test
            && r.caller == caller
            && r.entity == entity
            && r.answer == answer
            && r.players == players
            && r.listed == listed
    }) {
        row.count += 1;
        return;
    }
    if log.rows.len() >= BULLDOZE_ROWS {
        log.dropped += 1;
        return;
    }
    log.rows.push(BulldozeRow {
        owner_test,
        caller,
        entity,
        players,
        listed,
        answer,
        count: 1,
    });
}

/// The lines for `rows` (callers as RVAs from `base`), and for answers that
/// found no row.
pub fn bulldoze_text(rows: &[BulldozeRow], dropped: u64, base: u64) -> Vec<String> {
    let mut lines: Vec<String> = rows
        .iter()
        .map(|r| {
            let list = if r.listed == 0 {
                "an empty owner list".to_owned()
            } else {
                let shown: Vec<String> = r.players.iter().map(i32::to_string).collect();
                format!(
                    "owner list [{}{}]",
                    shown.join(", "),
                    if r.listed > r.players.len() {
                        ", ..."
                    } else {
                        ""
                    }
                )
            };
            format!(
                "probe: {} entity {}: {} with {list}; {} time(s), from rva {:#x}",
                if r.owner_test {
                    "the bulldozer's owner test on"
                } else {
                    "the street bulldozer's edge test on"
                },
                r.entity,
                if r.answer { "allowed" } else { "refused" },
                r.count,
                r.caller.saturating_sub(base)
            )
        })
        .collect();
    if dropped > 0 {
        lines.push(format!(
            "probe: {dropped} more bulldozer answer(s) not listed"
        ));
    }
    lines
}

/// The players of a `std::vector<Entity>` at `vector` (begin, end): the
/// first four, and how many; none when unreadable.
fn players_at(vector: usize) -> (Vec<i32>, usize) {
    let (Some(begin), Some(end)) = (pointer(vector), pointer(vector.wrapping_add(8))) else {
        return (Vec::new(), 0);
    };
    let Some(bytes) = end.checked_sub(begin) else {
        return (Vec::new(), 0);
    };
    let listed = bytes / 4;
    if listed > 4096 {
        return (Vec::new(), 0);
    }
    let shown = listed.min(4);
    if shown == 0 || !crate::image::readable(begin, shown * 4) {
        return (Vec::new(), listed);
    }
    // SAFETY: `shown` readable dwords at the list's start, only read.
    let players = (0..shown)
        .map(|i| unsafe { std::ptr::read_unaligned((begin + 4 * i) as *const i32) })
        .collect();
    (players, listed)
}

thread_local! {
    /// The owner test's caller, noted by its entry thunk for this call.
    static OWNER_CALLER: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

extern "system" fn note_owner_caller(ret: u64) {
    OWNER_CALLER.with(|c| c.set(ret));
}

/// The edge test's entry: its return address to [`bulldoze_edge`] in r8,
/// which the edge test does not take (captures in rcx, edge in edx).
#[cfg(all(windows, target_arch = "x86_64"))]
#[unsafe(naked)]
extern "C" fn bulldoze_edge_entry() {
    core::arch::naked_asm!("mov r8, [rsp]", "jmp {body}", body = sym bulldoze_edge);
}

/// The owner test's entry: it takes four arguments, so its return address
/// goes to [`note_owner_caller`] with the four argument registers saved,
/// then on to [`bulldoze_owner`] with the stack as the caller left it.
#[cfg(all(windows, target_arch = "x86_64"))]
#[unsafe(naked)]
extern "C" fn bulldoze_owner_entry() {
    core::arch::naked_asm!(
        // Entry rsp is 8 mod 16: 0x48 aligns it, with shadow space under
        // the four saved registers.
        "sub rsp, 0x48",
        "mov [rsp + 0x20], rcx",
        "mov [rsp + 0x28], rdx",
        "mov [rsp + 0x30], r8",
        "mov [rsp + 0x38], r9",
        "mov rcx, [rsp + 0x48]",
        "call {note}",
        "mov r9, [rsp + 0x38]",
        "mov r8, [rsp + 0x30]",
        "mov rdx, [rsp + 0x28]",
        "mov rcx, [rsp + 0x20]",
        "add rsp, 0x48",
        "jmp {body}",
        note = sym note_owner_caller,
        body = sym bulldoze_owner,
    )
}

extern "C" fn bulldoze_edge(captures: usize, entity: i32, caller: u64) -> bool {
    let original = BULLDOZE_EDGE_ORIGINAL.load(Ordering::Acquire);
    // SAFETY: the trampoline of the function this detours, stored before
    // the detour could be reached; called as the game called it.
    let original: extern "C" fn(usize, i32) -> bool =
        unsafe { std::mem::transmute::<usize, extern "C" fn(usize, i32) -> bool>(original) };
    let answer = original(captures, entity);
    let _ = std::panic::catch_unwind(|| {
        let query = pointer(captures.wrapping_add(0x18)).unwrap_or(0);
        let (players, listed) = if query == 0 {
            (Vec::new(), 0)
        } else {
            players_at(query + 8)
        };
        note_bulldoze(false, caller, entity, players, listed, answer);
    });
    answer
}

extern "C" fn bulldoze_owner(engine: usize, list: usize, entity: i32, flag: u8) -> bool {
    let original = BULLDOZE_OWNER_ORIGINAL.load(Ordering::Acquire);
    // SAFETY: as [`bulldoze_edge`]; the four arguments in rcx, rdx, r8d and
    // r9b, as the game passed them.
    let original: extern "C" fn(usize, usize, i32, u8) -> bool = unsafe {
        std::mem::transmute::<usize, extern "C" fn(usize, usize, i32, u8) -> bool>(original)
    };
    let caller = OWNER_CALLER.with(|c| c.replace(0));
    let answer = original(engine, list, entity, flag);
    let _ = std::panic::catch_unwind(|| {
        let (players, listed) = players_at(list);
        note_bulldoze(true, caller, entity, players, listed, answer);
    });
    answer
}

/// Detours the two bulldozer tests for logging; their log lines.
#[cfg(all(windows, target_arch = "x86_64"))]
fn install_bulldoze(resolved: &ResolvedProfile) -> Vec<String> {
    let mut lines = Vec::new();
    for (name, entry, original) in [
        (
            BULLDOZE_EDGE_TARGET,
            bulldoze_edge_entry as *const u8,
            &BULLDOZE_EDGE_ORIGINAL,
        ),
        (
            BULLDOZE_OWNER_TARGET,
            bulldoze_owner_entry as *const u8,
            &BULLDOZE_OWNER_ORIGINAL,
        ),
    ] {
        let Some(target) = resolved.get(name) else {
            lines.push(format!("probe: {name} not logged: the profile lacks it"));
            continue;
        };
        // SAFETY: a function the profile resolved and prologue-checked in
        // this build, detoured while the game starts; the detour takes the
        // same arguments and calls the original with them.
        let installed = unsafe {
            tpf3mp_hookcore::detour::InlineDetour::install(
                target.address as usize as *mut u8,
                entry,
            )
        };
        match installed {
            Ok(detoured) => {
                original.store(detoured.trampoline() as usize, Ordering::Release);
                let _kept = std::mem::ManuallyDrop::new(detoured);
                lines.push(format!(
                    "probe: logging the answers of {name} at {:#x}, unchanged; flushed every {} s",
                    target.address,
                    EVERY_MS / 1000
                ));
            }
            Err(error) => lines.push(format!(
                "probe: {name} not logged: detouring failed: {error:?}"
            )),
        }
    }
    lines
}

#[cfg(not(all(windows, target_arch = "x86_64")))]
fn install_bulldoze(_resolved: &ResolvedProfile) -> Vec<String> {
    vec!["probe: the bulldozer's tests are not logged: Windows x86-64 only".into()]
}

/// The bulldozer lines when due.
fn flush_bulldoze(now_ms: u64) -> Vec<String> {
    if BULLDOZE_EDGE_ORIGINAL.load(Ordering::Acquire) == 0
        && BULLDOZE_OWNER_ORIGINAL.load(Ordering::Acquire) == 0
    {
        return Vec::new();
    }
    let mut log = BULLDOZE_LOG.lock().unwrap_or_else(PoisonError::into_inner);
    if now_ms.saturating_sub(log.flushed_ms) < EVERY_MS {
        return Vec::new();
    }
    log.flushed_ms = now_ms;
    let rows = std::mem::take(&mut log.rows);
    let dropped = std::mem::take(&mut log.dropped);
    drop(log);
    bulldoze_text(&rows, dropped, IMAGE_BASE.load(Ordering::Acquire) as u64)
}

pub use crate::build_data::native::probe::VIEWER_BYTES;
pub use crate::build_data::native::probe::VIEWER_TARGET;
const VIEWER_ROWS: usize = 32;

/// One line's route data as the viewer found it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ViewerRow {
    pub line: i32,
    pub revision: i32,
    pub expected_revision: i32,
    pub segments: i64,
    pub expected_segments: i64,
}

static VIEWER_LOG: Mutex<Vec<ViewerRow>> = Mutex::new(Vec::new());
static VIEWER_SAID: Mutex<Vec<ViewerRow>> = Mutex::new(Vec::new());

/// A vector's element count, for elements of `size` bytes, from its begin
/// and end at `at`.
fn vector_len(at: usize, size: usize) -> Option<i64> {
    let begin = pointer(at)?;
    let end = pointer(at.wrapping_add(8))?;
    let bytes = end.checked_sub(begin)?;
    i64::try_from(bytes / size).ok()
}

fn read_i32(at: usize) -> Option<i32> {
    if at == 0 || !crate::image::readable(at, 4) {
        return None;
    }
    // SAFETY: four readable bytes; only read.
    Some(unsafe { std::ptr::read_unaligned(at as *const i32) })
}

/// The line for hook.log.
pub fn viewer_text(r: &ViewerRow) -> String {
    let drawn = r.revision == r.expected_revision && r.segments == r.expected_segments;
    format!(
        "probe: the line viewer's route data for line {}: revision {} (it expects {}), {} segment list(s) (it expects {}); {}",
        r.line,
        r.revision,
        r.expected_revision,
        r.segments,
        r.expected_segments,
        if drawn {
            "it draws the line"
        } else {
            "it skips the line"
        }
    )
}

unsafe extern "system" fn viewer_hook(regs: *mut tpf3mp_hookcore::detour::SavedRegs) {
    let _ = std::panic::catch_unwind(|| {
        // SAFETY: the stub's block, held until the hook returns; only read.
        let regs = unsafe { &*regs };
        let data = regs.rax as usize;
        let state = regs.r15 as usize;
        let Some(line) = read_i32(regs.rbx as usize) else {
            return;
        };
        let (Some(revision), Some(expected_revision)) = (
            read_i32(data.wrapping_add(0x18)),
            read_i32(state.wrapping_add(0x80)),
        ) else {
            return;
        };
        let (Some(segments), Some(expected_segments)) = (
            vector_len(data, 0x18),
            vector_len(state.wrapping_add(0x68), 0x18),
        ) else {
            return;
        };
        let row = ViewerRow {
            line,
            revision,
            expected_revision,
            segments,
            expected_segments,
        };
        let Ok(mut log) = VIEWER_LOG.try_lock() else {
            return;
        };
        if !log.contains(&row) && log.len() < VIEWER_ROWS {
            log.push(row);
        }
    });
}

#[cfg(all(windows, target_arch = "x86_64"))]
fn install_viewer(resolved: &ResolvedProfile) -> String {
    let Some(site) = resolved.get(VIEWER_TARGET) else {
        return format!("probe: {VIEWER_TARGET} not logged: the profile lacks it");
    };
    // SAFETY: a site the profile resolved by a unique signature inside the
    // line viewer, spliced before any world runs; the bytes are checked
    // again by Splice::install; no branch lands inside them (tpfre); the
    // hook only reads.
    match unsafe {
        tpf3mp_hookcore::detour::Splice::install(
            site.address as usize as *mut u8,
            &VIEWER_BYTES,
            VIEWER_BYTES.len(),
            viewer_hook,
        )
    } {
        Ok(splice) => {
            let _kept = std::mem::ManuallyDrop::new(splice);
            format!(
                "probe: logging the line viewer's route data test at {:#x}, unchanged",
                site.address
            )
        }
        Err(error) => format!("probe: {VIEWER_TARGET} not logged: {error}"),
    }
}

#[cfg(not(all(windows, target_arch = "x86_64")))]
fn install_viewer(_resolved: &ResolvedProfile) -> String {
    "probe: the line viewer is not logged: Windows x86-64 only".into()
}

/// The viewer's lines not said before, once each.
fn flush_viewer() -> Vec<String> {
    let rows = match VIEWER_LOG.try_lock() {
        Ok(mut log) => std::mem::take(&mut *log),
        Err(_) => return Vec::new(),
    };
    let mut said = VIEWER_SAID.lock().unwrap_or_else(PoisonError::into_inner);
    let mut lines = Vec::new();
    for row in rows {
        if !said.contains(&row) && said.len() < 4 * VIEWER_ROWS {
            said.push(row);
            lines.push(viewer_text(&row));
        }
    }
    lines
}

pub use crate::build_data::native::probe::GEOMETRY_TARGET;
const GEOMETRY_ROWS: usize = 32;

/// One line's geometry as the viewer built it: the byte sizes of the
/// result's three vectors.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GeometryRow {
    pub line: i32,
    pub filter: i32,
    pub bytes: [i64; 3],
}

static GEOMETRY_LOG: Mutex<Vec<GeometryRow>> = Mutex::new(Vec::new());
static GEOMETRY_SAID: Mutex<Vec<GeometryRow>> = Mutex::new(Vec::new());
static GEOMETRY_ORIGINAL: AtomicUsize = AtomicUsize::new(0);

pub fn geometry_text(r: &GeometryRow) -> String {
    format!(
        "probe: the line viewer built line {}'s edge geometry (stop filter {}): {} / {} / {} bytes{}",
        r.line,
        r.filter,
        r.bytes[0],
        r.bytes[1],
        r.bytes[2],
        if r.bytes.iter().all(|&b| b == 0) {
            "; nothing to draw"
        } else {
            ""
        }
    )
}

fn vector_bytes(at: usize) -> Option<i64> {
    let begin = pointer(at)?;
    let end = pointer(at.wrapping_add(8))?;
    i64::try_from(end.checked_sub(begin)?).ok()
}

type GeometryFn = extern "C" fn(u64, u64, u64, u64, u64, u64, u64) -> u64;

extern "C" fn geometry_detour(
    a: u64,
    out: u64,
    line: u64,
    filter: u64,
    p5: u64,
    p6: u64,
    p7: u64,
) -> u64 {
    let original = GEOMETRY_ORIGINAL.load(Ordering::Acquire);
    // SAFETY: the trampoline of the function this detours, stored before the
    // detour could be reached; called with all seven of its arguments, as
    // the game called it (four in registers, three on the stack).
    let original: GeometryFn = unsafe { std::mem::transmute::<usize, GeometryFn>(original) };
    let answer = original(a, out, line, filter, p5, p6, p7);
    let _ = std::panic::catch_unwind(|| {
        let out = out as usize;
        let (Some(b0), Some(b1), Some(b2)) = (
            vector_bytes(out.wrapping_add(0x08)),
            vector_bytes(out.wrapping_add(0x28)),
            vector_bytes(out.wrapping_add(0x50)),
        ) else {
            return;
        };
        let row = GeometryRow {
            line: line as u32 as i32,
            filter: filter as u32 as i32,
            bytes: [b0, b1, b2],
        };
        if let Ok(mut log) = GEOMETRY_LOG.try_lock()
            && !log.contains(&row)
            && log.len() < GEOMETRY_ROWS
        {
            log.push(row);
        }
    });
    answer
}

#[cfg(all(windows, target_arch = "x86_64"))]
fn install_geometry(resolved: &ResolvedProfile) -> String {
    let Some(target) = resolved.get(GEOMETRY_TARGET) else {
        return format!("probe: {GEOMETRY_TARGET} not logged: the profile lacks it");
    };
    // SAFETY: a function the profile resolved and prologue-checked in this
    // build, detoured before any world runs; the detour forwards its seven
    // arguments to the original and returns its answer.
    let installed = unsafe {
        tpf3mp_hookcore::detour::InlineDetour::install(
            target.address as usize as *mut u8,
            geometry_detour as *const u8,
        )
    };
    match installed {
        Ok(detoured) => {
            GEOMETRY_ORIGINAL.store(detoured.trampoline() as usize, Ordering::Release);
            let _kept = std::mem::ManuallyDrop::new(detoured);
            format!(
                "probe: logging the line viewer's edge geometry per line at {:#x}, unchanged",
                target.address
            )
        }
        Err(error) => format!("probe: {GEOMETRY_TARGET} not logged: detouring failed: {error:?}"),
    }
}

#[cfg(not(all(windows, target_arch = "x86_64")))]
fn install_geometry(_resolved: &ResolvedProfile) -> String {
    "probe: the line viewer's geometry is not logged: Windows x86-64 only".into()
}

fn flush_geometry() -> Vec<String> {
    let rows = match GEOMETRY_LOG.try_lock() {
        Ok(mut log) => std::mem::take(&mut *log),
        Err(_) => return Vec::new(),
    };
    let mut said = GEOMETRY_SAID.lock().unwrap_or_else(PoisonError::into_inner);
    let mut lines = Vec::new();
    for row in rows {
        if !said.contains(&row) && said.len() < 4 * GEOMETRY_ROWS {
            said.push(row);
            lines.push(geometry_text(&row));
        }
    }
    lines
}

/// When the probe last looked, and at which game.
struct Pace {
    game: usize,
    frames_left: u32,
    last_ms: u64,
    said_off: bool,
}

static PACE: Mutex<Pace> = Mutex::new(Pace {
    game: usize::MAX,
    frames_left: 0,
    last_ms: 0,
    said_off: false,
});

/// Turns the probe on when [`ENV`] is `1` and both targets resolved and
/// read as the getters they name; returns its log line.
pub fn install(resolved: &ResolvedProfile) -> String {
    install_with(resolved, std::env::var(ENV).ok().as_deref() == Some("1"))
}

pub fn install_with(resolved: &ResolvedProfile, wanted: bool) -> String {
    ON.store(false, Ordering::Release);
    if !wanted {
        return format!("probe: the engine's player is not probed ({ENV}=1 turns it on)");
    }
    // The GUI's Lua probes (tpf3mp/follow.lua, watchLineViewers) read this.
    crate::lua::set_note(LUA_NOTE, "1");
    let (Some(gui), Some(sim)) = (resolved.get(GUI_TARGET), resolved.get(SIM_TARGET)) else {
        return format!(
            "probe: off, the profile has no {GUI_TARGET} or {SIM_TARGET}; nothing is read"
        );
    };
    let code = |address: u64, len: usize| -> Option<&'static [u8]> {
        let address = usize::try_from(address).ok()?;
        if !crate::image::readable(address, len) {
            return None;
        }
        // SAFETY: `len` bytes of the game's mapped code at the address the
        // profile resolved in this very build, checked readable above; only
        // read.
        Some(unsafe { std::slice::from_raw_parts(address as *const u8, len) })
    };
    let (Some(g), Some(s)) = (
        code(gui.address, 19).and_then(gui_fields),
        code(sim.address, 37).and_then(sim_fields),
    ) else {
        return "probe: off, a getter is not the code it names; nothing is read".into();
    };
    GUI_GAME.store(g.game, Ordering::Release);
    GUI_STATE.store(g.state, Ordering::Release);
    SIM_STATES.store(s.states, Ordering::Release);
    SIM_INDEX.store(s.index, Ordering::Release);
    SIM_BASE.store(s.base, Ordering::Release);
    ON.store(true, Ordering::Release);
    let callers = install_callers(resolved);
    let owners = install_other_owner(resolved);
    let bulldoze = install_bulldoze(resolved).join("\n");
    let viewer = install_viewer(resolved);
    let geometry = install_geometry(resolved);
    format!(
        "{callers}\n{owners}\n{bulldoze}\n{viewer}\n{geometry}\nprobe: reading the engine's player, read only: the GUI's GameState at [[menu+{:#x}]+{:#x}], the engine's at [[game+{:#x}]+{:#x}+8*i], i at +{:#x}; {} bytes of each scanned every {} s",
        g.game,
        g.state,
        s.states,
        s.base,
        s.index,
        SCAN_BYTES,
        EVERY_MS / 1000
    )
}

/// A pointer at `address`, if it is readable.
fn pointer(address: usize) -> Option<usize> {
    if address == 0 || !crate::image::readable(address, 8) {
        return None;
    }
    // SAFETY: eight readable bytes, checked just above; one unaligned read,
    // never written.
    Some(unsafe { std::ptr::read_unaligned(address as *const usize) })
}

/// [`SCAN_BYTES`] of the object at `address`, if they are readable.
fn object(address: usize) -> Option<Vec<u8>> {
    if address == 0 || !crate::image::readable(address, SCAN_BYTES) {
        return None;
    }
    // SAFETY: SCAN_BYTES readable bytes, checked just above; copied, never
    // written.
    Some(unsafe { std::slice::from_raw_parts(address as *const u8, SCAN_BYTES) }.to_vec())
}

/// One look, after the menu's frame `menu` (a live `UI::CMenuUI`, on its
/// thread): the lines for `hook.log`, none when it is off or not due.
pub fn frame(menu: usize, now_ms: u64) -> Vec<String> {
    if !ON.load(Ordering::Acquire) || menu == 0 {
        return Vec::new();
    }
    let mut lines = flush_callers(now_ms);
    lines.extend(flush_other_owned(now_ms));
    lines.extend(flush_bulldoze(now_ms));
    lines.extend(flush_viewer());
    lines.extend(flush_geometry());
    lines.extend(states(menu, now_ms));
    lines
}

/// The states' lines when due.
fn states(menu: usize, now_ms: u64) -> Vec<String> {
    let game = pointer(menu + GUI_GAME.load(Ordering::Acquire)).unwrap_or(0);
    let mut pace = PACE.lock().unwrap_or_else(PoisonError::into_inner);
    if game != pace.game {
        pace.game = game;
        pace.frames_left = FRAMES_AFTER_CHANGE;
        pace.said_off = false;
    }
    let due = pace.frames_left > 0 || now_ms.saturating_sub(pace.last_ms) >= EVERY_MS;
    if !due {
        return Vec::new();
    }
    pace.frames_left = pace.frames_left.saturating_sub(1);
    pace.last_ms = now_ms;
    if game == 0 {
        if pace.said_off {
            return Vec::new();
        }
        pace.said_off = true;
        return vec!["probe: at the menu, no CGame (m_game is 0)".into()];
    }
    drop(pace);
    look(game)
}

fn look(game: usize) -> Vec<String> {
    let gui = pointer(game + GUI_STATE.load(Ordering::Acquire)).unwrap_or(0);
    let states = pointer(game + SIM_STATES.load(Ordering::Acquire)).unwrap_or(0);
    let base = SIM_BASE.load(Ordering::Acquire);
    let (b0, b1, index) = if states == 0 {
        (0, 0, None)
    } else {
        let index = SIM_INDEX.load(Ordering::Acquire);
        let i = if crate::image::readable(states + index, 4) {
            // SAFETY: four readable bytes, checked just above; read only.
            Some(unsafe { std::ptr::read_unaligned((states + index) as *const i32) })
        } else {
            None
        };
        (
            pointer(states + base).unwrap_or(0),
            pointer(states + base + 8).unwrap_or(0),
            i,
        )
    };
    let player = crate::lua::noted("tpf3mp.player").and_then(|v| v.trim().parse::<u32>().ok());
    let mut lines = vec![format!(
        "probe: CGame {game:#x}: GUI GameState [+{:#x}] {gui:#x}; engine buffers [+{:#x}] {states:#x}: [0] {b0:#x}, [1] {b1:#x}, i {}; the GUI's is {}",
        GUI_STATE.load(Ordering::Acquire),
        SIM_STATES.load(Ordering::Acquire),
        index.map_or("unreadable".into(), |i| i.to_string()),
        if gui == 0 {
            "unread".to_owned()
        } else if gui == b0 {
            "buffer [0]".to_owned()
        } else if gui == b1 {
            "buffer [1]".to_owned()
        } else {
            "neither buffer".to_owned()
        }
    )];
    match player {
        None => lines.push(
            "probe: the save's player is not known yet (the mod's game script notes it once linked)"
                .into(),
        ),
        Some(player) => {
            for (name, at) in [("GUI", gui), ("engine [0]", b0), ("engine [1]", b1)] {
                let found = object(at).map(|bytes| scan(&bytes, player));
                lines.push(format!(
                    "probe: player {player} in {name} {at:#x}: {}",
                    found.map_or("unreadable".into(), |hits| hits_text(&hits))
                ));
            }
        }
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The two getters as build 40408 has them (rva 0x6aa800, 0x11ffd0).
    const GUI: [u8; 19] = [
        0x48, 0x8B, 0x41, 0x08, 0x48, 0x8B, 0x80, 0xB0, 0x06, 0x00, 0x00, 0x48, 0x8B, 0x80, 0xE0,
        0x01, 0x00, 0x00, 0xC3,
    ];
    const SIM: [u8; 37] = [
        0x80, 0x79, 0x10, 0x00, 0x48, 0x8B, 0x41, 0x08, 0x48, 0x8B, 0x80, 0xF0, 0x01, 0x00, 0x00,
        0x74, 0x14, 0xB9, 0x01, 0x00, 0x00, 0x00, 0x2B, 0x88, 0x98, 0x00, 0x00, 0x00, 0x48, 0x63,
        0xD1, 0x48, 0x8B, 0x44, 0xD0, 0x78, 0xC3,
    ];

    #[test]
    fn the_getters_give_their_fields() {
        assert_eq!(
            gui_fields(&GUI),
            Some(GuiFields {
                game: 0x6b0,
                state: 0x1e0
            })
        );
        assert_eq!(
            sim_fields(&SIM),
            Some(SimFields {
                states: 0x1f0,
                index: 0x98,
                base: 0x78
            })
        );
        let mut other = GUI;
        other[18] = 0xCC;
        assert_eq!(gui_fields(&other), None, "not a getter: nothing read");
        let mut other = SIM;
        other[23] = 0x89;
        assert_eq!(sim_fields(&other), None);
        assert_eq!(gui_fields(&GUI[..10]), None);
    }

    #[test]
    fn the_scan_finds_a_dword_and_a_qword() {
        let mut bytes = vec![0u8; 64];
        bytes[8..12].copy_from_slice(&118_368u32.to_le_bytes());
        bytes[20..24].copy_from_slice(&118_368u32.to_le_bytes());
        bytes[24..28].copy_from_slice(&7u32.to_le_bytes());
        assert_eq!(scan(&bytes, 118_368), vec![(8, 'q'), (20, 'd')]);
        assert_eq!(hits_text(&scan(&bytes, 5)), "none");
        assert_eq!(hits_text(&scan(&bytes, 118_368)), "+0x8q +0x14d");
    }

    #[test]
    fn callers_are_counted_by_side_and_flushed_busiest_first() {
        count(0x1_4000_1000, Side::Gui);
        count(0x1_4000_1000, Side::Gui);
        count(0x1_4000_2000, Side::Step);
        count(0x1_4000_1000, Side::Pool);
        let (rows, overflow) = take_counts();
        assert_eq!(
            rows,
            vec![(0x1_4000_1000, [0, 1, 2]), (0x1_4000_2000, [1, 0, 0])]
        );
        assert_eq!(
            callers_text(&rows, overflow, 0x1_4000_0000),
            [
                "probe: owner read from rva 0x1000: in the step 0, sim pool 1, GUI 2",
                "probe: owner read from rva 0x2000: in the step 1, sim pool 0, GUI 0",
            ]
        );
        let (rows, overflow) = take_counts();
        assert!(rows.is_empty(), "taken once");
        assert_eq!(
            callers_text(&rows, overflow, 0),
            ["probe: owner reads: none in 3 s"]
        );
        // More callers than rows: the rest are counted, not lost.
        for i in 1..=(SLOTS as u64 + 3) {
            count(0x2_0000_0000 + i * 16, Side::Gui);
        }
        let (rows, overflow) = take_counts();
        // Rows keep their caller once claimed (call sites are few); the
        // two above still hold theirs.
        assert_eq!(rows.len(), SLOTS - 2);
        assert_eq!(overflow, [0, 0, 5]);
    }

    #[test]
    fn edges_taken_for_another_players_are_described_once_and_counted() {
        let mut asked = 0;
        // The street tool (player 214443) on a road the room built for
        // company 372363, three times, and on another edge once.
        for _ in 0..3 {
            note_other_owner(214_443, 380_001, 0x1_405f_c027, || {
                asked += 1;
                Owner::Company
            });
        }
        note_other_owner(214_443, 380_002, 0x1_405f_c027, || Owner::Other);
        assert_eq!(asked, 1, "whose it is is asked once per edge");
        let (rows, overflow) = take_other_owned();
        assert_eq!(overflow, 0);
        assert_eq!(
            other_owned_text(&rows, overflow, 0x1_4000_0000, 372_363, 214_443),
            [
                "probe: a native tool took entity 380001 for another player's: the tool acts as player 214443, the entity is owned by this player's company 372363; 3 time(s), first from rva 0x5fc027",
                "probe: a native tool took entity 380002 for another player's: the tool acts as player 214443, the entity is owned by neither this player's company 372363 nor the save's player 214443; 1 time(s), first from rva 0x5fc027",
            ]
        );
        let (rows, _) = take_other_owned();
        assert!(rows.is_empty(), "taken once");
        // Counted again later, still described.
        note_other_owner(214_443, 380_001, 0, || unreachable!("described already"));
        let (rows, _) = take_other_owned();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].owner, Owner::Company);
        assert_eq!(rows[0].count, 1);
        let other = OtherOwned {
            owner: Owner::Other,
            ..rows[0].clone()
        };
        assert!(
            other_owned_text(&[other], 0, 0, -1, 214_443)[0]
                .contains("owned by another player (no company noted for this player)")
        );
    }

    #[test]
    fn the_bulldozers_answers_are_merged_and_said() {
        let hover = 0x1_405f_2ae6;
        let click = 0x1_405f_376f;
        note_bulldoze(false, hover, 380_001, vec![372_363], 1, false);
        note_bulldoze(false, hover, 380_001, vec![372_363], 1, false);
        note_bulldoze(true, click, 380_001, vec![214_443], 1, false);
        note_bulldoze(true, hover, 380_002, Vec::new(), 0, true);
        let rows = std::mem::take(
            &mut BULLDOZE_LOG
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .rows,
        );
        assert_eq!(
            bulldoze_text(&rows, 2, 0x1_4000_0000),
            [
                "probe: the street bulldozer's edge test on entity 380001: refused with owner list [372363]; 2 time(s), from rva 0x5f2ae6",
                "probe: the bulldozer's owner test on entity 380001: refused with owner list [214443]; 1 time(s), from rva 0x5f376f",
                "probe: the bulldozer's owner test on entity 380002: allowed with an empty owner list; 1 time(s), from rva 0x5f2ae6",
                "probe: 2 more bulldozer answer(s) not listed",
            ]
        );
        let shown = BulldozeRow {
            owner_test: true,
            caller: 0,
            entity: 1,
            players: vec![1, 2, 3, 4],
            listed: 9,
            answer: true,
            count: 1,
        };
        assert!(bulldoze_text(&[shown], 0, 0)[0].contains("[1, 2, 3, 4, ...]"));
    }

    #[test]
    fn the_viewers_route_data_test_is_said() {
        let skipped = ViewerRow {
            line: 342_589,
            revision: 0,
            expected_revision: 7,
            segments: 0,
            expected_segments: 3,
        };
        assert_eq!(
            viewer_text(&skipped),
            "probe: the line viewer's route data for line 342589: revision 0 (it expects 7), 0 segment list(s) (it expects 3); it skips the line"
        );
        let drawn = ViewerRow {
            revision: 7,
            segments: 3,
            ..skipped
        };
        assert!(viewer_text(&drawn).ends_with("it draws the line"));
        assert_eq!(
            VIEWER_BYTES[..3],
            [0x41, 0x8B, 0x8F],
            "mov ecx,[r15+disp32]"
        );
    }

    #[test]
    fn the_viewers_geometry_per_line_is_said() {
        let empty = GeometryRow {
            line: 272_108,
            filter: -1,
            bytes: [0, 0, 0],
        };
        assert_eq!(
            geometry_text(&empty),
            "probe: the line viewer built line 272108's edge geometry (stop filter -1): 0 / 0 / 0 bytes; nothing to draw"
        );
        let built = GeometryRow {
            bytes: [480, 96, 0],
            ..empty
        };
        assert!(geometry_text(&built).ends_with("480 / 96 / 0 bytes"));
    }

    #[test]
    fn off_unless_asked_and_whole() {
        let empty = ResolvedProfile {
            name: String::new(),
            targets: Vec::new(),
            absent_optional: Vec::new(),
        };
        assert!(install_with(&empty, false).contains("not probed"));
        assert!(install_with(&empty, true).contains("off, the profile has no"));
        assert!(frame(0x1000, 0).is_empty(), "off: reads nothing");
    }
}
