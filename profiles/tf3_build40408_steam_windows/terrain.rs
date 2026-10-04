//! Native data for Steam Windows build 40408. See hooks.toml for identity.

/// Every offset read or written, from the payload (whose `Proposal` is at
/// its start, as [`crate::modules`] reads it).
pub mod layout {
    pub use crate::modules::layout::{
        ADDED_NODES, ADDED_SEGMENTS, EDGE_OBJECT_SIZE, EDGE_OBJECTS_TO_ADD, EDGE_OBJECTS_TO_REMOVE,
        ENTITY_SIZE, NODE_SIZE, REMOVED_NODES, REMOVED_SEGMENTS, SEGMENT_SIZE, TO_ADD, TO_REMOVE,
    };

    /// `Proposal.terrain.baseHeightMod`, a grid of `Vec2f`.
    pub const HEIGHTS: usize = 0x2d8;
    /// The grids a paint sets: materials (bytes) and their mask (words).
    pub const MATERIALS: usize = HEIGHTS + GRID_SIZE;
    pub const MASK: usize = MATERIALS + GRID_SIZE;
    /// The whole `Proposal`.
    pub const PROPOSAL_LEN: usize = 0x358;

    pub const GRID_SIZE: usize = 0x28;
    pub const GRID_X0: usize = 0x00;
    pub const GRID_Y0: usize = 0x04;
    pub const GRID_WIDTH: usize = 0x08;
    pub const GRID_HEIGHT: usize = 0x0c;
    pub const GRID_DATA: usize = 0x10;

    /// One height cell: two `f32`.
    pub const CELL_SIZE: usize = 8;
    /// MSVC's allocation of 4 KiB or more: aligned to 32, the block's own
    /// address in the 8 bytes before the data, 39 bytes more asked for.
    pub const BIG_ALLOCATION: usize = 0x1000;
    pub const BIG_ALIGNMENT: usize = 32;
    pub const BIG_EXTRA: usize = 8 + BIG_ALIGNMENT - 1;
}
