//! Native data for Steam Windows build 40408. See hooks.toml for identity.

pub const TRY: &str = "StreetDeveloper::TryCandidate";

pub const TRY_RETURN: &str = "StreetDeveloper::TryCandidate/return";

pub const REJECT: &str = "StreetDeveloper::Reject";

pub const ERRORS: &str = "StreetDeveloper::BuildStreet/errors";

/// Where the return site lies in `TryCandidate`.
pub const TRY_RETURN_AT: u64 = 0x7bc;

/// `TryCandidate`'s three calls of the reject function.
pub const REJECT_CALLS_AT: [u64; 3] = [0x2b5, 0x33f, 0x715];

/// `mov rax, rsp; mov [rax+0x10], rbx` (stolen), then `push rbp; push rsi`.
pub const TRY_EXPECTED: [u8; 9] = [0x48, 0x8B, 0xC4, 0x48, 0x89, 0x58, 0x10, 0x55, 0x56];

pub const TRY_STEAL: usize = 7;

/// `lea r11, [rsp+0x960]` (stolen), then `mov rbx, [r11+0x48]`, past the
/// cookie check, which both ways out reach.
pub const TRY_RETURN_EXPECTED: [u8; 12] = [
    0x4C, 0x8D, 0x9C, 0x24, 0x60, 0x09, 0x00, 0x00, // lea r11, [rsp+0x960]
    0x49, 0x8B, 0x5B, 0x48, // mov rbx, [r11+0x48]
];

pub const TRY_RETURN_STEAL: usize = 8;

/// `mov [rsp+8], rbx` (stolen), then `mov [rsp+0x10], rbp`.
pub const REJECT_EXPECTED: [u8; 10] = [0x48, 0x89, 0x5C, 0x24, 0x08, 0x48, 0x89, 0x6C, 0x24, 0x10];

pub const REJECT_STEAL: usize = 5;

/// `mov rax, [rbp+0x9a0]` (stolen), then `cmp [rbp+0x998], rax`.
pub const ERRORS_EXPECTED: [u8; 14] = [
    0x48, 0x8B, 0x85, 0xA0, 0x09, 0x00, 0x00, // mov rax, [rbp+0x9a0]
    0x48, 0x39, 0x85, 0x98, 0x09, 0x00, 0x00, // cmp [rbp+0x998], rax
];

pub const ERRORS_STEAL: usize = 7;

/// `TryCandidate`'s frame at its return site: the pass at `[rsp+0x30]`,
/// the direction at `[rbp+0x68]`, the street's end at `[rbp+0xa4]`.
pub const TRY_PASS: u64 = 0x30;

pub const TRY_DIR: u64 = 0x68;

pub const TRY_END: u64 = 0xa4;

/// The proposal's two error vectors in `0x9657c0`'s frame.
pub const ERRORS_FIRST: u64 = 0x998;

pub const ERRORS_SECOND: u64 = 0x9b0;

/// Bytes of each error vector logged.
pub const ERROR_BYTES: usize = 32;
