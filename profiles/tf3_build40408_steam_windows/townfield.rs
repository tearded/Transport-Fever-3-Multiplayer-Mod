//! Native data for Steam Windows build 40408. See hooks.toml for identity.

pub const FIELD: &str = "StreetField::At";

pub const SITE: &str = "StreetField::At/cache found";

/// Where the site lies in `At`.
pub const SITE_AT: u64 = 0xba;

/// Where the miss path starts in `At`: the site's `jne` must reach it.
pub const MISS_AT: u64 = 0xf6;

/// `cmp byte [r9+0x19], 0` (stolen), then `jne` to the miss path.
pub const EXPECTED: [u8; 7] = [0x41, 0x80, 0x79, 0x19, 0x00, 0x75, 0x35];

pub const STEAL: usize = 5;

/// The map's head node's nil flag.
pub const NIL: u64 = 0x19;
