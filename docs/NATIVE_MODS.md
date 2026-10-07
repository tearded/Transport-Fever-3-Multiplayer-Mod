# Native mods: a signed index, installed by the launcher

*Proposed* ([DECISIONS.md](DECISIONS.md), D29): nothing here is decided until
the owner approves it. `crates/tpf3mp-nativemods` builds the parts that work
without any native mod existing yet; what is built and what is not is listed
at the end.

A **native mod** is a mod that is more than Lua: it needs code in the game.
Big Maps ([BIGMAPS.md](BIGMAPS.md)) is the first: hook patches behind
switches, targets in the build's hook profile, a New Game page the hook
serves, settings, and a Lua mod folder. Mod Hub carries Lua mods for the
game's own loader, and a room's Mod Hub mods are handled by D28
([LOBBY.md](LOBBY.md), [MODS.md](MODS.md)); native mods come through a
channel of their own, managed the way CKAN manages Kerbal Space Program
mods, but only from the project's signed index.

## The index

`native-mods.json` lists every package the launcher may install.
`native-mods.json.sig` is an Ed25519 signature of its exact bytes. Both are
assets of the `native-mods` release of the repository
(`releases/download/native-mods/`), fetched over HTTPS only.

```json
{
  "format": 1,
  "serial": 12,
  "packages": [
    {
      "id": "bigmap",
      "version": "0.3.0",
      "name": "Big Maps",
      "description": "Maps up to 128 km a side.",
      "simulation": true,
      "builds": ["<SHA-256 of the game's executable>"],
      "features": ["bigmap.octree", "bigmap.street_raster", "bigmap.placement",
                   "bigmap.page", "bigmap.density", "bigmap.memory_gate"],
      "settings": {"octree_depth": 10, "street_raster": true},
      "depends": [{"id": "other", "version": "^1.2"}],
      "conflicts": ["another_big_map_mod"],
      "files": [
        {"path": "mod/tpf3mp_bigmap_1/mod.lua",
         "url": "https://github.com/…/releases/download/native-bigmap-v0.3.0/mod.lua",
         "size": 1234, "sha256": "<lowercase hex>"}
      ],
      "plugins": []
    }
  ]
}
```

| field | meaning |
|---|---|
| `format` | 1. Another format is refused whole: a launcher that does not know a format does not guess. |
| `serial` | Grows with every index published. The launcher remembers the highest it accepted and refuses an older one, so a withdrawn package cannot come back by replaying an old signed index. |
| `id` | 1 to 40 lowercase letters, digits and `_`. |
| `version` | Semantic version, at most 20 characters (it travels in the room's terms). |
| `simulation` | Whether it changes how the world runs. A feature the hook knows to change the simulation makes the package count as changing it too, whatever this says. |
| `builds` | The game builds it runs on, by the executable's SHA-256, as hook profiles pin them ([HOOKS.md](HOOKS.md)). On any other build it is not installed, and not enabled if it was. |
| `features` | The hook features it enables (below). |
| `settings` | Its settings with their defaults: booleans, integers or text. A player's choice must name one of these and keep its type. |
| `depends` | Packages it needs, with a semver requirement. |
| `conflicts` | Packages that must not be installed beside it. |
| `files` | Data, Lua and settings files: where each goes in the package's folder (plain names joined by `/`), where it comes from, its size and SHA-256. |
| `plugins` | Reserved for signed native libraries (`path`, `url`, `size`, `sha256`, and `abi`, the hook's plugin interface they are built against). This launcher refuses a package that has any. |

Unknown fields are refused, and one malformed entry refuses the whole index:
the project signs it, so a mistake is fixed there, never guessed around.

### Signing

The index is signed the way releases are (D7), with the same code: the
launcher's updater and the index call `tpf3mp_nativemods::signed` (Ed25519
through `ring`, public keys as base64 strings, more than one so that a key
can be changed). The key is the **native-mods key**, a key of its own as the
dev key is (D19): the public half is built into the launcher from
`TPF3MP_NATIVE_MODS_PUBLIC_KEY`, and a launcher built without one installs no
native mod. A release key cannot sign the index, and the native-mods key
cannot sign a release.

To be built with the decision: a `sign-native-mods.yml` workflow that signs
the index in a GitHub environment whose required reviewer approves each
signing, as `sign.yml` does for releases. No key is in the repository; the
tests make a fresh test key each time.

## Installing

The launcher keeps native mods in its own data folder, never the game's:

```text
<launcher data>/native-mods/
  native-mods.json, native-mods.json.sig   the last index accepted
  registry.json                            every installed package and its files
  enabled.json                             what the next game runs
  <id>/<version>/…                         a package's files
  <id>/.partial-<version>/                 a download in progress
```

`store::Store` does every step, refusing rather than guessing:

1. **Resolve** (`resolve`): the wanted package's newest version pinned to the
   running build, then its dependencies (an installed version is kept when it
   satisfies the requirement). Refused: a package the index does not list,
   none for this build, one with plugins, one naming a feature this hook does
   not have, a dependency no version satisfies, a dependency cycle, a
   conflict with a package chosen or installed. There is no backtracking: an
   install that does not hold together is refused with the reason.
2. **Download** each file into `.partial-<version>/`, checking its size and
   SHA-256 on the way (`fetch`, the updater's code). More bytes than signed
   stop the download; a short or changed file fails it.
3. **Move into place** only when every file checks out: the partial folder is
   renamed to `<id>/<version>/`. A failure deletes the partial folder; the
   version in use is untouched.
4. **Register**: the version becomes the one in use, the one before it is
   kept for a rollback, and any older one is deleted. The registry is written
   through a temporary file, so it is whole or not written.

- **Upgrade** is an install of a newer version: the old one stays in use
  until the new one has verified, then stays on disk for a rollback.
- **Rollback** goes back to the version before, after checking its files are
  still whole.
- **Uninstall** deletes exactly the files the registry names, then the
  folders that leaves empty; a file someone else put there stays. A package
  another installed package needs is not uninstalled.
- An install cut short after the rename leaves a folder the registry does
  not name: the next install takes it if every file is the package's, and
  refuses (`Occupied`) otherwise.
- A registry that cannot be read is an error, never replaced by an empty
  one.
- **Installing is not enabling.** The player switches a package on, and sets
  its settings, separately.

## What the game runs

When the launcher starts the game (D11) and the player has enabled a native
mod, it hashes the game's executable, writes the enabled packages for that
build to `enabled.json` with their settings (the defaults with the player's
choices over them) and their folders, and names that file in the game's
environment as `TPF3MP_NATIVE_MODS` (`tpf3mp-agent`'s `native_mods`). A
package enabled but not pinned to that build is left out, with a line in the
launcher's log. Nothing enabled, nothing installed: no variable, and the
executable is not hashed. A registry that cannot be read stops the start.

The hook makes its plan once, at bootstrap, after matching the build's
profile and before installing anything (`tpf3mp-hook`'s `native_mods`,
`enabled::plan`). A package's features run only when:

- the file names the build the hook runs in;
- this hook has every feature the package names
  (`tpf3mp_nativemods::features::BUILT_IN`), and the matching profiles have
  every target those features patch;
- for a package that changes the simulation, the room compares native mods
  (below).

Otherwise a package that changes only what its player sees is left off, with
the reason in `hook.log`; one that changes the simulation **keeps the game
out of rooms**: the hook installs nothing and logs
`multiplayer disabled (fail-closed): native mods that change the simulation
cannot run: …`, as for a build it has no profile for. An `enabled.json` that
cannot be read refuses the same way. No other switch turns a native feature
on: Big Maps' own branches read `TPF3MP_BIGMAP_*` variables by hand; as a
package, it would ask the plan instead (`native_mods::feature(id)`, which
gives the feature's settings and its package's folder).

### Features

In this first version a package carries no code. It names **features
compiled into the hook**, by id; the hook switches on those the launcher
enabled. Each feature has an id (the package's id, a dot, the feature's), an
effect (only what one player sees, or the simulation) and the profile
targets it patches. `features::BUILT_IN` is empty on `dev`: no native feature
is built in yet. Big Maps would add its own; `features::example::BIG_MAPS`
shows the shape, and the tests use it:

| feature | effect | profile targets |
|---|---|---|
| `bigmap.octree` | simulation | `bigmap::octree_root` |
| `bigmap.street_raster` | simulation | `bigmap::street_raster` |
| `bigmap.placement` | simulation | `bigmap::placement_attempts` |
| `bigmap.page` | what one player sees | `bigmap::tile_count`, `bigmap::size_list` |
| `bigmap.density` | simulation | `bigmap::density_levels` |
| `bigmap.memory_gate` | what one player sees | none |

The launcher reads the same registry to refuse a package this TPF3-MP cannot
run before downloading it.

**Later, not built:** signed plugin libraries. A package's `plugins` would be
native libraries, signed in the index like every file and built against a
versioned plugin interface of the hook (`abi`), loaded by the hook only for
the build they are pinned to. That needs its own decision: until then a
package with plugins is refused.

## In a room

A native mod that changes the simulation must run in every game of a room,
in the same version, with the same settings; one that changes only what a
player sees is that player's own, as personal mods are ([MODS.md](MODS.md),
D25).

The room's **native terms** (`terms`) are, for each simulation-changing
package enabled, its id, its version and a SHA-256 of its settings, ordered by
id. The room compares them as it compares content: a member whose terms
differ cannot start or join, and is told which package is missing, which
version or settings differ, and which they run that the room does not. A
member missing a package, or holding another version, gets the room's
version **from the signed index in one click** (`missing_from_index`): the
launcher installs exactly that id and version, for that member's build, or
says it cannot. Packages never pass between players.

Each term already has the shape of a mod in a content manifest:
`native:<id>` (no mod folder can be named so; `:` is not allowed in a
Windows file name) and `<version>+<the settings digest's first ten hex
digits>`, at most 31 characters. **Not built:** carrying the terms on the
wire. The room's declaration requires TPF3-MP's own mod last and the room's
mod information one for one ([PROTOCOL.md](PROTOCOL.md), protocol 18), so
the terms would go as an additive field of the content declaration beside
the manifest, compared by the server like the fingerprint, with a
`PROTOCOL_VERSION` bump. Until then the hook refuses multiplayer for a game
with a simulation-changing package enabled
(`enabled::ROOM_TERMS_CARRIED` is false), so no room can run two different
worlds.

## Built, and not yet

Built (`crates/tpf3mp-nativemods`, with its tests):

- the index format, its signature and validation, the serial floor;
- resolution: builds, dependencies, conflicts, cycles, features, plugins;
- the store: checked downloads, the registry, upgrade, rollback, uninstall,
  enabling and settings, `enabled.json`;
- the hook's plan and its fail-closed refusal (`tpf3mp-hook`), and the
  launcher passing `TPF3MP_NATIVE_MODS` to the game it starts
  (`tpf3mp-agent`);
- the room's terms, their comparison, and finding a missing package in the
  signed index;
- the launcher's updater uses the same signature and download code.

Not built:

- the native-mods key, its signing workflow and the first index;
- the launcher's page for native mods: a **Native mods** group in Settings,
  shown only in a launcher built with the native-mods key, listing each
  package of the index for this build with **Install**, **Uninstall**,
  **Roll back**, an on/off switch and its settings, and installed packages
  the index no longer lists;
- the room's terms on the wire, and the one-click install from the lobby;
- copying a package's Lua mod folder where the game loads mods, the way
  TPF3-MP's own mod is installed ([MODS.md](MODS.md), "TPF3-MP's own mod");
- Big Maps as the first package, with its features built into the hook;
- plugin libraries.
