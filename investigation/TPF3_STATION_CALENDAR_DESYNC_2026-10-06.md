# Station edits, calendar controls and vehicle drift (2026-10-06)

## Scope and evidence

The owner reported failed rail-station module edits, signals displaced along
bridges, calendar pause being blocked, and repeated automatic resynchronizations.
Tests use Steam Windows build 40408, two hook-injected games on the rig's local
server, and a separate copy of the owner's October 6 save. The original save and
production sessions are not modified. Logs and screenshots remain under
`target/validation-1006` and `target/game-runs/verify-bubu-1006*`; private saves
and player logs are not committed.

## Calendar

The first two-game run accepted CalendarSpeed through the ordinary Weather and
Time window. The host paused the date; the guest showed Calendar Speed: Paused
and August 21, 1909 while vehicles, population and finances continued to advance.
Resuming from the guest advanced the host's date too. This is calendar speed,
not time-of-day or weather synchronization. Protocol tests cover pause, positive
day lengths, range rejection, round trips and replay into two engine stand-ins.
All 55 paired rolling-check signatures in this first run matched. The rig's
separate final probe summary reported no probe lanes, so this statement is
based on the hook logs, not a claimed successful rig probe report.

## Rail-station replacement

Five earlier Basingstoke Station module edits were captured and ordered but
refused by the simple-proposal replay as Construction Not Possible. The station
owns 24 frozen track edges. The native replacement helper reconstructs the full
graph, including snapping, from the requested parameters on each replica.
Recapture checks the file, name, parameters, transform, removals and absence of
external street edits before sending it.

The first real test exposed an incorrect API declaration: `util.d.tl` declares
`makeProposalData(Proposal, Context)`, but the build 40408 runtime throws
`SimpleProposal expected, got Proposal`. A stand-in that accepted both missed
this. The regression now rejects that use. Full replacements instead use
`makeWorldBuildProposalCmd` with `ignoreErrors=false`, as the stock UI does,
and verify the resulting construction parameters. A refused command cannot be
reported as success simply because the old station still exists.

The corrected replay passed the ordinary module editor in two local games
(`verify-bubu-1006c`) using a copy of the affected save. Replacing a platform
with the bulk module succeeded in both games, followed by adding another
track. The station's rebuilt owned track graph increased from 24 to 30 edges.
The guest's station view showed the bulk structure and added track. Liquid,
flatbed and goods replacements have adapter regression coverage, not separate
live clicks in this run.

## Bridge signal placement

When a proposal supplies neither an edge parameter nor a model transform, the
old capture projected the terrain hit vertically onto the track. For elevated
track the cursor ray hits terrain beyond the bridge. The new fallback finds the
closest point of the track's 3D curve to that ray. Explicit proposal positions
still win; replay carries a world position, never the recipient's camera.
A regression reproduces a 50-metre displacement and checks the corrected point
to 1 mm, as well as explicit-parameter precedence.

Two ordinary signal placements on the rail bridge replayed with identical
parameters in both games (0.9628 and 0.4481). For the second placement, a
read-only query projected the new edge object's anchor to native screen
coordinates (1620, 651), at the selected horizontal position (1620); the
cursor was below the rail at y=670. The signal stood on the bridge at
(3050.703125, -8081.172852, 7.746444). The distant first screenshot was
initially misread as an offset; an existing signal had been mistaken for the
new one. No additional patch was made on that mistaken observation.

## Vehicle drift: observations, not a proven cause

- Earlier run: vehicle-2 on line-2 differed by roughly 1.11 metres; vehicle
  earnings, line takings and company balance also differed by $731.
- Later run: vehicle-22 on line-9 differed by about 0.148 metres at two adjacent
  diagnostic samples, with nearly equal speeds. BuyVehicle and AssignLine
  immediately preceded the first mismatch. The paired native tick counts
  matched at checkpoints 39700 and 39750.
- Different raw path hashes do **not** establish different routes: those hashes
  include replica-local entity IDs. The old diagnostic lines also risked
  truncation and accidental email redaction around `@`.

The additional diagnostics preserve long summaries in bounded records, label
local-ID hashes, and include ordered route geometry. Assignment-boundary state
records help establish whether movement differs before or after assignment.
These observations do not yet identify the original cause; automatic resync is
recovery, not proof that the underlying nondeterminism has been fixed.

The local run bought a Prussian Class T 3 with a gondola wagon at Bradley
Stoke Train Depot and assigned it to Line 1 through the vehicle manager.
Both assignment logs agreed: game time 14736000, state 0 before assignment,
state 1 / stop 1 / edge 0 / position 4.158709526062 / speed 0 afterwards.
This exercised the suspected action boundary but did not reproduce the
reported drift, and used Line 1 rather than the original Line 10.

Manually requested vehicle dumps at steps 3800 and 3850 contained 900 and
914 records respectively; `tools/lane_diff.py` found every record equal
between the two games. These included the new route geometry. The hook's
record-ingestion counters measured 0.83 ms and 0.48 ms on the host; those
counters do not include all Lua geometry collection and are not a total
dump-time benchmark. Normal rolling checks do not perform those extra reads.

After the purchase and assignment, dumps at steps 6300 and 6350 also agreed
in all 772 and 734 records. Across the complete run, 128 paired rolling
signatures through step 6400 matched (about 21 minutes at five steps per
second). The four host dumps contained 3320 records; the longest complete
UTF-8 log line was 868 bytes, below the relay's 1024-byte limit. This verifies
real log output size, not a production-relay upload. No production room was
changed. Both test games were quit through their menus.

## Checks

The Lua regressions exercise specialized platform parameters, regenerated
proposal rejection, silent command refusal, calendar replay, bridge cursor
geometry, relay-size/redaction-safe records and unchanged normal lane hashes.
The menu hook's fresh-process test also needed to reset LAST_STEP: without it,
previous tests' simulated world steps made its fresh menu appear to have had a
world loaded. No production menu behavior changed for that test correction.

Validation commands and outcomes:

- `cargo fmt --all` and workspace/all-target Clippy with `-D warnings` passed.
- The Lua adapter suite passed all 207 tests; lane-diff's six tests passed.
- Workspace testing found two integration issues and they were corrected:
  the menu test's stale `LAST_STEP`, and the mod scanner's command allowlist
  missing calendar speed. Hook library tests passed (376, five ignored),
  followed by a successful `cargo test --workspace --exclude tpf3mp-hook`.
  This is combined workspace coverage after fixes, not a claim that the
  original uninterrupted full-workspace invocation passed.
- All three modified PowerShell scripts parse. Double-click input was used
  to assemble the test train. The revised room helper's full automatic
  startup remains unverified: this run recovered through the normal Load
  Game screen after the earlier helper's room-wait condition deadlocked.
