# Selected ports from PR #37

This page records the integration into `dev`, not all work on Silver's
branch. It is not evidence of a fresh two-player game acceptance run.

## Enabled ports

| Area | Integrated behavior | Evidence |
|---|---|---|
| Identity and builds | Steam default name, binary build identification, incompatible launcher-instance protection | Rust unit tests |
| Content | Fingerprint the installed multiplayer mod; exclude generated portraits so cosmetic extraction does not split rooms | Content/fingerprint tests |
| Lobby | Loading stages, banners/portraits, copy invite, change starting save before play | Lua window and server tests |
| Companies | Read the game's balance, order loans, check HQ ownership and permits | Lua capture/replay fixtures |
| Vehicles | Buy onto a line, including bursts; use isolated harbour/airport depots and the selected second depot; buy planes at an airfield's or airport's hangar (its hangar module's subconstruction), refusing an airfield without one and an ambiguous depot with the reason; in multi-company replay, buy only at the acting company's owned depot (foreign and ownerless depots refused); preserve one-company native purchases | Lua capture/replay fixtures |
| Roads | Refuse street demolition if its affected town buildings changed; preserve junction settings and street precedence | Lua capture/replay fixtures |
| Command guard | Install the guard in the HUD's separate Lua state | Lua guard fixtures |

Our installer, updater, executable icon, desktop shortcut and separate server
package remain. Our Join a friend form, stock new-world setup, leave/rejoin
cleanup and native loading implementation remain. Depot connection replay
retains our queue-based pruning of internal construction branches.

## Follow-up ports from the updated PR

The follow-up includes HUD command-result routing (147d590), default vehicle
compartment loads (4f30083), and the native load-state fix (24cb7c8, adapted
with 1878b573's incoming-world handling). Callback forwarding adds bounded
admission, acknowledged batches and retry without discarding accepted actions.
Our close-before-load frame and all existing acceptance gates remain.

The new native field is checked against the installed executable, read-only.
Lua and hook fixtures exercise callback overflow, default/partial loads,
load gating and save setup. This is not a fresh two-player game playthrough.

## Implemented, refused pending game acceptance

Line waypoints passed local two-player acceptance on build 40420 on
2026-10-16 (a line edited with waypoints in a room, both games' rolling
world checks alike) and are enabled. Entity renaming and vehicle
recolouring
([the validation record](../investigation/TPF3_RENAME_2026-10-07.md)) and
subsidies, with their settlement between companies
([the validation record](../investigation/TPF3_SUBSIDIES_2026-10-07.md)),
passed two-player acceptance on 2026-10-07 and are enabled. Bridge/tunnel window rebuilds, gated at
capture behind `bridges`, passed it on 2026-10-11 and are enabled
([the validation record](../investigation/TPF3_BRIDGE_TUNNEL_WINDOW_2026-10-11.md)). Industry Greenification and marketing campaigns
remain gated at both submission and replay behind `perks`. Historic Preservation is likewise gated behind `preservation`. Mechanics fixtures explicitly enable a channel
only in their own Lua state. Enable a channel only after ordinary two-player
acceptance demonstrates matching outcomes, ownership and money. The gate
file is part of the installed-mod fingerprint.

## Deliberately excluded

- Automatic creation of competitive companies.
- Alternate simulation-buffer and world-loading experiments.
- The expanded scenario runner and notification/discard additions.

Station access per company (`CompanyOp::StationAccess`, action schema 23)
came in afterwards on its own. The owner approved this station-access
extension to D22 on 2026-10-02. Two local games demonstrated selection,
policy changes, ownership enforcement, pathing, passenger carriage and fares;
see [the validation record](../investigation/STATION_TERRAIN_2026-10-02.md).

Terraforming and the track upgrade tools came in afterwards on their own:
the hook reads a terrain tool's stroke at its existing `CommandList::Add`
detour (one more optional profile target, no new detour) and fills the
room's carrier at its existing apply detour. `Terraform` is enabled after
the 2026-10-02 two-game height-brush validation: raise, lower, smooth,
flatten and heightmap produced matching native ground. The validation
record distinguishes complete comparison windows from runs with missing
probe samples; oversized replay bands still have fixture-only coverage.
Terrain paint and asset brushes remain refused. The track tools travel as
`BuildTrack`, as the road modifiers do, and each upgrade is logged.

The existing junction gate remains off pending its own game acceptance.
The station-access decision is recorded in D22; other proposed policies remain unchanged.

## Company tools and stop follow-up (2026-10-03)

Ported the newer `local/combined-dev` changes through `34a2edf`: native GUI
and tool company selection, refreshed Lua APIs, room-company map markers,
stop/station ownership and names, and station entrance junction filtering.
The current branch-pruning algorithm and all existing acceptance gates stay.
The line-viewer probe remains diagnostic; it does not repair invalid route data.
Trees and rocks can be removed through the room only with
`TPF3MP_TREE_BULLDOZE=1`; they are still disabled by default.
Lua fixtures and native helper/static-profile checks cover this integration;
no fresh ordinary two-player game acceptance is claimed.

## Stock airports and airfields (2026-10-04)

The stock airfield and airport generate runway and taxiway signals inside
their construction proposals. Capture now recognizes those signals only when
their native object batch has unique type-2 carriers on isolated new airport
edges. A replacement or demolition may remove the old airport's signals only
when each has one carrier on the old construction's frozen edges; demolition
also requires the complete frozen-edge set. External stops, signals and
mixed proposals remain refused. The action format and version are unchanged:
both games regenerate the construction's own network during replay.

The ordinary two-game run in
[TF3_AIRPORT_SUPPORT_2026-10-04.md](../investigation/TF3_AIRPORT_SUPPORT_2026-10-04.md)
used a stock loan to build two airfields and a large airport, buy and assign
aircraft from both kinds of hangar, and fly a two-airfield and a mixed airport
line. A separate ordinary run added an airfield terminal, removed its hangar
with the module editor, connected two street segments toward it, removed the
whole edited airfield with the main bulldozer, and rebuilt it. Both replicas
had the same balance after demolition, and 70 overlapping determinism probes
had already matched by then. At room end all seven probe lanes matched in
138 comparable samples (two startup read errors). The rebuilt station's
native catchment included external town-road edges, establishing network
connection; no passenger ridership was observed at this distant site. In
the earlier flying run, the aircraft-position probe had 59 read errors, so
exact position agreement during those samples is unproven despite matching
construction, network, money and vehicle-count lanes and no reported room
divergence. A follow-up saved-game run corrected the probe's airport-hangar
lookup and aircraft position/state lane, then exercised an active and a parked
DC-3 while building and editing a stock large airport. The paired logs had
111 matching common samples across steps 12000–23400 in all seven lanes,
including real active-aircraft positions and explicit in-depot states, with
zero read errors. Four asynchronously skipped sample steps are reported
separately. The large-airport room added a passenger terminal, taxiway and
second runway and removed the added terminal. The owner then took over the
two running games for the remaining UI checks, reported "all works", and
accepted the manual result on 2026-10-04. That confirmation is user-reported
acceptance, not a separate automated demolition trace. Neither run establishes
profitability or demand for a passenger service.

## Compatibility

This selected combination is distinct from both the previous `dev` and PR #37:
protocol **16**, bridge **23**, action schema **25**. Update launcher, hook,
mod and relay together before release. Older peers must fail version checks;
this branch is not compatible with the currently deployed relay until upgraded.
