# Combined PR build, 2026-10-06

This build combines upstream `dev` at `9daed36e` (release 1.2.8) with the
heads of the open pull requests into `dev` listed below. It is a build for
testing; the normal integration and release gates in AGENTS.md still apply.

| Upstream PR | Change | Included head |
|---|---|---|
| #105 | Station edits, calendar synchronization and bridge signal placement | `a7fe4eca` |
| #106 | The hook's memory checks share readable regions between threads | `3e3d6d5d` |
| #107 | A solo game keeps running when the launcher's link drops | `e1d87c7e` |
| #108 | Faster saves: zstd level 1 with a 64 KiB buffer | `b38b94f2` |
| #109 | Bit-identical simulation speed-ups and `perf: sim` timers | `508e7a7e` |
| #110 | The game's memory read without asking the system first | `09818209` |
| #111 | The rolling world checks' static lanes in small native batches | `8bffc9ae` |
| #112 | Set up world shown after picking a new world in the room | `ca98480f` |

#91 (bulldoze previews) is left out on purpose.

#109 conflicted with #108 and #110 only where both add to the same lists:
the static proof's targets, the profile's hook targets, BIGMAPS.md and the
switch table in HOOKS.md. The merge keeps both sides; for
`TPF3MP_HOOK_PERF` it keeps #109's longer row.

The package version is 1.2.8; no PR here changes the protocol, bridge or
log format versions, so its games meet 1.2.8 launchers and servers. Build
packages with `release.yml`, dispatched on `feat/combined-pr-build-20261006`
in `tearded/Transport-Fever-3-Multiplayer-Mod`: a manual run on a feature
branch leaves the packages as workflow artifacts and drafts no release.

Automated CI and package builds do not replace real-game acceptance.
