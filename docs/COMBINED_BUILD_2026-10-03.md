# Combined PR build, 2026-10-03

This build combines the published PR heads from tearded's fork with
upstream `dev` at `33b4c518` (short commit prefix). It is a build for testing;
the normal integration and release gates in AGENTS.md still apply.

| Upstream PR | Change | Included head |
|---|---|---|
| #76 (already merged) | Other players' 3D build previews | `3e2176854d01ab34e6abab300324ccf197f5811a` |
| #78 | Building while paused | `bcad6103bf09684027eea301ee4916b033fd5951` |
| #80 | Cancelled and critical build previews | `89064f597e33a05eed1aea1120d4ef12411bccb8` |
| #81 | mod.io entries in a save's mod list | `51a5ab53cc27af3be658cc4ab47cee8af756aeaf` |
| #82 | Bounded waits in the rig's tests | `a07b748feb75c7af68fe9478466cbca53d6852c6` |
| #83 | Game restart links | `201d176d1285c1831ec88f7d8050a2a4d8fac155` |

The Windows launch merge retains both the bounded hook-ready wait from
#82 and process-exit detection from #83. Uncommitted local edits are not
part of this snapshot. The older closed PRs #64 and #65 already have their
changes integrated into upstream dev.

The package version is 1.2.0, protocol version 17, bridge version 24.
The binaries carry the combined branch's commit and build number.

Build packages with the existing `release.yml` workflow, dispatched on
`feat/combined-pr-build-20261003` in
`tearded/Transport-Fever-3-Multiplayer-Mod`. A manual run on this feature
branch produces artifacts; it does not draft or publish a release.

Download the platform ZIP or tarball from that run. On Windows, extract
the complete player ZIP and run `TPF3-MP.exe` from the extracted folder.
Keep the agent, hook DLL, package marker and Lua mod together. Install or
update the mod using the included readable installer before playing.
The matching server package is a separate artifact in each platform bundle.

The fork currently has no update public key configured, so these test
packages cannot update themselves or bootstrap an installation from the
standalone Windows executable. Use the complete player ZIP.

Automated CI and package builds do not replace real-game acceptance for
paused building, previews or restarting the game.
