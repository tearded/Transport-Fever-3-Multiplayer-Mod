-- Test-only staged mechanic probe. This deliberately changes one town's
-- cargo demands when explicitly enabled; never include it in a release mod.
local PREFIX = "[tpf3mp-probe industry-fixture] "

local function read_env(key)
	local ok, value = pcall(function()
		return os.getenv(key)
	end)
	if not ok then return nil, false end
	return value, true
end

local function integer(value)
	if type(value) ~= "number" or value ~= value or value == math.huge or value == -math.huge
		or value ~= math.floor(value) then return nil end
	return value
end

local function dense_length(value)
	if type(value) ~= "table" then return nil end
	local length = #value
	local count = 0
	for key in pairs(value) do
		if type(key) ~= "number" or key ~= math.floor(key) or key < 1 or key > length then
			return nil
		end
		count = count + 1
	end
	if count ~= length then return nil end
	return length
end

local function data_log(message)
	local text = PREFIX .. message
	local ok, native = pcall(function() return tpf3mp_native end)
	if ok and type(native) == "table" and type(native.log) == "function" then
		if pcall(native.log, text) then return end
	end
	local print_fn = debugPrint or print
	if type(print_fn) == "function" then pcall(print_fn, text) end
end

local function read_world_update()
	local world = api.engine.util.getWorld()
	local game_time = api.engine.getComponent(world, api.type.ComponentType.GAME_TIME)
	if type(game_time) ~= "table" then return nil, "GAME_TIME component is unreadable" end
	local update_count = integer(game_time.updateCount)
	if update_count == nil or update_count < 0 then
		return nil, "GAME_TIME.updateCount has an unknown shape"
	end
	return update_count
end

local function sorted_town_ids()
	local component = api.type.ComponentType.TOWN
	if component == nil or type(api.engine.forEachEntityWithComponent) ~= "function" then
		return nil, "TOWN enumeration is unavailable"
	end
	local ids = {}
	local ok, err = pcall(api.engine.forEachEntityWithComponent, function(entity)
		local id = integer(entity)
		if id == nil or id < 0 then error("TOWN entity id has an unknown shape") end
		ids[#ids + 1] = id
	end, component)
	if not ok then return nil, "TOWN enumeration failed: " .. tostring(err) end
	if #ids == 0 then return nil, "no TOWN entities are available" end
	table.sort(ids)
	for index = 2, #ids do
		if ids[index] == ids[index - 1] then return nil, "TOWN enumeration returned a duplicate entity" end
	end
	return ids
end

local function copy_cargo_needs(town)
	local source = town.cargoNeeds
	if dense_length(source) ~= 3 then
		return nil, nil, "TOWN.cargoNeeds must be three dense land-use buckets"
	end
	local copied = {}
	local used = {}
	for bucket = 1, 3 do
		local entries = source[bucket]
		local count = dense_length(entries)
		if count == nil then
			return nil, nil, "TOWN.cargoNeeds bucket " .. bucket .. " has an unknown shape"
		end
		copied[bucket] = {}
		for index = 1, count do
			local entry = entries[index]
			local entry_count = dense_length(entry)
			local cargo_id = type(entry) == "table" and integer(entry[1]) or nil
			local factor = type(entry) == "table" and entry[2] or nil
			if entry_count ~= 2 or cargo_id == nil or cargo_id < 0
				or type(factor) ~= "number" or factor ~= factor
				or factor <= -math.huge or factor >= math.huge then
				return nil, nil, "TOWN.cargoNeeds entry has an unknown shape"
			end
			-- TOWN stores {CargoTypeId, factor}; the official game script
			-- passes CargoTypeId-only rows to TownUpdateCargoNeeds.
			copied[bucket][#copied[bucket] + 1] = cargo_id
			used[cargo_id] = true
		end
	end
	return copied, used
end

local function candidate_cargo(used)
	local reps = api.res
	if type(reps) ~= "table" or type(reps.getBaseConfig) ~= "function"
		or type(reps.economyRep) ~= "table" or type(reps.economyRep.find) ~= "function"
		or type(reps.economyRep.get) ~= "function"
		or type(reps.cargoTypeRep) ~= "table" or type(reps.cargoTypeRep.getAll) ~= "function"
		or type(reps.cargoTypeRep.get) ~= "function" then
		return nil, nil, "cargo resource APIs are unavailable"
	end
	local land_use = api.type["enum"] and api.type["enum"].LandUseType
	if type(land_use) ~= "table" then return nil, nil, "LandUseType enum is unavailable" end
	local economy_id = reps.getBaseConfig().economyId
	local economy_index = reps.economyRep.find(economy_id)
	local economy = economy_index ~= nil and reps.economyRep.get(economy_index) or nil
	if type(economy) ~= "table" or type(economy.cargoCategories) ~= "table" then
		return nil, nil, "base economy cargo categories are unreadable"
	end

	local all = reps.cargoTypeRep.getAll()
	if type(all) ~= "table" then return nil, nil, "cargo resource list has an unknown shape" end
	local ids = {}
	for cargo_id in pairs(all) do
		local id = integer(cargo_id)
		if id == nil or id < 0 then return nil, nil, "CargoTypeId has an unknown shape" end
		ids[#ids + 1] = id
	end
	table.sort(ids)
	for index = 2, #ids do
		if ids[index] == ids[index - 1] then return nil, nil, "cargo resource IDs are ambiguous" end
	end

	for _, cargo_id in ipairs(ids) do
		if not used[cargo_id] then
			local cargo = reps.cargoTypeRep.get(cargo_id)
			if type(cargo) ~= "table" or type(cargo.category) ~= "string" then
				return nil, nil, "cargo resource has an unknown shape"
			end
			if cargo.category ~= "" then
				local category = economy.cargoCategories[cargo.category]
				if type(category) ~= "table" or dense_length(category.landUses) == nil then
					return nil, nil, "cargo category landUses has an unknown shape"
				end
				local buckets = {}
				for _, value in ipairs(category.landUses) do
					local use = integer(value)
					if use == nil or use < 0 or use > 2 then
						return nil, nil, "cargo category contains an unknown LandUseType"
					end
					buckets[use + 1] = true
				end
				local bucket_count, selected_bucket = 0, nil
				for bucket = 1, 3 do
					if buckets[bucket] then
						bucket_count = bucket_count + 1
						selected_bucket = bucket
					end
				end
				-- A resource valid for multiple land uses has no unique bucket
				-- for this fixture; skip it rather than guess.
				if bucket_count == 1 then
					if cargo.category == "fish" then
						local stock = api.engine.util.stock
						if type(stock) ~= "table" or type(stock.isCargoTypeCurrentlyProduced) ~= "function" then
							return nil, nil, "fish availability API is unavailable"
						end
						local ok, available = pcall(stock.isCargoTypeCurrentlyProduced, cargo_id)
						if not ok or type(available) ~= "boolean" then
							return nil, nil, "fish availability could not be read"
						end
						if available then return cargo_id, selected_bucket end
					else
					return cargo_id, selected_bucket
					end
				end
			end
		end
	end
	return nil, nil, "no unambiguous missing CargoTypeId is available"
end

function data()
	local enabled_value, has_env = read_env("TPF3MP_INDUSTRY_FIXTURE")
	local enabled = has_env and enabled_value == "1"
	local target_update
	local config_error
	if enabled then
		local raw_update, update_env_ok = read_env("TPF3MP_INDUSTRY_FIXTURE_UPDATE")
		if not update_env_ok or type(raw_update) ~= "string" or not raw_update:match("^%d+$") then
			config_error = "enabled without a decimal TPF3MP_INDUSTRY_FIXTURE_UPDATE"
		else
			target_update = tonumber(raw_update)
			if target_update == nil or target_update < 1 or target_update > 4294967295
				or target_update ~= math.floor(target_update) then
				config_error = "TPF3MP_INDUSTRY_FIXTURE_UPDATE must be a positive integer"
			end
		end
	end
	local finished = false

	return {
		update = function(_params, _state, _dt)
			if not enabled or finished then return end
			if config_error then
				finished = true
				data_log("disabled: " .. config_error)
				return
			end
			local time_ok, update_count, time_error = pcall(read_world_update)
			if not time_ok or update_count == nil then
				finished = true
				data_log("disabled: " .. tostring(time_ok and time_error or update_count))
				return
			end
			if update_count < target_update then return end
			finished = true
			if update_count > target_update then
				data_log("disabled: missed requested update " .. target_update .. " (current " .. update_count .. ")")
				return
			end

			local run_ok, run_error = pcall(function()
				local town_ids, town_error = sorted_town_ids()
				if town_ids == nil then
					data_log("disabled: " .. tostring(town_error))
					return
				end
				local town_id = town_ids[1]
				local town = api.engine.getComponent(town_id, api.type.ComponentType.TOWN)
				if type(town) ~= "table" then
					data_log("disabled: selected TOWN component is unreadable")
					return
				end
				local cargo_needs, used, needs_error = copy_cargo_needs(town)
				if cargo_needs == nil then
					data_log("disabled: " .. needs_error)
					return
				end
				local cargo_id, bucket, cargo_error = candidate_cargo(used)
				if cargo_id == nil then
					data_log("disabled: " .. cargo_error)
					return
				end
				cargo_needs[bucket][#cargo_needs[bucket] + 1] = cargo_id

				local ok_command, command, event = pcall(function()
					return api.cmd.makeTownUpdateCargoNeedsCmd(town_id, cargo_needs, true),
						api.cmd.makeScriptingSendEventCmd("", "Towns", "NewCargoTypeDemand", {})
				end)
				if not ok_command or command == nil or event == nil
					or type(api.cmd.sendCommand) ~= "function" then
					data_log("disabled: official cargo-needs command could not be prepared")
					return
				end
				local sent_command, send_error = pcall(api.cmd.sendCommand, command)
				if not sent_command then
					data_log("command failed: " .. tostring(send_error))
					return
				end
				local sent_event, event_error = pcall(api.cmd.sendCommand, event)
				if not sent_event then
					data_log("cargo-needs command sent; matching Towns/NewCargoTypeDemand event failed: " .. tostring(event_error))
					return
				end
				data_log("submitted update=" .. update_count .. " town=" .. town_id
					.. " cargo=" .. cargo_id .. " bucket=" .. bucket
					.. " selector=lowest-town-id,lowest-missing-cargo-id")
			end)
			if not run_ok then data_log("disabled: " .. tostring(run_error)) end
		end,
	}
end
