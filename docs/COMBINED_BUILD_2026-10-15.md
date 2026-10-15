# Combined PR build, 2026-10-15

This build combines upstream `dev` at `b7b995d7` (version 1.3.1, after
#111) with the heads of the open pull requests into `dev` listed below. It
is a build for testing; the normal integration and release gates in
AGENTS.md still apply.

| Upstream PR | Change | Included head |
|---|---|---|
| #138 | Join a construction snapped onto track ends as its tool did | `b9cc2b4a` |
| #156 | A double slip switch from the junction window builds in a room | `d790458c` |
| #157 | The street and track tools continue from the segment the room built | `a49bd043` |
| #158 | A track laid onto a tram street's end joins it for trams in a room | `a1a2c85a` |

Since the 2026-10-12 build, `dev` took #111, #143, #144 and #145 from that
build, and #155 (safe snapped station junctions) among others.

#138, #156 and #157 merged without conflicts. #158 conflicted only in the
version line at the top of `docs/PROTOCOL.md`: the pull requests branch
from an older `dev` (protocol 20, bridge 28), so the line keeps `dev`'s
protocol 23 and bridge 29 and takes #158's action schema 27.

#158 raises the action schema to 27. Every player in a room needs this
build: a game on schema 26 (1.3.1 or `dev`) cannot read this build's
actions. #138, #156 and #157 also change what the mod captures and
applies.

Build packages with `release.yml`, dispatched on
`feat/combined-pr-build-20261015` in
`tearded/Transport-Fever-3-Multiplayer-Mod`: a manual run on a feature
branch leaves the packages as workflow artifacts and drafts no release.

Automated CI and package builds do not replace real-game acceptance.
