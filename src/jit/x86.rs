// SPDX-License-Identifier: MIT OR Apache-2.0
//! Our x86-64 emitter: the handful of instructions the runtime renders kernels from, encoded by hand from the Intel SDM
//! (VEX 2-byte prefix, ModRM/disp32 forms), written into W^X pages (mmap RW → copy → mprotect RX) and called as a fn pointer.
//! Every encoding is checked against GNU objdump in `tests` (`cargo test jit`). No assembler crate: the surface is tiny.

/// A growing machine-code buffer.
#[derive(Default)]
pub struct Asm { pub b: Vec<u8> }

/// General-purpose registers by hardware number.
pub const RAX: u8 = 0; pub const RCX: u8 = 1; pub const RDX: u8 = 2; pub const RSI: u8 = 6; pub const RDI: u8 = 7;
pub const R8: u8 = 8; pub const R9: u8 = 9; pub const R10: u8 = 10; pub const R11: u8 = 11;

impl Asm {
    fn d32(&mut self, d: i32) { self.b.extend_from_slice(&d.to_le_bytes()); }
    /// ModRM for [base + disp] (no rsp/rbp/r12/r13: those need SIB or mean RIP); disp8 when it fits. Bases r8..r11 need the
    /// caller's VEX/REX B bit (vex3 / rex).
    fn mem(&mut self, reg: u8, base: u8, disp: i32) {
        debug_assert!(base & 7 != 4 && base & 7 != 5);
        if disp == 0 { self.b.push(((reg & 7) << 3) | (base & 7)); }
        else if (-128..=127).contains(&disp) { self.b.push(0x40 | ((reg & 7) << 3) | (base & 7)); self.b.push(disp as i8 as u8); }
        else { self.b.push(0x80 | ((reg & 7) << 3) | (base & 7)); self.d32(disp); }
    }
    /// 2-byte VEX: C5 [~R vvvv~ L pp]; map 0F only, base/index must be < 8.
    fn vex2(&mut self, r: u8, vvvv: u8, l256: bool, pp: u8) {
        self.b.push(0xC5);
        self.b.push((((r >> 3) ^ 1) << 7) | ((!vvvv & 0xF) << 3) | ((l256 as u8) << 2) | pp);
    }
    /// 3-byte VEX: C4 [~R ~X ~B mmmmm] [W vvvv~ L pp].
    fn vex3(&mut self, r: u8, b: u8, map: u8, w: bool, vvvv: u8, l256: bool, pp: u8) {
        self.b.push(0xC4);
        self.b.push((((r >> 3) ^ 1) << 7) | (1 << 6) | (((b >> 3) ^ 1) << 5) | map);
        self.b.push(((w as u8) << 7) | ((!vvvv & 0xF) << 3) | ((l256 as u8) << 2) | pp);
    }
    // ---- AVX2 integer, 256-bit, 66 0F map (2-byte VEX is enough for ymm0..15 with base < 8)
    fn v66_rm(&mut self, op: u8, d: u8, s: u8, base: u8, disp: i32) {
        if base < 8 { self.vex2(d, s, true, 1); } else { self.vex3(d, base, 1, false, s, true, 1); }
        self.b.push(op); self.mem(d, base, disp);
    }
    fn v66_rr(&mut self, op: u8, d: u8, s: u8, m: u8) {
        if m < 8 { self.vex2(d, s, true, 1); } else { self.vex3(d, m, 1, false, s, true, 1); }
        self.b.push(op); self.b.push(0xC0 | ((d & 7) << 3) | (m & 7));
    }
    pub fn vpaddd_m(&mut self, d: u8, s: u8, base: u8, disp: i32) { self.v66_rm(0xFE, d, s, base, disp) }
    pub fn vpsubd_m(&mut self, d: u8, s: u8, base: u8, disp: i32) { self.v66_rm(0xFA, d, s, base, disp) }
    pub fn vpaddd(&mut self, d: u8, s: u8, m: u8) { self.v66_rr(0xFE, d, s, m) }
    pub fn vpsubd(&mut self, d: u8, s: u8, m: u8) { self.v66_rr(0xFA, d, s, m) }
    pub fn vpxor(&mut self, d: u8, s: u8, m: u8) { self.v66_rr(0xEF, d, s, m) }
    pub fn vpaddw(&mut self, d: u8, s: u8, m: u8) { self.v66_rr(0xFD, d, s, m) }
    pub fn vpshufb(&mut self, d: u8, s: u8, m: u8) { self.v0f38_rr(0x00, d, s, m) }
    pub fn vpsignb(&mut self, d: u8, s: u8, m: u8) { self.v0f38_rr(0x08, d, s, m) }
    pub fn vpabsb(&mut self, d: u8, m: u8) { self.v0f38_rr(0x1C, d, 0, m) }
    pub fn vpmaddubsw(&mut self, d: u8, s: u8, m: u8) { self.v0f38_rr(0x04, d, s, m) }
    pub fn vpmaddwd(&mut self, d: u8, s: u8, m: u8) { self.v66_rr(0xF5, d, s, m) }
    pub fn vpmaddwd_m(&mut self, d: u8, s: u8, base: u8, disp: i32) { self.v66_rm(0xF5, d, s, base, disp) }
    pub fn vpand(&mut self, d: u8, s: u8, m: u8) { self.v66_rr(0xDB, d, s, m) }
    /// vpsrlw ymm_d, ymm_m, imm8  (VEX.NDD: 66 0F 71 /2 ib; destination in vvvv)
    pub fn vpsrlw_i(&mut self, d: u8, m: u8, imm: u8) {
        if m < 8 { self.vex2(0, d, true, 1); } else { self.vex3(0, m, 1, false, d, true, 1); }
        self.b.push(0x71); self.b.push(0xC0 | (2 << 3) | (m & 7)); self.b.push(imm);
    }
    fn v0f38_rr(&mut self, op: u8, d: u8, s: u8, m: u8) {
        self.vex3(d, m, 2, false, s, true, 1); self.b.push(op); self.b.push(0xC0 | ((d & 7) << 3) | (m & 7));
    }
    /// vmovdqu ymm_d, [base+disp]  (F3 0F 6F)
    pub fn vmovdqu_ld(&mut self, d: u8, base: u8, disp: i32) { self.vex_f3(d, base); self.b.push(0x6F); self.mem(d, base, disp); }
    /// vmovdqu [base+disp], ymm_s  (F3 0F 7F)
    pub fn vmovdqu_st(&mut self, base: u8, disp: i32, s: u8) { self.vex_f3(s, base); self.b.push(0x7F); self.mem(s, base, disp); }
    /// VEX.256.F3.0F prefix for vmovdqu; 3-byte form when the base register is r8..r11 (VEX.B).
    fn vex_f3(&mut self, r: u8, base: u8) { if base < 8 { self.vex2(r, 0, true, 2); } else { self.vex3(r, base, 1, false, 0, true, 2); } }
    /// vextracti128 xmm_d, ymm_s, 1  (VEX.256.66.0F3A.W0 39 /r ib; reg = source)
    pub fn vextracti128_hi(&mut self, d: u8, s: u8) { self.vex3(s, d, 3, false, 0, true, 1); self.b.push(0x39); self.b.push(0xC0 | ((s & 7) << 3) | (d & 7)); self.b.push(1); }
    /// vpaddd xmm_d, xmm_s, xmm_m (128-bit)
    pub fn vpaddd_x(&mut self, d: u8, s: u8, m: u8) { self.vex2(d, s, false, 1); self.b.push(0xFE); self.b.push(0xC0 | ((d & 7) << 3) | (m & 7)); }
    /// vpshufd xmm_d, xmm_m, imm  (VEX.128.66.0F 70 /r ib)
    pub fn vpshufd_x(&mut self, d: u8, m: u8, imm: u8) { self.vex2(d, 0, false, 1); self.b.push(0x70); self.b.push(0xC0 | ((d & 7) << 3) | (m & 7)); self.b.push(imm); }
    /// vmovd [base+disp], xmm_s  (VEX.128.66.0F.W0 7E /r)
    pub fn vmovd_st(&mut self, base: u8, disp: i32, s: u8) { self.vex2(s, 0, false, 1); self.b.push(0x7E); self.mem(s, base, disp); }
    /// 64-bit helpers: mov r64, r64 ; add r64, imm32 ; dec r64 ; test r64, r64
    pub fn mov64(&mut self, d: u8, s: u8) { self.rex(true, s, d); self.b.push(0x89); self.b.push(0xC0 | ((s & 7) << 3) | (d & 7)); }
    pub fn add64_i(&mut self, d: u8, imm: i32) { self.rex(true, 0, d); self.b.push(0x81); self.b.push(0xC0 | (d & 7)); self.d32(imm); }
    pub fn dec64(&mut self, d: u8) { self.rex(true, 0, d); self.b.push(0xFF); self.b.push(0xC8 | (d & 7)); }
    pub fn test64(&mut self, d: u8) { self.rex(true, d, d); self.b.push(0x85); self.b.push(0xC0 | ((d & 7) << 3) | (d & 7)); }
    /// jz/jnz rel32 to an absolute buffer offset (backward targets known; forward ones patched with `patch`).
    pub fn jcc(&mut self, nz: bool, target: usize) -> usize {
        self.b.extend_from_slice(&[0x0F, if nz { 0x85 } else { 0x84 }]);
        let at = self.b.len(); self.d32((target as i64 - (at as i64 + 4)) as i32); at
    }
    pub fn patch(&mut self, at: usize, target: usize) { let r = (target as i64 - (at as i64 + 4)) as i32; self.b[at..at + 4].copy_from_slice(&r.to_le_bytes()); }
    pub fn vzeroupper(&mut self) { self.b.extend_from_slice(&[0xC5, 0xF8, 0x77]); }
    pub fn ret(&mut self) { self.b.push(0xC3); }
    // ---- scalar 32-bit
    fn rex(&mut self, w: bool, r: u8, b: u8) {
        let x = 0x40 | ((w as u8) << 3) | (((r >> 3) & 1) << 2) | ((b >> 3) & 1);
        if x != 0x40 { self.b.push(x); }
    }
    /// add r32, [base+disp]   /  sub r32, [base+disp]
    pub fn add_m(&mut self, r: u8, base: u8, disp: i32) { self.rex(false, r, base); self.b.push(0x03); self.mem(r, base, disp); }
    pub fn sub_m(&mut self, r: u8, base: u8, disp: i32) { self.rex(false, r, base); self.b.push(0x2B); self.mem(r, base, disp); }
    /// xor r32, r32 ; add r32, r32 (dst += src)
    pub fn zero(&mut self, r: u8) { self.rex(false, r, r); self.b.push(0x31); self.b.push(0xC0 | ((r & 7) << 3) | (r & 7)); }
    pub fn add_rr(&mut self, d: u8, s: u8) { self.rex(false, s, d); self.b.push(0x01); self.b.push(0xC0 | ((s & 7) << 3) | (d & 7)); }
    /// mov [base+disp], r32
    pub fn st32(&mut self, base: u8, disp: i32, r: u8) { self.rex(false, r, base); self.b.push(0x89); self.mem(r, base, disp); }
    // ---- T25 core (T25-CORE-JIT step 2): 64-bit loads / magic-number reduction / byte stores, byte-lane AVX2
    /// mov r64, [base+disp]  (REX.W 8B /r)
    pub fn ld64(&mut self, r: u8, base: u8, disp: i32) { self.rex(true, r, base); self.b.push(0x8B); self.mem(r, base, disp); }
    /// movabs r64, imm64  (REX.W B8+r io)
    pub fn movabs(&mut self, r: u8, imm: u64) { self.rex(true, 0, r); self.b.push(0xB8 + (r & 7)); self.b.extend_from_slice(&imm.to_le_bytes()); }
    /// imul r64, r64  (REX.W 0F AF /r: d *= s)
    pub fn imul64(&mut self, d: u8, s: u8) { self.rex(true, d, s); self.b.extend_from_slice(&[0x0F, 0xAF]); self.b.push(0xC0 | ((d & 7) << 3) | (s & 7)); }
    /// imul r64, r64, imm32  (REX.W 69 /r id: d = s · imm)
    pub fn imul64_i(&mut self, d: u8, s: u8, imm: i32) { self.rex(true, d, s); self.b.push(0x69); self.b.push(0xC0 | ((d & 7) << 3) | (s & 7)); self.d32(imm); }
    /// shr r64, imm8  (REX.W C1 /5 ib)
    pub fn shr64_i(&mut self, d: u8, imm: u8) { self.rex(true, 0, d); self.b.push(0xC1); self.b.push(0xE8 | (d & 7)); self.b.push(imm); }
    /// sub r64, r64  (REX.W 29 /r: d -= s)
    pub fn sub64(&mut self, d: u8, s: u8) { self.rex(true, s, d); self.b.push(0x29); self.b.push(0xC0 | ((s & 7) << 3) | (d & 7)); }
    /// mov byte [base+disp], r8  (88 /r; rax/rcx/rdx/rbx low bytes, no REX needed for base < 8)
    pub fn st8(&mut self, base: u8, disp: i32, r: u8) { debug_assert!(r < 4 || r >= 8); self.rex(false, r, base); self.b.push(0x88); self.mem(r, base, disp); }
    /// vpsubb / vpaddb / vpcmpgtb ymm_d, ymm_s, ymm_m  (66 0F F8 / FC / 64)
    pub fn vpsubb(&mut self, d: u8, s: u8, m: u8) { self.v66_rr(0xF8, d, s, m) }
    pub fn vpaddb(&mut self, d: u8, s: u8, m: u8) { self.v66_rr(0xFC, d, s, m) }
    pub fn vpcmpgtb(&mut self, d: u8, s: u8, m: u8) { self.v66_rr(0x64, d, s, m) }
}

/// Executable code in W^X pages. Dropping it unmaps the pages.
pub struct Exec { p: *mut u8, len: usize, pub code_bytes: usize }
unsafe impl Send for Exec {}
unsafe impl Sync for Exec {}

impl Exec {
    pub fn new(code: &[u8]) -> Exec {
        let len = code.len().max(1).div_ceil(4096) * 4096;
        unsafe {
            let p = libc::mmap(std::ptr::null_mut(), len, libc::PROT_READ | libc::PROT_WRITE, libc::MAP_PRIVATE | libc::MAP_ANONYMOUS, -1, 0);
            assert!(p != libc::MAP_FAILED, "mmap failed");
            std::ptr::copy_nonoverlapping(code.as_ptr(), p as *mut u8, code.len());
            assert_eq!(libc::mprotect(p, len, libc::PROT_READ | libc::PROT_EXEC), 0, "mprotect RX failed");
            Exec { p: p as *mut u8, len, code_bytes: code.len() }
        }
    }
    /// The entry point as `extern "C" fn(a, b)` (SysV: rdi, rsi).
    pub fn f2(&self) -> extern "C" fn(*const u8, *mut u8) { unsafe { std::mem::transmute(self.p) } }
    pub fn ptr(&self) -> *const u8 { self.p }
    /// The sift entry: fn(lat, n, qr, qi, out, consts) (SysV: rdi, rsi, rdx, rcx, r8, r9).
    pub fn f6(&self) -> extern "C" fn(*const u8, usize, *const i8, *const i8, *mut i32, *const u8) { unsafe { std::mem::transmute(self.p) } }
}
impl Drop for Exec { fn drop(&mut self) { unsafe { libc::munmap(self.p as *mut libc::c_void, self.len); } } }

#[cfg(test)]
mod tests {
    use super::*;
    /// Disassemble with GNU objdump and compare mnemonics: our encodings are checked by an independent decoder.
    fn dis(code: &[u8]) -> Vec<String> {
        let f = std::env::temp_dir().join(format!("omni-jit-{}.bin", std::process::id()));
        std::fs::write(&f, code).unwrap();
        let o = std::process::Command::new("objdump").args(["-D", "-b", "binary", "-mi386:x86-64", "-M", "intel"]).arg(&f).output().unwrap();
        let _ = std::fs::remove_file(&f);
        String::from_utf8_lossy(&o.stdout).lines().filter_map(|l| l.split('\t').nth(2).map(|s| s.split_whitespace().collect::<Vec<_>>().join(" "))).collect()
    }
    #[test]
    fn jit_encodings_match_objdump() {
        let mut a = Asm::default();
        a.vpaddd_m(3, 3, RDI, 0x1000); a.vpsubd_m(0, 0, RDI, 32); a.vpaddd_m(15, 15, RDI, 0); a.vpxor(2, 2, 2);
        a.vpaddd(0, 0, 9); a.vpsubd(12, 12, 1); a.vmovdqu_st(RSI, 0x40, 0); a.vmovdqu_ld(5, RDI, 0x80);
        a.add_m(RAX, RDI, 4); a.sub_m(R8, RDI, 0x400); a.zero(R8); a.add_rr(RAX, R8); a.st32(RSI, 8, RDX);
        a.vpshufb(1, 2, 3); a.vpsignb(4, 5, 6); a.vpabsb(7, 8); a.vpmaddubsw(9, 10, 11); a.vpmaddwd(1, 1, 14); a.vpand(3, 4, 5);
        a.vpsrlw_i(6, 7, 4); a.vpaddw(2, 2, 3);
        a.vmovdqu_ld(12, R10, 0x40); a.vmovdqu_st(R8, 0x20, 3); a.vpmaddwd_m(12, 12, R9, 0x40); a.vpaddd_m(1, 1, R8, 0); a.add64_i(R10, -1024);
        a.vextracti128_hi(1, 0); a.vpaddd_x(0, 0, 1); a.vpshufd_x(1, 0, 0x4E); a.vmovd_st(R8 & 7, 0, 0);
        a.mov64(RAX, R9); a.add64_i(RDI, 5120); a.dec64(RSI); a.test64(RSI);
        a.ld64(RAX, RDI, 16); a.ld64(R9, R8, 0x80); a.movabs(R8, 0x0123_4567_89ab_cdef); a.imul64(RAX, R8); a.imul64_i(RAX, RAX, 101);
        a.shr64_i(RAX, 40); a.sub64(RCX, RAX); a.st8(RDX, 7, RCX); a.vpsubb(0, 1, 2); a.vpaddb(3, 3, 9); a.vpcmpgtb(4, 5, 6);
        a.vzeroupper(); a.ret();
        let want = [
            "vpaddd ymm3,ymm3,YMMWORD PTR [rdi+0x1000]", "vpsubd ymm0,ymm0,YMMWORD PTR [rdi+0x20]", "vpaddd ymm15,ymm15,YMMWORD PTR [rdi]",
            "vpxor ymm2,ymm2,ymm2", "vpaddd ymm0,ymm0,ymm9", "vpsubd ymm12,ymm12,ymm1", "vmovdqu YMMWORD PTR [rsi+0x40],ymm0",
            "vmovdqu ymm5,YMMWORD PTR [rdi+0x80]", "add eax,DWORD PTR [rdi+0x4]", "sub r8d,DWORD PTR [rdi+0x400]", "xor r8d,r8d",
            "add eax,r8d", "mov DWORD PTR [rsi+0x8],edx", "vpshufb ymm1,ymm2,ymm3", "vpsignb ymm4,ymm5,ymm6", "vpabsb ymm7,ymm8",
            "vpmaddubsw ymm9,ymm10,ymm11", "vpmaddwd ymm1,ymm1,ymm14", "vpand ymm3,ymm4,ymm5", "vpsrlw ymm6,ymm7,0x4",
            "vpaddw ymm2,ymm2,ymm3",
            "vmovdqu ymm12,YMMWORD PTR [r10+0x40]", "vmovdqu YMMWORD PTR [r8+0x20],ymm3", "vpmaddwd ymm12,ymm12,YMMWORD PTR [r9+0x40]",
            "vpaddd ymm1,ymm1,YMMWORD PTR [r8]", "add r10,0xfffffffffffffc00",
            "vextracti128 xmm1,ymm0,0x1", "vpaddd xmm0,xmm0,xmm1", "vpshufd xmm1,xmm0,0x4e", "vmovd DWORD PTR [rax],xmm0",
            "mov rax,r9", "add rdi,0x1400", "dec rsi", "test rsi,rsi",
            "mov rax,QWORD PTR [rdi+0x10]", "mov r9,QWORD PTR [r8+0x80]", "movabs r8,0x123456789abcdef", "imul rax,r8", "imul rax,rax,0x65",
            "shr rax,0x28", "sub rcx,rax", "mov BYTE PTR [rdx+0x7],cl", "vpsubb ymm0,ymm1,ymm2", "vpaddb ymm3,ymm3,ymm9", "vpcmpgtb ymm4,ymm5,ymm6",
            "vzeroupper", "ret"];
        let got = dis(&a.b);
        assert_eq!(got, want.iter().map(|s| s.to_string()).collect::<Vec<_>>());
    }
}
