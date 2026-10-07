//! Faster saves, after silver2127's Big Maps for TPF2 (tpf2-bigmap,
//! `save_fast`; docs/BIGMAPS.md, "Saving").
//!
//! TF3 compresses a save with zstd at level 3 through a 128-byte stream
//! buffer. On a big map that is most of a save's time: a 45 km world's
//! 438 MB save took 7.7 s. Two instructions of the save writer's
//! `PushCompressor` are rewritten in place (profiles/…/hooks.toml, "Faster
//! saves"): the level becomes 1 and the buffer 64 KiB. On TPF2 that made
//! compression 2.79 times faster for files about 9% larger. A save is still
//! a standard zstd frame of the same bytes, so every game loads it, with or
//! without the hook; loading is not touched (`PushDecompressor` keeps
//! reading the level constant, which stays 3).
//!
//! A room never compares save files: it judges the lanes' digests
//! (docs/PROTOCOL.md, "Saving"), so games with and without this agree.
//! Each rewrite checks the site's bytes first and installs alone; a build
//! where either is not exactly as expected keeps the game's own.

#![allow(unsafe_code)]

use tpf3mp_hookcore::detour::Rewrite;

pub use crate::build_data::native::savefast::{
    BUFFER_64K_BYTES, BUFFER_BYTES, BUFFER_SIZE, LEVEL_LOAD, LEVEL_LOAD_BYTES, LEVEL_ONE_BYTES,
};

/// The kill switch: `0` (or `off`, `false`, `no`) keeps the game's level
/// and buffer.
pub const ENV: &str = "TPF3MP_HOOK_SAVE_FAST";

/// Rewrites both sites the profile resolved, where their bytes are the
/// expected ones, and says what it did.
///
/// # Safety
///
/// `at` gives addresses in this process's image; nothing saves yet (the
/// hook installs before the game's first frame).
pub unsafe fn install(at: &dyn Fn(&str) -> Result<usize, String>) -> String {
    if !crate::ticks::wanted(std::env::var(ENV).ok().as_deref()) {
        return format!("faster saves: off ({ENV} says so)");
    }
    let rewrite = |name: &str, expected: &[u8], replacement: &[u8]| {
        at(name).and_then(|address| {
            // SAFETY: the caller's; Rewrite checks the bytes before writing.
            unsafe { Rewrite::install(address as *mut u8, expected, replacement) }
                .map_err(|error| error.to_string())
        })
    };
    let level = rewrite(LEVEL_LOAD, &LEVEL_LOAD_BYTES, &LEVEL_ONE_BYTES);
    let buffer = rewrite(BUFFER_SIZE, &BUFFER_BYTES, &BUFFER_64K_BYTES);
    let level = match level {
        Ok(patch) => {
            std::mem::forget(patch);
            "zstd level 1".to_owned()
        }
        Err(why) => format!("the game's zstd level ({why})"),
    };
    let buffer = match buffer {
        Ok(patch) => {
            std::mem::forget(patch);
            "a 64 KiB buffer".to_owned()
        }
        Err(why) => format!("the game's 128-byte buffer ({why})"),
    };
    format!("faster saves: {level}, {buffer} ({ENV}=0 turns them off)")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_rewrites_keep_each_instruction_whole() {
        // mov eax,imm32; nop in place of mov eax,[rip+disp32].
        assert_eq!(LEVEL_LOAD_BYTES[..2], [0x8B, 0x05]);
        assert_eq!(LEVEL_ONE_BYTES, [0xB8, 1, 0, 0, 0, 0x90]);
        // mov r8d,imm32, its immediate 0x80 then 0x10000.
        assert_eq!(BUFFER_BYTES[..2], BUFFER_64K_BYTES[..2]);
        assert_eq!(
            u32::from_le_bytes(BUFFER_BYTES[2..].try_into().unwrap()),
            0x80
        );
        assert_eq!(
            u32::from_le_bytes(BUFFER_64K_BYTES[2..].try_into().unwrap()),
            0x1_0000
        );
    }

    #[test]
    fn missing_sites_leave_the_game_its_own() {
        // SAFETY: nothing resolves, so nothing is written.
        let line = unsafe { install(&|name| Err(format!("{name} is not in this build"))) };
        assert!(line.contains("the game's zstd level"), "{line}");
        assert!(line.contains("the game's 128-byte buffer"), "{line}");
    }

    #[test]
    fn the_profile_names_both_sites_with_the_bytes_rewritten() {
        let profile =
            tpf3mp_hookcore::profile::Profile::from_toml(crate::build_data::native::PROFILE_TOML)
                .unwrap();
        for (name, bytes) in [
            (LEVEL_LOAD, &LEVEL_LOAD_BYTES),
            (BUFFER_SIZE, &BUFFER_BYTES),
        ] {
            let target = profile
                .targets
                .iter()
                .find(|target| target.name == name)
                .unwrap_or_else(|| panic!("{name} is in the profile"));
            assert!(!target.required, "{name} is optional");
            assert_eq!(&target.prologue[..], &bytes[..], "{name}");
        }
    }
}
