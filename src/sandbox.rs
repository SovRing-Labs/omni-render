// SPDX-License-Identifier: MIT OR Apache-2.0
//! sandbox — the untrusted-content process model (Firefox's shape, our code):
//!   broker (parent): owns the network, fetch policy and output; never parses page content.
//!   renderer (child): parses/styles/lays out untrusted bytes after locking itself down:
//!     1. no_new_privs   2. Landlock: filesystem = read-only fonts, TCP bind/connect denied (ABI ≥ 4)
//!     3. seccomp-BPF syscall allow-list (no socket, no exec, no fork, no ptrace) → EPERM otherwise.
//!   The child asks the broker for subresources over its pipes: 'G' url → length-prefixed bytes.
use std::io::{Read, Write};

// ---- Landlock (uapi/linux/landlock.h) ----
const SYS_LANDLOCK_CREATE_RULESET: libc::c_long = 444;
const SYS_LANDLOCK_ADD_RULE: libc::c_long = 445;
const SYS_LANDLOCK_RESTRICT_SELF: libc::c_long = 446;
const LANDLOCK_CREATE_RULESET_VERSION: u32 = 1;
const LANDLOCK_RULE_PATH_BENEATH: u32 = 1;
const FS_READ_FILE: u64 = 1 << 2;
const FS_READ_DIR: u64 = 1 << 3;

#[repr(C)]
struct RulesetAttr { handled_access_fs: u64, handled_access_net: u64 }

#[repr(C, packed)]
struct PathBeneath { allowed_access: u64, parent_fd: i32 }

fn landlock(read_only: &[&str]) -> Result<i64, String> {
    let abi = unsafe { libc::syscall(SYS_LANDLOCK_CREATE_RULESET, std::ptr::null::<u8>(), 0usize, LANDLOCK_CREATE_RULESET_VERSION) };
    if abi < 1 { return Err("landlock unavailable".into()); }
    let mut fs_all: u64 = (1 << 13) - 1;           // ABI 1: 13 rights
    if abi >= 2 { fs_all |= 1 << 13; }              // REFER
    if abi >= 3 { fs_all |= 1 << 14; }              // TRUNCATE
    if abi >= 5 { fs_all |= 1 << 15; }              // IOCTL_DEV
    let net_all: u64 = if abi >= 4 { 0b11 } else { 0 }; // BIND_TCP | CONNECT_TCP
    let attr = RulesetAttr { handled_access_fs: fs_all, handled_access_net: net_all };
    let size = if abi >= 4 { std::mem::size_of::<RulesetAttr>() } else { 8 };
    let rs = unsafe { libc::syscall(SYS_LANDLOCK_CREATE_RULESET, &attr as *const RulesetAttr, size, 0u32) };
    if rs < 0 { return Err(format!("create_ruleset: {}", std::io::Error::last_os_error())); }
    for p in read_only {
        let c = std::ffi::CString::new(*p).unwrap();
        let fd = unsafe { libc::open(c.as_ptr(), libc::O_PATH | libc::O_CLOEXEC) };
        if fd < 0 { continue; }
        let pb = PathBeneath { allowed_access: FS_READ_FILE | FS_READ_DIR, parent_fd: fd };
        let r = unsafe { libc::syscall(SYS_LANDLOCK_ADD_RULE, rs, LANDLOCK_RULE_PATH_BENEATH, &pb as *const PathBeneath, 0u32) };
        unsafe { libc::close(fd) };
        if r < 0 { return Err(format!("add_rule {p}: {}", std::io::Error::last_os_error())); }
    }
    let r = unsafe { libc::syscall(SYS_LANDLOCK_RESTRICT_SELF, rs, 0u32) };
    unsafe { libc::close(rs as i32) };
    if r < 0 { return Err(format!("restrict_self: {}", std::io::Error::last_os_error())); }
    Ok(abi)
}

// ---- seccomp-BPF ----
const AUDIT_ARCH_X86_64: u32 = 0xc000_003e;
const RET_ALLOW: u32 = 0x7fff_0000;
const RET_ERRNO: u32 = 0x0005_0000;
const RET_KILL_PROCESS: u32 = 0x8000_0000;

#[repr(C)]
#[derive(Clone, Copy)]
struct SockFilter { code: u16, jt: u8, jf: u8, k: u32 }
#[repr(C)]
struct SockFprog { len: u16, filter: *const SockFilter }

const fn stmt(code: u16, k: u32) -> SockFilter { SockFilter { code, jt: 0, jf: 0, k } }
const fn jump(code: u16, k: u32, jt: u8, jf: u8) -> SockFilter { SockFilter { code, jt, jf, k } }
const LD_W_ABS: u16 = 0x20; // BPF_LD | BPF_W | BPF_ABS
const JEQ_K: u16 = 0x15;    // BPF_JMP | BPF_JEQ | BPF_K
const RET_K: u16 = 0x06;    // BPF_RET | BPF_K

/// x86-64 syscalls the renderer needs after setup (memory, its pipes, clocks, fonts via openat under Landlock).
const ALLOW: &[u32] = &[
    0, 1, 3, 5, 8, 9, 10, 11, 12, 13, 14, 15, 17, 19, 20, 24, 25, 28, // read write close fstat lseek mmap mprotect munmap brk sigaction sigprocmask sigreturn pread readv writev yield mremap madvise
    39, 60, 72, 131, 186, 202, 228, 230, 231, 257, 262, 273, 302, 318, 332, 334, // getpid exit fcntl sigaltstack gettid futex clock_gettime clock_nanosleep exit_group openat newfstatat set_robust_list prlimit64 getrandom statx rseq
];

fn seccomp() -> Result<(), String> {
    let mut f: Vec<SockFilter> = vec![
        stmt(LD_W_ABS, 4),                               // seccomp_data.arch
        jump(JEQ_K, AUDIT_ARCH_X86_64, 1, 0),
        stmt(RET_K, RET_KILL_PROCESS),
        stmt(LD_W_ABS, 0),                               // seccomp_data.nr
    ];
    let n = ALLOW.len();
    for (i, &nr) in ALLOW.iter().enumerate() { f.push(jump(JEQ_K, nr, (n - i) as u8, 0)); }
    f.push(stmt(RET_K, RET_ERRNO | libc::EPERM as u32));
    f.push(stmt(RET_K, RET_ALLOW));
    let prog = SockFprog { len: f.len() as u16, filter: f.as_ptr() };
    let r = unsafe { libc::prctl(libc::PR_SET_SECCOMP, 2 /* SECCOMP_MODE_FILTER */, &prog as *const SockFprog as libc::c_ulong, 0, 0) };
    if r != 0 { return Err(format!("seccomp: {}", std::io::Error::last_os_error())); }
    Ok(())
}

/// Lock the current process down. Call after fonts are mapped? Not needed: fonts stay readable under Landlock.
pub fn lockdown() -> Result<String, String> {
    if unsafe { libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) } != 0 { return Err("no_new_privs".into()); }
    let abi = landlock(&["/usr/share/fonts", "/etc/fonts"])?;
    seccomp()?;
    Ok(format!("landlock abi {abi} + seccomp allow-list {}", ALLOW.len()))
}

/// Probes that must FAIL inside the sandbox (the render-gate for safety).
pub fn probes() -> Vec<(&'static str, bool)> {
    let file = std::fs::read("/etc/passwd").is_err();
    let home = std::fs::write("/tmp/omr-sandbox-escape", b"x").is_err();
    let sock = std::net::TcpStream::connect("127.0.0.1:8931").is_err();
    let exec = std::process::Command::new("/bin/true").status().is_err();
    let font = std::fs::metadata("/usr/share/fonts").is_ok();
    vec![("read /etc/passwd denied", file), ("write /tmp denied", home), ("tcp connect denied", sock), ("exec denied", exec), ("fonts readable", font)]
}

// ---- framing ----
pub fn write_frame(w: &mut impl Write, tag: u8, data: &[u8]) -> std::io::Result<()> {
    w.write_all(&[tag])?;
    w.write_all(&(data.len() as u32).to_le_bytes())?;
    w.write_all(data)?;
    w.flush()
}
pub fn read_frame(r: &mut impl Read) -> Option<(u8, Vec<u8>)> {
    let mut h = [0u8; 5];
    r.read_exact(&mut h).ok()?;
    let n = u32::from_le_bytes([h[1], h[2], h[3], h[4]]) as usize;
    if n > 64 << 20 { return None; }
    let mut v = vec![0u8; n];
    r.read_exact(&mut v).ok()?;
    Some((h[0], v))
}

/// Child-side Source: subresources come from the broker over the pipes.
pub struct PipeSource { pub url: String, pub doc: Vec<u8>, pub io: std::cell::RefCell<(std::io::Stdin, std::io::Stdout)> }

impl crate::page::Source for PipeSource {
    fn url(&self) -> String { self.url.clone() }
    fn html(&self) -> Vec<u8> { self.doc.clone() }
    fn get(&self, url: &str) -> Option<Vec<u8>> {
        // data: URLs carry their bytes: decoded here, nothing crosses to the broker
        if url.starts_with("data:") { return crate::net::data_url(url); }
        let mut io = self.io.borrow_mut();
        let (ref mut i, ref mut o) = *io;
        write_frame(&mut o.lock(), b'G', url.as_bytes()).ok()?;
        let (tag, data) = read_frame(&mut i.lock())?;
        if tag == b'B' { Some(data) } else { None }
    }
}
