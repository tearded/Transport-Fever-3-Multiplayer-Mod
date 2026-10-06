//! What the game's own costly systems take, timed in the running game
//! (docs/HOOKS.md, "What the hook costs: the `perf:` lines", the `perf:
//! sim` line; investigation/TF3_SIM_COST_2026-10-05.md).
//!
//! Timing only: each timer calls the game's function with the arguments it
//! was given and adds the call's nanoseconds to a counter, nothing else.
//! On while the hook's timing is ([`crate::perf::ENV`] not `0`); each call
//! costs two clock reads (about 56 ns) against the milliseconds the
//! systems take. A target the profile did not resolve, or a slot that does
//! not hold it, leaves that timer out, and hook.log says which.
//!
//! - `emission-grid`: `ecs::EmissionGridSystem::Update` (`0xaa9230`), the
//!   noise and pollution grids' diffusion, wind and averaging passes.
//! - `emission-emitters`: `ecs::EmissionEmitterSystem::Update2`
//!   (`0xaa51c0`), the emitters' splat into the grids.
//! - `towns`: `ecs::TownSystem`'s update (`0xb61cc0`), which sums the grids
//!   per district (`CalculateTownPollution`).
//! - `parcel-collision`: `parcel_util::UpdateParcelCollision`
//!   (`0x9312e0`), the octree walk of each applied proposal over the union
//!   of its boxes plus 50 m; with the boxes and the union's area.
//!
//! The three systems are reached only through their vtable slot (the
//! static proof checks there is no direct call), so the timer replaces the
//! slot's pointer, after checking it holds the resolved function: a
//! detour another feature puts on the function itself still runs, inside
//! the timer. The parcel walk has one call, which is redirected.

#![allow(unsafe_code)]
#![cfg_attr(not(all(windows, target_arch = "x86_64")), allow(dead_code))]

use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};

use tpf3mp_hookcore::profile::ResolvedProfile;

pub use crate::build_data::native::simperf::{
    EMISSION_EMITTERS, EMISSION_EMITTERS_SLOT, EMISSION_GRID, EMISSION_GRID_SLOT, PARCEL_COLLISION,
    PARCEL_COLLISION_CALL, PARCEL_MARGIN, TOWNS, TOWNS_SLOT,
};
use crate::perf::{Counter, Sample};

/// A timed piece of the game.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum System {
    EmissionGrid,
    EmissionEmitters,
    Towns,
    ParcelCollision,
}

impl System {
    pub const ALL: [System; 4] = [
        System::EmissionGrid,
        System::EmissionEmitters,
        System::Towns,
        System::ParcelCollision,
    ];

    /// Its name in the `perf: sim` line.
    pub const fn name(self) -> &'static str {
        match self {
            System::EmissionGrid => "emission-grid",
            System::EmissionEmitters => "emission-emitters",
            System::Towns => "towns",
            System::ParcelCollision => "parcel-collision",
        }
    }

    /// Its profile target.
    pub const fn target(self) -> &'static str {
        match self {
            System::EmissionGrid => EMISSION_GRID,
            System::EmissionEmitters => EMISSION_EMITTERS,
            System::Towns => TOWNS,
            System::ParcelCollision => PARCEL_COLLISION,
        }
    }
}

const N: usize = System::ALL.len();
static COUNTERS: [Counter; N] = [const { Counter::new() }; N];
static INSTALLED: [AtomicBool; N] = [const { AtomicBool::new(false) }; N];
/// What each timer calls: the game's function.
static ORIGINALS: [AtomicUsize; N] = [const { AtomicUsize::new(0) }; N];
/// The parcel walk's boxes, the union's area summed (m²) and the largest.
static PARCEL_BOXES: AtomicU64 = AtomicU64::new(0);
static PARCEL_AREA: AtomicU64 = AtomicU64::new(0);
static PARCEL_MAX_AREA: AtomicU64 = AtomicU64::new(0);

/// A system's update: `this`, the engine, `r8` (the node list or an int),
/// `xmm3` dt (the town update ignores it, and gets it back as it came).
type SystemFn = unsafe extern "system-unwind" fn(usize, usize, usize, f32);
/// The parcel walk: `rcx`, `rdx`, the boxes' vector, `r9`.
type ParcelFn = unsafe extern "system-unwind" fn(usize, usize, *const [usize; 3], usize);

/// The union of `{min x, min y, max x, max y}` boxes as the walk makes it,
/// widened by [`PARCEL_MARGIN`] on every side; its area in m². `None` for
/// no boxes (the walk asserts on those).
pub fn union_area(boxes: &[[f32; 4]]) -> Option<f64> {
    let first = boxes.first()?;
    let mut union = *first;
    for b in boxes {
        union[0] = union[0].min(b[0]);
        union[1] = union[1].min(b[1]);
        union[2] = union[2].max(b[2]);
        union[3] = union[3].max(b[3]);
    }
    let width = f64::from(union[2] + PARCEL_MARGIN) - f64::from(union[0] - PARCEL_MARGIN);
    let height = f64::from(union[3] + PARCEL_MARGIN) - f64::from(union[1] - PARCEL_MARGIN);
    Some(width.max(0.0) * height.max(0.0))
}

/// The most boxes the timer reads to measure the union.
const MAX_BOXES: usize = 1 << 16;

/// The boxes of the vector at `vector` (begin, end, capacity), when they
/// read as a vector of 16-byte boxes.
fn boxes<'a>(vector: *const [usize; 3]) -> Option<&'a [[f32; 4]]> {
    if vector.is_null() || !crate::image::readable_cached(vector as usize, 24) {
        return None;
    }
    // SAFETY: 24 readable bytes, checked just above.
    let [begin, end, _] = unsafe { std::ptr::read_unaligned(vector) };
    let len = end.checked_sub(begin)?;
    let count = len / 16;
    if len % 16 != 0
        || count > MAX_BOXES
        || begin % 4 != 0
        || !crate::image::readable_cached(begin, len)
    {
        return None;
    }
    // SAFETY: `count` readable, aligned boxes, checked just above; the
    // caller's vector, which nothing changes while the walk runs.
    Some(unsafe { std::slice::from_raw_parts(begin as *const [f32; 4], count) })
}

fn note_parcel_boxes(vector: *const [usize; 3]) {
    let Some(boxes) = boxes(vector) else { return };
    PARCEL_BOXES.fetch_add(boxes.len() as u64, Ordering::Relaxed);
    if let Some(area) = union_area(boxes) {
        let area = area.min(u64::MAX as f64) as u64;
        PARCEL_AREA.fetch_add(area, Ordering::Relaxed);
        PARCEL_MAX_AREA.fetch_max(area, Ordering::Relaxed);
    }
}

/// Calls the game's `system` and times it.
///
/// # Safety
///
/// The arguments are the ones the game passed to the slot.
unsafe fn timed(system: System, this: usize, engine: usize, r8: usize, dt: f32) {
    let original = ORIGINALS[system as usize].load(Ordering::Acquire);
    // SAFETY: set before the slot pointed here: the game's function, whose
    // signature SystemFn is (simperf.rs in the bundle).
    let original = unsafe { std::mem::transmute::<usize, SystemFn>(original) };
    let start = crate::perf::start();
    // SAFETY: the game's own call, with its own arguments.
    unsafe { original(this, engine, r8, dt) };
    if let Some(start) = start {
        COUNTERS[system as usize].add(crate::perf::nanos_since(start));
    }
}

unsafe extern "system-unwind" fn emission_grid(this: usize, engine: usize, r8: usize, dt: f32) {
    // SAFETY: the slot's caller's arguments.
    unsafe { timed(System::EmissionGrid, this, engine, r8, dt) }
}

unsafe extern "system-unwind" fn emission_emitters(this: usize, engine: usize, r8: usize, dt: f32) {
    // SAFETY: the slot's caller's arguments.
    unsafe { timed(System::EmissionEmitters, this, engine, r8, dt) }
}

unsafe extern "system-unwind" fn towns(this: usize, engine: usize, r8: usize, dt: f32) {
    // SAFETY: the slot's caller's arguments.
    unsafe { timed(System::Towns, this, engine, r8, dt) }
}

unsafe extern "system-unwind" fn parcel_collision(
    rcx: usize,
    rdx: usize,
    boxes: *const [usize; 3],
    r9: usize,
) {
    let original = ORIGINALS[System::ParcelCollision as usize].load(Ordering::Acquire);
    // SAFETY: set before the call was redirected here: the game's walk.
    let original = unsafe { std::mem::transmute::<usize, ParcelFn>(original) };
    let start = crate::perf::start();
    if start.is_some() {
        note_parcel_boxes(boxes);
    }
    // SAFETY: the game's own call, with its own arguments.
    unsafe { original(rcx, rdx, boxes, r9) };
    if let Some(start) = start {
        COUNTERS[System::ParcelCollision as usize].add(crate::perf::nanos_since(start));
    }
}

/// One window's samples: `None` for a timer that is not in.
#[derive(Debug, Clone, PartialEq)]
pub struct Window {
    pub systems: [Option<Sample>; N],
    pub parcel_boxes: u64,
    /// Summed and largest union area, m².
    pub parcel_area: u64,
    pub parcel_max_area: u64,
    /// `GetComponentDataIndex`'s calls (`crate::fastindex`), when counted.
    pub lookups: Option<u64>,
    pub lookup_state: crate::fastindex::State,
}

/// Takes every counter, zero again.
pub fn take() -> Window {
    let mut systems = [None; N];
    for (i, system) in systems.iter_mut().enumerate() {
        let sample = COUNTERS[i].take();
        if INSTALLED[i].load(Ordering::Acquire) {
            *system = Some(sample);
        }
    }
    Window {
        systems,
        parcel_boxes: PARCEL_BOXES.swap(0, Ordering::Relaxed),
        parcel_area: PARCEL_AREA.swap(0, Ordering::Relaxed),
        parcel_max_area: PARCEL_MAX_AREA.swap(0, Ordering::Relaxed),
        lookups: crate::fastindex::take_calls(),
        lookup_state: crate::fastindex::state(),
    }
}

/// The `perf: sim` line: every timer's `calls/total ms/mean µs`, their sum
/// per update, the parcel walk's boxes and union areas, and the component
/// lookups. `None` when no timer is in and nothing is counted.
pub fn line(window: &Window, updates: u64) -> Option<String> {
    use crate::fastindex::State;
    if window.systems.iter().all(Option::is_none) && window.lookup_state == State::Off {
        return None;
    }
    let mut out = String::from("perf: sim ");
    let mut total = 0u64;
    for (i, (system, sample)) in System::ALL.iter().zip(window.systems.iter()).enumerate() {
        if i > 0 {
            out.push_str(", ");
        }
        match sample {
            Some(sample) => {
                total += sample.nanos;
                out.push_str(&format!(
                    "{} {}/{:.2}ms/{:.2}us",
                    system.name(),
                    sample.calls,
                    sample.millis(),
                    sample.mean_micros()
                ));
            }
            None => out.push_str(&format!("{} absent", system.name())),
        }
    }
    if updates > 0 {
        out.push_str(&format!(
            " ({:.3} ms/update together)",
            total as f64 / 1e6 / updates as f64
        ));
    }
    if let Some(Some(walks)) = window.systems.get(System::ParcelCollision as usize) {
        let mean = if walks.calls == 0 {
            0.0
        } else {
            window.parcel_area as f64 / walks.calls as f64 / 1e6
        };
        out.push_str(&format!(
            "; parcel boxes {}, union mean {:.3} km², max {:.3} km²",
            window.parcel_boxes,
            mean,
            window.parcel_max_area as f64 / 1e6
        ));
    }
    out.push_str(&match (window.lookup_state, window.lookups) {
        (State::Counting, Some(n)) => format!("; component-index {n} calls"),
        (State::Off, _) => "; component-index the game's own (fast lookup off)".to_owned(),
        _ => format!(
            "; component-index fast, not counted ({}={} counts it)",
            crate::perf::ENV,
            crate::perf::FULL
        ),
    });
    Some(out)
}

/// The slot must hold the function the profile resolved.
pub fn check_slot(found: Option<u64>, function: u64) -> Result<(), String> {
    match found {
        None => Err("the slot is unreadable".to_owned()),
        Some(found) if found == function => Ok(()),
        Some(found) => Err(format!(
            "the slot holds {found:#x}, not the function at {function:#x}"
        )),
    }
}

/// Installs every timer it can while the hook's timing is on; `base` the
/// image's (the slots are RVAs). Returns the lines for hook.log.
pub fn install(resolved: &ResolvedProfile, base: u64) -> Vec<String> {
    if !crate::perf::enabled() {
        return vec![format!(
            "perf: sim timers off ({} says so)",
            crate::perf::ENV
        )];
    }
    let slots: [(System, u64, SystemFn); 3] = [
        (System::EmissionGrid, EMISSION_GRID_SLOT, emission_grid),
        (
            System::EmissionEmitters,
            EMISSION_EMITTERS_SLOT,
            emission_emitters,
        ),
        (System::Towns, TOWNS_SLOT, towns),
    ];
    let mut lines = Vec::new();
    for (system, slot, hook) in slots {
        let outcome = resolved
            .get(system.target())
            .ok_or_else(|| format!("the profile has no {:?}", system.target()))
            .and_then(|target| {
                install_slot(
                    system,
                    base.wrapping_add(slot),
                    target.address,
                    hook as usize,
                )
            });
        lines.push(timer_line(system, outcome));
    }
    let outcome = match (
        resolved.get(PARCEL_COLLISION),
        resolved.get(PARCEL_COLLISION_CALL),
    ) {
        (Some(function), Some(call)) => install_call(call.address, function.address),
        _ => Err(format!(
            "the profile lacks {PARCEL_COLLISION:?} or {PARCEL_COLLISION_CALL:?}"
        )),
    };
    lines.push(timer_line(System::ParcelCollision, outcome));
    lines
}

fn timer_line(system: System, outcome: Result<String, String>) -> String {
    match outcome {
        Ok(how) => format!("perf: sim timer {}: in ({how})", system.name()),
        Err(why) => format!("perf: sim timer {}: absent, {why}", system.name()),
    }
}

fn install_slot(system: System, slot: u64, function: u64, hook: usize) -> Result<String, String> {
    use tpf3mp_hookcore::detour::Rewrite;
    let at = usize::try_from(slot).map_err(|_| "a slot past usize".to_owned())?;
    let found = crate::image::readable(at, 8).then(|| {
        // SAFETY: eight readable bytes, checked just above.
        unsafe { std::ptr::read_unaligned(at as *const u64) }
    });
    check_slot(found, function)?;
    ORIGINALS[system as usize].store(function as usize, Ordering::Release);
    // SAFETY: the vtable slot holds the function (checked); no game thread
    // runs yet, and the hook has the function's ABI and calls it.
    let rewrite = unsafe {
        Rewrite::install(
            at as *mut u8,
            &function.to_le_bytes(),
            &(hook as u64).to_le_bytes(),
        )
    }
    .map_err(|error| format!("the slot at {slot:#x}: {error}"))?;
    // Kept for the life of the process. ManuallyDrop, not forget: where
    // patching is unsupported the type has no Drop (clippy::forget_non_drop).
    let _kept = std::mem::ManuallyDrop::new(rewrite);
    INSTALLED[system as usize].store(true, Ordering::Release);
    Ok(format!(
        "vtable slot {slot:#x}, the function at {function:#x}"
    ))
}

fn install_call(call: u64, function: u64) -> Result<String, String> {
    use tpf3mp_hookcore::detour::CallRedirect;
    let system = System::ParcelCollision;
    ORIGINALS[system as usize].store(function as usize, Ordering::Release);
    // SAFETY: the profile resolved the call and its callee; CallRedirect
    // checks the call reaches it; no game thread runs yet, and the hook
    // has the walk's ABI and calls it.
    let redirect = unsafe {
        CallRedirect::install(
            call as usize as *mut u8,
            function as usize,
            parcel_collision as *const u8,
        )
    }
    .map_err(|error| format!("the call at {call:#x}: {error}"))?;
    // Kept for the life of the process. ManuallyDrop, not forget: where
    // patching is unsupported the type has no Drop (clippy::forget_non_drop).
    let _kept = std::mem::ManuallyDrop::new(redirect);
    INSTALLED[system as usize].store(true, Ordering::Release);
    Ok(format!("its call at {call:#x}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fastindex::State;

    #[test]
    fn the_union_is_the_walks_query_box() {
        assert_eq!(union_area(&[]), None);
        // One 100 x 20 m box: 200 x 120 m with the margin.
        assert_eq!(union_area(&[[0.0, 0.0, 100.0, 20.0]]), Some(24_000.0));
        // Two far apart: the union spans both.
        let area = union_area(&[[0.0, 0.0, 10.0, 10.0], [5_000.0, 3_000.0, 5_010.0, 3_010.0]]);
        assert_eq!(area, Some(5_110.0 * 3_110.0));
    }

    #[test]
    fn a_slot_must_hold_the_resolved_function() {
        assert!(check_slot(Some(0x1_40aa_9230), 0x1_40aa_9230).is_ok());
        assert!(
            check_slot(Some(0x7ff0_0000_1000), 0x1_40aa_9230)
                .unwrap_err()
                .contains("not the function")
        );
        assert!(check_slot(None, 1).is_err());
    }

    fn window() -> Window {
        Window {
            systems: [
                Some(Sample {
                    calls: 600,
                    nanos: 24_000_000_000,
                }),
                Some(Sample {
                    calls: 600,
                    nanos: 3_000_000_000,
                }),
                None,
                Some(Sample {
                    calls: 40,
                    nanos: 2_000_000_000,
                }),
            ],
            parcel_boxes: 120,
            parcel_area: 40_000_000,
            parcel_max_area: 25_500_000,
            lookups: Some(123_456_789),
            lookup_state: State::Counting,
        }
    }

    #[test]
    fn the_sim_line_gives_each_system_and_the_parcel_walks() {
        assert_eq!(
            line(&window(), 600).unwrap(),
            "perf: sim emission-grid 600/24000.00ms/40000.00us, \
             emission-emitters 600/3000.00ms/5000.00us, towns absent, \
             parcel-collision 40/2000.00ms/50000.00us (48.333 ms/update together); \
             parcel boxes 120, union mean 1.000 km², max 25.500 km²; \
             component-index 123456789 calls"
        );
    }

    #[test]
    fn the_sim_line_says_how_the_lookup_runs() {
        let mut w = window();
        w.lookups = None;
        w.lookup_state = State::Fast;
        let fast = line(&w, 0).unwrap();
        assert!(
            fast.ends_with("; component-index fast, not counted (TPF3MP_HOOK_PERF=full counts it)"),
            "{fast}"
        );
        assert!(!fast.contains("ms/update"), "{fast}");
        w.lookup_state = State::Off;
        assert!(
            line(&w, 1)
                .unwrap()
                .ends_with("; component-index the game's own (fast lookup off)")
        );
        w.systems = [None; N];
        assert_eq!(line(&w, 1), None, "nothing in, no line");
    }

    #[test]
    fn a_timer_line_says_in_or_absent() {
        assert_eq!(
            timer_line(System::Towns, Ok("vtable slot 0x1".into())),
            "perf: sim timer towns: in (vtable slot 0x1)"
        );
        assert_eq!(
            timer_line(System::EmissionGrid, Err("why".into())),
            "perf: sim timer emission-grid: absent, why"
        );
    }

    /// A timer passes the game's arguments through, all of them, and adds
    /// one call.
    #[test]
    fn a_timer_calls_the_game_with_its_arguments() {
        use std::sync::Mutex;
        static SEEN: Mutex<Vec<(usize, usize, usize, u32)>> = Mutex::new(Vec::new());
        unsafe extern "system-unwind" fn game(a: usize, b: usize, c: usize, dt: f32) {
            SEEN.lock().unwrap().push((a, b, c, dt.to_bits()));
        }
        ORIGINALS[System::Towns as usize].store(game as *const () as usize, Ordering::Release);
        COUNTERS[System::Towns as usize].take();
        // SAFETY: `game` takes what `towns` passes.
        unsafe { towns(1, 2, 0xffff_ffff_0000_0003, 0.2) };
        assert_eq!(
            SEEN.lock().unwrap().as_slice(),
            &[(1, 2, 0xffff_ffff_0000_0003, 0.2f32.to_bits())]
        );
        if crate::perf::enabled() {
            assert_eq!(COUNTERS[System::Towns as usize].take().calls, 1);
        }
    }
}
