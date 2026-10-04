//! Native data for Steam Windows build 40408. See hooks.toml for identity.

pub const APPLY: &str = "TownUpdateSize::Apply";

pub const DEVELOP_SITE: &str = "TownUpdateSize::Apply/develop";

pub const RETURN_SITE: &str = "TownUpdateSize::Apply/return";

pub const DEVELOP: &str = "TownDeveloper::Develop";

/// Where the sites lie in the applier.
pub const DEVELOP_SITE_AT: u64 = 0x17a;

pub const RETURN_SITE_AT: u64 = 0x1c7;

/// The applier's call of `Develop`.
pub const DEVELOP_CALL_AT: u64 = 0x1bb;

/// `mov rax, [rbp+8]; mov rcx, [rax+0x200]` (stolen), then
/// `mov byte [rsp+0x9c], 0`.
pub const DEVELOP_SITE_EXPECTED: [u8; 19] = [
    0x48, 0x8B, 0x45, 0x08, // mov rax, [rbp+8]
    0x48, 0x8B, 0x88, 0x00, 0x02, 0x00, 0x00, // mov rcx, [rax+0x200]
    0xC6, 0x84, 0x24, 0x9C, 0x00, 0x00, 0x00, 0x00, // mov byte [rsp+0x9c], 0
];

pub const DEVELOP_SITE_STEAL: usize = 11;

/// `mov al, 1; add rsp, 0x50` (stolen), then `pop r15; pop r14`.
pub const RETURN_EXPECTED: [u8; 10] = [
    0xB0, 0x01, // mov al, 1
    0x48, 0x83, 0xC4, 0x50, // add rsp, 0x50
    0x41, 0x5F, 0x41, 0x5E, // pop r15; pop r14
];

pub const RETURN_STEAL: usize = 6;

/// `mov [rsp+0x18], r8d` (stolen), then `push rbp; push rbx; push rsi;
/// push rdi`.
pub const DEVELOP_EXPECTED: [u8; 9] = [0x44, 0x89, 0x44, 0x24, 0x18, 0x55, 0x53, 0x56, 0x57];

pub const DEVELOP_STEAL: usize = 5;

/// The command's fields, the `GameState`'s and the frame's.
pub const CMD_LEN: u64 = 0x11;

pub const GAME_STATE: u64 = 8;

pub const GAME_TIME: u64 = 0x28;

pub const DEVELOPER: u64 = 0x200;

pub const DEVELOPER_ENGINE: u64 = 0xb0;

/// The generator, in the applier's frame (`[rsp+0x90]`).
pub const GENERATOR: u64 = 0x90;

/// `Develop`'s fifth argument, the generator's address, at its entry.
pub const DEVELOP_GENERATOR_ARG: u64 = 0x28;
