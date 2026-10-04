//! The stop tool takes the next click at once in the room's game
//! (docs/HOOKS.md, "The build tools", the stop tool).
//!
//! Build 40408's stop tool (`UI::StreetTerminalBuilder`, the stop builder and
//! the signal and waypoint builder) waits for each click's answer before it
//! takes another: `MousePressed` (0x594f50) returns at once while its busy
//! byte (`+0x2c8`) is set, sets it just before it queues the click's
//! `WorldBuildProposal` (0x595305), and only the command's callback clears
//! it again (its `<lambda_1>::_Do_call`, 0x59777d), once the simulation has
//! applied the command. `Step` (0x595f30) shows no preview while it is set
//! either. In single player that wait is the build itself, and the stop is
//! there when the tool is ready again. In a room the click's own build is
//! refused at its apply ([`crate::builds`]) and the room orders the stop for
//! every game later, so the wait buys nothing, and every click made in it is
//! dropped by the tool without a word: the player clicks a row of stops and
//! only the first is placed.
//!
//! So in the room's game, once the stop tool's click is queued, the hook
//! clears the busy byte, as the callback would: the tool takes the next click
//! straight away, and each click becomes its own `PlaceStop`, handed to the
//! room in click order (the GUI pairs each click with the proposal it saw,
//! `tpf3mp_sim.script.lua`), which every game applies in the room's order.
//! Nothing is built here, so lockstep is untouched. The callback still comes
//! later, with the refused build's empty result, as it did before, and clears
//! the byte again.
//!
//! The tool is found from the click's own callback: `MousePressed` builds it
//! on its stack as a `std::function` whose impl pointer (`+0x38`) points to
//! itself and whose functor holds the tool (`+0x8`), as `{vftable, this}`.
//! The byte is cleared only when all of that reads so and the byte is 1, as
//! `MousePressed` just set it; anything else leaves the tool as the game
//! left it (fail closed: the tool waits, as before).

#![allow(unsafe_code)]

use std::sync::atomic::{AtomicUsize, Ordering};

use crate::modules::Memory;

pub use crate::build_data::native::stoptool::STOP_ADD_CALL;
pub use crate::build_data::native::stoptool::STOP_BUSY_SET;

pub use crate::build_data::native::stoptool::BUSY;
pub use crate::build_data::native::stoptool::FUNCTION_IMPL;
pub use crate::build_data::native::stoptool::FUNCTOR_TOOL;

/// Where `Add` returns to from the stop tool's call; 0 when the profile
/// does not name it (or the busy byte), and the tool waits as before.
static STOP_RETURN: AtomicUsize = AtomicUsize::new(0);

/// Notes the stop tool's call of `Add`, at `call` (its absolute address).
pub fn set_call(call: usize) {
    STOP_RETURN.store(call + 5, Ordering::Release);
}

/// Whether a call of `Add` returning to `return_address` is the stop tool's.
pub fn is_stop_tool(return_address: usize) -> bool {
    let stop = STOP_RETURN.load(Ordering::Acquire);
    stop != 0 && return_address == stop
}

/// Memory the hook may write a byte of.
pub trait Writable: Memory {
    /// Writes `value` at `address`; false when it cannot.
    fn write_u8(&self, address: usize, value: u8) -> bool;
}

impl Writable for crate::modules::Process {
    fn write_u8(&self, address: usize, value: u8) -> bool {
        if address == 0 || !crate::image::readable(address, 1) {
            return false;
        }
        // SAFETY: a committed byte of the tool object on the game's heap,
        // which `MousePressed` wrote on this thread just before (it read 1,
        // checked by the caller); the main thread is the tool's only user.
        unsafe { (address as *mut u8).write_volatile(value) };
        true
    }
}

/// The stop tool whose click queued with `callback`, if the callback reads
/// as `MousePressed` builds it and the tool is waiting on it.
pub fn waiting_tool(memory: &dyn Memory, callback: usize) -> Result<usize, String> {
    let function = memory
        .read(callback, FUNCTION_IMPL + 8)
        .ok_or("the click's callback does not read")?;
    let impl_pointer = crate::modules::u64_at(&function, FUNCTION_IMPL) as usize;
    if impl_pointer != callback {
        return Err(format!(
            "the click's callback keeps its functor elsewhere ({impl_pointer:#x})"
        ));
    }
    let tool = crate::modules::u64_at(&function, FUNCTOR_TOOL) as usize;
    let busy = memory
        .read(tool.wrapping_add(BUSY), 1)
        .filter(|_| tool != 0)
        .ok_or("the stop tool does not read")?;
    if busy[0] != 1 {
        return Err(format!("the stop tool's busy byte is {}", busy[0]));
    }
    Ok(tool)
}

/// Lets the stop tool `tool`, found by [`waiting_tool`] before its click
/// was queued, take its next click. Returns why not, when it does not.
pub fn free(memory: &dyn Writable, tool: usize) -> Result<(), String> {
    match memory.read(tool + BUSY, 1) {
        Some(busy) if busy[0] == 1 => {}
        Some(busy) => return Err(format!("the stop tool's busy byte is {}", busy[0])),
        None => return Err("the stop tool does not read".into()),
    }
    if !memory.write_u8(tool + BUSY, 0) {
        return Err("the stop tool's busy byte cannot be written".into());
    }
    Ok(())
}

/// Releases are logged once, and failures once each, so a playtest's log
/// says the tool is free without a line a click.
static LOGGED: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());

fn log_once(line: String) {
    let new = match LOGGED.lock() {
        Ok(mut logged) if !logged.contains(&line) && logged.len() < 16 => {
            logged.push(line.clone());
            true
        }
        _ => false,
    };
    if new {
        crate::log::line(&line);
    }
}

/// Called by the add's detour before the game queues a player's build in
/// the room's game that returns to `return_address`, with its `callback`:
/// the stop tool waiting on that click, if it is its click. Read before
/// `Add` runs: `Add` takes the callback by value and destroys it.
pub fn before_add(return_address: usize, callback: usize) -> Option<usize> {
    if !is_stop_tool(return_address) {
        return None;
    }
    match waiting_tool(&crate::modules::Process, callback) {
        Ok(tool) => Some(tool),
        Err(why) => {
            log_once(format!(
                "stop tool: waits for its click's answer, as the game does: {why}"
            ));
            None
        }
    }
}

/// Called by the add's detour once the stop tool's click is queued, with
/// the tool [`before_add`] found.
pub fn after_add(tool: usize) {
    log_once(match free(&crate::modules::Process, tool) {
        Ok(()) => "stop tool: takes the next click at once; the room places each stop".to_owned(),
        Err(why) => format!("stop tool: waits for its click's answer, as the game does: {why}"),
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::collections::BTreeMap;

    /// Bytes at addresses, as the game's stack and heap hold them.
    #[derive(Default)]
    struct Fake {
        bytes: RefCell<BTreeMap<usize, u8>>,
    }

    impl Fake {
        fn put(&self, address: usize, bytes: &[u8]) {
            let mut map = self.bytes.borrow_mut();
            for (i, b) in bytes.iter().enumerate() {
                map.insert(address + i, *b);
            }
        }
        fn byte(&self, address: usize) -> Option<u8> {
            self.bytes.borrow().get(&address).copied()
        }
    }

    impl Memory for Fake {
        fn read(&self, address: usize, len: usize) -> Option<Vec<u8>> {
            (0..len).map(|i| self.byte(address + i)).collect()
        }
    }

    impl Writable for Fake {
        fn write_u8(&self, address: usize, value: u8) -> bool {
            if self.byte(address).is_none() {
                return false;
            }
            self.put(address, &[value]);
            true
        }
    }

    const TOOL: usize = 0x0275_221e_7ea0;
    const CALLBACK: usize = 0x00e5_ff68;
    const RETURN: usize = 0x1_4059_54ed;

    /// The stop tool as build 40408 has it: `MousePressed` drops a click
    /// while the busy byte is set, sets it, builds the click's callback on
    /// its stack and queues the click with it; the callback alone clears
    /// the byte, once the simulation applied the command.
    struct StopTool<'a> {
        memory: &'a Fake,
        /// The clicks queued, by where they were.
        queued: Vec<&'static str>,
        /// Whether the hook's add detour runs after each queued click.
        hooked: bool,
    }

    impl<'a> StopTool<'a> {
        fn new(memory: &'a Fake, hooked: bool) -> Self {
            memory.put(TOOL + BUSY, &[0]);
            StopTool {
                memory,
                queued: Vec::new(),
                hooked,
            }
        }

        fn mouse_pressed(&mut self, at: &'static str) {
            if self.memory.byte(TOOL + BUSY) != Some(0) {
                return; // 0x594f95: the click is dropped
            }
            self.memory.put(TOOL + BUSY, &[1]); // 0x595305
            // The callback, `{vftable, this}`, impl pointer at +0x38 to itself.
            self.memory.put(CALLBACK, &[0; FUNCTION_IMPL + 8]);
            self.memory.put(CALLBACK, &0x1_436b_6eb0u64.to_le_bytes());
            self.memory
                .put(CALLBACK + FUNCTOR_TOOL, &(TOOL as u64).to_le_bytes());
            self.memory
                .put(CALLBACK + FUNCTION_IMPL, &(CALLBACK as u64).to_le_bytes());
            // The add's detour finds the tool before Add runs ...
            let found = self.hooked.then(|| waiting_tool(self.memory, CALLBACK));
            self.queued.push(at); // CommandList::Add, which destroys the callback
            self.memory
                .put(CALLBACK + FUNCTION_IMPL, &0u64.to_le_bytes());
            // ... and frees it once the click is queued.
            if let Some(found) = found {
                assert_eq!(found, Ok(TOOL), "the hook finds the waiting tool");
                assert_eq!(free(self.memory, TOOL), Ok(()));
            }
        }

        /// The command's callback, after the simulation applied it.
        fn answered(&mut self) {
            self.memory.put(TOOL + BUSY, &[0]); // 0x59777d
        }
    }

    #[test]
    fn several_quick_stop_clicks_each_queue_in_order() {
        // As the game is: clicks before the first's answer are dropped.
        let memory = Fake::default();
        let mut tool = StopTool::new(&memory, false);
        for at in ["a", "b", "c", "d"] {
            tool.mouse_pressed(at);
        }
        assert_eq!(
            tool.queued,
            ["a"],
            "the game drops the clicks after the first"
        );
        tool.answered();
        tool.mouse_pressed("e");
        assert_eq!(tool.queued, ["a", "e"]);

        // In the room's game: every click queues, in order, answers or not.
        let memory = Fake::default();
        let mut tool = StopTool::new(&memory, true);
        for at in ["a", "b", "c", "d"] {
            tool.mouse_pressed(at);
        }
        assert_eq!(tool.queued, ["a", "b", "c", "d"]);
        // The late answers (the refused builds') change nothing.
        tool.answered();
        tool.mouse_pressed("e");
        tool.answered();
        tool.answered();
        tool.mouse_pressed("f");
        assert_eq!(tool.queued, ["a", "b", "c", "d", "e", "f"]);
        assert_eq!(memory.byte(TOOL + BUSY), Some(0));
    }

    #[test]
    fn the_tool_is_left_waiting_unless_its_click_reads_as_the_games() {
        let memory = Fake::default();
        // Nothing there.
        assert!(waiting_tool(&memory, CALLBACK).is_err());
        assert!(free(&memory, TOOL).is_err());
        // A callback whose functor is on the heap, not in itself.
        memory.put(CALLBACK, &[0; FUNCTION_IMPL + 8]);
        memory.put(CALLBACK + FUNCTOR_TOOL, &(TOOL as u64).to_le_bytes());
        memory.put(CALLBACK + FUNCTION_IMPL, &0x1234u64.to_le_bytes());
        memory.put(TOOL + BUSY, &[1]);
        assert!(
            waiting_tool(&memory, CALLBACK)
                .unwrap_err()
                .contains("keeps its functor elsewhere")
        );
        assert_eq!(memory.byte(TOOL + BUSY), Some(1), "left as it was");
        // A tool that is not waiting (the byte not as MousePressed set it).
        memory.put(CALLBACK + FUNCTION_IMPL, &(CALLBACK as u64).to_le_bytes());
        memory.put(TOOL + BUSY, &[7]);
        assert!(
            waiting_tool(&memory, CALLBACK)
                .unwrap_err()
                .contains("busy byte is 7")
        );
        assert!(free(&memory, TOOL).unwrap_err().contains("busy byte is 7"));
        assert_eq!(memory.byte(TOOL + BUSY), Some(7), "left as it was");
        // No tool.
        memory.put(CALLBACK + FUNCTOR_TOOL, &0u64.to_le_bytes());
        assert!(waiting_tool(&memory, CALLBACK).is_err());
        // As the game builds it: found, then freed.
        memory.put(CALLBACK + FUNCTOR_TOOL, &(TOOL as u64).to_le_bytes());
        memory.put(TOOL + BUSY, &[1]);
        assert_eq!(waiting_tool(&memory, CALLBACK), Ok(TOOL));
        assert_eq!(free(&memory, TOOL), Ok(()));
        assert_eq!(memory.byte(TOOL + BUSY), Some(0));
        // Answered already (the byte back at 0): nothing to free.
        assert!(free(&memory, TOOL).is_err());
    }

    #[test]
    fn only_the_stop_tools_call_is_the_stop_tools() {
        assert!(!is_stop_tool(RETURN), "not before the profile names it");
        set_call(RETURN - 5);
        assert!(is_stop_tool(RETURN));
        assert!(!is_stop_tool(0x1_4054_3b2a), "the module editor's");
        assert!(!is_stop_tool(0));
    }
}
