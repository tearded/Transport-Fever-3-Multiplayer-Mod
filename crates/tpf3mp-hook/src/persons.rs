//! The person-order fixes: the batches the person simulation consumes in
//! an order that is the engine's history, not the room's state, sorted by
//! entity id right before the engine reads them (docs/HOOKS.md, "The
//! person-order fixes").
//!
//! Ported from silver2127's TPF2 Multiplayer (`tpf2-multiplayer`,
//! docs/re/HOTJOIN_ORDER.md "Mechanism" and "The fix",
//! native/src/slice/hotjoin_order.inl and the Linux reference
//! native/linux/src/order_canon_linux.cpp): there the person simulation read
//! its batches in ECS node-list order, which is add/swap-remove history in a
//! running world and registration order in a loaded one, and drew one random
//! stream across each batch in that order. The same draw then lands on
//! another person, or another building. TF3 build 40408 has the same shape,
//! read statically with `tools/tpfre`:
//!
//! | TF2 batch | TF3 site | what reads it in order |
//! |---|---|---|
//! | candidates | `GetTargetsByLandUse` `0x8e3d65` | `BinarySearchIndex` `0x8e4400`: cumulative free capacity, one draw |
//! | departures | `SimEntityAtBuildingSystem::Update2` `0xb05f92` | `NoteAtBuildingPersonsLeave` `0xb2e950`: one generator, batch order |
//! | arrivals | `PersonMoveSystem::Update2` `0xaecfcd` | `NoteWalkPersonsArrived` `0xb33a30`: stay draws, batch order |
//! | idle | `SimEntityNeedsPathSystem::Update` `0xb18213` | `PathFactory::Compute` `0x8d11e0`: a generator per chunk start |
//! | freed ids | `Engine::EndModification` `0x2bb4fd1` | the FIFO free-id deque `AddEntity` pops |
//!
//! Each batch is a `std::vector<Entity>` (4-byte ids) the engine is about
//! to read and that nothing indexes by position across the sort, so the
//! hook sorts it ascending in place. TF2's capacity-map relink is not
//! ported: TF3's `SimEntityUpdateHelper` keeps phmap flat maps, not linked
//! lists (the doc says why).
//!
//! The sorts change what the simulation decides, so **every game of a room
//! must run the same set**: one game with a batch sorted and another with
//! it in the engine's order decide apart. They are on by default;
//! [`MASTER_ENV`] `=0` turns all of them off and each batch's own switch
//! turns it alone off, and either must then be set alike on every machine
//! of the room.
//!
//! Each fix installs on its own and fails closed on its own, as the order
//! fixes do (`order.rs`): its profile target must resolve, the bytes at the
//! site (and the instructions around it that name the vector) must be the
//! measured ones, every read on the game's thread is checked through
//! [`crate::image`], a vector of another shape is refused for that call and
//! said once in hook.log, and a panic switches the fix off for the game.

#![allow(unsafe_code)]
#![cfg_attr(not(all(windows, target_arch = "x86_64")), allow(dead_code))]

use std::sync::{
    Mutex,
    atomic::{AtomicBool, AtomicU64, Ordering},
};

use tpf3mp_hookcore::detour::{SavedRegs, Splice, SpliceHook};
use tpf3mp_hookcore::profile::ResolvedProfile;

use crate::image::Readable as Probe;
use crate::log;
use crate::order::Sorted;
use crate::perf::{self, Piece};

/// Set to `0` (or `off`, `false`, `no`), none of the person-order fixes
/// installs: every batch keeps the engine's order. Must be the same on
/// every machine of a room.
pub const MASTER_ENV: &str = "TPF3MP_HOOK_PERSON_ORDER";

/// A sanity bound on one batch.
pub const MAX_IDS: u64 = 1 << 24;

/// One batch's fix: where it splices in, what it expects there, and where
/// the vectors are at the site.
pub struct Batch {
    /// The fix's name in hook.log.
    pub fix: &'static str,
    /// Its own kill switch.
    pub toggle_env: &'static str,
    /// The profile target of the site.
    pub target: &'static str,
    /// The bytes at the site: the stolen instructions and those after them
    /// up to the instruction that hands the vector on, so the slots the
    /// hook sorts are the ones the engine reads.
    pub expected: &'static [u8],
    pub steal: usize,
    /// More bytes the function must hold, at offsets from the site: the
    /// instructions that fill or read the vector elsewhere.
    pub context: &'static [(i64, &'static [u8])],
    /// What the installed line says the fix does.
    pub does: &'static str,
    /// The alive line's interval, in calls.
    pub alive_every: u64,
    /// The vectors at the site: the address of each `std::vector<Entity>`
    /// (begin at +0, end at +8, capacity at +0x10).
    vectors: fn(&SavedRegs, &mut Probe) -> Result<Vectors, &'static str>,
    hook: SpliceHook,
    state: &'static State,
}

/// Up to two vectors at one site.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Vectors {
    at: [u64; 2],
    len: usize,
}

impl Vectors {
    pub fn one(at: u64) -> Self {
        Self {
            at: [at, 0],
            len: 1,
        }
    }

    pub fn two(first: u64, second: u64) -> Self {
        Self {
            at: [first, second],
            len: 2,
        }
    }

    pub fn as_slice(&self) -> &[u64] {
        &self.at[..self.len]
    }
}

/// A fix's counters, from any thread.
pub struct State {
    broken: AtomicBool,
    calls: AtomicU64,
    reordered: AtomicU64,
    refused: AtomicU64,
    in_step_calls: AtomicU64,
    in_step_reordered: AtomicU64,
    last_refusal: Mutex<Option<&'static str>>,
}

impl State {
    pub const fn new() -> Self {
        Self {
            broken: AtomicBool::new(false),
            calls: AtomicU64::new(0),
            reordered: AtomicU64::new(0),
            refused: AtomicU64::new(0),
            in_step_calls: AtomicU64::new(0),
            in_step_reordered: AtomicU64::new(0),
            last_refusal: Mutex::new(None),
        }
    }
}

impl Default for State {
    fn default() -> Self {
        Self::new()
    }
}

/// Whether a fix is wanted: the master switch and its own both on.
pub fn wanted(master: Option<&str>, own: Option<&str>) -> bool {
    crate::ticks::wanted(master) && crate::ticks::wanted(own)
}

/// Sorts entity ids ascending in place; [`Sorted::Unchanged`] (nothing
/// written) when they already are.
pub fn sort_ids(ids: &mut [i32]) -> Sorted {
    if ids.windows(2).all(|pair| pair[0] <= pair[1]) {
        return Sorted::Unchanged;
    }
    ids.sort_unstable();
    Sorted::Reordered
}

/// The number of ids in a `std::vector<Entity>` with these three pointers,
/// or why it is not one.
pub fn vector_len(begin: u64, end: u64, capacity: u64) -> Result<u64, &'static str> {
    if begin == 0 {
        return if end == 0 && capacity == 0 {
            Ok(0)
        } else {
            Err("a vector with no storage has an end")
        };
    }
    if end < begin || capacity < end {
        return Err("the vector's pointers are out of order");
    }
    if !begin.is_multiple_of(4) || !(end - begin).is_multiple_of(4) {
        return Err("the vector's bounds are not whole ids");
    }
    let count = (end - begin) / 4;
    if count > MAX_IDS {
        return Err("more ids than any world holds");
    }
    Ok(count)
}

/// Sorts the `std::vector<Entity>` at `vector` in place, every read and
/// the span checked first.
fn sort_vector(vector: u64, probe: &mut Probe) -> Result<Sorted, &'static str> {
    let begin: u64 = probe.read(vector).ok_or("the vector is unreadable")?;
    let end: u64 = probe
        .read(vector.wrapping_add(8))
        .ok_or("the vector's end is unreadable")?;
    let capacity: u64 = probe
        .read(vector.wrapping_add(16))
        .ok_or("the vector's capacity is unreadable")?;
    let count = vector_len(begin, end, capacity)?;
    if count < 2 {
        return Ok(Sorted::Unchanged);
    }
    let count = usize::try_from(count).map_err(|_| "the vector is too long")?;
    if !probe.readable(begin as usize, count * 4) {
        return Err("the vector's ids are unreadable");
    }
    // SAFETY: `count` readable, 4-aligned ids at `begin`, the engine's own
    // vector on the thread about to read it; nothing else holds a view of
    // it across the site (the module's table), and the sort only permutes.
    let ids = unsafe { std::slice::from_raw_parts_mut(begin as usize as *mut i32, count) };
    Ok(sort_ids(ids))
}

/// The hook body every batch shares.
fn run(batch: &Batch, regs: *mut SavedRegs) {
    let state = batch.state;
    if state.broken.load(Ordering::Acquire) {
        return;
    }
    let _timer = perf::time(Piece::PersonOrder);
    let body = || {
        // SAFETY: the stub's block, held until the hook returns.
        let regs = unsafe { &*regs };
        let in_step = crate::order::in_step();
        let n = state.calls.fetch_add(1, Ordering::Relaxed) + 1;
        let in_step_n = if in_step {
            state.in_step_calls.fetch_add(1, Ordering::Relaxed) + 1
        } else {
            0
        };
        let mut probe = Probe::new();
        let outcome = (batch.vectors)(regs, &mut probe).and_then(|vectors| {
            let mut sorted = Sorted::Unchanged;
            for vector in vectors.as_slice() {
                if sort_vector(*vector, &mut probe)? == Sorted::Reordered {
                    sorted = Sorted::Reordered;
                }
            }
            Ok(sorted)
        });
        match outcome {
            Ok(Sorted::Reordered) => {
                let reorders = state.reordered.fetch_add(1, Ordering::Relaxed) + 1;
                if in_step {
                    state.in_step_reordered.fetch_add(1, Ordering::Relaxed);
                }
                if reorders <= 3 {
                    log::line(&format!(
                        "order fix {}: a batch put in entity order (call #{n})",
                        batch.fix
                    ));
                }
            }
            Ok(Sorted::Unchanged) => {}
            Err(why) => {
                state.refused.fetch_add(1, Ordering::Relaxed);
                let mut last = state.last_refusal.lock().unwrap_or_else(|p| p.into_inner());
                if *last != Some(why) {
                    *last = Some(why);
                    log::line(&format!(
                        "order fix {}: refused a batch, {why}; the engine's own order stands",
                        batch.fix
                    ));
                }
            }
        }
        if n == 1 || n.is_multiple_of(batch.alive_every) {
            log::line(&alive_line(batch.fix, state));
        }
        if in_step_n != 0 && in_step_n.is_multiple_of(batch.alive_every) {
            log::line(&format!(
                "order fix {}: in-step calls={in_step_n} reordered={}",
                batch.fix,
                state.in_step_reordered.load(Ordering::Relaxed)
            ));
        }
    };
    if std::panic::catch_unwind(std::panic::AssertUnwindSafe(body)).is_err() {
        state.broken.store(true, Ordering::Release);
        log::line(&format!(
            "order fix {}: panicked on the game's thread; switched off for this game",
            batch.fix
        ));
    }
}

/// The `alive` line: every call and those inside the game's step (the
/// ones two games of a room must agree on).
fn alive_line(fix: &str, state: &State) -> String {
    format!(
        "order fix {fix}: alive, calls={} reordered={} refused={} (in the step {}/{})",
        state.calls.load(Ordering::Relaxed),
        state.reordered.load(Ordering::Relaxed),
        state.refused.load(Ordering::Relaxed),
        state.in_step_calls.load(Ordering::Relaxed),
        state.in_step_reordered.load(Ordering::Relaxed),
    )
}

/// The target of the `call rel32` whose opcode is at `at`, read with `read`.
pub fn call_target(at: u64, read: &dyn Fn(u64, usize) -> Option<Vec<u8>>) -> Option<u64> {
    let bytes = read(at, 5)?;
    if bytes[0] != 0xE8 {
        return None;
    }
    let rel = i32::from_le_bytes([bytes[1], bytes[2], bytes[3], bytes[4]]);
    Some(at.wrapping_add(5).wrapping_add_signed(i64::from(rel)))
}

/// The function's bytes around the site: each context entry must read back
/// exactly.
pub fn check_context(
    site: u64,
    context: &[(i64, &[u8])],
    read: &dyn Fn(u64, usize) -> Option<Vec<u8>>,
) -> Result<(), String> {
    for (offset, bytes) in context {
        let at = site.wrapping_add_signed(*offset);
        match read(at, bytes.len()) {
            Some(found) if found == *bytes => {}
            Some(_) => return Err(format!("the bytes at {at:#x} are not the measured ones")),
            None => return Err(format!("the bytes at {at:#x} are unreadable")),
        }
    }
    Ok(())
}

/// Reads `len` bytes of the game's code, if readable.
fn read_code(at: u64, len: usize) -> Option<Vec<u8>> {
    let address = usize::try_from(at).ok()?;
    crate::image::readable(address, len).then(|| {
        // SAFETY: `len` readable bytes of the game's image.
        unsafe { std::slice::from_raw_parts(address as *const u8, len) }.to_vec()
    })
}

/// What installing one batch came to, for hook.log.
pub fn outcome_line(fix: &str, installed: bool, reason: &str) -> String {
    if installed {
        format!("order fix {fix}: installed ({reason})")
    } else {
        format!("order fix {fix}: off, {reason}")
    }
}

/// Installs one batch's fix, unless switched off, when its site and every
/// check of its shape hold.
fn install_batch(
    batch: &Batch,
    resolved: &ResolvedProfile,
    wanted: bool,
    read: &dyn Fn(u64, usize) -> Option<Vec<u8>>,
    extra: &dyn Fn(u64) -> Result<(), String>,
) -> String {
    if !wanted {
        return outcome_line(
            batch.fix,
            false,
            &format!(
                "{MASTER_ENV} or {} says so; the engine's own order stands. Every game of the room must run with the same setting",
                batch.toggle_env
            ),
        );
    }
    let Some(site) = resolved.get(batch.target) else {
        return outcome_line(
            batch.fix,
            false,
            &format!("the profile has no {:?}", batch.target),
        );
    };
    let site = site.address;
    if let Err(why) = check_context(site, batch.context, read).and_then(|()| extra(site)) {
        return outcome_line(batch.fix, false, &why);
    }
    // SAFETY: a resolved site whose bytes and surroundings were checked,
    // installed before any world exists; only the branches the profile
    // notes reach the site, at its first byte; the hook never unwinds and
    // only permutes the ids of the vectors the engine reads next.
    match unsafe {
        Splice::install(
            site as usize as *mut u8,
            batch.expected,
            batch.steal,
            batch.hook,
        )
    } {
        Ok(splice) => {
            let _kept = std::mem::ManuallyDrop::new(splice);
            outcome_line(batch.fix, true, &format!("at {site:#x}, {}", batch.does))
        }
        Err(error) => outcome_line(batch.fix, false, &format!("the site at {site:#x}: {error}")),
    }
}

/// Installs every person-order fix that resolves on this build. Returns the
/// lines for hook.log, one per fix.
pub fn install(resolved: &ResolvedProfile) -> Vec<String> {
    let master = std::env::var(MASTER_ENV).ok();
    let wanted_batch = |batch: &Batch| {
        wanted(
            master.as_deref(),
            std::env::var(batch.toggle_env).ok().as_deref(),
        )
    };
    let none = |_site: u64| Ok(());
    vec![
        master_line(crate::ticks::wanted(master.as_deref())),
        install_batch(
            &candidates::BATCH,
            resolved,
            wanted_batch(&candidates::BATCH),
            &read_code,
            &none,
        ),
        install_batch(
            &departures::BATCH,
            resolved,
            wanted_batch(&departures::BATCH),
            &read_code,
            &|site| departures::check_emits(site, &read_code).map(|_| ()),
        ),
        install_batch(
            &arrivals::BATCH,
            resolved,
            wanted_batch(&arrivals::BATCH),
            &read_code,
            &|site| arrivals::check_emit(site, resolved, &read_code),
        ),
        install_batch(
            &needs_path::BATCH,
            resolved,
            wanted_batch(&needs_path::BATCH),
            &read_code,
            &|_site| needs_path::resolve_getter(resolved, &read_code),
        ),
        install_batch(
            &freed_ids::BATCH,
            resolved,
            wanted_batch(&freed_ids::BATCH),
            &read_code,
            &none,
        ),
    ]
}

/// The first line: the master switch's answer, and the rule that goes with
/// it.
pub fn master_line(on: bool) -> String {
    if on {
        format!(
            "person-order: on ({MASTER_ENV}=0 turns every person-order fix off); every game of a room must run the same person-order settings"
        )
    } else {
        format!(
            "person-order: off ({MASTER_ENV} says so); every game of a room must run the same person-order settings"
        )
    }
}

/// The candidate buildings of a person's destination draw (TF2's
/// `candidates`). `destination_util::GetTargetsByLandUse` (`0x8e3ac0`, our
/// name) copies the `PersonCapacity` family's node list (8-byte `{entity,
/// index}` records) into a local `vector<Entity>` (`[rsp+0x70]`,
/// `0x8e3d10..0x8e3d3f`), builds one 0x24-byte `TargetDataEntry` per
/// entity in that order on the thread pool (chunks joined in index order),
/// and `BinarySearchIndex` (`0x8e4400`) later sums their free capacity in
/// entry order and binary-searches one draw. The entries also index a
/// sibling vector, so the entity list is sorted before the build, not the
/// entries after it.
pub mod candidates {
    use super::*;

    pub const FIX: &str = "person-candidates-order";
    /// Set to `0` (or `off`), this batch keeps the engine's order.
    pub const TOGGLE_ENV: &str = "TPF3MP_HOOK_PERSON_CANDIDATES_ORDER";
    pub use crate::build_data::native::persons::candidates::CONTEXT;
    pub use crate::build_data::native::persons::candidates::EXPECTED;
    pub use crate::build_data::native::persons::candidates::SITE;
    pub use crate::build_data::native::persons::candidates::VECTOR_AT_RSP;

    static STATE: State = State::new();

    pub static BATCH: Batch = Batch {
        fix: FIX,
        toggle_env: TOGGLE_ENV,
        target: SITE,
        expected: &EXPECTED,
        steal: 8,
        context: CONTEXT,
        does: "the PersonCapacity candidates are put in entity order before their capacities are summed for the destination draw",
        alive_every: 1 << 14,
        vectors,
        hook,
        state: &STATE,
    };

    /// The vector, from the site's stack pointer; `rbx` must name it too,
    /// as the copy loop left it.
    pub fn vectors_at(rsp: u64, rbx: u64) -> Result<Vectors, &'static str> {
        let vector = rsp.wrapping_add(VECTOR_AT_RSP);
        if rbx != vector {
            return Err("rbx is not the candidate vector");
        }
        Ok(Vectors::one(vector))
    }

    fn vectors(regs: &SavedRegs, _probe: &mut Probe) -> Result<Vectors, &'static str> {
        vectors_at(SavedRegs::rsp(regs), regs.rbx)
    }

    unsafe extern "system" fn hook(regs: *mut SavedRegs) {
        run(&BATCH, regs);
    }
}

/// The persons (and cargo) leaving a building (TF2's `departures`).
/// `ecs::SimEntityAtBuildingSystem::Update2` (vf12, `0xb05ea0`) gathers,
/// on the thread pool in node order (chunks of 256 joined in index order,
/// `0xb052c0`), the entities whose stay ran out into two `vector<Entity>`
/// at `[rbp+7]` and `[rbp+0x1f]`, and emits each to its signal (`0xaeaeb0`,
/// a signal of `(Engine&, vector<Entity> const&)`).
/// `SimPersonSystem::NoteAtBuildingPersonsLeave` (`0xb2e950`) seeds one
/// generator from `updateCount` and draws, person by person in batch order,
/// whether the person recomputes its destination. Both vectors are sorted.
pub mod departures {
    use super::*;

    pub const FIX: &str = "person-departures-order";
    pub const TOGGLE_ENV: &str = "TPF3MP_HOOK_PERSON_DEPARTURES_ORDER";
    pub use crate::build_data::native::persons::departures::CONTEXT;
    pub use crate::build_data::native::persons::departures::EMITS;
    pub use crate::build_data::native::persons::departures::EXPECTED;
    pub use crate::build_data::native::persons::departures::FIRST;
    pub use crate::build_data::native::persons::departures::SECOND;
    pub use crate::build_data::native::persons::departures::SITE;

    static STATE: State = State::new();

    pub static BATCH: Batch = Batch {
        fix: FIX,
        toggle_env: TOGGLE_ENV,
        target: SITE,
        expected: &EXPECTED,
        steal: 8,
        context: CONTEXT,
        does: "the persons and cargo leaving buildings are put in entity order before they are signalled",
        alive_every: 1 << 12,
        vectors,
        hook,
        state: &STATE,
    };

    /// Both emits must call one function, the entity-batch signal; it is
    /// what the arrivals fix checks its own emit against.
    pub fn check_emits(
        site: u64,
        read: &dyn Fn(u64, usize) -> Option<Vec<u8>>,
    ) -> Result<u64, String> {
        let first = call_target(site + EMITS[0], read);
        let second = call_target(site + EMITS[1], read);
        match (first, second) {
            (Some(a), Some(b)) if a == b => Ok(a),
            _ => Err(format!(
                "the two emits after {SITE:?} at {site:#x} do not call one signal"
            )),
        }
    }

    pub fn vectors_at(rbp: u64) -> Vectors {
        Vectors::two(
            rbp.wrapping_add_signed(FIRST),
            rbp.wrapping_add_signed(SECOND),
        )
    }

    fn vectors(regs: &SavedRegs, _probe: &mut Probe) -> Result<Vectors, &'static str> {
        Ok(vectors_at(regs.rbp))
    }

    unsafe extern "system" fn hook(regs: *mut SavedRegs) {
        run(&BATCH, regs);
    }
}

/// The persons arriving on foot (TF2's `arrivals`).
/// `ecs::PersonMoveSystem::Update2` (vf12, `0xaecaa0`) gathers the walk
/// arrivals on the thread pool in node order (chunks of 128 joined in index
/// order) into a `vector<Entity>` at `[rbp-0x78]` and emits it (`0xaecfe8`)
/// to `SimPersonSystem::NoteWalkPersonsArrived` (`0xb33a30`), which seeds a
/// generator from `updateCount` and draws stay durations in batch order.
pub mod arrivals {
    use super::*;

    pub const FIX: &str = "person-arrivals-order";
    pub const TOGGLE_ENV: &str = "TPF3MP_HOOK_PERSON_ARRIVALS_ORDER";
    pub use crate::build_data::native::persons::arrivals::EMIT;
    pub use crate::build_data::native::persons::arrivals::EXPECTED;
    pub use crate::build_data::native::persons::arrivals::SITE;
    pub use crate::build_data::native::persons::arrivals::VECTOR;

    static STATE: State = State::new();

    pub static BATCH: Batch = Batch {
        fix: FIX,
        toggle_env: TOGGLE_ENV,
        target: SITE,
        expected: &EXPECTED,
        steal: 8,
        context: &[],
        does: "the persons arriving on foot are put in entity order before they are signalled",
        alive_every: 1 << 12,
        vectors,
        hook,
        state: &STATE,
    };

    /// The emit must call the signal the departures' two emits call.
    pub fn check_emit(
        site: u64,
        resolved: &ResolvedProfile,
        read: &dyn Fn(u64, usize) -> Option<Vec<u8>>,
    ) -> Result<(), String> {
        let Some(departures) = resolved.get(departures::SITE) else {
            return Err(format!(
                "the profile has no {:?} to check the emit against",
                departures::SITE
            ));
        };
        let signal = departures::check_emits(departures.address, read)?;
        match call_target(site + EMIT, read) {
            Some(target) if target == signal => Ok(()),
            _ => Err(format!(
                "the emit after {SITE:?} at {site:#x} does not call the signal at {signal:#x}"
            )),
        }
    }

    pub fn vectors_at(rbp: u64) -> Vectors {
        Vectors::one(rbp.wrapping_add_signed(VECTOR))
    }

    fn vectors(regs: &SavedRegs, _probe: &mut Probe) -> Result<Vectors, &'static str> {
        Ok(vectors_at(regs.rbp))
    }

    unsafe extern "system" fn hook(regs: *mut SavedRegs) {
        run(&BATCH, regs);
    }
}

/// The persons waiting for a path (TF2's `idle`). TF3's
/// `ecs::SimEntityNeedsPathSystem::Update` (vf11, `0xb181b0`; every odd
/// `updateCount`) builds one `PathFactoryInput` per entry of its list
/// `m_systemData->add` (`[[this+0x10]]`, a `vector<Entity>` its
/// `EntityAdded` appends to and its `EntityToBeRemoved` erases from in
/// place: insertion history) and calls `PathFactory::Compute` (`0x8d11e0`),
/// which runs three items per person in chunks and seeds one generator per
/// chunk from `updateCount` and the chunk's start index: each trip's path
/// and mode follow the person's place in the list. The outputs are matched
/// back to `add` by position in the same call (`0xb183c0`), so sorting the
/// list in place before the first read keeps them paired.
///
/// The data is copy-on-write, shared with the replicated engine's copy;
/// the system's own writers take a private copy through the getter first
/// (`0xb186c0`), and so does this fix when the data is shared.
pub mod needs_path {
    use super::*;

    pub const FIX: &str = "person-needs-path-order";
    pub const TOGGLE_ENV: &str = "TPF3MP_HOOK_PERSON_NEEDS_PATH_ORDER";
    pub use crate::build_data::native::persons::needs_path::CONTEXT;
    pub use crate::build_data::native::persons::needs_path::CONTROL;
    pub use crate::build_data::native::persons::needs_path::DATA;
    pub use crate::build_data::native::persons::needs_path::EXPECTED;
    pub use crate::build_data::native::persons::needs_path::GETTER;
    pub use crate::build_data::native::persons::needs_path::GETTER_CALL;
    pub use crate::build_data::native::persons::needs_path::SITE;
    pub use crate::build_data::native::persons::needs_path::USES;

    type Getter = unsafe extern "system" fn(shared: u64) -> u64;
    static GETTER_AT: AtomicU64 = AtomicU64::new(0);
    static UNSHARED: AtomicU64 = AtomicU64::new(0);
    static STATE: State = State::new();

    pub static BATCH: Batch = Batch {
        fix: FIX,
        toggle_env: TOGGLE_ENV,
        target: SITE,
        expected: &EXPECTED,
        steal: 8,
        context: CONTEXT,
        does: "the persons waiting for a path are put in entity order before PathFactory::Compute seeds by their place",
        alive_every: 1 << 12,
        vectors,
        hook,
        state: &STATE,
    };

    /// Finds the copy-on-write getter through the call the profile names
    /// and checks its head.
    pub fn resolve_getter(
        resolved: &ResolvedProfile,
        read: &dyn Fn(u64, usize) -> Option<Vec<u8>>,
    ) -> Result<(), String> {
        let Some(call) = resolved.get(GETTER_CALL) else {
            return Err(format!("the profile has no {GETTER_CALL:?}"));
        };
        let Some(getter) = call_target(call.address, read) else {
            return Err(format!("no call at {:#x}", call.address));
        };
        if read(getter, GETTER.len()).as_deref() != Some(&GETTER[..]) {
            return Err(format!(
                "the data getter at {getter:#x} is not the measured copy-on-write getter"
            ));
        }
        GETTER_AT.store(getter, Ordering::Release);
        Ok(())
    }

    /// Whether the data must be copied first: a control block whose use
    /// count is above 1.
    pub fn shared(uses: Option<i32>) -> Result<bool, &'static str> {
        match uses {
            Some(n) if n >= 1 => Ok(n > 1),
            Some(_) => Err("the list's data has no owner"),
            None => Err("the list's control block is unreadable"),
        }
    }

    fn vectors(regs: &SavedRegs, probe: &mut Probe) -> Result<Vectors, &'static str> {
        let system = regs.r13;
        let control: u64 = probe
            .read(system.wrapping_add(CONTROL))
            .ok_or("the system's data is unreadable")?;
        if control == 0 {
            return Err("the list's data has no control block");
        }
        let uses: Option<i32> = probe.read(control.wrapping_add(USES));
        if shared(uses)? {
            let getter = GETTER_AT.load(Ordering::Acquire);
            if getter == 0 {
                return Err("the data getter is not resolved");
            }
            // SAFETY: the engine's own copy-on-write getter (head checked at
            // install), called as its writers call it, on the thread that
            // runs the system's update, with the system's shared pointer.
            let fun: Getter = unsafe { std::mem::transmute::<usize, Getter>(getter as usize) };
            unsafe { fun(system.wrapping_add(DATA)) };
            let n = UNSHARED.fetch_add(1, Ordering::Relaxed) + 1;
            if n == 1 {
                log::line(&format!(
                    "order fix {FIX}: the list's data was shared; took a private copy as the system's writers do"
                ));
            }
        }
        let data: u64 = probe
            .read(system.wrapping_add(DATA))
            .ok_or("the system's data pointer is unreadable")?;
        if data == 0 {
            return Err("the system has no data");
        }
        Ok(Vectors::one(data))
    }

    unsafe extern "system" fn hook(regs: *mut SavedRegs) {
        run(&BATCH, regs);
    }
}

/// The ids freed by one modification (TF2's `freed ids`).
/// `ecs::Engine::EndModification` (`0x2bb4d90`) appends the modification's
/// removed ids (`m_betweenChanges`, `[[engine+0x1f0]]`, a `vector<Entity>`
/// in `RemoveEntity` order) to the FIFO free-id deque at `engine+0xd8`
/// (`0x2bb1110`, insert at its end), and `AddEntity` (`0x2bb37b0`) pops its
/// front. Sorted before the append, the deque depends only on which ids
/// each modification removed. The replicated engine replays the same
/// removals through the same function (`Replicator::Apply`, `0x2bb4489`),
/// so it sorts alike.
pub mod freed_ids {
    use super::*;

    pub const FIX: &str = "freed-id-order";
    pub const TOGGLE_ENV: &str = "TPF3MP_HOOK_FREED_ID_ORDER";
    pub use crate::build_data::native::persons::freed_ids::EXPECTED;
    pub use crate::build_data::native::persons::freed_ids::SITE;

    static STATE: State = State::new();

    pub static BATCH: Batch = Batch {
        fix: FIX,
        toggle_env: TOGGLE_ENV,
        target: SITE,
        expected: &EXPECTED,
        steal: 7,
        context: &[],
        does: "each modification's freed ids join the free-id queue in entity order",
        alive_every: 1 << 16,
        vectors,
        hook,
        state: &STATE,
    };

    fn vectors(regs: &SavedRegs, probe: &mut Probe) -> Result<Vectors, &'static str> {
        let vector: u64 = probe
            .read(regs.r12)
            .ok_or("the removed-ids slot is unreadable")?;
        if vector == 0 {
            return Err("the modification has no removed ids");
        }
        Ok(Vectors::one(vector))
    }

    unsafe extern "system" fn hook(regs: *mut SavedRegs) {
        run(&BATCH, regs);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn empty() -> ResolvedProfile {
        ResolvedProfile {
            name: "empty".into(),
            targets: Vec::new(),
            absent_optional: Vec::new(),
        }
    }

    const ALL: [&Batch; 5] = [
        &candidates::BATCH,
        &departures::BATCH,
        &arrivals::BATCH,
        &needs_path::BATCH,
        &freed_ids::BATCH,
    ];

    #[test]
    fn ids_are_sorted_ascending_and_a_sorted_batch_is_left_alone() {
        let mut ids = [30, 10, 20, 10];
        assert_eq!(sort_ids(&mut ids), Sorted::Reordered);
        assert_eq!(ids, [10, 10, 20, 30]);
        assert_eq!(sort_ids(&mut ids), Sorted::Unchanged);
        let mut one = [5];
        assert_eq!(sort_ids(&mut one), Sorted::Unchanged);
        assert_eq!(sort_ids(&mut []), Sorted::Unchanged);
    }

    #[test]
    fn two_histories_of_one_batch_sort_alike() {
        let mut a = [7, 3, 9, 1, 4];
        let mut b = [4, 1, 9, 7, 3];
        sort_ids(&mut a);
        sort_ids(&mut b);
        assert_eq!(a, b);
    }

    #[test]
    fn only_a_whole_vector_of_ids_is_sorted() {
        assert_eq!(vector_len(0, 0, 0), Ok(0));
        assert!(vector_len(0, 8, 8).is_err());
        assert_eq!(vector_len(0x1000, 0x100c, 0x1010), Ok(3));
        assert!(vector_len(0x1000, 0x100a, 0x1010).is_err(), "not whole ids");
        assert!(vector_len(0x1002, 0x100a, 0x1010).is_err(), "misaligned");
        assert!(
            vector_len(0x1010, 0x1000, 0x1010).is_err(),
            "end before begin"
        );
        assert!(vector_len(0x1000, 0x1010, 0x100c).is_err(), "past capacity");
        assert!(vector_len(0x1000, 0x1000 + 4 * (MAX_IDS + 1), u64::MAX).is_err());
    }

    /// A `std::vector<Entity>` header and its ids in this test's memory.
    #[cfg(windows)]
    struct Vector {
        header: Box<[u64; 3]>,
        _ids: Vec<i32>,
    }

    #[cfg(windows)]
    impl Vector {
        fn new(ids: &[i32]) -> Self {
            let mut ids = ids.to_vec();
            ids.reserve(4);
            let begin = ids.as_mut_ptr() as u64;
            let header = Box::new([
                begin,
                begin + 4 * ids.len() as u64,
                begin + 4 * ids.capacity() as u64,
            ]);
            Self { header, _ids: ids }
        }

        fn at(&self) -> u64 {
            self.header.as_ptr() as u64
        }

        fn ids(&self) -> Vec<i32> {
            let n = ((self.header[1] - self.header[0]) / 4) as usize;
            // SAFETY: the ids this vector owns.
            unsafe { std::slice::from_raw_parts(self.header[0] as *const i32, n) }.to_vec()
        }
    }

    // Reads real memory through the hook's readable check, which only
    // answers on Windows.
    #[cfg(windows)]
    #[test]
    fn a_vector_in_memory_is_sorted_in_place() {
        let vector = Vector::new(&[9, 2, 5]);
        let mut probe = Probe::new();
        assert_eq!(sort_vector(vector.at(), &mut probe), Ok(Sorted::Reordered));
        assert_eq!(vector.ids(), vec![2, 5, 9]);
        assert_eq!(sort_vector(vector.at(), &mut probe), Ok(Sorted::Unchanged));
        assert!(sort_vector(0, &mut probe).is_err(), "unreadable header");
    }

    fn regs() -> SavedRegs {
        SavedRegs {
            rflags: 0,
            r15: 0,
            r14: 0,
            r13: 0,
            r12: 0,
            r11: 0,
            r10: 0,
            r9: 0,
            r8: 0,
            rdi: 0,
            rsi: 0,
            rbp: 0,
            rbx: 0,
            rdx: 0,
            rcx: 0,
            rax: 0,
        }
    }

    #[test]
    fn each_site_finds_its_vectors_where_the_engine_keeps_them() {
        // departures: [rbp+7] and [rbp+0x1f]; arrivals: [rbp-0x78].
        assert_eq!(departures::vectors_at(0x1000).as_slice(), &[0x1007, 0x101f]);
        assert_eq!(arrivals::vectors_at(0x1000).as_slice(), &[0xf88]);
        // candidates: [rsp+0x70], which rbx must name.
        assert_eq!(
            candidates::vectors_at(0x2000, 0x2070).map(|v| v.as_slice().to_vec()),
            Ok(vec![0x2070])
        );
        assert!(candidates::vectors_at(0x2000, 0x2078).is_err());
    }

    // Reads real memory through the hook's readable check, which only
    // answers on Windows.
    #[cfg(windows)]
    #[test]
    fn the_freed_ids_hook_sorts_the_modifications_removed_ids() {
        let vector = Vector::new(&[44, 12, 30]);
        let slot = Box::new(vector.at());
        let mut regs = regs();
        regs.r12 = &*slot as *const u64 as u64;
        run(&freed_ids::BATCH, &mut regs);
        assert_eq!(vector.ids(), vec![12, 30, 44]);
        // An empty slot is a refusal, nothing written.
        let none = Box::new(0u64);
        regs.r12 = &*none as *const u64 as u64;
        run(&freed_ids::BATCH, &mut regs);
        assert!(freed_ids::BATCH.state.refused.load(Ordering::Relaxed) >= 1);
    }

    // Reads real memory through the hook's readable check, which only
    // answers on Windows.
    #[cfg(windows)]
    #[test]
    fn the_departures_hook_sorts_both_batches() {
        let persons = Vector::new(&[8, 3]);
        let cargo = Vector::new(&[7, 1, 4]);
        // A frame whose [rbp+7] and [rbp+0x1f] hold the two headers.
        let mut frame = vec![0u8; 0x80];
        let rbp = frame.as_mut_ptr() as u64 + 0x20;
        for (offset, vector) in [(7i64, &persons), (0x1f, &cargo)] {
            let at = (rbp.wrapping_add_signed(offset) - frame.as_ptr() as u64) as usize;
            for (i, word) in vector.header.iter().enumerate() {
                frame[at + 8 * i..at + 8 * i + 8].copy_from_slice(&word.to_le_bytes());
            }
        }
        let mut regs = regs();
        regs.rbp = rbp;
        run(&departures::BATCH, &mut regs);
        assert_eq!(persons.ids(), vec![3, 8]);
        assert_eq!(cargo.ids(), vec![1, 4, 7]);
    }

    #[test]
    fn a_shared_list_is_copied_first() {
        assert_eq!(needs_path::shared(Some(1)), Ok(false));
        assert_eq!(needs_path::shared(Some(2)), Ok(true));
        assert!(needs_path::shared(Some(0)).is_err());
        assert!(needs_path::shared(None).is_err());
    }

    // Reads real memory through the hook's readable check, which only
    // answers on Windows.
    #[cfg(windows)]
    #[test]
    fn the_needs_path_hook_sorts_an_unshared_list() {
        // The system: [+0x10] the data (the vector itself), [+0x18] the
        // control block, whose use count at +8 is 1.
        let list = Vector::new(&[600, 100, 300]);
        let control = Box::new([0u32; 4]);
        let mut control = control;
        control[2] = 1;
        let mut system = Box::new([0u64; 4]);
        system[2] = list.at();
        system[3] = control.as_ptr() as u64;
        let mut regs = regs();
        regs.r13 = system.as_mut_ptr() as u64;
        run(&needs_path::BATCH, &mut regs);
        assert_eq!(list.ids(), vec![100, 300, 600]);
    }

    #[test]
    fn calls_and_context_are_read_from_the_code() {
        // call rel32 at 0x100 to 0x200: rel = 0x200 - 0x105.
        let code = |at: u64, len: usize| -> Option<Vec<u8>> {
            let mut image = vec![0u8; 0x300];
            image[0x100] = 0xE8;
            image[0x101..0x105].copy_from_slice(&(0xfb_i32).to_le_bytes());
            image[0x120..0x124].copy_from_slice(&[1, 2, 3, 4]);
            let at = usize::try_from(at).ok()?;
            image.get(at..at + len).map(<[u8]>::to_vec)
        };
        assert_eq!(call_target(0x100, &code), Some(0x200));
        assert_eq!(call_target(0x101, &code), None);
        assert_eq!(
            check_context(0x110, &[(0x10, &[1, 2, 3, 4])], &code),
            Ok(())
        );
        assert!(check_context(0x110, &[(0x10, &[1, 2, 3, 5])], &code).is_err());
        assert!(check_context(0x110, &[(0x1000, &[1])], &code).is_err());
    }

    #[test]
    fn every_fix_is_on_unless_its_switch_or_the_master_says_off() {
        assert!(wanted(None, None));
        assert!(!wanted(Some("0"), None));
        assert!(!wanted(None, Some("off")));
        assert!(!wanted(Some("1"), Some("0")));
        let resolved = empty();
        let none = |_: u64| Ok(());
        for batch in ALL {
            let line = install_batch(batch, &resolved, false, &read_code, &none);
            assert!(
                line.starts_with(&format!("order fix {}: off", batch.fix))
                    && line.contains(MASTER_ENV)
                    && line.contains(batch.toggle_env)
                    && line.contains("same setting"),
                "{line}"
            );
            let line = install_batch(batch, &resolved, true, &read_code, &none);
            assert!(line.contains("the profile has no"), "{line}");
        }
    }

    #[test]
    fn the_master_line_states_the_rule() {
        assert!(master_line(true).starts_with("person-order: on"));
        assert!(master_line(false).starts_with("person-order: off"));
        for on in [true, false] {
            assert!(master_line(on).contains("every game of a room must run the same"));
        }
    }

    #[test]
    fn every_switch_has_its_own_name() {
        let mut names: Vec<&str> = ALL.iter().map(|b| b.toggle_env).collect();
        names.push(MASTER_ENV);
        let mut fixes: Vec<&str> = ALL.iter().map(|b| b.fix).collect();
        names.sort_unstable();
        names.dedup();
        fixes.sort_unstable();
        fixes.dedup();
        assert_eq!(names.len(), ALL.len() + 1);
        assert_eq!(fixes.len(), ALL.len());
    }

    #[test]
    fn the_profile_states_each_sites_bytes() {
        let profile =
            tpf3mp_hookcore::profile::Profile::from_toml(crate::BUILT_IN_PROFILES[0].1).unwrap();
        let target = |name: &str| {
            profile
                .targets
                .iter()
                .find(|t| t.name == name)
                .unwrap_or_else(|| panic!("{name} in the profile"))
        };
        for batch in ALL {
            let site = target(batch.target);
            assert!(!site.required, "{} is optional", batch.target);
            assert!(
                batch.expected.starts_with(&site.prologue),
                "{}: the profile's prologue is the fix's expected bytes",
                batch.target
            );
            assert!(site.prologue.len() >= batch.steal);
            assert!(batch.steal >= 5 && batch.steal <= batch.expected.len());
        }
        assert_eq!(target(needs_path::GETTER_CALL).prologue, vec![0xE8]);
    }
}

#[cfg(all(test, windows, target_arch = "x86_64"))]
mod splice_tests {
    use super::*;

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

    /// The freed-id site's real bytes in a hand-written function: `push r12;
    /// push r13; push rbp; mov r12, imm64; mov r13, imm64; mov rbp, rsp;
    /// sub rsp, 0x80; <site>; ret-target: ...` where the call reaches a
    /// `ret` placed right after it, and then the function returns the first
    /// id of the vector `[r12]` names.
    #[test]
    fn the_freed_id_hook_sorts_through_its_real_site() {
        let ids = vec![50i32, 20, 40, 10];
        let mut ids = ids;
        let begin = ids.as_mut_ptr() as u64;
        let header = Box::new([begin, begin + 16, begin + 16]);
        let slot = Box::new(header.as_ptr() as u64);
        // engine+0xf8 is read by the site: a fake engine big enough.
        let engine = vec![0u8; 0x200];

        let mut code = vec![0x41, 0x54, 0x41, 0x55, 0x55]; // push r12; push r13; push rbp
        code.extend_from_slice(&[0x49, 0xBC]); // mov r12, imm64
        code.extend_from_slice(&(&*slot as *const u64 as u64).to_le_bytes());
        code.extend_from_slice(&[0x49, 0xBD]); // mov r13, imm64
        code.extend_from_slice(&(engine.as_ptr() as u64).to_le_bytes());
        code.extend_from_slice(&[0x48, 0x89, 0xE5]); // mov rbp, rsp
        code.extend_from_slice(&[0x48, 0x81, 0xEC, 0x80, 0x00, 0x00, 0x00]); // sub rsp, 0x80
        let site_at = code.len();
        code.extend_from_slice(&freed_ids::EXPECTED);
        // The call's rel32: to a `ret` placed after the epilogue below.
        let rel_at = code.len();
        code.extend_from_slice(&[0, 0, 0, 0]);
        // mov rax, [r12]; mov rax, [rax]; mov eax, [rax]; mov rsp, rbp;
        // pop rbp; pop r13; pop r12; ret
        code.extend_from_slice(&[
            0x49, 0x8B, 0x04, 0x24, 0x48, 0x8B, 0x00, 0x8B, 0x00, 0x48, 0x89, 0xEC, 0x5D, 0x41,
            0x5D, 0x41, 0x5C, 0xC3,
        ]);
        let callee_at = code.len();
        code.push(0xC3);
        let rel = (callee_at - (rel_at + 4)) as i32;
        code[rel_at..rel_at + 4].copy_from_slice(&rel.to_le_bytes());
        let page = fixture(&code);
        // SAFETY: the page holds our hand-written function of no arguments.
        let fun: extern "C" fn() -> u32 =
            unsafe { std::mem::transmute::<usize, extern "C" fn() -> u32>(page) };
        assert_eq!(fun(), 50, "without the hook, the removal order stands");

        // SAFETY: the fixture is this test's own code, not running now; the
        // hook only sorts the vector `[r12]` names.
        let splice = unsafe {
            Splice::install(
                (page + site_at) as *mut u8,
                &freed_ids::EXPECTED,
                freed_ids::BATCH.steal,
                freed_ids::BATCH.hook,
            )
        }
        .unwrap();
        assert_eq!(fun(), 10, "with the hook, the lowest id is first");
        assert_eq!(ids, vec![10, 20, 40, 50]);
        drop(splice);
        drop(header);
    }
}
