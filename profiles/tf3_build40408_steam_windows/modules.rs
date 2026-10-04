//! Native data for Steam Windows build 40408. See hooks.toml for identity.

/// Every offset read.
pub mod layout {
    pub const ADDED_NODES: usize = 0x000;
    pub const ADDED_SEGMENTS: usize = 0x018;
    pub const REMOVED_NODES: usize = 0x030;
    pub const REMOVED_SEGMENTS: usize = 0x048;
    pub const EDGE_OBJECTS_TO_REMOVE: usize = 0x110;
    pub const EDGE_OBJECTS_TO_ADD: usize = 0x128;
    pub const TO_REMOVE: usize = 0x240;
    pub const TO_ADD: usize = 0x258;
    pub const HEAD_LEN: usize = 0x270;

    pub const NODE_SIZE: usize = 0x40;
    pub const SEGMENT_SIZE: usize = 0x350;
    pub const EDGE_OBJECT_SIZE: usize = 0x1a0;

    pub const ENTITY_SIZE: usize = 0xe48;
    pub const ENTITY_FILE: usize = 0x000;
    pub const ENTITY_PARAMS: usize = 0xa18;
    pub const ENTITY_TRANSF: usize = 0xc98;
    pub const ENTITY_NAME: usize = 0xe18;

    pub const STRING_SIZE: usize = 0x20;
    pub const STRING_LEN: usize = 0x10;
    pub const STRING_CAPACITY: usize = 0x18;
    pub const STRING_INLINE: u64 = 0x10;

    pub const TABLE_SIZE: usize = 0x18;
    pub const TABLE_ROOT: usize = 0x00;
    pub const TABLE_COUNT: usize = 0x10;
    pub const NODE_START: usize = 0x09;
    pub const NODE_FINISH: usize = 0x0a;
    pub const NODE_MAX_COUNT: usize = 0x0b;
    pub const NODE_HEADER: usize = 0x10;
    pub const SLOT_SIZE: usize = 0x50;
    pub const SLOT_VALUE: usize = 0x28;
    pub const NODE_SLOTS: usize = 3;
    pub const NODE_CHILDREN: usize = NODE_HEADER + NODE_SLOTS * SLOT_SIZE;

    pub const VARIANT_SIZE: usize = 0x28;
    pub const VARIANT_TAG: usize = 0x20;
    pub const TAG_BOOL: i8 = 1;
    pub const TAG_NUMBER: i8 = 2;
    pub const TAG_STRING: i8 = 3;
    pub const TAG_TABLE: i8 = 4;
}
