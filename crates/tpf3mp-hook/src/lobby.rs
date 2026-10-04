//! The lobby as the main menu's Multiplayer window sees it (D17,
//! docs/LOBBY.md): the launcher's connection, room and chat, and the
//! player's actions back to the launcher, over the link to its agent.
//!
//! The window's Lua asks the hook for the state and hands it actions over
//! the request channel in `crate::menu_entry`. The state crosses as a Lua
//! table literal ([`LobbyState::to_lua`]), which the window evaluates with
//! `load` in an empty environment; an action crosses as one small JSON
//! object ([`parse_action`]), which the window builds by hand. Both are
//! plain text a person can read in a log.
//!
//! Behind the channel the launcher answers, not the hook: an action is
//! queued ([`queue`]) and handed to the agent as a `ToAgent::Lobby` by the
//! step driver ([`exchange`], `StepDriver::lobby`), and the lobby the agent
//! sends back (`ToHook::Lobby`) is what the window shows next. The exchange
//! runs whenever the window asks, which is how the link is read at the main
//! menu, where no step of the game runs, and after each of the game's
//! steps. A game whose hook has no link to its launcher (the step gate did
//! not install) says so, and takes no action (fail closed).

use std::{
    collections::VecDeque,
    sync::{Mutex, MutexGuard, PoisonError},
};

use serde::Deserialize;
use tpf3mp_bridge::{
    LobbyAction, LobbyConnection, LobbyHave, LobbyListing, LobbyModClass, LobbyRoomList, LobbyView,
    LobbyWorld, ModName,
};
use tpf3mp_proto::{FixedBytes, PlayerId, Text};

use crate::step::StepHandler;

/// Most actions waiting for the launcher.
const MAX_QUEUED: usize = 16;

/// Whether the launcher's connection to the server is up.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Connection {
    #[default]
    Disconnected,
    Connecting,
    Connected,
}

impl Connection {
    fn as_str(self) -> &'static str {
        match self {
            Self::Disconnected => "disconnected",
            Self::Connecting => "connecting",
            Self::Connected => "connected",
        }
    }
}

/// One player in the room.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Member {
    /// The player's key, as 64 hex digits: what a kick names.
    pub id: String,
    pub name: String,
    pub ready: bool,
    pub owner: bool,
    pub you: bool,
    pub connected: bool,
    /// Whether the player's game matches the owner's: `same`, `differs` or
    /// `unknown`.
    pub content: String,
    /// The banner the player picked, if any: empty for their default.
    pub banner: String,
    /// Where the player's game is with the room's world while it comes in:
    /// `fetching` (with [`Member::percent`]), `loading`, or empty.
    pub loading: String,
    /// How much of the world it has fetched, in percent, while `fetching`.
    pub percent: u8,
    /// How the player's game differs from the room's, in counts: the room's
    /// mods it lacks, has in another version, and runs besides; all 0 while
    /// it does not differ that way.
    pub missing: u16,
    pub changed: u16,
    pub extra: u16,
}

/// The room the player is in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Room {
    pub name: String,
    pub invite: String,
    /// `lobby` or `playing`.
    pub phase: String,
    pub you_own: bool,
    pub max_players: u32,
    pub has_password: bool,
    pub members: Vec<Member>,
    /// Co-op (`false`) or competitive.
    pub competitive: bool,
    /// In the lobby: the save the room starts from, as the room names it;
    /// `None` when the owner's game provides the world.
    pub start: Option<StartSave>,
    /// For the owner: the save they picked on its way to the room, and how
    /// much of it went up, in percent.
    pub upload: Option<(String, u8)>,
}

/// The save a room starts from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StartSave {
    pub name: String,
    /// Its climate, such as `temperate`; empty unknown.
    pub map: String,
    /// Its year; 0 unknown.
    pub year: u16,
    /// Whether the room has it.
    pub arrived: bool,
}

/// One line of the room's chat.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChatLine {
    pub from: String,
    pub text: String,
    pub you: bool,
}

/// Rules a room can be played by.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rules {
    pub name: String,
    pub description: String,
}

/// One mod the player has installed (docs/MODS.md, "Choosing mods").
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModEntry {
    /// What `choose_mod` names.
    pub id: String,
    pub name: String,
    /// `personal`, `carried` or `shared`.
    pub class: &'static str,
    pub reason: String,
    pub chosen: bool,
    pub choosable: bool,
}

/// One of the room's mods.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RoomMod {
    pub id: String,
    /// Its name for players, as the owner's game has it.
    pub name: String,
    /// The room's version.
    pub version: String,
    /// This player's version; empty when it lacks the mod.
    pub yours: String,
    /// `yes`, `no` or `other_version`: whether this player has it.
    pub have: &'static str,
    /// Where the owner's game has it from: `mod.io`, `StagingArea`, ...
    pub source: String,
    /// Its Mod Hub number as text, empty for none: the owner's claim, which
    /// the window resolves through this player's game before offering it.
    pub modio: String,
}

/// Everything the lobby window shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LobbyState {
    pub connection: Connection,
    /// The server as players see it: its name, or its address.
    pub server: String,
    /// The server's address, `host:port`, for the server setting.
    pub server_address: String,
    /// The launcher's default server, which `set_server` with an empty
    /// server goes back to; empty without one.
    pub server_default: String,
    /// The banner this player picked: empty for their default.
    pub banner: String,
    /// The campaign portraits this game can show, by id
    /// (`tpf3mp_proto::PORTRAITS`): what the banner picker offers besides
    /// the banners.
    pub portraits: Vec<String>,
    pub name: String,
    /// The last thing that went wrong, for the window to show.
    pub error: Option<String>,
    /// The last thing worth telling the player.
    pub notice: Option<String>,
    pub room: Option<Room>,
    pub chat: Vec<ChatLine>,
    /// Whether this game has a link to its launcher: without it the window
    /// can only say so.
    pub linked: bool,
    /// Whether the launcher has said anything yet.
    pub heard: bool,
    /// The rules the server offers new rooms, its default first.
    pub rules: Vec<Rules>,
    /// The player's saves, newest first.
    pub saves: Vec<String>,
    /// The save offered first for a new room.
    pub start_save: Option<String>,
    /// The room's world in this game: `none`, `fetching`, `loading` or
    /// `playing`, and while fetching, bytes of the total so far.
    pub world: &'static str,
    pub bytes: u64,
    pub total: u64,
    /// How this game differs from the room's, while it does.
    pub differences: Option<String>,
    /// The player's installed mods, the choosable first.
    pub mods: Vec<ModEntry>,
    /// The room's mods, and whether this player has each.
    pub room_mods: Vec<RoomMod>,
    /// The room's mods beyond `room_mods`.
    pub room_mods_more: u32,
    /// Of all the room's mods, how many this player lacks, and how many it
    /// has in another version.
    pub room_mods_missing: u16,
    pub room_mods_other: u16,
    /// The settings of the room's mods, as its owner picked them: mod,
    /// setting, value.
    pub room_params: Vec<(String, String, i64)>,
    /// The page of public rooms last asked for.
    pub rooms: Option<LobbyRoomList>,
    /// The launcher's run, which every line of its diagnostics carries:
    /// shown with a Copy. Empty while diagnostics are off.
    pub log_session: String,
}

/// What the window sends, as JSON: the tag `action` plus the fields, e.g.
/// `{"action":"create","room":"Alps","password":"","max_players":8}`. A
/// server the window names with Connect is not taken: the launcher plays on
/// its own (D12). Only `set_server`, the player's server setting, changes
/// it.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(tag = "action", rename_all = "snake_case")]
enum WindowAction {
    Connect {
        name: String,
    },
    Disconnect,
    Create {
        #[serde(default)]
        room: String,
        #[serde(default = "default_max_players")]
        max_players: u32,
        #[serde(default)]
        password: String,
        /// Empty for the server's default.
        #[serde(default)]
        rules: String,
        /// Absent for the launcher's own start save; empty for none.
        #[serde(default)]
        start_save: Option<String>,
        /// Lists the room in the server's room list.
        #[serde(default)]
        public: bool,
        /// The start save's climate and year, for the list.
        #[serde(default)]
        map: String,
        #[serde(default)]
        year: u16,
        /// Competitive rather than co-op.
        #[serde(default)]
        competitive: bool,
    },
    ListRooms {
        #[serde(default)]
        page: u16,
    },
    Join {
        invite: String,
        #[serde(default)]
        password: String,
    },
    Ready {
        ready: bool,
    },
    Start,
    Kick {
        player: String,
    },
    Chat {
        text: String,
    },
    Leave,
    ChooseMod {
        id: String,
        chosen: bool,
    },
    /// The server setting: a `host:port`, or empty for the default. The
    /// launcher checks it.
    SetServer {
        #[serde(default)]
        server: String,
    },
    /// Empty for the default.
    SetBanner {
        #[serde(default)]
        banner: String,
    },
    /// The owner's save for the room to start from now; empty for none.
    /// The map and year are what the window read of it.
    ChooseStart {
        #[serde(default)]
        save: String,
        #[serde(default)]
        map: String,
        #[serde(default)]
        year: u16,
    },
    /// The owner's choice of the room's mods on the game's Load Game page,
    /// in its activation order, with the settings it holds, and of the save
    /// the room starts from (none when `save` is empty).
    ChooseRoomMods {
        #[serde(default)]
        save: String,
        #[serde(default)]
        map: String,
        #[serde(default)]
        year: u16,
        #[serde(default)]
        mods: Vec<WindowSelected>,
        #[serde(default)]
        params: Vec<WindowSetting>,
    },
    /// Find the installed mods again.
    RescanMods,
}

/// One mod of the owner's selection, as the window names it.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
struct WindowSelected {
    id: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    source: String,
    /// The Mod Hub number, as text; empty for none.
    #[serde(default)]
    modio: String,
}

/// One setting of a mod of the selection.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
struct WindowSetting {
    #[serde(rename = "mod")]
    id: String,
    key: String,
    value: i64,
}

fn default_max_players() -> u32 {
    8
}

fn text<const MAX: usize>(value: &str, what: &str) -> Result<Text<MAX>, String> {
    Text::new(value.trim()).map_err(|_| format!("that {what} is too long"))
}

fn password(value: &str) -> Result<Option<Text<64>>, String> {
    if value.is_empty() {
        Ok(None)
    } else {
        Text::new(value)
            .map(Some)
            .map_err(|_| "that password is too long".to_owned())
    }
}

/// A player named by 64 hex digits, as [`Member::id`] names them.
pub(crate) fn player(hex: &str) -> Option<PlayerId> {
    let hex = hex.trim();
    if hex.len() != 64 || !hex.is_ascii() {
        return None;
    }
    let mut bytes = [0u8; 32];
    for (index, byte) in bytes.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&hex[index * 2..index * 2 + 2], 16).ok()?;
    }
    Some(PlayerId(FixedBytes(bytes)))
}

/// A player's id as the mod names it: 64 lowercase hex digits.
pub(crate) fn hex(player: &PlayerId) -> String {
    player
        .as_bytes()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// What the window asks of the hook itself, never the launcher.
#[derive(Debug, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
enum LocalAction {
    /// Puts the text, the room's invite code, on the clipboard.
    Copy { text: String },
}

/// Does what the window asks of the hook itself (`{"action":"copy",
/// "text":"K7QM2X"}`: the clipboard, [`crate::clipboard`]): `None` for an
/// action the launcher takes ([`parse_action`]).
pub fn local_action(json: &str) -> Option<Result<(), String>> {
    let action: LocalAction = serde_json::from_str(json).ok()?;
    Some(match action {
        LocalAction::Copy { text } => crate::clipboard::copy(&text),
    })
}

/// The room's invite as the launcher last told it, if the player is in a
/// room: the game's Multiplayer window shows it with its Copy.
pub fn invite() -> Option<String> {
    let menu = menu();
    let invite = menu.view.as_ref()?.room.as_ref()?.invite.as_ref()?;
    Some(invite.as_str().to_owned())
}

/// Parses one action from the window's JSON into what the launcher takes.
pub fn competitive() -> Option<bool> {
    Some(menu().view.as_ref()?.room.as_ref()?.competitive)
}

pub fn parse_action(json: &str) -> Result<LobbyAction, String> {
    let action: WindowAction =
        serde_json::from_str(json).map_err(|error| format!("not an action: {error}"))?;
    Ok(match action {
        WindowAction::Connect { name } => LobbyAction::Connect {
            name: text(&name, "name")?,
        },
        WindowAction::Disconnect => LobbyAction::Disconnect,
        WindowAction::Create {
            room,
            max_players,
            password: given,
            rules,
            start_save,
            public,
            map,
            year,
            competitive,
        } => LobbyAction::Create {
            room: text(&room, "room name")?,
            max_players: u8::try_from(max_players).unwrap_or(u8::MAX),
            password: password(&given)?,
            rules: Some(text(&rules, "rules name")?).filter(|rules| !rules.as_str().is_empty()),
            start_save: start_save
                .map(|save| text::<{ tpf3mp_bridge::MAX_SAVE_NAME }>(&save, "save name"))
                .transpose()?,
            listing: public
                .then(|| {
                    Ok::<_, String>(LobbyListing {
                        map: text(&map, "map")?,
                        year,
                    })
                })
                .transpose()?,
            competitive,
        },
        WindowAction::ListRooms { page } => LobbyAction::ListRooms { page },
        WindowAction::Join {
            invite,
            password: given,
        } => LobbyAction::Join {
            invite: text(&invite, "invite")?,
            password: password(&given)?,
        },
        WindowAction::Ready { ready } => LobbyAction::Ready { ready },
        WindowAction::Start => LobbyAction::Start,
        WindowAction::Kick { player: id } => LobbyAction::Kick {
            player: player(&id).ok_or("that is not a player")?,
        },
        WindowAction::Chat { text: said } => LobbyAction::Chat {
            text: text(&said, "message")?,
        },
        WindowAction::Leave => LobbyAction::Leave,
        WindowAction::ChooseMod { id, chosen } => LobbyAction::ChooseMod {
            id: ModName::new(id.trim()).map_err(|_| "that mod's id is too long".to_owned())?,
            chosen,
        },
        WindowAction::SetServer { server } => LobbyAction::SetServer {
            server: text(&server, "server")?,
        },
        WindowAction::SetBanner { banner } => LobbyAction::SetBanner {
            banner: match banner.trim() {
                "" => None,
                id if tpf3mp_proto::is_banner(id) => Some(text(id, "banner")?),
                _ => return Err("there is no such banner".to_owned()),
            },
        },
        WindowAction::ChooseStart { save, map, year } => LobbyAction::ChooseStart {
            save: text::<{ tpf3mp_bridge::MAX_SAVE_NAME }>(&save, "save name")?,
            // What the game read of the save is only shown: too long, it is
            // cut short rather than refused.
            map: Text::lossy(map.trim()),
            year,
        },
        WindowAction::ChooseRoomMods {
            save,
            map,
            year,
            mods,
            params,
        } => {
            // A mod's id names it whole: one too long is refused, never cut
            // short. Its name and source are only shown.
            let mods = mods
                .iter()
                .map(|m| {
                    Ok(tpf3mp_bridge::LobbySelected {
                        id: ModName::new(m.id.trim())
                            .map_err(|_| "that mod's id is too long".to_owned())?,
                        name: Text::lossy(m.name.trim()),
                        source: Text::lossy(m.source.trim()),
                        modio: match m.modio.trim() {
                            "" => None,
                            id => Some(id.parse().map_err(|_| "that is not a Mod Hub number")?),
                        },
                    })
                })
                .collect::<Result<Vec<_>, String>>()?;
            let params = params
                .iter()
                .map(|p| {
                    Ok(tpf3mp_bridge::LobbySetting {
                        id: ModName::new(p.id.trim())
                            .map_err(|_| "that mod's id is too long".to_owned())?,
                        key: Text::new(p.key.as_str())
                            .map_err(|_| "that mod setting's name is too long".to_owned())?,
                        value: p.value,
                    })
                })
                .collect::<Result<Vec<_>, String>>()?;
            LobbyAction::ChooseRoomMods {
                save: match save.trim() {
                    "" => None,
                    _ => Some(text::<{ tpf3mp_bridge::MAX_SAVE_NAME }>(
                        &save,
                        "save name",
                    )?),
                },
                map: Text::lossy(map.trim()),
                year,
                mods: tpf3mp_proto::BoundedVec::new(mods)
                    .map_err(|_| "more mods than a room runs".to_owned())?,
                params: tpf3mp_proto::BoundedVec::new(params)
                    .map_err(|_| "more mod settings than a room carries".to_owned())?,
            }
        }
        WindowAction::RescanMods => LobbyAction::RescanMods,
    })
}

/// What kind of action this is, for the log: never its fields, since a join
/// carries an invite and a password (D13).
pub fn kind(action: &LobbyAction) -> &'static str {
    match action {
        LobbyAction::Connect { .. } => "connect",
        LobbyAction::Disconnect => "disconnect",
        LobbyAction::Create { .. } => "create",
        LobbyAction::Join { .. } => "join",
        LobbyAction::Ready { .. } => "ready",
        LobbyAction::Start => "start",
        LobbyAction::Kick { .. } => "kick",
        LobbyAction::Chat { .. } => "chat",
        LobbyAction::Leave => "leave",
        LobbyAction::ChooseMod { .. } => "choose_mod",
        LobbyAction::ListRooms { .. } => "list_rooms",
        LobbyAction::SetServer { .. } => "set_server",
        LobbyAction::SetBanner { .. } => "set_banner",
        LobbyAction::ChooseStart { .. } => "choose_start",
        LobbyAction::ChooseRoomMods { .. } => "choose_room_mods",
        LobbyAction::RescanMods => "rescan_mods",
    }
}

/// A Lua string literal for `text`, safe for any bytes.
fn lua_str(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push('"');
    for byte in text.bytes() {
        match byte {
            b'"' => out.push_str("\\\""),
            b'\\' => out.push_str("\\\\"),
            b'\n' => out.push_str("\\n"),
            b'\r' => out.push_str("\\r"),
            b'\t' => out.push_str("\\t"),
            0..=0x1f | 0x7f => out.push_str(&format!("\\{byte}")),
            // Bytes of UTF-8 above ASCII go through as decimal escapes, so
            // the literal is ASCII and the window's Lua gets the same bytes.
            0x80..=0xff => out.push_str(&format!("\\{byte}")),
            other => out.push(other as char),
        }
    }
    out.push('"');
    out
}

fn lua_opt(text: Option<&str>) -> String {
    text.map_or_else(|| "nil".to_owned(), lua_str)
}

impl Default for LobbyState {
    fn default() -> Self {
        Self::new()
    }
}

impl LobbyState {
    /// The empty lobby: not connected, no room, nothing said, no launcher.
    /// Usable in a `static`.
    pub const fn new() -> Self {
        Self {
            connection: Connection::Disconnected,
            server: String::new(),
            server_address: String::new(),
            server_default: String::new(),
            banner: String::new(),
            portraits: Vec::new(),
            name: String::new(),
            error: None,
            notice: None,
            room: None,
            chat: Vec::new(),
            linked: false,
            heard: false,
            rules: Vec::new(),
            saves: Vec::new(),
            start_save: None,
            world: "none",
            bytes: 0,
            total: 0,
            differences: None,
            mods: Vec::new(),
            room_mods: Vec::new(),
            room_mods_more: 0,
            room_mods_missing: 0,
            room_mods_other: 0,
            room_params: Vec::new(),
            log_session: String::new(),
            rooms: None,
        }
    }

    /// What the window shows of the launcher's lobby `view`, if it sent one.
    pub fn of(view: Option<&LobbyView>, linked: bool) -> Self {
        let Some(view) = view else {
            return Self {
                linked,
                ..Self::new()
            };
        };
        Self {
            connection: match view.connection {
                LobbyConnection::Disconnected => Connection::Disconnected,
                LobbyConnection::Connecting => Connection::Connecting,
                LobbyConnection::Connected => Connection::Connected,
            },
            server: view.server.as_str().to_owned(),
            server_address: view.server_address.as_str().to_owned(),
            server_default: view.server_default.as_str().to_owned(),
            banner: view
                .banner
                .as_ref()
                .map(|id| id.as_str().to_owned())
                .unwrap_or_default(),
            portraits: view
                .portraits
                .iter()
                .map(|id| id.as_str().to_owned())
                .filter(|id| tpf3mp_proto::is_portrait(id))
                .collect(),
            name: view.name.as_str().to_owned(),
            error: view.error.as_ref().map(|text| text.as_str().to_owned()),
            notice: view.notice.as_ref().map(|text| text.as_str().to_owned()),
            room: view.room.as_ref().map(|room| Room {
                name: room.name.as_str().to_owned(),
                invite: room
                    .invite
                    .as_ref()
                    .map(|invite| invite.as_str().to_owned())
                    .unwrap_or_default(),
                phase: if room.running { "playing" } else { "lobby" }.to_owned(),
                you_own: room.you_own,
                max_players: u32::from(room.max_players),
                has_password: room.has_password,
                competitive: room.competitive,
                start: room.start.as_ref().map(|start| StartSave {
                    name: start.name.as_str().to_owned(),
                    map: start.map.as_str().to_owned(),
                    year: start.year,
                    arrived: start.arrived,
                }),
                upload: room
                    .upload
                    .as_ref()
                    .map(|upload| (upload.save.as_str().to_owned(), upload.percent.min(100))),
                members: room
                    .members
                    .iter()
                    .map(|member| Member {
                        id: hex(&member.player),
                        name: member.name.as_str().to_owned(),
                        ready: member.ready,
                        owner: member.owner,
                        you: member.you,
                        connected: member.connected,
                        content: match member.same_content {
                            Some(true) => "same",
                            Some(false) => "differs",
                            None => "unknown",
                        }
                        .to_owned(),
                        banner: member
                            .banner
                            .as_ref()
                            .map(|id| id.as_str().to_owned())
                            .unwrap_or_default(),
                        loading: match member.loading {
                            Some(tpf3mp_proto::LoadingStage::Fetching { .. }) => "fetching",
                            Some(tpf3mp_proto::LoadingStage::Loading) => "loading",
                            None => "",
                        }
                        .to_owned(),
                        percent: match member.loading {
                            Some(tpf3mp_proto::LoadingStage::Fetching { percent }) => {
                                percent.min(100)
                            }
                            _ => 0,
                        },
                        missing: member.differs.map_or(0, |d| d.missing),
                        changed: member.differs.map_or(0, |d| d.changed),
                        extra: member.differs.map_or(0, |d| d.extra),
                    })
                    .collect(),
            }),
            chat: view
                .chat
                .iter()
                .map(|line| ChatLine {
                    from: line.from.as_str().to_owned(),
                    text: line.text.as_str().to_owned(),
                    you: line.you,
                })
                .collect(),
            linked,
            heard: true,
            rules: view
                .rules
                .iter()
                .map(|rules| Rules {
                    name: rules.name.as_str().to_owned(),
                    description: rules.description.as_str().to_owned(),
                })
                .collect(),
            saves: view
                .saves
                .iter()
                .map(|save| save.as_str().to_owned())
                .collect(),
            start_save: view
                .start_save
                .as_ref()
                .map(|save| save.as_str().to_owned()),
            world: match view.world {
                LobbyWorld::None => "none",
                LobbyWorld::Fetching { .. } => "fetching",
                LobbyWorld::Loading => "loading",
                LobbyWorld::Playing => "playing",
            },
            bytes: match view.world {
                LobbyWorld::Fetching { bytes, .. } => bytes,
                _ => 0,
            },
            total: match view.world {
                LobbyWorld::Fetching { total, .. } => total,
                _ => 0,
            },
            differences: view
                .differences
                .as_ref()
                .map(|text| text.as_str().to_owned()),
            mods: view
                .mods
                .iter()
                .map(|m| ModEntry {
                    id: m.id.as_str().to_owned(),
                    name: m.name.as_str().to_owned(),
                    class: match m.class {
                        LobbyModClass::Personal => "personal",
                        LobbyModClass::Carried => "carried",
                        LobbyModClass::Shared => "shared",
                    },
                    reason: m.reason.as_str().to_owned(),
                    chosen: m.chosen,
                    choosable: m.choosable,
                })
                .collect(),
            room_mods: view
                .room_mods
                .iter()
                .map(|m| RoomMod {
                    id: m.id.as_str().to_owned(),
                    name: m.name.as_str().to_owned(),
                    version: m.version.as_str().to_owned(),
                    yours: m
                        .yours
                        .as_ref()
                        .map(|v| v.as_str().to_owned())
                        .unwrap_or_default(),
                    have: match m.have {
                        LobbyHave::Yes => "yes",
                        LobbyHave::No => "no",
                        LobbyHave::OtherVersion => "other_version",
                    },
                    source: m.source.as_str().to_owned(),
                    modio: m.modio.map(|id| id.to_string()).unwrap_or_default(),
                })
                .collect(),
            room_mods_more: view.room_mods_more,
            room_mods_missing: view.room_mods_missing,
            room_mods_other: view.room_mods_other,
            room_params: view
                .room_params
                .iter()
                .map(|p| (p.id.as_str().to_owned(), p.key.as_str().to_owned(), p.value))
                .collect(),
            log_session: view.log_session.as_str().to_owned(),
            rooms: view.rooms.clone(),
        }
    }

    /// The state as a Lua table literal: `{ connection = "…", … }`.
    pub fn to_lua(&self) -> String {
        let mut out = String::with_capacity(512);
        out.push_str("{ connection = ");
        out.push_str(lua_str(self.connection.as_str()).as_str());
        out.push_str(", server = ");
        out.push_str(&lua_str(&self.server));
        out.push_str(", server_address = ");
        out.push_str(&lua_str(&self.server_address));
        out.push_str(", server_default = ");
        out.push_str(&lua_str(&self.server_default));
        out.push_str(", banner = ");
        out.push_str(&lua_str(&self.banner));
        out.push_str(", portraits = {");
        for id in &self.portraits {
            out.push(' ');
            out.push_str(&lua_str(id));
            out.push(',');
        }
        out.push_str(" }, name = ");
        out.push_str(&lua_str(&self.name));
        out.push_str(", error = ");
        out.push_str(&lua_opt(self.error.as_deref()));
        out.push_str(", notice = ");
        out.push_str(&lua_opt(self.notice.as_deref()));
        out.push_str(", linked = ");
        out.push_str(if self.linked { "true" } else { "false" });
        out.push_str(", heard = ");
        out.push_str(if self.heard { "true" } else { "false" });
        out.push_str(", start_save = ");
        out.push_str(&lua_opt(self.start_save.as_deref()));
        out.push_str(", differences = ");
        out.push_str(&lua_opt(self.differences.as_deref()));
        out.push_str(&format!(
            ", world = {}, bytes = {}, total = {}",
            lua_str(self.world),
            self.bytes,
            self.total
        ));
        out.push_str(", rules = {");
        for rules in &self.rules {
            out.push_str(&format!(
                " {{ name = {}, description = {} }},",
                lua_str(&rules.name),
                lua_str(&rules.description)
            ));
        }
        match &self.rooms {
            None => out.push_str(" }, rooms = nil"),
            Some(list) => {
                out.push_str(&format!(
                    " }}, rooms = {{ page = {}, more = {}, list = {{",
                    list.page, list.more
                ));
                for room in list.rooms.iter() {
                    out.push_str(&format!(
                        " {{ invite = {}, name = {}, rules = {}, players = {}, max_players = {}, has_password = {}, running = {}, map = {}, year = {}, companies = {}, competitive = {} }},",
                        lua_str(room.invite.as_str()),
                        lua_str(room.name.as_str()),
                        lua_str(room.rules.as_str()),
                        room.players,
                        room.max_players,
                        room.has_password,
                        room.running,
                        lua_str(room.map.as_str()),
                        room.year,
                        room.companies,
                        room.competitive
                    ));
                }
                out.push_str(" } }");
            }
        }
        out.push_str(", saves = {");
        for save in &self.saves {
            out.push(' ');
            out.push_str(&lua_str(save));
            out.push(',');
        }
        out.push_str(" }");
        out.push_str(", mods = {");
        for m in &self.mods {
            out.push_str(&format!(
                " {{ id = {}, name = {}, class = {}, reason = {}, chosen = {}, choosable = {} }},",
                lua_str(&m.id),
                lua_str(&m.name),
                lua_str(m.class),
                lua_str(&m.reason),
                m.chosen,
                m.choosable
            ));
        }
        out.push_str(" }, room_mods = {");
        for m in &self.room_mods {
            out.push_str(&format!(
                " {{ id = {}, name = {}, version = {}, yours = {}, have = {}, source = {}, modio = {} }},",
                lua_str(&m.id),
                lua_str(&m.name),
                lua_str(&m.version),
                lua_str(&m.yours),
                lua_str(m.have),
                lua_str(&m.source),
                lua_str(&m.modio)
            ));
        }
        out.push_str(&format!(
            " }}, room_mods_more = {}, room_mods_missing = {}, room_mods_other = {}, room_params = {{",
            self.room_mods_more, self.room_mods_missing, self.room_mods_other
        ));
        for (id, key, value) in &self.room_params {
            out.push_str(&format!(
                " {{ mod = {}, key = {}, value = {} }},",
                lua_str(id),
                lua_str(key),
                value
            ));
        }
        out.push_str(" }");
        out.push_str(", log_session = ");
        out.push_str(&lua_str(&self.log_session));
        out.push_str(", chat = {");
        for line in &self.chat {
            out.push_str(&format!(
                " {{ from = {}, text = {}, you = {} }},",
                lua_str(&line.from),
                lua_str(&line.text),
                line.you
            ));
        }
        out.push_str(" }");
        match &self.room {
            None => out.push_str(", room = nil"),
            Some(room) => {
                out.push_str(&format!(
                    ", room = {{ name = {}, invite = {}, phase = {}, you_own = {}, max_players = {}, has_password = {}, competitive = {}, members = {{",
                    lua_str(&room.name),
                    lua_str(&room.invite),
                    lua_str(&room.phase),
                    room.you_own,
                    room.max_players,
                    room.has_password,
                    room.competitive
                ));
                for member in &room.members {
                    out.push_str(&format!(
                        " {{ id = {}, name = {}, ready = {}, owner = {}, you = {}, connected = {}, content = {}, banner = {}, loading = {}, percent = {}, missing = {}, changed = {}, extra = {} }},",
                        lua_str(&member.id),
                        lua_str(&member.name),
                        member.ready,
                        member.owner,
                        member.you,
                        member.connected,
                        lua_str(&member.content),
                        lua_str(&member.banner),
                        lua_str(&member.loading),
                        member.percent,
                        member.missing,
                        member.changed,
                        member.extra
                    ));
                }
                out.push_str(" }");
                match &room.start {
                    None => out.push_str(", start = nil"),
                    Some(start) => out.push_str(&format!(
                        ", start = {{ name = {}, map = {}, year = {}, arrived = {} }}",
                        lua_str(&start.name),
                        lua_str(&start.map),
                        start.year,
                        start.arrived
                    )),
                }
                match &room.upload {
                    None => out.push_str(", upload = nil"),
                    Some((save, percent)) => out.push_str(&format!(
                        ", upload = {{ save = {}, percent = {} }}",
                        lua_str(save),
                        percent
                    )),
                }
                out.push_str(" }");
            }
        }
        out.push_str(" }");
        out
    }
}

/// What the window and the step driver share.
struct Menu {
    /// The launcher's lobby as last heard.
    view: Option<LobbyView>,
    /// Whether a step driver, and so the link to the launcher, is there.
    linked: bool,
    /// The player's actions, for the launcher, oldest first.
    actions: VecDeque<LobbyAction>,
    /// Why the last action was not taken, until the next is.
    refused: Option<String>,
}

static MENU: Mutex<Menu> = Mutex::new(Menu {
    view: None,
    linked: false,
    actions: VecDeque::new(),
    refused: None,
});

fn menu() -> MutexGuard<'static, Menu> {
    MENU.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Queues an action of the window's for the launcher.
pub fn queue(action: LobbyAction) -> Result<(), String> {
    let mut menu = menu();
    if menu.actions.len() >= MAX_QUEUED {
        return Err("the launcher has not taken the last actions yet".into());
    }
    menu.actions.push_back(action);
    menu.refused = None;
    Ok(())
}

/// The window's actions for the launcher, oldest first.
pub fn take_actions() -> Vec<LobbyAction> {
    menu().actions.drain(..).collect()
}

/// Hands the window's actions to the launcher through `driver`, and keeps
/// the lobby it sent back, if it sent a new one.
pub fn exchange(driver: &mut dyn StepHandler) {
    let actions = take_actions();
    let heard = driver.lobby(actions);
    let mut menu = menu();
    menu.linked = true;
    if let Some(view) = heard {
        menu.view = Some(view);
    }
}

/// This game has no link to its launcher: the window's actions go nowhere,
/// and it says so.
pub fn unlinked() {
    let mut menu = menu();
    menu.linked = false;
    if !menu.actions.is_empty() {
        menu.actions.clear();
        menu.refused = Some(
            "this game has no link to the TPF3-MP launcher; start it from the launcher".into(),
        );
    }
}

/// What the window shows now.
pub fn state() -> LobbyState {
    let menu = menu();
    let mut state = LobbyState::of(menu.view.as_ref(), menu.linked);
    if let Some(refused) = &menu.refused {
        state.error = Some(refused.clone());
    }
    state
}

#[cfg(test)]
pub(crate) fn reset() {
    *menu() = Menu {
        view: None,
        linked: false,
        actions: VecDeque::new(),
        refused: None,
    };
}

#[cfg(test)]
mod window_tests;

#[cfg(test)]
mod tests {
    use tpf3mp_bridge::{LobbyLine, LobbyMember, LobbyRoom, LobbyRules};
    use tpf3mp_proto::BoundedVec;

    use super::*;

    #[test]
    fn actions_parse_from_the_windows_json() {
        assert_eq!(
            parse_action(r#"{"action":"connect","server":"play.example:4433","name":" Ada "}"#),
            Ok(LobbyAction::Connect {
                name: Text::new("Ada").unwrap()
            }),
            "the window's server is not taken (D12)"
        );
        assert_eq!(
            parse_action(r#"{"action":"create","room":"Alps"}"#),
            Ok(LobbyAction::Create {
                room: Text::new("Alps").unwrap(),
                max_players: 8,
                password: None,
                rules: None,
                start_save: None,
                listing: None,
                competitive: false,
            }),
            "without a save named, the launcher's own, and private"
        );
        assert_eq!(
            parse_action(
                r#"{"action":"create","room":"Alps","max_players":4,"rules":"native","start_save":"mptest"}"#
            ),
            Ok(LobbyAction::Create {
                room: Text::new("Alps").unwrap(),
                max_players: 4,
                password: None,
                rules: Some(Text::new("native").unwrap()),
                start_save: Some(Text::new("mptest").unwrap()),
                listing: None,
                competitive: false,
            })
        );
        assert!(matches!(
            parse_action(r#"{"action":"create","room":"Alps","public":true,"map":"dry","year":1900}"#),
            Ok(LobbyAction::Create { listing: Some(LobbyListing { ref map, year: 1900 }), .. }) if map.as_str() == "dry"
        ));
        assert_eq!(
            parse_action(r#"{"action":"list_rooms","page":1}"#),
            Ok(LobbyAction::ListRooms { page: 1 })
        );
        assert_eq!(
            parse_action(r#"{"action":"set_banner","banner":"dry"}"#),
            Ok(LobbyAction::SetBanner {
                banner: Some(Text::new("dry").unwrap())
            })
        );
        assert_eq!(
            parse_action(r#"{"action":"set_banner","banner":""}"#),
            Ok(LobbyAction::SetBanner { banner: None })
        );
        assert!(parse_action(r#"{"action":"set_banner","banner":"selfie"}"#).is_err());
        assert_eq!(
            parse_action(r#"{"action":"choose_start","save":"mptest","map":"dry","year":1900}"#),
            Ok(LobbyAction::ChooseStart {
                save: Text::new("mptest").unwrap(),
                map: Text::new("dry").unwrap(),
                year: 1900,
            })
        );
        assert!(
            matches!(
                parse_action(r#"{"action":"choose_start","save":""}"#),
                Ok(LobbyAction::ChooseStart { save, year: 0, .. }) if save.as_str().is_empty()
            ),
            "none: the owner's game provides the world"
        );
        let long = "s".repeat(tpf3mp_bridge::MAX_SAVE_NAME + 1);
        assert!(
            parse_action(&format!(r#"{{"action":"choose_start","save":"{long}"}}"#)).is_err(),
            "a save named only in part would be another"
        );
        assert!(
            matches!(
                parse_action(r#"{"action":"create","room":"Alps","start_save":""}"#),
                Ok(LobbyAction::Create { start_save: Some(save), .. }) if save.as_str().is_empty()
            ),
            "an empty save: none"
        );
        let long = "s".repeat(tpf3mp_bridge::MAX_SAVE_NAME + 1);
        assert!(
            parse_action(&format!(
                r#"{{"action":"create","room":"Alps","start_save":"{long}"}}"#
            ))
            .is_err()
        );
        assert_eq!(
            parse_action(r#"{"action":"join","invite":"K7QM2X","password":"pw"}"#),
            Ok(LobbyAction::Join {
                invite: Text::new("K7QM2X").unwrap(),
                password: Some(Text::new("pw").unwrap()),
            })
        );
        let id = "ab".repeat(32);
        assert_eq!(
            parse_action(&format!(r#"{{"action":"kick","player":"{id}"}}"#)),
            Ok(LobbyAction::Kick {
                player: PlayerId(FixedBytes([0xab; 32]))
            })
        );
        assert_eq!(
            parse_action(r#"{"action":"ready","ready":true}"#),
            Ok(LobbyAction::Ready { ready: true })
        );
        assert_eq!(
            parse_action(r#"{"action":"choose_mod","id":"schbrongx_minimap","chosen":true}"#),
            Ok(LobbyAction::ChooseMod {
                id: Text::new("schbrongx_minimap").unwrap(),
                chosen: true,
            })
        );
        assert_eq!(
            parse_action(r#"{"action":"leave"}"#),
            Ok(LobbyAction::Leave)
        );
        assert_eq!(
            parse_action(r#"{"action":"set_server","server":" eu.example:29470 "}"#),
            Ok(LobbyAction::SetServer {
                server: Text::new("eu.example:29470").unwrap()
            })
        );
        assert_eq!(
            parse_action(r#"{"action":"set_server"}"#),
            Ok(LobbyAction::SetServer {
                server: Text::new("").unwrap()
            }),
            "none named: back to the default"
        );
        let long = "s".repeat(129);
        assert!(parse_action(&format!(r#"{{"action":"set_server","server":"{long}"}}"#)).is_err());
        assert!(parse_action(r#"{"action":"fly"}"#).is_err());
        assert!(parse_action(r#"{"action":"kick","player":"7"}"#).is_err());
        let long = "x".repeat(300);
        assert!(parse_action(&format!(r#"{{"action":"chat","text":"{long}"}}"#)).is_err());
    }

    #[test]
    fn the_lua_literal_quotes_every_string() {
        let state = LobbyState {
            name: "A\"b\\c\nd é".into(),
            ..LobbyState::default()
        };
        let lua = state.to_lua();
        assert!(lua.contains(r#"name = "A\"b\\c\nd \195\169""#), "{lua}");
        assert!(lua.is_ascii());
        assert!(lua.starts_with("{ connection = \"disconnected\""));
        assert!(lua.contains("room = nil"));
        assert!(lua.ends_with(" }"));
    }

    fn view() -> LobbyView {
        LobbyView {
            connection: LobbyConnection::Connected,
            server: Text::new("EU").unwrap(),
            server_address: Text::new("eu.example.org:29470").unwrap(),
            server_default: Text::new("relay.example.org:29470").unwrap(),
            banner: None,
            portraits: BoundedVec::new(vec![
                Text::new("andrew").unwrap(),
                Text::new("selfie").unwrap(),
            ])
            .unwrap(),
            name: Text::new("Ann").unwrap(),
            error: None,
            notice: Some(Text::new("created the room").unwrap()),
            room: Some(LobbyRoom {
                name: Text::new("Alps").unwrap(),
                rules: Text::new("native").unwrap(),
                invite: Some(Text::new("K7QM2X").unwrap()),
                running: false,
                you_own: true,
                max_players: 4,
                has_password: false,
                members: BoundedVec::new(vec![LobbyMember {
                    player: PlayerId(FixedBytes([1; 32])),
                    name: Text::new("Ann").unwrap(),
                    ready: true,
                    connected: true,
                    owner: true,
                    you: true,
                    same_content: None,
                    differs: None,
                    banner: None,
                    loading: None,
                }])
                .unwrap(),
                competitive: false,
                start: Some(tpf3mp_bridge::LobbyStart {
                    name: Text::new("Güterzug").unwrap(),
                    map: Text::new("dry").unwrap(),
                    year: 1925,
                    arrived: false,
                }),
                upload: Some(tpf3mp_bridge::LobbyUpload {
                    save: Text::new("Güterzug").unwrap(),
                    percent: 35,
                }),
            }),
            chat: BoundedVec::new(vec![LobbyLine {
                from: Text::new("Bo").unwrap(),
                text: Text::new("hi").unwrap(),
                you: false,
            }])
            .unwrap(),
            rules: BoundedVec::new(vec![LobbyRules {
                name: Text::new("native").unwrap(),
                description: Text::new("The game's own economy").unwrap(),
            }])
            .unwrap(),
            saves: BoundedVec::new(vec![
                Text::new("mptest").unwrap(),
                Text::new("Güterzug").unwrap(),
            ])
            .unwrap(),
            start_save: Some(Text::new("mptest").unwrap()),
            world: LobbyWorld::Fetching {
                bytes: 5_000_000,
                total: 20_000_000,
            },
            differences: None,
            mods: tpf3mp_proto::BoundedVec::new(vec![tpf3mp_bridge::LobbyMod {
                id: Text::new("schbrongx_minimap").unwrap(),
                name: Text::new("Minimap").unwrap(),
                class: LobbyModClass::Personal,
                reason: Text::new("only what this player sees").unwrap(),
                chosen: true,
                choosable: true,
            }])
            .unwrap(),
            room_mods: tpf3mp_proto::BoundedVec::new(vec![tpf3mp_bridge::LobbyRoomMod {
                id: Text::new("vehicles_pack").unwrap(),
                version: Text::new("3").unwrap(),
                have: LobbyHave::OtherVersion,
                name: Text::lossy("Pack"),
                yours: None,
                source: Text::lossy("StagingArea"),
                modio: None,
            }])
            .unwrap(),
            room_mods_more: 2,
            room_mods_missing: 0,
            room_mods_other: 0,
            room_params: tpf3mp_proto::BoundedVec::empty(),
            log_session: Text::new("AB2CD3").unwrap(),
            rooms: Some(tpf3mp_bridge::LobbyRoomList {
                page: 0,
                more: false,
                rooms: BoundedVec::new(vec![tpf3mp_bridge::LobbyPublicRoom {
                    invite: Text::new("K7QM2X").unwrap(),
                    name: Text::new("Open \"alps\"").unwrap(),
                    rules: Text::new("native").unwrap(),
                    players: 2,
                    max_players: 4,
                    has_password: true,
                    running: false,
                    map: Text::new("temperate").unwrap(),
                    year: 1850,
                    companies: 1,
                    competitive: false,
                }])
                .unwrap(),
            }),
        }
    }

    #[test]
    fn the_window_shows_the_launchers_lobby_as_it_sent_it() {
        let state = LobbyState::of(Some(&view()), true);
        assert_eq!(state.connection, Connection::Connected);
        assert!(state.linked && state.heard);
        let room = state.room.as_ref().unwrap();
        assert_eq!(room.phase, "lobby");
        assert_eq!(room.invite, "K7QM2X");
        assert_eq!(room.members[0].id, "01".repeat(32));
        assert_eq!(room.members[0].content, "unknown");
        let lua = state.to_lua();
        assert!(lua.contains(r#"notice = "created the room""#), "{lua}");
        assert!(lua.contains(r#"{ from = "Bo", text = "hi", you = false }"#));
        // Nothing heard yet: not connected, and says whether it is linked.
        let quiet = LobbyState::of(None, true);
        assert!(quiet.linked && !quiet.heard && quiet.room.is_none());
    }

    /// The window evaluates the literal with a real Lua, as the menu's
    /// `load("return " .. reply)` does, and reads what it shows from it.
    #[test]
    fn the_windows_lua_reads_the_literal() {
        let mut shown = view();
        shown.name = Text::new("Ann \"the\" Bü\\").unwrap();
        let literal = LobbyState::of(Some(&shown), true).to_lua();
        let lua = mlua::Lua::new();
        let state: mlua::Table = lua.load(format!("return {literal}")).eval().unwrap();
        assert_eq!(state.get::<String>("connection").unwrap(), "connected");
        // The portraits this game has, an id that is none left out.
        assert_eq!(
            state.get::<Vec<String>>("portraits").unwrap(),
            ["andrew"],
            "{literal}"
        );
        assert_eq!(
            state.get::<String>("server_address").unwrap(),
            "eu.example.org:29470"
        );
        assert_eq!(
            state.get::<String>("server_default").unwrap(),
            "relay.example.org:29470"
        );
        assert_eq!(state.get::<String>("name").unwrap(), "Ann \"the\" Bü\\");
        assert!(state.get::<bool>("linked").unwrap());
        let room: mlua::Table = state.get("room").unwrap();
        assert_eq!(room.get::<String>("invite").unwrap(), "K7QM2X");
        let members: mlua::Table = room.get("members").unwrap();
        let first: mlua::Table = members.get(1).unwrap();
        assert!(first.get::<bool>("you").unwrap());
        // The save the room starts from, and the owner's upload of it.
        let start: mlua::Table = room.get("start").unwrap();
        assert_eq!(start.get::<String>("name").unwrap(), "Güterzug");
        assert_eq!(start.get::<String>("map").unwrap(), "dry");
        assert_eq!(start.get::<u16>("year").unwrap(), 1925);
        assert!(!start.get::<bool>("arrived").unwrap());
        let upload: mlua::Table = room.get("upload").unwrap();
        assert_eq!(upload.get::<String>("save").unwrap(), "Güterzug");
        assert_eq!(upload.get::<u8>("percent").unwrap(), 35);
        let chat: mlua::Table = state.get("chat").unwrap();
        let line: mlua::Table = chat.get(1).unwrap();
        assert_eq!(line.get::<String>("text").unwrap(), "hi");
        let saves: mlua::Table = state.get("saves").unwrap();
        assert_eq!(saves.get::<String>(2).unwrap(), "Güterzug");
        assert_eq!(state.get::<String>("start_save").unwrap(), "mptest");
        let rules: mlua::Table = state.get("rules").unwrap();
        let first: mlua::Table = rules.get(1).unwrap();
        assert_eq!(first.get::<String>("name").unwrap(), "native");
        assert_eq!(state.get::<String>("world").unwrap(), "fetching");
        assert_eq!(state.get::<u64>("bytes").unwrap(), 5_000_000);
        assert_eq!(state.get::<u64>("total").unwrap(), 20_000_000);
        assert!(
            state
                .get::<Option<String>>("differences")
                .unwrap()
                .is_none()
        );
        let mods: mlua::Table = state.get("mods").unwrap();
        let minimap: mlua::Table = mods.get(1).unwrap();
        assert_eq!(minimap.get::<String>("id").unwrap(), "schbrongx_minimap");
        assert_eq!(minimap.get::<String>("class").unwrap(), "personal");
        assert!(
            minimap.get::<bool>("chosen").unwrap() && minimap.get::<bool>("choosable").unwrap()
        );
        let room_mods: mlua::Table = state.get("room_mods").unwrap();
        let pack: mlua::Table = room_mods.get(1).unwrap();
        assert_eq!(pack.get::<String>("have").unwrap(), "other_version");
        assert_eq!(state.get::<u32>("room_mods_more").unwrap(), 2);
        assert_eq!(state.get::<String>("log_session").unwrap(), "AB2CD3");
    }

    /// A step driver that records what the window handed it and answers
    /// with a lobby.
    #[derive(Default)]
    struct Launcher {
        heard: Vec<LobbyAction>,
        answer: Option<LobbyView>,
    }

    impl StepHandler for Launcher {
        fn on_step(
            &mut self,
            _commands: Vec<crate::step::Handed>,
            _run: &mut crate::step::RunStep<'_>,
        ) -> crate::step::Outcome {
            unreachable!()
        }
        fn take_log(&mut self) -> Vec<String> {
            Vec::new()
        }
        fn take_refused(&mut self) -> Vec<(u64, String)> {
            Vec::new()
        }
        fn in_room(&self) -> bool {
            false
        }
        fn chosen_speed(&mut self, _speedup: u64) {}
        fn say(&mut self, _text: tpf3mp_proto::ChatText) {}
        fn preview(&mut self, _preview: Option<tpf3mp_proto::Payload>) {}
        fn on_menu(&mut self) {}
        fn lobby(&mut self, actions: Vec<LobbyAction>) -> Option<LobbyView> {
            self.heard.extend(actions);
            self.answer.take()
        }
    }

    #[test]
    fn the_windows_actions_reach_the_launcher_and_its_lobby_comes_back() {
        let _serial = crate::lua::tests::SERIAL
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        reset();
        assert!(!state().linked);
        queue(LobbyAction::Start).unwrap();
        let mut launcher = Launcher {
            answer: Some(view()),
            ..Launcher::default()
        };
        exchange(&mut launcher);
        assert_eq!(launcher.heard, vec![LobbyAction::Start]);
        let shown = state();
        assert!(shown.linked && shown.heard);
        assert_eq!(shown.name, "Ann");
        // Nothing new: the window keeps what it had.
        exchange(&mut launcher);
        assert_eq!(state().name, "Ann");
        reset();
    }

    /// Copy is the hook's own: never queued for the launcher.
    #[test]
    fn copy_is_done_by_the_hook_itself() {
        assert_eq!(
            local_action(r#"{"action":"copy","text":"  "}"#),
            Some(Err("nothing to copy".to_owned()))
        );
        assert_eq!(local_action(r#"{"action":"leave"}"#), None);
        assert!(parse_action(r#"{"action":"copy","text":"K7QM2X"}"#).is_err());
    }

    #[test]
    fn without_a_launcher_the_window_says_so_and_takes_no_action() {
        let _serial = crate::lua::tests::SERIAL
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        reset();
        queue(LobbyAction::Leave).unwrap();
        unlinked();
        assert!(take_actions().is_empty(), "dropped, never sent later");
        let shown = state();
        assert!(!shown.linked);
        assert!(
            shown
                .error
                .as_deref()
                .is_some_and(|e| e.contains("launcher")),
            "{shown:?}"
        );
        for _ in 0..MAX_QUEUED {
            queue(LobbyAction::Start).unwrap();
        }
        assert!(queue(LobbyAction::Start).is_err(), "bounded");
        reset();
    }
}
