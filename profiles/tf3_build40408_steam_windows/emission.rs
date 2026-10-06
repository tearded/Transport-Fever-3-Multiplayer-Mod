//! Native data for Steam Windows build 40408's fast emission grid
//! (crates/tpf3mp-hook/src/emission; investigation/
//! TF3_SIM_COST_2026-10-05.md §1). Every address is relative to
//! `ecs::EmissionGridSystem::Update`, the one profile target; see hooks.toml
//! for its signature.

/// `ecs::EmissionGridSystem::Update(this, engine, nodes, dt)` (0xaa9230).
pub const UPDATE: &str = "emission::EmissionGridSystem::Update";

/// A stretch of the game's code the fast path models, by its FNV-1a 64
/// hash: anything else there leaves the game's own update.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Code {
    pub what: &'static str,
    /// From `Update`, in bytes.
    pub offset: i64,
    pub len: usize,
    pub fnv1a: u64,
}

/// `LoopImpl` over Diffuse's rows (0xaa79d0).
pub const DIFFUSE_DISPATCH: i64 = -0x1860;
/// `LoopImpl` over Wind's rows (0xaa7b80), pollution only.
pub const WIND_DISPATCH: i64 = -0x16b0;
/// `LoopImpl` over Average's rows (0xaa7d20).
pub const AVERAGE_DISPATCH: i64 = -0x1510;
/// The row kernels (`Diffuse` 0xaa8570, `Wind` 0xaa96f0, `Average`
/// 0xaa8190), as each dispatcher's inline path calls them.
pub const DIFFUSE_KERNEL: i64 = -0xcc0;
pub const WIND_KERNEL: i64 = 0x4c0;
pub const AVERAGE_KERNEL: i64 = -0x10a0;

/// `Update`'s three `call <dispatcher>` sites (0xaa9422, 0xaa94ce,
/// 0xaa954c), which the hook redirects.
pub const DIFFUSE_CALL: i64 = 0x1f2;
pub const WIND_CALL: i64 = 0x29e;
pub const AVERAGE_CALL: i64 = 0x31c;

/// The modelled code: `Update`, the three dispatchers and the three
/// kernels, whole.
pub const CODE: [Code; 7] = [
    Code {
        what: "EmissionGridSystem::Update",
        offset: 0,
        len: 0x4b2,
        fnv1a: 0xc75c_fc42_6e79_d2a6,
    },
    Code {
        what: "the Diffuse dispatcher",
        offset: DIFFUSE_DISPATCH,
        len: 0x1a8,
        fnv1a: 0x9e39_6f63_41c5_32a6,
    },
    Code {
        what: "the Wind dispatcher",
        offset: WIND_DISPATCH,
        len: 0x197,
        fnv1a: 0xb0ce_7e81_c275_9b5b,
    },
    Code {
        what: "the Average dispatcher",
        offset: AVERAGE_DISPATCH,
        len: 0x170,
        fnv1a: 0x8f2b_5236_3033_28b7,
    },
    Code {
        what: "the Diffuse kernel",
        offset: DIFFUSE_KERNEL,
        len: 0x416,
        fnv1a: 0x795f_997d_a993_3c74,
    },
    Code {
        what: "the Average kernel",
        offset: AVERAGE_KERNEL,
        len: 0x1b5,
        fnv1a: 0x29aa_4cdd_7465_dca2,
    },
    Code {
        what: "the Wind kernel",
        offset: WIND_KERNEL,
        len: 0x4e2,
        fnv1a: 0xbc89_f50a_6b53_5145,
    },
];

/// The component's fields (`ecs::component::EmissionGrid`): the grid
/// point size (2 x f32), the concentration grid, the average grid, the
/// wind (2 x f32). A grid is `{i32 x0, i32 y0, i32 width, i32 height,
/// std::vector<float>}` (0x28 bytes).
pub const COMP_GRID_POINT_SIZE: usize = 0x10;
pub const COMP_CONCENTRATION: usize = 0x18;
pub const COMP_AVERAGE: usize = 0x40;
pub const COMP_WIND: usize = 0x6c;
/// The system's fields: the shared temporary grid and Average's,
/// Diffuse's decay and Diffuse's spread coefficients (f32).
pub const SYSTEM_TEMP: usize = 0x18;
pub const SYSTEM_AVERAGE_C: usize = 0x48;
pub const SYSTEM_DECAY_B: usize = 0x4c;
pub const SYSTEM_SPREAD_A: usize = 0x50;
/// Where a grid keeps its width, height and data vector.
pub const GRID_WIDTH: usize = 8;
pub const GRID_HEIGHT: usize = 0xc;
pub const GRID_DATA: usize = 0x10;
