# Test build 2026-10-19

A local test build for a launcher run, not for merging.

- Base: upstream `dev` at `02829e36` (Merge pull request #171), version 1.3.1.
- Only change: `mod/tpf3mp_1/content/scripts/tpf3mp/acceptance.lua` sets
  `crossings = true` and `crossingTypes = true`, so a room carries
  - a level crossing's type picked in its window (`SetCrossingType`), in the
    window's map-only shape only: dev refuses the rebuild shape (701fda20);
  - the crossing type picked in the road and track toolbar
    (`BuildRoad`/`BuildTrack` `crossing_type`, schema 28).
- The gates change the mod fingerprint: every room member needs this build.

## Launcher run 2026-10-19

Two games on one PC, each from its own launcher, against a local server
(`target/test-package/start-test.ps1`), TF3 build 40420 (public branch),
Japan_2 with mods (Japanese crossings and signals). Every point checked in
both hook logs; the rolling world checks stayed equal throughout.

| # | change | result |
|---|---|---|
| 1 | toolbar crossing type (#169), road over track and track over road | passed: both games laid `jp_double_rht` and `jp_gantry_single_lht` |
| 2 | crossing window type (#166) | refused as dev intends: every click was the rebuild shape ("a crossing rebuild with segments or junctions is not carried", 701fda20); nothing diverged |
| 4 | track onto a tram street's end (#158) | passed |
| 5 | double slip switch (#156) | partly: works at an older crossing that has junction settings (EditJunctions applied in both); **refused at a newly built crossing**: "junction tools await the two-player acceptance test (strict_junctions): a junction with no settings yet (lanes ->0)" (`junctions.lua` narrowWhyNot, `old == nil`) |
| 6 | stacked tracks: bulldoze the upper, the lower, split the lower (#161) | passed |
| 7 | bulldoze a track stub on a street and a street branch (#162) | passed |
| 8 | bulldoze a track with signals, a two-sided stop (#164) | passed |

### Findings

- **New crossings cannot get a double slip switch in a room** (point 5).
  A crossing the room just built has no `BaseNodeConfig`, and the narrow
  gate refuses an edit from no settings. Only crossings that already carry
  settings switch. Follow-up for #156: accept the edit from no settings
  when the new configuration is the game's default plus the slip switch.
- **A street with stops on it cannot be bulldozed in one go** (seen in
  point 8). Hovering the street with the bulldozer shows "Not in
  multiplayer yet: removing a stop or signal" and "2 Stops Will Be
  Removed"; single player removes street and stops together. The mod
  carries a stop removal alone and a track removal with its signals
  (`engine.lua` `engine.bulldoze`, the `edgeObjectsToRemove` check), but
  not edges removed with stops on them. Workaround: bulldoze the stops
  first, then the street. Not a regression: dev refused it before too.
  Follow-up: carry it like a track with signals, as `Bulldoze::Edges` whose
  stops every game resolves and removes with their street (the two-sided
  pair check of `Bulldoze::EdgeObjects` included).
