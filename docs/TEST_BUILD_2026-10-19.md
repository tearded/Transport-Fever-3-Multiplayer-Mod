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
| 9 | new track between two signals (#165) | passed: "kept 2 signal(s) … moved 0.00 m" in both |
| 10 | signal the track tool puts back near a node (#168) | passed: "kept 1 signal(s) … moved 0.00 m" in both |
| 11 | Signal Distance signals (#144) | passed: every signal of points 8–10 was one |
| 12 | bridge and tunnel type from the window (#143) | tunnel and a 28-piece track bridge passed; **a very long track bridge refused**: "BuildTrack.polyline.junctions: invalid length 128, expected a list of at most 64 items" |

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
- **A bridge's type change from its window was refused** (point 12, Max2's
  game): the window's rebuild carried 128 junction settings
  (`nodeConfigsToAdd` of the rebuilt track), over the action's
  `MAX_JUNCTIONS` = 64 (`tpf3mp-proto` action.rs), so the hook refused the
  action before it was sent; the bridge was a very long track bridge. A
  shorter track bridge (28 pieces rebuilt in place) changed type in both
  games.
  Nothing diverged. The 2026-10-11 acceptance
  ran on short bridges; a longer track bridge exceeds the bound. Follow-up:
  check whether the window lists each node's settings once, and whether
  unchanged settings can be left out, before raising the bound (a protocol
  change).
