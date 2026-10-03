//! The multiplayer rig end to end: several fake games on this PC, each with
//! its own agent, data folder and link, in one room on a throwaway server,
//! ending in the same world.

#![allow(clippy::unwrap_used)]

use std::{
    io::{BufRead, BufReader, Read},
    path::Path,
    process::{Command, ExitStatus, Stdio},
    sync::{Arc, Mutex, mpsc},
    time::{Duration, Instant},
};

/// How long the rig may run before the test ends it.
const LIMIT: Duration = Duration::from_secs(180);
/// The rig's own limit, inside the test's with room for a game still
/// starting (its hook may take 30 s to load), so that the rig stops what
/// it started itself.
const RIG_TIME_LIMIT: &str = "120";
/// How long the rig's output may stay open after it exited: a game it
/// left running would hold it open.
const OUTPUT_GRACE: Duration = Duration::from_secs(10);

/// How a run of the rig ended.
struct Run {
    status: ExitStatus,
    /// What it printed, then its errors.
    output: String,
    /// Whether its output closed: nothing it started outlived it.
    closed: bool,
}

/// Runs the rig with `args` and returns its output, failing if it fails.
fn rig(data_root: &Path, args: &[&str]) -> String {
    let run = run_rig(data_root, args);
    assert!(
        run.status.success(),
        "the rig failed ({}):\n{}",
        run.status,
        run.output
    );
    assert!(
        run.closed,
        "something the rig started outlived it:\n{}",
        run.output
    );
    run.output
}

/// Runs the rig with `args`.
fn run_rig(data_root: &Path, args: &[&str]) -> Run {
    run_rig_within(data_root, args, RIG_TIME_LIMIT)
}

/// Runs the rig with `args`, given `time_limit` seconds.
fn run_rig_within(data_root: &Path, args: &[&str], time_limit: &str) -> Run {
    let mut child = Command::new(env!("CARGO_BIN_EXE_tpf3mp-rig"))
        .args(["--server", "local", "--step-rate", "50"])
        .args(["--time-limit", time_limit, "--data-root"])
        .arg(data_root)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let stdout = Collected::start(child.stdout.take().unwrap());
    let stderr = Collected::start(child.stderr.take().unwrap());
    let started = Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        if started.elapsed() > LIMIT {
            let _ = child.kill();
            let _ = child.wait();
            let (output, _) = Collected::gather(stdout, stderr);
            panic!("the rig ran past {LIMIT:?}:\n{output}");
        }
        std::thread::sleep(Duration::from_millis(100));
    };
    let (output, closed) = Collected::gather(stdout, stderr);
    Run {
        status,
        output,
        closed,
    }
}

/// A stream read line by line as it comes, so that what came can be taken
/// while something still holds the stream open.
struct Collected {
    text: Arc<Mutex<String>>,
    /// Sent once the stream ended: `true` at its end, `false` when reading
    /// it failed.
    ended: mpsc::Receiver<bool>,
}

impl Collected {
    fn start(stream: impl Read + Send + 'static) -> Self {
        let text = Arc::new(Mutex::new(String::new()));
        let (tx, ended) = mpsc::channel();
        let into = Arc::clone(&text);
        std::thread::spawn(move || {
            let mut stream = BufReader::new(stream);
            let mut line = Vec::new();
            let closed = loop {
                line.clear();
                match stream.read_until(b'\n', &mut line) {
                    Ok(0) => break true,
                    Ok(_) => into
                        .lock()
                        .unwrap()
                        .push_str(&String::from_utf8_lossy(&line)),
                    Err(error) => {
                        into.lock()
                            .unwrap()
                            .push_str(&format!("[reading the output failed: {error}]\n"));
                        break false;
                    }
                }
            };
            let _ = tx.send(closed);
        });
        Self { text, ended }
    }

    /// Both streams' text, waiting at most [`OUTPUT_GRACE`] for them to
    /// close, and whether they did.
    fn gather(stdout: Self, stderr: Self) -> (String, bool) {
        let deadline = Instant::now() + OUTPUT_GRACE;
        let closed = [&stdout, &stderr].iter().all(|stream| {
            let left = deadline.saturating_duration_since(Instant::now());
            stream.ended.recv_timeout(left) == Ok(true)
        });
        let text = |stream: &Self| stream.text.lock().unwrap().clone();
        (text(&stdout) + &text(&stderr), closed)
    }
}

/// The lane digest lines `player`'s game printed.
fn lanes(output: &str, player: &str) -> Vec<String> {
    let prefix = format!("[{player}]   lane ");
    output
        .lines()
        .filter_map(|line| line.strip_prefix(&prefix))
        .map(str::to_owned)
        .collect()
}

#[test]
fn three_fake_games_play_one_room_and_agree() {
    let root = tempfile::tempdir().unwrap();
    let output = rig(root.path(), &["--players", "3", "--steps", "100"]);

    // The rig's launchers have a server of their own: the invite is the
    // room's code alone.
    let invite = output
        .lines()
        .find_map(|line| line.strip_prefix("rig: invite: "))
        .unwrap_or_else(|| panic!("no invite: {output}"));
    assert!(invite.parse::<tpf3mp_proto::Invite>().is_ok(), "{invite}");
    assert!(
        output.contains("rig: all 3 games ended on the same lane digests"),
        "{output}"
    );
    let host = lanes(&output, "p1");
    assert!(!host.is_empty(), "{output}");
    for guest in ["p2", "p3"] {
        assert_eq!(lanes(&output, guest), host, "{output}");
        // Guests ran every step, in the host's world.
        let report = output
            .lines()
            .find(|line| line.starts_with(&format!("[{guest}] ran ")))
            .unwrap_or_else(|| panic!("no report from {guest}:\n{output}"));
        assert!(
            report.starts_with(&format!("[{guest}] ran 100 steps")),
            "{report}"
        );
        assert!(report.contains("loaded 1 worlds from the room"), "{report}");
    }
    // Each player has a folder of its own, with its own identity.
    let keys: Vec<Vec<u8>> = ["p1", "p2", "p3"]
        .iter()
        .map(|player| std::fs::read(root.path().join(player).join("identity.key")).unwrap())
        .collect();
    assert_ne!(keys[0], keys[1]);
    assert_ne!(keys[1], keys[2]);
}

#[test]
fn staggered_games_start_one_after_another_and_still_agree() {
    let root = tempfile::tempdir().unwrap();
    let started = Instant::now();
    let output = rig(
        root.path(),
        &["--players", "2", "--steps", "20", "--stagger", "2"],
    );
    assert!(
        output.contains("rig: starting p2's game in 2 s"),
        "{output}"
    );
    assert!(started.elapsed() >= Duration::from_secs(2), "{output}");
    assert!(
        output.contains("rig: all 2 games ended on the same lane digests"),
        "{output}"
    );
}

#[test]
fn without_snapshots_every_game_loads_its_own_world_once_all_attached() {
    let root = tempfile::tempdir().unwrap();
    let output = rig(
        root.path(),
        &[
            "--players",
            "2",
            "--steps",
            "60",
            "--stagger",
            "2",
            "--wait-for-games",
            "--no-snapshots",
        ],
    );
    let waited = output
        .find("rig: waiting for every game to attach")
        .unwrap_or_else(|| panic!("{output}"));
    let started = output
        .find("rig: game started with 2 players")
        .unwrap_or_else(|| panic!("{output}"));
    let second = output
        .find("rig: p2 plays on link")
        .unwrap_or_else(|| panic!("{output}"));
    // p2's game may start before or after the rig begins to wait (setting
    // up the room can take longer than the stagger); the room's game starts
    // only after both.
    assert!(waited < started && second < started, "{output}");
    assert!(
        output.contains("rig: all 2 games ended on the same lane digests"),
        "{output}"
    );
    for player in ["p1", "p2"] {
        let report = output
            .lines()
            .find(|line| line.starts_with(&format!("[{player}] ran ")))
            .unwrap_or_else(|| panic!("no report from {player}:\n{output}"));
        assert!(report.contains("loaded 0 worlds from the room"), "{report}");
    }
}

/// A library every system has, to load in the hook's place: `cargo test`
/// does not build the hook as a library of its own. The hook's rules have
/// their own tests; this one is about how the rig starts a game.
fn stand_in_hook() -> Option<String> {
    let candidates: Vec<std::path::PathBuf> = if cfg!(windows) {
        let root = std::env::var_os("SystemRoot").unwrap_or_else(|| "C:\\Windows".into());
        vec![Path::new(&root).join("System32").join("version.dll")]
    } else {
        [
            "/lib/x86_64-linux-gnu/libc.so.6",
            "/usr/lib/x86_64-linux-gnu/libc.so.6",
            "/lib64/libc.so.6",
            "/usr/lib64/libc.so.6",
            "/usr/lib/libc.so.6",
        ]
        .map(std::path::PathBuf::from)
        .to_vec()
    };
    candidates
        .into_iter()
        .find(|path| path.is_file())
        .map(|path| path.display().to_string())
}

/// A game given by path, as the real one will be, is started as the
/// launcher starts it: with a library loaded into it, and told its link in
/// the environment. The fake game stands in for the game; it finds its
/// link where the hook does, and each one joins its player's room.
#[test]
fn a_game_by_path_starts_with_the_hook_and_finds_its_link() {
    let root = tempfile::tempdir().unwrap();
    let fakegame = env!("CARGO_BIN_EXE_tpf3mp-fakegame");
    let hook = stand_in_hook();
    let mut args = vec![
        "--players",
        "2",
        "--game",
        fakegame,
        "--game-arg",
        "--steps",
        "--game-arg",
        "60",
        // The stand-in never says it is ready.
        "--hook-ready-wait",
        "0",
    ];
    if let Some(hook) = &hook {
        args.extend(["--hook", hook]);
    }
    if cfg!(target_os = "macos") {
        // No game gets the hook on macOS yet; the rig says so and stops.
        let run = run_rig(root.path(), &args);
        assert!(!run.status.success(), "{}", run.output);
        assert!(
            run.output.contains("not possible on this system yet"),
            "{}",
            run.output
        );
        return;
    }
    let output = rig(root.path(), &args);
    for player in ["p1", "p2"] {
        assert!(
            output
                .lines()
                .any(|line| line.starts_with(&format!("[{player}] game "))
                    && line.contains("attached")),
            "{player}'s game never reached its agent:\n{output}"
        );
    }
    assert!(
        output.contains("rig: game started with 2 players"),
        "{output}"
    );
    // Both games ran to their last step and exited cleanly: a game that
    // fails fails the rig.
    assert!(!output.contains("game failed"), "{output}");
}

/// A rig out of time ends the games it started and fails. A game by path
/// writes to the rig's own output: one left running would hold a test's
/// pipe open for as long as it runs.
#[test]
fn a_rig_out_of_time_ends_what_it_started_and_fails() {
    if cfg!(target_os = "macos") {
        // No game by path starts on macOS yet.
        return;
    }
    let root = tempfile::tempdir().unwrap();
    let fakegame = env!("CARGO_BIN_EXE_tpf3mp-fakegame");
    let hook = stand_in_hook().expect("a library to stand in for the hook");
    // Without --steps the fake game plays on until it is ended.
    let args = [
        "--players",
        "1",
        "--game",
        fakegame,
        "--hook",
        &hook,
        "--hook-ready-wait",
        "0",
    ];
    let run = run_rig_within(root.path(), &args, "5");
    assert!(!run.status.success(), "{}", run.output);
    assert!(
        run.output.contains("rig: p1 plays on link"),
        "{}",
        run.output
    );
    assert!(
        run.output.contains("the run took longer than its 5 s"),
        "{}",
        run.output
    );
    assert!(run.closed, "the game outlived the rig:\n{}", run.output);
}

/// Out of time while a game is still starting (on Windows, suspended for
/// a hook that never says it is ready), the rig still ends that game once
/// its start returns.
#[test]
fn a_rig_out_of_time_ends_a_game_still_starting() {
    if !cfg!(windows) {
        // Only Windows holds a game for its hook.
        return;
    }
    let root = tempfile::tempdir().unwrap();
    let fakegame = env!("CARGO_BIN_EXE_tpf3mp-fakegame");
    let hook = stand_in_hook().expect("a library to stand in for the hook");
    let args = [
        "--players",
        "1",
        "--game",
        fakegame,
        "--hook",
        &hook,
        "--hook-ready-wait",
        "10",
    ];
    let started = Instant::now();
    let run = run_rig_within(root.path(), &args, "2");
    assert!(!run.status.success(), "{}", run.output);
    assert!(
        run.output.contains("the run took longer than its 2 s"),
        "{}",
        run.output
    );
    assert!(run.closed, "the game outlived the rig:\n{}", run.output);
    assert!(
        started.elapsed() < Duration::from_secs(60),
        "{:?}:\n{}",
        started.elapsed(),
        run.output
    );
}
