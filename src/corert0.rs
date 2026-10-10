// SPDX-License-Identifier: MIT OR Apache-2.0
//! CORE-RT-0 (OMNI-RENDER-0 step 9; plan an unpublished plan step 3): our cores INSIDE the engine's
//! frame loop, reading the packed DOM where it sits. Per page: parse → style → layout once, then a scripted scroll; every frame
//! rasterises the viewport (the engine's own work) and ticks the page core:
//!   (a) query rendered from the packed DOM columns of the node under the pointer (no copy, no serialise) → JIT sift over the
//!       page's marks (one hash-rendered phase codeword per element, d 1,024) → top-8;
//!   (b) one rg1 ternary MLP head (up 512×128 → clamp → down 128×512) rendered as add/sub code, 8 lanes = the top-8 marks;
//!   (c) the trial model's ring_core beat (libringcore.so, read-only dlopen) on a 64-state machine.
//! Compared against the same sift through a Unix socket with newline JSON (in-binary twin = transport cost) and an omnid ping.
use crate::jit::{p2, sift, ternary};
use crate::{dom, layout, page, raster};
use serde_json::json;
use std::io::{BufRead, BufReader, Write};
use std::time::Instant;

const D: usize = 1024;

#[inline]
fn splitmix(x: &mut u64) -> u64 {
    *x = x.wrapping_add(0x9E3779B97F4A7C15);
    let mut z = *x; z = (z ^ (z >> 30)).wrapping_mul(0xBF58476D1CE4E5B9); z = (z ^ (z >> 27)).wrapping_mul(0x94D049BB133111EB); z ^ (z >> 31)
}

/// The node's key, read in place from the packed columns: tag id, parent tag id, attribute names + values (bytes in dom.bytes).
fn node_key(d: &dom::Dom, i: u32) -> u64 {
    let n = &d.nodes[i as usize];
    let mut h: u64 = 0xcbf29ce484222325;
    let mut eat = |b: u8| { h ^= b as u64; h = h.wrapping_mul(0x100000001b3); };
    for b in n.name.to_le_bytes() { eat(b) }
    let pn = d.nodes.get(n.parent as usize).map(|p| p.name).unwrap_or(0);
    for b in pn.to_le_bytes() { eat(b) }
    for a in d.attr_list(i) {
        for b in a.name.to_le_bytes() { eat(b) }
        let s = &d.bytes[a.val.off as usize..(a.val.off + a.val.len) as usize];
        for &b in &s[..s.len().min(64)] { eat(b) }
    }
    h
}

/// Hash-rendered phase codeword (the trial model's "keys rendered per beat, never stored" discipline): 4-bit phases from splitmix64.
fn codeword(key: u64, out: &mut [u8]) {
    let mut st = key.wrapping_mul(1000003).wrapping_add(7);
    for c in out.chunks_mut(16) { let z = splitmix(&mut st); for (j, v) in c.iter_mut().enumerate() { *v = ((z >> (4 * j)) & 15) as u8; } }
}

fn top8(s: &[i32]) -> [u32; 8] {
    let mut t = [(i32::MIN, 0u32); 8];
    for (k, &v) in s.iter().enumerate() {
        if v > t[7].0 { let mut j = 7; while j > 0 && t[j - 1].0 < v { t[j] = t[j - 1]; j -= 1; } t[j] = (v, k as u32); }
    }
    t.map(|x| x.1)
}

fn pct(v: &mut Vec<f64>, p: f64) -> f64 { if v.is_empty() { return 0.0; } v.sort_by(|a, b| a.partial_cmp(b).unwrap()); v[((v.len() - 1) as f64 * p) as usize] }

// ---- the trial model's ring_core (another agent's library, used read-only by path; absent → that leg is skipped and reported)
type RcCreate = unsafe extern "C" fn(i32, i32, *const u8, *const u8, i32) -> *mut libc::c_void;
type RcWrite = unsafe extern "C" fn(*mut libc::c_void, i64, i32, i64);
type RcTick = unsafe extern "C" fn(*mut libc::c_void, *const i32, i32, i64, i32, *mut i32, *mut i32, *mut f32) -> i32;
type RcOut = unsafe extern "C" fn(*mut libc::c_void, *mut i64, i32) -> i32;
struct Ring { h: *mut libc::c_void, tick: RcTick, out: RcOut, trans: std::collections::HashMap<(i64, i32), i64>, s: i64, states: Vec<i64> }
impl Ring {
    fn load(d: usize) -> Option<Ring> {
        let p = std::ffi::CString::new(format!("{}/ai-stack/projects/omnid/ring_core/libringcore.so", std::env::var("HOME").ok()?)).ok()?;
        unsafe {
            let lib = libc::dlopen(p.as_ptr(), libc::RTLD_NOW);
            if lib.is_null() { return None; }
            let sym = |n: &std::ffi::CStr| { let s = libc::dlsym(lib, n.as_ptr()); if s.is_null() { None } else { Some(s) } };
            let create: RcCreate = std::mem::transmute(sym(c"rc_create")?);
            let write: RcWrite = std::mem::transmute(sym(c"rc_write")?);
            let tick: RcTick = std::mem::transmute(sym(c"rc_tick")?);
            let out: RcOut = std::mem::transmute(sym(c"rc_out")?);
            let mut st = 9u64;
            let cb: Vec<u8> = (0..158 * d).map(|_| (splitmix(&mut st) & 15) as u8).collect();
            let inp: Vec<u8> = (0..16 * d).map(|_| (splitmix(&mut st) & 15) as u8).collect();
            let h = create(d as i32, 16, cb.as_ptr(), inp.as_ptr(), 8);
            let states: Vec<i64> = (0..64).map(|_| (splitmix(&mut st) % 23205) as i64).collect();
            let mut trans = std::collections::HashMap::new();
            for &s in &states { for i in 0..16 { let t = states[(splitmix(&mut st) % 64) as usize]; write(h, s, i, t); trans.insert((s, i), t); } }
            Some(Ring { h, tick, out, trans, s: states[0], states })
        }
    }
    /// One beat: [UNBIND, SIEVE, RRNS 0, THRESH 7, PROG 1, COMMIT] (layer1 ids = op*8 + arg). → committed the true next state?
    fn beat(&mut self, i: i32) -> bool {
        const P0: [i32; 6] = [8, 16, 24, 39, 57, 48];
        let (mut ab, mut nx, mut mg) = (0i32, 0i32, 0f32);
        let mut o = [0i64; 8];
        unsafe {
            let c = (self.tick)(self.h, P0.as_ptr(), 6, self.s, i, &mut ab, &mut nx, &mut mg);
            let n = (self.out)(self.h, o.as_mut_ptr(), 8);
            let want = self.trans[&(self.s, i)];
            let ok = c != 0 && n >= 1 && o[0] == want;
            self.s = if ok { want } else { self.states[(i as usize * 7) % 64] };
            ok
        }
    }
}

/// In-binary socket twin: the same sift + top-8 behind a Unix socket speaking newline JSON (omnid's framing).
fn socket_twin(path: &str, lat: std::sync::Arc<Vec<u8>>) -> std::thread::JoinHandle<()> {
    let _ = std::fs::remove_file(path);
    let l = std::os::unix::net::UnixListener::bind(path).unwrap();
    std::thread::spawn(move || {
        crate::jit::pin(3);   // its own core: the client thread is pinned to CPU 2
        let k = sift::render(D);
        if let Ok((s, _)) = l.accept() {
            let mut w = s.try_clone().unwrap();
            let mut r = BufReader::new(s);
            let mut line = String::new();
            let n = lat.len() / D;
            let mut sc = vec![0i32; n];
            let mut q = vec![0u8; D];
            loop {
                // framing: '{' starts a JSON line (omnid's framing); 'B' is followed by D raw phase bytes (binary framing)
                let first = match r.fill_buf() { Ok(b) if !b.is_empty() => b[0], _ => break };
                let json_mode = first == b'{';
                if json_mode {
                    line.clear(); if r.read_line(&mut line).unwrap_or(0) == 0 { break; }
                    let v: serde_json::Value = serde_json::from_str(&line).unwrap();
                    for (d, x) in q.iter_mut().zip(v["q"].as_array().unwrap()) { *d = x.as_u64().unwrap() as u8; }
                } else {
                    use std::io::Read;
                    let mut tag = [0u8; 1]; r.read_exact(&mut tag).unwrap(); r.read_exact(&mut q).unwrap();
                }
                let (qr, qi) = sift::query(&q);
                k.run(&lat, &qr, &qi, &mut sc, 1);
                let t = top8(&sc);
                if json_mode { w.write_all(format!("{}\n", json!({"top": t})).as_bytes()).unwrap(); }
                else { let mut b = [0u8; 32]; for (j, v) in t.iter().enumerate() { b[4 * j..4 * j + 4].copy_from_slice(&v.to_le_bytes()); } w.write_all(&b).unwrap(); }
            }
        }
    })
}

fn omnid_ping(n: usize) -> Option<f64> {
    let s = std::os::unix::net::UnixStream::connect("/tmp/omnid.sock").ok()?;
    s.set_read_timeout(Some(std::time::Duration::from_secs(2))).ok()?;
    let mut w = s.try_clone().ok()?; let mut r = BufReader::new(s); let mut v = Vec::new(); let mut line = String::new();
    for _ in 0..n {
        let t = Instant::now();
        w.write_all(b"{\"op\":\"ping\"}\n").ok()?; line.clear(); r.read_line(&mut line).ok()?;
        v.push(t.elapsed().as_secs_f64() * 1e6);
    }
    Some(pct(&mut v, 0.5))
}

pub fn main(dir: &str, frames: usize) {
    crate::jit::pin(2);
    let fonts = crate::font::Fonts::new();
    let golden = std::fs::read("results/rg1_trits.bin").ok();
    // rg1 arm A block 0: up (512×128) and down (128×512) — matrices 4 and 5 in export order (n1? no: 2-D only: qkv, o, up, down)
    let mats: Vec<(usize, usize, Vec<i8>)> = golden.map(|b| {
        let cnt = u32::from_le_bytes(b[4..8].try_into().unwrap()) as usize; let mut o = 8; let mut v = vec![];
        for _ in 0..cnt { let r = u32::from_le_bytes(b[o..o + 4].try_into().unwrap()) as usize; let c = u32::from_le_bytes(b[o + 4..o + 8].try_into().unwrap()) as usize; o += 8; v.push((r, c, b[o..o + r * c].iter().map(|&x| x as i8).collect())); o += r * c; }
        v }).unwrap_or_default();
    let (up, down) = (&mats[2], &mats[3]);
    assert_eq!((up.0, up.1, down.0, down.1), (512, 128, 128, 512));
    let t_r = Instant::now();
    let h_up = ternary::render_b8(&up.2, up.0, up.1);
    let h_dn = ternary::render_b8(&down.2, down.0, down.1);
    let head_render_ms = t_r.elapsed().as_secs_f64() * 1e3;
    let ksift = sift::render(D);
    let mut ring = Ring::load(D);
    let ring_ok = ring.is_some();
    let sock = format!("/tmp/claude-1000/omr-core-{}.sock", std::process::id());
    let ping = omnid_ping(200);
    let mut ents: Vec<_> = std::fs::read_dir(dir).unwrap().filter_map(|e| e.ok()).map(|e| e.path()).filter(|p| p.join("manifest.json").exists()).collect();
    ents.sort();
    let mut rows = vec![];
    let (mut all_core, mut all_frame, mut all_sock, mut all_inproc, mut all_bin) = (vec![], vec![], vec![], vec![], vec![]);
    let (mut self_hits, mut queries, mut head_bad, mut ring_hits, mut ring_beats) = (0usize, 0usize, 0usize, 0usize, 0usize);
    for p in ents {
        let name = p.file_name().unwrap().to_string_lossy().to_string();
        let Some(b) = page::Bundle::open(&p) else { continue };
        let l = page::load(&b, (1280.0, 800.0));
        let mut base = b.url.clone();
        for i in 0..l.dom.nodes.len() as u32 { if l.dom.tag(i) == "base" { if let Some(h) = l.dom.attr(i, "href") { base = page::resolve(&b.url, h); break; } } }
        let dref = &l.dom;
        let sizes = |n: u32| -> Option<(u32, u32)> { crate::img_bytes(dref, n, &base, &b).and_then(|bytes| crate::img::size(&bytes)) };
        let mut lay = layout::Layout::new(&l.dom, &l.styled, &fonts, &sizes, 1280, 800);
        let page_h = lay.run();
        let out = std::mem::take(&mut lay.out);
        drop(lay);
        // the page core's marks: one codeword per element, rendered from the packed columns
        let t_ix = Instant::now();
        let elems: Vec<u32> = (0..l.dom.nodes.len() as u32).filter(|&i| matches!(l.dom.nodes[i as usize].kind, dom::Kind::Element)).collect();
        let mut lat = vec![0u8; elems.len() * D];
        for (k, &e) in elems.iter().enumerate() { codeword(node_key(&l.dom, e), &mut lat[k * D..(k + 1) * D]); }
        let index_ms = t_ix.elapsed().as_secs_f64() * 1e3;
        let mut ix_of = std::collections::HashMap::new(); for (k, &e) in elems.iter().enumerate() { ix_of.insert(e, k); }
        let lat_arc = std::sync::Arc::new(lat.clone());
        let srv = socket_twin(&sock, lat_arc);
        std::thread::sleep(std::time::Duration::from_millis(5));
        let cs = std::os::unix::net::UnixStream::connect(&sock).unwrap();
        let mut cw = cs.try_clone().unwrap(); let mut cr = BufReader::new(cs);
        let imgf = |_n: u32, _mode: u8, _w: i32, _h: i32| -> Option<std::rc::Rc<(u32, u32, Vec<u32>)>> { None };
        let max_y = ((page_h / crate::style::FX) as usize).saturating_sub(800);
        let mut store = raster::Store::default();
        let (mut fr, mut core, mut sk, mut inproc, mut bin) = (vec![], vec![], vec![], vec![], vec![]);
        let (mut t_q, mut t_s, mut t_h, mut t_rg) = (vec![], vec![], vec![], vec![]);
        let mut q = vec![0u8; D]; let mut sc = vec![0i32; elems.len()];
        let mut x8 = vec![0i32; 128 * 8]; let mut y_up = vec![0i32; 512 * 8]; let mut x_dn = vec![0i32; 512 * 8]; let mut y_dn = vec![0i32; 128 * 8];
        let mut line = String::new();
        for f in 0..frames {
            let y = if max_y == 0 { 0 } else { (f * 37) % (max_y + 1) };
            let t0 = Instant::now();
            let _pg = raster::render(&out.items, &fonts, &imgf, 1280, y, y + 800, &mut store);
            let t_frame = t0.elapsed().as_secs_f64() * 1e6;
            // pointer on a Lissajous path over the viewport → node under it (topmost box containing the point)
            let (px, py) = (640.0 + 560.0 * (f as f64 * 0.13).sin(), y as f64 + 400.0 + 360.0 * (f as f64 * 0.07).cos());
            let (pxf, pyf) = ((px * crate::style::FX as f64) as i32, (py * crate::style::FX as f64) as i32);
            let hit = out.boxes.iter().rev().find(|(_, bx)| pxf >= bx[0] && pxf < bx[0] + bx[2] && pyf >= bx[1] && pyf < bx[1] + bx[3]).map(|x| x.0)
                .filter(|n| ix_of.contains_key(n)).unwrap_or(elems.get(f % elems.len().max(1)).copied().unwrap_or(0));
            if elems.is_empty() { break; }
            // ---- core tick (in process, zero copy)
            let c0 = Instant::now();
            codeword(node_key(&l.dom, hit), &mut q);
            let c1 = Instant::now();
            let (qr, qi) = sift::query(&q);
            ksift.run(&lat, &qr, &qi, &mut sc, 1);
            let t8 = top8(&sc);
            let c2 = Instant::now();
            // head lanes = the top-8 marks' first 128 phases as C8 activations
            for (lane, &m) in t8.iter().enumerate() { for i in 0..128 { x8[i * 8 + lane] = sift::C8[(lat[m as usize * D + i] & 15) as usize] as i32; } }
            h_up.run_b8(&x8, &mut y_up);
            for (a, &v) in x_dn.iter_mut().zip(&y_up) { *a = (v >> 6).clamp(0, 127); }  // ReLU + requantise to int8 range
            h_dn.run_b8(&x_dn, &mut y_dn);
            let c3 = Instant::now();
            let mut ring_us = 0.0;
            if let Some(r) = ring.as_mut() { let t = Instant::now(); ring_hits += r.beat((f % 16) as i32) as usize; ring_beats += 1; ring_us = t.elapsed().as_secs_f64() * 1e6; }
            let c_all = c0.elapsed().as_secs_f64() * 1e6;
            self_hits += (t8[0] as usize == ix_of[&hit] || sc[t8[0] as usize] == sc[ix_of[&hit]]) as usize; queries += 1;
            if f == 0 {   // exactness of the in-frame head against the scalar reference, once per page
                for lane in 0..8 {
                    let xs: Vec<i8> = (0..128).map(|i| x8[i * 8 + lane] as i8).collect(); let mut r = vec![0; 512]; p2::reference(&up.2, 512, 128, &xs, &mut r);
                    head_bad += (0..512).filter(|&o| r[o] != y_up[o * 8 + lane]).count();
                    let xd: Vec<i8> = (0..512).map(|i| x_dn[i * 8 + lane] as i8).collect(); let mut r2 = vec![0; 128]; p2::reference(&down.2, 128, 512, &xd, &mut r2);
                    head_bad += (0..128).filter(|&o| r2[o] != y_dn[o * 8 + lane]).count();
                }
            }
            t_q.push((c1 - c0).as_secs_f64() * 1e6); t_s.push((c2 - c1).as_secs_f64() * 1e6); t_h.push((c3 - c2).as_secs_f64() * 1e6); t_rg.push(ring_us);
            // ---- same query through the socket twin (serialise → transport → parse → same kernel → reply)
            let s0 = Instant::now();
            cw.write_all(format!("{}\n", json!({"q": q})).as_bytes()).unwrap();
            line.clear(); cr.read_line(&mut line).unwrap();
            let v: serde_json::Value = serde_json::from_str(&line).unwrap();
            let sock_us = s0.elapsed().as_secs_f64() * 1e6;
            debug_assert_eq!(v["top"][0].as_u64().unwrap() as u32, t8[0]);
            let b0 = Instant::now();
            { use std::io::Read; let mut m = Vec::with_capacity(D + 1); m.push(b'B'); m.extend_from_slice(&q); cw.write_all(&m).unwrap();
              let mut rb = [0u8; 32]; cr.read_exact(&mut rb).unwrap(); assert_eq!(u32::from_le_bytes(rb[..4].try_into().unwrap()), t8[0]); }
            bin.push(b0.elapsed().as_secs_f64() * 1e6);
            fr.push(t_frame); core.push(c_all); sk.push(sock_us); inproc.push((c2 - c1 + (c1 - c0)).as_secs_f64() * 1e6);
        }
        drop(cw); drop(cr); let _ = srv.join(); let _ = std::fs::remove_file(&sock);
        let row = json!({"page": name, "nodes": l.dom.nodes.len(), "elements": elems.len(), "index_ms": index_ms, "frames": fr.len(),
            "raster_vp_us_p50": pct(&mut fr.clone(), 0.5), "core_us_p50": pct(&mut core.clone(), 0.5), "core_us_p99": pct(&mut core.clone(), 0.99),
            "query_render_us_p50": pct(&mut t_q, 0.5), "sift_top8_us_p50": pct(&mut t_s, 0.5), "head_us_p50": pct(&mut t_h, 0.5), "ring_us_p50": pct(&mut t_rg, 0.5),
            "inproc_sift_us_p50": pct(&mut inproc.clone(), 0.5), "socket_sift_us_p50": pct(&mut sk.clone(), 0.5), "socket_bin_sift_us_p50": pct(&mut bin.clone(), 0.5),
            "core_share_of_16_6ms_p50": pct(&mut core.clone(), 0.5) / 16_667.0});
        println!("{:<40} elems {:>6} | raster {:>7.0} µs | core {:>6.0} µs p50 ({:>5.0} p99) = query {:>4.1} + sift {:>6.0} + head {:>5.1} + ring {:>5.0} | {:>5.2}% of frame | socket json {:>6.0} bin {:>5.0} µs vs in-proc {:>6.0}",
            &name[..name.len().min(40)], elems.len(), row["raster_vp_us_p50"].as_f64().unwrap(), row["core_us_p50"].as_f64().unwrap(), row["core_us_p99"].as_f64().unwrap(),
            row["query_render_us_p50"].as_f64().unwrap(), row["sift_top8_us_p50"].as_f64().unwrap(), row["head_us_p50"].as_f64().unwrap(), row["ring_us_p50"].as_f64().unwrap(),
            100.0 * row["core_share_of_16_6ms_p50"].as_f64().unwrap(), row["socket_sift_us_p50"].as_f64().unwrap(), row["socket_bin_sift_us_p50"].as_f64().unwrap(), row["inproc_sift_us_p50"].as_f64().unwrap());
        all_core.extend(core); all_frame.extend(fr); all_sock.extend(sk); all_inproc.extend(inproc); all_bin.extend(bin);
        rows.push(row);
    }
    let b50 = pct(&mut all_bin, 0.5);
    let (c50, c99, f50, s50, i50) = (pct(&mut all_core.clone(), 0.5), pct(&mut all_core, 0.99), pct(&mut all_frame, 0.5), pct(&mut all_sock, 0.5), pct(&mut all_inproc, 0.5));
    let res = json!({"d": D, "frames_per_page": frames, "pages": rows.len(), "head": "rg1 arm A block0 up 512x128 -> clamp -> down 128x512, 8 lanes, rendered as code",
        "head_render_ms": head_render_ms, "head_code_bytes": h_up.exec.code_bytes + h_dn.exec.code_bytes, "head_mismatches": head_bad,
        "ring_core": if ring_ok { json!({"beats": ring_beats, "committed_correct": ring_hits, "d": D}) } else { json!("libringcore.so absent — leg skipped") },
        "self_recall_top1": self_hits as f64 / queries.max(1) as f64, "queries": queries,
        "all_frames": {"core_us_p50": c50, "core_us_p99": c99, "core_share_p50": c50 / 16_667.0, "core_share_p99": c99 / 16_667.0,
            "raster_vp_us_p50": f50, "inproc_sift_us_p50": i50, "socket_json_sift_us_p50": s50, "socket_over_inproc": s50 / i50.max(1e-9), "socket_bin_sift_us_p50": b50, "socket_bin_over_inproc": b50 / i50.max(1e-9), "bin_transport_us_p50": b50 - i50,
            "transport_us_p50": s50 - i50, "omnid_ping_us_p50": ping},
        "rows": rows,
        "notes": "raster = viewport re-raster per frame, images not decoded (frame-cost baseline only); socket twin = same kernel behind a Unix socket with newline JSON (omnid framing); omnid ping = deployed transport floor"});
    std::fs::write("results/corert0.json", serde_json::to_string_pretty(&res).unwrap()).unwrap();
    println!("ALL {} frames: core {:.0} µs p50 / {:.0} p99 = {:.2}% / {:.2}% of a 16.6 ms frame | raster {:.0} µs | sift in-proc {:.0} µs vs socket+JSON {:.0} µs ({:.1}x) vs socket+binary {:.0} µs | omnid ping {:?} µs | head mismatches {} | ring {}/{} | self-recall {:.3}",
        all_frame.len(), c50, c99, 100.0 * c50 / 16_667.0, 100.0 * c99 / 16_667.0, f50, i50, s50, s50 / i50.max(1e-9), b50, ping, head_bad, ring_hits, ring_beats, self_hits as f64 / queries.max(1) as f64);
}
