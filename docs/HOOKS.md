# The native hook

The native hook is the small library that runs *inside* the game process. It
captures and cancels player commands, gates the simulation step, controls speed
and save/load, and talks to the [agent](ARCHITECTURE.md#components) over
shared memory. This document specifies the parts built at milestone M0: the
build-signature engine, the per-build profile format, the detour engine, the
shared-memory ABI, and the release-day procedure. Locating and detouring the
actual TPF3 functions comes with the release-day profile (see
[DAY_ONE.md](DAY_ONE.md)); everything the hook needs to do it is here and
tested.

Crates:

| crate | contents |
|---|---|
| `tpf3mp-hookcore` | pattern scanning, per-build profiles + resolution, the x86-64 inline detour engine, a small read-only PE reader |
| `tpf3mp-ipc` | the shared-memory link (this document's ABI) |
| `tpf3mp-hook` | the `cdylib` loaded into the game: platform entry points, profile loading, agent connection |
| `tpf3mp-launch` | starts the game with the hook in that one process, and nowhere else |

## How the hook gets into the game

Only the TPF3-MP launcher puts the hook into the game, into the game it
starts for a room, and for as long as that game runs (D11 in
[DECISIONS.md](DECISIONS.md)). Nothing is installed into the game's folder
and no launch option is set: the game started from Steam is the plain game.

- **Windows.** `tpf3mp-launch` starts the game suspended, writes the hook's
  path into it, and runs `LoadLibraryW` on a thread it creates in the game.
  It checks the thread's result and the game's module list, and only then
  lets the game run. If the hook is not there, it ends the game rather than
  let it run without it. This is how TPF2MP's injector started Transport
  Fever 2 (`--launch`).
- **Linux.** The game starts with `LD_PRELOAD` naming the hook, in its own
  environment only, ahead of anything already preloaded (Steam's overlay).
- **macOS.** Not yet. The game's hardened runtime refuses libraries it did
  not load itself; how to get the hook in is a release-day question
  ([DAY_ONE.md](DAY_ONE.md)).

Before starting the game, the launcher checks four things, and refuses if
any fails:

- the player is in a room;
- the game and the hook are where they should be;
- Steam is running;
- no game it started is still running.

It gives the game `SteamAppId` and `SteamGameId` (3493540), so that the
game does not restart itself through Steam, which would start it without
the hook.

The hook runs only when the launcher started the game. The launcher names
two things in the game's environment:

- its link, `TPF3MP_GAME_LINK`;
- its own process, `TPF3MP_LAUNCHER_PID`.

Without the link the hook returns at once: it writes, hashes and opens
nothing. On Linux and macOS it also returns when its process's parent is
not that launcher. Programs the game starts, a browser opened from the
game for example, inherit `LD_PRELOAD` and the variables; the hook stays
out of them. On Windows nothing the game starts loads the hook.

## Design and the fail-closed rules

TPF3 will be patched often after launch, so the hook never pins raw addresses.
It carries one **profile** per game build. A profile binds a build identity
(executable SHA-256, and optionally file size and PE timestamp) to a set of
named **targets**, each located by a byte **signature** rather than an address.

Resolution is **fail-closed**. `tpf3mp_hookcore::profile::resolve` returns either
a complete, verified target table or a precise refusal, and it installs nothing
on the way to a refusal:

- **Unknown build** - the running executable's identity does not match the
  profile. The hook does not scan at all; multiplayer is disabled.
- **Missing** - a *required* target's signature is not found. Resolution fails
  as a whole, so required hooks are all-or-nothing; a partial install never
  happens.
- **Ambiguous** - a signature matches more than once. Refused, even for an
  optional target: a second, unexpected match is a corruption signal, not
  something to skip.
- **Prologue mismatch** - the signature matched but the exact bytes at the
  target are not the ones the profile expects. Refused.

An *optional* target that is simply absent is recorded and does not fail the
profile. Everything else fails closed. The hook logs the precise reason and
leaves the game untouched.

### Where the resolver scans (production vs. this repo's test)

`resolve` takes a byte slice plus the address its first byte corresponds to, so
it does not care whether those bytes come from a file or from memory.

- **Production**: the hook scans the running process's **mapped, unpacked module
  image** - the bytes the loader (and any DRM stub) produced in memory - passing
  the module's base address as the region base. This is the only correct source
  when a build's code section is packed or encrypted on disk.
- **This repo's static proof** (`tpf3mp-hookcore/tests/tpf2_static_proof.rs`)
  scans the executable **on disk**. That is a development convenience, valid
  only when the build's `.text` is readable on disk (see
  [the TPF2 verification](#what-was-verified-on-the-tpf2-binary)). The call is
  identical; only the byte source differs.

## Signatures

A signature is an IDA-style pattern: two hex digits per fixed byte and `??` (or
`?`) for a byte that may be anything, for example `48 8B ?? ?? E8`. Wildcards
exist so a signature skips the bytes that move between builds - RIP-relative
displacements, call targets, absolute addresses - and matches only the opcodes
and operands that identify the code. A signature must be **unique** across the
scanned region; the scanner reports zero, one, or many matches, and the resolver
treats "many" as a refusal.

Rules of thumb, applied to the TPF2 profile below:

- Prefer register/immediate operands; wildcard every relative or absolute
  displacement.
- Extend the pattern only as far as needed to make it unique. Two functions can
  share a prologue (the two TPF2 menu functions share a seven-`push` opening);
  run the signature to the first distinguishing bytes.
- Keep the **prologue** field free of wildcards: it is the exact code the detour
  engine relocates, and it is re-checked byte-for-byte after the scan.

## The profile format

A profile is TOML. `tpf3mp_hookcore::profile::Profile::from_toml` parses and
validates it.

```toml
name = "Transport Fever 2 Build 35924 (Windows x64)"
image_base = 0x140000000   # informational: what RVAs are relative to
region = ".text"           # informational: the section the resolver scans

[build]
sha256 = "782b904a8f7bbdac1f7a18528f1a5c778691e5aa3087c37c351bf6912585175c"
size = 72843280            # optional; checked when present
pe_timestamp = 0x675ABCC6  # optional; checked when present

[[target]]
name = "GameSim::Step"
signature = "40 53 41 56 48 83 EC 68 48 8B DA 4C 8B F1 48 81 FA E8 03 00 00"
offset = 0                 # bytes from the match to the target (default 0)
prologue = "40 53 41 56 48 83 EC 68 48 8B DA 4C 8B F1 48 81 FA E8 03 00 00"
required = true            # default true
```

- **`signature`** locates the target. **`offset`** (signed, default 0) is added
  to the match position to reach the target address, for the case where a
  signature must begin before or after the function it names.
- **`prologue`** is the exact, wildcard-free bytes expected at the target; the
  resolver verifies them and the detour engine relocates them.
- **`required`** (default `true`): a required target that does not resolve
  refuses the whole profile.

A resolved target's address is `region_base + match_index + offset`.

## The detour engine

Trampolines are allocated within 1 GiB of their target, so a relocated
RIP-relative operand keeps a 32-bit displacement to the data it addresses.
On Windows the free regions around the target are walked; on Unix, mmap
hints step away from it. A trampoline is written while read-write, then
turned read-execute; it is never writable and executable at once. On
Windows, the prologue read stops at the end of the target's memory region.
A relocation that still cannot reach fails the install cleanly
(`DetourError::Encode`); it never produces wrong code.

`tpf3mp_hookcore::detour::InlineDetour` is an x86-64 inline hook. Installing it
overwrites a function's first instructions with a jump to a replacement, after
copying those instructions into a **trampoline** that ends by jumping back into
the function; calling the trampoline therefore runs the original.

- **Relocation.** The stolen prologue is decoded and re-encoded at the
  trampoline's address with iced-x86's block encoder, so a RIP-relative operand
  keeps addressing the same absolute memory from its new home. A prologue that
  cannot be relocated - it branches, returns, or does not decode - is **refused**
  (`DetourError::UnsupportedPrologue`) rather than patched wrong. Only
  straight-line instructions are stolen.
- **Patch form.** A near replacement (within 2 GiB) is reached with a 5-byte
  `jmp rel32`; otherwise a 14-byte `jmp [rip+0]` absolute jump. The trampoline
  always returns with an absolute jump, so it works at any distance.
- **iced-x86, not `retour`.** `retour`'s stable line is 0.3.1 (0.4 is alpha) and
  it owns trampoline allocation and instruction relocation internally - exactly
  the part that must be inspectable and testable on a binary that shifts every
  patch. iced-x86 is a pure-Rust decoder/encoder with no build script; the
  engine drives decode/relocate directly and keeps the trampoline and patch
  bytes in this crate, where tests read them.
- **Architecture.** The engine is x86-64 only. On any other architecture it
  compiles to a stub that returns `DetourError::UnsupportedArchitecture`, so the
  workspace still builds and the caller fails closed (see
  [macOS arm64](#macos-arm64)).

### Thread-safety assumptions

Installing overwrites up to fourteen live code bytes with a non-atomic copy. The
caller must guarantee the target cannot execute during install or uninstall:
**install before the target's first run**, or **park every thread that could
reach it first**. The launcher loads the hook before the game's entry point
runs (into the suspended game on Windows, by `LD_PRELOAD` on Linux). The hook
installs from its bootstrap thread while the game starts, as TPF2MP's injector
let its worker do: the targets run only once a world is loaded, long after.
The engine does not stop threads itself. Detours are removed by
dropping the handle (or `detach`), under the same quiescence rule. The engine's
own tests only ever hook functions inside the test binary, never another
process.

## The `tpf3mp-ipc` ABI

The hook and the agent share one memory mapping: a fixed 64-byte header followed
by two single-producer/single-consumer ring buffers. This section is
byte-exact, because the agent is written separately.

### Object naming and security

The logical link name is mapped to a per-user OS object:

- **Windows**: `CreateFileMappingW`/`MapViewOfFile` in the per-session `Local\`
  namespace, object name `Local\tpf3mp.<hash>` where `<hash>` includes the user
  name. No explicit security descriptor is passed, so the mapping gets the
  process token's default DACL - access for the creating user and SYSTEM only.
- **Linux/macOS**: `shm_open`/`mmap` with mode `0600` (owner only). The name is
  `/tpf3mp.<hash>`, where `<hash>` includes the uid; it is kept within macOS's
  31-character `shm_open` limit.
- **Other users** cannot reach the link. **The same user's processes can.**
  Creation is not exclusive, because the agent re-creates the mapping after
  a restart while the game still holds it (see "Restart"). A process of the
  same user could therefore create the object first, or write into it. That
  process can already debug or inject into the game, so this opens no new
  boundary. Each side still treats the other's bytes as hostile:
  - `open` refuses sizes `create` would refuse;
  - no frame is read beyond its ring;
  - a producer refuses a consumer index claiming more than the ring holds;
  - `next_len` never exceeds `max_message`.

  A hostile peer can garble or stall the link, but cannot make this side
  read or write out of bounds (`tpf3mp-ipc/tests/poc_hostile_peer.rs`).
  Nor can it make the agent take in, and so delete, a file other than a
  save in the directory the agent named (`tests/hook_save_path.rs` in
  `tpf3mp-agent`). A game that stops reading makes the agent stop taking
  the room's turns once about 16 MiB wait for it, rather than hold them
  all.

### Header layout (little-endian)

Total mapping size is `64 + 2 * ring_capacity` bytes.

| offset | size | field | notes |
|---|---|---|---|
| 0  | 4 | `magic` | `T3MP` (bytes `54 33 4D 50`), written **last** as a readiness flag |
| 4  | 4 | `abi_version` | currently `1` |
| 8  | 4 | `header_size` | `64` |
| 12 | 4 | `ring_capacity` | bytes per ring; power of two, `<= 2^31` |
| 16 | 4 | `max_message` | largest payload per message |
| 20 | 4 | `session` | non-zero link generation; changes on re-create |
| 24 | 4 | `hook_pid` | 0 until the hook attaches |
| 28 | 4 | `agent_pid` | 0 until the agent attaches |
| 32 | 8 | `hook_heartbeat` | `u64`, bumped by the hook |
| 40 | 8 | `agent_heartbeat` | `u64`, bumped by the agent |
| 48 | 4 | `h2a_head` | hook->agent read index (consumer: agent) |
| 52 | 4 | `h2a_tail` | hook->agent write index (producer: hook) |
| 56 | 4 | `a2h_head` | agent->hook read index (consumer: hook) |
| 60 | 4 | `a2h_tail` | agent->hook write index (producer: agent) |

Then the data areas: hook->agent at `[64, 64 + ring_capacity)`, agent->hook at
`[64 + ring_capacity, 64 + 2 * ring_capacity)`.

### Rings

Each ring is a byte stream carrying length-prefixed messages: a little-endian
`u32` payload length followed by that many payload bytes. Both the length and
the payload may wrap around the end of the buffer.

- `head` and `tail` are **free-running** `u32` counters (they wrap at `2^32`,
  not at the capacity). Bytes in the ring = `tail - head` with wrapping
  subtraction; this is correct because capacity is a power of two `<= 2^31`. The
  index into the data area is `counter & (capacity - 1)`.
- **Producer**: writes the payload bytes, then stores `tail` with **Release**.
  It reads `head` with **Acquire** to compute free space; it only writes `tail`.
- **Consumer**: reads `tail` with **Acquire**, reads the bytes, then stores
  `head` with **Release**. It only writes `head`.
- A message larger than `max_message` is rejected by the producer; a full ring
  returns "full". Nothing is allocated on either side of a send or receive.

### Startup, heartbeat and restart

- **Startup.** One side (in TPF3-MP, the agent) is the owner: it creates the
  mapping, zeroes the header, writes the ABI version, ring sizes and a fresh
  non-zero `session`, sets its pid and heartbeat, and **publishes `magic` last**
  with a Release store. The other side opens the mapping and reads `magic` with
  an Acquire load; until it appears the open returns "not ready". The opener
  then checks `abi_version` and `header_size`, reads the ring sizes, and sets
  its own pid and heartbeat. The hook fails closed (runs solo) if no mapping is
  present.
- **Heartbeat.** Each side bumps its own counter and reads the peer's. A counter
  that stops advancing means the peer is gone.
- **Restart.** The owner re-creates the mapping with a new `session`. A peer
  that sees `session` change knows the rings were reset and drops anything in
  flight, then re-syncs from the new generation.

### Several games on one PC

The hook opens the link its launcher names, and keeps its log (`hook.log`)
and build profiles (`profiles/*.toml`) in the per-user `TPF3-MP` data
folder. The game's environment says which, so several games on one PC each
reach their own agent:

| variable | effect |
|---|---|
| `TPF3MP_GAME_LINK` | the link name to open, the launcher's `--game-link` (`tpf3mp.default` unless given); without it the hook does nothing |
| `TPF3MP_LAUNCHER_PID` | the process that started the game; on Linux and macOS, the hook does nothing in a process whose parent is another |
| `TPF3MP_DATA_DIR` | the folder for the hook's log and profiles; unset or empty, the per-user one |

`tpf3mp-fakegame` reads `TPF3MP_GAME_LINK` too, when no link is given on its
command line. The multiplayer rig (`tpf3mp-rig`, in
[DEVELOPMENT.md](DEVELOPMENT.md)) sets all three for every game it starts, and starts a real
game with the hook in it as the launcher does.

## The bridge: what travels over the link

`tpf3mp-bridge` defines the messages, postcard-encoded, one per ring frame,
at most 60 KiB each. It has no async runtime or network code, so the hook can
link it. The agent's side is `tpf3mp_agent::bridge`.

- **From the agent (`ToHook`):**
  - `Hello`: always first.
  - `Begin`: a game starts, and saves go in this directory. It also names the
    room's rules: with `native`, the game's own economy runs untouched;
    with canonical rules, the Lua mod shows the server's values instead.
  - `Load { file, next_step }`: load a world, then run `next_step`.
    Without a file, the game loads the world the player chose to start
    from: the owner's, or everyone's on a server that keeps no snapshots.
    With one, a save the room agreed on: the owner's world at the start,
    or the room's latest for a player who joins a running game, could no
    longer resume, or is rebased after diverging. Everything sent before
    a load is void.
  - `Apply(event)`: apply this event before its step. A `Save` event is not
    applied: the session saves the world there (see below).
  - `Release { through }`: steps up to and including this one may run.
  - `Speed`: the room's speed, for display only.
  - `Diverged`, `Refused`: tell the player.
  - `Chat { from, text }`: a member of the room said something. Sent only
    once the game has begun; talk in the lobby stays in the launcher.
  - `End`: the session is over.
- **From the hook (`ToAgent`):**
  - `Hello`: always first, with the game build.
  - `Loaded { next_step }`: the ordered world is loaded.
  - `Command { payload }`: the player acted; the room orders it.
  - `Ran { step }`: the game ran this step.
  - `Checkpoint { step, lanes }`: digests at a checkpoint.
  - `Saved { event, lanes, file }`: the world as saved at a save event, and
    its digests there; no file if saving failed. The file must be in the
    directory `Begin` named; the agent takes in no other.
  - `Chat { text }`: the player says something to the room
    (`Session::chat`).
  - `Log`: a line for the agent's log.
- **The step gate.** The game asks the hook's `Gate` before every step. Until
  the step is released, the hook reads messages and applies each event the
  gate hands over, so an event for step `s` is applied after step `s - 1`
  and before step `s`, never mid-step.
- **Ordering.** The agent sends every event for step `s` after the release of
  step `s - 1` and before the release of step `s`. It only merges releases
  of consecutive steps with no event between them. The hook stops reading
  once its next step is released. The gate refuses anything that breaks
  this: an event for another step, an event after its step's release, or a
  release that goes back. The hook must then stop following and say so.
- **Pacing.** The agent releases steps on its jitter-buffered schedule
  (`Playout`). The game runs a released step at its own speed and waits at
  the gate for the next one. It reports each step it ran; the agent reports
  progress to the server from that, at most every 20 ms.
- **Liveness.** The hook must beat its heartbeat from a thread of its own,
  since the game thread blocks while loading. The agent gives up on a hook
  whose heartbeat stands still for 60 s.

### The hook's session

`tpf3mp_bridge::Session` is the hook's whole side of the link, run on the
game thread. The game-specific part of the hook implements the `Game` trait
and calls the session from its detours:

- **Startup.** `Session::attach(DEFAULT_LINK, build, patience)`, then
  `wait_for_begin()`. The gate's first answer is `StepGate::Load`.
- **Loading.** Whenever `before_step` or `poll_step` answers
  `StepGate::Load(load)`, replace the world: with the save `load.file`, or
  without one with the world every player starts from. Then call
  `loaded(load.next_step)`. Call `heartbeat()` while loading.
- **Before each simulation step.**
  - `before_step(&mut game)` blocks until the room releases the step,
    calling `Game::apply` for each event on the way. A pause can hold it
    there for as long as the pause lasts.
  - A game whose simulation shares a thread with its rendering, which must
    never block, calls `poll_step` instead. It returns `Wait` until the
    step is released, and the detour skips the step for that frame.
- **After each step.** `after_step(&mut game)` reports it, and at
  checkpoint steps sends `Game::lanes()`.
- **Saving.** At a save event the session calls `Game::save(file)`, then
  `Game::lanes()`, and reports both. The save must hold everything needed
  to continue from that point, because it is what other players load. The
  agent cuts it into its chunk store and deletes the file.
- **When the player acts.** Capture the action before the game applies it
  locally and call `command(payload)`. For a build the payload is the
  action the Lua mod handed over as a table, converted by
  `tpf3mp_proto::lua` and encoded with `Action::to_payload` ("The action
  schema" in [BUILDING.md](BUILDING.md)). The action happens only when the
  room's event comes back through `Game::apply`, on every replica alike.
- **Notices.** `Game::notice` receives speed changes, refusals,
  divergences and the end of the session, for the game's UI.

The session gives up (`AgentGone`) only when the agent's heartbeat stands
still for its patience, never merely because a step is withheld.

`tpf3mp_testkit::fake_hook` implements `Game` for the toy game and runs it
through `Session`: the exact code the real hook will run, over the real
link. The `games_behind_the_bridge_and_gate_agree` scenario runs three of
them in one room end to end; others have a player join a running game,
rebase a replica that drifted, and hand a world on across a server restart.
`tpf3mp-fakegame` does the same as a separate process, for trying the stack
by hand (see [DEVELOPMENT.md](DEVELOPMENT.md)). On release day, what remains for TPF3 is:

- the build profile with its signatures;
- the detours that call the session;
- `Game` for the real world: applying an event means executing the player
  command it carries, the lanes are digests of the game state, and saving
  and loading use the game's own save format;
- checking that a save is complete and loads on every platform
  (DAY_ONE.md);
- checking a received save's script data before the game loads it
  (`tpf3mp_agent::save_check`, DAY_ONE.md section 7);
- on the server, a `Ruleset` that validates TPF3's command format and
  applies the canonical economy, with `save` and `restore` so its rooms'
  logs compact (`crates/tpf3mp-server/src/ruleset.rs`). It is added to
  the server's `RulesMenu` next to `native`, which stays offered.

### The Lua side

The Lua mod runs in the game's GUI state, started by a game bar plugin on
the first frame of a game (`mod/tpf3mp_1/content/gui/tpf3mp/`). The hook
and the mod meet through one global table, which the hook registers in
that state before the mod starts. Its contract is in
`mod/tpf3mp_1/content/scripts/tpf3mp/bridge.lua`:

- `tpf3mp_native.version`: 2. The mod refuses any other.
- `tpf3mp_native.command(action)`: an action table, in the game's units.
  The hook reads it into a `tpf3mp_proto::lua::LuaValue`, within
  `MAX_DEPTH` and `MAX_NODES`, converts it with `action_from_lua`, and
  passes `Action::to_payload` to `Session::command`. It returns `true`, or
  `false` and the conversion's error. `false` or an error means refused,
  and the mod then does not apply the action locally either.
- `tpf3mp_native.register(handlers)`: the mod's handlers, which the hook
  keeps with the Lua state they came from.
  - `handlers.apply(action)`, with the table `action_to_lua` makes of the
    event's action, returns `(ok, reason)`; it is
    `Game::apply`. This version refuses every event, so a hook that
    receives one stops following the room.
  - `handlers.notice(kind, text)` is `Game::notice`.
  - Neither raises: an error becomes a refusal.
- `tpf3mp_native.log(line)`: a line for `hook.log`.

Without the table, the mod logs "no hook in this game" and does nothing.
That is every game Steam started (D11).

## Release-day procedure: adding a target for a new build

The first TF3 build's targets are already located (RVAs, RTTI/source
names) in
[investigation/TPF3_RECON_2026-09-29.md](../investigation/TPF3_RECON_2026-09-29.md):
the command queue (`CommandList::Add`), the sim step (`GameSim::Step`,
`CGame::RunGameSimLoop`), `CGameTime`, the two-`GameState` swap, the
player/company commands, and a lockstep step-budget global. This procedure
turns each into a verified profile target; the recon page also lists the
reconciliations to settle in-game first (e.g. `CommandList` vs
`DeferredCommandBuffer`).

1. **Archive the build.** Record the executable SHA-256, file size and PE
   timestamp (`BuildIdentity::of_file`), plus the Steam build/manifest ids. Keep
   a private copy (see [DAY_ONE.md](DAY_ONE.md)).
2. **Find the function** with the RE pipeline, and note its RVA and the bytes at
   its start. `tools/tpfre` indexes the executable in seconds and answers
   `func`, `callers`, `xrefs`, `str`, `dis` and `whois` queries on it
   ([its README](../tools/tpfre/README.md)).
3. **Write a signature.** Take the opening bytes; replace every relative or
   absolute displacement with `??`; extend only until the pattern is unique
   across the scanned section. Record the exact, wildcard-free `prologue` (at
   least the number of bytes the detour must steal - 5 for a near hook, 14 for a
   far one, on an instruction boundary).
4. **Add a `[[target]]`** to the build's profile with `name`, `signature`,
   `offset`, `prologue` and `required`.

   `tools/re/make_profile.py` does steps 3 and 4 for x86-64 builds, from the
   binary and the symbol map `name_functions.py` wrote:

   ```
   python tools/re/make_profile.py TransportFever3.exe out/TransportFever3.symbols.json \
       GameSim::Step CGame::Step -o profile.toml
   ```

   It writes the `[build]` identity (SHA-256, size, and the PE timestamp on a
   PE) and one target per function, with `offset = 0`. Displacements it
   wildcards: branch and call targets (rel8 and rel32), RIP-relative operands,
   and immediates or absolute displacements that point into the image. The
   signature starts as the prologue's instructions and grows one instruction at
   a time until it matches once in the function's on-disk section, never past
   the function's end or `--max-length` (128) bytes. The prologue covers
   `--steal` bytes, 14 by default: a far jump, since how far the detour lands is
   only known at install. Like the engine, it refuses a prologue holding a
   branch, call, return or interrupt, and keeps RIP-relative data operands,
   which the engine relocates. Every refusal names the target and the reason: a
   name shared by several functions (pick one with `NAME@0xRVA`), a function
   byte-identical to another (never unique), a branch too early to steal around.
   `tpfre q <db> sig NAME --toml` applies the same rules to one function and
   prints its `[[target]]` block (identical to make_profile's on TPF2's
   targets), to try a target before writing the profile.
   `tools/re/test_make_profile.py` checks the tool on a synthetic PE and keeps
   `tpf3mp-hookcore/tests/data/make_profile_fixture.{pe,toml}` current, which
   `tests/make_profile_fixture.rs` resolves with hookcore itself.
5. **Verify.** Resolve the profile against the **in-memory module image** of the
   running build and confirm the target resolves uniquely to the expected
   address and that the prologue matches. Keep a static check against an
   archived copy where the code section is readable on disk.
6. **Never widen a signature to force a match** on a build you have not archived.
   An unknown build must stay unknown, so the hook fails closed.

## What was verified on the TPF2 binary

Against `TransportFever2.exe`, Steam build 35924 (SHA-256
`782b904a...585175c`, size 72,843,280, PE timestamp `0x675ABCC6`, image base
`0x140000000`), the profile in `tpf3mp-hookcore/tests/data/tpf2_build35924.toml`
resolves all five targets, each **matching exactly once** across `.text`:

| target | RVA |
|---|---|
| `GameSim::Step` | `0x15aa00` |
| `CGame::Step` | `0x118e90` |
| `CGameTime::GetSpeed` | `0x2877a0` |
| `UI::CMenuUI::StartSavegame` | `0x6785c0` |
| `UI::CMenuUI::CreatePage` | `0x663370` |

`CGameTime::GetSpeed` sits next to two near-identical siblings, so its signature
runs past the (wildcarded) call to the distinguishing `mov eax,[rax+4]`;
`StartSavegame` and `CreatePage` share a seven-`push` prologue, so each signature
runs to its distinct `lea`/frame bytes (and `CreatePage` to the `mov
[rsp+0x330],rbx` store that separates it from a twin at `0x215c480`). The test
also confirms the resolver refuses a modified copy (corrupting one target's
bytes yields a `Missing` refusal) and refuses a mismatched build identity.

**DRM note.** This build carries a SteamStub section (`.bind`, high entropy),
which can decrypt code at load time. For build 35924 the code section is
nonetheless **readable on disk**: all five prologues match the on-disk `.text`
exactly, consistent with the RE survey's ~88,000 assert-string references found
in the same on-disk section. On-disk verification is therefore valid *for this
build*. It is not guaranteed in general - a future build could encrypt `.text` -
which is why the production resolver scans the in-memory, unpacked module image,
and why on-disk scanning is documented as a development convenience only.

## What a shipped mod hooks on the same build

Build 35924 has a shipped lockstep mod hooking it, [TpF2 Multiplayer](https://github.com/silver2127/tpf2-multiplayer)
(0.6.1.12, 2026-09-20), so every target in the profile and everything in this
section runs in players' games rather than in a test. RVAs are from image base
`0x140000000`. Names are the ones the binary carries in its `__FUNCSIG__` assert
strings where it has one (`tools/re/name_functions.py` recovers those); the
rest are the mod's own names for functions it identified by decompiling or by
differential capture. Steal sizes are the bytes that mod's detour engine
overwrites; its engine refuses RIP-relative instructions in a prologue rather
than relocating them, so its steals are a conservative bound for one that does.

### The five profile targets, as the mod uses them

| target | RVA | how the mod uses it |
|---|---|---|
| `GameSim::Step` | `0x15aa00` | Not detoured whole. Two sites inside it are patched: the calls to `CGameTime::GetSpeed` at `0x15aa30` and `0x15aae4` (a fractional speed scales the batch interval) and the paused branch's `call 0xaea970` (GameTime advance) at `0x15aa4a`, so a paused game advances `GameTime+0x30` per simulation step and not per render batch. Both siblings of `GetSpeed` are real: the profile's extended signature is the right call. |
| `CGame::Step` | `0x118e90` | Detoured, 16-byte steal, for pacing (the leader is the clock; joiners pace to it). |
| `CGameTime::GetSpeed` | `0x2877a0` | Read through its call sites rather than hooked; `GameTime::get` at `0x2877c0` returns the counter at `+0x30`. |
| `UI::CMenuUI::StartSavegame` | `0x6785c0` | Detoured (the share observer: a host that loads another world pushes it), and called directly to load a shared save in-process: build a `SaveGameId` `{wstring path; string name; string namespace}`, get its `SavegameInfo` from the save manager (`0x2e6ca0`; the manager is `+200` on the app object from `0xbb23c0`), default-construct `LoadGameParams` (`0x553b70`, 0x138 bytes) and call from `CMenuUI`'s own per-frame update, vftable `0x301dc38` slot 33 (`0x672b10`), on the main thread, where the game starts its own queued loads. Guards on the menu object: `+0x4e8` non-zero while a game runs, `+0x1988` "initialization already active", `+0x19a0` a queued load. |
| `UI::CMenuUI::CreatePage` | `0x663370` | Detoured, 20-byte steal (the Multiplayer panel on the title menu). Two more menu entries go with it: the list-add at `0x22d99e0` (15) and the main-page builder at `0x667bc0` (14). |

### The command pipeline: two hooks, not one

TF3's command surface is now documented, not guessed: Urban Games'
reference lists **61 `api.cmd.make*Cmd` factories** with their argument
types, recorded in
[investigation/TF3_OFFICIAL_API_2026-09-29.md](../investigation/TF3_OFFICIAL_API_2026-09-29.md).
A TF3 profile's factory targets are found for that list, not ported name
for name from TPF2's. Two entries change the design directly:
`makeWorldBuildProposalCmd` takes a fourth `playerInitiated` argument (not
a player-vs-replay signal after all: the game's scripts act on it, so a
replay must carry the original value; see "The scripts, data and log" in
[TPF3_RECON_2026-09-29.md](../investigation/TPF3_RECON_2026-09-29.md)),
and companies are commands
(`makeGameAddPlayerCmd`, `makeEntitySetPlayerCmd`), so ownership changes go
through this same pipeline rather than the native, assert-bypassed
`setPlayer` binding TPF2 patched.

Every player action becomes a `Command` built by a `make_cmd::*` factory and
handed to `CommandList::Add(list, OUT handle, cmd, ..., callback)`. The mod
hooks both, and the reason is worth carrying into a TPF3 profile:

- The **factory** hook sees *what* the command is, while its arguments are still
  the caller's typed structures (a proposal, a `component::Line`, a vehicle
  configuration), which is the only moment they are cheap to decode.
- The **`Add`** hook is the only place a command can be *cancelled*: it zeroes
  the result handle and returns without queueing. The factory cannot cancel;
  its caller still holds the command.
- The mod's own replays go through the same factories (the script's
  `api.cmd.*` path), so the **return address of the factory call** is the only
  thing that tells a player's command from the mod's replay of one. That
  caller-RVA filter is load-bearing, not tidiness: without it every replay is
  captured again. On TF3 it may not hold: the GUI is script, and if the
  stock tools build their commands through `api.cmd` as our replays do,
  both arrive from the same caller. Check this before porting the filter
  ([investigation/TF3_MODS_2026-09-27.md](../investigation/TF3_MODS_2026-09-27.md)).

| factory | RVA | steal | |
|---|---|---|---|
| `BuildProposal` | `0x9dc750` | 19 | roads, track, constructions, terrain, assets, the bulldozer: one command, told apart by the proposal's shape ([BUILDING.md](BUILDING.md)) |
| `CommandList::Add` | `0x9d2a00` | 18 | the cancel point |
| `BuyVehicle` | `0x9dca00` | 15 | its UI waits on the result entity |
| `SellVehicle` | `0x9de380` | 20 | |
| `ReplaceVehicle` | `0x9dddb0` | 15 | its UI waits on the result entity |
| `SendToDepot` | `0x9de6f0` | 20 | |
| `SetLine` | `0x9dea10` | 18 | |
| `CreateLine` | `0x9dcde0` | 19 | its UI asserts on an empty result |
| `UpdateLine` | `0x9df4e0` | 19 | |
| `DeleteLine` | `0x9dd190` | 20 | |
| `Reverse` | `0x9ddfe0` | 20 | a toggle: replaying an uncancelled one applies it twice |
| `SetColor` | `0x9de8a0` | 20 | `r9 -> CVec3f*` |
| `SetName` | `0x9deb70` | 15 | `r9 -> std::string*` (MSVC SSO) |
| `SetGameSpeed` | `0x9de9e0` | 21 | the clock buttons |
| `SetDate`, `SetCalendarSpeed` | `0x9de9b0`, `0x9de870` | 21 | the editor's date controls |

Three rules the cancel point taught, each after a crash or a wedged tool:

1. **A cancelled command's completion callback is a contract.** Commands whose
   UI waits on the result (the build tools, `BuyVehicle`, `ReplaceVehicle`)
   must have their callback fired with a zeroed result at `Add`, or the tool
   hangs for the rest of the session. The callback is a `std::function` whose
   impl the game builds on the stack (`{vftable, captured this}`; `_Do_call`
   is vftable slot 2) or on the heap (impl pointer at `r9+0x38`).
2. **Fire-and-forget commands must not have it fired.** `SetLine` and
   `Reverse` fired with the success byte still 0 make the UI take its failure
   branch ("unable to find a path to a stop"). Suppress without firing.
3. **A callback that asserts on an empty result is moved, not fired.**
   `CreateLine`'s callers (`UI::LineList` `0x610490`, `UI::LineManager`
   `0x6154a0`) assert `resultEntity != ecs::Entity()`. The mod moves the
   callback object into a stash and fires it later, from a later `Add` on the
   same thread, with a stand-in result naming the entity the replay created
   (a 16-byte entry `{int32 entity; double gen; int32}` whose generation must
   match the registry's, `[reg+0xb8]+id*12`).

And one that holds everywhere: **never cancel when the decode failed.** A
command the mod cannot ship in full runs natively and is read back afterwards;
cancelling it would lose the player's action.

### Layouts the capture depends on

Every vector is read at the game's own length (`{begin, end, cap}`), with a
sanity bound on the span and a readability check on every page it touches,
under SEH: a misread pointer fails the decode loudly, and a failed decode is
never cancelled.

- `component::Line`: `vector<Stop>` at `+0x00`, `int waitingTime` `+0x18`,
  `VehicleInfo` `+0x1c` (8 bytes: a `std::bitset<16>` of transport modes plus
  4). `VehicleInfo` is **engine-maintained**: the sim-side `UpdateLine`
  handler (`0x9d9fd0`) restores its own copy, and a command cannot set it.
- `Line::Stop`, 0xa8 bytes: `Entity stationGroup` `+0x00`, `int station`
  `+0x04`, `int terminal` `+0x08`, `vector<StationTerminal{int,int}>
  alternativeTerminals` `+0x10`, `int loadMode` `+0x28` (0..3), two `float`
  waits `+0x2c`/`+0x30`, `vector waypoints` `+0x38`.
- A proposal's edge record, 120 bytes: node ids `+0x00`/`+0x04`, tangents
  `+0x10`/`+0x1c` (3 floats each), `BaseEdge` type and type index
  `+0x28`/`+0x2c` (1 bridge, 2 tunnel), and an optional `PlayerOwned` as
  `{int32 player +0x70; uint8 present +0x74}`.
- `TransportNetwork` and the other components are reached through the engine's
  type index: `GetComponentDataIndex` (`0xd0920`) with the component's
  `RTTI_Type_Descriptor`, then `engine+0x88[typeIndex]`, entries of 0x48
  bytes, data at `+0x68` (indices below `0x40000000`) or paged at `+0x80`.

### The game has two engines

`CGame::RunGameSimLoop` (`0x1184d0`) keeps two `GameState` objects at
`CGame+0x168 -> { GameState*[2], ..., int current at +0x20 }` and copies one
into the other every frame with `GameState::Replicate` (`0x241630`);
`CGame+0x158` is whichever is current this frame, and that is what the UI's
`GameStateProvider` returns (`0x8badf0`: `mov rax,[rcx+8]; mov rax,[rax+0x158]`).
`GameState+0x28` is that state's `ecs::Engine`, and each engine owns its own
system objects. A command carries a specific engine pointer, so anything
computed on its behalf (the mod re-runs the line editor's platform assignment
at the replay) has to take the state whose `+0x28` is that engine, never
"this frame's".

### What lockstep needed beyond command capture

Identical commands at identical steps were not enough; the mod patches four
places where the engine's order depended on memory layout or on a seed:

| | RVA | steal | |
|---|---|---|---|
| train reservation order | `0xabe02d` | 16 | the engine shuffles the order trains claim track with a `minstd_rand` seeded from `GameTime+0x30` over node-list positions, which differ per machine; the detour orders by train name with a seeded jitter |
| free space on a road edge | `0x2117350`, `0x2117140` | 5 | the sum is taken in ascending order in `double`, so every peer gets the same float |
| road edge use entries | `0xa64473` | | kept in name order after `EdgeUseManager::Add` |
| ship and aircraft claim order | `0xa6c1e0`, `0xa2bc60` | 5 | measured only: the family node vector is engine-owned |

The world comparison that finds the remaining divergences hashes geometry and
state, never entity ids: ids, seeds and town growth differ legitimately
between machines that agree on the world.

### UI patches for companies mode

Small in-place patches, each verified against the exact bytes at the site
before it is applied: the line editor's station owner gate (`0x609631`, a
5-byte `cmp eax,[rbx+0x28]; je` with accept `0x609605` and reject `0x609636`),
three owner gates that hide other players' icons, the icon draw call
(`0x80b613`), the station label background (`0x80a0ee`), a foreign entity's
window opening read-only (`jne` at `0x8b3060`), the window bind (`0x8b2390`),
and the HUD station and depot icons (`0x5e38e1`, `0x5e45d0`, depot ctor
`0x5e2b70`). The `setPlayer` binding's ownership assert is bypassed at the
`je` `0x11677a1`. Each is a separate patch with its own byte check, so a build
change disables one feature rather than the mod.

### Detour rules that held up

- **Verify, then steal.** Every target's expected bytes are compared before
  the patch; a mismatch logs and leaves that feature off. Steals stop on an
  instruction boundary at or past 14 bytes and cover only plain,
  position-independent instructions; a `call`, a jump or a RIP-relative
  operand in the prologue is a refusal. Where only five bytes are safe to
  take, a page within ±2 GiB of the site is allocated for the detour and the
  five bytes become a `jmp rel32` into it.
- **Install before the target's first run.** The mod's proxy `alut.dll` loads
  every DLL from `DllMain`, before the game's entry point, so no thread can
  be inside a target when it is patched.
- **The cancel is gated on evidence.** Cancelling is only safe because
  something replays, so it is switched on by fresh evidence from the script
  half on disk (its per-tick status file). With the mod's Lua side absent, the
  hooks capture nothing and cancel nothing, and the base game is unchanged.
