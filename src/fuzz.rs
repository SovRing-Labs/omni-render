// SPDX-License-Identifier: MIT OR Apache-2.0
//! fuzz — own mutational fuzzer (fork-server shape): the parent mutates corpus seeds and feeds a worker process;
//! the worker runs the real pipeline (HTML → DOM → CSS → cascade → layout → raster; image decoders; SVG) under the
//! production sandbox. A panic is caught and reported; a crash/abort/OOM/hang kills the worker, the parent saves
//! the input and respawns. Results: execs, execs/s, unique findings by message, inputs in results/fuzz/.
use crate::sandbox::{read_frame, write_frame};
use std::collections::HashMap;
use std::io::Write;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

thread_local! { static LOC: std::cell::RefCell<String> = std::cell::RefCell::new(String::new()); }

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 { self.0 ^= self.0 << 13; self.0 ^= self.0 >> 7; self.0 ^= self.0 << 17; self.0 }
    fn below(&mut self, n: usize) -> usize { if n == 0 { 0 } else { (self.next() % n as u64) as usize } }
}

const TOKENS: &[&[u8]] = &[b"<table>", b"<td colspan=999999>", b"</div>", b"<svg viewBox='0 0 0 0'>", b"style='display:grid;grid-template-columns:repeat(9999,1fr)'",
    b"{", b"}", b"@media (", b"@import url(", b"var(--x)", b"--x:var(--x);", b"calc(100%/0)", b"::before", b":nth-child(-n+0)", b":not(:not(:not(*)))",
    b"\xff\xfe", b"\x00", b"&#x10FFFF;", b"<img width=99999999 height=-1>", b"float:left;", b"position:absolute;", b"font-size:1e30px", b"<br>", b"<li>",
    b"A0 0 0 1 1 1e9 1e9", b"M0 0L", b"rotate(1e38)", b"<style>", b"</style>", b"<template>", b"<math>", b"<!--"];

fn mutate(r: &mut Rng, seed: &[u8], other: &[u8]) -> Vec<u8> {
    let mut v = seed.to_vec();
    let rounds = 1 + r.below(8);
    for _ in 0..rounds {
        if v.is_empty() { v.push(b'<'); }
        let i = r.below(v.len());
        match r.below(8) {
            0 => { let b = r.below(8); v[i] ^= 1 << b; }
            1 => { v[i] = r.next() as u8; }
            2 => { let n = 1 + r.below(64.min(v.len() - i)); v.drain(i..i + n.min(v.len() - i)); }
            3 => { let t = TOKENS[r.below(TOKENS.len())]; v.splice(i..i, t.iter().copied()); }
            4 => { if !other.is_empty() { let a = r.below(other.len()); let n = 1 + r.below(256.min(other.len() - a)); v.splice(i..i, other[a..a + n].iter().copied()); } }
            5 => { let n = 1 + r.below(32.min(v.len() - i)); let chunk = v[i..i + n].to_vec(); for _ in 0..1 + r.below(64) { v.splice(i..i, chunk.iter().copied()); } }
            6 => { v.truncate(i.max(1)); }
            _ => { let j = r.below(v.len()); v.swap(i, j); }
        }
        if v.len() > 2 << 20 { v.truncate(2 << 20); }
    }
    v
}

/// Re-run one saved finding in-process (kind from the file extension .kN), for debugging.
pub fn repro(path: &str) {
    let kind: u8 = path.rsplit(".k").next().and_then(|k| k.parse().ok()).unwrap_or(0);
    let data = std::fs::read(path).unwrap();
    let fonts = crate::font::Fonts::new();
    run_one(kind, &data, &fonts);
    println!("repro ok (no failure) kind {kind}");
}

/// Worker: one input per frame; kind byte selects the target.
pub fn worker() {
    let _ = crate::sandbox::lockdown();
    unsafe { let lim = libc::rlimit { rlim_cur: 1 << 30, rlim_max: 1 << 30 }; libc::setrlimit(libc::RLIMIT_AS, &lim); }
    // record the panic location; the message comes back through catch_unwind
    std::panic::set_hook(Box::new(|info| { if let Some(l) = info.location() { LOC.with(|c| *c.borrow_mut() = format!("{}:{}", l.file(), l.line())); } }));
    let fonts = crate::font::Fonts::new();
    let stdin = std::io::stdin();
    let stdout = std::io::stdout();
    while let Some((kind, data)) = read_frame(&mut stdin.lock()) {
        let res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| run_one(kind, &data, &fonts)));
        let mut o = stdout.lock();
        match res {
            Ok(()) => { write_frame(&mut o, b'K', &[]).ok(); }
            Err(e) => {
                let msg = e.downcast_ref::<String>().cloned().or_else(|| e.downcast_ref::<&str>().map(|s| s.to_string())).unwrap_or_else(|| "panic".into());
                let msg = format!("{} @ {}", msg, LOC.with(|c| c.borrow().clone()));
                write_frame(&mut o, b'P', msg.as_bytes()).ok();
            }
        }
    }
}

fn run_one(kind: u8, data: &[u8], fonts: &crate::font::Fonts) {
    match kind {
        0 | 1 => {
            let html: Vec<u8> = if kind == 1 { let mut h = b"<div class=a><p id=b>text <span>more</span></p><table><tr><td>x</td></tr></table></div><style>".to_vec(); h.extend_from_slice(data); h.extend_from_slice(b"</style>"); h } else { data.to_vec() };
            let d = crate::dom::Dom::parse(&html);
            let m = crate::css::Media { width: 1280.0, height: 800.0 };
            let mut sheet = crate::css::Sheet::new();
            sheet.filter = Some(crate::css::KeyFilter::from_dom(&d));
            let mut o = 0;
            sheet.add(crate::style::UA, &m, &mut o);
            sheet.ua_rules = o;
            for (_, css) in &d.sheets { sheet.add(css, &m, &mut o); }
            let st = crate::style::compute(&d, &sheet, (1280.0, 800.0));
            let sizes = |_: u32| -> Option<(u32, u32)> { None };
            let mut lay = crate::layout::Layout::new(&d, &st, fonts, &sizes, 1280, 800);
            let h = lay.run();
            let out = std::mem::take(&mut lay.out);
            let img = |_: u32, _: u8, _: i32, _: i32| -> Option<std::rc::Rc<(u32, u32, Vec<u32>)>> { None };
            let mut store = crate::raster::Store::default();
            let _ = crate::raster::render(&out.items, fonts, &img, 1280, 0, ((h / 64) as usize).clamp(64, 1024), &mut store);
            let _ = crate::agent::markdown(&d, &st, "http://x/");
        }
        2 => { let _ = crate::img::size(data); let _ = crate::img::decode(data, 256, 256); }
        3 => { let _ = crate::svg::render_at(data, 64, 64); }
        4 => {
            // web font: container decode → face parse → shape + rasterize a few glyphs at two sizes
            fonts.add_web("fuzzfont", 400, 400, false, data.to_vec());
            let id = fonts.resolve("fuzzfont", 400, false);
            for ch in ['a', 'g', 'W', '€', '漢'] { let (fid, g, _) = fonts.advance(id, ch, 16 * 64); let _ = fonts.glyph(fid, g, 16 * 64); let _ = fonts.glyph(fid, g, 200 * 64); }
        }
        _ => {}
    }
}

fn spawn() -> Child {
    Command::new(std::env::current_exe().unwrap()).arg("fuzz-worker").stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn().expect("spawn worker")
}

/// Parent: run for `secs`, seeds from the corpus. Writes results/fuzz/summary.json + finding inputs.
pub fn run(corpus: &str, secs: u64) {
    let mut seeds: Vec<(u8, Vec<u8>)> = Vec::new();
    for e in std::fs::read_dir(corpus).unwrap().filter_map(|e| e.ok()) {
        let p = e.path();
        if let Ok(h) = std::fs::read(p.join("page.html")) { seeds.push((0, h)); }
        if let Ok(rd) = std::fs::read_dir(p.join("res")) {
            for f in rd.filter_map(|f| f.ok()) {
                let n = f.file_name().to_string_lossy().to_string();
                let b = match std::fs::read(f.path()) { Ok(b) if b.len() < 1 << 20 => b, _ => continue };
                let kind = if n.ends_with(".css") { 1 } else if n.ends_with(".img") { if crate::svg::intrinsic(&b).is_some() { 3 } else { 2 } } else if n.ends_with(".font") { 4 } else { continue };
                seeds.push((kind, b));
            }
        }
    }
    std::fs::create_dir_all("results/fuzz").ok();
    let mut r = Rng(0x9e3779b97f4a7c15 ^ std::process::id() as u64);
    let mut child = spawn();
    let (mut execs, mut panics, mut crashes, mut hangs) = (0u64, 0u64, 0u64, 0u64);
    let mut findings: HashMap<String, (u64, String)> = HashMap::new();
    let mut by_kind = [0u64; 5];
    let t0 = Instant::now();
    let mut last = Instant::now();
    while t0.elapsed() < Duration::from_secs(secs) {
        let (kind, seed) = &seeds[r.below(seeds.len())];
        let other = &seeds[r.below(seeds.len())].1;
        let input = mutate(&mut r, seed, other);
        let mut cin = child.stdin.take().unwrap();
        let mut cout = child.stdout.take().unwrap();
        let sent = write_frame(&mut cin, *kind, &input).is_ok();
        // reply with a watchdog: a separate thread reads; hang = no reply in 5 s
        let (tx, rx) = std::sync::mpsc::channel();
        let h = std::thread::spawn(move || { let f = read_frame(&mut cout); let _ = tx.send(f); cout });
        let reply = if sent { rx.recv_timeout(Duration::from_secs(5)).ok() } else { None };
        execs += 1;
        by_kind[*kind as usize] += 1;
        let mut dead = false;
        match reply {
            Some(Some((b'K', _))) => {}
            Some(Some((b'P', msg))) => {
                panics += 1;
                let full = String::from_utf8_lossy(&msg).to_string();
                let (msg_part, loc) = full.rsplit_once(" @ ").map(|(a, b)| (a.to_string(), b.to_string())).unwrap_or((full.clone(), String::new()));
                let m: String = msg_part.chars().take(160).map(|c| if c.is_ascii_digit() { 'N' } else { c }).collect();
                let m = { let mut o = String::new(); for c in m.chars() { if !(c == 'N' && o.ends_with('N')) { o.push(c); } } format!("{o} @ {loc}") };
                let e = findings.entry(format!("panic: {m}")).or_insert((0, String::new()));
                e.0 += 1;
                if e.1.is_empty() { let f = format!("results/fuzz/panic-{:016x}.k{kind}", r.next()); std::fs::write(&f, &input).ok(); e.1 = f; }
            }
            None => { hangs += 1; dead = true; let f = format!("results/fuzz/hang-{:016x}.k{kind}", r.next()); std::fs::write(&f, &input).ok(); findings.entry("hang (>5 s)".into()).or_insert((0, f)).0 += 1; }
            Some(_) => {
                crashes += 1; dead = true;
                let f = format!("results/fuzz/crash-{:016x}.k{kind}", r.next());
                std::fs::write(&f, &input).ok();
                findings.entry("crash/abort/oom".into()).or_insert((0, f)).0 += 1;
            }
        }
        if dead {
            child.kill().ok(); child.wait().ok();
            drop(cin);
            child = spawn();
            if let Ok(c) = h.join() { drop(c); }
        } else {
            child.stdin = Some(cin);
            child.stdout = Some(h.join().unwrap());
        }
        if last.elapsed() > Duration::from_secs(30) {
            last = Instant::now();
            eprintln!("fuzz {:>5.0}s execs {execs} ({:.0}/s) panics {panics} crashes {crashes} hangs {hangs} unique {}", t0.elapsed().as_secs_f64(), execs as f64 / t0.elapsed().as_secs_f64(), findings.len());
        }
    }
    child.kill().ok();
    let secs_run = t0.elapsed().as_secs_f64();
    let mut f: Vec<_> = findings.iter().collect();
    f.sort_by(|a, b| b.1.0.cmp(&a.1.0));
    println!("FUZZ {:.0} s · {execs} execs ({:.0}/s) · html {} css {} img {} svg {} font {} · panics {panics} crashes {crashes} hangs {hangs} · unique findings {}",
        secs_run, execs as f64 / secs_run, by_kind[0], by_kind[1], by_kind[2], by_kind[3], by_kind[4], findings.len());
    for (k, (n, file)) in &f { println!("  {n:>6}  {k}  → {file}"); }
    let j = serde_json::json!({"seconds": secs_run, "execs": execs, "execs_per_s": execs as f64 / secs_run, "panics": panics, "crashes": crashes, "hangs": hangs,
        "by_kind": {"html": by_kind[0], "css": by_kind[1], "img": by_kind[2], "svg": by_kind[3], "font": by_kind[4]},
        "findings": f.iter().map(|(k, (n, file))| serde_json::json!({"what": k, "count": n, "input": file})).collect::<Vec<_>>()});
    std::fs::write("results/fuzz/summary.json", serde_json::to_string_pretty(&j).unwrap()).ok();
    let _ = std::io::stdout().flush();
}
