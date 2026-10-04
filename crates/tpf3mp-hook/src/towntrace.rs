//! The town trace (logging only; it changes nothing), off unless
//! [`ENV`] is `1` or `on` (docs/HOOKS.md, "The town trace").
//!
//! Round of 2026-10-02 on `twomptest`: one town street (a `town_old_small`
//! dead end) was built at another angle in one of three games between
//! steps 12750 and 12800, with `tickCount` and `updateCount` equal in all
//! three, so the town developer's seed was equal too. The trace says, for
//! every town size update the simulation applies, what went into the
//! developer and what came out, so two games' logs show which input split:
//!
//! - `TownUpdateSize::Apply` (`0x9dfb10`, our name; the `TownUpdateSize`
//!   command's applier, `rcx` its context, whose `GameState` is at `+8`,
//!   `rdx` the command: the town at `+0`, three size factors at `+4`, the
//!   flag at `+0x10`) writes the factors into the town, seeds a
//!   `minstd_rand` from an FNV-1a of `updateCount` ([`town_seed`]) and calls
//!   `TownDeveloper::Develop` (`0x8dc240`) with `GameState+0x200` (the
//!   developer's context, which `GameState::Replicate` does not copy) and
//!   the generator. Two splices: right before the call is set up
//!   (`+0x17a`, the seed in `ecx`) and at the return (`+0x1c7`, the
//!   generator's state after `Develop` still in the applier's frame). One
//!   line a call, from the second:
//!
//!   ```text
//!   town: step <s> update <n> town <e> size <hex>,<hex>,<hex> (<f>,<f>,<f>) flag <0|1> seed <n> (expected <n>) gamestate <ptr> buffer <0|1> engine <ptr> developer <ptr> developer-engine <own|other|ptr> gen-after <n> entity-ids <before>-><after>
//!   ```
//!
//! - `TownDeveloper::Develop`'s entry, from every caller (the applier, and
//!   the town system's creation paths):
//!
//!   ```text
//!   town: step <s> develop from +<rva> town <e> flag <0|1> gen <n> developer <ptr> engine <ptr>
//!   ```
//!
//! `step` is `-` outside a room's released update; `update` is `?` when
//! the counter could not be read. The pointers differ between games by
//! nature; `buffer` numbers the `GameState`s in the order this game first
//! saw them. `developer-engine` is the engine the developer's context names
//! at `+0xb0` (`Develop` reads its town there): this update's engine, the
//! other buffer's, or another. `gen-after` is the generator after `Develop`:
//! equal seeds and unequal `gen-after` mean `Develop` drew a different
//! number of times, so its input differed. `entity-ids` is the length of the
//! engine's entity table before and after.
//!
//! Before installing, the sites must lie at their offsets in the applier,
//! the call between them must be a call (of `Develop`, when the profile
//! has it), and every site's bytes must be the expected ones; otherwise
//! nothing is spliced and the log says why. A panic switches it off.

#![allow(unsafe_code)]
#![cfg_attr(not(all(windows, target_arch = "x86_64")), allow(dead_code))]

use std::cell::Cell;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use tpf3mp_hookcore::detour::{SavedRegs, Splice, SpliceHook};
use tpf3mp_hookcore::profile::ResolvedProfile;

use crate::image::Readable as Probe;
use crate::log;

/// `1` (or `on`) turns the trace on, and dumps the towns lane at every
/// checkpoint (`crate::lanedump::Setting::with_town_trace`).
pub const ENV: &str = "TPF3MP_HOOK_TOWN_TRACE";

pub use crate::build_data::native::towntrace::APPLY;
pub use crate::build_data::native::towntrace::DEVELOP;
pub use crate::build_data::native::towntrace::DEVELOP_SITE;
pub use crate::build_data::native::towntrace::RETURN_SITE;

pub use crate::build_data::native::towntrace::DEVELOP_CALL_AT;
pub use crate::build_data::native::towntrace::DEVELOP_SITE_AT;
pub use crate::build_data::native::towntrace::RETURN_SITE_AT;

pub use crate::build_data::native::towntrace::DEVELOP_EXPECTED;
pub use crate::build_data::native::towntrace::DEVELOP_SITE_EXPECTED;
pub use crate::build_data::native::towntrace::DEVELOP_SITE_STEAL;
pub use crate::build_data::native::towntrace::DEVELOP_STEAL;
pub use crate::build_data::native::towntrace::RETURN_EXPECTED;
pub use crate::build_data::native::towntrace::RETURN_STEAL;

use crate::build_data::native::towntrace::CMD_LEN;
use crate::build_data::native::towntrace::DEVELOP_GENERATOR_ARG;
use crate::build_data::native::towntrace::DEVELOPER;
use crate::build_data::native::towntrace::DEVELOPER_ENGINE;
use crate::build_data::native::towntrace::GAME_STATE;
use crate::build_data::native::towntrace::GAME_TIME;
use crate::build_data::native::towntrace::GENERATOR;

static ON: AtomicBool = AtomicBool::new(false);
static BROKEN: AtomicBool = AtomicBool::new(false);
static BASE: AtomicU64 = AtomicU64::new(0);
/// The `GameState`s seen, in order, with their engines.
static BUFFERS: Mutex<[(u64, u64); 2]> = Mutex::new([(0, 0); 2]);

/// Whether `value` (of [`ENV`]) turns the trace on.
pub fn wanted(value: Option<&str>) -> bool {
    matches!(value.map(str::trim), Some("1" | "on"))
}

/// The town developer's seed as `TownUpdateSize::Apply` makes it
/// (`0x9dfbf0..0x9dfc83`): FNV-1a over `updateCount`'s four bytes, plus and
/// xor two constants, its low 32 bits modulo 2^31-1, 0 made 1 (a
/// `minstd_rand`'s seed).
pub fn town_seed(update_count: u32) -> u32 {
    let mut hash = crate::order::Fnv1a::new();
    hash.write_u32(update_count);
    let mixed = hash.0.wrapping_add(0x45f1_6db3_af76_9df3) ^ 0x2cea_dadb_f054_a7e9;
    match (mixed as u32) % 0x7fff_ffff {
        0 => 1,
        seed => seed,
    }
}

/// What the first splice read, for the second.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Before {
    pub step: Option<u64>,
    pub update: Option<u32>,
    pub town: i32,
    pub size: [u32; 3],
    pub flag: u8,
    pub seed: u32,
    pub game_state: u64,
    pub buffer: u8,
    pub engine: u64,
    pub developer: u64,
    /// The engine the developer names: `Ok(own)`, `Err(other buffer)` or
    /// the pointer.
    pub developer_engine: DeveloperEngine,
    pub ids: Option<u64>,
}

/// Which engine the developer's context names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeveloperEngine {
    Own,
    Other(u8),
    Unknown(Option<u64>),
}

impl std::fmt::Display for DeveloperEngine {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Own => write!(f, "own"),
            Self::Other(buffer) => write!(f, "other(buffer {buffer})"),
            Self::Unknown(Some(at)) => write!(f, "{at:#x}"),
            Self::Unknown(None) => write!(f, "?"),
        }
    }
}

fn opt<T: std::fmt::Display>(value: Option<T>, none: &str) -> String {
    value.map_or_else(|| none.to_owned(), |v| v.to_string())
}

/// The applier's line.
pub fn apply_line(before: &Before, gen_after: Option<u32>, ids_after: Option<u64>) -> String {
    let [a, b, c] = before.size;
    format!(
        "town: step {} update {} town {} size {a:08x},{b:08x},{c:08x} ({:?},{:?},{:?}) flag {} seed {} (expected {}) gamestate {:#x} buffer {} engine {:#x} developer {:#x} developer-engine {} gen-after {} entity-ids {}->{}",
        opt(before.step, "-"),
        opt(before.update, "?"),
        before.town,
        f32::from_bits(a),
        f32::from_bits(b),
        f32::from_bits(c),
        before.flag,
        before.seed,
        opt(before.update.map(town_seed), "?"),
        before.game_state,
        before.buffer,
        before.engine,
        before.developer,
        before.developer_engine,
        opt(gen_after, "?"),
        opt(before.ids, "?"),
        opt(ids_after, "?"),
    )
}

/// `Develop`'s entry line.
#[allow(clippy::too_many_arguments)]
pub fn develop_line(
    step: Option<u64>,
    caller: Option<u64>,
    town: i32,
    flag: u8,
    generator: Option<u32>,
    developer: u64,
    engine: u64,
) -> String {
    format!(
        "town: step {} develop from {} town {town} flag {flag} gen {} developer {developer:#x} engine {engine:#x}",
        opt(step, "-"),
        caller.map_or_else(|| "?".to_owned(), |rva| format!("+{rva:#x}")),
        opt(generator, "?"),
    )
}

/// The number of `game_state` (0 or 1), numbering it if new; a third one
/// starts the numbering again (a world loaded).
fn buffer_of(buffers: &mut [(u64, u64); 2], game_state: u64, engine: u64) -> u8 {
    if let Some(i) = buffers.iter().position(|(gs, _)| *gs == game_state) {
        buffers[i].1 = engine;
        return i as u8;
    }
    if let Some(i) = buffers.iter().position(|(gs, _)| *gs == 0) {
        buffers[i] = (game_state, engine);
        return i as u8;
    }
    *buffers = [(game_state, engine), (0, 0)];
    0
}

/// Which engine `named` is, for the update running on `buffer`.
pub fn classify(buffers: &[(u64, u64); 2], buffer: u8, named: Option<u64>) -> DeveloperEngine {
    let Some(named) = named else {
        return DeveloperEngine::Unknown(None);
    };
    for (i, (gs, engine)) in buffers.iter().enumerate() {
        if *gs != 0 && *engine == named {
            return if i as u8 == buffer {
                DeveloperEngine::Own
            } else {
                DeveloperEngine::Other(i as u8)
            };
        }
    }
    DeveloperEngine::Unknown(Some(named))
}

/// The length of `engine`'s entity table (24-byte entries from
/// `[engine+0x90]` to `[engine+0x98]`, `GetComponentDataIndex`'s reads).
pub(crate) fn entity_ids(probe: &mut Probe, engine: u64) -> Option<u64> {
    let begin = probe.read::<u64>(engine.checked_add(0x90)?)?;
    let end = probe.read::<u64>(engine.checked_add(0x98)?)?;
    (end >= begin && (end - begin) % 24 == 0).then(|| (end - begin) / 24)
}

thread_local! {
    static PENDING: Cell<Option<Before>> = const { Cell::new(None) };
}

fn guarded(body: impl FnOnce()) {
    if BROKEN.load(Ordering::Acquire) || !ON.load(Ordering::Acquire) {
        return;
    }
    if std::panic::catch_unwind(std::panic::AssertUnwindSafe(body)).is_err() {
        BROKEN.store(true, Ordering::Release);
        log::line("town trace: panicked on the game's thread; switched off for this game");
    }
}

/// Right before the applier sets up `Develop`'s call: `rbp` the context,
/// `r15` the command, `r14` the engine, `ecx` the seed.
unsafe extern "system" fn develop_site_hook(regs: *mut SavedRegs) {
    guarded(|| {
        // SAFETY: the stub's block, held until the hook returns.
        let regs = unsafe { &*regs };
        PENDING.with(|p| p.set(None));
        let mut probe = Probe::new();
        let cmd = regs.r15;
        if !probe.readable(cmd as usize, CMD_LEN as usize) {
            return;
        }
        let (Some(town), Some(a), Some(b), Some(c), Some(flag)) = (
            probe.read::<i32>(cmd),
            probe.read::<u32>(cmd + 4),
            probe.read::<u32>(cmd + 8),
            probe.read::<u32>(cmd + 0xc),
            probe.read::<u8>(cmd + 0x10),
        ) else {
            return;
        };
        let Some(game_state) = probe.read::<u64>(regs.rbp.wrapping_add(GAME_STATE)) else {
            return;
        };
        let developer = probe
            .read::<u64>(game_state.wrapping_add(DEVELOPER))
            .unwrap_or(0);
        let named = (developer != 0)
            .then(|| probe.read::<u64>(developer.wrapping_add(DEVELOPER_ENGINE)))
            .flatten();
        let update = probe
            .read::<u64>(game_state.wrapping_add(GAME_TIME))
            .and_then(|gt| crate::ticks::read_counters(gt as usize).ok())
            .map(|c| c.update_count);
        let engine = regs.r14;
        let (buffer, developer_engine) = {
            let mut buffers = BUFFERS.lock().unwrap_or_else(|p| p.into_inner());
            let buffer = buffer_of(&mut buffers, game_state, engine);
            (buffer, classify(&buffers, buffer, named))
        };
        let before = Before {
            step: crate::seeds::current_step(),
            update,
            town,
            size: [a, b, c],
            flag,
            seed: regs.rcx as u32,
            game_state,
            buffer,
            engine,
            developer,
            developer_engine,
            ids: entity_ids(&mut probe, engine),
        };
        PENDING.with(|p| p.set(Some(before)));
    });
}

/// At the applier's return: the generator after `Develop` at `[rsp+0x90]`.
unsafe extern "system" fn return_hook(regs: *mut SavedRegs) {
    guarded(|| {
        let rsp = SavedRegs::rsp(regs);
        let Some(before) = PENDING.with(|p| p.take()) else {
            return;
        };
        let mut probe = Probe::new();
        let gen_after = probe.read::<u32>(rsp + GENERATOR);
        let ids_after = entity_ids(&mut probe, before.engine);
        log::line(&apply_line(&before, gen_after, ids_after));
    });
}

/// `Develop`'s entry: `rcx` the developer's context, `rdx` the engine,
/// `r8d` the town, `r9b` the flag, the generator's address at
/// `[rsp+0x28]`, the return address at `[rsp]`.
unsafe extern "system" fn develop_hook(regs: *mut SavedRegs) {
    guarded(|| {
        let rsp = SavedRegs::rsp(regs);
        // SAFETY: the stub's block, held until the hook returns.
        let regs = unsafe { &*regs };
        let mut probe = Probe::new();
        let base = BASE.load(Ordering::Relaxed);
        let caller = probe
            .read::<u64>(rsp)
            .and_then(|ret| ret.checked_sub(base))
            .and_then(|rva| rva.checked_sub(5));
        let generator = probe
            .read::<u64>(rsp + DEVELOP_GENERATOR_ARG)
            .and_then(|at| probe.read::<u32>(at));
        log::line(&develop_line(
            crate::seeds::current_step(),
            caller,
            regs.r8 as i32,
            regs.r9 as u8,
            generator,
            regs.rcx,
            regs.rdx,
        ));
    });
}

/// The sites' layout in the applier: each at its offset, and the call
/// between them a call (`call_bytes`, its five bytes) of `develop` when
/// the profile has it.
pub fn check_layout(
    apply: u64,
    develop_site: u64,
    return_site: u64,
    call_bytes: Option<[u8; 5]>,
    develop: Option<u64>,
) -> Result<(), String> {
    if develop_site != apply.wrapping_add(DEVELOP_SITE_AT) {
        return Err(format!(
            "{DEVELOP_SITE} at {develop_site:#x} is not {APPLY}+{DEVELOP_SITE_AT:#x}"
        ));
    }
    if return_site != apply.wrapping_add(RETURN_SITE_AT) {
        return Err(format!(
            "{RETURN_SITE} at {return_site:#x} is not {APPLY}+{RETURN_SITE_AT:#x}"
        ));
    }
    let call_at = apply.wrapping_add(DEVELOP_CALL_AT);
    let Some(bytes) = call_bytes.filter(|b| b[0] == 0xE8) else {
        return Err(format!("{APPLY}+{DEVELOP_CALL_AT:#x} is not a call"));
    };
    let rel = i32::from_le_bytes([bytes[1], bytes[2], bytes[3], bytes[4]]);
    let target = call_at.wrapping_add(5).wrapping_add_signed(i64::from(rel));
    match develop {
        Some(develop) if develop != target => Err(format!(
            "{APPLY}+{DEVELOP_CALL_AT:#x} calls {target:#x}, not {DEVELOP} at {develop:#x}"
        )),
        _ => Ok(()),
    }
}

fn splice(at: u64, expected: &[u8], steal: usize, hook: SpliceHook) -> Result<Splice, String> {
    // SAFETY: a site the profile resolved and the layout check placed in
    // its function, installed before any world exists; nothing branches
    // into the stolen bytes past the first (tpfre, noted in the profile);
    // the hooks only read and never unwind (`guarded`).
    unsafe { Splice::install(at as usize as *mut u8, expected, steal, hook) }
        .map_err(|e| format!("{at:#x}: {e}"))
}

/// Installs the trace when [`ENV`] asks for it; `base` the game's image.
/// Returns the lines for hook.log.
pub fn install(resolved: &ResolvedProfile, base: u64) -> Vec<String> {
    install_with(resolved, base, wanted(std::env::var(ENV).ok().as_deref()))
}

pub fn install_with(resolved: &ResolvedProfile, base: u64, wanted: bool) -> Vec<String> {
    if !wanted {
        return vec![format!(
            "town trace: off ({ENV} is not 1); town updates are not logged"
        )];
    }
    BASE.store(base, Ordering::Relaxed);
    let mut lines = Vec::new();
    let develop = resolved.get(DEVELOP).map(|t| t.address);
    match (
        resolved.get(APPLY),
        resolved.get(DEVELOP_SITE),
        resolved.get(RETURN_SITE),
    ) {
        (Some(apply), Some(site), Some(ret)) => {
            let call_at = apply.address.wrapping_add(DEVELOP_CALL_AT);
            let call_bytes = crate::image::readable(call_at as usize, 5).then(|| {
                // SAFETY: five readable bytes of the game's code.
                unsafe { std::ptr::read_unaligned(call_at as usize as *const [u8; 5]) }
            });
            let installed = check_layout(
                apply.address,
                site.address,
                ret.address,
                call_bytes,
                develop,
            )
            .and_then(|()| {
                let first = splice(
                    site.address,
                    &DEVELOP_SITE_EXPECTED,
                    DEVELOP_SITE_STEAL,
                    develop_site_hook,
                )?;
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
                        "town trace: {APPLY} at {:#x} spliced at +{DEVELOP_SITE_AT:#x} and +{RETURN_SITE_AT:#x}: one `town:` line a size update (logging only)",
                        apply.address
                    ));
                }
                Err(why) => lines.push(format!("town trace: {APPLY} not traced, {why}")),
            }
        }
        _ => lines.push(format!(
            "town trace: {APPLY} not traced, the profile lacks {APPLY}, {DEVELOP_SITE} or {RETURN_SITE}"
        )),
    }
    match develop {
        Some(at) => match splice(at, &DEVELOP_EXPECTED, DEVELOP_STEAL, develop_hook) {
            Ok(s) => {
                let _kept = std::mem::ManuallyDrop::new(s);
                lines.push(format!(
                    "town trace: {DEVELOP} at {at:#x} spliced at its entry: one `town: ... develop` line a call (logging only)"
                ));
            }
            Err(why) => lines.push(format!("town trace: {DEVELOP} not traced, {why}")),
        },
        None => lines.push(format!(
            "town trace: {DEVELOP} not traced, the profile lacks it"
        )),
    }
    ON.store(true, Ordering::Release);
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_seed_is_the_appliers() {
        // Pinned from the applier's arithmetic (0x9dfbf0..0x9dfc83),
        // computed apart in Python.
        assert_eq!(town_seed(0), 1_024_464_386);
        assert_eq!(town_seed(1), 123_359_934);
        assert_eq!(town_seed(99_183), 977_935_457);
        assert_eq!(town_seed(113_533), 683_308_850);
        assert!((0..10_000).all(|u| (1..0x7fff_ffff).contains(&town_seed(u))));
    }

    #[test]
    fn the_lines_hold_the_raw_bits_and_the_seed_check() {
        let before = Before {
            step: Some(12_790),
            update: Some(113_533),
            town: 5023,
            size: [1.5f32.to_bits(), 0.1f32.to_bits(), 2.0f32.to_bits()],
            flag: 0,
            seed: 683_308_850,
            game_state: 0x1000,
            buffer: 1,
            engine: 0x2000,
            developer: 0x3000,
            developer_engine: DeveloperEngine::Own,
            ids: Some(400_000),
        };
        assert_eq!(
            apply_line(&before, Some(42), Some(400_012)),
            "town: step 12790 update 113533 town 5023 size 3fc00000,3dcccccd,40000000 (1.5,0.1,2.0) flag 0 seed 683308850 (expected 683308850) gamestate 0x1000 buffer 1 engine 0x2000 developer 0x3000 developer-engine own gen-after 42 entity-ids 400000->400012"
        );
        let unread = Before {
            step: None,
            update: None,
            developer_engine: DeveloperEngine::Unknown(None),
            ids: None,
            ..before
        };
        let line = apply_line(&unread, None, None);
        assert!(
            line.starts_with("town: step - update ? town 5023 "),
            "{line}"
        );
        assert!(line.contains("(expected ?)") && line.ends_with("gen-after ? entity-ids ?->?"));
        assert_eq!(
            develop_line(Some(7), Some(0x9dfccb), 5023, 1, Some(9), 0x30, 0x20),
            "town: step 7 develop from +0x9dfccb town 5023 flag 1 gen 9 developer 0x30 engine 0x20"
        );
    }

    #[test]
    fn buffers_are_numbered_and_the_developers_engine_named() {
        let mut buffers = [(0, 0); 2];
        assert_eq!(buffer_of(&mut buffers, 0xa, 0x1a), 0);
        assert_eq!(buffer_of(&mut buffers, 0xb, 0x1b), 1);
        assert_eq!(buffer_of(&mut buffers, 0xa, 0x1a), 0);
        assert_eq!(classify(&buffers, 0, Some(0x1a)), DeveloperEngine::Own);
        assert_eq!(classify(&buffers, 0, Some(0x1b)), DeveloperEngine::Other(1));
        assert_eq!(
            classify(&buffers, 0, Some(0x99)),
            DeveloperEngine::Unknown(Some(0x99))
        );
        assert_eq!(DeveloperEngine::Other(1).to_string(), "other(buffer 1)");
        // A third buffer: a world was loaded.
        assert_eq!(buffer_of(&mut buffers, 0xc, 0x1c), 0);
        assert_eq!(buffers, [(0xc, 0x1c), (0, 0)]);
    }

    #[test]
    fn the_layout_must_be_the_release_builds() {
        let apply = 0x0001_409d_fb10;
        let call_at = apply + DEVELOP_CALL_AT;
        let develop = 0x0001_408d_c240_u64;
        let rel = (develop as i64 - (call_at as i64 + 5)) as i32;
        let mut call = [0xE8, 0, 0, 0, 0];
        call[1..].copy_from_slice(&rel.to_le_bytes());
        let site = apply + DEVELOP_SITE_AT;
        let ret = apply + RETURN_SITE_AT;
        assert_eq!(
            check_layout(apply, site, ret, Some(call), Some(develop)),
            Ok(())
        );
        assert_eq!(check_layout(apply, site, ret, Some(call), None), Ok(()));
        assert!(
            check_layout(apply, site, ret, Some(call), Some(develop + 1))
                .unwrap_err()
                .contains("calls")
        );
        assert!(check_layout(apply, site + 1, ret, Some(call), None).is_err());
        assert!(check_layout(apply, site, ret - 1, Some(call), None).is_err());
        assert!(check_layout(apply, site, ret, None, None).is_err());
        assert!(check_layout(apply, site, ret, Some([0x90; 5]), None).is_err());
    }

    #[test]
    fn nothing_installs_when_off_or_without_the_targets() {
        let resolved = ResolvedProfile {
            name: "empty".into(),
            targets: Vec::new(),
            absent_optional: Vec::new(),
        };
        let lines = install_with(&resolved, 0, false);
        assert_eq!(lines.len(), 1);
        assert!(lines[0].contains(ENV), "{lines:?}");
        let lines = install_with(&resolved, 0, true);
        assert!(lines.iter().all(|l| l.contains("not traced")), "{lines:?}");
        ON.store(false, Ordering::Release);
        assert!(wanted(Some("1")) && wanted(Some("on")));
        assert!(!wanted(None) && !wanted(Some("0")));
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
            target(DEVELOP_SITE).prologue,
            DEVELOP_SITE_EXPECTED[..DEVELOP_SITE_STEAL].to_vec()
        );
        assert_eq!(
            target(RETURN_SITE).prologue,
            RETURN_EXPECTED[..RETURN_STEAL].to_vec()
        );
        assert_eq!(
            target(DEVELOP).prologue,
            DEVELOP_EXPECTED[..DEVELOP_STEAL].to_vec()
        );
        for name in [APPLY, DEVELOP_SITE, RETURN_SITE, DEVELOP] {
            assert!(!target(name).required, "{name} is optional");
        }
    }
}
