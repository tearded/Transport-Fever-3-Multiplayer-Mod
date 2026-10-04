//! One compiled native ABI bundle, paired with hooks.toml.

pub const PROFILE_NAME: &str = "tf3_build40408_steam_windows/hooks.toml";
pub const PROFILE_TOML: &str = include_str!("hooks.toml");
pub const STEAM_BUILD_ID: u64 = 25533170;
pub const GAME_BUILD: u32 = 40408;

pub mod builds;
pub mod drawing;
pub mod edgewatch;
pub mod guiplayer;
pub mod junctions;
pub mod menu;
pub mod modules;
pub mod order;
pub mod persons;
pub mod probe;
pub mod seeds;
pub mod stoptool;
pub mod streettrace;
pub mod terrain;
pub mod ticks;
pub mod toolplayer;
pub mod townfield;
pub mod towntrace;
