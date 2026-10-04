# Release-day investigation

Transport Fever 3 releases on 2026-09-29. Until then, nothing in this project
has been checked against the real game. This plan settles the **[needs game]**
questions in [ARCHITECTURE.md](ARCHITECTURE.md), in order of how much they can
change the design. Results go into `investigation/TPF3_RECON_<date>.md`, and
each finding carries one label:

> **The static part is done** (2026-09-29):
> [investigation/TPF3_RECON_2026-09-29.md](../investigation/TPF3_RECON_2026-09-29.md).
> The build is a go (SteamStub only, `.text` readable, no Denuvo); the
> command queue, sim clock, a ready-made lockstep step-budget primitive,
> and the player/company commands are all located (RVAs). What remains is
> the in-game measurement (§4-§7 here) and the reconciliations that page
> lists. Note: TF3's release build has almost no `__FUNCSIG__` names, so
> naming leans on RTTI, source-file asserts and luabridge, not the TPF2
> funcsig pipeline (§2, §3 below).

- **CONFIRMED**: decompiled or named by the binary's own strings, and
  consistent with live behaviour.
- **MEASURED**: observed in the running game.
- **INFERRED**: placed by elimination only.
- **REPORTED**: relied on by third-party mods made for a pre-release
  build, not yet seen by us. What the first such mods show, for build
  40391, is in
  [investigation/TF3_MODS_2026-09-27.md](../investigation/TF3_MODS_2026-09-27.md).

## Release day, step by step

`tools/dayone/dayone.py` runs each check below and writes its report into
`investigation/dayone-<date>/`, ending in a verdict: **GO**, **CHECK** (a
person looks) or **STOP** (the plan does not hold as it stands). It needs
Python and nothing else; it reads the game, never starts or changes it.
Build `tpfre` once before: `cargo build --release --manifest-path
tools/tpfre/Cargo.toml`.

1. **Go or no-go** (§0, §5):
   - `dayone.py gonogo`: the executable's sections, imports and TLS
     callbacks. SteamStub alone is GO, as on TPF2 (whose executable reads
     GO); a known protector is STOP; packed code under another name is
     CHECK.
   - Start the game from the launcher in a room, then `dayone.py
     check-launch`: the game's parent process (the launcher: GO; Steam:
     STOP, the hook is lost), and the end of `hook.log`. In the game: signed
     in, Mod Hub works, no restart.
2. **Archive the build** (§1): `dayone.py archive`: the Steam build and depot
   manifests, every executable's and library's SHA-256, size and PE
   timestamp into `~/TPF3-MP-builds/<build>/build.json`, and a copy of each
   executable there, never in the repository. It refuses to overwrite a
   copy that differs.
3. **Decode the executable** (§2, §6): `dayone.py decode`: `tpfre` indexes it
   (seconds) and looks up every hook target by its TPF2 name and source
   file; `tpfre q <db> sig <name> --toml` then gives each target's profile
   block. On TPF2 it finds `GameSim::Step` at the known `0x15aa00`. TF3
   dropped the `__FUNCSIG__` strings those names came from (it keeps RTTI
   and `__FILE__`), so give it TPF2's executable or index too:
   `dayone.py decode --names-from <TransportFever2.exe>` carries TPF2's
   names over with `tpfre match` first. On build 40408 that found
   `GameSim::Step`, `CGame::Step`, `CGame::RunGameSimLoop`,
   `CommandList::Add` and `UI::CMenuUI::CreatePage`, each in the source
   file TF3 itself names for it.
4. **Correct the two guesses in the code**: `dayone.py find`: the game's
   executables against `find_executable`'s names (`crates/tpf3mp-launch`)
   and the logs and crash dumps it finds against `game_candidates_in`
   (`crates/tpf3mp-agent/src/logs.rs`), and the mods folders. Until the
   names are fixed, the launcher starts the game with `--game-exe`.
5. **The game's scripts and the probes** (§3):
   - `dayone.py scripts`: every `api.cmd.make*Cmd` the game's `.tl` and
     `.d.tl` files declare or use, the files that send commands, and the
     speed controls (`GameSpeedControl`).
   - `dayone.py probes install`, activate them in Mod Hub, start a game,
     then `dayone.py collect --log <the game's log>`: the script API of the
     GUI state and of the run script's state (`tools/probe/tf3`). The TPF2
     probes (`tools/probe/script_api_dump`, `determinism_probe`) are the
     fallback should TF3 still run game scripts.
6. **Determinism** (§4): two games from one save with the
   determinism probe, each log collected with its own `--label`, then
   `dayone.py compare A B`. The probe labels its samples by the simulation
   step, so frames need not line up: the game's own `GameTime.updateCount`
   where the release API has it (`stepTime=updateCount` in the log's
   header), else a step learned from the game time. Run both games at
   **1x**: the probe samples from GUI frames, and at 4x the game passes
   several steps a frame, so most samples are skipped (seen on build 40408).
   Load the same save for both runs, without saving over it. Two logs labelled differently are refused. The release builds are
   40408 on Steam and 40393 on Epic and GOG
   ([TF3_OFFICIAL_API_2026-09-29.md](../investigation/TF3_OFFICIAL_API_2026-09-29.md)).

7. **What the API reference says, confirmed** (documented on
   wiki.transportfever3.com/script-doc, never seen running;
   [TF3_OFFICIAL_API_2026-09-29.md](../investigation/TF3_OFFICIAL_API_2026-09-29.md)). In the
   API dump from step 5, and in the game's console:
   - `GameTime.updateCount` counts simulation steps and stops while paused;
   - `api.type.RoadType.STREET` and `TRACK` exist, and a build proposal's
     `BaseEdge.roadTemplate` and `roadStyle` are plain strings (the mod's
     capture refuses anything else, `engine.lua`);
   - a street's bus lane and tram track: part of the road template, or
     fields of their own (the capture records none, a guess to replace);
   - `makeWorldBuildProposalCmd`'s `playerInitiated`: whether the stock
     street tool sets it, and whether the hook sees it where the command is
     queued (HOOKS.md, the caller-RVA filter);
   - whether `api.cmd.Debug.makeGamePerformSimulationStepsCmd` works in the
     release build (REGRESSION.md, "With the real game");
   - the Epic/GOG build (40393) next to Steam's (40408): run steps 1 to 3 on
     both executables, since the hook needs a profile for each.

   The manual's in-game tools page (marked as possibly TPF2's) names the
   aids for this: debug mode (`debugMode` in `settings.lua`, or the
   advanced settings), a Lua console on the key below Esc that runs
   commands and prints to `stdout.txt` (the log `logs.rs` expects), and
   simulation speed up to 32x in debug mode. Local mods go to
   `<Steam>/userdata/<Steam ID>/3493540/local/staging_area/`, as the
   installer does; a mod there wins over a manually installed or
   subscribed one with the same modId.

`python tools/dayone/test_dayone.py` tests the tool on made-up folders,
executables and logs; `crates/tpf3mp-proto/tests/lua_probes.rs` runs the
probes in a stand-in for the game's GUI state.

## 0. Go or no-go first

Two findings can end the plan as it stands, so they come before
everything else ([PLAN.md](PLAN.md), Part 1):

- **The launcher's start** (§5): a game the launcher started, suspended
  with the hook loaded, runs signed in to Steam with the Workshop, and does
  not restart itself through Steam. If it restarts, the hook is lost and
  D11 needs another way in.
- **Anti-tamper** (§2): TPF2 had SteamStub only. Denuvo or a VM protector
  changes the native plan.

Then name one person on **patch duty**: on every game patch they rerun the
naming, the build diff and the profile (release-day order 1 and 2 below)
and hold releases until the hook matches the new build.

## 1. Archive every build

- Record for every build:
  - Steam build ID and depot manifest IDs;
  - executable SHA-256, PE timestamp and image size (Windows);
  - the Linux and macOS binaries' hashes.
- Keep a private copy of every executable. Signature profiles are verified
  against it, and hooks must fail closed on anything unrecorded.
- Expect a day-one patch and frequent patches after it.

## 2. Static recon (Windows executable first)

- **Anti-tamper:** packer or protection sections, section entropy, TLS
  callbacks. Anti-tamper changes the native plan and must be known first.
- **Symbols:**
  - RTTI;
  - MSVC `__FUNCSIG__` and `__FILE__` assert strings, run through the naming
    pipeline from `tpf2-multiplayer/tools/re` and `tools/ghidra`, made
    build-independent first;
  - TPF2-era names: `make_cmd::`, `CommandList::Add`, `GameSim::Step`,
    `CGame::RunGameSimLoop`.
- **Lua:** Lua version strings and the script API table names.
- **Linux and macOS:** repeat the symbol and string survey. Record whether the
  macOS binary is hardened-runtime, library-validated, and allows
  `DYLD_INSERT_LIBRARIES` (`codesign -dv --entitlements - <binary>`).

## 3. Script API recon (a probe mod, all three platforms)

*Changed* (TF3_MODS_2026-09-27.md): the game ships its GUI and much of
its rules as Teal (`.tl`) sources, with declaration files for its API
(`system.d.tl`). Read those first: `grep` them for `api.cmd.make` to list
the command factories and see which tool builds which command. The probes
fill what the sources do not say. They are TPF2 mods, and must be ported
to TF3's layout (`mod.json`, `_content.json`, `content/`) and to a GUI
`onStep` plugin before they can run.

*Added* (TF3_OFFICIAL_API_2026-09-29.md): Urban Games' generated
reference at `wiki.transportfever3.com/script-doc/` documents the API,
including the **61 `api.cmd.make*Cmd` factories** with their argument
types (recorded in
[investigation/TF3_OFFICIAL_API_2026-09-29.md](../investigation/TF3_OFFICIAL_API_2026-09-29.md)),
and the **companies** command API (`makeGameAddPlayerCmd`,
`makeEntitySetPlayerCmd`). The recon confirms and measures those against
the running game; it no longer discovers them from scratch. The reference
"is not yet complete", so the dump still fills gaps and catches build
changes.

- `_VERSION`; availability of `io`, `os`, `require`, `package`, `debug`,
  `load`/`loadstring` in both the game-script and GUI states.
  *Added:* Mod Hub's mods use `os.clock`, `os.time`, `os.date` and
  `require`, and none `io` or `load`
  ([TF3_MODHUB_SCRIPT_MODS_2026-09-29.md](../investigation/TF3_MODHUB_SCRIPT_MODS_2026-09-29.md), REPORTED).
- Dump `api.*` and `game.interface.*`; list `api.cmd.make.*` factories.
- Number formatting (`%.17g`), `math.random` behaviour, `pairs` order
  stability for string keys across runs.
- The mod layout: REPORTED as `mod.json`, `_content.json` and
  `_metadata/modinfo.json`, with scripts in `content/`. Confirm it, and
  what a Mod Hub script mod may contain.
- Whether game scripts (`res/config/game_script`, `update()` per step)
  still exist. *Changed:* they do, as `*.gs.lua` files the game finds by
  itself, with `update`, `postUpdate`, `handleEvent` and `guiUpdate`, and
  their state in a `GAME_SCRIPT` component (REPORTED by five Mod Hub mods,
  TF3_MODHUB_SCRIPT_MODS_2026-09-29.md). Confirm it with the probe, and
  measure whether a command a game script sends is queued like a
  player's: every game in a room runs the same game script, so its
  commands must run on each game and never be sent to the room.
- Other people's script mods, once Mod Hub is open: it runs on mod.io
  (`transportfever3` there, hidden until release; tags include
  `Script Mod`). `tools/modio/fetch.py --tag "Script Mod" --limit 100`
  downloads the most popular ones' scripts into the ignored `.modio/`,
  to read for the API in use. Their code is their authors': take what it
  shows about the API, never the code.
- How to list the active mods in load order, each with a name and a
  version, and the game's build: the hook reports them over the bridge,
  and the agent declares them (`ContentManifest`) in place of the
  `--game-build` and `--mods` options it takes until then.

## 4. Determinism measurement (the D2 calibration)

Run two instances from the same save with no input. Hash these lanes every
100 steps for 60 in-game days (the TPF2 baseline):

- vehicle count;
- vehicle positions at 1 m;
- edge geometry at 0.1 m;
- the construction list;
- town building counts;
- money per player;
- people count.

Then repeat with a scripted input sequence. Record, per lane, the step where
two runs first differ:

| pair | expectation |
|---|---|
| same PC, same binary | identical (TPF2 was) |
| Intel vs AMD, Windows | identical if no CPU-dispatched math paths |
| Windows vs Linux native | probably drifts (different compilers and C runtime) |
| Windows vs Linux under Proton | measure: same binary, but Wine's math library |
| Windows vs macOS arm64 | expected to drift |

The results decide how far same-binary rooms may relax drift control. They
do not change D2.

## 5. Hook feasibility, per platform

The launcher starts the game with the hook in it, and nothing else does
(D11 in [DECISIONS.md](DECISIONS.md); `crates/tpf3mp-launch`). Check, on
each platform, that a game started that way plays as one Steam starts:

- **The executable.** `find_executable` in `crates/tpf3mp-launch` looks for
  `TransportFever3(.exe)` or `Transport Fever 3(.exe)`, or on Windows the
  only other program in the folder. Correct the names, and drop the
  `TODO(TF3 release)`. Where the game is started through a script (a
  `.sh` that sets up libraries and runs the binary), start what the
  script starts, with its environment.
- **Steam.** Started directly with `SteamAppId` and `SteamGameId` 3493540,
  and Steam running, the game must start signed in, with achievements and
  the Workshop, and must not restart itself through Steam (which would
  lose the hook). If it does restart, find out why: a `steam_appid.txt`
  would be a file in the game's folder, against D11.
- **Windows:** the hook loads into the suspended game before its title menu,
  `hook.log` names the build, and the game runs on as usual. If the game's
  protection hides its modules or refuses a thread made before its own,
  let the loader run first as TPF2MP's injector does (`--launch`): resume
  the game until its main thread is inside its executable, suspend it,
  then load the hook.
- **Linux:** the same with `LD_PRELOAD`. Find out whether Steam starts the
  game inside its Linux runtime (pressure-vessel): if the game needs it,
  start it the same way, with the hook preloaded inside. TPF2's Linux build
  starts through a `run.sh` next to it; tearded's TPF2 multiplayer
  launcher puts its preload into that script. If TPF3's starts the same
  way, start the script with `LD_PRELOAD` in its environment. The script
  must `exec` the game: otherwise the game's parent is the script, not
  the launcher, and the hook stays out (`TPF3MP_LAUNCHER_PID`, HOOKS.md).
- **macOS:** how to get a library into the game at all: the binary's
  hardened runtime, library validation and `DYLD_INSERT_LIBRARIES` (§2).
  Test whether code pages can be patched under the process's code-signing
  flags.

## 6. Command pipeline and time

- Locate the command factories and the command queue, then prototype capture
  and cancel for one command (road build) on Windows. Use ground-truth sweeps
  through `api.cmd.make.*` rather than inferring from player clicks.
- *Added:* find out whether the stock tools, the road and track builders
  first, send their commands through script (`api.cmd.sendCommand`), as
  the GUI's own Teal code can. If they do, the caller-RVA filter that
  told a click from a replay on TPF2 (HOOKS.md) cannot, and a mod might
  capture and cancel by wrapping `api.cmd.sendCommand`. Test whether the
  GUI state lets a mod write that field.
- Locate the simulation step, the step size in game time, speed and pause
  control, and the injection point just before a step runs.
- Catch the game's own speed and pause buttons and send them as the room's
  speed (`Control::Speed`), as TPF2MP's mod does; every game then follows
  the server's pace. The launcher offers no speed control of its own.
  REPORTED: the speed row is a script recipe
  (`game_bar_widgets.GameSpeedControl`) a mod can replace, the keybinding
  `IA_GAME_PAUSE_OR_CYCLE_SPEED` changes speed too, and the engine takes
  any speed, not only 1x to 4x.
- Join a third game mid-game and rejoin one after killing it, then compare
  lanes: late join, rejoin and repair all load a save and apply turns, so
  a loaded game must walk its lists in the same order as a running one
  (TPF2's hot-join desync). This is part of the first playable room's
  test (PLAN.md, Test A), not left for later.

## 7. Saves

- Force a save and load a named save from native code.
- Load a save made on Windows on Linux and macOS, and the reverse.
- Measure save sizes for small, medium and large maps.
- Find what a save can make the game run (script state, mod code), and
  apply a check to a received save before the game loads it, as TPF2MP's
  `save_metadata.py` did. Its rule for TPF2's `.sav.lua` sidecar, pure data
  under `function data() return { ... } end`, is ported as
  `tpf3mp_agent::save_check::check_lua_data`; if TPF3 saves carry such a
  file, check it where the bridge takes a fetched world (`Done::Fetched` in
  `crates/tpf3mp-agent/src/bridge.rs`) and refuse to load on failure. Until
  then a received save is only as trustworthy as the player who uploaded
  it.
- Run `measure --pair` on two saves of one world taken minutes apart, to
  see how well snapshots deduplicate (SNAPSHOTS.md).

## Deliverable

A dated recon report containing:

- the determinism table;
- a go or no-go per platform for the hook;
- the first build's signature profile;
- the list of script API capabilities.

Start it from `investigation/TPF3_RECON_TEMPLATE.md`.

## Tools

The tools that turn this plan into findings live under `tools/`. They are
read-only (no game is launched or modified) and their output is deterministic.
They were built and validated against Transport Fever 2 build 35924; the baseline
outputs are in `investigation/tpf2-baseline/`. One-time setup:
`python -m venv .venv && .venv/Scripts/pip install -r tools/requirements.txt`,
and `cargo build --release` in `tools/tpfre`.

| tool | what it does | feeds |
|---|---|---|
| `tools/tpfre`: `tpfre index <bin> -o <db>`, then `tpfre q <db> ...` | the fast path for decoding the executable (Rust; [README](../tools/tpfre/README.md)). One parallel pass (about 4 s on TPF2) into one SQLite file: functions (`.pdata` and leaves), direct calls, tail jumps, imports, RIP-relative references, data pointers, strings, `__FUNCSIG__`/`__FILE__` names (the `name_functions.py` rules, agreeing name for name on TPF2), RTTI classes and vtable slots, and SteamStub/packing warnings. Queries answer in milliseconds: `func`, `dis`, `callers`, `callees`, `path`, `xrefs`, `str`, `fnstr`, `names`, `class`, `vtable`, `file`, `whois`, `sig` (make_profile's signatures), `bytes`, `validate`; `tpfre diff` compares two builds. Its "For agents" section is the cheat sheet to give a coding agent. PE x86-64 only. | §2, §6, §7, the hook profile |
| `tools/re/binary_survey.py <bin> -o out.md` | identity, sections+entropy, imports (system vs game-folder, with proxy-loader ranking), exports, TLS callbacks, packer/anti-tamper, RTTI, Lua version, `__FUNCSIG__`/`__FILE__` counts, TPF2-era names, and macOS code-signing posture. Handles PE, ELF and Mach-O. | §1, §2, §5 |
| `tools/re/name_functions.py <bin> -o dir [--validate spec]` | recovers a symbol map (RVA -> name, source file) from assert strings, build-independent: `.pdata` bounds on PE, LIEF function starts on ELF/Mach-O, x86-64 and arm64 reference resolution. Emits JSON+CSV and Ghidra/x64dbg/IDA scripts. | §2, §6 |
| `tools/re/diff_builds.py OLD.symbols.json NEW.symbols.json` | which named functions moved, resized, appeared or disappeared between two builds -- run it after a day-two patch to re-verify hook signatures. | §1, §2 |
| `tools/re/make_profile.py <bin> <symbols.json> NAME ... -o profile.toml` | writes the hook profile for named functions: the build identity, and per function a unique signature with every displacement wildcarded and the exact prologue the detour steals (docs/HOOKS.md, release-day procedure). Refuses what it cannot make safe. x86-64 only. | §2.4, the hook profile |
| `tools/re/test_make_profile.py [--update]` | checks `make_profile.py` on a synthetic PE and keeps the fixture hookcore's test resolves current. | -- |
| `tools/re/selftest.py` | validates the ELF / Mach-O / arm64 code paths on synthetic fixtures (no game binary needed). | -- |
| `tools/probe/script_api_dump/` | game-script mod: dumps the Lua sandbox and `api.*`/`game.interface.*` from the engine and GUI states. | §3 |
| `tools/probe/determinism_probe/` | game-script mod: hashes the §4 lanes every N steps to a per-instance log. | §4 |
| `tools/probe/tf3/` | the TF3 probe mods, in TF3's layout (REPORTED): `tpf3mp_apidump_1` dumps the GUI state from a game bar plugin, `tpf3mp_rundump_1` the run script's state, `tpf3mp_detprobe_1` samples the §4 lanes every 100 simulation steps, labelled by the step. Output to `%LOCALAPPDATA%/tpf3mp/probe`, or the game's log for `dayone.py collect`. | §3, §4 |
| `tools/dayone/dayone.py` | the release-day checks, one command each ("Release day, step by step"): `find`, `archive`, `gonogo`, `check-launch`, `decode`, `scripts`, `probes`, `collect`, `compare`. | §0-§5 |
| `tools/probe/compare_runs.py A.log B.log` | first per-lane divergence between two determinism logs. | §4 |
| `tools/probe/check_lua.py` | syntax-checks the probe Lua with a real Lua 5.2. | §3, §4 |
| `tools/modio/fetch.py` | downloads mods from mod.io, Mod Hub's backend, for study: the most popular first, scripts and text only unless `--all-files`, into the ignored `.modio/`. Needs a mod.io API key (`MODIO_API_KEY`). | §3 |

Which to use for what:

- **The Windows executable's code** (finding, naming and confirming hook
  targets, their callers and strings, and their signatures): `tpfre`. Index
  once (seconds), then ask: each question answers in milliseconds, so an
  agent can ask many small ones.
- **The survey report** of every platform's binary (imports with the
  proxy-loader ranking, TLS callbacks, Lua version, code signing):
  `binary_survey.py`.
- **Linux and macOS binaries** (ELF, Mach-O arm64), and Ghidra, x64dbg or IDA
  scripts that apply the names: `name_functions.py`.
- **A whole hook profile file** with its `[build]` identity: `make_profile.py`.
  `tpfre q <db> sig NAME --toml` prints the same `[[target]]` block for one
  function, to try a target before writing the profile.

Release-day order:

1. **Archive + static (per platform):** run `binary_survey.py` on each build
   (§1 hashes/sizes, §2 loader/packer/Lua/RTTI). `tpfre index` the Windows
   executable; if it warns that `.text` is encrypted, dump the running game's
   image by hand with a debugger and index the dump with `--image-base`. Find the hook targets with
   `tpfre q` (§2, §6, §7): `str`, `names` and `file` to locate a subsystem,
   `func`, `callers`, `xrefs` and `dis` to confirm, `validate` with a spec of
   the known targets once they are located. Run `name_functions.py` on the
   Linux and macOS builds, and on Windows for its symbol map and scripts. Then
   write the build's hook profile with `make_profile.py` from the binary, its
   symbol map and the target names.
2. **On a patch (Windows PE builds):** keep complete private snapshots of
   both installs with `tpfre archive`, including script/API sources rather
   than just the EXE. Run `tpfre audit OLD NEW --profiles profiles --json`:
   every old hook target is checked, even if named-function diff misses it;
   matching signatures also get a containing-function comparison. Review
   changes and matcher suggestions manually, investigate the script diff,
   then generate/review the new build's profile. Run the strict
   `tpfre verify NEW --profiles profiles --json` against its exact identity
   before the normal CI and real-game acceptance gates. Unknown builds and
   missing inputs fail; static evidence never approves runtime compatibility.
   EXE-only old archives cannot recover the old scripts: pass the explicit
   executable and keep that comparison marked unavailable. Commands, limits
   and report/exit-code meanings: [tpfre](../tools/tpfre/README.md#game-update-workflow-windows-pe-builds).
   On other binary formats, continue with `name_functions.py` and
   `diff_builds.py`; these new audit commands currently require PE32+ x64.
3. **Script API (per platform):** `check_lua.py` first, then `dayone.py
   probes install` (the TF3 probes), activate them in Mod Hub, start a game,
   and `dayone.py collect` the dumps (§3).
4. **Determinism (the D2 calibration):** the determinism probe in two games
   from the same save at 1x, each log collected with its own `--label`, the
   pairs in the §4 table, then `dayone.py compare` the logs.
5. Fill `investigation/TPF3_RECON_TEMPLATE.md` as results arrive.
6. **Player log bundle:** find where Transport Fever 3 writes its log
   (`stdout.txt` for TPF2) and crash dumps on each platform. The bundle
   already looks in its Steam folder (app 3493540) where TPF2 kept them:
   correct `game_candidates_in` in `crates/tpf3mp-agent/src/logs.rs` where
   that is wrong, drop the "TPF2 location, confirm on TF3" mark from the
   confirmed ones, and update "Sending your logs" in `docs/PLAYING.md`.
7. **Installer:** confirm on each platform that the game loads mods from
   `<Steam>/userdata/<account>/3493540/local/staging_area` (REPORTED for
   build 40391; TPF2 used `local/mods`), and that a player enables the
   TPF3-MP mod once, with Activate in Mod Hub. Where not,
   correct `Find-ModsDir` in `packaging/windows/tools/install.ps1` and
   `find_mods_dir` in `packaging/unix/install.sh`, with their tests in
   `packaging/*/test-install.*`. Their `Find-Game` and `find_game` find
   the game's folder only to see whether the game is running.
8. **Starting the game:** work through §5 and start a real game from the
   launcher in a room, on each platform: **Start Transport Fever 3**
   starts it, the Game part shows it connected, and the same game started
   from Steam shows nothing of TPF3-MP (no `hook.log` lines).
