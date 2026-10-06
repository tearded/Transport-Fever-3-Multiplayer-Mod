//! The emission grid's update, fused and bit-identical
//! (investigation/TF3_SIM_COST_2026-10-05.md §1; docs/HOOKS.md,
//! "The fast emission grid").
//!
//! `ecs::EmissionGridSystem::Update` moves the noise and pollution grids
//! (16 m cells; 1,602 x 16,002 each on a 100 x 1000-tile map) one step per
//! simulation update: Diffuse, Wind (pollution only) and Average, each a
//! full-grid pass on the game's thread pool that reads one buffer and
//! writes the shared temporary, then a swap. That is about 68 bytes of
//! memory traffic per cell and update: 1.75 GB on a 100 x 1000-tile map,
//! 0.22 GB on Gigantomaniac. The grid feeds the towns' noise and
//! pollution ratings and is saved, so it is simulation state: a room's games
//! must agree on every bit of it.
//!
//! The hook redirects `Update`'s three dispatcher calls. Diffuse's and
//! Wind's only note what they were asked; Average's then runs the whole
//! step at once ([`fused`]): one pass over row bands on the hook's own
//! threads ([`pool`]), eight cells at a time, each cell computed with the
//! game's scalar operations in the game's order. The step leaves every
//! buffer, the border ring and the temporary included, exactly as the
//! game's three passes and swaps would; `Update` itself (its lookups,
//! swaps, border checks and step count) is the game's.
//!
//! Fail-closed, at every level:
//!
//! - at install: [`ENV`] `=0` leaves the game's update; so does a
//!   profile without the target, a CPU without AVX, or any byte of the
//!   modelled code (`Update`, the three dispatchers, the three kernels)
//!   that differs from what was read;
//! - each step: anything the model does not cover (sizes that differ, a
//!   weight or wind the game's own code would assert on, a non-default
//!   MXCSR, calls out of the expected order) runs the game's own
//!   dispatchers instead, in the game's order;
//! - in the game: the first fused steps of each grid, and one in every
//!   [`CHECK_EVERY`] after, first run windows of the real grid through the
//!   game's own kernels and the fused step side by side; any difference
//!   turns the fused step off for the rest of the game and is logged.
//!
//! Since the result is the game's to the bit, a game with the hook and one
//! without agree: nothing for the room to set.

#![allow(unsafe_code)]

pub mod fused;
pub mod pool;

#[cfg(all(test, windows, target_arch = "x86_64"))]
mod original_tests;

use std::cell::RefCell;
use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::Instant;

use tpf3mp_hookcore::profile::ResolvedProfile;

pub use crate::build_data::native::emission::{
    AVERAGE_CALL, AVERAGE_DISPATCH, AVERAGE_KERNEL, CODE, COMP_AVERAGE, COMP_CONCENTRATION,
    COMP_GRID_POINT_SIZE, COMP_WIND, Code, DIFFUSE_CALL, DIFFUSE_DISPATCH, DIFFUSE_KERNEL,
    GRID_DATA, GRID_HEIGHT, GRID_WIDTH, SYSTEM_AVERAGE_C, SYSTEM_DECAY_B, SYSTEM_SPREAD_A,
    SYSTEM_TEMP, UPDATE, WIND_CALL, WIND_DISPATCH, WIND_KERNEL,
};
use crate::log;
use fused::{Buffers, Params, Step, Wind};

/// The fix's name in `hook.log`.
pub const FIX: &str = "emission grid";

/// `0` (or `off`) in the game's environment leaves the game's own update;
/// unset, the fused update runs.
pub const ENV: &str = "TPF3MP_HOOK_FAST_EMISSION";

/// The first fused steps of each grid checked against the game's kernels.
pub const FIRST_CHECKS: u64 = 3;
/// After those, one fused step in this many is checked.
pub const CHECK_EVERY: u64 = 1024;
/// Rows per checked window (three windows: top, middle, bottom).
pub const CHECK_ROWS: usize = 12;
/// A summary line in `hook.log` every this many fused grid steps.
pub const REPORT_EVERY: u64 = 4096;
/// Grids smaller than this many cells run on the calling thread alone.
const SMALL_GRID: usize = 1 << 16;
/// Threads at most, the caller's included: the step is bound by memory
/// bandwidth well before this.
const MAX_THREADS: usize = 16;
/// Bands per thread, so a thread that starts late still finds work.
const BANDS_PER_THREAD: usize = 4;
/// Rows a band has at least, so the four rows each band copies first stay
/// a small share of its reads.
const MIN_BAND_ROWS: usize = 32;

/// Whether [`ENV`]'s value leaves the fused update on: unset or empty, `1`,
/// `on`, `true` or `yes` is on, `0`, `off`, `false` or `no` off; anything
/// else is off, and the caller says why.
pub fn wanted(value: Option<&str>) -> Result<bool, String> {
    match value.map(|v| v.trim().to_ascii_lowercase()).as_deref() {
        None | Some("" | "1" | "on" | "true" | "yes") => Ok(true),
        Some("0" | "off" | "false" | "no") => Ok(false),
        Some(other) => Err(format!("{other:?} is not 1 or 0")),
    }
}

/// FNV-1a, 64 bits: the hash [`CODE`] records.
pub fn fnv1a(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325, |h, &b| {
        (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3)
    })
}

/// A `Grid<float>`: `{x0, y0, width, height, std::vector<float>}`.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct Grid {
    pub x0: i32,
    pub y0: i32,
    pub width: i32,
    pub height: i32,
    pub begin: *mut f32,
    pub end: *mut f32,
    pub cap: *mut f32,
}

const _: () = assert!(std::mem::size_of::<Grid>() == 0x28);
const _: () = assert!(std::mem::offset_of!(Grid, width) == GRID_WIDTH);
const _: () = assert!(std::mem::offset_of!(Grid, height) == GRID_HEIGHT);
const _: () = assert!(std::mem::offset_of!(Grid, begin) == GRID_DATA);

impl Grid {
    /// The floats its vector holds.
    fn len(&self) -> usize {
        if self.begin.is_null() || (self.end as usize) < (self.begin as usize) {
            return 0;
        }
        (self.end as usize - self.begin as usize) / 4
    }
}

/// Diffuse's lambda's captures: the system, the component, dt.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
struct DiffuseCtx {
    system: usize,
    comp: usize,
    dt: f32,
}

/// Wind's: the system, the component, the wind (a copy on `Update`'s
/// frame), dt.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
struct WindCtx {
    system: usize,
    comp: usize,
    wind: *const [f32; 2],
    dt: f32,
}

/// Average's: the system, the component.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
struct AverageCtx {
    system: usize,
    comp: usize,
}

/// `LoopImpl<lambda>(pool, &lambda, rows, 48, &out, tag, flag)`.
type Dispatch = unsafe extern "system" fn(usize, *const c_void, i32, i32, usize, usize, usize);
/// `Diffuse(row0, row1, &src, &dst, a, b, dt)`.
type DiffuseKernel = unsafe extern "system" fn(i32, i32, *const Grid, *mut Grid, f32, f32, f32);
/// `Wind(row0, row1, &src, &dst, &gridPointSize, &wind, dt)`.
type WindKernel = unsafe extern "system" fn(
    i32,
    i32,
    *const Grid,
    *mut Grid,
    *const [f32; 2],
    *const [f32; 2],
    f32,
);
/// `Average(row0, row1, component, &dst, c)`.
type AverageKernel = unsafe extern "system" fn(i32, i32, usize, *mut Grid, f32);

/// Where the game's code is: the dispatchers the hook replaces (and calls
/// when the model does not cover a step) and the kernels the self-check
/// runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Game {
    pub diffuse_dispatch: usize,
    pub wind_dispatch: usize,
    pub average_dispatch: usize,
    pub diffuse_kernel: usize,
    pub wind_kernel: usize,
    pub average_kernel: usize,
}

impl Game {
    /// From `Update`'s address, by the recorded offsets.
    pub fn from_update(update: usize) -> Self {
        let at = |offset: i64| update.wrapping_add_signed(offset as isize);
        Self {
            diffuse_dispatch: at(DIFFUSE_DISPATCH),
            wind_dispatch: at(WIND_DISPATCH),
            average_dispatch: at(AVERAGE_DISPATCH),
            diffuse_kernel: at(DIFFUSE_KERNEL),
            wind_kernel: at(WIND_KERNEL),
            average_kernel: at(AVERAGE_KERNEL),
        }
    }
}

static GAME: Mutex<Option<Game>> = Mutex::new(None);
/// Set when a check failed: the game's own update from then on.
static BROKEN: AtomicBool = AtomicBool::new(false);
static FUSED: AtomicU64 = AtomicU64::new(0);
static FUSED_NANOS: AtomicU64 = AtomicU64::new(0);
static REPLAYED: AtomicU64 = AtomicU64::new(0);
static CHECKED: AtomicU64 = AtomicU64::new(0);
/// Fused steps per grid kind (noise, pollution), for the checks.
static KIND_STEPS: [AtomicU64; 2] = [AtomicU64::new(0), AtomicU64::new(0)];
static POOL: OnceLock<pool::Pool> = OnceLock::new();
/// Tests: every put-off step runs the game's way (the replay path).
#[cfg(test)]
static FORCE_REPLAY: AtomicBool = AtomicBool::new(false);

fn game() -> Option<Game> {
    *GAME.lock().unwrap_or_else(|p| p.into_inner())
}

/// A dispatcher call's arguments besides the lambda.
#[derive(Debug, Clone, Copy)]
struct Args {
    pool: usize,
    rows: i32,
    min_chunk: i32,
    a5: usize,
    a6: usize,
    a7: usize,
}

impl Args {
    /// Calls `dispatch` with these arguments and `ctx`.
    ///
    /// # Safety
    ///
    /// `dispatch` is the game's dispatcher for `ctx`'s lambda, in the
    /// state `Update` would call it in.
    unsafe fn call(&self, dispatch: usize, ctx: *const c_void) {
        // SAFETY: the caller's.
        unsafe {
            std::mem::transmute::<usize, Dispatch>(dispatch)(
                self.pool,
                ctx,
                self.rows,
                self.min_chunk,
                self.a5,
                self.a6,
                self.a7,
            );
        }
    }
}

/// A step whose Diffuse (and Wind) the hook has put off until Average.
#[derive(Debug, Clone, Copy)]
struct Pending {
    diffuse: Args,
    dctx: DiffuseCtx,
    /// Diffuse's spread and decay, read as its dispatcher would.
    a: f32,
    b: f32,
    /// The concentration's and the temporary's data when Diffuse was due.
    conc: *mut f32,
    temp: *mut f32,
    wind: Option<PendingWind>,
}

#[derive(Debug, Clone, Copy)]
struct PendingWind {
    args: Args,
    wctx: WindCtx,
    value: [f32; 2],
}

/// Where the current step stands, on `Update`'s thread.
#[derive(Debug, Clone, Copy)]
enum Mode {
    Idle,
    /// The game's dispatchers run this step.
    Game,
    Deferred(Pending),
}

thread_local! {
    static MODE: RefCell<Mode> = const { RefCell::new(Mode::Idle) };
    /// The bands' copied rows, kept between steps.
    static HALO: RefCell<Vec<f32>> = const { RefCell::new(Vec::new()) };
}

fn take_mode() -> Mode {
    MODE.with(|m| std::mem::replace(&mut *m.borrow_mut(), Mode::Idle))
}

fn set_mode(mode: Mode) {
    MODE.with(|m| *m.borrow_mut() = mode);
}

/// # Safety
///
/// `at` is readable for a `T`.
unsafe fn read<T: Copy>(at: usize) -> T {
    // SAFETY: the caller's.
    unsafe { std::ptr::read_unaligned(at as *const T) }
}

/// Exchanges the component's concentration vector with the system's
/// temporary one, as `Update` does after Diffuse and Wind.
///
/// # Safety
///
/// `comp` and `system` are the step's.
unsafe fn swap_conc_temp(comp: usize, system: usize) {
    let a = (comp + COMP_CONCENTRATION + GRID_DATA) as *mut [usize; 3];
    let b = (system + SYSTEM_TEMP + GRID_DATA) as *mut [usize; 3];
    if a != b {
        // SAFETY: two distinct vectors of the step.
        unsafe { std::ptr::swap(a, b) };
    }
}

unsafe extern "system" fn diffuse_hook(
    pool: usize,
    ctx: *const c_void,
    rows: i32,
    min_chunk: i32,
    a5: usize,
    a6: usize,
    a7: usize,
) {
    let args = Args {
        pool,
        rows,
        min_chunk,
        a5,
        a6,
        a7,
    };
    let Some(game) = game() else { return };
    if let Mode::Deferred(_) = take_mode() {
        // A step that never reached Average: not a path `Update` has.
        broken("a Diffuse came before the last step's Average");
    }
    // SAFETY: `Update`'s lambda, on its frame.
    let dctx: DiffuseCtx = unsafe { read(ctx as usize) };
    let pending = (!BROKEN.load(Ordering::Acquire)).then(|| {
        // SAFETY: the system and the component `Update` passed.
        unsafe {
            Pending {
                diffuse: args,
                dctx,
                a: read(dctx.system + SYSTEM_SPREAD_A),
                b: read(dctx.system + SYSTEM_DECAY_B),
                conc: read::<Grid>(dctx.comp + COMP_CONCENTRATION).begin,
                temp: read::<Grid>(dctx.system + SYSTEM_TEMP).begin,
                wind: None,
            }
        }
    });
    match pending.filter(|p| diffuse_covered(p).is_ok()) {
        Some(pending) => set_mode(Mode::Deferred(pending)),
        None => {
            set_mode(Mode::Game);
            // SAFETY: the game's call, as `Update` made it.
            unsafe { args.call(game.diffuse_dispatch, ctx) };
        }
    }
}

unsafe extern "system" fn wind_hook(
    pool: usize,
    ctx: *const c_void,
    rows: i32,
    min_chunk: i32,
    a5: usize,
    a6: usize,
    a7: usize,
) {
    let args = Args {
        pool,
        rows,
        min_chunk,
        a5,
        a6,
        a7,
    };
    let Some(game) = game() else { return };
    // SAFETY: `Update`'s lambda, on its frame, and the wind it points at.
    let wctx: WindCtx = unsafe { read(ctx as usize) };
    match take_mode() {
        Mode::Deferred(mut p)
            if p.wind.is_none() && p.dctx.system == wctx.system && p.dctx.comp == wctx.comp =>
        {
            // SAFETY: the copy of the wind on `Update`'s frame.
            let value = unsafe { read::<[f32; 2]>(wctx.wind as usize) };
            p.wind = Some(PendingWind { args, wctx, value });
            set_mode(Mode::Deferred(p));
        }
        mode => {
            if let Mode::Deferred(p) = mode {
                // Not the step put off: run its Diffuse now, as the game
                // would have, then this Wind.
                // SAFETY: the step's own state; one swap since Diffuse.
                unsafe { replay_diffuse(&game, &p, true) };
            }
            set_mode(Mode::Game);
            // SAFETY: the game's call, as `Update` made it.
            unsafe { args.call(game.wind_dispatch, ctx) };
        }
    }
}

unsafe extern "system" fn average_hook(
    pool: usize,
    ctx: *const c_void,
    rows: i32,
    min_chunk: i32,
    a5: usize,
    a6: usize,
    a7: usize,
) {
    let args = Args {
        pool,
        rows,
        min_chunk,
        a5,
        a6,
        a7,
    };
    let Some(game) = game() else { return };
    // SAFETY: `Update`'s lambda, on its frame.
    let actx: AverageCtx = unsafe { read(ctx as usize) };
    if let Mode::Deferred(p) = take_mode() {
        // SAFETY: the step `Update` is in, with its own pointers.
        if unsafe { fused_step(&game, &p, &actx, rows) } {
            return;
        }
        REPLAYED.fetch_add(1, Ordering::Relaxed);
        // SAFETY: as above: the swaps `Update` made since Diffuse.
        unsafe { replay(&game, &p) };
    }
    // SAFETY: the game's call, as `Update` made it.
    unsafe { args.call(game.average_dispatch, ctx) };
}

/// Runs a put-off Diffuse as the game would have: undoes the swap that
/// followed it (`swapped`), runs it, swaps again.
///
/// # Safety
///
/// `p` is the current step's, and `swapped` says whether `Update` has
/// swapped once since (it has; twice, the identity, after a Wind).
unsafe fn replay_diffuse(game: &Game, p: &Pending, swapped: bool) {
    // SAFETY: the caller's.
    unsafe {
        if swapped {
            swap_conc_temp(p.dctx.comp, p.dctx.system);
        }
        p.diffuse
            .call(game.diffuse_dispatch, (&raw const p.dctx).cast());
        swap_conc_temp(p.dctx.comp, p.dctx.system);
    }
}

/// Runs the put-off passes the game's way, before Average.
///
/// # Safety
///
/// `p` is the current step's, called where `Update` calls Average.
unsafe fn replay(game: &Game, p: &Pending) {
    // SAFETY: the caller's. After a Wind `Update` swapped twice (the
    // identity), else once.
    unsafe {
        replay_diffuse(game, p, p.wind.is_none());
        if let Some(w) = p.wind {
            let wctx = WindCtx {
                wind: &raw const w.value,
                ..w.wctx
            };
            w.args.call(game.wind_dispatch, (&raw const wctx).cast());
            swap_conc_temp(p.dctx.comp, p.dctx.system);
        }
    }
}

/// What can be known at Diffuse: the grids and Diffuse's weights.
fn diffuse_covered(p: &Pending) -> Result<(), String> {
    if fused::diffuse_weights(p.a, p.b, p.dctx.dt).is_none() {
        return Err("Diffuse's weights are out of range".into());
    }
    if fused::mxcsr() & fused::MXCSR_CONTROL != fused::MXCSR_DEFAULT {
        return Err(format!("MXCSR is {:#x}", fused::mxcsr()));
    }
    // SAFETY: the step's component and system.
    let (conc, avg, temp) = unsafe { grids(p.dctx.comp, p.dctx.system) };
    shape(&conc, &avg, &temp, p.diffuse.rows).map(|_| ())
}

/// # Safety
///
/// `comp` and `system` are the step's.
unsafe fn grids(comp: usize, system: usize) -> (Grid, Grid, Grid) {
    // SAFETY: the caller's.
    unsafe {
        (
            read(comp + COMP_CONCENTRATION),
            read(comp + COMP_AVERAGE),
            read(system + SYSTEM_TEMP),
        )
    }
}

/// The grids' common width and height, if the model covers them: equal
/// sizes, at least 3 x 3, vectors long enough, `rows` the inner rows.
fn shape(conc: &Grid, avg: &Grid, temp: &Grid, rows: i32) -> Result<(usize, usize), String> {
    let (w, h) = (conc.width, conc.height);
    if (avg.width, avg.height) != (w, h) || (temp.width, temp.height) != (w, h) {
        return Err(format!(
            "grid sizes differ ({w}x{h}, {}x{}, {}x{})",
            avg.width, avg.height, temp.width, temp.height
        ));
    }
    if w < 3 || h < 3 || rows != h - 2 {
        return Err(format!("a {w}x{h} grid over {rows} rows"));
    }
    let (w, h) = (w as usize, h as usize);
    let cells = w.checked_mul(h).ok_or("the grid's size overflows")?;
    for grid in [conc, avg, temp] {
        if grid.len() < cells {
            return Err(format!("a vector of {} floats for {w}x{h}", grid.len()));
        }
    }
    let begins = [conc.begin, avg.begin, temp.begin];
    if begins[0] == begins[1] || begins[0] == begins[2] || begins[1] == begins[2] {
        return Err("two grids share a buffer".into());
    }
    Ok((w, h))
}

/// Everything the fused step needs, read and checked at Average.
struct Plan {
    buffers: Buffers,
    params: Params,
    a: f32,
    b: f32,
    dt: f32,
    /// The wind's raw inputs, for the self-check.
    wind: Option<([f32; 2], [f32; 2], f32)>,
}

/// # Safety
///
/// `p` and `actx` are the current step's.
unsafe fn plan(p: &Pending, actx: &AverageCtx, rows: i32) -> Result<Plan, String> {
    if actx.system != p.dctx.system || actx.comp != p.dctx.comp {
        return Err("Average is for another grid than Diffuse".into());
    }
    #[cfg(test)]
    if FORCE_REPLAY.load(Ordering::SeqCst) {
        return Err("forced by the test".into());
    }
    // SAFETY: the caller's.
    let (conc, avg, temp) = unsafe { grids(actx.comp, actx.system) };
    let (w, h) = shape(&conc, &avg, &temp, rows)?;
    if p.diffuse.rows != rows || p.wind.is_some_and(|w| w.args.rows != rows) {
        return Err("the passes cover different rows".into());
    }
    // Where `Update`'s swaps have put the buffers since Diffuse.
    let expected = if p.wind.is_some() {
        (p.conc, p.temp)
    } else {
        (p.temp, p.conc)
    };
    if (conc.begin, temp.begin) != expected {
        return Err("the buffers are not where the swaps put them".into());
    }
    let (w1, w2) = fused::diffuse_weights(p.a, p.b, p.dctx.dt).ok_or("Diffuse's weights")?;
    // SAFETY: the component's fields.
    let (c, gps) = unsafe {
        (
            read::<f32>(actx.system + SYSTEM_AVERAGE_C),
            read::<[f32; 2]>(actx.comp + COMP_GRID_POINT_SIZE),
        )
    };
    let wind = match p.wind {
        None => None,
        Some(pw) => {
            Some(Wind::new(pw.value, gps, pw.wctx.dt).ok_or("the wind is a grid point or more")?)
        }
    };
    if fused::mxcsr() & fused::MXCSR_CONTROL != fused::MXCSR_DEFAULT {
        return Err(format!("MXCSR is {:#x}", fused::mxcsr()));
    }
    Ok(Plan {
        buffers: Buffers {
            conc: p.conc,
            avg: avg.begin,
            temp: p.temp,
            width: w,
            height: h,
        },
        params: Params { w1, w2, c, wind },
        a: p.a,
        b: p.b,
        dt: p.dctx.dt,
        wind: p.wind.map(|pw| (pw.value, gps, pw.wctx.dt)),
    })
}

/// The fused step in `Update`'s place; `false` (nothing written) where
/// the model does not cover it.
///
/// # Safety
///
/// `p` and `actx` are the current step's.
unsafe fn fused_step(game: &Game, p: &Pending, actx: &AverageCtx, rows: i32) -> bool {
    // SAFETY: the caller's.
    let plan = match unsafe { plan(p, actx, rows) } {
        Ok(plan) => plan,
        Err(why) => {
            note_game_step(&why);
            return false;
        }
    };
    let kind = usize::from(plan.params.wind.is_some());
    let n = KIND_STEPS[kind].fetch_add(1, Ordering::Relaxed);
    if n < FIRST_CHECKS || n.is_multiple_of(CHECK_EVERY) {
        // SAFETY: the plan's buffers, which nothing else uses now.
        match unsafe { self_check(game, &plan) } {
            Ok(windows) => {
                let checked = CHECKED.fetch_add(1, Ordering::Relaxed) + 1;
                if n == 0 {
                    log::line(&format!(
                        "{FIX}: {} {}x{}: {windows} windows of the real grid bit-identical to the game's kernels; fused from now on",
                        ["noise", "pollution"][kind],
                        plan.buffers.width,
                        plan.buffers.height
                    ));
                } else if n >= FIRST_CHECKS && checked.is_power_of_two() {
                    log::line(&format!("{FIX}: self-check {checked} passed"));
                }
            }
            Err(why) => {
                broken(&format!("self-check failed: {why}"));
                return false;
            }
        }
    }
    let start = Instant::now();
    // SAFETY: the plan's buffers: `Update` waits on this call.
    let done = unsafe { run(&plan) };
    if !done {
        note_game_step("a thread's MXCSR differs");
        return false;
    }
    let nanos = u64::try_from(start.elapsed().as_nanos()).unwrap_or(u64::MAX);
    let total = FUSED_NANOS.fetch_add(nanos, Ordering::Relaxed) + nanos;
    let count = FUSED.fetch_add(1, Ordering::Relaxed) + 1;
    if count.is_multiple_of(REPORT_EVERY) {
        log::line(&format!(
            "{FIX}: {count} grid steps fused, {:.2} ms each on average; {} run the game's way",
            total as f64 / count as f64 / 1e6,
            REPLAYED.load(Ordering::Relaxed)
        ));
    }
    true
}

/// Runs the plan's step: on the hook's threads for a large grid.
///
/// # Safety
///
/// The plan's buffers are valid and used by nothing else.
unsafe fn run(plan: &Plan) -> bool {
    let cells = plan.buffers.width * plan.buffers.height;
    let pool = (cells >= SMALL_GRID).then(|| {
        POOL.get_or_init(|| {
            let threads = std::thread::available_parallelism()
                .map_or(1, |n| n.get())
                .clamp(1, MAX_THREADS);
            pool::Pool::new(threads - 1)
        })
    });
    let participants = pool.map_or(1, pool::Pool::participants);
    let bands = (participants * BANDS_PER_THREAD).min((plan.buffers.height / MIN_BAND_ROWS).max(1));
    HALO.with(|halo| {
        let mut halo = halo.borrow_mut();
        let len = Step::halo_len(&plan.buffers, bands);
        if halo.len() < len {
            halo.resize(len, 0.0);
        }
        // SAFETY: the caller's.
        let Some(step) =
            (unsafe { Step::new(plan.buffers, plan.params, bands, true, &mut halo[..len]) })
        else {
            return false;
        };
        run_step(&step, pool)
    })
}

/// Runs `step` on `pool`'s threads (or here): every band's phase 1, then,
/// if every thread had the default MXCSR, every band's phase 2. `false`
/// (nothing written) otherwise.
pub fn run_step(step: &Step<'_>, pool: Option<&pool::Pool>) -> bool {
    use std::sync::atomic::AtomicUsize;
    let Some(pool) = pool.filter(|p| p.participants() > 1) else {
        step.run_here();
        return true;
    };
    let barrier = std::sync::Barrier::new(pool.participants());
    let (kept, ran) = (AtomicUsize::new(0), AtomicUsize::new(0));
    let bad = AtomicBool::new(false);
    let finished = pool.run(&|_| {
        if fused::mxcsr() & fused::MXCSR_CONTROL != fused::MXCSR_DEFAULT {
            bad.store(true, Ordering::SeqCst);
        }
        let phase1 = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            loop {
                let i = kept.fetch_add(1, Ordering::SeqCst);
                if i >= step.bands() {
                    break;
                }
                // SAFETY: phase 1, once per band, before the barrier.
                unsafe { step.keep(i) };
            }
        }));
        if phase1.is_err() {
            bad.store(true, Ordering::SeqCst);
        }
        barrier.wait();
        if bad.load(Ordering::SeqCst) {
            return;
        }
        loop {
            let i = ran.fetch_add(1, Ordering::SeqCst);
            if i >= step.bands() {
                break;
            }
            // SAFETY: phase 2, once per band, after every phase 1.
            unsafe { step.run(i) };
        }
    });
    if !finished && !bad.load(Ordering::SeqCst) {
        // A band panicked while writing: the grid is half done.
        broken("a band of the fused step panicked");
    }
    finished && !bad.load(Ordering::SeqCst)
}

/// Runs windows of the plan's grid through the game's own kernels and
/// through the fused step, on copies, and compares every bit. Returns how
/// many windows matched.
///
/// # Safety
///
/// The plan's buffers are valid; the game's kernels are at `game`'s
/// addresses.
unsafe fn self_check(game: &Game, plan: &Plan) -> Result<usize, String> {
    let (w, h) = (plan.buffers.width, plan.buffers.height);
    let k = h.min(CHECK_ROWS);
    let mut starts = vec![0, (h - k) / 2, h - k];
    starts.dedup();
    for &s in &starts {
        let copy = |from: *const f32| {
            // SAFETY: rows s .. s+k of a w*h buffer.
            unsafe { std::slice::from_raw_parts(from.add(s * w), k * w) }.to_vec()
        };
        let window = (
            copy(plan.buffers.conc),
            copy(plan.buffers.avg),
            copy(plan.buffers.temp),
        );
        // SAFETY: the game's kernels, on copies.
        let theirs = unsafe { reference_step(game, plan, w, window.clone()) };
        let mut ours = window;
        let buffers = Buffers {
            conc: ours.0.as_mut_ptr(),
            avg: ours.1.as_ptr(),
            temp: ours.2.as_mut_ptr(),
            width: w,
            height: k,
        };
        let mut halo = vec![0.0; Step::halo_len(&buffers, 3)];
        // SAFETY: three distinct copies of w*k floats.
        let step = unsafe { Step::new(buffers, plan.params, 3, true, &mut halo) }
            .ok_or("the window is too small")?;
        step.run_here();
        let ours = arrange(ours, plan.params.wind.is_some());
        for (name, a, b) in [
            ("concentration", &ours.0, &theirs.0),
            ("average", &ours.1, &theirs.1),
            ("temporary", &ours.2, &theirs.2),
        ] {
            if let Some(i) = (0..a.len()).find(|&i| a[i].to_bits() != b[i].to_bits()) {
                return Err(format!(
                    "{name} differs at ({}, {}) of rows {s}..{}: {:#010x}, the game {:#010x}",
                    i % w,
                    s + i / w,
                    s + k,
                    a[i].to_bits(),
                    b[i].to_bits()
                ));
            }
        }
    }
    Ok(starts.len())
}

/// The fused step's buffers as (concentration, average, temporary).
fn arrange(
    (pc, pa, pt): (Vec<f32>, Vec<f32>, Vec<f32>),
    pollution: bool,
) -> (Vec<f32>, Vec<f32>, Vec<f32>) {
    if pollution {
        (pc, pt, pa)
    } else {
        (pt, pc, pa)
    }
}

/// One step of a `w`-wide grid by the game's own kernels, with the game's
/// swaps: (concentration, average, temporary) after it.
///
/// # Safety
///
/// `game`'s kernels are the game's; the plan's weights and wind pass
/// their asserts (`plan` checked them).
unsafe fn reference_step(
    game: &Game,
    plan: &Plan,
    w: usize,
    (mut c, mut a, mut t): (Vec<f32>, Vec<f32>, Vec<f32>),
) -> (Vec<f32>, Vec<f32>, Vec<f32>) {
    let k = c.len() / w;
    let grid = |v: &mut Vec<f32>| {
        let begin = v.as_mut_ptr();
        Grid {
            x0: 0,
            y0: 0,
            width: w as i32,
            height: k as i32,
            begin,
            // SAFETY: one past the end of the vector.
            end: unsafe { begin.add(v.len()) },
            cap: unsafe { begin.add(v.len()) },
        }
    };
    let rows = (1, k as i32 - 1);
    // SAFETY: the kernels' own types; grids of w*k floats.
    unsafe {
        let diffuse = std::mem::transmute::<usize, DiffuseKernel>(game.diffuse_kernel);
        let (gc, mut gt) = (grid(&mut c), grid(&mut t));
        diffuse(rows.0, rows.1, &gc, &mut gt, plan.a, plan.b, plan.dt);
        std::mem::swap(&mut c, &mut t);
        if let Some((value, gps, dt)) = plan.wind {
            let windk = std::mem::transmute::<usize, WindKernel>(game.wind_kernel);
            let (gc, mut gt) = (grid(&mut c), grid(&mut t));
            windk(rows.0, rows.1, &gc, &mut gt, &gps, &value, dt);
            std::mem::swap(&mut c, &mut t);
        }
        // Average reads the concentration's and the average's data at
        // the component's +0x28 and +0x50.
        let mut comp = [0usize; 16];
        comp[(COMP_CONCENTRATION + GRID_DATA) / 8] = c.as_ptr() as usize;
        comp[(COMP_AVERAGE + GRID_DATA) / 8] = a.as_ptr() as usize;
        let average = std::mem::transmute::<usize, AverageKernel>(game.average_kernel);
        let mut gt = grid(&mut t);
        average(
            rows.0,
            rows.1,
            comp.as_ptr() as usize,
            &mut gt,
            plan.params.c,
        );
        std::mem::swap(&mut a, &mut t);
    }
    (c, a, t)
}

fn note_game_step(why: &str) {
    static SAID: AtomicU64 = AtomicU64::new(0);
    let n = SAID.fetch_add(1, Ordering::Relaxed);
    if n < 4 || n.is_power_of_two() {
        log::line(&format!(
            "{FIX}: this step runs the game's way: {why} (step {})",
            n + 1
        ));
    }
}

fn broken(why: &str) {
    if !BROKEN.swap(true, Ordering::AcqRel) {
        log::line(&format!(
            "{FIX}: OFF for the rest of this game: {why}; the game's own update runs from the next step"
        ));
    }
}

/// What installing came to, for `hook.log`.
pub fn outcome_line(installed: bool, reason: &str) -> String {
    if installed {
        format!("{FIX}: fused update installed ({reason})")
    } else {
        format!("{FIX}: the game's own update, {reason}")
    }
}

/// Installs the fused update unless [`ENV`] turns it off. Returns the
/// line for `hook.log`.
pub fn install(resolved: &ResolvedProfile) -> String {
    match wanted(std::env::var(ENV).ok().as_deref()) {
        Ok(true) => match install_from_profile(resolved) {
            Ok(line) => outcome_line(true, &line),
            Err(why) => outcome_line(false, &why),
        },
        Ok(false) => outcome_line(false, &format!("{ENV}=0")),
        Err(why) => outcome_line(false, &format!("{ENV}: {why}")),
    }
}

/// Checks each stretch of [`CODE`] at `update` against its hash.
pub fn check_code(
    update: usize,
    bytes: &dyn Fn(usize, usize) -> Option<Vec<u8>>,
) -> Result<(), String> {
    for code in CODE {
        let at = update.wrapping_add_signed(code.offset as isize);
        let found = bytes(at, code.len).ok_or_else(|| format!("{} is unreadable", code.what))?;
        if fnv1a(&found) != code.fnv1a {
            return Err(format!(
                "{} at {at:#x} is not the code the fused step models",
                code.what
            ));
        }
    }
    Ok(())
}

fn install_from_profile(resolved: &ResolvedProfile) -> Result<String, String> {
    if !fused::avx() {
        return Err("this CPU has no AVX".into());
    }
    let at = resolved
        .get(UPDATE)
        .ok_or_else(|| format!("the profile has no {UPDATE:?}"))?
        .address;
    let update = usize::try_from(at).map_err(|_| "an address past usize".to_owned())?;
    check_code(update, &|at, len| {
        // SAFETY: read only after the region answered readable.
        crate::image::readable(at, len)
            .then(|| unsafe { std::slice::from_raw_parts(at as *const u8, len) }.to_vec())
    })?;
    let at = |offset: i64| update.wrapping_add_signed(offset as isize);
    // SAFETY: the profile's `Update`, whose code was just checked; nothing
    // runs it before the world exists.
    let redirects = unsafe {
        install_at(
            Game::from_update(update),
            [at(DIFFUSE_CALL), at(WIND_CALL), at(AVERAGE_CALL)],
        )
    }?;
    let _kept = std::mem::ManuallyDrop::new(redirects);
    Ok(format!(
        "at {update:#x}: one pass, AVX, up to {MAX_THREADS} threads, bit-identical to the game's three passes; {ENV}=0 turns it off"
    ))
}

/// Redirects `Update`'s three dispatcher calls (`sites`, in Diffuse,
/// Wind, Average order) to the hook, with the game's code at `game`.
///
/// # Safety
///
/// `sites` are `Update`'s calls of `game`'s dispatchers, which no thread
/// runs during this call.
pub(crate) unsafe fn install_at(
    game: Game,
    sites: [usize; 3],
) -> Result<Vec<tpf3mp_hookcore::detour::CallRedirect>, String> {
    *GAME.lock().unwrap_or_else(|p| p.into_inner()) = Some(game);
    BROKEN.store(false, Ordering::Release);
    let hooks: [(usize, usize, *const u8); 3] = [
        (sites[0], game.diffuse_dispatch, diffuse_hook as *const u8),
        (sites[1], game.wind_dispatch, wind_hook as *const u8),
        (sites[2], game.average_dispatch, average_hook as *const u8),
    ];
    let mut done = Vec::new();
    for (site, expected, to) in hooks {
        // SAFETY: the caller's; `to` has the dispatcher's ABI.
        match unsafe {
            tpf3mp_hookcore::detour::CallRedirect::install(site as *mut u8, expected, to)
        } {
            Ok(redirect) => done.push(redirect),
            Err(error) => {
                // Dropping the ones made restores their calls.
                drop(done);
                return Err(format!("the call at {site:#x}: {error}"));
            }
        }
    }
    Ok(done)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_switch_is_on_unless_it_says_off() {
        for on in [None, Some(""), Some("1"), Some(" ON "), Some("yes")] {
            assert_eq!(wanted(on), Ok(true), "{on:?}");
        }
        for off in ["0", "off", " FALSE ", "no"] {
            assert_eq!(wanted(Some(off)), Ok(false), "{off}");
        }
        assert!(wanted(Some("2")).is_err());
    }

    #[test]
    fn the_hash_is_fnv1a() {
        assert_eq!(fnv1a(b""), 0xcbf2_9ce4_8422_2325);
        assert_eq!(fnv1a(b"a"), 0xaf63_dc4c_8601_ec8c);
    }

    #[test]
    fn code_that_differs_is_refused() {
        assert!(
            check_code(0x1000, &|_, _| None)
                .unwrap_err()
                .contains("unreadable")
        );
        let err = check_code(0x1000, &|_, len| Some(vec![0xCC; len])).unwrap_err();
        assert!(err.contains("Update"), "{err}");
    }

    #[test]
    fn nothing_installs_without_the_target() {
        let empty = ResolvedProfile {
            name: "empty".into(),
            targets: Vec::new(),
            absent_optional: Vec::new(),
        };
        let line = install_from_profile(&empty).unwrap_err();
        assert!(
            line.contains("the profile has no") || line.contains("AVX"),
            "{line}"
        );
    }

    #[test]
    fn shapes_the_model_does_not_cover_are_refused() {
        let mut bufs = [vec![0.0f32; 20], vec![0.0f32; 20], vec![0.0f32; 20]];
        let grid = |v: &mut Vec<f32>, w: i32, h: i32| Grid {
            x0: 0,
            y0: 0,
            width: w,
            height: h,
            begin: v.as_mut_ptr(),
            // SAFETY: one past the end.
            end: unsafe { v.as_mut_ptr().add(v.len()) },
            cap: unsafe { v.as_mut_ptr().add(v.len()) },
        };
        let [a, b, c] = &mut bufs;
        let (ga, gb, gc) = (grid(a, 4, 5), grid(b, 4, 5), grid(c, 4, 5));
        assert_eq!(shape(&ga, &gb, &gc, 3), Ok((4, 5)));
        assert!(shape(&ga, &gb, &gc, 4).is_err());
        assert!(shape(&ga, &gb, &grid(c, 5, 4), 3).is_err());
        assert!(shape(&ga, &ga, &gc, 3).is_err());
        assert!(shape(&grid(a, 5, 5), &grid(b, 5, 5), &grid(c, 5, 5), 3).is_err());
        assert!(shape(&grid(a, 2, 10), &grid(b, 2, 10), &grid(c, 2, 10), 8).is_err());
    }
}
