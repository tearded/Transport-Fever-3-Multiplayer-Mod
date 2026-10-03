# Combined PR build, 2026-10-03 (evening)

This build combines the one still-open PR head from tearded's fork with
upstream `dev` at `309175e1` (short commit prefix). It is a build for
testing; the normal integration and release gates in AGENTS.md still
apply.

| Upstream PR | Change | Included head |
|---|---|---|
| #90 | Mod Hub mods in the users' shared mod.io folder | `17eebec4288c07c9f1b6fdd122c92ce3760e0d5b` |

Everything from the earlier combined build (#78, #80, #81, #82, #83) and
#89 (station edit snap and road split) is already merged into upstream
`dev` and so part of this build. Uncommitted local edits are not part of
this snapshot.

The package version is 1.2.0, protocol version 17, bridge version 24.
The binaries carry the combined branch's commit and build number.

Build packages with the existing `release.yml` workflow, dispatched on
`feat/combined-pr-build-20261003b` in
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

Automated CI and package builds do not replace real-game acceptance.
