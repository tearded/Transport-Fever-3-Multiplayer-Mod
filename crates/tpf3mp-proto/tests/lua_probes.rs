//! The release-day probe mods for Transport Fever 3 (`tools/probe/tf3`), as
//! the game would load them: laid out as TF3 mods are, and run in the same
//! stand-in for the game's GUI state as our mod (`tests/lua/fake_gui.lua`),
//! over a stand-in world. What the layout rests on is in
//! `investigation/TF3_MODS_2026-09-27.md`; what the probes are for is
//! `docs/DAY_ONE.md` sections 3 and 4.

#![allow(clippy::unwrap_used)]

use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
};

use mlua::Lua;

const FAKE_GUI: &str = include_str!("lua/fake_gui.lua");
const PROBES: [&str; 4] = [
    "tpf3mp_apidump_1",
    "tpf3mp_rundump_1",
    "tpf3mp_detprobe_1",
    "tpf3mp_industryfixture_1",
];

fn tf3_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tools/probe/tf3")
}

fn json(mod_id: &str, path: &str) -> serde_json::Value {
    let text = std::fs::read_to_string(tf3_dir().join(mod_id).join(path)).unwrap();
    serde_json::from_str(&text).unwrap_or_else(|error| panic!("{mod_id}/{path}: {error}"))
}

fn content_files(mod_id: &str) -> BTreeSet<String> {
    fn walk(dir: &Path, root: &Path, out: &mut BTreeSet<String>) {
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                walk(&path, root, out);
            } else {
                let rel = path.strip_prefix(root).unwrap().to_string_lossy();
                out.insert(rel.replace('\\', "/"));
            }
        }
    }
    let root = tf3_dir().join(mod_id).join("content");
    let mut out = BTreeSet::new();
    walk(&root, &root, &mut out);
    out
}

#[test]
fn each_probe_is_laid_out_as_tf3_mods_are() {
    for mod_id in PROBES {
        let manifest = json(mod_id, "mod.json");
        assert_eq!(manifest["modId"], mod_id, "modId is the folder's name");
        assert_eq!(manifest["severityAdd"], "None");
        assert_eq!(manifest["severityRemove"], "None");
        for script in ["preRunScript", "runScript", "postRunScript"] {
            let file = manifest[script]["fileName"].as_str().unwrap();
            if file.is_empty() {
                continue;
            }
            let (path, function) = file
                .strip_prefix(&format!("{mod_id}::/"))
                .unwrap_or_else(|| panic!("{mod_id}: {script} names another mod's file"))
                .split_once('@')
                .unwrap();
            assert!(
                content_files(mod_id).contains(&format!("{path}.lua")),
                "{mod_id}: {script} names {path}.lua, which is not there"
            );
            assert!(!function.is_empty());
        }
        let listed: BTreeSet<String> = json(mod_id, "_content.json")["files"]
            .as_array()
            .unwrap()
            .iter()
            .map(|file| file.as_str().unwrap().to_owned())
            .collect();
        assert_eq!(
            listed,
            content_files(mod_id),
            "{mod_id}: _content.json lists content/"
        );
        let info = json(mod_id, "_metadata/modinfo.json");
        assert!(info["name"].as_str().unwrap().starts_with("TPF3-MP probe"));
        assert!(
            info["tags"]
                .as_array()
                .unwrap()
                .contains(&"Script Mod".into())
        );
        assert!(
            !tf3_dir().join(mod_id).join("mod.lua").exists(),
            "{mod_id}: a TPF2 mod.lua"
        );
        // Each GUI resource names a script and recipe the probe has.
        for resource in content_files(mod_id)
            .iter()
            .filter(|f| f.ends_with(".res.lua"))
        {
            let text =
                std::fs::read_to_string(tf3_dir().join(mod_id).join("content").join(resource))
                    .unwrap();
            let prefix = format!("\"{mod_id}::/");
            let start = text.find(&prefix).unwrap() + prefix.len();
            let target = &text[start..];
            let target = &target[..target.find('"').unwrap()];
            let (script, recipe) = target.split_once('@').unwrap();
            let source = std::fs::read_to_string(
                tf3_dir()
                    .join(mod_id)
                    .join("content")
                    .join(format!("{script}.lua")),
            )
            .unwrap();
            assert!(
                source.contains(&format!("{recipe} = {recipe}")),
                "{mod_id}: no {recipe}"
            );
        }
    }
}

/// The dump between the core marks, each line without `indent`: the mods
/// carry it inside their `data()`, two spaces in.
fn core_block(text: &str, indent: &str) -> String {
    let begin = text.find("-- TPF3MP_DUMP_CORE_BEGIN").unwrap();
    let end = text.find("-- TPF3MP_DUMP_CORE_END").unwrap();
    text[begin..end]
        .lines()
        .map(|line| line.strip_prefix(indent).unwrap_or(line).trim_end())
        .collect::<Vec<_>>()
        .join("\n")
        .trim_end()
        .to_owned()
}

#[test]
fn both_dumps_carry_the_same_core() {
    let core = core_block(
        &std::fs::read_to_string(tf3_dir().join("dump_core.lua")).unwrap(),
        "",
    );
    for (mod_id, script) in [
        ("tpf3mp_apidump_1", "gui/tpf3mp_apidump/apidump.script.lua"),
        ("tpf3mp_rundump_1", "mod.script.lua"),
    ] {
        let text =
            std::fs::read_to_string(tf3_dir().join(mod_id).join("content").join(script)).unwrap();
        assert_eq!(
            core_block(&text, "  "),
            core,
            "{mod_id} carries another dump than dump_core.lua"
        );
    }
}

/// The stand-in GUI state with `mod_id`'s files readable, and the world
/// `world` (Lua that defines `api`) around it. `io` and `os` are taken away
/// unless `files_to` names a folder for the probe to write in.
fn gui(mod_id: &str, world: &str, files_to: Option<&Path>) -> Lua {
    let lua = Lua::new();
    let dir = tf3_dir().join(mod_id).join("content");
    let source = lua
        .create_function(move |_, rel: String| {
            assert!(!rel.contains(".."), "{rel} leaves the mod");
            std::fs::read_to_string(dir.join(&rel))
                .map_err(|error| mlua::Error::external(format!("{rel}: {error}")))
        })
        .unwrap();
    lua.globals().set("mod_source", source).unwrap();
    lua.globals().set("MOD_ID", mod_id).unwrap();
    match files_to {
        Some(folder) => {
            let folder = folder.to_string_lossy().replace('\\', "/");
            lua.load(format!(
                "os.getenv = function(key) if key == 'TPF3MP_PROBE_DIR' then return '{folder}' end return nil end"
            ))
            .exec()
            .unwrap();
        }
        None => lua.load("io = nil; os = nil").exec().unwrap(),
    }
    lua.load(FAKE_GUI).set_name("@fake_gui.lua").exec().unwrap();
    lua.load(world).set_name("@world").exec().unwrap();
    lua
}

fn log(lua: &Lua) -> String {
    lua.load("return logText()").eval().unwrap()
}

const SMALL_API: &str = r#"
api = {
    cmd = { sendCommand = function() end, makeLineCreateCmd = function() end, makeTownCreateCmd = function() end },
    engine = { getComponent = function() return nil end },
    type = { ComponentType = { TOWN = 1, GAME_TIME = 2 } },
}
"#;

#[test]
fn the_api_dump_writes_the_gui_state_into_the_game_log() {
    let lua = gui("tpf3mp_apidump_1", SMALL_API, None);
    lua.load(
        "local m = mount(loadPlugin('gui/tpf3mp_apidump/apidump.script.lua', 'Tpf3mpApiDump')) \
         for _ = 1, 3 do m.render(); m.step() end",
    )
    .exec()
    .unwrap_or_else(|error| panic!("{error}\n{}", log(&lua)));
    let log = log(&lua);
    // Once as the file loads, once in a game, each whole.
    for (tag, name) in [
        ("api-gui-load", "script_api_dump_gui_load.txt"),
        ("api-gui", "script_api_dump_gui.txt"),
    ] {
        assert!(
            log.contains(&format!("[tpf3mp-probe {tag}] BEGIN {name}")),
            "{log}"
        );
        assert!(
            log.contains(&format!("[tpf3mp-probe {tag}] END {name} lines=")),
            "{log}"
        );
        assert_eq!(
            log.matches(&format!("[tpf3mp-probe {tag}] BEGIN")).count(),
            1,
            "once"
        );
    }
    for expected in [
        "_VERSION = Lua 5.1",
        "io=nil os=nil",
        "debugPrint=function ug_require=function",
        "api.cmd.makeLineCreateCmd : fn",
        "api.cmd.makeTownCreateCmd : fn",
        "count=2",
        "::/gui/main/react.lua -> table",
        "## The global table",
        "game.interface is absent",
    ] {
        assert!(log.contains(expected), "no {expected:?} in\n{log}");
    }
}

#[test]
fn the_api_dump_writes_a_file_where_it_can() {
    let folder = tempfile::tempdir().unwrap();
    let lua = gui("tpf3mp_apidump_1", SMALL_API, Some(folder.path()));
    lua.load(
        "local m = mount(loadPlugin('gui/tpf3mp_apidump/apidump.script.lua', 'Tpf3mpApiDump')) m.step()",
    )
    .exec()
    .unwrap();
    let dump = std::fs::read_to_string(folder.path().join("script_api_dump_gui.txt")).unwrap();
    assert!(dump.starts_with("# TPF3-MP script_api_dump (TF3) -- state: gui (in a game)"));
    assert!(dump.contains("api.cmd.makeLineCreateCmd : fn"));
    assert!(!log(&lua).contains("BEGIN"), "nothing into the log");
}

#[test]
fn the_run_script_dump_runs_from_its_run_function() {
    let lua = gui("tpf3mp_rundump_1", SMALL_API, None);
    lua.load(
        "assert(loadstring(mod_source('mod.script.lua'), '@mod.script.lua'))() \
         data().runFn({}, {})",
    )
    .exec()
    .unwrap();
    let log = log(&lua);
    assert!(
        log.contains("[tpf3mp-probe api-run] BEGIN script_api_dump_run.txt"),
        "{log}"
    );
    assert!(log.contains("state: run script"), "{log}");
}

/// A stand-in world whose game time advances 200 a step from step `start`:
/// vehicles move, a road and a town grow with the step, so every lane
/// changes between samples. `advance(n)` runs n steps.
const WORLD: &str = r#"
local CT = { GAME_TIME = 1, TRANSPORT_VEHICLE = 2, BASE_EDGE = 3, BASE_NODE = 4,
             CONSTRUCTION = 5, TOWN = 6, PLAYER = 7, SIM_PERSON = 8, ACCOUNT = 9 }
-- TF3's shape (release build 40408): edges carry position0/position1,
-- money is each player's ACCOUNT, and util.finance has no balances.
-- TPF2_SHAPE switches the stand-in back to TPF2's node positions and
-- finance.getPlayersBalance.
TPF2_SHAPE = false
local world = { step = 1234 }
-- Whether the game time carries the release API's updateCount.
WITH_UPDATE_COUNT = false
function advance(n) world.step = world.step + n end
local function s() return world.step end
game = { interface = { getEntity = function(id)
    if id == 11 or id == 12 then return { position = { id * 10 + s() * 0.25, 5, 1 } } end
    return nil
end } }
api = {
    type = { ComponentType = CT },
    engine = {
        util = {
            getWorld = function() return 0 end,
            finance = setmetatable({}, { __index = function(_, k)
                if k == "getPlayersBalance" and TPF2_SHAPE then
                    return function() return { [21] = 5000000 - s() * 3, [22] = 4000000 } end
                end
                return nil
            end }),
        },
        system = {
            simPersonSystem = { getCount = function() return 900 + math.floor(s() / 50) end },
            townBuildingSystem = { getTown2BuildingMap = function()
                local b = {}
                for i = 1, 3 + math.floor(s() / 300) do b[i] = true end
                return { [31] = b }
            end },
        },
        getEntitiesWithComponent = function(comp)
            if comp == CT.TRANSPORT_VEHICLE then return { 12, 11 } end
            if comp == CT.BASE_EDGE then return { 41 } end
            if comp == CT.PLAYER then return { 22, 21 } end
            if comp == CT.CONSTRUCTION then return { 51 } end
            if comp == CT.TOWN then return { 31 } end
            return {}
        end,
        getComponent = function(id, comp)
            if (id == 11 or id == 12) and comp == CT.TRANSPORT_VEHICLE then
                -- The state lane reads the native vehicle component as well
                -- as its position; both fixtures represent active vehicles.
                return { state = 1 }
            end
            if id == 0 and comp == CT.GAME_TIME then
                return { gameTime = s() * 200, updateCount = WITH_UPDATE_COUNT and s() or nil }
            end
            if id == 41 and comp == CT.BASE_EDGE then
                if TPF2_SHAPE then return { node0 = 42, node1 = 43 } end
                return { node0 = 42, node1 = 43, position0 = { x = 0, y = 0, z = 0 },
                         position1 = { x = s() / 1000, y = 1, z = 0 } }
            end
            if id == 21 and comp == CT.ACCOUNT then return { balance = 5000000 - s() * 3, loan = 0 } end
            if id == 22 and comp == CT.ACCOUNT then return { balance = 4000000, loan = 0 } end
            if id == 42 and comp == CT.BASE_NODE then return { position = { x = 0, y = 0, z = 0 } } end
            if id == 43 and comp == CT.BASE_NODE then return { position = { x = s() / 1000, y = 1, z = 0 } } end
            if id == 51 and comp == CT.CONSTRUCTION then
                local t = {}; for i = 1, 16 do t[i] = 0 end; t[13] = 100; t[14] = 200
                return { fileName = "station/rail/modular.con", transf = t }
            end
            return nil
        end,
    },
}
"#;

/// Runs `frames` frames of a game that advances `steps_per_frame` steps a
/// frame (0.5: one step every other frame), and returns what the
/// determinism probe logged, by step.
fn determinism_run(frames: usize, steps_per_frame: f64) -> (BTreeMap<u64, String>, String) {
    determinism_run_with(frames, steps_per_frame, false)
}

thread_local! {
    /// Runs the stand-in in TPF2's shape: node positions, finance balances.
    static TPF2_WORLD: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// As [`determinism_run`], with the game time's `updateCount` or without.
fn determinism_run_with(
    frames: usize,
    steps_per_frame: f64,
    update_count: bool,
) -> (BTreeMap<u64, String>, String) {
    let lua = gui("tpf3mp_detprobe_1", WORLD, None);
    run_probe(&lua, frames, steps_per_frame, update_count);
    let log = log(&lua);
    (samples(&log), log)
}

/// Mounts the determinism probe in `lua` and runs it for `frames` frames
/// (see [`determinism_run`]).
fn run_probe(lua: &Lua, frames: usize, steps_per_frame: f64, update_count: bool) {
    let tpf2 = TPF2_WORLD.with(|flag| flag.get());
    lua.load(format!(
        "WITH_UPDATE_COUNT = {update_count} TPF2_SHAPE = {tpf2}          local m = mount(loadPlugin('gui/tpf3mp_detprobe/detprobe.script.lua', 'Tpf3mpDetProbe')) \
         local owed = 0 \
         for _ = 1, {frames} do \
             m.step() \
             owed = owed + {steps_per_frame} \
             local whole = math.floor(owed) \
             owed = owed - whole \
             advance(whole) \
         end"
    ))
    .exec()
    .unwrap_or_else(|error| panic!("{error}\n{}", log(lua)));
}

/// The samples in the probe's lines, by step.
fn samples(lines: &str) -> BTreeMap<u64, String> {
    let mut samples = BTreeMap::new();
    for line in lines.lines() {
        let Some(sample) = line.strip_prefix("[tpf3mp-probe det] step=") else {
            continue;
        };
        let (step, rest) = sample.split_once(' ').unwrap();
        samples.insert(step.parse().unwrap(), rest.to_owned());
    }
    samples
}

#[test]
fn in_a_game_with_the_hook_the_samples_go_to_its_log() {
    // Games on one PC share the game's log, and write over each other's
    // lines in it; each hook keeps a log of its own.
    let lua = gui("tpf3mp_detprobe_1", WORLD, None);
    lua.load(
        "HOOK_LOG = {} \
         tpf3mp_native = { log = function(line) HOOK_LOG[#HOOK_LOG + 1] = line end }",
    )
    .exec()
    .unwrap();
    run_probe(&lua, 2000, 1.0, true);
    let hook_log: Vec<String> = lua.load("return HOOK_LOG").eval().unwrap();
    let hooked = samples(&hook_log.join("\n"));
    assert!(hooked.len() >= 15, "{hook_log:?}");
    let game_log = log(&lua);
    assert!(samples(&game_log).is_empty(), "{game_log}");
    // The same samples the game's log gets without the hook.
    let (plain, _) = determinism_run_with(2000, 1.0, true);
    assert_eq!(hooked, plain);
}

#[test]
fn vehicle_probe_reads_native_vectors_and_refuses_missing_positions() {
    let (expected, _) = determinism_run_with(300, 1.0, true);
    let lua = gui("tpf3mp_detprobe_1", WORLD, None);
    lua.load(
        r#"
        api.engine.util.vehicle = { getPosition = function(id)
            local p = game.interface.getEntity(id).position
            local v = newproxy(true)
            getmetatable(v).__index = { x = p[1], y = p[2], z = p[3] }
            return v
        end }
    "#,
    )
    .exec()
    .unwrap();
    run_probe(&lua, 300, 1.0, true);
    assert_eq!(
        samples(&log(&lua)),
        expected,
        "native vectors must hash their coordinates"
    );

    let unreadable = gui("tpf3mp_detprobe_1", WORLD, None);
    unreadable
        .load("game.interface.getEntity = function() return nil end")
        .exec()
        .unwrap();
    run_probe(&unreadable, 300, 1.0, true);
    assert!(
        samples(&log(&unreadable))
            .values()
            .all(|row| row.contains("p=err"))
    );
}

#[test]
fn edge_probe_uses_the_native_node_map_when_generic_enumeration_is_refused() {
    let (expected, _) = determinism_run_with(300, 1.0, true);
    let lua = gui("tpf3mp_detprobe_1", WORLD, None);
    lua.load(
        r#"
        local generic = api.engine.getEntitiesWithComponent
        api.engine.getEntitiesWithComponent = function(comp)
            if comp == api.type.ComponentType.BASE_EDGE then error("Cannot loop over this component type") end
            return generic(comp)
        end
        api.engine.system.streetSystem = { getNode2SegmentMap = function()
            return { [43] = {41}, [42] = {41} }
        end }
        "#,
    )
    .exec()
    .unwrap();
    run_probe(&lua, 300, 1.0, true);
    assert_eq!(samples(&log(&lua)), expected, "hash each edge once");
}

#[test]
fn money_probe_compares_all_companies_when_native_player_enumeration_is_refused() {
    let (expected, _) = determinism_run_with(300, 1.0, true);
    for local_player in [21, 22] {
        let lua = gui("tpf3mp_detprobe_1", WORLD, None);
        lua.load(format!(r#"
            api.type.ComponentType.GAME_SCRIPT = 10
            api.engine.util.getPlayer = function() return {local_player} end
            local generic = api.engine.getEntitiesWithComponent
            api.engine.getEntitiesWithComponent = function(comp)
                if comp == api.type.ComponentType.PLAYER then error('Cannot loop over this component type') end
                return generic(comp)
            end
            api.engine.system.gameScriptSystem = {{ getEntityForGameScript = function() return 77 end }}
            local component = api.engine.getComponent
            api.engine.getComponent = function(id, comp)
                if id == 77 and comp == 10 then
                    return {{ state = {{ companies = {{ list = {{ {{entity=22}}, {{entity=21}} }} }} }} }}
                end
                return component(id, comp)
            end
        "#)).exec().unwrap();
        run_probe(&lua, 300, 1.0, true);
        assert_eq!(
            samples(&log(&lua)),
            expected,
            "the GUI's company must not change the probe"
        );
    }
}

#[test]
fn two_games_at_other_frame_rates_sample_the_same_steps() {
    // One step a frame, and one step every third frame.
    let (fast, fast_log) = determinism_run(2000, 1.0);
    let (slow, slow_log) = determinism_run(6000, 1.0 / 3.0);
    assert!(
        fast_log.contains("stride=100 stepTime=200.000000"),
        "{fast_log}"
    );
    assert!(slow_log.contains("stepTime=200.000000"), "{slow_log}");
    assert!(fast.len() >= 15, "{fast_log}");
    assert!(fast.keys().all(|step| step % 100 == 0), "{fast:?}");
    // The steps both saw carry the same digests, lane for lane.
    let common: Vec<&u64> = fast.keys().filter(|step| slow.contains_key(step)).collect();
    assert!(common.len() >= 10, "{fast:?}\n{slow:?}");
    for step in common {
        assert_eq!(fast[step], slow[step], "step {step}");
    }
    // Every lane was read, and the world's changes show in them.
    let first = fast.values().next().unwrap();
    assert!(!first.contains("=err"), "{first}");
    assert_ne!(fast.values().next(), fast.values().last());
}

#[test]
fn a_game_that_skips_steps_logs_only_the_steps_it_saw() {
    // Three steps a frame: most multiples of 100 are never seen.
    let (samples, log) = determinism_run(3000, 3.0);
    assert!(samples.keys().all(|step| step % 100 == 0));
    assert!(log.contains("# skipped step="), "{log}");
    // The step time is learned from the smallest change seen.
    assert!(
        log.contains("stepTime=600.000000"),
        "three steps a frame looks like one step: {log}"
    );
}

#[test]
fn with_the_update_count_the_probe_samples_it_directly() {
    let (fast, fast_log) = determinism_run_with(2000, 1.0, true);
    // Three steps a frame: only every third step is seen.
    let (skipping, skipping_log) = determinism_run_with(3000, 3.0, true);
    for log in [&fast_log, &skipping_log] {
        assert!(log.contains("stride=100 stepTime=updateCount"), "{log}");
    }
    assert!(fast.len() >= 15, "{fast_log}");
    assert!(fast.keys().all(|step| step % 100 == 0), "{fast:?}");
    // Every third step is seen, so every third multiple of 100 is sampled
    // and the others are logged as skipped; the samples agree.
    assert!(skipping_log.contains("# skipped step="), "{skipping_log}");
    let common: Vec<&u64> = fast
        .keys()
        .filter(|step| skipping.contains_key(step))
        .collect();
    assert!(
        !common.is_empty(),
        "{fast:?}
{skipping:?}"
    );
    for step in common {
        assert_eq!(fast[step], skipping[step], "step {step}");
    }
}

#[test]
fn the_lanes_read_tpf2s_shape_too() {
    TPF2_WORLD.with(|flag| flag.set(true));
    let (samples, log) = determinism_run(1000, 1.0);
    TPF2_WORLD.with(|flag| flag.set(false));
    let first = samples.values().next().unwrap_or_else(|| panic!("{log}"));
    assert!(!first.contains("=err"), "{first}");
}

const INDUSTRY_WORLD: &str = r#"
local CT = { GAME_TIME = 1, TOWN = 2 }
local LU = { RESIDENTIAL = 0, COMMERCIAL = 1, INDUSTRIAL = 2 }
TEST = {
    update = 9,
    ids = { 20, 10 },
    towns = {
        [10] = { cargoNeeds = { { { 200, 1.0 } }, { { 80, 1.0 } }, { { 90, 1.0 } } } },
        [20] = { cargoNeeds = { {}, {}, {} } },
    },
    cargos = { [300] = true, [200] = true, [100] = true },
    cargoById = {
        [100] = { category = "food" },
        [200] = { category = "food" },
        [300] = { category = "fuel" },
    },
    commands = {},
    log = {},
}
os = { getenv = function(key) return FIXTURE_ENV[key] end }
function debugPrint(message) TEST.log[#TEST.log + 1] = message end
tpf3mp_native = { log = function(message) TEST.log[#TEST.log + 1] = message end }
api = {
    type = { ComponentType = CT, ["enum"] = { LandUseType = LU } },
    engine = {
        util = {
            getWorld = function() return 0 end,
            stock = { isCargoTypeCurrentlyProduced = function() return true end },
        },
        forEachEntityWithComponent = function(callback, component)
            assert(component == CT.TOWN)
            for _, entity in ipairs(TEST.ids) do callback(entity) end
        end,
        getComponent = function(entity, component)
            if component == CT.GAME_TIME then return { updateCount = TEST.update } end
            if component == CT.TOWN then return TEST.towns[entity] end
            return nil
        end,
    },
    res = {
        getBaseConfig = function() return { economyId = 1 } end,
        economyRep = {
            find = function(id) assert(id == 1); return 7 end,
            get = function(index)
                assert(index == 7)
                return { cargoCategories = {
                    food = { landUses = { 0 } },
                    fuel = { landUses = { 1 } },
                } }
            end,
        },
        cargoTypeRep = {
            getAll = function() return TEST.cargos end,
            get = function(id) return TEST.cargoById[id] end,
        },
    },
    cmd = {
        makeTownUpdateCargoNeedsCmd = function(town, needs, update_buildings)
            return { kind = "needs", town = town, needs = needs, updateBuildings = update_buildings }
        end,
        makeScriptingSendEventCmd = function(player, channel, event, payload)
            return { kind = "event", player = player, channel = channel, event = event, payload = payload }
        end,
        sendCommand = function(command) TEST.commands[#TEST.commands + 1] = command end,
    },
}
"#;

fn industry_fixture(enabled: bool, target_update: Option<&str>) -> Lua {
    let lua = Lua::new();
    let env = lua.create_table().unwrap();
    if enabled {
        env.set("TPF3MP_INDUSTRY_FIXTURE", "1").unwrap();
    }
    if let Some(target) = target_update {
        env.set("TPF3MP_INDUSTRY_FIXTURE_UPDATE", target).unwrap();
    }
    lua.globals().set("FIXTURE_ENV", env).unwrap();
    lua.load(INDUSTRY_WORLD)
        .set_name("@industry-world")
        .exec()
        .unwrap();
    let source = std::fs::read_to_string(
        tf3_dir()
            .join("tpf3mp_industryfixture_1")
            .join("content/tpf3mp_industryfixture/industry_fixture.script.lua"),
    )
    .unwrap();
    lua.load(&source)
        .set_name("@industry_fixture.script.lua")
        .exec()
        .unwrap();
    lua.load("INDUSTRY_FIXTURE = data()").exec().unwrap();
    lua
}

#[test]
fn industry_fixture_is_opt_in_exact_step_and_appends_deterministically() {
    let disabled = industry_fixture(false, Some("10"));
    disabled
        .load("TEST.update = 10; INDUSTRY_FIXTURE.update()")
        .exec()
        .unwrap();
    assert_eq!(
        disabled
            .load("return #TEST.commands")
            .eval::<usize>()
            .unwrap(),
        0
    );

    let lua = industry_fixture(true, Some("10"));
    lua.load(
        r#"
        INDUSTRY_FIXTURE.update()
        assert(#TEST.commands == 0, "must wait for the requested update")
        TEST.update = 10
        INDUSTRY_FIXTURE.update()
        assert(#TEST.commands == 2, "send the update and its matching event")
        local update, event = TEST.commands[1], TEST.commands[2]
        assert(update.kind == "needs" and update.town == 10)
        assert(update.updateBuildings == true)
        assert(#update.needs == 3)
        assert(#update.needs[1] == 2 and update.needs[1][1] == 200 and update.needs[1][2] == 100)
        assert(#update.needs[2] == 1 and update.needs[2][1] == 80)
        assert(#update.needs[3] == 1 and update.needs[3][1] == 90)
        assert(event.kind == "event" and event.player == "")
        assert(event.channel == "Towns" and event.event == "NewCargoTypeDemand")
        TEST.update = 11
        INDUSTRY_FIXTURE.update()
        assert(#TEST.commands == 2, "fire at most once")
        "#,
    )
    .exec()
    .unwrap();
}

#[test]
fn industry_fixture_fails_closed_on_missing_target_or_unknown_world_shape() {
    let missing_target = industry_fixture(true, None);
    missing_target
        .load("TEST.update = 9; INDUSTRY_FIXTURE.update()")
        .exec()
        .unwrap();
    assert_eq!(
        missing_target
            .load("return #TEST.commands")
            .eval::<usize>()
            .unwrap(),
        0
    );

    let missed_update = industry_fixture(true, Some("10"));
    missed_update
        .load("TEST.update = 11; INDUSTRY_FIXTURE.update()")
        .exec()
        .unwrap();
    assert_eq!(
        missed_update
            .load("return #TEST.commands")
            .eval::<usize>()
            .unwrap(),
        0
    );

    let malformed = industry_fixture(true, Some("10"));
    malformed
        .load(
            r#"
            TEST.towns[10].cargoNeeds = { {}, {}, [4] = {} }
            TEST.update = 10
            INDUSTRY_FIXTURE.update()
            assert(#TEST.commands == 0)
            "#,
        )
        .exec()
        .unwrap();
    let log: String = malformed
        .load("return table.concat(TEST.log, '\\n')")
        .eval()
        .unwrap();
    assert!(log.contains("cargoNeeds"), "{log}");

    let ambiguous = industry_fixture(true, Some("10"));
    ambiguous
        .load(
            r#"
            api.res.economyRep.get = function()
                return { cargoCategories = {
                    food = { landUses = { 0, 1 } },
                    fuel = { landUses = { 1, 2 } },
                } }
            end
            TEST.update = 10
            INDUSTRY_FIXTURE.update()
            assert(#TEST.commands == 0)
            "#,
        )
        .exec()
        .unwrap();
}
