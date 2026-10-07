-- tpf3mp/modbuild.lua -- the builds a script sends from the game scripts'
-- GUI state, in the room's game (docs/HOOKS.md, "Scripts' follow-up
-- builds"; D27).
--
-- Some mods build after the player builds: Parallel Tracks lays tracks
-- beside the one drawn, Parallel Roads roads. They hear the build in their
-- game script (`onPostBuildProposal`), which runs in every game, and build
-- from its GUI half (`guiUpdate`) with api.cmd.makeWorldBuildProposalCmd.
-- So every game that runs the mod sends the follow-up, each from its own
-- settings, for whichever player built (2026-10-02, in the game). The hook
-- counts each such build as a click and stops it at the apply
-- (crates/tpf3mp-hook/src/builds.rs), so none of them built anything, in
-- any game.
--
-- This module wraps makeWorldBuildProposalCmd in that state. In the room's
-- game:
--
-- - while this state applies the room's own builds (`applying`), it is left
--   alone (only in tests: the game applies them in its engine states);
-- - every build made through it is marked playerInitiated, whatever the
--   script asked: the hook counts it and stops it at the apply. A script
--   that asked for false would otherwise build in this game alone;
-- - the build goes to the room only when the last build this game applied
--   was its own player's, and that a few frames ago at most (`follows`):
--   the follow-up of this player's build. The capture makes it the action
--   a tool's build makes (tpf3mp/engine.lua captureBuild), kept for the
--   click its command will count (`keep`); the mod's game script then
--   runs its guiUpdate as the tool's click would. Another player's build
--   leaves it to that player's game, which sends its own;
-- - a build that rebuilds existing tracks in place with signals added or
--   removed (Auto Signals, after its player's signal) goes as the signals
--   it places (`signals`: tpf3mp/engine.lua placeSignals, a PlaceSignals
--   action);
-- - any other build with constructions, removals, stops or signals is not
--   carried yet, and is stopped with why.
--
-- Two players whose builds apply within one window may both hand a mod's
-- follow-up: the room then orders both, and every game applies both alike
-- (a duplicate, or a collision the game refuses). Never a world of one game
-- alone.
--
-- Pure Lua; the tests hand install() a fake api.cmd.

local modbuild = {}

-- How many of the GUI's frames after this game applied its player's build a
-- script's build still counts as its follow-up. A mod's job runs in its
-- first guiUpdate after the apply.
modbuild.FOLLOW_FRAMES = 120

-- The note the simulation's state leaves for the GUI's: "<n> mine" or
-- "<n> other", n counting the builds applied (tpf3mp_native.note).
modbuild.NOTE = "tpf3mp.lastbuild"

-- The actions that build, after which a mod may follow up.
modbuild.BUILDS = {
	BuildRoad = true, BuildTrack = true, Bulldoze = true, BuildConstruction = true,
	PlaceStop = true, EditJunctions = true, PlaceSignals = true,
}

-- The api.cmd tables already wrapped.
local wrappedCmds = setmetatable({}, { __mode = "k" })

local function get(value, key)
	local ok, v = pcall(function() return value[key] end)
	if ok then return v end
	return nil
end

-- The game's vector (or a table) as a Lua array; nil if it cannot be read.
local function list(v)
	if v == nil then return {} end
	local ok, n = pcall(function() return #v end)
	if not ok or type(n) ~= "number" then return nil end
	local out = {}
	for i = 1, n do out[i] = v[i] end
	return out
end

-- The note after the builds of one update applied: `previous` is the note
-- as it was, `mine` whether the last of them was this player's own.
function modbuild.noted(previous, mine)
	local n = tonumber(type(previous) == "string" and previous:match("^(%d+) ") or nil) or 0
	return tostring(n + 1) .. (mine and " mine" or " other")
end

-- Whose build the note says this game applied last: "mine", "other" or nil,
-- and its count.
function modbuild.read(note)
	if type(note) ~= "string" then return nil end
	local n, who = note:match("^(%d+) (%a+)$")
	if who ~= "mine" and who ~= "other" then return nil end
	return who, tonumber(n)
end

-- Tracks the note frame by frame, for one GUI state: `seen(note)` once a
-- frame; `follows(note)` says "mine", "other" or nil (none, or too long
-- ago), taking a note newer than the frame's first (a mod's guiUpdate may
-- run before this mod's in the frame the build applied).
function modbuild.tracker()
	local frame, count, who, at = 0, nil, nil, nil
	local t = {}
	local function take(note)
		local w, n = modbuild.read(note)
		if n ~= nil and n ~= count then
			count, who, at = n, w, frame
		end
	end
	function t.seen(note)
		frame = frame + 1
		take(note)
	end
	function t.follows(note)
		take(note)
		if who == nil or frame - at > modbuild.FOLLOW_FRAMES then return nil end
		return who
	end
	return t
end

-- A script's SimpleProposal in the shape the build tools hand game scripts
-- (`builder.proposalCreate`, what tpf3mp/engine.lua fromProposal reads):
-- the nodes and edges it adds. Nil and why for one the room cannot carry
-- from a script yet; false for one of nothing.
function modbuild.shape(simple)
	local street = get(simple, "streetProposal")
	if street == nil then return nil, "a script's build with no street part" end
	for _, name in ipairs({ "constructionsToAdd", "constructionsToRemove" }) do
		local l = list(get(simple, name))
		if l == nil then return nil, "a script's build it cannot read" end
		if #l > 0 then return nil, "a script's build with constructions" end
	end
	for _, name in ipairs({ "edgesToRemove", "nodesToRemove" }) do
		local l = list(get(street, name))
		if l == nil then return nil, "a script's build it cannot read" end
		if #l > 0 then return nil, "a script's build that removes edges" end
	end
	for _, name in ipairs({ "edgeObjectsToAdd", "edgeObjectsToRemove" }) do
		local l = list(get(street, name))
		if l == nil then return nil, "a script's build it cannot read" end
		if #l > 0 then return nil, "a script's build with a stop or signal" end
	end
	local nodes, edges = list(get(street, "nodesToAdd")), list(get(street, "edgesToAdd"))
	if nodes == nil or edges == nil then return nil, "a script's build it cannot read" end
	if #edges == 0 then return false end
	return {
		toAdd = {}, toRemove = {},
		proposal = { addedNodes = nodes, addedSegments = edges, removedSegments = {}, removedNodes = {},
			edgeObjectsToAdd = {} },
	}
end

-- Whether a script's SimpleProposal rebuilds existing edges with edge
-- objects added or removed: the shape of signals placed along a track
-- (Auto Signals), which its own capture reads (install()'s `signals`).
function modbuild.isSignals(simple)
	local street = get(simple, "streetProposal")
	if street == nil then return false end
	local removes = list(get(street, "edgesToRemove"))
	local adds, drops = list(get(street, "edgeObjectsToAdd")), list(get(street, "edgeObjectsToRemove"))
	if removes == nil or adds == nil or drops == nil then return false end
	return #removes > 0 and (#adds > 0 or #drops > 0)
end

-- The network of a shaped build: its first edge's (SegmentAndEntity.type:
-- 0 street, 1 track).
function modbuild.network(shaped)
	local first = shaped.proposal.addedSegments[1]
	local t = get(first, "type")
	if t == 1 then return "Track" end
	if t == 0 then return "Street" end
	return nil
end

-- What becomes of a script's build `proposal`: { action = } to hand the room
-- at its click, or { why = } to stop it with. `env` as install()'s.
function modbuild.judge(proposal, env, from)
	local who = env.follows()
	if who ~= "mine" then
		if who == "other" then
			return { why = "a script's follow-up of another player's build: that player's game hands it to the room" }
		end
		return { why = "a script's build with no build of this player's just before it" }
	end
	local suffix = from and (" from " .. from) or ""
	if env.signals and modbuild.isSignals(proposal) then
		local ok, action, whyNot = pcall(env.signals, proposal)
		if not ok then action, whyNot = nil, tostring(action) end
		if action == false then return { why = "a script's build of nothing" } end
		if not action then return { why = tostring(whyNot) } end
		return { action = action, shape = "a script's signals" .. suffix }
	end
	local shaped, why = modbuild.shape(proposal)
	if shaped == false then return { why = "a script's build of nothing" } end
	if not shaped then return { why = why } end
	local network = modbuild.network(shaped)
	if network == nil then return { why = "a script's build of an edge neither street nor track" } end
	local ok, action, whyNot = pcall(env.capture, shaped, network)
	if not ok then action, whyNot = nil, tostring(action) end
	if action == false then return { why = "a script's build of nothing" } end
	if not action then return { why = tostring(whyNot) } end
	return { action = action, shape = "a script's follow-up build" .. suffix }
end

-- Wraps cmd.makeWorldBuildProposalCmd. `env`:
--   inRoom()          -> whether this is the room's game;
--   applying()        -> whether this state is applying the room's builds;
--   follows()         -> "mine", "other" or nil (modbuild.tracker);
--   clicks()          -> the player's builds counted so far, or nil where the
--                        hook does not stop them;
--   keep(count, seen) -> keeps judge()'s answer for the click `count`;
--   capture(shaped, network) -> the action, false, or nil and why
--                        (tpf3mp/engine.lua captureBuild);
--   signals(proposal) -> optional: the action of signals placed along
--                        tracks (modbuild.isSignals), false, or nil and why
--                        (tpf3mp/engine.lua placeSignals);
--   callers()         -> optional: the mods on the stack (guard.callers);
--   log(line)         -> a line for the hook's log.
-- Returns true, or nil and why.
function modbuild.install(cmd, env)
	if type(cmd) ~= "table" then return nil, "api.cmd is not a table" end
	if wrappedCmds[cmd] then return true end
	local factory = cmd.makeWorldBuildProposalCmd
	if factory == nil then return nil, "api.cmd has no makeWorldBuildProposalCmd" end
	local told = {}
	cmd.makeWorldBuildProposalCmd = function(proposal, context, ignoreErrors, _playerInitiated, ...)
		if not env.inRoom() or env.applying() then
			return factory(proposal, context, ignoreErrors, _playerInitiated, ...)
		end
		local count = env.clicks()
		if count == nil then
			-- The hook does not stop builds here: one sent would build in
			-- this game alone.
			error("Not in multiplayer yet: building from a script", 0)
		end
		local mods = env.callers and env.callers() or {}
		local seen = modbuild.judge(proposal, env, mods[1])
		env.keep(count, seen)
		local key = seen.action and "handed" or seen.why
		if not told[key] then
			told[key] = true
			env.log((seen.action and "a script's follow-up build goes to the room" or ("a script's build is stopped: " .. seen.why))
				.. (mods[1] and (", from the mod " .. mods[1]) or ""))
		end
		return factory(proposal, context, ignoreErrors, true, ...)
	end
	wrappedCmds[cmd] = true
	return true
end

return modbuild
