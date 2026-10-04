//! Native data for Steam Windows build 40408. See hooks.toml for identity.

/// Where a `Command` keeps its payload.
pub const COMMAND_PAYLOAD: usize = 0;

/// Where a payload keeps its variant index (build 40408).
pub const PAYLOAD_INDEX: usize = 0x9b8;

/// The variant index of a `WorldBuildProposal` (the dispatcher's case 53).
pub const WORLD_BUILD_PROPOSAL: i8 = 52;

/// Where a `WorldBuildProposal` payload keeps `playerInitiated`.
pub const PLAYER_INITIATED: usize = 0x3d2;
