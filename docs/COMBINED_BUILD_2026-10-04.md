# Combined PR build, 2026-10-04

This build combines every open PR head from tearded's fork with upstream
`dev` at `911b03f6` (short commit prefix). It is a build for testing; the
normal integration and release gates in AGENTS.md still apply.

| Upstream PR | Change | Included head |
|---|---|---|
| #92 | A bus stop between traffic lights no longer crashes every game | `18892bf6720f5dfd6f653019f1f5da0c87e73493` |
| #93 | The game's built-in mods in its `mods\release` folder | `7ee5c8d6faac431f58af54cdcc5b87b0c2125d10` |
| #94 | Game-update analysis, native verification and builds | `0bd267c63a66b7c5946c139df6431ada3d571118` |
| #95 | The host's fixture loads once its main menu is really up | `ff244c991f20e848766c4e6374919a4a358c8dd5` |
| #96 (draft) | The room's owner picks its save and mods on the game's pages | `07e313bc8b08bdc8b5b662b5fe788035d472568f` |

#90 (Mod Hub mods in the shared mod.io folder), part of the previous
combined build, is merged into upstream `dev` and so part of this build.

#93 and #96 both add `<game>\mods\release` to the mod roots. The merge keeps
the one code change and #93's wording in `roots.rs` and `docs/MODS.md`.

The package version is 1.2.0, protocol version 18, bridge version 25 (both
raised by #96). A launcher, agent or server from an earlier build cannot
join rooms of this one. Uncommitted local edits are not part of this
snapshot.

Build packages with the existing `release.yml` workflow, dispatched on
`feat/combined-pr-build-20261004` in
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
