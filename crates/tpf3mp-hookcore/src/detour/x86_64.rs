//! The x86-64 inline detour engine (see the module docs in `mod.rs`).

#![allow(unsafe_code)]

use iced_x86::{
    BlockEncoder, BlockEncoderOptions, Decoder, DecoderOptions, FlowControl, Instruction,
    InstructionBlock,
};

use super::{DetourError, SpliceHook, sys};

/// `FF 25 00000000` + an absolute 8-byte target: `jmp [rip+0]`, reachable
/// anywhere in the address space.
const ABS_JMP_LEN: usize = 14;
/// `E9` + a signed 32-bit relative displacement: `jmp rel32`.
const REL_JMP_LEN: usize = 5;
/// Enough bytes to decode any reasonable prologue we would steal.
const PROLOGUE_SCAN: usize = 32;

/// An installed inline detour. Dropping it (or calling [`InlineDetour::detach`])
/// restores the original bytes.
pub struct InlineDetour {
    target: *mut u8,
    steal_len: usize,
    original: Vec<u8>,
    trampoline: sys::ExecBuffer,
    active: bool,
}

impl InlineDetour {
    /// Redirects the function at `target` to `detour`, returning a handle whose
    /// [`trampoline`](Self::trampoline) calls the original.
    ///
    /// # Safety
    ///
    /// - `target` must point at a real function in this process's code.
    /// - No thread may execute `target` (or the bytes just after its prologue)
    ///   during this call. Install before the target's first run, or park its
    ///   threads first.
    /// - `detour` must be a function that is ABI-compatible with `target`.
    pub unsafe fn install(target: *mut u8, detour: *const u8) -> Result<Self, DetourError> {
        let target_addr = target as usize;
        let detour_addr = detour as usize;

        // A near detour is reached with a 5-byte relative jump; otherwise a
        // 14-byte absolute jump, which needs a longer prologue to steal.
        let rel_ok = fits_i32((detour_addr as i128) - ((target_addr + REL_JMP_LEN) as i128));
        let min_patch = if rel_ok { REL_JMP_LEN } else { ABS_JMP_LEN };

        // Only as many bytes as the target's memory region holds: a function
        // near the region's end is read no further.
        let scan = sys::readable(target_addr, PROLOGUE_SCAN);
        // SAFETY: `scan` bytes at `target` are readable code.
        let code = unsafe { std::slice::from_raw_parts(target, scan) };
        let (instrs, steal_len) = decode_prologue(code, target_addr as u64, min_patch)?;
        let original = code[..steal_len].to_vec();

        // Trampoline: the relocated prologue, then an absolute jump back to the
        // first instruction after it. Near the target, so relocated
        // RIP-relative operands still reach what they address.
        let trampoline = sys::alloc_near(target_addr, steal_len + ABS_JMP_LEN + 64)?;
        let tramp_addr = trampoline.as_mut_ptr() as u64;
        let mut body = encode_block(&instrs, tramp_addr)?;
        body.extend_from_slice(&abs_jmp((target_addr + steal_len) as u64));
        if body.len() > trampoline.len() {
            return Err(DetourError::Encode(
                "relocated prologue does not fit the trampoline".to_owned(),
            ));
        }
        // SAFETY: `trampoline` is a fresh writable buffer of at least
        // `body.len()` bytes.
        unsafe {
            std::ptr::copy_nonoverlapping(body.as_ptr(), trampoline.as_mut_ptr(), body.len());
        }
        sys::make_executable(&trampoline)?;
        // SAFETY: the trampoline is now executable code of `body.len()` bytes.
        unsafe {
            sys::flush_icache(trampoline.as_mut_ptr(), body.len());
        }

        // Patch the target with the jump to the detour.
        let patch = if rel_ok {
            rel_jmp_patch(target_addr + REL_JMP_LEN, detour_addr, steal_len)
        } else {
            abs_jmp_patch(detour_addr as u64, steal_len)
        };
        // SAFETY: the caller guarantees `target` is quiescent for `patch.len()`
        // bytes, which equals `steal_len`.
        unsafe {
            sys::write_code(target, &patch)?;
        }

        Ok(Self {
            target,
            steal_len,
            original,
            trampoline,
            active: true,
        })
    }

    /// The original function: run the stolen prologue, then continue in place.
    /// Cast this to the target's function type to call through.
    pub fn trampoline(&self) -> *const u8 {
        self.trampoline.as_ptr()
    }

    pub fn target(&self) -> *mut u8 {
        self.target
    }

    pub fn steal_len(&self) -> usize {
        self.steal_len
    }

    /// Removes the detour, restoring the original bytes.
    ///
    /// # Safety
    ///
    /// As with install, no thread may execute `target` during the call.
    pub unsafe fn detach(mut self) -> Result<(), DetourError> {
        // SAFETY: forwarded under the same quiescence contract.
        unsafe { self.restore() }
    }

    unsafe fn restore(&mut self) -> Result<(), DetourError> {
        if self.active {
            // SAFETY: writing the saved original bytes back over the patch.
            unsafe {
                sys::write_code(self.target, &self.original)?;
            }
            self.active = false;
        }
        Ok(())
    }
}

impl Drop for InlineDetour {
    fn drop(&mut self) {
        // SAFETY: same contract as install; a failure here can only mean the
        // OS refused to restore protection, which we cannot report from drop.
        let _ = unsafe { self.restore() };
    }
}

// SAFETY: the raw pointers are owned addresses of this process's own memory; a
// detour handle can be moved between threads. Sending it does not by itself make
// concurrent patching safe - the install/detach contract still applies.
unsafe impl Send for InlineDetour {}

fn fits_i32(value: i128) -> bool {
    i32::try_from(value).is_ok()
}

/// Decodes instructions at `ip` until at least `min` bytes are covered, on an
/// instruction boundary. Refuses anything that cannot be relocated verbatim: an
/// undecodable byte, or an instruction that is not straight-line
/// ([`FlowControl::Next`]). RIP-relative operands are fine - the block encoder
/// fixes their displacements - so this is exactly the "relocate RIP-relative,
/// refuse branches" rule.
fn decode_prologue(
    code: &[u8],
    ip: u64,
    min: usize,
) -> Result<(Vec<Instruction>, usize), DetourError> {
    let mut decoder = Decoder::with_ip(64, code, ip, DecoderOptions::NONE);
    let mut instrs = Vec::new();
    let mut covered = 0usize;
    while covered < min {
        if !decoder.can_decode() {
            return Err(DetourError::UnsupportedPrologue {
                reason: format!("only {covered} bytes decode before the patch needs {min}"),
            });
        }
        let insn = decoder.decode();
        if insn.is_invalid() {
            return Err(DetourError::UnsupportedPrologue {
                reason: format!("undecodable byte at offset {covered}"),
            });
        }
        if insn.flow_control() != FlowControl::Next {
            return Err(DetourError::UnsupportedPrologue {
                reason: format!(
                    "prologue branches at offset {covered} ({:?}, {:?})",
                    insn.mnemonic(),
                    insn.flow_control()
                ),
            });
        }
        instrs.push(insn);
        covered = (decoder.ip() - ip) as usize;
    }
    Ok((instrs, covered))
}

fn encode_block(instrs: &[Instruction], rip: u64) -> Result<Vec<u8>, DetourError> {
    let block = InstructionBlock::new(instrs, rip);
    match BlockEncoder::encode(64, block, BlockEncoderOptions::NONE) {
        Ok(result) => Ok(result.code_buffer),
        Err(error) => Err(DetourError::Encode(error.to_string())),
    }
}

fn abs_jmp(to: u64) -> Vec<u8> {
    let mut bytes = vec![0xFF, 0x25, 0x00, 0x00, 0x00, 0x00];
    bytes.extend_from_slice(&to.to_le_bytes());
    bytes
}

fn abs_jmp_patch(to: u64, steal_len: usize) -> Vec<u8> {
    let mut bytes = abs_jmp(to);
    bytes.resize(steal_len, 0xCC);
    bytes
}

fn rel_jmp_patch(after: usize, to: usize, steal_len: usize) -> Vec<u8> {
    // The caller checked this fits; the cast cannot lose information.
    let displacement = (to as i128 - after as i128) as i32;
    let mut bytes = vec![0xE9];
    bytes.extend_from_slice(&displacement.to_le_bytes());
    bytes.resize(steal_len, 0xCC);
    bytes
}

/// `E8` + a signed 32-bit displacement: `call rel32`.
const CALL_REL32_LEN: usize = 5;
/// `mov rax, imm64` (`48 B8` + 8 bytes) then `jmp rax` (`FF E0`).
const STUB_LEN: usize = 12;

/// One `call rel32` instruction redirected to another function, through a
/// stub near it: the rest of the calling function runs unchanged, and every
/// other caller of the old callee still reaches it. Dropping it (or
/// [`CallRedirect::detach`]) restores the original displacement.
pub struct CallRedirect {
    site: *mut u8,
    original: [u8; 4],
    _stub: sys::ExecBuffer,
    active: bool,
}

impl CallRedirect {
    /// Makes the `call` at `site` call `to` instead of `expected`.
    ///
    /// Refuses unless `site` holds a 5-byte `call rel32` whose target is
    /// exactly `expected`: a build whose code moved is never patched wrong.
    ///
    /// # Safety
    ///
    /// - `site` must point at an instruction in this process's code.
    /// - No thread may execute the instruction at `site` during this call.
    /// - `to` must be ABI-compatible with `expected`.
    pub unsafe fn install(
        site: *mut u8,
        expected: usize,
        to: *const u8,
    ) -> Result<Self, DetourError> {
        let site_addr = site as usize;
        if sys::readable(site_addr, CALL_REL32_LEN) < CALL_REL32_LEN {
            return Err(DetourError::UnsupportedPrologue {
                reason: "the call site is not readable".to_owned(),
            });
        }
        // SAFETY: five readable bytes at `site`.
        let code = unsafe { std::slice::from_raw_parts(site, CALL_REL32_LEN) };
        if code[0] != 0xE8 {
            return Err(DetourError::UnsupportedPrologue {
                reason: format!("expected a call rel32 at the site, found {:#04x}", code[0]),
            });
        }
        let mut original = [0u8; 4];
        original.copy_from_slice(&code[1..5]);
        let rel = i32::from_le_bytes(original);
        let callee = (site_addr as i128) + CALL_REL32_LEN as i128 + i128::from(rel);
        if callee != expected as i128 {
            return Err(DetourError::UnsupportedPrologue {
                reason: format!("the call targets {callee:#x}, not the expected {expected:#x}"),
            });
        }

        let stub = sys::alloc_near(site_addr, STUB_LEN)?;
        let mut body = vec![0x48, 0xB8];
        body.extend_from_slice(&(to as u64).to_le_bytes());
        body.extend_from_slice(&[0xFF, 0xE0]);
        // SAFETY: `stub` is a fresh writable buffer of at least STUB_LEN bytes.
        unsafe {
            std::ptr::copy_nonoverlapping(body.as_ptr(), stub.as_mut_ptr(), body.len());
        }
        sys::make_executable(&stub)?;
        // SAFETY: the stub is now executable code of `body.len()` bytes.
        unsafe {
            sys::flush_icache(stub.as_mut_ptr(), body.len());
        }
        let delta = (stub.as_ptr() as i128) - (site_addr as i128 + CALL_REL32_LEN as i128);
        let Ok(delta) = i32::try_from(delta) else {
            return Err(DetourError::Alloc(0));
        };
        // SAFETY: the caller guarantees the call at `site` is not executing;
        // only its four displacement bytes change.
        unsafe {
            sys::write_code(site.add(1), &delta.to_le_bytes())?;
        }
        Ok(Self {
            site,
            original,
            _stub: stub,
            active: true,
        })
    }

    /// Restores the original call.
    ///
    /// # Safety
    ///
    /// As with install, no thread may execute the call during this.
    pub unsafe fn detach(mut self) -> Result<(), DetourError> {
        // SAFETY: forwarded under the same contract.
        unsafe { self.restore() }
    }

    unsafe fn restore(&mut self) -> Result<(), DetourError> {
        if self.active {
            // SAFETY: writing the saved displacement back.
            unsafe {
                sys::write_code(self.site.add(1), &self.original)?;
            }
            self.active = false;
        }
        Ok(())
    }
}

impl Drop for CallRedirect {
    fn drop(&mut self) {
        // SAFETY: same contract as install.
        let _ = unsafe { self.restore() };
    }
}

/// The most bytes a splice steals: whole instructions, verified byte for
/// byte, none of which may be relocated.
const SPLICE_MAX_STEAL: usize = 32;

/// A hook in the middle of a function: at `site`, a `jmp rel32` into a stub
/// near it that saves every general-purpose register and the flags, calls
/// a [`SpliceHook`] with them (as a [`SavedRegs`] block on the stack), puts
/// them back, runs the stolen instructions verbatim and jumps back to the
/// instruction after them. The volatile `xmm0`-`xmm5` are saved too, so the
/// only thing the function can see is what the hook did to memory, or to
/// the block. Dropping it (or [`Splice::detach`]) restores the site.
///
/// The stolen instructions run unchanged at another address, so they must
/// not depend on their own: a RIP-relative operand, a branch, a call or a
/// return in them is refused. The caller knows the site's register and
/// frame state (that is the point of a mid-function hook) and states the
/// bytes it expects there; a site whose bytes differ is refused and left
/// alone. Nothing may branch into the stolen bytes past their first, which
/// the caller establishes from the disassembly.
pub struct Splice {
    site: *mut u8,
    original: Vec<u8>,
    _stub: sys::ExecBuffer,
    active: bool,
}

impl Splice {
    /// Splices `hook` in at `site`, whose next `expected.len()` bytes must
    /// be exactly `expected`; the first `steal` of them (at least 5, whole
    /// instructions) are replaced by the jump and run from the stub.
    ///
    /// # Safety
    ///
    /// - `site` must point at an instruction boundary in this process's
    ///   code, and no thread may execute the site's bytes during this call.
    /// - `hook` must uphold [`SpliceHook`]'s contract: it never unwinds, and
    ///   what it changes through the block is what the code after the site
    ///   can bear.
    /// - Nothing branches into `site+1..site+steal`.
    pub unsafe fn install(
        site: *mut u8,
        expected: &[u8],
        steal: usize,
        hook: SpliceHook,
    ) -> Result<Self, DetourError> {
        let site_addr = site as usize;
        if !(REL_JMP_LEN..=SPLICE_MAX_STEAL).contains(&steal) || steal > expected.len() {
            return Err(DetourError::UnsupportedPrologue {
                reason: format!(
                    "a splice steals between {REL_JMP_LEN} and {SPLICE_MAX_STEAL} of the bytes it expects, not {steal} of {}",
                    expected.len()
                ),
            });
        }
        if sys::readable(site_addr, expected.len()) < expected.len() {
            return Err(DetourError::UnsupportedPrologue {
                reason: "the site is not readable".to_owned(),
            });
        }
        // SAFETY: `expected.len()` readable bytes at `site`.
        let found = unsafe { std::slice::from_raw_parts(site, expected.len()) };
        if found != expected {
            return Err(DetourError::UnsupportedPrologue {
                reason: format!(
                    "the site holds {} where {} was expected",
                    hex(found),
                    hex(expected)
                ),
            });
        }
        let stolen = &expected[..steal];
        check_splice_stolen(stolen, site_addr as u64)?;

        let body = splice_stub(hook as usize as u64, stolen, (site_addr + steal) as u64);
        let stub = sys::alloc_near(site_addr, body.len())?;
        // SAFETY: `stub` is a fresh writable buffer of at least `body.len()`
        // bytes.
        unsafe {
            std::ptr::copy_nonoverlapping(body.as_ptr(), stub.as_mut_ptr(), body.len());
        }
        sys::make_executable(&stub)?;
        // SAFETY: the stub is now executable code of `body.len()` bytes.
        unsafe {
            sys::flush_icache(stub.as_mut_ptr(), body.len());
        }
        let delta = (stub.as_ptr() as i128) - (site_addr as i128 + REL_JMP_LEN as i128);
        if i32::try_from(delta).is_err() {
            return Err(DetourError::Alloc(0));
        }
        let patch = rel_jmp_patch(site_addr + REL_JMP_LEN, stub.as_ptr() as usize, steal);
        // SAFETY: the caller guarantees the site is quiescent for `steal`
        // bytes, which equals `patch.len()`.
        unsafe {
            sys::write_code(site, &patch)?;
        }
        Ok(Self {
            site,
            original: stolen.to_vec(),
            _stub: stub,
            active: true,
        })
    }

    /// Restores the site's bytes.
    ///
    /// # Safety
    ///
    /// As with install, no thread may execute the site during this.
    pub unsafe fn detach(mut self) -> Result<(), DetourError> {
        // SAFETY: forwarded under the same contract.
        unsafe { self.restore() }
    }

    unsafe fn restore(&mut self) -> Result<(), DetourError> {
        if self.active {
            // SAFETY: writing the saved original bytes back over the patch.
            unsafe {
                sys::write_code(self.site, &self.original)?;
            }
            self.active = false;
        }
        Ok(())
    }
}

impl Drop for Splice {
    fn drop(&mut self) {
        // SAFETY: same contract as install.
        let _ = unsafe { self.restore() };
    }
}

// SAFETY: as for `InlineDetour`: owned addresses of this process's memory.
unsafe impl Send for Splice {}

/// Bytes rewritten in place: an instruction's operand changed where the
/// code around it stays the game's, for a change no hook has to run for
/// (a constant, a buffer size). Restored when detached or dropped.
pub struct Rewrite {
    site: *mut u8,
    original: Vec<u8>,
    active: bool,
}

impl Rewrite {
    /// Writes `replacement` over the site after checking it holds exactly
    /// `expected`, of the same length. A site holding anything else is left
    /// alone and refused.
    ///
    /// # Safety
    ///
    /// No thread may execute the site's bytes while they are written, and
    /// `replacement` must be whole instructions in place of whole
    /// instructions, valid wherever the original ran.
    pub unsafe fn install(
        site: *mut u8,
        expected: &[u8],
        replacement: &[u8],
    ) -> Result<Self, DetourError> {
        if expected.is_empty() || expected.len() != replacement.len() {
            return Err(DetourError::UnsupportedPrologue {
                reason: format!(
                    "a rewrite replaces bytes one for one, not {} with {}",
                    expected.len(),
                    replacement.len()
                ),
            });
        }
        if sys::readable(site as usize, expected.len()) < expected.len() {
            return Err(DetourError::UnsupportedPrologue {
                reason: "the site is not readable".to_owned(),
            });
        }
        // SAFETY: `expected.len()` readable bytes at `site`.
        let found = unsafe { std::slice::from_raw_parts(site, expected.len()) };
        if found != expected {
            return Err(DetourError::UnsupportedPrologue {
                reason: format!(
                    "the site holds {} where {} was expected",
                    hex(found),
                    hex(expected)
                ),
            });
        }
        // SAFETY: the caller guarantees the site is quiescent.
        unsafe {
            sys::write_code(site, replacement)?;
        }
        Ok(Self {
            site,
            original: expected.to_vec(),
            active: true,
        })
    }

    /// Restores the site's bytes.
    ///
    /// # Safety
    ///
    /// As with install, no thread may execute the site during this.
    pub unsafe fn detach(mut self) -> Result<(), DetourError> {
        // SAFETY: forwarded under the same contract.
        unsafe { self.restore() }
    }

    unsafe fn restore(&mut self) -> Result<(), DetourError> {
        if self.active {
            // SAFETY: writing the saved original bytes back.
            unsafe {
                sys::write_code(self.site, &self.original)?;
            }
            self.active = false;
        }
        Ok(())
    }
}

impl Drop for Rewrite {
    fn drop(&mut self) {
        // SAFETY: same contract as install.
        let _ = unsafe { self.restore() };
    }
}

// SAFETY: as for `InlineDetour`: owned addresses of this process's memory.
unsafe impl Send for Rewrite {}

fn hex(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|byte| format!("{byte:02X}"))
        .collect::<Vec<_>>()
        .join(" ")
}

/// The stolen bytes must be whole instructions that run the same anywhere:
/// straight-line, and without a RIP-relative operand.
fn check_splice_stolen(stolen: &[u8], ip: u64) -> Result<(), DetourError> {
    let mut decoder = Decoder::with_ip(64, stolen, ip, DecoderOptions::NONE);
    let mut covered = 0usize;
    while covered < stolen.len() {
        if !decoder.can_decode() {
            return Err(DetourError::UnsupportedPrologue {
                reason: format!("the stolen bytes end inside an instruction at offset {covered}"),
            });
        }
        let insn = decoder.decode();
        if insn.is_invalid() {
            return Err(DetourError::UnsupportedPrologue {
                reason: format!("undecodable byte at offset {covered}"),
            });
        }
        if insn.flow_control() != FlowControl::Next {
            return Err(DetourError::UnsupportedPrologue {
                reason: format!(
                    "the stolen bytes branch at offset {covered} ({:?}, {:?})",
                    insn.mnemonic(),
                    insn.flow_control()
                ),
            });
        }
        if insn.is_ip_rel_memory_operand() {
            return Err(DetourError::UnsupportedPrologue {
                reason: format!(
                    "the stolen bytes address memory relative to rip at offset {covered} ({:?})",
                    insn.mnemonic()
                ),
            });
        }
        covered = (decoder.ip() - ip) as usize;
    }
    if covered != stolen.len() {
        return Err(DetourError::UnsupportedPrologue {
            reason: format!(
                "the stolen bytes end inside an instruction ({covered} decode, {} stolen)",
                stolen.len()
            ),
        });
    }
    Ok(())
}

/// The stub a [`Splice`] runs at its site, byte for byte:
///
/// ```text
/// push rax; push rcx; push rdx; push rbx; push rbp; push rsi; push rdi
/// push r8 .. push r15; pushfq            ; the SavedRegs block, rsp -> rflags
/// mov rbx, rsp                           ; rbx keeps the block's address
/// and rsp, -16; sub rsp, 0x80            ; aligned: 6 xmm slots + shadow space
/// vmovups [rsp+0x20+16*i], xmm_i         ; i = 0..5, the volatile xmm
/// mov rcx, rbx  (unix: mov rdi, rbx)     ; the hook's one argument
/// mov rax, hook; call rax
/// vmovups xmm_i, [rsp+0x20+16*i]
/// mov rsp, rbx; popfq; pop r15 .. pop rax
/// <the stolen instructions, verbatim>
/// jmp [rip+0]; dq resume
/// ```
///
/// Pure, so the bytes can be checked in a test.
pub fn splice_stub(hook: u64, stolen: &[u8], resume: u64) -> Vec<u8> {
    let mut code = Vec::with_capacity(160 + stolen.len());
    // push rax, rcx, rdx, rbx, rbp, rsi, rdi
    code.extend_from_slice(&[0x50, 0x51, 0x52, 0x53, 0x55, 0x56, 0x57]);
    // push r8 .. r15
    for low in 0x50u8..=0x57 {
        code.extend_from_slice(&[0x41, low]);
    }
    code.push(0x9C); // pushfq
    code.extend_from_slice(&[0x48, 0x89, 0xE3]); // mov rbx, rsp
    code.extend_from_slice(&[0x48, 0x83, 0xE4, 0xF0]); // and rsp, -16
    // sub rsp, 0x80: the imm32 form, since imm8 0x80 would be -128.
    code.extend_from_slice(&[0x48, 0x81, 0xEC, 0x80, 0x00, 0x00, 0x00]);
    // vmovups [rsp+0x20+16*i], xmm_i
    for i in 0..6u8 {
        code.extend_from_slice(&[0xC5, 0xF8, 0x11, 0x44 | (i << 3), 0x24, 0x20 + 16 * i]);
    }
    // The hook's argument register: rcx on Windows, rdi elsewhere.
    if cfg!(windows) {
        code.extend_from_slice(&[0x48, 0x89, 0xD9]); // mov rcx, rbx
    } else {
        code.extend_from_slice(&[0x48, 0x89, 0xDF]); // mov rdi, rbx
    }
    code.extend_from_slice(&[0x48, 0xB8]); // mov rax, imm64
    code.extend_from_slice(&hook.to_le_bytes());
    code.extend_from_slice(&[0xFF, 0xD0]); // call rax
    // vmovups xmm_i, [rsp+0x20+16*i]
    for i in 0..6u8 {
        code.extend_from_slice(&[0xC5, 0xF8, 0x10, 0x44 | (i << 3), 0x24, 0x20 + 16 * i]);
    }
    code.extend_from_slice(&[0x48, 0x89, 0xDC]); // mov rsp, rbx
    code.push(0x9D); // popfq
    // pop r15 .. r8
    for low in (0x58u8..=0x5F).rev() {
        code.extend_from_slice(&[0x41, low]);
    }
    // pop rdi, rsi, rbp, rbx, rdx, rcx, rax
    code.extend_from_slice(&[0x5F, 0x5E, 0x5D, 0x5B, 0x5A, 0x59, 0x58]);
    code.extend_from_slice(stolen);
    code.extend_from_slice(&abs_jmp(resume));
    code
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::super::SavedRegs;
    use super::*;

    type Fun = extern "C" fn() -> u32;

    /// Hand-assembles, into an RWX buffer, a function whose prologue is a
    /// RIP-relative load of a constant stored in the same buffer:
    ///
    /// ```text
    /// 0: 8B 05 3A 00 00 00   mov eax, [rip+0x3a]   ; -> dword at offset 0x40
    /// 6: 90 x8                nop                   ; padding to steal 14 bytes
    /// e: C3                   ret
    /// ...
    /// 40: EF BE 34 12         .dword 0x1234beef
    /// ```
    fn rip_relative_function(value: u32) -> sys::ExecBuffer {
        let mut code = [0u8; 0x80];
        code[0..6].copy_from_slice(&[0x8B, 0x05, 0x3A, 0x00, 0x00, 0x00]);
        for byte in code.iter_mut().take(0x0E).skip(6) {
            *byte = 0x90;
        }
        code[0x0E] = 0xC3;
        code[0x40..0x44].copy_from_slice(&value.to_le_bytes());
        let buffer = sys::alloc(code.len()).unwrap();
        // SAFETY: `buffer` is a fresh writable region of exactly `code.len()`
        // bytes.
        unsafe {
            std::ptr::copy_nonoverlapping(code.as_ptr(), buffer.as_mut_ptr(), code.len());
        }
        sys::make_executable(&buffer).unwrap();
        // SAFETY: the buffer now holds executable code.
        unsafe {
            sys::flush_icache(buffer.as_mut_ptr(), code.len());
        }
        buffer
    }

    extern "C" fn detour_fn() -> u32 {
        0xDEAD
    }

    #[test]
    fn hooks_and_restores_a_rip_relative_prologue() {
        let buffer = rip_relative_function(0x1234_BEEF);
        // SAFETY: the buffer holds our hand-written, ABI-`extern "C"` function.
        let target: Fun = unsafe { std::mem::transmute::<*mut u8, Fun>(buffer.as_mut_ptr()) };
        assert_eq!(target(), 0x1234_BEEF, "the fixture reads its own constant");

        let detour_ptr = (detour_fn as *const ()).cast::<u8>();
        // SAFETY: single-threaded test; the target is not running elsewhere.
        let hook = unsafe { InlineDetour::install(buffer.as_mut_ptr(), detour_ptr) }.unwrap();

        assert_eq!(target(), 0xDEAD, "the detour now runs instead");
        // SAFETY: the trampoline reproduces the original, including the
        // relocated RIP-relative load.
        let original: Fun = unsafe { std::mem::transmute::<*const u8, Fun>(hook.trampoline()) };
        assert_eq!(
            original(),
            0x1234_BEEF,
            "the relocated prologue still reads the same constant"
        );

        // SAFETY: still single-threaded.
        unsafe { hook.detach() }.unwrap();
        assert_eq!(target(), 0x1234_BEEF, "detach restores the original bytes");
    }

    #[test]
    fn refuses_a_prologue_that_branches_immediately() {
        // A buffer that starts with `ret`: it cannot be stolen.
        let buffer = sys::alloc(PROLOGUE_SCAN).unwrap();
        // SAFETY: fresh writable buffer; fill it with `ret` bytes.
        unsafe {
            std::ptr::write_bytes(buffer.as_mut_ptr(), 0xC3, PROLOGUE_SCAN);
        }
        sys::make_executable(&buffer).unwrap();
        // SAFETY: the buffer now holds executable code.
        unsafe {
            sys::flush_icache(buffer.as_mut_ptr(), PROLOGUE_SCAN);
        }
        // SAFETY: the target is never executed; install refuses before patching.
        let result = unsafe {
            InlineDetour::install(buffer.as_mut_ptr(), (detour_fn as *const ()).cast::<u8>())
        };
        assert!(matches!(
            result,
            Err(DetourError::UnsupportedPrologue { .. })
        ));
    }

    #[test]
    fn decode_prologue_relocation_adjusts_rip_displacement() {
        // Decode `mov eax,[rip+0x3a]` at one address, re-encode it at another,
        // and confirm the displacement changed so it still targets the same
        // absolute address.
        let code = [0x8B, 0x05, 0x3A, 0x00, 0x00, 0x00, 0x90, 0x90];
        let (instrs, steal) = decode_prologue(&code, 0x1000, 5).unwrap();
        assert_eq!(
            steal, 6,
            "only the RIP-relative mov is needed for a 5-byte patch"
        );
        let here = encode_block(&instrs, 0x1000).unwrap();
        let moved = encode_block(&instrs, 0x5000).unwrap();
        assert_ne!(
            here, moved,
            "moving the instruction rewrote its displacement"
        );
        assert_eq!(here[0..2], moved[0..2], "the opcode is unchanged");
    }

    #[inline(never)]
    extern "C" fn callee_old() -> u32 {
        std::hint::black_box(4)
    }

    #[inline(never)]
    extern "C" fn callee_new() -> u32 {
        std::hint::black_box(1)
    }

    /// Hand-assembles, near `callee_old`, a function that calls it:
    ///
    /// ```text
    /// 0: 48 83 EC 28      sub rsp, 0x28
    /// 4: E8 rel32         call callee_old
    /// 9: 48 83 C4 28      add rsp, 0x28
    /// d: C3               ret
    /// ```
    fn caller_of_old() -> sys::ExecBuffer {
        let old = callee_old as *const () as usize;
        let buffer = sys::alloc_near(old, 0x20).unwrap();
        let base = buffer.as_ptr() as i128;
        let rel = i32::try_from(old as i128 - (base + 9)).unwrap();
        let mut code = vec![0x48, 0x83, 0xEC, 0x28, 0xE8];
        code.extend_from_slice(&rel.to_le_bytes());
        code.extend_from_slice(&[0x48, 0x83, 0xC4, 0x28, 0xC3]);
        // SAFETY: a fresh writable buffer of 0x20 bytes.
        unsafe {
            std::ptr::copy_nonoverlapping(code.as_ptr(), buffer.as_mut_ptr(), code.len());
        }
        sys::make_executable(&buffer).unwrap();
        // SAFETY: the buffer now holds executable code.
        unsafe {
            sys::flush_icache(buffer.as_mut_ptr(), code.len());
        }
        buffer
    }

    #[test]
    fn redirects_one_call_and_restores_it() {
        let buffer = caller_of_old();
        // SAFETY: the buffer holds our hand-written extern "C" function.
        let caller: Fun = unsafe { std::mem::transmute::<*const u8, Fun>(buffer.as_ptr()) };
        assert_eq!(caller(), 4);
        // SAFETY: nothing runs the buffer's code now; callee_new has the ABI.
        let site = unsafe { buffer.as_mut_ptr().add(4) };
        let redirect = unsafe {
            CallRedirect::install(
                site,
                callee_old as *const () as usize,
                callee_new as *const u8,
            )
        }
        .unwrap();
        assert_eq!(caller(), 1, "the site calls the new function");
        assert_eq!(callee_old(), 4, "other callers still reach the old one");
        // SAFETY: as above.
        unsafe { redirect.detach() }.unwrap();
        assert_eq!(caller(), 4, "restored");
    }

    /// What the splice hook saw and did: the site's registers, recorded
    /// once, and `rcx` rewritten.
    static SEEN: Mutex<Option<SavedRegs>> = Mutex::new(None);

    unsafe extern "system" fn splice_hook(regs: *mut SavedRegs) {
        // SAFETY: the stub hands a block it just pushed on this thread's
        // stack, and holds it until the hook returns.
        let block = unsafe { &mut *regs };
        *SEEN.lock().unwrap() = Some(*block);
        block.rcx = 0x100;
    }

    /// Hand-assembles a function that sets the volatile registers to known
    /// values, then runs the site (a 7-byte `mov rax, 0x1234`), then sums:
    ///
    /// ```text
    /// 00: 48 C7 C1 11 00 00 00   mov rcx, 0x11
    /// 07: 48 C7 C2 22 00 00 00   mov rdx, 0x22
    /// 0e: 49 C7 C0 33 00 00 00   mov r8, 0x33
    /// 15: 49 C7 C3 44 00 00 00   mov r11, 0x44
    /// 1c: 48 C7 C0 34 12 00 00   mov rax, 0x1234        <- the site
    /// 23: 48 01 C8               add rax, rcx
    /// 26: 4C 01 C0               add rax, r8
    /// 29: C3                     ret
    /// ```
    fn splice_fixture() -> sys::ExecBuffer {
        let code: [u8; 0x2A] = [
            0x48, 0xC7, 0xC1, 0x11, 0x00, 0x00, 0x00, 0x48, 0xC7, 0xC2, 0x22, 0x00, 0x00, 0x00,
            0x49, 0xC7, 0xC0, 0x33, 0x00, 0x00, 0x00, 0x49, 0xC7, 0xC3, 0x44, 0x00, 0x00, 0x00,
            0x48, 0xC7, 0xC0, 0x34, 0x12, 0x00, 0x00, 0x48, 0x01, 0xC8, 0x4C, 0x01, 0xC0, 0xC3,
        ];
        let buffer = sys::alloc(code.len()).unwrap();
        // SAFETY: a fresh writable buffer of exactly `code.len()` bytes.
        unsafe {
            std::ptr::copy_nonoverlapping(code.as_ptr(), buffer.as_mut_ptr(), code.len());
        }
        sys::make_executable(&buffer).unwrap();
        // SAFETY: the buffer now holds executable code.
        unsafe {
            sys::flush_icache(buffer.as_mut_ptr(), code.len());
        }
        buffer
    }

    const SITE_BYTES: [u8; 14] = [
        0x48, 0xC7, 0xC0, 0x34, 0x12, 0x00, 0x00, 0x48, 0x01, 0xC8, 0x4C, 0x01, 0xC0, 0xC3,
    ];

    #[test]
    fn a_splice_hands_the_hook_the_sites_registers_and_takes_its_changes_back() {
        let buffer = splice_fixture();
        // SAFETY: the buffer holds our hand-written function of no arguments.
        let fun: Fun = unsafe { std::mem::transmute::<*const u8, Fun>(buffer.as_ptr()) };
        assert_eq!(fun(), 0x1234 + 0x11 + 0x33, "the fixture on its own");

        // SAFETY: nothing runs the buffer now; the hook keeps the contract.
        let site = unsafe { buffer.as_mut_ptr().add(0x1C) };
        let splice = unsafe { Splice::install(site, &SITE_BYTES, 7, splice_hook) }.unwrap();
        assert_eq!(
            fun(),
            0x1234 + 0x100 + 0x33,
            "the stolen mov ran after the hook, and the hook's rcx reached the add"
        );
        let seen = SEEN.lock().unwrap().expect("the hook ran");
        assert_eq!(seen.rcx, 0x11);
        assert_eq!(seen.rdx, 0x22);
        assert_eq!(seen.r8, 0x33);
        assert_eq!(seen.r11, 0x44);
        assert_eq!(seen.rflags & 2, 2, "bit 1 of rflags always reads as set");

        // SAFETY: as above.
        unsafe { splice.detach() }.unwrap();
        assert_eq!(
            fun(),
            0x1234 + 0x11 + 0x33,
            "detached, the site is its own again"
        );
    }

    #[test]
    fn a_rewrite_changes_an_operand_and_gives_it_back() {
        let buffer = splice_fixture();
        // SAFETY: the buffer holds our hand-written function of no arguments.
        let fun: Fun = unsafe { std::mem::transmute::<*const u8, Fun>(buffer.as_ptr()) };
        // SAFETY: nothing runs the buffer while it is written.
        let site = unsafe { buffer.as_mut_ptr().add(0x1C) };
        // mov rax,0x1234 becomes mov rax,0x4321.
        let mut faster = SITE_BYTES[..7].to_vec();
        faster[3] = 0x21;
        faster[4] = 0x43;
        let rewrite = unsafe { Rewrite::install(site, &SITE_BYTES[..7], &faster) }.unwrap();
        assert_eq!(fun(), 0x4321 + 0x11 + 0x33);
        // SAFETY: as above.
        unsafe { rewrite.detach() }.unwrap();
        assert_eq!(fun(), 0x1234 + 0x11 + 0x33);

        // Bytes it did not expect, or a length change, are refused unwritten.
        let mut wrong = SITE_BYTES[..7].to_vec();
        wrong[3] = 0x35;
        assert!(unsafe { Rewrite::install(site, &wrong, &faster) }.is_err());
        assert!(unsafe { Rewrite::install(site, &SITE_BYTES[..7], &faster[..6]) }.is_err());
        assert_eq!(fun(), 0x1234 + 0x11 + 0x33);
    }

    #[test]
    fn a_splice_refuses_bytes_it_did_not_expect_and_steals_that_cannot_move() {
        let buffer = splice_fixture();
        // SAFETY: nothing runs the buffer; every refusal happens before a write.
        let site = unsafe { buffer.as_mut_ptr().add(0x1C) };
        let mut wrong = SITE_BYTES;
        wrong[3] = 0x35;
        let mismatch = unsafe { Splice::install(site, &wrong, 7, splice_hook) };
        assert!(matches!(
            mismatch,
            Err(DetourError::UnsupportedPrologue { ref reason }) if reason.contains("expected")
        ));
        // A steal that ends inside an instruction.
        let split = unsafe { Splice::install(site, &SITE_BYTES, 6, splice_hook) };
        assert!(matches!(
            split,
            Err(DetourError::UnsupportedPrologue { .. })
        ));
        // A steal that includes the ret.
        let branch = unsafe { Splice::install(site, &SITE_BYTES, 14, splice_hook) };
        assert!(matches!(
            branch,
            Err(DetourError::UnsupportedPrologue { ref reason }) if reason.contains("branch")
        ));
        // Fewer than five bytes cannot hold the jump.
        let short = unsafe { Splice::install(site, &SITE_BYTES, 4, splice_hook) };
        assert!(matches!(
            short,
            Err(DetourError::UnsupportedPrologue { .. })
        ));
        // SAFETY: the fixture is untouched.
        let fun: Fun = unsafe { std::mem::transmute::<*const u8, Fun>(buffer.as_ptr()) };
        assert_eq!(fun(), 0x1234 + 0x11 + 0x33, "nothing was written");
    }

    #[test]
    fn a_rip_relative_steal_is_refused() {
        // `mov eax, [rip+0x3a]` then `nop`s: moving it would read elsewhere.
        let code = [0x8B, 0x05, 0x3A, 0x00, 0x00, 0x00, 0x90, 0x90];
        let refused = check_splice_stolen(&code, 0x1000);
        assert!(matches!(
            refused,
            Err(DetourError::UnsupportedPrologue { ref reason }) if reason.contains("rip")
        ));
        // Plain instructions pass.
        check_splice_stolen(&SITE_BYTES[..7], 0x1000).unwrap();
        check_splice_stolen(&SITE_BYTES[..13], 0x1000).unwrap();
    }

    #[test]
    fn the_splice_stub_is_the_documented_bytes() {
        let stub = splice_stub(
            0x1122_3344_5566_7788,
            &[0x90, 0x90, 0x90, 0x90, 0x90],
            0xAABB_CCDD,
        );
        // 7 + 16 pushes, pushfq, the frame set-up.
        let head: &[u8] = &[
            0x50, 0x51, 0x52, 0x53, 0x55, 0x56, 0x57, 0x41, 0x50, 0x41, 0x51, 0x41, 0x52, 0x41,
            0x53, 0x41, 0x54, 0x41, 0x55, 0x41, 0x56, 0x41, 0x57, 0x9C, 0x48, 0x89, 0xE3, 0x48,
            0x83, 0xE4, 0xF0, 0x48, 0x81, 0xEC, 0x80, 0x00, 0x00, 0x00,
        ];
        assert_eq!(&stub[..head.len()], head);
        let mut at = head.len();
        for i in 0..6u8 {
            assert_eq!(
                &stub[at..at + 6],
                &[0xC5, 0xF8, 0x11, 0x44 | (i << 3), 0x24, 0x20 + 16 * i],
                "vmovups [rsp+{:#x}], xmm{i}",
                0x20 + 16 * i
            );
            at += 6;
        }
        let arg: &[u8] = if cfg!(windows) {
            &[0x48, 0x89, 0xD9]
        } else {
            &[0x48, 0x89, 0xDF]
        };
        assert_eq!(&stub[at..at + 3], arg);
        at += 3;
        assert_eq!(&stub[at..at + 2], &[0x48, 0xB8]);
        assert_eq!(
            &stub[at + 2..at + 10],
            &0x1122_3344_5566_7788u64.to_le_bytes()
        );
        assert_eq!(&stub[at + 10..at + 12], &[0xFF, 0xD0]);
        at += 12;
        for i in 0..6u8 {
            assert_eq!(
                &stub[at..at + 6],
                &[0xC5, 0xF8, 0x10, 0x44 | (i << 3), 0x24, 0x20 + 16 * i]
            );
            at += 6;
        }
        let tail: &[u8] = &[
            0x48, 0x89, 0xDC, 0x9D, 0x41, 0x5F, 0x41, 0x5E, 0x41, 0x5D, 0x41, 0x5C, 0x41, 0x5B,
            0x41, 0x5A, 0x41, 0x59, 0x41, 0x58, 0x5F, 0x5E, 0x5D, 0x5B, 0x5A, 0x59, 0x58, 0x90,
            0x90, 0x90, 0x90, 0x90, 0xFF, 0x25, 0x00, 0x00, 0x00, 0x00,
        ];
        assert_eq!(&stub[at..at + tail.len()], tail);
        at += tail.len();
        assert_eq!(&stub[at..], &0xAABB_CCDDu64.to_le_bytes());
        // The block's size is what SavedRegs::rsp adds.
        assert_eq!(std::mem::size_of::<SavedRegs>(), 16 * 8);
    }

    #[test]
    fn refuses_a_site_that_is_not_the_expected_call() {
        let buffer = caller_of_old();
        // SAFETY: nothing runs the buffer's code now.
        let site = unsafe { buffer.as_mut_ptr().add(4) };
        let wrong_callee = unsafe {
            CallRedirect::install(
                site,
                callee_new as *const () as usize,
                callee_new as *const u8,
            )
        };
        assert!(wrong_callee.is_err(), "the call targets another function");
        // SAFETY: as above.
        let not_a_call = unsafe {
            CallRedirect::install(
                buffer.as_mut_ptr(),
                callee_old as *const () as usize,
                callee_new as *const u8,
            )
        };
        assert!(not_a_call.is_err(), "the site is not a call");
    }
}
