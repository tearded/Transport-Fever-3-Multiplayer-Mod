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
-- Signals kept on the pieces of a track cut between them (splitSignals:
-- a track started or ended there, Link.kept) passed local two-game
-- acceptance on build 40420, 2026-10-17 (run-1017-024110, Japan_2: two
-- cuts by a moved node, each signal applied in both games and moved
-- 0.00 m, every checkpoint's free-id fingerprint equal).
-- Subsidies passed local two-game acceptance on build 40408, 2026-10-07:
-- investigation/TPF3_SUBSIDIES_2026-10-07.md.
-- Line waypoints passed local two-game acceptance on build 40420, 2026-10-16:
-- a line edited with waypoints in a room, both games' rolling world checks alike.
-- Terrain paint and the asset brush (action schema 27) passed local two-game
-- acceptance on build 40420, 2026-10-16: paint strokes of several sizes, the
-- large ones in bands, and tree strokes of 78 and 124 trees, alike in both.
-- The crossing type-only path has staged Japan_2 runs, but not ordinary-start
-- two-player acceptance yet; keep it refused until that run is recorded.
local acceptance = {
    subsidies = true,
    rename = true,
    waypoints = true,
    terraform = true,
    bridges = true,
    perks = false,
    preservation = false,
    signals = true,
    splitSignals = true,
    paint = true,
    assets = true,
    crossings = false,
}

function acceptance.check(action)
    local feature
    if action.Subsidy then feature = "subsidies" end
    if action.Terraform then feature = "terraform" end
    -- Terrain paint (action::TerrainPaint, schema 27).
    if action.Paint then feature = "paint" end
    -- The asset brush (action::AssetStroke, schema 27).
    if action.PlantAssets then feature = "assets" end
    if action.Perk then feature = "perks" end
    -- A town building's Historic Preservation (action::Preservation).
    if action.Preserve then feature = "preservation" end
    if action.PlaceSignals then feature = "signals" end
    -- A track cut where a signal stands, the signal kept on the piece it
    -- stood on (action::KeptSignal).
    local polyline = (action.BuildTrack or action.BuildRoad or {}).polyline
        or (action.BuildConstruction or {}).connection
    for _, link in ipairs(polyline and polyline.links or {}) do
        if link.kept and #link.kept > 0 then feature = "splitSignals" end
    end
    -- A level crossing's type from its window (action::CrossingType).
    if action.SetCrossingType then feature = "crossings" end
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
