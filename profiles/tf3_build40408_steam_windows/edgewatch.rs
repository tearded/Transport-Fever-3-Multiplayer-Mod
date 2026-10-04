//! Native data for Steam Windows build 40408. See hooks.toml for identity.

pub const APPLY: &str = "CommandApply::One";

pub const RETURN_SITE: &str = "CommandApply::One/return";

/// Where the epilogue site lies in `One`.
pub const RETURN_SITE_AT: u64 = 0x352;

/// Where `One` reads the payload's kind for the dispatcher.
pub const KIND_READ_AT: u64 = 0xaf;

/// `movsx rcx, byte ptr [r8+0x9b8]`: the kind's read.
pub const KIND_READ: [u8; 8] = [0x49, 0x0F, 0xBE, 0x88, 0xB8, 0x09, 0x00, 0x00];

/// `mov [rsp+0x18], rbx` (stolen), then `mov [rsp+0x20], rsi`.
pub const ENTRY_EXPECTED: [u8; 10] = [0x48, 0x89, 0x5C, 0x24, 0x18, 0x48, 0x89, 0x74, 0x24, 0x20];

pub const ENTRY_STEAL: usize = 5;

/// `lea r11, [rsp+0xb0]` (stolen), then `mov rbx, [r11+0x40]; mov rsi,
/// [r11+0x48]`, past the cookie check, which every path reaches.
pub const RETURN_EXPECTED: [u8; 16] = [
    0x4C, 0x8D, 0x9C, 0x24, 0xB0, 0x00, 0x00, 0x00, // lea r11, [rsp+0xb0]
    0x49, 0x8B, 0x5B, 0x40, // mov rbx, [r11+0x40]
    0x49, 0x8B, 0x73, 0x48, // mov rsi, [r11+0x48]
];

pub const RETURN_STEAL: usize = 8;

/// The epilogue site's `rsp` lies this far below the entry's: five pushes
/// and `sub rsp, 0xb0`.
pub const FRAME: u64 = 0xd8;

/// The command's fields and the `GameState`'s.
pub const PAYLOAD_KIND: u64 = 0x9b8;

pub const ENTITIES: u64 = 8;

pub const RESULT: u64 = 0x30;

pub const ENTITY_ENTRY: u64 = 16;

pub const ENGINE: u64 = 0x18;

pub const GAME_TIME: u64 = 0x28;

/// The paths into `One` by their call sites.
pub const PATHS: [(u64, &str); 3] = [
    (0x11eb96, "queue"),
    (0x1204bf, "script"),
    (0x120334, "direct"),
];
