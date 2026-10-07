//! Read-only probe sites for automatic industry spawning in Steam build 40408.
//! Exact bytes and surrounding disassembly are documented in hooks.toml.

pub const SEED_SITE: &str = "industry spawn seed";
pub const SEED_EXPECTED: [u8; 7] = [0x4c, 0x8b, 0x46, 0x08, 0x48, 0x8b, 0x16];
pub const SEED_STEAL: usize = 7;

pub const CANDIDATE_SITE: &str = "industry candidate loop";
pub const CANDIDATE_EXPECTED: [u8; 8] = [
    0xc6, 0x44, 0x24, 0x70, 0x00, // mov byte ptr [rsp+0x70], 0
    0x44, 0x8b, 0xcf, // mov r9d, edi
];
pub const CANDIDATE_STEAL: usize = 8;

pub const CANDIDATE_RESULT_SITE: &str = "industry candidate gate result";
pub const CANDIDATE_RESULT_EXPECTED: [u8; 7] = [
    0x48, 0x89, 0x8d, 0xb0, 0x00, 0x00, 0x00, // mov [rbp+0xb0], rcx
];
pub const CANDIDATE_RESULT_STEAL: usize = 7;

pub const ENTITY_SITE: &str = "industry emitted entity";
pub const ENTITY_EXPECTED: [u8; 6] = [
    0x8b, 0x5d, 0x84, // mov ebx, [rbp-0x7c]
    0x83, 0xfb, 0xff, // cmp ebx, -1
];
pub const ENTITY_STEAL: usize = 6;

pub const CALLBACK_END_SITE: &str = "industry spawn callback end";
pub const CALLBACK_END_EXPECTED: [u8; 9] = [
    0x90, // nop after the spawn lambda returns
    0x48, 0x8b, 0x9c, 0x24, 0xb0, 0x00, 0x00, 0x00, // mov rbx, [rsp+0xb0]
];
pub const CALLBACK_END_STEAL: usize = 9;

pub const CAPACITY: &str = "GetFreeTargetCapacity";
pub const CAPACITY_EXPECTED: [u8; 16] = [
    0x48, 0x89, 0x74, 0x24, 0x20, // mov [rsp+0x20], rsi
    0x41, 0x56, // push r14
    0x48, 0x83, 0xec, 0x20, // sub rsp, 0x20
    0x48, 0x83, 0x7a, 0x30, 0x00, // cmp qword ptr [rdx+0x30], 0
];
