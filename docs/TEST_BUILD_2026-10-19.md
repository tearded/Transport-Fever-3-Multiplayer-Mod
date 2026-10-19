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
