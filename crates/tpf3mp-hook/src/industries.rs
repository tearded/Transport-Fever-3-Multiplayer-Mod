//! Read-only diagnostics for build 40408's automatic industry demand spawn.
//!
//! This probe is deliberately opt-in. It records the native callback's
//! GameTime/FNV seed, target-capacity key/result, candidate loop order and
//! returned entity IDs; it never changes a game value. The profile sites are
//! specific to the compiled 40408 native bundle (docs/HOOKS.md,
//! "Automatic industry spawn probe").

use tpf3mp_hookcore::profile::{self, Profile, ResolvedProfile};

pub const TRACE_ENV: &str = "TPF3MP_HOOK_TRACE_INDUSTRIES";
const PROBE_PROFILE: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../profiles/tf3_build40408_steam_windows/industry_probe.toml"
));

/// Diagnostics are off unless this exact opt-in is present. The game's
/// ordinary hook fixes use a default-on toggle; this probe must not.
fn requested(value: Option<&str>) -> bool {
    value == Some("1")
}

/// Resolves the probe's exact-build signatures before the ordinary hooks
/// patch the image. These sites do not belong to the release profile: a
/// different supported game build remains fully covered without them.
pub fn resolve_probe(
    active: &Profile,
    code: &[u8],
    region_base: u64,
) -> Result<Option<ResolvedProfile>, String> {
    if !requested(std::env::var(TRACE_ENV).ok().as_deref()) {
        return Ok(None);
    }
    let probe = Profile::from_toml(PROBE_PROFILE)
        .map_err(|error| format!("the compiled 40408 probe profile is invalid: {error}"))?;
    if active.build != probe.build || active.region != probe.region {
        return Err("the probe is only for the exact compiled 40408 build".into());
    }
    profile::resolve(&probe, code, region_base)
        .map(Some)
        .map_err(|error| format!("the compiled 40408 probe sites did not resolve: {error}"))
}

/// Installs the 40408-only industry probe when explicitly requested.
pub fn install(resolved: &ResolvedProfile) -> Vec<String> {
    #[cfg(all(windows, target_arch = "x86_64"))]
    {
        live::install(resolved)
    }
    #[cfg(not(all(windows, target_arch = "x86_64")))]
    {
        let _ = resolved;
        if requested(std::env::var(TRACE_ENV).ok().as_deref()) {
            vec![format!(
                "industry-spawn probe: unavailable on this platform ({TRACE_ENV} is set)"
            )]
        } else {
            Vec::new()
        }
    }
}

const MAX_TEXT: usize = 48;
const MAX_CANDIDATES: usize = 32;
const MAX_CALLBACKS: u64 = 512;
const QUEUE_CAPACITY: usize = 64;
const CAPACITY_PROLOGUE_LEN: usize = 16;

/// Runs the capacity detour installation only when the resolved address still
/// holds the compiled native function's entry bytes. A profile in the user's
/// data directory can shadow the built-in profile, so the resolver's
/// profile-supplied signature is not sufficient for this detour's ABI.
fn install_capacity_if_prologue_matches<T>(
    observed: Option<[u8; CAPACITY_PROLOGUE_LEN]>,
    install: impl FnOnce() -> Result<T, String>,
) -> Result<T, String> {
    let expected = crate::build_data::native::industries::CAPACITY_EXPECTED;
    let Some(observed) = observed else {
        return Err("GetFreeTargetCapacity: compiled prologue is unreadable".to_owned());
    };
    if observed != expected {
        return Err(
            "GetFreeTargetCapacity: target differs from compiled native prologue".to_owned(),
        );
    }
    install()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Text {
    bytes: [u8; MAX_TEXT],
    len: u8,
}

impl Default for Text {
    fn default() -> Self {
        Self {
            bytes: [0; MAX_TEXT],
            len: 0,
        }
    }
}

impl Text {
    fn escaped(&self) -> String {
        format!(
            "{:?}",
            String::from_utf8_lossy(&self.bytes[..usize::from(self.len)])
        )
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Capacity {
    first: Option<Text>,
    second: Option<Text>,
    result: i32,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Candidate {
    ordinal: u32,
    resource: i32,
    count: i32,
    closed_gate: Option<bool>,
    entity: Option<i32>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Callback {
    game_time: u32,
    seed: u64,
    capacity: Option<Capacity>,
    candidates: [Candidate; MAX_CANDIDATES],
    candidate_count: usize,
    truncated: bool,
}

impl Callback {
    const fn new(game_time: u32, seed: u64) -> Self {
        Self {
            game_time,
            seed,
            capacity: None,
            candidates: [Candidate {
                ordinal: 0,
                resource: 0,
                count: 0,
                closed_gate: None,
                entity: None,
            }; MAX_CANDIDATES],
            candidate_count: 0,
            truncated: false,
        }
    }

    fn add_candidate(&mut self, candidate: Candidate) {
        if self.candidate_count == self.candidates.len() {
            self.truncated = true;
            return;
        }
        self.candidates[self.candidate_count] = candidate;
        self.candidate_count += 1;
    }

    fn candidate_mut(&mut self, ordinal: u32) -> Option<&mut Candidate> {
        self.candidates[..self.candidate_count]
            .iter_mut()
            .rev()
            .find(|candidate| candidate.ordinal == ordinal)
    }

    fn line(&self) -> String {
        let capacity = match self.capacity {
            Some(capacity) => format!(
                "capacity=({},{})=>{}",
                capacity
                    .first
                    .map_or_else(|| "?".to_owned(), |text| text.escaped()),
                capacity
                    .second
                    .map_or_else(|| "?".to_owned(), |text| text.escaped()),
                capacity.result
            ),
            None => "capacity=missing".to_owned(),
        };
        let candidates = self.candidates[..self.candidate_count]
            .iter()
            .map(|candidate| {
                format!(
                    "{}:{}/{}:closed={}:entity={}",
                    candidate.ordinal,
                    candidate.resource,
                    candidate.count,
                    candidate
                        .closed_gate
                        .map_or_else(|| "?".to_owned(), |value| value.to_string()),
                    candidate
                        .entity
                        .map_or_else(|| "?".to_owned(), |value| value.to_string())
                )
            })
            .collect::<Vec<_>>()
            .join(",");
        format!(
            "industry-spawn probe: time={} seed={:#018x} {capacity} order=[{candidates}] truncated={}",
            self.game_time, self.seed, self.truncated
        )
    }
}

/// Fixed-size handoff between native callbacks and the formatting/logger
/// worker. Producers never allocate, format strings, or touch hook.log.
struct TraceQueue {
    records: [Option<Callback>; QUEUE_CAPACITY],
    head: usize,
    len: usize,
    dropped: u64,
    failed: bool,
    limit_reached: bool,
}

impl TraceQueue {
    const fn new() -> Self {
        Self {
            records: [None; QUEUE_CAPACITY],
            head: 0,
            len: 0,
            dropped: 0,
            failed: false,
            limit_reached: false,
        }
    }

    fn push(&mut self, callback: Callback) -> bool {
        if self.len == QUEUE_CAPACITY {
            self.dropped = self.dropped.saturating_add(1);
            return false;
        }
        let tail = (self.head + self.len) % QUEUE_CAPACITY;
        self.records[tail] = Some(callback);
        self.len += 1;
        true
    }

    fn drain(&mut self, out: &mut [Option<Callback>; QUEUE_CAPACITY]) -> (usize, u64, bool, bool) {
        let count = self.len;
        for slot in out.iter_mut().take(count) {
            *slot = self.records[self.head].take();
            self.head = (self.head + 1) % QUEUE_CAPACITY;
        }
        self.len = 0;
        let dropped = std::mem::take(&mut self.dropped);
        let failed = std::mem::take(&mut self.failed);
        let limit_reached = std::mem::take(&mut self.limit_reached);
        (count, dropped, failed, limit_reached)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn industry_trace_requires_explicit_one() {
        assert!(!requested(None));
        assert!(!requested(Some("0")));
        assert!(!requested(Some("true")));
        assert!(requested(Some("1")));
    }

    #[test]
    fn probe_profile_is_separate_and_pinned_to_the_release_build() {
        let probe = Profile::from_toml(PROBE_PROFILE).unwrap();
        let release = Profile::from_toml(include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../profiles/tf3_build40408_steam_windows/hooks.toml"
        )))
        .unwrap();
        assert_eq!(probe.build, release.build);
        assert_eq!(probe.region, release.region);
        assert_eq!(probe.targets.len(), 6);
        assert!(probe.targets.iter().all(|target| target.required));
        assert!(probe.targets.iter().all(|target| {
            release
                .targets
                .iter()
                .all(|regular| regular.name != target.name)
        }));
    }

    #[test]
    fn callback_format_preserves_input_order_and_emitted_entity() {
        let mut callback = Callback::new(30_450, 0x0123_4567_89ab_cdef);
        callback.capacity = Some(Capacity {
            first: Some(text(b"coal")),
            second: Some(text(b"industry")),
            result: 17,
        });
        callback.add_candidate(Candidate {
            ordinal: 0,
            resource: 3,
            count: 1,
            closed_gate: Some(false),
            entity: Some(150_714),
        });
        callback.add_candidate(Candidate {
            ordinal: 1,
            resource: 8,
            count: 2,
            closed_gate: Some(true),
            entity: Some(-1),
        });

        assert_eq!(
            callback.line(),
            "industry-spawn probe: time=30450 seed=0x0123456789abcdef capacity=(\"coal\",\"industry\")=>17 order=[0:3/1:closed=false:entity=150714,1:8/2:closed=true:entity=-1] truncated=false"
        );
    }

    #[test]
    fn candidate_storage_is_bounded_and_marks_truncation() {
        let mut callback = Callback::new(12, 34);
        for ordinal in 0..(MAX_CANDIDATES as u32 + 1) {
            callback.add_candidate(Candidate {
                ordinal,
                ..Candidate::default()
            });
        }
        assert_eq!(callback.candidate_count, MAX_CANDIDATES);
        assert!(callback.truncated);
        assert!(callback.line().ends_with("truncated=true"));
    }

    #[test]
    fn trace_queue_preserves_callback_order_and_counts_overflow() {
        let mut queue = TraceQueue::new();
        for time in 0..QUEUE_CAPACITY {
            assert!(queue.push(Callback::new(time as u32, time as u64)));
        }
        assert!(!queue.push(Callback::new(999, 999)));

        let mut out = [None; QUEUE_CAPACITY];
        let (count, dropped, failed, limit) = queue.drain(&mut out);
        assert_eq!(
            (count, dropped, failed, limit),
            (QUEUE_CAPACITY, 1, false, false)
        );
        assert_eq!(out[0].map(|record| record.game_time), Some(0));
        assert_eq!(
            out[QUEUE_CAPACITY - 1].map(|record| record.game_time),
            Some((QUEUE_CAPACITY - 1) as u32)
        );
    }

    #[test]
    fn a_shadowed_capacity_target_is_rejected_before_installing_its_detour() {
        let mut observed = crate::build_data::native::industries::CAPACITY_EXPECTED;
        observed[0] ^= 0xff;
        let mut install_calls = 0;

        let result = install_capacity_if_prologue_matches(Some(observed), || {
            install_calls += 1;
            Ok(())
        });

        assert!(result.is_err());
        assert_eq!(install_calls, 0, "mismatch must leave the target untouched");
    }

    #[test]
    fn an_unreadable_capacity_target_is_rejected_before_installing_its_detour() {
        let mut install_calls = 0;

        let result = install_capacity_if_prologue_matches(None, || {
            install_calls += 1;
            Ok(())
        });

        assert!(result.is_err());
        assert_eq!(install_calls, 0, "unreadable target must remain untouched");
    }

    #[test]
    fn the_compiled_capacity_prologue_allows_exactly_the_guarded_install() {
        let mut install_calls = 0;

        let result = install_capacity_if_prologue_matches(
            Some(crate::build_data::native::industries::CAPACITY_EXPECTED),
            || {
                install_calls += 1;
                Ok(17)
            },
        );

        assert_eq!(result, Ok(17));
        assert_eq!(install_calls, 1);
    }

    fn text(bytes: &[u8]) -> Text {
        let mut text = Text::default();
        text.bytes[..bytes.len()].copy_from_slice(bytes);
        text.len = bytes.len() as u8;
        text
    }
}

#[cfg(all(windows, target_arch = "x86_64"))]
#[allow(unsafe_code)]
mod live {
    use super::*;
    use std::cell::RefCell;
    use std::sync::{
        Condvar, Mutex,
        atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
    };

    use tpf3mp_hookcore::detour::{InlineDetour, SavedRegs, Splice};
    use tpf3mp_hookcore::profile::ResolvedProfile;

    use crate::{image::Readable, log};

    use crate::build_data::native::industries as profile;

    static ENABLED: AtomicBool = AtomicBool::new(false);
    static BROKEN: AtomicBool = AtomicBool::new(false);
    static CALLBACKS: AtomicU64 = AtomicU64::new(0);
    static CAPACITY_ORIGINAL: AtomicUsize = AtomicUsize::new(0);
    static SPLICES: Mutex<Vec<Splice>> = Mutex::new(Vec::new());
    static TRACE_QUEUE: Mutex<TraceQueue> = Mutex::new(TraceQueue::new());
    static QUEUE_READY: Condvar = Condvar::new();

    thread_local! {
        static CALLBACK: RefCell<Option<Callback>> = const { RefCell::new(None) };
    }

    type CapacityFn = unsafe extern "system" fn(usize, usize) -> i32;

    pub fn install(resolved: &ResolvedProfile) -> Vec<String> {
        if !requested(std::env::var(TRACE_ENV).ok().as_deref()) {
            return Vec::new();
        }

        let sites = [
            profile::SEED_SITE,
            profile::CANDIDATE_SITE,
            profile::CANDIDATE_RESULT_SITE,
            profile::ENTITY_SITE,
            profile::CALLBACK_END_SITE,
            profile::CAPACITY,
        ];
        let missing: Vec<_> = sites
            .iter()
            .filter(|name| resolved.get(name).is_none())
            .copied()
            .collect();
        if !missing.is_empty() {
            return vec![format!(
                "industry-spawn probe: off; 40408 profile is missing {}",
                missing.join(", ")
            )];
        }

        let result = install_all(resolved);
        match result {
            Ok(()) => {
                ENABLED.store(true, Ordering::Release);
                vec![format!(
                    "industry-spawn probe: installed (opt-in {TRACE_ENV}; max {MAX_CALLBACKS} callbacks)"
                )]
            }
            Err(error) => vec![format!("industry-spawn probe: off; {error}")],
        }
    }

    fn install_all(resolved: &ResolvedProfile) -> Result<(), String> {
        let capacity = resolved
            .get(profile::CAPACITY)
            .ok_or_else(|| format!("missing {}", profile::CAPACITY))?;
        let detour = install_capacity_if_prologue_matches(
            read::<[u8; CAPACITY_PROLOGUE_LEN]>(capacity.address),
            || {
                // SAFETY: the compiled prologue and the exact-build resolver
                // both checked this function; the game remains suspended and
                // this thunk forwards its ABI.
                unsafe {
                    InlineDetour::install(
                        capacity.address as usize as *mut u8,
                        capacity_detour as *const () as *const u8,
                    )
                }
                .map_err(|error| format!("{}: {error}", profile::CAPACITY))
            },
        )?;
        CAPACITY_ORIGINAL.store(detour.trampoline() as usize, Ordering::Release);

        let mut splices = Vec::new();
        for (name, expected, steal, hook) in [
            (
                profile::SEED_SITE,
                profile::SEED_EXPECTED.as_slice(),
                profile::SEED_STEAL,
                seed_hook as tpf3mp_hookcore::detour::SpliceHook,
            ),
            (
                profile::CANDIDATE_SITE,
                profile::CANDIDATE_EXPECTED.as_slice(),
                profile::CANDIDATE_STEAL,
                candidate_hook as tpf3mp_hookcore::detour::SpliceHook,
            ),
            (
                profile::CANDIDATE_RESULT_SITE,
                profile::CANDIDATE_RESULT_EXPECTED.as_slice(),
                profile::CANDIDATE_RESULT_STEAL,
                candidate_result_hook as tpf3mp_hookcore::detour::SpliceHook,
            ),
            (
                profile::ENTITY_SITE,
                profile::ENTITY_EXPECTED.as_slice(),
                profile::ENTITY_STEAL,
                entity_hook as tpf3mp_hookcore::detour::SpliceHook,
            ),
            (
                profile::CALLBACK_END_SITE,
                profile::CALLBACK_END_EXPECTED.as_slice(),
                profile::CALLBACK_END_STEAL,
                callback_end_hook as tpf3mp_hookcore::detour::SpliceHook,
            ),
        ] {
            let target = resolved
                .get(name)
                .ok_or_else(|| format!("missing {name}"))?;
            // SAFETY: each exact-build profile site has whole straight-line
            // stolen instructions; install runs before the game starts.
            let splice = match unsafe {
                Splice::install(target.address as *mut u8, expected, steal, hook)
            } {
                Ok(splice) => splice,
                Err(error) => {
                    CAPACITY_ORIGINAL.store(0, Ordering::Release);
                    drop(splices);
                    drop(detour);
                    return Err(format!("{name}: {error}"));
                }
            };
            splices.push(splice);
        }
        let worker = std::thread::Builder::new()
            .name("tpf3mp-industry-probe".to_owned())
            .spawn(logger_worker);
        let worker = match worker {
            Ok(worker) => worker,
            Err(error) => {
                CAPACITY_ORIGINAL.store(0, Ordering::Release);
                drop(splices);
                drop(detour);
                return Err(format!("starting the diagnostic logger: {error}"));
            }
        };
        drop(worker);
        *SPLICES.lock().unwrap_or_else(|poison| poison.into_inner()) = splices;
        *CAPACITY_DETOUR
            .lock()
            .unwrap_or_else(|poison| poison.into_inner()) = Some(detour);
        Ok(())
    }

    fn guarded(body: impl FnOnce()) {
        if BROKEN.load(Ordering::Acquire) {
            return;
        }
        if std::panic::catch_unwind(std::panic::AssertUnwindSafe(body)).is_err() {
            BROKEN.store(true, Ordering::Release);
            ENABLED.store(false, Ordering::Release);
            CALLBACK.with(|callback| callback.borrow_mut().take());
            let mut queue = TRACE_QUEUE
                .lock()
                .unwrap_or_else(|poison| poison.into_inner());
            queue.failed = true;
            drop(queue);
            QUEUE_READY.notify_one();
        }
    }

    unsafe extern "system" fn seed_hook(regs: *mut SavedRegs) {
        guarded(|| {
            if !ENABLED.load(Ordering::Acquire) {
                return;
            }
            // SAFETY: the splice passes the saved register block for this site.
            let regs_ptr = regs;
            let regs = unsafe { &*regs_ptr };
            let time = read::<u32>(SavedRegs::rsp(regs_ptr).wrapping_add(0x60));
            let Some(time) = time else {
                return;
            };
            CALLBACK.with(|callback| {
                *callback.borrow_mut() = Some(Callback::new(time, regs.rdi));
            });
        });
    }

    unsafe extern "system" fn candidate_hook(regs: *mut SavedRegs) {
        guarded(|| {
            if !ENABLED.load(Ordering::Acquire) {
                return;
            }
            // SAFETY: the splice passes the saved register block for this site.
            let regs = unsafe { &*regs };
            let count = read::<i32>(regs.rbp.wrapping_add(0x198));
            CALLBACK.with(|callback| {
                if let Some(callback) = callback.borrow_mut().as_mut() {
                    callback.add_candidate(Candidate {
                        ordinal: regs.r12 as u32,
                        resource: regs.rdi as u32 as i32,
                        count: count.unwrap_or(-1),
                        closed_gate: None,
                        entity: None,
                    });
                }
            });
        });
    }

    unsafe extern "system" fn candidate_result_hook(regs: *mut SavedRegs) {
        guarded(|| {
            if !ENABLED.load(Ordering::Acquire) {
                return;
            }
            // SAFETY: the splice passes the saved register block for this site.
            let regs_ptr = regs;
            let regs = unsafe { &*regs_ptr };
            let flag = read::<u8>(SavedRegs::rsp(regs_ptr).wrapping_add(0x70));
            CALLBACK.with(|callback| {
                if let Some(callback) = callback.borrow_mut().as_mut()
                    && let Some(candidate) = callback.candidate_mut(regs.r12 as u32)
                {
                    candidate.closed_gate = flag.map(|value| value != 0);
                }
            });
        });
    }

    unsafe extern "system" fn entity_hook(regs: *mut SavedRegs) {
        guarded(|| {
            if !ENABLED.load(Ordering::Acquire) {
                return;
            }
            // SAFETY: the splice passes the saved register block for this site.
            let regs = unsafe { &*regs };
            let entity = read::<i32>(regs.rbp.wrapping_sub(0x7c));
            CALLBACK.with(|callback| {
                if let Some(callback) = callback.borrow_mut().as_mut()
                    && let Some(candidate) = callback.candidate_mut(regs.r12 as u32)
                {
                    candidate.entity = entity;
                }
            });
        });
    }

    unsafe extern "system" fn callback_end_hook(_regs: *mut SavedRegs) {
        guarded(|| {
            if !ENABLED.load(Ordering::Acquire) {
                return;
            }
            let callback = CALLBACK.with(|callback| callback.borrow_mut().take());
            let Some(callback) = callback else {
                return;
            };
            let count = CALLBACKS.fetch_add(1, Ordering::AcqRel);
            let mut queue = TRACE_QUEUE
                .lock()
                .unwrap_or_else(|poison| poison.into_inner());
            if count < MAX_CALLBACKS {
                queue.push(callback);
            }
            if count + 1 >= MAX_CALLBACKS {
                ENABLED.store(false, Ordering::Release);
                queue.limit_reached = true;
            }
            drop(queue);
            QUEUE_READY.notify_one();
        });
    }

    unsafe extern "system" fn capacity_detour(context: usize, key: usize) -> i32 {
        let original = CAPACITY_ORIGINAL.load(Ordering::Acquire);
        if original == 0 {
            return -1;
        }
        // SAFETY: this is the exact-build function's two-argument ABI; the
        // trampoline returned by InlineDetour preserves its original body.
        let original: CapacityFn = unsafe { std::mem::transmute(original) };
        // SAFETY: arguments came from the engine's call of this native function.
        let result = unsafe { original(context, key) };
        let active = CALLBACK.with(|callback| callback.borrow().is_some());
        if ENABLED.load(Ordering::Acquire) && !BROKEN.load(Ordering::Acquire) && active {
            guarded(|| {
                let record = Capacity {
                    first: read_msvc_string(key as u64),
                    second: read_msvc_string((key as u64).wrapping_add(0x20)),
                    result,
                };
                CALLBACK.with(|callback| {
                    if let Some(callback) = callback.borrow_mut().as_mut() {
                        callback.capacity = Some(record);
                    }
                });
            });
        }
        result
    }

    fn read<T: Copy>(address: u64) -> Option<T> {
        Readable::new().read(address)
    }

    fn read_msvc_string(object: u64) -> Option<Text> {
        let len = read::<u64>(object.checked_add(0x10)?)?;
        let capacity = read::<u64>(object.checked_add(0x18)?)?;
        let len = usize::try_from(len).ok()?;
        let capacity = usize::try_from(capacity).ok()?;
        if len > MAX_TEXT || capacity < len || capacity > 4096 {
            return None;
        }
        let data = if capacity < 16 {
            object
        } else {
            read::<u64>(object)?
        };
        let mut text = Text::default();
        for (index, byte) in text.bytes.iter_mut().take(len).enumerate() {
            *byte = read::<u8>(data.checked_add(index as u64)?)?;
        }
        text.len = len as u8;
        Some(text)
    }

    fn logger_worker() {
        loop {
            let mut records = [None; QUEUE_CAPACITY];
            let (count, dropped, failed, limit_reached) = {
                let mut queue = TRACE_QUEUE
                    .lock()
                    .unwrap_or_else(|poison| poison.into_inner());
                while queue.len == 0 && queue.dropped == 0 && !queue.failed && !queue.limit_reached
                {
                    queue = QUEUE_READY
                        .wait(queue)
                        .unwrap_or_else(|poison| poison.into_inner());
                }
                queue.drain(&mut records)
            };
            for record in records.iter().take(count).flatten() {
                log::line(&record.line());
            }
            if dropped != 0 {
                log::line(&format!(
                    "industry-spawn probe: dropped {dropped} callbacks because its fixed queue was full"
                ));
            }
            if failed {
                log::line(
                    "industry-spawn probe: panicked on the game's thread; disabled for this game",
                );
            }
            if limit_reached {
                log::line("industry-spawn probe: callback limit reached; disabled for this game");
            }
        }
    }

    static CAPACITY_DETOUR: Mutex<Option<InlineDetour>> = Mutex::new(None);
}
