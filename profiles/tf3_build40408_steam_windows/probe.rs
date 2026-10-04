//! Native probe anchors and bytes for Steam Windows build 40408.

/// The GUI's `GameState` getter, `CMenuUI::SwitchToGameUI`'s lambda_2
/// (`mov rax,[rcx+8]; mov rax,[rax+m_game]; mov rax,[rax+state]; ret`).
pub const GUI_TARGET: &str = "probe: GUI GameState getter";

/// The game scripts' `GameState` getter, `CGame::CGame`'s lambda_1
/// (`[[CGame+states] + base + 8*i]`, `i` the word at `+index`, or `1 - i`).
pub const SIM_TARGET: &str = "probe: engine GameState getter";

/// The proposal street graph's owner read,
/// `street_util::ProposalStreetGraph::GetPlayerOwnedPtr` (vf5: `this` and an
/// entity in, the entity's `PlayerOwned` or null out): detoured to count its
/// callers, never to change its answer.
pub const CALLER_TARGET: &str = "probe: ProposalStreetGraph::GetPlayerOwnedPtr";

/// The native tools' "owned by another player" test,
/// `street_util::IsOwnedByOtherPlayer` as this probe names it (rva 0x610ea0,
/// `game\ui\actions\street_builder_util.cpp`): the engine in rcx, the
/// tool's player in edx, an entity in r8d; true when the entity has a
/// `PlayerOwned` whose player is not the tool's. The street builder's snap
/// (`CreateFindSnapPointRoadEarlyAbortContext`, the call at 0x5fc022) marks
/// both ends of an edge it answers true for as fixed, so the tool snaps to
/// the edge's ends only, never into its middle. Detoured to log what it
/// answers true for, always answered by the original.
pub const OTHER_OWNER_TARGET: &str = "probe: street_util IsOwnedByOtherPlayer";

/// The street bulldozer action's test of one edge (rva 0x5f2a00, a lambda
/// of `UI::StreetBulldozerAction::vf2`, `game\ui\actions\bulldozer`): its
/// captures in rcx (the query at `+0x18`, whose owner list is at `+8`), the
/// edge in edx; true when the edge may be bulldozed. Detoured to log each
/// edge it answers and the owner list it had, its answer unchanged.
pub const BULLDOZE_EDGE_TARGET: &str = "probe: StreetBulldozerAction edge test";

/// The bulldozer actions' owner test `sub_5f7db0` (rva 0x5f7db0): the engine
/// in rcx, the owner list in rdx, the entity in r8d, a flag in r9b; true
/// when the list is empty, the entity's owner is in it, or (flag clear) it
/// has no owner. Detoured to log each answer, unchanged.
pub const BULLDOZE_OWNER_TARGET: &str = "probe: bulldozer owner test";

/// The map's line viewer's test of a line's route data (in `sub_7f01d0`,
/// `UI::LineViewer::Update`'s lambda): right after
/// `LineSystem::GetData(line)` (rax the data, rbx the line's entity, r15
/// the line's own state), before `mov ecx,[r15+0x80]; cmp [rax+0x18],ecx`
/// and the segment count check. Spliced to log, per line, the data's
/// revision and segments against what the viewer expects; nothing changes.
pub const VIEWER_TARGET: &str = "probe: LineViewer route data test";

/// The bytes the splice takes there.
pub const VIEWER_BYTES: [u8; 7] = [0x41, 0x8B, 0x8F, 0x80, 0x00, 0x00, 0x00];

/// The map's line viewer's edge geometry of one line (`sub_7f10a0`, in
/// `game\ui\util\lineviewer.cpp`, called by `LineViewer::Update` at 0x7f6598,
/// 0x7f69e9 and 0x7f6a55): rcx the viewer's buffers, rdx the result it
/// fills (vectors at +0x08, +0x28 and +0x50), r8d the line, r9d a stop
/// filter, then three on the stack. Detoured to say, per line, how much it
/// built; the original's work and answer are unchanged.
pub const GEOMETRY_TARGET: &str = "probe: LineViewer GetEdgeGeometries";
