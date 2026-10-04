//! Loading the room's world from the game's main menu (docs/HOOKS.md, "The
//! room's world"; `investigation/TPF3_MENU_JOIN_2026-09-30.md`).
//!
//! The mod's GUI runs only in a world, so a game at its main menu has no Lua
//! of the mod's to load the room's save. The menu itself loads saves from
//! Lua: its Load Game page builds a `SavegameId` with
//! `api.type.SavegameId.new()` and calls `app.loadGame(id, false, nil)`
//! (`gui/menu/savegame_react_util.tl`, build 40408). The hook does the same,
//! in the menu's own Lua state, on the menu's own frame:
//!
//! - **Finding the state.** The game gives a Lua state its `app` table in
//!   one function, `RegisterAppUsertypes(lua::State&, UI::CMenuUI&,
//!   std::function<bool()> const&)` (profile target
//!   [`REGISTER_APP_TARGET`]). The hook detours it and, after the game's own
//!   registration, runs [`CHUNK`] in that state ([`adopt`]): Lua that hands
//!   the hook a function loading a save by name, kept in the state's
//!   registry, and a sentinel whose `__gc` tells the hook the state is gone.
//!   Lua 5.2 runs every finalizer when a state closes, so the hook never
//!   calls into a state that no longer exists.
//! - **Running the load.** `UI::CMenuUI::DoStep`, the menu's per-frame
//!   update on the main thread ([`MENU_STEP_TARGET`]), is detoured; after
//!   the game's own frame, `crate::install::menu_frame` drives the room from
//!   there while the game is at its main menu with no world
//!   (`crate::at_menu`), and a load the room ordered is started with
//!   [`serve`] in the newest menu state adopted on that thread.
//! - **Knowing whether a world is loaded.** `DoStep` tests
//!   `CMenuUI::m_game`, the loaded world, before it hands its frame to the
//!   world's UI; the profile target [`MENU_GAME_TARGET`] is that test, and
//!   its displacement is the field's offset ([`world_loaded`]). A state the
//!   game gives `app` while a world is loaded is the world's GUI's: it is
//!   never the menu's, and it is forgotten when the world closes
//!   ([`forget_world_states`]). Loading is read natively from the verified
//!   `CMenuUI::m_loadGameResult` field ([`load_in_progress`]); querying the
//!   progress monitor here can wait on a lock held by the loader.
//!
//! Without `app.setWaitForStartReadyGame()`, which the menu's own pages call
//! first, the game starts the loaded world by itself, with no Start Game
//! button (the game's `api/tealdef/app.d.tl`). A save the menu loads keeps its
//! own mod list (`info` is nil), and the room's save comes from a game whose
//! GUI had TPF3-MP linked, so the room's world loads with TPF3-MP active.
//!
//! Anything missing (a target, the Lua API, an adopted state on the menu's
//! thread) leaves the menu alone and the game as before: the room's world is
//! then loaded only by the GUI of a world the player has up.

#![allow(unsafe_code)]
// Elsewhere the detours are not installed, so their code is unused there.
#![cfg_attr(not(all(windows, target_arch = "x86_64")), allow(dead_code))]

use std::{
    ffi::{c_char, c_int, c_void},
    panic::AssertUnwindSafe,
    sync::{
        Mutex, MutexGuard, OnceLock, PoisonError,
        atomic::{AtomicU64, AtomicUsize, Ordering},
    },
    thread::ThreadId,
};

use crate::lua::{self, CFunction, LuaApi, State};

pub use crate::build_data::native::menu::LOAD_TARGET;
pub use crate::build_data::native::menu::MENU_GAME_OPCODE;
pub use crate::build_data::native::menu::MENU_GAME_TARGET;
pub use crate::build_data::native::menu::MENU_LOAD_OPCODE;
pub use crate::build_data::native::menu::MENU_LOAD_TARGET;
pub use crate::build_data::native::menu::MENU_STEP_TARGET;
pub use crate::build_data::native::menu::PCALL_TARGET;
pub use crate::build_data::native::menu::REF_TARGET;
pub use crate::build_data::native::menu::REGISTER_APP_TARGET;

pub use crate::build_data::native::menu::LUA52_REGISTRY;

use crate::build_data::native::menu::TFUNCTION;
use crate::build_data::native::menu::TSTRING;

/// A `lua_Reader`.
pub type Reader = unsafe extern "C-unwind" fn(State, *mut c_void, *mut usize) -> *const c_char;

/// What the menu needs of Lua beyond the link's [`LuaApi`], with Lua 5.2's
/// signatures.
#[derive(Clone, Copy)]
pub struct MenuApi {
    /// `lua_load(L, reader, data, chunkname, mode)`.
    pub load: unsafe extern "C-unwind" fn(
        State,
        Reader,
        *mut c_void,
        *const c_char,
        *const c_char,
    ) -> c_int,
    /// `lua_pcallk(L, nargs, nresults, errfunc, ctx, k)`.
    pub pcallk:
        unsafe extern "C-unwind" fn(State, c_int, c_int, c_int, c_int, *const c_void) -> c_int,
    /// `luaL_ref(L, t)`.
    pub reference: unsafe extern "C-unwind" fn(State, c_int) -> c_int,
    /// `LUA_REGISTRYINDEX`.
    pub registry: c_int,
}

static MENU_API: OnceLock<MenuApi> = OnceLock::new();

/// Makes `api` the one the menu uses; the first one stays. Returns whether
/// this one was taken.
pub fn install_api(api: MenuApi) -> bool {
    MENU_API.set(api).is_ok()
}

/// Run once in each Lua state the game gives `app`, with functions of the
/// hook's as its arguments: `here(load, busy)`, which keeps `load` and
/// `busy` and returns the state's number, `gone(number)`, which the
/// sentinel's finalizer calls when the state closes, and `plan`, the
/// room's mods.
///
/// `load(name)` loads the save `name` of the game's save folder, as the
/// menu's Load Game page does, except that it does not ask the game to wait
/// for Start Game. It answers `"started"`, `"busy"` while save details or
/// lobby removal are pending, or why it could not. The caller first checks
/// the native load field and never calls this during a load.
///
/// `busy()` is a compatibility placeholder returning nil; it queries no API.
pub const CHUNK: &str = r#"
local here, gone, plan = ...
local number
local sentinel = setmetatable({}, { __gc = function() if number then gone(number) end end })
-- A save whose details are being read, for the mods it lists.
local reading
local function savegameId(theApp, name)
	local id = api.type.SavegameId.new()
	id.path = ""
	id.saveGameName = name
	id.saveGameNamespace = theApp.SaveGameNamespace.getSavegame()
	return id
end
-- The save's details with the mods the room's world loads with in this
-- game, nil for the save's own, or nil and why it cannot load.
local function withMods(theApp, data)
	local names = {}
	for _, m in ipairs(data.info.mods or {}) do names[#names + 1] = m.name end
	local list = plan(table.concat(names, "\n"))
	if list == nil then return nil end
	local modRep = theApp.getUserProfile():getModRep()
	local mods = {}
	for name in string.gmatch(list, "[^\n]+") do
		local m = api.type.ModId.new()
		m.name = name
		if not modRep:exists(m) then
			return nil, "the room's world needs the mod " .. name .. ", which is not installed"
		end
		mods[#mods + 1] = m
	end
	local info = api.type.SaveGameDetails.new(data.info)
	info.mods = mods
	return info
end
local function load(name)
	local keep = sentinel
	local found, theApp = pcall(function() return app end)
	if not found or theApp == nil then return "this Lua state has no app" end
	-- Dismiss the lobby while the menu still runs. Loading suspends its
	-- callbacks, so a state-poll-based close can leave a frozen overlay.
	if resolveutil and resolveutil.__tpf3mp_before_load then
		local ok, err = pcall(resolveutil.__tpf3mp_before_load)
		if not ok then return "closing the multiplayer menu failed: " .. tostring(err) end
		resolveutil.__tpf3mp_before_load = nil
		return "busy" -- let the menu apply the removal before starting a load
	end
	local info = nil
	if plan and plan() then
		-- The room's mods, not the save's: its details first, read by the
		-- game in the background; asked again until they are.
		if reading == nil or reading.name ~= name then
			local ok, async = pcall(theApp.getSavegameInfo, savegameId(theApp, name))
			if not ok then return "reading the save's mods failed: " .. tostring(async) end
			reading = { name = name, async = async }
		end
		if not reading.async:isCompleted() then return "busy" end
		local data = reading.async:get()
		reading = nil
		if data == nil or data.info == nil then
			return "the save's mods did not read: " .. tostring(data and data.errorMsg)
		end
		local ok, made, why = pcall(withMods, theApp, data)
		if not ok then return "the room's mods for the save failed: " .. tostring(made) end
		if made == nil and why then return why end
		info = made
	end
	local ok, err = pcall(function()
		theApp.loadGame(savegameId(theApp, name), false, info)
	end)
	if ok then return "started" end
	return "app.loadGame failed: " .. tostring(err)
end
local function busy()
	return nil
end
number = here(load, busy)
"#;

/// A state that ran [`CHUNK`] and has not closed.
struct Adopted {
    number: u64,
    state: usize,
    /// Its `load`, in the state's registry.
    reference: c_int,
    /// Its `busy`, in the state's registry; negative without one.
    busy: c_int,
    /// The thread it was adopted on: the menu only calls into a state on
    /// that thread.
    thread: ThreadId,
    /// Adopted while a world was loaded: the world's GUI's, never the
    /// menu's.
    world: bool,
}

impl Adopted {
    /// A state of the menu's on this thread.
    fn menus(&self, here: ThreadId) -> bool {
        self.thread == here && !self.world
    }
}

static ADOPTED: Mutex<Vec<Adopted>> = Mutex::new(Vec::new());
static NEXT: AtomicU64 = AtomicU64::new(1);

fn adopted() -> MutexGuard<'static, Vec<Adopted>> {
    ADOPTED.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Whether the menu can load a save from this thread: a state of the
/// menu's adopted on it is open.
pub fn available() -> bool {
    let here = std::thread::current().id();
    adopted().iter().any(|state| state.menus(here))
}

/// Forgets the states adopted while a world was loaded, once that world
/// has closed: they were its GUI's, and the menu never calls them. Returns
/// how many.
pub fn forget_world_states() -> usize {
    let mut adopted = adopted();
    let before = adopted.len();
    adopted.retain(|state| !state.world);
    before - adopted.len()
}

/// The offset of `CMenuUI::m_game` in the menu, from [`MENU_GAME_TARGET`];
/// 0 while unknown.
static GAME_FIELD: AtomicUsize = AtomicUsize::new(0);

/// The offset of `CMenuUI::m_loadGameResult` in the menu, from
/// [`MENU_LOAD_TARGET`]; 0 while unknown.
static LOAD_FIELD: AtomicUsize = AtomicUsize::new(0);

/// The field offset an instruction `opcode [rsi+disp32]` at `code` reads,
/// if it is that instruction and the offset is plausible for a pointer
/// field.
fn field_at(code: &[u8], opcode: [u8; 3]) -> Option<usize> {
    if code.get(..3)? != opcode {
        return None;
    }
    let disp = i32::from_le_bytes(code.get(3..7)?.try_into().ok()?);
    let offset = usize::try_from(disp).ok()?;
    (offset > 0 && offset < 0x1_0000 && offset % 8 == 0).then_some(offset)
}

/// The offset of `CMenuUI::m_game` that `DoStep`'s test at `code` (the
/// instruction [`MENU_GAME_TARGET`] names) reads.
pub fn game_field_at(code: &[u8]) -> Option<usize> {
    field_at(code, MENU_GAME_OPCODE)
}

/// The offset of `CMenuUI::m_loadGameResult` that `DoStep`'s read at `code`
/// (the instruction [`MENU_LOAD_TARGET`] names) reads.
pub fn load_field_at(code: &[u8]) -> Option<usize> {
    field_at(code, MENU_LOAD_OPCODE)
}

/// The worlds that closed in this game other than for a load the hook
/// started ([`note_world_closed`]).
static WORLD_CLOSES: AtomicU64 = AtomicU64::new(0);

/// A world closed (`CMenuUI::m_game` cleared), and no load the hook started
/// closed it: the player left it, for the main menu, a new game or a save of
/// their own (`crate::install`'s menu frame).
pub fn note_world_closed() {
    WORLD_CLOSES.fetch_add(1, Ordering::AcqRel);
}

/// How many worlds closed as [`note_world_closed`] counts them, or `None`
/// where the hook cannot see a world close: no `CMenuUI::m_game` known
/// (`crate::step::WorldMark::closed`).
pub fn world_closes() -> Option<u64> {
    (GAME_FIELD.load(Ordering::Acquire) != 0).then(|| WORLD_CLOSES.load(Ordering::Acquire))
}

/// Makes `offset` the one [`world_loaded`] reads; 0 forgets it.
pub fn set_game_field(offset: usize) {
    GAME_FIELD.store(offset, Ordering::Release);
}

/// Makes `offset` the one [`load_in_progress`] reads; 0 forgets it.
pub fn set_load_field(offset: usize) {
    LOAD_FIELD.store(offset, Ordering::Release);
}

/// Reads the pointer at `menu + offset`: whether it is set, `None` when
/// either is unknown.
///
/// # Safety
///
/// As [`world_loaded`], `offset` a pointer field the game's own `DoStep`
/// reads in the menu.
unsafe fn pointer_set(menu: usize, offset: usize) -> Option<bool> {
    if offset == 0 || menu == 0 {
        return None;
    }
    // SAFETY: the caller's: the menu is live and at least as large as the
    // field the game's own DoStep reads at this offset; the read is of one
    // aligned pointer, which is never dereferenced, and takes no lock.
    let value = unsafe { std::ptr::read_volatile((menu + offset) as *const usize) };
    Some(value != 0)
}

/// Whether the menu `menu` is loading a world: its `m_loadGameResult`, the
/// future of the load under way, is set. `None` when the offset is unknown
/// or there is no menu. A plain read of the menu's field on its own thread:
/// no Lua, no lock of the game's.
///
/// # Safety
///
/// As [`world_loaded`].
pub unsafe fn load_in_progress(menu: usize) -> Option<bool> {
    // SAFETY: the caller's.
    unsafe { pointer_set(menu, LOAD_FIELD.load(Ordering::Acquire)) }
}

/// The value of `CMenuUI::m_loadGameResult` and its offset, for the log;
/// `None` when either is unknown.
///
/// # Safety
///
/// As [`world_loaded`].
pub unsafe fn load_field_value(menu: usize) -> Option<(usize, usize)> {
    let offset = LOAD_FIELD.load(Ordering::Acquire);
    if offset == 0 || menu == 0 {
        return None;
    }
    // SAFETY: as in pointer_set.
    let value = unsafe { std::ptr::read_volatile((menu + offset) as *const usize) };
    Some((offset, value))
}

/// Whether the menu `menu` (a live `UI::CMenuUI`) has a world loaded: its
/// `m_game` is set. `None` when the offset is unknown or there is no menu.
///
/// # Safety
///
/// `menu` is 0 or the game's live `UI::CMenuUI`, as `DoStep` and
/// `RegisterAppUsertypes` are handed it, on the thread that runs it.
pub unsafe fn world_loaded(menu: usize) -> Option<bool> {
    // SAFETY: the caller's.
    unsafe { pointer_set(menu, GAME_FIELD.load(Ordering::Acquire)) }
}

/// One buffer handed to `lua_load`, whole, once.
struct Chunk {
    text: &'static str,
    done: bool,
}

unsafe extern "C-unwind" fn read_chunk(
    _l: State,
    data: *mut c_void,
    size: *mut usize,
) -> *const c_char {
    // SAFETY: `data` is the `Chunk` `adopt` passed to lua_load, and `size`
    // its out-parameter.
    unsafe {
        let chunk = &mut *data.cast::<Chunk>();
        if chunk.done {
            *size = 0;
            return std::ptr::null();
        }
        chunk.done = true;
        *size = chunk.text.len();
        chunk.text.as_ptr().cast()
    }
}

/// The string at the top of `l`'s stack, if it is one.
///
/// # Safety
///
/// `l` is live, on this thread.
unsafe fn string_at_top(api: &LuaApi, l: State) -> Option<String> {
    // SAFETY: the caller's; only a string is read, so nothing is converted.
    unsafe {
        if (api.type_of)(l, -1) != TSTRING {
            return None;
        }
        let mut len = 0;
        let text = (api.tolstring)(l, -1, &raw mut len);
        if text.is_null() {
            return None;
        }
        let bytes = std::slice::from_raw_parts(text.cast::<u8>(), len.min(1000));
        Some(String::from_utf8_lossy(bytes).into_owned())
    }
}

/// Runs [`CHUNK`] in `l`, the first time `l` is seen, as a state of the
/// menu's. Returns whether it was adopted now (`false`: already), or why it
/// could not be. Leaves the stack as it was.
///
/// # Safety
///
/// `l` is a live Lua state, used on this thread, that no Lua code of this
/// thread is running in the middle of an API call on.
pub unsafe fn adopt(l: State) -> Result<bool, String> {
    // SAFETY: the caller's.
    unsafe { adopt_as(l, false) }
}

/// A new app state created during a load belongs to the incoming world,
/// even before CMenuUI publishes m_game (dev's #36): one given `app` while
/// the menu `menu` has a world loaded or a load under way
/// (`m_loadGameResult` set, [`load_in_progress`]) is never the menu's. Read
/// natively, never through the menu's Lua and its progress monitor, whose
/// lock a running load holds.
///
/// # Safety
/// As [`world_loaded`].
unsafe fn incoming_world(menu: usize) -> bool {
    // SAFETY: the caller's.
    unsafe { world_loaded(menu) == Some(true) || load_in_progress(menu) == Some(true) }
}

/// [`adopt`], for a state given `app` while a world is loaded (`world`):
/// the world's GUI's, which the menu never calls.
///
/// # Safety
///
/// As [`adopt`].
pub unsafe fn adopt_as(l: State, world: bool) -> Result<bool, String> {
    // SAFETY: the caller's.
    let adopted_now = unsafe { run_chunk(l) }?;
    if adopted_now && world {
        for state in adopted()
            .iter_mut()
            .filter(|state| state.state == l as usize)
        {
            state.world = true;
        }
    }
    Ok(adopted_now)
}

/// # Safety
///
/// As [`adopt`].
unsafe fn run_chunk(l: State) -> Result<bool, String> {
    let (Some(api), Some(menu)) = (lua::api(), MENU_API.get()) else {
        return Err("the hook has no Lua API for the menu".into());
    };
    if adopted().iter().any(|state| state.state == l as usize) {
        return Ok(false);
    }
    // SAFETY: the caller's; everything pushed is popped by the final settop.
    unsafe {
        let top = (api.gettop)(l);
        if (api.checkstack)(l, 4) == 0 {
            return Err("no room on the Lua stack".into());
        }
        let mut chunk = Chunk {
            text: CHUNK,
            done: false,
        };
        let status = (menu.load)(
            l,
            read_chunk,
            (&raw mut chunk).cast(),
            c"=tpf3mp-menu".as_ptr(),
            c"t".as_ptr(),
        );
        if status != 0 {
            let why = string_at_top(api, l).unwrap_or_default();
            (api.settop)(l, top);
            return Err(format!("the menu's Lua did not load ({status}): {why}"));
        }
        (api.pushcclosure)(l, native_here as CFunction, 0);
        (api.pushcclosure)(l, native_gone as CFunction, 0);
        (api.pushcclosure)(l, lua::native_mods as CFunction, 0);
        let status = (menu.pcallk)(l, 3, 0, 0, 0, std::ptr::null());
        if status != 0 {
            let why = string_at_top(api, l).unwrap_or_default();
            (api.settop)(l, top);
            return Err(format!("the menu's Lua failed ({status}): {why}"));
        }
        (api.settop)(l, top);
    }
    if adopted().iter().any(|state| state.state == l as usize) {
        Ok(true)
    } else {
        Err("the menu's Lua ran but did not hand over its load".into())
    }
}

/// `here(load, busy)`: keeps `load` and `busy` in the registry; returns the
/// state's number, or nil.
unsafe extern "C-unwind" fn native_here(l: State) -> c_int {
    let (Some(api), Some(menu)) = (lua::api(), MENU_API.get()) else {
        return 0;
    };
    // SAFETY: Lua calls this with its own state, on its thread; a C
    // function has LUA_MINSTACK free slots.
    unsafe {
        if (api.gettop)(l) < 1 || (api.type_of)(l, 1) != TFUNCTION {
            (api.pushnil)(l);
            return 1;
        }
        (api.pushvalue)(l, 1);
        let reference = (menu.reference)(l, menu.registry);
        if reference < 0 {
            (api.pushnil)(l);
            return 1;
        }
        let busy = if (api.gettop)(l) >= 2 && (api.type_of)(l, 2) == TFUNCTION {
            (api.pushvalue)(l, 2);
            (menu.reference)(l, menu.registry)
        } else {
            -1
        };
        let number = NEXT.fetch_add(1, Ordering::Relaxed);
        adopted().push(Adopted {
            number,
            state: l as usize,
            reference,
            busy,
            thread: std::thread::current().id(),
            world: false,
        });
        #[allow(clippy::cast_precision_loss)]
        (api.pushnumber)(l, number as f64);
        1
    }
}

/// `gone(number)`: the state is closing; the menu never calls into it
/// again.
unsafe extern "C-unwind" fn native_gone(l: State) -> c_int {
    let Some(api) = lua::api() else {
        return 0;
    };
    // SAFETY: Lua calls this with its own state, on its thread.
    let number = unsafe { (api.tonumberx)(l, 1, std::ptr::null_mut()) };
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let number = number as u64;
    adopted().retain(|state| state.number != number);
    0
}

/// What the menu made of a load.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Served {
    /// The game is loading the save.
    Started,
    /// The game is loading something else; ask again later.
    Busy,
    /// It could not, and why.
    Failed(String),
}

/// The newest state of the menu's adopted on this thread, and its `load`
/// and `busy`.
fn newest_menu_state() -> Option<(usize, c_int, c_int)> {
    let here = std::thread::current().id();
    adopted()
        .iter()
        .rev()
        .find(|state| state.menus(here))
        .map(|state| (state.state, state.reference, state.busy))
}

/// Loads the save `name` of the game's save folder from the newest state of
/// the menu's adopted on this thread: `None` when there is none.
///
/// # Safety
///
/// Called on the thread that runs the menu's Lua, between its frames (no
/// Lua of this thread is running).
pub unsafe fn serve(name: &str) -> Option<Served> {
    let (Some(api), Some(menu)) = (lua::api(), MENU_API.get()) else {
        return None;
    };
    // The lock is let go before Lua runs: a collection there may finalize
    // another state's sentinel, which takes it.
    let (state, reference, _) = newest_menu_state()?;
    let l = state as State;
    let result = std::panic::catch_unwind(AssertUnwindSafe(|| {
        // SAFETY: the state is open (its sentinel has not been finalized)
        // and belongs to this thread, which runs no Lua now; everything
        // pushed is popped by the final settop.
        unsafe {
            let top = (api.gettop)(l);
            if (api.checkstack)(l, 3) == 0 {
                return Served::Failed("no room on the Lua stack".into());
            }
            (api.rawgeti)(l, menu.registry, reference);
            (api.pushlstring)(l, name.as_ptr().cast(), name.len());
            let status = (menu.pcallk)(l, 1, 1, 0, 0, std::ptr::null());
            let answer = string_at_top(api, l).unwrap_or_default();
            (api.settop)(l, top);
            if status != 0 {
                return Served::Failed(format!("the menu's load raised: {answer}"));
            }
            match answer.as_str() {
                "started" => Served::Started,
                "busy" => Served::Busy,
                _ => Served::Failed(answer),
            }
        }
    }));
    Some(result.unwrap_or_else(|_| Served::Failed("the hook failed calling the menu".into())))
}

/// The game's `RegisterAppUsertypes`, reached through its trampoline.
static REGISTER_ORIGINAL: AtomicU64 = AtomicU64::new(0);
/// The menu's `DoStep`, reached through its trampoline.
static STEP_ORIGINAL: AtomicU64 = AtomicU64::new(0);

/// Both detours pass four registers through: the targets take three
/// (`RegisterAppUsertypes`: the state, the menu, the callback; `DoStep`: the
/// menu and two more) and their results pass back in `rax`.
type Passthrough = unsafe extern "C-unwind" fn(usize, usize, usize, usize) -> usize;

/// After the game gave a state `app`: adopt it. `state` is the game's
/// `lua::State&`, whose `lua_State*` is its first field.
unsafe extern "C-unwind" fn register_detour(
    state: usize,
    menu: usize,
    ready: usize,
    d: usize,
) -> usize {
    let original = REGISTER_ORIGINAL.load(Ordering::Acquire);
    if original == 0 {
        return 0;
    }
    // SAFETY: the trampoline of the function the profile resolved, called
    // with the arguments the game passed.
    let result =
        unsafe { std::mem::transmute::<u64, Passthrough>(original)(state, menu, ready, d) };
    let _ = std::panic::catch_unwind(|| {
        if state == 0 {
            return;
        }
        // SAFETY: `state` is the game's live `lua::State`, whose first word
        // is its `lua_State*` (`lua::State::State` stores it there).
        let l = unsafe { *(state as *const usize) };
        if l == 0 {
            return;
        }
        // A state given `app` while a world is loaded is that world's GUI's.
        // SAFETY: `menu` is the CMenuUI& the game passed, live on this
        // thread for the call.
        let world = unsafe { incoming_world(menu) };
        // SAFETY: the game just registered into this state on this thread
        // and is not inside any of its API calls now.
        match unsafe { adopt_as(l as State, world) } {
            Ok(true) if world => crate::install::log_line(&format!(
                "menu: Lua state {l:#x} has app, in a loaded world: its GUI's, never used by the main menu"
            )),
            Ok(true) => crate::install::log_line(&format!(
                "menu: Lua state {l:#x} has app; the main menu can load the room's world from it"
            )),
            Ok(false) => {}
            Err(why) => crate::install::log_line(&format!(
                "menu: Lua state {l:#x} has app, but the menu cannot load from it: {why}"
            )),
        }
    });
    result
}

/// Whether a menu frame ends now with no other of the menu's frames under
/// it on the stack: only such a frame is the menu's own.
static FRAMES_RUNNING: AtomicUsize = AtomicUsize::new(0);

pub fn outermost_frame() -> bool {
    FRAMES_RUNNING.load(Ordering::Acquire) == 0
}

/// After each of the menu's frames: the room, from the menu. A frame that
/// runs inside another (the game's own work, a load) is left alone.
unsafe extern "C-unwind" fn step_detour(menu: usize, a: usize, b: usize, c: usize) -> usize {
    let original = STEP_ORIGINAL.load(Ordering::Acquire);
    if original == 0 {
        return 0;
    }
    FRAMES_RUNNING.fetch_add(1, Ordering::AcqRel);
    // SAFETY: as above.
    let result = unsafe { std::mem::transmute::<u64, Passthrough>(original)(menu, a, b, c) };
    FRAMES_RUNNING.fetch_sub(1, Ordering::AcqRel);
    if outermost_frame() {
        let _ = std::panic::catch_unwind(|| crate::install::menu_frame(menu));
    }
    result
}

/// Installs the menu's load: its Lua API, then the two detours. `at` gives
/// the address of a profile target in this process, `detour` installs a
/// detour for good and returns its trampoline (`crate::install`). Returns
/// the line for the log; any piece missing installs nothing more and says
/// why (the room's world then needs a world up, as before).
///
/// # Safety
///
/// Every address `at` gives is the function the profile names in this very
/// build, which no thread runs yet; `detour` is as
/// `InlineDetour::install`.
#[cfg(all(windows, target_arch = "x86_64"))]
pub unsafe fn install(
    at: &dyn Fn(&str) -> Result<usize, String>,
    detour: unsafe fn(*mut u8, *const u8) -> Result<usize, String>,
) -> Result<String, String> {
    let why = |error: String| {
        format!(
            "the main menu cannot load the room's world (fail closed): {error}; a game needs a world up to take the room's"
        )
    };
    let (load, pcallk, reference, register, step) = (|| {
        Ok::<_, String>((
            at(LOAD_TARGET)?,
            at(PCALL_TARGET)?,
            at(REF_TARGET)?,
            at(REGISTER_APP_TARGET)?,
            at(MENU_STEP_TARGET)?,
        ))
    })()
    .map_err(why)?;
    if lua::api().is_none() {
        return Err(why("the Lua link is not installed".into()));
    }
    // SAFETY: each address is the Lua 5.2 function the profile names, found
    // by its signature and prologue in this very build (a call target only,
    // never detoured). Each transmute's type is its field's: the API's
    // signature, spelled once, in `MenuApi`.
    #[allow(clippy::missing_transmute_annotations)]
    install_api(unsafe {
        MenuApi {
            load: std::mem::transmute::<usize, _>(load),
            pcallk: std::mem::transmute::<usize, _>(pcallk),
            reference: std::mem::transmute::<usize, _>(reference),
            registry: LUA52_REGISTRY,
        }
    });
    // Before the detours: a state the game gives `app` in a loaded world is
    // known for the world's from the first.
    // SAFETY: the caller's.
    let game = unsafe { find_fields(at) };
    // SAFETY: both targets are functions the profile resolved and
    // prologue-checked; the hook installs while the game starts, before its
    // menu or any Lua state exists, so no thread runs them; each detour has
    // their ABI (register arguments passed through, the result in rax).
    let register = unsafe { detour(register as *mut u8, register_detour as *const u8) }
        .map_err(|error| why(format!("detouring {REGISTER_APP_TARGET}: {error}")))?;
    REGISTER_ORIGINAL.store(register as u64, Ordering::Release);
    // SAFETY: as above.
    let step = unsafe { detour(step as *mut u8, step_detour as *const u8) }
        .map_err(|error| why(format!("detouring {MENU_STEP_TARGET}: {error}")))?;
    STEP_ORIGINAL.store(step as u64, Ordering::Release);
    Ok(format!(
        "the main menu can load the room's world: detours on {REGISTER_APP_TARGET} and {MENU_STEP_TARGET}{game}"
    ))
}

/// Finds `CMenuUI::m_game` and `CMenuUI::m_loadGameResult` from
/// [`MENU_GAME_TARGET`] and [`MENU_LOAD_TARGET`] and keeps their offsets;
/// returns the end of the install's log line.
///
/// # Safety
///
/// As [`install`].
#[cfg(all(windows, target_arch = "x86_64"))]
unsafe fn find_fields(at: &dyn Fn(&str) -> Result<usize, String>) -> String {
    // SAFETY: the caller's.
    let game = unsafe { find_game_field(at) };
    // Whether a load runs: without it, the menu never calls into the
    // menu's Lua (fail closed), since a call there during a load can wait
    // on the loader's locks.
    let load = match at(MENU_LOAD_TARGET) {
        Ok(read) => {
            // SAFETY: as in find_game_field.
            let code = unsafe { std::slice::from_raw_parts(read as *const u8, 7) };
            match load_field_at(code) {
                Some(offset) => {
                    set_load_field(offset);
                    format!(
                        "; a load runs while CMenuUI::m_loadGameResult (+{offset:#x}) is set, and the menu's Lua is called only while none does"
                    )
                }
                None => format!(
                    "; {MENU_LOAD_TARGET} is not the read it names, so the menu never calls its Lua and does not load the room's world (fail closed)"
                ),
            }
        }
        Err(error) => format!(
            "; {error}, so the menu never calls its Lua and does not load the room's world (fail closed)"
        ),
    };
    format!("{game}{load}")
}

/// Finds `CMenuUI::m_game` from [`MENU_GAME_TARGET`] and keeps its offset;
/// returns the end of the install's log line.
///
/// # Safety
///
/// As [`install`].
#[cfg(all(windows, target_arch = "x86_64"))]
unsafe fn find_game_field(at: &dyn Fn(&str) -> Result<usize, String>) -> String {
    // Whether a world is loaded: without it, the menu follows the room only
    // in a game that has had no world up yet (fail closed).
    match at(MENU_GAME_TARGET) {
        Ok(test) => {
            // SAFETY: the instruction the profile resolved and checked in
            // this process's code (its opcode is the target's prologue);
            // the code is mapped and only read.
            let code = unsafe { std::slice::from_raw_parts(test as *const u8, 7) };
            match game_field_at(code) {
                Some(offset) => {
                    set_game_field(offset);
                    format!(
                        "; a world is loaded while CMenuUI::m_game (+{offset:#x}) is set, so it follows the room back at the menu after a world"
                    )
                }
                None => format!(
                    "; {MENU_GAME_TARGET} is not the test it names, so only a game that has had no world up follows the room from the menu"
                ),
            }
        }
        Err(error) => format!(
            "; {error}, so only a game that has had no world up follows the room from the menu"
        ),
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use std::sync::PoisonError;

    use mlua::ffi;

    use super::*;
    use crate::lua::tests::{Lua, SERIAL, lua51};

    unsafe extern "C-unwind" fn load51(
        l: State,
        reader: Reader,
        data: *mut c_void,
        name: *const c_char,
        _mode: *const c_char,
    ) -> c_int {
        // The same reader under Lua 5.1's type.
        let reader = unsafe { std::mem::transmute::<Reader, ffi::lua_Reader>(reader) };
        unsafe { ffi::lua_load(l.cast(), reader, data, name) }
    }
    unsafe extern "C-unwind" fn pcall51(
        l: State,
        nargs: c_int,
        nresults: c_int,
        errfunc: c_int,
        _ctx: c_int,
        _k: *const c_void,
    ) -> c_int {
        PCALLS.fetch_add(1, Ordering::SeqCst);
        unsafe { ffi::lua_pcall(l.cast(), nargs, nresults, errfunc) }
    }
    pub(crate) static PCALLS: AtomicUsize = AtomicUsize::new(0);
    unsafe extern "C-unwind" fn ref51(l: State, t: c_int) -> c_int {
        unsafe { ffi::luaL_ref(l.cast(), t) }
    }

    /// The link's and the menu's Lua 5.1 API, installed once for the test
    /// binary.
    pub(crate) fn menu51() {
        lua51();
        install_api(MenuApi {
            load: load51,
            pcallk: pcall51,
            reference: ref51,
            registry: ffi::LUA_REGISTRYINDEX,
        });
    }

    /// The menu's `app` and `api`, as far as a load uses them: `LOADS`
    /// records each load, `TASK` is the progress monitor's task.
    const FAKE_MENU: &str = "\
        LOADS = {} TASK = '' \
        api = { type = { SavegameId = { new = function() return {} end } } } \
        app = { \
          SaveGameNamespace = { getSavegame = function() return 'savegame' end }, \
          getProgressMonitor = function() return { getTask = function() return TASK end } end, \
          loadGame = function(id, isMapEditor, info) \
            if FAIL then error(FAIL, 0) end \
            LOADS[#LOADS + 1] = id.saveGameName .. '|' .. id.path .. '|' .. id.saveGameNamespace \
              .. '|' .. tostring(isMapEditor) .. '|' .. tostring(info) \
          end }";

    pub(crate) fn forget_all() {
        adopted().clear();
    }

    #[test]
    fn a_state_with_app_loads_the_rooms_save_as_the_menus_load_page_does() {
        let _serial = SERIAL.lock().unwrap_or_else(PoisonError::into_inner);
        menu51();
        forget_all();
        let menu = Lua::new();
        menu.run(FAKE_MENU).unwrap();
        assert!(!available(), "nothing adopted yet");
        assert_eq!(unsafe { serve("tpf3mp_room_7") }, None);
        let l: *mut ffi::lua_State = menu.state().cast();
        let top = unsafe { ffi::lua_gettop(l) };
        assert_eq!(unsafe { adopt(menu.state()) }, Ok(true));
        assert_eq!(unsafe { adopt(menu.state()) }, Ok(false), "once a state");
        assert_eq!(unsafe { ffi::lua_gettop(l) }, top, "the stack is as it was");
        assert!(available());
        assert_eq!(unsafe { serve("tpf3mp_room_7") }, Some(Served::Started));
        assert_eq!(
            menu.run("return #LOADS, LOADS[1]"),
            Ok("1|tpf3mp_room_7||savegame|false|nil".into()),
            "no Start Game wait, the save's own mods"
        );
        // The caller checks the native load field: never ask the progress monitor.
        menu.run("app.getProgressMonitor = function() error('must not query monitor') end")
            .unwrap();
        assert_eq!(unsafe { serve("tpf3mp_room_7") }, Some(Served::Started));
        menu.run("TASK = '' FAIL = 'Game initialization is already active!'")
            .unwrap();
        assert_eq!(
            unsafe { serve("tpf3mp_room_7") },
            Some(Served::Failed(
                "app.loadGame failed: Game initialization is already active!".into()
            ))
        );
        assert_eq!(menu.run("return #LOADS"), Ok("2".into()));
        forget_all();
    }

    #[test]
    fn loading_closes_the_lobby_and_gives_its_removal_a_frame() {
        let _serial = SERIAL.lock().unwrap_or_else(PoisonError::into_inner);
        menu51();
        forget_all();
        let menu = Lua::new();
        menu.run(FAKE_MENU).unwrap();
        menu.run("CLOSED = 0; resolveutil = { __tpf3mp_before_load = function() CLOSED = CLOSED + 1 end }")
            .unwrap();
        assert_eq!(unsafe { adopt(menu.state()) }, Ok(true));
        assert_eq!(unsafe { serve("room") }, Some(Served::Busy));
        assert_eq!(menu.run("return CLOSED, #LOADS"), Ok("1|0".into()));
        assert_eq!(unsafe { serve("room") }, Some(Served::Started));
        assert_eq!(menu.run("return CLOSED, #LOADS"), Ok("1|1".into()));
        menu.run("resolveutil.__tpf3mp_before_load = function() error('cannot close', 0) end")
            .unwrap();
        assert_eq!(
            unsafe { serve("room") },
            Some(Served::Failed(
                "closing the multiplayer menu failed: cannot close".into()
            ))
        );
        assert_eq!(menu.run("return #LOADS"), Ok("1".into()));
        forget_all();
    }

    #[test]
    fn a_state_without_app_says_so_and_a_closed_state_is_never_called() {
        let _serial = SERIAL.lock().unwrap_or_else(PoisonError::into_inner);
        menu51();
        forget_all();
        let bare = Lua::new();
        assert_eq!(unsafe { adopt(bare.state()) }, Ok(true));
        assert_eq!(
            unsafe { serve("x") },
            Some(Served::Failed("this Lua state has no app".into()))
        );
        // The state closes: its sentinel's finalizer (Lua 5.2 runs them all
        // at close) says so, and the menu has nothing to call.
        let number = adopted()[0].number;
        // Lua 5.1 has no finalizers on tables: call what the sentinel would.
        let l: *mut ffi::lua_State = bare.state().cast();
        unsafe {
            ffi::lua_pushcclosure(
                l,
                std::mem::transmute::<CFunction, ffi::lua_CFunction>(native_gone),
                0,
            );
            #[allow(clippy::cast_precision_loss)]
            ffi::lua_pushnumber(l, number as f64);
            assert_eq!(ffi::lua_pcall(l, 1, 0, 0), 0);
        }
        assert!(!available());
        assert_eq!(unsafe { serve("x") }, None);
        forget_all();
    }

    #[test]
    fn a_state_adopted_on_another_thread_is_not_the_menus() {
        let _serial = SERIAL.lock().unwrap_or_else(PoisonError::into_inner);
        menu51();
        forget_all();
        let menu = Lua::new();
        menu.run(FAKE_MENU).unwrap();
        assert_eq!(unsafe { adopt(menu.state()) }, Ok(true));
        let elsewhere = std::thread::spawn(|| (available(), unsafe { serve("x") }))
            .join()
            .unwrap();
        assert_eq!(elsewhere, (false, None));
        assert_eq!(menu.run("return #LOADS"), Ok("0".into()));
        forget_all();
    }

    #[test]
    fn a_state_given_app_in_a_loaded_world_is_never_the_menus_and_is_forgotten_at_its_close() {
        let _serial = SERIAL.lock().unwrap_or_else(PoisonError::into_inner);
        menu51();
        forget_all();
        let menu = Lua::new();
        menu.run(FAKE_MENU).unwrap();
        let world = Lua::new();
        world.run(FAKE_MENU).unwrap();
        assert_eq!(unsafe { adopt(menu.state()) }, Ok(true));
        assert_eq!(unsafe { adopt_as(world.state(), true) }, Ok(true));
        // The newest state is the world's: the menu loads in its own.
        assert_eq!(unsafe { serve("tpf3mp_room_7") }, Some(Served::Started));
        assert_eq!(menu.run("return #LOADS"), Ok("1".into()));
        assert_eq!(world.run("return #LOADS"), Ok("0".into()));
        // The world closes: its states go, the menu's stays.
        assert_eq!(forget_world_states(), 1);
        assert_eq!(forget_world_states(), 0);
        assert!(available());
        // Only a world's state left: nothing for the menu.
        forget_all();
        assert_eq!(unsafe { adopt_as(world.state(), true) }, Ok(true));
        assert!(!available());
        assert_eq!(unsafe { serve("x") }, None);
        forget_all();
    }

    #[test]
    fn incoming_world_state_is_not_used_by_menu_before_m_game_is_published() {
        let _serial = SERIAL.lock().unwrap_or_else(PoisonError::into_inner);
        menu51();
        forget_all();
        let menu = Lua::new();
        menu.run(FAKE_MENU).unwrap();
        assert_eq!(unsafe { adopt(menu.state()) }, Ok(true));
        // m_game at 8, m_loadGameResult at 16: no world, no load.
        let mut cmenu = [0usize; 4];
        set_game_field(8);
        set_load_field(16);
        let at = cmenu.as_mut_ptr() as usize;
        assert!(!unsafe { incoming_world(at) });
        // A load under way, m_game still null: a new state is the world's.
        cmenu[2] = 0x1234;
        let at = cmenu.as_mut_ptr() as usize;
        let world = Lua::new();
        world.run(FAKE_MENU).unwrap();
        let is_world = unsafe { incoming_world(at) };
        assert!(is_world, "m_game is still null while the world is loading");
        assert_eq!(unsafe { adopt_as(world.state(), is_world) }, Ok(true));
        // Only the original menu's state may load saves.
        assert_eq!(unsafe { serve("next_world") }, Some(Served::Started));
        assert_eq!(world.run("return #LOADS"), Ok("0".into()));
        assert_eq!(menu.run("return #LOADS"), Ok("1".into()));
        set_game_field(0);
        set_load_field(0);
        forget_all();
    }

    #[test]
    fn the_m_load_game_result_read_gives_the_fields_offset() {
        let _serial = SERIAL.lock().unwrap_or_else(PoisonError::into_inner);
        // Build 40408's `mov rbx, [rsi+0x1bd0]`.
        let read = [0x48, 0x8B, 0x9E, 0xD0, 0x1B, 0x00, 0x00, 0x48];
        assert_eq!(load_field_at(&read), Some(0x1bd0));
        assert_eq!(
            load_field_at(&[0x4C, 0x39, 0xAE, 0xD0, 0x1B, 0x00, 0x00]),
            None
        );
        assert_eq!(game_field_at(&read), None);
        // The field read from a menu: no Lua, no state needed.
        menu51();
        forget_all();
        let before = PCALLS.load(Ordering::SeqCst);
        let menu = [0usize, 0, 0x1234];
        set_load_field(16);
        assert_eq!(
            unsafe { load_in_progress(menu.as_ptr() as usize) },
            Some(true)
        );
        set_load_field(8);
        assert_eq!(
            unsafe { load_in_progress(menu.as_ptr() as usize) },
            Some(false)
        );
        set_load_field(0);
        assert_eq!(unsafe { load_in_progress(menu.as_ptr() as usize) }, None);
        assert_eq!(PCALLS.load(Ordering::SeqCst), before, "no Lua");
    }

    #[test]
    fn the_m_game_test_gives_the_fields_offset() {
        // The field is the process's, as the menu frame's tests use it.
        let _serial = SERIAL.lock().unwrap_or_else(PoisonError::into_inner);
        // Build 40408's `cmp [rsi+0x6b0], r13`.
        let test = [0x4C, 0x39, 0xAE, 0xB0, 0x06, 0x00, 0x00, 0x74];
        assert_eq!(game_field_at(&test), Some(0x6b0));
        // Another instruction, a short read, an implausible field: none.
        assert_eq!(
            game_field_at(&[0x48, 0x39, 0xAE, 0xB0, 0x06, 0x00, 0x00]),
            None
        );
        assert_eq!(game_field_at(&test[..5]), None);
        assert_eq!(
            game_field_at(&[0x4C, 0x39, 0xAE, 0xB1, 0x06, 0x00, 0x00]),
            None
        );
        assert_eq!(
            game_field_at(&[0x4C, 0x39, 0xAE, 0x00, 0x00, 0x00, 0x80]),
            None
        );
        // The field read from a menu.
        let menu = [0usize, 0, 0x1234];
        set_game_field(16);
        assert_eq!(unsafe { world_loaded(menu.as_ptr() as usize) }, Some(true));
        assert_eq!(unsafe { world_loaded(0) }, None);
        set_game_field(8);
        assert_eq!(unsafe { world_loaded(menu.as_ptr() as usize) }, Some(false));
        set_game_field(0);
        assert_eq!(unsafe { world_loaded(menu.as_ptr() as usize) }, None);
    }

    #[test]
    fn the_chunk_keeps_its_sentinel_and_waits_for_no_start_button() {
        assert!(CHUNK.contains("__gc"));
        assert!(
            CHUNK.contains("local keep = sentinel"),
            "load keeps it alive"
        );
        assert!(!CHUNK.contains("setWaitForStartReadyGame"));
        assert!(CHUNK.contains("theApp.loadGame(savegameId(theApp, name), false, info)"));
    }

    /// The menu's save details and mods, for a load with the room's mods:
    /// `SAVED` is the mods the save lists, `INSTALLED` those this player
    /// has, `READY` whether the game has read the save's details yet.
    const FAKE_MODS: &str = r#"
        SAVED = { 'vehicles_pack', 'tpf3mp_1', 'owner_minimap' }
        INSTALLED = { vehicles_pack = true, tpf3mp_1 = true, my_colours = true }
        READY = false
        api.type.ModId = { new = function() return {} end }
        api.type.SaveGameDetails = { new = function(info)
            local copy = {} for k, v in pairs(info) do copy[k] = v end return copy end }
        app.getSavegameInfo = function(id)
            local mods = {}
            for i, name in ipairs(SAVED) do mods[i] = { name = name } end
            return { isCompleted = function() return READY end,
                     get = function() return { errorMsg = '', info = { mods = mods, modParams = {} } } end }
        end
        app.getUserProfile = function() return { getModRep = function() return {
            exists = function(_, m) return INSTALLED[m.name] == true end } end } end
        local load = app.loadGame
        app.loadGame = function(id, isMapEditor, info)
            load(id, isMapEditor, info)
            if info then
                local names = {} for _, m in ipairs(info.mods) do names[#names + 1] = m.name end
                LOADS[#LOADS] = LOADS[#LOADS] .. '|' .. table.concat(names, ',')
            end
        end"#;

    #[test]
    fn with_the_rooms_lists_the_menu_loads_the_save_with_the_rooms_mods_and_mine() {
        let _serial = SERIAL.lock().unwrap_or_else(PoisonError::into_inner);
        menu51();
        forget_all();
        fn to<const N: usize>(l: &[&str]) -> tpf3mp_proto::BoundedVec<tpf3mp_bridge::ModName, N> {
            tpf3mp_proto::BoundedVec::new(
                l.iter()
                    .map(|n| tpf3mp_proto::Text::new(*n).unwrap())
                    .collect(),
            )
            .unwrap()
        }
        lua::set_mods(Some(tpf3mp_bridge::ModLists {
            shared: to(&["vehicles_pack"]),
            personal: to(&["my_colours"]),
        }));
        let menu = Lua::new();
        menu.run(FAKE_MENU).unwrap();
        menu.run(FAKE_MODS).unwrap();
        assert_eq!(unsafe { adopt(menu.state()) }, Ok(true));
        // The game reads the save's details in the background: asked again.
        assert_eq!(unsafe { serve("tpf3mp_room_7") }, Some(Served::Busy));
        assert_eq!(menu.run("return #LOADS"), Ok("0".into()));
        menu.run("READY = true").unwrap();
        assert_eq!(unsafe { serve("tpf3mp_room_7") }, Some(Served::Started));
        let loaded = menu.run("return LOADS[1]").unwrap();
        assert!(
            loaded.ends_with("|vehicles_pack,tpf3mp_1,my_colours"),
            "the owner's minimap left out, this player's colours added: {loaded}"
        );

        // A shared mod this player lacks: the world cannot load here.
        menu.run("INSTALLED.vehicles_pack = nil").unwrap();
        assert_eq!(
            unsafe { serve("tpf3mp_room_8") },
            Some(Served::Failed(
                "the room's world needs the mod vehicles_pack, which is not installed".into()
            ))
        );

        // Without the room's lists, the save's own mods, as before.
        lua::set_mods(None);
        assert_eq!(unsafe { serve("tpf3mp_room_9") }, Some(Served::Started));
        assert_eq!(
            menu.run("return LOADS[#LOADS]"),
            Ok("tpf3mp_room_9||savegame|false|nil".into())
        );
        forget_all();
    }
}
