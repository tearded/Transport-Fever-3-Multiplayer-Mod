//! Native data for Steam Windows build 40408. See hooks.toml for identity.

use crate::guiplayer::{Kind, Reg, Site};

pub const SITES: [Site; 17] = [
    Site {
        name: "view: HudIconManager::PreemptiveOctreeTraversal/player",
        expected: &[0x4C, 0x89, 0x75, 0xB8, 0x48, 0x89, 0x5D, 0xC0],
        reg: Reg::Rax,
        kind: Kind::Value,
        what: "the icons above the map's stations",
    },
    Site {
        name: "view: StationViewer::vf4/player",
        expected: &[0x48, 0x8B, 0x46, 0x10, 0x4C, 0x8D, 0x70, 0x78],
        reg: Reg::Rbx,
        kind: Kind::Value,
        what: "the station viewer",
    },
    Site {
        name: "view: CSelector pick/player",
        expected: &[0x48, 0x8B, 0x47, 0x10, 0x4C, 0x8D, 0x70, 0x78],
        reg: Reg::Rbx,
        kind: Kind::Value,
        what: "what the selector picks",
    },
    Site {
        name: "view: ViewCreator::vf1/player",
        expected: &[0x48, 0x8D, 0x55, 0x38, 0x48, 0x89, 0x45, 0x38],
        reg: Reg::Rbx,
        kind: Kind::Value,
        what: "the selector's filter",
    },
    Site {
        name: "view: CatchmentAreaHelper/player 1",
        expected: &[0x48, 0x8B, 0x5D, 0x90, 0x48, 0x8B, 0x53, 0x10],
        reg: Reg::R8,
        kind: Kind::Value,
        what: "the catchment overlay",
    },
    Site {
        name: "view: CatchmentAreaHelper/player 2",
        expected: &[0x40, 0x88, 0x7C, 0x24, 0x20],
        reg: Reg::Rbx,
        kind: Kind::Value,
        what: "the catchment overlay",
    },
    Site {
        name: "view: CatchmentAreaHelper/player 3",
        expected: &[0x4D, 0x8D, 0x77, 0x10, 0x49, 0x8B, 0xCE],
        reg: Reg::Rbx,
        kind: Kind::Value,
        what: "the catchment overlay",
    },
    Site {
        name: "view: CatchmentAreaHelper/player 4",
        expected: &[0xC6, 0x44, 0x24, 0x20, 0x00],
        reg: Reg::Rbx,
        kind: Kind::Value,
        what: "the catchment overlay",
    },
    Site {
        name: "view: CatchmentAreaHelper/owner test",
        expected: &[0x8B, 0x80, 0x0C, 0x02, 0x00, 0x00, 0x39, 0x03],
        reg: Reg::Rbx,
        kind: Kind::Owner,
        what: "the catchment overlay's own stations",
    },
    Site {
        name: "view: LayerManagerColorMap/player 1",
        expected: &[0x49, 0x8B, 0x10, 0x49, 0x8B, 0x48, 0x10],
        reg: Reg::Rax,
        kind: Kind::Value,
        what: "the map layers' colours",
    },
    Site {
        name: "view: LayerManagerColorMap/player 2",
        expected: &[0x48, 0x8B, 0x79, 0x38, 0x48, 0x8B, 0x71, 0x30],
        reg: Reg::Rbx,
        kind: Kind::Value,
        what: "the map layers' colours",
    },
    Site {
        name: "view: LayerManagerColorMap/player 3",
        expected: &[0x48, 0x8B, 0x79, 0x38, 0x48, 0x8B, 0x71, 0x30],
        reg: Reg::Rbx,
        kind: Kind::Value,
        what: "the map layers' colours",
    },
    Site {
        name: "view: LayerManager colour lambda/player",
        expected: &[0x48, 0x8B, 0x51, 0x10, 0x4C, 0x8B, 0x09],
        reg: Reg::Rax,
        kind: Kind::Value,
        what: "the map layers' colours",
    },
    Site {
        name: "view: LayerManager colour/owner test",
        expected: &[0x8B, 0x85, 0x0C, 0x02, 0x00, 0x00, 0x39, 0x02],
        reg: Reg::Rdx,
        kind: Kind::Owner,
        what: "the map layers' own lines and stations",
    },
    Site {
        name: "view: react RendererComponentDelegate/player",
        expected: &[0x49, 0x8B, 0x07, 0x49, 0x8B, 0xCF],
        reg: Reg::Rbx,
        kind: Kind::Value,
        what: "a React renderer component",
    },
    Site {
        name: "view: react RailroadCrossingComp/player",
        expected: &[0x48, 0x8B, 0x03, 0x48, 0x8B, 0x08],
        reg: Reg::R14,
        kind: Kind::Value,
        what: "the railroad crossing component",
    },
    Site {
        name: "view: HudIconManager icon pass/owner",
        expected: &[0x48, 0x8D, 0x14, 0x88, 0x48, 0x85, 0xD2],
        reg: Reg::Rdx,
        kind: Kind::IconOwner,
        what: "the map's icons of every company",
    },
];

/// The site whose owner test also passes every company of the room (the map
/// layers' colours, for lines and stations).
pub const LAYER_OWNER_SITE: usize = 13;

/// The line viewers' candidate lines (`sub_7f3ea0`, called by
/// `LineViewer::Update` 0x7f554c and `UI::MetroViewer::vf4` 0x7fdbd9): its
/// call of the line system's player-to-lines index, `sub_ad2620(lines,
/// player)` (rva 0x7f3f12), with the player the viewer stored when it was
/// made (`[viewer+0x28]`, from `sub_29f66d0`'s read of the GUI's player).
/// Only these lines get their whole route built and drawn. Redirected: in a
/// room the call answers every company's lines (or, with
/// [`crate::guiplayer::ALL_ENV`]`=0`, the player's company's), so a viewer made before the
/// player's company was known still draws the right lines. The index and
/// its other callers, the simulation's among them, are untouched.
pub const LINES_SITE: &str = "view: LineViewer lines of the player/call";

/// The first bytes of `sub_ad2620`, checked before the call is redirected:
/// `mov [rsp+0x10],edx; sub rsp,0x28; mov r9,[rcx+0x58]`.
pub const LINES_CALLEE_BYTES: [u8; 12] = [
    0x89, 0x54, 0x24, 0x10, 0x48, 0x83, 0xEC, 0x28, 0x4C, 0x8B, 0x49, 0x58,
];

/// The `getPlayer` binding's push of its answer (rva 0x24ed2d2, a `call` of
/// the Lua integer push `sub_2fbe300` right after `movsxd rdx,
/// [getter()+0x20c]`, in the closure `SetupUtilInterface` registers as
/// `getPlayer`). Redirected through `guiplayer::push_entry`: where the closure's
/// getter is one of the GUI's (it reads the GUI's slot, `+0x1e0`), the
/// answer is the player's company in a room; for the game scripts' states,
/// whose getter reads the engine's buffers, it is the game's.
pub const GET_PLAYER_PUSH: &str = "view: getPlayer binding/push";

/// The push the call reaches (its first bytes, checked before redirecting).
pub const PUSH_BYTES: [u8; 13] = [
    0x48, 0x8B, 0x41, 0x10, 0xC5, 0xF8, 0x57, 0xC0, 0xC4, 0xE1, 0xFB, 0x2A, 0xC2,
];
