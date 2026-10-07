# The Multiplayer entry on the main menu

D17 moves the room into the game (the owner lifted its hold on
2026-09-30): connecting, rooms, the lobby and chat in a Multiplayer window
reached from the main menu. This page says how the entry gets onto
Transport Fever 3's main menu at all, which took three tries on release
day, what the mod and the hook each contribute, and how the window talks
to the launcher that started the game. Players' steps are in
[PLAYING.md](PLAYING.md), "The Multiplayer menu in the game".

## What the game allows, and what it does not

TF3 draws its main menu from Teal scripts, `gui/menu/main_menu.tl` and
`gui/menu/main_page.tl`, loaded through the game's Lua loader. Three ways
in were tried against build 40408:

| route | result |
|---|---|
| A mod's own copy of `gui/menu/main_page.tl`, hoping the mod filesystem overlays the game's files at the menu | Not applied: mods are merged into the filesystem at startup but the game's `::/` files win until a game is loaded (as TPF2 applied mods per save). The menu has no mod extension point either. |
| The game's `--script <uri>` switch, pointing at a copy of `main_menu.tl` | It is a plain startup script runner (`Lua_Core`, before any menu exists): the file ran, but `_react.builtin` was empty and `react.lua` failed. A dead end for a menu. |
| **The hook, at the game's Lua loader** | Works. Below. |

## How it works

The game's `base/init.lua` resolves every `ug_require` and then calls
`resolveutil.loadfile(resolved)`, a Lua function whose body is a C++ lambda
(`framework/lua/Loader.cpp`, `lua::MakeState::<lambda_8>`). The hook, loaded
into the suspended game before any of its code runs (D11), detours that body
(`crates/tpf3mp-hook/src/menu_entry.rs`):

1. The detour's thunk saves the argument registers, reads the Lua state out
   of the lambda's closure (`**(closure + 0x10)`, the layout the body's own
   `lua_pcallk` call shows), and tail-jumps into the original body with the
   stack untouched. The loader runs exactly as before.
2. The first time a Lua state is seen, the hook runs a short Lua chunk in it
   through `lua_load` and `lua_pcallk`. The chunk wraps
   `resolveutil.loadfile`: a request for the game's `gui/menu/main_page.tl`
   is answered with the mod's `tpf3mp_1::/gui/menu/main_page.tl`; every
   other request passes through. The original resolved path stays the
   module's cache key, so the rest of the menu sees the same `MainPage`
   value it always did.
3. The mod's `main_page.tl` is the game's file with marked `TPF3-MP:`
   additions (below), and the Multiplayer page, `Tpf3mpLobbyPage`: a keyed
   child (`tpf3mp-lobby`) laid over the main page as the game lays its
   own pages, while the menu's cards stay mounted under it, hidden by the
   game's `title-icon-only` class (their node refs must stay attached, or
   the game asserts "Could not initialize all node refs").

**Before the game runs.** The game loads its main menu within seconds of
starting, so the entry must be armed first. The launcher starts the game
suspended, loads the hook, and keeps it suspended until the hook sets the
event `tpf3mp_ipc::hook_ready_event` names for the game's process: the
hook arms the entry first in its bootstrap, sets the event, and only then
installs the step gate and the rest. A hook that never sets it lets the
game run after 30 seconds. Without this the entry was sometimes missing
(2026-09-30): the slower installs came first, and the game had loaded its
own main page before the patch was in.

The game log shows each step: `[tpf3mp] main menu: resolveutil.loadfile is
wrapped`, `... ::/gui/menu/main_page.tl is served from
tpf3mp_1::/gui/menu/main_page.tl`, `... TPF3-MP main_page.tl is in effect`.
The hook's `hook.log` shows the profile match, `main-menu Multiplayer entry
armed`, and `menu patch installed in Lua state ...`. Then `main_page.tl SERVED: ...` when the menu's page came from the mod,
or `main_page.tl MISSED: ...` when the game's own loaded first.

A game Steam started has no hook and keeps the plain menu.

## What the menu shows

- **Two cards**, in a column right of the game's own grid of cards, each
  a quarter of the menu wide and half high: the size the game gives its
  `level2b` cards, so the grid's rows stay as they are and the menu grows
  by one column. **Multiplayer** has the grid's top-right corner (the Map
  Editor card gives it up), two of the game's own pictures, the TPF3-MP
  glyph, and a live line under its title: not connected, online on EU,
  the room with its players and ready count, or the room's world on its
  way. **Join a friend** opens the window with joining first, and shows
  the room's invite once in one. The labels are the game's card label
  (`menu_icon_react_util.makeCardLabelBottomComponent`) with the live
  line (`lobby.CardLine`, its own recipe, so only it redraws, once a
  second) in place of the fixed description.
- **A button in the top bar**, next to Settings: a glyph drawn as the
  game's top-bar icons are, 100 px greyscale, white on black, for the game
  to tint (`gui/tpf3mp/icons/menu_multiplayer_50@2x.tga` and its 50 px
  copy, from `tools/art/icons/menu_icon.py`).
- **The Multiplayer page** (`gui/menu/lobby.lua`), laid out as the game's
  own pages: its top bar with Back and the title
  (`menu_icon_react_util.makePage`), one big card
  (`makeMainOuterCard`, 1600 by 700) with a header (the page's name, the
  connection, the player's name and server), and a footer row of buttons
  of one size: the one that moves on (`primary`) on the right, leaving
  and taking back being ready red (`error-tape`), the rest secondary. The
  game's Back key and the top bar's Back step back one page, out of the
  page last. Under the header: what is under way until the launcher
  answers, what went wrong (until the next action), and a notice of what
  just happened, which fades after a few seconds; the room's world while
  it comes and loads, with a progress bar. No flavour text. The pages:
  - not connected: the name, and **Connect**;
  - the first page: two big cards in the main menu's style (the game's
    `CardButton` with its cut corners), **Join a room** and **Host a
    room**; **Disconnect**, **Server...** and **Your banner** in the
    footer, the log session on the right;
  - **Join**: **Public rooms**, the server's room list (D26 proposed;
    PROTOCOL.md, "Rooms"), as cards in the main menu's style, each with
    its climate's picture (the game's own, `app.res.climateRep`, else its
    New Game card's), name, players/limit, companies, year and a lock for
    a password; a click joins, asking for a password first. The page asks
    for the list when shown and every 10 seconds; **Previous**, **Next**,
    **Refresh**. A launcher on its release's several servers (D12's
    proposed amendment of 2026-10-06) lists the rooms of all of them:
    each card adds its server and ping (`EU · 24 ms`), the title names
    the servers with theirs, and a click joins on the room's server. **Join with code** opens a page laid out as Host's: the
    picture of joining on the left; on the right the invite code, large
    (as the room's page shows it), the room's password, and the server
    and name it joins with; **Cancel** and **Join**;
  - **Host**: the room's world as a big card (the save's own picture as
    the Load Game page shows it, else its climate's; **New world**
    without one), a click picks it on the game's Load Game page ("The
    room's save and mods" below); **How you play**, two picture cards,
    Co-op and Competitive; the room's name, players, private or public
    (a public room is listed with the save's climate and year), rules
    and a password; **Create room**;
  - **Server...** and **Your banner** (the game's pictures a player shows
    on their card, and under **Characters** the campaign's portraits this
    game has, "Portraits" below, both sent as `set_banner`);
  - **in a room**, three tabs of the game's `TabWidget`:
    - **Room**: the world as a big card (for the owner, a click picks
      another save and its mods); the players as cards of their banners
      (a portrait, if they picked one, beside the card) with their marks
      (owner, you, ready, away, how many of the room's mods they lack or
      have in another version) and, for the owner, a Remove in a card's
      corner that asks first; the room's card: the invite code, large,
      with **Copy** (the hook puts it on the clipboard:
      `crate::clipboard`), players, play style, password, server and the
      room's mods; the chat. **Leave room** (asks first), **Ready** or
      **Not ready**, and, for the owner, **Start the game**, which waits
      until everyone is ready and is off, naming who, while a member's
      mods differ. Once the room's game runs, the chat and Leave stay;
    - **The room's mods (n)**, with how many are missing: a tile each, as
      the game's mod selector shows mods ("The room's mods" below);
    - **Only for you (n)**: this player's personal mods (D25), with the
      game's own Activate button (`choose_mod`); outside a room the same
      page opens from **Your mods** in the footer.

  `crates/tpf3mp-hook/src/lobby/window_tests.rs` draws the window in every
  view against a stand-in for the menu (`tests/lua/fake_menu.lua`), clicks
  its buttons, and parses every action it sends as the hook does.

The pause menu has no Multiplayer entry: in the room's game, the game
bar's line and the Multiplayer window it opens are the room's (PLAYING.md,
"While you play"), and a copy of the pause menu would be one more game file
to carry over on every patch.

## The room's save and mods

The owner picks the room's save, its mods and their settings on the
game's own **Load Game** page (`gui/menu/load_game_page.tl`), from the Host
page's world card or the room's: its save tiles, its details with the
**Mods** and **Gameplay Settings** tabs (the game's mod selector, with its
order, dependencies, warnings and presets), as the player knows them.
`roommods.lua` (`roommods.begin`) swaps a few of the page's module helpers
for the time of the pick; the page reads them at each draw:

- its title says what it picks for (`menu_icon_react_util.makePage`);
- its Load Game button reads **Use for the room** and plays no game-start
  sound (`makePrimaryButton`, by its class `loadSavegameButton`);
- a save tile opens its details instead of loading
  (`savegame_react_util.SavegameCard`);
- the list of saves starts with a **New world** tile
  (`tile_list_react_util.TileList`): the room then starts from a world the
  game's New Game page makes. Picked in a room that had a save, the
  launcher stops offering that save, so the room's page shows **Set up
  world** instead of Ready: the owner cannot ready, nor start, a room
  without a world;
- its load (`app.loadGame`, after `app.setWaitForStartReadyGame`) takes
  the save and what the page holds for it (its mods, in the game's
  activation order, each with its name and source from the game's
  `ModRep`, and the settings, `modParams`, the game's own under `""`)
  instead of loading.

The page's own nodes are never replaced by others: the game's react
asserts and closes the game when a node's recipe changes between draws
(`oldNode->recipeId == newNode->recipeId`, seen 2026-10-04 when a text
turned into the mod selector), and `pcall` cannot catch it. So each swap
returns the same recipes, with other parameters.

Everything is put back as soon as the pick ends: picked, or the page left
(`roommods.finish`, which `main_page.tl` calls when the main page mounts
again, and which reopens the Multiplayer page). Should one of the helpers
not be as the mod knows it (a game patch), nothing is swapped, the pick
does not begin, and the page says so. The window sends the pick as
`choose_room_mods` (bridge version 25) with the save, its map and year:
in a room at once; on the Host page once the room is made. MODS.md,
"Choosing mods", says what the launcher makes of it.

## The room's mods, and installing from Mod Hub

The room's mods tab shows each of the room's mods as the game's mod
selector shows mods (`tile_list_react_util.TileElement`): its logo from
Mod Hub (`mod_manager_react_util.ModImage`; the game's placeholder for a
mod without one), its name, whether this player has it (**Installed**,
**Missing**, **Another version**), where it comes from (Mod Hub, a local
mod, a DLC, built in), and the game's Details button, which says its id,
versions and source. **Only missing** hides the rest.

A guest installs a missing Mod Hub mod from there, with their own game and
Mod Hub account; mods never pass between players, and the launcher never
talks to mod.io (D28 proposed):

- **Install** on a tile opens the game's own Mod Hub page of the mod
  (`mod_manager_react_util.ModDetailsWindow`, as the game's Mod Hub opens
  a mod's tile; `roommods.showDetails`), after the game's own access check
  (`checkModManagerAccess`, which says itself why not): its pictures,
  description, author and the game's **Subscribe**. Closed, an install
  begun there is followed on the tile.
- **Install all missing (n)** looks each up in this player's Mod Hub
  first (`getModDetailsAsync`) and asks once, in place of the tiles: each
  mod as Mod Hub names it, with its logo, author, size and number, a
  **Mod Hub page** button, **Cancel** and **Subscribe & install**
  (`subscribeModAsync`). The owner's Mod Hub number is only their claim,
  so the player sees what it resolves to before anything is subscribed;
  a mod whose name differs from the room's is marked.
- Without the game's window container (an older menu) a tile's Install
  asks the same way.
- Not signed in to Mod Hub, the tab says so and offers the game's Mod Hub
  page (`onModHub`, back to the Multiplayer page). Without Mod Hub, it
  says it is not available.
- While Mod Hub downloads, the tile says **Installing...**. Once the game
  has it installed, the install counts only when the installed mod's id
  is the room's (`roommods.installedId`); then the launcher is asked to
  find the installed mods again (`rescan_mods`). Until it does, the tile
  says **Installed, not found yet** with **Look again**. The game takes a
  new mod at its main menu without a restart (seen 2026-10-04: Signal
  Distance subscribed, downloaded and found within about three seconds).
- Answers from Mod Hub can arrive several before the page draws again;
  each one changes what the one before it changed (`changeInstalls`), so
  none is lost.
- Each Mod Hub request returns a handle (`UniquePendingRequestId`) that
  aborts the request once Lua collects it; the game's own pages keep
  theirs (`createAsyncRef`). The lobby keeps each one until its answer
  comes (`roommods`, `keep`). Without that, a guest's mods stayed at
  **Looking up...** for good (playtest 2026-10-06: nine at once).
- A lookup Mod Hub has not answered within 30 seconds, or a subscription
  it has not taken by then, fails (**Install failed**, "Mod Hub did not
  answer") and can be installed again; an answer that comes later changes
  nothing. An answer this mod cannot read fails too, instead of waiting.

A mod not from Mod Hub cannot be installed from the lobby: its tile says to
ask the owner where to get it.

## The build profile

The entry's targets are in the release's built-in profile for the build
(`profiles/tf3_build40408_steam_windows/hooks.toml`, `docs/HOOKS.md`), next to the
step gate's: `lua_loadfile` is detoured, the others only called. The first
three are optional there: without them the menu stays the game's and the
step gate still installs. `tpf3mp-hookcore/tests/tf3_static_proof.rs` pins
their addresses in the installed game (`TPF3MP_TF3_EXE`).

| target | what | how to find it again |
|---|---|---|
| `lua_loadfile` (0x2fa1d50) | the `resolveutil.loadfile` body | `Loader.cpp`: the function with the strings `Could no load file`, `base/tl.lua`, `Error while pcalling` |
| `lua_load` (0x2fbdf70) | Lua 5.2 `lua_load` | the only caller of `luaD_protectedparser` (the function that references the `attempt to load a %s chunk` check); `luaZ_init`, a `"?"` default chunk name, then the `_ENV` upvalue fix-up. Not the nearby `lua_dump`, which checks for a Lua closure on the stack top and returns 1 |
| `lua_pcallk` (0x2fbe0c0) | Lua 5.2 `lua_pcallk` | called right before `Error while pcalling` in the loader body; reads `L->top`, `L->stack`, `L->nny`, calls `luaD_pcall` |
| `lua_settop`, `lua_pushlstring`, `lua_tolstring` | Lua 5.2 | already the step gate's (the Lua link, `docs/HOOKS.md`) |

On a patch: `tpfre index` the new exe, find them again with `tpfre q`
(`str`, `callers`, `dis`), regenerate the signatures with `sig --toml`, and
put them in the new build's profile. The hook tries the menu's targets in
every profile that matches the build, the data folder's first.

## Talking to the launcher

The window asks the hook for the lobby through the request channel above
(`tpf3mp/state.lua`, a few times a second) and sends its actions the same
way (`tpf3mp/act.lua`). The hook does not answer on its own: an action is
queued for the launcher that started the game and handed to its agent over
the link (`ToAgent::Lobby`), and the state is the launcher's lobby as the
agent last sent it (`ToHook::Lobby`, bridge version 10). Every request also
reads the link, since at the main menu no step of the game does
(`crates/tpf3mp-hook/src/lobby.rs`; `docs/HOOKS.md`, "The main menu's
Multiplayer window"). The launcher carries the actions out as if its own
window had asked; Connect goes to its own server (D12). A game whose hook
has no link to its launcher shows so in the window and sends nothing. The
hook's answer to an action is `ok`, or `error: ` and why it refused it
(such as a name too long), which the window shows.

**Mods** (docs/MODS.md, "Choosing mods"). The lobby carries the player's
installed mods (`mods`: `{ id, name, class, reason, chosen, choosable }`,
class `personal`, `carried` or `shared`, those the player may choose first,
64 at most) and the room's shared mods once known (`room_mods`: `{ id,
version, have }`, have `yes`, `no` or `other_version`, 32 at most, and
`room_mods_more` beyond). The window chooses one with
`{"action":"choose_mod","id":"<id>","chosen":true|false}`; the launcher
refuses a mod that is not choosable, with why. Bridge version 13.

**The server** (D12, proposed amendment). The lobby carries the server as
players see it (`server`, its name or address), its address
(`server_address`, `host:port`) and the launcher's default
(`server_default`; empty without one). The window changes the server with
`{"action":"set_server","server":"host:port"}`, or `"server":""` to go
back to the default: the launcher refuses anything but a `host:port`, and
any change while in a room, with why; otherwise it remembers the server,
and if connected it disconnects and connects there under the same name.
An invite never changes the server. Bridge version 15.

**Several servers** (D12, PROPOSED amendment of 2026-10-06, not
decided). A release may list servers besides its default
(`TPF3MP_SERVERS`; `--more-servers`). A launcher playing on its default
then plays on all of them: Connect goes to the closest that answers, by
the round trip of its QUIC connection, and keeps a quiet connection, a
*lookout*, to every other (`launcher::servers`): no content, no room, no
diagnostics. `ListRooms` asks its own server and each lookout for the same
page and merges them, lobbies first, then the fuller, then by name, each
room with its server's name and ping (`server`, `ping_ms`; in the game's
lobby `server`, `ping`, and the list's `servers`: name, ping, `here`,
`reachable`; bridge version 26). Create first moves to the closest server
(pings within 10 ms count as equal: the current stays, else the first
listed). Join goes to the server the last list showed the room on; a code
it did not show is tried on the server played on, then on the other
servers that answer, closest first, while each answers `BadInvite`, and
the launcher comes back where it was when none has it. An invite naming
a server not on the list is refused. The state carries the servers
(`servers`: name, `ping_ms`, `here`, `reachable`), which the launcher's
Settings show. With one server, `--server`, or a server the player typed
in Settings, none of this happens and the lobby is as before.

**The start save.** The lobby lists the player's saves, newest first, by
name: those `steam::find_save` finds by that name, in the save folder of
the Steam account playing (`steam::list_saves`, looked at every 5
seconds). Create names one of them, or none. The launcher takes only a
name it listed, never a path, finds the file and hands it to the room as
`--start-save` does (`BridgeOptions::start_world`); a save it cannot find
creates no room. The launcher's own `--start-save` is offered first, then
the save last picked.

**Changing the start save in the room** (bridge version 20, protocol 14).
The Host page keeps its **Start from this save** (the launcher's save or
the newest picked, or none), so a room usually starts as it was made; the
room's page shows the save it starts from to everyone and lets its owner
change it until the game starts:

- The room in the lobby carries `start` (`{ name, map, year, arrived }`,
  as the room names it to every member: `RoomView::start`; `nil` when the
  owner's game provides the world) and, for the owner, `upload` (`{ save,
  percent }` while their pick goes up to the room). Others see the line
  under **Starts from**: the save's name, its climate and year when the
  room knows them, and "on its way to the room" until it arrived.
- The owner sees **Start from this save** instead: the Host page's saves,
  newest first, the room's own first if it has dropped off the list, and
  **New world: choose map and settings**. A pick sends
  `{"action":"choose_start","save":"<name>","map":"<climate>","year":<year>}`
  (`"save":""` for a new world), with the climate and year the game reads of the
  save (`lobby.saveDetails`, as the Host page lists a public room); the
  window waits up to eight polls for them. The launcher takes only a
  listed name, as Create does, works out the room's shared mods from the
  save again (`picker::Mods::own_start`), and the room session declares
  them and hands the save over (`Control::StartWorld`). The room marks
  everyone not ready, as it replaced the save they agreed to; each guest
  is told so, and presses Ready again.
- While the save goes up, the page shows "Sending <save> to the room: N%"
  with a bar (the share of its chunks served), and **Start the game** is
  disabled, as it is while the room does not have the save
  (`arrived` false), with "The save is still on its way to the room".
- A room made private names its save without a map and year; once the
  game has read them, the owner's window tells the room once, with the
  same save (`choose_start` with its name): the room only updates what it
  shows, and nobody is asked to agree again. The same save picked again,
  unchanged on disk, is not uploaded again either.

## Portraits

A player may show one of the campaign's characters instead of a banner:
25 of them, by the name the game gives their picture
(`tpf3mp_proto::PORTRAITS`, such as `dr_karl_brandt`). The id travels as a
banner id does (`SetBanner`, protocol 13; bridge version 19), the server
checks it against the same list, and the launcher remembers it in
`launcher.json`'s `banner`.

The pictures are the game's own art, so TPF3-MP never ships them, nor
anything made from them. Each player's launcher makes its own copies at
startup (`crates/tpf3mp-agent/src/portraits.rs`):

- it reads each campaign mission's `mission.zip` in the player's game
  (`<game>/mods/release/urbangames_campaign_mission_0N/content/`, N 1 to
  8) for `mission/dialogue/<id>_neutral.tga` (1024 pixels square), makes
  each 256 square, and writes it as the game writes its TGAs into the
  installed mod the game loads (the first `tpf3mp_1` in its mod folders,
  usually `staging_area/tpf3mp_1`) as
  `content/gui/tpf3mp/portraits/<id>.tga`, listed in that copy's
  `_content.json`; the windows load it as
  `tpf3mp_1::/gui/tpf3mp/portraits/<id>.tga`;
- it does this once per game build (`portraits/build.txt` names the build
  they came from): later starts only check the files are there and make
  any missing ones, as after the mod is installed again. All 25 take
  about 0.15 s;
- without the campaign's missions (another build, a game without them) it
  makes nothing, keeps what it has, and says so in the launcher's log.

The window offers only the portraits the launcher has (`LobbyView::portraits`).
A room member's portrait reaches either window only when this game has it
(`portraits::shown`); otherwise the member shows as before, their key's
banner. A member who picked a portrait shows their key's banner on their
card, with the portrait square beside it: in the main menu's room and on the
room page in the game (`tpf3mp/banners.lua`, `portraitOf`).

## The mod's copies

`mod/tpf3mp_1/content/gui/menu/main_page.tl` is a copy of the game's file.
Every change is marked `TPF3-MP:`; the relative requires and asset paths are
made absolute (`::/...`), because a leading-slash path is resolved against
the requiring file's root, which for the mod's copy is `tpf3mp_1::/`. On a
game patch, take the new game file and re-apply the marked blocks. The copy
is listed in `_content.json` like any other file of the mod.

## Trying it

Start the launcher, then **Start Transport Fever 3** (before or in a
room), and click **Multiplayer** on the game's main menu. The mod must be
installed in the game's staging area and active. After changing
`main_page.tl`'s additions, run `tools/lobby/make_main_page.py` on the
game's file again rather than editing the copy.

`tools/lobby/launch-tf3-dev.bat` and `tpf3mp-launch` start the game with the
hook but without a launcher: the entry and window appear, and the window
says the game has no link to the launcher. They are for checking the entry
alone.

## v1.1 menu journeys

The main-menu friend card uses `focus = "friend"`: it shows name, invite
and optional password even before connecting. One queued intention waits
for the named connection, then joins once; connection errors and a timeout
cancel it. Hosting and discovery are accessible before connecting too.
Polling compares the serialized lobby before setting GUI state, so idle
polls do not rebuild dropdowns while a player uses them.

Before the hook loads a room's snapshot, it calls the lobby's registered
close callback and yields a menu frame for the window removal. The loader
suspends menu callbacks, so waiting for the next room-state poll can leave
the old lobby frozen over the loaded world. A failed close refuses the load
and reports the error instead of hiding it.

Creating with an explicitly empty start-save opens the stock `NewGame`
page through the main page's navigation callback only after room creation
succeeds. The launcher's bridge starts that generated world once the owner
reports a loaded world and all members are ready. Existing-save rooms
retain their explicit Start button. No game-install files are changed.
Before opening stock setup, the menu adds `tpf3mp_1` to its active mod
selection without removing other mods. Otherwise a freshly generated world
could silently run without the multiplayer script while guests wait.

That is the only way a new world enters a room: in its lobby, before its
game starts, after which the room saves it before step 1 and every game
loads that save, the owner's too. The room's page offers **Set up world**
only then. A new game or a save started from the game's own menus while
the room's game runs is none the room loaded: the hook holds it before it
runs a single room's step, rather than let each game start it at another
step ([HOOKS.md](HOOKS.md), "A world the room did not load"). To play the
room's world again, leave the room and join it again.

The launcher retains its hook link between rooms. The hook therefore resets
its menu-arrival notification when the lobby invite changes, even if the
link generation is unchanged. After a running room ends, returning to the
main menu resets its completed gate; an early Begin for the next room is
preserved until that menu transition. A running world cannot reset its gate.
When Leave or Disconnect is requested after returning to the menu, the hook
drains the old session through its real End message. Queued simulation steps
are neither executed nor reported as executed. This prevents an unrun step
from blocking the leave response and the next room on the same game process.

### Native acceptance, 2026-10-01

On Windows with Steam build 40408, two games started by the launcher against
a local server exercised the native menu. This used normal menu actions,
not console-created rooms or a prepared save:

- Offline **Join a friend** connected and joined from the name/code form;
  public browsing and joining also worked.
- Hosting **New world** opened the stock climate/settings/mods screens.
  Cancelling returned to the room, where **Set up world** resumed setup.
  TPF3-MP was selected automatically while existing mod choices were kept.
- A small temperate European world, starting in 1900 with the tutorial off
  and only the stock DLCs plus TPF3-MP selected, generated successfully.
  Finishing setup began multiplayer automatically. Both games loaded the
  shared snapshot and displayed **Worlds match**.
- A guest joined the running room from its main menu. The lobby disappeared
  before loading, and the in-game multiplayer panel opened and closed.
- That same guest process used **Quit → Return to Main Menu**, left the
  room, then joined it again by invite without restarting either the game
  or launcher. It loaded the shared world and displayed **Worlds match**
  again; the host stayed in the world throughout.

These checks found and reproduced the generation-time Lua-state race, the
frozen menu overlay during loading, and a leave response blocked behind an
unrun simulation step. Each correction has focused regression coverage.
