-- TPF3-MP: the game's gui/menu/main_page.tl with a Multiplayer entry added to
-- the main menu. The hook serves this copy in place of the game's file
-- (docs/LOBBY.md). Every change is marked "TPF3-MP:" so a game patch can be
-- re-applied: tools/lobby/make_main_page.py takes the new game file and
-- re-applies the marked blocks. Relative and leading-slash paths are made
-- absolute ("::/") so the copy resolves the game's files from the mod's folder.
local react = ug_require "::/gui/main/react.lua" as React
local builtin = ug_require "::/gui/main/builtin.lua" as Builtin
local button_react_util = ug_require "::/gui/main/button_react_util.tl" as ButtonReactUtil
local callout_react_util = ug_require "::/gui/main/callout_react_util.tl" as CalloutReactUtil
local deluxeEditionContentFn = ug_require "::/gui/menu/configs/deluxe_content.lua" as function(GameEditionTextParam) : ReleaseNoteData
local engine_react_util = ug_require "::/gui/main/engine_react_util.tl" as EngineReactUtil
local gui_react_util = ug_require "::/gui/main/gui_react_util.tl" as GuiReactUtil
local mathutil = ug_require "::/scripts/mathutil.lua" as MathUtil
local menu_icon_react_util = ug_require "::/gui/menu/menu_icon_react_util.tl" as MenuIconReactUtil
local menu_util = ug_require "::/gui/menu/menu_util.tl" as MenuUtil
local mod_manager_react_util = ug_require "::/gui/menu/mod_manager_react_util.tl" as ModManagerReactUtil
local ReleaseNotesWindowContent = ug_require "::/gui/menu/releasenotes_window.tl" as Recipe<table>
local savegame_react_util = ug_require "::/gui/menu/savegame_react_util.tl" as SavegameReactUtil
local table_util = ug_require "::/scripts/table_util.tl" as TableUtil

-- TPF3-MP: the game log shows the copy is in effect, even if nothing is clicked.
pcall(debugPrint, "[tpf3mp] main menu: TPF3-MP main_page.tl is in effect")

local ModManagementStatusMainPage = react.RegisterRecipe("ModManagementStatusMainPage", function() : TreeNodeId
	local remoteModhub = mod_manager_react_util.getModPublishBackendId()

	local modManagementBusyState = engine_react_util.useStepState(function() : boolean

		if remoteModhub == nil then
			return false
		end

		return api.modhub.getModManagementState(remoteModhub).busy
	end)

	return gui_react_util.makeHorizontalBox({
		modManagementBusyState:old() and mod_manager_react_util.ModManagementStatus {
			context = {
				backendId = remoteModhub
			},
			upload = false,
		} or nil
	})
end)

local record DeluxeContentWindowParam
	onClose : function()
	pos : Vec2f
	edition : GameEditionTextParam
end

local DeluxeContentWindow = react.RegisterWrapperRecipe("DeluxeContentWindow", builtin.Window, function(param : DeluxeContentWindowParam) : TreeNodeId
	local data = deluxeEditionContentFn(param.edition)
	return builtin.Window{
		title = data.title or " ",
		id = "window.deluxe.content",
		meta = {
			class = "fade-in"
		},
		initialX = param.pos and param.pos.x or nil,
		initialY = param.pos and param.pos.y or nil,
		movable = false,
		closable = true,
		onClose = param.onClose,
		content = ReleaseNotesWindowContent{
			releaseNotes = data,
			onClose = param.onClose,
			edition = param.edition,
			hasDoNotShowAgainButton = false
		},
	}
end)

-- TPF3-MP: the Multiplayer window, opened from the main menu into the menu's
-- own window container (as DeluxeContentWindow is). Its content is the mod's
-- gui/menu/lobby.lua: the lobby, talking to the hook.
local record LobbyModule
	prepareNewWorld : function()
	content : function(onClose : function(), focus : string, onNewGame : function()) : TreeNodeId
	CardLine : function(params : any) : TreeNodeId
	joinLine : function(state : any) : string
end
local lobby = ug_require "tpf3mp_1::/gui/menu/lobby.lua" as LobbyModule

local record Tpf3mpLobbyWindowParam
	onNewGame : function()
	onClose : function()
	pos : Vec2f
	focus : string
end

-- TPF3-MP: the lobby's content, or, should its Lua fail, the error and a way
-- out - never a window that cannot be closed.
local function safeContent(onClose : function(), focus : string, onNewGame : function()) : TreeNodeId
	local ok, result = pcall(lobby.content, onClose, focus, onNewGame)
	if ok then
		return result as TreeNodeId
	end
	pcall(debugPrint, "[tpf3mp] lobby: content failed: " .. tostring(result))
	return builtin.BoxLayout{
		orientation = builtin.type.Orientation.Vertical,
		children = {
			builtin.TextView{ meta = { class = "font-scale-title-4" }, text = _("Multiplayer") },
			builtin.TextView{ meta = { class = "font-scale-body, error" }, text = tostring(result) },
			builtin.Button{
				meta = { class = "secondary" },
				content = builtin.TextView{ text = _("Close") },
				onClick = onClose,
			},
		},
	}
end

local Tpf3mpLobbyWindow = react.RegisterWrapperRecipe("Tpf3mpLobbyWindow", builtin.Window, function(param : Tpf3mpLobbyWindowParam) : TreeNodeId
	-- Centred on the screen at any resolution, as the game centres its mod
	-- validation report (mod_manager_react_util.tl): by that window's class,
	-- whose rule in the game's style sheet (mod_browser.css.lua,
	-- "Window!validation-report-dialog") sets anchorPoint and gravity to
	-- 0.5, 0.5 and nothing else. A wrapper recipe may pass meta only for its
	-- class: a styleSheet with anchorPoint made the game assert and close
	-- ("Wrapper recipe must return child", 2026-09-30). initialX and
	-- initialY are shares of the screen for the window's anchor point, so
	-- none is given: the class's gravity places it.
	return builtin.Window{
		title = _("Multiplayer"),
		id = "window.tpf3mp.lobby",
		meta = { class = "fade-in, validation-report-dialog" },
		movable = false,
		closable = true,
		onClose = param.onClose,
		content = safeContent(param.onClose, param.focus, param.onNewGame),
	}
end)

local GameEditionTextMainPage = react.RegisterRecipe("GameEditionTextMainPage", function(param : GameEditionTextParam, showDeluxeContent : function()) : TreeNodeId
	if not param.hasDeluxeDLC and not param.hasPreorderDLC then
		return nil
	end

	local editionText = param.hasDeluxeDLC and _("Deluxe Edition") or _("Standard Edition")

	if param.hasPreorderDLC then
		editionText = editionText .. "\n" .. _("(+ Pre-Order content)")
	end

	local gameEditionLayout = gui_react_util.makeHorizontalBox({
			builtin.ImageView {
				meta = { },
				path = "::/gui/menu/icons/tf3_icon.tga"
			},
			builtin.TextView {
					meta = {
						class = "font-scale-title-4",
					},
					text = editionText
				},
		});

	if param.hasDeluxeDLC and showDeluxeContent ~= nil then
		local onClick = function()
			showDeluxeContent()
		end

		return gui_react_util.makeHorizontalBox({
			builtin.Button{
				meta = {},
				content = builtin.Component{
					layout = gameEditionLayout
				},
				onClick = onClick,
			}
		})
	end

	return gameEditionLayout
end)


-- keep in sync with main_menu.css.lua!
local fadeInDuration = 0.24
local fadeInInitialDelay = 0.04
local fadeInDelayPerStep = 0.15

local function computeClickAllowedDelay(i : integer) : number
	local start = fadeInInitialDelay + fadeInDelayPerStep * (i - 1)
	return start + fadeInDuration * 0.5
end

local enum DelayUntilClickableKey
	"Cards"
	"TopBar"
end
local DelayUntilClickable : {DelayUntilClickableKey : number} = {
	Cards = computeClickAllowedDelay(3),
	TopBar = computeClickAllowedDelay(4),
}

return react.RegisterRecipe("MainPage", function(mainPageParams : MenuPageParam<MainPageParam>) : TreeNodeId
	local startWithReleaseNotes = mainPageParams.params and mainPageParams.params.startWithReleaseNotes or false

	local titleIconOnlyState = react.useState(startWithReleaseNotes)
	local showReleaseNotesWhenReadyRef = react.useRef(startWithReleaseNotes)

	local cardsFadeInStartTimeRef = react.useRef(api.util.getApplicationTime())
	local fadingIn = mainPageParams.commonParams.fadeIn == "FadeIn"
	local fastFadeInState = react.useState(titleIconOnlyState:old())
	local prevFadingInRef = react.useRef(false)

	local gameEditionParamState = engine_react_util.useStepStateTimer(function() : GameEditionTextParam
		return mainPageParams.commonParams.getDLCModState()
	end)

	if fadingIn and not prevFadingInRef:get() then
		cardsFadeInStartTimeRef:set(api.util.getApplicationTime())
		prevFadingInRef:set(fadingIn)
	end

	local cardWrapRef = react.useNodeRef()
	local showReleaseNotes = function()
		local pos = api.type.Vec2f.new(0.5, 0.5)
		showReleaseNotesWhenReadyRef:set(false)
		titleIconOnlyState:set(true)
		mainPageParams.commonParams.showReleaseNotes(true, function()
			titleIconOnlyState:set(false)
			fastFadeInState:set(true)
			cardsFadeInStartTimeRef:set(api.util.getApplicationTime())
		end, pos)
	end

	local showDeluxeContent = function()
		local pos = api.type.Vec2f.new(0.5, 0.5)
		titleIconOnlyState:set(true)
		local wc = mainPageParams.commonParams.windowContainer:get():getApi()
		wc.addSingletonWindow(DeluxeContentWindow, {
			onClose = function()
				titleIconOnlyState:set(false)
				fastFadeInState:set(true)
				cardsFadeInStartTimeRef:set(api.util.getApplicationTime())
				mainPageParams.commonParams.windowContainer:get():getApi().removeAllWindows(DeluxeContentWindow)
			end,
			pos = pos,
			edition = gameEditionParamState:old(),
		})
	end

	-- TPF3-MP: open the Multiplayer window, as showDeluxeContent opens its
	-- window; `focus` "join" puts joining by invite first.
	local showMultiplayer = function(focus : string)
		local pos = api.type.Vec2f.new(0.5, 0.5)
		titleIconOnlyState:set(true)
		local wc = mainPageParams.commonParams.windowContainer:get():getApi()
		wc.addSingletonWindow(Tpf3mpLobbyWindow, {
			onNewGame = function()
				lobby.prepareNewWorld()
				wc.removeAllWindows(Tpf3mpLobbyWindow)
				mainPageParams.commonParams.setPage("NewGame", { map = false })
			end,
			onClose = function()
				titleIconOnlyState:set(false)
				fastFadeInState:set(true)
				cardsFadeInStartTimeRef:set(api.util.getApplicationTime())
				mainPageParams.commonParams.windowContainer:get():getApi().removeAllWindows(Tpf3mpLobbyWindow)
			end,
			pos = pos,
			focus = focus,
		})
	end

	if titleIconOnlyState:old() then
		react.setStyleClasses("title-icon-only")
	elseif fastFadeInState:old() then
		react.setStyleClasses("fast-fade-in")
	end

	-- local helpers -------------------------------------------------------------------------------
	local isGamepad = function() : boolean
		return api.util.getInputMode() == api.type["enum"].InputMode.Gamepad
	end

	local function getCalloutApi() : CalloutApi
		return mainPageParams.commonParams.calloutContainerRef:get():getApi()
	end


	local function clickAllowed(key : DelayUntilClickableKey) : boolean
		if mainPageParams.commonParams.fadeIn == "Invisible" then
			return false
		end
		if not fadingIn then
			return true
		end
		return api.util.getApplicationTime() >= cardsFadeInStartTimeRef:get() + DelayUntilClickable[key]
	end

	local hBox = function(children : {TreeNodeId}, class? : string) : TreeNodeId
		return builtin.Component{
			meta = {
				class = class
			},
			mouseTransparent = true,
			layout = builtin.BoxLayout {
				orientation = builtin.type.Orientation.Horizontal,
				children = children,
			},
		}
	end
	
	local vBox = function(children : {TreeNodeId}, class? : string, ref? : ReactRefWrap) : TreeNodeId
		if ref then
			return builtin.Component(react.ref(ref), {
				meta = {
					class = class
				},
				mouseTransparent = true,
				layout = builtin.BoxLayout {
					orientation = builtin.type.Orientation.Vertical,
					children = children,
				},
			})
		end
		return builtin.Component{
			meta = {
				class = class
			},
			mouseTransparent = true,
			layout = builtin.BoxLayout {
				orientation = builtin.type.Orientation.Vertical,
				children = children,
			},
		}
	end

	local makeFloatChild = function(pos : {number, number}, children : TreeNodeId) : TreeNodeId
		return builtin.FloatingLayoutChild{
			h = pos[1],
			v = pos[2],
			item = children
		}
	end

	-- main card images ------------------------------------------------------------------------------
	local imagesMapEditor = {
		"::/gui/menu/images/mapeditor_ingame.tga",
		"::/gui/menu/images/mapeditor_ingame_2.tga",
	}
	local imagesModHub = {
		"::/gui/menu/images/mod01_ingame.tga",
		-- "::/gui/menu/images/mod02_ingame.tga",
	}
	local imagesCampaignAll = {
		"::/gui/menu/images/m01_ingame.tga",
		"::/gui/menu/images/m02_ingame.tga",
		"::/gui/menu/images/m03_ingame.tga",
		"::/gui/menu/images/m04_ingame.tga",
		"::/gui/menu/images/m05_ingame.tga",
		"::/gui/menu/images/m06_ingame.tga",
		"::/gui/menu/images/m07_ingame.tga",
		"::/gui/menu/images/m08_ingame.tga",
	}
	local imagesClimates = {
		"::/gui/menu/images/temperate_ingame.tga",
		"::/gui/menu/images/subarctic_ingame.tga",
		"::/gui/menu/images/tropical_ingame.tga",
		"::/gui/menu/images/dry_ingame.tga",
	}
	local imagesTutorial = {
		"::/gui/menu/images/main.tga",
	}
	local imagesLoadGame = {
		"::/gui/menu/images/loadgame.tga",
	}

	local mapEditorImgIdx = math.ceil(math.random(#imagesMapEditor))
	local campaignImgIdx = 1
	local climateImgIdx = math.ceil(math.random(#imagesClimates))
	local modhubImgIdx = math.ceil(math.random(#imagesModHub))
	local loadGameImgIdx = 1
	local tutorialImgIdx = 1

	local climateDisplayDuration = 36.0
	local campaignDisplayDuration = 120.0
	local mapEditorDisplayDuration = 240.0
	local modhubDisplayDuration = 240.0
	local loadGameDisplayDuration = 36.0

	local climateSwapOffset = 0.16 * climateDisplayDuration
	local campaignSwapOffset = 0.32 * campaignDisplayDuration
	local mapEditorSwapOffset = 0.5 * mapEditorDisplayDuration
	local modhubSwapOffset = 0.83 * modhubDisplayDuration
	local loadGameSwapOffset = 0.63 * loadGameDisplayDuration

	-- states and refs -------------------------------------------------------------------------------

	local toSelect = react.useNodeRef()
	react.onMount(function()
		app.refreshMods()
		toSelect:get():focus()
	end)

	react.onStep(function()
		if showReleaseNotesWhenReadyRef:get() then
			if cardsFadeInStartTimeRef:get() + DelayUntilClickable["TopBar"] < api.util.getApplicationTime() then
				showReleaseNotes()
			end
		end
	end)

	local asyncContinueRef : ReactRefT<Async<SaveGameData>> = react.useRef()
	local dataContinueState : ReactStateT<SaveGameData> = react.useState()

	local record ContinueState
		lastSaveGameId : SaveGameId
		recentCampaign : MissionCampaign
 	end
	local makeContinueState = function() : ContinueState
		local missionOrSaveGameId = app.getUserProfile():getLastGame()
		local lastSaveGameId = missionOrSaveGameId as SaveGameId
		local recentCampaign : MissionCampaign
		if missionOrSaveGameId and not lastSaveGameId.saveGameName then
			lastSaveGameId = nil
			recentCampaign = missionOrSaveGameId as MissionCampaign
		end

		return {
			lastSaveGameId = lastSaveGameId,
			recentCampaign = recentCampaign,
		}
	end
	local continueState : ReactStateT<ContinueState> = react.useStateLazy(makeContinueState)

	local modMissingIconRef = react.useNodeRef()
	local isGamepadOnlyForErrorCallout = react.useState(isGamepad())

	-- actual components -------------------------------------------------------------------------------
	local continueCard : TreeNodeId
	if continueState:old().lastSaveGameId ~= nil or continueState:old().recentCampaign ~= nil then
		if continueState:old().lastSaveGameId ~= nil and asyncContinueRef:get() == nil then
			asyncContinueRef:set(app.getSavegameInfo(continueState:old().lastSaveGameId))
		end

		react.onStep(function()
			local mostRecentContinueState = makeContinueState()
			if not table_util.deepEquals(mostRecentContinueState, continueState:old()) then
				continueState:set(mostRecentContinueState)
				if asyncContinueRef:get() then
					asyncContinueRef:get():abort()
				end
				asyncContinueRef:set(nil)
				dataContinueState:set(nil)
			end
			if dataContinueState:old() == nil and asyncContinueRef:get() and asyncContinueRef:get():isCompleted() then
				dataContinueState:set(asyncContinueRef:get():get())
			end
		end)

		local showContinueButton = false
		local errorTypes : {SavegameReactUtil.ErrorType} = {}
		local errorMsg : string = ""
		if dataContinueState:old() ~= nil and continueState:old().lastSaveGameId then
			errorTypes, errorMsg = savegame_react_util.checkForError(dataContinueState:old())
			showContinueButton = not table_util.arrayContains(errorTypes, "CannotLoad")
		elseif continueState:old().recentCampaign then
			showContinueButton = true
		end

		local function setShowCallout(show : boolean)
			if not show then
				getCalloutApi().close(modMissingIconRef)
			elseif #errorTypes > 0 and showContinueButton then
				local calloutParams = {
					recipe = callout_react_util.DefaultCalloutWrapper,
					recipeParam = {
						meta = {
							class = "critical",
						},
						recipe = builtin.TextView,
						recipeParam = {
							text = errorMsg,
							meta = {
								class = "font-scale-body"
							}
						},
					},
					anchor = api.type.Vec2f.new(0.5, 1.0),
					gravity = api.type.Vec2f.new(0.5, 0.0),
					parent = modMissingIconRef,
				} as CalloutApi.CalloutParamsT<IRecipeParamWithMeta>
				getCalloutApi().open(calloutParams)
			end
		end
		react.onStep(function()
			-- very special case when someone with gamepad has a callout shown
			-- and in EXACTLY this moment this person decides to take the mouse
			if isGamepadOnlyForErrorCallout:old() ~= isGamepad() then
				isGamepadOnlyForErrorCallout:set(isGamepad())
				if not isGamepad() then
					setShowCallout(false)
				end
			end
		end)

		if showContinueButton then
			local continueTitle = continueState:old().lastSaveGameId and savegame_react_util.getSaveGroupAndType(continueState:old().lastSaveGameId.saveGameName) or ""
			local continueImage : Builtin.ImageViewParam = {
				path = "::/gui/menu/icons/loading.tga",
				scaling = builtin.type.ImageViewScaling.AutoZoom,
			}

			if dataContinueState:old() then
				if dataContinueState:old().info then
					local imageNative = dataContinueState:old()
					continueImage = {
						data_native = imageNative.info.metadata.screenshot.image_native:clone(),
						size = api.type.Vec2i.new(
							dataContinueState:old().info.metadata.screenshot.width,
							dataContinueState:old().info.metadata.screenshot.height),
						scaling = builtin.type.ImageViewScaling.AutoZoom,
					}
				else
					continueImage = {
						path = "::/gui/menu/icons/error.tga",
						scaling = builtin.type.ImageViewScaling.AutoZoom,
					}
				end
			elseif continueState:old().recentCampaign ~= nil then
				local campaignInfo = savegame_react_util.getCampaignInfoSorted(continueState:old().recentCampaign.campaign)
				local missionImage : string
				for __, missionInfo in ipairs(campaignInfo.missionInfo) do
					if missionInfo.enabled then
						missionImage = missionInfo.desc.image
						continueTitle = missionInfo.desc.name
					end
				end
				continueImage = {
					path = missionImage or "::/gui/menu/images/campaign.tga",
					scaling = builtin.type.ImageViewScaling.AutoZoom,
				}
			end

			local function checkForSavegameErrors(errorTypesParam : {SavegameReactUtil.ErrorType}) : boolean
				for __, v in ipairs(errorTypesParam) do
					if v ~= "CannotLoad" then
						return true
					end
				end
				return false
			end
			
			local hasSavegameErrors = checkForSavegameErrors(errorTypes)
			local continueOld = continueState:old()
			local dataOld = dataContinueState:old()
			continueCard = menu_icon_react_util.CardButton{
				meta = {
					class = "continue-game-cardbutton",
					onFocusChange = function(hasFocus : boolean)
						if isGamepad() then
							setShowCallout(hasFocus)
						end
					end or nil
				},
				bottomComponent = menu_icon_react_util.makeCardLabelBottomComponent(
					_("Continue"),
					nil,
					continueTitle,
					hasSavegameErrors and builtin.ImageView(react.ref(modMissingIconRef), {
						meta = {
							class = "error, loading-issue",
							onMouseOver = function(hover : boolean)
								setShowCallout(hover)
							end or nil,
						},
						path = "::/gui/menu/icons/alert.tga",
					}) or nil,
					true -- mainTitleCard
				),
				onClick = function()
					if continueOld.lastSaveGameId then
						local errorTypesOld, _errorMessageOld = savegame_react_util.checkForError(dataOld)
						if #errorTypesOld == 0 then
							mainPageParams.commonParams.setPage("ProgressPage", {
								saveGameId = continueOld.lastSaveGameId,
								mission = dataContinueState:old().info and dataContinueState:old().info.mission or "",
								onMount = function()
									app.setWaitForStartReadyGame()
									app.loadGame(continueOld.lastSaveGameId, false, nil)
								end
							})
						else
							mainPageParams.commonParams.setPage("LoadGame", { map = false })
						end
					elseif continueOld.recentCampaign then
						mainPageParams.commonParams.setPage("Campaign", { campaign = continueOld.recentCampaign.campaign })
					end
				end,
				onAttention = function(x : number, y : number)
					mainPageParams.commonParams.triggerBackgroundEvent(api.type.Vec2f.new(x, y))
				end,
				images = { continueImage, },
				class =  "load-savegame-sound, small-card",
				enabled = true,
			}
		end
	end

	-- local notifications = button_react_util.makeIconButton(nil, "::/gui/menu/icons/news_50.tga",
	-- 	function()
	-- 		if clickAllowed("TopBar") then
	-- 			showReleaseNotes()
	-- 		end
	-- 	end, _("Check the Latest Release Notes"))

	local profile = menu_util.UserProfile()
	
	local achievements = button_react_util.makeIconButton(nil, "::/gui/menu/icons/achievements_50.tga", function()
		if clickAllowed("TopBar") then
			mainPageParams.commonParams.setPage("Achievements")
		end
	end, getStringForPlatform(pGetText("platform-playstation", "Trophies"), pGetText("platform-xbox", "Achievements"), pGetText("platform-pc", "Achievements")))

	-- TPF3-MP: the Multiplayer button in the top bar, a glyph drawn as the
	-- game's own top-bar icons are (tools/art/icons/menu_icon.py).
	local multiplayer = button_react_util.makeIconButton(nil, "tpf3mp_1::/gui/tpf3mp/icons/menu_multiplayer_50.tga", function()
		if clickAllowed("TopBar") then
			showMultiplayer(nil)
		end
	end, _("Multiplayer"))

	local settings = button_react_util.makeIconButton(nil, "::/gui/menu/icons/settings_50.tga", function()
		if clickAllowed("TopBar") then
			mainPageParams.commonParams.setPage("Settings")
		end
	end, _("Settings"))

	local credits = button_react_util.makeIconButton(nil, "::/gui/menu/icons/credits_50.tga", function()
		if clickAllowed("TopBar") then
			mainPageParams.commonParams.setPage("Credits")
		end
	end, _("Credits"))
	
	local exit = not api.util.isConsoleMode() and button_react_util.makeIconButton(nil, "::/gui/menu/icons/quit_50.tga", function()
		if clickAllowed("TopBar") then
			app.quit(false)
		end
	end, _("Quit Game")) or nil

	local currentMissionIndex = 1
	local allCampaigns = {}
	for __, k in pairs(app.res.campaignRep.getAll()) do
		table.insert(allCampaigns, k)
	end
	if #allCampaigns >= 1 and allCampaigns[1] ~= nil then
		local campaignInfo = savegame_react_util.getCampaignInfoSorted(allCampaigns[1])
		currentMissionIndex = mathutil.clamp(campaignInfo.highestUnlocked + 1, 1, #imagesCampaignAll)
	end

	local campaignCard = menu_icon_react_util.makeCardButton(
		_("Campaign"),
		function()
			if clickAllowed("Cards") then
				mainPageParams.commonParams.setPage("Campaign")
			end
		end,
		function(x : number, y : number) -- onAttention
			mainPageParams.commonParams.triggerBackgroundEvent(api.type.Vec2f.new(x, y))
		end,
		"",
		{ imagesCampaignAll[currentMissionIndex] },
		campaignImgIdx,
		campaignSwapOffset,
		campaignDisplayDuration
	)
	
	local newGameCard = menu_icon_react_util.makeCardButton(
		_("New Game"), 
		function() 
			if clickAllowed("Cards") then
				mainPageParams.commonParams.setPage("NewGame", { map = false })
			end
		end,
		function(x : number, y : number) -- onAttention
			mainPageParams.commonParams.triggerBackgroundEvent(api.type.Vec2f.new(x, y))
		end,
		"",
		imagesClimates,
		climateImgIdx,
		climateSwapOffset,
		climateDisplayDuration
	)

	local loadGameCard = menu_icon_react_util.makeCardButton(
		_("Load Game"),
		function()
			if clickAllowed("Cards") then
				mainPageParams.commonParams.setPage("LoadGame", { map = false })
			end
		end,
		function(x : number, y : number) -- onAttention
			mainPageParams.commonParams.triggerBackgroundEvent(api.type.Vec2f.new(x, y))
		end,
		"",
		imagesLoadGame,
		loadGameImgIdx,
		loadGameSwapOffset,
		loadGameDisplayDuration,
		"small-card, bottom-left" 
	)

	local modBrowserCard = menu_icon_react_util.makeCardButton(
		_("Mod Hub"),
		function()
			if not clickAllowed("Cards") then
				return
			end
			
			if not mod_manager_react_util.checkModManagerAccess(mainPageParams.commonParams.windowContainer:get():getApi()) then
				return
			end

			mainPageParams.commonParams.setPage("ModManager") 
		end,
		function(x : number, y : number) -- onAttention
			mainPageParams.commonParams.triggerBackgroundEvent(api.type.Vec2f.new(x, y))
		end,
		"",
		imagesModHub,
		modhubImgIdx,
		modhubSwapOffset,
		modhubDisplayDuration
	)

	local mapEditorCard = menu_icon_react_util.makeCardButton(
		_("Map Editor"),
		function()
			if clickAllowed("Cards") then
				mainPageParams.commonParams.setPage("NewGame", { map = true })
			end
		end,
		function(x : number, y : number) -- onAttention
			mainPageParams.commonParams.triggerBackgroundEvent(api.type.Vec2f.new(x, y))
		end,
		"",
		imagesMapEditor,
		mapEditorImgIdx,
		mapEditorSwapOffset,
		mapEditorDisplayDuration,
		"small-card" -- TPF3-MP: was "small-card, top-right"; the Multiplayer card has that corner
	)

	-- TPF3-MP: the Multiplayer cards, a column right of the game's own cards:
	-- Multiplayer (connect, create or join) and Join a friend (the window with
	-- the invite first). Each card's line under its title is live, from the
	-- lobby the hook has (lobby.CardLine). The label is the game's own
	-- (menu_icon_react_util.makeCardLabelBottomComponent), with that line in
	-- place of the fixed description.
	local tpf3mpCardLabel = function(title : string, line : function(any) : string) : TreeNodeId
		return builtin.FloatingLayout{
			children = {
				builtin.FloatingLayoutChild{
					item = builtin.ShaderQuad{
						meta = {
							mouseTransparent = true,
						},
						scaling = builtin.type.ImageViewScaling.AutoZoom,
						path0 = "::/gui/menu/design/blackOpaque_effects.tga",
						path1 = nil, -- nrm
						path2 = "::/gui/menu/design/allgreen_masks.tga",
						path3 = "::/gui/menu/design/allwhite_main.tga",
						specularColor = api.type.Vec3f.new(0.42, 0.75, 0.87),
						specularMixAmount = 1.0,
						animatedRippleStrength = 0.18,
						mouseGradDist = 360.0,
						mouseGradBaseStr = 0.34,
						mouseClickStr = 0.51,
						useFullOpacity = false,
						rippleEffectOnClick = true,
						useNormalMap = true,
						mouseGradAdditional = true,
					},
				},
				builtin.FloatingLayoutChild{
					item = builtin.Component{
						layout = builtin.BoxLayout{
							orientation = builtin.type.Orientation.Horizontal,
							children = {
								builtin.Component{
									meta = { class = "title-and-description", },
									layout = builtin.BoxLayout{
										orientation = builtin.type.Orientation.Vertical,
										children = {
											builtin.TextView{
												meta = { class = "font-scale-main-card-title" },
												text = title,
											},
											lobby.CardLine{ line = line },
										},
									},
								},
								gui_react_util.makeHorizontalSpacer(),
							},
						},
					},
				},
			},
		}
	end

	local multiplayerCard = menu_icon_react_util.CardButton{
		bottomComponent = tpf3mpCardLabel(_("Multiplayer"), nil),
		onClick = function()
			if clickAllowed("Cards") then
				showMultiplayer(nil)
			end
		end,
		onAttention = function(x : number, y : number)
			mainPageParams.commonParams.triggerBackgroundEvent(api.type.Vec2f.new(x, y))
		end,
		tooltip = _("Play together online: connect, create a room or join one"),
		images = { "::/gui/menu/images/m02_ingame.tga", "::/gui/menu/images/m07_ingame.tga" },
		initialImageIndex = 1,
		imageSwapOffsetSeconds = 0.41 * 36.0,
		displayDurationSeconds = 36.0,
		class = "small-card, top-right",
		extraChildren = {
			builtin.FloatingLayoutChild{
				h = 0.06,
				v = 0.08,
				item = builtin.ImageView{
					meta = { mouseTransparent = true },
					path = "tpf3mp_1::/gui/tpf3mp/icons/menu_multiplayer_50.tga",
				},
			},
		},
	}

	local joinCard = menu_icon_react_util.CardButton{
		bottomComponent = tpf3mpCardLabel(_("Join a friend"), lobby.joinLine),
		onClick = function()
			if clickAllowed("Cards") then
				showMultiplayer("friend")
			end
		end,
		onAttention = function(x : number, y : number)
			mainPageParams.commonParams.triggerBackgroundEvent(api.type.Vec2f.new(x, y))
		end,
		tooltip = _("Join a friend's room with the invite code they send you"),
		images = { "::/gui/menu/images/m05_ingame.tga" },
		initialImageIndex = 1,
		class = "small-card",
	}

	local multiplayerColumn = vBox({
		vBox({ multiplayerCard }, "level2b"),
		vBox({ joinCard }, "level2b"),
	}, "level1")

	local saveId = api.type.SavegameId.new()
	saveId.saveGameNamespace = app.SaveGameNamespace.getSavegame()
	saveId.saveGameName = "::/savegames/tutorial_temperate_1.sav"
	saveId.path = ""

	local asyncTutorialRef : ReactRefT<Async<SaveGameData>> = react.useRef()
	if asyncTutorialRef:get() == nil then
		asyncTutorialRef:set(app.getSavegameInfo(saveId))
	end

	local dataTutorialState : ReactStateT<SaveGameData> = react.useState()

	react.onStep(function()
		if asyncTutorialRef:get() then
			asyncTutorialRef:get():abort()
		end
		if dataTutorialState:old() == nil and asyncTutorialRef:get() and asyncTutorialRef:get():isCompleted() then
			dataTutorialState:set(asyncTutorialRef:get():get())
		end
	end)

	if not continueCard then
		continueCard = menu_icon_react_util.makeCardButton(
			_("Quickstart Tutorial"),
			function()
				if not clickAllowed("Cards") then
					return
				end

				local displaySaveId = api.type.SavegameId.new()
				displaySaveId.saveGameName = _("Tutorial")

				local data = dataTutorialState:old()
				local infoCopy = api.type.SaveGameDetails.new(data.info)
				local modParams = {
					[""] = {
						["guideSystemConfig.tutorial"] = 2,
					},
				}

				infoCopy.modParams = modParams

				mainPageParams.commonParams.setPage("ProgressPage", {
					saveGameId = displaySaveId,
					mission = "",
					onMount = function()
						app.setWaitForStartReadyGame()
						app.loadGame(saveId, false, infoCopy, true)
					end
				})
			end,
			function(x : number, y : number) -- onAttention
				mainPageParams.commonParams.triggerBackgroundEvent(api.type.Vec2f.new(x, y))
			end,
			"",
			imagesTutorial,
			tutorialImgIdx,
			nil, -- imageSwapOffsetSeconds
			nil, -- displayDurationSeconds
			"small-card, load-savegame-sound" 
		)
	end

	local cardWrap = vBox({
		hBox({ vBox({ -- TPF3-MP: the game's cards, then the Multiplayer column
		hBox({
			newGameCard, 
			modBrowserCard and vBox({
				mapEditorCard,
				modBrowserCard
			}, "level2") or vBox({
				mapEditorCard,
			}, "level2b")
		}, "level1"),
		hBox({
			vBox({
				continueCard,
				loadGameCard
			}, "level2"),
			campaignCard
		}, "level1"),
		}), multiplayerColumn }), -- TPF3-MP
	}, "card-wrap", cardWrapRef)

	local startMenu = vBox({
		menu_util.EditionLogo{
			gameEditionText = gameEditionParamState:old(),
		},
		cardWrap,
	}, "level0", toSelect)
	
	return menu_icon_react_util.makePage(nil, nil, nil, nil, {
		makeFloatChild({0.5, 0.0}, hBox({
			vBox({
				profile,
				GameEditionTextMainPage(gameEditionParamState:old(), nil),
				ModManagementStatusMainPage(),
			}, "left"),
			vBox({
				hBox({
					--notifications, 
					gui_react_util.makeHorizontalSpacer(),
					multiplayer, -- TPF3-MP
					settings, 
					achievements,
					credits,
					exit,
				}),
			}, "right"),
		}, "menu-top-bar")),
		makeFloatChild({0.5, 0.5}, startMenu),
		builtin.FloatingLayoutChild{
			h = 1.0,
			v = 1.0,
			item = builtin.TextView{
				meta = {
					class = "version-string, font-scale-body",
				},
				text = api.util.getBuildVersionString(),
			},
		},
	})
end)
