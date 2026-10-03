//! The launcher's lobby in the game (D17): what the main menu's Multiplayer
//! window shows ([`view`]), the launcher actions its buttons stand for
//! ([`action`]), and the game's link while no room session holds it
//! ([`IdleLink`]).
//!
//! The launcher owns one link to its game for as long as it runs. While a
//! room session runs, its [`Bridge`](crate::bridge::Bridge) holds the link
//! and passes the lobby both ways (`BridgeOptions::lobby`); otherwise the
//! launcher serves it here: it answers the hook's hello, sends the lobby
//! whenever it changes and hands the window's actions back. A session takes
//! the link over already greeted, and gives it back when it ends.

use std::time::{Duration, Instant};
use tpf3mp_bridge::{
    BRIDGE_VERSION, LobbyAction, LobbyConnection, LobbyHave, LobbyLine, LobbyMember, LobbyMod,
    LobbyModClass, LobbyPublicRoom, LobbyRoom, LobbyRoomList, LobbyRoomMod, LobbyRules, LobbyStart,
    LobbyUpload, LobbyView, LobbyWorld, MAX_LOBBY_CHAT, MAX_LOBBY_MODS, MAX_LOBBY_ROOM_MODS,
    MAX_LOBBY_RULES, MAX_LOBBY_SAVES, MAX_SAVE_NAME, ModName, SaveName, ToAgent, ToHook,
    check_version, decode, encode,
};

use tpf3mp_proto::{BoundedVec, Text};
use tracing::{debug, info, warn};

use super::api::{self, Action, Connection, MemberContent, ModClass, ModHave, Phase, State, World};
use crate::bridge::{BridgeFault, HookLink};

/// The lobby the menu's window shows, from what the launcher shows.
pub(crate) fn view(state: &State) -> LobbyView {
    let chat: Vec<LobbyLine> = state
        .chat
        .iter()
        .rev()
        .take(MAX_LOBBY_CHAT)
        .rev()
        .map(|line| LobbyLine {
            from: Text::lossy(&line.from),
            text: Text::lossy(&line.text),
            you: line.you,
        })
        .collect();
    let room = state.room.as_ref().map(|room| LobbyRoom {
        name: Text::lossy(&room.name),
        rules: Text::lossy(&room.rules),
        invite: room.invite.as_deref().map(Text::lossy),
        running: room.phase == Phase::Running,
        you_own: room.you_own,
        max_players: room.max_players,
        has_password: room.has_password,
        members: BoundedVec::new(
            room.members
                .iter()
                .filter_map(|member| {
                    Some(LobbyMember {
                        player: api::parse_player(&member.id)?,
                        name: Text::lossy(&member.name),
                        ready: member.ready,
                        connected: member.connected,
                        owner: member.owner,
                        you: member.you,
                        same_content: match member.content {
                            MemberContent::Same => Some(true),
                            MemberContent::Differs => Some(false),
                            MemberContent::Unknown => None,
                        },
                        banner: member.banner.as_deref().and_then(banner),
                        loading: member.loading,
                    })
                })
                .take(usize::from(tpf3mp_proto::MAX_ROOM_MEMBERS))
                .collect(),
        )
        .unwrap_or_default(),
        competitive: room.competitive,
        start: state.start.as_ref().map(|start| LobbyStart {
            name: Text::lossy(&start.name),
            map: Text::lossy(&start.map),
            year: start.year,
            arrived: start.arrived,
        }),
        upload: state.start_upload.as_ref().map(|upload| LobbyUpload {
            save: Text::lossy(&upload.save),
            percent: upload.percent.min(100),
        }),
    });
    LobbyView {
        banner: state.banner.as_deref().and_then(banner),
        portraits: BoundedVec::new(
            crate::portraits::available()
                .into_iter()
                .filter_map(|id| Text::new(id).ok())
                .take(tpf3mp_bridge::MAX_LOBBY_PORTRAITS)
                .collect(),
        )
        .unwrap_or_default(),
        connection: match state.connection {
            Connection::Disconnected => LobbyConnection::Disconnected,
            Connection::Connecting => LobbyConnection::Connecting,
            Connection::Connected => LobbyConnection::Connected,
        },
        server: Text::lossy(
            state
                .server_name
                .as_deref()
                .or(state.server.as_deref())
                .unwrap_or_default(),
        ),
        server_address: Text::lossy(state.server.as_deref().unwrap_or_default()),
        server_default: Text::lossy(state.server_default.as_deref().unwrap_or_default()),
        name: Text::lossy(&state.name),
        error: state.error.as_deref().map(Text::lossy),
        // The newest notice meant for the game: the one that says the
        // launcher started it is for the launcher's window, and out of
        // place in the game it started.
        notice: state
            .notices
            .iter()
            .rev()
            .find(|notice| notice.as_str() != super::GAME_STARTED)
            .map(|notice| Text::lossy(notice)),
        room,
        chat: BoundedVec::new(chat).unwrap_or_default(),
        rules: BoundedVec::new(
            state
                .rules
                .iter()
                .take(MAX_LOBBY_RULES)
                .map(|rules| LobbyRules {
                    name: Text::lossy(&rules.name),
                    description: Text::lossy(&rules.description),
                })
                .collect(),
        )
        .unwrap_or_default(),
        saves: BoundedVec::new(
            state
                .saves
                .iter()
                .filter_map(|name| save(name))
                .take(MAX_LOBBY_SAVES)
                .collect(),
        )
        .unwrap_or_default(),
        start_save: state.start_save.as_deref().and_then(save),
        world: match state.game.world {
            World::None => LobbyWorld::None,
            World::Fetching => LobbyWorld::Fetching {
                bytes: state.game.bytes,
                total: state.game.total,
            },
            World::Loading => LobbyWorld::Loading,
            World::Playing => LobbyWorld::Playing,
        },
        differences: state
            .content_diff
            .as_ref()
            .map(|diff| Text::lossy(&diff.summary)),
        // A mod whose id is too long to name whole is left out: a shortened
        // one would name no mod.
        mods: BoundedVec::new(
            state
                .mods
                .iter()
                .filter_map(|m| {
                    Some(LobbyMod {
                        id: ModName::new(&m.id).ok()?,
                        name: Text::lossy(&m.name),
                        class: match m.class {
                            ModClass::Personal => LobbyModClass::Personal,
                            ModClass::Carried => LobbyModClass::Carried,
                            ModClass::Shared => LobbyModClass::Shared,
                        },
                        reason: Text::lossy(&m.reason),
                        chosen: m.chosen,
                        choosable: m.choosable,
                    })
                })
                .take(MAX_LOBBY_MODS)
                .collect(),
        )
        .unwrap_or_default(),
        room_mods: BoundedVec::new(
            state
                .room_mods
                .iter()
                .take(MAX_LOBBY_ROOM_MODS)
                .map(|m| LobbyRoomMod {
                    id: Text::lossy(&m.id),
                    version: Text::lossy(&m.version),
                    have: match m.have {
                        ModHave::Yes => LobbyHave::Yes,
                        ModHave::No => LobbyHave::No,
                        ModHave::OtherVersion => LobbyHave::OtherVersion,
                    },
                })
                .collect(),
        )
        .unwrap_or_default(),
        room_mods_more: u32::try_from(state.room_mods.len().saturating_sub(MAX_LOBBY_ROOM_MODS))
            .unwrap_or(u32::MAX),
        log_session: Text::lossy(state.log_session.as_deref().unwrap_or_default()),
        rooms: state.rooms.as_ref().map(|list| LobbyRoomList {
            page: list.page,
            more: list.more,
            rooms: BoundedVec::new(
                list.rooms
                    .iter()
                    .take(tpf3mp_proto::ROOMS_PER_PAGE)
                    .map(|room| LobbyPublicRoom {
                        invite: Text::lossy(&room.invite),
                        name: Text::lossy(&room.name),
                        rules: Text::lossy(&room.rules),
                        players: room.players,
                        max_players: room.max_players,
                        has_password: room.has_password,
                        running: room.running,
                        map: Text::lossy(&room.map),
                        year: room.year,
                        companies: room.companies,
                        competitive: room.competitive,
                    })
                    .collect(),
            )
            .unwrap_or_default(),
        }),
    }
}

/// A save's name as the window lists it; a name too long to name whole is
/// left out, since a shortened one would name no save.
fn save(name: &str) -> Option<SaveName> {
    (name.len() <= MAX_SAVE_NAME)
        .then(|| Text::new(name).ok())
        .flatten()
}

/// The launcher action a button of the menu's window stands for. Connect
/// goes to the server the launcher plays on (D12): the window names none.
/// The server setting changes that server, as in the launcher's window.
pub(crate) fn action(action: LobbyAction, state: &State) -> Action {
    match action {
        LobbyAction::Connect { name } => Action::Connect {
            server: state.server.clone().unwrap_or_default(),
            name: name.as_str().to_owned(),
        },
        LobbyAction::Disconnect => Action::Disconnect,
        LobbyAction::Create {
            room,
            max_players,
            password,
            rules,
            start_save,
            listing,
            competitive,
        } => Action::Create {
            room: room.as_str().to_owned(),
            max_players,
            password: password.map(|password| password.as_str().to_owned()),
            rules: rules.map(|rules| rules.as_str().to_owned()),
            start_save: start_save.map(|save| save.as_str().to_owned()),
            listing: listing.map(|listing| api::Listing {
                map: listing.map.as_str().to_owned(),
                year: listing.year,
            }),
            competitive,
        },
        LobbyAction::ListRooms { page } => Action::ListRooms { page },
        LobbyAction::Join { invite, password } => Action::Join {
            invite: invite.as_str().to_owned(),
            password: password.map(|password| password.as_str().to_owned()),
        },
        LobbyAction::Ready { ready } => Action::Ready { ready },
        LobbyAction::Start => Action::Start,
        LobbyAction::Kick { player } => Action::Kick {
            player: api::player_hex(&player),
        },
        LobbyAction::Chat { text } => Action::Chat {
            text: text.as_str().to_owned(),
        },
        LobbyAction::Leave => Action::Leave,
        LobbyAction::ChooseMod { id, chosen } => Action::ChooseMod {
            id: id.as_str().to_owned(),
            chosen,
        },
        LobbyAction::SetServer { server } => Action::SetServer {
            server: server.as_str().to_owned(),
        },
        LobbyAction::SetBanner { banner } => Action::SetBanner {
            banner: banner.map(|id| id.as_str().to_owned()),
        },
        LobbyAction::ChooseStart { save, map, year } => Action::ChooseStart {
            save: save.as_str().to_owned(),
            map: map.as_str().to_owned(),
            year,
        },
    }
}

/// A banner id as the window may show it: one of the known ones, and a
/// portrait only where this game has it (`crate::portraits::shown`): the
/// window shows the player's default banner instead.
fn banner(id: &str) -> Option<tpf3mp_proto::BannerId> {
    (tpf3mp_proto::is_banner(id) && crate::portraits::shown(id))
        .then(|| Text::new(id).ok())
        .flatten()
}

/// The game's link while no room session holds it.
pub(crate) struct IdleLink<L> {
    link: L,
    /// The game's build, once its hook said hello and was answered.
    build: Option<String>,
    buf: Vec<u8>,
    /// The lobby the hook was last sent.
    told: Option<LobbyView>,
    /// The bridge version of a hook that said hello in another, not
    /// greeted: a game started by another TPF3-MP.
    other_bridge: Option<u32>,
    /// The hook's heartbeat as last seen to move, and when.
    hook_beat: Option<(u64, Instant)>,
}

impl<L: HookLink> IdleLink<L> {
    /// A new link, which no hook has greeted yet.
    pub(crate) fn new(link: L) -> Self {
        Self::resumed(link, None)
    }

    /// A link a room session gave back, greeted if `build` says so. The
    /// lobby is sent again at once.
    pub(crate) fn resumed(link: L, build: Option<String>) -> Self {
        Self {
            link,
            build,
            buf: Vec::new(),
            told: None,
            other_bridge: None,
            hook_beat: None,
        }
    }

    /// The bridge version of a hook this launcher cannot greet, which spoke
    /// last: a game that another TPF3-MP's launcher started.
    pub(crate) fn other_bridge(&self) -> Option<u32> {
        self.other_bridge
    }

    /// How long the hook's heartbeat has stood still, as of `now`: for a
    /// game this launcher did not start, and so cannot see close.
    pub(crate) fn hook_quiet(&self, now: Instant) -> Duration {
        self.hook_beat
            .map_or(Duration::ZERO, |(_, at)| now.saturating_duration_since(at))
    }

    /// The game's build, if its hook said hello.
    pub(crate) fn build(&self) -> Option<&str> {
        self.build.as_deref()
    }

    /// A link a room session gave back when it ended. A game that no
    /// longer runs left no hook on it, so its build does not come back:
    /// otherwise the launcher would go on showing a game attached, and its
    /// window would never offer to start the game again.
    pub(crate) fn given_back(link: L, build: Option<String>, game_runs: bool) -> Self {
        Self::resumed(link, build.filter(|_| game_runs))
    }

    /// The game that said hello on this link closed: the link waits for
    /// the next one.
    pub(crate) fn forget_game(self) -> Self {
        Self::new(self.link)
    }

    /// The link itself.
    pub(crate) fn link(&self) -> &L {
        &self.link
    }

    /// The link and the game's build, for a room session to take over.
    pub(crate) fn into_parts(self) -> (L, Option<String>) {
        (self.link, self.build)
    }

    /// One round: beats for the hook, reads it, answers its hello and sends
    /// `lobby` if the hook has not seen it yet. Returns the actions the
    /// player took in the menu's window.
    pub(crate) fn pump(&mut self, lobby: &LobbyView) -> Result<Vec<LobbyAction>, BridgeFault> {
        self.link.heartbeat();
        let beat = self.link.peer_heartbeat();
        if self.hook_beat.is_none_or(|(last, _)| last != beat) {
            self.hook_beat = Some((beat, Instant::now()));
        }
        let mut actions = Vec::new();
        while self.link.recv(&mut self.buf)? {
            match decode::<ToAgent>(&self.buf)? {
                ToAgent::Hello { version, build } => {
                    if let Err(error) = check_version(version) {
                        warn!(%error, "the game's hook speaks another bridge version");
                        self.build = None;
                        self.other_bridge = Some(version);
                        continue;
                    }
                    info!(%build, "the game's hook attached");
                    self.other_bridge = None;
                    // A game started again says hello again: answer it anew.
                    self.link.send(&encode(&ToHook::Hello {
                        version: BRIDGE_VERSION,
                    })?)?;
                    self.build = Some(build.as_str().to_owned());
                    self.told = None;
                }
                ToAgent::Lobby(action) if self.build.is_some() => actions.push(action),
                ToAgent::Log { message } => info!(hook = %message),
                other => debug!(?other, "the game said something outside a room session"),
            }
        }
        if self.build.is_some() && self.told.as_ref() != Some(lobby) {
            let bytes = encode(&ToHook::Lobby(Box::new(lobby.clone())))?;
            if self.link.send(&bytes)? {
                self.told = Some(lobby.clone());
            }
        }
        Ok(actions)
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use std::{
        collections::VecDeque,
        sync::{Arc, Mutex},
    };

    use tpf3mp_proto::{FixedBytes, PlayerId};

    use super::*;
    use crate::launcher::{ChatLine, Differences, Game, Member, Room, RulesChoice};

    /// Both ends of a link in memory: what each side sent the other.
    #[derive(Clone, Default)]
    pub(crate) struct FakeLink {
        pub(crate) to_hook: Arc<Mutex<VecDeque<Vec<u8>>>>,
        pub(crate) to_agent: Arc<Mutex<VecDeque<Vec<u8>>>>,
        /// The hook's heartbeat.
        pub(crate) beat: Arc<Mutex<u64>>,
    }

    impl FakeLink {
        pub(crate) fn hook_says(&self, message: &ToAgent) {
            self.to_agent
                .lock()
                .unwrap()
                .push_back(encode(message).unwrap());
        }

        pub(crate) fn hook_hears(&self) -> Vec<ToHook> {
            self.to_hook
                .lock()
                .unwrap()
                .drain(..)
                .map(|bytes| decode(&bytes).unwrap())
                .collect()
        }
    }

    impl HookLink for FakeLink {
        fn send(&mut self, message: &[u8]) -> Result<bool, BridgeFault> {
            self.to_hook.lock().unwrap().push_back(message.to_vec());
            Ok(true)
        }
        fn recv(&mut self, buf: &mut Vec<u8>) -> Result<bool, BridgeFault> {
            match self.to_agent.lock().unwrap().pop_front() {
                Some(bytes) => {
                    *buf = bytes;
                    Ok(true)
                }
                None => Ok(false),
            }
        }
        fn heartbeat(&mut self) {}
        fn peer_heartbeat(&self) -> u64 {
            *self.beat.lock().unwrap()
        }
    }

    fn hello() -> ToAgent {
        ToAgent::Hello {
            version: BRIDGE_VERSION,
            build: Text::lossy("40408"),
        }
    }

    fn lobby(name: &str) -> LobbyView {
        LobbyView {
            name: Text::lossy(name),
            ..LobbyView::default()
        }
    }

    #[test]
    fn a_game_at_its_menu_is_greeted_sent_the_lobby_and_heard() {
        let fake = FakeLink::default();
        let mut idle = IdleLink::new(fake.clone());
        // Before its hello, nothing is sent and nothing taken.
        fake.hook_says(&ToAgent::Lobby(LobbyAction::Start));
        assert!(idle.pump(&lobby("Ann")).unwrap().is_empty());
        assert!(fake.hook_hears().is_empty());

        fake.hook_says(&hello());
        fake.hook_says(&ToAgent::Lobby(LobbyAction::Ready { ready: true }));
        let actions = idle.pump(&lobby("Ann")).unwrap();
        assert_eq!(actions, vec![LobbyAction::Ready { ready: true }]);
        assert_eq!(idle.build(), Some("40408"));
        assert_eq!(
            fake.hook_hears(),
            vec![
                ToHook::Hello {
                    version: BRIDGE_VERSION
                },
                ToHook::Lobby(Box::new(lobby("Ann")))
            ]
        );
        // Unchanged, it is not sent again; changed, it is.
        idle.pump(&lobby("Ann")).unwrap();
        assert!(fake.hook_hears().is_empty());
        idle.pump(&lobby("Ann B")).unwrap();
        assert_eq!(
            fake.hook_hears(),
            vec![ToHook::Lobby(Box::new(lobby("Ann B")))]
        );

        // Given back by a session, the lobby goes out again at once.
        let (link, build) = idle.into_parts();
        let mut idle = IdleLink::resumed(link, build);
        idle.pump(&lobby("Ann B")).unwrap();
        assert_eq!(
            fake.hook_hears(),
            vec![ToHook::Lobby(Box::new(lobby("Ann B")))]
        );
    }

    #[test]
    fn a_hook_of_another_version_is_not_greeted() {
        let fake = FakeLink::default();
        let mut idle = IdleLink::new(fake.clone());
        fake.hook_says(&ToAgent::Hello {
            version: BRIDGE_VERSION + 1,
            build: Text::lossy("40408"),
        });
        fake.hook_says(&ToAgent::Lobby(LobbyAction::Start));
        assert!(idle.pump(&lobby("Ann")).unwrap().is_empty());
        assert!(fake.hook_hears().is_empty());
        assert_eq!(idle.build(), None);
        assert_eq!(
            idle.other_bridge(),
            Some(BRIDGE_VERSION + 1),
            "said, so the player hears why"
        );
        fake.hook_says(&hello());
        idle.pump(&lobby("Ann")).unwrap();
        assert_eq!(idle.other_bridge(), None, "a game of this version came");
    }

    #[test]
    fn a_hooks_heartbeat_that_stands_still_shows() {
        let fake = FakeLink::default();
        let mut idle = IdleLink::new(fake.clone());
        let start = Instant::now();
        assert_eq!(idle.hook_quiet(start), Duration::ZERO, "nothing seen yet");
        idle.pump(&lobby("Ann")).unwrap();
        let later = Instant::now() + Duration::from_secs(30);
        assert!(idle.hook_quiet(later) >= Duration::from_secs(29));
        *fake.beat.lock().unwrap() += 1;
        idle.pump(&lobby("Ann")).unwrap();
        assert!(
            idle.hook_quiet(Instant::now()) < Duration::from_secs(1),
            "it moved"
        );
    }

    fn state() -> State {
        let ann = PlayerId(FixedBytes([1; 32]));
        State {
            name: "Ann".into(),
            player: Some(ann.to_string()),
            server: Some("tpf3mp.example.org:29470".into()),
            server_default: Some("tpf3mp.example.org:29470".into()),
            server_name: Some("EU".into()),
            connection: Connection::Connected,
            error: Some("that room is full".into()),
            notices: vec!["old".into(), "new".into()],
            room: Some(Room {
                name: "Alps".into(),
                rules: "native".into(),
                phase: Phase::Lobby,
                invite: Some("K7QM2X".into()),
                you_own: true,
                max_players: 4,
                has_password: false,
                members: vec![
                    Member {
                        id: api::player_hex(&ann),
                        name: "Ann".into(),
                        platform: "Windows x86-64".into(),
                        ready: true,
                        connected: true,
                        owner: true,
                        you: true,
                        content: MemberContent::Same,
                        banner: None,
                        loading: None,
                    },
                    Member {
                        id: "not a player".into(),
                        name: "?".into(),
                        platform: String::new(),
                        ready: false,
                        connected: false,
                        owner: false,
                        you: false,
                        content: MemberContent::Unknown,
                        banner: None,
                        loading: None,
                    },
                ],
                competitive: false,
            }),
            chat: (0..50)
                .map(|n| ChatLine {
                    from: "Bo".into(),
                    text: format!("line {n}"),
                    you: false,
                })
                .collect(),
            rules: vec![RulesChoice {
                name: "native".into(),
                description: "The game's own economy".into(),
            }],
            saves: vec![
                "mptest".into(),
                "x".repeat(MAX_SAVE_NAME + 1),
                "older".into(),
            ],
            start_save: Some("mptest".into()),
            start: Some(api::RoomStart {
                name: "mptest".into(),
                map: "dry".into(),
                year: 1900,
                arrived: false,
            }),
            start_upload: Some(api::StartProgress {
                save: "mptest".into(),
                percent: 40,
            }),
            game: Game {
                world: World::Fetching,
                bytes: 10,
                total: 40,
                ..Game::default()
            },
            content_diff: Some(Differences {
                summary: "you lack stations 3".into(),
                ..Differences::default()
            }),
            mods: vec![
                api::ModRow {
                    id: "schbrongx_minimap".into(),
                    name: "Minimap".into(),
                    class: ModClass::Personal,
                    reason: "only what this player sees".into(),
                    chosen: true,
                    choosable: true,
                },
                api::ModRow {
                    id: "m".repeat(200),
                    name: "A mod whose id is too long".into(),
                    class: ModClass::Shared,
                    reason: String::new(),
                    chosen: false,
                    choosable: false,
                },
            ],
            room_mods: (0..40)
                .map(|n| api::RoomModRow {
                    id: format!("pack{n}"),
                    version: "1".into(),
                    have: if n == 0 { ModHave::No } else { ModHave::Yes },
                })
                .collect(),
            ..State::default()
        }
    }

    #[test]
    fn the_menus_window_sees_the_players_mods_and_the_rooms() {
        let view = view(&state());
        assert_eq!(
            view.mods.len(),
            1,
            "an id too long to name whole is left out"
        );
        let minimap = &view.mods[0];
        assert_eq!(minimap.id.as_str(), "schbrongx_minimap");
        assert_eq!(minimap.class, LobbyModClass::Personal);
        assert!(minimap.chosen && minimap.choosable);
        assert_eq!(view.room_mods.len(), MAX_LOBBY_ROOM_MODS);
        assert_eq!(view.room_mods[0].have, LobbyHave::No);
        assert_eq!(view.room_mods_more, 8);
        assert_eq!(
            action(
                LobbyAction::ChooseMod {
                    id: Text::new("schbrongx_minimap").unwrap(),
                    chosen: false
                },
                &state()
            ),
            Action::ChooseMod {
                id: "schbrongx_minimap".into(),
                chosen: false
            }
        );
    }

    /// A portrait this game lacks reaches the window as no pick at all, so
    /// the window shows the player's default banner; a banner always
    /// reaches it. (No test makes portraits available.)
    #[test]
    fn a_portrait_this_game_lacks_shows_as_the_default_banner() {
        let mut with = state();
        with.banner = Some("dr_karl_brandt".into());
        let members = &mut with.room.as_mut().unwrap().members;
        members[0].banner = Some("andrew".into());
        let shown = view(&with);
        assert!(crate::portraits::available().is_empty());
        assert!(shown.portraits.is_empty());
        assert_eq!(shown.banner, None);
        assert_eq!(shown.room.unwrap().members[0].banner, None);
        with.banner = Some("dry".into());
        with.room.as_mut().unwrap().members[0].banner = Some("m03".into());
        let shown = view(&with);
        assert_eq!(shown.banner.as_ref().map(Text::as_str), Some("dry"));
        assert_eq!(
            shown.room.unwrap().members[0]
                .banner
                .as_ref()
                .map(Text::as_str),
            Some("m03")
        );
    }

    #[test]
    fn the_menus_window_sees_what_the_launcher_shows() {
        let view = view(&state());
        assert_eq!(view.connection, LobbyConnection::Connected);
        assert_eq!(view.server.as_str(), "EU", "as players see it");
        assert_eq!(view.server_address.as_str(), "tpf3mp.example.org:29470");
        assert_eq!(view.server_default.as_str(), "tpf3mp.example.org:29470");
        assert_eq!(view.error.as_ref().unwrap().as_str(), "that room is full");
        assert_eq!(view.notice.as_ref().unwrap().as_str(), "new", "the newest");
        let mut started = state();
        started.notices.push(super::super::GAME_STARTED.into());
        assert_eq!(
            super::view(&started).notice.unwrap().as_str(),
            "new",
            "the launcher's own notice stays in the launcher"
        );
        assert!(encode(&ToHook::Lobby(Box::new(view.clone()))).is_ok());
        let room = view.room.unwrap();
        assert!(!room.running && room.you_own);
        assert_eq!(room.invite.unwrap().as_str(), "K7QM2X");
        assert_eq!(room.members.len(), 1, "a member it cannot name is left out");
        assert_eq!(room.members[0].same_content, Some(true));
        assert_eq!(view.chat.len(), MAX_LOBBY_CHAT);
        assert_eq!(
            view.chat.last().unwrap().text.as_str(),
            "line 49",
            "the newest"
        );
        assert_eq!(view.rules[0].name.as_str(), "native");
        let saves: Vec<&str> = view.saves.iter().map(Text::as_str).collect();
        assert_eq!(saves, ["mptest", "older"], "a name too long is left out");
        assert_eq!(view.start_save.as_ref().unwrap().as_str(), "mptest");
        // The save the room starts from, as the room names it, and the
        // owner's upload of it.
        let room_start = room.start.as_ref().unwrap();
        assert_eq!(
            (
                room_start.name.as_str(),
                room_start.map.as_str(),
                room_start.year,
                room_start.arrived
            ),
            ("mptest", "dry", 1900, false)
        );
        let upload = room.upload.as_ref().unwrap();
        assert_eq!((upload.save.as_str(), upload.percent), ("mptest", 40));
        assert_eq!(
            view.world,
            LobbyWorld::Fetching {
                bytes: 10,
                total: 40
            }
        );
        assert_eq!(
            view.differences.as_ref().unwrap().as_str(),
            "you lack stations 3"
        );
    }

    #[test]
    fn the_windows_buttons_are_the_launchers_actions_on_its_own_server() {
        let state = state();
        assert_eq!(
            action(
                LobbyAction::Connect {
                    name: Text::lossy("Ann")
                },
                &state
            ),
            Action::Connect {
                server: "tpf3mp.example.org:29470".into(),
                name: "Ann".into()
            }
        );
        assert_eq!(
            action(
                LobbyAction::Create {
                    room: Text::lossy("Alps"),
                    max_players: 4,
                    password: None,
                    rules: Some(Text::lossy("native")),
                    start_save: Some(Text::lossy("mptest")),
                    listing: Some(tpf3mp_bridge::LobbyListing {
                        map: Text::lossy("dry"),
                        year: 1900,
                    }),
                    competitive: false,
                },
                &state
            ),
            Action::Create {
                room: "Alps".into(),
                max_players: 4,
                password: None,
                rules: Some("native".into()),
                start_save: Some("mptest".into()),
                listing: Some(api::Listing {
                    map: "dry".into(),
                    year: 1900,
                }),
                competitive: false,
            }
        );
        assert_eq!(
            action(LobbyAction::ListRooms { page: 2 }, &state),
            Action::ListRooms { page: 2 }
        );
        assert_eq!(
            action(
                LobbyAction::SetBanner {
                    banner: Some(Text::lossy("dry"))
                },
                &state
            ),
            Action::SetBanner {
                banner: Some("dry".into())
            }
        );
        let bo = PlayerId(FixedBytes([2; 32]));
        assert_eq!(
            action(LobbyAction::Kick { player: bo }, &state),
            Action::Kick {
                player: api::player_hex(&bo)
            }
        );
        assert_eq!(action(LobbyAction::Start, &state), Action::Start);
        assert_eq!(action(LobbyAction::Leave, &state), Action::Leave);
        assert_eq!(
            action(
                LobbyAction::ChooseStart {
                    save: Text::lossy("older"),
                    map: Text::lossy("tropical"),
                    year: 1950,
                },
                &state
            ),
            Action::ChooseStart {
                save: "older".into(),
                map: "tropical".into(),
                year: 1950,
            }
        );
        assert_eq!(
            action(
                LobbyAction::SetServer {
                    server: Text::lossy("play.example.net:29470")
                },
                &state
            ),
            Action::SetServer {
                server: "play.example.net:29470".into()
            }
        );
    }

    #[test]
    fn a_closed_game_leaves_no_build_behind_so_it_can_be_started_again() {
        let running = IdleLink::given_back(FakeLink::default(), Some("40408".into()), true);
        assert_eq!(
            running.build(),
            Some("40408"),
            "a running game stays attached"
        );
        let closed = IdleLink::given_back(FakeLink::default(), Some("40408".into()), false);
        assert_eq!(closed.build(), None, "a closed game does not");
        assert_eq!(running.forget_game().build(), None);
    }
}
