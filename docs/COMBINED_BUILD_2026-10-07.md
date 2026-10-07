# Combined PR build, 2026-10-07

This build combines upstream `dev` at `82234467` (version 1.2.8, after
#126) with the heads of the open pull requests into `dev` listed below. It
is a build for testing; the normal integration and release gates in
AGENTS.md still apply.

| Upstream PR | Change | Included head |
|---|---|---|
| #108 | Faster saves: zstd level 1 with a 64 KiB buffer | `b38b94f2` |
| #111 | The rolling world checks' static lanes in small native batches | `8bffc9ae` |
| #116 | Every server's rooms listed, new rooms hosted on the closest (proposed D12 amendment) | `2613c7bf` |
| #117 | Native mods from a signed index (proposes D29) | `905a2e13` |
| #121 | Industry spawning traced in local diagnostics (draft) | `f12fee99` |
| #122 | Auto Signals spaces its signals in a room | `20360def` |

#91 (bulldoze previews) is left out on purpose. #116 and #117 propose
decision changes the owner has not made yet; they are here for testing
only.

Conflicts, all where both sides add to the same place, resolved by keeping
both:

- #122 with `dev`'s rename acceptance: `acceptance.lua` keeps
  `rename = true` from `dev` and adds `signals = true` from #122.
- #108 with `dev`'s simulation speed-ups: the static proof's targets, the
  profile's hook targets and BIGMAPS.md keep both additions.
- #117 with #111: the hook's module list keeps both `native_mods` and
  `netread`.
- #116 with `dev`'s renderer recovery test: the test's `LauncherConfig`
  gets #116's new `servers` field, empty (a launcher on its own server).
- #108 with `dev`: both add the same `Rewrite` stub to the non-x86-64
  detour module; one copy is kept. `dev`'s preview profile test lists the
  release-only optional targets; #108's two save targets are added to it.
- #121 fails clippy off Windows on its own branch (its probe's parts go
  unused there). The combined build allows dead code in `industries.rs`
  off Windows so the Linux and macOS tests run.

The package version is 1.2.8, but #122 raises the action schema to 26 and
#116 the bridge version to 26: every player in a room needs this build.
Build packages with `release.yml`, dispatched on
`feat/combined-pr-build-20261007` in
`tearded/Transport-Fever-3-Multiplayer-Mod`: a manual run on a feature
branch leaves the packages as workflow artifacts and drafts no release.

Automated CI and package builds do not replace real-game acceptance.
