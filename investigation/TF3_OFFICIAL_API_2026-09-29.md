# What Urban Games' official API reference tells us -- 2026-09-29

On release day Urban Games' modding wiki went live at
`wiki.transportfever3.com`, and with it a generated script reference at
`wiki.transportfever3.com/script-doc/` (linked from the wiki's
[Scripting API](https://wiki.transportfever3.com/doku.php?id=modding:scripting:api)
page as "Scripting Reference"): one page per module, typed like the game's
own Teal (`.tl`) declarations. It is the reference the probe
(`tools/probe/tf3`) was built to reconstruct, and it now exists.

Findings here are labelled **DOCUMENTED**: read from that reference, which
is Urban Games' own, not inferred and not a mod's usage. It is stronger
than the **REPORTED** label of
[TF3_MODS_2026-09-27.md](TF3_MODS_2026-09-27.md) and
[TF3_MODHUB_SCRIPT_MODS_2026-09-29.md](TF3_MODHUB_SCRIPT_MODS_2026-09-29.md),
which read third-party mods. It is still not **MEASURED**: we have not run
any of it. The reference notes it "is not yet complete"; a later build may
add or change entries. Only names and type signatures are recorded here,
as facts; the reference's own text is not copied, and the mods and the
reference are not in the repository.

## The headline: players and companies are a documented command API

The open question of whether TF3 lets a script make and switch players
(companies) -- and the worst case of having to build them natively -- is
settled. It does, through two documented commands (`api/cmd.html`):

```
api.cmd.makeGameAddPlayerCmd(name: string, color: Vec3f)
    : Command<GameAddPlayerCommandData>
api.cmd.makeEntitySetPlayerCmd(entity: Engine.Entity, player: Engine.Entity)
    : Command<EntitySetPlayerCommandData>
```

These are the TF3 equivalents of TPF2's `game.interface.addPlayer()` and
`game.interface.setPlayer()` (used by `tf2mod`'s companies mode), but now
**commands**: they go through the command queue, so the room can order them
like any other action, rather than the native, assert-bypassed
`setPlayer` binding TPF2MP patched (HOOKS.md, "UI patches for companies
mode"). `game.interface` does not appear in the reference at all.

A company is a first-class game mechanic, documented under
`content/game_mechanics/company/`: `CompaniesState` and `CompanyState`
(company entity, rank, consumed permits, pending prospections),
`company_util` with `externalGetCompanyState(entity)` and
`externalGetCompaniesState()`, ranks, permits and prospection. So a room
where players "each have their own company" rests on the game's own
system, not one we simulate.

**Consequence for the plan.** PLAN.md Part 3's "Companies: create, switch,
dissolve" has a documented, in-script path: create with
`makeGameAddPlayerCmd`, assign ownership with `makeEntitySetPlayerCmd`,
read state with the company util. None of it needs native player
construction. It is still the owner's to decide whether the project ships
companies mode, and it still needs measuring: whether `makeEntitySetPlayerCmd`
on a vehicle works (TPF2's crashed, vehicles.lua:1176) and whether the
stock UI shows other companies' entities as editable (companies mode's ten
UI patches on TPF2). Those go on the release-day list, not into a decision.

## The command surface: 61 factories

Every player or script action is a command from an `api.cmd.make*Cmd`
factory (`api/cmd.html`). The full list, with the argument types the
reference gives, is the "verbs" our capture and the server's rules are
designed against; it replaces guessing from TPF2's set. The `Command<...>`
result type is left off each line below.

**Players and companies**
```
makeGameAddPlayerCmd(name: string, color: Vec3f)
makeEntitySetPlayerCmd(entity: Engine.Entity, player: Engine.Entity)
```

**World building and terrain**
```
makeWorldBuildProposalCmd(proposal: Proposal, context: any,
    ignoreErrors: boolean, playerInitiated: boolean, doDust?: boolean)
    -- overloaded: a second signature takes a SimpleProposal in place of Proposal
makeWorldReplaceTerrainCmd(map: GameMap, terrainConfig: BaseConfig.Terrain,
    seedText: string, worldEntity: Engine.Entity, keepAssets: boolean)
makeWorldSetBulldozableCmd(entity: Engine.Entity, bulldozable: boolean)
makeWorldChangeWindCmd(emissionGridEntity: Engine.Entity, wind: Vec2f)
```

**Vehicles**
```
makeVehicleBuyCmd(playerEntity: Engine.Entity, depotEntity: Engine.Entity,
    tvc: TransportVehicleConfig)
makeVehicleReplaceCmd(vehicleEntity: Engine.Entity, tvc: TransportVehicleConfig)
makeVehicleSellCmd(vehicleEntities: {Engine.Entity})
makeVehicleReverseCmd(vehicleEntity: Engine.Entity)
makeVehicleSendToDepotCmd(vehicleEntity: Engine.Entity, sellOnArrival: boolean,
    jumpToDepoEntity: Engine.Entity)
makeVehicleSetLineCmd(vehicleEntity: Engine.Entity, lineEntity: Engine.Entity,
    stopIndex: integer)
makeVehicleSetManualDepartureCmd(vehicleEntity: Engine.Entity, manual: boolean)
makeVehicleTryToDepartCmd(vehicleEntity: Engine.Entity)
makeVehicleSetStoppedByUserCmd(vehicleEntity: Engine.Entity, stopped: boolean)
makeVehicleSetModifiersCmd(vehicleEntity: Engine.Entity,
    modifiers: Engine.Component.TransportVehicle.Modifiers)
makeCustomVehicleCreateOrUpdateCmd(vehicleEntity: Engine.Entity, carrier: Carrier,
    vehicles: {{integer, boolean}}, movePathAircraft: ...|nil)
```

**Lines**
```
makeLineCreateCmd(name: string, color: Vec3f, player: Engine.Entity,
    line: Engine.Component.Line)
makeLineUpdateCmd(lineEntity: Engine.Entity, data: Engine.Component.Line)
makeLineDestroyCmd(lineEntity: Engine.Entity)
```

**Time and simulation (the server's to hold, PLAN Part 2)**
```
makeGameSetSpeedCmd(speedup: integer)
makeGameSetCalendarSpeedCmd(millisPerDay: integer)
makeGameSetDateCmd(date: Date)
makeGameSetTimeOfDayCmd(timeOfDaySec: integer)
makeGameSetCloudCoverageCmd(cloudCoverage: number)
makeGamePerformSimulationStepsCmd(amount: integer)
```

**Towns**
```
makeTownCreateCmd(towns: {TownInfo})
makeTownDestroyCmd(townEntity: Engine.Entity)
makeTownDevelopAtCmd(position: Vec2f, developStreets: boolean, upgradeBuildings: boolean)
makeTownSetDevelopmentActiveCmd(entity: Engine.Entity, developmentActive: boolean)
makeTownSetInitialLandUseCapacitiesCmd(entity: Engine.Entity, landUseCapacities: {integer})
makeTownUpdateSizeCmd(townEntity: Engine.Entity, sizeFactors: {number},
    updateAllBuildings: boolean)
makeTownUpdateCargoNeedsCmd(entity: Engine.Entity, cargoNeeds: {{CargoTypeId}},
    updateTownBuildings: boolean)
makeTownConnectWithIndustriesCmd(townEntities: {Engine.Entity},
    connections: {{integer, integer}}, keep: boolean)
makeTownCustomDistributionWeightsCmd(townEntitiy: Engine.Entity,
    levelToCapacityDistributionWeights: {{number}})
makeTownBuildingSetBlockedDevelopmentCmd(townBuildingEntity: Engine.Entity,
    blockedDevelopment: boolean)
```

**Industries and stocks**
```
makeIndustrySetManualDevelopmentCmd(industryEntity: Engine.Entity, manual: boolean)
makeIndustrySetDespawnTimeCmd(industryEntity: Engine.Entity, timeStamp: integer)
makeCreateIndustryExtendProposalCmd(extendConstruction: Engine.Entity,
    numTrySlots: integer, numKeepSlots: integer)
makeStockListSetModifiersCmd(industryEntity: Engine.Entity,
    modifiers: Engine.Component.StockList.Modifiers)
makeStockListSetStocksCargoTypeCmd(stockListEntity: Engine.Entity,
    stockIds: {StockId}, cargoType: CargoTypeId)
makeStockListDiscardCargoCmd(stockListEntity: Engine.Entity, stockIds: {StockId},
    remainingTimeToDelivery: number)
makeStockSetCargoAmountCmd(entity: Engine.Entity, stockId: StockId, amount: integer,
    cargoType: string)
```

**Money, journal and maintenance**
```
makeJournalBookAssetCmd(player: Engine.Entity, entry: JournalEntry, position: Vec3f)
makeJournalLogEntryCmd(entity: Engine.Entity, logNamesAndValues: {{string, integer, boolean}})
makeJournalClearAllCmd()
makeClearLogbooksCmd(...)
makeMaintenanceCostUpdateCmd(...)
```

**Naming, colour and emissions**
```
makeEntitySetNameCmd(entity: Engine.Entity, name: string, forceSameEntity: boolean)
makeEntitySetColorCmd(entity: Engine.Entity, color: Vec3f)
makeEntitySetEmissionsCmd(entity: Engine.Entity, noisePower: number,
    pollutionPower: number, radius: number, pollutionRadius: number)
makeComponentExchangeCmd(entity: Engine.Entity, component: Engine.Component)
```

**Scripting, custom entities, animals and people**
```
makeScriptingSendEventCmd(src: string, id: string, name: string, param: any)
makeCustomEntityCreateCmd(modelId: integer)
makeCustomEntityDestroyCmd(entity: Engine.Entity)
makeCustomEntityUpdateStateCmd(entity: Engine.Entity, customState: ComponentCustomState)
makeCustomEntityUpdateTransformationCmd(entity: Engine.Entity, transf: Mat4f)
makeSimPersonSetStateCmd(entity: Engine.Entity, simPersonState: integer)
makeAnimalSpawnAtCmd(fileName: string, position: Vec2f, lookAt: Vec2f)
makeAnimalSetStateCmd(animalEntity: Engine.Entity, movementType: integer,
    targetChangedElapsed: number, invalidTileElapsed: number,
    movementSpeed: number, angularSpeed: number)
```

## What it confirms, corrects and adds

- **`makeWorldBuildProposalCmd` takes a `playerInitiated` argument**
  (`proposal, context, ignoreErrors, playerInitiated, doDust?`; corrected
  from the on-disk `.d.tl` in TPF3_RECON_2026-09-29.md -- the web
  reference's order was wrong). It is overloaded: a second signature takes
  a `SimpleProposal`. The Mod Hub tools call it `(proposal, nil, true, true)`
  (TF3_MODHUB_SCRIPT_MODS_2026-09-29.md), so they set it to true. It is
  not the flag that tells a player's build from our replay: the game's
  scripts read it (a town's reputation changes only for player-initiated
  builds), so a replay must carry the original value
  (TPF3_RECON_2026-09-29.md, "The scripts, data and log").
- **`makeScriptingSendEventCmd(src, id, name, param)`**: the four
  arguments are named. Mods pass `src = ""` and use `id` as the channel
  (TF3_MODHUB_SCRIPT_MODS_2026-09-29.md).
- **`makeGameSetSpeedCmd(speedup: integer)`** is a command, so the room's
  pace is set through the queue, not only through the game-bar recipe
  (TF3_MODS_2026-09-27.md item 4). `makeGamePerformSimulationStepsCmd(amount)`
  advances the simulation by a count -- a lockstep-shaped primitive to
  understand before relying on it.
- **`makeLineCreateCmd(name, color, player, line)`** confirms the
  signature `engine.lua` was written against, and that a line names its
  player.
- **`makeVehicleBuyCmd`, `makeJournalBookAssetCmd`** take a
  `playerEntity` / `player`: ownership and money are per player entity, as
  the mods showed.
- **Line component**: `makeLineUpdateCmd` takes an
  `Engine.Component.Line`, and Timetables sets its `reservationPriority`
  (line priority, PLAN Part 3) through it
  (TF3_MODHUB_SCRIPT_MODS_2026-09-29.md).

## Cross-check against our mod

`mod/tpf3mp_1/.../engine.lua` reads the world to capture a build. Its
names against the reference:

- **Confirmed by the reference:** `makeLineCreateCmd`,
  `makeWorldBuildProposalCmd`, `makeEntitySetNameCmd`,
  `makeGameSetSpeedCmd`, and the per-player line, the `Proposal`.
- **The street graph:** `StreetSystem` documents
  `getNode2StreetEdgeMap` and `getNode2TrackEdgeMap`, the names
  `engine.lua` uses, besides `getNodeSegments`, `getNodeStreetSegments`,
  `getNodeTrackSegments` and `getEdgeObject2EdgeMap`.
- **Corrected: roads and tracks are one kind of edge.** A proposal's
  `SegmentAndEntity.comp` is a `BaseEdge` with `roadType`
  (`api.type.RoadType.STREET` or `TRACK`), `roadTemplate` and `roadStyle`
  (resource names); its `streetEdge` (`BaseEdgeStreet`) holds only
  precedence, and there is no track edge. `api.res` has `streetTemplateRep`
  and `streetStyleRep` but no `streetTypeRep` or `trackTypeRep`. The
  capture read TPF2's `streetEdge.streetType`, so it would have failed on
  the first road. It now reads `roadType`, `roadTemplate` and `roadStyle`
  (falling back to TPF2's names), refuses a TF3 edge whose network or names
  it cannot read, and the action schema (version 2) carries the style.
  `api.type.transformator` numbers road types the other way round
  (`0 track, 1 street`), so compare against the `RoadType` values, never a
  number.
- **Vehicle positions:** `api.engine.util.transport.getPosition(vehicle)`;
  the determinism probe reads it first.

## More from the reference and the release download

- **The release builds** (the wiki's release notes): **40408 on Steam,
  40393 on Epic and GOG**, both September 29. The stores ship different
  builds, so the hook needs a profile for each, and the day-one tools run
  against both executables. The mods of TF3_MODS_2026-09-27.md were made
  for 40391.
- **The step counter.** `GameTime.updateCount` counts simulation updates
  and stops while paused; `tickCount` counts frames, paused or not.
  `TickEpoch` stamps each entity with the update, the command within it
  and a sub-index. The determinism probe now labels its samples with
  `updateCount` where the API has it, so the two games no longer need 1x,
  and falls back to learning the step from the game time.
- **Game scripts exist, and run much of the economy.** A `GameScript`
  component holds each script's state; `content/scripts/gamescript` types
  `update`, `postUpdate`, `handleEvent`, `guiUpdate` and `guiHandleEvent`.
  Companies, finance and loans, subsidies, notifications and town growth
  are Teal modules whose state lives there, and scripts price tickets
  (`CalcTicketPriceEvent`, `CalcTicketPriceCargoEvent`) and see arrivals
  (`ArriveAtStop`). Lua's float formatting, `math.random` and `pairs`
  order therefore reach the simulation, and the lanes must cover
  game-script state. The manual's game-script page is still TPF2's ("not
  yet adapted for TF3").
- **Money is integer:** `Account.balance` and `loan`. `PlayerOwned`
  marks an entity bulldozable only by its owner.
- **Script events as one action.** Script mechanics and mods reach the
  engine through `makeScriptingSendEventCmd`. One portable action carrying
  a script event, its Lua parameter checked like save data, could cover
  loans, prospecting, subsidies and mods such as Timetables. It widens what
  a room accepts, so it is the owner's to decide.
- **`api.modhub`** names mods by a `ModId` and knows install states: a
  source for the `ContentManifest`'s active mods.
- **The shipped definitions.** The release download carries the game's
  own typed API, `api/tealdef/*.d.tl`. It confirms `ResName` is a plain
  string (as the capture requires), `RoadType`, the edge's road fields and
  `updateCount`. `api_def.d.tl` loads `api.cmd`, which had not yet
  downloaded when this was written: whether `playerInitiated` is in the
  shipped signature is still to see.
- **Content archives.** `base/content/*.zip` are zips whose local file
  headers start `UG\x03\x04` instead of `PK\x03\x04`; the central
  directory is a plain zip's and entries are stored. Standard zip readers
  refuse them; `dayone.py scripts` reads the scripts inside them.

How our action schema maps to the commands:

| our action | the command |
|---|---|
| `BuildRoad`, `BuildTrack`, `BuildConstruction`, `Bulldoze`, `PlaceStop` | `makeWorldBuildProposalCmd` |
| `Terraform` | a proposal too, to confirm |
| `BuyVehicle`, `SellVehicle` | `makeVehicleBuyCmd`, `makeVehicleSellCmd` (and `makeVehicleReplaceCmd`) |
| `CreateLine`, `EditLine` | `makeLineCreateCmd`, `makeLineUpdateCmd`, `makeLineDestroyCmd` |
| `AssignLine` | `makeVehicleSetLineCmd` |
| `CompanyOp` | `makeGameAddPlayerCmd`, `makeEntitySetPlayerCmd`, company scripts |
| speed and pause | `makeGameSetSpeedCmd` |

Not yet in the schema, and so refused in a multiplayer game until each has
an action and a checked channel (PLAN Part 3): the vehicle commands besides
buy, sell and line; names and colours; stocks and warehouses; industry
expansion and development; script events; the journal and logbooks;
`makeWorldSetBulldozableCmd`. A multiplayer game never sends the
map-editor and debug commands (towns, terrain, animals, date, time of day,
weather).

## Measured on the release build, 2026-09-29 evening

From Steam build 40408 as installed (`investigation/dayone-2026-09-29/`),
read without starting the game:

- **Go/no-go: GO.** SteamStub alone, as on TPF2; `.text` is not
  encrypted on disk (entropy 6.49); 3 TLS callbacks, as TPF2; the game
  still imports `alut.dll`.
- **Names.** 16 functions keep a `__FUNCSIG__` name (TPF2: 20,746), but
  `__FILE__` survives (858 source files, 6,204 functions tied directly)
  and so does RTTI (7,947 vtables). `tpfre match` carries TPF2's names
  over by shared assert strings, RTTI slots, the call graph and
  source-file order: 3,942 named functions, among them `GameSim::Step`
  (`0x159390`), `CGame::Step` (`0x11f3b0`), `CGame::RunGameSimLoop`
  (`0x11e210`), `CommandList::Add` (its lambda, `0x9d23c0`) and
  `UI::CMenuUI::CreatePage` (`0x6a2ee0`), each in the file TF3 names for
  it. Not found yet: `CMenuUI::StartSavegame`, `CGameTime::GetSpeed`.
- **`playerInitiated` is the fourth argument, not the fifth.** The
  shipped `api/tealdef/api/cmd.d.tl` declares
  `makeWorldBuildProposalCmd(proposal, context, ignoreErrors,
  playerInitiated, doDust?)`, for a `Proposal` or a `SimpleProposal`; the
  wiki's order is wrong. The mods' `(proposal, nil, true, true)` therefore
  sets `playerInitiated`.
- **Who builds through script, and with what flag.** The stock
  construction tool (`gui/construction/construction.tl`) sends its
  proposal with `playerInitiated = true`, as do the entity windows (bridge
  and tunnel, double slip switch); the game's own scripts (industries,
  companies, missions, the map editor) send `false`. So the flag tells a
  player's build from the game's, but not from a mod's.
- **Streets and tracks are built natively.** The street and track tools
  are native builder actions (`ConstructionActionStreetEdgeBuilder`,
  `TrackEdgeBuilder`) that the GUI script only configures, so road and
  track capture still needs the native hook on `CommandList::Add`;
  constructions placed through the script can be captured in Lua.
- **Speed is a script command.** The game bar sends
  `makeGameSetSpeedCmd` (`gui/main/game.tl`), which the room can take over
  in Lua.
- **Scripts.** 6,041 script files, 5,356 of them packed in the `UG`
  archives; 311 places send commands (`5-scripts.md`).

## Measured in the running game, 2026-09-29 evening

The three probe mods, active in a new game on build 40408, logged to
`stdout.txt` (`investigation/dayone-2026-09-29/probe/`):

- **Lua 5.2**; no `io`, so a mod cannot write files (the probes fell back
  to the log as designed); `os` has only `clock`, `date`, `difftime`,
  `getenv` and `time`; `load` and `debug` exist, `loadstring`, `dofile`
  and `collectgarbage` do not; `package.path` is nil.
- **Floats print as in TPF2**: `%.0f` rounds ties to even.
- **`math.random` is MT19937**: seeded with 1 it gives 0.41702199843712
  first, the Mersenne Twister's value, so games that seed it alike draw
  alike.
- **`pairs` order** is stable within a state across repeated
  constructions.
- **The step counter works**: the determinism probe's header says
  `stepTime=updateCount`, and it sampled every 100 updates, game time 200
  per update (5 a second at 1x, as TPF2).
- **Lanes**: vehicles and positions read; edges and money read `err`
  with TPF2's calls. The probe now reads `BASE_EDGE.position0/position1`
  and each player's `ACCOUNT.balance` (`fix/detprobe-tf3-lanes`).
- **`CMenuUI::StartSavegame`**: its lambdas keep the name in RTTI, and
  its log line, now "Game initialization is already active!", is used
  only by `0x6a2880` (in `menuui.cpp`, with "Preparing to load game" and
  "Starting Game..."): the load entry, to confirm with its callers.
- **A crash while loading** a game with the probes active (16:49 UTC,
  `crash_dump/b4b39727-…_0.txt`, last line an error naming
  `WithComponentParams`, a stock UI recipe the log warns about from the
  main menu on). The next game, with the same probes, ran past step 1,200.
  Not attributed; watch for it again.

## The modding manual

Documented in the wiki's modding manual:

- **Local mods** go to `<Steam>/userdata/<Steam ID>/3493540/local/staging_area/`,
  as the installer puts them. A mod is known by its `modId`, not its
  folder; when two share one, the staging area wins over the manual
  installation directory, which wins over subscribed mods.
- **`modId`** takes `a-z`, `0-9` and `_`, and TF3 drops TPF2's version
  suffix; ours, `tpf3mp_1`, is still valid.
- **`mod.json`** gains `visible` and `cosmetic`. `cosmetic` is the
  author's claim that a mod leaves the simulation alone; a room never takes
  it as a reason to let content differ.
- **Debug aids** (the in-game tools page, marked possibly TPF2's): debug
  mode (`debugMode` in `settings.lua`, or the advanced settings), a Lua
  console on the key below Esc printing to `stdout.txt`, and simulation
  speed up to 32x. DAY_ONE.md step 7 uses them.

## Still to do on release day

- In the game, confirm the road model the capture now reads: `roadType`,
  and `roadTemplate` and `roadStyle` as plain strings; and whether a
  street's bus lane and tram track are part of its template (the capture
  records none, a guess to replace).
- Measure the companies path: `makeGameAddPlayerCmd`,
  `makeEntitySetPlayerCmd` on stations, lines and a vehicle, and whether
  the stock UI gates other companies.
- Measure `makeWorldBuildProposalCmd`'s `playerInitiated`: whether it
  distinguishes a player's build from a script's, and so whether the
  capture can tell them apart in script (TF3_MODS_2026-09-27.md item 3).
- Run the API dump probe (`tools/probe/tf3`) and diff it against this
  reference, to find what the reference leaves out ("not yet complete")
  and what a build changed.
- Whether `makeGamePerformSimulationStepsCmd` (under `api.cmd.debug`)
  works in the release build: it would let the regression harness step a
  real game without drawing (REGRESSION.md).
- Run the day-one steps on the Epic/GOG build too.
