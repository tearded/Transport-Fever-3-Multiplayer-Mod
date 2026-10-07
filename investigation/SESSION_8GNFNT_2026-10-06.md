# Session 8GNFNT: launcher loss and later industry divergence

Evidence collected from the relay and the players' diagnostic sessions in
`target/session-8GNFNT/` on 2026-10-06. Times below are UTC. This is a
postmortem of that run, not a new game test.

## Launcher loss at 17:23

Fin's launcher (`N3QTWS`) logged an `egui_wgpu` validation error, dropped a
frame, then panicked in `renderer.rs:984` while creating an index-data staging
buffer. The requested index data was 47,976 bytes (11,994 indices) and the
reported buffer capacity was 90,000 bytes. The log does not establish why
`wgpu` declined that staging write. The launcher panic explains Fin's lost
connection: the relay stopped waiting for Fin at step 9,171 at 17:23:48 and
closed the connection at 17:23:58. Brad stopped advancing at step 9,232 at
17:24:18 and closed at 17:24:28; Brad's collected diagnostics end without a
comparable launcher panic, so their cause is unproven. Both reconnected after
restarting the 1.2.8 launcher around 17:28. Fin's first post-rejoin verdict
diverged at step 9,200 (lanes 3 and 4) at 17:32:33, and the room rebased it.

## Separate divergence at 18:28

The three restarted games' rolling-world checks had the same signature at
step 30,400 (`0178095351-1144511357`). During steps 30,401–30,450, the game
logged `Attempting to spawn new industry due to new cargo demands` and three
different results:

| Game | Industry reported by the game | Entity |
| --- | --- | ---: |
| Fin | State College Canning Factory | 175463 |
| Brad | Burlington Steel Mill | 135158 |
| Porkster | Lockport Coal Mine | 150714 |

At step 30,450, Fin's rolling-world signature was
`1896732691-1912806008`; Brad and Porkster's was
`1130394931-1526546530`. The relay judged Fin divergent in the network and
construction lanes (0, 1) at 18:28:38. At step 30,550, the three construction
lane summaries all differed. `tools/lane_diff.py` on the collected diagnostic
lines found different industry construction rows, including a Fin-only steel
mill at `(-1384, 3224)` and a Porkster-only iron ore mine at
`(-2200, -5964)`. The diagnostic collector retained only part of the very
large lane dumps, so this diff cannot enumerate every changed construction or
identify the first differing operation.

All three game logs show an earlier spawned industry with entity `179590`
around 18:28:21–22, before the differing outcomes at 18:28:28–34. This
narrows the observed split, but the logs do not contain the native candidate
list, resource demand, placement inputs, or proposal for each attempt.

The room requested a save at step 30,461 after the first divergent verdict.
While that save and rebase were in flight, Porkster was marked divergent in all
lanes at step 30,550 at 18:28:58. This later all-lane verdict does not by
itself identify another initiating cause. The room accepted Porkster's save at
18:29:17 and rebased the replicas. A further save was agreed at step 30,728 at
18:30:41.

The differing automatic industry placements are a concrete divergence in the
game's world. They occurred more than an hour after launcher reconnection, so
the launcher crash is not a sufficient explanation for the 18:28 event. The
logs do not show whether the native industry's random selection, placement,
or an earlier unobserved world difference first caused those placements to
split. A gameplay fix needs a deterministic way to make each game apply the
same automatic industry outcome, or a reproducible earlier cause; changing
the launcher renderer alone will not establish that.

The installed game's SHA-256 is
`de1daad3a13f3b7e9f79903361bb43769cf4f15e59271a263aefe1f075f23ef2`,
matching the existing 40408 `tpfre` index. In that read-only index, the
`Attempting to spawn new industry due to new cargo demands` string points to
native function RVA `0x150310`. It reads game time through `0x2a9680`, folds
that value into an FNV-derived seed, and calls the `SpawnIndustries` closure
at `0x158cb0`, which calls placement helper `0x8ed760` and logs
`Spawned industry`. This identifies the native route but does not establish
which input first differs across the three games.

Further read-only xrefs of build 40408 found a 12,000-unit
`CGameTime::OnIntervalStep` callback that walks Town entities and registers a
per-town watcher. The watcher's only target is the demand-spawn body at
`0x150310`. The game's `TownUpdateCargoNeeds` command handler calls
`ecs::Engine::NoteComponentAboutToBeChanged` before changing the Town
component. The base `town_growth.script.tl` appends a newly demanded cargo
to the existing needs, issues that command, and then emits
`Towns/NewCargoTypeDemand`. This supports a component-change trigger seam;
it does not yet show which capacity, candidate or placement input differed
in the affected room.

A useful next probe would record those inputs at every demand-spawn call in
three games started from the same save, then compare the first differing call.
Only after finding an unstable input can an ordering or selection fix be
tested against matching post-spawn lanes. The collected run cannot establish
that fix by itself.

## Reproduction inputs

The relay agreed to snapshot `w-c85871b8f3ce1d79` at step 27,074,
before the second divergence. A read-only check of the production snapshot
store after the room closed found no retained roots or pending manifests.
It still had 2,163 chunk files, but the missing manifest means the exact
save cannot be assembled from that store. No production snapshot pointer or
player game was changed.

An initial three-game local room from `tpf3mp_validation_1006.sav` stayed
in sync through about step 5,350, including the rolling signatures observed
at 50-step checkpoints. All three games loaded the same host snapshot and
the opt-in native industry probe attached, but no automatic industry-spawn
callback occurred in that window. That save started at game time about
13,854,000, later than the affected room's spawning burst at about 6,087,600;
the synchronized run is an ordinary-start baseline, not a reproduction or
proof of an industry fix. The three locally started games and rig were
closed cleanly afterward.

The older-looking `autosave_New Game_1904-03-17.sav` was ruled out before a
room startup. Its decompressed 40408 save header has 30,675,908 at byte
offset 24; `bubujujuoct6.sav` has 13,559,617 at the same offset, consistent
with the ≈13.854 million native game time observed after loading it. This
field appears to record the saved native game time.
The calendar year in the filename therefore does not identify a save near
the affected 6.088-million-unit burst.
