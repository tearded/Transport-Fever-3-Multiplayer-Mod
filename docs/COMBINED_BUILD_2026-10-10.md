# Combined PR build, 2026-10-10

This build combines upstream `dev` at `b87dd521` (version 1.3.0, after
#135 and #136) with the heads of the open pull requests into `dev` listed
below. It is a build for testing; the normal integration and release gates
in AGENTS.md still apply.

| Upstream PR | Change | Included head |
|---|---|---|
| #111 | The rolling world checks' static lanes in small native batches | `8bffc9ae` |
| #138 | Constructions snapped onto existing tracks built in every game | `0230a867` |

#91 (bulldoze previews) and #137 (the owner's v1.3.1 release preparation)
are left out.

Conflicts, resolved:

- #111 with `dev`: the hook's module list keeps both `native_mods` and
  `netread`.
- #111 with `dev`'s `tools/game/quit.ps1`: #111's version is kept, which
  measures the pause menu's buttons on the window as it is now; `-QuitAt`
  and `-DesktopAt` still override them.

#111 is 110 commits behind `dev` and brings its native network layout
(`netread.rs`) for game build 40408 only, while `dev` now selects the 40420
bundle. The combined build adds `profiles/tf3_build40420_steam_windows/netread.rs`:
the four component-pool vtables are taken from the 40420 index (`tpfre`,
RTTI names), BaseEdge `0x3688b08`, BaseNode `0x3688b78`, BaseNodeConfig
`0x3688be8`, Construction `0x3689550`. Sizes and offsets are 40408's: the
pools' functions are the same code shifted by 0x80, and the other 40420
layout files equal 40408's. A pool the reader does not find by its vtable
is refused, not guessed. This port belongs in #111 once it is rebased.

#111's native reader is off by default: a game reads the world in native
parts only with `TPF3MP_HOOK_NATIVE_LANES=on` (or `compare`) in its
environment, the batches set by `TPF3MP_HOOK_PARTS=NxS` (default `10x1`;
`8x2` was the measured choice). Without it the rolling checks run as on
`dev`.

Neither PR raises the protocol, bridge or action schema version, so the
servers and launchers do not tell this build from 1.3.0. Every player in a
room still needs this build, since #138 changes what a snapped construction
sends and applies, and all of a room's games should set the same native
reader settings.

Build packages with `release.yml`, dispatched on
`feat/combined-pr-build-20261010` in
`tearded/Transport-Fever-3-Multiplayer-Mod`: a manual run on a feature
branch leaves the packages as workflow artifacts and drafts no release.

Automated CI and package builds do not replace real-game acceptance.
