//! Native data for Steam Windows build 40408. See hooks.toml for identity.

/// The paused path's call of the GameTime advance, inside `GameSim::Step`:
/// `mov rcx,[rbp+8]; xor r8d,r8d; mov edx,[rcx+0x208]; mov rcx,[rcx+0x18];
/// call advance`.
pub const SITE: &str = "GameSim::Step/paused GameTime advance";

/// The advance the call must reach (name ours): `(engine, GameTime entity,
/// bool update)`.
pub const ADVANCE: &str = "CGameTime::Advance";

/// The advance's two increments, which say what it counts: `inc [rdi+0x3c];
/// test bpl,bpl; je +3; inc [rdi+0x40]`.
pub const ADVANCE_TICK: &str = "CGameTime::Advance/tick";

/// The increments' bytes, as the profile's prologue states them.
pub const TICK_BYTES: [u8; 11] = [
    0xFF, 0x47, 0x3C, // inc dword [rdi+0x3c]   (tickCount)
    0x40, 0x84, 0xED, // test bpl, bpl          (the update flag)
    0x74, 0x03, // je +3
    0xFF, 0x47, 0x40, // inc dword [rdi+0x40]   (updateCount)
];

/// Where the increments sit in the advance (0xbace99 - 0xbace10).
pub const TICK_OFFSET: u64 = 0x89;

/// The two getters the checkpoint line reads the counters with: each takes
/// a `CGameTime*` (`[+8]` the engine, `[+0x10]` a pointer to the GameTime
/// entity) and returns the counter.
pub const GET_TICK_COUNT: &str = "CGameTime::GetTickCount";

pub const GET_UPDATE_COUNT: &str = "CGameTime::GetUpdateCount";
