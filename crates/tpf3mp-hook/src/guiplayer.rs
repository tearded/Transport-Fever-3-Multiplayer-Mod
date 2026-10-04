//! The GUI's native views see the player's company in a room (docs/HOOKS.md,
//! "The views' player"; investigation/TF3_LOCAL_PLAYER_2026-10-01.md, "The
//! map's markers and overlays").
//!
//! The station icons above the map, the line and catchment overlays and
//! their colours, what the selector lets the player pick, and two React
//! components decide "the player's own" natively: each asks the GUI's
//! `IGameStateProvider` for the `GameState` and reads its player
//! (`+0x20c`) inline, with no helper in between. That `GameState` is one of
//! the simulation's two buffers (the probe, seen 2026-10-02), so its player
//! is never written. Instead each of those reads is spliced
//! (`tpf3mp_hookcore::detour::Splice`) right after it, and in a room the
//! register that holds the save's player is given the player's company,
//! the one the GUI notes (`note("tpf3mp.company")`). Two reads compare the
//! player with an owner straight away (`cmp [reg], eax`); there the splice
//! is on the read and the compare, and the owner's pointer is pointed at a
//! copy that answers as the company would: the save's player where the
//! owner is the company, and no one where it is the save's player.
//!
//! Every site is a UI function (listed in the profile with its callers:
//! `UI::HudIconManager`, `UI::StationViewer`, the selector, `UI::ViewCreator`,
//! `UI::layers::CatchmentAreaHelper`, `UI::layers::LayerManager`'s colours,
//! two `UI::react` components), reached from the GUI's frame, never from
//! `GameSim::Step`; what it computes only draws or picks. Each site's bytes
//! are checked before it is spliced and a site that differs is left alone.
//! Outside a room, for the room's first company, while either note is
//! missing, or with [`ENV`]`=0`, every read answers as the game's.

#![allow(unsafe_code)]
#![cfg_attr(not(all(windows, target_arch = "x86_64")), allow(dead_code))]

use std::sync::atomic::{
    AtomicBool, AtomicI32, AtomicI64, AtomicU32, AtomicU64, AtomicUsize, Ordering,
};

use tpf3mp_hookcore::detour::{SavedRegs, Splice};
use tpf3mp_hookcore::profile::ResolvedProfile;

/// The kill switch: `0` (or `off`, `false`, `no`) leaves every view the
/// save's player's.
pub const ENV: &str = "TPF3MP_HOOK_GUI_COMPANY";
/// The name in hook.log.
pub const FIX: &str = "view-company";
/// The kill switch of the map's every-company display: `0` keeps the map's
/// icons and line colours to the player's own company's.
pub const ALL_ENV: &str = "TPF3MP_HOOK_GUI_ALL_COMPANIES";
/// The note the GUI keeps the room's companies' entities under, comma
/// separated (`tpf3mp/follow.lua`, `noteCompanies`).
pub const COMPANIES_NOTE: &str = "tpf3mp.companies";
/// Companies a room can hold (DECISIONS.md, D21).
pub const MAX_COMPANIES: usize = 8;

/// Which register a site's player is in, and how the site uses it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reg {
    Rax,
    Rbx,
    Rdx,
    R8,
    R14,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// The register holds the player read just before the splice.
    Value,
    /// The register points at an owner the next instruction compares with
    /// the player.
    Owner,
    /// The HUD icon pass: the owner's pointer is about to be formed as
    /// `rax + rcx*4` and compared with the traversal's player (`r12d`).
    IconOwner,
}

/// One spliced read.
#[derive(Debug, Clone, Copy)]
pub struct Site {
    /// The profile target (at the splice).
    pub name: &'static str,
    /// The bytes the splice takes (whole instructions).
    pub expected: &'static [u8],
    pub reg: Reg,
    pub kind: Kind,
    /// What it decides, for hook.log.
    pub what: &'static str,
}

pub use crate::build_data::native::guiplayer::SITES;

pub use crate::build_data::native::guiplayer::GUI_THREAD_ONLY_FROM;

pub use crate::build_data::native::guiplayer::LAYER_OWNER_SITE;

/// The company and the save's player while a room asks for the company,
/// each -1 otherwise; refreshed from the notes once a frame on the main
/// thread ([`refresh`]), read by the sites on whatever thread they run.
static COMPANY: AtomicI64 = AtomicI64::new(-1);
static SAVE: AtomicI64 = AtomicI64::new(-1);
static ON: AtomicBool = AtomicBool::new(false);
static BROKEN: AtomicBool = AtomicBool::new(false);
/// The owner a pointer site compares when the owner is the company (the
/// save's player, as the read gives), and one no player is.
static SAVE_SLOT: AtomicI32 = AtomicI32::new(-1);
static NONE_SLOT: AtomicI32 = AtomicI32::new(-2);
/// A copy of the company, for the icon pass whose player is the company.
static COMPANY_SLOT: AtomicI32 = AtomicI32::new(-1);
/// The room's companies' entities, -1 where none; refreshed with the rest.
static COMPANIES: [AtomicI64; MAX_COMPANIES] = [const { AtomicI64::new(-1) }; MAX_COMPANIES];
/// Whether the map shows every company ([`ALL_ENV`]).
static ALL: AtomicBool = AtomicBool::new(false);
/// Per site, how many reads it answered with the company; the first is said.
static ANSWERED: [AtomicU64; 19] = [const { AtomicU64::new(0) }; 19];
/// The GUI's thread, as the menu's frame runs on it ([`refresh`]); 0 while
/// unknown.
static GUI_THREAD: AtomicU32 = AtomicU32::new(0);

#[cfg(windows)]
fn thread_id() -> u32 {
    // SAFETY: reads the calling thread's id; no arguments, no failure.
    unsafe { windows_sys::Win32::System::Threading::GetCurrentThreadId() }
}

#[cfg(not(windows))]
fn thread_id() -> u32 {
    0
}

/// Whether a site of [`GUI_THREAD_ONLY_FROM`] on may answer the company
/// here: on the GUI's thread, and not inside the simulation's step.
pub fn on_gui_thread(gui: u32, here: u32, in_step: bool) -> bool {
    gui != 0 && gui == here && !in_step
}

fn noted(key: &str) -> Option<i64> {
    crate::lua::noted(key)
        .and_then(|v| v.trim().parse::<i64>().ok())
        .filter(|&v| (0..=i64::from(i32::MAX)).contains(&v))
}

/// The GUI company and save player in a room. An absent company note means
/// the first company; it must also see the other room companies on the map.
pub fn wanted(room: bool, save: Option<i64>, company: Option<i64>) -> Option<(i64, i64)> {
    match (room, save, company) {
        (true, Some(save), company) => Some((company.unwrap_or(save), save)),
        _ => None,
    }
}

/// Once a frame, on the main thread: what the sites answer until the next.
pub fn refresh() {
    if !ON.load(Ordering::Acquire) {
        return;
    }
    let pair = wanted(
        crate::lua::in_room(),
        noted("tpf3mp.player"),
        noted(crate::toolplayer::COMPANY_NOTE),
    );
    let (company, save) = pair.unwrap_or((-1, -1));
    SAVE.store(save, Ordering::Release);
    SAVE_SLOT.store(i32::try_from(save).unwrap_or(-1), Ordering::Release);
    COMPANY_SLOT.store(i32::try_from(company).unwrap_or(-1), Ordering::Release);
    // The room's companies while in a room, the first company's player
    // included (the line viewers show every company's lines either way).
    let room = crate::lua::in_room();
    ROOM.store(room, Ordering::Release);
    let listed = if room {
        companies(crate::lua::noted(COMPANIES_NOTE).as_deref())
    } else {
        Vec::new()
    };
    for (k, slot) in COMPANIES.iter().enumerate() {
        slot.store(listed.get(k).copied().unwrap_or(-1), Ordering::Release);
    }
    GUI_THREAD.store(thread_id(), Ordering::Release);
    COMPANY.store(company, Ordering::Release);
}

pub use crate::build_data::native::guiplayer::LINES_CALLEE_BYTES;
pub use crate::build_data::native::guiplayer::LINES_SITE;
/// Lines read from one player's list, at most.
const LINES_MAX: usize = 1 << 16;
static LINES_ORIGINAL: AtomicUsize = AtomicUsize::new(0);
/// Whether the last refresh found the game in a room.
static ROOM: AtomicBool = AtomicBool::new(false);
static LINES_ANSWERED: AtomicU64 = AtomicU64::new(0);

thread_local! {
    /// The lines answered last on this thread, and the `std::vector`
    /// header (begin, end, capacity) the caller reads them through. Kept
    /// until the next call on the thread; the caller copies them at once.
    static LINES_BUF: std::cell::RefCell<(Vec<i32>, [usize; 3])> =
        const { std::cell::RefCell::new((Vec::new(), [0; 3])) };
}

/// Whose lines the viewers are to be given in a room: every company's
/// (`all`), else the player's company's; none (the game's own answer) where
/// neither is known.
pub fn line_players(room: bool, all: bool, companies: &[i64], company: i64) -> Vec<i64> {
    if !room {
        return Vec::new();
    }
    if all && !companies.is_empty() {
        return companies.to_vec();
    }
    if company >= 0 {
        return vec![company];
    }
    Vec::new()
}

/// The entities of the `std::vector<Entity>` at `vector`.
fn entities_at(vector: usize, into: &mut Vec<i32>) {
    let read = |a: usize| -> Option<usize> {
        if a == 0 || !crate::image::readable(a, 8) {
            return None;
        }
        // SAFETY: eight readable bytes; only read.
        Some(unsafe { std::ptr::read_unaligned(a as *const usize) })
    };
    let (Some(begin), Some(end)) = (read(vector), read(vector.wrapping_add(8))) else {
        return;
    };
    let Some(bytes) = end.checked_sub(begin) else {
        return;
    };
    let count = (bytes / 4).min(LINES_MAX);
    if count == 0 || !crate::image::readable(begin, count * 4) {
        return;
    }
    // SAFETY: `count` readable dwords from `begin`; only read.
    let slice = unsafe { std::slice::from_raw_parts(begin as *const i32, count) };
    into.extend_from_slice(slice);
}

/// The redirected call: the game's answer outside a room, else the lines
/// of [`line_players`], in a vector this keeps.
extern "C" fn lines_for(index: usize, player: i32) -> usize {
    let original = LINES_ORIGINAL.load(Ordering::Acquire);
    // SAFETY: the index lookup the call reached, its address read from the
    // call and its bytes checked at install; same two arguments.
    let original: extern "C" fn(usize, i32) -> usize =
        unsafe { std::mem::transmute::<usize, extern "C" fn(usize, i32) -> usize>(original) };
    if BROKEN.load(Ordering::Acquire) || !ON.load(Ordering::Acquire) {
        return original(index, player);
    }
    let companies: Vec<i64> = COMPANIES
        .iter()
        .map(|c| c.load(Ordering::Acquire))
        .filter(|&c| c >= 0)
        .collect();
    let players = line_players(
        ROOM.load(Ordering::Acquire),
        ALL.load(Ordering::Acquire),
        &companies,
        COMPANY.load(Ordering::Acquire),
    );
    if players.is_empty() {
        return original(index, player);
    }
    let built = std::panic::catch_unwind(|| {
        LINES_BUF.with(|buf| {
            let mut buf = buf.borrow_mut();
            let (lines, header) = &mut *buf;
            lines.clear();
            for p in &players {
                let Ok(p) = i32::try_from(*p) else { continue };
                entities_at(original(index, p), lines);
            }
            let begin = lines.as_ptr() as usize;
            *header = [begin, begin + lines.len() * 4, begin + lines.capacity() * 4];
            (header.as_ptr() as usize, lines.len())
        })
    });
    match built {
        Ok((header, n)) => {
            if LINES_ANSWERED.fetch_add(1, Ordering::Relaxed) == 0 {
                crate::log::line(&format!(
                    "{FIX}: the line viewers draw the lines of {} ({n} line(s)) in place of player {player}'s ({LINES_SITE})",
                    players
                        .iter()
                        .map(i64::to_string)
                        .collect::<Vec<_>>()
                        .join(", ")
                ));
            }
            header
        }
        Err(_) => {
            BROKEN.store(true, Ordering::Release);
            original(index, player)
        }
    }
}

#[cfg(all(windows, target_arch = "x86_64"))]
fn install_lines(resolved: &ResolvedProfile) -> String {
    let Some(site) = resolved.get(LINES_SITE) else {
        return format!(
            "{FIX}: the line viewers draw the player's lines only: the profile lacks {LINES_SITE}"
        );
    };
    let at = site.address as usize;
    if !crate::image::readable(at, 5) {
        return format!("{FIX}: {LINES_SITE} is unreadable");
    }
    // SAFETY: five readable bytes of the call; only read.
    let call = unsafe { std::slice::from_raw_parts(at as *const u8, 5) };
    let rel = i32::from_le_bytes([call[1], call[2], call[3], call[4]]);
    let target = (at as isize + 5 + rel as isize) as usize;
    if call[0] != 0xE8 || !crate::image::readable(target, LINES_CALLEE_BYTES.len()) {
        return format!("{FIX}: {LINES_SITE} at {at:#x} is not a call it reads");
    }
    // SAFETY: readable bytes of the call's target; only read.
    let bytes =
        unsafe { std::slice::from_raw_parts(target as *const u8, LINES_CALLEE_BYTES.len()) };
    if bytes != LINES_CALLEE_BYTES {
        return format!("{FIX}: {LINES_SITE} at {at:#x} reaches {target:#x}, not the line index");
    }
    LINES_ORIGINAL.store(target, Ordering::Release);
    // SAFETY: the call inside the line viewers' candidate builder, which no
    // viewer runs yet (installed before any world); install checks it is a
    // 5-byte call of `target`; lines_for takes and returns as it does.
    match unsafe {
        tpf3mp_hookcore::detour::CallRedirect::install(
            at as *mut u8,
            target,
            lines_for as *const u8,
        )
    } {
        Ok(redirect) => {
            let _kept = std::mem::ManuallyDrop::new(redirect);
            format!(
                "{FIX}: the line viewers draw every company's lines in a room ({LINES_SITE} at {at:#x})"
            )
        }
        Err(error) => {
            LINES_ORIGINAL.store(0, Ordering::Release);
            format!("{FIX}: the line viewers draw the player's lines only: {error}")
        }
    }
}

#[cfg(not(all(windows, target_arch = "x86_64")))]
fn install_lines(_resolved: &ResolvedProfile) -> String {
    format!("{FIX}: the line viewers draw the player's lines only: Windows x86-64 only")
}

/// The room's companies' entities from their note ("372426,214443"), at most
/// [`MAX_COMPANIES`]; nothing where it does not read.
pub fn companies(note: Option<&str>) -> Vec<i64> {
    let Some(note) = note else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for part in note.split(',') {
        let Ok(v) = part.trim().parse::<i64>() else {
            return Vec::new();
        };
        if !(0..=i64::from(i32::MAX)).contains(&v) {
            return Vec::new();
        }
        if out.len() < MAX_COMPANIES && !out.contains(&v) {
            out.push(v);
        }
    }
    out
}

/// Whether `owner` is a company of the room (the save's player, the room's
/// first, included).
fn a_company(owner: i32) -> bool {
    COMPANIES
        .iter()
        .any(|c| c.load(Ordering::Acquire) == i64::from(owner))
}

/// The icon pass and the layer colours with every company: whether an
/// owner the game compares with its player is shown as if it were that
/// player's.
pub fn shown(owner: i32, player: i32, all: bool, is_company: bool) -> bool {
    owner != player && all && is_company
}

/// What a value site's register becomes: the company where it holds the
/// save's player and the views are to see the company; else as it is.
pub fn value(current: u64, company: i64, save: i64) -> u64 {
    if company < 0 || save < 0 || (current as u32) != save as u32 {
        return current;
    }
    // A 32-bit load zero-extends; the company does so too.
    u64::from(company as u32)
}

/// Where an owner site's pointer should point: at a copy of the save's
/// player where the owner is the company (so the compare matches), at no
/// one where the owner is the save's player; else where it points.
pub fn owner(owner: i32, company: i64, save: i64) -> Option<bool> {
    if company < 0 || save < 0 {
        return None;
    }
    if i64::from(owner) == company {
        Some(true)
    } else if i64::from(owner) == save {
        Some(false)
    } else {
        None
    }
}

fn reg(regs: &mut SavedRegs, reg: Reg) -> &mut u64 {
    match reg {
        Reg::Rax => &mut regs.rax,
        Reg::Rbx => &mut regs.rbx,
        Reg::Rdx => &mut regs.rdx,
        Reg::R8 => &mut regs.r8,
        Reg::R14 => &mut regs.r14,
    }
}

/// A site's work: never unwinds; a panic switches every site off.
fn at_site(index: usize, regs: *mut SavedRegs) {
    if BROKEN.load(Ordering::Acquire) {
        return;
    }
    let done = std::panic::catch_unwind(|| {
        let company = COMPANY.load(Ordering::Acquire);
        let save = SAVE.load(Ordering::Acquire);
        if company < 0 {
            return false;
        }
        let site = SITES[index];
        if index >= GUI_THREAD_ONLY_FROM
            && !on_gui_thread(
                GUI_THREAD.load(Ordering::Acquire),
                thread_id(),
                crate::order::in_step(),
            )
        {
            return false;
        }
        // SAFETY: the stub's block, held until the hook returns.
        let regs = unsafe { &mut *regs };
        match site.kind {
            Kind::Value => {
                let slot = reg(regs, site.reg);
                let next = value(*slot, company, save);
                let changed = next != *slot;
                *slot = next;
                changed
            }
            Kind::IconOwner => {
                // The owner's pointer the next instruction forms.
                let at = regs.rax.wrapping_add(regs.rcx.wrapping_mul(4));
                let at = usize::try_from(at).unwrap_or(0);
                if at == 0 || !at.is_multiple_of(4) || !crate::image::readable(at, 4) {
                    return false;
                }
                // SAFETY: four readable bytes, the owner the game compares
                // next; only read.
                let held = unsafe { std::ptr::read_volatile(at as *const i32) };
                let player = regs.r12 as u32 as i32;
                if !shown(held, player, ALL.load(Ordering::Acquire), a_company(held)) {
                    return false;
                }
                // The copy that holds the pass's player: the company (as the
                // traversal's player read is answered) or the save's.
                let copy = if i64::from(player) == company {
                    COMPANY_SLOT.as_ptr()
                } else if i64::from(player) == save {
                    SAVE_SLOT.as_ptr()
                } else {
                    return false;
                };
                // rax + rcx*4 becomes the copy; both are dead after the
                // compare on either path (tpfre: rax is loaded again at
                // 0x674949 or returned from r14 at 0x674990, rcx written at
                // 0x67492f, 0x6744a5 or 0x674542 before any read).
                regs.rax = copy as u64;
                regs.rcx = 0;
                true
            }
            Kind::Owner => {
                let slot = reg(regs, site.reg);
                let at = usize::try_from(*slot).unwrap_or(0);
                if at == 0 || !at.is_multiple_of(4) || !crate::image::readable(at, 4) {
                    return false;
                }
                // SAFETY: four readable bytes, the owner the game compares
                // next; only read.
                let held = unsafe { std::ptr::read_volatile(at as *const i32) };
                // The layer colours with every company: any company's line
                // or station is coloured as the player's (its own colour).
                if index == LAYER_OWNER_SITE && ALL.load(Ordering::Acquire) && a_company(held) {
                    *slot = SAVE_SLOT.as_ptr() as u64;
                    return true;
                }
                match owner(held, company, save) {
                    Some(true) => {
                        *slot = SAVE_SLOT.as_ptr() as u64;
                        true
                    }
                    Some(false) => {
                        *slot = NONE_SLOT.as_ptr() as u64;
                        true
                    }
                    None => false,
                }
            }
        }
    });
    match done {
        Ok(true) => {
            if ANSWERED[index].fetch_add(1, Ordering::Relaxed) == 0 {
                let site = SITES[index];
                crate::log::line(&format!(
                    "{FIX}: {} sees the player's company {} ({})",
                    site.what,
                    COMPANY.load(Ordering::Relaxed),
                    site.name
                ));
            }
        }
        Ok(false) => {}
        Err(_) => BROKEN.store(true, Ordering::Release),
    }
}

macro_rules! hooks {
    ($($name:ident = $index:expr),* $(,)?) => {
        $(
            unsafe extern "system" fn $name(regs: *mut SavedRegs) {
                at_site($index, regs);
            }
        )*
        const HOOKS: [tpf3mp_hookcore::detour::SpliceHook; 19] = [$($name),*];
    };
}

hooks!(
    h0 = 0,
    h1 = 1,
    h2 = 2,
    h3 = 3,
    h4 = 4,
    h5 = 5,
    h6 = 6,
    h7 = 7,
    h8 = 8,
    h9 = 9,
    h10 = 10,
    h11 = 11,
    h12 = 12,
    h13 = 13,
    h14 = 14,
    h15 = 15,
    h16 = 16,
    h17 = 17,
    h18 = 18,
);

pub use crate::build_data::native::guiplayer::GET_PLAYER_PUSH;
pub use crate::build_data::native::guiplayer::PUSH_BYTES;
static PUSH_ORIGINAL: AtomicUsize = AtomicUsize::new(0);
static PUSH_ANSWERED: AtomicU64 = AtomicU64::new(0);

/// The GUI's getter (`CMenuUI::SwitchToGameUI`'s: `mov rax,[rcx+8]; mov
/// rax,[rax+m_game]; mov rax,[rax+0x1e0]; ret`) or the React GUI's
/// (`ScriptComponentRoot::ReloadInterfaces`'s: through its own function
/// object, then `mov rax,[rax+0x1e0]`): both give the GUI's `GameState`.
/// The engine's getter reads its two buffers and is neither.
pub fn gui_getter(code: &[u8]) -> bool {
    let menu = code.len() >= 19
        && code[..7] == [0x48, 0x8B, 0x41, 0x08, 0x48, 0x8B, 0x80]
        && code[11..19] == [0x48, 0x8B, 0x80, 0xE0, 0x01, 0x00, 0x00, 0xC3];
    let react = code.len() >= 31
        && code[..11]
            == [
                0x48, 0x83, 0xEC, 0x28, 0x48, 0x8B, 0x49, 0x40, 0x48, 0x85, 0xC9,
            ]
        && code[11] == 0x74
        && code[13..30]
            == [
                0x48, 0x8B, 0x01, 0xFF, 0x50, 0x10, 0x48, 0x8B, 0x80, 0xE0, 0x01, 0x00, 0x00, 0x48,
                0x83, 0xC4, 0x28,
            ]
        && code[30] == 0xC3;
    menu || react
}

/// Whether the `getPlayer` closure at `closure` gets its state from one of
/// the GUI's getters: its `std::function` (`+0x38`), that function's
/// vtable, its call (slot 2) and that call's code.
fn closure_is_gui(closure: usize) -> bool {
    let read = |a: usize| -> Option<usize> {
        if a == 0 || !crate::image::readable(a, 8) {
            return None;
        }
        // SAFETY: eight readable bytes, checked just above; only read.
        Some(unsafe { std::ptr::read_unaligned(a as *const usize) })
    };
    let Some(call) = read(closure.wrapping_add(0x38))
        .and_then(read)
        .and_then(|vtable| read(vtable.wrapping_add(0x10)))
    else {
        return false;
    };
    if !crate::image::readable(call, 31) {
        return false;
    }
    // SAFETY: 31 readable bytes of the getter's code; only read.
    gui_getter(unsafe { std::slice::from_raw_parts(call as *const u8, 31) })
}

/// The push's body: the closure in r8 (from [`push_entry`]); the answer
/// becomes the company for a GUI state in a room, then the game's push.
extern "C" fn push_body(state: usize, value: i64, closure: usize) {
    let mut value = value;
    if !BROKEN.load(Ordering::Acquire) {
        let company = COMPANY.load(Ordering::Acquire);
        let save = SAVE.load(Ordering::Acquire);
        let swapped =
            std::panic::catch_unwind(|| company >= 0 && value == save && closure_is_gui(closure));
        match swapped {
            Ok(true) => {
                value = company;
                if PUSH_ANSWERED.fetch_add(1, Ordering::Relaxed) == 0 {
                    crate::log::line(&format!(
                        "{FIX}: the GUI's Lua getPlayer answers the player's company {company} natively ({GET_PLAYER_PUSH})"
                    ));
                }
            }
            Ok(false) => {}
            Err(_) => BROKEN.store(true, Ordering::Release),
        }
    }
    let original = PUSH_ORIGINAL.load(Ordering::Acquire);
    // SAFETY: the push this call reached, its address read from the call
    // and its bytes checked at install; called with its two arguments.
    let original: extern "C" fn(usize, i64) =
        unsafe { std::mem::transmute::<usize, extern "C" fn(usize, i64)>(original) };
    original(state, value);
}

/// The redirected call's entry: rdi still holds the closure (the binding
/// keeps it there from its start, 0x24ed248, and calls only the getter
/// before), handed to [`push_body`] in r8, which the push does not take.
#[cfg(all(windows, target_arch = "x86_64"))]
#[unsafe(naked)]
extern "C" fn push_entry() {
    core::arch::naked_asm!("mov r8, rdi", "jmp {body}", body = sym push_body);
}

#[cfg(all(windows, target_arch = "x86_64"))]
fn install_push(resolved: &ResolvedProfile) -> String {
    let Some(site) = resolved.get(GET_PLAYER_PUSH) else {
        return format!(
            "{FIX}: the GUI's Lua getPlayer stays the game's natively: the profile lacks {GET_PLAYER_PUSH}"
        );
    };
    let at = site.address as usize;
    if !crate::image::readable(at, 5) {
        return format!("{FIX}: {GET_PLAYER_PUSH} is unreadable");
    }
    // SAFETY: five readable bytes of the call; only read.
    let call = unsafe { std::slice::from_raw_parts(at as *const u8, 5) };
    let rel = i32::from_le_bytes([call[1], call[2], call[3], call[4]]);
    let target = (at as isize + 5 + rel as isize) as usize;
    if call[0] != 0xE8 || !crate::image::readable(target, PUSH_BYTES.len()) {
        return format!("{FIX}: {GET_PLAYER_PUSH} at {at:#x} is not a call it reads");
    }
    // SAFETY: readable bytes of the call's target; only read.
    let bytes = unsafe { std::slice::from_raw_parts(target as *const u8, PUSH_BYTES.len()) };
    if bytes != PUSH_BYTES {
        return format!(
            "{FIX}: {GET_PLAYER_PUSH} at {at:#x} reaches {target:#x}, not the integer push"
        );
    }
    PUSH_ORIGINAL.store(target, Ordering::Release);
    // SAFETY: the call inside the getPlayer binding, which no Lua state runs
    // yet (installed before any world); install checks it is a 5-byte call
    // of `target`; push_entry keeps every argument and adds r8.
    match unsafe {
        tpf3mp_hookcore::detour::CallRedirect::install(
            at as *mut u8,
            target,
            push_entry as *const u8,
        )
    } {
        Ok(redirect) => {
            let _kept = std::mem::ManuallyDrop::new(redirect);
            format!(
                "{FIX}: the GUI's Lua getPlayer answers the player's company natively in a room, in every GUI state ({GET_PLAYER_PUSH} at {at:#x})"
            )
        }
        Err(error) => {
            PUSH_ORIGINAL.store(0, Ordering::Release);
            format!("{FIX}: the GUI's Lua getPlayer stays the game's natively: {error}")
        }
    }
}

#[cfg(not(all(windows, target_arch = "x86_64")))]
fn install_push(_resolved: &ResolvedProfile) -> String {
    format!("{FIX}: the GUI's Lua getPlayer stays the game's natively: Windows x86-64 only")
}

/// Splices every site unless [`ENV`] says no; the lines for hook.log.
pub fn install(resolved: &ResolvedProfile) -> Vec<String> {
    install_with(
        resolved,
        crate::ticks::wanted(std::env::var(ENV).ok().as_deref()),
    )
}

pub fn install_with(resolved: &ResolvedProfile, wanted: bool) -> Vec<String> {
    ON.store(false, Ordering::Release);
    if !wanted {
        return vec![format!(
            "{FIX}: off, {ENV} says so; the map's markers and overlays show the save's player's"
        )];
    }
    let mut lines = Vec::new();
    let mut spliced = 0;
    for (index, site) in SITES.iter().enumerate() {
        let Some(target) = resolved.get(site.name) else {
            lines.push(format!(
                "{FIX}: {} stays the save's player's: the profile lacks {}",
                site.what, site.name
            ));
            continue;
        };
        // SAFETY: a site the profile resolved in this build by a unique
        // signature, spliced while the game starts, before any view runs;
        // Splice::install compares the bytes again and refuses others; only
        // instruction boundaries no branch lands inside (tpfre, noted in
        // the profile); the hook changes one register the code after the
        // site reads as the player, or points at a copy of an owner.
        let installed = unsafe {
            Splice::install(
                target.address as usize as *mut u8,
                site.expected,
                site.expected.len(),
                HOOKS[index],
            )
        };
        match installed {
            Ok(splice) => {
                let _kept = std::mem::ManuallyDrop::new(splice);
                spliced += 1;
            }
            Err(error) => lines.push(format!(
                "{FIX}: {} stays the save's player's: {} at {:#x}: {error}",
                site.what, site.name, target.address
            )),
        }
    }
    let all = crate::ticks::wanted(std::env::var(ALL_ENV).ok().as_deref());
    ALL.store(all, Ordering::Release);
    lines.push(install_push(resolved));
    lines.push(install_lines(resolved));
    ON.store(
        spliced > 0
            || PUSH_ORIGINAL.load(Ordering::Acquire) != 0
            || LINES_ORIGINAL.load(Ordering::Acquire) != 0,
        Ordering::Release,
    );
    lines.insert(
        0,
        format!(
            "{FIX}: {spliced} of {} of the views' player reads see the player's company in a room ({ENV}=0 turns it off); the map shows {} ({ALL_ENV}=0 keeps it to the player's own)",
            SITES.len(),
            if all { "every company's icons and lines" } else { "the player's company's icons and lines only" }
        ),
    );
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_room_company_can_see_the_map_but_outside_rooms_stays_native() {
        assert_eq!(wanted(false, Some(214_443), Some(372_631)), None);
        assert_eq!(wanted(true, None, Some(372_631)), None);
        assert_eq!(wanted(true, Some(214_443), None), Some((214_443, 214_443)));
        assert_eq!(
            wanted(true, Some(214_443), Some(214_443)),
            Some((214_443, 214_443))
        );
        assert!(shown(372_631, 214_443, true, true));
        assert_eq!(value(214_443, 214_443, 214_443), 214_443);
        assert_eq!(owner(372_631, 214_443, 214_443), None);
        assert_eq!(
            wanted(true, Some(214_443), Some(372_631)),
            Some((372_631, 214_443))
        );
    }

    #[test]
    fn a_value_site_answers_the_company_for_the_saves_player_only() {
        // eax loaded with the save's player, the rest of rax cleared.
        assert_eq!(value(214_443, 372_631, 214_443), 372_631);
        // Garbage above a 32-bit load is not there; still the low half.
        assert_eq!(value(0xFFFF_FFFF_0003_45AB, 372_631, 214_443), 372_631);
        assert_eq!(value(5, 372_631, 214_443), 5, "another value is left");
        assert_eq!(value(214_443, -1, 214_443), 214_443, "not in a room");
    }

    #[test]
    fn an_owner_site_answers_as_the_company_would() {
        assert_eq!(owner(372_631, 372_631, 214_443), Some(true));
        assert_eq!(owner(214_443, 372_631, 214_443), Some(false));
        assert_eq!(owner(400_000, 372_631, 214_443), None);
        assert_eq!(owner(372_631, -1, 214_443), None);
    }

    #[test]
    fn every_site_is_whole_and_distinct() {
        for site in SITES {
            assert!((5..=16).contains(&site.expected.len()), "{}", site.name);
        }
        let mut names: Vec<&str> = SITES.iter().map(|s| s.name).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), SITES.len());
        // The owner sites take the read and the compare whole.
        for site in SITES.iter().filter(|s| s.kind == Kind::Owner) {
            assert_eq!(site.expected[0], 0x8B, "{}: a load", site.name);
            assert_eq!(
                &site.expected[2..6],
                &[0x0C, 0x02, 0x00, 0x00],
                "{}: of +0x20c",
                site.name
            );
            assert_eq!(site.expected[6], 0x39, "{}: then a compare", site.name);
        }
    }

    #[test]
    fn the_rooms_companies_are_read_from_their_note() {
        assert_eq!(companies(Some("372426,214443")), vec![372_426, 214_443]);
        assert_eq!(companies(Some(" 1, 1 ,2")), vec![1, 2]);
        assert_eq!(
            companies(Some("1,x")),
            Vec::<i64>::new(),
            "a bad note reads as none"
        );
        assert_eq!(companies(Some("-5")), Vec::<i64>::new());
        assert_eq!(companies(None), Vec::<i64>::new());
        assert_eq!(companies(Some("1,2,3,4,5,6,7,8,9")).len(), MAX_COMPANIES);
    }

    #[test]
    fn every_companys_icons_show_only_when_asked() {
        // Another company's station under the company's pass: shown.
        assert!(shown(214_443, 372_426, true, true));
        // Not a company of the room, or the switch off: the game's rule.
        assert!(!shown(999, 372_426, true, false));
        assert!(!shown(214_443, 372_426, false, true));
        // The pass's own player: the game shows it already.
        assert!(!shown(372_426, 372_426, true, true));
    }

    #[test]
    fn the_guis_getters_are_told_from_the_engines() {
        // CMenuUI::SwitchToGameUI's lambda_2 (rva 0x6aa800).
        let menu = [
            0x48, 0x8B, 0x41, 0x08, 0x48, 0x8B, 0x80, 0xB0, 0x06, 0x00, 0x00, 0x48, 0x8B, 0x80,
            0xE0, 0x01, 0x00, 0x00, 0xC3, 0xCC, 0xCC, 0xCC, 0xCC, 0xCC, 0xCC, 0xCC, 0xCC, 0xCC,
            0xCC, 0xCC, 0xCC,
        ];
        assert!(gui_getter(&menu));
        // ScriptComponentRoot::ReloadInterfaces's lambda_6 (rva 0x27c80a0).
        let react = [
            0x48, 0x83, 0xEC, 0x28, 0x48, 0x8B, 0x49, 0x40, 0x48, 0x85, 0xC9, 0x74, 0x12, 0x48,
            0x8B, 0x01, 0xFF, 0x50, 0x10, 0x48, 0x8B, 0x80, 0xE0, 0x01, 0x00, 0x00, 0x48, 0x83,
            0xC4, 0x28, 0xC3,
        ];
        assert!(gui_getter(&react));
        // CGame::CGame's lambda_1 (rva 0x11ffd0): the engine's buffers.
        let engine = [
            0x80, 0x79, 0x10, 0x00, 0x48, 0x8B, 0x41, 0x08, 0x48, 0x8B, 0x80, 0xF0, 0x01, 0x00,
            0x00, 0x74, 0x14, 0xB9, 0x01, 0x00, 0x00, 0x00, 0x2B, 0x88, 0x98, 0x00, 0x00, 0x00,
            0x48, 0x63, 0xD1,
        ];
        assert!(!gui_getter(&engine));
        let mut other = menu;
        other[14] = 0xF0;
        assert!(
            !gui_getter(&other),
            "another slot of the game is no GUI getter"
        );
    }

    #[test]
    fn the_line_viewers_get_every_companys_lines_in_a_room() {
        let companies = [214_443, 372_609];
        assert_eq!(
            line_players(false, true, &companies, 372_609),
            Vec::<i64>::new(),
            "outside a room: the game's"
        );
        assert_eq!(
            line_players(true, true, &companies, 372_609),
            vec![214_443, 372_609]
        );
        assert_eq!(
            line_players(true, true, &companies, -1),
            vec![214_443, 372_609],
            "playing for the first company: still every company's"
        );
        assert_eq!(
            line_players(true, false, &companies, 372_609),
            vec![372_609],
            "the switch off: the player's own"
        );
        assert_eq!(line_players(true, false, &companies, -1), Vec::<i64>::new());
        assert_eq!(line_players(true, true, &[], 372_609), vec![372_609]);
        assert_eq!(
            LINES_CALLEE_BYTES[..4],
            [0x89, 0x54, 0x24, 0x10],
            "mov [rsp+0x10],edx"
        );
    }

    #[test]
    fn the_store_depot_tests_answer_only_on_the_guis_thread() {
        assert!(on_gui_thread(7, 7, false));
        assert!(
            !on_gui_thread(7, 7, true),
            "inside the simulation's step: the game's"
        );
        assert!(!on_gui_thread(7, 8, false), "another thread: the game's");
        assert!(
            !on_gui_thread(0, 0, false),
            "the GUI's thread not known yet: the game's"
        );
        // Both are owner tests: a load of the player and its compare.
        for site in &SITES[GUI_THREAD_ONLY_FROM..] {
            assert_eq!(site.kind, Kind::Owner);
            assert_eq!(site.reg, Reg::Rax);
            assert_eq!(site.expected[6], 0x39, "{}: then a compare", site.name);
        }
        // A depot of the player's company is the player's; the first
        // company's is not.
        assert_eq!(owner(372_609, 372_609, 214_443), Some(true));
        assert_eq!(owner(214_443, 372_609, 214_443), Some(false));
    }

    #[test]
    fn off_unless_wanted() {
        let empty = ResolvedProfile {
            name: String::new(),
            targets: Vec::new(),
            absent_optional: Vec::new(),
        };
        assert!(install_with(&empty, false)[0].contains("off"));
        let lines = install_with(&empty, true);
        assert!(lines[0].starts_with(&format!("{FIX}: 0 of 19")));
        refresh();
        assert_eq!(COMPANY.load(Ordering::Relaxed), -1);
    }
}
