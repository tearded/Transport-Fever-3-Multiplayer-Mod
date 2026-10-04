-- A stand-in for Transport Fever 3's main menu, as much of it as the mod's
-- Multiplayer window (mod/tpf3mp_1/content/gui/menu/lobby.lua) uses:
-- ug_require with react, builtin, gui_react_util and button_react_util,
-- api.gui.StyleSheet, the loader's resolveutil.loadfile the hook answers,
-- _ and debugPrint. Run by the tests in crates/tpf3mp-hook/src/lobby.rs,
-- which define LOBBY_SOURCE (the window's file) and set STATE (the hook's
-- answer to a state request, a Lua table literal).
--
-- It holds the window to the rules the game enforces, which it learnt in
-- the game on 2026-09-30:
-- - a recipe returns a layout: the game refused a recipe returning a bare
--   TextView among a layout's children ("Recipe child must be a layout");
-- - a list of children has no holes: a nil in it cuts the Lua list short,
--   and whatever came after is silently not drawn;
-- - a size is -1 (left to the content) or more than 0: the game drew
--   columns sized {w, 0} as thin bars, with everything in them hidden;
-- - a text field or a combo box has a size, its own or its box's.
--
-- Defines LOG (debugPrint lines), SENT (the JSON of every action the
-- window sent), REPLY (what the hook answers an action, "ok" unless set),
-- and render(focus), tick(), texts(), find(text), click(text),
-- choose(caption, value), type_into(placeholder, text) and enabled(text).

LOG = {}
SENT = {}
REPLY = "ok"
STATE = nil

function debugPrint(line) LOG[#LOG + 1] = tostring(line) end
function _(text) return text end

api = {
	gui = { StyleSheet = { new = function() return {} end } },
	type = {
		Vec2f = { new = function(x, y) return { x = x, y = y } end },
		Vec4f = { new = function(a, b, c, d) return { a, b, c, d } end },
		Vec2i = { new = function(x, y) return { x = x, y = y } end },
		SavegameId = { new = function() return {} end },
		-- A save's metadata.date, read as the game's Load Game page does.
		Date = { new = function(date) return { year = date } end },
	},
}

-- The game's saves and climates, as the menu's app gives them. SAVES maps
-- a save's name to its climate resource and year; READS counts the saves
-- read.
SAVES = {}
READS = 0
app = {
	SaveGameNamespace = { getSavegame = function() return "savegame" end },
	findAllSavegames = function(namespace)
		local found = {}
		for name in pairs(SAVES) do found[#found + 1] = { saveName = name, path = "saves/" .. name .. ".sav" } end
		return found
	end,
	getSavegameInfo = function(id)
		READS = READS + 1
		local save = assert(SAVES[id.saveGameName], "no save " .. tostring(id.saveGameName))
		return {
			isCompleted = function() return true end,
			get = function()
				return { info = {
					configDict = { { "climate", save.climate }, { "seed", "1" } },
					metadata = { date = save.year, startYear = save.year },
				} }
			end,
		}
	end,
	-- The menu's own load, which a pick of the room's save and mods takes
	-- over (roommods.lua): LOADS counts the loads that went through.
	setWaitForStartReadyGame = function() WAITS = (WAITS or 0) + 1 end,
	loadGame = function() LOADS = (LOADS or 0) + 1 end,
	-- The installed mods, as the game's ModRep tells them: INSTALLED maps a
	-- mod's id to { name, source, hub } (hub its Mod Hub number, as text).
	getUserProfile = function()
		return { getModRep = function()
			return {
				getInstalledMods = function()
					local out = {}
					for id in pairs(INSTALLED) do out[#out + 1] = { name = id } end
					table.sort(out, function(a, b) return a.name < b.name end)
					return out
				end,
				exists = function(_self, id) return INSTALLED[id.name] ~= nil end,
				getGameModDesc = function(_self, id) return { name = (INSTALLED[id.name] or {}).name or "" } end,
				getModSource = function(_self, id) return (INSTALLED[id.name] or {}).source or "" end,
			}
		end }
	end,
	res = { climateRep = {
		find = function(res) return res == "::/climates/dry/dry.clima" and 3 or -1 end,
		get = function(id) return { desc = { name = "Dry", icon = "::/climates/dry/icon.tga" } } end,
	} },
}

INSTALLED = {}

-- Mod Hub, as api.modhub gives it: HUB.mods maps a number (as text) to what
-- Mod Hub tells of it; HUB.state to its install state's name; subscribing
-- records the number in HUB.subscribed. Lookups and subscriptions answer
-- at once, as if Mod Hub were quick.
HUB = { backend = 1, signedIn = true, mods = {}, state = {}, subscribed = {} }
api.type.modhub = {
	ModId = { new = function() return { isValid = function(self) return self.value ~= nil end } end },
	GetModDetailsRequest = { new = function(id) return { id = id } end },
	GetModMediaRequest = { new = function(id, kind, size) return { id = id, kind = kind } end },
	ModMediaType = { Logo = "Logo" },
	SubscribeModRequest = { new = function(id) return { id = id } end },
	InstallState = { None = 0, Downloading = 1, UpdatePending = 2, Installed = 3, DownloadPending = 4,
		InstallationPending = 5, UninstallPending = 6, Extracting = 7, InsufficientSpace = 8, MiscError = 9 },
}
local function result(ok, data, message)
	return {
		isSuccess = function() return ok end,
		getData = function() return data end,
		getError = function() return { message = message } end,
	}
end
api.type.ModId = { new = function() return {} end }
api.modhub = {
	getBackendIdForSource = function(source) return source == "mod.io" and HUB.backend or -1 end,
	isInitialized = function() return true end,
	getCapabilities = function() return { isInfoOnly = false } end,
	getUserInfo = function() return HUB.signedIn and { userName = "max" } or nil end,
	getModDetailsAsync = function(_b, request, done)
		local mod = HUB.mods[request.id.value]
		done(result(true, { found = mod ~= nil, modInfo = mod or {}, author = mod and mod.author or "" }))
	end,
	subscribeModAsync = function(_b, request, done)
		HUB.subscribed[#HUB.subscribed + 1] = request.id.value
		HUB.state[request.id.value] = HUB.state[request.id.value] or "DownloadPending"
		done(result(true, {}))
	end,
	getModSubscriptionState = function(_b, id)
		for _i, n in ipairs(HUB.subscribed) do if n == id.value then return true end end
		return false
	end,
	getModInstallState = function(_b, id)
		return api.type.modhub.InstallState[HUB.state[id.value] or "None"]
	end,
	getModInfoForInstalledMod = function(_b, id) return { logoImage = id.value and "logo.png" or "" } end,
	getModHubModIdForModId = function(modId)
		local id = api.type.modhub.ModId.new()
		id.value = (INSTALLED[modId.name] or {}).hub
		return id
	end,
}

-- The hook's answers, as crates/tpf3mp-hook/src/menu_entry.rs gives them.
resolveutil = {}
function resolveutil.loadfile(path)
	if path == "tpf3mp_1::/tpf3mp/state.lua" then
		return STATE
	elseif path == "tpf3mp_1::/tpf3mp/act.lua" then
		SENT[#SENT + 1] = resolveutil.__tpf3mp_action
		return REPLY
	end
	error("no such file: " .. tostring(path))
end

local mount = { refs = {}, index = 0, timers = {} }

local react = {}
function react.useRef(initial)
	mount.index = mount.index + 1
	local ref = mount.refs[mount.index]
	if not ref then
		ref = { value = initial }
		function ref:get() return self.value end
		function ref:set(v) self.value = v end
		function ref:old() return self.value end
		mount.refs[mount.index] = ref
	end
	return ref
end
STATE_WRITES = 0
function react.useState(initial)
	local ref = react.useRef(initial)
	if not ref.stateTracked then
		ref.stateTracked = true
		local set = ref.set
		function ref:set(value)
			STATE_WRITES = STATE_WRITES + 1
			set(self, value)
		end
		-- As the game's: what was set shows from the next draw on, not
		-- before (two sets before a draw: the last one holds).
		ref.drawn = initial
		function ref:old() return self.drawn end
	end
	return ref
end
function react.onStepTimer(fn) mount.timers[#mount.timers + 1] = fn end
local LAYOUTS = { BoxLayout = true, FloatingLayout = true }
-- A wrapper recipe's widget takes meta for its class only: the game
-- asserted and closed on a window whose meta had a styleSheet ("Wrapper
-- recipe must return child", 2026-09-30).
function react.RegisterWrapperRecipe(name, wrapped, fn)
	return setmetatable({ name = name }, { __call = function(_, params)
		local node = fn(params)
		assert(type(node) == "table" and node.view == wrapped.viewName,
			"Wrapper recipe must return child: " .. name)
		for key in pairs(node.params.meta or {}) do
			assert(key == "class", "a wrapper recipe's meta may hold its class only, not " .. key .. ": " .. name)
		end
		return node
	end })
end
function react.RegisterRecipe(name, fn)
	return setmetatable({ name = name }, { __call = function(_, params)
		local node = fn(params)
		assert(type(node) == "table" and LAYOUTS[node.view],
			"Recipe child must be a layout: " .. name .. " returned " .. tostring(node and node.view))
		return node
	end })
end

local builtin = { type = {
	Orientation = { Horizontal = "Horizontal", Vertical = "Vertical" },
	ScrollBarPolicy = { AlwaysOff = "AlwaysOff", AsNeeded = "AsNeeded" },
	ImageViewScaling = { AutoFit = "AutoFit", AutoZoom = "AutoZoom" },
	TabOrientation = { North = "North", South = "South" },
} }
-- A list of children with no holes, every one a node.
local function whole(list, what)
	if list == nil then return end
	local most = table.maxn(list)
	assert(most == #list, what .. ": a nil among the children cuts the list at " .. #list .. " of " .. most)
	for i = 1, most do
		assert(type(list[i]) == "table", what .. ": child " .. i .. " is " .. type(list[i]))
	end
end

local function sized(params, what)
	local sheet = params and params.meta and params.meta.styleSheet
	local size = sheet and sheet.size
	if size then
		for _i, side in ipairs({ size.x, size.y }) do
			assert(side == -1 or side > 0, what .. ": a size of " .. tostring(side) .. " hides what is in it")
		end
	end
	return size
end

for _i, view in ipairs({ "BoxLayout", "Component", "TextView", "Button", "ImageView", "TextInputField",
		"ScrollArea", "ComboBox", "ComboBoxItem", "ProgressBar", "FloatingLayout", "FloatingLayoutChild",
		"ShaderQuad", "Window", "TabWidget", "TabWidgetChild" }) do
	-- A view is a recipe the game has: called, it gives the node; its name
	-- says which view a wrapper recipe wraps.
	builtin[view] = setmetatable({ viewName = view }, { __call = function(_, params)
		whole(params.children, view)
		whole(params.items, view)
		sized(params, view)
		if view == "Component" and params.layout ~= nil then
			assert(LAYOUTS[params.layout.view], "a Component's layout must be a layout, not " .. tostring(params.layout.view))
		end
		return { view = view, params = params }
	end })
end

-- Every text field and combo box has a width and a height, its own or its
-- box's: the game gives an unsized one none.
local function checkInputs(node, box)
	if type(node) ~= "table" then return end
	if node.view then
		local size = node.params and sized(node.params, node.view)
		local own = size and size.x > 0 and size.y > 0 and size or nil
		if node.view == "TextInputField" or node.view == "ComboBox" then
			assert(own or box, node.view .. " without a size")
		elseif node.view == "Component" then
			-- The box a Component's own layout lays its children out in.
			box = own
		elseif node.view ~= "BoxLayout" then
			box = nil
		end
	end
	for _k, value in pairs(node) do
		if type(value) == "table" then checkInputs(value, box) end
	end
end

local gui_react_util = {
	makeHorizontalSpacer = function() return { view = "Spacer" } end,
	makeVerticalSpacer = function() return { view = "Spacer" } end,
}
local button_react_util = {
	makeIconButton = function(_ref, path, onClick, tooltip)
		return { view = "Button", params = { icon = path, onClick = onClick, meta = { tooltip = tooltip } } }
	end,
}

-- The main menu's card button, as menu_icon_react_util.tl builds it: a
-- recipe (so it returns a layout) around a Button.
CARD_CLICKS = {}
-- The game's Load Game page draws with these (load_game_page.tl,
-- savegame_react_util.tl); a pick of the room's save and mods swaps them
-- while it lasts. PAGE_TITLE, PAGE_BUTTON and PAGE_CARD hold what the page
-- would draw now.
local menu_icon_react_util
function LOAD_PAGE()
	return {
		title = menu_icon_react_util.makePage({}, "Load Game", nil, nil, {}).title,
		button = menu_icon_react_util.makePrimaryButton("Load Game", nil,
			"loadSavegameButton, keyhint-builtin-right-inside, load-savegame-sound"),
		card = savegame_react_util.SavegameCard({ onClickDetails = "details" }),
		-- The save tiles' list, drawn after them: its tiles, as drawn.
		tiles = (function()
			tile_list_react_util.TileList{ elements = { { view = "SavegameCard" } } }
			return LAST_TILES
		end)(),
	}
end
-- A recipe of the game's is callable userdata, not a Lua function (what
-- a pick first refused, 2026-10-04): a callable table here.
savegame_react_util = {
	SavegameCard = setmetatable({}, { __call = function(_self, first, second)
		return { view = "SavegameCard", params = second or first }
	end }),
}
menu_icon_react_util = {
	makePage = function(_common, text, back, center, extra)
		return { view = "Page", title = text, params = { back = back, center = center, extra = extra } }
	end,
	makeMainOuterCard = function(_spacer, children)
		return builtin.Component{ layout = builtin.BoxLayout{ children = children } }
	end,
	makeTabAnalogue = function(header, body)
		return builtin.Component{ layout = builtin.BoxLayout{ children = {
			builtin.Component{ layout = builtin.BoxLayout{ children = header } },
			builtin.Component{ layout = builtin.BoxLayout{ children = body } },
		} } }
	end,
	makePrimaryButton = function(text, onClick, classes, enabled, tooltip)
		local node = builtin.Button{
			meta = { class = tostring(classes) .. ", primary", enabled = enabled ~= false, tooltip = tooltip },
			content = builtin.TextView{ text = text },
			onClick = onClick,
		}
		node.text, node.classes = text, classes
		return node
	end,
	makeSecondaryButton = function(text, onClick, classes, enabled, tooltip)
		return builtin.Button{
			meta = { class = tostring(classes) .. ", secondary", enabled = enabled ~= false, tooltip = tooltip },
			content = builtin.TextView{ text = text },
			onClick = onClick,
		}
	end,
	makeCardLabelBottomComponent = function(title, description, right)
		local children = {
			builtin.TextView{ text = title },
			description and builtin.TextView{ text = description } or builtin.TextView{ text = "" },
			right and builtin.TextView{ text = right } or builtin.TextView{ text = "" },
		}
		return builtin.FloatingLayout{ children = children }
	end,
}
menu_icon_react_util.CardButton = react.RegisterRecipe("CardButton", function(params)
	return builtin.BoxLayout{ children = {
		builtin.Button{
			meta = { tooltip = params.tooltip, enabled = params.enabled, class = "main-menu-card, " .. tostring(params.class) },
			content = builtin.Component{ layout = builtin.FloatingLayout{ children = (function()
				-- Its label, then what lies on its picture (marks, corners).
				local children = { params.bottomComponent }
				for _i, child in ipairs(params.extraChildren or {}) do children[#children + 1] = child end
				return children
			end)() } },
			onClick = params.onClick,
			card = true,
			images = params.images,
		},
	} }
end)

-- The game's cards (content_card.tl), tiles (tile_list_react_util.tl) and
-- mod pictures and Activate button (mod_manager_react_util.tl), as far as
-- the window uses them: a tile shows its title, its info icons' labels,
-- its picture and its buttons, and a Details button when it has any.
content_card = {
	ContentCard = react.RegisterRecipe("ContentCard", function(p)
		local children = { builtin.TextView{ text = p.title } }
		for _i, child in ipairs(p.extraChildrenPermanent or {}) do children[#children + 1] = child end
		return builtin.BoxLayout{ children = children }
	end),
}
tile_list_react_util = {
	TileList = react.RegisterRecipe("TileList", function(p)
		LAST_TILES = p.elements
		return builtin.BoxLayout{ children = p.elements }
	end),
	TileElement = react.RegisterRecipe("TileElement", function(p)
		assert(type(p.createImage) == "function", "a tile needs its picture")
		local children = { builtin.TextView{ text = p.title } }
		for _i, i in ipairs(p.infoIcons or {}) do
			children[#children + 1] = builtin.TextView{ text = i.label, meta = { tooltip = i.tooltip } }
		end
		children[#children + 1] = p.createImage()
		local buttons, fallback = {}, nil
		if p.createButtons then buttons, fallback = p.createButtons() end
		buttons = buttons or {}
		-- As the game's tile does: its Details button added to the list
		-- it was given.
		if #buttons > 0 then
			buttons[#buttons + 1] = builtin.Button{ meta = { tooltip = "Details" }, onClick = p.onClickDetails or fallback }
		end
		for _i, b in ipairs(buttons) do children[#children + 1] = b end
		local node = builtin.BoxLayout{ children = children }
		node.tile = p
		return node
	end),
}
mod_manager_react_util = {
	ModImage = react.RegisterRecipe("ModImage", function(p)
		return builtin.BoxLayout{ children = { builtin.ImageView{ path = p.request and ("logo:" .. tostring(p.request.id.value)) or p.imagePath } } }
	end),
	ModActivateButton = react.RegisterRecipe("ModActivateButton", function(p)
		return builtin.BoxLayout{ children = { builtin.Button{
			content = builtin.TextView{ text = p.active and "Activated" or "Activate" },
			onClick = function() p.onValueChange(not p.active) end,
		} } }
	end),
	ModDetailsWindow = "ModDetailsWindow",
	-- The game's check before Mod Hub: HUB.access false, it says why not.
	checkModManagerAccess = function(_wc) return HUB.access ~= false end,
}

-- The menu's window container and modal block (commonParams): WINDOWS[recipe]
-- holds the params of the window shown, MODAL whether the menu is blocked.
-- With NO_WINDOWS the menu gives none, as an older game might not.
WINDOWS = {}
MODAL = false
local windowApi = {
	addSingletonWindow = function(recipe, params) WINDOWS[recipe] = params end,
	removeAllWindows = function(recipe) WINDOWS[recipe] = nil end,
}
local COMMON = {
	windowContainer = { get = function() return { getApi = function() return windowApi end } end },
	setBlockedForModal = function(blocked) MODAL = blocked end,
}

local modules = {
	["::/gui/main/react.lua"] = react,
	["::/gui/main/builtin.lua"] = builtin,
	["::/gui/main/gui_react_util.tl"] = gui_react_util,
	["::/gui/main/button_react_util.tl"] = button_react_util,
	["::/gui/menu/menu_icon_react_util.tl"] = menu_icon_react_util,
	["/gui/menu/menu_icon_react_util.tl"] = menu_icon_react_util,
	["/gui/menu/savegame_react_util.tl"] = savegame_react_util,
	["/gui/main/tile_list_react_util.tl"] = tile_list_react_util,
	["::/gui/main/content_card.tl"] = content_card,
	["::/gui/main/tile_list_react_util.tl"] = tile_list_react_util,
	["::/gui/menu/mod_manager_react_util.tl"] = mod_manager_react_util,
}
function ug_require(path)
    if path == "tpf3mp_1::/scripts/tpf3mp/banners.lua" then return assert(loadstring(BANNERS_SOURCE))() end
	if path == "tpf3mp_1::/gui/menu/roommods.lua" then
		ROOMMODS = ROOMMODS or assert(loadstring(ROOMMODS_SOURCE, "@roommods.lua"))()
		return ROOMMODS
	end
	return assert(modules[path], "no module " .. path)
end

local lobby = assert(loadstring(LOBBY_SOURCE, "@lobby.lua"))()
LOBBY = lobby
CLOSED = 0
local tree
local focus

function render(f)
	if f ~= nil then focus = f end
	for _i, ref in pairs(mount.refs) do
		if ref.stateTracked then ref.drawn = ref.value end
	end
	mount.index = 0
	mount.timers = {}
	tree = lobby.content(function() CLOSED = CLOSED + 1 end, focus, function() GENERATED = (GENERATED or 0) + 1 end,
		function()
			-- main_page.tl's: the pick begins and the Load Game page opens.
			if lobby.beginPick(function(page) PAGE = page end) then PAGE = "LoadGame" end
		end,
		function() MODHUB = (MODHUB or 0) + 1 end, NO_WINDOWS and {} or COMMON)
	checkInputs(tree, nil)
	return tree
end

-- The game drops the main page, and the window with it, when it shows
-- another page (Load Game, Mod Hub): what the window held is gone, and it
-- is made anew when the main page comes back.
function unmount()
	mount.refs = {}
	mount.timers = {}
	tree = nil
end

-- The page's Back, top left (or the game's Back key), then a redraw.
function page_back()
	assert(tree.view == "Page" and tree.params.back, "the window is a page with a Back")
	tree.params.back()
	return render()
end

-- The page's title in its top bar.
function page_title() return tree.title end

-- One poll of the window's timer, then a redraw, as the game does.
function tick()
	for _i, fn in ipairs(mount.timers) do fn() end
	return render()
end

local function walk(node, visit)
	if type(node) ~= "table" then return end
	visit(node)
	for _k, value in pairs(node) do
		if type(value) == "table" then walk(value, visit) end
	end
end

-- Every text shown, in one string, one per line.
function texts()
	local out = {}
	walk(tree, function(node)
		if node.view == "TextView" then out[#out + 1] = node.params.text end
		if node.view == "ComboBoxItem" then end
	end)
	return table.concat(out, "\n")
end

-- Every picture an ImageView shows, in drawing order.
function images()
	local out = {}
	walk(tree, function(node)
		if node.view == "ImageView" then out[#out + 1] = node.params.path end
	end)
	return out
end

-- The button showing `text` (or with that tooltip), nil if none.
function find(text)
	local found
	walk(tree, function(node)
		if node.view == "Button" and not found then
			local content = node.params.content
			local shown = content and content.params and content.params.text
			if shown == text or (node.params.meta and node.params.meta.tooltip == text) then
				found = node.params
			end
		end
	end)
	return found
end

function enabled(text)
	local button = assert(find(text), "no button " .. text)
	return button.meta == nil or button.meta.enabled ~= false
end

function click(text)
	local button = assert(find(text), "no button " .. text)
	assert(button.meta == nil or button.meta.enabled ~= false, "button disabled: " .. text)
	button.onClick()
	return render()
end

-- Types into the field showing `placeholder`, and presses Enter.
function type_into(placeholder, text)
	local done = false
	walk(tree, function(node)
		if node.view == "TextInputField" and node.params.placeholderText == placeholder and not done then
			node.params.onTyping(text)
			node.params.onValueChange(text)
			done = true
		end
	end)
	assert(done, "no field " .. placeholder)
	return render()
end

-- Picks `value` in the combo box under the caption `caption`.
function choose(caption, value)
	local done = false
	walk(tree, function(node)
		if node.view == "Component" and not done then
			local children = node.params.layout and node.params.layout.params.children
			local first = children and children[1]
			if first and first.view == "TextView" and first.params.text == caption then
				walk(node, function(inner)
					if inner.view == "ComboBox" and not done then
						inner.params.onValueChange(value)
						done = true
					end
				end)
			end
		end
	end)
	assert(done, "no choice " .. caption)
	return render()
end

-- The values a choice under `caption` offers, and the one chosen.
function offered(caption)
	local values, chosen = {}, nil
	walk(tree, function(node)
		if node.view == "Component" and chosen == nil then
			local children = node.params.layout and node.params.layout.params.children
			local first = children and children[1]
			if first and first.view == "TextView" and first.params.text == caption then
				walk(node, function(inner)
					if inner.view == "ComboBox" and chosen == nil then
						chosen = inner.params.value
						for _i, item in ipairs(inner.params.items) do values[#values + 1] = item.params.value end
					end
				end)
			end
		end
	end)
	return values, chosen
end

-- The room cards shown: each the texts on it, its picture and its click.
function room_cards()
	local found = {}
	walk(tree, function(node)
		if node.view == "Button" and node.params.card then
			local texts = {}
			walk(node.params.content, function(inner)
				if inner.view == "TextView" then texts[#texts + 1] = inner.params.text end
			end)
			found[#found + 1] = {
				text = table.concat(texts, "\n"),
				-- A picture by path, or an image's parameters with its path.
				picture = type(node.params.images[1]) == "table" and node.params.images[1].path
					or node.params.images[1],
				tooltip = node.params.meta.tooltip,
				click = node.params.onClick,
				enabled = node.params.meta.enabled ~= false,
			}
		end
	end)
	return found
end

-- Clicks the card (Join, Host, a room) whose texts include `title`.
function click_card(title)
	for _i, card in ipairs(room_cards()) do
		if card.text:find(title, 1, true) then
			assert(card.enabled, "card disabled: " .. title)
			card.click()
			return render()
		end
	end
	error("no card " .. title)
end
-- Count member cards across a row, including cards beside portraits.
function most_cards_in_a_row()
    local most = 0
    walk(tree, function(node)
        if node.view == "BoxLayout" and node.params.orientation == "Horizontal" then
            local count = 0
            for _, child in ipairs(node.params.children or {}) do
                local has = false
                walk(child, function(inner)
                    if inner.view == "Button" and inner.params.card then has = true end
                end)
                if has then count = count + 1 end
            end
            if count > most then most = count end
        end
    end)
    return most
end

-- The tab whose indicator says `text`, picked as a click on it does.
function tab(text)
	local found
	walk(tree, function(node)
		if node.view == "TabWidget" and not found then
			for _i, child in ipairs(node.params.tabs or {}) do
				local indicator = child.params.indicator
				local shown = indicator and indicator.params and indicator.params.text
				if shown == text then found = { widget = node.params, value = child.params.value } end
			end
		end
	end)
	assert(found, "no tab " .. text)
	found.widget.onValueChange(found.value)
	return render()
end

-- The tab shown now: its indicator's text.
function current_tab()
	local shown
	walk(tree, function(node)
		if node.view == "TabWidget" and not shown then
			for _i, child in ipairs(node.params.tabs or {}) do
				if child.params.value == node.params.value then shown = child.params.indicator.params.text end
			end
		end
	end)
	return shown
end

-- How many buttons show `text` (or have it as their tooltip).
function count_buttons(text)
	local n = 0
	walk(tree, function(node)
		if node.view == "Button" then
			local content = node.params.content
			local shown = content and content.params and content.params.text
			if shown == text or (node.params.meta and node.params.meta.tooltip == text) then n = n + 1 end
		end
	end)
	return n
end
