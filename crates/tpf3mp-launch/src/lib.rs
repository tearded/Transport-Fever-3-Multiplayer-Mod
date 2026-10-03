//! Starts Transport Fever 3 with TPF3-MP's hook in it, for one multiplayer
//! session, and nowhere else (D11 in `docs/DECISIONS.md`). Nothing is put in
//! the game's folder: a game started from Steam is the plain game, and one
//! started here runs the hook only for as long as it runs. TPF2MP's
//! launcher started its game the same way.
//!
//! - **Windows:** the game starts suspended; the hook is loaded into it by
//!   `LoadLibraryW` on a thread of the game's own, as TPF2MP's injector
//!   does, checked to be there, and only then does the game run. When
//!   anything fails, the suspended game is ended, never left behind.
//! - **Linux:** the game starts with `LD_PRELOAD` naming the hook, for that
//!   process alone; no Steam launch option.
//! - **macOS:** not yet. The game's hardened runtime refuses libraries it
//!   did not load itself.
//!
//! The game also gets `SteamAppId`, so that it does not restart itself
//! through Steam (which would start it without the hook), and whatever
//! [`Launch::env`] holds: the launcher passes the name of its link there,
//! without which the hook does nothing. [`steam_running`] tells whether the
//! game can start at all: the launcher, like TPF2MP's, starts it only while
//! Steam runs.

#[cfg(all(unix, not(target_os = "macos")))]
use std::process::Command;
use std::{
    path::{Path, PathBuf},
    time::Duration,
};

use thiserror::Error;

#[cfg(windows)]
mod windows;

/// Transport Fever 3's Steam app ID, which the game reads from `SteamAppId`.
pub const STEAM_APP_ID: &str = "3493540";

/// The hook library's file name, in the package next to the launcher.
pub const HOOK_FILE: &str = if cfg!(windows) {
    "tpf3mp_hook.dll"
} else if cfg!(target_os = "macos") {
    "libtpf3mp_hook.dylib"
} else {
    "libtpf3mp_hook.so"
};

/// How long a game started on Windows stays suspended for TPF3-MP's hook
/// to say it is ready, before it is let run anyway.
pub const HOOK_READY_WAIT: Duration = Duration::from_secs(30);

/// A game to start, and the hook to start it with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Launch {
    /// The game's executable.
    pub exe: PathBuf,
    pub args: Vec<String>,
    /// The hook library, by its full path.
    pub hook: PathBuf,
    /// Variables for the game's environment, on top of the launcher's own.
    pub env: Vec<(String, String)>,
    /// How long the game stays suspended for the hook to say it is ready
    /// (Windows): [`HOOK_READY_WAIT`] for TPF3-MP's hook. Test rigs that
    /// load a stand-in library, which never says so, give it less.
    pub ready_wait: Duration,
}

/// Whether this system can start the game with the hook in it.
pub const SUPPORTED: bool = cfg!(any(windows, all(unix, not(target_os = "macos"))));

/// A game started with the hook in it. Dropping this leaves the game
/// running.
#[derive(Debug)]
pub struct Started {
    pub pid: u32,
    process: Process,
}

impl Started {
    /// Whether the game is still running.
    pub fn is_running(&mut self) -> bool {
        self.process.exit_code().is_none()
    }

    /// The game's exit code, once it has ended.
    pub fn exit_code(&mut self) -> Option<i64> {
        self.process.exit_code()
    }

    /// Ends the game, for test rigs that stop what they started.
    pub fn kill(&mut self) {
        self.process.kill();
    }
}

#[cfg(windows)]
use windows::Process;

/// The game's process, reaped once it ends.
#[cfg(all(unix, not(target_os = "macos")))]
#[derive(Debug)]
struct Process(std::process::Child);

#[cfg(all(unix, not(target_os = "macos")))]
impl Process {
    /// `-1` for a game ended by a signal; a game that cannot be asked
    /// counts as ended.
    fn exit_code(&mut self) -> Option<i64> {
        match self.0.try_wait() {
            Ok(None) => None,
            Ok(Some(status)) => Some(status.code().map_or(-1, i64::from)),
            Err(_) => Some(-1),
        }
    }

    fn kill(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// No game is started on macOS yet.
#[cfg(target_os = "macos")]
#[derive(Debug)]
enum Process {}

#[cfg(target_os = "macos")]
impl Process {
    fn exit_code(&mut self) -> Option<i64> {
        match *self {}
    }

    fn kill(&mut self) {
        match *self {}
    }
}

#[derive(Debug, Error)]
pub enum LaunchError {
    #[error("there is no game at {0}")]
    NoGame(PathBuf),
    #[error("the hook library {0} is missing from the TPF3-MP folder")]
    NoHook(PathBuf),
    #[error("cannot start the game: {0}")]
    Start(std::io::Error),
    #[error(
        "the game started, but the hook could not be loaded into it ({0}); it was stopped again"
    )]
    HookNotLoaded(String),
    #[error("Steam is not running: start Steam and sign in, then start the game again")]
    NoSteam,
    #[error("starting the game with the hook is not possible on this system yet")]
    Unsupported,
}

/// Starts `launch.exe` with the hook in it.
pub fn start(launch: &Launch) -> Result<Started, LaunchError> {
    if !launch.exe.is_file() {
        return Err(LaunchError::NoGame(launch.exe.clone()));
    }
    if !launch.hook.is_file() {
        return Err(LaunchError::NoHook(launch.hook.clone()));
    }
    let env = environment(launch);
    start_on_this_system(launch, &env)
}

/// Whether Steam's client is running: `Some(false)` only when the programs
/// running could be listed and Steam is not among them.
pub fn steam_running() -> Option<bool> {
    running_programs().map(|names| names.iter().any(|name| is_steam(name)))
}

/// The file process `pid` runs, where the system says: to name another
/// launcher that is running. `None` when the process is gone, or the
/// system does not tell (macOS).
pub fn process_path(pid: u32) -> Option<PathBuf> {
    process_path_on_this_system(pid)
}

#[cfg(windows)]
fn process_path_on_this_system(pid: u32) -> Option<PathBuf> {
    windows::process_path(pid)
}

#[cfg(all(unix, not(target_os = "macos")))]
fn process_path_on_this_system(pid: u32) -> Option<PathBuf> {
    std::fs::read_link(format!("/proc/{pid}/exe")).ok()
}

#[cfg(target_os = "macos")]
fn process_path_on_this_system(_pid: u32) -> Option<PathBuf> {
    None
}

/// Whether process `pid` may still run. Fails closed: a process the system
/// cannot be asked about is taken as running.
pub fn process_runs(pid: u32) -> bool {
    process_runs_on_this_system(pid)
}

#[cfg(windows)]
fn process_runs_on_this_system(pid: u32) -> bool {
    windows::process_runs(pid)
}

#[cfg(all(unix, not(target_os = "macos")))]
fn process_runs_on_this_system(pid: u32) -> bool {
    // Only "no such entry" says it is gone.
    std::fs::metadata(format!("/proc/{pid}")).map_or_else(
        |error| error.kind() != std::io::ErrorKind::NotFound,
        |_| true,
    )
}

#[cfg(target_os = "macos")]
fn process_runs_on_this_system(_pid: u32) -> bool {
    true
}

/// Steam's client, by the file name of its program in lower case.
fn is_steam(program: &str) -> bool {
    matches!(program, "steam.exe" | "steam" | "steam_osx")
}

#[cfg(windows)]
fn running_programs() -> Option<Vec<String>> {
    windows::running_programs()
}

/// The programs' names from `/proc/<pid>/comm`, in lower case.
#[cfg(all(unix, not(target_os = "macos")))]
fn running_programs() -> Option<Vec<String>> {
    let mut names = Vec::new();
    for entry in std::fs::read_dir("/proc").ok()?.flatten() {
        let pid = entry.file_name();
        if !pid
            .to_string_lossy()
            .bytes()
            .all(|byte| byte.is_ascii_digit())
        {
            continue;
        }
        if let Ok(name) = std::fs::read_to_string(entry.path().join("comm")) {
            names.push(name.trim_end().to_lowercase());
        }
    }
    Some(names)
}

#[cfg(target_os = "macos")]
fn running_programs() -> Option<Vec<String>> {
    None
}

/// The variables the game gets on top of the launcher's own.
fn environment(launch: &Launch) -> Vec<(String, String)> {
    let mut env = vec![
        ("SteamAppId".to_owned(), STEAM_APP_ID.to_owned()),
        ("SteamGameId".to_owned(), STEAM_APP_ID.to_owned()),
    ];
    env.extend(launch.env.iter().cloned());
    env
}

#[cfg(windows)]
fn start_on_this_system(launch: &Launch, env: &[(String, String)]) -> Result<Started, LaunchError> {
    windows::start(launch, env)
}

#[cfg(all(unix, not(target_os = "macos")))]
fn start_on_this_system(launch: &Launch, env: &[(String, String)]) -> Result<Started, LaunchError> {
    // Ahead of whatever else is preloaded, such as Steam's overlay.
    let mut preload = launch.hook.as_os_str().to_owned();
    if let Some(other) = std::env::var_os("LD_PRELOAD").filter(|other| !other.is_empty()) {
        preload.push(":");
        preload.push(other);
    }
    let child = Command::new(&launch.exe)
        .args(&launch.args)
        .current_dir(folder_of(&launch.exe))
        .env("LD_PRELOAD", preload)
        .envs(env.iter().map(|(name, value)| (name, value)))
        .spawn()
        .map_err(LaunchError::Start)?;
    Ok(Started {
        pid: child.id(),
        process: Process(child),
    })
}

#[cfg(target_os = "macos")]
fn start_on_this_system(
    _launch: &Launch,
    _env: &[(String, String)],
) -> Result<Started, LaunchError> {
    Err(LaunchError::Unsupported)
}

#[cfg_attr(target_os = "macos", allow(dead_code))]
fn folder_of(exe: &Path) -> &Path {
    exe.parent().unwrap_or_else(|| Path::new("."))
}

/// The game's executable in its folder: the name it has on this system,
/// or else the one executable there. `None` when that is not clear.
pub fn find_executable(game_dir: &Path) -> Option<PathBuf> {
    // TransportFever3.exe on Windows (build 40408). TODO(TF3 release):
    // confirm the name on Linux and macOS.
    let names: &[&str] = if cfg!(windows) {
        &["TransportFever3.exe", "Transport Fever 3.exe"]
    } else {
        &["TransportFever3", "Transport Fever 3"]
    };
    if let Some(found) = names
        .iter()
        .map(|name| game_dir.join(name))
        .find(|path| path.is_file())
    {
        return Some(found);
    }
    if !cfg!(windows) {
        return None;
    }
    let executables: Vec<PathBuf> = std::fs::read_dir(game_dir)
        .ok()?
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.is_file()
                && path
                    .extension()
                    .is_some_and(|extension| extension.eq_ignore_ascii_case("exe"))
                && !path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(is_helper)
        })
        .collect();
    match executables.as_slice() {
        [only] => Some(only.clone()),
        _ => None,
    }
}

/// Programs games ship next to themselves that are not the game.
fn is_helper(name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    ["crash", "redist", "setup", "unins", "launcher", "helper"]
        .iter()
        .any(|word| name.contains(word))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_running_program_is_named_by_its_file() {
        let own = process_path(std::process::id());
        if cfg!(target_os = "macos") {
            assert_eq!(own, None, "macOS does not say");
        } else {
            let own = own.expect("this test's own file");
            assert_eq!(
                own.canonicalize().unwrap(),
                std::env::current_exe().unwrap().canonicalize().unwrap()
            );
        }
        assert_eq!(process_path(u32::MAX - 1), None, "no such process");
    }

    #[test]
    fn the_game_is_found_by_its_name_or_as_the_only_program() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(find_executable(dir.path()), None);
        if cfg!(windows) {
            std::fs::write(dir.path().join("CrashReporter.exe"), "").unwrap();
            std::fs::write(dir.path().join("tpf3.exe"), "").unwrap();
            assert_eq!(
                find_executable(dir.path()),
                Some(dir.path().join("tpf3.exe"))
            );
            std::fs::write(dir.path().join("other.exe"), "").unwrap();
            assert_eq!(find_executable(dir.path()), None, "two could be the game");
        }
        let named = if cfg!(windows) {
            "TransportFever3.exe"
        } else {
            "TransportFever3"
        };
        std::fs::write(dir.path().join(named), "").unwrap();
        assert_eq!(find_executable(dir.path()), Some(dir.path().join(named)));
    }

    #[test]
    fn nothing_starts_without_the_game_and_the_hook() {
        let dir = tempfile::tempdir().unwrap();
        let launch = Launch {
            exe: dir.path().join("missing"),
            args: Vec::new(),
            hook: dir.path().join(HOOK_FILE),
            env: Vec::new(),
            ready_wait: HOOK_READY_WAIT,
        };
        assert!(matches!(start(&launch), Err(LaunchError::NoGame(_))));
        let exe = dir.path().join("game");
        std::fs::write(&exe, "").unwrap();
        let launch = Launch { exe, ..launch };
        assert!(matches!(start(&launch), Err(LaunchError::NoHook(_))));
    }

    #[test]
    fn steam_is_known_by_its_program() {
        for steam in ["steam.exe", "steam", "steam_osx"] {
            assert!(is_steam(steam), "{steam}");
        }
        for other in ["steamwebhelper.exe", "steamservice.exe", "steam.sh", ""] {
            assert!(!is_steam(other), "{other}");
        }
        // This test's own program is running, so the list can be read here.
        assert!(
            cfg!(target_os = "macos") || running_programs().is_some_and(|names| !names.is_empty())
        );
    }

    #[test]
    fn the_game_is_told_its_steam_app_and_the_launchers_variables() {
        let launch = Launch {
            exe: PathBuf::from("game"),
            args: Vec::new(),
            hook: PathBuf::from("hook"),
            env: vec![("TPF3MP_GAME_LINK".into(), "tpf3mp.default".into())],
            ready_wait: HOOK_READY_WAIT,
        };
        let env = environment(&launch);
        assert!(env.contains(&("SteamAppId".into(), STEAM_APP_ID.into())));
        assert!(env.contains(&("TPF3MP_GAME_LINK".into(), "tpf3mp.default".into())));
    }

    #[test]
    fn a_process_that_exited_no_longer_runs() {
        assert!(process_runs(std::process::id()));
        let mut child = if cfg!(windows) {
            std::process::Command::new("cmd")
                .args(["/C", "exit"])
                .spawn()
        } else {
            std::process::Command::new("true").spawn()
        }
        .unwrap();
        let pid = child.id();
        child.wait().unwrap();
        // Reaped and its handle closed: gone, unless macOS cannot tell.
        drop(child);
        assert_eq!(process_runs(pid), cfg!(target_os = "macos"));
    }
}
