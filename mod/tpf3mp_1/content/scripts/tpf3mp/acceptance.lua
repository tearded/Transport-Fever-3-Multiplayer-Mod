-- New channels remain refused until ordinary two-game acceptance is recorded.
-- These defaults travel with the mod fingerprint: every room member has the
-- same settings. Fixtures may enable a channel explicitly to test its mechanics.
-- Height brushes passed local two-game acceptance on build 40408, 2026-10-02.
-- See investigation/STATION_TERRAIN_2026-10-02.md for evidence and limits.
-- Bridge/tunnel window rebuilds use ordinary road/track actions, gated at
-- capture.windowBuild; they passed local two-game acceptance on build
-- 40420, 2026-10-11 (track and road bridges, a road tunnel, a road bridge
-- with a stop on it): investigation/TPF3_BRIDGE_TUNNEL_WINDOW_2026-10-11.md.
-- Signals a script places along tracks (PlaceSignals: Auto Signals) passed
-- local two-game acceptance on build 40408, 2026-10-06 (runs
-- run-1006-225251 and run-1006-230635; docs/MODS.md).
-- Subsidies passed local two-game acceptance on build 40408, 2026-10-07:
-- investigation/TPF3_SUBSIDIES_2026-10-07.md.
-- Line waypoints passed local two-game acceptance on build 40420, 2026-10-16:
-- a line edited with waypoints in a room, both games' rolling world checks alike.
local acceptance = { subsidies = true, rename = true, waypoints = true, terraform = true, bridges = true, perks = false, preservation = false, signals = true }

function acceptance.check(action)
    local feature
    if action.Subsidy then feature = "subsidies" end
    if action.Terraform then feature = "terraform" end
    if action.Perk then feature = "perks" end
    -- A town building's Historic Preservation (action::Preservation).
    if action.Preserve then feature = "preservation" end
    if action.PlaceSignals then feature = "signals" end
    if action.Rename or (action.VehicleOp and type(action.VehicleOp.change) == "table"
        and action.VehicleOp.change.Recolor) then feature = "rename" end
    local line = action.CreateLine and action.CreateLine.line
    if action.EditLine and type(action.EditLine.change) == "table" then line = action.EditLine.change.Update end
    for _, stop in ipairs(line and line.stops or {}) do
        if #(stop.waypoints or {}) > 0 then feature = "waypoints" end
    end
    if feature and acceptance[feature] ~= true then
        return false, feature .. " awaits two-player game acceptance"
    end
    return true
end

return acceptance
