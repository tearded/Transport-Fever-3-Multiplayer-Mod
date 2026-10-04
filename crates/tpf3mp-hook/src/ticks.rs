//! The game's frame counter in the room's game: `GameTime.tickCount` kept
//! equal in every game of a room (docs/HOOKS.md, "Seeds, as built";
//! investigation/TPF3_TRAIN_PRIORITY_2026-09-30.md).
//!
//! TF3's `GameTime` component has two counters. `updateCount` (+0x40)
//! counts simulation updates. `tickCount` (+0x3c) counts them too, and it
//! also counts once per call of the game's step on its **paused path**
//! (the API says so: "ticks also when the game is paused (once per
//! frame)"). `GameSim::Step` (0x159390) takes that path when its speed
//! call answers 0, and there it calls the GameTime advance (0xbace10) with
//! `r8b = 0` at 0x159412: the advance always runs `inc [GameTime+0x3c]` and
//! runs `inc [GameTime+0x40]` only when `r8b` is set.
//!
//! The room's game takes the paused path whenever the room holds the
//! world: the room is paused, a player is behind, or the game loads or
//! saves the room's world. How many such calls a game makes depends on its
//! own frame pacing and on how long its load took, so every hold leaves the
//! games' `tickCount` a different number apart, for good. The simulation
//! reads `tickCount` in the land-vehicle reservation shuffle's seed (which
//! train or road vehicle gets contested track first), in
//! `AccountSystem::Update2` (`tickCount % n`), in the town developer's and
//! street proposals' stamps, and in the base game's notifications script.
//! TPF2 Multiplayer found the same counter and made the paused call a NOP
//! (`156824d`, `pausedtick`).
//!
//! Here the paused call is redirected, checked, to [`paused_advance`]: in
//! the room's game it skips the advance, so a held world's `tickCount`
//! stands still like the rest of it; anywhere else it calls the game's own
//! advance, so a game outside a room pauses exactly as it always did. The
//! running loop's call of the same advance (0x15954b, `r8b = 1`) is not
//! touched: every update the room releases counts on every game.
//!
//! What skipping drops, checked in the binary: the advance opens an engine
//! modification scope around the two increments and records a
//! `ComponentChanged` for `GameTime` (0x2bb6b50, into each observer's change
//! log). Skipped, a held frame records no change of `GameTime`, which is
//! true: nothing changed. The readers of `tickCount` that see the
//! difference are the UI's: the notifications script refreshes one of its
//! four type groups per paused frame by `tickCount % 4`, and the industry
//! window rate-limits its expansion preview by `tickCount`, so while the
//! room holds, those stand still too. The town and street builder tools
//! stamp their proposals with `tickCount`; their proposals travel to the
//! room in the action's bytes, so every game applies the same stamp.
//!
//! [`TOGGLE_ENV`] set to `0` leaves the call alone (the kill switch).
//!
//! At every checkpoint the step detour logs both counters with the room's
//! step ([`checkpoint_line`]); two games' lines must be equal.

#![allow(unsafe_code)]
// Elsewhere the redirect is not installed, so its code is unused there.
#![cfg_attr(not(all(windows, target_arch = "x86_64")), allow(dead_code))]

use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};

use tpf3mp_hookcore::profile::ResolvedProfile;

use crate::log;

/// The fix's name in hook.log.
pub const FIX: &str = "paused-tick";
pub use crate::build_data::native::ticks::ADVANCE;
pub use crate::build_data::native::ticks::ADVANCE_TICK;
pub use crate::build_data::native::ticks::GET_TICK_COUNT;
pub use crate::build_data::native::ticks::GET_UPDATE_COUNT;
pub use crate::build_data::native::ticks::SITE;
pub use crate::build_data::native::ticks::TICK_BYTES;
pub use crate::build_data::native::ticks::TICK_OFFSET;
/// Set to `0` (or `off`, `false`) in the game's environment, the paused
/// call is left as the game has it.
pub const TOGGLE_ENV: &str = "TPF3MP_HOOK_PAUSED_TICK";

/// The advance's address; 0 until installed.
static ADVANCE_AT: AtomicUsize = AtomicUsize::new(0);
/// The getters' addresses; 0 when the profile lacks them.
static TICK_GETTER: AtomicUsize = AtomicUsize::new(0);
static UPDATE_GETTER: AtomicUsize = AtomicUsize::new(0);
/// Set by the step detour for each call of the game's step: this call is
/// the room's game's.
static ROOM: AtomicBool = AtomicBool::new(false);
/// Paused advances skipped, and paused advances passed on to the game.
static HELD: AtomicU64 = AtomicU64::new(0);
static PASSED: AtomicU64 = AtomicU64::new(0);

/// What one paused call of the advance does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Advance {
    /// The game's own advance runs: `tickCount` counts the frame.
    Run,
    /// Skipped: the held world's `tickCount` stands still.
    Hold,
}

/// The decision, pure: only a paused frame (`update` false, as the site
/// always passes) of the room's game is held; an update, or any frame
/// outside a room, runs as the game has it.
pub fn decide(room: bool, update: bool) -> Advance {
    if room && !update {
        Advance::Hold
    } else {
        Advance::Run
    }
}

/// Whether the fix is wanted, from [`TOGGLE_ENV`]'s value: on unless it
/// says `0`, `off` or `false`.
pub fn wanted(value: Option<&str>) -> bool {
    !matches!(
        value.map(|v| v.trim().to_ascii_lowercase()).as_deref(),
        Some("0" | "off" | "false" | "no")
    )
}

/// From the step detour, before each call of the game's step: whether this
/// call is the room's game's.
pub fn set_room(room: bool) {
    ROOM.store(room, Ordering::Release);
}

/// The advance's signature, passed through as the call site uses it.
type AdvanceFn = unsafe extern "system" fn(usize, usize, usize, usize);

/// Where the paused path's call goes: in the room's game nothing; otherwise
/// the game's own advance with the arguments the step passed.
unsafe extern "system" fn paused_advance(engine: usize, entity: usize, update: usize, r9: usize) {
    // The redirect's own work: the game's advance, when passed on, is not
    // the hook's.
    let timer = crate::perf::time(crate::perf::Piece::PausedTick);
    let decision = decide(ROOM.load(Ordering::Acquire), update & 0xff != 0);
    let advance = ADVANCE_AT.load(Ordering::Acquire);
    if decision == Advance::Hold || advance == 0 {
        let held = HELD.fetch_add(1, Ordering::Relaxed) + 1;
        if held == 1 || held.is_multiple_of(1 << 14) {
            log::line(&format!(
                "{FIX}: holding tickCount on the room's paused frames (held {held}, passed {})",
                PASSED.load(Ordering::Relaxed)
            ));
        }
        return;
    }
    PASSED.fetch_add(1, Ordering::Relaxed);
    drop(timer);
    // SAFETY: the advance the profile resolved, and the redirect's install
    // checked the call reached, called with the registers the step set up.
    let advance: AdvanceFn = unsafe { std::mem::transmute::<usize, AdvanceFn>(advance) };
    unsafe { advance(engine, entity, update, r9) };
}

/// The two counters, as the game has them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Counters {
    pub tick_count: u32,
    pub update_count: u32,
}

/// The checkpoint's line: the room's step and both counters, and nothing
/// that differs between two games that agree.
pub fn checkpoint_line(step: u64, counters: Result<Counters, &str>) -> String {
    match counters {
        Ok(c) => format!(
            "ticks: step {step}: tickCount={} updateCount={}",
            c.tick_count, c.update_count
        ),
        Err(why) => format!("ticks: step {step}: counters unread ({why})"),
    }
}

/// Reads both counters through the game's getters, from the `CGameTime`
/// the step's speed call was made on. Every read is checked first.
pub fn read_counters(game_time: usize) -> Result<Counters, &'static str> {
    let tick = TICK_GETTER.load(Ordering::Acquire);
    let update = UPDATE_GETTER.load(Ordering::Acquire);
    if tick == 0 || update == 0 {
        return Err("the profile lacks the GameTime getters");
    }
    if game_time == 0 || !crate::image::readable(game_time, 0x18) {
        return Err("no CGameTime this call");
    }
    // SAFETY: 0x18 readable bytes at `game_time`, checked above.
    let engine = unsafe { std::ptr::read_unaligned((game_time + 8) as *const usize) };
    let entity = unsafe { std::ptr::read_unaligned((game_time + 0x10) as *const usize) };
    if engine == 0 || !crate::image::readable(entity, 4) {
        return Err("the CGameTime names no engine or entity");
    }
    type Getter = unsafe extern "system" fn(usize) -> u32;
    // SAFETY: the getters the profile resolved, each `int (CGameTime*)`,
    // called on the game's thread with the object the game's own step
    // called its speed getter on, whose engine and entity were checked.
    let (tick_count, update_count) = unsafe {
        (
            std::mem::transmute::<usize, Getter>(tick)(game_time),
            std::mem::transmute::<usize, Getter>(update)(game_time),
        )
    };
    Ok(Counters {
        tick_count,
        update_count,
    })
}

/// Installs the redirect of the paused call where the profile has every
/// piece and its bytes are what the fix expects; otherwise leaves the call
/// alone and says why. Returns the line for hook.log.
pub fn install(resolved: &ResolvedProfile) -> String {
    install_with(resolved, wanted(std::env::var(TOGGLE_ENV).ok().as_deref()))
}

pub fn install_with(resolved: &ResolvedProfile, wanted: bool) -> String {
    // The getters serve the checkpoint line whether or not the fix is on:
    // the line is how two games show they agree, or that they do not.
    if let (Some(tick), Some(update)) =
        (resolved.get(GET_TICK_COUNT), resolved.get(GET_UPDATE_COUNT))
    {
        TICK_GETTER.store(tick.address as usize, Ordering::Release);
        UPDATE_GETTER.store(update.address as usize, Ordering::Release);
    }
    let off = |why: String| {
        format!("{FIX} fix: off, {why}; tickCount counts the room's paused frames as the game does")
    };
    if !wanted {
        return off(format!("{TOGGLE_ENV} says so"));
    }
    let (site, advance, tick) = match (
        resolved.get(SITE),
        resolved.get(ADVANCE),
        resolved.get(ADVANCE_TICK),
    ) {
        (Some(site), Some(advance), Some(tick)) => (site, advance, tick),
        (site, advance, _) => {
            let missing = if site.is_none() {
                SITE
            } else if advance.is_none() {
                ADVANCE
            } else {
                ADVANCE_TICK
            };
            return off(format!("the profile has no {missing:?}"));
        }
    };
    if let Err(why) = check_layout(site.address, advance.address, tick.address) {
        return off(why);
    }
    // The increments, read again from the running game: the advance counts
    // tickCount always and updateCount only on an update.
    let len = TICK_BYTES.len();
    if !crate::image::readable(tick.address as usize, len) {
        return off(format!(
            "{ADVANCE_TICK} at {:#x} is unreadable",
            tick.address
        ));
    }
    // SAFETY: `len` readable bytes, checked just above.
    let found = unsafe { std::slice::from_raw_parts(tick.address as usize as *const u8, len) };
    if found != TICK_BYTES {
        return off(format!(
            "{ADVANCE_TICK} at {:#x} is not the two increments the fix expects",
            tick.address
        ));
    }
    ADVANCE_AT.store(advance.address as usize, Ordering::Release);
    // SAFETY: the site is a call inside `GameSim::Step`, which no thread runs
    // yet (the hook installs before any world exists); install checks it is
    // a 5-byte call of the advance and patches only its displacement;
    // `paused_advance` has the advance's ABI (four register arguments passed
    // through, nothing returned).
    match unsafe {
        tpf3mp_hookcore::detour::CallRedirect::install(
            site.address as usize as *mut u8,
            advance.address as usize,
            paused_advance as *const u8,
        )
    } {
        Ok(redirect) => {
            // For the life of the game: never dropped, so never restored.
            let _kept = std::mem::ManuallyDrop::new(redirect);
            format!(
                "{FIX} fix: installed (at {:#x}, the room's paused frames leave tickCount alone; outside a room the game's advance runs)",
                site.address
            )
        }
        Err(error) => {
            ADVANCE_AT.store(0, Ordering::Release);
            off(format!("the call at {:#x}: {error}", site.address))
        }
    }
}

/// The three targets must be where the one build the fix was read on has
/// them relative to each other: the increments inside the advance, and the
/// call site far from both (another function).
pub fn check_layout(site: u64, advance: u64, tick: u64) -> Result<(), String> {
    if tick.checked_sub(advance) != Some(TICK_OFFSET) {
        return Err(format!(
            "{ADVANCE_TICK} at {tick:#x} is not {TICK_OFFSET:#x} into {ADVANCE} at {advance:#x}"
        ));
    }
    if (advance..=tick).contains(&site) {
        return Err(format!("{SITE} at {site:#x} lies inside {ADVANCE}"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_rooms_paused_frames_are_held() {
        assert_eq!(decide(true, false), Advance::Hold);
        assert_eq!(decide(true, true), Advance::Run, "an update always counts");
        assert_eq!(
            decide(false, false),
            Advance::Run,
            "outside a room, the game's own"
        );
        assert_eq!(decide(false, true), Advance::Run);
    }

    #[test]
    fn the_kill_switch_reads_the_environment() {
        assert!(wanted(None));
        assert!(wanted(Some("")));
        assert!(wanted(Some("1")));
        for off in ["0", "off", " OFF ", "false", "no"] {
            assert!(!wanted(Some(off)), "{off}");
        }
    }

    #[test]
    fn the_layout_is_the_release_builds() {
        assert_eq!(check_layout(0x159412, 0xbace10, 0xbace99), Ok(()));
        assert!(check_layout(0x159412, 0xbace10, 0xbace9a).is_err());
        assert!(check_layout(0x159412, 0xbace99, 0xbace10).is_err());
        assert!(check_layout(0xbace20, 0xbace10, 0xbace99).is_err());
    }

    #[test]
    fn the_checkpoint_line_holds_only_what_two_agreeing_games_share() {
        let line = checkpoint_line(
            1200,
            Ok(Counters {
                tick_count: 81_234,
                update_count: 80_000,
            }),
        );
        assert_eq!(line, "ticks: step 1200: tickCount=81234 updateCount=80000");
        assert_eq!(
            checkpoint_line(7, Err("no CGameTime this call")),
            "ticks: step 7: counters unread (no CGameTime this call)"
        );
    }

    #[test]
    fn nothing_installs_without_the_targets_or_when_switched_off() {
        let resolved = ResolvedProfile {
            name: "empty".into(),
            targets: Vec::new(),
            absent_optional: Vec::new(),
        };
        let line = install_with(&resolved, true);
        assert!(line.starts_with("paused-tick fix: off"), "{line}");
        assert!(line.contains("the profile has no"), "{line}");
        let line = install_with(&resolved, false);
        assert!(line.contains(TOGGLE_ENV), "{line}");
        assert_eq!(
            read_counters(0),
            Err("the profile lacks the GameTime getters")
        );
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
        assert_eq!(target(SITE).prologue, vec![0xE8], "the call itself");
        assert_eq!(target(ADVANCE_TICK).prologue, TICK_BYTES.to_vec());
        for name in [ADVANCE, GET_TICK_COUNT, GET_UPDATE_COUNT] {
            assert!(!target(name).prologue.is_empty(), "{name}");
            assert!(!target(name).required, "{name} is optional");
        }
    }
}

/// The redirect through a real call: a hand-written caller calls a
/// hand-written "advance" that bumps a counter, and the redirect holds it
/// only while the room says so.
#[cfg(all(test, windows, target_arch = "x86_64"))]
mod redirect_tests {
    use super::*;

    fn page(code: &[u8]) -> usize {
        use windows_sys::Win32::System::Memory::{
            MEM_COMMIT, MEM_RESERVE, PAGE_EXECUTE_READWRITE, VirtualAlloc,
        };
        // SAFETY: a fresh read-write-execute page for the test's own code.
        let page = unsafe {
            VirtualAlloc(
                std::ptr::null(),
                0x1000,
                MEM_COMMIT | MEM_RESERVE,
                PAGE_EXECUTE_READWRITE,
            )
        };
        assert!(!page.is_null());
        // SAFETY: `code.len()` bytes into a 4 KiB writable page.
        unsafe { std::ptr::copy_nonoverlapping(code.as_ptr(), page.cast::<u8>(), code.len()) };
        page as usize
    }

    #[test]
    fn the_redirected_call_skips_the_advance_only_in_the_room() {
        // The step detour's tests set the room flag too.
        let _serial = crate::lua::tests::SERIAL
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        // The "advance" at +0x40: inc dword [rcx]; ret. The caller at +0:
        // sub rsp,0x28; xor r8d,r8d; call +0x40; add rsp,0x28; ret.
        let mut code = vec![0u8; 0x60];
        let caller = [0x48, 0x83, 0xEC, 0x28, 0x45, 0x33, 0xC0];
        code[..caller.len()].copy_from_slice(&caller);
        let call_at = caller.len();
        let rel = (0x40 - (call_at + 5)) as i32;
        code[call_at] = 0xE8;
        code[call_at + 1..call_at + 5].copy_from_slice(&rel.to_le_bytes());
        code[call_at + 5..call_at + 10].copy_from_slice(&[0x48, 0x83, 0xC4, 0x28, 0xC3]);
        code[0x40..0x43].copy_from_slice(&[0xFF, 0x01, 0xC3]);
        let base = page(&code);
        // SAFETY: the page holds a function of one pointer argument.
        let run: extern "C" fn(*mut u32) =
            unsafe { std::mem::transmute::<usize, extern "C" fn(*mut u32)>(base) };
        let mut ticks = 0u32;
        run(&mut ticks);
        assert_eq!(ticks, 1, "the advance runs before the redirect");

        ADVANCE_AT.store(base + 0x40, Ordering::Release);
        // SAFETY: the test's own call, not running now.
        let redirect = unsafe {
            tpf3mp_hookcore::detour::CallRedirect::install(
                (base + call_at) as *mut u8,
                base + 0x40,
                paused_advance as *const u8,
            )
        }
        .unwrap();
        set_room(false);
        run(&mut ticks);
        assert_eq!(ticks, 2, "outside a room the game's advance runs");
        set_room(true);
        run(&mut ticks);
        run(&mut ticks);
        assert_eq!(ticks, 2, "the room's paused frames are held");
        set_room(false);
        run(&mut ticks);
        assert_eq!(ticks, 3);
        // SAFETY: nothing runs the fixture now.
        unsafe { redirect.detach() }.unwrap();
        ADVANCE_AT.store(0, Ordering::Release);
    }
}
