//! Starts the game with a library loaded into it before its code runs, the
//! way the launcher does, from the command line - for development and the
//! release-day checks (`docs/DAY_ONE.md`).
//!
//! ```text
//! tpf3mp-launch --exe "C:\...\TransportFever3.exe" --hook target\release\tpf3mp_hook.dll \
//!     --env TPF3MP_GAME_LINK=dev --arg --some-game-flag
//! ```
//!
//! Prints the game's pid and returns; the game keeps running.

use std::path::PathBuf;
use std::process::ExitCode;

use tpf3mp_launch::{HOOK_READY_WAIT, Launch, start};

const USAGE: &str = "\
usage: tpf3mp-launch --exe <game exe> --hook <library> [--env NAME=VALUE]... [--arg ARG]...

  --exe <path>       the game's executable
  --hook <path>      the library to load into it before it runs
  --env NAME=VALUE   a variable for the game's environment (repeatable)
  --arg ARG          an argument for the game (repeatable)";

fn main() -> ExitCode {
    let mut exe = None;
    let mut hook = None;
    let mut env = Vec::new();
    let mut args = Vec::new();

    let mut it = std::env::args().skip(1);
    while let Some(flag) = it.next() {
        let Some(value) = it.next() else {
            eprintln!("{flag} needs a value\n\n{USAGE}");
            return ExitCode::FAILURE;
        };
        match flag.as_str() {
            "--exe" => exe = Some(PathBuf::from(value)),
            "--hook" => hook = Some(PathBuf::from(value)),
            "--env" => match value.split_once('=') {
                Some((name, val)) => env.push((name.to_owned(), val.to_owned())),
                None => {
                    eprintln!("--env takes NAME=VALUE\n\n{USAGE}");
                    return ExitCode::FAILURE;
                }
            },
            "--arg" => args.push(value),
            other => {
                eprintln!("unknown flag {other}\n\n{USAGE}");
                return ExitCode::FAILURE;
            }
        }
    }
    let (Some(exe), Some(hook)) = (exe, hook) else {
        eprintln!("{USAGE}");
        return ExitCode::FAILURE;
    };

    match start(&Launch {
        exe,
        args,
        hook,
        env,
        ready_wait: HOOK_READY_WAIT,
    }) {
        Ok(started) => {
            println!("started pid {}", started.pid);
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("{error}");
            ExitCode::FAILURE
        }
    }
}
