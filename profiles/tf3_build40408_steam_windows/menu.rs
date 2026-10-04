//! Native data for Steam Windows build 40408. See hooks.toml for identity.

use std::ffi::c_int;

/// The profile's name for the function that gives a Lua state `app`.
pub const REGISTER_APP_TARGET: &str = "RegisterAppUsertypes";

/// The profile's name for the main menu's per-frame update.
pub const MENU_STEP_TARGET: &str = "UI::CMenuUI::DoStep";

/// The profile's name for `DoStep`'s test of `CMenuUI::m_game`: `cmp
/// [rsi+disp32], r13` (`4C 39 AE`), whose displacement is the field's
/// offset.
pub const MENU_GAME_TARGET: &str = "UI::CMenuUI::DoStep/m_game test";

/// The bytes of that instruction before its displacement.
pub const MENU_GAME_OPCODE: [u8; 3] = [0x4C, 0x39, 0xAE];

/// The profile's name for `DoStep`'s read of `CMenuUI::m_loadGameResult`:
/// `mov rbx, [rsi+disp32]` (`48 8B 9E`), whose displacement is the field's
/// offset.
pub const MENU_LOAD_TARGET: &str = "UI::CMenuUI::DoStep/m_loadGameResult read";

/// The bytes of that instruction before its displacement.
pub const MENU_LOAD_OPCODE: [u8; 3] = [0x48, 0x8B, 0x9E];

/// The profile's names for the Lua 5.2 functions only the menu needs.
pub const LOAD_TARGET: &str = "lua_load";

pub const PCALL_TARGET: &str = "lua_pcallk";

pub const REF_TARGET: &str = "luaL_ref";

/// Lua 5.2's `LUA_REGISTRYINDEX`.
pub const LUA52_REGISTRY: c_int = -1_001_000;

pub const TSTRING: c_int = 4;

pub const TFUNCTION: c_int = 6;
