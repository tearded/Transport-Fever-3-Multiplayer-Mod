//! `tpf3mp-launcher`: the TPF3-MP launcher window.

// A windowed program on Windows, without a console behind it. Debug builds
// keep the console, for running from a terminal.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

use std::{
    process::ExitCode,
    sync::{Arc, OnceLock},
    time::Duration,
};

use anyhow::{Context, Result};
use clap::Parser;
use eframe::egui;
use tpf3mp_agent::{
    about,
    diagnostics::Recorder,
    launcher::{
        Launcher, LauncherConfig, Remembered,
        instance::{self, Arrived},
        setup,
    },
};
use tpf3mp_launcher::{
    app::{Extras, LauncherApp, Shown},
    backend::Local,
    icon, logs,
    notes::ReleaseNotes,
    probe::Probe,
    update,
};
use tracing::{error, info, warn};

/// The TPF3-MP launcher: connect to a server, create or join a room, and
/// play Transport Fever 3 together.
#[derive(Debug, Parser)]
#[command(version, about)]
// A flag given twice counts once, the later winning, so a player may add
// flags to a shortcut.
#[command(args_override_self = true)]
struct Args {
    #[command(flatten)]
    launcher: setup::LauncherArgs,

    /// Show the launcher as a page in the browser instead of a window.
    #[arg(long)]
    browser: bool,

    /// Repair the Windows installation and its multiplayer mod.
    #[arg(long, conflicts_with = "uninstall")]
    repair: bool,

    /// Remove the Windows installation, keeping saves and settings.
    #[arg(long)]
    uninstall: bool,

    #[command(flatten)]
    auto: AutoRoom,
}

/// For playtests on one PC: get into a room without clicking. The owner's
/// launcher connects, creates the room and writes its invite to
/// `--invite-file`; every other launcher waits for that file and joins.
#[derive(Debug, Clone, clap::Args)]
struct AutoRoom {
    /// Connect at start, create a room of this name and write its invite
    /// to --invite-file.
    #[arg(long, requires = "invite_file", conflicts_with = "auto_join")]
    auto_create: Option<String>,

    /// Connect at start, wait for --invite-file and join its room.
    #[arg(long, requires = "invite_file")]
    auto_join: bool,

    /// The file the owner's invite is written to and read from.
    #[arg(long)]
    invite_file: Option<std::path::PathBuf>,

    /// With --auto-create: start the room's game once at least this many
    /// players are in it and every one is ready.
    #[arg(long, requires = "auto_create", conflicts_with = "auto_join")]
    auto_start: Option<usize>,

    /// Start Transport Fever 3 by itself, as its button does: once in the
    /// room with --auto-create or --auto-join, otherwise right away.
    #[arg(long)]
    auto_play: bool,

    /// The name of a save in the game's save folder, such as `mptest`, that
    /// the game loads by itself from its main menu, once, and starts with
    /// no Start Game to press. For the room's owner: a guest waits at the
    /// menu and gets the room's world.
    #[arg(long, value_name = "SAVE")]
    auto_load: Option<String>,

    /// The name of a save in the game's save folder, such as `mptest`, or
    /// the path of a save file, that rooms this launcher creates start
    /// from. The launcher hands it to the room in the lobby, and every
    /// game, this one too, loads it from its main menu when the room's game
    /// starts; this player is marked ready at the menu once the room has
    /// it. Instead of --auto-load, which has this game load the world and
    /// save it for the room.
    #[arg(long, value_name = "SAVE", conflicts_with = "auto_load")]
    start_save: Option<String>,

    /// The folder the game's hook keeps its log and profiles in, instead of
    /// the per-user one. With --game-link, --listen, --identity and
    /// --worlds, several games run on one PC without a sandbox, each with
    /// its own hook.log.
    #[arg(long, value_name = "DIR")]
    game_data_dir: Option<std::path::PathBuf>,
}

/// The server a package plays on by default, set when it is built. Without
/// it, the project's relay ([`setup::RELAY`]), in every build.
const DEFAULT_SERVER: Option<&str> = option_env!("TPF3MP_DEFAULT_SERVER");
/// What players see of that server, such as EU, set when it is built.
const SERVER_NAME: Option<&str> = option_env!("TPF3MP_SERVER_NAME");

/// The launcher's default server and its name: the ones the package was
/// built with (`built`, `named`), else the project's relay, named
/// [`setup::RELAY_NAME`] unless `named` says otherwise. Empty counts as
/// unset, as a release's unset repository variable arrives.
fn package_server(built: Option<&str>, named: Option<&str>) -> (String, Option<String>) {
    let given = |value: Option<&str>| {
        value
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_owned)
    };
    match given(built) {
        Some(server) => (server, given(named)),
        None => (
            setup::RELAY.to_owned(),
            Some(given(named).unwrap_or_else(|| setup::RELAY_NAME.to_owned())),
        ),
    }
}

fn main() -> ExitCode {
    let mut args = Args::parse();
    if args.launcher.default_server.is_none() {
        let (server, name) = package_server(DEFAULT_SERVER, SERVER_NAME);
        args.launcher.default_server = Some(server);
        if args.launcher.server_name.is_none() {
            args.launcher.server_name = name;
        }
    }
    let logs = logs::dir().ok();
    // The log's lines also wait here to go to the server, redacted.
    let diagnostics = Recorder::new();
    let _logging = logs
        .as_deref()
        .and_then(|dir| logs::start(dir, Some(diagnostics.clone())).ok());
    // First, which file runs and which build it is, so every log says.
    info!(
        os = std::env::consts::OS,
        arch = std::env::consts::ARCH,
        "{}",
        about::startup_line(about::exe().as_deref(), &about::Build::this(), about::BUILT)
    );
    // The code every line of this run's diagnostics carries, which the
    // window shows: the local log names it too, to put the two side by side.
    info!(log_session = %diagnostics.run(), "this run's log session");
    // An update downloaded last time installs before anything connects.
    if update::at_start() {
        return ExitCode::SUCCESS;
    }
    match run(args, diagnostics) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            error!("{error:#}");
            show_error(&format!("{error:#}"));
            ExitCode::FAILURE
        }
    }
}

fn run(args: Args, diagnostics: Recorder) -> Result<()> {
    if tpf3mp_launcher::installation::before_launch(args.repair, args.uninstall)? {
        return Ok(());
    }
    // The owner of an automatic room takes the last room's invite away
    // first thing, before the seconds the configuration takes (the mods are
    // scanned): joiners started beside it read the file meanwhile, and took
    // the last room's invite, a room the server may still hold running
    // (2026-10-01).
    if args.auto.auto_create.is_some()
        && let Some(file) = &args.auto.invite_file
    {
        let _ = std::fs::remove_file(file);
    }
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .thread_name("tpf3mp")
        .build()
        .context("starting the launcher")?;
    // Never quietly next to a launcher of another build: it would keep the
    // game, and play with its own protocol. Before the configuration, which
    // takes the link's worlds for this launcher.
    let arrived = instance::arrive(&args.launcher.game_link).map_err(anyhow::Error::msg)?;
    // Its record stays beside the link while `arrived` lives: to the end.
    let serving = matches!(arrived, Arrived::Serving(_));
    let mut config = match (&arrived, args.launcher.config()) {
        (_, Ok(config)) => config,
        // One of this build has the game's link, and so its worlds.
        (Arrived::Beside(other), Err(error)) => {
            return Err(error.context(instance::already_running(other)));
        }
        (Arrived::Serving(_), Err(error)) => return Err(error),
    };
    if let Some(save) = &args.auto.auto_load {
        config
            .game_env
            .push((tpf3mp_ipc::AUTO_LOAD_ENV.to_owned(), save.clone()));
    }
    if let Some(save) = &args.auto.start_save {
        let file = tpf3mp_agent::steam::find_save(save)
            .map_err(anyhow::Error::msg)
            .context("finding the save rooms start from (--start-save)")?;
        info!(file = %file.display(), "rooms this launcher creates start from this save");
        config.start_save = Some(file);
    }
    if let Some(dir) = &args.auto.game_data_dir {
        std::fs::create_dir_all(dir).context("making the game's data folder")?;
        config.game_env.push((
            tpf3mp_ipc::DATA_DIR_ENV.to_owned(),
            dir.to_string_lossy().into_owned(),
        ));
    }
    // On unless the player switched them off.
    if let Some(file) = &config.remember {
        diagnostics.set_on(Remembered::load(file).diagnostics.unwrap_or(true));
    }
    config.diagnostics = Some(diagnostics);
    if args.browser {
        return in_browser(&runtime, config, serving);
    }
    let launcher = {
        let _entered = runtime.enter();
        Launcher::start_local(config.clone())
    };
    // A launcher of another build asks this one to close: the window
    // closes, or the process ends before the window is open.
    let window: Arc<OnceLock<egui::Context>> = Arc::default();
    if serving {
        let window = Arc::clone(&window);
        instance::watch(
            config.link.clone(),
            launcher.handle(),
            move || match window.get() {
                Some(ctx) => {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                    ctx.request_repaint();
                }
                None => std::process::exit(0),
            },
        );
    }
    auto_room(&runtime, &launcher, &config, args.auto.clone());
    // Whether the package's own server is up, shown before connecting.
    let probe = config
        .server
        .as_deref()
        .filter(|_| config.server_fixed)
        .and_then(Probe::start);
    let mut backend = Local::new(launcher.handle(), runtime.handle().clone());
    let updater = update::Updater::start(runtime.handle().clone());
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Transport Fever 3 · Multiplayer")
            .with_app_id("tpf3mp-launcher")
            // The page's size, as tearded's launcher opens, and still
            // within a 1366x768 screen.
            .with_inner_size([1100.0, 690.0])
            .with_min_inner_size([960.0, 620.0])
            .with_icon(icon::icon()),
        ..Default::default()
    };
    let opened = eframe::run_native(
        "TPF3-MP",
        options,
        Box::new(move |creation| {
            // The window and its renderer exist: this version works, so an
            // update just installed is complete.
            update::started();
            let _ = window.set(creation.egui_ctx.clone());
            backend.repaint_with(creation.egui_ctx.clone());
            updater.repaint_with(creation.egui_ctx.clone());
            Ok(Box::new(LauncherApp::new(
                backend,
                Extras {
                    updater: Some(updater),
                    probe,
                    notes: Some(ReleaseNotes::fetch()),
                    shown: Shown::default(),
                },
            )))
        }),
    );
    drop(launcher);
    match opened {
        Ok(()) => {
            info!("the launcher closes");
            // A download may still be running; it can be picked up next time.
            runtime.shutdown_timeout(Duration::from_secs(2));
            Ok(())
        }
        Err(error) => {
            warn!(%error, "cannot open the launcher's window; opening it in the browser instead");
            in_browser(&runtime, config, serving)
        }
    }
}

/// Gets into a room without clicking (see [`AutoRoom`]). Failures are
/// logged and shown in the launcher; the player can still click.
fn auto_room(
    runtime: &tokio::runtime::Runtime,
    launcher: &Launcher,
    config: &LauncherConfig,
    auto: AutoRoom,
) {
    use tpf3mp_agent::launcher::Action;
    let in_a_room = auto.auto_create.is_some() || auto.auto_join;
    if auto.auto_play && !in_a_room {
        let handle = launcher.handle();
        runtime.spawn(async move { play(&handle).await });
        return;
    }
    let Some(file) = auto.invite_file.clone() else {
        return;
    };
    if !in_a_room {
        return;
    }
    let Some(server) = config.server.clone() else {
        warn!("--auto-create and --auto-join need --server");
        return;
    };
    let handle = launcher.handle();
    let name = config.name.clone();
    // The owner starts from no file, so a joiner never takes an old invite.
    if auto.auto_create.is_some() {
        let _ = std::fs::remove_file(&file);
    }
    runtime.spawn(async move {
        let connect = || Action::Connect {
            server: server.clone(),
            name: name.clone(),
        };
        let mut connected = false;
        for _ in 0..60 {
            if handle.act(connect()).await.is_ok() {
                connected = true;
                break;
            }
            tokio::time::sleep(Duration::from_secs(2)).await;
        }
        if !connected {
            warn!("auto room: could not connect to {server}");
            return;
        }
        if let Some(room) = auto.auto_create {
            if let Err(error) = handle
                .act(Action::Create {
                    room,
                    max_players: 8,
                    password: None,
                    rules: None,
                    start_save: None,
                    listing: None,
                    competitive: false,
                })
                .await
            {
                warn!(%error, "auto room: could not create the room");
                return;
            }
            let invite = handle.state().room.and_then(|room| room.invite);
            match invite {
                Some(invite) => match std::fs::write(&file, &invite) {
                    Ok(()) => {
                        info!(%invite, file = %file.display(), "auto room: created, invite written")
                    }
                    Err(error) => warn!(%error, "auto room: cannot write the invite file"),
                },
                None => warn!("auto room: the room has no invite"),
            }
            if auto.auto_play {
                play(&handle).await;
            }
            let Some(players) = auto.auto_start else {
                return;
            };
            // Start once everyone is in and ready (auto-ready marks each
            // player once their save's world is up, or their game waits at
            // its menu for the room's world; with --start-save, the owner
            // once the room has that save too).
            loop {
                tokio::time::sleep(Duration::from_secs(1)).await;
                let Some(room) = handle.state().room else {
                    return;
                };
                if room.phase != tpf3mp_agent::launcher::Phase::Lobby {
                    return;
                }
                if ready_to_start(&room, players) {
                    match handle.act(Action::Start).await {
                        Ok(()) => info!("auto room: everyone is ready; started the game"),
                        Err(error) => warn!(%error, "auto room: could not start the game"),
                    }
                    return;
                }
            }
        }
        // Join: wait for the owner's invite. One that is refused may be the
        // last room's, read before the owner took it away: wait for another.
        let mut refused: Option<String> = None;
        for _ in 0..600 {
            if let Ok(invite) = std::fs::read_to_string(&file) {
                let invite = invite.trim().to_owned();
                if !invite.is_empty() && !stale_invite(refused.as_deref(), &invite) {
                    match handle
                        .act(Action::Join {
                            invite: invite.clone(),
                            password: None,
                        })
                        .await
                    {
                        Ok(()) => {
                            info!(%invite, "auto room: joined");
                            if auto.auto_play {
                                play(&handle).await;
                            }
                            return;
                        }
                        Err(error) => {
                            warn!(%error, %invite, "auto room: could not join; waiting for another invite");
                            refused = Some(invite);
                        }
                    }
                }
            }
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
        warn!("auto room: no invite to join appeared in {}", file.display());
    });
}

/// Whether `invite`, read from the invite file, is the one a join was just
/// refused with: the last room's, until the owner writes its own.
fn stale_invite(refused: Option<&str>, invite: &str) -> bool {
    refused == Some(invite)
}

/// `--auto-play`: starts the game as the launcher's button does.
async fn play(handle: &tpf3mp_agent::launcher::LauncherHandle) {
    match handle.act(tpf3mp_agent::launcher::Action::LaunchGame).await {
        Ok(()) => info!("auto play: started Transport Fever 3"),
        Err(error) => warn!(%error, "auto play: could not start Transport Fever 3"),
    }
}

/// Whether `--auto-start <players>` starts `room` now: in its lobby, with at
/// least `players` members, every one of them ready.
fn ready_to_start(room: &tpf3mp_agent::launcher::Room, players: usize) -> bool {
    room.phase == tpf3mp_agent::launcher::Phase::Lobby
        && room.members.len() >= players
        && room.members.iter().all(|member| member.ready)
}

/// Runs the launcher as a page in the browser until Ctrl-C, or until a
/// launcher of another build takes over (`serving`: this one holds the
/// game's link).
fn in_browser(
    runtime: &tokio::runtime::Runtime,
    config: LauncherConfig,
    serving: bool,
) -> Result<()> {
    runtime.block_on(async {
        let listen = config.listen;
        let link = config.link.clone();
        let launcher = Launcher::start(config)
            .await
            .with_context(|| format!("serving the launcher on {listen}"))?;
        if serving {
            instance::watch(link, launcher.handle(), || std::process::exit(0));
        }
        let url = launcher
            .url()
            .context("the launcher serves no page")?
            .to_owned();
        info!("the launcher runs in the browser");
        println!("TPF3-MP launcher: {url}");
        println!("Keep this running while you play. Ctrl-C stops the launcher.");
        if !setup::open_in_browser(&url) {
            println!("Open the address above in your browser.");
        }
        tokio::select! {
            () = launcher.wait() => {}
            _ = tokio::signal::ctrl_c() => {}
        }
        Ok(())
    })
}

/// Says why the launcher cannot start, in a window, since a windowed
/// program has no console to print it on.
fn show_error(message: &str) {
    eprintln!("TPF3-MP cannot start: {message}");
    let message = message.to_owned();
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("TPF3-MP")
            .with_inner_size([540.0, 200.0])
            .with_icon(icon::icon()),
        ..Default::default()
    };
    let _ = eframe::run_native(
        "TPF3-MP",
        options,
        Box::new(move |_| Ok(Box::new(ErrorWindow(message)))),
    );
}

struct ErrorWindow(String);

impl eframe::App for ErrorWindow {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        egui::CentralPanel::default_margins().show(ui, |ui| {
            ui.heading("TPF3-MP cannot start");
            ui.add_space(6.0);
            ui.label(&self.0);
            ui.add_space(6.0);
            ui.label(
                egui::RichText::new("The launcher's log, in the TPF3-MP logs folder, has more.")
                    .weak(),
            );
            if ui.button("Close").clicked() {
                ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use clap::Parser;

    use tpf3mp_agent::launcher::{Member, MemberContent, Phase, Room};

    use super::{Args, package_server, ready_to_start, stale_invite};
    use tpf3mp_agent::launcher::setup::{RELAY, RELAY_NAME};

    /// A joiner started beside the owner may read the last room's invite
    /// before the owner takes it away; refused there, it waits for another
    /// (2026-10-01: both guests were refused by the last, running room).
    #[test]
    fn a_refused_invite_is_not_tried_again() {
        assert!(!stale_invite(None, "QHK8QR"), "the first is tried");
        assert!(
            stale_invite(Some("CP8HKQ"), "CP8HKQ"),
            "the refused one waits"
        );
        assert!(
            !stale_invite(Some("CP8HKQ"), "QHK8QR"),
            "the owner's new one is tried"
        );
    }

    #[test]
    fn the_default_server_is_the_packages_else_the_relay() {
        assert_eq!(
            package_server(None, None),
            (RELAY.to_owned(), Some(RELAY_NAME.to_owned())),
            "a build without TPF3MP_DEFAULT_SERVER, a developer's too"
        );
        assert_eq!(
            package_server(Some(""), Some(" ")),
            (RELAY.to_owned(), Some(RELAY_NAME.to_owned())),
            "an unset repository variable arrives empty"
        );
        assert_eq!(
            package_server(None, Some("EU")),
            (RELAY.to_owned(), Some("EU".to_owned()))
        );
        assert_eq!(
            package_server(Some(" eu.example.org:29470 "), None),
            ("eu.example.org:29470".to_owned(), None),
            "the relay's name is the relay's alone"
        );
        assert_eq!(
            package_server(Some("eu.example.org:29470"), Some("EU")),
            ("eu.example.org:29470".to_owned(), Some("EU".to_owned()))
        );
    }

    fn parse(args: &[&str]) -> Result<Args, clap::Error> {
        Args::try_parse_from(std::iter::once("tpf3mp-launcher").chain(args.iter().copied()))
    }

    #[test]
    fn the_auto_room_flags_need_an_invite_file_and_one_role() {
        let owner = parse(&[
            "--auto-create",
            "test",
            "--invite-file",
            "invite.txt",
            "--auto-start",
            "2",
        ])
        .unwrap();
        assert_eq!(owner.auto.auto_create.as_deref(), Some("test"));
        assert_eq!(owner.auto.auto_start, Some(2));
        let guest = parse(&["--auto-join", "--invite-file", "invite.txt"]).unwrap();
        assert!(guest.auto.auto_join);
        assert!(parse(&["--auto-create", "test"]).is_err(), "no invite file");
        assert!(parse(&["--auto-join"]).is_err(), "no invite file");
        assert!(
            parse(&["--auto-create", "test", "--auto-join", "--invite-file", "i"]).is_err(),
            "both roles"
        );
        assert!(
            parse(&["--auto-join", "--invite-file", "i", "--auto-start", "2"]).is_err(),
            "only the owner starts"
        );
        assert!(parse(&[]).unwrap().auto.invite_file.is_none());
    }

    #[test]
    fn the_game_can_start_and_load_a_save_by_itself() {
        let owner = parse(&[
            "--auto-create",
            "test",
            "--invite-file",
            "i",
            "--auto-play",
            "--auto-load",
            "mptest",
        ])
        .unwrap();
        assert!(owner.auto.auto_play);
        assert_eq!(owner.auto.auto_load.as_deref(), Some("mptest"));
        let guest = parse(&["--auto-join", "--invite-file", "i", "--auto-play"]).unwrap();
        assert!(guest.auto.auto_play && guest.auto.auto_load.is_none());
        let plain = parse(&[]).unwrap();
        assert!(!plain.auto.auto_play && plain.auto.auto_load.is_none());
        let starting = parse(&[
            "--auto-create",
            "test",
            "--invite-file",
            "i",
            "--auto-play",
            "--start-save",
            "twomptest",
        ])
        .unwrap();
        assert_eq!(starting.auto.start_save.as_deref(), Some("twomptest"));
        assert!(starting.auto.auto_load.is_none());
        assert!(
            parse(&["--start-save", "a", "--auto-load", "b"]).is_err(),
            "the game loads the save itself, or every game loads it from the room"
        );
        let apart = parse(&["--game-data-dir", "games/cat"]).unwrap();
        assert_eq!(
            apart.auto.game_data_dir.as_deref(),
            Some(std::path::Path::new("games/cat"))
        );
    }

    fn room(ready: &[bool], phase: Phase) -> Room {
        Room {
            name: "test".into(),
            rules: "native".into(),
            phase,
            invite: None,
            you_own: true,
            max_players: 8,
            has_password: false,
            members: ready
                .iter()
                .enumerate()
                .map(|(i, &ready)| Member {
                    id: i.to_string(),
                    name: format!("p{i}"),
                    platform: "windows".into(),
                    ready,
                    connected: true,
                    owner: i == 0,
                    you: i == 0,
                    content: MemberContent::Same,
                    differs: None,
                    banner: None,
                    loading: None,
                })
                .collect(),
            competitive: false,
        }
    }

    #[test]
    fn auto_start_waits_for_enough_players_all_ready_in_the_lobby() {
        assert!(ready_to_start(&room(&[true, true], Phase::Lobby), 2));
        assert!(
            !ready_to_start(&room(&[true], Phase::Lobby), 2),
            "one short"
        );
        assert!(!ready_to_start(&room(&[true, false], Phase::Lobby), 2));
        assert!(!ready_to_start(&room(&[true, true], Phase::Running), 2));
        assert!(ready_to_start(&room(&[true, true, true], Phase::Lobby), 2));
    }
}
