//! Native data for Steam Windows build 40408. See hooks.toml for identity.

/// The profile's name for PushCompressor's load of the zstd level.
pub const LEVEL_LOAD: &str = "save: PushCompressor level load";

/// The profile's name for PushCompressor's stream buffer size.
pub const BUFFER_SIZE: &str = "save: PushCompressor buffer size";

/// `mov eax,[rip+0x349564e]`: the level, 3.
pub const LEVEL_LOAD_BYTES: [u8; 6] = [0x8B, 0x05, 0x4E, 0x56, 0x49, 0x03];

/// `mov eax,1; nop`: zstd level 1.
pub const LEVEL_ONE_BYTES: [u8; 6] = [0xB8, 0x01, 0x00, 0x00, 0x00, 0x90];

/// `mov r8d,0x80`: a 128-byte stream buffer.
pub const BUFFER_BYTES: [u8; 6] = [0x41, 0xB8, 0x80, 0x00, 0x00, 0x00];

/// `mov r8d,0x10000`: a 64 KiB stream buffer.
pub const BUFFER_64K_BYTES: [u8; 6] = [0x41, 0xB8, 0x00, 0x00, 0x01, 0x00];
