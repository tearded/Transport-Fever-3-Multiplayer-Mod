//! Which Transport Fever 3 mods the players of a room may differ in.
//!
//! A mod is **personal** when it can only change what one player sees: its
//! windows, overlays and styles. Everything such a mod does to the world goes
//! through `api.cmd`, which the mod's command guard carries through the room
//! or refuses (`mod/tpf3mp_1/content/scripts/tpf3mp/guard.lua`). Any other
//! mod is **shared**: every player of the room must run it, in the same
//! version (the room's content check). The rules, and why, are in
//! `docs/MODS.md`.
//!
//! [`scan`] reads a mod's folder and gives its [`Class`] with the
//! [`Reason`]s for it. It fails closed: a mod is personal only when every
//! file in it is accounted for as one that cannot touch the simulation, and
//! anything it cannot read or does not know makes the mod shared. The scan
//! is advisory, since Lua can reach anything by a name it builds at run
//! time; what the scan cannot see, the guard refuses in the room's game.

pub mod lexer;
pub mod roots;
pub mod save;

use std::{
    collections::BTreeSet,
    fmt, fs,
    path::{Path, PathBuf},
};

use serde::Serialize;

use lexer::{Located, Token};

/// Largest file read; a bigger script or resource makes the mod shared.
pub const MAX_FILE_BYTES: u64 = 16 << 20;
/// Most files looked at in one mod; more makes the mod shared.
pub const MAX_FILES: usize = 20_000;
/// Deepest folder looked into.
const MAX_DEPTH: usize = 24;
/// Reasons of one kind and detail printed one by one; more print as one.
const GROUPED: usize = 3;
/// Version control folders at a mod's top, skipped.
const VCS_DIRS: &[&str] = &[".git", ".hg", ".svn"];

/// Whether the players of a room may differ in a mod.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Class {
    /// Only this player's view: may be active in one player's game and not
    /// in another's.
    Personal,
    /// Decides in a game script, but acts only through commands the room
    /// carries from a personal mod's game script (`tpf3mp/modguard.lua`):
    /// may be personal once the room lets game-script mods be (docs/MODS.md;
    /// until then it is treated as shared unless the player asks).
    Carried,
    /// Must be active, in the same version, in every game of the room.
    Shared,
}

impl fmt::Display for Class {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Personal => "personal",
            Self::Carried => "carried (personal once game-script mods may be)",
            Self::Shared => "shared",
        })
    }
}

/// The commands the room carries from a personal mod: those the GUI's guard
/// makes actions of (`guard.lua`'s CARRY and PASS, less builds, which it
/// carries from construction windows only) and those the personal mods'
/// guard hands on or drops (`modguard.lua`'s CARRY and DROP). A game-script
/// mod that makes anything else is shared.
pub const ROOM_CARRIED: &[&str] = &[
    "makeEntitySetColorCmd",
    "makeEntitySetNameCmd",
    "makeGameSetCalendarSpeedCmd",
    "makeGameSetSpeedCmd",
    "makeLineCreateCmd",
    "makeLineDestroyCmd",
    "makeLineUpdateCmd",
    "makeScriptingSendEventCmd",
    "makeTownBuildingSetBlockedDevelopmentCmd",
    "makeVehicleBuyCmd",
    "makeVehicleReplaceCmd",
    "makeVehicleReverseCmd",
    "makeVehicleSellCmd",
    "makeVehicleSendToDepotCmd",
    "makeVehicleSetLineCmd",
    "makeVehicleSetManualDepartureCmd",
    "makeVehicleSetStoppedByUserCmd",
    "makeVehicleTryToDepartCmd",
];

/// What a reason is about.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    // Each of these makes the mod shared.
    /// No `mod.json`, or one that does not parse.
    Manifest,
    /// A pre-run, run or post-run script: it runs when the game loads its
    /// resources and can change any of them.
    RunScript,
    /// `addModifier`: changes resources as the game loads them.
    Modifier,
    /// A game script (`*.gs.lua`): runs in the simulation, in every game
    /// that has the mod, and sends its commands there, past the guard.
    GameScript,
    /// World content: models, constructions, vehicles, streets, economy,
    /// names, or any file in one of the game's resource folders.
    Resource,
    /// A GUI resource of a type not known to be the GUI's alone.
    UnknownResource,
    /// Writes to the resource repositories at run time (`api.res`).
    ResourceWrite,
    /// `game.interface`, TPF2's way to change the world outside `api.cmd`.
    GameInterface,
    /// Sets the game's configuration (`game.config`), as a run script can.
    ConfigWrite,
    /// Code built or reached at run time (`load`, `_G[...]`, `setfenv`,
    /// `rawset`, the `debug` library's setters): what it does cannot be read.
    Dynamic,
    /// Keeps or replaces `api.cmd.sendCommand` itself, so its commands
    /// could pass the guard unseen.
    CommandBypass,
    /// A file of a kind the scan does not know.
    UnknownFile,
    /// A file or folder that could not be read, or too many or too large.
    Unreadable,

    // These are noted and leave a personal mod personal.
    /// Sends commands through `api.cmd`: carried through the room or
    /// refused by the guard.
    Commands,
    /// A GUI plugin, replacement or other GUI resource.
    Gui,
    /// Keeps its own data in the save (`setGuiSaveData`): not simulation.
    SaveData,
    /// Uses `os` or `io`: this machine, not the world.
    System,
    /// Calls `app` to load, start, stop or save a game.
    App,
    /// Says `"cosmetic": true`, which is the author's word only: mods that
    /// change the world say it too.
    CosmeticFlag,
    /// Depends on another mod, which is scanned on its own.
    Dependency,
    /// A file outside `content/`, which the game does not load.
    NotLoaded,
}

impl Kind {
    /// Whether a reason of this kind makes the mod shared.
    pub fn shares(self) -> bool {
        matches!(
            self,
            Self::Manifest
                | Self::RunScript
                | Self::Modifier
                | Self::GameScript
                | Self::Resource
                | Self::UnknownResource
                | Self::ResourceWrite
                | Self::GameInterface
                | Self::ConfigWrite
                | Self::Dynamic
                | Self::CommandBypass
                | Self::UnknownFile
                | Self::Unreadable
        )
    }

    fn label(self) -> &'static str {
        match self {
            Self::Manifest => "manifest",
            Self::RunScript => "run script",
            Self::Modifier => "resource modifier",
            Self::GameScript => "game script",
            Self::Resource => "world content",
            Self::UnknownResource => "unknown resource type",
            Self::ResourceWrite => "resource write",
            Self::GameInterface => "game.interface",
            Self::ConfigWrite => "configuration",
            Self::Dynamic => "dynamic code",
            Self::CommandBypass => "command bypass",
            Self::UnknownFile => "unknown file",
            Self::Unreadable => "unreadable",
            Self::Commands => "commands",
            Self::Gui => "gui",
            Self::SaveData => "save data",
            Self::System => "system",
            Self::App => "app",
            Self::CosmeticFlag => "cosmetic flag",
            Self::Dependency => "dependency",
            Self::NotLoaded => "not loaded",
        }
    }
}

/// One thing the scan found, and where.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub struct Reason {
    pub kind: Kind,
    /// The file, relative to the mod's folder, with `/`; empty for the mod
    /// as a whole.
    pub file: String,
    /// The line, from 1, when it is one line's doing.
    pub line: Option<usize>,
    pub detail: String,
}

impl fmt::Display for Reason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.kind.label(), self.detail)?;
        match (self.file.is_empty(), self.line) {
            (true, _) => Ok(()),
            (false, Some(line)) => write!(f, " ({}:{line})", self.file),
            (false, None) => write!(f, " ({})", self.file),
        }
    }
}

/// What the scan made of one mod.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Report {
    /// The mod's id (`modId` in `mod.json`), else its folder's name.
    pub id: String,
    /// `revision` in `mod.json`.
    pub revision: Option<i64>,
    pub path: PathBuf,
    pub class: Class,
    /// Every reason, those that make the mod shared first.
    pub reasons: Vec<Reason>,
    /// Every command factory its loaded scripts name.
    pub commands: Vec<String>,
}

impl Report {
    /// The reasons that make the mod shared.
    pub fn sharing(&self) -> impl Iterator<Item = &Reason> {
        self.reasons.iter().filter(|r| r.kind.shares())
    }
}

impl fmt::Display for Report {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.id)?;
        if let Some(revision) = self.revision {
            write!(f, " (revision {revision})")?;
        }
        writeln!(f, ": {}", self.class)?;
        // Many files for one reason (a vehicle pack's meshes) print as one
        // line; the list keeps them all.
        let mut i = 0;
        while i < self.reasons.len() {
            let reason = &self.reasons[i];
            let same = self.reasons[i..]
                .iter()
                .take_while(|r| r.kind == reason.kind && r.detail == reason.detail)
                .count();
            let mark = if reason.kind.shares() { "!" } else { "-" };
            if same > GROUPED {
                writeln!(
                    f,
                    "  {mark} {}: {} ({} files, the first {})",
                    reason.kind.label(),
                    reason.detail,
                    same,
                    reason.file
                )?;
                i += same;
            } else {
                writeln!(f, "  {mark} {reason}")?;
                i += 1;
            }
        }
        Ok(())
    }
}

/// The game's own resource folders (under `base/content/` of build 40408,
/// and TPF2's `res/`): a mod's file under one of these is world content.
const RESOURCE_DIRS: &[&str] = &[
    "animal",
    "assets",
    "base",
    "bridge",
    "buildings",
    "cargos",
    "characters",
    "climates",
    "config",
    "construction",
    "depots",
    "economy",
    "environments",
    "game_mechanics",
    "industries",
    "infrastructure",
    "landmarks",
    "mission",
    "model",
    "models",
    "multiple_unit",
    "names",
    "placeholders",
    "rendering",
    "res",
    "stations",
    "street",
    "terrain",
    "track",
    "tunnel",
    "vehicle",
    "warehouses",
];

/// Extensions of the game's resource files.
const RESOURCE_EXTENSIONS: &[&str] = &[
    "ani", "blob", "bridge", "con", "fbx", "grp", "lod", "mdl", "module", "msh", "mtl", "street",
    "zip", "track", "tunnel",
];

/// Extensions of files that cannot change the simulation by themselves:
/// pictures, sounds, fonts, text, and Teal type declarations. A picture or
/// sound a resource uses is judged with that resource.
const INERT_EXTENSIONS: &[&str] = &[
    "bmp", "dds", "jpeg", "jpg", "ktx", "md", "mo", "ogg", "otf", "pdf", "png", "po", "svg", "tga",
    "ttf", "txt", "wav", "webp",
];

/// GUI resource types (`type = "..."` in a `.res.lua`) that only the GUI
/// reads: those in the game's own `gui.zip` (build 40408) and the react
/// ones the mods use. What they make, a name, a menu entry, a tool, goes
/// through `api.cmd` like any click.
const GUI_RESOURCE_TYPES: &[&str] = &[
    "drag_and_drop",
    "firstStopToSend_scheme",
    "menu_category",
    "menu_filter_category",
    "react-replacement-config",
    "rename_scheme",
    "rename_scheme_component",
];

/// `app` functions that load, start, stop or save a game.
const APP_CALLS: &[&str] = &[
    "loadGame",
    "loadMission",
    "restart",
    "saveGame",
    "startGame",
    "startGame2",
    "stopGame",
];

/// The `debug` library's functions that change code or state.
const DEBUG_SETTERS: &[&str] = &[
    "setfenv",
    "sethook",
    "setlocal",
    "setmetatable",
    "setupvalue",
    "setuservalue",
    "upvaluejoin",
];

/// Scans the mod in `dir`.
pub fn scan(dir: &Path) -> Report {
    let mut reasons = BTreeSet::new();
    let mut id = dir
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let mut revision = None;

    match fs::read_to_string(dir.join("mod.json")) {
        Ok(text) => match serde_json::from_str::<serde_json::Value>(&text) {
            Ok(manifest) => {
                if let Some(named) = manifest.get("modId").and_then(|v| v.as_str()) {
                    id = named.to_owned();
                }
                revision = manifest.get("revision").and_then(serde_json::Value::as_i64);
                read_manifest(&manifest, &mut reasons);
            }
            Err(error) => {
                add(
                    &mut reasons,
                    Kind::Manifest,
                    "mod.json",
                    None,
                    format!("mod.json does not parse: {error}"),
                );
            }
        },
        Err(_) => {
            add(
                &mut reasons,
                Kind::Manifest,
                "",
                None,
                "no mod.json: not a Transport Fever 3 mod".into(),
            );
        }
    }

    let mut files = Vec::new();
    if let Err(why) = walk(dir, dir, 0, &mut files) {
        add(&mut reasons, Kind::Unreadable, "", None, why);
    }
    let has_content = dir.join("content").is_dir();
    let mut commands = BTreeSet::new();
    for file in &files {
        scan_file(dir, file, has_content, &mut reasons, &mut commands);
    }

    let mut reasons: Vec<Reason> = reasons.into_iter().collect();
    reasons.sort_by_key(|r| (!r.kind.shares(), r.kind, r.file.clone(), r.line));
    let class = classify(&reasons, &commands);
    Report {
        id,
        revision,
        path: dir.to_path_buf(),
        class,
        reasons,
        commands: commands.into_iter().collect(),
    }
}

/// Personal when nothing makes the mod shared; carried when all that does
/// is its game scripts (and run scripts that change nothing), and every
/// command it makes is one the room carries; else shared.
fn classify(reasons: &[Reason], commands: &BTreeSet<String>) -> Class {
    let sharing: Vec<Kind> = reasons
        .iter()
        .filter(|r| r.kind.shares())
        .map(|r| r.kind)
        .collect();
    if sharing.is_empty() {
        return Class::Personal;
    }
    let only_scripts = sharing
        .iter()
        .all(|k| matches!(k, Kind::GameScript | Kind::RunScript));
    let has_game_script = sharing.contains(&Kind::GameScript);
    let carried = commands.iter().all(|c| ROOM_CARRIED.contains(&c.as_str()));
    if only_scripts && has_game_script && carried {
        Class::Carried
    } else {
        Class::Shared
    }
}

fn add(
    reasons: &mut BTreeSet<Reason>,
    kind: Kind,
    file: &str,
    line: Option<usize>,
    detail: String,
) {
    reasons.insert(Reason {
        kind,
        file: file.to_owned(),
        line,
        detail,
    });
}

fn read_manifest(manifest: &serde_json::Value, reasons: &mut BTreeSet<Reason>) {
    for hook in ["preRunScript", "runScript", "postRunScript"] {
        let named = manifest
            .get(hook)
            .and_then(|v| v.get("fileName"))
            .and_then(|v| v.as_str())
            .unwrap_or_default();
        if !named.is_empty() {
            add(
                reasons,
                Kind::RunScript,
                "mod.json",
                None,
                format!("{hook} {named} runs as the game loads its resources"),
            );
        }
    }
    if manifest
        .get("cosmetic")
        .and_then(serde_json::Value::as_bool)
        == Some(true)
    {
        add(
            reasons,
            Kind::CosmeticFlag,
            "mod.json",
            None,
            "says it is cosmetic, which decides nothing here".into(),
        );
    }
    if let Some(list) = manifest.get("dependencies").and_then(|v| v.as_array()) {
        for dependency in list {
            let name = dependency
                .pointer("/mod/modId/name")
                .or_else(|| dependency.pointer("/modId"))
                .and_then(|v| v.as_str())
                .map_or_else(|| dependency.to_string(), str::to_owned);
            add(
                reasons,
                Kind::Dependency,
                "mod.json",
                None,
                format!("depends on {name}"),
            );
        }
    }
}

/// Every file under `dir`, by path; follows no links.
fn walk(root: &Path, dir: &Path, depth: usize, out: &mut Vec<PathBuf>) -> Result<(), String> {
    if depth > MAX_DEPTH {
        return Err(format!(
            "{} is nested deeper than {MAX_DEPTH} folders",
            relative(root, dir)
        ));
    }
    let mut entries: Vec<_> = fs::read_dir(dir)
        .map_err(|e| format!("{}: {e}", relative(root, dir)))?
        .filter_map(Result::ok)
        .collect();
    entries.sort_by_key(fs::DirEntry::file_name);
    for entry in entries {
        let path = entry.path();
        let kind = entry
            .file_type()
            .map_err(|e| format!("{}: {e}", relative(root, &path)))?;
        if kind.is_dir()
            && depth == 0
            && VCS_DIRS.contains(&entry.file_name().to_string_lossy().as_ref())
        {
            // A version control folder beside the mod: never loaded.
            continue;
        }
        if kind.is_symlink() {
            return Err(format!(
                "{} is a link, which is not followed",
                relative(root, &path)
            ));
        } else if kind.is_dir() {
            walk(root, &path, depth + 1, out)?;
        } else {
            if out.len() >= MAX_FILES {
                return Err(format!("more than {MAX_FILES} files"));
            }
            out.push(path);
        }
    }
    Ok(())
}

fn relative(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

/// What a file is, by its name and where it lies.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Role {
    /// Metadata of the mod itself.
    Meta,
    Inert,
    GameScript,
    GuiResource,
    Style,
    Script,
    Resource,
    Unknown,
}

fn role(rel: &str) -> Role {
    let lower = rel.to_ascii_lowercase();
    let name = lower.rsplit('/').next().unwrap_or(&lower);
    let extension = name.rsplit_once('.').map(|(_, e)| e).unwrap_or("");
    if matches!(lower.as_str(), "mod.json" | "_content.json")
        || lower.starts_with("_metadata/")
        || name.starts_with("license")
        || name.starts_with("readme")
        || name.starts_with("changelog")
        || matches!(name, "strings.json" | "strings.lua" | "tlconfig.lua")
    {
        return Role::Meta;
    }
    if name.ends_with(".d.tl") || INERT_EXTENSIONS.contains(&extension) {
        return Role::Inert;
    }
    if name.ends_with(".gs.lua") || name.ends_with(".gs.tl") {
        return Role::GameScript;
    }
    if RESOURCE_EXTENSIONS.contains(&extension) {
        return Role::Resource;
    }
    if name.ends_with(".res.lua") {
        return Role::GuiResource;
    }
    if name.ends_with(".css.lua") {
        return Role::Style;
    }
    if matches!(extension, "lua" | "tl") {
        // A script in one of the game's resource folders is a resource
        // (a construction's script, a names list, a config).
        let inside = lower.strip_prefix("content/").unwrap_or(&lower);
        let first = inside.split('/').next().unwrap_or("");
        if inside.contains('/') && RESOURCE_DIRS.contains(&first) {
            return Role::Resource;
        }
        return Role::Script;
    }
    Role::Unknown
}

fn scan_file(
    root: &Path,
    path: &Path,
    has_content: bool,
    reasons: &mut BTreeSet<Reason>,
    commands: &mut BTreeSet<String>,
) {
    let rel = relative(root, path);
    let role = role(&rel);
    let loaded = !has_content || rel.starts_with("content/");
    match role {
        Role::Meta | Role::Inert => return,
        Role::GameScript => {
            add(
                reasons,
                Kind::GameScript,
                &rel,
                None,
                "runs in the simulation of every game that has the mod".into(),
            );
        }
        Role::Resource => {
            if loaded {
                add(reasons, Kind::Resource, &rel, None, "world content".into());
            } else {
                add(
                    reasons,
                    Kind::NotLoaded,
                    &rel,
                    None,
                    "outside content/".into(),
                );
            }
            return;
        }
        Role::Unknown => {
            if loaded {
                add(
                    reasons,
                    Kind::UnknownFile,
                    &rel,
                    None,
                    "a file of a kind not known to be inert".into(),
                );
            } else {
                add(
                    reasons,
                    Kind::NotLoaded,
                    &rel,
                    None,
                    "outside content/".into(),
                );
            }
            return;
        }
        Role::Style => {
            add(reasons, Kind::Gui, &rel, None, "a style sheet".into());
            return;
        }
        Role::GuiResource | Role::Script => {}
    }
    let text = match read_text(path) {
        Ok(text) => text,
        Err(why) => {
            add(reasons, Kind::Unreadable, &rel, None, why);
            return;
        }
    };
    let tokens = lexer::tokens(&text);
    if role == Role::GuiResource {
        gui_resource(&rel, &tokens, reasons);
    }
    if !loaded {
        // Sources beside content/ (Teal before compiling, say) are not
        // loaded, but what they call is still worth knowing: noted only.
        let mut found = BTreeSet::new();
        calls(&rel, &tokens, &mut found, &mut BTreeSet::new());
        if found.iter().any(|r| r.kind.shares()) {
            add(
                reasons,
                Kind::NotLoaded,
                &rel,
                None,
                "outside content/; its calls are not counted".into(),
            );
        }
        return;
    }
    calls(&rel, &tokens, reasons, commands);
}

fn read_text(path: &Path) -> Result<String, String> {
    let size = fs::metadata(path).map_err(|e| e.to_string())?.len();
    if size > MAX_FILE_BYTES {
        return Err(format!("{size} bytes, more than the scan reads"));
    }
    let bytes = fs::read(path).map_err(|e| e.to_string())?;
    let bytes = bytes.strip_prefix(b"\xef\xbb\xbf").unwrap_or(&bytes);
    String::from_utf8(bytes.to_vec()).map_err(|_| "not UTF-8 text".to_owned())
}

/// A `.res.lua`: its `type = "..."`.
fn gui_resource(rel: &str, tokens: &[Located], reasons: &mut BTreeSet<Reason>) {
    let mut types = Vec::new();
    for window in tokens.windows(3) {
        if let [
            Located {
                token: Token::Name(key),
                ..
            },
            Located {
                token: Token::Punct('='),
                ..
            },
            Located {
                token: Token::Str(value),
                line,
            },
        ] = window
            && key == "type"
        {
            types.push((value.clone(), *line));
        }
    }
    if types.is_empty() {
        add(
            reasons,
            Kind::UnknownResource,
            rel,
            None,
            "a resource that names no type".into(),
        );
    }
    for (kind, line) in types {
        if kind.starts_with("react-plugin") || GUI_RESOURCE_TYPES.contains(&kind.as_str()) {
            add(
                reasons,
                Kind::Gui,
                rel,
                Some(line),
                format!("GUI resource {kind}"),
            );
        } else {
            add(
                reasons,
                Kind::UnknownResource,
                rel,
                Some(line),
                format!("resource type {kind} is not known to be the GUI's alone"),
            );
        }
    }
}

fn name_at(tokens: &[Located], i: usize) -> Option<&str> {
    match tokens.get(i).map(|t| &t.token) {
        Some(Token::Name(n)) => Some(n),
        _ => None,
    }
}

fn punct_at(tokens: &[Located], i: usize, c: char) -> bool {
    matches!(tokens.get(i).map(|t| &t.token), Some(Token::Punct(p)) if *p == c)
}

/// Whether the name at `i` is reached as a field (`x.name`, `x:name`).
fn is_field(tokens: &[Located], i: usize) -> bool {
    i > 0 && (punct_at(tokens, i - 1, '.') || punct_at(tokens, i - 1, ':'))
        // `..` is concatenation, not a field.
        && !(i > 1 && punct_at(tokens, i - 2, '.'))
}

/// Whether `name` at `i` is `api.<path...>.name`, the path given
/// outermost first (`["cmd"]` for `api.cmd.name`).
fn path_before(tokens: &[Located], i: usize, path: &[&str]) -> bool {
    let mut at = i;
    for part in path.iter().rev() {
        if at < 2 || !punct_at(tokens, at - 1, '.') || name_at(tokens, at - 2) != Some(part) {
            return false;
        }
        at -= 2;
    }
    true
}

/// Whether the path starting at the name at `i` (`name.a.b[...]`) is
/// assigned to: followed by `=`, not `==`.
fn assigns_after(tokens: &[Located], i: usize) -> bool {
    let mut at = i + 1;
    loop {
        match tokens.get(at).map(|t| &t.token) {
            // `.name`
            Some(Token::Punct('.')) if name_at(tokens, at + 1).is_some() => at += 2,
            // `[...]`, nested brackets included
            Some(Token::Punct('[')) => {
                let mut depth = 0usize;
                loop {
                    match tokens.get(at).map(|t| &t.token) {
                        Some(Token::Punct('[')) => depth += 1,
                        Some(Token::Punct(']')) => {
                            depth -= 1;
                            if depth == 0 {
                                break;
                            }
                        }
                        None => return false,
                        _ => {}
                    }
                    at += 1;
                }
                at += 1;
            }
            Some(Token::Punct('=')) => return !punct_at(tokens, at + 1, '='),
            _ => return false,
        }
    }
}

/// The names a script binds to `game`, `game.config` or a resource
/// repository (`local g = game`, `local rep = api.res.modelRep`), which the
/// checks below follow as they follow the paths themselves.
#[derive(Default)]
struct Aliases {
    game: BTreeSet<String>,
    config: BTreeSet<String>,
    res: BTreeSet<String>,
}

impl Aliases {
    fn of(tokens: &[Located]) -> Self {
        let mut aliases = Self::default();
        for i in 0..tokens.len() {
            let Some(name) = name_at(tokens, i) else {
                continue;
            };
            if is_field(tokens, i) || !punct_at(tokens, i + 1, '=') || punct_at(tokens, i + 2, '=')
            {
                continue;
            }
            // The value: a plain path, `a.b.c`, and nothing after it.
            let mut at = i + 2;
            let mut path = Vec::new();
            while let Some(part) = name_at(tokens, at) {
                path.push(part);
                if punct_at(tokens, at + 1, '.') && name_at(tokens, at + 2).is_some() {
                    at += 2;
                } else {
                    at += 1;
                    break;
                }
            }
            if path.is_empty()
                || punct_at(tokens, at, '(')
                || punct_at(tokens, at, '[')
                || punct_at(tokens, at, ':')
            {
                continue;
            }
            let set = match path.as_slice() {
                [root] if *root == "game" || aliases.game.contains(*root) => &mut aliases.game,
                ["game", "config"] => &mut aliases.config,
                [root, "config"] if aliases.game.contains(*root) => &mut aliases.config,
                ["api", "res", ..] => &mut aliases.res,
                [root, ..] if aliases.res.contains(*root) => &mut aliases.res,
                _ => continue,
            };
            set.insert(name.to_owned());
        }
        aliases
    }

    fn is_game(&self, name: Option<&str>) -> bool {
        name.is_some_and(|n| n == "game" || self.game.contains(n))
    }

    fn is_rep(&self, name: Option<&str>) -> bool {
        name.is_some_and(|n| n.ends_with("Rep") || self.res.contains(n))
    }
}

/// The names whose fields a script must name as written: an index by a
/// string (`api["cmd"]`) or a computed one hides what it reaches.
fn hides_its_field(tokens: &[Located], i: usize, aliases: &Aliases) -> Option<String> {
    let name = name_at(tokens, i)?;
    if !punct_at(tokens, i + 1, '[') {
        return None;
    }
    let field = is_field(tokens, i);
    let watched = match name {
        "api" | "game" => !field,
        "cmd" | "res" => field,
        "interface" | "config" => field && i >= 2 && aliases.is_game(name_at(tokens, i - 2)),
        n => {
            (!field && (aliases.is_game(Some(n)) || aliases.config.contains(n)))
                || aliases.is_rep(Some(n))
        }
    };
    watched.then(|| name.to_owned())
}

/// What a loaded script calls.
fn calls(
    rel: &str,
    tokens: &[Located],
    reasons: &mut BTreeSet<Reason>,
    commands: &mut BTreeSet<String>,
) {
    let mut factories = BTreeSet::new();
    let mut first_command = None;
    let aliases = Aliases::of(tokens);
    for (i, located) in tokens.iter().enumerate() {
        let Token::Name(name) = &located.token else {
            continue;
        };
        let line = Some(located.line);
        if let Some(indexed) = hides_its_field(tokens, i, &aliases) {
            add(
                reasons,
                Kind::Dynamic,
                rel,
                line,
                format!("{indexed} indexed by a string or a computed name"),
            );
        }
        if !is_field(tokens, i)
            && aliases.config.contains(name.as_str())
            && (punct_at(tokens, i + 1, '.') || punct_at(tokens, i + 1, '['))
            && assigns_after(tokens, i)
        {
            add(
                reasons,
                Kind::ConfigWrite,
                rel,
                line,
                format!("sets game.config through {name}"),
            );
        }
        let called = punct_at(tokens, i + 1, '(')
            || matches!(
                tokens.get(i + 1).map(|t| &t.token),
                Some(Token::Str(_)) | Some(Token::Punct('{'))
            );
        let assigned = punct_at(tokens, i + 1, '=') && !punct_at(tokens, i + 2, '=');
        match name.as_str() {
            n if n.starts_with("make") && n.ends_with("Cmd") && is_field(tokens, i) => {
                factories.insert(n.to_owned());
                first_command.get_or_insert(located.line);
            }
            "sendCommand" if is_field(tokens, i) || path_before(tokens, i, &["cmd"]) => {
                if assigned {
                    add(
                        reasons,
                        Kind::CommandBypass,
                        rel,
                        line,
                        "replaces sendCommand".into(),
                    );
                } else if called {
                    first_command.get_or_insert(located.line);
                } else {
                    add(
                        reasons,
                        Kind::CommandBypass,
                        rel,
                        line,
                        "keeps sendCommand as a value, which the guard may not see".into(),
                    );
                }
            }
            "interface"
                if is_field(tokens, i) && i >= 2 && aliases.is_game(name_at(tokens, i - 2)) =>
            {
                add(
                    reasons,
                    Kind::GameInterface,
                    rel,
                    line,
                    "uses game.interface".into(),
                );
            }
            "config"
                if is_field(tokens, i)
                    && i >= 2
                    && aliases.is_game(name_at(tokens, i - 2))
                    && assigns_after(tokens, i) =>
            {
                add(
                    reasons,
                    Kind::ConfigWrite,
                    rel,
                    line,
                    "sets game.config".into(),
                );
            }
            "addModifier" if !is_field(tokens, i) => {
                add(
                    reasons,
                    Kind::Modifier,
                    rel,
                    line,
                    "addModifier changes resources as they load".into(),
                );
            }
            "addAsTable" | "setAsTable" | "removeAsTable" if is_field(tokens, i) => {
                add(
                    reasons,
                    Kind::ResourceWrite,
                    rel,
                    line,
                    format!("{name} writes a resource"),
                );
            }
            "add" | "set" | "remove" | "setVisible"
                if is_field(tokens, i)
                    && called
                    && i >= 2
                    && aliases.is_rep(name_at(tokens, i - 2)) =>
            {
                add(
                    reasons,
                    Kind::ResourceWrite,
                    rel,
                    line,
                    format!(
                        "{}.{name} changes a resource repository",
                        name_at(tokens, i - 2).unwrap_or("")
                    ),
                );
            }
            "load" | "loadstring" | "dofile" | "loadfile" | "setfenv" | "_ENV"
                if !is_field(tokens, i) && (called || name == "_ENV") =>
            {
                add(
                    reasons,
                    Kind::Dynamic,
                    rel,
                    line,
                    format!("{name} runs or reaches code the scan cannot read"),
                );
            }
            "_G" if !is_field(tokens, i) && punct_at(tokens, i + 1, '[') => {
                add(
                    reasons,
                    Kind::Dynamic,
                    rel,
                    line,
                    "_G indexed by a computed name".into(),
                );
            }
            "setmetatable" | "rawset"
                if !is_field(tokens, i)
                    && called
                    && matches!(name_at(tokens, i + 2), Some("_G" | "api" | "game")) =>
            {
                add(
                    reasons,
                    Kind::Dynamic,
                    rel,
                    line,
                    format!("{name} on {}", name_at(tokens, i + 2).unwrap_or("")),
                );
            }
            setter if DEBUG_SETTERS.contains(&setter) && path_before(tokens, i, &["debug"]) => {
                add(reasons, Kind::Dynamic, rel, line, format!("debug.{setter}"));
            }
            "os" | "io" if !is_field(tokens, i) && punct_at(tokens, i + 1, '.') => {
                let what = name_at(tokens, i + 2).unwrap_or("");
                add(reasons, Kind::System, rel, line, format!("{name}.{what}"));
            }
            "setGuiSaveData" if is_field(tokens, i) => {
                add(
                    reasons,
                    Kind::SaveData,
                    rel,
                    line,
                    "keeps its data in the save".into(),
                );
            }
            call if APP_CALLS.contains(&call)
                && i >= 2
                && is_field(tokens, i)
                && name_at(tokens, i - 2) == Some("app") =>
            {
                add(reasons, Kind::App, rel, line, format!("app.{call}"));
            }
            _ => {}
        }
    }
    commands.extend(factories.iter().cloned());
    if let Some(line) = first_command {
        let detail = if factories.is_empty() {
            "sends commands".to_owned()
        } else {
            format!(
                "makes {}",
                factories.into_iter().collect::<Vec<_>>().join(", ")
            )
        };
        add(reasons, Kind::Commands, rel, Some(line), detail);
    }
}

#[cfg(test)]
mod tests;
