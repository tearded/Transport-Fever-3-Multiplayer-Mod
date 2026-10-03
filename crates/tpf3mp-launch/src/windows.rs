//! Starts the game suspended, loads the hook into it and lets it run: the
//! way TPF2MP's injector started Transport Fever 2 (`--launch`).
//!
//! The hook is loaded by `LoadLibraryW`, run on a thread the launcher
//! creates in the game's process with the hook's path as its argument.
//! `kernel32.dll` lies at the same address in every process of a session,
//! so the launcher's own `LoadLibraryW` is the game's. Both are 64-bit.

#![allow(unsafe_code)]

use std::{ffi::c_void, os::windows::ffi::OsStrExt, path::Path};

use windows_sys::Win32::{
    Foundation::{CloseHandle, HANDLE, INVALID_HANDLE_VALUE, WAIT_OBJECT_0, WAIT_TIMEOUT},
    System::{
        Diagnostics::{
            Debug::WriteProcessMemory,
            ToolHelp::{
                CreateToolhelp32Snapshot, MODULEENTRY32W, Module32FirstW, Module32NextW,
                PROCESSENTRY32W, Process32FirstW, Process32NextW, TH32CS_SNAPMODULE,
                TH32CS_SNAPPROCESS,
            },
        },
        LibraryLoader::{GetModuleHandleW, GetProcAddress},
        Memory::{
            MEM_COMMIT, MEM_RELEASE, MEM_RESERVE, PAGE_READWRITE, VirtualAllocEx, VirtualFreeEx,
        },
        Threading::{
            CREATE_SUSPENDED, CREATE_UNICODE_ENVIRONMENT, CreateEventW, CreateProcessW,
            CreateRemoteThread, GetExitCodeProcess, GetExitCodeThread, LPTHREAD_START_ROUTINE,
            PROCESS_INFORMATION, ResumeThread, STARTUPINFOW, TerminateProcess, WaitForSingleObject,
        },
    },
};

use crate::{Launch, LaunchError, Started, folder_of};

/// How long loading the hook may take.
const LOAD_TIMEOUT_MS: u32 = 30_000;

/// The event the hook sets once it is ready (`tpf3mp_ipc::hook_ready_event`).
struct ReadyEvent(HANDLE);

impl ReadyEvent {
    fn create(pid: u32) -> Option<Self> {
        let name: Vec<u16> = tpf3mp_ipc::hook_ready_event(pid)
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect();
        // SAFETY: a NUL-terminated name; manual reset, not set.
        let event = unsafe { CreateEventW(std::ptr::null(), 1, 0, name.as_ptr()) };
        (!event.is_null()).then_some(Self(event))
    }

    /// Whether the hook said it is ready within `timeout_ms`.
    fn wait(&self, timeout_ms: u32) -> bool {
        // SAFETY: this event's handle.
        unsafe { WaitForSingleObject(self.0, timeout_ms) == WAIT_OBJECT_0 }
    }
}

impl Drop for ReadyEvent {
    fn drop(&mut self) {
        // SAFETY: the handle is this one's, closed once, here.
        unsafe {
            CloseHandle(self.0);
        }
    }
}

/// A process started suspended. Dropped before it was let run, it is ended:
/// a game half started is never left behind.
struct Suspended {
    info: PROCESS_INFORMATION,
    running: bool,
}

impl Drop for Suspended {
    fn drop(&mut self) {
        // SAFETY: the handles are the ones CreateProcessW returned, each
        // closed once, here, unless the process handle was handed on.
        unsafe {
            if !self.running {
                TerminateProcess(self.info.hProcess, 1);
            }
            CloseHandle(self.info.hThread);
            if !self.info.hProcess.is_null() {
                CloseHandle(self.info.hProcess);
            }
        }
    }
}

/// A game let run, by its process's handle.
#[derive(Debug)]
pub(crate) struct Process(HANDLE);

// SAFETY: a process handle may be used and closed from any thread.
unsafe impl Send for Process {}

impl Process {
    /// A game that cannot be asked counts as ended, with `-1`.
    pub(crate) fn exit_code(&mut self) -> Option<i64> {
        // SAFETY: a process handle this owns; waiting no time only asks.
        if unsafe { WaitForSingleObject(self.0, 0) } == WAIT_TIMEOUT {
            return None;
        }
        let mut code = 0u32;
        // SAFETY: the same handle, and a place for the code.
        let asked = unsafe { GetExitCodeProcess(self.0, &mut code) };
        Some(if asked == 0 { -1 } else { i64::from(code) })
    }

    pub(crate) fn kill(&mut self) {
        // SAFETY: a process handle this owns, with the rights
        // CreateProcessW gives its creator.
        unsafe {
            TerminateProcess(self.0, 1);
            WaitForSingleObject(self.0, LOAD_TIMEOUT_MS);
        }
    }
}

impl Drop for Process {
    fn drop(&mut self) {
        // SAFETY: the handle is this one's, closed once, here.
        unsafe {
            CloseHandle(self.0);
        }
    }
}

pub(crate) fn start(launch: &Launch, env: &[(String, String)]) -> Result<Started, LaunchError> {
    let mut process = create_suspended(launch, env)?;
    // Made before the hook loads, so the hook finds it however fast it is.
    let ready = ReadyEvent::create(process.info.dwProcessId);
    load_hook(&process, &launch.hook)?;
    // The game stays suspended until the hook has armed what must be in
    // place before the game runs its first line (the main menu's entry):
    // otherwise the game could load its main menu first, and it would be
    // the game's own. A hook that never says so lets the game run anyway,
    // after the wait.
    if let Some(ready) = &ready {
        ready.wait(millis(launch.ready_wait));
    }
    // SAFETY: the main thread's handle, from CreateProcessW.
    if unsafe { ResumeThread(process.info.hThread) } == u32::MAX {
        return Err(LaunchError::HookNotLoaded(format!(
            "the game would not resume: {}",
            std::io::Error::last_os_error()
        )));
    }
    process.running = true;
    let handle = std::mem::replace(&mut process.info.hProcess, std::ptr::null_mut());
    Ok(Started {
        pid: process.info.dwProcessId,
        process: Process(handle),
    })
}

fn create_suspended(launch: &Launch, env: &[(String, String)]) -> Result<Suspended, LaunchError> {
    let application = wide(launch.exe.as_os_str());
    let mut command_line = wide(command_line(launch).as_ref());
    let folder = wide(folder_of(&launch.exe).as_os_str());
    let block = environment_block(env);
    // SAFETY: zeroed is a valid STARTUPINFOW once its size is set.
    let mut startup: STARTUPINFOW = unsafe { std::mem::zeroed() };
    startup.cb = u32::try_from(std::mem::size_of::<STARTUPINFOW>()).unwrap_or(u32::MAX);
    // SAFETY: zeroed is a valid PROCESS_INFORMATION for CreateProcessW to fill.
    let mut info: PROCESS_INFORMATION = unsafe { std::mem::zeroed() };
    // SAFETY: every string is NUL-terminated UTF-16 that outlives the call;
    // the command line is a mutable buffer, as CreateProcessW requires; the
    // environment block is double-NUL-terminated UTF-16, as
    // CREATE_UNICODE_ENVIRONMENT says.
    let created = unsafe {
        CreateProcessW(
            application.as_ptr(),
            command_line.as_mut_ptr(),
            std::ptr::null(),
            std::ptr::null(),
            0,
            CREATE_SUSPENDED | CREATE_UNICODE_ENVIRONMENT,
            block.as_ptr().cast::<c_void>(),
            folder.as_ptr(),
            &startup,
            &mut info,
        )
    };
    if created == 0 {
        return Err(LaunchError::Start(std::io::Error::last_os_error()));
    }
    Ok(Suspended {
        info,
        running: false,
    })
}

/// Runs `LoadLibraryW(hook)` in the process, and checks the hook is there.
fn load_hook(process: &Suspended, hook: &Path) -> Result<(), LaunchError> {
    let failed = |what: &str| {
        LaunchError::HookNotLoaded(format!("{what}: {}", std::io::Error::last_os_error()))
    };
    let path = wide(hook.as_os_str());
    let bytes = path.len() * std::mem::size_of::<u16>();
    let handle = process.info.hProcess;
    // SAFETY: memory in the suspended process, of the path's size, freed
    // below whatever happens.
    let remote = unsafe {
        VirtualAllocEx(
            handle,
            std::ptr::null(),
            bytes,
            MEM_COMMIT | MEM_RESERVE,
            PAGE_READWRITE,
        )
    };
    if remote.is_null() {
        return Err(failed("no memory in the game's process"));
    }
    let result = (|| {
        let mut written = 0;
        // SAFETY: `remote` holds `bytes`; `path` is that long.
        let wrote = unsafe {
            WriteProcessMemory(
                handle,
                remote,
                path.as_ptr().cast::<c_void>(),
                bytes,
                &mut written,
            )
        };
        if wrote == 0 || written != bytes {
            return Err(failed("cannot write the hook's path"));
        }
        let kernel32 = wide("kernel32.dll".as_ref());
        // SAFETY: kernel32 is loaded in every process; the name is NUL
        // terminated.
        let load = unsafe {
            GetProcAddress(
                GetModuleHandleW(kernel32.as_ptr()),
                c"LoadLibraryW".as_ptr().cast(),
            )
        };
        let Some(load) = load else {
            return Err(failed("no LoadLibraryW"));
        };
        // SAFETY: LoadLibraryW takes one pointer and returns a handle: the
        // shape of a thread's start routine, whose exit code gets the
        // handle's low half.
        let routine: LPTHREAD_START_ROUTINE = Some(unsafe {
            std::mem::transmute::<
                unsafe extern "system" fn() -> isize,
                unsafe extern "system" fn(*mut c_void) -> u32,
            >(load)
        });
        // SAFETY: the routine is LoadLibraryW, its argument the path written
        // above.
        let thread: HANDLE = unsafe {
            CreateRemoteThread(
                handle,
                std::ptr::null(),
                0,
                routine,
                remote,
                0,
                std::ptr::null_mut(),
            )
        };
        if thread.is_null() {
            return Err(failed("cannot start a thread in the game's process"));
        }
        // SAFETY: the thread handle from CreateRemoteThread, closed once.
        let exit = unsafe {
            let waited = WaitForSingleObject(thread, LOAD_TIMEOUT_MS);
            let mut exit = 0;
            let got = GetExitCodeThread(thread, &mut exit);
            CloseHandle(thread);
            if waited != WAIT_OBJECT_0 || got == 0 {
                return Err(failed("loading the hook did not finish"));
            }
            exit
        };
        // The exit code is the handle's low half, which can be 0 for a
        // handle that is not: then the process's modules decide.
        if exit != 0 || has_module(process.info.dwProcessId, hook) {
            Ok(())
        } else {
            Err(LaunchError::HookNotLoaded(format!(
                "Windows would not load {}",
                hook.display()
            )))
        }
    })();
    // SAFETY: the memory allocated above, released once.
    unsafe {
        VirtualFreeEx(handle, remote, 0, MEM_RELEASE);
    }
    result
}

/// Whether process `pid` has `module` loaded.
fn has_module(pid: u32, module: &Path) -> bool {
    let want = module.as_os_str().to_string_lossy().to_lowercase();
    // SAFETY: a snapshot handle, closed below; the entry's size is set
    // before the first call, as Module32FirstW requires.
    unsafe {
        let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPMODULE, pid);
        if snapshot == INVALID_HANDLE_VALUE {
            return false;
        }
        let mut entry: MODULEENTRY32W = std::mem::zeroed();
        entry.dwSize = u32::try_from(std::mem::size_of::<MODULEENTRY32W>()).unwrap_or(u32::MAX);
        let mut found = false;
        let mut more = Module32FirstW(snapshot, &mut entry) != 0;
        while more && !found {
            let len = entry
                .szExePath
                .iter()
                .position(|&unit| unit == 0)
                .unwrap_or(0);
            found = String::from_utf16_lossy(&entry.szExePath[..len]).to_lowercase() == want;
            more = Module32NextW(snapshot, &mut entry) != 0;
        }
        CloseHandle(snapshot);
        found
    }
}

/// The file names of the programs running, in lower case; `None` when
/// Windows would not list them.
pub(crate) fn running_programs() -> Option<Vec<String>> {
    // SAFETY: a snapshot handle, closed below; the entry's size is set
    // before the first call, as Process32FirstW requires.
    unsafe {
        let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
        if snapshot == INVALID_HANDLE_VALUE {
            return None;
        }
        let mut entry: PROCESSENTRY32W = std::mem::zeroed();
        entry.dwSize = u32::try_from(std::mem::size_of::<PROCESSENTRY32W>()).unwrap_or(u32::MAX);
        let mut names = Vec::new();
        let mut more = Process32FirstW(snapshot, &mut entry) != 0;
        while more {
            let len = entry
                .szExeFile
                .iter()
                .position(|&unit| unit == 0)
                .unwrap_or(entry.szExeFile.len());
            names.push(String::from_utf16_lossy(&entry.szExeFile[..len]).to_lowercase());
            more = Process32NextW(snapshot, &mut entry) != 0;
        }
        CloseHandle(snapshot);
        Some(names)
    }
}

/// The command line: the program, quoted, then its arguments.
fn command_line(launch: &Launch) -> std::ffi::OsString {
    let mut line = std::ffi::OsString::from("\"");
    line.push(launch.exe.as_os_str());
    line.push("\"");
    for arg in &launch.args {
        line.push(" ");
        if arg.is_empty() || arg.contains([' ', '\t', '"']) {
            line.push(format!("\"{}\"", arg.replace('"', "\\\"")));
        } else {
            line.push(arg);
        }
    }
    line
}

/// The launcher's environment with `env` on top, as a block of
/// `NAME=value` UTF-16 strings sorted by name without regard to case, each
/// ending in NUL, and the block in a second NUL.
fn environment_block(env: &[(String, String)]) -> Vec<u16> {
    let mut vars: Vec<(String, String)> = std::env::vars_os()
        .map(|(name, value)| {
            (
                name.to_string_lossy().into_owned(),
                value.to_string_lossy().into_owned(),
            )
        })
        .filter(|(name, _)| !env.iter().any(|(ours, _)| ours.eq_ignore_ascii_case(name)))
        .collect();
    vars.extend(env.iter().cloned());
    vars.sort_by_key(|(name, _)| name.to_uppercase());
    let mut block = Vec::new();
    for (name, value) in vars {
        block.extend(format!("{name}={value}").encode_utf16());
        block.push(0);
    }
    block.push(0);
    block
}

/// The full path of the program process `pid` runs.
pub(crate) fn process_path(pid: u32) -> Option<std::path::PathBuf> {
    use std::os::windows::ffi::OsStringExt;
    use windows_sys::Win32::System::Threading::{
        OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
        QueryFullProcessImageNameW,
    };
    // SAFETY: a handle opened for querying only, closed once; the buffer's
    // length is passed in and the length written read back.
    unsafe {
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if handle.is_null() {
            return None;
        }
        let mut buffer = vec![0u16; 32_768];
        let mut len = u32::try_from(buffer.len()).unwrap_or(u32::MAX);
        let got =
            QueryFullProcessImageNameW(handle, PROCESS_NAME_WIN32, buffer.as_mut_ptr(), &mut len);
        CloseHandle(handle);
        if got == 0 {
            return None;
        }
        buffer.truncate(len as usize);
        Some(std::ffi::OsString::from_wide(&buffer).into())
    }
}

/// `wait` in milliseconds for a wait call, short of `INFINITE`.
fn millis(wait: std::time::Duration) -> u32 {
    u32::try_from(wait.as_millis()).map_or(u32::MAX - 1, |ms| ms.min(u32::MAX - 1))
}

/// Whether process `pid` may still run: it exists and has no exit code.
/// A process that exited may still be there while another program holds a
/// handle to it. Fails closed: one that cannot be asked about (access
/// denied) is taken as running. As the hook's `worlds::running`.
pub(crate) fn process_runs(pid: u32) -> bool {
    use windows_sys::Win32::{
        Foundation::{ERROR_INVALID_PARAMETER, GetLastError},
        System::Threading::{GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION},
    };
    /// The exit code of a process still running.
    const STILL_ACTIVE: u32 = 259;
    // SAFETY: a handle opened for querying only, closed once.
    unsafe {
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if handle.is_null() {
            return GetLastError() != ERROR_INVALID_PARAMETER;
        }
        let mut code = 0u32;
        let asked = GetExitCodeProcess(handle, &raw mut code);
        CloseHandle(handle);
        asked == 0 || code == STILL_ACTIVE
    }
}

fn wide(text: &std::ffi::OsStr) -> Vec<u16> {
    text.encode_wide().chain(std::iter::once(0)).collect()
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use windows_sys::Win32::System::Threading::{
        OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SYNCHRONIZE,
    };

    use super::*;

    /// How long the tests' games wait for a stand-in hook, which never says
    /// it is ready.
    const STAND_IN_READY_WAIT: std::time::Duration = std::time::Duration::from_millis(300);

    fn system(name: &str) -> PathBuf {
        let root = std::env::var_os("SystemRoot").unwrap_or_else(|| "C:\\Windows".into());
        PathBuf::from(root).join("System32").join(name)
    }

    /// Waits for process `pid` to end and returns its exit code.
    fn wait_for_exit(pid: u32) -> Option<u32> {
        // SAFETY: a handle to a process we started, closed once.
        unsafe {
            let handle = OpenProcess(
                PROCESS_SYNCHRONIZE | PROCESS_QUERY_LIMITED_INFORMATION,
                0,
                pid,
            );
            if handle.is_null() {
                return None;
            }
            WaitForSingleObject(handle, 30_000);
            let mut code = 0;
            let got = GetExitCodeProcess(handle, &mut code);
            CloseHandle(handle);
            (got != 0).then_some(code)
        }
    }

    #[test]
    fn the_game_waits_for_the_hooks_ready_event_and_no_longer() {
        use windows_sys::Win32::System::Threading::{EVENT_MODIFY_STATE, OpenEventW, SetEvent};
        // A process id no game has: the event is this test's alone.
        let pid = u32::MAX - std::process::id();
        let ready = ReadyEvent::create(pid).unwrap();
        assert!(!ready.wait(50), "not set: the wait runs out");
        let name: Vec<u16> = tpf3mp_ipc::hook_ready_event(pid)
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect();
        // As the hook sets it, from another thread.
        std::thread::spawn(move || unsafe {
            let event = OpenEventW(EVENT_MODIFY_STATE, 0, name.as_ptr());
            assert!(!event.is_null());
            SetEvent(event);
            CloseHandle(event);
        })
        .join()
        .unwrap();
        let begun = std::time::Instant::now();
        assert!(ready.wait(10_000), "set: the game runs");
        assert!(begun.elapsed() < std::time::Duration::from_secs(1));
    }

    #[test]
    fn a_program_starts_with_the_library_in_it_and_runs() {
        // version.dll stands in for the hook; cmd waits a second, then exits
        // with the code given.
        let args = "/c ping -n 2 127.0.0.1 >nul & exit 7";
        let mut started = start(
            &Launch {
                exe: system("cmd.exe"),
                args: args.split(' ').map(str::to_owned).collect(),
                hook: system("version.dll"),
                env: vec![("TPF3MP_GAME_LINK".into(), "test".into())],
                ready_wait: STAND_IN_READY_WAIT,
            },
            &[],
        )
        .unwrap();
        assert!(started.is_running(), "it runs");
        assert_eq!(started.exit_code(), None);
        assert_eq!(wait_for_exit(started.pid), Some(7), "it ran to its end");
        assert!(!started.is_running(), "and it has ended");
        assert_eq!(started.exit_code(), Some(7));
    }

    #[test]
    fn a_rig_can_end_what_it_started() {
        let args = "/c ping -n 30 127.0.0.1 >nul";
        let mut started = start(
            &Launch {
                exe: system("cmd.exe"),
                args: args.split(' ').map(str::to_owned).collect(),
                hook: system("version.dll"),
                env: Vec::new(),
                ready_wait: STAND_IN_READY_WAIT,
            },
            &[],
        )
        .unwrap();
        assert!(started.is_running());
        started.kill();
        assert!(!started.is_running(), "ended at once");
    }

    #[test]
    fn the_game_stays_suspended_for_as_long_as_the_launch_says() {
        let wait = std::time::Duration::from_secs(2);
        let begun = std::time::Instant::now();
        let started = start(
            &Launch {
                exe: system("cmd.exe"),
                args: vec!["/c".into(), "exit".into(), "0".into()],
                hook: system("version.dll"),
                env: Vec::new(),
                ready_wait: wait,
            },
            &[],
        )
        .unwrap();
        assert!(begun.elapsed() >= wait, "held for the wait it was given");
        assert_eq!(wait_for_exit(started.pid), Some(0));
    }

    #[test]
    fn a_wait_is_never_infinite() {
        use windows_sys::Win32::System::Threading::INFINITE;
        assert_eq!(millis(std::time::Duration::ZERO), 0);
        assert_eq!(millis(std::time::Duration::from_secs(30)), 30_000);
        assert!(millis(std::time::Duration::MAX) < INFINITE);
        assert!(millis(std::time::Duration::from_millis(u64::from(INFINITE))) < INFINITE);
    }

    #[test]
    fn a_library_that_does_not_load_stops_the_program() {
        let dir = tempfile::tempdir().unwrap();
        let not_a_library = dir.path().join("tpf3mp_hook.dll");
        std::fs::write(&not_a_library, b"not a library").unwrap();
        let refused = start(
            &Launch {
                exe: system("cmd.exe"),
                args: vec!["/c".into(), "exit".into(), "0".into()],
                hook: not_a_library,
                env: Vec::new(),
                ready_wait: STAND_IN_READY_WAIT,
            },
            &[],
        );
        assert!(
            matches!(refused, Err(LaunchError::HookNotLoaded(_))),
            "{refused:?}"
        );
    }

    #[test]
    fn the_command_line_quotes_what_needs_it() {
        let launch = Launch {
            exe: PathBuf::from(r"C:\Games\Transport Fever 3\TransportFever3.exe"),
            args: vec!["-window".into(), "a b".into()],
            hook: PathBuf::new(),
            env: Vec::new(),
            ready_wait: crate::HOOK_READY_WAIT,
        };
        assert_eq!(
            command_line(&launch),
            r#""C:\Games\Transport Fever 3\TransportFever3.exe" -window "a b""#
        );
    }

    #[test]
    fn the_environment_has_ours_on_top_sorted() {
        let block = environment_block(&[("PATH".into(), "ours".into())]);
        let text = String::from_utf16(&block).unwrap();
        let vars: Vec<&str> = text.split('\0').filter(|var| !var.is_empty()).collect();
        assert_eq!(
            vars.iter()
                .filter(|var| var.to_uppercase().starts_with("PATH="))
                .count(),
            1
        );
        assert!(vars.contains(&"PATH=ours"));
        let mut sorted = vars.clone();
        sorted.sort_by_key(|var| var.split('=').next().unwrap_or("").to_uppercase());
        assert_eq!(vars, sorted);
        assert!(block.ends_with(&[0, 0]));
    }
}
