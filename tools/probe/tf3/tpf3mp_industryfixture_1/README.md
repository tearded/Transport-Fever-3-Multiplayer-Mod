# Controlled industry-demand fixture

This standalone probe is deliberately outside `mod/tpf3mp_1` and is not part
of a release package. Installing this test-only mod is not sufficient to mutate
a world: its game script also requires `TPF3MP_INDUSTRY_FIXTURE=1` and a
positive `TPF3MP_INDUSTRY_FIXTURE_UPDATE` value. Set the same values in the
environment inherited by every local game process.

At exactly that `GAME_TIME.updateCount`, the fixture reads the world and picks
the numerically lowest Town entity ID. It copies the Town's existing three
`cargoNeeds` buckets into the ID-only shape used by the game's own
`town_growth.script.tl`, then picks the lowest available CargoTypeId that is
missing from the Town and has one unambiguous land-use bucket. It sends the
official `makeTownUpdateCargoNeedsCmd(town, needs, true)` and the same
`Towns/NewCargoTypeDemand` event sent by the base game's town-growth script.
Unknown shapes, unavailable APIs, ambiguous cargo categories, and a missed
update all fail closed before a command is sent. The command can update town
buildings and cause an industry spawn. TPF3-MP refuses a player's or personal
GUI mod's town command, but shared game-script mods run in each replica's
simulation state. A staged room test therefore requires this probe to be
enabled in the save and present in every local game, with the same environment
values; it must never be installed as a personal mod. Do not interpret its log
as an accepted mutation unless the normal room logs and post-spawn state
confirm acceptance. Never run it from the room console.

The offline Lua test establishes the fixture's command shape, exact-update
gate, deterministic selectors, append-only behavior, and fail-closed cases.
No real-game command acceptance has been demonstrated. The fixture does not
reproduce the ordinary-play trigger, establish the original divergence's first
differing input, or replace ordinary-start acceptance. Remove or disable this
separate probe after any controlled test.
