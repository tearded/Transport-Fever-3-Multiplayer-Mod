-- tools/probe/tf3/tpf3mp_detprobe_1: the determinism probe for Transport
-- Fever 3 (docs/DAY_ONE.md section 4). Read only.
--
-- TF3's mods run code per frame, from a game bar plugin's react.onStep, not
-- per simulation step as TPF2's game scripts did. Frames and steps do not
-- line up between two games, so this probe labels each sample by the
-- simulation step it saw. The release API documents the step count itself,
-- GameTime.updateCount (simulation updates, stopped while paused); where it
-- is there, the probe samples when it lands on a multiple of STRIDE and
-- writes step=<updateCount>, with stepTime=updateCount in its header.
-- Otherwise it takes the step from the game time: it learns the time one
-- step advances (the smallest change it sees over its first frames), then
-- samples only when the game time lands exactly on a multiple of STRIDE
-- steps, and writes step=<steps since time 0>. Two games from one save then
-- sample the same steps whenever their frames see them (at 1x to 4x every
-- step is seen; a skipped one is logged as skipped), and
-- tools/probe/compare_runs.py compares the steps both logged.
--
-- Lanes, as the TPF2 probe: v vehicle count, p vehicle positions (1 m),
-- e edge geometry (0.1 m), c constructions, t town building counts, m money
-- per player, n people. Each is read through api.engine first, as mods for
-- build 40391 do, then TPF2's game.interface; a lane it cannot read is
-- "err", never a guess. Output goes to $TPF3MP_PROBE_DIR (or
-- %LOCALAPPDATA%/tpf3mp/probe) where the state can write files, else to the
-- game's log as "[tpf3mp-probe det] ..." lines for
-- tools/dayone/dayone.py collect.
function data()
  local STRIDE = 100      -- steps between samples
  local CALIBRATE = 30    -- game-time changes watched to learn one step

  local function global(name)
    local ok, v = pcall(function()
      local env = _G
      if type(env) == "table" then return env[name] end
      return nil
    end)
    if ok then return v end
    return nil
  end

  -- A pure-Lua hash, as tools/probe/determinism_probe's stablehash.lua:
  -- identical on every Lua 5.1+ and every platform.
  local M1, A1 = 2147483647, 48271
  local M2, A2 = 2147483629, 40692
  local function hashStr(s)
    local h1, h2 = 2166136261 % M1, 2166136261 % M2
    for i = 1, #s do
      local b = string.byte(s, i)
      h1 = (h1 * A1 + b) % M1
      h2 = (h2 * A2 + b) % M2
    end
    return string.format("%010d-%010d", h1, h2)
  end
  local function hashList(list) return hashStr(table.concat(list, "\30")) end

  local function q1(v) return math.floor((v or 0) + 0.5) end
  local function q01(v) return math.floor((v or 0) * 10 + 0.5) / 10 end

  local function api() return global("api") end
  local function ct()
    local ok, t = pcall(function() return api().type.ComponentType end)
    return (ok and t) or {}
  end
  local function getComp(id, comp)
    if id == nil or comp == nil then return nil end
    local ok, c = pcall(function() return api().engine.getComponent(id, comp) end)
    return ok and c or nil
  end

  -- The entities with a component: api.engine.getEntitiesWithComponent,
  -- else forEachEntityWithComponent, else game.interface.getEntities.
  local function entitiesWith(comp, kind)
    local out = {}
    if comp ~= nil then
      local ok, list = pcall(function() return api().engine.getEntitiesWithComponent(comp) end)
      if ok and type(list) == "table" then
        for _, e in pairs(list) do out[#out + 1] = tonumber(e) or e end
        return out
      end
      ok = pcall(function()
        api().engine.forEachEntityWithComponent(function(e) out[#out + 1] = tonumber(e) or e end, comp)
      end)
      if ok and #out > 0 then return out end
    end
    local ok, t = pcall(function()
      return global("game").interface.getEntities({ radius = 1e9 }, { type = kind, includeData = false })
    end)
    if ok and type(t) == "table" then
      out = {}
      for _, e in pairs(t) do out[#out + 1] = tonumber(e) or e end
      return out
    end
    return nil
  end

  local function sortedIds(list)
    table.sort(list, function(a, b) return tostring(a) < tostring(b) end)
    return list
  end

  -- A vehicle's position: TF3's documented api.engine.util.transport
  -- .getPosition first, then TPF2's game.interface.getEntity.
  local function position(id)
    local p = nil
    pcall(function() p = api().engine.util.transport.getPosition(id) end)
    if p then return p end
    pcall(function() p = global("game").interface.getEntity(id).position end)
    return p
  end

  local function laneVehicles()
    local list = entitiesWith(ct().TRANSPORT_VEHICLE, "VEHICLE")
    if not list then return nil, nil end
    local pos = {}
    for _, id in ipairs(sortedIds(list)) do
      local p = position(id)
      if type(p) == "table" then
        pos[#pos + 1] = string.format("%d,%d,%d",
          q1(p.x or p[1]), q1(p.y or p[2]), q1(p.z or p[3]))
      else
        pos[#pos + 1] = "nopos:" .. tostring(id)
      end
    end
    table.sort(pos)
    return tostring(#list), hashList(pos)
  end

  local function nodePos(nid, cache)
    local s = cache[nid]
    if s then return s end
    local c = getComp(nid, ct().BASE_NODE)
    if c and c.position then
      local p = c.position
      s = string.format("%s,%s,%s", q01(p.x or p[1]), q01(p.y or p[2]), q01(p.z or p[3] or 0))
    else
      s = "?"
    end
    cache[nid] = s
    return s
  end

  local function vecKey(p)
    return string.format("%s,%s,%s", q01(p.x or p[1]), q01(p.y or p[2]), q01(p.z or p[3] or 0))
  end

  -- Edge geometry: TF3's BASE_EDGE carries position0 and position1
  -- (documented, api/tealdef/api/engine.d.tl); TPF2's needs its nodes. Each
  -- edge is read on its own, so one unreadable edge is counted ("!N"), not
  -- the whole lane lost.
  local function laneEdges()
    local list = entitiesWith(ct().BASE_EDGE, "BASE_EDGE")
    if not list then return nil end
    local cache, geo, bad = {}, {}, 0
    for _, eid in ipairs(list) do
      local ok, key = pcall(function()
        local c = getComp(eid, ct().BASE_EDGE)
        if not c then return nil end
        local a, b
        if c.position0 ~= nil and c.position1 ~= nil then
          a, b = vecKey(c.position0), vecKey(c.position1)
        else
          a, b = nodePos(c.node0, cache), nodePos(c.node1, cache)
        end
        if a > b then a, b = b, a end
        return a .. ">" .. b
      end)
      if ok and key then geo[#geo + 1] = key else bad = bad + 1 end
    end
    table.sort(geo)
    return #geo .. ":" .. hashList(geo) .. (bad > 0 and ("!" .. bad) or "")
  end

  local function laneConstructions()
    local list = entitiesWith(ct().CONSTRUCTION, "CONSTRUCTION")
    if not list then return nil end
    local cons = {}
    for _, cid in ipairs(list) do
      local co = getComp(cid, ct().CONSTRUCTION)
      local fn = co and co.fileName and tostring(co.fileName) or "?"
      local x, y = 0, 0
      if co and co.transf then x, y = co.transf[13] or 0, co.transf[14] or 0 end
      cons[#cons + 1] = string.format("%s@%s,%s", fn, q01(x), q01(y))
    end
    table.sort(cons)
    return hashList(cons)
  end

  local function laneTowns()
    local towns = entitiesWith(ct().TOWN, "TOWN")
    if not towns or #towns == 0 then return nil end
    local map = nil
    pcall(function() map = api().engine.system.townBuildingSystem.getTown2BuildingMap() end)
    local rows = {}
    for _, tid in ipairs(towns) do
      local count = "?"
      if type(map) == "table" and type(map[tid]) == "table" then
        count = 0
        for _ in pairs(map[tid]) do count = count + 1 end
      end
      rows[#rows + 1] = string.format("%s:%s", tostring(tid), tostring(count))
    end
    table.sort(rows)
    return hashList(rows)
  end

  -- Money per player: TF3 keeps it in each player's ACCOUNT component, an
  -- integer (documented); TPF2's finance.getPlayersBalance and
  -- game.interface are the fallbacks.
  local function laneMoney()
    local rows = {}
    local ok, balances = pcall(function() return api().engine.util.finance.getPlayersBalance() end)
    if ok and type(balances) == "table" then
      for pid, bal in pairs(balances) do
        rows[#rows + 1] = string.format("%s:%s", tostring(pid), tostring(math.floor((tonumber(bal) or 0) + 0.5)))
      end
    else
      local players = entitiesWith(ct().PLAYER, "PLAYER") or {}
      if #players == 0 then
        pcall(function() players = { api().engine.util.getPlayer() } end)
      end
      if #players == 0 then return nil end
      for _, pid in ipairs(players) do
        local bal = nil
        pcall(function() bal = getComp(pid, ct().ACCOUNT).balance end)
        if bal == nil then
          pcall(function() bal = global("game").interface.getEntity(pid).balance end)
        end
        rows[#rows + 1] = string.format("%s:%s", tostring(pid), bal ~= nil and string.format("%d", bal) or "?")
      end
    end
    table.sort(rows)
    return hashList(rows)
  end

  local function lanePeople()
    local ok, n = pcall(function() return api().engine.system.simPersonSystem.getCount() end)
    if ok and type(n) == "number" then return tostring(math.floor(n)) end
    local list = entitiesWith(ct().SIM_PERSON, "SIM_PERSON")
    if not list then return nil end
    return tostring(#list)
  end

  -- The game time: the world's GAME_TIME component, as mods for build 40391
  -- read it, else TPF2's game.interface.getGameTime().
  local function gameTime()
    local t = nil
    pcall(function()
      local world = api().engine.util.getWorld()
      t = api().engine.getComponent(world, ct().GAME_TIME).gameTime
    end)
    if type(t) ~= "number" then
      pcall(function() t = global("game").interface.getGameTime().time end)
    end
    if type(t) == "number" then return t end
    return nil
  end

  -- The simulation's own update count, where the API has it.
  local function updateCount()
    local n = nil
    pcall(function()
      local world = api().engine.util.getWorld()
      n = api().engine.getComponent(world, ct().GAME_TIME).updateCount
    end)
    if type(n) == "number" and n == math.floor(n) then return n end
    return nil
  end

  -- Output: a file where the state can write one, else the game's log.
  local function getenv(key)
    local ok, v = pcall(function() return os.getenv(key) end)
    if ok and type(v) == "string" and #v > 0 then return v end
    return nil
  end
  local DIR = getenv("TPF3MP_PROBE_DIR")
    or (getenv("LOCALAPPDATA") and (getenv("LOCALAPPDATA") .. "/tpf3mp/probe"))
    or (getenv("HOME") and (getenv("HOME") .. "/.local/share/tpf3mp/probe"))
  local function emit(text)
    if DIR then
      local ok, f = pcall(function() return io.open(DIR .. "/determinism_probe.log", "a") end)
      if ok and f then
        f:write(text .. "\n")
        f:close()
        return
      end
    end
    local ok = pcall(function() debugPrint("[tpf3mp-probe det] " .. text) end)
    if not ok then pcall(function() print("[tpf3mp-probe det] " .. text) end) end
  end

  local function sample(step, t)
    local vcount, vpos = nil, nil
    pcall(function() vcount, vpos = laneVehicles() end)
    local function dig(fn) local ok, v = pcall(fn); return (ok and v) or "err" end
    emit(string.format("step=%d time=%s v=%s p=%s e=%s c=%s t=%s m=%s n=%s",
      step, string.format("%.6f", t), vcount or "err", vpos or "err",
      dig(laneEdges), dig(laneConstructions), dig(laneTowns), dig(laneMoney), dig(lanePeople)))
  end

  -- The sampler: learns one step's time, then samples exact multiples of
  -- STRIDE steps.
  local state = { last = nil, changes = 0, step = nil, nextStep = nil, header = false }
  local function onFrame()
    local n = updateCount()
    if n ~= nil then
      if not state.header then
        state.header = true
        emit(string.format("# determinism_probe (tf3) stride=%d stepTime=updateCount lanes=v,p,e,c,t,m,n", STRIDE))
        state.nextStep = (math.floor(n / STRIDE) + 1) * STRIDE
      end
      if n >= state.nextStep then
        if n == state.nextStep then
          sample(n, gameTime() or -1)
        else
          emit(string.format("# skipped step=%d (this frame saw step %d)", state.nextStep, n))
        end
        state.nextStep = (math.floor(n / STRIDE) + 1) * STRIDE
      end
      return
    end
    local t = gameTime()
    if t == nil then return end
    if state.last == nil then state.last = t return end
    if t == state.last then return end
    local delta = t - state.last
    state.last = t
    if delta <= 0 then return end
    if state.changes < CALIBRATE then
      state.changes = state.changes + 1
      if state.step == nil or delta < state.step then state.step = delta end
      return
    end
    local stepSize = state.step
    local steps = math.floor(t / stepSize + 0.5)
    if not state.header then
      state.header = true
      emit(string.format("# determinism_probe (tf3) stride=%d stepTime=%.6f lanes=v,p,e,c,t,m,n", STRIDE, stepSize))
      state.nextStep = (math.floor(steps / STRIDE) + 1) * STRIDE
    end
    if steps >= state.nextStep then
      if steps == state.nextStep and math.abs(t - steps * stepSize) < stepSize / 4 then
        sample(steps, t)
      else
        emit(string.format("# skipped step=%d (this frame saw step %d)", state.nextStep, steps))
      end
      state.nextStep = (math.floor(steps / STRIDE) + 1) * STRIDE
    end
  end

  local react = ug_require "::/gui/main/react.lua"
  local builtin = ug_require "::/gui/main/builtin.lua"
  local game_bar_widgets = ug_require "::/gui/game_bar/game_bar_widgets.tl"

  local Tpf3mpDetProbe = react.RegisterPluginRecipe(game_bar_widgets.GameBarInfoDisplayExtension, "Tpf3mpDetProbe", function()
    react.onStep(function()
      local ok, err = pcall(onFrame)
      if not ok then emit("# frame failed: " .. tostring(err)) end
    end)
    return builtin.BoxLayout{ orientation = builtin.type.Orientation.Horizontal, children = {} }
  end)

  return { Tpf3mpDetProbe = Tpf3mpDetProbe }
end
