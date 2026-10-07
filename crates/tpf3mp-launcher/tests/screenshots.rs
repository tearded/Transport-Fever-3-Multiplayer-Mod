//! Renders the launcher's screens to images for a person to look at:
//!
//! ```text
//! cargo test -p tpf3mp-launcher --test screenshots -- --ignored
//! ```
//!
//! The images land in `target/launcher-screenshots/`, one for each of the
//! sample states the page it copies offers (its `preview.js`), with the
//! same data and the lobby in the window, so the two can be laid side by
//! side (D20); and the `g*` ones, the window as it opens, with the lobby
//! in the game's menu (D17). They need a GPU (or a software renderer), so
//! the test is not part of the normal run.

#![allow(clippy::unwrap_used)]

use std::{cell::RefCell, path::PathBuf};

use eframe::egui;
use egui_kittest::{Harness, kittest::Queryable};
use tpf3mp_agent::launcher::{
    Action, ChatLine, Connection, Differences, Game, InstalledGame, Member, MemberContent, Phase,
    Room, RulesChoice, ServerRow, State, World,
};
use tpf3mp_launcher::{
    app::{Extras, LauncherApp, Shown},
    backend::Backend,
    notes::{Block, Notes, ReleaseNotes},
    probe::{Probe, Reach},
    update::UpdateState,
    view::Place,
};

struct Still(RefCell<State>);

impl Backend for Still {
    fn state(&self) -> State {
        self.0.borrow().clone()
    }

    fn act(&self, _action: Action) {}

    fn busy(&self) -> bool {
        false
    }
}

fn render(name: &str, state: State, scroll_left: f32) {
    render_clicking(name, state, scroll_left, None, Place::Launcher);
}

fn render_in_game(name: &str, state: State) {
    render_clicking(name, state, 0.0, None, Place::Game);
}

fn render_clicking(name: &str, state: State, scroll_left: f32, click: Option<&str>, place: Place) {
    let app = LauncherApp::new(
        Still(RefCell::new(state)),
        Extras {
            updater: None,
            probe: Some(Probe::showing(Reach::Online)),
            notes: Some(ReleaseNotes::showing(Notes::Release {
                version: "0.2.0".into(),
                blocks: vec![
                    Block::Heading("New".into()),
                    Block::Item("Rooms remember their rules across a server restart.".into()),
                    Block::Item("The launcher shows release notes.".into()),
                    Block::Heading("Fixes".into()),
                    Block::Item("Joining by invite works in either case.".into()),
                ],
            })),
            shown: Shown {
                update: Some(UpdateState::Ready {
                    version: "0.2.0".into(),
                }),
                installed_mod: Some(Some("0.1.0".into())),
            },
        },
    )
    .with_place(place);
    let mut harness = Harness::builder()
        .with_size(egui::vec2(1100.0, 690.0))
        .wgpu()
        .build_ui_state(|ui, app: &mut LauncherApp<Still>| app.show(ui), app);
    harness.run_steps(4);
    if scroll_left > 0.0 {
        harness.hover_at(egui::pos2(300.0, 300.0));
        harness.event(egui::Event::MouseWheel {
            unit: egui::MouseWheelUnit::Point,
            delta: egui::vec2(0.0, -scroll_left),
            phase: egui::TouchPhase::Move,
            modifiers: egui::Modifiers::NONE,
        });
        harness.run_steps(8);
    }
    if let Some(label) = click {
        harness.get_by_label(label).click();
        harness.run_steps(6);
    }
    let image = harness.render().unwrap();
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/launcher-screenshots");
    std::fs::create_dir_all(&dir).unwrap();
    image.save(dir.join(format!("{name}.png"))).unwrap();
}

/// The page's `base` state.
fn base() -> State {
    State {
        name: "Ann".into(),
        player: Some("7QM2".into()),
        server: Some("play.tpf3mp.example:29470".into()),
        server_fixed: true,
        server_default: Some("play.tpf3mp.example:29470".into()),
        server_name: Some("EU".into()),
        rules: vec![
            RulesChoice {
                name: "native".into(),
                description: "The game's own economy, as in single player.".into(),
            },
            RulesChoice {
                name: "canonical".into(),
                description: "The server settles the economy.".into(),
            },
        ],
        installed: Some(InstalledGame {
            dir: r"C:\Program Files (x86)\Steam\steamapps\common\Transport Fever 3".into(),
            build: "20364158".into(),
        }),
        diagnostics: Some(true),
        ..State::default()
    }
}

fn connected() -> State {
    State {
        connection: Connection::Connected,
        server_version: Some("0.1.0".into()),
        support_id: Some("S4TK9Q".into()),
        log_session: Some("AB2CD3".into()),
        ..base()
    }
}

fn member(
    id: &str,
    name: &str,
    platform: &str,
    owner: bool,
    you: bool,
    ready: bool,
    content: MemberContent,
) -> Member {
    Member {
        id: id.into(),
        name: name.into(),
        platform: platform.into(),
        ready,
        connected: true,
        owner,
        you,
        content,
        differs: None,
        banner: None,
        loading: None,
    }
}

fn members() -> Vec<Member> {
    vec![
        member(
            "p1",
            "Ann",
            "Windows x86-64",
            true,
            true,
            true,
            MemberContent::Same,
        ),
        member(
            "p2",
            "Bob",
            "Windows x86-64",
            false,
            false,
            true,
            MemberContent::Same,
        ),
        member(
            "p3",
            "Cat",
            "Linux x86-64",
            false,
            false,
            false,
            MemberContent::Differs,
        ),
    ]
}

fn room(phase: Phase, members: Vec<Member>) -> Room {
    Room {
        name: "Friday trains".into(),
        rules: "native".into(),
        phase,
        invite: Some("K7QM2X".into()),
        you_own: true,
        max_players: 4,
        has_password: false,
        members,
        competitive: false,
    }
}

fn chat() -> Vec<ChatLine> {
    vec![
        ChatLine {
            from: "Bob".into(),
            text: "I'll take the coal line up north.".into(),
            you: false,
        },
        ChatLine {
            from: "Ann".into(),
            text: "Fine by me, I'm on passengers.".into(),
            you: true,
        },
    ]
}

/// The window as it opens: the lobby in the game's menu (D17). Part of
/// [`screens`]: two tests rendering at once crash some GPU drivers.
fn screens_with_the_lobby_in_the_game() {
    render_in_game("g1-start-the-game", base());
    let attached = Game {
        attached: Some("40408".into()),
        ..Game::default()
    };
    render_in_game(
        "g2-game-running",
        State {
            game: attached.clone(),
            ..base()
        },
    );
    render_in_game(
        "g3-in-the-room",
        State {
            room: Some(room(Phase::Lobby, members())),
            chat: chat(),
            game: attached,
            ..connected()
        },
    );
    render_in_game(
        "g4-receiving-the-world",
        State {
            room: Some(room(Phase::Running, members())),
            game: Game {
                attached: Some("40408".into()),
                world: World::Fetching,
                bytes: 48_000_000,
                total: 112_000_000,
                step: None,
                speed: 100,
            },
            ..connected()
        },
    );
    render_in_game(
        "g5-playing",
        State {
            room: Some(room(Phase::Running, members())),
            notices: vec!["Bob joined the room.".into(), "The room started.".into()],
            game: Game {
                attached: Some("40408".into()),
                world: World::Playing,
                bytes: 0,
                total: 0,
                step: Some(18432),
                speed: 200,
            },
            ..connected()
        },
    );
}

#[test]
#[ignore = "renders images for review; needs a GPU or a software renderer"]
fn screens() {
    screens_with_the_lobby_in_the_game();
    render("1-not-connected", base(), 0.0);
    render(
        "2-connecting",
        State {
            connection: Connection::Connecting,
            ..base()
        },
        0.0,
    );
    render("3-connected", connected(), 0.0);
    render_clicking(
        "9-settings",
        connected(),
        0.0,
        Some("Settings"),
        Place::Launcher,
    );
    // A release with several servers (D12's proposed amendment of
    // 2026-10-06): the setting names each with its ping.
    render_clicking(
        "9b-settings-servers",
        State {
            servers: vec![
                ServerRow {
                    name: "EU".into(),
                    ping_ms: Some(24),
                    here: true,
                    reachable: true,
                },
                ServerRow {
                    name: "US".into(),
                    ping_ms: Some(108),
                    here: false,
                    reachable: true,
                },
            ],
            ..connected()
        },
        0.0,
        Some("Settings"),
        Place::Launcher,
    );
    let lobby = State {
        room: Some(room(Phase::Lobby, members())),
        chat: chat(),
        content_diff: Some(Differences {
            summary: "Cat runs other mods than the room.".into(),
            game: None,
            missing: vec![],
            missing_more: 0,
            extra: vec!["gw_cheats_1 1".into()],
            extra_more: 0,
            changed: vec![],
            changed_more: 0,
            reordered: false,
            unlisted: false,
        }),
        ..connected()
    };
    render("4-in-the-lobby", lobby.clone(), 0.0);
    render("4b-in-the-lobby-scrolled", lobby, 420.0);
    let ready: Vec<Member> = members()
        .into_iter()
        .map(|member| Member {
            ready: true,
            content: MemberContent::Same,
            differs: None,
            ..member
        })
        .collect();
    render(
        "5-game-started",
        State {
            room: Some(room(Phase::Lobby, ready)),
            chat: chat(),
            game: Game {
                attached: Some("40391".into()),
                ..Game::default()
            },
            ..connected()
        },
        0.0,
    );
    render(
        "6-receiving-the-world",
        State {
            room: Some(room(Phase::Running, members())),
            chat: chat(),
            game: Game {
                attached: Some("40391".into()),
                world: World::Fetching,
                bytes: 48_000_000,
                total: 112_000_000,
                step: None,
                speed: 100,
            },
            ..connected()
        },
        0.0,
    );
    render(
        "7-playing",
        State {
            room: Some(room(Phase::Running, members())),
            chat: chat(),
            notices: vec![
                "Bob joined the room.".into(),
                "The room started.".into(),
                "Speed 2×.".into(),
            ],
            game: Game {
                attached: Some("40391".into()),
                world: World::Playing,
                bytes: 0,
                total: 0,
                step: Some(18432),
                speed: 200,
            },
            ..connected()
        },
        0.0,
    );
    render(
        "8-update-needed",
        State {
            outdated: true,
            error: Some("The server speaks a newer protocol: update TPF3-MP to play there.".into()),
            ..base()
        },
        0.0,
    );
}
