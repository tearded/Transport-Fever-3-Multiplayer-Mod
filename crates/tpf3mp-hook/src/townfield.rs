//! The town street field's cache fix (`town-field-cache`; on unless
//! [`TOGGLE_ENV`] is `0` or `off`; docs/HOOKS.md, "The town street field").
//!
//! Soak 4 of 2026-10-02 on save twomptest: every street try at step 12771
//! was alike in three games but one, the open pass's build of node 261290,
//! whose direction came out at 27.03 degrees in two games and 19.72 in the
//! third. The open pass turns a direction by a draw from a generator seeded
//! by the node's position (alike everywhere) and then, when the town has a
//! street field, bends the street's end towards it
//! (`StreetDeveloper::TryCandidate`, `0x967920`, at `0x967dc8..0x967ebc`):
//!
//! - `StreetField::At` (`0x2b76a90`, our name; `rcx` the field, `r8` the
//!   point) answers the field's two axes at the street's end and the
//!   distance to its nearest source: a sum, over the field's sources
//!   (`[field]..[field+8]`, 16 bytes each), of each source's axis weighted
//!   by `exp(-d/100)`, within the field's radius (`0x2b769a0`).
//! - It caches the answer in a `std::map` at `field+0x18`, keyed by the
//!   point's 50 m cell (`round(x/50+0.5)`, likewise y), and answers a later
//!   point in the same cell from the cache: the answer computed at the
//!   **first point that asked in that cell**, not at this one.
//! - The field is reached through the town developer's context
//!   (`GameState+0x200`; `[[ctx+0x1f0]+8]+0x30`, `0x95a080`), one context
//!   per `GameState`: which points asked in a cell before, in this game and
//!   on whichever buffer ran those updates, decides the answer. No lane
//!   reads the cache.
//! - `0x2b76820` then snaps the street's direction to whichever of the two
//!   axes lies within its angle, or leaves it: two answers, two angles.
//!
//! The fix splices the lookup's end test (`0x2b76b4a`, `cmp byte
//! [r9+0x19], 0`, the found node's nil flag) and points `r9` at the map's
//! head (`r10`, whose nil flag is set), so every call takes the miss path:
//! the answer is computed at the point asked, from the sources, and the
//! engine's own insert (`0x2b76660`) runs as before. The answer is then a
//! function of the point and the sources alone.

#![allow(unsafe_code)]
#![cfg_attr(not(all(windows, target_arch = "x86_64")), allow(dead_code))]

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use tpf3mp_hookcore::detour::{SavedRegs, Splice};
use tpf3mp_hookcore::profile::ResolvedProfile;

use crate::image::Readable as Probe;
use crate::log;

pub const FIX: &str = "town-field-cache";
/// Set to `0` (or `off`), the site stays out: the field answers from its
/// cache, as the game does.
pub const TOGGLE_ENV: &str = "TPF3MP_HOOK_TOWN_FIELD_CACHE";

pub use crate::build_data::native::townfield::EXPECTED;
pub use crate::build_data::native::townfield::FIELD;
pub use crate::build_data::native::townfield::MISS_AT;
use crate::build_data::native::townfield::NIL;
pub use crate::build_data::native::townfield::SITE;
pub use crate::build_data::native::townfield::SITE_AT;
pub use crate::build_data::native::townfield::STEAL;

static BROKEN: AtomicBool = AtomicBool::new(false);
static CALLS: AtomicU64 = AtomicU64::new(0);
static FOUND: AtomicU64 = AtomicU64::new(0);
static REFUSED: AtomicU64 = AtomicU64::new(0);

/// What the hook puts in the site's `r9` (the lookup's node), given `r10`
/// (the map's head) and the head's nil flag: `Some(head)` to force the
/// miss, `None` to leave the lookup alone (the head does not look like
/// one).
pub fn forced(r10: u64, head_nil: Option<u8>) -> Option<u64> {
    (r10 != 0 && head_nil.is_some_and(|nil| nil != 0)).then_some(r10)
}

/// The site's layout: at its offset in `At`, and its `jne` (`rel8`, the
/// byte after the stolen five) reaching the miss path.
pub fn check_layout(field: u64, site: u64, bytes: Option<[u8; 7]>) -> Result<(), String> {
    if site != field.wrapping_add(SITE_AT) {
        return Err(format!("{SITE} at {site:#x} is not {FIELD}+{SITE_AT:#x}"));
    }
    let Some(bytes) = bytes else {
        return Err(format!("{SITE} at {site:#x} is unreadable"));
    };
    if bytes[5] != 0x75 {
        return Err(format!("{SITE}+5 is not a jne"));
    }
    let target = site
        .wrapping_add(7)
        .wrapping_add_signed(i64::from(bytes[6] as i8));
    if target != field.wrapping_add(MISS_AT) {
        return Err(format!(
            "{SITE}'s jne reaches {target:#x}, not {FIELD}+{MISS_AT:#x}"
        ));
    }
    Ok(())
}

/// What installing came to, for hook.log.
pub fn outcome_line(installed: bool, reason: &str) -> String {
    if installed {
        format!("order fix {FIX}: installed ({reason})")
    } else {
        format!("order fix {FIX}: off, {reason}")
    }
}

unsafe extern "system" fn hook(regs: *mut SavedRegs) {
    if BROKEN.load(Ordering::Acquire) {
        return;
    }
    let body = || {
        // SAFETY: the stub's block, held until the hook returns.
        let regs = unsafe { &mut *regs };
        let n = CALLS.fetch_add(1, Ordering::Relaxed) + 1;
        let mut probe = Probe::new();
        let nil = probe.read::<u8>(regs.r10.wrapping_add(NIL));
        match forced(regs.r10, nil) {
            Some(head) => {
                if regs.r9 != head {
                    FOUND.fetch_add(1, Ordering::Relaxed);
                }
                regs.r9 = head;
            }
            None => {
                if REFUSED.fetch_add(1, Ordering::Relaxed) == 0 {
                    log::line(&format!(
                        "order fix {FIX}: the cache's head at {:#x} does not read as one; that lookup left to the game",
                        regs.r10
                    ));
                }
            }
        }
        if n == 1 || n.is_multiple_of(1 << 12) {
            log::line(&format!(
                "order fix {FIX}: alive, calls={n} cache-entries-passed={} refused={}",
                FOUND.load(Ordering::Relaxed),
                REFUSED.load(Ordering::Relaxed)
            ));
        }
    };
    if std::panic::catch_unwind(std::panic::AssertUnwindSafe(body)).is_err() {
        BROKEN.store(true, Ordering::Release);
        log::line(&format!(
            "order fix {FIX}: panicked on the game's thread; switched off for this game"
        ));
    }
}

/// Installs the fix unless [`TOGGLE_ENV`] says no. Returns the line for
/// hook.log.
pub fn install(resolved: &ResolvedProfile) -> String {
    install_with(
        resolved,
        crate::ticks::wanted(std::env::var(TOGGLE_ENV).ok().as_deref()),
    )
}

pub fn install_with(resolved: &ResolvedProfile, wanted: bool) -> String {
    if !wanted {
        return outcome_line(
            false,
            &format!("{TOGGLE_ENV} says so; the town street field answers from its cache"),
        );
    }
    let (Some(field), Some(site)) = (resolved.get(FIELD), resolved.get(SITE)) else {
        return outcome_line(false, &format!("the profile lacks {FIELD} or {SITE}"));
    };
    let bytes = crate::image::readable(site.address as usize, EXPECTED.len()).then(|| {
        // SAFETY: seven readable bytes of the game's code.
        unsafe { std::ptr::read_unaligned(site.address as usize as *const [u8; 7]) }
    });
    if let Err(why) = check_layout(field.address, site.address, bytes) {
        return outcome_line(false, &why);
    }
    // SAFETY: a resolved site, checked in its function, installed before
    // any world exists; only the jne before it branches to its first byte
    // and nothing past it (tpfre, noted in the profile); the hook only
    // changes r9 to the map's own head, which the miss path expects.
    match unsafe { Splice::install(site.address as usize as *mut u8, &EXPECTED, STEAL, hook) } {
        Ok(splice) => {
            let _kept = std::mem::ManuallyDrop::new(splice);
            outcome_line(
                true,
                &format!(
                    "at {:#x}, the town street field is computed at every point asked, never answered from its per-cell cache",
                    site.address
                ),
            )
        }
        Err(error) => outcome_line(false, &format!("the site at {:#x}: {error}", site.address)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_lookup_is_made_a_miss_when_the_head_reads() {
        assert_eq!(forced(0x20, Some(1)), Some(0x20));
        assert_eq!(forced(0x20, Some(0)), None, "not a head");
        assert_eq!(forced(0x20, None), None, "unreadable");
        assert_eq!(forced(0, Some(1)), None);
    }

    #[test]
    fn the_layout_must_be_the_release_builds() {
        let field = 0x0001_42b7_6a90_u64;
        let site = field + SITE_AT;
        assert_eq!(check_layout(field, site, Some(EXPECTED)), Ok(()));
        assert!(check_layout(field, site + 1, Some(EXPECTED)).is_err());
        assert!(check_layout(field, site, None).is_err());
        let mut other = EXPECTED;
        other[6] = 0x36;
        assert!(
            check_layout(field, site, Some(other))
                .unwrap_err()
                .contains("reaches")
        );
        other[5] = 0x74;
        assert!(check_layout(field, site, Some(other)).is_err());
    }

    #[test]
    fn it_is_on_unless_switched_off_and_needs_its_targets() {
        let resolved = ResolvedProfile {
            name: "empty".into(),
            targets: Vec::new(),
            absent_optional: Vec::new(),
        };
        let line = install_with(&resolved, false);
        assert!(line.starts_with("order fix town-field-cache: off") && line.contains(TOGGLE_ENV));
        assert!(install_with(&resolved, true).contains("lacks"));
        assert!(crate::ticks::wanted(None) && !crate::ticks::wanted(Some("0")));
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
        assert_eq!(target(SITE).prologue, EXPECTED[..STEAL].to_vec());
        for name in [FIELD, SITE] {
            assert!(!target(name).required, "{name} is optional");
        }
    }
}
