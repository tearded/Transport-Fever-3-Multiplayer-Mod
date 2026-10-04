//! The order fixes: where the engine's simulation depends on the order of a
//! container that is not saved state, made a pure function of the entity
//! set instead (docs/HOOKS.md, "The order fixes, as built";
//! investigation/TPF3_RNG_2026-09-29.md, "What must change for lockstep").
//!
//! Two replicas that run the same commands at the same steps still hold
//! their ECS node lists in different orders (a running world's is its
//! add/swap-remove history, a loaded world's its registration order), and
//! TPF2 Multiplayer found four places where the engine let that order decide
//! the simulation. TF3's counterparts, from the survey, each installed on
//! its own and each failing closed on its own:
//!
//! 1. **Land-vehicle reservation order** ([`land_vehicle`]): sorted by
//!    entity id before the engine's seeded shuffle. A fix. The shuffle's
//!    seed is the game's `tickCount`, which [`crate::ticks`] keeps equal;
//!    a line sampled by the seed's value logs both for two games to diff.
//! 2. **Ship and aircraft claim order** ([`measure`]): measured through the
//!    reservation manager, as TPF2 did, before anything is changed.
//! 3. **Road edge entries** ([`road`]): each edge's entries kept in entity
//!    order after every append (every other writer keeps order). A fix.
//! 4. **Vehicles at a stop** ([`terminal`]): sorted by entity id where the
//!    boarding loop reads them, TPF2's `vehstop`. A fix. The unload deques
//!    (TPF2's `unload`) are not located in TF3 yet.
//! 5. **Platform choice** ([`platform`]): the vehicles asked for a free
//!    platform in entity order, and the candidate terminals in one order
//!    before their cost sort. A fix.
//!
//! Every fix has the same shape: the profile must resolve its site, the
//! bytes there must be exactly what the fix expects (the resolver checks
//! them, and the splice checks them again), and every read the hook makes
//! on the game's thread goes through [`crate::image::readable`]. A shape it
//! does not recognise is refused for that step and said once in the log; a
//! panic switches the fix off for good. A fix never guesses.
//!
//! The measurement hooks install only with [`MEASURE_ENV`] set, so they
//! cost nothing otherwise.

#![allow(unsafe_code)]
// Elsewhere the fixes are not installed, so their code is unused there.
#![cfg_attr(not(all(windows, target_arch = "x86_64")), allow(dead_code))]

use std::sync::{
    Mutex,
    atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
};

use tpf3mp_hookcore::detour::{InlineDetour, SavedRegs, Splice};
use tpf3mp_hookcore::profile::ResolvedProfile;

use crate::image::Readable as Probe;
use crate::log;
use crate::perf::{self, Piece};

/// Set to `1` (or any non-empty value) in the game's environment, the
/// measurement hooks install: the claim order at the reservation manager,
/// the road-edge appends and the two fixes' own before-sort orders are
/// hashed per simulation update and written to hook.log every 100 updates.
/// A number above 1 sets that interval. Two replicas' logs can then be
/// diffed line by line.
pub const MEASURE_ENV: &str = "TPF3MP_HOOK_MEASURE_ORDER";

/// What installing one fix came to, for hook.log.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Outcome {
    pub fix: &'static str,
    pub installed: bool,
    pub reason: String,
}

impl std::fmt::Display for Outcome {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.installed {
            write!(f, "order fix {}: installed ({})", self.fix, self.reason)
        } else {
            write!(f, "order fix {}: off, {}", self.fix, self.reason)
        }
    }
}

/// Installs every order fix that resolves on this build, for the life of
/// the game, and says what each came to. Called once the step gate is in,
/// before any world exists (the quiescence rule in docs/HOOKS.md).
pub fn install(resolved: &ResolvedProfile) -> Vec<Outcome> {
    let measuring = measure::configure_from_env();
    let wanted = |env: &str| crate::ticks::wanted(std::env::var(env).ok().as_deref());
    let mut outcomes = vec![
        land_vehicle::install(resolved, wanted(land_vehicle::TOGGLE_ENV)),
        terminal::install(resolved, wanted(terminal::TOGGLE_ENV)),
    ];
    outcomes.extend(platform::install(resolved, wanted(platform::TOGGLE_ENV)));
    outcomes.extend(road::install(resolved, wanted(road::TOGGLE_ENV), measuring));
    outcomes.extend(measure::install(resolved, measuring));
    outcomes.extend(route_trace::install(resolved));
    outcomes
}

thread_local! {
    /// Set on this thread while it runs the game's own step
    /// ([`set_in_step`]).
    static IN_STEP: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// From the step detour, around its call of the game's `GameSim::Step`:
/// this thread is inside the simulation's step. The second engine's copy
/// (`GameState::Replicate` -> `Replicator::Apply`) runs from the game's
/// frame, outside the step, as often as the frames come, so the fixes count
/// what they see inside the step apart: those counts are what two games'
/// logs must agree on.
pub fn set_in_step(inside: bool) {
    IN_STEP.with(|flag| flag.set(inside));
}

/// Whether this thread is inside the game's step.
pub fn in_step() -> bool {
    IN_STEP.with(|flag| flag.get())
}

/// Opt-in, read-only route-cache trace for reproducing terminal divergence.
mod route_trace {
    use super::*;
    use std::collections::BTreeMap;

    use crate::build_data::native::order::route_trace::EXPECTED;
    use crate::build_data::native::order::route_trace::SITE;
    static LINE: AtomicU64 = AtomicU64::new(u64::MAX);
    static BROKEN: AtomicBool = AtomicBool::new(false);
    static SEEN: Mutex<BTreeMap<i32, String>> = Mutex::new(BTreeMap::new());

    pub fn install(resolved: &ResolvedProfile) -> Vec<Outcome> {
        let Some(line) = std::env::var("TPF3MP_HOOK_TRACE_LINE")
            .ok()
            .and_then(|v| v.parse::<u32>().ok())
        else {
            return Vec::new();
        };
        LINE.store(u64::from(line), Ordering::Release);
        let result = resolved
            .get(SITE)
            .ok_or_else(|| "profile has no route trace site".to_owned())
            .and_then(|site| {
                // SAFETY: a checked return site, installed before worlds run.
                unsafe { Splice::install(site.address as *mut u8, &EXPECTED, 8, hook) }
                    .map(|s| {
                        let _kept = std::mem::ManuallyDrop::new(s);
                    })
                    .map_err(|e| e.to_string())
            });
        vec![Outcome {
            fix: "route-trace",
            installed: result.is_ok(),
            reason: result
                .err()
                .unwrap_or_else(|| format!("line {line}, cache changes only")),
        }]
    }

    fn vector(probe: &mut Probe, at: u64, stride: u64, max: u64) -> Option<(u64, u64)> {
        let start: u64 = probe.read(at)?;
        let end: u64 = probe.read(at.checked_add(8)?)?;
        let bytes = end.checked_sub(start)?;
        if !bytes.is_multiple_of(stride) || bytes / stride > max {
            return None;
        }
        if bytes > 0 && !probe.readable(usize::try_from(start).ok()?, usize::try_from(bytes).ok()?)
        {
            return None;
        }
        Some((start, bytes / stride))
    }

    fn describe(at: u64) -> Option<String> {
        let mut p = Probe::new();
        let (sections, n) = vector(&mut p, at, 24, 4096)?;
        let mut out = Vec::new();
        for s in 0..n {
            let (paths, count) = vector(&mut p, sections + s * 24, 0xe8, 4096)?;
            for i in 0..count {
                let path = paths + i * 0xe8;
                let terminals: [i32; 4] = p.read(path)?;
                let (edges, len) = vector(&mut p, path + 0x10, 12, 65536)?;
                let mut hash = Fnv1a::new();
                for e in 0..len {
                    let id: [u8; 8] = p.read(edges + e * 12)?;
                    let dir: u8 = p.read(edges + e * 12 + 8)?;
                    hash.write(&id);
                    hash.write(&[dir]);
                }
                // ComputeTerminalDecisionIndices writes this signed index;
                // FindPathToStop1 consumes it when attaching a vehicle path.
                let decision: i32 = p.read(path + 0x90)?;
                let invalid: u8 = p.read(path + 0xbc)?;
                out.push(format!(
                    "{s}/{i}:{terminals:?}:{len}/{:016x}:decision={decision}:invalid={invalid}",
                    hash.0
                ));
            }
        }
        Some(out.join(";"))
    }

    unsafe extern "system" fn hook(regs: *mut SavedRegs) {
        guarded("route-trace", &BROKEN, || {
            // SAFETY: the splice holds its register block for this callback.
            let regs = unsafe { &*regs };
            if regs.rdx as u32 as u64 != LINE.load(Ordering::Acquire) {
                return;
            }
            let line = regs.rdx as u32 as i32;
            let description = regs
                .rax
                .checked_add(0x18)
                .and_then(describe)
                .unwrap_or_else(|| "unreadable cache shape".into());
            let mut seen = SEEN.lock().unwrap_or_else(|e| e.into_inner());
            if seen.get(&line) != Some(&description) {
                log::line(&format!(
                    "route cache: step={:?} line={line} {description}",
                    crate::seeds::current_step()
                ));
                seen.insert(line, description);
            }
        });
    }
}

/// Reads a plain value from the game's memory, only if it is readable (the
/// check through the per-thread region cache, [`crate::image::Readable`]).
fn read<T: Copy>(address: u64) -> Option<T> {
    Probe::new().read(address)
}

/// Whether `len` bytes at `address` may be read.
fn readable(address: u64, len: usize) -> bool {
    usize::try_from(address).is_ok_and(|address| crate::image::readable_cached(address, len))
}

/// Says a refusal once per reason (a fix refuses per step, the log is not
/// per step), and counts it, in all and by reason since the last
/// [`Refusals::take_window`] (the `perf:` line's).
struct Refusals {
    state: Mutex<RefusalState>,
    count: AtomicU64,
}

struct RefusalState {
    last: Option<&'static str>,
    /// Refusals by reason since the window began; one entry per reason, so
    /// as short as the fix's list of reasons.
    window: Vec<(&'static str, u64)>,
}

impl Refusals {
    const fn new() -> Self {
        Self {
            state: Mutex::new(RefusalState {
                last: None,
                window: Vec::new(),
            }),
            count: AtomicU64::new(0),
        }
    }

    fn note(&self, fix: &str, why: &'static str) {
        self.count.fetch_add(1, Ordering::Relaxed);
        let mut state = self.state.lock().unwrap_or_else(|p| p.into_inner());
        match state.window.iter_mut().find(|(reason, _)| *reason == why) {
            Some((_, n)) => *n += 1,
            None => state.window.push((why, 1)),
        }
        if state.last != Some(why) {
            state.last = Some(why);
            log::line(&format!(
                "order fix {fix}: refused this step, {why}; the engine's own order stands"
            ));
        }
    }

    /// The refusals by reason since the last take.
    fn take_window(&self) -> Vec<(&'static str, u64)> {
        std::mem::take(&mut self.state.lock().unwrap_or_else(|p| p.into_inner()).window)
    }
}

/// FNV-1a over bytes, 64-bit: the measurement's hash. Two replicas that
/// log the same value hashed the same sequence.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Fnv1a(pub u64);

impl Fnv1a {
    pub const fn new() -> Self {
        Self(0xcbf2_9ce4_8422_2325)
    }

    pub fn write(&mut self, bytes: &[u8]) {
        for byte in bytes {
            self.0 ^= u64::from(*byte);
            self.0 = self.0.wrapping_mul(0x100_0000_01b3);
        }
    }

    pub fn write_u32(&mut self, value: u32) {
        self.write(&value.to_le_bytes());
    }
}

impl Default for Fnv1a {
    fn default() -> Self {
        Self::new()
    }
}

/// Runs a hook body on the game's thread without letting a panic cross
/// into the engine: a panic switches the fix off for good and is said once.
fn guarded(fix: &str, broken: &AtomicBool, body: impl FnOnce()) {
    if broken.load(Ordering::Acquire) {
        return;
    }
    if std::panic::catch_unwind(std::panic::AssertUnwindSafe(body)).is_err() {
        broken.store(true, Ordering::Release);
        log::line(&format!(
            "order fix {fix}: panicked on the game's thread; switched off for this game"
        ));
    }
}

/// What a sort at a site came to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Sorted {
    /// Fewer than two entries, or already in order: nothing written.
    Unchanged,
    /// The entries were put in order.
    Reordered,
}

/// The land-vehicle reservation order (survey item 1): the site inside
/// `ecs::LandVehicleMoveSystem::Update2` where the vector of vehicles that
/// want track is complete and the engine is about to shuffle it with a
/// `minstd_rand` seeded from the game time, then stable-sort it by a
/// priority and reserve track in that order. The vector is built by walking
/// the family's node list, so its order is the list's; the shuffle's seed
/// is lockstep state, the order it permutes is not (TPF2's train-order bug).
/// Sorting the vector by the entity id of each entry's node before the
/// shuffle makes the shuffled order a pure function of the entity set and
/// the seed. The engine's seed, shuffle and priority sort stay as they are.
pub mod land_vehicle {
    use super::*;

    pub const FIX: &str = "land-vehicle-order";
    /// Set to `0` (or `off`), the site stays out: the engine shuffles the
    /// vehicles in its own order.
    pub const TOGGLE_ENV: &str = "TPF3MP_HOOK_LAND_VEHICLE_ORDER";
    pub use crate::build_data::native::order::land_vehicle::ENTRY_LEN;
    pub use crate::build_data::native::order::land_vehicle::EXPECTED;
    pub use crate::build_data::native::order::land_vehicle::RECORD_LEN;
    pub use crate::build_data::native::order::land_vehicle::RECORDS;
    pub use crate::build_data::native::order::land_vehicle::SITE;
    pub use crate::build_data::native::order::land_vehicle::STEAL;
    use crate::build_data::native::order::land_vehicle::THIS;
    use crate::build_data::native::order::land_vehicle::VEC_BEGIN;
    use crate::build_data::native::order::land_vehicle::VEC_END;
    /// A sanity bound on the vehicle count.
    pub const MAX_ENTRIES: u64 = 1 << 20;

    static BROKEN: AtomicBool = AtomicBool::new(false);
    static REFUSALS: Refusals = Refusals::new();
    static CALLS: AtomicU64 = AtomicU64::new(0);
    static REORDERS: AtomicU64 = AtomicU64::new(0);

    pub fn install(resolved: &ResolvedProfile, wanted: bool) -> Outcome {
        let off = |reason: String| Outcome {
            fix: FIX,
            installed: false,
            reason,
        };
        if !wanted {
            return off(format!(
                "{TOGGLE_ENV} says so; the engine shuffles in its own order"
            ));
        }
        let Some(site) = resolved.get(SITE) else {
            return off(format!("the profile has no {SITE:?}"));
        };
        let Some(records) = resolved.get(RECORDS) else {
            return off(format!("the profile has no {RECORDS:?}"));
        };
        // The records walk is the reservation loop's head, a few hundred
        // bytes after the shuffle in the same function.
        if records.address <= site.address || records.address - site.address > 0x1000 {
            return off(format!(
                "{RECORDS:?} at {:#x} is not just after {SITE:?} at {:#x}",
                records.address, site.address
            ));
        }
        // SAFETY: the site is inside a function the profile resolved and
        // prologue-checked, the hook installs before any world exists, so
        // no thread is in it; nothing branches into the stolen bytes (the
        // profile's note, from the disassembly); `hook` never unwinds
        // (`guarded`) and only rewrites the vector's entries in place.
        match unsafe { Splice::install(site.address as usize as *mut u8, &EXPECTED, STEAL, hook) } {
            Ok(splice) => {
                let _kept = std::mem::ManuallyDrop::new(splice);
                Outcome {
                    fix: FIX,
                    installed: true,
                    reason: format!(
                        "at {:#x}, the vehicles that want track are sorted by entity id before the engine's seeded shuffle",
                        site.address
                    ),
                }
            }
            Err(error) => off(format!("the site at {:#x}: {error}", site.address)),
        }
    }

    /// The hook the stub calls at the site, with the site's registers.
    pub(super) unsafe extern "system" fn hook(regs: *mut SavedRegs) {
        let _timer = perf::time(Piece::LandVehicle);
        guarded(FIX, &BROKEN, || {
            // SAFETY: the stub hands the block it pushed on this thread's
            // stack and holds it until the hook returns.
            let regs = unsafe { &*regs };
            let n = CALLS.fetch_add(1, Ordering::Relaxed) + 1;
            // The site follows the seed's fix-up (`cmove r8d, r12d`) and the
            // mask loop, which leaves r8 alone: r8d is the shuffle's seed.
            let seed = regs.r8 as u32;
            match apply(regs.rbp) {
                Ok((sorted, before)) => {
                    if sorted == Sorted::Reordered {
                        let reorders = REORDERS.fetch_add(1, Ordering::Relaxed) + 1;
                        if reorders <= 3 {
                            log::line(&format!(
                                "order fix {FIX}: {} vehicles put in entity order before the shuffle (call #{n}, seed {seed})",
                                before.len()
                            ));
                        }
                    }
                    if let Some(line) = sample_line(seed, &before) {
                        log::line(&line);
                    }
                    measure::note_land_vehicles(&before, sorted, seed);
                }
                Err(why) => REFUSALS.note(FIX, why),
            }
            if n == 1 || n.is_multiple_of(1 << 16) {
                log::line(&format!(
                    "order fix {FIX}: alive, calls={n} reordered={} refused={}",
                    REORDERS.load(Ordering::Relaxed),
                    REFUSALS.count.load(Ordering::Relaxed)
                ));
            }
        });
    }

    /// Reads the vector and the records through the frame, sorts the
    /// vector's entries by their node's entity id, and hands back the
    /// entity ids in the order the engine had them. Every read is checked;
    /// any shape but the measured one is a refusal with nothing written.
    fn apply(rbp: u64) -> Result<(Sorted, Vec<u32>), &'static str> {
        let slot = |offset: i64| rbp.checked_add_signed(offset);
        let begin: u64 = slot(VEC_BEGIN)
            .and_then(read)
            .ok_or("the frame's vector begin is unreadable")?;
        let end: u64 = slot(VEC_END)
            .and_then(read)
            .ok_or("the frame's vector end is unreadable")?;
        let this: u64 = slot(THIS)
            .and_then(read)
            .ok_or("the frame's this is unreadable")?;
        if end < begin || !(end - begin).is_multiple_of(ENTRY_LEN) {
            return Err("the vector's bounds are not whole entries");
        }
        let count = (end - begin) / ENTRY_LEN;
        if count > MAX_ENTRIES {
            return Err("more entries than any world holds");
        }
        if count < 2 {
            return Ok((Sorted::Unchanged, Vec::new()));
        }
        let holder: u64 = this
            .checked_add(8)
            .and_then(read)
            .ok_or("the node-list holder is unreadable")?;
        let records: u64 = read(holder).ok_or("the node records are unreadable")?;
        let records_end: u64 = holder
            .checked_add(8)
            .and_then(read)
            .ok_or("the node records' end is unreadable")?;
        if records_end < records || !(records_end - records).is_multiple_of(RECORD_LEN) {
            return Err("the node records are not whole records");
        }
        let record_count = (records_end - records) / RECORD_LEN;
        let vector_len =
            usize::try_from(count * ENTRY_LEN).map_err(|_| "the vector is too long")?;
        let records_len = usize::try_from(record_count * RECORD_LEN)
            .map_err(|_| "the node records are too long")?;
        if !readable(begin, vector_len) {
            return Err("the vector's entries are unreadable");
        }
        if !readable(records, records_len) {
            return Err("the node records are unreadable");
        }
        if measure::enabled() {
            // SAFETY: the readable records span, whole 20-byte records; the
            // entity id is each record's first dword.
            let order = (0..record_count).map(|i| unsafe {
                std::ptr::read_unaligned((records + i * RECORD_LEN) as *const u32)
            });
            measure::note_land_nodes(order);
        }
        // SAFETY: `vector_len` readable bytes at `begin`, in whole 8-byte
        // entries; the copy is by value.
        let entries: Vec<u64> = (0..count)
            .map(|i| unsafe { std::ptr::read_unaligned((begin + i * ENTRY_LEN) as *const u64) })
            .collect();
        let key_of = |entry: u64| -> Option<u32> {
            let index = u64::from(entry as u32);
            if index >= record_count {
                return None;
            }
            // SAFETY: the record lies inside the readable records span.
            Some(unsafe { std::ptr::read_unaligned((records + index * RECORD_LEN) as *const u32) })
        };
        let (order, before) = canonical_order(&entries, key_of)?;
        let Some(order) = order else {
            return Ok((Sorted::Unchanged, before));
        };
        for (i, entry) in order.iter().enumerate() {
            // SAFETY: the same readable span the entries were read from;
            // the engine's own thread is the one writing it, in place, and
            // the engine reads the vector only after this site.
            unsafe {
                std::ptr::write_unaligned((begin + i as u64 * ENTRY_LEN) as *mut u64, *entry)
            };
        }
        Ok((Sorted::Reordered, before))
    }

    /// One seed value in this many gets a [`sample_line`].
    pub const SAMPLE: u32 = 256;

    /// For one seed value in [`SAMPLE`] (the seed is the game's tickCount,
    /// so about one update in 256): the seed, how many vehicles want track
    /// (0 when fewer than two) and a hash of their entity ids in the order
    /// the engine shuffles them. Sampled by the seed's value, not by a
    /// count of calls, so two games that agree write the same lines and two
    /// logs can be diffed; the engine's shuffle and priority sort are a
    /// function of exactly these, so equal lines mean an equal claim order.
    pub fn sample_line(seed: u32, before: &[u32]) -> Option<String> {
        if !seed.is_multiple_of(SAMPLE) {
            return None;
        }
        let mut ids = before.to_vec();
        ids.sort_unstable();
        let mut hash = Fnv1a::new();
        for id in &ids {
            hash.write_u32(*id);
        }
        Some(format!(
            "order fix {FIX}: sample seed={seed} n={} ids={:016x}",
            ids.len(),
            hash.0
        ))
    }

    /// The entries sorted by their key (the entity id of the node an entry
    /// indexes), with the keys in the engine's order. `None` when the order
    /// already was that. Two entries with one key, or an entry whose index
    /// names no record, is a refusal: the sort would not be a total order
    /// of lockstep state.
    pub fn canonical_order(
        entries: &[u64],
        key_of: impl Fn(u64) -> Option<u32>,
    ) -> Result<(Option<Vec<u64>>, Vec<u32>), &'static str> {
        let mut keyed = Vec::with_capacity(entries.len());
        for entry in entries {
            let key = key_of(*entry).ok_or("an entry indexes no node record")?;
            keyed.push((key, *entry));
        }
        let before: Vec<u32> = keyed.iter().map(|(key, _)| *key).collect();
        if before.windows(2).all(|pair| pair[0] < pair[1]) {
            return Ok((None, before));
        }
        keyed.sort_unstable_by_key(|(key, _)| *key);
        if keyed.windows(2).any(|pair| pair[0].0 == pair[1].0) {
            return Err("two entries name one entity");
        }
        Ok((
            Some(keyed.into_iter().map(|(_, entry)| entry).collect()),
            before,
        ))
    }
}

/// The vehicles standing at a line stop (survey item 4, TPF2's `vehstop`):
/// `ecs::SimEntityAtTerminalSystem::Update` asks `TransportVehicleSystem`
/// for the vector of vehicles at each `(line, stopIndex)` and hands the
/// waiting cargo and people to them in the vector's order, one running
/// index shared by all of them. The vector is append order while the game
/// runs and load order after a load (its owner adds after a `std::find`
/// and erases in place, as TPF2's did), so two replicas load two trucks at
/// one stop differently. Sorted by entity id right after the lookup, where
/// the boarding loop reads it.
pub mod terminal {
    use super::*;

    pub const FIX: &str = "vehicles-at-stop-order";
    /// Set to `0` (or `off`), the site stays out: the boarding loop reads
    /// the vehicles in the engine's order.
    pub const TOGGLE_ENV: &str = "TPF3MP_HOOK_VEHICLES_AT_STOP_ORDER";
    pub use crate::build_data::native::order::terminal::EXPECTED;
    pub use crate::build_data::native::order::terminal::GETTER;
    pub use crate::build_data::native::order::terminal::SITE;
    pub use crate::build_data::native::order::terminal::STEAL;
    pub const MAX_IDS: u64 = 1 << 20;

    static BROKEN: AtomicBool = AtomicBool::new(false);
    static REFUSALS: Refusals = Refusals::new();
    static CALLS: AtomicU64 = AtomicU64::new(0);
    static REORDERS: AtomicU64 = AtomicU64::new(0);

    pub fn install(resolved: &ResolvedProfile, wanted: bool) -> Outcome {
        let off = |reason: String| Outcome {
            fix: FIX,
            installed: false,
            reason,
        };
        if !wanted {
            return off(format!(
                "{TOGGLE_ENV} says so; the boarding loop reads the engine's order"
            ));
        }
        let Some(site) = resolved.get(SITE) else {
            return off(format!("the profile has no {SITE:?}"));
        };
        let Some(getter) = resolved.get(GETTER) else {
            return off(format!("the profile has no {GETTER:?}"));
        };
        // The five bytes before the site must be a call of the getter: the
        // vector in rax is its answer and nothing else's.
        let call = site.address.wrapping_sub(5);
        let opcode: Option<u8> = read(call);
        let rel: Option<i32> = read(call + 1);
        match (opcode, rel) {
            (Some(0xE8), Some(rel)) => {
                let callee = site.address.wrapping_add_signed(i64::from(rel));
                if callee != getter.address {
                    return off(format!(
                        "the call before the site reaches {callee:#x}, not {GETTER:?} at {:#x}",
                        getter.address
                    ));
                }
            }
            _ => return off(format!("no call before the site at {:#x}", site.address)),
        }
        // SAFETY: as for the land-vehicle site: resolved, quiescent, nothing
        // branches into the stolen bytes, and the hook only sorts the ids of
        // the vector `rax` names, in place.
        match unsafe { Splice::install(site.address as usize as *mut u8, &EXPECTED, STEAL, hook) } {
            Ok(splice) => {
                let _kept = std::mem::ManuallyDrop::new(splice);
                Outcome {
                    fix: FIX,
                    installed: true,
                    reason: format!(
                        "at {:#x}, the vehicles at a line stop are sorted by entity id before the boarding loop",
                        site.address
                    ),
                }
            }
            Err(error) => off(format!("the site at {:#x}: {error}", site.address)),
        }
    }

    /// The hook the stub calls at the site, with the site's registers.
    pub(super) unsafe extern "system" fn hook(regs: *mut SavedRegs) {
        let _timer = perf::time(Piece::VehiclesAtStop);
        guarded(FIX, &BROKEN, || {
            // SAFETY: the stub's block, held until the hook returns.
            let regs = unsafe { &*regs };
            let n = CALLS.fetch_add(1, Ordering::Relaxed) + 1;
            match apply(regs.rax) {
                Ok((sorted, before)) => {
                    if sorted == Sorted::Reordered {
                        let reorders = REORDERS.fetch_add(1, Ordering::Relaxed) + 1;
                        if reorders <= 3 {
                            log::line(&format!(
                                "order fix {FIX}: {} vehicles at a stop put in entity order (call #{n})",
                                before.len()
                            ));
                        }
                    }
                    measure::note_vehicles_at_stop(&before, sorted);
                }
                Err(why) => REFUSALS.note(FIX, why),
            }
            if n == 1 || n.is_multiple_of(1 << 16) {
                log::line(&format!(
                    "order fix {FIX}: alive, calls={n} reordered={} refused={}",
                    REORDERS.load(Ordering::Relaxed),
                    REFUSALS.count.load(Ordering::Relaxed)
                ));
            }
        });
    }

    /// Sorts the `std::vector<Entity>` (begin at +0, end at +8) at `vector`
    /// in place, and hands back its ids in the order the engine had them.
    fn apply(vector: u64) -> Result<(Sorted, Vec<i32>), &'static str> {
        let begin: u64 = read(vector).ok_or("the vector is unreadable")?;
        let end: u64 = vector
            .checked_add(8)
            .and_then(read)
            .ok_or("the vector's end is unreadable")?;
        if end < begin || !(end - begin).is_multiple_of(4) {
            return Err("the vector's bounds are not whole ids");
        }
        let count = (end - begin) / 4;
        if count > MAX_IDS {
            return Err("more vehicles at one stop than any world holds");
        }
        if count < 2 {
            return Ok((Sorted::Unchanged, Vec::new()));
        }
        let len = usize::try_from(count * 4).map_err(|_| "the vector is too long")?;
        if !readable(begin, len) {
            return Err("the vector's ids are unreadable");
        }
        // SAFETY: `len` readable bytes at `begin`, whole 4-byte ids.
        let ids: Vec<i32> = (0..count)
            .map(|i| unsafe { std::ptr::read_unaligned((begin + i * 4) as *const i32) })
            .collect();
        let Some(sorted) = sorted_ids(&ids) else {
            return Ok((Sorted::Unchanged, ids));
        };
        for (i, id) in sorted.iter().enumerate() {
            // SAFETY: the same span, written in place on the engine's thread
            // before the engine reads it.
            unsafe { std::ptr::write_unaligned((begin + i as u64 * 4) as *mut i32, *id) };
        }
        Ok((Sorted::Reordered, ids))
    }

    /// The ids ascending, or `None` when they already are.
    pub fn sorted_ids(ids: &[i32]) -> Option<Vec<i32>> {
        if ids.windows(2).all(|pair| pair[0] <= pair[1]) {
            return None;
        }
        let mut sorted = ids.to_vec();
        sorted.sort_unstable();
        Some(sorted)
    }
}

/// Sorts `keys` ascending and reports what that came to: `Ok(None)` when
/// they already were strictly ascending, the permutation otherwise, a
/// refusal when two share a key (not a total order of lockstep state).
fn canonical_permutation<K: Ord + Copy>(keys: &[K]) -> Result<Option<Vec<usize>>, &'static str> {
    if keys.windows(2).all(|pair| pair[0] < pair[1]) {
        return Ok(None);
    }
    let mut order: Vec<usize> = (0..keys.len()).collect();
    order.sort_by_key(|&i| keys[i]);
    if order.windows(2).any(|pair| keys[pair[0]] == keys[pair[1]]) {
        return Err("two entries name one entity");
    }
    Ok(Some(order))
}

/// The platform choice (investigation/TPF3_TRAIN_PRIORITY_2026-09-30.md,
/// "Platforms"). `ecs::TransportVehicleSystem::Update2` (`0xb8bae0`) walks
/// its node list (8-byte records `{entity, TransportVehicle index}` at
/// `[[this+8]]`, as many as its `int` argument says) and asks
/// `FindNextFreeTerminal` (`0xb84e20`) for each en-route vehicle, and a
/// choice is stored for the rest of the update, so a vehicle visited later
/// sees what an earlier one took. Two sites, each on its own:
///
/// - **visit**: at the loop head, after the engine loads the list's begin
///   into `rdi`, the hook points `rdi` at a copy of the list sorted by
///   entity id, built at the loop's first iteration. The loop only reads
///   `[rdi+rsi]` and `[rdi+rsi+4]` and reloads `rdi` every iteration, and
///   `rdi` is set anew after the loop, so the engine's list is never
///   written: it visits the same vehicles, in entity order.
/// - **candidates**: right before `FindNextFreeTerminal` `std::sort`s its
///   candidate terminals by cost (`0xb85453`; the comparator looks each
///   cost up in a map and compares floats only, so equal costs keep an
///   introsort order that depends on the input's), the 12-byte candidates
///   are put in one canonical order, so equal costs break the same way in
///   every game.
pub mod platform {
    use std::cell::RefCell;

    use super::*;

    pub const FIX: &str = "platform-order";
    /// Set to `0` (or `off`), both sites stay out.
    pub const TOGGLE_ENV: &str = "TPF3MP_HOOK_PLATFORM_ORDER";
    pub use crate::build_data::native::order::platform::CANDIDATE_LEN;
    pub use crate::build_data::native::order::platform::CANDIDATES_EXPECTED;
    pub use crate::build_data::native::order::platform::CANDIDATES_SITE;
    pub use crate::build_data::native::order::platform::CANDIDATES_STEAL;
    use crate::build_data::native::order::platform::COUNT;
    pub use crate::build_data::native::order::platform::RECORD_LEN;
    pub use crate::build_data::native::order::platform::VISIT_EXPECTED;
    pub use crate::build_data::native::order::platform::VISIT_SITE;
    pub use crate::build_data::native::order::platform::VISIT_STEAL;
    pub const MAX_RECORDS: u64 = 1 << 20;
    pub const MAX_CANDIDATES: u64 = 1 << 12;

    static VISIT_BROKEN: AtomicBool = AtomicBool::new(false);
    static VISIT_REFUSALS: Refusals = Refusals::new();
    static VISIT_CALLS: AtomicU64 = AtomicU64::new(0);
    static VISIT_REORDERS: AtomicU64 = AtomicU64::new(0);
    static CANDIDATE_BROKEN: AtomicBool = AtomicBool::new(false);
    static CANDIDATE_REFUSALS: Refusals = Refusals::new();
    static CANDIDATE_CALLS: AtomicU64 = AtomicU64::new(0);
    static CANDIDATE_REORDERS: AtomicU64 = AtomicU64::new(0);

    /// The loop being walked on this thread: the engine's list, and the
    /// sorted copy `rdi` is pointed at while `active`. The copy's buffer is
    /// kept from update to update, so an update allocates nothing once it
    /// has grown to the fleet's size.
    #[derive(Default)]
    struct Visit {
        list: u64,
        count: u64,
        sorted: Vec<u64>,
        active: bool,
    }

    thread_local! {
        static VISIT: RefCell<Visit> = RefCell::new(Visit::default());
    }

    pub fn install(resolved: &ResolvedProfile, wanted: bool) -> Vec<Outcome> {
        if !wanted {
            return vec![Outcome {
                fix: FIX,
                installed: false,
                reason: format!("{TOGGLE_ENV} says so; the engine's visit and tie order stand"),
            }];
        }
        vec![
            splice(
                resolved,
                VISIT_SITE,
                &VISIT_EXPECTED,
                VISIT_STEAL,
                visit_hook,
                "the vehicles are asked for a free platform in entity order",
            ),
            splice(
                resolved,
                CANDIDATES_SITE,
                &CANDIDATES_EXPECTED,
                CANDIDATES_STEAL,
                candidates_hook,
                "the candidate terminals are in one order before the cost sort",
            ),
        ]
    }

    fn splice(
        resolved: &ResolvedProfile,
        name: &str,
        expected: &[u8],
        steal: usize,
        hook: tpf3mp_hookcore::detour::SpliceHook,
        what: &str,
    ) -> Outcome {
        let off = |reason: String| Outcome {
            fix: FIX,
            installed: false,
            reason,
        };
        let Some(site) = resolved.get(name) else {
            return off(format!("the profile has no {name:?}"));
        };
        // SAFETY: a site inside a function the profile resolved and
        // prologue-checked, installed before any world exists; nothing
        // branches into the stolen bytes past the first (tpfre xrefs, noted
        // in the profile); the hook never unwinds (`guarded`).
        match unsafe { Splice::install(site.address as usize as *mut u8, expected, steal, hook) } {
            Ok(splice) => {
                let _kept = std::mem::ManuallyDrop::new(splice);
                Outcome {
                    fix: FIX,
                    installed: true,
                    reason: format!("{name} at {:#x}: {what}", site.address),
                }
            }
            Err(error) => off(format!("{name} at {:#x}: {error}", site.address)),
        }
    }

    /// The visit order the fix gives: the records by entity id (the low
    /// dword), each whole; `None` when they already are in it.
    pub fn visit_order(records: &[u64]) -> Result<Option<Vec<u64>>, &'static str> {
        let keys: Vec<i32> = records.iter().map(|r| *r as u32 as i32).collect();
        Ok(canonical_permutation(&keys)?.map(|order| order.iter().map(|&i| records[i]).collect()))
    }

    /// The candidates' canonical order: by station, terminal and the first
    /// word, as unsigned words; `None` when they already are in it. Equal
    /// candidates are interchangeable, so any tie among them is harmless.
    /// The reference for [`sort_candidates_in_place`], which the hook runs.
    pub fn candidate_order(candidates: &[[u32; 3]]) -> Option<Vec<[u32; 3]>> {
        let key = |c: &[u32; 3]| (c[1], c[2], c[0]);
        if candidates.windows(2).all(|p| key(&p[0]) <= key(&p[1])) {
            return None;
        }
        let mut sorted = candidates.to_vec();
        sorted.sort_by_key(key);
        Some(sorted)
    }

    /// One 12-byte candidate as the engine lays it out: three words.
    pub type Candidate = [u8; CANDIDATE_LEN as usize];

    fn candidate_key(c: &Candidate) -> (u32, u32, u32) {
        let word =
            |i: usize| u32::from_le_bytes([c[4 * i], c[4 * i + 1], c[4 * i + 2], c[4 * i + 3]]);
        (word(1), word(2), word(0))
    }

    /// [`candidate_order`], in place and allocating nothing: one scan when
    /// the candidates already are in order. Two candidates with one key are
    /// the same twelve bytes, so the unstable sort gives the same bytes as
    /// the reference's stable one.
    pub fn sort_candidates_in_place(candidates: &mut [Candidate]) -> Sorted {
        if candidates
            .windows(2)
            .all(|p| candidate_key(&p[0]) <= candidate_key(&p[1]))
        {
            return Sorted::Unchanged;
        }
        candidates.sort_unstable_by_key(candidate_key);
        Sorted::Reordered
    }

    /// The visit order of `count` records (`record(i)` the `i`th, entity id
    /// in its low dword) into `sorted`, the same order [`visit_order`]
    /// gives: nothing is copied when the records already are strictly in
    /// entity order, and `sorted`'s buffer is reused.
    pub fn sort_records(
        count: u64,
        record: impl Fn(u64) -> u64,
        sorted: &mut Vec<u64>,
    ) -> Result<Sorted, &'static str> {
        let key = |r: u64| r as u32 as i32;
        if (1..count).all(|i| key(record(i - 1)) < key(record(i))) {
            return Ok(Sorted::Unchanged);
        }
        sorted.clear();
        sorted.extend((0..count).map(&record));
        // Unique keys (checked next) are a total order: stable or not, one
        // result.
        sorted.sort_unstable_by_key(|r| key(*r));
        if sorted.windows(2).any(|p| key(p[0]) == key(p[1])) {
            sorted.clear();
            return Err("two entries name one entity");
        }
        Ok(Sorted::Reordered)
    }

    pub(super) unsafe extern "system" fn visit_hook(regs: *mut SavedRegs) {
        let _timer = perf::time(Piece::PlatformVisit);
        guarded(FIX, &VISIT_BROKEN, || {
            // SAFETY: the stub's block, held until the hook returns.
            let regs = unsafe { &mut *regs };
            VISIT.with(|visit| {
                let mut visit = visit.borrow_mut();
                if regs.rsi == 0 {
                    // The loop's first iteration: rax is the node-list
                    // holder, rdi its begin, just loaded.
                    visit.active = false;
                    visit.list = 0;
                    visit.count = 0;
                    let n = VISIT_CALLS.fetch_add(1, Ordering::Relaxed) + 1;
                    match begin_loop(regs.rbp, regs.rax, regs.rdi, &mut visit.sorted) {
                        Ok((records, Sorted::Reordered)) => {
                            let reorders = VISIT_REORDERS.fetch_add(1, Ordering::Relaxed) + 1;
                            if reorders <= 3 {
                                log::line(&format!(
                                    "order fix {FIX}: {records} vehicles asked for platforms in entity order (update #{n})"
                                ));
                            }
                            visit.list = regs.rdi;
                            visit.count = records;
                            visit.active = true;
                        }
                        Ok((_, Sorted::Unchanged)) => {}
                        Err(why) => VISIT_REFUSALS.note(FIX, why),
                    }
                    if n == 1 || n.is_multiple_of(1 << 14) {
                        log::line(&format!(
                            "order fix {FIX}: visit alive, updates={n} reordered={} refused={}",
                            VISIT_REORDERS.load(Ordering::Relaxed),
                            VISIT_REFUSALS.count.load(Ordering::Relaxed)
                        ));
                    }
                }
                if !visit.active {
                    return;
                }
                if regs.rsi / RECORD_LEN >= visit.count || regs.rdi != visit.list {
                    // Not the loop the copy was made for: the engine's own
                    // list from here, said once (never seen; the list is not
                    // changed inside the loop).
                    visit.active = false;
                    VISIT_REFUSALS.note(FIX, "the node list changed inside the loop");
                    return;
                }
                regs.rdi = visit.sorted.as_ptr() as u64;
            });
        });
    }

    /// Reads the node list through the frame: the record count, and whether
    /// `sorted` now holds the list in entity order (it does not when the
    /// list already was). The measurement notes the engine's order.
    fn begin_loop(
        rbp: u64,
        holder: u64,
        begin: u64,
        sorted: &mut Vec<u64>,
    ) -> Result<(u64, Sorted), &'static str> {
        let mut probe = Probe::new();
        let count: i32 = rbp
            .checked_add_signed(COUNT)
            .and_then(|at| probe.read(at))
            .ok_or("the node count is unreadable")?;
        let count = u64::try_from(count).map_err(|_| "a negative node count")?;
        if count > MAX_RECORDS {
            return Err("more vehicles than any world holds");
        }
        let stored: u64 = probe.read(holder).ok_or("the node list is unreadable")?;
        let end: u64 = holder
            .checked_add(8)
            .and_then(|at| probe.read(at))
            .ok_or("the node list's end is unreadable")?;
        if stored != begin || end < begin || end - begin != count * RECORD_LEN {
            return Err("the node list is not the count's whole records");
        }
        if count < 2 {
            measure::note_visits(&[], Sorted::Unchanged);
            return Ok((count, Sorted::Unchanged));
        }
        let len = usize::try_from(count * RECORD_LEN).map_err(|_| "the list is too long")?;
        if !usize::try_from(begin).is_ok_and(|begin| probe.readable(begin, len)) {
            return Err("the node records are unreadable");
        }
        // SAFETY: `len` readable bytes at `begin`, whole 8-byte records;
        // `i < count` for every index the sort asks for.
        let record =
            |i: u64| unsafe { std::ptr::read_unaligned((begin + i * RECORD_LEN) as *const u64) };
        let outcome = sort_records(count, record, sorted)?;
        if measure::enabled() {
            let before: Vec<i32> = (0..count).map(|i| record(i) as u32 as i32).collect();
            measure::note_visits(&before, outcome);
        }
        Ok((count, outcome))
    }

    pub(super) unsafe extern "system" fn candidates_hook(regs: *mut SavedRegs) {
        let _timer = perf::time(Piece::PlatformCandidates);
        guarded(FIX, &CANDIDATE_BROKEN, || {
            // SAFETY: the stub's block, held until the hook returns.
            let regs = unsafe { &*regs };
            let n = CANDIDATE_CALLS.fetch_add(1, Ordering::Relaxed) + 1;
            match sort_candidates(regs.r13, regs.r14) {
                Ok(sorted) => {
                    if sorted == Sorted::Reordered {
                        CANDIDATE_REORDERS.fetch_add(1, Ordering::Relaxed);
                    }
                    measure::note_candidates(sorted);
                }
                Err(why) => CANDIDATE_REFUSALS.note(FIX, why),
            }
            if n == 1 || n.is_multiple_of(1 << 16) {
                log::line(&format!(
                    "order fix {FIX}: candidates alive, sorts={n} reordered={} refused={}",
                    CANDIDATE_REORDERS.load(Ordering::Relaxed),
                    CANDIDATE_REFUSALS.count.load(Ordering::Relaxed)
                ));
            }
        });
    }

    fn sort_candidates(begin: u64, end: u64) -> Result<Sorted, &'static str> {
        if end < begin || !(end - begin).is_multiple_of(CANDIDATE_LEN) {
            return Err("the candidates are not whole entries");
        }
        let count = (end - begin) / CANDIDATE_LEN;
        if count > MAX_CANDIDATES {
            return Err("more candidates than a station has");
        }
        if count < 2 {
            return Ok(Sorted::Unchanged);
        }
        let len = usize::try_from(count * CANDIDATE_LEN).map_err(|_| "too many candidates")?;
        if !readable(begin, len) {
            return Err("the candidates are unreadable");
        }
        // SAFETY: `len` readable bytes at `begin` (not null: at least two
        // candidates), whole 12-byte entries of alignment 1; the engine's
        // thread is the one running, and its sort reads them only after
        // this site, so nothing else holds them while the slice lives.
        let candidates = unsafe {
            std::slice::from_raw_parts_mut(begin as usize as *mut Candidate, count as usize)
        };
        Ok(sort_candidates_in_place(candidates))
    }
}

/// The vehicles on a road or track edge (the survey's item 3; TPF2's
/// `roadentries`). `transport::EdgeUseManager` keeps, per edge, a vector
/// of 20-byte entries `{int32 entity, int32 component, float back, float
/// front, bool forward}`, and its nearest-occupant searches (`0x255f340`,
/// `0x255ef60`) keep the first entry on an exact tie, so two games whose
/// entries are in different orders can pick a different leader. Every
/// writer, read in the binary:
///
/// - `Add` (`0x255e940`, persons, from `PersonMoveSystem`'s node-added
///   callback) and `AddRange` (`0x255cc70`, vehicles, from
///   `LandVehicleMoveSystem`'s): `push_back`, or an in-place update of an
///   entry the vehicle already has;
/// - `Remove` (`0x2561440`) and `RemoveRange` (`0x2561690`): find, then
///   `memmove` the tail down: order kept;
/// - `RemoveEntity` (`0x2561510`): drops a whole edge;
/// - `GetOrAddEdgeData`: grows an edge list, moving the vectors whole.
///
/// So the entries are append order, which is history while running and
/// registration order after a load; nothing else reorders them. Sorting
/// each touched edge's entries by entity id right after every append keeps
/// every list canonical with no cost per update: the fix detours `Add` and
/// `AddRange` whole, runs the engine's, then sorts the edges it touched
/// (`Add`'s one edge; `AddRange`'s path edges `from..=to`).
///
/// The two appenders take the manager's data differently: `Add`'s `this`
/// is the manager, whose data is at `[this+0x18]` (`Add` gets it through
/// the copy-on-write getter `0x255f0b0`, which answers `[this+0x18]`);
/// `AddRange`'s `this` is that data already (its one caller, `0x255edc0`,
/// calls the getter and passes its answer), with the manager as its ninth
/// argument. Until 2026-09-30 the fix read `[this+0x18]` for both, which
/// in `AddRange` is the data's slot vector, so every vehicle append was
/// refused with "the edge's entity has no slot" (70% of the appends in the
/// three-player playtest) and the vehicles' lists were never sorted.
///
/// Cheap per append: the eight words from the data to an edge's entries
/// are checked through the per-thread region cache
/// ([`crate::image::Readable`]), so a region is asked of the system about
/// once per update, not once a word; a list that was in order before the
/// append needs one scan and, at most, the new entry moved into place
/// ([`place`]); nothing is allocated unless a list was out of order.
pub mod road {
    use super::*;

    pub const FIX: &str = "road-entry-order";
    /// Set to `0` (or `off`), the entries keep the engine's order (the
    /// measurement still installs the detours when it is on).
    pub const TOGGLE_ENV: &str = "TPF3MP_HOOK_ROAD_ENTRY_ORDER";
    pub use crate::build_data::native::order::road::ADD;
    pub use crate::build_data::native::order::road::ADD_RANGE;
    pub use crate::build_data::native::order::road::EDGE_DATA_LEN;
    pub use crate::build_data::native::order::road::EDGE_ID_LEN;
    pub use crate::build_data::native::order::road::ENTRY_LEN;
    pub use crate::build_data::native::order::road::SLOT_LEN;
    pub const MAX_ENTRIES: u64 = 1 << 16;
    pub const MAX_PATH: u64 = 1 << 20;

    static SORTING: AtomicBool = AtomicBool::new(false);
    static BROKEN: AtomicBool = AtomicBool::new(false);
    static REFUSALS: Refusals = Refusals::new();
    static CALLS: AtomicU64 = AtomicU64::new(0);
    static REORDERS: AtomicU64 = AtomicU64::new(0);
    static ADD_ORIGINAL: AtomicUsize = AtomicUsize::new(0);
    static ADD_RANGE_ORIGINAL: AtomicUsize = AtomicUsize::new(0);

    /// `Add(this, &edgeId, entity, component, {back, front})`, five
    /// arguments; three more slots are forwarded, unused.
    type AddFn =
        unsafe extern "system" fn(usize, usize, usize, usize, usize, usize, usize, usize) -> usize;
    /// `AddRange(this, entity, component, &pathEdges, currentIndex,
    /// {back, front}, from, to, context)`: nine arguments (the last at the
    /// caller's `[rsp+0x48]`, read as `[rbp+0x140]` in the function).
    type AddRangeFn = unsafe extern "system" fn(
        usize,
        usize,
        usize,
        usize,
        usize,
        usize,
        usize,
        usize,
        usize,
    ) -> usize;

    /// Installs the two detours when the fix is wanted or the measurement
    /// is on (the `appends` lane is fed from here).
    pub fn install(resolved: &ResolvedProfile, wanted: bool, measuring: bool) -> Vec<Outcome> {
        if !wanted && !measuring {
            return vec![Outcome {
                fix: FIX,
                installed: false,
                reason: format!(
                    "{TOGGLE_ENV} says so and nothing is measured; the entries keep the engine's order"
                ),
            }];
        }
        let mut outcomes = Vec::new();
        let targets: [(&str, *const u8, &AtomicUsize); 2] = [
            (ADD, add as *const u8, &ADD_ORIGINAL),
            (ADD_RANGE, add_range as *const u8, &ADD_RANGE_ORIGINAL),
        ];
        let mut installed = 0;
        for (name, detour, original) in targets {
            let Some(target) = resolved.get(name) else {
                outcomes.push(Outcome {
                    fix: FIX,
                    installed: false,
                    reason: format!("the profile has no {name:?}"),
                });
                continue;
            };
            // SAFETY: a function the profile resolved and prologue-checked,
            // detoured before any world exists; each detour has the target's
            // ABI with every argument forwarded (integer and pointer
            // registers; the floats ride in stack slots, forwarded whole).
            match unsafe { InlineDetour::install(target.address as usize as *mut u8, detour) } {
                Ok(detoured) => {
                    original.store(detoured.trampoline() as usize, Ordering::Release);
                    let _kept = std::mem::ManuallyDrop::new(detoured);
                    installed += 1;
                    outcomes.push(Outcome {
                        fix: FIX,
                        installed: wanted,
                        reason: format!(
                            "{name} at {:#x} detoured: {}",
                            target.address,
                            if wanted {
                                "the edge lists it appends to are kept in entity order"
                            } else {
                                "measured only"
                            }
                        ),
                    });
                }
                Err(error) => outcomes.push(Outcome {
                    fix: FIX,
                    installed: false,
                    reason: format!("{name} at {:#x}: {error}", target.address),
                }),
            }
        }
        // Both appenders or neither: one sorted and one not is no order.
        if wanted && installed == 2 {
            SORTING.store(true, Ordering::Release);
        } else if wanted {
            outcomes.push(Outcome {
                fix: FIX,
                installed: false,
                reason: "not both appenders are detoured; the entries keep the engine's order"
                    .to_owned(),
            });
        }
        outcomes
    }

    pub use crate::build_data::native::order::road::MANAGER_DATA;

    /// One entry, as the engine lays it out.
    pub type Entry = [u8; ENTRY_LEN as usize];

    /// An entry's entity id: its first dword.
    #[inline]
    pub fn key(entry: &Entry) -> i32 {
        i32::from_le_bytes([entry[0], entry[1], entry[2], entry[3]])
    }

    /// The order the fix keeps: the entries by entity id, each whole;
    /// `None` when they already are in it. The reference for [`place`],
    /// which the hook runs.
    pub fn entry_order(entries: &[Entry]) -> Result<Option<Vec<Entry>>, &'static str> {
        let keys: Vec<i32> = entries.iter().map(key).collect();
        Ok(canonical_permutation(&keys)?.map(|order| order.iter().map(|&i| entries[i]).collect()))
    }

    /// Puts `entries` in the order [`entry_order`] gives, in place, and
    /// writes nothing when it refuses. One scan finds how far the list is
    /// strictly ascending. A list sorted before the append (every list the
    /// fix has kept) is either whole, or out of order only in its last
    /// entry, the one appended: that entry's place is found by binary
    /// search and the tail after it moves up one. Anything else (a list the
    /// fix never kept) is sorted whole through `scratch`, reused from call
    /// to call, and written back only when no two entries name one entity.
    pub fn place(entries: &mut [Entry], scratch: &mut Vec<Entry>) -> Result<Sorted, &'static str> {
        let n = entries.len();
        let mut prefix = 1;
        while prefix < n && key(&entries[prefix - 1]) < key(&entries[prefix]) {
            prefix += 1;
        }
        if prefix >= n {
            return Ok(Sorted::Unchanged);
        }
        if prefix == n - 1 {
            let last = key(&entries[n - 1]);
            return match entries[..n - 1].binary_search_by_key(&last, key) {
                Ok(_) => Err("two entries name one entity"),
                Err(at) => {
                    entries[at..].rotate_right(1);
                    Ok(Sorted::Reordered)
                }
            };
        }
        scratch.clear();
        scratch.extend_from_slice(entries);
        // Unique keys (checked next) are a total order: stable or not, one
        // result.
        scratch.sort_unstable_by_key(key);
        if scratch
            .windows(2)
            .any(|pair| key(&pair[0]) == key(&pair[1]))
        {
            return Err("two entries name one entity");
        }
        entries.copy_from_slice(scratch);
        Ok(Sorted::Reordered)
    }

    thread_local! {
        /// [`place`]'s buffer for a list out of order in more than its last
        /// entry.
        static SCRATCH: std::cell::RefCell<Vec<Entry>> = const { std::cell::RefCell::new(Vec::new()) };
    }

    /// The entries vector (`&begin`) of the edge `edge_id` names, through
    /// the manager's data (`EdgeUseManagerData*`) the way `GetOrAddEdgeData`
    /// (`0x255d5a0`, `0x255d740`) walks it: its entity-to-slot index
    /// `[data+0]..[data+8]` (int32s); its slots `[data+0x18]..[data+0x20]`
    /// (72 bytes each); the slot's edges `[slot]..[slot+8]` (32 bytes each);
    /// the entries at `edge+8`.
    fn entries_of(probe: &mut Probe, data: u64, edge_id: u64) -> Result<u64, &'static str> {
        let (entity, index): (i32, i32) = (
            probe.read(edge_id).ok_or("the edge id is unreadable")?,
            edge_id
                .checked_add(4)
                .and_then(|at| probe.read(at))
                .ok_or("the edge id is unreadable")?,
        );
        let entity = u64::try_from(entity).map_err(|_| "a negative edge entity")?;
        let index = u64::try_from(index).map_err(|_| "a negative edge index")?;
        let slots_of: u64 = probe.read(data).ok_or("the slot index is unreadable")?;
        let slots_end: u64 = data
            .checked_add(8)
            .and_then(|at| probe.read(at))
            .ok_or("the slot index is unreadable")?;
        if slots_end < slots_of || entity >= (slots_end - slots_of) / 4 {
            return Err("the edge's entity has no slot");
        }
        let slot: i32 = probe
            .read(slots_of + entity * 4)
            .ok_or("the slot index is unreadable")?;
        let slot = u64::try_from(slot).map_err(|_| "the edge's entity has no slot")?;
        let slots: u64 = data
            .checked_add(0x18)
            .and_then(|at| probe.read(at))
            .ok_or("the slots are unreadable")?;
        let slots_last: u64 = data
            .checked_add(0x20)
            .and_then(|at| probe.read(at))
            .ok_or("the slots are unreadable")?;
        if slots_last < slots
            || !(slots_last - slots).is_multiple_of(SLOT_LEN)
            || slot >= (slots_last - slots) / SLOT_LEN
        {
            return Err("the slots are not whole slots");
        }
        let at = slots + slot * SLOT_LEN;
        let edges: u64 = probe.read(at).ok_or("the slot's edges are unreadable")?;
        let edges_end: u64 = probe
            .read(at + 8)
            .ok_or("the slot's edges are unreadable")?;
        if edges_end < edges
            || !(edges_end - edges).is_multiple_of(EDGE_DATA_LEN)
            || index >= (edges_end - edges) / EDGE_DATA_LEN
        {
            return Err("the edge index is past the slot's edges");
        }
        Ok(edges + index * EDGE_DATA_LEN + 8)
    }

    /// Sorts the entries of the edge `edge_id` names by entity id, in place,
    /// in the manager's data `data`.
    pub(crate) fn sort_edge(
        probe: &mut Probe,
        data: u64,
        edge_id: u64,
    ) -> Result<Sorted, &'static str> {
        let vector = entries_of(probe, data, edge_id)?;
        let begin: u64 = probe.read(vector).ok_or("the entries are unreadable")?;
        let end: u64 = vector
            .checked_add(8)
            .and_then(|at| probe.read(at))
            .ok_or("the entries are unreadable")?;
        if end < begin || !(end - begin).is_multiple_of(ENTRY_LEN) {
            return Err("the entries are not whole entries");
        }
        let count = (end - begin) / ENTRY_LEN;
        if count > MAX_ENTRIES {
            return Err("more entries on one edge than any world holds");
        }
        let len = usize::try_from(count * ENTRY_LEN).map_err(|_| "too many entries")?;
        let begin = usize::try_from(begin).map_err(|_| "the entries are unreadable")?;
        if !probe.readable(begin, len) {
            return Err("the entries are unreadable");
        }
        let entries: &mut [Entry] = if count == 0 {
            &mut []
        } else {
            // SAFETY: `len` readable bytes at `begin` (not null: readable),
            // whole 20-byte entries of alignment 1; on the thread that just
            // appended to them, inside the engine's own call chain (the
            // node-added callbacks run serially at the end of a
            // modification), so nothing else reads or writes them while the
            // slice lives.
            unsafe { std::slice::from_raw_parts_mut(begin as *mut Entry, count as usize) }
        };
        let sorted = SCRATCH.with(|scratch| place(entries, &mut scratch.borrow_mut()))?;
        if measure::enabled() {
            let edge: [u8; EDGE_ID_LEN as usize] =
                probe.read(edge_id).ok_or("the edge id is unreadable")?;
            let ids: Vec<i32> = entries.iter().map(key).collect();
            measure::note_road(&edge, &ids, sorted);
        }
        Ok(sorted)
    }

    /// For the tests: the detours call `add` and `add_range` as the
    /// engine's, and sort after them.
    #[cfg(test)]
    pub(super) fn arm_for_test(add: usize, add_range: usize) {
        ADD_ORIGINAL.store(add, Ordering::Release);
        ADD_RANGE_ORIGINAL.store(add_range, Ordering::Release);
        SORTING.store(add != 0 || add_range != 0, Ordering::Release);
    }

    /// The road fix's refusals by reason since the last call (the `perf:`
    /// line's).
    pub fn take_refusals() -> Vec<(&'static str, u64)> {
        REFUSALS.take_window()
    }

    /// An edge id's meaningful bytes: entity, index, direction.
    fn edge_key(probe: &mut Probe, edge_id: u64) -> Option<crate::roadtrace::EdgeKey> {
        Some((
            probe.read(edge_id)?,
            probe.read(edge_id.checked_add(4)?)?,
            probe.read(edge_id.checked_add(8)?)?,
        ))
    }

    /// The entries of the edge `edge_id` names, as the road entry trace
    /// lists them: entity, component, back, front.
    fn listed(probe: &mut Probe, data: u64, edge_id: u64) -> Option<Vec<crate::roadtrace::Listed>> {
        let vector = entries_of(probe, data, edge_id).ok()?;
        let begin: u64 = probe.read(vector)?;
        let end: u64 = probe.read(vector.checked_add(8)?)?;
        if end < begin
            || !(end - begin).is_multiple_of(ENTRY_LEN)
            || end - begin > MAX_ENTRIES * ENTRY_LEN
        {
            return None;
        }
        (0..(end - begin) / ENTRY_LEN)
            .map(|i| {
                let at = begin + i * ENTRY_LEN;
                Some((
                    probe.read::<i32>(at)?,
                    probe.read::<i32>(at + 4)?,
                    probe.read::<f32>(at + 8)?,
                    probe.read::<f32>(at + 12)?,
                ))
            })
            .collect()
    }

    /// What was appended, for the road entry trace (`crate::roadtrace`):
    /// everything but its edges, which [`sorted`] reads.
    struct Appended {
        kind: crate::roadtrace::Kind,
        entity: i32,
        component: i32,
        current: Option<i32>,
        range: Option<(i32, i32)>,
        bounds: u64,
    }

    fn sorted(
        probe: &mut Probe,
        data: u64,
        appended: Appended,
        edge_ids: impl Iterator<Item = u64>,
    ) {
        guarded(FIX, &BROKEN, || {
            let n = CALLS.fetch_add(1, Ordering::Relaxed) + 1;
            let tracing = in_step() && crate::roadtrace::enabled();
            let mut traced_edges = Vec::new();
            let mut reordered_now = 0;
            for edge_id in edge_ids {
                if tracing {
                    traced_edges.push(edge_id);
                }
                match sort_edge(probe, data, edge_id) {
                    Ok(Sorted::Reordered) => {
                        reordered_now += 1;
                        let reorders = REORDERS.fetch_add(1, Ordering::Relaxed) + 1;
                        if reorders <= 3 {
                            log::line(&format!(
                                "order fix {FIX}: an edge's entries put in entity order (append #{n})"
                            ));
                        }
                    }
                    Ok(Sorted::Unchanged) => {}
                    Err(why) => REFUSALS.note(FIX, why),
                }
            }
            if tracing {
                // The road entry trace (logging only): the simulation's own
                // appends, which two agreeing games make alike.
                let step = crate::seeds::current_step();
                let append = crate::roadtrace::Append {
                    kind: appended.kind,
                    entity: appended.entity,
                    component: appended.component,
                    current: appended.current,
                    range: appended.range,
                    bounds: appended.bounds,
                    edges: traced_edges
                        .iter()
                        .filter_map(|&id| edge_key(probe, id))
                        .collect(),
                };
                let entries: Option<Vec<_>> =
                    crate::roadtrace::wants_entries(appended.entity, step).then(|| {
                        traced_edges
                            .iter()
                            .filter_map(|&id| Some((edge_key(probe, id)?, listed(probe, data, id))))
                            .collect()
                    });
                crate::roadtrace::note(step, &append, reordered_now, entries.as_deref());
            }
            if n == 1 || n.is_multiple_of(1 << 16) {
                log::line(&format!(
                    "order fix {FIX}: alive, appends={n} reordered={} refused={}",
                    REORDERS.load(Ordering::Relaxed),
                    REFUSALS.count.load(Ordering::Relaxed)
                ));
            }
        });
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) unsafe extern "system" fn add(
        this: usize,
        edge_id: usize,
        entity: usize,
        component: usize,
        bounds: usize,
        s6: usize,
        s7: usize,
        s8: usize,
    ) -> usize {
        if measure::enabled() {
            measure::note_add(
                edge_id as u64,
                entity as u32,
                component as u32,
                bounds as u64,
            );
        }
        let original = ADD_ORIGINAL.load(Ordering::Acquire);
        if original == 0 {
            return 0;
        }
        // SAFETY: the trampoline of the function this detour replaced, every
        // argument forwarded.
        let original: AddFn = unsafe { std::mem::transmute::<usize, AddFn>(original) };
        let result = unsafe { original(this, edge_id, entity, component, bounds, s6, s7, s8) };
        if SORTING.load(Ordering::Acquire) {
            let _timer = perf::time(Piece::RoadEntry);
            let mut probe = Probe::new();
            // `Add`'s `this` is the manager: its data at `[this+0x18]`.
            match (this as u64)
                .checked_add(MANAGER_DATA)
                .and_then(|at| probe.read::<u64>(at))
            {
                Some(data) => sorted(
                    &mut probe,
                    data,
                    Appended {
                        kind: crate::roadtrace::Kind::Person,
                        entity: entity as u32 as i32,
                        component: component as u32 as i32,
                        current: None,
                        range: None,
                        bounds: bounds as u64,
                    },
                    std::iter::once(edge_id as u64),
                ),
                None => REFUSALS.note(FIX, "the manager's data is unreadable"),
            }
        }
        result
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) unsafe extern "system" fn add_range(
        this: usize,
        entity: usize,
        component: usize,
        path: usize,
        current: usize,
        bounds: usize,
        from: usize,
        to: usize,
        context: usize,
    ) -> usize {
        if measure::enabled() {
            // The context is an address: not hashed.
            measure::note_add_range(
                [
                    entity as u32,
                    component as u32,
                    current as u32,
                    bounds as u32,
                    from as u32,
                    to as u32,
                ],
                path as u64,
            );
        }
        let original = ADD_RANGE_ORIGINAL.load(Ordering::Acquire);
        if original == 0 {
            return 0;
        }
        // SAFETY: as for `add`, the ninth argument included.
        let original: AddRangeFn = unsafe { std::mem::transmute::<usize, AddRangeFn>(original) };
        let result = unsafe {
            original(
                this, entity, component, path, current, bounds, from, to, context,
            )
        };
        if SORTING.load(Ordering::Acquire) {
            let _timer = perf::time(Piece::RoadEntry);
            let mut probe = Probe::new();
            let data = this as u64;
            // `AddRange`'s `this` is the manager's data itself; its ninth
            // argument, the manager, names it at `[+0x18]`.
            let checked = (context as u64)
                .checked_add(MANAGER_DATA)
                .and_then(|at| probe.read::<u64>(at))
                == Some(data);
            if !checked {
                REFUSALS.note(FIX, "AddRange's data is not its manager's");
            } else {
                match path_edges(
                    &mut probe,
                    path as u64,
                    from as u32 as i32,
                    to as u32 as i32,
                ) {
                    Ok(edges) => sorted(
                        &mut probe,
                        data,
                        Appended {
                            kind: crate::roadtrace::Kind::Vehicle,
                            entity: entity as u32 as i32,
                            component: component as u32 as i32,
                            current: Some(current as u32 as i32),
                            range: Some((from as u32 as i32, to as u32 as i32)),
                            bounds: bounds as u64,
                        },
                        edges,
                    ),
                    Err(why) => REFUSALS.note(FIX, why),
                }
            }
        }
        result
    }

    /// The addresses of `path[from..=to]`'s edge ids, checked against the
    /// path vector's bounds.
    fn path_edges(
        probe: &mut Probe,
        path: u64,
        from: i32,
        to: i32,
    ) -> Result<impl Iterator<Item = u64> + use<>, &'static str> {
        let (from, to) = (
            u64::try_from(from).map_err(|_| "a negative path range")?,
            u64::try_from(to).map_err(|_| "a negative path range")?,
        );
        let begin: u64 = probe.read(path).ok_or("the path is unreadable")?;
        let end: u64 = path
            .checked_add(8)
            .and_then(|at| probe.read(at))
            .ok_or("the path is unreadable")?;
        if end < begin || !(end - begin).is_multiple_of(EDGE_ID_LEN) {
            return Err("the path is not whole edge ids");
        }
        let len = (end - begin) / EDGE_ID_LEN;
        if to < from || to >= len || len > MAX_PATH {
            return Err("the path range is past the path");
        }
        Ok((from..=to).map(move |i| begin + i * EDGE_ID_LEN))
    }
}

/// The measurement (survey items 2 and 3, and the two fixes' own evidence):
/// with [`MEASURE_ENV`] set, whole-function detours on the reservation
/// manager's two `Reserve` overloads, the road-edge appenders and
/// `ecs::Engine::Update` hash, per simulation update, the sequence of
/// (reserver entity, edge) claims and of (edge, entity) appends, and
/// the two sort sites hash the order they found. Every `interval` updates
/// one line goes to hook.log with the update number and the hashes since
/// the last line; two replicas' lines can be diffed. Updates are numbered
/// from the room's step once the step driver says which it is
/// ([`measure::room_step`]); before that, from the hook's start.
pub mod measure {
    use super::*;

    pub use crate::build_data::native::order::measure::EDGE_USE_ADD;
    pub use crate::build_data::native::order::measure::EDGE_USE_ADD_RANGE;
    pub use crate::build_data::native::order::measure::ENGINE_UPDATE;
    pub use crate::build_data::native::order::measure::RESERVE;
    pub use crate::build_data::native::order::measure::RESERVE_SIMPLE;
    pub const DEFAULT_INTERVAL: u64 = 100;
    use crate::build_data::native::order::measure::EDGE_LEN;
    /// A sanity bound on one reservation's edge count.
    const MAX_EDGES: u64 = 1 << 16;

    /// The hashes of one update, and how many items went into each.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct Lanes {
        pub claims: Fnv1a,
        pub claim_count: u64,
        pub appends: Fnv1a,
        pub append_count: u64,
        pub land_vehicles: Fnv1a,
        pub land_vehicle_calls: u64,
        pub land_vehicle_reorders: u64,
        /// The land-vehicle shuffle's seeds (the game's tickCount), one per
        /// call of the sort site.
        pub land_seeds: Fnv1a,
        /// The land-vehicle family's node list, entity by entity, in its
        /// own order (hashed where at least two vehicles want track): the
        /// order the engine's shuffle permutes, and the survey's item 3.
        pub land_nodes: Fnv1a,
        pub land_node_count: u64,
        pub vehicles_at_stop: Fnv1a,
        pub vehicle_stop_calls: u64,
        pub vehicle_stop_reorders: u64,
        /// The platform chooser's visit order as the engine had it
        /// ([`super::platform`]), one hash per update.
        pub visits: Fnv1a,
        pub visit_calls: u64,
        pub visit_reorders: u64,
        /// The terminal candidates put in canonical order before the cost
        /// sort, and how many of those sorts changed something.
        pub candidate_sorts: u64,
        pub candidate_reorders: u64,
        /// Each edge whose entries the road fix checked after an append
        /// ([`super::road`]): its id and its entities in the order kept.
        pub road: Fnv1a,
        pub road_sorts: u64,
        pub road_reorders: u64,
    }

    impl Lanes {
        pub const fn new() -> Self {
            Self {
                claims: Fnv1a::new(),
                claim_count: 0,
                appends: Fnv1a::new(),
                append_count: 0,
                land_vehicles: Fnv1a::new(),
                land_vehicle_calls: 0,
                land_vehicle_reorders: 0,
                land_seeds: Fnv1a::new(),
                land_nodes: Fnv1a::new(),
                land_node_count: 0,
                vehicles_at_stop: Fnv1a::new(),
                vehicle_stop_calls: 0,
                vehicle_stop_reorders: 0,
                visits: Fnv1a::new(),
                visit_calls: 0,
                visit_reorders: 0,
                candidate_sorts: 0,
                candidate_reorders: 0,
                road: Fnv1a::new(),
                road_sorts: 0,
                road_reorders: 0,
            }
        }

        /// One log line: the update it closes and every lane.
        pub fn line(&self, update: u64, interval: u64) -> String {
            format!(
                "order measure: updates {}..={update}: claims={:016x}/{} appends={:016x}/{} land={:016x}/{} reordered {} seeds={:016x} nodes={:016x}/{} vehstop={:016x}/{} reordered {} visits={:016x}/{} reordered {} candidates={}/{} road={:016x}/{} reordered {}",
                update.saturating_sub(interval.saturating_sub(1)),
                self.claims.0,
                self.claim_count,
                self.appends.0,
                self.append_count,
                self.land_vehicles.0,
                self.land_vehicle_calls,
                self.land_vehicle_reorders,
                self.land_seeds.0,
                self.land_nodes.0,
                self.land_node_count,
                self.vehicles_at_stop.0,
                self.vehicle_stop_calls,
                self.vehicle_stop_reorders,
                self.visits.0,
                self.visit_calls,
                self.visit_reorders,
                self.candidate_reorders,
                self.candidate_sorts,
                self.road.0,
                self.road_sorts,
                self.road_reorders,
            )
        }
    }

    impl Default for Lanes {
        fn default() -> Self {
            Self::new()
        }
    }

    static ENABLED: AtomicBool = AtomicBool::new(false);
    static INTERVAL: AtomicU64 = AtomicU64::new(DEFAULT_INTERVAL);
    /// The update the engine is in; `Engine::Update`'s detour advances it.
    static UPDATE: AtomicU64 = AtomicU64::new(0);
    static LANES: Mutex<Lanes> = Mutex::new(Lanes::new());
    static RESERVE_ORIGINAL: AtomicUsize = AtomicUsize::new(0);
    static RESERVE_SIMPLE_ORIGINAL: AtomicUsize = AtomicUsize::new(0);
    static UPDATE_ORIGINAL: AtomicUsize = AtomicUsize::new(0);

    /// Reads [`MEASURE_ENV`]; `true` when measuring.
    pub fn configure_from_env() -> bool {
        let (enabled, interval) = configure(std::env::var(MEASURE_ENV).ok().as_deref());
        ENABLED.store(enabled, Ordering::Release);
        INTERVAL.store(interval, Ordering::Release);
        enabled
    }

    /// Off when unset or empty; a number above 1 is the interval, anything
    /// else means on at the default interval.
    pub fn configure(value: Option<&str>) -> (bool, u64) {
        match value.map(str::trim) {
            None | Some("") => (false, DEFAULT_INTERVAL),
            Some(text) => match text.parse::<u64>() {
                Ok(n) if n > 1 => (true, n),
                _ => (true, DEFAULT_INTERVAL),
            },
        }
    }

    pub fn enabled() -> bool {
        ENABLED.load(Ordering::Acquire)
    }

    /// The step driver says the next update is the room's step `step`:
    /// from here the log's update numbers are the room's.
    pub fn room_step(step: u64) {
        UPDATE.store(step.saturating_sub(1), Ordering::Release);
    }

    fn lanes() -> std::sync::MutexGuard<'static, Lanes> {
        LANES.lock().unwrap_or_else(|p| p.into_inner())
    }

    pub(super) fn note_land_nodes(order: impl Iterator<Item = u32>) {
        let mut lanes = lanes();
        for entity in order {
            lanes.land_nodes.write_u32(entity);
            lanes.land_node_count += 1;
        }
        lanes.land_nodes.write(b"|");
    }

    pub(super) fn note_land_vehicles(before: &[u32], sorted: Sorted, seed: u32) {
        if !enabled() {
            return;
        }
        let mut lanes = lanes();
        lanes.land_seeds.write_u32(seed);
        lanes.land_vehicle_calls += 1;
        lanes.land_vehicles.write_u32(before.len() as u32);
        for key in before {
            lanes.land_vehicles.write_u32(*key);
        }
        if sorted == Sorted::Reordered {
            lanes.land_vehicle_reorders += 1;
        }
    }

    pub(super) fn note_vehicles_at_stop(before: &[i32], sorted: Sorted) {
        if !enabled() {
            return;
        }
        let mut lanes = lanes();
        lanes.vehicle_stop_calls += 1;
        lanes.vehicles_at_stop.write_u32(before.len() as u32);
        for id in before {
            lanes.vehicles_at_stop.write_u32(*id as u32);
        }
        if sorted == Sorted::Reordered {
            lanes.vehicle_stop_reorders += 1;
        }
    }

    pub(super) fn note_visits(before: &[i32], sorted: Sorted) {
        if !enabled() {
            return;
        }
        let mut lanes = lanes();
        lanes.visit_calls += 1;
        lanes.visits.write_u32(before.len() as u32);
        for id in before {
            lanes.visits.write_u32(*id as u32);
        }
        if sorted == Sorted::Reordered {
            lanes.visit_reorders += 1;
        }
    }

    pub(super) fn note_candidates(sorted: Sorted) {
        if !enabled() {
            return;
        }
        let mut lanes = lanes();
        lanes.candidate_sorts += 1;
        if sorted == Sorted::Reordered {
            lanes.candidate_reorders += 1;
        }
    }

    /// One edge the road fix checked: its id and the entities on it in the
    /// order they are kept.
    pub(super) fn note_road(edge: &[u8], kept: &[i32], sorted: Sorted) {
        if !enabled() {
            return;
        }
        let mut lanes = lanes();
        lanes.road_sorts += 1;
        lanes.road.write(edge);
        lanes.road.write_u32(kept.len() as u32);
        for id in kept {
            lanes.road.write_u32(*id as u32);
        }
        if sorted == Sorted::Reordered {
            lanes.road_reorders += 1;
        }
    }

    /// One claim: `Reserve(this, engine, typeIndex, entity, &path, from,
    /// to)` reserves `path[from..to)` for `entity`. Hashes the entity and
    /// each edge's twelve bytes; an unreadable path hashes as a marker.
    pub(super) fn note_claim(entity: u32, path: u64, from: i32, to: i32) {
        let mut lanes = lanes();
        lanes.claim_count += 1;
        lanes.claims.write_u32(entity);
        let edges = claim_edges(path, from, to);
        match edges {
            Some(edges) => {
                lanes.claims.write_u32(edges.len() as u32);
                for edge in edges {
                    lanes.claims.write(&edge);
                }
            }
            None => lanes.claims.write(b"?path"),
        }
    }

    /// The edges of `path[from..to)`, read through readable checks.
    fn claim_edges(path: u64, from: i32, to: i32) -> Option<Vec<[u8; EDGE_LEN as usize]>> {
        let (from, to) = (u64::try_from(from).ok()?, u64::try_from(to).ok()?);
        if to < from || to - from > MAX_EDGES {
            return None;
        }
        let base: u64 = read(path)?;
        let first = base.checked_add(from.checked_mul(EDGE_LEN)?)?;
        let len = usize::try_from((to - from) * EDGE_LEN).ok()?;
        if !readable(first, len) {
            return None;
        }
        Some(
            (0..to - from)
                // SAFETY: `len` readable bytes at `first`, whole edges.
                .map(|i| unsafe {
                    std::ptr::read_unaligned(
                        (first + i * EDGE_LEN) as *const [u8; EDGE_LEN as usize],
                    )
                })
                .collect(),
        )
    }

    /// One append at `EdgeUseManager::Add(this, &edgeId, entity, component,
    /// bounds)`: the edge id's twelve bytes, the entity, the component
    /// index and the bounds' bits.
    pub(super) fn note_add(edge_id: u64, entity: u32, component: u32, bounds: u64) {
        let mut lanes = lanes();
        lanes.append_count += 1;
        lanes.appends.write(b"add");
        match read::<[u8; EDGE_LEN as usize]>(edge_id) {
            Some(edge) => lanes.appends.write(&edge),
            None => lanes.appends.write(b"?edge"),
        }
        lanes.appends.write_u32(entity);
        lanes.appends.write_u32(component);
        lanes.appends.write(&bounds.to_le_bytes());
    }

    /// One `EdgeUseManager::AddRange(this, a2, a3, &edges, a5..a8)`: the
    /// raw integer arguments and the edges vector's bytes (begin at +0,
    /// end at +8), as they are; naming them is not needed for a diff.
    pub(super) fn note_add_range(args: [u32; 6], edges: u64) {
        let mut lanes = lanes();
        lanes.append_count += 1;
        lanes.appends.write(b"range");
        for arg in args {
            lanes.appends.write_u32(arg);
        }
        match range_bytes(edges) {
            Some(bytes) => {
                lanes.appends.write_u32(bytes.len() as u32);
                lanes.appends.write(&bytes);
            }
            None => lanes.appends.write(b"?edges"),
        }
    }

    fn range_bytes(edges: u64) -> Option<Vec<u8>> {
        let begin: u64 = read(edges)?;
        let end: u64 = edges.checked_add(8).and_then(read)?;
        if end < begin || end - begin > MAX_EDGES * EDGE_LEN {
            return None;
        }
        let len = usize::try_from(end - begin).ok()?;
        if !readable(begin, len) {
            return None;
        }
        // SAFETY: `len` readable bytes at `begin`.
        Some(unsafe { std::slice::from_raw_parts(begin as *const u8, len) }.to_vec())
    }

    /// The engine advances one update: the previous one's lanes close, and
    /// every `interval` updates they go to the log.
    pub(crate) fn update_begins() {
        let closed = UPDATE.fetch_add(1, Ordering::AcqRel);
        let interval = INTERVAL.load(Ordering::Acquire).max(1);
        if closed == 0 || !closed.is_multiple_of(interval) {
            return;
        }
        let lanes = std::mem::take(&mut *lanes());
        log::line(&lanes.line(closed, interval));
    }

    type Fn8 =
        unsafe extern "system" fn(usize, usize, usize, usize, usize, usize, usize, usize) -> usize;
    /// `Engine::Update(engine, float dt)`: the second argument rides in
    /// `xmm1`, which a float parameter forwards.
    type UpdateFn = unsafe extern "system" fn(usize, f64, usize, usize) -> usize;

    #[allow(clippy::too_many_arguments)]
    unsafe extern "system" fn reserve(
        this: usize,
        engine: usize,
        type_index: usize,
        entity: usize,
        path: usize,
        from: usize,
        to: usize,
        s7: usize,
    ) -> usize {
        note_claim(
            entity as u32,
            path as u64,
            from as u32 as i32,
            to as u32 as i32,
        );
        let original = RESERVE_ORIGINAL.load(Ordering::Acquire);
        if original == 0 {
            return 0;
        }
        // SAFETY: the trampoline of the function this detour replaced, with
        // every argument forwarded (the three on the stack included).
        let original: Fn8 = unsafe { std::mem::transmute::<usize, Fn8>(original) };
        unsafe { original(this, engine, type_index, entity, path, from, to, s7) }
    }

    #[allow(clippy::too_many_arguments)]
    unsafe extern "system" fn reserve_simple(
        this: usize,
        engine: usize,
        type_index: usize,
        entity: usize,
        path: usize,
        from: usize,
        to: usize,
        s7: usize,
    ) -> usize {
        note_claim(
            entity as u32,
            path as u64,
            from as u32 as i32,
            to as u32 as i32,
        );
        let original = RESERVE_SIMPLE_ORIGINAL.load(Ordering::Acquire);
        if original == 0 {
            return 0;
        }
        // SAFETY: as above.
        let original: Fn8 = unsafe { std::mem::transmute::<usize, Fn8>(original) };
        unsafe { original(this, engine, type_index, entity, path, from, to, s7) }
    }

    unsafe extern "system" fn engine_update(engine: usize, dt: f64, a3: usize, a4: usize) -> usize {
        update_begins();
        let original = UPDATE_ORIGINAL.load(Ordering::Acquire);
        if original == 0 {
            return 0;
        }
        // SAFETY: the trampoline of `Engine::Update`, with the engine and
        // its `dt` (in xmm1) forwarded.
        let original: UpdateFn = unsafe { std::mem::transmute::<usize, UpdateFn>(original) };
        unsafe { original(engine, dt, a3, a4) }
    }

    /// Installs the measurement detours when `measuring`; otherwise says
    /// they are off and how to turn them on.
    pub fn install(resolved: &ResolvedProfile, measuring: bool) -> Vec<Outcome> {
        const FIX: &str = "order-measure";
        if !measuring {
            return vec![Outcome {
                fix: FIX,
                installed: false,
                reason: format!("not measuring ({MEASURE_ENV} is not set); nothing is hooked"),
            }];
        }
        let interval = INTERVAL.load(Ordering::Acquire);
        let mut outcomes = Vec::new();
        // `EdgeUseManager::Add` and `AddRange` are detoured by the road
        // entry fix ([`super::road`]), which feeds the `appends` lane too.
        let targets: [(&str, *const u8, &AtomicUsize); 3] = [
            (ENGINE_UPDATE, engine_update as *const u8, &UPDATE_ORIGINAL),
            (RESERVE, reserve as *const u8, &RESERVE_ORIGINAL),
            (
                RESERVE_SIMPLE,
                reserve_simple as *const u8,
                &RESERVE_SIMPLE_ORIGINAL,
            ),
        ];
        for (name, detour, original) in targets {
            // Seeds owns the update prologue in normal games. Its callback
            // closes these measurements too; installing another detour on
            // the same prologue would fail and leave the lanes unclosed.
            if name == ENGINE_UPDATE && crate::seeds::update_hooked() {
                outcomes.push(Outcome {
                    fix: FIX,
                    installed: true,
                    reason: format!(
                        "{name} is measured through the shared seeds hook, every {interval} updates"
                    ),
                });
                continue;
            }
            let Some(target) = resolved.get(name) else {
                outcomes.push(Outcome {
                    fix: FIX,
                    installed: false,
                    reason: format!("the profile has no {name:?}"),
                });
                continue;
            };
            // SAFETY: a function the profile resolved and prologue-checked,
            // detoured before any world exists (no thread is in it); each
            // detour has the target's ABI with every argument forwarded.
            match unsafe { InlineDetour::install(target.address as usize as *mut u8, detour) } {
                Ok(installed) => {
                    original.store(installed.trampoline() as usize, Ordering::Release);
                    let _kept = std::mem::ManuallyDrop::new(installed);
                    outcomes.push(Outcome {
                        fix: FIX,
                        installed: true,
                        reason: format!(
                            "{name} at {:#x} is measured, every {interval} updates",
                            target.address
                        ),
                    });
                }
                Err(error) => outcomes.push(Outcome {
                    fix: FIX,
                    installed: false,
                    reason: format!("{name} at {:#x}: {error}", target.address),
                }),
            }
        }
        outcomes
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_land_vehicle_sort_orders_entries_by_their_nodes_entity_id() {
        // Records: node 0 is entity 30, node 1 entity 10, node 2 entity 20.
        let entity_of = |node: u32| [30u32, 10, 20].get(node as usize).copied();
        let key_of = move |entry: u64| entity_of(entry as u32);
        let entry =
            |node: u32, priority: f32| u64::from(node) | (u64::from(priority.to_bits()) << 32);
        // The engine's order is node order; the priorities ride along.
        let entries = [entry(0, 1.5), entry(1, 2.5), entry(2, 0.5)];
        let (order, before) = land_vehicle::canonical_order(&entries, key_of).unwrap();
        assert_eq!(before, vec![30, 10, 20]);
        assert_eq!(
            order,
            Some(vec![entry(1, 2.5), entry(2, 0.5), entry(0, 1.5)]),
            "entity 10's entry first, each entry whole"
        );
        // Already in entity order: nothing to write.
        let sorted = [entry(1, 2.5), entry(2, 0.5), entry(0, 1.5)];
        assert_eq!(
            land_vehicle::canonical_order(&sorted, key_of).unwrap(),
            (None, vec![10, 20, 30])
        );
        // One entry: nothing to do.
        assert_eq!(
            land_vehicle::canonical_order(&entries[..1], key_of).unwrap(),
            (None, vec![30])
        );
    }

    #[test]
    fn the_land_vehicle_sort_refuses_what_is_not_a_total_order() {
        let key_of = |entry: u64| [30u32, 10, 10].get(entry as u32 as usize).copied();
        let duplicate = land_vehicle::canonical_order(&[0, 1, 2], key_of);
        assert_eq!(duplicate, Err("two entries name one entity"));
        let out_of_range = land_vehicle::canonical_order(&[0, 7], key_of);
        assert_eq!(out_of_range, Err("an entry indexes no node record"));
    }

    #[test]
    fn the_vehicles_at_a_stop_sort_by_id_and_leave_a_sorted_vector_alone() {
        assert_eq!(terminal::sorted_ids(&[7, 3, 5]), Some(vec![3, 5, 7]));
        assert_eq!(terminal::sorted_ids(&[3, 5, 7]), None);
        assert_eq!(terminal::sorted_ids(&[3, 3]), None);
        assert_eq!(terminal::sorted_ids(&[]), None);
        assert_eq!(terminal::sorted_ids(&[-2, -5]), Some(vec![-5, -2]));
    }

    #[test]
    fn the_site_bytes_are_what_the_profile_checks() {
        // The stolen bytes are the two loads of the vector's bounds; the
        // compare after them stays in place.
        assert_eq!(land_vehicle::STEAL, 8);
        assert_eq!(&land_vehicle::EXPECTED[..4], &[0x4C, 0x8B, 0x6D, 0xE0]);
        assert_eq!(&land_vehicle::EXPECTED[4..8], &[0x48, 0x8B, 0x75, 0xE8]);
        assert_eq!(terminal::STEAL, 8);
        // The profile's prologues for the two sites are these bytes.
        let profile =
            tpf3mp_hookcore::profile::Profile::from_toml(crate::BUILT_IN_PROFILES[0].1).unwrap();
        let prologue = |name: &str| {
            profile
                .targets
                .iter()
                .find(|t| t.name == name)
                .unwrap_or_else(|| panic!("{name} in the profile"))
                .prologue
                .clone()
        };
        assert_eq!(
            prologue(land_vehicle::SITE),
            land_vehicle::EXPECTED.to_vec()
        );
        assert_eq!(prologue(terminal::SITE), terminal::EXPECTED.to_vec());
        assert_eq!(
            prologue(platform::VISIT_SITE),
            platform::VISIT_EXPECTED.to_vec()
        );
        assert_eq!(
            prologue(platform::CANDIDATES_SITE),
            platform::CANDIDATES_EXPECTED.to_vec()
        );
        for name in [
            land_vehicle::RECORDS,
            terminal::GETTER,
            measure::RESERVE,
            measure::RESERVE_SIMPLE,
            measure::EDGE_USE_ADD,
            measure::EDGE_USE_ADD_RANGE,
            measure::ENGINE_UPDATE,
        ] {
            assert!(!prologue(name).is_empty(), "{name} in the profile");
        }
    }

    #[test]
    fn fnv1a_is_the_reference_hash() {
        // The FNV-1a 64-bit test vectors.
        let mut empty = Fnv1a::new();
        empty.write(b"");
        assert_eq!(empty.0, 0xcbf2_9ce4_8422_2325);
        let mut a = Fnv1a::new();
        a.write(b"a");
        assert_eq!(a.0, 0xaf63_dc4c_8601_ec8c);
        let mut foobar = Fnv1a::new();
        foobar.write(b"foobar");
        assert_eq!(foobar.0, 0x8594_4171_f739_67e8);
    }

    #[test]
    fn measuring_is_off_unless_the_environment_says_so() {
        assert_eq!(measure::configure(None), (false, 100));
        assert_eq!(measure::configure(Some("")), (false, 100));
        assert_eq!(measure::configure(Some("1")), (true, 100));
        assert_eq!(measure::configure(Some("yes")), (true, 100));
        assert_eq!(measure::configure(Some("250")), (true, 250));
        assert_eq!(measure::configure(Some(" 25 ")), (true, 25));
    }

    #[test]
    fn a_measurement_line_names_the_updates_and_every_lane() {
        let mut lanes = measure::Lanes::new();
        lanes.claim_count = 3;
        lanes.claims.write(b"x");
        let line = lanes.line(300, 100);
        assert!(line.starts_with("order measure: updates 201..=300: claims="));
        assert!(line.contains("/3 appends="));
        assert!(line.contains("land="));
        assert!(line.contains(" seeds="));
        assert!(line.contains(" nodes="));
        assert!(line.contains("vehstop="));
    }

    #[test]
    fn the_land_vehicle_sample_is_chosen_by_the_seeds_value_and_hashes_the_sorted_ids() {
        assert_eq!(land_vehicle::sample_line(255, &[3, 1]), None);
        assert_eq!(land_vehicle::sample_line(257, &[3, 1]), None);
        let line = land_vehicle::sample_line(512, &[30, 10, 20]).unwrap();
        assert!(
            line.starts_with("order fix land-vehicle-order: sample seed=512 n=3 ids="),
            "{line}"
        );
        // The engine's order before the sort does not change the line: two
        // games whose node lists differ in order, and that the fix puts in
        // one order, log the same.
        assert_eq!(
            land_vehicle::sample_line(512, &[10, 20, 30]),
            Some(line.clone())
        );
        let mut hash = Fnv1a::new();
        for id in [10u32, 20, 30] {
            hash.write_u32(id);
        }
        assert!(line.ends_with(&format!("{:016x}", hash.0)), "{line}");
        assert_ne!(land_vehicle::sample_line(512, &[10, 20]), Some(line));
        assert!(
            land_vehicle::sample_line(0, &[])
                .unwrap()
                .contains("seed=0 n=0 ")
        );
    }

    #[test]
    fn an_outcome_reads_as_a_log_line() {
        let on = Outcome {
            fix: "x",
            installed: true,
            reason: "because".into(),
        };
        assert_eq!(on.to_string(), "order fix x: installed (because)");
        let off = Outcome {
            fix: "x",
            installed: false,
            reason: "the profile has no site".into(),
        };
        assert_eq!(off.to_string(), "order fix x: off, the profile has no site");
    }

    #[test]
    fn nothing_installs_without_the_sites() {
        let resolved = ResolvedProfile {
            name: "empty".into(),
            targets: Vec::new(),
            absent_optional: Vec::new(),
        };
        let fixes = [
            land_vehicle::install(&resolved, true),
            terminal::install(&resolved, true),
        ];
        for outcome in &fixes {
            assert!(!outcome.installed, "{outcome}");
            assert!(outcome.reason.contains("the profile has no"), "{outcome}");
        }
        let off = measure::install(&resolved, false);
        assert_eq!(off.len(), 1);
        assert!(!off[0].installed);
        assert!(off[0].reason.contains(MEASURE_ENV));
        let on = measure::install(&resolved, true);
        assert_eq!(on.len(), 3, "one outcome per measured function");
        assert!(
            on.iter()
                .all(|o| !o.installed && o.reason.contains("the profile has no"))
        );
        // The platform fix: both sites, each off on its own; and its switch.
        let platform = platform::install(&resolved, true);
        assert_eq!(platform.len(), 2);
        assert!(platform.iter().all(|o| !o.installed));
        let switched = platform::install(&resolved, false);
        assert!(switched[0].reason.contains(platform::TOGGLE_ENV));
        // The road fix: no appenders, no sorting; switched off and not
        // measuring, nothing is hooked.
        let road = road::install(&resolved, true, false);
        assert!(road.iter().all(|o| !o.installed), "{road:?}");
        assert!(road.iter().any(|o| o.reason.contains("not both appenders")));
        let road = road::install(&resolved, false, false);
        assert_eq!(road.len(), 1);
        assert!(road[0].reason.contains(road::TOGGLE_ENV));
    }

    #[test]
    fn the_visit_order_is_by_entity_with_each_record_whole() {
        // {entity, TransportVehicle index} records.
        let record = |entity: u32, index: u32| u64::from(entity) | (u64::from(index) << 32);
        let records = [record(30, 0), record(10, 1), record(20, 2)];
        assert_eq!(
            platform::visit_order(&records).unwrap(),
            Some(vec![record(10, 1), record(20, 2), record(30, 0)])
        );
        assert_eq!(
            platform::visit_order(&[record(1, 5), record(2, 4)]).unwrap(),
            None
        );
        assert_eq!(platform::visit_order(&[]).unwrap(), None);
        assert_eq!(
            platform::visit_order(&[record(7, 0), record(7, 1)]),
            Err("two entries name one entity")
        );
    }

    #[test]
    fn candidates_with_equal_costs_reach_the_sort_in_one_order() {
        // {word, station, terminal}: the cost is looked up by the engine,
        // so the order is by station and terminal, whatever came first.
        let a = [9, 100, 2];
        let b = [3, 100, 1];
        let c = [1, 50, 7];
        let one = platform::candidate_order(&[a, b, c]).unwrap();
        let other = platform::candidate_order(&[b, c, a]).unwrap();
        assert_eq!(one, vec![c, b, a]);
        assert_eq!(one, other, "two games' orders end up the same");
        assert_eq!(platform::candidate_order(&[c, b, a]), None);
        assert_eq!(platform::candidate_order(&[a]), None);
    }

    #[test]
    fn road_entries_are_kept_in_entity_order_each_whole() {
        let entry = |entity: i32, tag: u8| {
            let mut e = [tag; road::ENTRY_LEN as usize];
            e[..4].copy_from_slice(&entity.to_le_bytes());
            e
        };
        let entries = [entry(40, 1), entry(12, 2), entry(33, 3)];
        assert_eq!(
            road::entry_order(&entries).unwrap(),
            Some(vec![entry(12, 2), entry(33, 3), entry(40, 1)])
        );
        assert_eq!(
            road::entry_order(&[entry(1, 0), entry(2, 0)]).unwrap(),
            None
        );
        assert_eq!(
            road::entry_order(&[entry(5, 0), entry(5, 1)]),
            Err("two entries name one entity")
        );
    }

    /// xorshift64*: the tests' own random numbers, the same every run.
    struct Rng(u64);

    impl Rng {
        fn next(&mut self) -> u64 {
            self.0 ^= self.0 >> 12;
            self.0 ^= self.0 << 25;
            self.0 ^= self.0 >> 27;
            self.0.wrapping_mul(0x2545_f491_4f6c_dd1d)
        }

        fn below(&mut self, n: u64) -> u64 {
            self.next() % n.max(1)
        }
    }

    fn road_entry(entity: i32, tag: u32) -> road::Entry {
        let mut e = [0u8; road::ENTRY_LEN as usize];
        e[..4].copy_from_slice(&entity.to_le_bytes());
        e[4..8].copy_from_slice(&tag.to_le_bytes());
        e[16] = tag as u8;
        e
    }

    /// What the hook's in-place sort does to `entries`, set against the
    /// reference: the same entries in the same order, or the same refusal
    /// with nothing written.
    fn place_agrees(entries: &[road::Entry], scratch: &mut Vec<road::Entry>) {
        let mut placed = entries.to_vec();
        let outcome = road::place(&mut placed, scratch);
        match road::entry_order(entries) {
            Ok(None) => {
                assert_eq!(outcome, Ok(Sorted::Unchanged));
                assert_eq!(placed, entries);
            }
            Ok(Some(order)) => {
                assert_eq!(outcome, Ok(Sorted::Reordered));
                assert_eq!(placed, order);
            }
            Err(why) => {
                assert_eq!(outcome, Err(why));
                assert_eq!(placed, entries, "a refusal writes nothing");
            }
        }
    }

    #[test]
    fn the_in_place_road_sort_gives_the_reference_order_on_random_lists() {
        let mut rng = Rng(0x9e37_79b9_7f4a_7c15);
        let mut scratch = Vec::new();
        for round in 0..20_000 {
            let n = rng.below(40) as usize;
            // Small id ranges give duplicates, large ones do not.
            let range = if round % 3 == 0 { 8 } else { 1 << 20 };
            let mut ids: Vec<i32> = (0..n)
                .map(|_| rng.below(range) as i32 - (range / 4) as i32)
                .collect();
            match round % 4 {
                // A list kept sorted, then one entry appended: the hook's
                // common case.
                0 | 1 => {
                    ids.sort_unstable();
                    ids.dedup();
                    ids.push(rng.below(range) as i32 - (range / 4) as i32);
                }
                // Already sorted.
                2 => ids.sort_unstable(),
                // Anything.
                _ => {}
            }
            let entries: Vec<road::Entry> = ids
                .iter()
                .enumerate()
                .map(|(i, id)| road_entry(*id, i as u32))
                .collect();
            place_agrees(&entries, &mut scratch);
        }
    }

    #[test]
    fn the_in_place_road_sort_places_an_appended_entry_at_each_end_and_between() {
        let mut scratch = Vec::new();
        let kept = [10, 20, 30, 40];
        for (new, at) in [(5, 0), (15, 1), (35, 3), (45, 4)] {
            let mut entries: Vec<road::Entry> =
                kept.iter().map(|id| road_entry(*id, *id as u32)).collect();
            entries.push(road_entry(new, 99));
            let outcome = road::place(&mut entries, &mut scratch);
            let expected = if at == 4 {
                Ok(Sorted::Unchanged)
            } else {
                Ok(Sorted::Reordered)
            };
            assert_eq!(outcome, expected, "{new}");
            let ids: Vec<i32> = entries.iter().map(road::key).collect();
            let mut want = kept.to_vec();
            want.insert(at, new);
            assert_eq!(ids, want);
            assert_eq!(entries[at][16], 99, "the entry moved whole");
        }
        // The appended entry names an entity the list has: refused.
        let mut entries: Vec<road::Entry> = [10, 20, 30, 20]
            .iter()
            .map(|id| road_entry(*id, 0))
            .collect();
        let before = entries.clone();
        assert_eq!(
            road::place(&mut entries, &mut scratch),
            Err("two entries name one entity")
        );
        assert_eq!(entries, before);
        // An empty list and one entry: nothing to do.
        assert_eq!(road::place(&mut [], &mut scratch), Ok(Sorted::Unchanged));
        assert_eq!(
            road::place(&mut [road_entry(3, 0)], &mut scratch),
            Ok(Sorted::Unchanged)
        );
    }

    #[test]
    fn the_visit_records_sort_as_the_reference_with_one_buffer() {
        let mut rng = Rng(0x1234_5678_9abc_def1);
        let mut buffer = Vec::new();
        for round in 0..5_000 {
            let n = rng.below(30);
            let range = if round % 3 == 0 { 10 } else { 1 << 24 };
            let mut records: Vec<u64> = (0..n)
                .map(|i| u64::from(rng.below(range) as u32) | (i << 32))
                .collect();
            if round % 2 == 0 {
                records.sort_unstable_by_key(|r| *r as u32 as i32);
            }
            let outcome = platform::sort_records(n, |i| records[i as usize], &mut buffer);
            match platform::visit_order(&records) {
                Ok(None) => assert_eq!(outcome, Ok(Sorted::Unchanged)),
                Ok(Some(order)) => {
                    assert_eq!(outcome, Ok(Sorted::Reordered));
                    assert_eq!(buffer, order);
                }
                Err(why) => assert_eq!(outcome, Err(why)),
            }
        }
    }

    #[test]
    fn the_candidates_sort_in_place_as_the_reference() {
        let mut rng = Rng(0x0fed_cba9_8765_4321);
        for round in 0..5_000 {
            let n = rng.below(12) as usize;
            let range = if round % 2 == 0 { 3 } else { 1000 };
            let words: Vec<[u32; 3]> = (0..n)
                .map(|_| {
                    [
                        rng.below(range) as u32,
                        rng.below(range) as u32,
                        rng.below(range) as u32,
                    ]
                })
                .collect();
            let mut bytes: Vec<platform::Candidate> = words
                .iter()
                .map(|w| {
                    let mut c = [0u8; 12];
                    for (i, word) in w.iter().enumerate() {
                        c[4 * i..4 * i + 4].copy_from_slice(&word.to_le_bytes());
                    }
                    c
                })
                .collect();
            let outcome = platform::sort_candidates_in_place(&mut bytes);
            let back: Vec<[u32; 3]> = bytes
                .iter()
                .map(|c| {
                    let word =
                        |i: usize| u32::from_le_bytes(c[4 * i..4 * i + 4].try_into().unwrap());
                    [word(0), word(1), word(2)]
                })
                .collect();
            match platform::candidate_order(&words) {
                None => {
                    assert_eq!(outcome, Sorted::Unchanged);
                    assert_eq!(back, words);
                }
                Some(order) => {
                    assert_eq!(outcome, Sorted::Reordered);
                    assert_eq!(back, order);
                }
            }
        }
    }

    #[test]
    fn a_refusal_is_counted_by_reason_until_taken() {
        let refusals = Refusals::new();
        refusals.note("test", "a");
        refusals.note("test", "b");
        refusals.note("test", "a");
        assert_eq!(refusals.take_window(), vec![("a", 2), ("b", 1)]);
        assert_eq!(refusals.take_window(), vec![]);
        assert_eq!(refusals.count.load(Ordering::Relaxed), 3);
    }

    /// The road sort alone, before and after: the reference's copy, key
    /// array, permutation and write-back against [`road::place`], on lists
    /// kept sorted with one entry appended at a random place. Run with
    /// `cargo test --release -p tpf3mp-hook order::tests::road_sort_bench -- --ignored --nocapture`.
    #[test]
    #[ignore = "a benchmark: prints the road sort's cost before and after"]
    fn road_sort_bench() {
        let mut rng = Rng(42);
        let mut scratch = Vec::new();
        for n in [2usize, 8, 32, 128] {
            let lists: Vec<Vec<road::Entry>> = (0..2_000)
                .map(|_| {
                    let mut ids: Vec<i32> = (0..n - 1).map(|_| rng.below(1 << 30) as i32).collect();
                    ids.sort_unstable();
                    ids.dedup();
                    ids.push(rng.below(1 << 30) as i32);
                    ids.iter().map(|id| road_entry(*id, 0)).collect()
                })
                .collect();
            let rounds = 50;
            let begin = std::time::Instant::now();
            for _ in 0..rounds {
                for list in &lists {
                    let mut memory = list.clone();
                    // The reference path as the hook ran it: a copy, the
                    // order, the write-back.
                    let entries: Vec<road::Entry> = memory.to_vec();
                    if let Ok(Some(order)) = road::entry_order(&entries) {
                        memory.copy_from_slice(&order);
                    }
                    std::hint::black_box(&memory);
                }
            }
            let before = begin.elapsed().as_nanos() as f64 / (rounds * lists.len()) as f64;
            let begin = std::time::Instant::now();
            for _ in 0..rounds {
                for list in &lists {
                    let mut memory = list.clone();
                    let _ = road::place(&mut memory, &mut scratch);
                    std::hint::black_box(&memory);
                }
            }
            let after = begin.elapsed().as_nanos() as f64 / (rounds * lists.len()) as f64;
            println!(
                "road sort, {n} entries: before {before:.0} ns, after {after:.0} ns (the list's clone included in both)"
            );
        }
    }
}

/// The two sort hooks through their real stolen bytes: a hand-written
/// function stands where the engine's site would be, with the frame or
/// register the site reads pointing at a world built in memory.
#[cfg(all(test, windows, target_arch = "x86_64"))]
mod splice_tests {
    use super::*;

    /// A page of this test's own code, executable: the engine's buffers are
    /// private to hookcore.
    fn fixture(code: &[u8]) -> usize {
        use windows_sys::Win32::System::Memory::{
            MEM_COMMIT, MEM_RESERVE, PAGE_EXECUTE_READWRITE, VirtualAlloc,
        };
        assert!(code.len() <= 0x1000);
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

    /// A node list of three records (entity ids 30, 10, 20) and a vector of
    /// three entries in node order, inside one frame laid out as the site's:
    /// `[rbp-0x20]` begin, `[rbp-0x18]` end, `[rbp-0x80]` this, `this+8`
    /// the holder, `[holder]`/`[holder+8]` the records' span.
    struct World {
        memory: Vec<u8>,
    }

    impl World {
        const RBP: usize = 0x200;
        const THIS: usize = 0x300;
        const HOLDER: usize = 0x340;
        const RECORDS: usize = 0x400;
        const VECTOR: usize = 0x500;

        fn new() -> Self {
            let mut memory = vec![0u8; 0x600];
            let base = memory.as_ptr() as u64;
            let put = |memory: &mut Vec<u8>, at: usize, value: u64| {
                memory[at..at + 8].copy_from_slice(&value.to_le_bytes());
            };
            for (node, entity) in [30u32, 10, 20].into_iter().enumerate() {
                let at = Self::RECORDS + node * 20;
                memory[at..at + 4].copy_from_slice(&entity.to_le_bytes());
            }
            for node in 0..3u32 {
                let at = Self::VECTOR + node as usize * 8;
                memory[at..at + 4].copy_from_slice(&node.to_le_bytes());
                memory[at + 4..at + 8].copy_from_slice(&(node as f32).to_bits().to_le_bytes());
            }
            put(&mut memory, Self::HOLDER, base + Self::RECORDS as u64);
            put(
                &mut memory,
                Self::HOLDER + 8,
                base + Self::RECORDS as u64 + 60,
            );
            put(&mut memory, Self::THIS + 8, base + Self::HOLDER as u64);
            put(&mut memory, Self::RBP - 0x20, base + Self::VECTOR as u64);
            put(
                &mut memory,
                Self::RBP - 0x18,
                base + Self::VECTOR as u64 + 24,
            );
            put(&mut memory, Self::RBP - 0x80, base + Self::THIS as u64);
            Self { memory }
        }

        fn rbp(&self) -> u64 {
            self.memory.as_ptr() as u64 + Self::RBP as u64
        }

        fn vector(&self) -> Vec<(u32, f32)> {
            (0..3)
                .map(|i| {
                    let at = Self::VECTOR + i * 8;
                    let dword = |at: usize| {
                        let m = &self.memory;
                        u32::from_le_bytes([m[at], m[at + 1], m[at + 2], m[at + 3]])
                    };
                    (dword(at), f32::from_bits(dword(at + 4)))
                })
                .collect()
        }
    }

    #[test]
    fn the_land_vehicle_hook_sorts_the_engines_vector_through_its_real_site() {
        let world = World::new();
        assert_eq!(world.vector(), vec![(0, 0.0), (1, 1.0), (2, 2.0)]);
        // push rbp; push r13; push rsi; mov rbp, imm64; <site>; mov rax,
        // [r13]; pop rsi; pop r13; pop rbp; ret
        let mut code = vec![0x55, 0x41, 0x55, 0x56, 0x48, 0xBD];
        code.extend_from_slice(&world.rbp().to_le_bytes());
        let site_at = code.len();
        code.extend_from_slice(&land_vehicle::EXPECTED);
        code.extend_from_slice(&[0x49, 0x8B, 0x45, 0x00, 0x5E, 0x41, 0x5D, 0x5D, 0xC3]);
        let page = fixture(&code);
        // SAFETY: the page holds our hand-written function of no arguments.
        let fun: extern "C" fn() -> u64 =
            unsafe { std::mem::transmute::<usize, extern "C" fn() -> u64>(page) };
        assert_eq!(fun() as u32, 0, "without the hook, node 0 is first");

        // SAFETY: the fixture is this test's own code, not running now, and
        // the real hook only rewrites the world's vector.
        let splice = unsafe {
            Splice::install(
                (page + site_at) as *mut u8,
                &land_vehicle::EXPECTED,
                land_vehicle::STEAL,
                land_vehicle::hook,
            )
        }
        .unwrap();
        let first = fun();
        assert_eq!(
            world.vector(),
            vec![(1, 1.0), (2, 2.0), (0, 0.0)],
            "entity 10's node first, entries whole"
        );
        assert_eq!(first as u32, 1, "the stolen loads ran after the sort");
        assert_eq!(f32::from_bits((first >> 32) as u32), 1.0);
        // A second pass finds the vector in order and leaves it.
        assert_eq!(fun() as u32, 1);
        assert_eq!(world.vector(), vec![(1, 1.0), (2, 2.0), (0, 0.0)]);
        // SAFETY: nothing runs the fixture now.
        unsafe { splice.detach() }.unwrap();
    }

    #[test]
    fn the_vehicles_at_stop_hook_sorts_the_vector_rax_names_through_its_real_site() {
        let mut ids: Vec<i32> = vec![9, 4, 6, 1];
        // A std::vector<Entity> of the first three ids: begin, end, cap.
        let vector: [u64; 3] = [
            ids.as_mut_ptr() as u64,
            ids.as_mut_ptr() as u64 + 12,
            ids.as_mut_ptr() as u64 + 16,
        ];
        // sub rsp, 0x260; mov rax, imm64; <site>; add rsp, 0x260; ret
        let mut code = vec![0x48, 0x81, 0xEC, 0x60, 0x02, 0x00, 0x00, 0x48, 0xB8];
        code.extend_from_slice(&(vector.as_ptr() as u64).to_le_bytes());
        let site_at = code.len();
        code.extend_from_slice(&terminal::EXPECTED[..terminal::STEAL]);
        code.extend_from_slice(&[0x48, 0x81, 0xC4, 0x60, 0x02, 0x00, 0x00, 0xC3]);
        let page = fixture(&code);
        // SAFETY: as above.
        let fun: extern "C" fn() -> u64 =
            unsafe { std::mem::transmute::<usize, extern "C" fn() -> u64>(page) };
        assert_eq!(fun(), vector.as_ptr() as u64);
        assert_eq!(ids, vec![9, 4, 6, 1]);
        // The site's expected bytes go on past the steal; the fixture holds
        // only the stolen ones, so it states those as its expectation.
        let expected = &terminal::EXPECTED[..terminal::STEAL];
        // SAFETY: as above.
        let splice = unsafe {
            Splice::install(
                (page + site_at) as *mut u8,
                expected,
                terminal::STEAL,
                terminal::hook,
            )
        }
        .unwrap();
        assert_eq!(
            fun(),
            vector.as_ptr() as u64,
            "rax reaches the stolen store"
        );
        assert_eq!(
            ids,
            vec![4, 6, 9, 1],
            "the three ids in the vector sorted, the fourth untouched"
        );
        // SAFETY: nothing runs the fixture now.
        unsafe { splice.detach() }.unwrap();
    }

    /// The platform chooser's loop head, as the engine has it: `this` in
    /// r13 (`[this+8]` the node-list holder), the count at `[rbp+0x5b0]`,
    /// the byte offset of the iteration in rsi. The fixture returns the
    /// entity the iteration reads at `[rsi+rdi]` after the site.
    #[test]
    fn the_visit_hook_walks_the_node_list_in_entity_order_without_writing_it() {
        let mut memory = vec![0u8; 0x1000];
        let base = memory.as_mut_ptr() as u64;
        let put = |memory: &mut Vec<u8>, at: usize, value: u64| {
            memory[at..at + 8].copy_from_slice(&value.to_le_bytes());
        };
        const THIS: usize = 0x100;
        const HOLDER: usize = 0x140;
        const RECORDS: usize = 0x200;
        const RBP: usize = 0x300;
        // {entity, index}: 30, 10, 20.
        for (i, (entity, index)) in [(30u32, 0u32), (10, 1), (20, 2)].into_iter().enumerate() {
            let at = RECORDS + i * 8;
            memory[at..at + 4].copy_from_slice(&entity.to_le_bytes());
            memory[at + 4..at + 8].copy_from_slice(&index.to_le_bytes());
        }
        put(&mut memory, THIS + 8, base + HOLDER as u64);
        put(&mut memory, HOLDER, base + RECORDS as u64);
        put(&mut memory, HOLDER + 8, base + RECORDS as u64 + 24);
        memory[RBP + 0x5b0..RBP + 0x5b4].copy_from_slice(&3i32.to_le_bytes());
        // push rbp; push r13; push rsi; push rdi; push rbx; mov rbp, imm64;
        // mov r13, imm64; mov rsi, rcx; mov rax, [r13+8]; mov rdi, [rax];
        // <site>; mov eax, [rsi+rdi]; pop rbx; pop rdi; pop rsi; pop r13;
        // pop rbp; ret
        let mut code = vec![0x55, 0x41, 0x55, 0x56, 0x57, 0x53, 0x48, 0xBD];
        code.extend_from_slice(&(base + RBP as u64).to_le_bytes());
        code.extend_from_slice(&[0x49, 0xBD]);
        code.extend_from_slice(&(base + THIS as u64).to_le_bytes());
        code.extend_from_slice(&[0x48, 0x89, 0xCE, 0x49, 0x8B, 0x45, 0x08, 0x48, 0x8B, 0x38]);
        let site_at = code.len();
        code.extend_from_slice(&platform::VISIT_EXPECTED);
        code.extend_from_slice(&[0x8B, 0x04, 0x3E, 0x5B, 0x5F, 0x5E, 0x41, 0x5D, 0x5D, 0xC3]);
        let page = fixture(&code);
        // SAFETY: the page holds a function of one integer argument.
        let fun: extern "C" fn(u64) -> u32 =
            unsafe { std::mem::transmute::<usize, extern "C" fn(u64) -> u32>(page) };
        let walk = || (0..3).map(|i| fun(i * 8)).collect::<Vec<u32>>();
        assert_eq!(walk(), vec![30, 10, 20], "the engine's own order");

        // SAFETY: the fixture is this test's own code, not running now.
        let splice = unsafe {
            Splice::install(
                (page + site_at) as *mut u8,
                &platform::VISIT_EXPECTED,
                platform::VISIT_STEAL,
                platform::visit_hook,
            )
        }
        .unwrap();
        assert_eq!(walk(), vec![10, 20, 30], "visited in entity order");
        let records: Vec<u8> = memory[RECORDS..RECORDS + 24].to_vec();
        assert_eq!(
            &records[..4],
            &30u32.to_le_bytes(),
            "the list is not written"
        );
        // A count that is not the list's: the engine's own order stands.
        memory[RBP + 0x5b0..RBP + 0x5b4].copy_from_slice(&2i32.to_le_bytes());
        assert_eq!(fun(0), 30);
        // SAFETY: nothing runs the fixture now.
        unsafe { splice.detach() }.unwrap();
        drop(memory);
    }

    /// `EdgeUseManager`'s data as `GetOrAddEdgeData` walks it, built in
    /// memory: the entries of edge (entity 1, index 1) are sorted in place.
    #[test]
    fn the_road_sort_finds_an_edges_entries_and_puts_them_in_entity_order() {
        let mut memory = vec![0u8; 0x800];
        let base = memory.as_mut_ptr() as u64;
        let put = |memory: &mut Vec<u8>, at: usize, value: u64| {
            memory[at..at + 8].copy_from_slice(&value.to_le_bytes());
        };
        let int = |memory: &mut Vec<u8>, at: usize, value: i32| {
            memory[at..at + 4].copy_from_slice(&value.to_le_bytes());
        };
        const DATA: usize = 0x100;
        const INDEX: usize = 0x200;
        const SLOTS: usize = 0x300;
        const EDGES: usize = 0x400;
        const ENTRIES: usize = 0x500;
        const EDGE_ID: usize = 0x600;
        put(&mut memory, 0x18, base + DATA as u64);
        put(&mut memory, DATA, base + INDEX as u64);
        put(&mut memory, DATA + 8, base + INDEX as u64 + 12);
        put(&mut memory, DATA + 0x18, base + SLOTS as u64);
        put(&mut memory, DATA + 0x20, base + SLOTS as u64 + 2 * 72);
        // Entity 0 has no slot, entity 1 slot 1, entity 2 slot 0.
        for (i, slot) in [-1, 1, 0].into_iter().enumerate() {
            int(&mut memory, INDEX + i * 4, slot);
        }
        put(&mut memory, SLOTS + 72, base + EDGES as u64);
        put(&mut memory, SLOTS + 72 + 8, base + EDGES as u64 + 2 * 32);
        put(&mut memory, EDGES + 32 + 8, base + ENTRIES as u64);
        put(
            &mut memory,
            EDGES + 32 + 0x10,
            base + ENTRIES as u64 + 3 * 20,
        );
        for (i, entity) in [40, 12, 33].into_iter().enumerate() {
            int(&mut memory, ENTRIES + i * 20, entity);
            memory[ENTRIES + i * 20 + 16] = i as u8;
        }
        int(&mut memory, EDGE_ID, 1);
        int(&mut memory, EDGE_ID + 4, 1);
        let entities = |memory: &Vec<u8>| {
            (0..3)
                .map(|i| {
                    let at = ENTRIES + i * 20;
                    let id = i32::from_le_bytes(memory[at..at + 4].try_into().unwrap());
                    (id, memory[at + 16])
                })
                .collect::<Vec<_>>()
        };
        let sorted = road::sort_edge(&mut Probe::new(), base + DATA as u64, base + EDGE_ID as u64);
        assert_eq!(sorted, Ok(Sorted::Reordered));
        assert_eq!(entities(&memory), vec![(12, 1), (33, 2), (40, 0)]);
        assert_eq!(
            road::sort_edge(&mut Probe::new(), base + DATA as u64, base + EDGE_ID as u64),
            Ok(Sorted::Unchanged)
        );
        // An edge whose entity has no slot, and one past its slot's edges.
        int(&mut memory, EDGE_ID, 0);
        assert_eq!(
            road::sort_edge(&mut Probe::new(), base + DATA as u64, base + EDGE_ID as u64),
            Err("the edge's entity has no slot")
        );
        int(&mut memory, EDGE_ID, 1);
        int(&mut memory, EDGE_ID + 4, 2);
        assert_eq!(
            road::sort_edge(&mut Probe::new(), base + DATA as u64, base + EDGE_ID as u64),
            Err("the edge index is past the slot's edges")
        );
    }

    /// A manager whose data has one edge entity (1) with `edges` edges, each
    /// holding the entries `lists[i]` (entity ids; the byte at +16 tags
    /// each), and a path of those edges: `[0]` the manager (its data at
    /// `+0x18`), the data at `DATA`, the path vector at `PATH`.
    struct RoadWorld {
        memory: Vec<u8>,
        entries: Vec<Vec<road::Entry>>,
    }

    impl RoadWorld {
        const DATA: usize = 0x100;
        const INDEX: usize = 0x200;
        const SLOTS: usize = 0x300;
        const EDGES: usize = 0x400;
        const PATH: usize = 0x600;
        const PATH_IDS: usize = 0x700;

        fn new(lists: &[&[i32]]) -> Self {
            let mut memory = vec![0u8; 0x1000];
            let base = memory.as_mut_ptr() as u64;
            let put = |memory: &mut Vec<u8>, at: usize, value: u64| {
                memory[at..at + 8].copy_from_slice(&value.to_le_bytes());
            };
            let mut entries: Vec<Vec<road::Entry>> = lists
                .iter()
                .map(|ids| {
                    let mut list: Vec<road::Entry> = ids
                        .iter()
                        .enumerate()
                        .map(|(i, id)| {
                            let mut e = [0u8; 20];
                            e[..4].copy_from_slice(&id.to_le_bytes());
                            e[16] = i as u8;
                            e
                        })
                        .collect();
                    list.reserve(4);
                    list
                })
                .collect();
            put(&mut memory, 0x18, base + Self::DATA as u64);
            put(&mut memory, Self::DATA, base + Self::INDEX as u64);
            put(&mut memory, Self::DATA + 8, base + Self::INDEX as u64 + 8);
            put(&mut memory, Self::DATA + 0x18, base + Self::SLOTS as u64);
            put(
                &mut memory,
                Self::DATA + 0x20,
                base + Self::SLOTS as u64 + 72,
            );
            // Entity 0 has no slot, entity 1 slot 0.
            memory[Self::INDEX..Self::INDEX + 4].copy_from_slice(&(-1i32).to_le_bytes());
            memory[Self::INDEX + 4..Self::INDEX + 8].copy_from_slice(&0i32.to_le_bytes());
            put(&mut memory, Self::SLOTS, base + Self::EDGES as u64);
            put(
                &mut memory,
                Self::SLOTS + 8,
                base + Self::EDGES as u64 + 32 * lists.len() as u64,
            );
            for (i, list) in entries.iter_mut().enumerate() {
                let at = Self::EDGES + 32 * i;
                let begin = list.as_mut_ptr() as u64;
                put(&mut memory, at + 8, begin);
                put(&mut memory, at + 0x10, begin + 20 * list.len() as u64);
                let id = Self::PATH_IDS + 12 * i;
                memory[id..id + 4].copy_from_slice(&1i32.to_le_bytes());
                memory[id + 4..id + 8].copy_from_slice(&(i as i32).to_le_bytes());
            }
            put(&mut memory, Self::PATH, base + Self::PATH_IDS as u64);
            put(
                &mut memory,
                Self::PATH + 8,
                base + Self::PATH_IDS as u64 + 12 * lists.len() as u64,
            );
            Self { memory, entries }
        }

        fn at(&self, offset: usize) -> usize {
            self.memory.as_ptr() as usize + offset
        }

        fn ids(&self, edge: usize) -> Vec<i32> {
            self.entries[edge].iter().map(road::key).collect()
        }
    }

    /// Stands in for the engine's appender: appends nothing.
    #[allow(clippy::too_many_arguments)]
    unsafe extern "system" fn engine_add(
        _: usize,
        _: usize,
        _: usize,
        _: usize,
        _: usize,
        _: usize,
        _: usize,
        _: usize,
    ) -> usize {
        7
    }

    #[allow(clippy::too_many_arguments)]
    unsafe extern "system" fn engine_add_range(
        _: usize,
        _: usize,
        _: usize,
        _: usize,
        _: usize,
        _: usize,
        _: usize,
        _: usize,
        _: usize,
    ) -> usize {
        9
    }

    /// `AddRange` hands the manager's data as `this` and the manager as its
    /// ninth argument; `Add` hands the manager. Both sort the edges they
    /// touched, and nothing else.
    #[test]
    fn both_appenders_sort_their_edges_add_range_from_the_data_it_is_given() {
        let _serial = crate::lua::tests::SERIAL
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let world = RoadWorld::new(&[&[40, 12], &[5, 9, 7], &[3, 1], &[8, 2]]);
        road::arm_for_test(
            engine_add as *const () as usize,
            engine_add_range as *const () as usize,
        );
        // AddRange(data, entity, component, &path, current, bounds, from=1,
        // to=2, manager): edges 1 and 2 sorted, 0 and 3 not.
        // SAFETY: the detour with the arguments the engine's caller passes,
        // on a world built in memory; the "engine" appends nothing.
        let result = unsafe {
            road::add_range(
                world.at(RoadWorld::DATA),
                9,
                0,
                world.at(RoadWorld::PATH),
                1,
                0,
                1,
                2,
                world.at(0),
            )
        };
        assert_eq!(result, 9, "the engine's answer passes through");
        assert_eq!(world.ids(0), vec![40, 12]);
        assert_eq!(world.ids(1), vec![5, 7, 9]);
        assert_eq!(world.ids(2), vec![1, 3]);
        assert_eq!(world.ids(3), vec![8, 2]);
        assert_eq!(world.entries[1][1][16], 2, "entries moved whole");
        // Read the old way, through `[this+0x18]` of the data, the edge's
        // entity has no slot: a manager that does not name this data is
        // refused, nothing written.
        let _ = road::take_refusals();
        // SAFETY: as above.
        unsafe {
            road::add_range(
                world.at(RoadWorld::DATA),
                9,
                0,
                world.at(RoadWorld::PATH),
                0,
                0,
                0,
                3,
                world.at(RoadWorld::DATA),
            )
        };
        assert_eq!(world.ids(0), vec![40, 12]);
        assert_eq!(
            road::take_refusals(),
            vec![("AddRange's data is not its manager's", 1)]
        );
        // Add(manager, &edgeId, ...): its one edge.
        // SAFETY: as above.
        let result = unsafe {
            road::add(
                world.at(0),
                world.at(RoadWorld::PATH_IDS + 3 * 12),
                2,
                0,
                0,
                0,
                0,
                0,
            )
        };
        assert_eq!(result, 7);
        assert_eq!(world.ids(3), vec![2, 8]);
        assert_eq!(world.ids(0), vec![40, 12]);
        road::arm_for_test(0, 0);
    }

    /// The road fix's cost per append on a real edge in memory, before and
    /// after: before, every word was checked with its own `VirtualQuery`
    /// (13 for one edge) and the list copied, keyed, permuted and written
    /// back; after, one probe per append and the in-place placement. Run
    /// with `cargo test --release -p tpf3mp-hook order::splice_tests::road_append_bench -- --ignored --nocapture`.
    #[test]
    #[ignore = "a benchmark: prints the road fix's cost per append before and after"]
    fn road_append_bench() {
        let rounds = 100_000;
        for n in [2usize, 8, 32] {
            let ids: Vec<i32> = (0..n as i32).map(|i| i * 10).collect();
            let world = RoadWorld::new(&[&ids]);
            let data = world.at(RoadWorld::DATA) as u64;
            let edge = world.at(RoadWorld::PATH_IDS) as u64;
            // The words the old walk checked, one system call each.
            let words: Vec<usize> = {
                let slot = world.at(RoadWorld::SLOTS);
                let edge_data = world.at(RoadWorld::EDGES);
                vec![
                    edge as usize,
                    edge as usize + 4,
                    world.at(0x18),
                    data as usize,
                    data as usize + 8,
                    world.at(RoadWorld::INDEX + 4),
                    data as usize + 0x18,
                    data as usize + 0x20,
                    slot,
                    slot + 8,
                    edge_data + 8,
                    edge_data + 0x10,
                ]
            };
            let entries_at = world.entries[0].as_ptr() as usize;
            let begin = std::time::Instant::now();
            for _ in 0..rounds {
                for word in &words {
                    assert!(crate::image::readable(*word, 8));
                }
                assert!(crate::image::readable(entries_at, 20 * n));
                let copy: Vec<road::Entry> = world.entries[0].clone();
                std::hint::black_box(road::entry_order(&copy).unwrap());
            }
            let before = begin.elapsed().as_nanos() as f64 / f64::from(rounds);
            let begin = std::time::Instant::now();
            for _ in 0..rounds {
                let mut probe = Probe::new();
                std::hint::black_box(road::sort_edge(&mut probe, data, edge).unwrap());
            }
            let after = begin.elapsed().as_nanos() as f64 / f64::from(rounds);
            println!(
                "road append, one edge of {n} entries in order: before {before:.0} ns, after {after:.0} ns ({:.1}x)",
                before / after
            );
        }
    }
}
