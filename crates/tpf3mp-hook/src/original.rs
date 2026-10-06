//! The game's own machine code, run in a test: the harness that proves a
//! hook exact against the function it stands in for (`crate::fastindex`,
//! `crate::emission`).
//!
//! After silver2127's Big Maps for TPF2 (tpf2-bigmap), which proved each
//! patch against the original code by running it in Unicorn; this harness
//! came from TPF3-MP's big-map line. Here the original instructions are
//! read from the executable (READ-ONLY, as `tf3_static_proof.rs` reads it),
//! relocated with iced-x86's block encoder into an executable page of the
//! test process, and every call or data reference that leaves the copied
//! range is pointed at a stand-in the test supplies: an allocator, an
//! assert's stub, a copy of a constant from `.rdata`. A reference the test
//! did not map is refused, so nothing runs that the test did not name.
//!
//! The tests skip when the executable is absent (CI) or is another build.
//! `TPF3MP_TF3_EXE` points them at the game.

#![allow(unsafe_code, clippy::unwrap_used, clippy::expect_used)]

use std::path::PathBuf;

use iced_x86::{
    BlockEncoder, BlockEncoderOptions, Decoder, DecoderOptions, InstructionBlock, OpKind,
};
use tpf3mp_hookcore::pe::PeHeaders;
use tpf3mp_hookcore::profile::{BuildIdentity, Profile};

const DEFAULT_EXES: [&str; 2] = [
    r"C:\Program Files (x86)\Steam\steamapps\common\Transport Fever 3\TransportFever3.exe",
    r"F:\SteamLibrary\steamapps\common\Transport Fever 3\TransportFever3.exe",
];

/// The release executable, read once.
pub struct Exe {
    image: Vec<u8>,
    pe: PeHeaders,
}

impl Exe {
    /// The executable the compiled profile names, or `None` (and a note)
    /// when it is absent or another build.
    pub fn load() -> Option<Self> {
        let path = std::env::var_os("TPF3MP_TF3_EXE")
            .map(PathBuf::from)
            .or_else(|| {
                DEFAULT_EXES
                    .iter()
                    .map(PathBuf::from)
                    .find(|path| path.exists())
            });
        let Some(path) = path.filter(|path| path.exists()) else {
            eprintln!("skipping: the game's executable is not present (expected in CI)");
            return None;
        };
        let profile = Profile::from_toml(crate::BUILT_IN_PROFILES[0].1).unwrap();
        let identity = BuildIdentity::of_file(&path).unwrap();
        if profile.verify_identity(&identity).is_err() {
            eprintln!("skipping: {} is another build", path.display());
            return None;
        }
        let image = std::fs::read(&path).unwrap();
        let pe = PeHeaders::parse(&image).unwrap();
        Some(Self { image, pe })
    }

    /// `len` bytes at `rva`, from whichever section holds them.
    pub fn bytes(&self, rva: u64, len: usize) -> &[u8] {
        self.try_bytes(rva, len)
            .unwrap_or_else(|| panic!("{rva:#x}+{len} is in no section's raw data"))
    }

    /// `len` bytes at `rva`, or `None` outside every section's raw data.
    pub fn try_bytes(&self, rva: u64, len: usize) -> Option<&[u8]> {
        for section in &self.pe.sections {
            let start = u64::from(section.virtual_address);
            let end = start + u64::from(section.size_of_raw_data);
            if rva >= start && rva + len as u64 <= end {
                let raw = section.raw(&self.image).unwrap();
                let at = (rva - start) as usize;
                return Some(&raw[at..at + len]);
            }
        }
        None
    }
}

/// A read-write-execute region for the test's code and data: code grows
/// from the start, data from the end, so every RIP-relative reference
/// stays within reach.
pub struct Page {
    base: usize,
    len: usize,
    code: usize,
    data: usize,
}

impl Page {
    pub fn new() -> Self {
        use windows_sys::Win32::System::Memory::{
            MEM_COMMIT, MEM_RESERVE, PAGE_EXECUTE_READWRITE, VirtualAlloc,
        };
        let len = 0x10000;
        // SAFETY: a fresh region for the test's own code and data.
        let base = unsafe {
            VirtualAlloc(
                std::ptr::null(),
                len,
                MEM_COMMIT | MEM_RESERVE,
                PAGE_EXECUTE_READWRITE,
            )
        } as usize;
        assert_ne!(base, 0);
        Self {
            base,
            len,
            code: 0,
            data: len,
        }
    }

    /// Copies `bytes` to the data end, aligned to 16; returns the address.
    pub fn data(&mut self, bytes: &[u8]) -> usize {
        self.data = (self.data - bytes.len()) & !15;
        assert!(self.data >= self.code, "the page is full");
        let at = self.base + self.data;
        // SAFETY: inside the region, which nothing else uses.
        unsafe { std::ptr::copy_nonoverlapping(bytes.as_ptr(), at as *mut u8, bytes.len()) };
        at
    }

    /// Where the next code goes.
    pub fn here(&self) -> usize {
        self.base + self.code
    }

    /// Copies `bytes` as code; returns the address.
    pub fn code(&mut self, bytes: &[u8]) -> usize {
        let at = self.here();
        self.code += bytes.len();
        assert!(self.code <= self.data, "the page is full");
        // SAFETY: inside the region, which nothing else uses.
        unsafe { std::ptr::copy_nonoverlapping(bytes.as_ptr(), at as *mut u8, bytes.len()) };
        at
    }

    /// Relocates the game's `len` bytes of code at `rva` to here: branches
    /// and data references inside the range follow the copy; every one
    /// leaving it goes where `map` says (an RVA in, an address out), and
    /// one `map` does not name is refused. Returns the copy's address and
    /// each instruction's offset in the copy, by its offset in the game.
    pub fn relocate(
        &mut self,
        exe: &Exe,
        rva: u64,
        len: usize,
        map: &dyn Fn(u64) -> Option<usize>,
    ) -> (usize, Vec<(usize, usize)>) {
        let code = exe.bytes(rva, len);
        let mut decoder = Decoder::with_ip(64, code, rva, DecoderOptions::NONE);
        let inside = |target: u64| (rva..rva + len as u64).contains(&target);
        let mut instructions = Vec::new();
        while decoder.can_decode() {
            let mut insn = decoder.decode();
            assert!(
                !insn.is_invalid(),
                "an invalid instruction at {:#x}",
                insn.ip()
            );
            if insn.op_count() > 0 && insn.op0_kind() == OpKind::NearBranch64 {
                let target = insn.near_branch_target();
                if !inside(target) {
                    let to = map(target).unwrap_or_else(|| {
                        panic!("{:#x} branches to unmapped {target:#x}", insn.ip())
                    });
                    insn.set_near_branch64(to as u64);
                }
            }
            if insn.is_ip_rel_memory_operand() {
                let target = insn.ip_rel_memory_address();
                if !inside(target) {
                    let to = map(target)
                        .unwrap_or_else(|| panic!("{:#x} reads unmapped {target:#x}", insn.ip()));
                    insn.set_memory_displacement64(to as u64);
                }
            }
            instructions.push(insn);
        }
        let at = self.here() as u64;
        let block = InstructionBlock::new(&instructions, at);
        let result = BlockEncoder::encode(
            64,
            block,
            BlockEncoderOptions::RETURN_NEW_INSTRUCTION_OFFSETS,
        )
        .unwrap_or_else(|error| panic!("relocating {rva:#x}: {error}"));
        let offsets = instructions
            .iter()
            .zip(&result.new_instruction_offsets)
            .map(|(insn, &new)| ((insn.ip() - rva) as usize, new as usize))
            .collect();
        let start = self.code(&result.code_buffer);
        assert_eq!(start as u64, at);
        (start, offsets)
    }
}

impl Default for Page {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for Page {
    fn drop(&mut self) {
        use windows_sys::Win32::System::Memory::{MEM_RELEASE, VirtualFree};
        let _ = self.len;
        // SAFETY: the region VirtualAlloc gave; nothing runs in it now.
        unsafe { VirtualFree(self.base as *mut _, 0, MEM_RELEASE) };
    }
}
