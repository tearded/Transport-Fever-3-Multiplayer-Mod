//! Terraforming through the room (docs/HOOKS.md, "Terraforming").
//!
//! The terrain tools (raise, lower, smooth, flatten and the heightmap brush:
//! `UI::TerrainModifier`), the terrain painter and the asset brush tell game
//! scripts nothing on build 40408: the game forwards `builder.proposalCreate`
//! for six other tools only. All three are `UI::ProposalAction`s, and queue
//! their `WorldBuildProposal` from `ProposalAction::DoApply` (0x549ac0), the
//! one call of `CommandList::Add` there (0x549be5, returning to 0x549bea),
//! after the factory 0x9ee860 made the command with `playerInitiated` 1. So
//! the click is counted and its apply stopped as every player's build is
//! ([`crate::builds`]), and at that call the hook reads the proposal here
//! ([`record`]): a terraform is the proposal's height grid and nothing else.
//! The GUI takes it for that click (`tpf3mp_native.built(n)`, as the module
//! editor's) and hands the room `Terraform` actions of it.
//!
//! Every game applies a terraform as the tool would: its mod's game script
//! arms the hook with the grid (`tpf3mp_native.terrain(t)`, [`arm`]) and
//! sends an empty proposal of its own, the carrier, as the player's build;
//! the hook fills the carrier's height grid at the build's apply, which it
//! sees while the room's actions are applied ([`inject`]), and the game
//! applies it as the tool's. A script cannot fill the grid itself: Lua's
//! `GridVec2f` has `width`, `height`, `x0`, `y0` and `at` and no setter
//! (`RegisterUsertypesBase`, 0x12e4580). TPF2's TPF2-MP did the same with a
//! file (`tpf2-multiplayer`, `mod/mp_lockstep_1/res/scripts/mp/terrain.lua`).
//!
//! The layout (build 40408, read with `tools/tpfre`):
//!
//! - `Proposal.terrain` (a `ProposalTerrain`) at + 0x2d8, its
//!   `baseHeightMod` first (the Lua bindings: `RegisterUsertypesTransport`,
//!   0x22c1ab0, registers `Proposal` with `terrain` at 0x2d8 and
//!   `ProposalTerrain` with `baseHeightMod` at 0);
//! - the `Proposal`'s destructor (0x48ee90) frees three vectors there, at
//!   0x2e8 (8-byte elements: the heights' `Vec2f` cells), 0x310 (bytes: the
//!   materials a paint sets) and 0x338 (4-byte words: their mask), each
//!   0x28 past the last: three grids `{ x0, y0, width, height; data }` of
//!   0x28 bytes, as TPF2's `Grid<T>`. The `Proposal` is 0x358 bytes
//!   (`ProposalAction::DoApply` frees it with that size);
//! - its vectors are allocated by the game's `operator new` (0x3184230,
//!   the UCRT's `malloc`) and freed by `operator delete` (0x3184970, `free`),
//!   with MSVC's alignment of 32 for 4 KiB or more, the block's own address
//!   just before the data (the destructor checks it).
//!
//! INFERRED, not yet seen in the game: the order of a grid's four integers
//! (TPF2's; a grid whose cell count is not width times height is refused),
//! that a cell is `{ height, height before }` (TPF2's; both are carried
//! whatever they are), that the cells go row by row, that the tool's
//! proposal has no street part, and that the carrier's grid is empty when
//! the game script sends it.

#![allow(unsafe_code)]

use std::sync::{Mutex, PoisonError};

use tpf3mp_proto::lua::LuaValue;

use crate::modules::{Memory, i32_at, read, vector};

/// The profile's name for `ProposalAction::DoApply`'s call of
/// `CommandList::Add`. `Add` returns 5 bytes past it.
pub const DO_APPLY_ADD_CALL: &str = "ProposalAction::DoApply/Add call";

pub use crate::build_data::native::terrain::layout;

/// Most cells one stroke's grid has before it is a misread: a square of 256
/// cells, 1 km on a side at 4 m. The GUI cuts what it hands the room into
/// actions the room carries ([`tpf3mp_proto::action::MAX_TERRAIN_CELLS`]).
pub const MAX_CELLS: usize = 65_536;

/// A terraform's grid: its first cell, its size in cells, and each cell's
/// two values, row by row.
#[derive(Debug, Clone, PartialEq)]
pub struct Grid {
    pub x0: i32,
    pub y0: i32,
    pub width: u32,
    pub height: u32,
    pub cells: Vec<(f32, f32)>,
}

impl Grid {
    /// The grid as the GUI takes it from `built(n)`: `{ terrain = { x0 =,
    /// y0 =, width =, height =, cells = { v1, w1, v2, w2, ... } } }`.
    pub fn to_lua(&self) -> LuaValue {
        let key = LuaValue::string;
        let cells = self
            .cells
            .iter()
            .flat_map(|(a, b)| [*a, *b])
            .enumerate()
            .map(|(i, v)| {
                (
                    LuaValue::Integer(i as i64 + 1),
                    LuaValue::Number(f64::from(v)),
                )
            })
            .collect();
        let grid = LuaValue::Table(vec![
            (key("x0"), LuaValue::Integer(i64::from(self.x0))),
            (key("y0"), LuaValue::Integer(i64::from(self.y0))),
            (key("width"), LuaValue::Integer(i64::from(self.width))),
            (key("height"), LuaValue::Integer(i64::from(self.height))),
            (key("cells"), LuaValue::Table(cells)),
        ]);
        LuaValue::Table(vec![(key("terrain"), grid)])
    }

    /// The grid `tpf3mp_native.terrain(t)` is given, `t` as [`Grid::to_lua`]'s
    /// `terrain` part: whole numbers in range, a cell count of width times
    /// height, every value finite. Anything else is refused.
    pub fn from_lua(value: &LuaValue) -> Result<Self, String> {
        let LuaValue::Table(entries) = value else {
            return Err("a terrain grid is a table".into());
        };
        let field = |name: &str| {
            entries
                .iter()
                .find(|(k, _)| *k == LuaValue::string(name))
                .map(|(_, v)| v)
                .ok_or_else(|| format!("a terrain grid with no {name}"))
        };
        let whole = |name: &str| -> Result<i64, String> {
            match field(name)? {
                LuaValue::Integer(n) => Ok(*n),
                LuaValue::Number(n) if n.fract() == 0.0 && n.abs() < 1e15 => Ok(*n as i64),
                _ => Err(format!("a terrain grid whose {name} is not a whole number")),
            }
        };
        let x0 = i32::try_from(whole("x0")?).map_err(|_| "x0 out of range".to_string())?;
        let y0 = i32::try_from(whole("y0")?).map_err(|_| "y0 out of range".to_string())?;
        let width =
            u32::try_from(whole("width")?).map_err(|_| "a width out of range".to_string())?;
        let height =
            u32::try_from(whole("height")?).map_err(|_| "a height out of range".to_string())?;
        let count = (width as usize)
            .checked_mul(height as usize)
            .filter(|n| *n > 0 && *n <= MAX_CELLS)
            .ok_or_else(|| format!("a terrain grid of {width} by {height} cells"))?;
        let LuaValue::Table(cells) = field("cells")? else {
            return Err("a terrain grid whose cells are not a list".into());
        };
        if cells.len() != 2 * count {
            return Err(format!(
                "a terrain grid of {width} by {height} cells with {} values",
                cells.len()
            ));
        }
        let mut values = vec![f32::NAN; 2 * count];
        for (k, v) in cells {
            let index = match k {
                LuaValue::Integer(n) => usize::try_from(*n).ok(),
                LuaValue::Number(n) if n.fract() == 0.0 && *n >= 1.0 => Some(*n as usize),
                _ => None,
            }
            .filter(|i| (1..=2 * count).contains(i))
            .ok_or_else(|| "a terrain grid's cells are not a list".to_string())?;
            let number = match v {
                LuaValue::Integer(n) => *n as f64,
                LuaValue::Number(n) => *n,
                _ => return Err("a terrain cell that is not a number".into()),
            };
            #[allow(clippy::cast_possible_truncation)]
            let single = number as f32;
            if !single.is_finite() {
                return Err("a terrain cell that is not finite".into());
            }
            values[index - 1] = single;
        }
        if values.iter().any(|v| v.is_nan()) {
            return Err("a terrain grid's cells are not a list".into());
        }
        Ok(Self {
            x0,
            y0,
            width,
            height,
            cells: values
                .as_chunks::<2>()
                .0
                .iter()
                .map(|c| (c[0], c[1]))
                .collect(),
        })
    }

    /// One line for the log.
    pub fn summary(&self) -> String {
        let (low, high) = self
            .cells
            .iter()
            .fold((f32::INFINITY, f32::NEG_INFINITY), |(lo, hi), (v, _)| {
                (lo.min(*v), hi.max(*v))
            });
        let changed = self.cells.iter().filter(|(v, b)| v != b).count();
        format!(
            "{} by {} cells from cell ({}, {}), {} changed, heights {:.2} to {:.2} m",
            self.width, self.height, self.x0, self.y0, changed, low, high
        )
    }
}

/// How many elements the vector at `offset` of `head` has.
fn count(head: &[u8], offset: usize, stride: usize, what: &str) -> Result<usize, String> {
    vector(head, offset, stride, usize::MAX / stride, what).map(|(_, n)| n)
}

/// Why the proposal in `head` is more than terrain, if it is.
fn only_terrain(head: &[u8]) -> Result<(), String> {
    for (offset, stride, what) in [
        (layout::ADDED_NODES, layout::NODE_SIZE, "the nodes added"),
        (
            layout::ADDED_SEGMENTS,
            layout::SEGMENT_SIZE,
            "the segments added",
        ),
        (
            layout::REMOVED_NODES,
            layout::NODE_SIZE,
            "the nodes removed",
        ),
        (
            layout::REMOVED_SEGMENTS,
            layout::SEGMENT_SIZE,
            "the segments removed",
        ),
        (
            layout::EDGE_OBJECTS_TO_ADD,
            layout::EDGE_OBJECT_SIZE,
            "the edge objects added",
        ),
        (
            layout::EDGE_OBJECTS_TO_REMOVE,
            4,
            "the edge objects removed",
        ),
        (layout::TO_REMOVE, 4, "the entities removed"),
    ] {
        if count(head, offset, stride, what)? > 0 {
            return Err(format!("a terrain tool's build with {what}"));
        }
    }
    if count(
        head,
        layout::TO_ADD,
        layout::ENTITY_SIZE,
        "the constructions added",
    )? > 0
    {
        return Err("the asset brush: the room does not carry it yet".into());
    }
    Ok(())
}

/// Reads the terraform of the `WorldBuildProposal` payload at `payload`: its
/// height grid, which must be all it changes.
pub fn decode(memory: &dyn Memory, payload: usize) -> Result<Grid, String> {
    let head = read(memory, payload, layout::PROPOSAL_LEN, "the proposal")?;
    only_terrain(&head)?;
    let materials = count(&head, layout::MATERIALS + layout::GRID_DATA, 1, "the paint")?;
    let mask = count(
        &head,
        layout::MASK + layout::GRID_DATA,
        4,
        "the paint's mask",
    )?;
    let (begin, cells) = vector(
        &head,
        layout::HEIGHTS + layout::GRID_DATA,
        layout::CELL_SIZE,
        MAX_CELLS,
        "the terrain grid",
    )?;
    if cells == 0 {
        if materials > 0 || mask > 0 {
            return Err("terrain paint: the room does not carry it yet".into());
        }
        return Err("a terrain tool's build that changes nothing".into());
    }
    if materials > 0 || mask > 0 {
        return Err("a terraform that paints too: the room does not carry paint yet".into());
    }
    let grid = &head[layout::HEIGHTS..layout::HEIGHTS + layout::GRID_SIZE];
    let x0 = i32_at(grid, layout::GRID_X0);
    let y0 = i32_at(grid, layout::GRID_Y0);
    let (width, height) = (
        i32_at(grid, layout::GRID_WIDTH),
        i32_at(grid, layout::GRID_HEIGHT),
    );
    let fits = u32::try_from(width)
        .ok()
        .zip(u32::try_from(height).ok())
        .filter(|(w, h)| (*w as usize).checked_mul(*h as usize) == Some(cells));
    let Some((width, height)) = fits else {
        return Err(format!(
            "a terrain grid of {width} by {height} with {cells} cells"
        ));
    };
    let raw = read(
        memory,
        begin,
        cells * layout::CELL_SIZE,
        "the terrain cells",
    )?;
    let float = |at: usize| {
        let mut word = [0u8; 4];
        word.copy_from_slice(&raw[at..at + 4]);
        f32::from_le_bytes(word)
    };
    let cells: Vec<(f32, f32)> = (0..cells)
        .map(|i| (float(i * 8), float(i * 8 + 4)))
        .collect();
    if cells.iter().any(|(a, b)| !a.is_finite() || !b.is_finite()) {
        return Err("a terrain cell that is not finite".into());
    }
    Ok(Grid {
        x0,
        y0,
        width,
        height,
        cells,
    })
}

/// Where `Add` returns to from `DoApply`'s call; 0 when the profile does not
/// name it.
static DO_APPLY_RETURN: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// Notes `DoApply`'s call of `Add`, at `call` (its absolute address).
pub fn set_call(call: usize) {
    DO_APPLY_RETURN.store(call + 5, std::sync::atomic::Ordering::Release);
}

/// Whether a call of `Add` returning to `return_address` is a terrain tool's
/// (or the painter's, or the asset brush's).
pub fn is_terrain_tool(return_address: usize) -> bool {
    let at = DO_APPLY_RETURN.load(std::sync::atomic::Ordering::Acquire);
    at != 0 && return_address == at
}

/// Keeps what a terrain tool queued at click `click` (the count before it)
/// for the GUI, as [`crate::modules::record`] keeps the module editor's:
/// the grid, or why the room cannot carry it. Logs one line.
pub fn record(memory: &dyn Memory, click: u64, payload: usize) {
    let read = decode(memory, payload);
    match &read {
        Ok(grid) => crate::log::line(&format!(
            "terraform: click {click} queued {}",
            grid.summary()
        )),
        Err(why) => crate::log::line(&format!(
            "terrain tool: click {click} cannot go to the room: {why}"
        )),
    }
    // A reason says whose it is, so the GUI does not take it for the module
    // editor's.
    crate::modules::keep(
        click,
        read.map(|grid| grid.to_lua())
            .map_err(|why| format!("terrain tool: {why}")),
    );
}

/// The grid armed for the next build the room's actions send, and whether
/// one was filled with it.
struct Armed {
    grid: Option<Grid>,
    filled: bool,
}

static ARMED: Mutex<Option<Armed>> = Mutex::new(None);

/// Arms the next build applied while the room's actions run with `grid`.
pub fn arm(grid: Grid) {
    *ARMED.lock().unwrap_or_else(PoisonError::into_inner) = Some(Armed {
        grid: Some(grid),
        filled: false,
    });
}

/// Disarms: whether a build was filled with the grid armed, or `None` when
/// none was armed.
pub fn disarm() -> Option<bool> {
    ARMED
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .take()
        .map(|armed| armed.filled)
}

/// Memory a carrier is filled in: the game's, or a test's.
pub trait Heap: Memory {
    /// Writes `bytes` at `address`; false when it cannot.
    fn write(&mut self, address: usize, bytes: &[u8]) -> bool;
    /// `len` bytes from the game's own allocator, as a `std::vector` of
    /// that many bytes allocates them, or `None`.
    fn allocate(&mut self, len: usize) -> Option<usize>;
}

/// Fills the carrier at `payload`, an empty proposal, with `grid`: the grid's
/// cells in a vector of the game's own, and its header. Refuses a carrier
/// with anything in it.
pub fn fill(heap: &mut dyn Heap, payload: usize, grid: &Grid) -> Result<(), String> {
    let head = read(heap, payload, layout::PROPOSAL_LEN, "the carrier")?;
    only_terrain(&head).map_err(|why| format!("the carrier is not empty: {why}"))?;
    let grids = &head[layout::HEIGHTS..layout::HEIGHTS + 3 * layout::GRID_SIZE];
    if grids.iter().any(|b| *b != 0) {
        return Err("the carrier's terrain grids are not empty".into());
    }
    let count = grid.cells.len();
    if count == 0 || count != (grid.width as usize) * (grid.height as usize) || count > MAX_CELLS {
        return Err("a grid whose cells are not width times height".into());
    }
    let mut bytes = Vec::with_capacity(count * layout::CELL_SIZE);
    for (a, b) in &grid.cells {
        bytes.extend_from_slice(&a.to_le_bytes());
        bytes.extend_from_slice(&b.to_le_bytes());
    }
    let data = heap
        .allocate(bytes.len())
        .ok_or_else(|| "the game's allocator gave nothing".to_string())?;
    if !heap.write(data, &bytes) {
        return Err("the cells could not be written".into());
    }
    let mut header = Vec::with_capacity(layout::GRID_SIZE);
    header.extend_from_slice(&grid.x0.to_le_bytes());
    header.extend_from_slice(&grid.y0.to_le_bytes());
    #[allow(clippy::cast_possible_wrap)]
    {
        header.extend_from_slice(&(grid.width as i32).to_le_bytes());
        header.extend_from_slice(&(grid.height as i32).to_le_bytes());
    }
    let end = (data + bytes.len()) as u64;
    header.extend_from_slice(&(data as u64).to_le_bytes());
    header.extend_from_slice(&end.to_le_bytes());
    header.extend_from_slice(&end.to_le_bytes());
    if !heap.write(payload + layout::HEIGHTS, &header) {
        return Err("the grid could not be written".into());
    }
    Ok(())
}

/// At the apply of a build the room's actions send: fills it with the grid
/// armed, if one is. `Ok(true)` when it filled, `Ok(false)` when nothing was
/// armed, `Err(why)` when the build is refused (the grid is then spent).
pub fn inject(heap: &mut dyn Heap, payload: usize) -> Result<bool, String> {
    let grid = {
        let mut armed = ARMED.lock().unwrap_or_else(PoisonError::into_inner);
        match armed.as_mut().and_then(|a| a.grid.take()) {
            Some(grid) => grid,
            None => return Ok(false),
        }
    };
    fill(heap, payload, &grid)?;
    if let Some(armed) = ARMED
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .as_mut()
    {
        armed.filled = true;
    }
    crate::log::line(&format!(
        "terraform: filled the room's carrier with {}",
        grid.summary()
    ));
    Ok(true)
}

/// The running game's memory and allocator.
pub struct GameHeap;

impl Memory for GameHeap {
    fn read(&self, address: usize, len: usize) -> Option<Vec<u8>> {
        crate::modules::Process.read(address, len)
    }
}

#[cfg(all(windows, target_arch = "x86_64"))]
impl Heap for GameHeap {
    fn write(&mut self, address: usize, bytes: &[u8]) -> bool {
        if address == 0 || !crate::image::readable(address, bytes.len()) {
            return false;
        }
        // SAFETY: committed memory of the game's heap (the payload being
        // applied, or a block its allocator just gave), checked readable;
        // the game's thread is inside the apply that owns it.
        unsafe { std::ptr::copy_nonoverlapping(bytes.as_ptr(), address as *mut u8, bytes.len()) };
        true
    }

    fn allocate(&mut self, len: usize) -> Option<usize> {
        let malloc = crt_malloc()?;
        if len < layout::BIG_ALLOCATION {
            // SAFETY: the UCRT's malloc, as the game's operator new calls it.
            let block = unsafe { malloc(len) } as usize;
            return (block != 0).then_some(block);
        }
        // SAFETY: as above.
        let block = unsafe { malloc(len + layout::BIG_EXTRA) } as usize;
        if block == 0 {
            return None;
        }
        let data = (block + layout::BIG_EXTRA) & !(layout::BIG_ALIGNMENT - 1);
        // SAFETY: `data - 8` is inside the block just allocated (at least 8
        // bytes past its start), where MSVC keeps the block's address.
        unsafe { ((data - 8) as *mut usize).write_unaligned(block) };
        Some(data)
    }
}

#[cfg(not(all(windows, target_arch = "x86_64")))]
impl Heap for GameHeap {
    fn write(&mut self, _address: usize, _bytes: &[u8]) -> bool {
        false
    }

    fn allocate(&mut self, _len: usize) -> Option<usize> {
        None
    }
}

/// The UCRT's `malloc`, which the game's `operator new` calls (it imports it
/// from `api-ms-win-crt-heap-l1-1-0.dll`, which forwards to `ucrtbase.dll`).
#[cfg(all(windows, target_arch = "x86_64"))]
fn crt_malloc() -> Option<unsafe extern "C" fn(usize) -> *mut u8> {
    use std::sync::OnceLock;
    use windows_sys::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress};
    static MALLOC: OnceLock<usize> = OnceLock::new();
    let address = *MALLOC.get_or_init(|| {
        for module in ["api-ms-win-crt-heap-l1-1-0.dll", "ucrtbase.dll"] {
            let wide: Vec<u16> = module.encode_utf16().chain(std::iter::once(0)).collect();
            // SAFETY: a NUL-terminated wide string; a loaded module's handle
            // is not ours to free.
            let handle = unsafe { GetModuleHandleW(wide.as_ptr()) };
            if handle.is_null() {
                continue;
            }
            // SAFETY: a valid module handle and a NUL-terminated name.
            if let Some(function) = unsafe { GetProcAddress(handle, c"malloc".as_ptr().cast()) } {
                return function as usize;
            }
        }
        0
    });
    // SAFETY: the UCRT's malloc, whose ABI this is.
    (address != 0).then(|| unsafe {
        std::mem::transmute::<usize, unsafe extern "C" fn(usize) -> *mut u8>(address)
    })
}

/// Serialises the tests that arm a grid.
#[cfg(test)]
pub(crate) static TEST_LOCK: Mutex<()> = Mutex::new(());

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modules::u64_at;

    /// Memory laid out by a test, with an allocator.
    struct Fake {
        regions: Vec<(usize, Vec<u8>)>,
        next: usize,
        allocated: Vec<usize>,
    }

    impl Memory for Fake {
        fn read(&self, address: usize, len: usize) -> Option<Vec<u8>> {
            self.regions.iter().find_map(|(base, bytes)| {
                let offset = address.checked_sub(*base)?;
                bytes
                    .get(offset..offset.checked_add(len)?)
                    .map(<[u8]>::to_vec)
            })
        }
    }

    impl Heap for Fake {
        fn write(&mut self, address: usize, bytes: &[u8]) -> bool {
            for (base, region) in &mut self.regions {
                if let Some(offset) = address.checked_sub(*base)
                    && offset + bytes.len() <= region.len()
                {
                    region[offset..offset + bytes.len()].copy_from_slice(bytes);
                    return true;
                }
            }
            false
        }

        fn allocate(&mut self, len: usize) -> Option<usize> {
            let at = self.alloc(vec![0; len]);
            self.allocated.push(at);
            Some(at)
        }
    }

    impl Fake {
        fn new() -> Self {
            Self {
                regions: Vec::new(),
                next: 0x100_0000,
                allocated: Vec::new(),
            }
        }
        fn alloc(&mut self, bytes: Vec<u8>) -> usize {
            let at = self.next;
            self.next += ((bytes.len() + 0xfff) & !0xfff) + 0x1000;
            self.regions.push((at, bytes));
            at
        }
        fn bytes(&mut self, at: usize) -> &mut Vec<u8> {
            &mut self.regions.iter_mut().find(|(b, _)| *b == at).unwrap().1
        }
    }

    fn put_u64(bytes: &mut [u8], at: usize, value: u64) {
        bytes[at..at + 8].copy_from_slice(&value.to_le_bytes());
    }

    fn put_i32(bytes: &mut [u8], at: usize, value: i32) {
        bytes[at..at + 4].copy_from_slice(&value.to_le_bytes());
    }

    /// A vector of `count` elements of `stride` bytes at `offset` of `head`,
    /// its data at `base`.
    fn put_vector(head: &mut [u8], offset: usize, base: usize, count: usize, stride: usize) {
        put_u64(head, offset, base as u64);
        put_u64(head, offset + 8, (base + count * stride) as u64);
        put_u64(head, offset + 16, (base + count * stride) as u64);
    }

    /// A terrain tool's proposal: a 3 by 2 grid from cell (-10, 7), each
    /// cell `{ 100 + i, 100 }`.
    fn stroke() -> (Fake, usize) {
        let mut fake = Fake::new();
        let mut cells = Vec::new();
        for i in 0..6u8 {
            cells.extend_from_slice(&(100.0f32 + f32::from(i)).to_le_bytes());
            cells.extend_from_slice(&100.0f32.to_le_bytes());
        }
        let data = fake.alloc(cells);
        let payload = fake.alloc(vec![0; layout::PROPOSAL_LEN + 0x100]);
        let head = fake.bytes(payload);
        put_i32(head, layout::HEIGHTS + layout::GRID_X0, -10);
        put_i32(head, layout::HEIGHTS + layout::GRID_Y0, 7);
        put_i32(head, layout::HEIGHTS + layout::GRID_WIDTH, 3);
        put_i32(head, layout::HEIGHTS + layout::GRID_HEIGHT, 2);
        put_vector(head, layout::HEIGHTS + layout::GRID_DATA, data, 6, 8);
        (fake, payload)
    }

    #[test]
    fn a_terraform_reads_as_its_height_grid() {
        let (fake, payload) = stroke();
        let grid = decode(&fake, payload).unwrap();
        assert_eq!((grid.x0, grid.y0, grid.width, grid.height), (-10, 7, 3, 2));
        assert_eq!(grid.cells[0], (100.0, 100.0));
        assert_eq!(grid.cells[5], (105.0, 100.0));
        assert_eq!(
            grid.summary(),
            "3 by 2 cells from cell (-10, 7), 5 changed, heights 100.00 to 105.00 m"
        );
        // As the GUI takes it, and as the game script gives it back.
        let lua = grid.to_lua();
        let LuaValue::Table(top) = &lua else { panic!() };
        assert_eq!(Grid::from_lua(&top[0].1).unwrap(), grid);
    }

    #[test]
    fn what_is_not_only_a_height_grid_is_refused_with_why() {
        // A grid whose cells are not width times height: a misread.
        let (mut fake, payload) = stroke();
        put_i32(fake.bytes(payload), layout::HEIGHTS + layout::GRID_WIDTH, 4);
        assert!(
            decode(&fake, payload)
                .unwrap_err()
                .contains("4 by 2 with 6 cells")
        );
        // The painter's: no heights, a material grid.
        let (mut fake, payload) = stroke();
        let paint = fake.alloc(vec![1; 16]);
        let head = fake.bytes(payload);
        put_vector(head, layout::HEIGHTS + layout::GRID_DATA, 0, 0, 8);
        put_vector(head, layout::MATERIALS + layout::GRID_DATA, paint, 16, 1);
        assert_eq!(
            decode(&fake, payload).unwrap_err(),
            "terrain paint: the room does not carry it yet"
        );
        // Heights and paint at once.
        let (mut fake, payload) = stroke();
        let mask = fake.alloc(vec![1; 16]);
        put_vector(
            fake.bytes(payload),
            layout::MASK + layout::GRID_DATA,
            mask,
            4,
            4,
        );
        assert!(decode(&fake, payload).unwrap_err().contains("paints too"));
        // The asset brush: constructions added.
        let (mut fake, payload) = stroke();
        let entity = fake.alloc(vec![0; layout::ENTITY_SIZE]);
        put_vector(
            fake.bytes(payload),
            layout::TO_ADD,
            entity,
            1,
            layout::ENTITY_SIZE,
        );
        assert_eq!(
            decode(&fake, payload).unwrap_err(),
            "the asset brush: the room does not carry it yet"
        );
        // A street part.
        let (mut fake, payload) = stroke();
        let node = fake.alloc(vec![0; layout::NODE_SIZE]);
        put_vector(
            fake.bytes(payload),
            layout::ADDED_NODES,
            node,
            1,
            layout::NODE_SIZE,
        );
        assert!(
            decode(&fake, payload)
                .unwrap_err()
                .contains("the nodes added")
        );
        // A cell that is not a number.
        let (mut fake, payload) = stroke();
        let data = u64_at(fake.bytes(payload), layout::HEIGHTS + layout::GRID_DATA) as usize;
        fake.bytes(data)[0..4].copy_from_slice(&f32::NAN.to_le_bytes());
        assert!(decode(&fake, payload).unwrap_err().contains("not finite"));
    }

    #[test]
    fn a_carrier_is_filled_with_the_armed_grid_once() {
        let _lock = TEST_LOCK.lock().unwrap_or_else(PoisonError::into_inner);
        let (source, at) = stroke();
        let grid = decode(&source, at).unwrap();
        let mut fake = Fake::new();
        let carrier = fake.alloc(vec![0; layout::PROPOSAL_LEN]);
        // Nothing armed: the build goes as it is.
        assert_eq!(disarm(), None);
        assert_eq!(inject(&mut fake, carrier), Ok(false));
        arm(grid.clone());
        assert_eq!(inject(&mut fake, carrier), Ok(true));
        // The carrier now reads as the stroke did.
        assert_eq!(decode(&fake, carrier).unwrap(), grid);
        assert_eq!(fake.allocated.len(), 1);
        // Spent: a second build is left alone; disarming says it was used.
        let other = fake.alloc(vec![0; layout::PROPOSAL_LEN]);
        assert_eq!(inject(&mut fake, other), Ok(false));
        assert_eq!(disarm(), Some(true));
        assert_eq!(disarm(), None);
    }

    #[test]
    fn a_carrier_with_anything_in_it_is_refused() {
        let _lock = TEST_LOCK.lock().unwrap_or_else(PoisonError::into_inner);
        let (mut fake, payload) = stroke();
        let grid = decode(&fake, payload).unwrap();
        arm(grid);
        // The stroke's own proposal is no empty carrier.
        assert!(
            inject(&mut fake, payload)
                .unwrap_err()
                .contains("terrain grids are not empty")
        );
        assert_eq!(disarm(), Some(false), "armed, and filled nothing");
    }

    #[test]
    fn the_grid_the_game_script_gives_is_checked() {
        let grid = |cells: Vec<LuaValue>, width: i64| {
            let key = LuaValue::string;
            LuaValue::Table(vec![
                (key("x0"), LuaValue::Integer(0)),
                (key("y0"), LuaValue::Integer(0)),
                (key("width"), LuaValue::Integer(width)),
                (key("height"), LuaValue::Integer(1)),
                (
                    key("cells"),
                    LuaValue::Table(
                        cells
                            .into_iter()
                            .enumerate()
                            .map(|(i, v)| (LuaValue::Integer(i as i64 + 1), v))
                            .collect(),
                    ),
                ),
            ])
        };
        let ok = Grid::from_lua(&grid(vec![LuaValue::Number(1.5), LuaValue::Integer(1)], 1));
        assert_eq!(ok.unwrap().cells, vec![(1.5, 1.0)]);
        assert!(Grid::from_lua(&grid(vec![LuaValue::Number(1.5)], 1)).is_err());
        assert!(
            Grid::from_lua(&grid(
                vec![LuaValue::Number(f64::INFINITY), LuaValue::Integer(1)],
                1
            ))
            .is_err()
        );
        assert!(Grid::from_lua(&grid(vec![], 0)).is_err());
        assert!(Grid::from_lua(&LuaValue::Integer(3)).is_err());
    }
}
