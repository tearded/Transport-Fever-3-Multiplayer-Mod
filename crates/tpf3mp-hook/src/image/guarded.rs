//! Guarded reads: reading the game's memory without asking the system
//! first, and still failing closed (docs/HOOKS.md, "Reading the game's
//! memory").
//!
//! Two tiny leaf routines, written in assembly so their instructions sit
//! at addresses known to the hook, do every read: [`probe`] touches one
//! byte of each page a range spans, [`copy`] copies bytes out. A
//! process-wide vectored exception handler, added first, looks at every
//! exception the process raises and acts on exactly one kind: a **read**
//! fault (`EXCEPTION_ACCESS_VIOLATION`, `STATUS_GUARD_PAGE_VIOLATION` or
//! `EXCEPTION_IN_PAGE_ERROR` whose first parameter says "read") whose
//! instruction lies inside those routines. It sends that thread on to the
//! routines' shared recovery code, which answers 0 ("not readable"), and
//! the read is refused as before. Every other exception, a write fault in
//! the routines included, goes on to the game's own handlers and its crash
//! reporter exactly as if the hook were not there
//! (`EXCEPTION_CONTINUE_SEARCH`).
//!
//! The normal case, readable memory, costs a call and the loads; no
//! system call, no cache. A refused read costs one exception dispatch (a
//! few microseconds), and refusals are rare: they mean a layout the hook
//! misread. The `perf:` line counts both.
//!
//! **Guard pages.** Reading a `PAGE_GUARD` page clears its guard and
//! raises `STATUS_GUARD_PAGE_VIOLATION` (only a thread's own stack guard
//! page is handled by the kernel, which grows the stack: harmless). The
//! guard is how another thread's stack, or a guarded heap, notices growth,
//! so the handler puts it back (`VirtualProtect` with the page's
//! protection plus `PAGE_GUARD`) before refusing, the same answer the
//! `VirtualQuery` check gave. Between the system clearing it and the
//! handler re-arming it is a window of a few microseconds; a guard page is
//! reachable only through a pointer the hook misread, so this is the rare
//! path of a rare path.
//!
//! **Writes.** A read proves nothing about writing. The fixes that sort in
//! place write through memory they checked readable, as they did with the
//! `VirtualQuery` check (which never asked about writing either): the
//! memory is a vector the engine itself just wrote, on the engine's thread.
//!
//! `TPF3MP_HOOK_GUARDED_READS=0` (or `off`) in the game's environment, a
//! handler the system refuses, or a build that is not x86-64 Windows: the
//! hook checks with `VirtualQuery` through the region caches, as before.

use std::sync::{
    OnceLock,
    atomic::{AtomicU64, Ordering},
};

/// The kill switch: `0` (or `off`) checks with `VirtualQuery` instead.
pub const ENV: &str = "TPF3MP_HOOK_GUARDED_READS";

/// Below this nothing is ever mapped on Windows (the first 64 KiB are
/// reserved): a null pointer plus a small offset, refused without a fault.
pub const MIN_ADDRESS: usize = 0x1_0000;
/// The highest user-mode address on x86-64 Windows; past it is the
/// kernel's, and a range that wraps is refused too.
pub const MAX_ADDRESS: usize = 0x7FFF_FFFE_FFFF;

/// How guarded reads came out at first use.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// The handler is in: reads go through the routines.
    On,
    /// The kill switch said no.
    Off,
    /// The system refused the handler, or this build has no routines.
    Unavailable,
}

static MODE: OnceLock<Mode> = OnceLock::new();

/// Reads done through the routines, and of those, the ones refused by a
/// fault (the `perf:` line's).
static READS: AtomicU64 = AtomicU64::new(0);
static FAULTS: AtomicU64 = AtomicU64::new(0);
/// Guard pages put back after a guarded read cleared them.
static REARMED: AtomicU64 = AtomicU64::new(0);

thread_local! {
    /// This thread's reads not yet added to [`READS`]: one shared counter
    /// bumped from every game thread a few million times a second was
    /// itself a cost.
    static PENDING: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };
}

/// Reads a thread keeps before adding them to [`READS`].
const FLUSH_EVERY: u32 = 256;

fn count_read() {
    PENDING.with(|pending| {
        let n = pending.get() + 1;
        if n >= FLUSH_EVERY {
            READS.fetch_add(u64::from(n), Ordering::Relaxed);
            pending.set(0);
        } else {
            pending.set(n);
        }
    });
}

/// The mode the kill switch's value asks for.
pub fn wanted(value: Option<&str>) -> bool {
    crate::ticks::wanted(value)
}

/// The mode, settled at the first call: the kill switch read, the handler
/// added. Cheap after that (one atomic load).
pub fn mode() -> Mode {
    *MODE.get_or_init(|| {
        if !wanted(std::env::var(ENV).ok().as_deref()) {
            return Mode::Off;
        }
        if sys::install() {
            Mode::On
        } else {
            Mode::Unavailable
        }
    })
}

/// Whether reads go through the routines.
pub fn active() -> bool {
    mode() == Mode::On
}

/// Settles the mode now and says it, for hook.log at install.
pub fn configure_from_env() -> String {
    match mode() {
        Mode::On => format!(
            "image: guarded reads on (a vectored handler refuses a misread address; {ENV}=0 checks with VirtualQuery)"
        ),
        Mode::Off => {
            format!("image: guarded reads off ({ENV} says so), checking with VirtualQuery")
        }
        Mode::Unavailable => {
            "image: guarded reads unavailable (no handler), checking with VirtualQuery".to_owned()
        }
    }
}

/// Guarded reads and faults since the last take; the calling thread's
/// pending reads are added first (the others' lag by under
/// [`FLUSH_EVERY`]).
pub fn take_counts() -> (u64, u64) {
    PENDING.with(|pending| {
        READS.fetch_add(u64::from(pending.replace(0)), Ordering::Relaxed);
    });
    (
        READS.swap(0, Ordering::Relaxed),
        FAULTS.swap(0, Ordering::Relaxed),
    )
}

/// Guard pages put back since the process began.
pub fn rearmed() -> u64 {
    REARMED.load(Ordering::Relaxed)
}

/// Whether `[address, address + len)` lies where user memory may be.
fn plausible(address: usize, len: usize) -> Option<usize> {
    let last = address.checked_add(len.checked_sub(1)?)?;
    (address >= MIN_ADDRESS && last <= MAX_ADDRESS).then_some(last)
}

/// Whether `len` bytes at `address` could be read just now: one byte of
/// every page they span is read, and a page that cannot be is a refusal.
/// `false` while guarded reads are not [`active`].
pub fn probe(address: usize, len: usize) -> bool {
    if len == 0 {
        return true;
    }
    let Some(last) = plausible(address, len) else {
        return false;
    };
    if !active() {
        return false;
    }
    count_read();
    sys::touch(address, last)
}

/// Copies `out.len()` bytes from `address` into `out`, or answers `false`
/// (and `out` holds whatever was read before the fault). `false` while
/// guarded reads are not [`active`].
pub fn copy(address: usize, out: &mut [u8]) -> bool {
    // SAFETY: `out` is ours, writable for its length.
    unsafe { copy_to(address, out.as_mut_ptr(), out.len()) }
}

/// [`copy`] into `len` bytes at `dst`.
///
/// # Safety
///
/// `dst` is valid for writing `len` bytes (it need not be initialised).
unsafe fn copy_to(address: usize, dst: *mut u8, len: usize) -> bool {
    if len == 0 {
        return true;
    }
    if plausible(address, len).is_none() || !active() {
        return false;
    }
    count_read();
    // SAFETY: the caller's.
    unsafe { sys::copy(address, dst, len) }
}

/// A plain value read from `address`, or `None` where it cannot be. `T`
/// must be plain data (integers, floats, arrays of them): every bit
/// pattern a valid value, as for the `read_unaligned` it replaces.
pub fn read<T: Copy>(address: usize) -> Option<T> {
    let mut value = std::mem::MaybeUninit::<T>::uninit();
    // SAFETY: the bytes of a `MaybeUninit<T>` of our own; the copy writes
    // them all or answers `false`.
    let copied = unsafe {
        copy_to(
            address,
            value.as_mut_ptr().cast::<u8>(),
            std::mem::size_of::<T>(),
        )
    };
    if !copied {
        return None;
    }
    // SAFETY: every byte written by the copy; `T` is plain data (above).
    Some(unsafe { value.assume_init() })
}

#[cfg(all(windows, target_arch = "x86_64"))]
mod sys {
    use super::{FAULTS, REARMED};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use windows_sys::Win32::{
        Foundation::{
            EXCEPTION_ACCESS_VIOLATION, EXCEPTION_IN_PAGE_ERROR, STATUS_GUARD_PAGE_VIOLATION,
        },
        System::{
            Diagnostics::Debug::{
                AddVectoredExceptionHandler, EXCEPTION_CONTINUE_EXECUTION,
                EXCEPTION_CONTINUE_SEARCH, EXCEPTION_POINTERS,
            },
            Memory::{
                MEM_COMMIT, MEMORY_BASIC_INFORMATION, PAGE_GUARD, PAGE_NOACCESS, VirtualProtect,
                VirtualQuery,
            },
        },
    };

    // The routines. Leaf functions in the Windows x64 convention: arguments
    // in rcx, rdx, r8; the answer in rax; they touch only volatile
    // registers and never rsp, so they need no unwind data, and at any
    // instruction in them the return address is at [rsp]. That is what
    // lets one recovery stub serve them all: the handler moves the faulting
    // thread to `tpf3mp_guarded_recover`, which answers 0 and returns to
    // the caller.
    //
    // touch(first, last): reads the byte at `first`, then the first byte
    // of every following page up to the one holding `last` (inclusive);
    // `last` is below the kernel's half, so the page step never wraps.
    //
    // copy(dst, src, len): eight bytes at a time, then single bytes. The
    // loads may fault (a read fault: recovered); the stores go to the
    // caller's own buffer and a fault there is a write fault, which the
    // handler does not touch.
    std::arch::global_asm!(
        ".text",
        ".p2align 4",
        ".globl tpf3mp_guarded_touch",
        "tpf3mp_guarded_touch:",
        "movzx eax, byte ptr [rcx]",
        "or rcx, 0xfff",
        "inc rcx",
        "cmp rcx, rdx",
        "jbe tpf3mp_guarded_touch",
        "mov eax, 1",
        "ret",
        ".p2align 4",
        ".globl tpf3mp_guarded_copy",
        "tpf3mp_guarded_copy:",
        "cmp r8, 8",
        "jb .Ltpf3mp_guarded_bytes",
        ".Ltpf3mp_guarded_words:",
        "mov rax, qword ptr [rdx]",
        "mov qword ptr [rcx], rax",
        "add rdx, 8",
        "add rcx, 8",
        "sub r8, 8",
        "cmp r8, 8",
        "jae .Ltpf3mp_guarded_words",
        ".Ltpf3mp_guarded_bytes:",
        "test r8, r8",
        "jz .Ltpf3mp_guarded_done",
        ".Ltpf3mp_guarded_byte:",
        "movzx eax, byte ptr [rdx]",
        "mov byte ptr [rcx], al",
        "inc rdx",
        "inc rcx",
        "dec r8",
        "jnz .Ltpf3mp_guarded_byte",
        ".Ltpf3mp_guarded_done:",
        "mov eax, 1",
        "ret",
        ".globl tpf3mp_guarded_end",
        "tpf3mp_guarded_end:",
        ".globl tpf3mp_guarded_recover",
        "tpf3mp_guarded_recover:",
        "xor eax, eax",
        "ret",
        // range(out): the routines' first byte, their end, and the
        // recovery stub, taken here so no linker thunk can stand in.
        ".p2align 4",
        ".globl tpf3mp_guarded_range",
        "tpf3mp_guarded_range:",
        "lea rax, [rip + tpf3mp_guarded_touch]",
        "mov qword ptr [rcx], rax",
        "lea rax, [rip + tpf3mp_guarded_end]",
        "mov qword ptr [rcx + 8], rax",
        "lea rax, [rip + tpf3mp_guarded_recover]",
        "mov qword ptr [rcx + 16], rax",
        "ret",
    );

    unsafe extern "C" {
        fn tpf3mp_guarded_touch(first: usize, last: usize) -> usize;
        fn tpf3mp_guarded_copy(dst: *mut u8, src: usize, len: usize) -> usize;
        fn tpf3mp_guarded_range(out: *mut [usize; 3]);
    }

    /// The routines' `[begin, end)` and their recovery stub; set before the
    /// handler is added.
    static BEGIN: AtomicUsize = AtomicUsize::new(0);
    static END: AtomicUsize = AtomicUsize::new(0);
    static RECOVER: AtomicUsize = AtomicUsize::new(0);

    /// The routines' `[begin, end)` and their recovery stub.
    pub fn range() -> [usize; 3] {
        let mut out = [0; 3];
        // SAFETY: writes the three words of `out`, nothing else.
        unsafe { tpf3mp_guarded_range(&mut out) };
        out
    }

    pub fn install() -> bool {
        let [begin, end, recover] = range();
        if !(begin < end && end <= recover) {
            return false;
        }
        BEGIN.store(begin, Ordering::Release);
        END.store(end, Ordering::Release);
        RECOVER.store(recover, Ordering::Release);
        // SAFETY: a handler that lives as long as the process (the hook is
        // never unloaded) and acts only on faults inside the routines.
        let handle = unsafe { AddVectoredExceptionHandler(1, Some(handler)) };
        !handle.is_null()
    }

    pub fn touch(first: usize, last: usize) -> bool {
        // SAFETY: the handler is in (the caller checked `active`), so a
        // read fault answers 0 instead of crashing; nothing is written.
        unsafe { tpf3mp_guarded_touch(first, last) != 0 }
    }

    /// # Safety
    ///
    /// `dst` is valid for writing `len` bytes.
    pub unsafe fn copy(src: usize, dst: *mut u8, len: usize) -> bool {
        // SAFETY: as `touch`; the stores go to `dst`, the caller's.
        unsafe { tpf3mp_guarded_copy(dst, src, len) != 0 }
    }

    /// The exception's read-fault address, if it is a read fault one of the
    /// routines raised.
    ///
    /// # Safety
    ///
    /// `info` is the system's, for the duration of a vectored handler.
    unsafe fn ours(info: *mut EXCEPTION_POINTERS) -> Option<(i32, usize)> {
        // SAFETY: the caller's.
        let (record, context) = unsafe { (&*(*info).ExceptionRecord, &*(*info).ContextRecord) };
        let code = record.ExceptionCode;
        if code != EXCEPTION_ACCESS_VIOLATION
            && code != STATUS_GUARD_PAGE_VIOLATION
            && code != EXCEPTION_IN_PAGE_ERROR
        {
            return None;
        }
        let rip = context.Rip as usize;
        if rip < BEGIN.load(Ordering::Acquire) || rip >= END.load(Ordering::Acquire) {
            return None;
        }
        // The first parameter: 0 a read, 1 a write, 8 an execution.
        if record.NumberParameters < 2 || record.ExceptionInformation[0] != 0 {
            return None;
        }
        Some((code, record.ExceptionInformation[1]))
    }

    /// The vectored handler: a read fault in the routines resumes at the
    /// recovery stub; anything else is passed on untouched.
    unsafe extern "system" fn handler(info: *mut EXCEPTION_POINTERS) -> i32 {
        // SAFETY: the system's pointers, for this call.
        let Some((code, address)) = (unsafe { ours(info) }) else {
            return EXCEPTION_CONTINUE_SEARCH;
        };
        if code == STATUS_GUARD_PAGE_VIOLATION {
            rearm(address);
        }
        FAULTS.fetch_add(1, Ordering::Relaxed);
        // SAFETY: the system's context of the faulting thread, which is in
        // one of the routines: at the stub it returns 0 to their caller.
        unsafe { (*(*info).ContextRecord).Rip = RECOVER.load(Ordering::Acquire) as u64 };
        EXCEPTION_CONTINUE_EXECUTION
    }

    /// Puts back the guard the system took off the page holding `address`
    /// when a routine read it.
    fn rearm(address: usize) {
        let page = address & !0xfff;
        // SAFETY: plain data; VirtualQuery only writes into it.
        let mut info: MEMORY_BASIC_INFORMATION = unsafe { std::mem::zeroed() };
        let written = unsafe {
            VirtualQuery(
                page as *const _,
                &mut info,
                std::mem::size_of::<MEMORY_BASIC_INFORMATION>(),
            )
        };
        if written == 0
            || info.State != MEM_COMMIT
            || info.Protect & (PAGE_GUARD | PAGE_NOACCESS) != 0
            || info.Protect == 0
        {
            return;
        }
        let mut old = 0;
        // SAFETY: one committed page, its protection unchanged but for the
        // guard it had a moment ago.
        if unsafe { VirtualProtect(page as *const _, 1, info.Protect | PAGE_GUARD, &mut old) } != 0
        {
            REARMED.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        }
    }
}

#[cfg(not(all(windows, target_arch = "x86_64")))]
mod sys {
    pub fn install() -> bool {
        false
    }

    pub fn touch(_first: usize, _last: usize) -> bool {
        false
    }

    /// # Safety
    ///
    /// None needed: nothing is read or written.
    pub unsafe fn copy(_src: usize, _dst: *mut u8, _len: usize) -> bool {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_kill_switch_turns_them_off() {
        assert!(wanted(None));
        assert!(wanted(Some("1")));
        for off in ["0", "off", "OFF", " no "] {
            assert!(!wanted(Some(off)), "{off:?}");
        }
    }

    #[test]
    fn implausible_ranges_are_refused_without_a_read() {
        assert!(plausible(0, 8).is_none(), "the null page");
        assert!(plausible(MIN_ADDRESS - 8, 8).is_none());
        assert!(
            plausible(MAX_ADDRESS, 2).is_none(),
            "into the kernel's half"
        );
        assert!(plausible(usize::MAX - 4, 8).is_none(), "wraps");
        assert_eq!(plausible(MIN_ADDRESS, 8), Some(MIN_ADDRESS + 7));
        assert!(probe(0x10, 0), "zero bytes");
        assert!(copy(0x10, &mut []), "zero bytes");
    }
}

#[cfg(all(test, windows, target_arch = "x86_64"))]
mod windows_tests {
    use super::*;
    use windows_sys::Win32::System::Memory::{
        MEM_COMMIT, MEM_RELEASE, MEM_RESERVE, MEMORY_BASIC_INFORMATION, PAGE_GUARD, PAGE_NOACCESS,
        PAGE_READONLY, PAGE_READWRITE, VirtualAlloc, VirtualFree, VirtualProtect, VirtualQuery,
    };

    const PAGE: usize = 0x1000;

    /// `pages` pages of our own: the first `committed` committed
    /// read-write, the rest only reserved. Freed on drop.
    struct Pages {
        base: usize,
    }

    impl Pages {
        fn new(pages: usize, committed: usize) -> Self {
            // SAFETY: fresh memory of our own.
            let base =
                unsafe { VirtualAlloc(std::ptr::null(), pages * PAGE, MEM_RESERVE, PAGE_NOACCESS) }
                    as usize;
            assert_ne!(base, 0);
            if committed > 0 {
                // SAFETY: inside the reservation just made.
                let at = unsafe {
                    VirtualAlloc(
                        base as *const _,
                        committed * PAGE,
                        MEM_COMMIT,
                        PAGE_READWRITE,
                    )
                };
                assert_eq!(at as usize, base);
            }
            Self { base }
        }

        fn protect(&self, page: usize, protection: u32) {
            let mut old = 0;
            // SAFETY: a committed page of ours.
            let done = unsafe {
                VirtualProtect(
                    (self.base + page * PAGE) as *const _,
                    PAGE,
                    protection,
                    &mut old,
                )
            };
            assert_ne!(done, 0);
        }

        fn protection(&self, page: usize) -> u32 {
            // SAFETY: plain data; VirtualQuery only writes into it.
            let mut info: MEMORY_BASIC_INFORMATION = unsafe { std::mem::zeroed() };
            let written = unsafe {
                VirtualQuery(
                    (self.base + page * PAGE) as *const _,
                    &mut info,
                    std::mem::size_of::<MEMORY_BASIC_INFORMATION>(),
                )
            };
            assert_ne!(written, 0);
            info.Protect
        }
    }

    impl Drop for Pages {
        fn drop(&mut self) {
            // SAFETY: our reservation; nothing refers to it any more.
            unsafe { VirtualFree(self.base as *mut _, 0, MEM_RELEASE) };
        }
    }

    fn on() {
        assert_eq!(mode(), Mode::On, "the tests run without {ENV}");
    }

    #[test]
    fn readable_memory_reads_and_unreadable_memory_is_refused() {
        on();
        let words: Vec<u64> = (0..64).map(|i| i * 0x0101_0101).collect();
        let at = words.as_ptr() as usize;
        assert!(probe(at, 64 * 8));
        assert_eq!(read::<u64>(at + 8 * 5), Some(5 * 0x0101_0101));
        assert_eq!(read::<[u8; 3]>(at + 8), Some([1, 1, 1]), "odd sizes");
        let mut out = [0u8; 61];
        assert!(copy(at + 3, &mut out), "unaligned, words then bytes");
        // SAFETY: the vector's own bytes.
        let source = unsafe { std::slice::from_raw_parts((at + 3) as *const u8, 61) };
        assert_eq!(out[..], source[..]);
        assert_eq!(out[5..9], [1, 1, 1, 1]);
        let here = on as *const () as usize;
        assert!(probe(here, 16), "code");
        assert_eq!(read::<u32>(0x10), None, "null page, without a fault");
    }

    #[test]
    fn reserved_freed_and_no_access_pages_are_refused() {
        on();
        let pages = Pages::new(4, 2);
        assert!(probe(pages.base, 2 * PAGE));
        assert_eq!(read::<u64>(pages.base + 8), Some(0));
        assert!(!probe(pages.base + 2 * PAGE, 8), "reserved, not committed");
        assert_eq!(read::<u64>(pages.base + 3 * PAGE), None);
        pages.protect(1, PAGE_NOACCESS);
        assert!(!probe(pages.base + PAGE + 16, 1), "no access");
        assert_eq!(read::<u8>(pages.base + PAGE), None);
        assert!(probe(pages.base, 8), "the page before it is still fine");
        pages.protect(1, PAGE_READONLY);
        assert_eq!(read::<u64>(pages.base + PAGE), Some(0), "read-only reads");
        let freed = pages.base;
        drop(pages);
        assert!(!probe(freed, 8), "freed");
        assert_eq!(read::<u64>(freed), None);
    }

    #[test]
    fn a_range_into_an_uncommitted_page_is_refused_whole() {
        on();
        let pages = Pages::new(2, 1);
        let edge = pages.base + PAGE;
        for len in 1..=8 {
            assert!(probe(edge - len, len), "{len} bytes up to the edge");
            assert!(!probe(edge - len, len + 1), "{len} + 1 bytes over it");
            let mut out = vec![0u8; len + 1];
            assert!(!copy(edge - len, &mut out), "copy of {len} + 1");
            assert!(copy(edge - len, &mut out[..len]));
        }
        // An eight-byte word straddling the edge.
        assert_eq!(read::<u64>(edge - 4), None);
        assert_eq!(read::<u32>(edge - 4), Some(0));
        // A long range: every page is touched, not just the ends.
        let three = Pages::new(3, 3);
        three.protect(1, PAGE_NOACCESS);
        assert!(!probe(three.base, 3 * PAGE), "the middle page");
        assert!(probe(three.base + 2 * PAGE, PAGE));
    }

    #[test]
    fn a_guard_page_is_refused_and_keeps_its_guard() {
        on();
        let pages = Pages::new(2, 2);
        pages.protect(1, PAGE_READWRITE | PAGE_GUARD);
        let before = rearmed();
        assert!(!probe(pages.base + PAGE, 8));
        assert_ne!(pages.protection(1) & PAGE_GUARD, 0, "the guard is back");
        assert_eq!(
            read::<u64>(pages.base + PAGE + 8),
            None,
            "and refuses again"
        );
        assert_ne!(pages.protection(1) & PAGE_GUARD, 0);
        assert!(rearmed() >= before + 2);
        assert!(probe(pages.base, PAGE), "the page before it reads");
    }

    #[test]
    fn many_threads_fault_at_once() {
        on();
        let pages = std::sync::Arc::new(Pages::new(3, 2));
        pages.protect(1, PAGE_NOACCESS);
        let good = pages.base;
        let bad = [pages.base + PAGE, pages.base + 2 * PAGE];
        let threads: Vec<_> = (0..16)
            .map(|t| {
                let pages = pages.clone();
                std::thread::spawn(move || {
                    let _keep = &pages;
                    for i in 0..2_000 {
                        assert_eq!(read::<u64>(good + 8 * (i % 64)), Some(0), "thread {t}");
                        assert_eq!(read::<u64>(bad[(i + t) % 2] + 8), None, "thread {t}");
                        assert!(!probe(good, 2 * PAGE), "thread {t}");
                    }
                })
            })
            .collect();
        for thread in threads {
            thread.join().expect("no thread panicked");
        }
    }

    // A test routine outside the guarded range that faults like them,
    // and a stub for a later handler to resume it at (answering 2).
    std::arch::global_asm!(
        ".text",
        ".p2align 4",
        ".globl tpf3mp_guarded_test_load",
        "tpf3mp_guarded_test_load:",
        "movzx eax, byte ptr [rcx]",
        "ret",
        ".globl tpf3mp_guarded_test_recover",
        "tpf3mp_guarded_test_recover:",
        "mov eax, 2",
        "ret",
        ".globl tpf3mp_guarded_test_range",
        "tpf3mp_guarded_test_range:",
        "lea rax, [rip + tpf3mp_guarded_test_load]",
        "mov qword ptr [rcx], rax",
        "lea rax, [rip + tpf3mp_guarded_test_recover]",
        "mov qword ptr [rcx + 8], rax",
        "ret",
    );

    unsafe extern "C" {
        fn tpf3mp_guarded_test_load(at: usize) -> usize;
        fn tpf3mp_guarded_test_range(out: *mut [usize; 2]);
        fn tpf3mp_guarded_copy(dst: *mut u8, src: usize, len: usize) -> usize;
    }

    static SEEN_OUTSIDE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    static SEEN_WRITES: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

    /// Stands for the game's own handlers, behind ours: recovers the test
    /// routine's faults, and write faults in the guarded routines.
    unsafe extern "system" fn later_handler(
        info: *mut windows_sys::Win32::System::Diagnostics::Debug::EXCEPTION_POINTERS,
    ) -> i32 {
        use windows_sys::Win32::System::Diagnostics::Debug::{
            EXCEPTION_CONTINUE_EXECUTION, EXCEPTION_CONTINUE_SEARCH,
        };
        // SAFETY: the system's pointers, for this call.
        let (record, context) = unsafe { (&*(*info).ExceptionRecord, &mut *(*info).ContextRecord) };
        let mut test = [0; 2];
        // SAFETY: writes `test`.
        unsafe { tpf3mp_guarded_test_range(&mut test) };
        let [begin, end, recover] = sys::range();
        let rip = context.Rip as usize;
        if rip == test[0] {
            SEEN_OUTSIDE.fetch_add(1, Ordering::Relaxed);
            context.Rip = test[1] as u64;
            return EXCEPTION_CONTINUE_EXECUTION;
        }
        if (begin..end).contains(&rip) && record.ExceptionInformation[0] == 1 {
            SEEN_WRITES.fetch_add(1, Ordering::Relaxed);
            context.Rip = recover as u64;
            return EXCEPTION_CONTINUE_EXECUTION;
        }
        EXCEPTION_CONTINUE_SEARCH
    }

    #[test]
    fn faults_that_are_not_guarded_reads_go_on_to_the_next_handler() {
        use windows_sys::Win32::System::Diagnostics::Debug::{
            AddVectoredExceptionHandler, RemoveVectoredExceptionHandler,
        };
        on();
        // Added last: it sees only what ours passes on.
        // SAFETY: removed below; it acts only on the test's own faults.
        let handle = unsafe { AddVectoredExceptionHandler(0, Some(later_handler)) };
        assert!(!handle.is_null());
        let outside = SEEN_OUTSIDE.load(Ordering::Relaxed);
        // SAFETY: the later handler resumes the routine's fault.
        assert_eq!(unsafe { tpf3mp_guarded_test_load(0x10) }, 2, "not ours");
        assert_eq!(SEEN_OUTSIDE.load(Ordering::Relaxed), outside + 1);
        // A write fault in a guarded routine: not ours either.
        let writes = SEEN_WRITES.load(Ordering::Relaxed);
        let source = [7u8; 16];
        let pages = Pages::new(1, 1);
        pages.protect(0, PAGE_READONLY);
        // SAFETY: the destination is read-only on purpose; the later
        // handler resumes the write fault at the stub.
        let copied =
            unsafe { tpf3mp_guarded_copy(pages.base as *mut u8, source.as_ptr() as usize, 16) };
        assert_eq!(copied, 0);
        assert_eq!(SEEN_WRITES.load(Ordering::Relaxed), writes + 1);
        // Guarded reads still answer while it is in.
        assert_eq!(read::<u64>(0x7FFF_0000_0000), None);
        assert_eq!(SEEN_OUTSIDE.load(Ordering::Relaxed), outside + 1);
        // SAFETY: the handle just added.
        assert_ne!(unsafe { RemoveVectoredExceptionHandler(handle) }, 0);
    }

    /// The child process's environment variable for
    /// [`child_crashes_into_the_last_chance_filter`].
    const CHILD: &str = "TPF3MP_GUARDED_TEST_CHILD";
    /// The exit code the child's last-chance filter ends it with.
    const FILTER_EXIT: u32 = 0x5EF;

    #[test]
    fn a_stray_fault_still_crashes_into_the_games_filter() {
        // The game's crash reporter is a last-chance filter
        // (SetUnhandledExceptionFilter): a fault outside the routines must
        // still reach it. A child process faults for real; in this one it
        // would end the test run.
        let exe = std::env::current_exe().expect("the test binary");
        let out = std::process::Command::new(exe)
            .args([
                "--exact",
                "image::guarded::windows_tests::child_crashes_into_the_last_chance_filter",
                "--nocapture",
                "--test-threads=1",
            ])
            .env(CHILD, "1")
            .env_remove(ENV)
            .output()
            .expect("the child runs");
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert!(
            stdout.contains("guarded reads refused the null page"),
            "{stdout}"
        );
        assert_eq!(
            out.status.code(),
            Some(FILTER_EXIT as i32),
            "the filter ended it: {stdout}"
        );
    }

    #[test]
    fn child_crashes_into_the_last_chance_filter() {
        use windows_sys::Win32::System::{
            Diagnostics::Debug::{EXCEPTION_POINTERS, SetUnhandledExceptionFilter},
            Threading::ExitProcess,
        };
        if std::env::var_os(CHILD).is_none() {
            return;
        }
        unsafe extern "system" fn filter(_info: *const EXCEPTION_POINTERS) -> i32 {
            // SAFETY: ends this child process, as a crash reporter would.
            unsafe { ExitProcess(FILTER_EXIT) }
        }
        // SAFETY: the child's own filter, for the rest of its life.
        unsafe { SetUnhandledExceptionFilter(Some(filter)) };
        on();
        assert_eq!(read::<u64>(0x7FFF_0000_0000), None);
        println!("guarded reads refused the null page");
        // A fault outside the routines: the filter must end the process.
        // SAFETY: deliberately not; the filter ends the process first.
        let value = unsafe { std::ptr::read_volatile(0x7FFF_0000_0000usize as *const u64) };
        println!("unreachable: {value}");
    }
}
