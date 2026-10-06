//! What the hook's work costs in the running game (docs/HOOKS.md, "What the
//! hook costs: the `perf:` lines").
//!
//! Each piece of per-update work the hook does on the game's threads (the
//! order fixes, the game scripts' reseed, the paused-tick redirect, the
//! lanes and lane dumps, the step gate's own work) is timed where it runs:
//! one clock read before and one after, the nanoseconds and the call added
//! to two atomics. The game's own `GameSim::Step`, the call the step detour
//! makes of the original, is timed the same way, with the updates it ran,
//! so the hook's cost can be set against the game's.
//!
//! Every [`WINDOW`] of wall time the step detour takes the counters and
//! writes two lines to hook.log ([`lines`]). [`ENV`] set to `0` turns the
//! timing off; it is on otherwise, because a timed call costs two reads of
//! the clock and two atomic adds, about 56 ns (measured on the development
//! PC, see the `a_timed_call_costs_two_clock_reads` benchmark), against
//! the microseconds of the work it times.
//!
//! Pure but for the clock and the statics: the lines are made by
//! [`lines`] from a [`Window`], which the tests build by hand.

use std::{
    sync::{
        Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

/// The game's environment: `0` (or `off`, `false`, `no`) turns the timing
/// and its lines off; anything else, or unset, leaves them on.
pub const ENV: &str = "TPF3MP_HOOK_PERF";
/// The wall time between two `perf:` line pairs.
pub const WINDOW: Duration = Duration::from_secs(10);

/// A piece of the hook's work, timed on its own.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Piece {
    /// `road-entry-order`: the sort after `EdgeUseManager::Add` or
    /// `AddRange`, one call per append (the engine's append not counted).
    RoadEntry,
    /// `platform-order`, the visit site: one call per iteration of the
    /// chooser's loop; the copy and sort are in its first.
    PlatformVisit,
    /// `platform-order`, the candidates put in order before the cost sort.
    PlatformCandidates,
    /// `land-vehicle-order`: the sort before the reservation shuffle.
    LandVehicle,
    /// `vehicles-at-stop-order`: the sort before the boarding loop.
    VehiclesAtStop,
    /// The person-order fixes (`persons.rs`): one call per batch sorted.
    PersonOrder,
    /// The game scripts' per-call reseed (update, postUpdate, handleEvent),
    /// on the threads that run the scripts.
    Reseed,
    /// `paused-tick`: the redirect's decision (the game's own advance, when
    /// it is passed on, not counted).
    PausedTick,
    /// The lanes the mod hands the hook at a checkpoint (`lanes(t)`).
    Lanes,
    /// A lane dump's entries (`dump()`, `dumped(lane, entry)`).
    LaneDump,
    /// The step gate: the step detour's own work around the game's step
    /// (the room's session, the driver, the log).
    Gate,
}

impl Piece {
    pub const ALL: [Piece; 11] = [
        Piece::RoadEntry,
        Piece::PlatformVisit,
        Piece::PlatformCandidates,
        Piece::LandVehicle,
        Piece::VehiclesAtStop,
        Piece::PersonOrder,
        Piece::Reseed,
        Piece::PausedTick,
        Piece::Lanes,
        Piece::LaneDump,
        Piece::Gate,
    ];

    /// Its name in the `perf:` line.
    pub const fn name(self) -> &'static str {
        match self {
            Piece::RoadEntry => "road-entry",
            Piece::PlatformVisit => "platform-visit",
            Piece::PlatformCandidates => "platform-candidates",
            Piece::LandVehicle => "land-vehicle",
            Piece::VehiclesAtStop => "vehicles-at-stop",
            Piece::PersonOrder => "person-order",
            Piece::Reseed => "reseed",
            Piece::PausedTick => "paused-tick",
            Piece::Lanes => "lanes",
            Piece::LaneDump => "lane-dump",
            Piece::Gate => "gate",
        }
    }
}

/// Calls and nanoseconds, accumulated from any thread.
#[derive(Debug)]
pub struct Counter {
    calls: AtomicU64,
    nanos: AtomicU64,
}

impl Counter {
    pub const fn new() -> Self {
        Self {
            calls: AtomicU64::new(0),
            nanos: AtomicU64::new(0),
        }
    }

    /// One call that took `nanos`.
    pub fn add(&self, nanos: u64) {
        self.add_calls(1, nanos);
    }

    /// `calls` calls that took `nanos` together.
    pub fn add_calls(&self, calls: u64, nanos: u64) {
        self.calls.fetch_add(calls, Ordering::Relaxed);
        self.nanos.fetch_add(nanos, Ordering::Relaxed);
    }

    /// What was accumulated since the last take, and zero again.
    pub fn take(&self) -> Sample {
        Sample {
            calls: self.calls.swap(0, Ordering::Relaxed),
            nanos: self.nanos.swap(0, Ordering::Relaxed),
        }
    }
}

impl Default for Counter {
    fn default() -> Self {
        Self::new()
    }
}

/// A counter's calls and nanoseconds over one window.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Sample {
    pub calls: u64,
    pub nanos: u64,
}

impl Sample {
    pub fn millis(self) -> f64 {
        self.nanos as f64 / 1e6
    }

    /// The mean call in microseconds; 0 without calls.
    pub fn mean_micros(self) -> f64 {
        if self.calls == 0 {
            0.0
        } else {
            self.nanos as f64 / self.calls as f64 / 1e3
        }
    }
}

static ENABLED: AtomicBool = AtomicBool::new(true);
static PIECES: [Counter; Piece::ALL.len()] = [const { Counter::new() }; Piece::ALL.len()];
/// The game's `GameSim::Step`, one call per batch.
static GAME: Counter = Counter::new();
/// The simulation updates run (`ecs::Engine::Update` calls).
static UPDATES: AtomicU64 = AtomicU64::new(0);
/// When the last window began; `None` before the first step.
static WINDOW_START: Mutex<Option<Instant>> = Mutex::new(None);

/// Whether the timing is wanted, from [`ENV`]'s value: on unless it says
/// `0`, `off`, `false` or `no`.
pub fn wanted(value: Option<&str>) -> bool {
    crate::ticks::wanted(value)
}

/// Reads [`ENV`] and switches the timing on or off; `true` when on.
pub fn configure_from_env() -> bool {
    let on = wanted(std::env::var(ENV).ok().as_deref());
    ENABLED.store(on, Ordering::Release);
    on
}

pub fn enabled() -> bool {
    ENABLED.load(Ordering::Relaxed)
}

/// Times its piece from [`time`] until it is dropped.
#[must_use = "the time is taken when the timer is dropped"]
pub struct Timer {
    piece: Piece,
    start: Option<Instant>,
}

impl Drop for Timer {
    fn drop(&mut self) {
        if let Some(start) = self.start {
            PIECES[self.piece as usize].add(nanos_since(start));
        }
    }
}

/// Starts timing one call of `piece`; nothing when the timing is off.
pub fn time(piece: Piece) -> Timer {
    Timer {
        piece,
        start: enabled().then(Instant::now),
    }
}

/// The clock, when the timing is on.
pub fn start() -> Option<Instant> {
    enabled().then(Instant::now)
}

pub fn nanos_since(start: Instant) -> u64 {
    u64::try_from(start.elapsed().as_nanos()).unwrap_or(u64::MAX)
}

/// Adds one call of `piece` that took `nanos`.
pub fn add(piece: Piece, nanos: u64) {
    PIECES[piece as usize].add(nanos);
}

/// One call of the game's step that took `nanos`.
pub fn game_step(nanos: u64) {
    GAME.add(nanos);
}

/// One simulation update began.
pub fn update() {
    if enabled() {
        UPDATES.fetch_add(1, Ordering::Relaxed);
    }
}

/// One window's counters.
#[derive(Debug, Clone, PartialEq)]
pub struct Window {
    pub seconds: f64,
    pub game: Sample,
    pub updates: u64,
    pub pieces: [Sample; Piece::ALL.len()],
    /// The road fix's refusals in the window, by reason.
    pub road_refusals: Vec<(&'static str, u64)>,
    /// Readability checks through the region cache
    /// ([`crate::image::Readable`]): answered from it, and asked of the
    /// system.
    pub cache_hits: u64,
    pub cache_misses: u64,
    /// Guarded reads ([`crate::image::guarded`]), and those a fault
    /// refused.
    pub guarded_reads: u64,
    pub guarded_faults: u64,
}

/// The window's two lines: the game's step and the hook's total against
/// it, then every piece and the road fix's refusals.
pub fn lines(window: &Window) -> [String; 2] {
    let seconds = window.seconds.max(1e-9);
    let game = window.game;
    let hook_nanos: u64 = window.pieces.iter().map(|p| p.nanos).sum();
    let hook = Sample {
        calls: 0,
        nanos: hook_nanos,
    };
    let per_update = if window.updates == 0 {
        "no updates".to_owned()
    } else {
        format!("{:.3} ms/update", game.millis() / window.updates as f64)
    };
    let share = if game.nanos == 0 {
        "no game step to compare".to_owned()
    } else {
        format!(
            "{:.2}% of the game's step",
            hook_nanos as f64 * 100.0 / game.nanos as f64
        )
    };
    let first = format!(
        "perf: {seconds:.1}s: game step {:.1} ms ({:.1} ms/s) in {} batches, {} updates ({per_update}); hook {:.1} ms ({:.2} ms/s, {share})",
        game.millis(),
        game.millis() / seconds,
        game.calls,
        window.updates,
        hook.millis(),
        hook.millis() / seconds,
    );
    let first = format!(
        "{first}; readable cache {} hits, {} misses; guarded reads {}, {} faults",
        window.cache_hits, window.cache_misses, window.guarded_reads, window.guarded_faults
    );
    let mut second = String::from("perf: ");
    for (i, (piece, sample)) in Piece::ALL.iter().zip(window.pieces.iter()).enumerate() {
        if i > 0 {
            second.push_str(", ");
        }
        second.push_str(&format!(
            "{} {}/{:.2}ms/{:.2}us",
            piece.name(),
            sample.calls,
            sample.millis(),
            sample.mean_micros()
        ));
    }
    let refused: u64 = window.road_refusals.iter().map(|(_, n)| n).sum();
    second.push_str(&format!("; road-entry refused {refused}"));
    if !window.road_refusals.is_empty() {
        let reasons: Vec<String> = window
            .road_refusals
            .iter()
            .map(|(why, n)| format!("{n} {why}"))
            .collect();
        second.push_str(&format!(" ({})", reasons.join("; ")));
    }
    [first, second]
}

/// Takes every counter, zero again, into a window of `seconds`.
fn take(seconds: f64) -> Window {
    let (cache_hits, cache_misses) = crate::image::take_counts();
    let (guarded_reads, guarded_faults) = crate::image::take_guarded_counts();
    let mut pieces = [Sample::default(); Piece::ALL.len()];
    for (sample, counter) in pieces.iter_mut().zip(PIECES.iter()) {
        *sample = counter.take();
    }
    Window {
        seconds,
        game: GAME.take(),
        updates: UPDATES.swap(0, Ordering::Relaxed),
        pieces,
        road_refusals: crate::order::road::take_refusals(),
        cache_hits,
        cache_misses,
        guarded_reads,
        guarded_faults,
    }
}

/// From the step detour after each call: once a window has passed, its
/// lines for the log. The first call starts the first window.
pub fn tick(now: Instant) -> Option<[String; 2]> {
    if !enabled() {
        return None;
    }
    let mut start = WINDOW_START.try_lock().ok()?;
    let Some(began) = *start else {
        *start = Some(now);
        // Whatever ran before the first step belongs to no window.
        let _ = take(0.0);
        return None;
    };
    let elapsed = now.saturating_duration_since(began);
    if elapsed < WINDOW {
        return None;
    }
    *start = Some(now);
    drop(start);
    Some(lines(&take(elapsed.as_secs_f64())))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_counter_adds_calls_and_time_and_starts_again_when_taken() {
        let counter = Counter::new();
        counter.add(1_500);
        counter.add(500);
        counter.add_calls(3, 3_000);
        let sample = counter.take();
        assert_eq!(
            sample,
            Sample {
                calls: 5,
                nanos: 5_000
            }
        );
        assert!((sample.mean_micros() - 1.0).abs() < 1e-12);
        assert!((sample.millis() - 0.005).abs() < 1e-12);
        assert_eq!(counter.take(), Sample::default(), "zero after a take");
        assert_eq!(Sample::default().mean_micros(), 0.0);
    }

    #[test]
    fn a_counter_takes_every_threads_calls() {
        let counter = std::sync::Arc::new(Counter::new());
        let threads: Vec<_> = (0..4)
            .map(|_| {
                let counter = counter.clone();
                std::thread::spawn(move || {
                    for _ in 0..1000 {
                        counter.add(2);
                    }
                })
            })
            .collect();
        for thread in threads {
            thread.join().unwrap();
        }
        assert_eq!(
            counter.take(),
            Sample {
                calls: 4000,
                nanos: 8000
            }
        );
    }

    fn window() -> Window {
        let mut pieces = [Sample::default(); Piece::ALL.len()];
        pieces[Piece::RoadEntry as usize] = Sample {
            calls: 19_000,
            nanos: 9_500_000,
        };
        pieces[Piece::Reseed as usize] = Sample {
            calls: 6_000,
            nanos: 30_000_000,
        };
        pieces[Piece::Gate as usize] = Sample {
            calls: 600,
            nanos: 3_000_000,
        };
        Window {
            seconds: 10.0,
            game: Sample {
                calls: 600,
                nanos: 2_000_000_000,
            },
            updates: 600,
            pieces,
            road_refusals: vec![("the edge's entity has no slot", 12)],
            cache_hits: 0,
            cache_misses: 0,
            guarded_reads: 90_000,
            guarded_faults: 3,
        }
    }

    #[test]
    fn the_first_line_sets_the_hook_against_the_games_step() {
        let [first, _] = lines(&window());
        assert_eq!(
            first,
            "perf: 10.0s: game step 2000.0 ms (200.0 ms/s) in 600 batches, 600 updates \
             (3.333 ms/update); hook 42.5 ms (4.25 ms/s, 2.12% of the game's step); readable cache 0 hits, 0 misses; guarded reads 90000, 3 faults"
        );
    }

    #[test]
    fn the_second_line_gives_every_piece_and_the_road_refusals_by_reason() {
        let [_, second] = lines(&window());
        assert!(
            second.starts_with(
                "perf: road-entry 19000/9.50ms/0.50us, platform-visit 0/0.00ms/0.00us, "
            ),
            "{second}"
        );
        assert!(
            second.contains(", reseed 6000/30.00ms/5.00us, "),
            "{second}"
        );
        assert!(second.contains(", gate 600/3.00ms/5.00us; "), "{second}");
        assert!(
            second.ends_with("; road-entry refused 12 (12 the edge's entity has no slot)"),
            "{second}"
        );
        for piece in Piece::ALL {
            assert!(
                second.contains(&format!(" {} ", piece.name()))
                    || second.starts_with(&format!("perf: {} ", piece.name())),
                "{piece:?}"
            );
        }
    }

    #[test]
    fn a_window_without_the_game_says_so_rather_than_divide_by_zero() {
        let mut empty = window();
        empty.game = Sample::default();
        empty.updates = 0;
        empty.road_refusals.clear();
        let [first, second] = lines(&empty);
        assert!(
            first.contains("in 0 batches, 0 updates (no updates)"),
            "{first}"
        );
        assert!(first.contains("no game step to compare);"), "{first}");
        assert!(second.ends_with("; road-entry refused 0"), "{second}");
    }

    #[test]
    fn the_switch_reads_the_environment() {
        assert!(wanted(None));
        assert!(wanted(Some("1")));
        for off in ["0", "off", "false", "no"] {
            assert!(!wanted(Some(off)), "{off}");
        }
    }

    /// The timing's own cost: two clock reads and two atomic adds a timed
    /// call; one atomic load when it is off. Run with
    /// `cargo test --release -p tpf3mp-hook perf::tests::a_timed_call -- --ignored --nocapture`.
    #[test]
    #[ignore = "a benchmark: prints the cost of a timed call"]
    fn a_timed_call_costs_two_clock_reads() {
        const N: u32 = 2_000_000;
        let bench = |on: bool| {
            ENABLED.store(on, Ordering::Release);
            let begin = Instant::now();
            for _ in 0..N {
                let timer = time(Piece::LaneDump);
                std::hint::black_box(&timer);
                drop(timer);
            }
            let each = begin.elapsed().as_nanos() as f64 / f64::from(N);
            PIECES[Piece::LaneDump as usize].take();
            each
        };
        let off = bench(false);
        let on = bench(true);
        ENABLED.store(true, Ordering::Release);
        println!("a timed call: {on:.1} ns with the timing on, {off:.1} ns off");
    }
}
