//! The Lua mod's road and track capture makes exactly the tables the action
//! schema takes: `tests/lua/road_capture.lua` runs the mod's own modules
//! against a world of tables, and the action tables it returns, in metres,
//! must convert (`tpf3mp_proto::lua`, as the hook converts them) to the
//! actions below, in millimetres. Tables changed to break the schema must
//! be refused. Lua 5.1 here (the workspace's vendored Lua); the mod targets
//! 5.2 and keeps to what both run.
//!
//! The modules are compiled into the test binary, so the test does not
//! depend on the working directory.

#![allow(clippy::unwrap_used)]

mod common;

use common::tree;
use mlua::{Lua, Table, Value};
use tpf3mp_proto::{
    BoundedVec, Text,
    action::{
        Action, AssetRef, AssetRemoval, Bulldoze, EdgeEnds, EdgeKind, EdgeRef, Link, Network,
        NodeRef, Polyline, Pos, Resolve, RoadBuild, Structure, Tangent, TrackBuild, Tram, Vertex,
    },
    lua::{LuaValue, action_from_lua},
};

macro_rules! modules {
    ($($name:literal),* $(,)?) => {
        [$(($name, include_str!(concat!("../../../mod/tpf3mp_1/content/scripts/tpf3mp/", $name, ".lua")))),*]
    };
}

/// Every module of the mod, as `require "tpf3mp.<name>"` finds it.
const MODULES: [(&str, &str); 6] = modules!(
    "geom",
    "roads",
    "junctions",
    "engine",
    "acceptance",
    "capture"
);

const TEST: &str = include_str!("lua/road_capture.lua");

struct Captured {
    road: LuaValue,
    track: LuaValue,
    must_refuse: Vec<(String, LuaValue)>,
}

/// Runs the Lua test and returns the tables it produced.
fn run() -> Captured {
    let lua = Lua::new();
    let preload: Table = lua
        .globals()
        .get::<Table>("package")
        .unwrap()
        .get("preload")
        .unwrap();
    for (name, source) in MODULES {
        let chunk = lua
            .load(source)
            .set_name(format!("@tpf3mp/{name}.lua"))
            .into_function()
            .unwrap();
        preload.set(format!("tpf3mp.{name}"), chunk).unwrap();
    }
    // Loading the engine adapter needs no game: its API calls happen only
    // when a capture runs.
    lua.load("require 'tpf3mp.engine'").exec().unwrap();
    let (road, track, refuse): (Value, Value, Table) = lua
        .load(TEST)
        .set_name("@road_capture.lua")
        .eval()
        .unwrap_or_else(|error| panic!("{error}"));
    let mut must_refuse: Vec<(String, LuaValue)> = refuse
        .pairs::<String, Value>()
        .map(|pair| {
            let (why, action) = pair.unwrap();
            (why, tree(&action))
        })
        .collect();
    must_refuse.sort_by(|a, b| a.0.cmp(&b.0));
    Captured {
        road: tree(&road),
        track: tree(&track),
        must_refuse,
    }
}

fn decode(table: LuaValue) -> Action {
    action_from_lua(&table).unwrap_or_else(|error| panic!("{error}"))
}

fn text<const N: usize>(value: &str) -> Text<N> {
    Text::new(value).unwrap()
}

fn list<T, const N: usize>(items: Vec<T>) -> BoundedVec<T, N> {
    BoundedVec::new(items).unwrap()
}

fn pos(x: i32, y: i32, z: i32) -> Pos {
    Pos { x, y, z }
}

fn tangent(x: i32, y: i32, z: i32) -> Tangent {
    Tangent { x, y, z }
}

fn link(from: u16, to: u16, t0: Tangent, t1: Tangent, structure: Structure) -> Link {
    Link {
        precedence: None,
        position0: None,
        position1: None,

        from,
        to,
        tangent0: t0,
        tangent1: t1,
        structure,
        kind: None,
        decorations: BoundedVec::default(),
        locked: false,
        owned: false,
        lanes: BoundedVec::default(),
    }
}

/// A link of another kind than the build's.
fn of(link: Link, network: Network, template: &str) -> Link {
    Link {
        kind: Some(EdgeKind {
            network,
            template: text(template),
            style: None,
        }),
        ..link
    }
}

fn node(x: i32, y: i32, z: i32, network: Network) -> Vertex {
    Vertex {
        pos: pos(x, y, z),
        resolve: Resolve::Node(network),
    }
}

fn new(x: i32, y: i32, z: i32) -> Vertex {
    Vertex {
        pos: pos(x, y, z),
        resolve: Resolve::New,
    }
}

fn street_edge(a: Pos, b: Pos) -> EdgeRef {
    EdgeRef {
        network: Network::Street,
        ends: EdgeEnds { a, b },
    }
}

const COUNTRY: &str = "street/country.street_template";

#[test]
fn a_captured_road_decodes_as_the_schema_says() {
    let road = run().road;
    let flat = |x| tangent(x, 0, 0);
    let expected = Action::BuildRoad(RoadBuild {
        street: text("street/town_medium.street_template"),
        style: Some(text("style/old_town.lua")),
        bus_lane: false,
        tram: Tram::None,
        polyline: Polyline::new(
            list(vec![
                node(100_000, 200_000, 10_000, Network::Street),
                new(160_000, 200_000, 12_000),
                new(220_000, 230_000, 15_000),
                new(452_000, 0, 2_000),
                node(400_000, 0, 2_000, Network::Street),
                node(500_000, 0, 2_000, Network::Street),
            ]),
            list(vec![
                link(
                    0,
                    1,
                    tangent(60_000, 0, 2_000),
                    tangent(60_000, 0, 2_000),
                    Structure::Ground,
                ),
                link(
                    1,
                    2,
                    tangent(55_000, 20_000, 0),
                    tangent(60_000, 28_250, -2_000),
                    Structure::Bridge(text("bridge/cement.lua")),
                ),
                link(
                    2,
                    3,
                    tangent(232_000, -230_000, -13_000),
                    tangent(232_000, -230_000, -13_000),
                    Structure::Ground,
                ),
                of(
                    link(4, 3, flat(52_000), flat(52_000), Structure::Ground),
                    Network::Street,
                    COUNTRY,
                ),
                of(
                    link(3, 5, flat(48_000), flat(48_000), Structure::Ground),
                    Network::Street,
                    COUNTRY,
                ),
            ]),
            list(vec![
                street_edge(pos(400_000, 0, 2_000), pos(450_000, 0, 2_000)),
                street_edge(pos(450_000, 0, 2_000), pos(500_000, 0, 2_000)),
                street_edge(pos(100_000, 300_000, 10_000), pos(100_000, 200_000, 10_000)),
            ]),
        )
        .unwrap()
        .with_removed_nodes(list(vec![NodeRef {
            network: Network::Street,
            at: pos(450_000, 0, 2_000),
        }])),
    });
    assert_eq!(decode(road), expected);
}

#[test]
fn a_captured_track_decodes_as_the_schema_says() {
    let track = run().track;
    let expected = Action::BuildTrack(TrackBuild {
        track: text("track/high_speed.street_template"),
        style: None,
        catenary: true,
        polyline: Polyline::new(
            list(vec![
                node(0, 0, 1_000, Network::Track),
                new(50_000, 1, 1_250),
                node(50_000, -40_000, 1_000, Network::Street),
                node(50_000, 40_000, 1_000, Network::Street),
                new(150_000, -1, 1_000),
                node(200_000, 0, 1_000, Network::Street),
            ]),
            list(vec![
                link(
                    0,
                    1,
                    tangent(50_000, 0, 250),
                    tangent(50_000, 0, 250),
                    Structure::Ground,
                ),
                of(
                    link(
                        2,
                        1,
                        tangent(0, 40_000, 0),
                        tangent(0, 40_000, 0),
                        Structure::Ground,
                    ),
                    Network::Street,
                    COUNTRY,
                ),
                of(
                    link(
                        1,
                        3,
                        tangent(0, 40_000, 0),
                        tangent(0, 40_000, 0),
                        Structure::Ground,
                    ),
                    Network::Street,
                    COUNTRY,
                ),
                link(
                    1,
                    4,
                    tangent(100_000, 0, 0),
                    tangent(100_000, 0, 0),
                    Structure::Tunnel(text("tunnel/concrete.lua")),
                ),
                link(
                    4,
                    5,
                    tangent(50_000, 0, 0),
                    tangent(50_000, 0, 0),
                    Structure::Ground,
                ),
            ]),
            list(vec![street_edge(
                pos(50_000, 40_000, 1_000),
                pos(50_000, -40_000, 1_000),
            )]),
        )
        .unwrap(),
    });
    assert_eq!(decode(track), expected);
}

#[test]
fn a_capture_the_schema_does_not_allow_is_refused() {
    let refused = run().must_refuse;
    assert_eq!(refused.len(), 10);
    for (why, table) in refused {
        assert!(
            action_from_lua(&table).is_err(),
            "{why}: the schema took it"
        );
    }
}

/// Reads a builder proposal with the mod's `engine.fromProposal`, in a
/// stand-in for the game's API, and returns `street`, `track`, `style` and
/// the first edge's network, joined by `|`, or the error.
fn read_proposal(api: &str, segment: &str, network: &str) -> Result<String, String> {
    let lua = Lua::new();
    let preload: Table = lua
        .globals()
        .get::<Table>("package")
        .unwrap()
        .get("preload")
        .unwrap();
    for (name, source) in MODULES {
        let chunk = lua.load(source).into_function().unwrap();
        preload.set(format!("tpf3mp.{name}"), chunk).unwrap();
    }
    lua.load(format!(
        "api = {api}
         local engine = require 'tpf3mp.engine'
         local function v(x, y, z) return {{ x = x, y = y, z = z }} end
         local seg = {segment}
         local proposal = {{ toAdd = {{}}, toRemove = {{}}, proposal = {{
             addedNodes = {{ {{ entity = -1, comp = {{ position = v(0, 0, 0) }} }},
                            {{ entity = -2, comp = {{ position = v(100, 0, 0) }} }} }},
             addedSegments = {{ seg }},
             removedSegments = {{}},
         }} }}
         local c = engine.fromProposal(proposal, '{network}')
         return table.concat({{ tostring(c.street), tostring(c.track), tostring(c.style), c.edges[1].network }}, '|')"
    ))
    .eval::<String>()
    .map_err(|error| error.to_string())
}

/// As much of TF3's api as reading a proposal needs.
const TF3_API: &str =
    "{ type = { enum = { BaseEdgeType = { NORMAL = 0, BRIDGE = 1, TUNNEL = 2 } } } }";

/// A segment of the tool's proposal: `kind` 0 for a street, 1 for a track.
fn tf3_segment(kind: u8, style: &str) -> String {
    format!(
        "{{ entity = -3, type = {kind}, comp = {{ node0 = -1, node1 = -2, \
         tangent0 = v(100, 0, 0), tangent1 = v(100, 0, 0), type = 0, typeIndex = -1, \
         roadType = {kind}, roadTemplate = 'road/town_medium.lua', roadStyle = {style} }} }}"
    )
}

#[test]
fn a_tf3_proposal_is_read_by_road_template_and_style() {
    let street = read_proposal(TF3_API, &tf3_segment(0, "'style/old_town.lua'"), "Street");
    assert_eq!(
        street.unwrap(),
        "road/town_medium.lua|nil|style/old_town.lua|Street"
    );
    let track = read_proposal(TF3_API, &tf3_segment(1, "''"), "Track");
    assert_eq!(
        track.unwrap(),
        "nil|road/town_medium.lua|nil|Track",
        "an empty style is none"
    );
}

#[test]
fn a_tf3_proposal_the_mod_cannot_place_fails_the_capture() {
    // An edge of neither network.
    let unknown = read_proposal(TF3_API, &tf3_segment(5, "nil"), "Track");
    assert!(unknown.unwrap_err().contains("an edge of type 5"));
    // A style that is not a resource name.
    let odd = read_proposal(TF3_API, &tf3_segment(0, "7"), "Street");
    assert!(odd.unwrap_err().contains("roadStyle"));
    // No enum to tell a bridge by.
    let no_enum = read_proposal("{ type = {} }", &tf3_segment(0, "nil"), "Street");
    assert!(no_enum.unwrap_err().contains("api.type.enum.BaseEdgeType"));
}

/// Captures a street tool's proposal with the mod's `engine.captureBuild`, in
/// a stand-in for TF3's api over the network below, and returns the action
/// table, `false` for a proposal of nothing, or why not.
fn capture_street(street: &str) -> Result<LuaValue, String> {
    capture_with(
        street,
        "engine.captureBuild({ toAdd = {}, toRemove = {}, proposal = street }, 'Street')",
    )
}

/// As [`capture_street`], through the road modifiers' capture.
fn capture_modify(street: &str) -> Result<LuaValue, String> {
    capture_with(
        street,
        "engine.captureModify({ toAdd = {}, toRemove = {}, proposal = street })",
    )
}

fn capture_with(street: &str, call: &str) -> Result<LuaValue, String> {
    let lua = Lua::new();
    let preload: Table = lua
        .globals()
        .get::<Table>("package")
        .unwrap()
        .get("preload")
        .unwrap();
    for (name, source) in MODULES {
        let chunk = lua.load(source).into_function().unwrap();
        preload.set(format!("tpf3mp.{name}"), chunk).unwrap();
    }
    let (action, why): (Value, Option<String>) = lua
        .load(format!(
            "local function v(x, y, z) return {{ x = x, y = y, z = z }} end
             -- The country street 8-9-10 runs north through (50, 0); street
             -- 20-21 east-west under (120, 0), where the new road's bridge ends.
             -- The track 30-31 runs north at x = 300.
             local NODES = {{ [7] = v(0, 0, 0), [8] = v(50, -40, 0), [9] = v(50, 1, 0), [10] = v(50, 40, 0),
                              [20] = v(120, -30, 0), [21] = v(120, 30, 0), [30] = v(300, -30, 0),
                              [31] = v(300, 30, 0) }}
             local TRACKS = {{ [30] = {{ 110 }}, [31] = {{ 110 }} }}
             local STREETS = {{ [7] = {{ 104 }}, [8] = {{ 100 }}, [9] = {{ 100, 101 }}, [10] = {{ 101 }},
                                [20] = {{ 102 }}, [21] = {{ 102 }} }}
             api = {{
                 type = {{ enum = {{ BaseEdgeType = {{ NORMAL = 0, BRIDGE = 1, TUNNEL = 2 }} }},
                          ComponentType = {{ BASE_NODE = 1, CONSTRUCTION = 2 }} }},
                 res = {{ bridgeTypeRep = {{ getName = function(i) if i == 3 then return 'bridge/stone.lua' end end }},
                          edgeDecorationRep = {{ getName = function(i)
                              if i == 3 then return '::/infrastructure/edge_addons/barrier_b.edge' end end }} }},
                 engine = {{
                     getComponent = function(id, kind)
                         if kind == 1 and NODES[id] then return {{ position = NODES[id] }} end
                         -- 900 a town's house, 901 a station.
                         if kind == 2 and id == 900 then return {{ townBuildings = {{ 1 }} }} end
                         if kind == 2 and id == 901 then return {{ townBuildings = {{}} }} end
                     end,
                     system = {{ streetSystem = {{
                         getNodeStreetSegments = function(n) return STREETS[n] or {{}} end,
                         getNodeTrackSegments = function(n) return TRACKS[n] or {{}} end,
                     }} }},
                 }},
             }}
             local street = {street}
             local engine, roads = require('tpf3mp.engine'), require('tpf3mp.roads')
             return {call}"
        ))
        .eval()
        .map_err(|error| error.to_string())?;
    match (action, why) {
        (Value::Nil, why) => Err(why.unwrap_or_default()),
        (action, _) => Ok(tree(&action)),
    }
}

/// The street tool's proposal, as build 40408 hands it to game scripts: from
/// node 7 onto the country street 8-9, whose node 9 the new junction -1
/// replaces, rebuilding the street from 8 and 10 through it; then over a
/// bridge.
const SPLIT_PROPOSAL: &str = "{
    addedNodes = { { entity = -1, comp = { position = v(50, 0, 0) } },
                   { entity = -2, comp = { position = v(120, 0, 12) } } },
    addedSegments = {
        { entity = -3, type = 0, comp = { node0 = 7, node1 = -1, tangent0 = v(50, 0, 0), tangent1 = v(50, 0, 0),
          type = 0, typeIndex = -1, roadTemplate = 'street/town.street_template', roadStyle = '', objects = {} } },
        { entity = -4, type = 0, comp = { node0 = 8, node1 = -1, tangent0 = v(0, 40, 0), tangent1 = v(0, 40, 0),
          type = 0, typeIndex = -1, roadTemplate = 'street/country.street_template', roadStyle = '' } },
        { entity = -5, type = 0, comp = { node0 = -1, node1 = 10, tangent0 = v(0, 40, 0), tangent1 = v(0, 40, 0),
          type = 0, typeIndex = -1, roadTemplate = 'street/country.street_template', roadStyle = '' } },
        { entity = -6, type = 0, comp = { node0 = -1, node1 = -2, tangent0 = v(70, 0, 12), tangent1 = v(70, 0, 12),
          type = 1, typeIndex = 3, roadTemplate = 'street/town.street_template', roadStyle = '' } },
    },
    removedSegments = { { entity = 100, type = 0, comp = { node0 = 8, node1 = 9 } },
                        { entity = 101, type = 0, comp = { node0 = 9, node1 = 10 } } },
    removedNodes = { { entity = 9, comp = { position = v(50, 1, 0) } } },
}";

#[test]
fn a_tf3_proposal_travels_as_the_tool_made_it() {
    let action = decode(capture_street(SPLIT_PROPOSAL).unwrap());
    let expected = Action::BuildRoad(RoadBuild {
        street: text("street/town.street_template"),
        style: None,
        bus_lane: false,
        tram: Tram::None,
        polyline: Polyline::new(
            list(vec![
                node(0, 0, 0, Network::Street),
                new(50_000, 0, 0),
                node(50_000, -40_000, 0, Network::Street),
                node(50_000, 40_000, 0, Network::Street),
                new(120_000, 0, 12_000),
            ]),
            list(vec![
                link(
                    0,
                    1,
                    tangent(50_000, 0, 0),
                    tangent(50_000, 0, 0),
                    Structure::Ground,
                ),
                of(
                    link(
                        2,
                        1,
                        tangent(0, 40_000, 0),
                        tangent(0, 40_000, 0),
                        Structure::Ground,
                    ),
                    Network::Street,
                    COUNTRY,
                ),
                of(
                    link(
                        1,
                        3,
                        tangent(0, 40_000, 0),
                        tangent(0, 40_000, 0),
                        Structure::Ground,
                    ),
                    Network::Street,
                    COUNTRY,
                ),
                link(
                    1,
                    4,
                    tangent(70_000, 0, 12_000),
                    tangent(70_000, 0, 12_000),
                    Structure::Bridge(text("bridge/stone.lua")),
                ),
            ]),
            list(vec![
                street_edge(pos(50_000, -40_000, 0), pos(50_000, 1_000, 0)),
                street_edge(pos(50_000, 1_000, 0), pos(50_000, 40_000, 0)),
            ]),
        )
        .unwrap()
        .with_removed_nodes(list(vec![NodeRef {
            network: Network::Street,
            at: pos(50_000, 1_000, 0),
        }])),
    });
    assert_eq!(
        action, expected,
        "the bridge's end above 20-21 is new: only what the tool removes is removed"
    );
}

#[test]
fn a_street_proposal_of_nothing_or_of_more_is_not_carried() {
    // The tool before its first point.
    let nothing = capture_street("{ addedNodes = {}, addedSegments = {}, removedSegments = {} }");
    assert!(
        matches!(nothing, Ok(LuaValue::Boolean(false))),
        "{nothing:?}"
    );
    // A node the world does not have.
    let unknown = SPLIT_PROPOSAL.replace("node0 = 7,", "node0 = 77,");
    assert!(
        capture_street(&unknown)
            .unwrap_err()
            .contains("node 77 has no position")
    );
    // A stop or signal built with the road.
    let objects = SPLIT_PROPOSAL.replace(
        "removedSegments =",
        "edgeObjectsToAdd = { {} }, removedSegments =",
    );
    assert!(
        capture_street(&objects)
            .unwrap_err()
            .contains("a build with a stop or signal")
    );
    // An edge the tool moves with a stop on it: the stop would go onto an
    // edge that is not the one it stood on.
    let moved = SPLIT_PROPOSAL.replace(
        "roadStyle = '', objects = {}",
        "roadStyle = '', objects = { { 555, 1 } }",
    );
    let why = capture_street(&moved).unwrap_err();
    assert!(why.contains("a build that moves a stop or signal"), "{why}");
}

/// A road modifier's proposal as build 40408 hands it to game scripts (the
/// tram track tool, seen in a room): the country street 8-9-10 rebuilt edge
/// by edge between the same places, its middle node 9 removed and added
/// again there as -1, in the tram template; the edge 8-9 keeps its stop
/// 555, the edge 9-10 gets a noise barrier, and is locked and owned (the
/// player-owned tool).
const MODIFY_PROPOSAL: &str = "{
    addedNodes = { { entity = -1, comp = { position = v(50, 1, 0) } } },
    addedSegments = {
        { entity = -2, type = 0, comp = { node0 = 8, node1 = -1, tangent0 = v(0, 41, 0), tangent1 = v(0, 41, 0),
          type = 0, typeIndex = -1, roadTemplate = 'street/country_tram.street_template', roadStyle = '',
          objects = OBJECTS_A } },
        { entity = -3, type = 0, playerOwned = { player = 25 },
          comp = { node0 = -1, node1 = 10, tangent0 = v(0, 39, 0), tangent1 = v(0, 39, 0),
          type = 0, typeIndex = -1, roadTemplate = 'street/country_tram.street_template', roadStyle = '',
          objects = OBJECTS_B, edgeDecorations = { { 3, false } }, roadDevelopmentLocked = true } },
    },
    removedSegments = { { entity = 100, type = 0, comp = { node0 = 8, node1 = 9, objects = { { 555, 0 } } } },
                        { entity = 101, type = 0, comp = { node0 = 9, node1 = 10, objects = {} } } },
    removedNodes = { { entity = 9, comp = { position = v(50, 1, 0) } } },
}";

/// The modifier proposal with the stop 555 on the first new edge (as the
/// tool rebuilds it), on the second, or on neither.
fn modify_proposal(a: &str, b: &str) -> String {
    MODIFY_PROPOSAL
        .replace("OBJECTS_A", a)
        .replace("OBJECTS_B", b)
}

#[test]
fn a_road_modifier_travels_as_the_road_it_rebuilds() {
    let action = decode(capture_modify(&modify_proposal("{ { 555, 0 } }", "{}")).unwrap());
    let Action::BuildRoad(road) = action else {
        panic!("{action:?}")
    };
    assert_eq!(road.street.as_str(), "street/country_tram.street_template");
    let links = &road.polyline.links;
    assert_eq!(links.len(), 2);
    // The edge that keeps its stop, and the one dressed and locked.
    assert!(links[0].decorations.is_empty() && !links[0].locked && !links[0].owned);
    assert_eq!(links[1].decorations.len(), 1);
    assert_eq!(
        links[1].decorations[0].name.as_str(),
        "::/infrastructure/edge_addons/barrier_b.edge"
    );
    assert!(links[1].locked && links[1].owned);
    // Both old edges and the old middle node go.
    assert_eq!(road.polyline.removals.len(), 2);
    assert_eq!(road.polyline.removed_nodes.len(), 1);
}

#[test]
fn a_stop_moves_with_a_road_only_on_the_edge_rebuilt_in_place() {
    // Onto the other edge: refused.
    let why = capture_modify(&modify_proposal("{}", "{ { 555, 0 } }")).unwrap_err();
    assert!(why.contains("a build that moves a stop or signal"), "{why}");
    // Dropped from the rebuilt edge: refused.
    let why = capture_modify(&modify_proposal("{}", "{}")).unwrap_err();
    assert!(
        why.contains("removes an edge with a stop or signal"),
        "{why}"
    );
    // A new stop with the road: refused.
    let why =
        capture_modify(&modify_proposal("{ { 555, 0 }, { -400000000, 0 } }", "{}")).unwrap_err();
    assert!(why.contains("adds a stop or signal"), "{why}");
}

/// A road the tool draws through a town's house: the house goes with the
/// build, which every game's build clears again; a station in the way does
/// not.
#[test]
fn a_road_through_a_town_house_is_carried_and_through_a_station_is_not() {
    let through_house = capture_with(
        SPLIT_PROPOSAL,
        "engine.captureBuild({ toAdd = {}, toRemove = { 900 }, proposal = street }, 'Street')",
    );
    assert!(through_house.is_ok(), "{through_house:?}");
    let why = capture_with(
        SPLIT_PROPOSAL,
        "engine.captureBuild({ toAdd = {}, toRemove = { 901 }, proposal = street }, 'Street')",
    )
    .unwrap_err();
    assert!(why.contains("removes a construction"), "{why}");
}

/// A track modifier's proposal (INFERRED from the street tools' seen
/// shape, not yet seen in the game): the track 30-31 rebuilt in place in
/// the high-speed template, its one lane electrified (ELECTRIC_TRAIN, mode
/// 8, beside TRAIN, 7) and faster.
const TRACK_MODIFY_PROPOSAL: &str = "{
    addedNodes = {},
    addedSegments = {
        { entity = -1, type = 1, comp = { node0 = 30, node1 = 31, tangent0 = v(0, 60, 0), tangent1 = v(0, 60, 0),
          type = 0, typeIndex = -1, roadTemplate = 'track/high_speed.track_template', roadStyle = '',
          objects = {}, laneConfigs = { { speed = 83.333, width = 1.435, height = 0, offset = 0,
          forward = true, transportModes = { [7] = true, [8] = true } } } } },
    },
    removedSegments = { { entity = 110, type = 1, comp = { node0 = 30, node1 = 31, objects = {} } } },
    removedNodes = {},
}";

#[test]
fn a_track_modifier_carries_its_type_catenary_and_speed() {
    let action = decode(capture_modify(TRACK_MODIFY_PROPOSAL).unwrap());
    let Action::BuildTrack(track) = action else {
        panic!("{action:?}")
    };
    assert_eq!(track.track.as_str(), "track/high_speed.track_template");
    let link = &track.polyline.links[0];
    assert_eq!(link.lanes.len(), 1);
    assert_eq!(
        link.lanes[0].modes,
        (1 << 7) | (1 << 8),
        "TRAIN and ELECTRIC_TRAIN"
    );
    assert_eq!(
        link.lanes[0].speed, 83_333,
        "the lane's speed, in thousandths"
    );
    assert_eq!(track.polyline.removals.len(), 1);
    assert_eq!(track.polyline.removals[0].network, Network::Track);
}

/// The track tool's proposal for a track laid onto the end of street 104 at
/// node 7 (0, 0, 0), as build 40420 makes it: the new edge starts 2 m beside
/// the node, on the street's tram lane, and ends at its new node.
const TRACK_ONTO_STREET_END: &str = "{
    addedNodes = { { entity = -1, comp = { position = v(-50, -10, 0) } } },
    addedSegments = {
        { entity = -2, type = 1, comp = { node0 = 7, node1 = -1,
          position0 = v(-0.3146, -1.9752, 0), position1 = v(-50, -10, 0),
          tangent0 = v(-50, -8, 0), tangent1 = v(-50, -8, 0),
          type = 0, typeIndex = -1, roadTemplate = 'track/standard.track_template', roadStyle = '' } },
    },
    removedSegments = {},
}";

#[test]
fn a_track_onto_a_street_end_starts_where_the_tool_put_it() {
    let action = decode(
        capture_with(
            TRACK_ONTO_STREET_END,
            "engine.captureBuild({ toAdd = {}, toRemove = {}, proposal = street }, 'Track')",
        )
        .unwrap(),
    );
    let Action::BuildTrack(track) = action else {
        panic!("{action:?}")
    };
    assert_eq!(
        track.polyline.vertices[0],
        node(0, 0, 0, Network::Street),
        "the street's end node"
    );
    let link = &track.polyline.links[0];
    assert_eq!(
        link.position0,
        Some(pos(-315, -1975, 0)),
        "the start on the tram lane, beside its node, rounded to millimetres"
    );
    assert_eq!(link.position1, None, "the end is its new node");
}

#[test]
fn an_upgrade_is_said_in_one_line_and_another_build_is_not_an_upgrade() {
    let summary = |street: &str, call: &str| match capture_with(street, call).unwrap() {
        LuaValue::String(text) => String::from_utf8(text).unwrap(),
        other => panic!("{other:?}"),
    };
    assert_eq!(
        summary(
            TRACK_MODIFY_PROPOSAL,
            "roads.upgradeSummary(engine.captureModify({ toAdd = {}, toRemove = {}, proposal = street }))"
        ),
        "track upgrade of 1 edge(s) rebuilt in place; template track/high_speed.track_template; 1 lane(s) carrying TRAIN ELECTRIC_TRAIN; lane speeds 83.333 to 83.333; decorations none; locked 0, owned 0"
    );
    assert_eq!(
        summary(
            &modify_proposal("{ { 555, 0 } }", "{}"),
            "roads.upgradeSummary(engine.captureModify({ toAdd = {}, toRemove = {}, proposal = street }))"
        ),
        "street upgrade of 2 edge(s) rebuilt in place; template street/country_tram.street_template; 2 edge(s) with their template's lanes; decorations ::/infrastructure/edge_addons/barrier_b.edge; locked 1, owned 1"
    );
    // A new street onto another's middle is no upgrade.
    let other = capture_with(
        SPLIT_PROPOSAL,
        "roads.upgradeSummary(engine.captureBuild({ toAdd = {}, toRemove = {}, proposal = street }, 'Street'))",
    );
    assert_eq!(other, Err(String::new()));
}

/// The bridge and tunnel window's type change (`bridge_and_tunnel.tl`,
/// `createBridgeOrTunnelProposal`), as the window sends it: the track
/// 30-31 rebuilt in place between the same nodes as a stone bridge (the
/// shape the window sent in a two-player game on build 40420, 2026-10-11).
const BRIDGE_WINDOW_PROPOSAL: &str = "{
    addedNodes = {},
    addedSegments = {
        { entity = -1, type = 1, comp = { node0 = 30, node1 = 31, tangent0 = v(0, 60, 0), tangent1 = v(0, 60, 0),
          type = 1, typeIndex = 3, roadTemplate = 'track/standard.track_template', roadStyle = '',
          objects = {} } },
    },
    removedSegments = { { entity = 110, type = 1, comp = { node0 = 30, node1 = 31, objects = {} } } },
    removedNodes = {},
}";

fn window_build(street: &str, gate: bool) -> Result<LuaValue, String> {
    capture_with(
        street,
        &format!(
            "(function() require('tpf3mp.acceptance').bridges = {gate} \
             return require('tpf3mp.capture').windowBuild(nil, {{ toAdd = {{}}, toRemove = {{}}, proposal = street }}) end)()"
        ),
    )
}

/// A bridge's type changed in its window was refused in a room ("building
/// from this window"): an in-place rebuild of edges travels as the track
/// modifiers' does, once its gate is on; until then it says it waits for
/// acceptance. A window build that adds or removes nodes is no in-place
/// rebuild and stays refused.
#[test]
fn a_bridge_window_rebuild_travels_as_the_track_it_rebuilds() {
    let why = window_build(BRIDGE_WINDOW_PROPOSAL, false).unwrap_err();
    assert!(why.contains("awaits two-player game acceptance"), "{why}");
    // On by default since the two-player game of 2026-10-11.
    let default = capture_with(
        BRIDGE_WINDOW_PROPOSAL,
        "require('tpf3mp.capture').windowBuild(nil, { toAdd = {}, toRemove = {}, proposal = street })",
    );
    assert!(default.is_ok(), "{default:?}");

    let action = decode(window_build(BRIDGE_WINDOW_PROPOSAL, true).unwrap());
    let Action::BuildTrack(track) = action else {
        panic!("{action:?}")
    };
    assert_eq!(track.track.as_str(), "track/standard.track_template");
    assert_eq!(track.polyline.links.len(), 1);
    assert_eq!(
        track.polyline.links[0].structure,
        Structure::Bridge(text("bridge/stone.lua"))
    );
    assert_eq!(track.polyline.removals.len(), 1);
    assert_eq!(track.polyline.removals[0].network, Network::Track);
    assert!(track.polyline.removed_nodes.is_empty());

    // Rebuilt the other way round: still in place.
    let reversed = BRIDGE_WINDOW_PROPOSAL.replace(
        "node0 = 30, node1 = 31, tangent0",
        "node0 = 31, node1 = 30, tangent0",
    );
    assert!(window_build(&reversed, true).is_ok());
    // A road modifier's rebuild through a new node is not the window's.
    let moved = modify_proposal("{ { 555, 0 } }", "{}");
    let why = window_build(&moved, true).unwrap_err();
    assert!(!why.contains("acceptance"), "{why}");
    // A bridge that keeps the signal on it, and one that drops it.
    let signal = BRIDGE_WINDOW_PROPOSAL
        .replace(
            "objects = {} } },\n    },",
            "objects = { { 556, 2 } } } },\n    },",
        )
        .replace(
            "node1 = 31, objects = {}",
            "node1 = 31, objects = { { 556, 2 } }",
        );
    assert!(window_build(&signal, true).is_ok());
    let dropped = BRIDGE_WINDOW_PROPOSAL.replace(
        "node1 = 31, objects = {}",
        "node1 = 31, objects = { { 556, 2 } }",
    );
    let why = window_build(&dropped, true).unwrap_err();
    assert!(
        why.contains("removes an edge with a stop or signal"),
        "{why}"
    );
}

/// Runs the asset bulldozer's capture (`engine.captureAssets`) over three
/// asset groups as build 40408 holds them, and returns the action table,
/// or why it was refused. 50 holds four firs as thin instances; 51 a fir
/// and a boulder, a full instance with its own matrix; 52 one boulder.
/// `TOOL(group, removed)` makes the tool's proposal as
/// UI::AssetBulldozerAction does: the group removed and, unless every asset
/// went, one construction entity at the origin whose models are the assets
/// kept, thin ones then full ones, each its file and world matrix.
fn capture_assets(call: &str) -> Result<LuaValue, String> {
    let lua = Lua::new();
    let preload: Table = lua
        .globals()
        .get::<Table>("package")
        .unwrap()
        .get("preload")
        .unwrap();
    for (name, source) in MODULES {
        let chunk = lua.load(source).into_function().unwrap();
        preload.set(format!("tpf3mp.{name}"), chunk).unwrap();
    }
    let (action, why): (Value, Option<String>) = lua
        .load(format!(
            "local FILES = {{ [41] = 'assets/trees/fir.mdl', [42] = 'assets/rocks/boulder.mdl' }}
             local function fir(x, y, rot)
                 return {{ modelId = 41, pos = {{ x = x, y = y, z = 3 }}, rot = rot, scale = 1.25 }}
             end
             local function boulder(x, y)
                 return {{ modelId = 42, transf = {{ 0, 2, 0, 0, -2, 0, 0, 0, 0, 0, 2, 0, x, y, 2, 1 }} }}
             end
             local THIN = {{ [50] = {{ fir(10, 20, 0), fir(14, 21, 0.5), fir(18, 19, 1), fir(22, 20, 2) }},
                             [51] = {{ fir(600, 50, 0.25) }}, [52] = {{}} }}
             local FULL = {{ [51] = {{ boulder(602, 50) }}, [52] = {{ boulder(700, 60) }} }}
             api = {{
                 type = {{ ComponentType = {{ ASSET_GROUP = 30, MODEL_INSTANCE_LIST = 31 }} }},
                 res = {{ modelRep = {{ getName = function(id) return FILES[id] end }} }},
                 engine = {{ getComponent = function(e, kind)
                     if kind == 30 and THIN[e] then return {{}} end
                     if kind == 31 and THIN[e] then
                         return {{ thinInstances = THIN[e], fatInstances = FULL[e] or {{}} }}
                     end
                 end }},
             }}
             local engine = require('tpf3mp.engine')
             function TOOL(group, removed)
                 local gone, models, n = {{}}, {{}}, 0
                 for _, i in ipairs(removed) do gone[i] = true end
                 for _, t in ipairs(THIN[group]) do
                     n = n + 1
                     if not gone[n] then
                         models[#models + 1] = {{ id = '::/' .. FILES[t.modelId], thin = false,
                             transf = engine.assetMatrix({{ x = t.pos.x, y = t.pos.y, z = t.pos.z,
                                 rot = t.rot, scale = t.scale }}, false) }}
                     end
                 end
                 for _, f in ipairs(FULL[group] or {{}}) do
                     n = n + 1
                     if not gone[n] then
                         models[#models + 1] = {{ id = '::/' .. FILES[f.modelId], thin = false, transf = f.transf }}
                     end
                 end
                 local toAdd = {{}}
                 if #models > 0 then
                     toAdd[1] = {{ fileName = '', playerEntity = -1,
                         transf = {{ 1,0,0,0, 0,1,0,0, 0,0,1,0, 0,0,0,1 }},
                         construction = {{ subconstructions = {{ {{ models = models }} }} }} }}
                 end
                 return {{ toRemove = {{ group }}, toAdd = toAdd,
                     proposal = {{ addedNodes = {{}}, addedSegments = {{}}, removedNodes = {{}},
                         removedSegments = {{}} }} }}
             end
             return {call}"
        ))
        .eval()
        .map_err(|error| error.to_string())?;
    match (action, why) {
        (Value::Nil, why) => Err(why.unwrap_or_default()),
        (action, _) => Ok(tree(&action)),
    }
}

fn asset(model: &str, x: i32, y: i32, z: i32) -> AssetRef {
    AssetRef {
        model: text(model),
        at: pos(x, y, z),
    }
}

/// The three shapes the asset bulldozer makes decode as the schema says,
/// in millimetres: a fir taken out of a group of thin instances, the rest
/// rebuilt; a boulder (a full instance) taken out of a group with a fir;
/// a group of one boulder taken whole, nothing rebuilt.
#[test]
fn an_asset_bulldoze_decodes_as_the_schema_says() {
    const FIR: &str = "::/assets/trees/fir.mdl";
    const BOULDER: &str = "::/assets/rocks/boulder.mdl";
    let removal = |call: &str| match decode(capture_assets(call).unwrap()) {
        Action::Bulldoze(Bulldoze::Assets(removal)) => removal,
        other => panic!("{other:?}"),
    };
    assert_eq!(
        removal("engine.captureAssets(TOOL(50, { 2 }))"),
        AssetRemoval {
            first: asset(FIR, 10_000, 20_000, 3_000),
            count: 4,
            removed: list(vec![asset(FIR, 14_000, 21_000, 3_000)]),
            mirrored: false,
            owned: false,
        }
    );
    assert_eq!(
        removal("engine.captureAssets(TOOL(51, { 2 }))"),
        AssetRemoval {
            first: asset(FIR, 600_000, 50_000, 3_000),
            count: 2,
            removed: list(vec![asset(BOULDER, 602_000, 50_000, 2_000)]),
            mirrored: false,
            owned: false,
        }
    );
    assert_eq!(
        removal("engine.captureAssets(TOOL(52, { 1 }))"),
        AssetRemoval {
            first: asset(BOULDER, 700_000, 60_000, 2_000),
            count: 1,
            removed: list(vec![asset(BOULDER, 700_000, 60_000, 2_000)]),
            mirrored: false,
            owned: false,
        }
    );
    // A proposal that also changes streets is no asset bulldoze.
    let why = capture_assets(
        "engine.captureAssets((function() local p = TOOL(52, { 1 }) \
             p.proposal.removedSegments = { { entity = 100 } } return p end)())",
    )
    .unwrap_err();
    assert_eq!(why, "an asset bulldoze that changes streets too");
}
