// SPDX-License-Identifier: MIT OR Apache-2.0
//! Files + Find (NEURAL-APPS-0) as a WASM app on the omni-render runtime. The app owns its data: the ternary index is loaded
//! INTO its linear memory; the runtime's intrinsics run the P2 kernel on that memory in place (no copy out of the sandbox).
//! Index file (written by `omni-render find-index`): b"OFX1" u32 n, u32 dim, f32[dim] mean, n × P2 row (dim/4 B), then
//! n paths as (u16 len, bytes).
#![no_std]
extern crate alloc;
use alloc::vec;
use alloc::vec::Vec;

#[link(wasm_import_module = "omni")]
unsafe extern "C" {
    /// size of a host file the app may read (read-only, path relative to the app's granted root); -1 if absent
    fn file_size(path: *const u8, len: u32) -> i64;
    /// copy the file into guest memory at dst (≤ cap bytes); returns bytes copied
    fn file_read(path: *const u8, len: u32, dst: *mut u8, cap: u32) -> i64;
    /// embed text (nomic search_query), centre by `mean`, quantise to int8 (|x| ≤ 127) into dst[dim]
    fn embed_query(text: *const u8, len: u32, mean: *const f32, dim: u32, dst: *mut i8) -> i32;
    /// exact top-k of Σ x·t over n P2 rows (row bytes = dim/4) — AVX2 P2 kernel on guest memory; writes k (index, score) pairs
    fn p2_topk(rows: *const u8, n: u32, dim: u32, x: *const i8, k: u32, out: *mut i32) -> u32;
    fn print(s: *const u8, len: u32);
}

struct Alloc;
unsafe impl core::alloc::GlobalAlloc for Alloc {
    unsafe fn alloc(&self, l: core::alloc::Layout) -> *mut u8 {
        // bump allocator over memory.grow (the app is short-lived; a reset per call is enough)
        static mut TOP: usize = 0;
        unsafe {
            let a = l.align().max(8);
            if TOP == 0 { TOP = core::arch::wasm32::memory_size(0) * 65536; }
            let start = (TOP + a - 1) & !(a - 1);
            let end = start + l.size();
            let have = core::arch::wasm32::memory_size(0) * 65536;
            if end > have { if core::arch::wasm32::memory_grow(0, (end - have).div_ceil(65536)) == usize::MAX { return core::ptr::null_mut(); } }
            TOP = end;
            start as *mut u8
        }
    }
    unsafe fn dealloc(&self, _: *mut u8, _: core::alloc::Layout) {}
}
#[global_allocator]
static A: Alloc = Alloc;
#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! { core::arch::wasm32::unreachable() }

fn say(s: &[u8]) { unsafe { print(s.as_ptr(), s.len() as u32) } }
fn num(mut v: i64, out: &mut Vec<u8>) {
    if v < 0 { out.push(b'-'); v = -v; }
    let mut d = [0u8; 20]; let mut i = 0;
    loop { d[i] = b'0' + (v % 10) as u8; i += 1; v /= 10; if v == 0 { break; } }
    while i > 0 { i -= 1; out.push(d[i]); }
}

static mut INDEX: Vec<u8> = Vec::new();

/// Load the index once (into this app's memory). Returns the number of documents, or -1.
#[unsafe(no_mangle)]
pub extern "C" fn find_open(path: *const u8, len: u32) -> i32 {
    unsafe {
        let sz = file_size(path, len);
        if sz < 12 { return -1; }
        let mut v = vec![0u8; sz as usize];
        file_read(path, len, v.as_mut_ptr(), sz as u32);
        if &v[..4] != b"OFX1" { return -1; }
        let n = u32::from_le_bytes([v[4], v[5], v[6], v[7]]) as i32;
        INDEX = v;
        n
    }
}

/// Query → top-k paths printed through the host, best first. Returns the number printed.
#[unsafe(no_mangle)]
pub extern "C" fn find(q: *const u8, len: u32, k: u32) -> i32 {
    let ix: &Vec<u8> = unsafe { &*core::ptr::addr_of!(INDEX) };
    if ix.len() < 12 { return -1; }
    let n = u32::from_le_bytes([ix[4], ix[5], ix[6], ix[7]]);
    let dim = u32::from_le_bytes([ix[8], ix[9], ix[10], ix[11]]);
    let mean = ix[12..].as_ptr() as *const f32;
    let rows_off = 12 + 4 * dim as usize;
    let rb = dim as usize / 4;
    let mut x = vec![0i8; dim as usize];
    let mut out = vec![0i32; 2 * k as usize];
    let got = unsafe {
        if embed_query(q, len, mean, dim, x.as_mut_ptr()) != 0 { return -2; }
        p2_topk(ix[rows_off..].as_ptr(), n, dim, x.as_ptr(), k, out.as_mut_ptr())
    };
    // paths table after the rows
    let mut paths: Vec<&[u8]> = Vec::with_capacity(n as usize);
    let mut o = rows_off + rb * n as usize;
    for _ in 0..n { let l = u16::from_le_bytes([ix[o], ix[o + 1]]) as usize; paths.push(&ix[o + 2..o + 2 + l]); o += 2 + l; }
    for j in 0..got as usize {
        let (doc, score) = (out[2 * j] as usize, out[2 * j + 1]);
        let mut line = Vec::new();
        num(j as i64 + 1, &mut line); line.extend_from_slice(b". "); num(score as i64, &mut line); line.extend_from_slice(b"  ");
        line.extend_from_slice(paths[doc]);
        say(&line);
    }
    got as i32
}
