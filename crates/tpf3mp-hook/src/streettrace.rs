//! The street trace (logging only; it changes nothing), off unless [`ENV`]
//! is `1` or `on` (docs/HOOKS.md, "The street trace").
//!
//! Rounds of 2026-10-02 on `twomptest`: at step 12771 (update 105104) the
//! size update of town 214465 grows a dead-end `town_old_small` street out
//! of node 261290, and its free end lands at (-2360.9, -20680.5) in some
//! games and (-2356.4, -20690.8) in others, with the update's every input
//! alike (the town trace). Both streets are 88 m long. The first leaves the
//! node at a right angle to the street through it; the second 7.3 degrees
//! off. That is the street developer's two passes (read with `tools/tpfre`):
//!
//! - `TownDeveloper::Develop` (`0x8dc240`) calls the street step
//!   (`0x967720`, our name) until it builds nothing. The step lists the
//!   town's street nodes (an octree query, sorted by distance to the town's
//!   centre) and tries each, first in the **block** pass and then in the
//!   **open** pass; the first street built ends the step.
//! - `StreetDeveloper::TryCandidate` (`0x967920`, our name; `rcx` the
//!   candidate, the node at `+0`, its x and y at `+4` and `+8`; `dl` 1 in
//!   the block pass) expands the node into directions (`Expand`, `0x968130`:
//!   along and at right angles to each street at the node, 88 m), and tries
//!   each. The block pass takes only a direction that points into a closed
//!   street loop around the node (`StreetLoopFactory::Extract`, at most 50
//!   streets a loop), exactly; the open pass only one that does not, turned
//!   by a random angle from a `minstd_rand` seeded with the node's position.
//!   So the right-angle street is the block pass's and the turned one the
//!   open pass's: in the games that got the turned street, the block pass
//!   refused the node's right-angle direction.
//!
//! Every refusal goes into a set (`0x963320`, a `std::set` insert; key: the
//! node, its streets, the pass, its loops, the direction's index), from one
//! of six places, each with its own slot in `TryCandidate`'s frame; the
//! slot names the reason ([`Reason`]). The trace logs, within the steps
//! [`STEPS_ENV`] names and for nodes in the box [`BOX_ENV`] names (each
//! unset: all):
//!
//! ```text
//! street: step <s> try <k> node <e> at <x>,<y> pass block|open -> built dir <dx>,<dy> end <x>,<y>
//! street: step <s> try <k> node <e> at <x>,<y> pass block|open -> no
//! street: step <s> try <k> reject node <e> edges <n> loops <n> dir <i> pass block|open reason <reason>
//! street: step <s> try <k> proposal errors <bytes>/<bytes> [<hex>] [<hex>]
//! ```
//!
//! `try <k>` numbers the tries within a step, so two games' lines pair up
//! by step and number. A `proposal` line is the street's proposal's error
//! state after `CreateProposalData` (`0xa1fd10`, from `0x9657c0`, which
//! builds a direction that passed the checks): both vectors empty builds.
//!
//! Before splicing, the try's return site must lie at its offset, the three
//! calls of the reject function must be where the profile's build has
//! them, and each site's bytes must be the expected ones; otherwise nothing
//! is spliced and the log says why. A panic switches it off.

#![allow(unsafe_code)]
#![cfg_attr(not(all(windows, target_arch = "x86_64")), allow(dead_code))]

use std::cell::Cell;
use std::fmt::Write as _;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use tpf3mp_hookcore::detour::{SavedRegs, Splice, SpliceHook};
use tpf3mp_hookcore::profile::ResolvedProfile;

use crate::image::Readable as Probe;
use crate::log;

/// `1` (or `on`) turns the trace on.
pub const ENV: &str = "TPF3MP_HOOK_STREET_TRACE";
/// `from-to`, the steps traced (both included); unset, every step.
pub const STEPS_ENV: &str = "TPF3MP_HOOK_STREET_TRACE_STEPS";
/// `x0,y0,x1,y1`, the box the tried node must lie in; unset, anywhere.
pub const BOX_ENV: &str = "TPF3MP_HOOK_STREET_TRACE_BOX";

pub use crate::build_data::native::streettrace::ERRORS;
pub use crate::build_data::native::streettrace::REJECT;
pub use crate::build_data::native::streettrace::TRY;
pub use crate::build_data::native::streettrace::TRY_RETURN;

pub use crate::build_data::native::streettrace::REJECT_CALLS_AT;
pub use crate::build_data::native::streettrace::TRY_RETURN_AT;

pub use crate::build_data::native::streettrace::ERRORS_EXPECTED;
pub use crate::build_data::native::streettrace::ERRORS_STEAL;
pub use crate::build_data::native::streettrace::REJECT_EXPECTED;
pub use crate::build_data::native::streettrace::REJECT_STEAL;
pub use crate::build_data::native::streettrace::TRY_EXPECTED;
pub use crate::build_data::native::streettrace::TRY_RETURN_EXPECTED;
pub use crate::build_data::native::streettrace::TRY_RETURN_STEAL;
pub use crate::build_data::native::streettrace::TRY_STEAL;

pub use crate::build_data::native::streettrace::ERROR_BYTES;
use crate::build_data::native::streettrace::ERRORS_FIRST;
use crate::build_data::native::streettrace::ERRORS_SECOND;
use crate::build_data::native::streettrace::TRY_DIR;
use crate::build_data::native::streettrace::TRY_END;
use crate::build_data::native::streettrace::TRY_PASS;

/// Why `TryCandidate` refused a direction: the frame slot (from its
/// `rbp`) it hands the reject function names it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reason {
    /// `0x966470` refused the direction (the branches' angles,
    /// `CheckBranchesRec`).
    Branches,
    /// Block pass: the direction leaves every closed street loop.
    NotInBlock,
    /// Open pass: the direction points into a closed street loop.
    InBlock,
    /// The street's end is off the map (`0x388e60`).
    OffMap,
    /// The snapped end failed `0x9689c0` (block pass, snapped to a street).
    Snapped,
    /// The street did not build (`0x9692c0`: water, its proposal, or its
    /// errors).
    Build,
}

impl Reason {
    /// From the slot's offset to `TryCandidate`'s `rbp`.
    pub fn from_slot(offset: u64) -> Option<Self> {
        Some(match offset {
            0x08 => Self::Branches,
            0x18 => Self::NotInBlock,
            0x28 => Self::InBlock,
            0x38 => Self::OffMap,
            0x48 => Self::Snapped,
            0x58 => Self::Build,
            _ => return None,
        })
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Branches => "branches",
            Self::NotInBlock => "not-in-block",
            Self::InBlock => "in-block",
            Self::OffMap => "off-map",
            Self::Snapped => "snapped",
            Self::Build => "build",
        }
    }
}

/// What the trace covers.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Filter {
    pub steps: Option<(u64, u64)>,
    /// `[x0, y0, x1, y1]`, `x0 <= x1`, `y0 <= y1`.
    pub rect: Option<[f64; 4]>,
}

impl Filter {
    /// From [`ENV`]'s, [`STEPS_ENV`]'s and [`BOX_ENV`]'s values: `Ok(None)`
    /// when the trace is off, `Err(why)` for a value that does not read (the
    /// trace is then off).
    pub fn from_env(
        on: Option<&str>,
        steps: Option<&str>,
        rect: Option<&str>,
    ) -> Result<Option<Self>, String> {
        if !crate::towntrace::wanted(on) {
            return Ok(None);
        }
        let steps = match steps.map(str::trim).filter(|v| !v.is_empty()) {
            None => None,
            Some(value) => Some(crate::lanedump::parse_step_range(value).ok_or_else(|| {
                format!(
                    "{STEPS_ENV}={value} is not a step range such as 12750-12800; the street trace is off"
                )
            })?),
        };
        let rect = match rect.map(str::trim).filter(|v| !v.is_empty()) {
            None => None,
            Some(value) => {
                let corners: Option<Vec<f64>> =
                    value.split(',').map(|n| n.trim().parse().ok()).collect();
                let corners = corners
                    .filter(|c| c.len() == 4 && c.iter().all(|v| v.is_finite()))
                    .ok_or_else(|| {
                        format!(
                            "{BOX_ENV}={value} is not four numbers x0,y0,x1,y1; the street trace is off"
                        )
                    })?;
                Some([
                    corners[0].min(corners[2]),
                    corners[1].min(corners[3]),
                    corners[0].max(corners[2]),
                    corners[1].max(corners[3]),
                ])
            }
        };
        Ok(Some(Self { steps, rect }))
    }

    /// Whether a try at `step` (`None` outside a room's update) of a node
    /// at `(x, y)` is traced.
    pub fn covers(&self, step: Option<u64>, x: f32, y: f32) -> bool {
        let in_steps = match (self.steps, step) {
            (None, _) => true,
            (Some((from, to)), Some(step)) => (from..=to).contains(&step),
            (Some(_), None) => false,
        };
        let in_box = self.rect.is_none_or(|[x0, y0, x1, y1]| {
            let (x, y) = (f64::from(x), f64::from(y));
            (x0..=x1).contains(&x) && (y0..=y1).contains(&y)
        });
        in_steps && in_box
    }

    pub fn describe(&self) -> String {
        let steps = match self.steps {
            Some((from, to)) => format!("steps {from} to {to}"),
            None => "every step".to_owned(),
        };
        let place = match self.rect {
            Some([x0, y0, x1, y1]) => format!("nodes in x {x0}..{x1}, y {y0}..{y1}"),
            None => "every node".to_owned(),
        };
        format!("{steps}, {place}")
    }
}

/// The try in progress on this thread.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Try {
    pub step: Option<u64>,
    pub number: u32,
    pub node: i32,
    pub x: f32,
    pub y: f32,
    pub block: bool,
}

fn opt<T: std::fmt::Display>(value: Option<T>, none: &str) -> String {
    value.map_or_else(|| none.to_owned(), |v| v.to_string())
}

fn pass(block: bool) -> &'static str {
    if block { "block" } else { "open" }
}

fn head(t: &Try) -> String {
    format!("street: step {} try {}", opt(t.step, "-"), t.number)
}

/// A try's line: built with its direction and end, or not.
pub fn try_line(t: &Try, built: Option<([f32; 2], [f32; 2])>) -> String {
    let mut line = format!(
        "{} node {} at {:?},{:?} pass {} -> ",
        head(t),
        t.node,
        t.x,
        t.y,
        pass(t.block)
    );
    match built {
        Some(([dx, dy], [ex, ey])) => {
            let _ = write!(line, "built dir {dx:?},{dy:?} end {ex:?},{ey:?}");
        }
        None => line.push_str("no"),
    }
    line
}

/// The reject function's key: the node, its streets, the pass, its loops
/// and the direction's index.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Key {
    pub node: i32,
    pub edges: i32,
    pub block: bool,
    pub loops: i32,
    pub dir: i32,
}

/// A refusal's line.
pub fn reject_line(t: &Try, key: &Key, reason: Option<Reason>) -> String {
    format!(
        "{} reject node {} edges {} loops {} dir {} pass {} reason {}",
        head(t),
        key.node,
        key.edges,
        key.loops,
        key.dir,
        pass(key.block),
        reason.map_or("?", Reason::name),
    )
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// A proposal's error state: each vector's length in bytes and its first
/// bytes.
pub fn errors_line(t: &Try, first: (u64, &[u8]), second: (u64, &[u8])) -> String {
    format!(
        "{} proposal errors {}/{} [{}] [{}]",
        head(t),
        first.0,
        second.0,
        hex(first.1),
        hex(second.1)
    )
}

/// The tries numbered within a step.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Counter {
    step: Option<u64>,
    next: u32,
}

impl Counter {
    pub fn next(&mut self, step: Option<u64>) -> u32 {
        if self.step != step {
            *self = Self { step, next: 0 };
        }
        self.next += 1;
        self.next
    }
}

static ON: AtomicBool = AtomicBool::new(false);
static BROKEN: AtomicBool = AtomicBool::new(false);
static TRY_AT: AtomicU64 = AtomicU64::new(0);
static FILTER: OnceLock<Filter> = OnceLock::new();

thread_local! {
    static CURRENT: Cell<Option<Try>> = const { Cell::new(None) };
    static COUNTER: Cell<Counter> = const { Cell::new(Counter { step: None, next: 0 }) };
}

fn guarded(body: impl FnOnce()) {
    if BROKEN.load(Ordering::Acquire) || !ON.load(Ordering::Acquire) {
        return;
    }
    if std::panic::catch_unwind(std::panic::AssertUnwindSafe(body)).is_err() {
        BROKEN.store(true, Ordering::Release);
        log::line("street trace: panicked on the game's thread; switched off for this game");
    }
}

/// `TryCandidate`'s entry: `rcx` the candidate, `dl` the pass.
unsafe extern "system" fn try_hook(regs: *mut SavedRegs) {
    guarded(|| {
        // SAFETY: the stub's block, held until the hook returns.
        let regs = unsafe { &*regs };
        CURRENT.with(|c| c.set(None));
        let Some(filter) = FILTER.get() else {
            return;
        };
        let mut probe = Probe::new();
        let cand = regs.rcx;
        let (Some(node), Some(x), Some(y)) = (
            probe.read::<i32>(cand),
            probe.read::<f32>(cand.wrapping_add(4)),
            probe.read::<f32>(cand.wrapping_add(8)),
        ) else {
            return;
        };
        let step = crate::seeds::current_step();
        if !filter.covers(step, x, y) {
            return;
        }
        let number = COUNTER.with(|c| {
            let mut counter = c.get();
            let n = counter.next(step);
            c.set(counter);
            n
        });
        CURRENT.with(|c| {
            c.set(Some(Try {
                step,
                number,
                node,
                x,
                y,
                block: regs.rdx as u8 != 0,
            }))
        });
    });
}

/// `TryCandidate`'s return, past the cookie check: `al` the result, `rbp`
/// its frame.
unsafe extern "system" fn try_return_hook(regs: *mut SavedRegs) {
    guarded(|| {
        let rsp = SavedRegs::rsp(regs);
        // SAFETY: the stub's block, held until the hook returns.
        let regs = unsafe { &*regs };
        let Some(t) = CURRENT.with(|c| c.take()) else {
            return;
        };
        let mut probe = Probe::new();
        // The pass the frame holds is the one the entry saw.
        if probe.read::<u8>(rsp + TRY_PASS).map(|p| p != 0) != Some(t.block) {
            return;
        }
        let built = (regs.rax as u8 != 0)
            .then(|| {
                let rbp = regs.rbp;
                Some((
                    [
                        probe.read::<f32>(rbp + TRY_DIR)?,
                        probe.read::<f32>(rbp + TRY_DIR + 4)?,
                    ],
                    [
                        probe.read::<f32>(rbp + TRY_END)?,
                        probe.read::<f32>(rbp + TRY_END + 4)?,
                    ],
                ))
            })
            .flatten();
        if regs.rax as u8 != 0 && built.is_none() {
            log::line(&format!("{} built, unread", head(&t)));
            return;
        }
        log::line(&try_line(&t, built));
    });
}

/// The reject function's entry: `rbp` still `TryCandidate`'s, `rdx` the
/// slot, `r8` the key, the return address at `[rsp]`.
unsafe extern "system" fn reject_hook(regs: *mut SavedRegs) {
    guarded(|| {
        let rsp = SavedRegs::rsp(regs);
        // SAFETY: the stub's block, held until the hook returns.
        let regs = unsafe { &*regs };
        let Some(t) = CURRENT.with(Cell::get) else {
            return;
        };
        let mut probe = Probe::new();
        let try_at = TRY_AT.load(Ordering::Relaxed);
        let from_try = probe.read::<u64>(rsp).is_some_and(|ret| {
            REJECT_CALLS_AT
                .iter()
                .any(|at| ret == try_at.wrapping_add(at + 5))
        });
        if !from_try {
            return;
        }
        let k = regs.r8;
        let (Some(node), Some(edges), Some(block), Some(loops), Some(dir)) = (
            probe.read::<i32>(k),
            probe.read::<i32>(k.wrapping_add(4)),
            probe.read::<u8>(k.wrapping_add(8)),
            probe.read::<i32>(k.wrapping_add(0xc)),
            probe.read::<i32>(k.wrapping_add(0x10)),
        ) else {
            return;
        };
        let key = Key {
            node,
            edges,
            block: block != 0,
            loops,
            dir,
        };
        let reason = regs.rdx.checked_sub(regs.rbp).and_then(Reason::from_slot);
        log::line(&reject_line(&t, &key, reason));
    });
}

/// After `CreateProposalData`, in `0x9657c0`: the two error vectors at
/// `[rbp+0x998]` and `[rbp+0x9b0]`.
unsafe extern "system" fn errors_hook(regs: *mut SavedRegs) {
    guarded(|| {
        // SAFETY: the stub's block, held until the hook returns.
        let regs = unsafe { &*regs };
        let Some(t) = CURRENT.with(Cell::get) else {
            return;
        };
        let mut probe = Probe::new();
        let mut vector = |at: u64| -> Option<(u64, Vec<u8>)> {
            let begin = probe.read::<u64>(regs.rbp.checked_add(at)?)?;
            let end = probe.read::<u64>(regs.rbp.checked_add(at + 8)?)?;
            let len = end.checked_sub(begin)?;
            let take = len.min(ERROR_BYTES as u64) as usize;
            let mut bytes = Vec::with_capacity(take);
            for i in 0..take as u64 {
                bytes.push(probe.read::<u8>(begin + i)?);
            }
            Some((len, bytes))
        };
        let (Some(first), Some(second)) = (vector(ERRORS_FIRST), vector(ERRORS_SECOND)) else {
            log::line(&format!("{} proposal errors unread", head(&t)));
            return;
        };
        log::line(&errors_line(&t, (first.0, &first.1), (second.0, &second.1)));
    });
}

/// The sites' layout: the return site at its offset in `TryCandidate`, and
/// the three calls there (`calls`, their five bytes each) calls of the
/// reject function.
pub fn check_layout(
    try_at: u64,
    try_return: u64,
    reject: u64,
    calls: &[Option<[u8; 5]>; 3],
) -> Result<(), String> {
    if try_return != try_at.wrapping_add(TRY_RETURN_AT) {
        return Err(format!(
            "{TRY_RETURN} at {try_return:#x} is not {TRY}+{TRY_RETURN_AT:#x}"
        ));
    }
    for (at, bytes) in REJECT_CALLS_AT.iter().zip(calls) {
        let call_at = try_at.wrapping_add(*at);
        let Some(bytes) = bytes.filter(|b| b[0] == 0xE8) else {
            return Err(format!("{TRY}+{at:#x} is not a call"));
        };
        let rel = i32::from_le_bytes([bytes[1], bytes[2], bytes[3], bytes[4]]);
        let target = call_at.wrapping_add(5).wrapping_add_signed(i64::from(rel));
        if target != reject {
            return Err(format!(
                "{TRY}+{at:#x} calls {target:#x}, not {REJECT} at {reject:#x}"
            ));
        }
    }
    Ok(())
}

fn splice(at: u64, expected: &[u8], steal: usize, hook: SpliceHook) -> Result<Splice, String> {
    // SAFETY: a site the profile resolved and the layout check placed in
    // its function, installed before any world exists; nothing branches
    // into the stolen bytes past the first (tpfre, noted in the profile);
    // the hooks only read and never unwind (`guarded`).
    unsafe { Splice::install(at as usize as *mut u8, expected, steal, hook) }
        .map_err(|e| format!("{at:#x}: {e}"))
}

/// Installs the trace when [`ENV`] asks for it. Returns the lines for
/// hook.log.
pub fn install(resolved: &ResolvedProfile) -> Vec<String> {
    let filter = Filter::from_env(
        std::env::var(ENV).ok().as_deref(),
        std::env::var(STEPS_ENV).ok().as_deref(),
        std::env::var(BOX_ENV).ok().as_deref(),
    );
    install_with(resolved, filter)
}

pub fn install_with(
    resolved: &ResolvedProfile,
    filter: Result<Option<Filter>, String>,
) -> Vec<String> {
    let filter = match filter {
        Ok(Some(filter)) => filter,
        Ok(None) => {
            return vec![format!(
                "street trace: off ({ENV} is not 1); the town street developer's tries are not logged"
            )];
        }
        Err(why) => return vec![format!("street trace: {why}")],
    };
    let (Some(try_at), Some(ret), Some(reject), Some(errors)) = (
        resolved.get(TRY),
        resolved.get(TRY_RETURN),
        resolved.get(REJECT),
        resolved.get(ERRORS),
    ) else {
        return vec![format!(
            "street trace: not traced, the profile lacks {TRY}, {TRY_RETURN}, {REJECT} or {ERRORS}"
        )];
    };
    let calls = REJECT_CALLS_AT.map(|at| {
        let call_at = try_at.address.wrapping_add(at);
        crate::image::readable(call_at as usize, 5).then(|| {
            // SAFETY: five readable bytes of the game's code.
            unsafe { std::ptr::read_unaligned(call_at as usize as *const [u8; 5]) }
        })
    });
    if let Err(why) = check_layout(try_at.address, ret.address, reject.address, &calls) {
        return vec![format!("street trace: not traced, {why}")];
    }
    let sites: [(u64, &[u8], usize, SpliceHook); 4] = [
        (try_at.address, &TRY_EXPECTED, TRY_STEAL, try_hook),
        (
            ret.address,
            &TRY_RETURN_EXPECTED,
            TRY_RETURN_STEAL,
            try_return_hook,
        ),
        (reject.address, &REJECT_EXPECTED, REJECT_STEAL, reject_hook),
        (errors.address, &ERRORS_EXPECTED, ERRORS_STEAL, errors_hook),
    ];
    let mut installed = Vec::new();
    for (at, expected, steal, hook) in sites {
        match splice(at, expected, steal, hook) {
            Ok(s) => installed.push(s),
            Err(why) => {
                for s in installed {
                    // SAFETY: as installed, no world runs yet.
                    let _ = unsafe { s.detach() };
                }
                return vec![format!("street trace: not traced, {why}")];
            }
        }
    }
    let _kept = std::mem::ManuallyDrop::new(installed);
    TRY_AT.store(try_at.address, Ordering::Relaxed);
    let described = filter.describe();
    let _ = FILTER.set(filter);
    ON.store(true, Ordering::Release);
    vec![format!(
        "street trace: {TRY} at {:#x} spliced at its entry and +{TRY_RETURN_AT:#x}, {REJECT} at {:#x} and {ERRORS} at {:#x}, for {described}: `street:` lines (logging only)",
        try_at.address, reject.address, errors.address
    )]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t() -> Try {
        Try {
            step: Some(12_771),
            number: 3,
            node: 261_290,
            x: -2439.249,
            y: -20720.467,
            block: true,
        }
    }

    #[test]
    fn the_lines_say_the_try_and_why() {
        assert_eq!(
            try_line(&t(), Some(([0.5, 0.25], [-2360.861, -20680.475]))),
            "street: step 12771 try 3 node 261290 at -2439.249,-20720.467 pass block -> built dir 0.5,0.25 end -2360.861,-20680.475"
        );
        let open = Try {
            block: false,
            step: None,
            ..t()
        };
        assert_eq!(
            try_line(&open, None),
            "street: step - try 3 node 261290 at -2439.249,-20720.467 pass open -> no"
        );
        let key = Key {
            node: 261_290,
            edges: 2,
            block: true,
            loops: 1,
            dir: 4,
        };
        assert_eq!(
            reject_line(&t(), &key, Some(Reason::NotInBlock)),
            "street: step 12771 try 3 reject node 261290 edges 2 loops 1 dir 4 pass block reason not-in-block"
        );
        assert!(reject_line(&t(), &key, None).ends_with("reason ?"));
        assert_eq!(
            errors_line(&t(), (8, &[1, 0, 0, 0, 0xab, 0, 0, 0]), (0, &[])),
            "street: step 12771 try 3 proposal errors 8/0 [01000000ab000000] []"
        );
    }

    #[test]
    fn the_frame_slots_name_the_reasons() {
        let all = [0x08, 0x18, 0x28, 0x38, 0x48, 0x58].map(Reason::from_slot);
        assert_eq!(
            all.map(|r| r.map(Reason::name)),
            [
                Some("branches"),
                Some("not-in-block"),
                Some("in-block"),
                Some("off-map"),
                Some("snapped"),
                Some("build"),
            ]
        );
        assert_eq!(Reason::from_slot(0x10), None);
        assert_eq!(Reason::from_slot(0x68), None);
    }

    #[test]
    fn the_filter_reads_and_covers() {
        assert_eq!(Filter::from_env(None, None, None), Ok(None));
        assert_eq!(Filter::from_env(Some("0"), Some("1-2"), None), Ok(None));
        let all = Filter::from_env(Some("1"), None, None).unwrap().unwrap();
        assert!(all.covers(None, 0.0, 0.0) && all.covers(Some(5), 1e6, -1e6));
        let f = Filter::from_env(
            Some("on"),
            Some("12700-12850"),
            Some("-2260,-20580,-2460,-20790"),
        )
        .unwrap()
        .unwrap();
        assert_eq!(f.rect, Some([-2460.0, -20790.0, -2260.0, -20580.0]));
        assert!(f.covers(Some(12_771), -2439.2, -20720.5));
        assert!(!f.covers(Some(12_699), -2439.2, -20720.5));
        assert!(!f.covers(None, -2439.2, -20720.5));
        assert!(!f.covers(Some(12_771), -2000.0, -20720.5));
        assert!(f.describe().contains("steps 12700 to 12850"));
        assert!(
            Filter::from_env(Some("1"), Some("x"), None)
                .unwrap_err()
                .contains(STEPS_ENV)
        );
        assert!(
            Filter::from_env(Some("1"), None, Some("1,2,3"))
                .unwrap_err()
                .contains(BOX_ENV)
        );
    }

    #[test]
    fn tries_are_numbered_within_a_step() {
        let mut c = Counter::default();
        assert_eq!(c.next(Some(7)), 1);
        assert_eq!(c.next(Some(7)), 2);
        assert_eq!(c.next(Some(8)), 1);
        assert_eq!(c.next(None), 1);
        assert_eq!(c.next(None), 2);
    }

    #[test]
    fn the_layout_must_be_the_release_builds() {
        let try_at = 0x0001_4096_7920_u64;
        let reject = 0x0001_4096_3320_u64;
        let call = |at: u64, target: u64| {
            let rel = (target as i64 - (try_at + at + 5) as i64) as i32;
            let mut bytes = [0xE8, 0, 0, 0, 0];
            bytes[1..].copy_from_slice(&rel.to_le_bytes());
            Some(bytes)
        };
        let calls = REJECT_CALLS_AT.map(|at| call(at, reject));
        let ret = try_at + TRY_RETURN_AT;
        assert_eq!(check_layout(try_at, ret, reject, &calls), Ok(()));
        assert!(check_layout(try_at, ret + 1, reject, &calls).is_err());
        let mut wrong = calls;
        wrong[1] = call(REJECT_CALLS_AT[1], reject + 0x10);
        assert!(
            check_layout(try_at, ret, reject, &wrong)
                .unwrap_err()
                .contains("calls")
        );
        let mut missing = calls;
        missing[2] = Some([0x90; 5]);
        assert!(check_layout(try_at, ret, reject, &missing).is_err());
        missing[2] = None;
        assert!(check_layout(try_at, ret, reject, &missing).is_err());
    }

    #[test]
    fn nothing_installs_when_off_or_without_the_targets() {
        let resolved = ResolvedProfile {
            name: "empty".into(),
            targets: Vec::new(),
            absent_optional: Vec::new(),
        };
        let lines = install_with(&resolved, Ok(None));
        assert_eq!(lines.len(), 1);
        assert!(lines[0].contains(ENV), "{lines:?}");
        let lines = install_with(&resolved, Err("bad".into()));
        assert_eq!(lines, vec!["street trace: bad".to_owned()]);
        let filter = Filter {
            steps: None,
            rect: None,
        };
        let lines = install_with(&resolved, Ok(Some(filter)));
        assert!(lines[0].contains("not traced"), "{lines:?}");
        assert!(!ON.load(Ordering::Acquire));
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
        assert_eq!(target(TRY).prologue, TRY_EXPECTED[..TRY_STEAL].to_vec());
        assert_eq!(
            target(TRY_RETURN).prologue,
            TRY_RETURN_EXPECTED[..TRY_RETURN_STEAL].to_vec()
        );
        assert_eq!(
            target(REJECT).prologue,
            REJECT_EXPECTED[..REJECT_STEAL].to_vec()
        );
        assert_eq!(
            target(ERRORS).prologue,
            ERRORS_EXPECTED[..ERRORS_STEAL].to_vec()
        );
        for name in [TRY, TRY_RETURN, REJECT, ERRORS] {
            assert!(!target(name).required, "{name} is optional");
        }
    }
}
