//! Native data for Steam Windows build 40408. See hooks.toml for identity.

/// The profile's name for the stop tool's call of `CommandList::Add` in
/// `MousePressed`. `Add` returns 5 bytes past it.
pub const STOP_ADD_CALL: &str = "StreetTerminalBuilder::MousePressed/Add call";

/// The profile's name for `MousePressed` setting the busy byte, whose
/// signature holds [`BUSY`]: a build that moved it does not resolve.
pub const STOP_BUSY_SET: &str = "StreetTerminalBuilder::MousePressed/busy set";

/// Where the tool keeps its busy byte (build 40408).
pub const BUSY: usize = 0x2c8;

/// Where a `std::function` keeps its impl pointer (MSVC x64).
pub const FUNCTION_IMPL: usize = 0x38;

/// Where the click's functor keeps the tool, after its vftable.
pub const FUNCTOR_TOOL: usize = 0x8;
