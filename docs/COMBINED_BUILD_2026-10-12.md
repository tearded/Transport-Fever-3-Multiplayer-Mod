# Combined PR build, 2026-10-12

This build combines upstream `dev` at `8500932a` (version 1.3.1, after
#142) with the heads of the open pull requests into `dev` listed below. It
is a build for testing; the normal integration and release gates in
AGENTS.md still apply.

| Upstream PR | Change | Included head |
|---|---|---|
| #111 | The rolling world checks' static lanes in small native batches | `39fd7376` |
| #138 | Constructions snapped onto existing tracks built in every game | `0230a867` |
| #140 | Install even when Steam lists a library on an offline network share | `9d7ef6f0` |
| #141 | Leave the installed mod whole when a file in it is open | `e692b569` |
| #143 | Players change a bridge's or tunnel's type in a room | `f8599d22` |
| #144 | Signals with Signal Distance's setting placed in a room | `c5239288` |
| #145 | A room's game keeps running when a subsidy's number passes 2^31 | `ecffa17f` |

#91 (bulldoze previews) is left out.

Every pull request merged without conflicts. Compared with the 2026-10-11
build, #143 adds its test record and #145 is new: with it, a subsidy offer
numbered above 2^31 no longer stops the rolling world check, which had held
the world and refused every action in the 2026-10-12 playtest.

#111's native reader is off by default: a game reads the world in native
parts only with `TPF3MP_HOOK_NATIVE_LANES=on` (or `compare`) in its
environment, the batches set by `TPF3MP_HOOK_PARTS=NxS` (default `10x1`).
Without it the rolling checks run as on `dev`.

No pull request raises the protocol, bridge or action schema version, so
the servers and launchers do not tell this build from 1.3.1. Every player
in a room still needs this build: #138, #143 and #144 change what the mod
captures and applies, #145 changes what the rolling check reads, and all of a room's games should set the same
native reader settings.

Build packages with `release.yml`, dispatched on
`feat/combined-pr-build-20261012` in
`tearded/Transport-Fever-3-Multiplayer-Mod`: a manual run on a feature
branch leaves the packages as workflow artifacts and drafts no release.

Automated CI and package builds do not replace real-game acceptance.
