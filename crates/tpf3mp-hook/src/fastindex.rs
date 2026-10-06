//! The faster component lookup (`fast-component-index`; on unless
//! [`TOGGLE_ENV`] is `0` or `off`; docs/HOOKS.md, "The faster component
//! lookup").
//!
//! `ecs::Engine::GetComponentDataIndex(engine, entity, type)` (`0xa4b90`,
//! `Engine.h:0x143`, about 1,600 direct calls) answers which slot of a
//! component type's array holds an entity's component:
//!
//! ```text
//! list = [engine+0x90] + (int64)entity * 24      // std::vector<{int type, int index}>
//! for (p = list.begin; p != list.end; ++p)
//!     if (p->type == type) return p->index;       // the first match
//! assert(it != components.end())                  // a miss: the game's assert, no return
//! ```
//!
//! The search itself is a dozen instructions, but the function keeps a
//! 0xE0-byte frame and a `/GS` cookie for the assert's inlined formatting
//! on every call: a cookie load, xor and store, a call of
//! `__security_check_cookie`, three stores and two loads to the stack. In a
//! profile of a 100 x 1000-tile map it was the main thread's top function
//! (investigation/TF3_SIM_COST_2026-10-05.md §3.1).
//!
//! The hook jumps from its entry to [`fast`]: the same reads, in the same
//! order, the same first match, with no frame. A miss (the list ends
//! without the type) jumps to the original through its trampoline with
//! every argument register as it came, so the game's own code searches
//! again and asserts as it would have. The answer is the original's for
//! every input, which the tests check against the game's own code,
//! relocated from the executable. Nothing is cached: there is no state to
//! keep coherent, and a call from any thread reads exactly what the
//! original read, so it is as safe with concurrent callers as the original.
//!
//! Registers: on a hit the original changes `rax`, `rcx`, `rdx`, `r9` and
//! the flags (the cookie check clobbers `rcx`); [`fast`] changes `rax` and
//! `r9` only, and on a miss leaves `rcx`, `rdx` and `r8` for the original.
//! No caller can tell the two apart but by time.
//!
//! With `TPF3MP_HOOK_PERF=full` the entry is [`fast_counted`] instead: one
//! `lock add` to a counter picked by the thread's stack, so threads seldom
//! share one, and [`take_calls`] sums them for the `perf: sim` line.

#![allow(unsafe_code)]
#![cfg_attr(not(all(windows, target_arch = "x86_64")), allow(dead_code))]

use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};

use tpf3mp_hookcore::profile::ResolvedProfile;

pub use crate::build_data::native::simperf::{
    COMPONENT_INDEX, ENTITY_LIST_STRIDE, ENTITY_LISTS, PAIR_SIZE,
};

pub const FIX: &str = "fast-component-index";
/// Set to `0` (or `off`), the game's own lookup runs.
pub const TOGGLE_ENV: &str = "TPF3MP_HOOK_FAST_COMPONENT_INDEX";

// The assembly below is written for these.
const _: () = assert!(ENTITY_LIST_STRIDE == 24 && PAIR_SIZE == 8);

/// The original's trampoline (its stolen prologue, then the rest of it in
/// place); [`fast`] jumps there on a miss.
static ORIGINAL: AtomicUsize = AtomicUsize::new(0);
/// Whether the counting entry is in.
static COUNTING: AtomicBool = AtomicBool::new(false);
/// Whether either entry is in.
static ON: AtomicBool = AtomicBool::new(false);

/// One counter on a cache line of its own.
#[repr(C, align(64))]
pub struct Slot(AtomicU64);

/// How many counters the counting entry spreads its calls over.
const SLOTS: usize = 64;
static COUNTS: [Slot; SLOTS] = [const { Slot(AtomicU64::new(0)) }; SLOTS];
/// The counters' sum at the last [`take_calls`].
static TAKEN: AtomicU64 = AtomicU64::new(0);

/// The lookup's hot path, frameless. `rcx` the engine, `edx` the entity,
/// `r8d` the type; only `rax` and `r9` change before the miss jump.
#[cfg(all(windows, target_arch = "x86_64"))]
macro_rules! lookup {
    () => {
        concat!(
            "movsxd rax, edx\n",
            "lea rax, [rax + rax*2]\n",
            "mov r9, qword ptr [rcx + {lists}]\n",
            "lea r9, [r9 + rax*8]\n",
            "mov rax, qword ptr [r9]\n",
            "mov r9, qword ptr [r9 + 8]\n",
            "cmp rax, r9\n",
            "je 3f\n",
            "2:\n",
            "cmp dword ptr [rax], r8d\n",
            "je 4f\n",
            "add rax, 8\n",
            "cmp rax, r9\n",
            "jne 2b\n",
            "3:\n",
            "jmp qword ptr [rip + {original}]\n",
            "4:\n",
            "mov eax, dword ptr [rax + 4]\n",
            "ret\n",
        )
    };
}

/// `GetComponentDataIndex`'s entry while the fix is in.
///
/// # Safety
///
/// Called by the game in place of the original, with its arguments.
#[cfg(all(windows, target_arch = "x86_64"))]
#[unsafe(naked)]
pub unsafe extern "system" fn fast(_engine: usize, _entity: i32, _type_index: i32) -> i32 {
    core::arch::naked_asm!(
        lookup!(),
        lists = const ENTITY_LISTS,
        original = sym ORIGINAL,
    )
}

/// [`fast`], counting its calls first: `rsp >> 16`, hashed to one of
/// [`SLOTS`] counters, so the threads (whose stacks lie a megabyte or more
/// apart) seldom meet on one line.
///
/// # Safety
///
/// As [`fast`].
#[cfg(all(windows, target_arch = "x86_64"))]
#[unsafe(naked)]
pub unsafe extern "system" fn fast_counted(_engine: usize, _entity: i32, _type_index: i32) -> i32 {
    core::arch::naked_asm!(
        "mov rax, rsp",
        "shr rax, 16",
        "mov r9, 0x9E3779B97F4A7C15",
        "imul rax, r9",
        "shr rax, 58",
        "shl eax, 6",
        "lea r9, [rip + {counts}]",
        "lock add qword ptr [r9 + rax], 1",
        lookup!(),
        counts = sym COUNTS,
        lists = const ENTITY_LISTS,
        original = sym ORIGINAL,
    )
}

/// What the fix is doing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    Off,
    Fast,
    Counting,
}

pub fn state() -> State {
    if !ON.load(Ordering::Acquire) {
        State::Off
    } else if COUNTING.load(Ordering::Acquire) {
        State::Counting
    } else {
        State::Fast
    }
}

/// The counters' total.
fn calls() -> u64 {
    COUNTS
        .iter()
        .map(|slot| slot.0.load(Ordering::Relaxed))
        .fold(0u64, u64::wrapping_add)
}

/// The calls since the last take, while the counting entry is in.
pub fn take_calls() -> Option<u64> {
    if state() != State::Counting {
        return None;
    }
    let now = calls();
    Some(now.wrapping_sub(TAKEN.swap(now, Ordering::Relaxed)))
}

/// What installing came to, for hook.log.
pub fn outcome_line(installed: bool, reason: &str) -> String {
    if installed {
        format!("{FIX}: installed ({reason})")
    } else {
        format!("{FIX}: off, {reason}")
    }
}

/// Installs the fix unless [`TOGGLE_ENV`] says no. Returns the line for
/// hook.log.
pub fn install(resolved: &ResolvedProfile) -> String {
    if !crate::ticks::wanted(std::env::var(TOGGLE_ENV).ok().as_deref()) {
        return outcome_line(
            false,
            &format!("{TOGGLE_ENV} says so; the game's own lookup"),
        );
    }
    let Some(target) = resolved.get(COMPONENT_INDEX) else {
        return outcome_line(
            false,
            &format!("the profile has no {COMPONENT_INDEX:?}; the game's own lookup"),
        );
    };
    install_at(target.address as usize, crate::perf::full())
}

#[cfg(all(windows, target_arch = "x86_64"))]
fn install_at(target: usize, counted: bool) -> String {
    use tpf3mp_hookcore::detour::InlineDetour;
    let entry = if counted {
        fast_counted as *const u8
    } else {
        fast as *const u8
    };
    // SAFETY: the profile resolved `target` to GetComponentDataIndex, its
    // signature the whole hit path [`fast`] repeats; no game thread runs
    // yet (the hook installs before the game's entry point); `fast` has
    // its ABI and hands every miss to the trampoline.
    match unsafe { InlineDetour::install(target as *mut u8, entry) } {
        Ok(detour) => {
            ORIGINAL.store(detour.trampoline() as usize, Ordering::Release);
            COUNTING.store(counted, Ordering::Release);
            ON.store(true, Ordering::Release);
            std::mem::forget(detour);
            outcome_line(
                true,
                &format!(
                    "GetComponentDataIndex at {target:#x}: the same search without a frame, a miss to the game's own{}; {TOGGLE_ENV}=0 turns it off",
                    if counted {
                        ", calls counted (TPF3MP_HOOK_PERF=full)"
                    } else {
                        ""
                    }
                ),
            )
        }
        Err(error) => outcome_line(
            false,
            &format!("{COMPONENT_INDEX} at {target:#x}: {error}; the game's own lookup"),
        ),
    }
}

#[cfg(not(all(windows, target_arch = "x86_64")))]
fn install_at(_target: usize, _counted: bool) -> String {
    outcome_line(false, "built for Windows x64 only; the game's own lookup")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_line_says_what_the_fix_did() {
        assert_eq!(outcome_line(false, "why"), "fast-component-index: off, why");
        assert_eq!(
            outcome_line(true, "how"),
            "fast-component-index: installed (how)"
        );
    }
}

/// Against the game's own code: `GetComponentDataIndex` relocated from
/// the executable, its miss path's assert answered by a stub that returns
/// [`MISS`] from the original's frame.
#[cfg(all(test, windows, target_arch = "x86_64"))]
mod original_tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use std::collections::HashMap;
    use std::sync::{Mutex, OnceLock};

    use super::*;
    use crate::original::{Exe, Page};

    static SERIAL: Mutex<()> = Mutex::new(());

    const RVA: u64 = 0xa4b90;
    /// Entry to the `int3` after the assert's call (0xa4caf), inclusive.
    const LEN: usize = 0x120;
    const COOKIE: u64 = 0x3ce3a38;
    const CHECK_COOKIE: u64 = 0x3184140;
    const ASSERT: u64 = 0x303d3e0;
    /// The miss path's helpers (the type's name, the message), and the
    /// strings it hands the assert.
    const HELPERS: [u64; 4] = [0x9d4f0, 0x9d510, 0x9dba0, 0x94f90];
    const STRINGS: [u64; 3] = [0x3675378, 0x36752c0, 0x36753a0];
    /// What the stub makes a miss return.
    const MISS: i32 = 0x7EAD_BEEF;

    static MISSES: AtomicU64 = AtomicU64::new(0);

    /// `__security_check_cookie`: returns, keeping `rax` (the answer).
    #[unsafe(naked)]
    extern "system" fn check_cookie() {
        core::arch::naked_asm!("ret")
    }

    extern "system" fn nothing(rcx: usize) -> usize {
        rcx
    }

    /// The assert, reached at `E - 0xf0` (E the original's entry `rsp`):
    /// unwinds its frame as its epilogue would (`rdi` pushed, `rbx` homed
    /// at `E+0x20`) and returns [`MISS`] to its caller.
    #[unsafe(naked)]
    extern "system" fn escape() {
        core::arch::naked_asm!(
            "lock add qword ptr [rip + {misses}], 1",
            "add rsp, 0xe8",
            "pop rdi",
            "mov rbx, qword ptr [rsp + 0x20]",
            "mov eax, {miss}",
            "ret",
            misses = sym MISSES,
            miss = const MISS,
        )
    }

    /// Calls `f(engine, entity, type)` with every other register set to a
    /// known value and writes them all back afterwards: `out[0]` rax, then
    /// rbx, rcx, rdx, rsi, rdi, rbp, r8 to r15.
    ///
    /// # Safety
    ///
    /// `f` takes the lookup's arguments; `out` is writable.
    #[unsafe(naked)]
    unsafe extern "system" fn probe_call(
        _f: usize,
        _engine: usize,
        _entity: i32,
        _type_index: i32,
        _out: *mut [u64; 16],
    ) {
        core::arch::naked_asm!(
            "push rbx",
            "push rbp",
            "push rsi",
            "push rdi",
            "push r12",
            "push r13",
            "push r14",
            "push r15",
            "sub rsp, 0x28",
            // out: the fifth argument, above the 0x40 pushed, the 0x28 and
            // the return address's 8 + 0x20 of home space.
            "mov rax, qword ptr [rsp + 0x90]",
            "mov qword ptr [rsp + 0x20], rax",
            "mov rax, rcx",
            "mov rcx, rdx",
            "mov edx, r8d",
            "mov r8d, r9d",
            "mov rbx, 0x1111111111111111",
            "mov rbp, 0x2222222222222222",
            "mov rsi, 0x3333333333333333",
            "mov rdi, 0x4444444444444444",
            "mov r9, 0x5555555555555555",
            "mov r10, 0x6666666666666666",
            "mov r11, 0x7777777777777777",
            "mov r12, 0x8888888888888888",
            "mov r13, 0x9999999999999999",
            "mov r14, 0xAAAAAAAAAAAAAAAA",
            "mov r15, 0xBBBBBBBBBBBBBBBB",
            "call rax",
            "xchg rax, qword ptr [rsp + 0x20]",
            "mov qword ptr [rax + 0x08], rbx",
            "mov qword ptr [rax + 0x10], rcx",
            "mov qword ptr [rax + 0x18], rdx",
            "mov qword ptr [rax + 0x20], rsi",
            "mov qword ptr [rax + 0x28], rdi",
            "mov qword ptr [rax + 0x30], rbp",
            "mov qword ptr [rax + 0x38], r8",
            "mov qword ptr [rax + 0x40], r9",
            "mov qword ptr [rax + 0x48], r10",
            "mov qword ptr [rax + 0x50], r11",
            "mov qword ptr [rax + 0x58], r12",
            "mov qword ptr [rax + 0x60], r13",
            "mov qword ptr [rax + 0x68], r14",
            "mov qword ptr [rax + 0x70], r15",
            "mov rcx, qword ptr [rsp + 0x20]",
            "mov qword ptr [rax], rcx",
            "add rsp, 0x28",
            "pop r15",
            "pop r14",
            "pop r13",
            "pop r12",
            "pop rdi",
            "pop rsi",
            "pop rbp",
            "pop rbx",
            "ret",
        )
    }

    const NAMES: [&str; 16] = [
        "rax", "rbx", "rcx", "rdx", "rsi", "rdi", "rbp", "r8", "r9", "r10", "r11", "r12", "r13",
        "r14", "r15", "-",
    ];

    /// A near jump through a slot holding `f`.
    fn jump(page: &mut Page, f: usize) -> usize {
        let slot = page.data(&(f as u64).to_le_bytes());
        let at = page.code(&[0xFF, 0x25, 0, 0, 0, 0]);
        let rel = (slot as i64 - (at as i64 + 6)) as i32;
        // SAFETY: the jump's displacement, in the page just written.
        unsafe {
            std::ptr::copy_nonoverlapping(rel.to_le_bytes().as_ptr(), (at + 2) as *mut u8, 4)
        };
        at
    }

    struct Rig {
        _page: Page,
        original: usize,
    }

    /// The game's function, relocated once; [`ORIGINAL`] points at it, as
    /// at the trampoline in the game.
    fn rig() -> Option<&'static Rig> {
        static RIG: OnceLock<Option<Rig>> = OnceLock::new();
        RIG.get_or_init(|| {
            let exe = Exe::load()?;
            let mut page = Page::new();
            let mut stubs: HashMap<u64, usize> = HashMap::new();
            stubs.insert(
                CHECK_COOKIE,
                jump(&mut page, check_cookie as *const () as usize),
            );
            stubs.insert(ASSERT, jump(&mut page, escape as *const () as usize));
            for helper in HELPERS {
                stubs.insert(helper, jump(&mut page, nothing as *const () as usize));
            }
            stubs.insert(COOKIE, page.data(exe.bytes(COOKIE, 8)));
            for string in STRINGS {
                stubs.insert(string, page.data(&[0u8; 16]));
            }
            let (original, _) = page.relocate(&exe, RVA, LEN, &|rva| stubs.get(&rva).copied());
            ORIGINAL.store(original, Ordering::Release);
            Some(Rig {
                _page: page,
                original,
            })
        })
        .as_ref()
    }

    /// An engine as the lookup reads it: `[engine+0x90]` the entities'
    /// list headers, which start a few headers into their allocation so
    /// that negative entities read headers too.
    struct World {
        engine: Box<[u8; 0x100]>,
        _headers: Vec<[usize; 3]>,
        _lists: Vec<Vec<[i32; 2]>>,
        entities: i32,
    }

    const BEFORE: usize = 3;

    struct Rng(u64);
    impl Rng {
        fn next(&mut self) -> u64 {
            // splitmix64
            self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
            let mut z = self.0;
            z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
            z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
            z ^ (z >> 31)
        }
        fn below(&mut self, n: u64) -> u64 {
            self.next() % n
        }
    }

    /// Lists of 0 to 23 pairs, a few of 40 to 200; with `short`, 1 to
    /// `short` pairs.
    fn world(rng: &mut Rng, entities: usize, types: i32) -> World {
        world_of(rng, entities, types, None)
    }

    fn world_of(rng: &mut Rng, entities: usize, types: i32, short: Option<u64>) -> World {
        let mut lists: Vec<Vec<[i32; 2]>> = (0..entities + BEFORE)
            .map(|i| {
                let len = match (short, rng.below(20)) {
                    (Some(short), _) => 1 + rng.below(short) as usize,
                    (None, 0) => 0,
                    (None, 1) => 40 + rng.below(160) as usize,
                    (None, _) => rng.below(24) as usize,
                };
                let mut list: Vec<[i32; 2]> = (0..len)
                    .map(|_| {
                        let ty = match rng.below(50) {
                            0 => i32::MIN,
                            1 => i32::MAX,
                            2 => -1 - rng.below(5) as i32,
                            _ => rng.below(types as u64) as i32,
                        };
                        [ty, rng.below(1 << 30) as i32]
                    })
                    .collect();
                if i % 7 == 0 {
                    list.reserve(8); // a capacity past the end
                }
                list
            })
            .collect();
        let headers: Vec<[usize; 3]> = lists
            .iter_mut()
            .map(|list| {
                let begin = list.as_mut_ptr() as usize;
                [
                    begin,
                    begin + list.len() * PAIR_SIZE,
                    begin + list.capacity() * PAIR_SIZE,
                ]
            })
            .collect();
        let mut engine = Box::new([0u8; 0x100]);
        let table = headers.as_ptr() as usize + BEFORE * ENTITY_LIST_STRIDE;
        engine[ENTITY_LISTS..ENTITY_LISTS + 8].copy_from_slice(&(table as u64).to_le_bytes());
        World {
            engine,
            _headers: headers,
            _lists: lists,
            entities: entities as i32,
        }
    }

    type Lookup = unsafe extern "system" fn(usize, i32, i32) -> i32;

    fn call(f: usize, world: &World, entity: i32, ty: i32) -> i32 {
        // SAFETY: `f` is the relocated original or one of the fast entries,
        // `world` lays out every list `entity` can name.
        unsafe {
            std::mem::transmute::<usize, Lookup>(f)(world.engine.as_ptr() as usize, entity, ty)
        }
    }

    /// Queries: mostly types the entity has, some it may not, and the
    /// extremes.
    fn query(rng: &mut Rng, world: &World, types: i32) -> (i32, i32) {
        let entity = rng.below((world.entities + BEFORE as i32) as u64) as i32 - BEFORE as i32;
        let ty = match rng.below(40) {
            0 => i32::MIN,
            1 => i32::MAX,
            2 => -1 - rng.below(5) as i32,
            3 => types + rng.below(10) as i32,
            _ => rng.below(types as u64) as i32,
        };
        (entity, ty)
    }

    #[test]
    fn the_fast_lookup_answers_as_the_games_own_code() {
        let _serial = SERIAL.lock().unwrap_or_else(|p| p.into_inner());
        let Some(rig) = rig() else { return };
        let mut rng = Rng(0x005E_ED0F_C0DE);
        let mut hits = 0;
        let mut misses = 0;
        for round in 0..8 {
            let types = [4, 12, 40, 200][round % 4];
            let world = world(&mut rng, 2_000, types);
            for _ in 0..40_000 {
                let (entity, ty) = query(&mut rng, &world, types);
                let before = MISSES.load(Ordering::Acquire);
                let want = call(rig.original, &world, entity, ty);
                let original_missed = MISSES.load(Ordering::Acquire) - before;
                let got = call(fast as *const () as usize, &world, entity, ty);
                let fast_missed = MISSES.load(Ordering::Acquire) - before - original_missed;
                let counted = call(fast_counted as *const () as usize, &world, entity, ty);
                assert_eq!(got, want, "entity {entity} type {ty}");
                assert_eq!(counted, want, "counted: entity {entity} type {ty}");
                assert_eq!(
                    fast_missed, original_missed,
                    "a miss reaches the original's assert, a hit never does"
                );
                if want == MISS {
                    misses += 1;
                } else {
                    hits += 1;
                }
            }
        }
        assert!(
            hits > 100_000 && misses > 20_000,
            "{hits} hits, {misses} misses"
        );
    }

    /// The registers a call changed, by name.
    fn changed(out: &[u64; 16], engine: usize, entity: i32, ty: i32) -> Vec<&'static str> {
        let before: [u64; 16] = [
            out[0],
            0x1111111111111111,
            engine as u64,
            entity as u32 as u64,
            0x3333333333333333,
            0x4444444444444444,
            0x2222222222222222,
            ty as u32 as u64,
            0x5555555555555555,
            0x6666666666666666,
            0x7777777777777777,
            0x8888888888888888,
            0x9999999999999999,
            0xAAAAAAAAAAAAAAAA,
            0xBBBBBBBBBBBBBBBB,
            0,
        ];
        (1..15)
            .filter(|&i| out[i] != before[i])
            .map(|i| NAMES[i])
            .collect()
    }

    #[test]
    fn the_fast_lookup_changes_no_register_the_original_keeps() {
        let _serial = SERIAL.lock().unwrap_or_else(|p| p.into_inner());
        let Some(rig) = rig() else { return };
        let mut rng = Rng(77);
        let world = world(&mut rng, 300, 12);
        let engine = world.engine.as_ptr() as usize;
        let mut seen_hit = false;
        let mut seen_miss = false;
        for _ in 0..2_000 {
            let (entity, ty) = query(&mut rng, &world, 12);
            let run = |f: usize| {
                let mut out = [0u64; 16];
                // SAFETY: `f` is a lookup over `world`; `out` is ours.
                unsafe { probe_call(f, engine, entity, ty, &mut out) };
                out
            };
            let original = run(rig.original);
            let original_changed = changed(&original, engine, entity, ty);
            for f in [
                fast as *const () as usize,
                fast_counted as *const () as usize,
            ] {
                let out = run(f);
                assert_eq!(out[0] as u32, original[0] as u32, "the answer");
                let fast_changed = changed(&out, engine, entity, ty);
                if original[0] as u32 as i32 == MISS {
                    // The miss went to the original: its registers.
                    seen_miss = true;
                    assert_eq!(fast_changed, original_changed, "a miss");
                } else {
                    seen_hit = true;
                    assert_eq!(fast_changed, ["r9"], "a hit changes r9 only");
                    for name in &fast_changed {
                        assert!(original_changed.contains(name), "{name}");
                    }
                }
            }
        }
        assert!(seen_hit && seen_miss);
    }

    #[test]
    fn the_counting_entry_counts_every_call_on_every_thread() {
        let _serial = SERIAL.lock().unwrap_or_else(|p| p.into_inner());
        if rig().is_none() {
            return;
        }
        let mut rng = Rng(3);
        let world = std::sync::Arc::new(world(&mut rng, 100, 4));
        let before = calls();
        let threads: Vec<_> = (0..4)
            .map(|t| {
                let world = world.clone();
                std::thread::spawn(move || {
                    let mut rng = Rng(t);
                    for _ in 0..25_000 {
                        let entity = rng.below(100) as i32;
                        let ty = rng.below(4) as i32;
                        call(fast_counted as *const () as usize, &world, entity, ty);
                        call(fast as *const () as usize, &world, entity, ty);
                    }
                })
            })
            .collect();
        for thread in threads {
            thread.join().unwrap();
        }
        assert_eq!(calls() - before, 100_000, "the counted entry's calls only");
    }

    /// The gain, on this PC. Run with `cargo test --release -p tpf3mp-hook
    /// fastindex::original_tests::a_lookup_costs -- --ignored --nocapture`.
    #[test]
    #[ignore = "a benchmark: prints the cost of a lookup"]
    fn a_lookup_costs() {
        let _serial = SERIAL.lock().unwrap_or_else(|p| p.into_inner());
        let Some(rig) = rig() else { return };
        let mut rng = Rng(11);
        for (label, entities, short) in [
            ("hot, 64 entities, 0-200 pairs", 64, None),
            ("hot, 64 entities, 1-8 pairs", 64, Some(8)),
            ("cold, 2 M entities, 0-200 pairs", 2_000_000, None),
            ("cold, 2 M entities, 1-8 pairs", 2_000_000, Some(8)),
        ] {
            let world = world_of(&mut rng, entities, 12, short);
            // Only hits: the queries take a type the entity has.
            let queries: Vec<(i32, i32)> = (0..1_000_000)
                .filter_map(|_| {
                    let entity = rng.below(entities as u64) as i32;
                    let list = &world._lists[entity as usize + BEFORE];
                    (!list.is_empty())
                        .then(|| (entity, list[rng.below(list.len() as u64) as usize][0]))
                })
                .collect();
            let time = |f: usize| {
                let start = std::time::Instant::now();
                let mut sum = 0i64;
                for &(entity, ty) in &queries {
                    sum += i64::from(call(f, &world, entity, ty));
                }
                std::hint::black_box(sum);
                start.elapsed().as_nanos() as f64 / queries.len() as f64
            };
            // The best of seven rounds, the three entries in turn.
            let entries = [
                rig.original,
                fast as *const () as usize,
                fast_counted as *const () as usize,
            ];
            let mut best = [f64::MAX; 3];
            for _ in 0..7 {
                for (best, &f) in best.iter_mut().zip(&entries) {
                    *best = best.min(time(f));
                }
            }
            println!(
                "{label}: the game's {:.2} ns, fast {:.2} ns, counted {:.2} ns a lookup",
                best[0], best[1], best[2]
            );
        }
    }
}
