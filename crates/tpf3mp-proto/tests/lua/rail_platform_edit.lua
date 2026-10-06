-- Stand-in engine contract test, not a live-game reproduction.
local capture = ug_require('tpf3mp_1::/scripts/tpf3mp/capture.lua')
local apply = ug_require('tpf3mp_1::/scripts/tpf3mp/apply.lua')
local rail = '::/stations/rail/modular_station/modular_station.con'
CONSTRUCTIONS[77].fileName = rail
EDIT.toAdd[1].fileName = rail
api.type.ComponentType.PLAYER_OWNED = 15
local get = api.engine.getComponent
api.engine.getComponent = function(e, kind)
    if kind == 15 then return { player = 25 } end
    return get(e, kind)
end
local function copy(t)
    if type(t) ~= 'table' then return t end
    local out = {}
    for k, v in pairs(t) do out[k] = copy(v) end
    return out
end
local mode, requested, full
api.engine.util.proposal = {
    createProposalReplaceConstruction = function(old, parameters)
        assert(old == LOCAL_ID, 'must resolve this replica\'s local entity')
        requested = copy(parameters)
        full = copy(EDIT)
        full.toRemove = { old }
        full.toAdd[1].params = copy(parameters)
        full.nativeRailGraph = true
        if mode == 'params' then full.toAdd[1].params.modules[12].name = 'wrong.module' end
        if mode == 'move' then full.toAdd[1].transf[13] = 81 end
        if mode == 'extra' then full.toAdd[2] = copy(full.toAdd[1]) end
        if mode == 'remove' then full.toRemove[2] = 123456 end
        if mode == 'streets' then full.proposal.removedSegments[2] = { entity = 99999 } end
        if mode == 'missing' then return nil end
        return full
    end,
    makeProposalData = function(p, context)
        error('SimpleProposal expected, got Proposal (build 40408)')
    end,
    refreshConstruction = function() error('native replacement already snaps its graph') end,
}
local sent = {}
api.cmd.sendCommand = function(command, callback)
    assert(command.proposal == full, 'must send the full native graph, not a simplified rebuild')
    assert(command.playerInitiated and not command.ignoreErrors)
    if mode == 'critical' then callback({}, false); return end
    if mode == 'silent' then return end
    sent[#sent + 1] = command
    CONSTRUCTIONS[LOCAL_ID].params = copy(requested)
    CONSTRUCTIONS[LOCAL_ID].frozenEdges = {}
    if callback then callback({}, true) end
end
for _, specialization in ipairs({ 'bulk', 'liquid', 'flatbed', 'goods' }) do
    local name = '::/trainstation___/stations/rail/modular_station/platform_cargo_era_a_' .. specialization .. '.module'
    EDIT.toAdd[1].params.modules[12].name = name
    -- Capture with the originator's entity, replay with this peer's entity.
    local station = CONSTRUCTIONS[LOCAL_ID] or CONSTRUCTIONS[77]
    CONSTRUCTIONS = { [77] = station }
    station.frozenEdges = { 6000 }
    local action = assert(capture.construction(EDIT))
    CONSTRUCTIONS = { [LOCAL_ID] = station }
    local ok, why = apply.run(action, { company = 25 })
    assert(ok, tostring(why))
    assert(requested.modules[12].name == name)
    assert(station.params.modules[12].name == name)
    for _, rejected in ipairs({ 'params', 'move', 'extra', 'remove', 'streets', 'missing', 'critical' }) do
        station.frozenEdges = { 6000 }
        mode = rejected
        local before = #sent
        local accepted = apply.run(action, { company = 25 })
        assert(not accepted and #sent == before, 'must refuse ' .. rejected .. ' before a command')
    end
    mode = nil
end
assert(#sent == 4)
-- Engine states can disallow callbacks. A silently refused command must not
-- be reported as a successful edit merely because the old station exists.
CONSTRUCTIONS[LOCAL_ID].params.modules[12].name = 'unchanged.module'
CONSTRUCTIONS[LOCAL_ID].frozenEdges = { 6000 }
CONSTRUCTIONS[77] = CONSTRUCTIONS[LOCAL_ID]
local action = assert(capture.construction(EDIT))
if LOCAL_ID ~= 77 then CONSTRUCTIONS[77] = nil end
mode = 'silent'
local ok, why = apply.run(action, { company = 25 })
assert(not ok and tostring(why):find('did not apply', 1, true), tostring(why))
assert(#sent == 4)
