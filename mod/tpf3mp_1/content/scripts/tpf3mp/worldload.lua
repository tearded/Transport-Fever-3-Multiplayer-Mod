-- tpf3mp/worldload.lua -- loading the room's world from the GUI of a world
-- the player has up, with the room's mods (docs/MODS.md).
--
-- A save lists the mods of the game that wrote it, another player's
-- personal ones included. The hook knows which mods the room's world loads
-- with in this game (tpf3mp_native.mods, from the room's Begin): the room's
-- shared mods and this player's personal ones. The game loads a save with
-- other mods when app.loadGame is given the save's details with its `mods`
-- replaced, as its own Load Game page does when the player changes them
-- (gui/menu/savegame_react_util.tl, build 40408). The details are read in
-- the background (app.getSavegameInfo), so a load takes a few frames:
-- step() is called every frame until it says "started" or why not.
--
-- The main menu's load does the same in the hook's own Lua
-- (crates/tpf3mp-hook/src/menu.rs, CHUNK). Without the room's lists the
-- save loads with its own mods, as before.
--
-- Pure Lua; the tests hand it a fake app, api and link.

local worldload = {}

local function savegameId(app, api, name)
	local id = api.type.SavegameId.new()
	id.path = ""
	id.saveGameName = name
	id.saveGameNamespace = app.SaveGameNamespace.getSavegame()
	return id
end

-- The save's details with the mods of `plan` (a list of names) and the
-- room's settings of them and of the game, `params` (by mod, by setting, the
-- game's own under ""; other mods' stay the save's), or nil and why a mod of
-- it is not here.
local function withMods(app, api, data, plan, params)
	local modRep = app.getUserProfile():getModRep()
	local mods = {}
	for _, name in ipairs(plan) do
		local m = api.type.ModId.new()
		m.name = name
		if not modRep:exists(m) then
			return nil, "the room's world needs the mod " .. name .. ", which is not installed"
		end
		mods[#mods + 1] = m
	end
	local info = api.type.SaveGameDetails.new(data.info)
	info.mods = mods
	if params then
		local all = {}
		for mod, of in pairs(info.modParams or {}) do all[mod] = of end
		for mod, of in pairs(params) do all[mod] = of end
		info.modParams = all
	end
	return info
end

-- A load of the save `name`, for step().
function worldload.new(name)
	return { name = name }
end

-- Goes on with `load` (from new()): "busy" while the game reads the save's
-- details, "started" once the game loads it, or nil and why it cannot.
function worldload.step(load, app, api, link)
	local info = nil
	if link:hasMods() then
		if load.async == nil then
			local ok, async = pcall(app.getSavegameInfo, savegameId(app, api, load.name))
			if not ok or async == nil then
				return nil, "reading the save's mods failed: " .. tostring(async)
			end
			load.async = async
		end
		if not load.async:isCompleted() then return "busy" end
		local data = load.async:get()
		if data == nil or data.info == nil then
			return nil, "the save's mods did not read: " .. tostring(data and data.errorMsg)
		end
		local names = {}
		for _, m in ipairs(data.info.mods or {}) do names[#names + 1] = m.name end
		local plan = link:mods(names)
		if plan then
			local ok, made, why = pcall(withMods, app, api, data, plan, link:modParams())
			if not ok then return nil, "the room's mods for the save failed: " .. tostring(made) end
			if made == nil then return nil, why end
			info = made
		end
	end
	local ok, err = pcall(function()
		app.loadGame(savegameId(app, api, load.name), false, info)
	end)
	if not ok then return nil, "app.loadGame failed: " .. tostring(err) end
	return "started"
end

return worldload
