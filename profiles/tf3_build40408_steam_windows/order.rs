//! Native data for Steam Windows build 40408. See hooks.toml for identity.

pub mod route_trace {
    pub const SITE: &str = "ecs::LineSystem::GetData/return";

    pub const EXPECTED: [u8; 9] = [0x48, 0x83, 0xc0, 0x18, 0x48, 0x83, 0xc4, 0x28, 0xc3];
}

pub mod land_vehicle {
    /// The site: `mov r13, [rbp-0x20]; mov rsi, [rbp-0x18]; cmp r13, rsi`.
    pub const SITE: &str = "ecs::LandVehicleMoveSystem::Update2/shuffle";

    /// The engine's own walk from an entry to its node record, which the
    /// hook copies: `this` at `[rbp-0x80]`, the node-list holder at
    /// `this+8`, the records at `[holder]`, 20 bytes each, the entry's
    /// node index at its first dword.
    pub const RECORDS: &str = "ecs::LandVehicleMoveSystem::Update2/records";

    /// The bytes at the site; the first [`STEAL`] are run from the stub.
    pub const EXPECTED: [u8; 11] = [
        0x4C, 0x8B, 0x6D, 0xE0, // mov r13, [rbp-0x20]
        0x48, 0x8B, 0x75, 0xE8, // mov rsi, [rbp-0x18]
        0x4C, 0x3B, 0xEE, // cmp r13, rsi
    ];

    pub const STEAL: usize = 8;

    /// Frame slots, as the stolen bytes encode them.
    pub const VEC_BEGIN: i64 = -0x20;

    pub const VEC_END: i64 = -0x18;

    /// Where `this` is, as the records walk encodes it (`mov r8, [rbp-0x80]`).
    pub const THIS: i64 = -0x80;

    /// One entry: `{int32 nodeIndex, float priority}`.
    pub const ENTRY_LEN: u64 = 8;

    /// One node record: the entity id, then four component indices.
    pub const RECORD_LEN: u64 = 20;
}

pub mod terminal {
    /// The site: `mov [rsp+0x248], rax` right after the getter's call, with
    /// `rax` the `std::vector<Entity>*`.
    pub const SITE: &str = "ecs::SimEntityAtTerminalSystem::Update/vehicles at stop";

    /// The getter the call before the site must reach.
    pub const GETTER: &str = "ecs::TransportVehicleSystem::GetVehiclesAtLineStop";

    pub const EXPECTED: [u8; 12] = [
        0x48, 0x89, 0x84, 0x24, 0x48, 0x02, 0x00, 0x00, // mov [rsp+0x248], rax
        0x41, 0x8B, 0x7F, 0x50, // mov edi, [r15+0x50]
    ];

    pub const STEAL: usize = 8;
}

pub mod platform {
    /// `mov rcx,[r13+0x10]; movsxd rax,[rsi+rdi+4]; imul rbx,rax,0x1e8`,
    /// right after `mov rax,[r13+8]; mov rdi,[rax]`.
    pub const VISIT_SITE: &str = "ecs::TransportVehicleSystem::Update2/visit";

    pub const VISIT_EXPECTED: [u8; 16] = [
        0x49, 0x8B, 0x4D, 0x10, // mov rcx, [r13+0x10]
        0x48, 0x63, 0x44, 0x3E, 0x04, // movsxd rax, [rsi+rdi+4]
        0x48, 0x69, 0xD8, 0xE8, 0x01, 0x00, 0x00, // imul rbx, rax, 0x1e8
    ];

    pub const VISIT_STEAL: usize = 9;

    /// `mov rcx,r14; sub rcx,r13; mov rax,rdi; imul rcx`: the candidates
    /// are `[r13, r14)`, the sort's call follows.
    pub const CANDIDATES_SITE: &str = "FindNextFreeTerminal/candidate sort";

    pub const CANDIDATES_EXPECTED: [u8; 12] = [
        0x49, 0x8B, 0xCE, // mov rcx, r14
        0x49, 0x2B, 0xCD, // sub rcx, r13
        0x48, 0x8B, 0xC7, // mov rax, rdi
        0x48, 0xF7, 0xE9, // imul rcx
    ];

    pub const CANDIDATES_STEAL: usize = 6;

    /// Update2's `int` argument, the node count, spilled at `[rbp+0x5b0]`.
    pub const COUNT: i64 = 0x5b0;

    pub const RECORD_LEN: u64 = 8;

    pub const CANDIDATE_LEN: u64 = 12;
}

pub mod road {
    use super::measure;

    pub const ADD: &str = measure::EDGE_USE_ADD;

    pub const ADD_RANGE: &str = measure::EDGE_USE_ADD_RANGE;

    /// One entry: the entity id first.
    pub const ENTRY_LEN: u64 = 20;

    /// One edge's data: its length (a float), then the entries vector.
    pub const EDGE_DATA_LEN: u64 = 32;

    /// One edge entity's slot: its edges vector first.
    pub const SLOT_LEN: u64 = 72;

    /// An `EdgeId`: entity, index, direction.
    pub const EDGE_ID_LEN: u64 = 12;

    /// Where `Add`'s manager keeps its data (`EdgeUseManagerData*`).
    pub const MANAGER_DATA: u64 = 0x18;
}

pub mod measure {
    pub const RESERVE: &str = "transport::EdgeReservationManager::Reserve";

    pub const RESERVE_SIMPLE: &str = "transport::EdgeReservationManager::Reserve_simple";

    pub const EDGE_USE_ADD: &str = "transport::EdgeUseManager::Add";

    pub const EDGE_USE_ADD_RANGE: &str = "transport::EdgeUseManager::AddRange";

    pub const ENGINE_UPDATE: &str = "ecs::Engine::Update";

    /// One `EdgeId` on a path: entity, index, direction.
    pub const EDGE_LEN: u64 = 12;
}
