//! Native data for Steam Windows build 40408. See hooks.toml for identity.

/// Where the script's entity (`ecs::Entity`, an `int`) is in each call's
/// functor, as the engine's own code reads it (build 40408): the update
/// functor's `mov eax, [rdi+0x20]` (0xf45490), the post-update functor's
/// `mov eax, [rdi+0x18]` (0xf449e6), the event operator's captures'
/// `mov eax, [rdi+0x20]` (0xf417d5).
pub const UPDATE_ENTITY: usize = 0x20;

pub const POST_UPDATE_ENTITY: usize = 0x18;

pub const EVENT_ENTITY: usize = 0x20;

/// Where the event operator's captures keep the event's `int const*` seed:
/// the engine reseeds the state itself when it is not null (`mov rax,
/// [rcx+0x28]` ... `call lua::State::RandomSeed`, 0xf417a7).
pub const EVENT_SEED: usize = 0x28;
