//! Native data for Steam Windows build 40408. See hooks.toml for identity.

/// The kill switch: `0` (or `off`, `false`, `no`) leaves every tool the
/// save's player's.
pub const ENV: &str = "TPF3MP_HOOK_TOOL_COMPANY";

/// The name in hook.log.
pub const FIX: &str = "tool-company";

/// The note the GUI keeps the player's company's entity under
/// (`tpf3mp/follow.lua`, `COMPANY_NOTE`), "" for none.
pub const COMPANY_NOTE: &str = "tpf3mp.company";

/// `UI::StreetBuilder::Step` (vf5, rva 0x585e50).
pub const STREET_STEP: &str = "UI::StreetBuilder::Step";

/// The street builder constructor's store of its player (rva 0x56a7f4:
/// `... mov eax,[rbp+0x128]; mov [rsi+0xc0],eax`).
pub const STREET_STORE: &str = "UI::StreetBuilder ctor/player store";

/// `UI::TrackModifier::Step` (vf5, rva 0x5cbf80).
pub const MODIFIER_STEP: &str = "UI::TrackModifier::Step";

/// The track modifier constructor's store of its player (rva 0x5b4238:
/// `... mov eax,[rbp+0x330]; mov [r14+0xa0],eax`).
pub const MODIFIER_STORE: &str = "UI::TrackModifier ctor/player store";

/// `UI::Bulldozer::Step` (vf5, rva 0x4d6340).
pub const BULLDOZER_STEP: &str = "UI::Bulldozer::Step";

/// The bulldozer constructor's `BulldozerFilter` (rva 0x4c4c32: its vtable,
/// its player list at `+0x10`, and the filter's place in the bulldozer).
pub const BULLDOZER_FILTER: &str = "UI::Bulldozer ctor/filter";

/// The street builder's store: the bytes before the field's disp32, the
/// field's disp32 at [`STREET_DISP_AT`], and the length read.
pub const STREET_STORE_BYTES: [u8; 40] = [
    0x48, 0x8B, 0x85, 0x10, 0x01, 0x00, 0x00, 0x48, 0x89, 0x86, 0xB0, 0x00, 0x00, 0x00, 0x48, 0x8B,
    0x85, 0x18, 0x01, 0x00, 0x00, 0x48, 0x89, 0x86, 0xB8, 0x00, 0x00, 0x00, 0x8B, 0x85, 0x28, 0x01,
    0x00, 0x00, 0x89, 0x86, 0xC0, 0x00, 0x00, 0x00,
];

pub const STREET_DISP_AT: usize = 36;

/// The track modifier's store.
pub const MODIFIER_STORE_BYTES: [u8; 27] = [
    0x48, 0x8B, 0x85, 0x20, 0x03, 0x00, 0x00, 0x49, 0x89, 0x86, 0x98, 0x00, 0x00, 0x00, 0x8B, 0x85,
    0x30, 0x03, 0x00, 0x00, 0x41, 0x89, 0x86, 0xA0, 0x00, 0x00, 0x00,
];

pub const MODIFIER_DISP_AT: usize = 23;

/// The bulldozer's filter: `lea rax,[rip+vtable]` (disp32 at 3, wildcard),
/// the filter's fields set (its player list at `+0x10..+0x20`), and `mov
/// [r14+disp32],rbx` (disp32 at [`FILTER_DISP_AT`]).
pub const FILTER_BYTES: [Option<u8>; 48] = {
    const B: [u8; 48] = [
        0x48, 0x8D, 0x05, 0, 0, 0, 0, 0x48, 0x89, 0x03, 0x48, 0x89, 0x73, 0x08, 0x48, 0x89, 0x7B,
        0x10, 0x48, 0x89, 0x7B, 0x18, 0x48, 0x89, 0x7B, 0x20, 0x66, 0xC7, 0x43, 0x28, 0x00, 0x00,
        0xEB, 0x03, 0x48, 0x8B, 0xDF, 0x48, 0x89, 0x5D, 0xD8, 0x49, 0x89, 0x9E, 0xC0, 0x00, 0x00,
        0x00,
    ];
    let mut out = [None; 48];
    let mut i = 0;
    while i < 48 {
        if i < 3 || i >= 7 {
            out[i] = Some(B[i]);
        }
        i += 1;
    }
    out
};

pub const FILTER_DISP_AT: usize = 44;

/// The filter's player list (a `std::vector<Entity>`: begin, end, capacity).
pub const FILTER_PLAYERS: usize = 0x10;

/// `UI::ConstructionBuilder::Step` (vf5, rva 0x51cd60): stations, depots
/// and every construction the construction menu places.
pub const CONSTRUCTION_STEP: &str = "UI::ConstructionBuilder::Step";

/// Its constructor's store of its player (rva 0x50b438: `... mov
/// eax,[rbp+0x2f0]; mov [r14+0xa0],eax`).
pub const CONSTRUCTION_STORE: &str = "UI::ConstructionBuilder ctor/player store";

pub const CONSTRUCTION_STORE_BYTES: [u8; 34] = [
    0x49, 0x89, 0xB6, 0x90, 0x00, 0x00, 0x00, 0x48, 0x8B, 0x85, 0xE0, 0x02, 0x00, 0x00, 0x49, 0x89,
    0x86, 0x98, 0x00, 0x00, 0x00, 0x8B, 0x85, 0xF0, 0x02, 0x00, 0x00, 0x41, 0x89, 0x86, 0xA0, 0x00,
    0x00, 0x00,
];

pub const CONSTRUCTION_DISP_AT: usize = 30;

/// `UI::StreetTerminalBuilder::Step` (vf5, rva 0x595f30): the stop builder
/// and, a second instance of the class, the signal and waypoint builder.
pub const TERMINAL_STEP: &str = "UI::StreetTerminalBuilder::Step";

/// Its constructor's store of its player (rva 0x590773: `... mov
/// eax,[rbp+0x1d0]; mov [r14+0xa0],eax`).
pub const TERMINAL_STORE: &str = "UI::StreetTerminalBuilder ctor/player store";

pub const TERMINAL_STORE_BYTES: [u8; 41] = [
    0x49, 0x89, 0xBE, 0x88, 0x00, 0x00, 0x00, 0x49, 0x89, 0xB6, 0x90, 0x00, 0x00, 0x00, 0x48, 0x8B,
    0x85, 0xC0, 0x01, 0x00, 0x00, 0x49, 0x89, 0x86, 0x98, 0x00, 0x00, 0x00, 0x8B, 0x85, 0xD0, 0x01,
    0x00, 0x00, 0x41, 0x89, 0x86, 0xA0, 0x00, 0x00, 0x00,
];

pub const TERMINAL_DISP_AT: usize = 37;

/// `UI::ModuleBuilder::Step` (vf5, rva 0x545b50): a station's modules.
pub const MODULE_STEP: &str = "UI::ModuleBuilder::Step";

/// Its constructor's store of its player (rva 0x540431: `... mov
/// eax,[rbp+0x2e0]; mov [rdi+0xa8],eax`).
pub const MODULE_STORE: &str = "UI::ModuleBuilder ctor/player store";

pub const MODULE_STORE_BYTES: [u8; 43] = [
    0x48, 0x8B, 0x85, 0xC8, 0x02, 0x00, 0x00, 0x48, 0x89, 0x87, 0x98, 0x00, 0x00, 0x00, 0x49, 0x8B,
    0x04, 0x24, 0x33, 0xC9, 0x49, 0x89, 0x0C, 0x24, 0x48, 0x89, 0x87, 0xA0, 0x00, 0x00, 0x00, 0x8B,
    0x85, 0xE0, 0x02, 0x00, 0x00, 0x89, 0x87, 0xA8, 0x00, 0x00, 0x00,
];

pub const MODULE_DISP_AT: usize = 39;

/// The bulldozer constructor's store of its own player (rva 0x4c4a41:
/// `lea rax,[vtable]; mov [r14],rax; mov [r14+0x20],rsi; mov
/// [r14+0x28],ebx; ...`), which its proposals are made for (`Step`
/// 0x4d687e, 0x4d46b0, 0x4d2b70, its lambda 0x4d2650).
pub const BULLDOZER_STORE: &str = "UI::Bulldozer ctor/player store";

/// The store, with the vtable's disp32 at 3..7 a wildcard; the field's
/// disp8 at [`BULLDOZER_DISP_AT`].
pub const BULLDOZER_STORE_BYTES: [u8; 25] = [
    0x48, 0x8D, 0x05, 0, 0, 0, 0, 0x49, 0x89, 0x06, 0x49, 0x89, 0x76, 0x20, 0x41, 0x89, 0x5E, 0x28,
    0x48, 0x8B, 0x85, 0xC8, 0x01, 0x00, 0x00,
];

pub const BULLDOZER_DISP_AT: usize = 17;

/// The bulldozer constructor's owner list, a `std::vector<Entity>` it fills
/// with its player and copies into its filter (rva 0x4c4ad5: `... lea
/// r15,[r14+0xa8]; mov [r15],rdi; ...`; filled at 0x4c4f6f, copied at
/// 0x4c4fb7). Every query the bulldozer makes is built from it
/// (`sub_4d51c0`, called by `Step` 0x4d6a82, 0x4d4ac0, 0x4d2eb1 and its
/// lambda 0x4d2897, 0x4d29ff).
pub const BULLDOZER_LIST: &str = "UI::Bulldozer ctor/owner list";

pub const BULLDOZER_LIST_BYTES: [u8; 32] = [
    0x49, 0x89, 0xBE, 0x98, 0x00, 0x00, 0x00, 0x49, 0x89, 0xBE, 0xA0, 0x00, 0x00, 0x00, 0x4D, 0x8D,
    0xBE, 0xA8, 0x00, 0x00, 0x00, 0x49, 0x89, 0x3F, 0x49, 0x89, 0x7F, 0x08, 0x49, 0x89, 0x7F, 0x10,
];

pub const BULLDOZER_LIST_DISP_AT: usize = 17;

/// The bulldozer's setter of its owner list, `sub_4d6220` (rva 0x4d6220:
/// this in rcx, the new list in rdx; assigns it to the list and to the
/// filter's copy). The menu's step calls it (`CMenuUI::DoStep`'s lambda,
/// `sub_68f070`, `sub_6a76e0`, `sub_6a1410` at 0x6a14bc) with the GUI's
/// player (`[[game+0x1e0]+0x20c]`, the save's) or an empty list, so it
/// undoes a company written at the tool's frame until the next one.
pub const BULLDOZER_SETTER: &str = "UI::Bulldozer set owner list";

pub const BULLDOZER_SETTER_BYTES: [u8; 26] = [
    0x48, 0x89, 0x5C, 0x24, 0x08, 0x57, 0x48, 0x83, 0xEC, 0x30, 0x48, 0x8D, 0x99, 0xA8, 0x00, 0x00,
    0x00, 0x4C, 0x8B, 0xC2, 0x48, 0x8B, 0xF9, 0x48, 0x3B, 0xDA,
];

pub const BULLDOZER_SETTER_DISP_AT: usize = 13;
