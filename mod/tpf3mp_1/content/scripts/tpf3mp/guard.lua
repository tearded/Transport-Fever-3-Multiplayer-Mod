-- tpf3mp/guard.lua -- the room's guard on the commands the GUI sends.
--
-- Transport Fever 3's GUI sends most of what a player does as commands,
-- through api.cmd.sendCommand: buying, selling and assigning vehicles,
-- lines, loans (as script events), a construction's parameters, the speed
-- (docs/HOOKS.md, "The player's commands"). In the room's game a command
-- must run in every game at the same update or in none, so the guard sits in
-- front of sendCommand in the GUI's Lua state:
--
-- - a command of a kind in PASS is sent as the player gave it;
-- - a command CARRY makes an action of is handed to the room instead,
--   which orders it for every game, this one included; its callback hears
--   what became of it when this game has applied it (deliver()), with what
--   it made: the new vehicle a window then puts on a line, the new line it
--   opens;
-- - every other kind is refused, as docs/PLAN.md (Part 3) says of every
--   action the room does not carry yet: it is not sent, its callback hears
--   on the next frame that it failed, and the player is told.
--
-- A command's kind is the name of the factory that made it
-- (api.cmd.make<Kind>Cmd), which the guard wraps to note it; a command no
-- wrapped factory made is refused. Outside the room's game (before the room
-- begins, after it ends) every command is sent as it would be.
--
-- Only the GUI state's api.cmd is wrapped. The mod's game script applies the
-- room's actions through its own state's api.cmd, which is left alone, but
-- for commands a player's personal mods' game scripts send there
-- (tpf3mp/modguard.lua).
--
-- Personal mods (docs/MODS.md): a mod only one player may run goes through
-- this guard like the player's own clicks. The one thing it may send past
-- the room is an event to its own game script (makeScriptingSendEventCmd),
-- which runs in this game alone: one whose id names the mod
-- (guard.ownEvent), and neither an id nor a name the game's own scripts or
-- TPF3-MP listen to (guard.RESERVED_IDS, guard.RESERVED_NAMES). Any other
-- event of a personal mod goes the way a click's does: carried if the room
-- carries it, refused if not. A refusal names the mod the command came from
-- (guard.caller).
--
-- Pure Lua; the tests hand install() a fake api.cmd.

local guard = {}

-- Lua 5.2 (the game's) has table.unpack; Lua 5.1 (the tests') unpack.
local unpackArgs = table.unpack or unpack

-- The kinds sent as they are in the room's game, and why.
guard.PASS = {
	-- The speed row. In the room's game the step gate runs the room's pace
	-- whatever the game's own speed says, and reads that speed as the
	-- player's request to the room (docs/HOOKS.md, "The step gate in the
	-- game").
	makeGameSetSpeedCmd = true,
}

-- The vehicle and line commands are made actions by tpf3mp/capture.lua.
local function capture() return require("tpf3mp.capture") end
local function by(name)
	return function(ctx, ...) return capture()[name](ctx, ...) end
end

-- The commands the room carries, by kind: each makes an action table
-- (tpf3mp_proto::action, in the game's units) of the command's arguments,
-- given the context naming what they name (env.context, see capture.lua);
-- nil, or an error, for one it does not carry, which is then refused.
guard.CARRY = {
	makeGameSetCalendarSpeedCmd = by("calendarSpeed"),
	-- The finance window's loans (finances_loan_gui.tl): the loan script's
	-- events, with the loans as the script keeps them. The construction
	-- menu's prospecting: the company script's spawnIndustry
	-- (capture.prospect). The company window's ranks: the growth script's
	-- applyLevel. The subsidy window's answers: the subsidy script's
	-- onAccept and onDecline.
	makeScriptingSendEventCmd = function(ctx, _src, id, name, param)
		if id == "Loan" and type(param) == "table" then
			if name == "Obtain" and type(param[1]) == "table" and type(param[2]) == "table" then
				return { Loan = { Take = { next = param[1], offer = param[2] } } }
			elseif name == "Repay" and type(param[2]) == "table" then
				return { Loan = { Repay = { loan = param[2] } } }
			end
		elseif id == "Companies" and name == "spawnIndustry" then
			return capture().prospect(ctx, param)
		elseif id == "Companies" and name == "MakeGreen" then
			-- The construction menu's Industry Greenification
			-- (industry_greenify_tool.script.tl): capture.greenify.
			return capture().greenify(ctx, param)
		elseif id == "Companies" and name == "startMarketingCampaign" then
			-- The construction menu's marketing campaign
			-- (marketing_campaign_tool.script.tl): capture.marketing.
			return capture().marketing(ctx, param)
		elseif id == "Companies" and name == "applyLevel" then
			-- The company window taking a rank (company.tl): the acting
			-- player's company takes it in every game (tpf3mp/progression.lua).
			local level = type(param) == "table" and param.level or nil
			if type(level) ~= "number" or level ~= math.floor(level) or level < 1 or level > 255 then
				error("a rank of " .. tostring(level), 0)
			end
			return { ApplyRank = { level = level } }
		elseif id == "Notifications" and name == "initialSound" and type(param) == "table"
			and type(param.notificationId) == "number" and param.notificationId >= 0
			and param.notificationId == math.floor(param.notificationId) then
			-- A popup played a notification's first sound (the game's
			-- notification_popups.tl): marked so in every game.
			return { NotificationSeen = { notification = param.notificationId } }
		elseif id == "Subvention" and (name == "onAccept" or name == "onDecline") then
			-- The subsidy window's Accept and Decline (subventions_gui.tl):
			-- the offer by its number and kind (capture.subsidy).
			return capture().subsidy(ctx, name, param)
		end
		-- Which event, for the log.
		error("the " .. tostring(id) .. " script's " .. tostring(name) .. " event", 0)
	end,
	makeVehicleBuyCmd = by("vehicleBuy"),
	makeVehicleReplaceCmd = by("vehicleReplace"),
	makeVehicleSetLineCmd = by("vehicleSetLine"),
	makeVehicleSellCmd = by("vehicleSell"),
	makeVehicleSetStoppedByUserCmd = by("vehicleStop"),
	makeVehicleSendToDepotCmd = by("vehicleToDepot"),
	makeVehicleReverseCmd = by("vehicleReverse"),
	makeVehicleTryToDepartCmd = by("vehicleDepart"),
	makeVehicleSetManualDepartureCmd = by("vehicleManualDeparture"),
	makeLineCreateCmd = by("lineCreate"),
	makeLineUpdateCmd = by("lineUpdate"),
	makeLineDestroyCmd = by("lineDestroy"),
	makeEntitySetNameCmd = by("setName"),
	makeEntitySetColorCmd = by("setColor"),
	-- A town building's Historic Preservation checkbox.
	makeTownBuildingSetBlockedDevelopmentCmd = by("preserve"),
	-- A construction's parameters changed in its window: an edit of that
	-- construction, which every game replaces alike. Other builds a window
	-- sends stay refused.
	makeWorldBuildProposalCmd = by("windowBuild"),
}

-- What a window's callback reads of a command it made that went, by kind:
-- the entity it made (the game's own command data has it there).
guard.RESULT = {
	makeVehicleBuyCmd = function(entity, args)
		return { resultVehicleEntity = entity, playerEntity = args[1], depotEntity = args[2], config = args[3] }
	end,
	makeLineCreateCmd = function(entity) return { resultEntity = entity } end,
	-- The game's windows send a replacement without a callback (build 40408,
	-- vehicle_react_util.tl); one that has one hears the vehicle as it is
	-- after (VehicleReplaceCommandData, api/tealdef/api/cmd.d.tl).
	makeVehicleReplaceCmd = function(entity, args)
		return { vehicleEntity = entity, config = args[2] }
	end,
}

-- Why a RESULT kind with a callback is refused where answers do not reach.
guard.UNTOLD = "a window that waits on what it made, in a Lua state the room's answers do not reach"

-- What a command makes that the window may name in its next command, by
-- the registry's kind (tpf3mp/registry.lua): the store's "buy and put on a
-- line" puts the new vehicle on its line in the buy's callback, and that
-- command names the vehicle by its id, which the GUI reads from the game
-- script's state a moment after its world has the vehicle (2026-09-30:
-- "a vehicle the room cannot name"). So deliver() holds such an answer
-- until the GUI can name what it made, as it holds one for the world.
guard.NAMED = {
	makeVehicleBuyCmd = "vehicles",
	makeLineCreateCmd = "lines",
}

-- What an event's own callback sends after it that the room's action does
-- in every game already, by the event's id and name: the marketing tool
-- books the campaign's price in its callback (marketing_campaign_tool
-- .script.tl), which the room's action books (tpf3mp/apply.lua,
-- HANDLERS.Perk). While that callback runs, the first such command for the
-- player's company is neither sent nor refused, whatever became of the
-- event: carried, the room books it; refused, nothing is booked, as nothing
-- ran.
guard.FOLLOWS = {
	["Companies.startMarketingCampaign"] = "makeJournalBookAssetCmd",
}

-- What the player is told a refused kind is, where "this" would not do.
guard.WHAT = {
	makeVehicleBuyCmd = "buying vehicles",
	makeVehicleSellCmd = "selling vehicles",
	makeVehicleReplaceCmd = "replacing vehicles",
	makeVehicleSetLineCmd = "assigning vehicles to lines",
	makeVehicleSendToDepotCmd = "sending vehicles to a depot",
	makeVehicleReverseCmd = "reversing vehicles",
	makeVehicleSetStoppedByUserCmd = "stopping vehicles",
	makeVehicleSetModifiersCmd = "changing vehicles",
	makeLineCreateCmd = "creating lines",
	makeLineUpdateCmd = "changing lines",
	makeLineDestroyCmd = "deleting lines",
	makeWorldBuildProposalCmd = "building from this window",
	makeEntitySetNameCmd = "renaming",
	makeEntitySetColorCmd = "changing colours",
	makeTownBuildingSetBlockedDevelopmentCmd = "historic preservation",
	makeGameSetCalendarSpeedCmd = "changing the calendar speed",
}

-- The factories the game's API reference declares (build 40408,
-- api/tealdef/api/cmd.d.tl). They are wrapped by name as well as by what
-- pairs() finds, in case api.cmd serves some through a metatable.
guard.FACTORIES = {
	"makeAnimalSetStateCmd", "makeAnimalSpawnAtCmd", "makeClearLogbooksCmd",
	"makeComponentExchangeCmd", "makeCreateIndustryExtendProposalCmd",
	"makeCustomEntityCreateCmd", "makeCustomEntityDestroyCmd",
	"makeCustomEntityUpdateStateCmd", "makeCustomEntityUpdateTransformationCmd",
	"makeCustomVehicleCreateOrUpdateCmd", "makeEntitySetColorCmd",
	"makeEntitySetEmissionsCmd", "makeEntitySetNameCmd", "makeEntitySetPlayerCmd",
	"makeGameAddPlayerCmd", "makeGamePerformSimulationStepsCmd",
	"makeGameSetCalendarSpeedCmd", "makeGameSetCloudCoverageCmd",
	"makeGameSetDateCmd", "makeGameSetSpeedCmd", "makeGameSetTimeOfDayCmd",
	"makeIndustrySetDespawnTimeCmd", "makeIndustrySetManualDevelopmentCmd",
	"makeJournalBookAssetCmd", "makeJournalClearAllCmd", "makeJournalLogEntryCmd",
	"makeLineCreateCmd", "makeLineDestroyCmd", "makeLineUpdateCmd",
	"makeMaintenanceCostUpdateCmd", "makeScriptingSendEventCmd",
	"makeSimPersonSetStateCmd", "makeStockListDiscardCargoCmd",
	"makeStockListSetModifiersCmd", "makeStockListSetStocksCargoTypeCmd",
	"makeStockSetCargoAmountCmd", "makeTownAutoDetectConnectionsCmd",
	"makeTownBuildingSetBlockedDevelopmentCmd", "makeTownConnectWithIndustriesCmd",
	"makeTownCreateCmd", "makeTownCustomDistributionWeightsCmd",
	"makeTownDestroyCmd", "makeTownDevelopAtCmd", "makeTownSetDevelopmentActiveCmd",
	"makeTownSetInitialLandUseCapacitiesCmd", "makeTownUpdateCargoNeedsCmd",
	"makeTownUpdateSizeCmd", "makeVehicleBuyCmd", "makeVehicleReplaceCmd",
	"makeVehicleReverseCmd", "makeVehicleSellCmd", "makeVehicleSendToDepotCmd",
	"makeVehicleSetLineCmd", "makeVehicleSetManualDepartureCmd",
	"makeVehicleSetModifiersCmd", "makeVehicleSetStoppedByUserCmd",
	"makeVehicleTryToDepartCmd", "makeWorldBuildProposalCmd",
	"makeWorldChangeWindCmd", "makeWorldReplaceTerrainCmd",
	"makeWorldSetBulldozableCmd",
}

-- TPF3-MP's own mod, whose files are never "a mod's" to the guards.
guard.OWN = "tpf3mp_1"

-- The mods whose scripts are on the stack of a call, nearest first, each
-- once: every function whose source names a mod's file ("<modId>::/...", as
-- the game names a mod's files) other than TPF3-MP's own. The game's own
-- files ("::/...") name no mod, so a click in the game's own windows gives
-- none. `getinfo` is debug.getinfo (the tests hand a fake one).
function guard.callers(getinfo)
	local mods, seen = {}, {}
	if getinfo == nil and type(debug) == "table" then getinfo = debug.getinfo end
	if type(getinfo) ~= "function" then return mods end
	for level = 1, 64 do
		local ok, info = pcall(getinfo, level, "S")
		if not ok or type(info) ~= "table" then break end
		local source = info.source
		local mod = type(source) == "string" and source:match("^@?([%w_%.%-]+)::/") or nil
		if mod and mod ~= guard.OWN and not seen[mod] then
			seen[mod] = true
			mods[#mods + 1] = mod
		end
	end
	return mods
end

-- The mod whose script made a call, the nearest (guard.callers), or nil.
function guard.caller(getinfo)
	return guard.callers(getinfo)[1]
end

-- Event ids the game's own game scripts and TPF3-MP listen to (build 40408,
-- each base game script's handleEvent), and the empty id of the game's
-- init events: a personal mod's event under one of them is never its own.
guard.RESERVED_IDS = {
	[""] = true, ArrivalTracker = true, CloudCoverage = true, Companies = true,
	Company = true, Emissions = true, GameTime = true, Industries = true, Loan = true,
	MissionEndWindow = true, MissionWindow = true, Notifications = true,
	SimCargoSystem = true, SimEntityAtBuildingSystem = true,
	SimEntityAtTerminalSystem = true, SimPersonAtVehicleSystem = true,
	SimPersonSystem = true, StockListSystem = true, Subvention = true, Towns = true,
	TransportVehicleSystem = true, VehicleModifier = true, apply_command = true,
	fireworks = true, ["guide-system"] = true, ["mission-dialogue"] = true,
	tpf3mp = true,
}

-- Name prefixes some of the game's scripts listen to whatever the id
-- (company.script.tl: "company.lockPermits", "builder.proposalApply").
guard.RESERVED_NAMES = { "company.", "builder.", "init", "handleLegacy" }

-- The id's letters and digits, in lower case.
local function squeeze(text)
	return (tostring(text):lower():gsub("[^%w]", ""))
end

-- Whether the event id `squeezed` (squeeze()d) names the mod `mod`: it
-- contains the mod's id, or one of its words of four letters or more
-- (Timetables' "TimetablesEdit" for celmi_timetables).
local function names(mod, squeezed)
	if squeezed:find(squeeze(mod), 1, true) then return true end
	for word in mod:gmatch("[%w]+") do
		if #word >= 4 and not word:match("^%d+$") and squeezed:find(word:lower(), 1, true) then
			return true
		end
	end
	return false
end

-- Whether an event from the personal mod `mod` (its modId) is addressed to
-- the mod's own game script: its id is not one the game's scripts or
-- TPF3-MP listen to, its name is none they listen to under any id, the id
-- names the mod, and it names none of `shared`, the room's shared mods,
-- whose game scripts run in every game and might hear it too (a personal
-- "timetables_ui_tweak" sending "TimetablesEdit", which the shared
-- celmi_timetables hears, would desync this game). Without the shared
-- list (nil), no event is the mod's own.
function guard.ownEvent(mod, id, name, shared)
	if type(mod) ~= "string" or type(id) ~= "string" or type(name) ~= "string" then return false end
	if type(shared) ~= "table" then return false end
	if guard.RESERVED_IDS[id] then return false end
	for _, prefix in ipairs(guard.RESERVED_NAMES) do
		if name:sub(1, #prefix) == prefix then return false end
	end
	local squeezed = squeeze(id)
	if squeezed == "" or not names(mod, squeezed) then return false end
	for _, other in ipairs(shared) do
		if other ~= mod and names(other, squeezed) then return false end
	end
	return true
end

-- What the player is told when a command of `kind` is refused.
function guard.notice(kind)
	return "Not in multiplayer yet: " .. (guard.WHAT[kind] or "this action")
end

-- The api.cmd tables already guarded, so a second install() changes
-- nothing, the callbacks waiting on each one's commands, by ticket, and
-- the answers held back until the GUI sees what they made.
local guarded = setmetatable({}, { __mode = "k" })
local wakeReplay = setmetatable({}, { __mode = "k" })
local waiting = setmetatable({}, { __mode = "k" })
local held = setmetatable({}, { __mode = "k" })

-- Calls to deliver() an answer waits at most for the GUI to see the entity
-- it made, where deliver() has no clock (one a frame: a few seconds at
-- 60 frames a second, one at 240).
guard.HOLD = 240
-- Seconds an answer waits at most, where deliver() has a clock: frames are
-- no measure of time (2026-10-01: five vehicles bought onto a line in a
-- burst, all but one answered before the GUI could name them, so their
-- line assignments were refused). Each answer has its own, counted from
-- when it is first the one waited on.
guard.HOLD_SECONDS = 20

-- Puts the guard in front of `cmd` (the GUI state's api.cmd). `env` is:
--   inRoom()      -> whether the room's game runs;
--   command(t)    -> hands an action table to the room: true and its ticket,
--                    or nil and why;
--   refused(kind, why) -> a command of `kind` (nil: made by no factory the
--                    guard knows) was refused;
--   later(fn)     -> runs fn on the next frame;
--   context       -> names what commands name (tpf3mp/capture.lua);
--   personal(mod) -> optional: whether `mod` is one of this player's
--                    personal mods (docs/MODS.md);
--   shared()      -> optional: the room's shared mods, a list of names, or
--                    nil (then no personal mod's event is its own);
--   caller()      -> optional: the mod a command came from (guard.caller).
--   caller()      -> optional: the mod a command came from (guard.caller);
--   untold        -> optional: true in a Lua state the room's answers do not
--                    reach (tpf3mp/hudguard.lua): a command whose window
--                    waits on what it made (RESULT) is refused there, why
--                    guard.UNTOLD.
-- refused() is also given the mod the command came from, if one did.
-- Returns the number of factories wrapped, or nil and why the guard could
-- not be put there.
function guard.install(cmd, env)
	if type(cmd) ~= "table" then return nil, "api.cmd is not a table" end
	if guarded[cmd] then return guarded[cmd] end
	local send = cmd.sendCommand
	if send == nil then return nil, "api.cmd has no sendCommand" end
	local event = cmd.makeScriptingSendEventCmd
	if event then
		-- This bypass can send only a wake token, never a player's action.
		wakeReplay[cmd] = function(token)
			return send(event("", "tpf3mp", "command", token))
		end
	end

	-- The factory each command came from, and its arguments, by the command
	-- itself.
	local kinds = setmetatable({}, { __mode = "k" })
	local calls = setmetatable({}, { __mode = "k" })
	-- The mod that made each command, if one did: a window's helper
	-- (engine_react_util's commit) sends what a mod's function made, with
	-- that mod no longer on the stack.
	local makers = setmetatable({}, { __mode = "k" })
	local factories = {}
	local follower = {}
	for _, kind in pairs(guard.FOLLOWS) do follower[kind] = true end
	-- The follow-up a running callback may send that is not to be sent
	-- (guard.FOLLOWS): { kind =, company = }, or nil.
	local absorbing = nil
	local function absorbs(kind, args)
		local a = absorbing
		if a == nil or kind ~= a.kind or args == nil or args[1] ~= a.company then return false end
		absorbing = nil
		return true
	end
	for name, factory in pairs(cmd) do
		if type(name) == "string" and name:match("^make.+Cmd$") then
			factories[name] = factory
		end
	end
	for _, name in ipairs(guard.FACTORIES) do
		if factories[name] == nil then factories[name] = cmd[name] end
	end
	local wrapped = 0
	for name, factory in pairs(factories) do
		cmd[name] = function(...)
			local command = factory(...)
			local t = type(command)
			if t == "table" or t == "userdata" then
				kinds[command] = name
				makers[command] = (env.caller or guard.caller)()
				if guard.CARRY[name] or name == "makeScriptingSendEventCmd" or follower[name] then
					calls[command] = { n = select("#", ...), ... }
				end
			end
			return command
		end
		wrapped = wrapped + 1
	end

	-- The arguments go on exactly as given: a callback left out is not the
	-- same, to the game, as one passed as nil.
	cmd.sendCommand = function(command, ...)
		if not env.inRoom() then
			return send(command, ...)
		end
		local kind = kinds[command]
		if kind ~= nil and guard.PASS[kind] then
			return send(command, ...)
		end
		if kind ~= nil and absorbs(kind, calls[command]) then return end
		local from = makers[command] or (env.caller or guard.caller)()
		-- A personal mod's event to its own game script reaches this game's
		-- scripts alone, where that script runs (docs/MODS.md); any other of
		-- its events is carried or refused below, as a click's.
		if kind == "makeScriptingSendEventCmd" and from and env.personal and env.personal(from) then
			local args = calls[command]
			if args and guard.ownEvent(from, args[2], args[3], env.shared and env.shared()) then
				return send(command, ...)
			end
		end
		local callback = ...
		local carry, args = kind and guard.CARRY[kind], calls[command]
		local follows = kind == "makeScriptingSendEventCmd" and args
			and guard.FOLLOWS[tostring(args[2]) .. "." .. tostring(args[3])]
		if follows and type(callback) == "function" then
			local inner = callback
			local company = env.context and env.context.player and env.context.player()
			callback = function(...)
				absorbing = { kind = follows, company = company }
				local ok, err = pcall(inner, ...)
				absorbing = nil
				if not ok then error(err, 0) end
			end
		end
		local made, action = false, nil
		if carry and args then
			made, action = pcall(carry, env.context, unpackArgs(args, 1, args.n))
		end
		-- A window that waits on what its command made (RESULT) hears it
		-- only where the room's answers reach (env.untold: not here).
		if made and action and env.untold and callback ~= nil and guard.RESULT[kind] then
			made, action = false, guard.UNTOLD
		end
		if made and action then
			local ok, ticket = env.command(action)
			if ok then
				if callback ~= nil then
					if type(ticket) == "number" then
						waiting[cmd][ticket] = { callback = callback, command = command, kind = kind, args = args }
					else
						env.later(function() callback(command, true, {}) end)
					end
				end
				return
			end
			env.refused(kind, ticket, from)
		elseif carry and args and not made then
			-- Why the room cannot carry it: what the capture raised.
			env.refused(kind, tostring(action), from)
		else
			env.refused(kind, nil, from)
		end
		if callback ~= nil then
			env.later(function() callback(command, false, {}) end)
		end
	end
	guarded[cmd] = wrapped
	waiting[cmd] = {}
	return wrapped
end

function guard.wakeReplay(cmd, token)
	if type(token) ~= "string" or not token:match("^%d+$") or #token > 20 then
		return nil, "an ordered replay token is a decimal string"
	end
	local wake = wakeReplay[cmd]
	if not wake then return nil, "the GUI cannot wake the game script" end
	local ok, why = pcall(wake, token)
	return ok or nil, not ok and tostring(why) or nil
end

-- What became of the commands the guard handed to the room: `results` is
-- the hook's list ({ ticket =, ok =, entity =, why = }, bridge.lua's
-- results()). Each waiting callback hears it, with what the room's action
-- made, as the game's own command would have answered, once `sees(entity,
-- kind)` says the GUI's world has what it made, and for the kinds in NAMED
-- that the GUI names it by its id (`kind`, the registry's): the game script
-- made it in the simulation, and a window that hears of it opens it, or
-- puts it on a line, at once. Answers keep
-- their order; one held back holds those after it. Each answer waits on
-- its own entity for HOLD_SECONDS by `now()` (seconds; HOLD calls where
-- there is no clock), counted from when it is first the one waited on, so
-- answers queued behind it keep all of theirs.
-- A command that should have made something and made nothing the game
-- could name is answered as failed, which the windows handle, not as made.
-- Returns how many heard.
function guard.deliver(cmd, results, sees, now)
	local pending = waiting[cmd]
	if pending == nil then return 0 end
	local queue = held[cmd] or {}
	for _, r in ipairs(results or {}) do queue[#queue + 1] = { r = r, calls = 0 } end
	local t = nil
	if now ~= nil then
		local ok, v = pcall(now)
		if ok and type(v) == "number" then t = v end
	end
	local function patient(h)
		if t ~= nil then
			h.since = h.since or t
			return t - h.since < guard.HOLD_SECONDS
		end
		return h.calls < guard.HOLD
	end
	local heard, later = 0, {}
	for _, h in ipairs(queue) do
		local r = h.r
		local w = r.ticket and pending[r.ticket]
		if w then
			local unseen = r.entity ~= nil and sees ~= nil and not sees(r.entity, guard.NAMED[w.kind])
			if #later > 0 then
				-- Behind one waited on: in order, its own wait not begun.
				later[#later + 1] = h
			elseif unseen and patient(h) then
				h.calls = h.calls + 1
				later[#later + 1] = h
			else
				pending[r.ticket] = nil
				local result = guard.RESULT[w.kind]
				if result and r.ok == true and r.entity == nil then
					pcall(w.callback, w.command, false, {})
				else
					local data = (result and r.entity) and result(r.entity, w.args) or w.command
					local entities = r.entity and { { r.entity, 0 } } or {}
					pcall(w.callback, data, r.ok == true, entities)
				end
				heard = heard + 1
			end
		end
	end
	held[cmd] = later
	return heard
end

return guard
