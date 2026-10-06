-- tpf3mp/apply.lua -- runs an action the room ordered, in the mod's game
-- script's postUpdate (docs/HOOKS.md, "Actions in the game").
--
-- A game script runs in an engine state, where a command runs at once (the
-- game's own api/tealdef/api/cmd.d.tl). The game takes no callback in
-- update ("Callbacks are currently disallowed", build 40408), but in
-- postUpdate, where this runs, it calls one at once, with the command's
-- result: the game's mission scripts read the line they made from it right
-- after sendCommand (mission_vehicle_util.tl). A command the game refuses
-- raises, or its callback hears that it failed.
-- Every game applies the room's action in the same simulation update, so
-- what this makes of an action may depend on nothing but the action and the
-- world, which every game has alike: no time of day, no camera, no GUI.
--
-- An action is the table the hook hands over (tpf3mp_proto::lua): one
-- entry, the action's name and its body, in the game's units (metres,
-- plain fractions).
--
-- Pure Lua against the game's `api`; the tests give it a fake one.

local acceptance = ug_require and ug_require("tpf3mp_1::/scripts/tpf3mp/acceptance.lua")
    or require("tpf3mp.acceptance")

local apply = {}

-- A matrix from a Transform: its basis is elements 1-3, 5-7 and 9-11 of the
-- game's matrix and its origin elements 13-15 (columns of four).
local function matrix(transform)
	local b, o = transform.basis, transform.origin
	local column = api.type.Vec4f.new
	return api.type.Mat4f.new(
		column(b[1], b[2], b[3], 0),
		column(b[4], b[5], b[6], 0),
		column(b[7], b[8], b[9], 0),
		column(o.x, o.y, o.z, 1)
	)
end

-- A parameter's value: exactly one of Int, Fixed, Bool and Text.
local function paramValue(value)
	if value.Int ~= nil then return value.Int end
	if value.Fixed ~= nil then return value.Fixed end
	if value.Bool ~= nil then return value.Bool end
	if value.Text ~= nil then return value.Text end
	error("a parameter value of no kind")
end

-- The keys a flattened parameter path names: "modules[3801].name" is
-- modules, 3801, name.
local function pathKeys(path)
	local keys = {}
	for part in string.gmatch(path, "[^%.]+") do
		local name, rest = string.match(part, "^([^%[]*)(.*)$")
		if name ~= "" then keys[#keys + 1] = name end
		for index in string.gmatch(rest, "%[(%-?%d+)%]") do
			keys[#keys + 1] = tonumber(index)
		end
	end
	if #keys == 0 then error("an empty parameter path") end
	return keys
end

-- The construction's parameters, from their flattened paths.
local function params(list)
	local out = {}
	for _, param in ipairs(list) do
		local keys = pathKeys(param.key)
		local node = out
		for i = 1, #keys - 1 do
			local key = keys[i]
			if type(node[key]) ~= "table" then node[key] = {} end
			node = node[key]
		end
		node[keys[#keys]] = paramValue(param.value)
	end
	return out
end

-- Whether a handler runs dry (apply.proposalOf): it makes the proposal its
-- action would build and stops there, sending nothing and saying nothing.
local dry = false

-- A line for the hook's log; the game script sets apply.log once linked.
local function log(line)
	if dry then return end
	if apply.log then pcall(apply.log, line) end
end

-- A list of the action, afresh, for the game to copy. The game copies a
-- list it is handed into its own vector in the order `next` walks it, and
-- the actions reach postUpdate as the game's own copy of what update
-- returned, whose lists `next` walks in hash order (build 40408: a bus
-- line's stops set to load grain, one cargo over from passengers). A table
-- filled 1, 2, 3... walks in order.
local function seq(list)
	local out = {}
	for i = 1, #list do out[i] = list[i] end
	return out
end

-- Whether this Lua state's game takes a command's callback here; the game
-- runs game scripts on a pool of states, so each learns on its own.
local callbacks = true

-- Sends `command`, which runs at once, and returns what the game answered,
-- its command data and result entities (nil, where it answers nothing
-- here). A command the game refuses raises, and apply.run reports it.
local function send(command)
	if dry then error({ dry = "a command that is not a build" }, 0) end
	if callbacks then
		local heard, went, data, entities = false, nil, nil, nil
		local sent, err = pcall(api.cmd.sendCommand, command, function(d, success, e)
			heard, went, data, entities = true, success, d, e
		end)
		if sent then
			if heard and went ~= true then error("the game refused it", 0) end
			return data, entities
		end
		-- The game refuses a callback before it runs anything: sent again
		-- without one, the command runs once.
		if not tostring(err):find("allbacks are currently disallowed", 1, true) then error(err, 0) end
		callbacks = false
		log("the game takes no command callbacks in this state: what an action makes is found by the registry alone")
	end
	api.cmd.sendCommand(command)
	return nil, nil
end

local function run(command)
	send(command)
	return true
end

-- For the game script's own commands (tpf3mp/companies.lua's monthly loan
-- payments): the same send, which answers what the game made.
apply.send = send

local require_companies

-- The action running now: `ctx` as apply.run was given it. Its `company` is
-- the player entity of the acting player's company (tpf3mp/companies.lua);
-- without one, the save's own player, as before companies.
local acting = nil

local function company()
	return (acting and acting.company) or api.engine.util.getPlayer()
end

-- Refuses changing `entity` when another company owns it, naming the owner,
-- the same in every game (tpf3mp/companies.lua).
local function mine(entity, what)
	local companies = require_companies()
	local ok, why = companies.mayTouch(acting and acting.roster, company(), entity, api, what)
	if not ok then error(why, 0) end
end

-- The entity a command made: its data's field `field`, else the first of
-- its result entities; nil when the game did not say.
local function madeBy(field, data, entities)
	local ok, e = pcall(function() return data[field] end)
	if ok and type(e) == "number" and e >= 0 then return e end
	local first = type(entities) == "table" and entities[1]
	e = type(first) == "table" and first[1] or nil
	if type(e) == "number" and e >= 0 then return e end
	return nil
end

-- Builds `proposal` as the player's own build. The game's verdict first, as
-- its tools ask it: a build it would refuse (a collision, too steep, not
-- enough money) fails here with its reasons, the same in every game, and is
-- never sent; sent without a callback, a refused build would fail unseen.
local function buildProposal(proposal, context)
	-- Dry: the proposal is what was asked for; the game's verdict and the
	-- build are not.
	if dry then error({ dry = true, proposal = proposal, context = context }, 0) end
	local proposals = api.engine.util.proposal
	if proposals and proposals.makeProposalData then
		local data = proposals.makeProposalData(proposal, context)
		local state = data and data.errorState
		local messages = {}
		for _, m in ipairs(state and state.messages or {}) do messages[#messages + 1] = tostring(m) end
		if state and state.critical then
			error("the game refuses the build: " .. table.concat(messages, "; "), 0)
		end
		if #messages > 0 then log("the game warns of the build: " .. table.concat(messages, "; ")) end
	end
	-- What is not critical the tool builds through once the player clicks,
	-- town buildings in the way included: ignoreErrors, as the player's own
	-- build (with it false the game drops such a build unseen).
	return run(api.cmd.makeWorldBuildProposalCmd(proposal, context, true, true))
end

local HANDLERS = {}

-- Fills a SimpleProposal's street proposal from a polyline ("roads", below).
local networkInto
-- The construction of a file at a place ("vehicles and lines", below).
local constructionAt
-- Removes a stop from its edge ("stops", below).
local removeEdgeObject

-- Names station groups the room built, the same in every game (2026-10-02:
-- room-built stops and stations stood unnamed). The game's own tools name
-- them natively; the room carries that name from the originator's proposal.
-- A script build with an empty name leaves its entities with no NAME (docs/BUILDING.md,
-- "What a script proposal must carry"). `groups` lists { group =, stations
-- = { ... } }, each a group this build's own stations alone make up; one
-- with a name of its own already keeps it. `name` is the name to give, or
-- nil for the town's: the town the game counts the group's first station
-- in (stationSystem.getTown), and a number after it where another station
-- group of that town has that name ("Didcot", "Didcot 2", ...), so two
-- games, which hold the same world, give the same. `fallback` where the
-- station has no town. Each group and its stations get the name
-- (makeEntitySetNameCmd); hook.log says which.
local PROVISIONAL_STOP_NAME = "Stop"
-- Whether a stop is built with the name the originator's tool gave it
-- (PlaceStop.name, the game's own: street_util::MakeEdgeObjectName). The
-- kill switch: false, and every stop is named by the town as above.
apply.NATIVE_STOP_NAMES = true
local function nameStationGroups(groups, name, fallback)
	local named = {}
	for _, g in ipairs(groups) do
		local current
		pcall(function() current = api.engine.util.getEntityName(g.group) end)
		if type(current) ~= "string" or current == "" or current == PROVISIONAL_STOP_NAME then
			local chosen = name
			if chosen == nil then
				local town = -1
				pcall(function() town = api.engine.system.stationSystem.getTown(g.stations[1]) end)
				local townName
				if type(town) == "number" and town >= 0 then
					pcall(function() townName = api.engine.util.getEntityName(town) end)
				end
				if type(townName) == "string" and townName ~= "" then
					-- The names the town's other station groups have.
					local taken = {}
					pcall(function()
						for _, s in ipairs(api.engine.system.stationSystem.getStations(town) or {}) do
							local other = api.engine.system.stationGroupSystem.getStationGroup(s)
							if type(other) == "number" and other >= 0 and other ~= g.group then
								local n = api.engine.util.getEntityName(other)
								if type(n) == "string" then taken[n] = true end
							end
						end
					end)
					chosen = townName
					local k = 2
					while taken[chosen] do
						chosen = townName .. " " .. k
						k = k + 1
					end
				else
					chosen = fallback
				end
			end
			if type(chosen) == "string" and chosen ~= "" then
				local ok, why = pcall(function()
					send(api.cmd.makeEntitySetNameCmd(g.group, chosen))
					for _, s in ipairs(g.stations) do send(api.cmd.makeEntitySetNameCmd(s, chosen)) end
				end)
				named[#named + 1] = "station group " .. g.group .. " \"" .. chosen .. "\""
					.. (ok and "" or (": refused, " .. tostring(why)))
			end
		end
	end
	if #named > 0 then log("named " .. table.concat(named, ", ")) end
end

-- The station groups construction `con`'s stations alone make up, as
-- nameStationGroups takes them.
local function constructionGroups(con, mustResolve)
	local c = api.engine.getComponent(con, api.type.ComponentType.CONSTRUCTION)
	local mine, byGroup, out = {}, {}, {}
	for _, s in ipairs(seq(c and c.stations or {})) do mine[s] = true end
	for _, s in ipairs(seq(c and c.stations or {})) do
		local group = -1
		local found, result = pcall(api.engine.system.stationGroupSystem.getStationGroup, s)
		if found then
			group = result
		elseif mustResolve then
			error("cannot find station group for station " .. tostring(s) .. ": " .. tostring(result), 0)
		end
		if type(group) == "number" and group >= 0 and not byGroup[group] then
			local g = api.engine.getComponent(group, api.type.ComponentType.STATION_GROUP)
			if mustResolve and g == nil then error("no station group component on " .. tostring(group), 0) end
			local alone = g ~= nil
			for _, other in ipairs(seq(g and g.stations or {})) do
				if not mine[other] then alone = false end
			end
			if alone then
				byGroup[group] = { group = group, stations = {} }
				out[#out + 1] = byGroup[group]
			end
		end
		if byGroup[group] then table.insert(byGroup[group].stations, s) end
	end
	return out
end

-- The game's refresh of construction `con`, and whether it changes its
-- streets. A scripted build does not snap; the game's refresh of a
-- construction does, as its tool does: the entrance then ends at the street
-- node beside it (refreshConstruction, build 40408: the same edge the tool
-- proposed). Logged as what it snaps. The game's verdict takes simple
-- proposals only ("SimpleProposal expected, got Proposal", build 40408): a
-- refresh the game refuses fails in the command's own answer instead (run).
local function refreshOf(con)
	local refresh = api.engine.util.proposal.refreshConstruction(con)
	local street, shape = refresh.proposal, {}
	for i = 1, #street.addedSegments do
		local s = street.addedSegments[i]
		shape[#shape + 1] = "+e" .. s.entity .. ":" .. tostring(s.comp.node0) .. ">" .. tostring(s.comp.node1)
	end
	for i = 1, #street.removedSegments do shape[#shape + 1] = "-e" .. tostring(street.removedSegments[i].entity) end
	log("snapping " .. tostring(con) .. " " .. table.concat(shape, " "))
	return refresh, #shape > 0
end

-- A construction the room built is the acting company's, the same in every
-- game, as the game's own missions hand one over
-- (mission_framework_util_entity.tl, setPlayerForConstruction): the
-- construction, its depots, its stations and the station groups they alone
-- make up, and its own (frozen) edges with what stands on them, each
-- given with makeEntitySetPlayerCmd where anyone else owns it, or no one
-- (2026-10-02: a company's depots did not count as its own). The build
-- names the company (`playerEntity`, `Context.player`); this makes sure of
-- what the engine made from it. hook.log names each one handed over.
local function settleConstruction(con, file)
	local C = api.type.ComponentType
	local me = company()
	local seen, fixed = {}, {}
	local function ownerOf(entity)
		local ok, owned = pcall(api.engine.getComponent, entity, C.PLAYER_OWNED)
		if not ok then error("cannot read " .. tostring(entity) .. " owner: " .. tostring(owned), 0) end
		if owned == nil then return nil end
		local read, owner = pcall(function() return owned.player end)
		if not read then error("cannot read " .. tostring(entity) .. " owner: " .. tostring(owner), 0) end
		if owner == nil or (type(owner) == "number" and owner < 0) then return nil end
		if type(owner) ~= "number" then error("invalid owner on " .. tostring(entity), 0) end
		return owner
	end
	local function give(entity, what)
		if type(entity) ~= "number" or entity < 0 then
			error("the game gave no " .. what .. " entity for " .. tostring(file), 0)
		end
		if seen[entity] then return end
		seen[entity] = true
		local owner = ownerOf(entity)
		if owner == me then return end
		send(api.cmd.makeEntitySetPlayerCmd(entity, me))
		fixed[#fixed + 1] = what .. " " .. entity .. " (was " .. tostring(owner) .. ")"
	end
	if type(con) ~= "number" or con < 0 then error("the game made no " .. tostring(file) .. " construction", 0) end
	local c = api.engine.getComponent(con, C.CONSTRUCTION)
	if c == nil then error("no construction component on " .. tostring(con), 0) end
	give(con, "construction")
	for _, depot in ipairs(seq(c.depots or {})) do give(depot, "depot") end
	for _, station in ipairs(seq(c.stations or {})) do give(station, "station") end
	for _, g in ipairs(constructionGroups(con, true)) do give(g.group, "station group") end
	for _, edge in ipairs(seq(c.frozenEdges or {})) do
		give(edge, "edge")
		local e = api.engine.getComponent(edge, C.BASE_EDGE)
		if e == nil then error("no edge component on " .. tostring(edge), 0) end
		for _, o in ipairs(seq(e.objects or {})) do give(o[1], "edge object") end
	end
	if #fixed > 0 then
		log("the new " .. tostring(file) .. " made the acting company's (" .. tostring(me) .. "): "
			.. table.concat(fixed, ", "))
	end
end

-- An edit of a construction (its modules or parameters, an upgrade): the
-- construction the action names removed and the new one built in one
-- proposal, the old mapped to the new (old2new), as the game's own upgrade
-- makes one (mission_framework_util_entity.tl, upgradeConstruction), so
-- what stood on the old one (its stations, their station groups and the
-- lines that stop there) passes to the new. The streets the edit changes
-- around it (a road split for a new exit, 2026-10-03) go in the same
-- proposal, as a new construction's connection does; the old one's own
-- streets go with it, and the connection may not name them. The game's
-- verdict first, and built as the player's own build, paid by the player
-- (buildProposal). The new one stands where the old one stood, so the next
-- edit, a depot or a line finds it by the same file and place.
-- The stock rail editor's own replacement generator preserves its track
-- reconstruction and snapping information. A SimpleProposal only carries
-- the new construction parameters. Regenerate locally, then recapture to
-- ensure the engine has not expanded this into a different requested edit.
-- Reference: build 40408 gui/construction/construction.tl:1295.
local function sameParameter(a, b, tolerance)
	if type(a) ~= type(b) then return false end
	if type(a) == "number" then return a == b or math.abs(a - b) <= tolerance end
	if type(a) ~= "table" then return a == b end
	for k, v in pairs(a) do if not sameParameter(v, b[k], tolerance) then return false end end
	for k in pairs(b) do if a[k] == nil then return false end end
	return true
end

local function railReplacement(build, old, entity)
	local rail = "::/stations/rail/modular_station/modular_station.con"
	if build.file ~= rail or build.replaces.file ~= rail or build.connection ~= nil then return nil end
	local util = api.engine.util and api.engine.util.proposal
	if not util or not util.createProposalReplaceConstruction then
		error("the game cannot regenerate the rail station edit", 0)
	end
	local full = util.createProposalReplaceConstruction(old, entity.params)
	if full == nil then error("the game could not regenerate the rail station edit", 0) end
	if #full.toRemove ~= 1 or full.toRemove[1] ~= old or #full.toAdd ~= 1 then
		error("the game regenerated a different rail station edit", 0)
	end
	local capture = ug_require and ug_require("tpf3mp_1::/scripts/tpf3mp/capture.lua")
		or require("tpf3mp.capture")
	local action, why = capture.construction(full)
	local rebuilt = action and action.BuildConstruction
	if not rebuilt then error("the regenerated rail station edit cannot travel: " .. tostring(why), 0) end
	local same = sameParameter
	if rebuilt.file ~= build.file or rebuilt.name ~= build.name
		or rebuilt.connection ~= nil or not same(rebuilt.replaces, build.replaces, 0.0011)
		or not same(rebuilt.transform.origin, build.transform.origin, 0.0011)
		or not same(rebuilt.transform.basis, build.transform.basis, 0.0000011)
		or not same(params(rebuilt.params), params(build.params), 0.0000011) then
		error("the game regenerated a different rail station edit", 0)
	end
	return full
end

local function replaceConstruction(build, proposal, entity)
	local old, oldComponent = constructionAt(build.replaces)
	mine(old, "construction")
	proposal.constructionsToAdd = { entity }
	proposal.constructionsToRemove = { old }
	proposal.old2new = { [old] = 0 }
	log("replacing " .. tostring(old) .. " " .. tostring(build.replaces.file) .. " with " .. tostring(build.file))
	if build.connection ~= nil then
		local gone = {}
		local frozen = oldComponent and oldComponent.frozenEdges or {}
		for i = 1, #frozen do gone[frozen[i]] = true end
		networkInto(proposal, nil, nil, nil, build.connection, true, gone)
	end
	local context = api.type.Context.new()
	context.player = company()
	context.gatherBuildings = true
	context.gatherFields = true
	local native = railReplacement(build, old, entity)
	if native then
		-- Build 40408's makeProposalData binding only accepts SimpleProposal,
		-- despite its Teal declaration. The stock UI sends this full native
		-- proposal directly, with errors enabled. Do not discard its graph
		-- just to pass it through the simple-proposal validator.
		if dry then error({ dry = true, proposal = native, context = context }, 0) end
		run(api.cmd.makeWorldBuildProposalCmd(native, context, false, true))
	else
		buildProposal(proposal, context)
	end
	-- What it made, where the action says: this game could name it.
	local new, component = constructionAt({ file = build.file, at = build.transform.origin })
	if native and not sameParameter(component and component.params, entity.params, 0.0000011) then
		error("the game did not apply the requested rail station parameters", 0)
	end
	-- The acting company's, whatever the engine made of it, as a new one.
	settleConstruction(new, build.file)
	-- The native replacement already includes its snapped track graph.
	if native then return true, new end
	-- The new one makes its entrances again itself, unsnapped, as a build
	-- does: a road station edited by the street came loose from it, its
	-- entrance no longer joined to the junction (2026-10-03, in both games).
	-- So every game refreshes it as a build's, which snaps its entrances,
	-- a new one included, onto the streets beside them as the game's own
	-- edit does. Where nothing is to snap, nothing is sent. The edit stands
	-- in every game either way: a refresh the game refuses leaves it as it
	-- was before, the same everywhere, and is logged.
	local snapped, why = pcall(function()
		local refresh, changes = refreshOf(new)
		if changes then run(api.cmd.makeWorldBuildProposalCmd(refresh, nil, true, false)) end
	end)
	if not snapped then log("the edited construction stays unsnapped: " .. tostring(why)) end
	return true, new
end

function HANDLERS.BuildConstruction(build)
	local proposal = api.type.SimpleProposal.new()
	local entity = api.type.SimpleProposal.ConstructionEntity.new()
	entity.fileName = build.file
	entity.transf = matrix(build.transform)
	entity.params = params(build.params)
	entity.name = build.name
	entity.playerEntity = company()
	if build.replaces ~= nil then return replaceConstruction(build, proposal, entity) end
	-- One headquarters a company (tpf3mp/companies.lua): the game's own
	-- permit counts the whole world's, so every game checks the acting
	-- company's instead. An edit of its headquarters (above) is no second.
	local may, why = require_companies().mayBuild(acting and acting.roster, company(), build.file, api)
	if not may then error(why, 0) end
	proposal.constructionsToAdd = { entity }
	-- The streets the tool built around it, in the same proposal: the
	-- street it joins rebuilt through a junction. Not the construction's own
	-- entrance edge, which the tool snapped onto that junction: the
	-- construction makes its entrance again itself, unsnapped, ending a few
	-- metres short (build 40408).
	if build.connection ~= nil then networkInto(proposal, nil, nil, nil, build.connection, true) end
	-- Paid by the player, and clearing town buildings in its way, as the
	-- construction tool builds (the game's bridge and tunnel window names the
	-- player so, gui/entity_window/bridge_and_tunnel.tl); without a context
	-- the game builds for free. playerInitiated true: as the player's own
	-- build (buildProposal).
	local context = api.type.Context.new()
	context.player = company()
	context.gatherBuildings = true
	context.gatherFields = true
	local built = buildProposal(proposal, context)
	-- A station's group by the name the tool gave the construction, where
	-- the game left it unnamed (nameStationGroups).
	pcall(function()
		local con = constructionAt({ file = build.file, at = build.transform.origin })
		local groups = constructionGroups(con)
		if #groups > 0 then nameStationGroups(groups, build.name) end
	end)
	-- The acting company's, whatever the engine made of it.
	do
		local con = constructionAt({ file = build.file, at = build.transform.origin })
		settleConstruction(con, build.file)
	end
	if require_companies().isHeadquarters(api, build.file) then
		-- Whether the engine took it as the company's headquarters (its
		-- PLAYER component's `headquarters`), for hook.log: the game's
		-- capital town and its company views read it.
		pcall(function()
			local p = api.engine.getComponent(company(), api.type.ComponentType.PLAYER)
			log("headquarters for company entity " .. tostring(company()) .. ": its PLAYER names "
				.. tostring(p and p.headquarters))
		end)
	end
	if build.connection == nil then return built end
	-- A scripted build does not snap; the game's refresh of a construction
	-- does, as its tool does: the entrance then ends at the street node
	-- beside it, the junction built above (refreshConstruction, build 40408:
	-- the same edge the tool proposed). So every game refreshes it at once,
	-- for free, as part of this action.
	local con = constructionAt({ file = build.file, at = build.transform.origin })
	return run(api.cmd.makeWorldBuildProposalCmd(refreshOf(con), nil, true, false))
end

-- ---------------------------------------------------------------- roads
--
-- A road or track build (tpf3mp_proto action::Polyline) as a SimpleProposal's
-- street proposal, as the game's own scripted track builder makes one
-- (mission/tasks/auto_builder/track_builder.tl): new nodes and edges with
-- negative ids, existing nodes by their own. A vertex resolves as the
-- originator's tool resolved it: New; the existing node of its network
-- within 1.5 m horizontally, the nearest; or a split of the existing edge
-- between the nodes at its ends, cut in two at the vertex into halves that
-- keep the edge's own component, their tangents scaled to the part of the
-- curve each covers. The edges and nodes a build removes are found the same
-- way: an edge by the nodes at its ends, a node by its position. Each link
-- is the build's street or track, or the kind it names: a piece of the
-- street it joins, rebuilt through the new junction, keeps that street's.
--
-- Every game has the same world, so every game resolves alike; anything that
-- resolves to nothing fails the whole build, in every game.

local function module(name)
	local loaded = package and package.loaded and package.loaded["tpf3mp." .. name]
	if loaded then return loaded end
	if ug_require then return ug_require("tpf3mp_1::/scripts/tpf3mp/" .. name .. ".lua") end
	return require("tpf3mp." .. name)
end

local geom = module("geom")
local junctions = module("junctions")

-- How near an existing node a vertex resolving to it is, horizontally.
local NODE_TOLERANCE = 1.5
-- How near its node each end of a named edge is: the ends are the node's own
-- positions, rounded to the millimetre.
local END_TOLERANCE = 0.5

local function arr(v) return { v.x or v[1], v.y or v[2], v.z or v[3] } end
local function vec(p) return api.type.Vec3f.new(p[1], p[2], p[3]) end
local function scaled(p, k) return { p[1] * k, p[2] * k, p[3] * k } end

-- The game's enums, under api.type.enum ("enum" is a word in Teal, which
-- writes api.type["enum"]).
local function enum(name)
	local e = api.type.enum and api.type.enum[name]
	if e == nil then error("no api.type.enum." .. name) end
	return e
end

-- A resource's id by its name; the game answers -1 for none.
local function find(rep, name)
	local id = api.res[rep].find(name)
	if type(id) ~= "number" or id < 0 then error("no " .. rep .. " resource " .. tostring(name)) end
	return id
end

-- The nodes of a network: each node with an edge of it, and its position.
-- Held while it is read (on TPF2 a pairs() straight off the call let the
-- GC free the map mid-loop).
local function readNodes(network)
	local streets = api.engine.system.streetSystem
	local map
	if network == "Track" then map = streets.getNode2TrackEdgeMap() else map = streets.getNode2StreetEdgeMap() end
	local nodes = {}
	for node in pairs(map) do
		local c = api.engine.getComponent(node, api.type.ComponentType.BASE_NODE)
		if c and c.position then nodes[#nodes + 1] = { id = node, pos = arr(c.position) } end
	end
	return nodes
end

-- The node of `nodes` nearest `p` horizontally within `tol`, the lower id
-- on a tie; nil when none is.
local function nearest(nodes, p, tol)
	local best, bestD
	for _, n in ipairs(nodes) do
		local dx, dy = n.pos[1] - p[1], n.pos[2] - p[2]
		local d = dx * dx + dy * dy
		if d <= tol * tol and (bestD == nil or d < bestD or (d == bestD and n.id < best.id)) then
			best, bestD = n, d
		end
	end
	return best
end

-- The existing edge of `network` between the nodes at `a` and `b`: its id,
-- component and geometry (ends a and b, tangents ta and tb, as geom.lua takes
-- edges), oriented as the game has it. The lowest id if there are several.
local function edgeBetween(nodes, network, a, b)
	local na, nb = nearest(nodes, a, END_TOLERANCE), nearest(nodes, b, END_TOLERANCE)
	if na == nil or nb == nil or na.id == nb.id then return nil end
	local streets = api.engine.system.streetSystem
	local ids
	if network == "Track" then ids = streets.getNodeTrackSegments(na.id) else ids = streets.getNodeStreetSegments(na.id) end
	local found
	for i = 1, (ids and #ids or 0) do
		local id = ids[i]
		local c = api.engine.getComponent(id, api.type.ComponentType.BASE_EDGE)
		if c and ((c.node0 == na.id and c.node1 == nb.id) or (c.node0 == nb.id and c.node1 == na.id))
			and (found == nil or id < found.id) then
			local n0, n1 = na, nb
			if c.node0 == nb.id then n0, n1 = nb, na end
			found = { id = id, comp = c, node0 = n0.id, node1 = n1.id, a = n0.pos, b = n1.pos,
				ta = arr(c.tangent0), tb = arr(c.tangent1) }
		end
	end
	return found
end

local STRUCTURE = { Ground = "NORMAL", Bridge = "BRIDGE", Tunnel = "TUNNEL" }

-- A link's lanes: its template's, or the ones the tool made (a tram track, a
-- bus lane: a lane's transport modes on TF3). The game has no constructor
-- for a lane, so each is a copy of one of the template's (read afresh, so
-- no two are one), set to what the link says.
-- The transport modes a lane names (api.type.enum.TransportMode, 0 to 15).
local MODES = 16
local function lanesFor(link, t)
	if link.lanes == nil or #link.lanes == 0 then return t.laneConfigs end
	local out = {}
	for i, l in ipairs(link.lanes) do
		local fresh = t.laneConfigs
		local lane = fresh[math.min(i, #fresh)]
		if lane == nil then error("a lane its template has none to make it from", 0) end
		lane.speed, lane.width, lane.height, lane.offset = l.speed, l.width, l.height, l.offset
		lane.forward = l.forward == true
		-- Every mode, true or false. Build 40408 reads a lane's modes keyed
		-- from 0 (the TransportMode value) but takes them as a Lua array,
		-- from 1: mode m at m + 1. Keyed from 0 they land one mode off, and
		-- a sidewalk that carries vehicles failed every game's build, then
		-- crashed its simulation (TransportNetworkSystem, `person0 ==
		-- person1`; 2026-10-01).
		local modes = {}
		for m = 0, MODES - 1 do modes[m + 1] = math.floor(l.modes / 2 ^ m) % 2 == 1 end
		lane.transportModes = modes
		out[i] = lane
	end
	return out
end

-- Adds the polyline's nodes and edges, and its removals, to `proposal`'s
-- street proposal. `network`, `templateName` and `style` are the build's
-- own kind, for the links that name none; nil for a construction's
-- streets, whose every link names its kind. With `dangling` true (a
-- construction's streets), peel back complete branches ending at new
-- vertices: the construction makes its own entrance and internal track;
-- and every edge it removes or splits must be the acting company's or no
-- company's (D21), as a bulldozed one. `gone` names the edges an edit's old
-- construction takes with it (its frozen edges): the polyline may not
-- remove or split them, and no junction's settings may name them. Removing only the outermost links
-- leaves duplicate track inside a branched depot (Steam 40408). Existing
-- nodes and splits anchor the external network and are never peeled off.
function apply.ownStreets(polyline)
	local links, skipped = polyline.links, {}
	local degree, incident = {}, {}
	for i = 0, #polyline.vertices - 1 do degree[i], incident[i] = 0, {} end
	for k, link in ipairs(links) do
		for _, i in ipairs({ link.from, link.to }) do
			degree[i] = degree[i] + 1
			incident[i][#incident[i] + 1] = k
		end
	end
	local function loose(i) return polyline.vertices[i + 1].resolve == "New" and degree[i] == 1 end
	local queue, removed = {}, {}
	for i = 0, #polyline.vertices - 1 do if loose(i) then queue[#queue + 1] = i end end
	local head = 1
	while head <= #queue do
		local i = queue[head]
		head = head + 1
		for _, k in ipairs(incident[i]) do
			if not removed[k] then
				removed[k] = true
				local link = links[k]
				local other = link.from == i and link.to or link.from
				degree[i], degree[other] = degree[i] - 1, degree[other] - 1
				if loose(other) then queue[#queue + 1] = other end
			end
		end
	end
	links = {}
	for k, link in ipairs(polyline.links) do if not removed[k] then links[#links + 1] = link end end
	for i, v in ipairs(polyline.vertices) do skipped[i] = v.resolve == "New" and degree[i - 1] == 0 end
	return links, skipped
end

function networkInto(proposal, network, templateName, style, polyline, dangling, gone)
	local links, skipped = polyline.links, {}
	local settings = polyline.junctions
	if dangling then
		links, skipped = apply.ownStreets(polyline)
		-- The tool's settings at the construction's own street's nodes, or
		-- naming its own edges (the entrance at the junction it joins), go
		-- with that street: they name what this build does not make
		-- (2026-10-02: a street station refused in every game, "the
		-- junction no longer exists"). The construction and its refresh
		-- give those junctions the game's own, alike in every game.
		local keep, ownEdges, ownNodes = {}, {}, {}
		for _, link in ipairs(links) do keep[link] = true end
		local function place(i)
			local p = arr(polyline.vertices[i + 1].pos)
			return { x = p[1], y = p[2], z = p[3] }
		end
		for _, link in ipairs(polyline.links) do
			if not keep[link] then
				local net = link.kind and link.kind.network
				ownEdges[#ownEdges + 1] = { network = net, ends = { a = place(link.from), b = place(link.to) } }
				for _, i in ipairs({ link.from, link.to }) do
					if skipped[i + 1] then ownNodes[#ownNodes + 1] = { network = net, at = place(i) } end
				end
			end
		end
		local left
		settings, left = junctions.without(settings, ownNodes, ownEdges)
		if #left > 0 then
			local ok, text = pcall(junctions.summary, { EditJunctions = { changes = left } })
			log("left to the construction: " .. (ok and text or (#left .. " junction(s)")))
		end
	end
	-- The junctions' settings go with it (junctions.into, below).
	polyline = { vertices = polyline.vertices, links = links, removals = polyline.removals,
		removed_nodes = polyline.removed_nodes, junctions = settings }
	local nodesOf = {}
	local function nodes(n)
		if nodesOf[n] == nil then nodesOf[n] = readNodes(n) end
		return nodesOf[n]
	end
	local templates = {}
	local function template(name)
		if templates[name] == nil then
			templates[name] = api.res.streetTemplateRep.get(find("streetTemplateRep", name))
		end
		return templates[name]
	end
	local edgeType = enum("BaseEdgeType")

	-- Ids: the edges from -1, the links first and then two halves per split;
	-- the new nodes after them.
	local splits = 0
	for _, v in ipairs(polyline.vertices) do
		if type(v.resolve) == "table" and v.resolve.Split then splits = splits + 1 end
	end
	local nextEdge, nextNode = -1, -(#polyline.links + 2 * splits) - 1

	local nodesToAdd, edgesToAdd, edgesToRemove = {}, {}, {}
	-- The nodes at the ends of the edges removed, in order: their lane
	-- configurations name those edges, and go with them (below).
	local ends, endSeen = {}, {}
	local function removeEdge(e)
		edgesToRemove[#edgesToRemove + 1] = e.id
		for _, node in ipairs({ e.comp.node0, e.comp.node1 }) do
			if not endSeen[node] then
				endSeen[node] = true
				ends[#ends + 1] = node
			end
		end
	end
	local function addNode(p)
		local n = api.type.NodeAndEntity.new()
		n.entity = nextNode
		nextNode = nextNode - 1
		n.comp.position = vec(p)
		nodesToAdd[#nodesToAdd + 1] = n
		return n.entity
	end
	local function addEdge(kind, node0, node1, p0, p1, t0, t1, comp)
		local s = api.type.SegmentAndEntity.new()
		s.entity = nextEdge
		nextEdge = nextEdge - 1
		if comp ~= nil then s.comp = comp end
		s.type = kind
		s.comp.node0, s.comp.node1 = node0, node1
		s.comp.position0, s.comp.position1 = vec(p0), vec(p1)
		s.comp.tangent0, s.comp.tangent1 = vec(t0), vec(t1)
		edgesToAdd[#edgesToAdd + 1] = s
		return s
	end
	local function kindOf(n) if n == "Track" then return 1 end return 0 end

	-- The links' edges first, as their ids were counted.
	local links = {}
	local ids, at = {}, {}
	for i, v in ipairs(polyline.vertices) do at[i] = arr(v.pos) end
	for k, link in ipairs(polyline.links) do
		local own = link.kind and link.kind.network or network
		if own == nil then error("link " .. k .. " names no kind", 0) end
		links[k] = addEdge(kindOf(own), 0, 0, at[link.from + 1], at[link.to + 1],
			arr(link.tangent0), arr(link.tangent1))
	end

	for i, v in ipairs(polyline.vertices) do
		local p, r = at[i], v.resolve
		if skipped[i] then
			-- Left out, with its link.
		elseif r == "New" then
			ids[i] = addNode(p)
		elseif type(r) == "table" and r.Node then
			local n = nearest(nodes(r.Node), p, NODE_TOLERANCE)
			if n == nil then error("no " .. r.Node .. " node at vertex " .. i) end
			ids[i] = n.id
		elseif type(r) == "table" and r.Split then
			local s = r.Split
			local e = edgeBetween(nodes(s.network), s.network, arr(s.ends.a), arr(s.ends.b))
			if e == nil then error("no " .. s.network .. " edge to split at vertex " .. i) end
			if gone and gone[e.id] then error("vertex " .. i .. " splits the old construction's own edge", 0) end
			if dangling then mine(e.id, "road or track") end
			if #(e.comp.objects or {}) > 0 then
				error("vertex " .. i .. " splits an edge with a stop or signal on it")
			end
			local tol = s.network == "Track" and geom.SPLIT_EPS_TRACK or geom.SPLIT_EPS
			local u, off = geom.parameterAt(e.a, e.ta, e.b, e.tb, p[1], p[2])
			local function from(q) local dx, dy = p[1] - q[1], p[2] - q[2] return math.sqrt(dx * dx + dy * dy) end
			if off > tol then error("vertex " .. i .. " is not on the edge it splits") end
			if from(e.a) < geom.SPLIT_MIN_DIST or from(e.b) < geom.SPLIT_MIN_DIST then
				error("vertex " .. i .. " splits the edge at its end")
			end
			local tm = geom.hermiteTangent(e.a, e.ta, e.b, e.tb, u)
			local mid = addNode(p)
			ids[i] = mid
			removeEdge(e)
			-- Each half the split edge's own component, read afresh, as the
			-- game's electrify task rebuilds an edge (electrify.tl).
			local component = api.type.ComponentType.BASE_EDGE
			addEdge(kindOf(s.network), e.node0, mid, e.a, p, scaled(e.ta, u), scaled(tm, u),
				api.engine.getComponent(e.id, component))
			addEdge(kindOf(s.network), mid, e.node1, p, e.b, scaled(tm, 1 - u), scaled(e.tb, 1 - u),
				api.engine.getComponent(e.id, component))
		else
			error("vertex " .. i .. " resolves as nothing this mod knows")
		end
	end

	for k, link in ipairs(polyline.links) do
		local s = links[k]
		s.comp.node0, s.comp.node1 = ids[link.from + 1], ids[link.to + 1]
		local structure, name = link.structure, "Ground"
		if type(structure) == "table" then name = next(structure) end
		s.comp.type = edgeType[STRUCTURE[name] or error("a link of structure " .. tostring(name))]
		if name == "Bridge" then
			s.comp.typeIndex = find("bridgeTypeRep", structure.Bridge)
		elseif name == "Tunnel" then
			s.comp.typeIndex = find("tunnelTypeRep", structure.Tunnel)
		else
			s.comp.typeIndex = -1
		end
		-- The build's own kind, or the kind the link names.
		local kind = link.kind or { network = network, template = templateName, style = style }
		local t = template(kind.template)
		s.comp.laneConfigs = lanesFor(link, t)
		s.comp.roadTemplate = kind.template
		s.comp.roadStyle = kind.style or t.streetStyle
		s.comp.roadType = kind.network == "Track" and enum("RoadType").TRACK or enum("RoadType").STREET
		-- A track's distance between its centre and its neighbours', its
		-- template's (StreetTemplate.trackDistance): without it the game lays
		-- no shared ballast bed or catenary with the tracks beside it, and
		-- the ground shows between them (2026-10-02, tracks laid side by
		-- side in a room). Every game reads the same template.
		if kind.network == "Track" then
			local ok, d = pcall(function() return t.trackDistance end)
			if ok and type(d) == "number" and d > 0 then s.comp.distance = d end
		end
		-- What the tool left on it: its decorations (by name, as every game
		-- numbers them), the towns' lock, and the acting company's ownership.
		local decorations = {}
		for _, d in ipairs(link.decorations or {}) do
			decorations[#decorations + 1] = { find("edgeDecorationRep", d.name), d.flag == true }
		end
		s.comp.edgeDecorations = decorations
		s.comp.roadDevelopmentLocked = link.locked == true
		-- A street's precedence at its ends, as the tool set it.
		if link.precedence ~= nil then
			if s.streetEdge == nil then
				local ok, street = pcall(function() return api.type.BaseEdgeStreet.new() end)
				s.streetEdge = ok and street or {}
			end
			s.streetEdge.precedenceNode0 = link.precedence.node0
			s.streetEdge.precedenceNode1 = link.precedence.node1
		end
		if link.owned == true then
			local ok = pcall(function() s.playerOwned.player = company() end)
			if not ok then
				local owned = api.type.PlayerOwned.new()
				owned.player = company()
				s.playerOwned = owned
			end
		end
	end

	-- An edge removed with its stops or signals leaves them pointing
	-- nowhere: on TPF2 that crashed every game at the same step
	-- (docs/BUILDING.md). So one with any is removed only where a link
	-- rebuilds it in place, between the same places in the same direction,
	-- which takes its objects under their own entities (as the capture
	-- demands, tpf3mp/engine.lua keptInPlace).
	local taken = {}
	local function sameAt(a, b)
		return math.abs(a[1] - b[1]) < 0.05 and math.abs(a[2] - b[2]) < 0.05 and math.abs(a[3] - b[3]) < 0.05
	end
	for k, r in ipairs(polyline.removals or {}) do
		local e = edgeBetween(nodes(r.network), r.network, arr(r.ends.a), arr(r.ends.b))
		if e == nil then error("no " .. r.network .. " edge to remove (" .. k .. ")") end
		if gone and gone[e.id] then error("removal " .. k .. " is the old construction's own edge", 0) end
		if dangling then mine(e.id, "road or track") end
		local objects = e.comp.objects or {}
		if #objects > 0 then
			local into
			for j, link in ipairs(polyline.links) do
				if not taken[j] and sameAt(at[link.from + 1], e.a) and sameAt(at[link.to + 1], e.b) then
					into = j
					break
				end
			end
			if into == nil then error("removal " .. k .. " has a stop or signal on it and no link rebuilds it") end
			taken[into] = true
			local kept = {}
			for i, o in ipairs(objects) do kept[i] = { o[1], o[2] } end
			links[into].comp.objects = kept
		end
		removeEdge(e)
	end

	local nodesToRemove, removedNode = {}, {}
	for k, n in ipairs(polyline.removed_nodes or {}) do
		local found = nearest(nodes(n.network), arr(n.at), END_TOLERANCE)
		if found == nil then error("no " .. n.network .. " node to remove (" .. k .. ")") end
		nodesToRemove[#nodesToRemove + 1] = found.id
		removedNode[found.id] = true
	end

	-- A node's lane configuration (BASE_NODE_CONFIG) names the edges at it,
	-- and the game cannot read a proposal that removes an edge a
	-- configuration still names (build 40408: "Unknown exception" from
	-- makeProposalData). So the configurations at the ends of the removed
	-- edges go too. junctions.into below adds their settings back with the
	-- replacement edges; a removed node takes its own configuration with it
	-- and may not be named for both.
	local configsToRemove = {}
	for _, node in ipairs(ends) do
		if not removedNode[node]
			and api.engine.getComponent(node, api.type.ComponentType.BASE_NODE_CONFIG) ~= nil then
			configsToRemove[#configsToRemove + 1] = node
		end
	end

	proposal.streetProposal.nodesToAdd = nodesToAdd
	proposal.streetProposal.edgesToAdd = edgesToAdd
	proposal.streetProposal.edgesToRemove = edgesToRemove
	if #nodesToRemove > 0 then proposal.streetProposal.nodesToRemove = nodesToRemove end
	if #configsToRemove > 0 then proposal.streetProposal.nodeConfigsToRemove = configsToRemove end
	-- A preview (dry) leaves the junctions' lane and light settings out:
	-- they draw nothing, and a snapped build's may name a node only its
	-- originator's tool has.
	if not dry then
		local left = junctions.into(proposal, polyline.junctions, ends, mine, gone)
		if left and #left > 0 then
			log("left to the construction: the settings of " .. #left .. " junction(s) at its old edges")
		end
	end

	-- What is sent, in the log before it goes: an exception from the game
	-- does not always come back through pcall.
	local shape = {}
	for _, n in ipairs(nodesToAdd) do
		local p = n.comp.position
		shape[#shape + 1] = string.format("+n%d(%.1f,%.1f,%.1f)", n.entity, p.x, p.y, p.z)
	end
	for _, s in ipairs(edgesToAdd) do
		shape[#shape + 1] = "+e" .. s.entity .. "/" .. tostring(s.type) .. ":" .. tostring(s.comp.node0) .. ">"
			.. tostring(s.comp.node1) .. " " .. tostring(s.comp.roadTemplate)
	end
	shape[#shape + 1] = "-e" .. table.concat(edgesToRemove, ",") .. " -n" .. table.concat(nodesToRemove, ",")
		.. " -c" .. table.concat(configsToRemove, ",")
	log("building " .. table.concat(shape, " "))
end

local function buildNetwork(network, templateName, style, polyline)
	local proposal = api.type.SimpleProposal.new()
	networkInto(proposal, network, templateName, style, polyline)
	-- Paid by the player, as the tool builds; the town buildings in the way
	-- cleared, as the tool clears them (the capture lets only those through).
	local context = api.type.Context.new()
	context.player = company()
	context.gatherBuildings = true
	return buildProposal(proposal, context)
end

function HANDLERS.BuildRoad(road)
	return buildNetwork("Street", road.street, road.style, road.polyline)
end

function HANDLERS.EditJunctions(edit)
	junctions.requireEnabled()
	local proposal = api.type.SimpleProposal.new()
	junctions.into(proposal, edit.changes, {}, mine)
	local context = api.type.Context.new()
	context.player = company()
	return buildProposal(proposal, context)
end

-- The town buildings the game's removal of streets takes with them
-- (makeSegmentsRemoveProposal gathers them as the bulldozer did, through
-- the same street_util::FinishProposal: build 40408, read statically),
-- checked against those the action names, `named`: every one it removes
-- must be a town building named there, of its file within 2 m, and every
-- one named must be removed. So no game removes a building the player did
-- not see go, and none keeps one the player's did not (PLAN.md: no
-- demolition beyond what was asked). Raises otherwise; returns the
-- entities it removes, for the log. A removal whose list does not read
-- passes only when it names none (as before schema 15).
-- The asset groups near (x, y) that hold an asset of `model` there: the
-- game's octree first, every asset group where it gives none.
local function assetGroupsAt(model, x, y, z)
	local engine = module("engine")
	local GROUP = api.type.ComponentType.ASSET_GROUP
	local candidates = {}
	local ok, near = pcall(function()
		return api.engine.util.octree.findEntitiesInCircle(api.type.Vec2f.new(x, y), 1, GROUP)
	end)
	if ok and type(near) == "table" then candidates = near end
	-- TF3 (build 40408) refuses to list asset groups ("Cannot loop over this
	-- component type"): then the octree's answer, none, stands.
	if #candidates == 0 then
		local okAll, all = pcall(api.engine.getEntitiesWithComponent, GROUP)
		if okAll and type(all) == "table" then candidates = all end
	end
	local out = {}
	for i = 1, #candidates do
		local g = candidates[i]
		local okA, assets = pcall(engine.assetsOf, g)
		if okA then
			for _, a in ipairs(assets) do
				if engine.assetAt(a, model, x, y, z) then
					out[#out + 1] = { entity = g, assets = assets }
					break
				end
			end
		end
	end
	return out
end

-- Trees and other assets the asset bulldozer took out of their group
-- (action::Bulldoze::Assets): the one group here that holds exactly
-- `count` assets, the first and every one removed among them, removed and,
-- unless every asset of it went, built again from this game's own copy
-- without them, as the tool builds it (tpf3mp/engine.lua, captureAssets;
-- construction_builder_util::CreateProposalAddAsset, build 40408): one
-- construction entity at the world's origin whose desc is autoRemovable and
-- whose one subconstruction lists the assets kept, the thin ones then the
-- full ones, each its model's file and its world matrix. TF3's
-- Proposal.ConstructionEntity has no writable fileName (it reads the
-- desc's, empty as new() makes it), and its construction is a
-- Proposal.ConstructionResult, whose subconstructions are set. Any other
-- group here, or a removed asset this game cannot tell from another,
-- refuses it, so no game removes other trees than the player's. Paid by the
-- player's company, as the player's own build. The log says the group and
-- its assets before and after, in every game.
local function removeAssets(a, context)
	local engine = module("engine")
	local f = a.first
	local found = {}
	for _, g in ipairs(assetGroupsAt(f.model, f.at.x, f.at.y, f.at.z)) do
		if #g.assets == a.count then
			local used, all = {}, true
			for _, r in ipairs(a.removed) do
				if not engine.takeAsset(g.assets, used, r.model, r.at.x, r.at.y, r.at.z) then all = false break end
			end
			if all then found[#found + 1] = { entity = g.entity, assets = g.assets, used = used } end
		end
	end
	if #found ~= 1 then
		error(#found == 0 and ("no asset group of " .. a.count .. " assets with those trees here")
			or ("more than one asset group of " .. a.count .. " assets with those trees here"), 0)
	end
	local group = found[1]
	for _, r in ipairs(a.removed) do
		if engine.assetsAt(group.assets, r.model, r.at.x, r.at.y, r.at.z) > 1 then
			error(string.format("two assets of %s at %.3f, %.3f, %.3f here: which one went is not clear",
				tostring(r.model), r.at.x, r.at.y, r.at.z), 0)
		end
	end
	local P = api.type.Proposal
	local column = api.type.Vec4f.new
	local function mat4(m)
		return api.type.Mat4f.new(column(m[1], m[2], m[3], m[4]), column(m[5], m[6], m[7], m[8]),
			column(m[9], m[10], m[11], m[12]), column(m[13], m[14], m[15], m[16]))
	end
	-- The assets kept, in the group's order (thin, then full), as the tool
	-- lists them.
	local models = {}
	for i, asset in ipairs(group.assets) do
		if not group.used[i] then
			local tm = P.TransformedModel.new()
			tm.id = asset.model
			tm.transf = mat4(engine.assetMatrix(asset, a.mirrored))
			models[#models + 1] = tm
		end
	end
	local proposal = P.new()
	proposal.toRemove = { group.entity }
	if #models > 0 then
		local sub = P.Subconstruction.new()
		sub.models = models
		local ce = P.ConstructionEntity.new()
		local desc = ce.desc
		if desc == nil then error("this game makes no construction desc for an asset group", 0) end
		desc.autoRemovable = true
		ce.desc = desc
		local con = ce.construction
		if con == nil then error("this game makes no construction for an asset group", 0) end
		con.subconstructions = { sub }
		ce.construction = con
		ce.transf = mat4({ 1, 0, 0, 0, 0, 1, 0, 0, 0, 0, 1, 0, 0, 0, 0, 1 })
		ce.playerEntity = a.owned and company() or -1
		proposal.toAdd = { ce }
	end
	log(string.format("trees: asset group %d of %d assets, %d removed (%s at %.2f,%.2f), rebuilt with %d",
		group.entity, #group.assets, #a.removed, tostring(a.removed[1].model), a.removed[1].at.x,
		a.removed[1].at.y, #models))
	run(api.cmd.makeWorldBuildProposalCmd(proposal, context, true, true))
	-- What stands now: the group that holds the first asset kept, and how
	-- many it holds; with none kept, whether a group still holds the first
	-- one removed (the same in every game, or the replay differed).
	local function holding(model, x, y, z, what)
		local now = assetGroupsAt(model, x, y, z)
		local counts = {}
		for _, g in ipairs(now) do counts[#counts + 1] = tostring(#g.assets) end
		return #now == 0 and ("no group holds the " .. what)
			or (#now .. " group(s) hold the " .. what .. ", of " .. table.concat(counts, ",") .. " assets")
	end
	local after
	for i, asset in ipairs(group.assets) do
		if not group.used[i] then
			after = holding(asset.model, asset.x, asset.y, asset.z, "first tree kept")
			break
		end
	end
	if after == nil then
		local r = a.removed[1]
		after = holding(r.model, r.at.x, r.at.y, r.at.z, "first tree removed")
	end
	log("trees: after the rebuild " .. after)
	return true
end

local function townBuildingsRemoved(proposal, named)
	local ok, removed = pcall(function()
		local l, out = proposal.toRemove, {}
		for i = 1, #l do out[i] = l[i] end
		return out
	end)
	if not ok or type(removed) ~= "table" then
		if #named > 0 then error("the game's removal does not say which town buildings it takes", 0) end
		return ""
	end
	local CONSTRUCTION = api.type.ComponentType.CONSTRUCTION
	local taken, ids = {}, {}
	for _, e in ipairs(removed) do
		local c = api.engine.getComponent(e, CONSTRUCTION)
		local file = c and tostring(c.fileName) or ("entity " .. tostring(e))
		if c == nil or #(c.townBuildings or {}) == 0 then
			error("the game would remove " .. file .. " with the streets", 0)
		end
		local t, best, bestD = c.transf, nil, nil
		for i, ref in ipairs(named) do
			if not taken[i] and ref.file == c.fileName then
				local dx, dy, dz = t[13] - ref.at.x, t[14] - ref.at.y, t[15] - ref.at.z
				local d = dx * dx + dy * dy + dz * dz
				if d <= 4 and (bestD == nil or d < bestD) then best, bestD = i, d end
			end
		end
		if best == nil then error("the game would also remove the town building " .. file .. ", which the player's did not", 0) end
		taken[best] = true
		ids[#ids + 1] = tostring(e)
	end
	for i, ref in ipairs(named) do
		if not taken[i] then error("no town building " .. tostring(ref.file) .. " there to remove", 0) end
	end
	return table.concat(ids, ",")
end

-- The bulldozer's removals, as the game makes them itself: a construction
-- with what is its own (createProposalRemove: its entrance edge and node, as
-- the bulldozer proposed them on build 40408; a town building alone), or
-- edges with the nodes they leave on their own and the town buildings along
-- them (makeSegmentsRemoveProposal; townBuildingsRemoved). Paid by the
-- player's company, as the tool removes (the context's player), town
-- buildings' demolition included; the town's reputation follows from the
-- same proposal, as the player's own build (playerInitiated: the game's
-- towns script, onPreBuildProposal), in every game at the same step.
function HANDLERS.Bulldoze(b)
	local context = api.type.Context.new()
	context.player = company()
	local proposals = api.engine.util.proposal
	local proposal
	if b.Construction then
		local con = constructionAt(b.Construction)
		mine(con, "construction")
		proposal = proposals.createProposalRemove(con, context)
		if proposal == nil then error("the game will not remove the " .. tostring(b.Construction.file), 0) end
		log("removing " .. tostring(con) .. " " .. tostring(b.Construction.file))
	elseif b.Edges then
		local network = b.Edges.network
		local nodes = readNodes(network)
		local ids = {}
		for k, ends in ipairs(b.Edges.edges) do
			local e = edgeBetween(nodes, network, arr(ends.a), arr(ends.b))
			if e == nil then error("no " .. network .. " edge to remove (" .. k .. ")", 0) end
			if #(e.comp.objects or {}) > 0 then error("edge " .. k .. " has a stop or signal on it", 0) end
			mine(e.id, "road or track")
			ids[#ids + 1] = e.id
		end
		proposal = proposals.makeSegmentsRemoveProposal(ids)
		local gone = townBuildingsRemoved(proposal, b.Edges.buildings or {})
		log("removing " .. network .. " edges " .. table.concat(ids, ",")
			.. (gone ~= "" and (" and town buildings " .. gone) or ""))
	elseif b.EdgeObject then
		-- A simple proposal: the game's verdict first (buildProposal).
		return removeEdgeObject(b.EdgeObject, context)
	elseif b.Assets then
		return removeAssets(b.Assets, context)
	else
		return false, "a bulldoze of no kind"
	end
	-- The game's verdict takes simple proposals only: a removal it refuses
	-- fails in the command's own answer (run).
	return run(api.cmd.makeWorldBuildProposalCmd(proposal, context, true, true))
end

function HANDLERS.BuildTrack(track)
	return buildNetwork("Track", track.track, track.style, track.polyline)
end

-- An upgrade tool's build, said in the log once every game built it
-- (tpf3mp/roads.lua upgradeSummary): the same line in every game.
for _, name in ipairs({ "BuildRoad", "BuildTrack" }) do
	local build = HANDLERS[name]
	HANDLERS[name] = function(body)
		local ok, why = build(body)
		if ok == true then
			local summarised, text = pcall(module("roads").upgradeSummary, { [name] = body })
			if summarised and text then log("upgrade applied: " .. text) end
		end
		return ok, why
	end
end


-- ------------------------------------------------------------ terraform
--
-- A terrain tool's stroke, as its height grid (tpf3mp/capture.lua,
-- capture.terraform). A script cannot fill a proposal's height grid (Lua's
-- GridVec2f has no setter, build 40408), so the hook does: the grid goes to
-- the hook (apply.terrain, which the game script sets to the link's
-- tpf3mp_native.terrain), then an empty proposal, the carrier, is sent as
-- the player's build, paid by the player as the tool's is; the hook fills
-- the carrier's height grid at its apply (crates/tpf3mp-hook/src/terrain.rs).
-- Then the hook is disarmed, and the action fails unless the carrier was
-- filled. No verdict first: the game's verdict reads the proposal as sent,
-- empty.
function HANDLERS.Terraform(t)
	if type(apply.terrain) ~= "function" then error("this hook cannot apply a terraform", 0) end
	local ok, resolution = pcall(function() return api.engine.terrain.getBaseResolution() end)
	local cell = ok and resolution and (resolution.x or resolution[1])
	if type(cell) ~= "number" or math.abs(cell - t.cell) > 1e-6 then
		error("a grid of " .. tostring(t.cell) .. " m cells; this map's are " .. tostring(cell), 0)
	end
	local x0, y0 = t.origin.x / t.cell, t.origin.y / t.cell
	if x0 ~= math.floor(x0) or y0 ~= math.floor(y0) then error("a grid that starts between cells", 0) end
	local width = t.columns
	local height = #t.cells / width
	local values, low, high = {}, nil, nil
	for i, c in ipairs(t.cells) do
		values[2 * i - 1], values[2 * i] = c.target, c.before
		low, high = math.min(low or c.target, c.target), math.max(high or c.target, c.target)
	end
	local armed, why = apply.terrain({ x0 = x0, y0 = y0, width = width, height = height, cells = values })
	if armed ~= true then error("the hook would not take the grid: " .. tostring(why), 0) end
	local context = api.type.Context.new()
	context.player = company()
	local sent, err = pcall(function()
		return run(api.cmd.makeWorldBuildProposalCmd(api.type.Proposal.new(), context, true, true))
	end)
	local filled = apply.terrain(nil)
	if not sent then error(err, 0) end
	if filled ~= true then error("the hook filled no build with the grid", 0) end
	log(string.format("terraform applied: %d by %d cells from cell (%d, %d), heights %.2f to %.2f m",
		width, height, x0, y0, low or 0, high or 0))
	return true
end

-- ---------------------------------------------------------------- stops
--
-- A stop is placed, or removed, as the stop tool and the bulldozer propose
-- it (tpf3mp/engine.lua): the edge removed and added again between the same
-- nodes, its own component read afresh (as the game's electrify task
-- rebuilds an edge, electrify.tl), so every stop and signal it had stays
-- under its own entity, re-parented with its station group and lines. A new
-- stop is `edgeObjectsToAdd[1]`, named in the edge's objects as -1 (TPF2's
-- tool and scripts did so; INFERRED on TF3); a removed one goes into
-- `edgeObjectsToRemove`. The lane configurations at the edge's ends name it:
-- they are replaced by the same settings naming the rebuilt edge, as for any
-- edge a replay removes (networkInto).

-- The existing edge a stop action names, as edgeBetween finds it.
local function stopEdge(ref)
	local e = edgeBetween(readNodes(ref.network), ref.network, arr(ref.ends.a), arr(ref.ends.b))
	if e == nil then error("no " .. ref.network .. " edge for the stop", 0) end
	return e
end

-- A proposal that removes edge `e` and adds it again with `objects`.
local function rebuildWith(e, network, objects)
	local proposal = api.type.SimpleProposal.new()
	local s = api.type.SegmentAndEntity.new()
	s.entity = -1
	s.comp = api.engine.getComponent(e.id, api.type.ComponentType.BASE_EDGE)
	s.type = network == "Track" and 1 or 0
	s.comp.objects = objects
	-- The edge keeps its owner: its PlayerOwned is a component of its own,
	-- which the edge's BASE_EDGE does not carry, and without it the rebuilt
	-- edge would be no one's (a company's road everyone's).
	local owner = require_companies().ownerOf(api, e.id)
	if owner ~= nil then
		local ok = pcall(function() s.playerOwned.player = owner end)
		if not ok then
			local owned = api.type.PlayerOwned.new()
			owned.player = owner
			s.playerOwned = owned
		end
	end
	proposal.streetProposal.edgesToAdd = { s }
	proposal.streetProposal.edgesToRemove = { e.id }
	-- The lane configurations at its ends name the edge, so they go and come
	-- back naming the rebuilt one, their turns, crosswalks and lights as they
	-- were (junctions.renamed); one that cannot fails the stop in every game.
	-- Removed alone, a junction with traffic lights kept its lights with no
	-- configuration: a fatal assert in every game (build 40408, 2026-10-04: a
	-- stop on a town road between two traffic lights crashed a room,
	-- ecs::Engine::GetComponentDataIndex, BaseNodeConfig).
	junctions.renamed(proposal, { e.comp.node0, e.comp.node1 }, e.id, -1, s.comp)
	return proposal
end

-- A stop the room placed is the acting company's, the same in every game
-- (2026-10-02: a company's stops came out another company's, and its
-- player could not open them). The stop's edge object is named for it
-- (`playerEntity`), but what the game's windows and its line manager ask
-- is the owner of the stop's station group (gui/entity_window/
-- station_group.tl). A street stop's edge object is its station itself
-- (mission/name_util.tl: an entity with EDGE_OBJECT and STATION), and its
-- group is the station group system's (getStationGroup of the object); a
-- stop built as a construction has its stations in the construction. So,
-- once built, each new object on the edge, its station group, and any
-- construction with its stations and their groups are given to the acting
-- company where they are anyone else's, as the game's own missions hand a
-- stop over (transfer_ownership_util.tl, makeEntitySetPlayerCmd). A
-- station group that holds a station of another stop is left as it is: it
-- is not this stop's to give. hook.log says, for each new object, whether
-- it is a station and which group holds it (2026-10-02, retest: the hand-
-- over found the objects alone, and no icon showed). `kept` are the
-- edge's objects as the proposal listed them, the new ones negative.
local function settleStop(ref, kept, model, name)
	local ok, e = pcall(stopEdge, ref)
	if not ok then
		log("the new " .. tostring(model) .. ": its edge cannot be found again to settle its owner")
		return
	end
	local had = {}
	for _, o in ipairs(kept) do
		if o[1] >= 0 then had[o[1]] = true end
	end
	local me = company()
	local companies = require_companies()
	local C = api.type.ComponentType
	local seen, fixed, found = {}, {}, {}
	local function give(entity, what)
		if type(entity) ~= "number" or entity < 0 or seen[entity] then return end
		seen[entity] = true
		local owner = companies.ownerOf(api, entity)
		if owner == me then return end
		-- The stop stands either way: a refusal is logged, not the action's.
		local sent, why = pcall(function() send(api.cmd.makeEntitySetPlayerCmd(entity, me)) end)
		fixed[#fixed + 1] = what .. " " .. entity .. " (was " .. tostring(owner) .. ")"
			.. (sent and "" or (": refused, " .. tostring(why)))
	end
	local function groupOf(station)
		local group = -1
		pcall(function() group = api.engine.system.stationGroupSystem.getStationGroup(station) end)
		if type(group) ~= "number" or group < 0 then return nil end
		return group
	end
	-- The new objects, and every station of this stop: what its groups may
	-- hold and still be its own.
	local objects, stations, mine, conOf = {}, {}, {}, {}
	for _, o in ipairs(e.comp.objects or {}) do
		if not had[o[1]] then
			objects[#objects + 1] = o[1]
			mine[o[1]] = true
		end
	end
	for _, object in ipairs(objects) do
		local isStation = false
		pcall(function() isStation = api.engine.getComponent(object, C.STATION) ~= nil end)
		local con = -1
		pcall(function() con = api.engine.util.construction.getConstructionEntity(object) end)
		if type(con) ~= "number" then con = -1 end
		local group = groupOf(object)
		found[#found + 1] = tostring(object) .. (isStation and " a station" or " no station")
			.. (group and (" in group " .. group .. " (owner " .. tostring(companies.ownerOf(api, group)) .. ")")
				or " in no group")
			.. (con >= 0 and (", construction " .. con) or "")
		if isStation or group then stations[#stations + 1] = object end
		if con >= 0 then
			local c = api.engine.getComponent(con, C.CONSTRUCTION)
			for _, s in ipairs(c and c.stations or {}) do
				stations[#stations + 1] = s
				mine[s] = true
			end
			conOf[object] = con
		end
	end
	for _, object in ipairs(objects) do
		give(object, "stop")
		if conOf[object] then give(conOf[object], "construction") end
	end
	local own, byGroup = {}, {}
	for _, s in ipairs(stations) do
		give(s, "station")
		local group = groupOf(s)
		local g = group and api.engine.getComponent(group, C.STATION_GROUP)
		local alone = g ~= nil and g ~= false
		for _, other in ipairs(g and g.stations or {}) do
			if not mine[other] then alone = false end
		end
		if alone then
			give(group, "station group")
			if not byGroup[group] then
				byGroup[group] = { group = group, stations = {} }
				own[#own + 1] = byGroup[group]
			end
			table.insert(byGroup[group].stations, s)
		end
	end
	log("the new " .. tostring(model) .. ": " .. table.concat(found, "; "))
	if #fixed > 0 then
		log("the new " .. tostring(model) .. " made the acting company's (" .. tostring(me) .. "): "
			.. table.concat(fixed, ", "))
	end
	-- Named after its town, as nameStationGroups names a stop's group.
	-- By the tool's name where the stop carries one (only where the game
	-- left its group unnamed), else after its town.
	nameStationGroups(own, name, PROVISIONAL_STOP_NAME)
end

-- How near its edge's centreline a stop's place is: the originator's own
-- point of that centreline, rounded to the millimetre.
local STOP_TOLERANCE = 0.5

-- The entity a proposal gives its first new edge object (build 40408).
local NEW_EDGE_OBJECT = -400000000

function HANDLERS.PlaceStop(stop)
	local network = stop.edge.network
	-- The tool's own name for it, as every game received it.
	local native = apply.NATIVE_STOP_NAMES and type(stop.name) == "string" and stop.name ~= "" and stop.name or nil
	local e = stopEdge(stop.edge)
	local u, off = geom.parameterAt(e.a, e.ta, e.b, e.tb, stop.at.x, stop.at.y)
	if off > STOP_TOLERANCE then error("the stop's place is not on its edge", 0) end
	-- The engine's side, flipped where this edge runs the other way.
	local left = stop.left == true
	local t, d = geom.hermiteTangent(e.a, e.ta, e.b, e.tb, u), stop.direction
	if t[1] * d.x + t[2] * d.y + t[3] * d.z < 0 then left = not left end
	local types = enum("EdgeObjectType")
	local isStop = stop.object == nil or stop.object == "Stop"
	local function typeOf(l)
		if not isStop then return types.SIGNAL end
		return l and types.STOP_LEFT or types.STOP_RIGHT
	end
	-- The sides it takes: one, or both for a two-sided stop, the
	-- originator's first side first, as its tool added them.
	local sides = { left }
	if isStop and stop.two_sided == true then sides[2] = not left end
	-- One stop a side: a second is a fatal assert in the game's lane
	-- creation (TPF2, docs/BUILDING.md). Signals are not by side.
	local objects = {}
	for i, o in ipairs(e.comp.objects or {}) do
		if isStop then
			for _, l in ipairs(sides) do
				if o[2] == typeOf(l) then error("the edge has a stop on that side already", 0) end
			end
		end
		objects[i] = { o[1], o[2] }
	end
	local added = {}
	for k, l in ipairs(sides) do
		-- A new edge object is named by its place in edgeObjectsToAdd,
		-- from -400000000 down (build 40408: con_util_entity_index.h
		-- asserts the range, a fatal error; game_mechanics/towns/
		-- town_util.tl; the stop tool's own proposals).
		objects[#objects + 1] = { NEW_EDGE_OBJECT - (k - 1), typeOf(l) }
		local eo = api.type.SimpleStreetProposal.EdgeObject.new()
		eo.edgeEntity = -1
		eo.param = u
		eo.left = l
		eo.oneWay = stop.one_way == true
		eo.model = stop.model
		eo.playerEntity = company()
		-- A name, so the engine gives the stop its NAME and its owner
		-- (docs/BUILDING.md: an empty name leaves both off); its group is
		-- named after its town once built (settleStop).
		eo.name = native or PROVISIONAL_STOP_NAME
		added[k] = eo
	end
	local proposal = rebuildWith(e, network, objects)
	proposal.streetProposal.edgeObjectsToAdd = added
	log(string.format("placing %s on %s edge %d at %.4f, %s", tostring(stop.model), network, e.id, u,
		stop.two_sided == true and "both sides" or (left and "left" or "right")))
	-- Paid by the player, as the tool builds.
	local context = api.type.Context.new()
	context.player = company()
	local built = buildProposal(proposal, context)
	settleStop(stop.edge, objects, stop.model, native)
	return built
end

-- A stop the bulldozer removes: the object of that construction on the
-- edge, nearest where it stood, within 2 m.
function removeEdgeObject(ref, context)
	local network = ref.edge.network
	local e = stopEdge(ref.edge)
	local best, bestD
	for _, o in ipairs(e.comp.objects or {}) do
		local c = api.engine.getComponent(o[1], api.type.ComponentType.EDGE_OBJECT)
		local t = c and c.transf
		if c and c.edgeObjectConstruction == ref.model and t then
			local dx, dy, dz = t[13] - ref.at.x, t[14] - ref.at.y, t[15] - ref.at.z
			local dist = dx * dx + dy * dy + dz * dz
			if dist <= 4 and (bestD == nil or dist < bestD or (dist == bestD and o[1] < best)) then
				best, bestD = o[1], dist
			end
		end
	end
	if best == nil then error("no " .. tostring(ref.model) .. " there", 0) end
	mine(best, "stop")
	local objects = {}
	for _, o in ipairs(e.comp.objects) do
		if o[1] ~= best then objects[#objects + 1] = { o[1], o[2] } end
	end
	local proposal = rebuildWith(e, network, objects)
	proposal.streetProposal.edgeObjectsToRemove = { best }
	log("removing " .. tostring(ref.model) .. " " .. tostring(best) .. " from " .. network .. " edge " .. e.id)
	return buildProposal(proposal, context)
end

-- ------------------------------------------------------ vehicles and lines
--
-- Vehicles, lines and station groups are named by canonical id
-- (tpf3mp/registry.lua): `ctx.registry` is the game script's, up to date.
-- A depot is named by its construction's file and position.

local registry = module("registry")
local captureModule = module("capture")
local companiesModule = module("companies")
require_companies = function() return companiesModule end
local progressionModule = module("progression")

local function entityOf(ctx, kind, id)
	local e = registry.entity(ctx and ctx.registry, kind, id)
	if e == nil then error("no " .. kind .. " " .. tostring(id) .. " in this world", 0) end
	return e
end

-- As entityOf, for one the acting company must own: its own vehicles and
-- lines, never another company's.
local OWNED_WHAT = { vehicles = "vehicle", lines = "line" }
local function ownOf(ctx, kind, id)
	local e = entityOf(ctx, kind, id)
	mine(e, OWNED_WHAT[kind] or kind)
	return e
end

-- The construction of `ref.file` whose origin is within 2 m of `ref.at`, the
-- nearest, the lower entity on a tie.
function constructionAt(ref)
	local CONSTRUCTION = api.type.ComponentType.CONSTRUCTION
	local list = api.engine.getEntitiesWithComponent(CONSTRUCTION)
	local best, bestD
	for i = 1, #list do
		local e = list[i]
		local c = api.engine.getComponent(e, CONSTRUCTION)
		if c and c.fileName == ref.file then
			local t = c.transf
			local dx, dy, dz = t[13] - ref.at.x, t[14] - ref.at.y, t[15] - ref.at.z
			local d = dx * dx + dy * dy + dz * dz
			if d <= 4 and (bestD == nil or d < bestD or (d == bestD and e < best)) then best, bestD = e, d end
		end
	end
	if best == nil then error("no " .. tostring(ref.file) .. " there", 0) end
	return best, api.engine.getComponent(best, CONSTRUCTION)
end

-- One channel of a colour as the room carries it, in millionths, put back
-- on the game's 1/255 steps when it was on one. The game's colours are
-- steps (gui/main/color_internal.lua: channel / 255), and its line manager
-- tells which palette colour a line wears by floor(channel * 255)
-- (line_vehicle_mgmt/line_util.tl, calcColorKey): 127/255 carried as
-- 0.498039 is step 126 there, so a new line never counted against the
-- colour it took, and every next line took that colour again. The
-- millionths are at most 0.00013 of a step off; a channel further from a
-- step is not one, and stays as carried. Plain arithmetic, the same in
-- every game.
local function channel(v)
	local steps = v * 255
	local step = math.floor(steps + 0.5)
	if math.abs(steps - step) < 0.001 then return step / 255 end
	return v
end

local function tint(c) return api.type.Vec3f.new(channel(c.r), channel(c.g), channel(c.b)) end

-- The game's time here, the same in every game: when a part is bought.
local function now()
	return api.engine.getComponent(api.engine.util.getWorld(), api.type.ComponentType.GAME_TIME).gameTime
end

-- A ConsistPart as the game's TransportVehiclePart, bought at `time`. Every
-- compartment loads automatically, as the store sends it
-- (vehicle_react_util.tl).
-- How many compartments a vehicle model has (its transportVehicle
-- metadata), or nil where the game does not say.
function apply.compartments(model)
	local ok, n = pcall(function()
		local tv = api.res.modelRep.get(model).metadata.transportVehicle
		local count = 0
		for _ in ipairs(tv.compartments) do count = count + 1 end
		return count
	end)
	if ok and type(n) == "number" then return n end
	return nil
end

local function vehiclePart(p, time)
	local model = api.res.modelRep.find(p.model)
	if type(model) ~= "number" or model < 0 then error("no vehicle model " .. tostring(p.model), 0) end
	local part = api.type.TransportVehiclePart.new()
	part.part.modelId = model
	part.part.reversed = p.reversed == true
	local loads, auto = {}, {}
	for k, l in ipairs(p.loads) do
		local lc = api.type.LoadConfig.new()
		lc.loadConfigIndex = l.config
		lc.cargoTypeId = l.cargo
		loads[k], auto[k] = lc, true
	end
	-- The game takes a load for every compartment of the model, and throws
	-- ("Unknown exception", build 40408) for a part with fewer. A part that
	-- names none gets the store's own: the first load configuration of each
	-- compartment (gui/line_vehicle_mgmt/vehicle_util.tl); one that names
	-- some but not all is refused, the same in every game.
	local compartments = apply.compartments(model)
	if compartments ~= nil and #loads ~= compartments then
		if #loads > 0 then
			error(string.format("%s has %d compartments, and the part loads %d", tostring(p.model), compartments,
				#loads), 0)
		end
		for k = 1, compartments do
			local lc = api.type.LoadConfig.new()
			lc.loadConfigIndex = 0
			loads[k], auto[k] = lc, true
		end
	end
	part.part.compartment2loadConfig = loads
	part.part.color = tint(p.color)
	part.purchaseTime = time
	part.autoLoadConfig = auto
	return part
end

-- A TransportVehicleConfig of these parts, groups and multiple units.
local function vehicleConfig(vehicles, groups, units)
	local config = api.type.TransportVehicleConfig.new()
	config.vehicles = vehicles
	config.vehicleGroups = seq(groups)
	config.muFileNames = seq(units)
	return config
end

-- The depot a purchase names: its construction's depot by its index there,
-- among the construction's depots as the capture listed them
-- (capture.depotsOf: its `depots`, then its subconstructions that are
-- depots, such as an airfield's or airport's hangar). One the construction
-- does not have is refused, the same in every game, saying how many it has;
-- never another depot of it.
local function purchaseDepot(buy)
	local _, construction = constructionAt(buy.depot)
	local depots = captureModule.depotsOf(api, construction)
	local index = (buy.depot_index or 0) + 1
	local depot = depots[index]
	if depot ~= nil then return depot end
	local file = tostring(buy.depot.file)
	if #depots == 0 then
		error(string.format("the %s there has no depot: an airfield or airport has one only with a hangar "
			.. "module, and a harbour never has one (ships are bought at a ship depot)", file), 0)
	end
	error(string.format("the %s there has %d depot(s), and no depot %d", file, #depots, index), 0)
end

function HANDLERS.BuyVehicle(buy)
	local depot = purchaseDepot(buy)
	local allowed, why = companiesModule.mayBuyAtDepot(acting and acting.roster, company(), depot, api)
	if not allowed then error(why, 0) end
	local time = now()
	local vehicles = {}
	for i, p in ipairs(buy.consist) do vehicles[i] = vehiclePart(p, time) end
	local config = vehicleConfig(vehicles, buy.groups, buy.multiple_units)
	-- The game's buy says nothing of why it refused: the likeliest reasons,
	-- the company's money and the depot's room, go with the refusal.
	local sent, data, entities = pcall(send, api.cmd.makeVehicleBuyCmd(company(), depot, config))
	if not sent then
		local facts = {}
		pcall(function()
			local account = api.engine.getComponent(company(), api.type.ComponentType.ACCOUNT)
			facts[#facts + 1] = "the company has " .. string.format("%d", account.balance)
		end)
		for i, p in ipairs(buy.consist) do
			pcall(function()
				local model = api.res.modelRep.find(p.model)
				facts[#facts + 1] = string.format("part %d %s: %s compartments, %d loads", i, tostring(p.model),
					tostring(apply.compartments(model)), #p.loads)
			end)
		end
		facts[#facts + 1] = "depot entity " .. tostring(depot)
		pcall(function()
			local d = api.engine.getComponent(depot, api.type.ComponentType.VEHICLE_DEPOT)
			if d and d.vehicles then facts[#facts + 1] = "the depot holds " .. #d.vehicles end
		end)
		error(tostring(data) .. (#facts > 0 and (" (" .. table.concat(facts, "; ") .. ")") or ""), 0)
	end

	local vehicle = madeBy("resultVehicleEntity", data, entities)
	-- With more than one company, in its company's colour.
	local roster = acting and acting.roster
	if vehicle and companiesModule.painting(roster) then
		companiesModule.paintVehicle(companiesModule.byEntity(roster, company()), vehicle, send, api)
	end
	return true, vehicle
end

-- Whether `e` is a vehicle in this world.
local function isVehicle(e)
	if type(e) ~= "number" or e < 0 then return false end
	local ok, c = pcall(api.engine.getComponent, e, api.type.ComponentType.TRANSPORT_VEHICLE)
	return ok and c ~= nil
end

-- A vehicle's consist replaced, as the store's HandleVehicleChanges sends it
-- (vehicle_react_util.tl): a part the vehicle keeps is its own part, its
-- purchase time and wear as this game has them now, with the facing, loads
-- and colour the player chose; a new part is bought now. The vehicle is
-- then the entity the game names (its command data, its result entities)
-- that is a vehicle, else the vehicle itself: TF3's API says the vehicle is
-- replaced (cmd.d.tl), and its command data has no result field. Returns
-- that entity, for the registry to keep the vehicle's id on.
function HANDLERS.ReplaceVehicle(replace, ctx)
	local vehicle = ownOf(ctx, "vehicles", replace.vehicle)
	local tv = api.engine.getComponent(vehicle, api.type.ComponentType.TRANSPORT_VEHICLE)
	local own = tv and tv.transportVehicleConfig and tv.transportVehicleConfig.vehicles
	if own == nil then error("vehicle " .. tostring(replace.vehicle) .. " has no parts to read", 0) end
	local time = now()
	local vehicles, kept = {}, {}
	for i, r in ipairs(replace.consist) do
		local part = vehiclePart(r.part, time)
		if r.kept ~= nil then
			local old = own[r.kept + 1]
			if old == nil then error("part " .. i .. " keeps a part the vehicle does not have", 0) end
			if old.part.modelId ~= part.part.modelId then
				error("part " .. i .. " keeps a part of another model", 0)
			end
			if kept[r.kept] then error("part " .. i .. " keeps a part kept already", 0) end
			kept[r.kept] = true
			part.purchaseTime = old.purchaseTime
			part.maintenanceState = old.maintenanceState
			part.maintenanceChange = old.maintenanceChange
		end
		vehicles[i] = part
	end
	local config = vehicleConfig(vehicles, replace.groups, replace.multiple_units)
	local count = 0
	for _ in pairs(kept) do count = count + 1 end
	log("replacing vehicle " .. tostring(replace.vehicle) .. " (entity " .. tostring(vehicle) .. "): "
		.. #vehicles .. " part(s), " .. count .. " kept")
	local data, entities = send(api.cmd.makeVehicleReplaceCmd(vehicle, config))
	local candidates = {}
	for _, pair in ipairs(type(entities) == "table" and entities or {}) do
		if type(pair) == "table" then candidates[#candidates + 1] = pair[1] end
	end
	local ok, named = pcall(function() return data.vehicleEntity end)
	if ok then candidates[#candidates + 1] = named end
	candidates[#candidates + 1] = vehicle
	for _, e in ipairs(candidates) do
		if isVehicle(e) then
			if e ~= vehicle then log("vehicle " .. tostring(replace.vehicle) .. " is entity " .. e .. " now, was " .. vehicle) end
			return true, e
		end
	end
	return true, nil
end

function HANDLERS.SellVehicle(sell, ctx)
	local vehicles = {}
	for i, v in ipairs(sell.vehicles) do vehicles[i] = ownOf(ctx, "vehicles", v) end
	return run(api.cmd.makeVehicleSellCmd(vehicles))
end

-- Small before/after records at the action boundary, not a per-tick scan.
-- A new train diverged immediately after BuyVehicle/AssignLine; these show
-- whether movement already differed before the engine chose its route.
local function vehicleActionState(entity, canonical, phase, line, first)
	local ok, why = pcall(function()
		local c = api.type.ComponentType
		local v = api.engine.getComponent(entity, c.TRANSPORT_VEHICLE)
		local p = api.engine.getComponent(entity, c.MOVE_PATH)
		local d = p and p.dyn
		local pos = d and d.pathPos
		log("vehicle-action " .. phase .. " vehicle-" .. tostring(canonical)
			.. " line-" .. tostring(line) .. " first=" .. tostring(first)
			.. " time=" .. tostring(now()) .. " entity=" .. tostring(entity)
			.. " state=" .. tostring(v and v.state) .. " stop=" .. tostring(v and v.stopIndex)
			.. " edge=" .. tostring(pos and pos.edgeIndex) .. " pos=" .. tostring(pos and pos.pos)
			.. " speed=" .. tostring(d and d.speed))
	end)
	if not ok then
		log("vehicle-action " .. phase .. " vehicle-" .. tostring(canonical)
			.. " unavailable: " .. tostring(why))
	end
end

function HANDLERS.AssignLine(assign, ctx)
	if assign.line == nil then
		return false, "this version of the mod does not take vehicles off their line yet"
	end
	local line = ownOf(ctx, "lines", assign.line)
	-- No first stop: the game's choice, the next stop each can reach (-1).
	local first = assign.first_stop
	if first == nil then first = -1 end
	for _, v in ipairs(assign.vehicles) do
		local entity = ownOf(ctx, "vehicles", v)
		vehicleActionState(entity, v, "before-assign", assign.line, first)
		run(api.cmd.makeVehicleSetLineCmd(entity, line, first))
		vehicleActionState(entity, v, "after-assign", assign.line, first)
	end
	return true
end

function HANDLERS.VehicleOp(op, ctx)
	local vehicle = ownOf(ctx, "vehicles", op.vehicle)
	local change = op.change
	if type(change) == "table" and change.Stop ~= nil then
		return run(api.cmd.makeVehicleSetStoppedByUserCmd(vehicle, change.Stop == true))
	elseif type(change) == "table" and change.ToDepot then
		-- Sold on arrival, the game crashes at the depot (capture.vehicleToDepot).
		if change.ToDepot.sell == true then
			return false, "selling a vehicle when it reaches the depot (the game crashes there)"
		end
		return run(api.cmd.makeVehicleSendToDepotCmd(vehicle, false))
	elseif change == "Reverse" then
		return run(api.cmd.makeVehicleReverseCmd(vehicle))
	elseif change == "Depart" then
		return run(api.cmd.makeVehicleTryToDepartCmd(vehicle))
	elseif type(change) == "table" and change.ManualDeparture ~= nil then
		return run(api.cmd.makeVehicleSetManualDepartureCmd(vehicle, change.ManualDeparture == true))
	elseif type(change) == "table" and change.Recolor then
		return run(api.cmd.makeEntitySetColorCmd(vehicle, tint(change.Recolor)))
	end
	return false, "a vehicle change of no kind"
end

-- Renaming a vehicle, a station, a town or a construction (action::Renamed),
-- as its window does: the acting company's own vehicle, a station or
-- construction no other company owns, any town.
function HANDLERS.Rename(r, ctx)
	local what, e = r.what, nil
	if type(what) ~= "table" then return false, "renaming nothing" end
	if what.Vehicle ~= nil then
		e = ownOf(ctx, "vehicles", what.Vehicle)
	elseif what.Station ~= nil then
		e = entityOf(ctx, "groups", what.Station)
		mine(e, "station")
	elseif what.Town ~= nil then
		e = entityOf(ctx, "towns", what.Town)
	elseif what.Construction ~= nil then
		e = constructionAt(what.Construction)
		mine(e, "construction")
	else
		return false, "renaming nothing"
	end
	return run(api.cmd.makeEntitySetNameCmd(e, r.name))
end

-- The game's load modes, by the schema's names, as numbers.
local LOAD_MODES = { LoadIfAvailable = 0, FullLoadAny = 1, FullLoadAll = 2, LegacyUnloadOnly = 3 }

-- A line's waypoint (action::Waypoint) as the game's Waypoint: on a lane,
-- the edge found by its ends (the lowest entity between those nodes, as for
-- a stop), which must run node 0 to node 1 as on the originator's (else
-- the lane's index and place would name another: refused), or the
-- construction at its place; in the open, its position. Its tag as carried.
local function waypointFor(w)
	local wp = api.type.Waypoint.new()
	local at = w.at or {}
	if at.Open then
		wp.pos = api.type.Vec3f.new(at.Open.x, at.Open.y, at.Open.z)
	elseif at.Lane then
		local lane, entity = at.Lane, nil
		if lane.of.Edge then
			local ref = lane.of.Edge
			local nodes = readNodes(ref.network)
			local e = edgeBetween(nodes, ref.network, arr(ref.ends.a), arr(ref.ends.b))
			if e == nil then error("no " .. ref.network .. " edge for a waypoint", 0) end
			local a = nearest(nodes, arr(ref.ends.a), END_TOLERANCE)
			if a == nil or e.node0 ~= a.id then error("a waypoint's edge runs the other way here", 0) end
			entity = e.id
		elseif lane.of.Construction then
			entity = constructionAt(lane.of.Construction)
		else
			error("a waypoint on no network", 0)
		end
		local edgePos = api.type.EdgePos.new()
		edgePos.edgeId = api.type.EdgeId.new(entity, lane.index)
		edgePos.param = lane.param
		wp.edgePos = edgePos
	else
		error("a waypoint with no place", 0)
	end
	wp.tag = w.tag
	return wp
end

-- A LineData as the game's Line component. Each stop is at a station the
-- acting company may use (tpf3mp/companies.lua, mayUse): no company's, its
-- own, or another company's that keeps its stations open (DECISIONS.md, D22,
-- proposed). The game itself stops a line anywhere (build 40408: no owner
-- check on a line's stops); its line manager offers only the player's own
-- stations, which the GUI lifts for open ones (gui/tpf3mp/tpf3mp.script.lua).
local function lineComponent(data, ctx)
	local line = api.type.Line.new()
	local stops = {}
	for i, s in ipairs(data.stops) do
		local stop = api.type.Line.Stop.new()
		local group = entityOf(ctx, "groups", s.group)
		local usable, why = companiesModule.mayUse(ctx and ctx.roster, company(), group, api)
		if not usable then error("stop " .. i .. ": " .. why, 0) end
		stop.stationGroup = group
		stop.station = s.terminal.station
		stop.terminal = s.terminal.terminal
		local alternatives = {}
		for k, a in ipairs(s.alternatives) do
			alternatives[k] = api.type.StationTerminal.new(a.station, a.terminal)
		end
		stop.alternativeTerminals = alternatives
		stop.loadMode = LOAD_MODES[s.load_mode] or error("a load mode " .. tostring(s.load_mode), 0)
		stop.minWaitingTime = s.min_wait
		stop.maxWaitingTime = s.max_wait
		stop.maxAdditionalWaitingTime = s.max_extra_wait
		local config = api.type.Line.StopConfig.new()
		config.load = seq(s.rules.load)
		config.maxLoad = seq(s.rules.max_load)
		config.forceUnload = s.rules.force_unload == true
		config.destroyForConfigChange = s.rules.destroy_for_config_change == true
		config.destroyForRefresh = s.rules.destroy_for_refresh == true
		stop.stopConfig = config
		local waypoints = {}
		for k, w in ipairs(s.waypoints or {}) do waypoints[k] = waypointFor(w) end
		stop.waypoints = waypoints
		stops[i] = stop
	end
	line.stops = stops
	local modes = {}
	for _, m in ipairs(data.modes) do modes[m] = true end
	line.vehicleInfo.transportModes = modes
	line.customFilters = data.custom_filters == true
	line.reservationPriority = data.reservation_priority
	return line
end

function HANDLERS.CreateLine(create, ctx)
	local line = lineComponent(create.line, ctx)
	local data, entities = send(api.cmd.makeLineCreateCmd(create.name, tint(create.color),
		company(), line))
	return true, madeBy("resultEntity", data, entities)
end

function HANDLERS.EditLine(edit, ctx)
	local line = ownOf(ctx, "lines", edit.line)
	local change = edit.change
	if change == "Delete" then
		return run(api.cmd.makeLineDestroyCmd(line))
	elseif type(change) == "table" and change.Update then
		return run(api.cmd.makeLineUpdateCmd(line, lineComponent(change.Update, ctx)))
	elseif type(change) == "table" and change.Rename then
		return run(api.cmd.makeEntitySetNameCmd(line, change.Rename))
	elseif type(change) == "table" and change.Recolor then
		return run(api.cmd.makeEntitySetColorCmd(line, tint(change.Recolor)))
	end
	return false, "a line change of no kind"
end

-- What an action makes (the new vehicle, the new line): its kind in the
-- registry, which the game script binds it in after the action. Its
-- handler returns the entity, where the game said which.
apply.CREATES = { BuyVehicle = "vehicles", CreateLine = "lines" }

-- What an action changes and names by canonical id, which keeps its id
-- whatever entity it is after: the replaced vehicle. Its kind in the
-- registry and the field of the action naming it; its handler returns the
-- entity it is now, where this game could name it.
apply.KEEPS = { ReplaceVehicle = { kind = "vehicles", field = "vehicle" } }

-- A loan's terms as the loan script keeps them (loan.d.tl): the action's
-- table has the script's own field names and fractions.
local function loanTerms(terms)
	local out = {}
	for key, value in pairs(terms) do out[key] = value end
	return out
end

-- Loans go through the loan script's own events, with the parameters the
-- game's finance window sends (game_mechanics/finance/finances_loan_gui.tl):
-- here they run at once, in every game at the same update.
function HANDLERS.Loan(op, ctx)
	-- Another company's loans are the room's (tpf3mp/companies.lua): on the
	-- terms the game offers, booked to that company. Which company is the
	-- roster's to say (company 0 is the save's own player), not
	-- getPlayer()'s, which a GUI state answers with the player's company.
	local roster = ctx and ctx.roster
	local mine = roster and companiesModule.byEntity(roster, company())
	if roster and not mine then return false, "the acting company is not in the room's roster" end
	if mine and mine.id ~= 0 then
		if op.Take then return companiesModule.borrow(roster, mine.id, op.Take.offer, op.Take.next, send, api) end
		if op.Repay then return companiesModule.repay(roster, mine.id, op.Repay.loan, send, api) end
		return false, "a loan is taken or paid back"
	end
	if op.Take then
		return run(api.cmd.makeScriptingSendEventCmd("", "Loan", "Obtain",
			{ loanTerms(op.Take.next), loanTerms(op.Take.offer) }))
	elseif op.Repay then
		local param = {}
		param[2] = loanTerms(op.Repay.loan)
		return run(api.cmd.makeScriptingSendEventCmd("", "Loan", "Repay", param))
	end
	return false, "a loan is taken or paid back"
end

-- Answering a subsidy offer (tpf3mp/companies.lua, "subsidies"): the
-- subsidy script's own events, as the game's subsidy window sends them
-- (game_mechanics/subventions/subventions_gui.tl), here at once, in every
-- game at the same update. Every game checks the offer against its own
-- script's state first, alike: the first company in the room's order to
-- accept an offer takes it, and every later one is refused, naming who took
-- it. The money goes to the acting player's company. The accept carries
-- that company's player entity (`tpf3mpCompany`), which the subsidy's kind
-- keeps as its taker: from then on only the taker's transport counts
-- towards it (tpf3mp/subsidies.lua).
function HANDLERS.Subsidy(op, ctx)
	local roster = ctx and ctx.roster
	if not roster then return false, "no roster to book the subsidy to" end
	local mine = companiesModule.byEntity(roster, company())
	if not mine then return false, "the acting company is not in the room's roster" end
	local state = companiesModule.subsidyState(api)
	local function event(name, ref)
		return function()
			local param = { uid = ref.uid }
			if name == "onAccept" then param.tpf3mpCompany = mine.entity end
			send(api.cmd.makeScriptingSendEventCmd("", "Subvention", name, param))
		end
	end
	if op.Accept then
		local ok, why = companiesModule.acceptSubsidy(roster, mine.id, op.Accept, state,
			event("onAccept", op.Accept), send, api)
		if not ok then return false, why end
		log("subsidy " .. string.format("%d", op.Accept.uid) .. " (" .. tostring(op.Accept.kind) .. ") taken by "
			.. tostring(mine.name))
		return true
	elseif op.Decline then
		return companiesModule.declineSubsidy(roster, op.Decline, state, event("onDecline", op.Decline))
	end
	return false, "a subsidy is accepted or declined"
end

-- A notification's popup played its first sound: the game's Notifications
-- script's own event marks it (game_mechanics/notifications/
-- notifications.script.tl, "initialSound"), in every game, so no game
-- plays it again.
function HANDLERS.NotificationSeen(n)
	if type(n.notification) ~= "number" then error("a notification by its id", 0) end
	return run(api.cmd.makeScriptingSendEventCmd("", "Notifications", "initialSound",
		{ notificationId = n.notification }))
end

-- Prospecting goes through the company script's own event, with the
-- parameters the construction menu sends it (gui/construction/
-- construction_react_util.tl): here it runs at once, in every game at the
-- same update, so every game's company script keeps the same prospection
-- from the same game time, and months later draws the same outcome and
-- builds the same industry at the same place: it seeds its draws, and the
-- game its placement, from the game time
-- (investigation/TPF3_PROSPECTING_2026-09-30.md). The company is the
-- player's, as the menu names it; the industry types go in the order the
-- originator's menu listed them.
function HANDLERS.Prospect(p, ctx)
	local town = entityOf(ctx, "towns", p.town)
	local types = seq(p.industries)
	if #types == 0 then error("a prospection that can find no industry", 0) end
	log("prospecting for " .. tostring(p.cargo) .. " near town-" .. tostring(p.town) .. " (" .. tostring(town)
		.. "): " .. table.concat(types, ", "))
	return run(api.cmd.makeScriptingSendEventCmd("", "Companies", "spawnIndustry", {
		companyEntity = company(),
		townEntity = town,
		types = types,
		permitKey = p.permit,
		cargoType = p.cargo,
	}))
end

-- A company perk (action::PerkOp) goes through the company script's own
-- event, with the parameters the construction menu's perk tool sends it
-- (gui/construction/tools/industry_greenify_tool.script.tl,
-- marketing_campaign_tool.script.tl): here it runs at once, in every game
-- at the same update, for the acting company, which spends the permit. The
-- company script hands it on to the emissions or towns script, in this game
-- alone, as in every other.

function HANDLERS.Perk(op, ctx)
	if op.Greenify then
		local g = op.Greenify
		local con = entityOf(ctx, "industries", g.industry)
		local part = captureModule.industryPart(con)
		if part == nil then error("industry-" .. tostring(g.industry) .. " is no one industry here", 0) end
		log("greenifying industry-" .. tostring(g.industry) .. " (" .. tostring(part) .. ")")
		return run(api.cmd.makeScriptingSendEventCmd("", "Companies", "MakeGreen", {
			companyEntity = company(),
			constructionEntity = part,
			permitKey = g.permit,
		}))
	elseif op.Marketing then
		local m = op.Marketing
		local town = entityOf(ctx, "towns", m.town)
		local cost = tonumber(m.cost)
		if cost == nil or cost < 0 then error("a campaign of no price", 0) end
		-- The tool will not start one the company cannot pay for; neither
		-- does any game.
		local read, balance = pcall(function() return api.engine.util.finance.getPlayersBalance(company()) end)
		if not read or type(balance) ~= "number" or balance ~= balance or math.abs(balance) == math.huge then
			error("cannot read the company balance for the campaign", 0)
		end
		if balance < cost then
			error("not enough money for the campaign", 0)
		end
		log("marketing in town-" .. tostring(m.town) .. " (" .. tostring(town) .. ") for " .. tostring(cost))
		send(api.cmd.makeScriptingSendEventCmd("", "Companies", "startMarketingCampaign", {
			townEntity = town,
			companyEntity = company(),
			marketingParams = { durationMs = m.duration_ms, lineCostFactor = m.line_cost_factor },
			permitKey = m.permit,
		}))
		-- What the tool books once the campaign started (its command's
		-- callback): the price, to the company, as another expense.
		local entry = api.type.JournalEntry.new()
		entry.amount = -cost
		entry.time = -1
		entry.category.type = api.type.JournalEntry.Type.OTHER
		return run(api.cmd.makeJournalBookAssetCmd(company(), entry, api.type.Vec3f.new(0, 0, 0)))
	end
	return false, "a perk of no kind"
end

-- The shared date pace; zero holds the date without stopping vehicles.
function HANDLERS.CalendarSpeed(p)
	local value = p.millis_per_day
	-- Also validate direct Lua replay, before constructing an engine command.
	local capture = ug_require and ug_require("tpf3mp_1::/scripts/tpf3mp/capture.lua")
		or require("tpf3mp.capture")
	capture.calendarSpeed(nil, value)
	return run(api.cmd.makeGameSetCalendarSpeedCmd(value))
end

-- A town building's Historic Preservation (action::Preservation), as its
-- window sets it (gui/entity_window/town_building/town_building.tl): the
-- town building at that place in the construction's list. Town buildings
-- are the town's: any company may, as in single player.
function HANDLERS.Preserve(p)
	local con, c = constructionAt(p.building)
	local list = c and c.townBuildings
	local building = list and list[p.index + 1]
	if type(building) ~= "number" then
		error("no town building " .. tostring(p.index) .. " in the " .. tostring(p.building.file), 0)
	end
	log((p.preserved and "preserving " or "no longer preserving ") .. tostring(building) .. " of "
		.. tostring(con) .. " " .. tostring(p.building.file))
	return run(api.cmd.makeTownBuildingSetBlockedDevelopmentCmd(building, p.preserved == true))
end

-- The room's companies (tpf3mp/companies.lua): the acting player founds,
-- joins, renames, recolours or dissolves one, and its head locks it, sends a
-- player out or shares its stations, in `ctx.roster`; `ctx.seal` is the
-- room's seal of a password sent with it.
function HANDLERS.CompanyOp(op, ctx)
	if not (ctx and ctx.roster and ctx.player) then return false, "no roster to change" end
	local ok, why = companiesModule.run(ctx.roster, ctx.player, op, send, api, ctx.seal)
	if not ok then return false, why end
	return true
end

-- Taking a company rank (tpf3mp/progression.lua). With one company in the
-- room, the company growth script's own event, as the company window sends
-- it: the game keeps that company's rank, and checks the rank is reached.
-- With more, the acting company's rank in the mod's state, when it reached
-- it; the save's own player also takes it in the game's own state, so its
-- rank stays when the room is one company again.
function HANDLERS.ApplyRank(r, ctx)
	local level = tonumber(r.level)
	if level == nil then return false, "a rank is a number" end
	local roster = ctx and ctx.roster
	local event = function()
		return api.cmd.makeScriptingSendEventCmd("", "Companies", "applyLevel", { level = level })
	end
	if progressionModule.multi(roster) then
		local ok, why = progressionModule.take(ctx.progression, roster, company(), level)
		if not ok then return false, why end
		if company() == api.engine.util.getPlayer() then send(event()) end
		return true
	end
	if company() ~= api.engine.util.getPlayer() then
		return false, "only the room's first company has the game's own rank"
	end
	-- The game ignores a rank not reached; this says why, where it can read it.
	local game = progressionModule.game(api)
	local read, own = pcall(function() return game and game.own(company()) end)
	if read and type(own) == "table" and type(own.level) == "number" and type(own.potentialLevel) == "number" then
		if level <= own.level then return false, "the company has rank " .. own.level .. " already" end
		if level > own.potentialLevel then
			return false, "the company has reached rank " .. own.potentialLevel .. ", not " .. level
		end
	end
	return run(event())
end

-- Runs one action. `ctx` is { registry = } (tpf3mp/registry.lua), for the
-- actions that name vehicles, lines and station groups; with companies, also
-- `roster`, `player` (who sent it) and `company` (their company's player
-- entity), which the action is booked to, and `progression`, the companies'
-- ranks (tpf3mp/progression.lua). Returns true, nil
-- and the entity it made or changed (for the kinds in CREATES and KEEPS,
-- where the game said), or false and why not; never raises.
function apply.run(action, ctx)
	if type(action) ~= "table" then return false, "an action is a table" end
	local allowed, why = acceptance.check(action)
	if not allowed then return false, why end
	local kind, body = next(action)
	if kind == nil or next(action, kind) ~= nil then
		return false, "an action is a table of one entry"
	end
	local handler = HANDLERS[kind]
	if handler == nil then
		return false, "this version of the mod does not apply " .. tostring(kind) .. " yet"
	end
	acting = ctx
	local ok, applied, detail = pcall(handler, body, ctx)
	acting = nil
	if not ok then return false, tostring(applied) end
	if applied == true then return true, nil, detail end
	return false, detail
end

-- The actions whose proposal apply.proposalOf makes: the builds a player's
-- tool previews (tpf3mp/previews.lua).
apply.PREVIEWS = { BuildConstruction = true, BuildRoad = true, BuildTrack = true, PlaceStop = true }

-- The proposal `action` would build in this game, and the context it would
-- be built with, as apply.run would make them for `ctx`, without sending
-- anything, building anything or writing the log: for showing another
-- player's build preview (docs/HOOKS.md, "Build previews"). Or nil and why:
-- an action that is not a build, or one this game cannot make (a street
-- type it lacks, an edge it does not have).
function apply.proposalOf(action, ctx)
	if type(action) ~= "table" then return nil, "an action is a table" end
	local kind, body = next(action)
	if kind == nil or next(action, kind) ~= nil then return nil, "an action is a table of one entry" end
	if not apply.PREVIEWS[kind] then return nil, "no preview of " .. tostring(kind) end
	local allowed, why = acceptance.check(action)
	if not allowed then return nil, why end
	dry, acting = true, ctx
	local ok, stopped = pcall(HANDLERS[kind], body, ctx)
	dry, acting = false, nil
	if not ok and type(stopped) == "table" and stopped.proposal ~= nil then
		return stopped.proposal, stopped.context
	end
	if ok then return nil, "the build made no proposal" end
	if type(stopped) == "table" then return nil, tostring(stopped.dry) end
	return nil, tostring(stopped)
end

-- The actions this version applies, for tests and the log.
function apply.kinds()
	local kinds = {}
	for kind in pairs(HANDLERS) do kinds[#kinds + 1] = kind end
	table.sort(kinds)
	return kinds
end

return apply
