-- The room's mods in the game's own pages (docs/LOBBY.md, "The room's save
-- and mods"; docs/MODS.md, "The room's mods").
--
-- The owner picks the room's save, its mods and their settings on the
-- game's own Load Game page: the Multiplayer window sends them there with
-- `begin`, and while they pick, the page's own helpers are swapped for a
-- moment (its module tables, which it reads at each draw):
-- - its title says it picks for the room (menu_icon_react_util.makePage);
-- - its Load Game button says Use for the room and plays no game-start
--   sound (makePrimaryButton, by its class "loadSavegameButton");
-- - its save tiles open their details, the page with the Mods and Gameplay
--   Settings tabs, and have no Load Game of their own
--   (savegame_react_util.SavegameCard);
-- - its list of save tiles starts with one for a new world instead, as the
--   room may start from (tile_list_react_util.TileList, the list drawn
--   right after the save tiles);
-- - its load (app.loadGame, after app.setWaitForStartReadyGame, as both
--   ways the page loads call them) takes the save and what the page holds
--   for it instead of loading.
-- Everything is put back as soon as the pick ends: picked, or the page left
-- for the main menu (`finish`, which main_page.tl calls when it mounts).
-- Should any of it fail, nothing is swapped and the pick does not begin.
--
-- A player lacking a room's mod that comes from Mod Hub installs it here:
-- the game's own Mod Hub subscribes them to it (api.modhub, as its Mod
-- Manager does) and downloads it; once the game has it installed, the
-- launcher is asked to find the installed mods again.
--
-- Plain Lua, loaded by lobby.lua through ug_require; the tests run it
-- against the stand-in for the game's GUI (tests/lua/fake_menu.lua).

local roommods = {}

local function say(line)
	pcall(debugPrint, "[tpf3mp] room mods: " .. line)
end

-- Picking ---------------------------------------------------------------------

-- The pick under way: { finish = function, picked = table or nil }.
local picking = nil

-- What the page held for the save when Use for the room was pressed: the
-- save's name, its map and year as the page read them, the mods in their
-- activation order (id, name, source, Mod Hub number) and every setting
-- (mod, key, value; the game's own under the mod ""). `info` nil (a save
-- loaded straight from its list, as a double click does) gives the save
-- alone: the room then runs the save's own mods.
function roommods.choice(id, info, describe)
	local choice = { save = tostring(id and id.saveGameName or ""):gsub("%.sav$", "") }
	if not info then return choice end
	choice.map, choice.year = describe.mapYear(info)
	choice.mods, choice.params = {}, {}
	for _i, modId in ipairs(info.mods or {}) do
		local mod = describe.mod(modId)
		if mod then choice.mods[#choice.mods + 1] = mod end
	end
	local mods = {}
	for mod in pairs(info.modParams or {}) do mods[#mods + 1] = tostring(mod) end
	table.sort(mods)
	for _i, mod in ipairs(mods) do
		local keys = {}
		for key in pairs(info.modParams[mod] or {}) do keys[#keys + 1] = tostring(key) end
		table.sort(keys)
		for _k, key in ipairs(keys) do
			local value = tonumber(info.modParams[mod][key])
			if value then
				choice.params[#choice.params + 1] = { mod = mod, key = key, value = math.floor(value) }
			end
		end
	end
	return choice
end

-- What the game says of an installed mod, by its ModId: its id, name, where
-- it comes from and its Mod Hub number ("" for none).
local function describeMod(modId)
	local name = tostring(modId.name or "")
	if name == "" then return nil end
	local mod = { id = name, name = name, source = "", modio = "" }
	pcall(function()
		local rep = app.getUserProfile():getModRep()
		local desc = rep:getGameModDesc(modId)
		if desc and type(desc.name) == "string" and desc.name ~= "" then mod.name = desc.name end
		mod.source = tostring(rep:getModSource(modId) or "")
	end)
	if mod.source == "mod.io" then
		pcall(function()
			local hub = api.modhub.getModHubModIdForModId(modId)
			if hub and hub:isValid() and tostring(hub.value):match("^%d+$") then mod.modio = tostring(hub.value) end
		end)
	end
	return mod
end

roommods.describe = {
	mod = describeMod,
	-- The climate (configDict "climate") and year, as lobby.saveDetails
	-- reads them; "" and 0 when the page cannot say.
	mapYear = function(info)
		local map, year = "", 0
		pcall(function()
			for _i, pair in ipairs(info.configDict or {}) do
				if pair[1] == "climate" and type(pair[2]) == "string" then
					map = pair[2]:match("([%w_]+)%.clima$") or pair[2]
				end
			end
		end)
		pcall(function()
			local y = api.type.Date.new(info.metadata.date).year
			if type(y) == "number" and y > 1000 and y < 3000 then year = math.floor(y) end
		end)
		return map, year
	end,
}

-- Begins picking on the game's Load Game page, which the caller then opens.
-- Whether a pick is under way.
function roommods.isPicking()
	return picking ~= nil
end

-- Returns whether it began; `picked(choice)` is called with what the owner
-- chose, before the main menu comes back (`setPage`, the menu's).
function roommods.begin(setPage, picked)
	if picking then return false end
	local menuUtil = ug_require "/gui/menu/menu_icon_react_util.tl"
	local saveUtil = ug_require "/gui/menu/savegame_react_util.tl"
	local tileUtil = ug_require "/gui/main/tile_list_react_util.tl"
	local builtin = ug_require "::/gui/main/builtin.lua"
	local was = {
		tiles = tileUtil.TileList,
		tile = tileUtil.TileElement,
		load = app.loadGame,
		wait = app.setWaitForStartReadyGame,
		button = menuUtil.makePrimaryButton,
		page = menuUtil.makePage,
		card = saveUtil.SavegameCard,
	}
	-- Each of them is something the page calls: a Lua function, or the
	-- game's own (bound functions and recipes are tables or userdata,
	-- whose call cannot always be read here).
	local function callable(v)
		return type(v) == "function" or type(v) == "table" or type(v) == "userdata"
	end
	if not (callable(was.load) and callable(was.button) and callable(was.page) and callable(was.card)
		and callable(was.tiles) and callable(was.tile)) then
		say("the game's Load Game page is not as this mod knows it (" .. table.concat({ type(was.load),
			type(was.button), type(was.page), type(was.card) }, ", ") .. "): not picking there")
		return false
	end
	local function finish()
		app.loadGame = was.load
		app.setWaitForStartReadyGame = was.wait
		menuUtil.makePrimaryButton = was.button
		menuUtil.makePage = was.page
		saveUtil.SavegameCard = was.card
		tileUtil.TileList = was.tiles
	end
	-- What a pick ends with: the choice, then the main menu again.
	local function done(choice)
		finish()
		if picking then picking.picked = choice end
		picked(choice)
		setPage("Main", {})
	end
	-- The new world's tile, as the page draws a save's: a picture of the
	-- game's New Game card and its name.
	local function newWorldTile()
		return was.tile{
			meta = { localKey = "tpf3mp-new-world" },
			title = _("New world"),
			tileTooltip = _("Set up a new map and settings for the room instead of a save"),
			infoIcons = { {
				path = "::/gui/menu/icons/add.tga",
				label = _("Map and settings"),
				secondary = false,
			} },
			createImage = function()
				local sheet = api.gui.StyleSheet.new()
				sheet.size = api.type.Vec2f.new(317, 180)
				return builtin.Component{
					meta = { styleSheet = sheet },
					layout = builtin.BoxLayout{ children = {
						builtin.ImageView{
							path = "::/gui/menu/images/temperate_ingame.tga",
							scaling = builtin.type.ImageViewScaling.AutoZoom,
						},
					} },
				}
			end,
			onClickMain = function()
				say("picked a new world")
				done({ save = "", newWorld = true })
			end,
		}
	end
	-- Whether the page drew its save tiles in this draw, so that the list
	-- it draws next is theirs.
	local cardsDrawn = false
	say("picking on the Load Game page (" .. table.concat({ type(was.load), type(was.button), type(was.page),
		type(was.card) }, ", ") .. ")")
	local title = _("Load Game")
	local ok, why = pcall(function()
		menuUtil.makePrimaryButton = function(text, onClick, classes, ...)
			if type(classes) == "string" and classes:find("loadSavegameButton", 1, true) then
				text = _("Use for the room")
				classes = classes:gsub("%s*,?%s*load%-savegame%-sound", "")
			end
			return was.button(text, onClick, classes, ...)
		end
		menuUtil.makePage = function(commonParams, text, ...)
			if text == title then text = _("The room's save and mods") end
			return was.page(commonParams, text, ...)
		end
		saveUtil.SavegameCard = function(first, second)
			local params = second or first
			if type(params) == "table" then
				params.save = true
				params.onClickMain = params.onClickDetails
			end
			cardsDrawn = true
			if second == nil then return was.card(first) end
			return was.card(first, second)
		end
		tileUtil.TileList = function(params, ...)
			if cardsDrawn and type(params) == "table" and type(params.elements) == "table" then
				cardsDrawn = false
				local elements = { newWorldTile() }
				for _i, element in ipairs(params.elements) do elements[#elements + 1] = element end
				params.elements = elements
			end
			return was.tiles(params, ...)
		end
		app.setWaitForStartReadyGame = function() end
		app.loadGame = function(id, _isMapEditor, info)
			local got, choice = pcall(roommods.choice, id, info, roommods.describe)
			if not got then
				say("what the page held could not be read: " .. tostring(choice))
				choice = { save = tostring(id and id.saveGameName or "") }
			end
			say("picked " .. tostring(choice.save) .. " with " .. tostring(choice.mods and #choice.mods or "its own")
				.. " mods")
			done(choice)
		end
	end)
	if not ok then
		finish()
		say("cannot pick on the Load Game page: " .. tostring(why))
		return false
	end
	picking = { finish = finish }
	return true
end

-- Ends the pick under way, putting the page's helpers back. Returns nil
-- when none was under way, or what was picked (false when nothing was).
function roommods.finish()
	if not picking then return nil end
	local done = picking
	picking = nil
	done.finish()
	return done.picked or false
end

-- Installing from Mod Hub ------------------------------------------------------

-- Mod Hub's backend, nil without one (no network, another platform).
local function backend()
	local ok, id = pcall(function() return api.modhub.getBackendIdForSource("mod.io") end)
	if ok and type(id) == "number" and id >= 0 then return id end
	return nil
end

local function hubId(number)
	local id = api.type.modhub.ModId.new()
	id.value = tostring(number)
	return id
end

-- Whether this player can install from Mod Hub here: "ok", or why not:
-- "offline" (no Mod Hub), "signed_out" (not signed in to it), "busy" (it
-- takes no downloads now).
function roommods.hubState()
	local b = backend()
	if not b then return "offline" end
	local ok, state = pcall(function()
		if not api.modhub.isInitialized(b) then return "offline" end
		if api.modhub.getCapabilities(b).isInfoOnly then return "offline" end
		if api.modhub.getUserInfo(b) == nil then return "signed_out" end
		return "ok"
	end)
	return ok and state or "offline"
end

-- Where the install of the Mod Hub mod `number` stands: "none", "subscribed"
-- (waiting for its download), "downloading", "installed" or "failed".
function roommods.installState(number)
	local b = backend()
	if not b then return "none" end
	local ok, state = pcall(function()
		local id = hubId(number)
		local states = api.type.modhub.InstallState
		local s = api.modhub.getModInstallState(b, id)
		if s == states.Installed then return "installed" end
		if s == states.InsufficientSpace or s == states.MiscError then return "failed" end
		if s == states.Downloading or s == states.Extracting or s == states.InstallationPending
			or s == states.UpdatePending then
			return "downloading"
		end
		if api.modhub.getModSubscriptionState(b, id) then return "subscribed" end
		return "none"
	end)
	return ok and state or "none"
end

-- Looks up the Mod Hub mod `number` (the owner's claim of where to get one
-- of the room's mods) in this player's own Mod Hub: `done(details)` with
-- its title, author and size, or `done(nil, why)`.
function roommods.lookUp(number, done)
	local b = backend()
	if not b then return done(nil, _("Mod Hub is not available")) end
	local ok, why = pcall(function()
		api.modhub.getModDetailsAsync(b, api.type.modhub.GetModDetailsRequest.new(hubId(number)), function(result)
			if not result:isSuccess() then
				local err = result:getError()
				return done(nil, err and err.message or _("Mod Hub did not answer"))
			end
			local data = result:getData()
			if not data.found then return done(nil, _("Mod Hub has no such mod")) end
			done({
				title = tostring(data.modInfo.title or ""),
				author = tostring(data.author or ""),
				size = tonumber(data.modInfo.installSize) or 0,
				url = tostring(data.modInfo.url or ""),
			})
		end)
	end)
	if not ok then done(nil, tostring(why)) end
end

-- Subscribes this player to the Mod Hub mod `number`, which Mod Hub then
-- downloads and installs: `done(nil)`, or `done(why)`.
function roommods.install(number, done)
	local b = backend()
	if not b then return done(_("Mod Hub is not available")) end
	local ok, why = pcall(function()
		api.modhub.subscribeModAsync(b, api.type.modhub.SubscribeModRequest.new(hubId(number)), function(result)
			if result:isSuccess() then return done(nil) end
			local err = result:getError()
			done(err and err.message or _("Mod Hub refused it"))
		end)
	end)
	if not ok then done(tostring(why)) end
end

-- Opens the game's own Mod Hub page of the Mod Hub mod `number` over the
-- menu, as its Mod Hub opens a mod's tile (mod_manager_page.tl, a tile's
-- onClick: mod_manager_react_util.ModDetailsWindow with its details page):
-- its description, pictures, author, and the game's Subscribe, so the
-- player sees what they subscribe to. Mod Hub's access check first, as the
-- main menu's Mod Hub card does (it says itself why not). `closed()` once
-- the player closes it. `commonParams` are the menu's. True when the game
-- took it (opened, or said why not); false when it could not, and nothing
-- shows.
function roommods.showDetails(commonParams, number, title, closed)
	local b = backend()
	if not b or not commonParams or not commonParams.windowContainer then return false end
	local blocked = false
	local ok, taken = pcall(function()
		local util = ug_require "::/gui/menu/mod_manager_react_util.tl"
		local container = commonParams.windowContainer
		local wc = container:get():getApi()
		if not util.checkModManagerAccess(wc) then return true end
		local function close()
			pcall(function()
				if commonParams.setBlockedForModal then commonParams.setBlockedForModal(false) end
				wc.removeAllWindows(util.ModDetailsWindow)
			end)
			closed()
		end
		if commonParams.setBlockedForModal then
			commonParams.setBlockedForModal(true)
			blocked = true
		end
		wc.addSingletonWindow(util.ModDetailsWindow, {
			title = title,
			onClose = close,
			modManagerParams = {
				context = {
					backendId = b,
					wc = container,
					setBlockedForModal = commonParams.setBlockedForModal,
					calloutContainerRef = commonParams.calloutContainerRef,
				},
				modId = hubId(number),
				onClose = close,
				-- Only its author's other mods call it, offered without a
				-- user name search: none here.
				detailsBack = close,
				isUserNameSearchEnabled = false,
				isCurated = false,
				inPauseMenu = false,
			},
		})
		return true
	end)
	if ok then return taken end
	say("Mod Hub's page of " .. tostring(number) .. " did not open: " .. tostring(taken))
	if blocked then pcall(commonParams.setBlockedForModal, false) end
	return false
end

-- Where the game gets the logo of the mod `id` (Mod Hub's `modio` when it
-- is not installed): { backend, request } for mod_manager_react_util's
-- ModImage, as the game's mod selector asks it, or nil for none. The game
-- keeps its local mods behind a Mod Hub backend too ("StagingArea").
function roommods.logo(id, modio)
	local ok, logo = pcall(function()
		local rep = app.getUserProfile():getModRep()
		local modId = api.type.ModId.new()
		modId.name = id
		local b, hub
		if rep:exists(modId) then
			b = api.modhub.getBackendIdForSource(rep:getModSource(modId))
			hub = api.modhub.getModHubModIdForModId(modId)
			-- A mod without a logo of its own (the game's, a local one): the
			-- placeholder, not an error for the image Mod Hub cannot give.
			if type(b) ~= "number" or b < 0 or not hub or not hub:isValid() then return nil end
			local info = api.modhub.getModInfoForInstalledMod(b, hub)
			if not info or info.logoImage == nil or info.logoImage == "" then return nil end
		elseif modio ~= "" then
			b, hub = backend(), hubId(modio)
		end
		if type(b) ~= "number" or b < 0 or not hub or not hub:isValid() then return nil end
		return {
			backend = b,
			request = api.type.modhub.GetModMediaRequest.new(hub, api.type.modhub.ModMediaType.Logo, -1),
		}
	end)
	return ok and logo or nil
end

-- The installed mod of Mod Hub's `number`, by its id: "" when none is, or
-- Mod Hub cannot say. Mod Hub's number is the owner's claim; a mod
-- installed for it counts only when its id is the room's.
function roommods.installedId(number)
	local b = backend()
	if not b then return "" end
	local ok, name = pcall(function()
		local rep = app.getUserProfile():getModRep()
		for _i, modId in ipairs(rep:getInstalledMods()) do
			if rep:getModSource(modId) == "mod.io" then
				local hub = api.modhub.getModHubModIdForModId(modId)
				if hub and hub:isValid() and tostring(hub.value) == tostring(number) then return modId.name end
			end
		end
		return ""
	end)
	return ok and tostring(name or "") or ""
end

return roommods
