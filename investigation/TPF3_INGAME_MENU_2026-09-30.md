# The in-game Multiplayer menu: findings and checks -- 2026-09-30

The owner asked: "lets also get the in game multiplayer menu working as
well". This lifts the hold on D17 (DECISIONS.md, amended 2026-09-30): the
room's lobby moves into the game now; D11 stays.

Built on `feat/ingame-menu`, which merges `feat/ingame-lobby` (the main
menu's Multiplayer entry and window, which reached the real game's menu)
into dev and connects the window to the launcher over dev's bridge
(version 7; 9 once merged with `feat/on-dev-2`, whose messages took 7 and 8). Design in [docs/LOBBY.md](../docs/LOBBY.md) and
[docs/HOOKS.md](../docs/HOOKS.md), "The main menu's Multiplayer window";
players' steps in [docs/PLAYING.md](../docs/PLAYING.md), "The Multiplayer
menu in the game". The game was not launched for this work; Steam build
40408 (`de1daad3...`) was only read on disk.

Marks: **CONFIRMED-static** = the binary or the game's own scripts show it,
or a test runs it; **CONFIRMED in-game** = seen in the real game before
this branch; **INFERRED** = concluded from shape or analogy, to be seen in
the game; **UNKNOWN** = open.

## 1. The entry on the main menu

- **CONFIRMED in-game** (feat/ingame-lobby, before this branch, reported by
  the owner): the hook logged `main-menu Multiplayer entry armed from
  profile ...` and `[menu] menu patch installed in Lua state ...`, and the
  entry and its window reached the main menu. Kept as it was: the loader
  patch (`crates/tpf3mp-hook/src/menu_entry.rs`, renamed from `menu.rs` so
  it does not collide with `feat/join-from-menu`'s `menu.rs`) and the mod's
  `gui/menu/main_page.tl` and `gui/menu/lobby.lua`.
- **CONFIRMED-static.** The entry's targets are now in the built-in
  profile (`profiles/tf3_build40408_steam_windows/hooks.toml`, optional there),
  not a separate profile file to copy: `lua_loadfile` at `0x2fa1d50`,
  `lua_load` at `0x2fbdf70`, `lua_pcallk` at `0x2fbe0c0`, each unique in
  the installed game's `.text` with its prologue
  (`tf3_static_proof.rs` with `TPF3MP_TF3_EXE` pointing at the Steam
  install: 2 passed). The signatures are the ones feat/ingame-lobby armed
  in the real game; `lua_load` and `lua_pcallk` match feat/join-from-menu's
  independently found ones.
- **INFERRED.** The optional targets cannot break the step gate: a missing
  one is recorded and skipped. An ambiguous or wrong-prologue one would
  refuse the whole profile, which the static proof rules out for this
  build.

## 2. The window and the launcher

- **CONFIRMED-static** (tests). The launcher opens the game's link when it
  starts and serves it while no room session holds it: hello, the lobby on
  every change, the window's actions carried out as its own
  (`launcher::lobby::IdleLink`, its tests). A room session takes the link
  over greeted and gives it back (`bridge.rs`,
  `a_greeted_bridge_passes_the_launchers_lobby_both_ways`).
- **CONFIRMED-static** (end-to-end test with a real server and launcher,
  the game's side a real `Session` at "the main menu":
  `tpf3mp-testkit/tests/launcher.rs`,
  `a_game_at_its_main_menu_plays_the_lobby_through_the_launcher`): connect,
  create, chat, ready and start from the window; the room's `Begin` reaches
  the game; the launcher's own window shows the same room and chat.
- **CONFIRMED-static** (tests). At the menu the window's requests are the
  only reader of the link: `Session::poll_lobby` keeps the lobby and stops
  at the room's `Begin`, which the gate takes when a world steps
  (`at_the_menu_the_lobby_is_read_and_the_rooms_game_waits_for_its_gate`).
  A lobby during a load does not fault the gate. The lobby never reaches the
  in-game Multiplayer window's notices (dev's `feat/mp-window` window is
  unchanged).
- **CONFIRMED-static** (tests). The state literal the hook answers is read
  by a real Lua as the window reads it (`the_windows_lua_reads_the_literal`);
  bytes above ASCII now go as `\ddd` escapes (before, they were re-encoded
  wrongly). `lobby.lua` parses under Lua 5.2 (`tools/probe/check_lua.py`).
- **INFERRED.** The request channel (`tpf3mp/state.lua`, `tpf3mp/act.lua`
  answered from the loader's detour) works in the game: feat/ingame-lobby's
  window showed its connect form, which needs the state request answered.
  New here: each request calls into the step driver (`install::lobby_pump`,
  a `try_lock`, never a wait) on the menu's own thread.
- **INFERRED.** The loader is called on the main thread for the window's
  requests (the window's `react.onStepTimer`), the same thread that runs
  `GameSim::Step`; the driver's lock is only tried, so a request during a
  step (none expected) just shows the last lobby.
- **CONFIRMED-static.** Logs never carry an invite or password: the hook
  logs a lobby action's kind only, the launcher likewise (D13).

## 3. What the room's game needs, on dev

- **CONFIRMED-static.** Start from the window begins the room; the game
  takes the `Begin` at its gate once a world steps. On dev a world must be
  up for that: the owner loads their save (Load Game) and the room plays it
  (`Load` without a file); a guest loads any save and the mod loads the
  room's (`Load` with a file, through the world's GUI). Joining straight
  from the main menu is `feat/join-from-menu`, being ported onto dev in
  `feat/on-dev-2`.
  Since merged with it (`feat/combined`): a guest waits at the menu, and
  the menu's lobby reads the link only while that port's session waits for
  a room's game or after it ended.
- **CONFIRMED-static.** A room left before its game began sends the game
  no `End` (the dev hook held its world on one), so the game follows the
  player into the next room on the same link.
- **UNKNOWN.** How long the room waits for the owner to load their save
  after Start, before the server gives up on the owner's progress.

## 4. Two menu mechanisms, side by side

`feat/join-from-menu` reaches the menu's Lua states by detouring
`RegisterAppUsertypes` (`0xdc5fa0`) and runs on `UI::CMenuUI::DoStep`
(`0x6a0160`); this branch's entry detours the loader's body (`0x2fa1d50`).
They detour different functions, live in different files (`menu.rs`,
`menu_entry.rs`), share `lua_load` and `lua_pcallk` under the same profile
names and signatures, and both reach the room through the same step
driver. They coexist; unifying them is not needed for either to work. When
`feat/on-dev-2` lands its `Session` rework (the `Lobby` state enum for
following the launcher from room to room), `poll_lobby` must read only in
its `Waiting` state (and after the game ended), as `begun`/`peeked` do
here.

## 5. UNKNOWN (for the game)

- Whether the window's requests keep coming while the window is closed
  (they do not: the lobby is then read only when it reopens or a world
  steps). Harmless: only the newest lobby is kept on either side.
- Whether the Multiplayer card and button keep their place after the game
  updates `gui/menu/main_page.tl` (the mod's copy is the game's file plus
  marked blocks; `tools/lobby/make_main_page.py` rebuilds it).

## 6. In-game check list

One game, started from the launcher (`TPF3-MP.exe`) with **Start
Transport Fever 3** before joining any room, the deployed server, Steam
running. Expected `hook.log` lines (`%LOCALAPPDATA%\TPF3-MP\hook.log`),
`<...>` varies:

1. At start:
   - `matched profile "Transport Fever 3 Build 40408 (Steam, Windows x64)" (<n> targets; <m> matching in all)`
   - `step gate installed on GameSim::Step at 0x<...>; the session is attached to "<link>"`
   - `main-menu Multiplayer entry armed from profile "Transport Fever 3 Build 40408 (Steam, Windows x64)" (loader at 0x<...>)`
   - `[menu] menu patch installed in Lua state 0x<...>` (at least one)

   Failure lines: `multiplayer disabled (fail-closed): ...` (no step gate:
   the window will say there is no link) and `main-menu entry not armed
   (fail-closed): ...` (no entry: the launcher's window still works).
   The launcher's log shows `the game's hook attached`, and its Game part
   says the game is connected.
2. On the main menu, click **Multiplayer** (the card, or the top-bar
   button). The window opens with the launcher's server and your name,
   **not** "This game has no link to the TPF3-MP launcher".
3. Type a name, **Connect**. `hook.log`: `[menu] lobby action connect for
   the launcher`; the launcher log: `the game's Multiplayer window asks
   action="connect"`. The window's header shows your name at the server;
   the launcher's window shows connected.
4. **Create** a room. The window shows the room, its invite code, you with
   the crown. The launcher's window shows the same room.
5. From a second launcher (or the second player's game,
   `tools/sandboxie/second_player.ps1`), **Join** with the code: both
   windows list both players within a second.
6. Write in the chat: the line shows in both games' windows and both
   launchers.
7. **Ready** in both; the owner presses **Start**. Both launchers show the
   room running. The owner loads a save with **Load Game**: `hook.log`
   `the room began a game: rules native, ...` then `playing the room's
   world from step <n>`. The guest loads any save; the room's world follows
   (`playing the room's world from its save, from step <n>`).
8. In the room's game, dev's in-game Multiplayer window (the game bar's
   Multiplayer line) shows the room and its chat as before.
9. Before any Start, **Leave** and create another room: the game follows,
   and **Start** there begins that room's game (no `holding the world`
   line).

Lines that mean something is wrong: `[menu] lobby action refused: ...`,
`reading the launcher's lobby failed: ...`, `the launcher did not hear the
lobby window: ...`, and in the game's own log `[tpf3mp] main menu: the
mod's main_page.tl is not loadable (...)` (the menu stays the game's: the
mod is not installed or not active).
