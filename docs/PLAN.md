# The team's plan, from release day on

What the team works on from Transport Fever 3's release (2026-09-29), in
order, and who takes what. It is the team's plan as agreed on 2026-09-27,
with the review's changes worked in; each change says why. Tick an item
(`[x]`) in the same change that finishes it.

**For agents.** Before starting a task, check it against this page and
[DECISIONS.md](DECISIONS.md). If the task conflicts with either, or with
"Asks that conflict with a decision" below, do not quietly do it and do
not quietly refuse it: say so to the person who asked, name the decision,
and ask how to go on. A decision changes only by a new entry in
DECISIONS.md, never by a task.

## Asks that conflict with a decision

Flag each of these when a task asks for it:

| an ask like | conflicts with | instead |
|---|---|---|
| Steam networking, peer-to-peer, a player hosting, a player's game as the truth | D2, D4: the server orders every turn; no player is the host | Rooms on the project's server |
| Joining a room from the game without the launcher, a Multiplayer entry that works in a game Steam started | D11: the hook runs only in a game the launcher started | An in-game panel for a game the launcher started (D17, after release) |
| Typing or choosing a server, following an invite to its server | D12: one server, built in | `--server` for development only |
| A proxy DLL (`alut.dll`), files in the game's folder, an installer `.bat` that patches the game | D9, D11 | The readable install scripts put in the mod alone; the launcher injects the hook |
| An action sent to other games before its channel was checked (a "strict" flag off meaning "send it unchecked") | Fail closed (AGENTS.md) | Off means the action is refused in a multiplayer game; see Part 3 |
| A speed control in the launcher | Part 2, Dev A: the game's own speed buttons, synced by the server | |
| Long invites or support IDs, or one code for both | D13 | Six-character codes, separate for a room and a session |
| Engine entity IDs on the wire | D8: positions in millimetres, resource names, canonical IDs | |
| A web view for the launcher (Tauri, WebView2, WebKitGTK), or another launcher | D20: a native egui window, in the look of tearded's launcher | Change the look in `theme.rs` and `app.rs` |
| A Dev track, choosing or going back to versions, the room moved into the game | Held until after launch by the owner (D20; D17, D18, D19) | Ask the owner first |
| Writing or changing a decision, or settling a question left open for the owner | The owner decides (AGENTS.md) | A pull request the owner approves |
| Logging an invite code bare | D13: codes cannot be spotted in a log line | `invite=<code>`, which redaction hides |

## Before release (done)

- [x] `Action` enum in `tpf3mp-proto`, one variant per player action,
  positions in fixed point, resource names and canonical IDs, no engine
  IDs; every variant round-trips through postcard (D8).
- [x] `tools/re/make_profile.py` writes a hook profile from a symbol map
  and function names, tested on a synthetic PE.
- [x] The rig: N games on one PC, each with its own data folder and game
  link.
- [x] `tpf3mp-agent collect-logs` zips the game's own logs and crash dumps.
  The launcher's log goes to the server by itself (D10).
- [x] Packaging. *Changed:* no proxy DLL and no install `.bat`. The
  packages hold the launcher, agent, hook and mod, with readable install
  scripts (D9), and the launcher injects the hook (D11).
- [x] The road and track capture ported from TPF2's Lua into
  `mod/tpf3mp_1`, producing `Action::BuildRoad`.
- [x] `tools/probe/check_lua.py` on both probe mods, ready to install.

## Part 1: release day, before anything else

**Go or no-go first.** *Added:* these can end the plan, so they come
before the rest ([DAY_ONE.md](DAY_ONE.md) §0).

- [ ] **The launcher's start.** Started by the launcher (suspended, hook
  loaded, `SteamAppId` set, Steam running), the game runs signed in, with
  the Workshop, and does not restart itself through Steam (which would
  lose the hook). DAY_ONE.md §5.
- [x] **Anti-tamper.** `binary_survey.py`: packer sections, entropy, TLS
  callbacks. TPF2 had SteamStub only; Denuvo or a VM protector changes
  the native plan.
- [x] **Archive the build.** Steam build ID, depot manifests, executable
  hashes, and a private copy of each executable (DAY_ONE.md §1).
- [ ] **Patch duty.** Name one person who, on every game patch, reruns the
  naming, the build diff and the profile, and holds the release until the
  hook matches. Expect a day-one patch.

*Added* (2026-09-27, from third-party mods made for build 40391,
[investigation/TF3_MODS_2026-09-27.md](../investigation/TF3_MODS_2026-09-27.md)):
our mod and both probes are TPF2 mods and will not load in TF3 as they
are.

- [x] Before release day: `mod/tpf3mp_1` in TF3's layout (`mod.json`,
  `_content.json`, `_metadata/modinfo.json`, `content/`), loaded by a
  game bar plugin, with the Lua side of the link to the hook
  (`tpf3mp/bridge.lua`, HOOKS.md "The Lua side"). How the rest of TpF2
  Multiplayer's mod comes over: [PORTING_TPF2MP.md](PORTING_TPF2MP.md).
- [x] Before release day: `script_api_dump` and `determinism_probe` in
  the same layout (`tools/probe/tf3`), run from a GUI `onStep` plugin and
  logging through `debugPrint` where `io` is missing; the run script's
  state has a dump of its own. The TPF2 probes stay as the fallback.
- [x] Before release day: a tool for each release-day check
  (`tools/dayone/dayone.py`, DAY_ONE.md "Release day, step by step").
- [x] Read the game's `.tl` sources and `.d.tl` API declarations before
  running the probes; list every `api.cmd.make*Cmd` and the tool that
  sends it.

Then:

- [x] `binary_survey.py`: the Lua version, whether RTTI and `__FUNCSIG__`
  strings are present. (*Changed:* no proxy DLL to find, D11.)
- [x] `tools/tpfre` (D14; `name_functions.py` to cross-check): TF3's
  equivalents of TPF2's `GameSim::Step`, `CGame::Step`,
  `CommandList::Add`, save and load, into the recon log. TF3's names may
  differ from TPF2's. *Added:* found for build 40408 from RTTI and the
  assert names, and cross-checked by source file
  ([TPF3_RECON_2026-09-29.md](../investigation/TPF3_RECON_2026-09-29.md)).
- [ ] `script_api_dump`: both state dumps (game script and GUI); every
  `api.cmd.make.*` factory; whether `io`, `os`, `require` and `load`
  exist.
- [ ] `determinism_probe` on two games from one save: 60 in-game days
  without input, then with scripted input; `compare_runs.py`, the first
  differing step per lane.
- [ ] A Windows save loaded on Linux and the reverse: does it load, do the
  lanes match.
- [x] Where the game writes its log and crash dumps, and where it loads
  mods from (DAY_ONE.md, release-day order 6 and 7).

## Part 2: the first playable room

Dev A (the hook; on everyone's critical path, so Part 3 work moves off
Dev A where it can):

- [ ] Find `GameSim::Step`, `CGame::Step`, `CGameTime::GetSpeed`,
  `UI::CMenuUI::StartSavegame`, `UI::CMenuUI::CreatePage` (or TF3's
  names for them).
- [ ] Detour the step: `Session::before_step` before each step,
  `after_step` after. Done when the game holds while the agent withholds
  a turn and goes on when it releases it.
- [ ] Detour the command queue's add: a road build is cancelled locally
  and its payload goes to `Session::command`. Done when a click builds
  nothing locally and the command shows in the agent's log.
- [ ] `Game::save(file)` calls the native save; the file loads in a fresh
  game.
- [ ] `StepGate::Load`: load the named save from native code, then
  `loaded(next_step)`.
- [ ] Speed and pause follow the room: the game's own buttons go through
  `Session::command` (`Control::Speed`). The launcher has no speed
  control. *Added:* the speed row is a script recipe a mod can replace,
  so this may need no native code; the pause-or-cycle key must be caught
  too.
- [ ] *Added:* whether the stock tools send their commands through
  `api.cmd.sendCommand`. If they do, the caller-RVA filter cannot tell a
  click from our replay (HOOKS.md), and the hook needs another way to
  tell them apart.

Dev B (the game's side of the bridge):

- [ ] `Tf3Game` in `tpf3mp-hook`, implementing `Game`:
  - `apply(event)`: decode to an `Action` and hand it to the Lua mod,
    which runs it with `api.cmd`;
  - `lanes()`: vehicle count, vehicle positions to 1 m, edge geometry to
    0.1 m, the construction list, town building counts, money per player,
    people count;
  - `save(file)`: Dev A's native save; `notice()`: to the Lua mod.
- [ ] Lanes for TF3's new systems, each added after measuring what it
  costs to read at every checkpoint (*changed:* these were mixed into the
  method list): warehouse stock and spoilage; vehicle wear; town
  happiness, pollution, noise and reputation; company rank, subsidies,
  contracts and built landmarks; time of day, if it affects the
  simulation.
- [ ] Report the game build and active mods (`ContentManifest`), so
  `--game-build` and `--mods` are no longer needed.
- [ ] `save_check::check_lua_data` on a fetched save at `Done::Fetched`
  (`crates/tpf3mp-agent/src/bridge.rs`); refuse to load one that fails.
- [ ] Two games through the real server with the rig, two players.

Dev C (building):

- [ ] Roads and track: `Action::BuildRoad`/`BuildTrack` into a proposal on
  the receiving game, snapping to nodes within 0.5 m, else new ones
  (BUILDING.md, "Resolving a vertex").
- [ ] Level crossings take the rail's height; a bridge over a road never
  becomes a crossing (both were TPF2 bugs).
- [ ] Bulldoze: find the edge by its endpoints plus a 14 m search along
  the centreline.
- [ ] *Changed:* an in-game Multiplayer panel for a game the launcher
  started: the room, its players, chat and whether the worlds match. Not
  a way to join without the launcher, and no Steam networking (D2, D11).
  Moving the lobby itself into the game (D17) is held until after launch
  (D20).

**Test A** (the gate to Part 3):

- [ ] Two players each build 20 roads and tracks, crossing each other's;
  lanes match at every checkpoint.
- [ ] 60 in-game days without divergence.
- [ ] *Added:* a third player joins mid-game, and a player rejoins after
  killing their game; lanes still match. Late join, rejoin and repair
  all load a save and then apply turns. If a loaded game walks its lists
  in another order than the running one (TPF2's hot-join desync), all
  three break, so this is found here, not in Part 3.
- [ ] *Added:* if a list's order differs, sort every engine list walked
  during a step by entity ID (moved here from Part 3).

*Added:* **Test A, automated** ([REGRESSION.md](REGRESSION.md)): scripted
scenarios that build, buy, make lines and assign vehicles, two or more
games to a room, checked at every step of the script.

- [x] The harness, `tpf3mp-regress`, playing its scenarios on a model of
  the game through the whole stack, in `ci` and `acceptance`.
- [ ] The hook answers the harness's `Observation` from the game.
- [ ] The hook's test mode walks a scenario file, and scenarios get a file
  format.
- [ ] Rooms start from a fixture save everyone has.
- [ ] Measure how fast the game steps at top speed, minimized, and with
  drawing skipped; the budget is 10 minutes a platform.

## Part 3: every action

For each action below:

1. Dump the proposal the game makes for a click and the one the replay
   makes for the same build, and diff them field by field.
2. Add a `strict_<action>` flag, off by default. **Off means the action is
   refused in a multiplayer game**: the hook cancels it and the player is
   told it is not available yet. It is never sent unchecked (fail closed).
3. A 2-player sandbox playtest without divergence. *Added:* and a
   regression scenario for the action ([REGRESSION.md](REGRESSION.md)).
4. Turn the flag on.

Dev A (moves to whoever finishes Part 2 first where Dev A is still on
the hook):

- [ ] Traffic light phases: the intersection by position, the full phase
  table.
- [ ] Line priority per line; loading rules per station or line.
- [ ] The new click-to-assign line creation: a new UI flow, captured from
  scratch.
- [ ] Buying a vehicle: the same key on every game, bound in creation
  order, not by entity ID. Selling, replacing, cloning (joining the
  original's line on the same step).
- [ ] Lines: create, edit stops and platforms, delete; assign a vehicle.
- [ ] Vehicle colour and name, with no echo between games (TPF2 froze on
  100,000 colour commands).

Dev B:

- [ ] Subsidies, contracts and loans, tied to a company; the server
  decides when two companies want the same subsidy on the same turn (the
  first in order gets it). A loan is never replayed twice for its taker.
- [ ] Headquarters upgrades; prospecting (the industry at the same place,
  with the same ID, on every game); boosting industry.
- [ ] Greening and other new brushes, terraforming, terrain paint, the
  asset brush.
- [ ] Companies: create, switch, dissolve; owners move with the company,
  and no money is created in the switch.
- [ ] Roadside stops and signals, the side included, never rebuilding an
  edge a line runs on.
- [ ] The room's required mods from Mod Hub IDs; a missing mod is
  installed from Mod Hub, never received from another player.
- [ ] *Added, open for the team:* a rule for mods that send commands from
  the GUI (GW Big City and Startup Fortune do, once per save). Every
  player's game sends them: forwarded, the room gets one city per player;
  dropped, the worlds differ.

Dev C:

- [ ] Roads with lane connections, tram lanes and ramps; track types and
  underground segments.
- [ ] Landmarks, built once: the second on the same turn is refused and
  pays nothing.
- [ ] Warehouses, specialised terminals, maintenance facilities, noise
  barriers, pollution plants through the construction replay.
- [ ] Stations and depots: first place one on flat ground on two games
  and diff every edge's height. Module edits and upgrades.
- [ ] Bulldozing a construction, charged on every game, with no demolition
  beyond what was asked (the TPF2 "heal" bug).
- [ ] The in-game Multiplayer window: players, speed, chat, and whether
  the worlds match.

## Part 4: after launch

- [ ] Linux: the hook and profile, passing Test A.
- [ ] Mixed-platform rooms, only for the pairs whose determinism matched.
- [ ] macOS: *added:* not at launch. The launcher refuses to start the
  game there (`tpf3mp-launch`), though mixed platforms remain a
  requirement (D2).
- [ ] A `Tf3Canonical` rule set on the server from `tpf3mp-canon`:
  validate, apply, save and restore (D6).
- [ ] Big maps: *open:* say what is wanted; [BIGMAPS.md](BIGMAPS.md) is
  what is known.
- [x] The auto-updater (D7; needs the owner's update key, OPERATIONS.md
  "Before the first release").
- [x] The server: deployed beside tf2mp-relay (OPERATIONS.md).
- [x] Building into the TF2 launcher: *decided by the owner* (D20): our
  own native window, drawn in the exact look of tearded's launcher as
  ported to TF3, with its release notes. No web view.
- [ ] *Added:* the minimap ([MINIMAP.md](MINIMAP.md)): TPF2 Big Maps'
  minimap for TF3, as a game bar plugin in the mod. First script only
  (towns, industries, network, stations, camera, click to move, companies
  and industry types); then the terrain picture rendered by the hook; in a
  room, other players' cameras and builds.
- [ ] *Held* (D17, D20): the room in the game. Connecting, rooms, the
  lobby and chat in an in-game panel. The owner decides after launch.
- [ ] *Held* (D18, D19, D20): choosing versions and tracks, and a Dev
  track of untested builds. The owner decides after launch, once `dev`
  takes reviewed pull requests only.
