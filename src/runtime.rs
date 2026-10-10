// SPDX-License-Identifier: MIT OR Apache-2.0
//! RUNTIME-0 (OMNI-RENDER-0 step 7): the program runtime beside the document engine. Apps compiled to WASM (C, Rust, …) run
//! sandboxed (wasmtime/Cranelift: linear-memory isolation, fuel metering, no ambient I/O); the runtime grants them our cores as
//! intrinsics in the `omni` import module, and every intrinsic runs on the app's OWN memory in place (zero copy out of the
//! sandbox): P2 ternary top-k, the JIT-rendered phase sift, RNS exact multiply, integer 2T rotors, plus read-only files under a
//! granted root and query embedding through omnid.
use crate::jit::{p2, sift};
use serde_json::json;
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::time::Instant;
use wasmtime::{Caller, Config, Engine, Linker, Module, Store};

pub const M10: [i64; 10] = [3, 5, 7, 11, 13, 17, 19, 23, 29, 31];

pub struct Host { pub root: std::path::PathBuf, pub out: Vec<String>, sifts: HashMap<usize, sift::Sift>, pub embed_ms: f64, pub kernel_us: f64 }

fn mem<'a>(c: &'a mut Caller<'_, Host>) -> &'a mut [u8] {
    let m = c.get_export("memory").and_then(|e| e.into_memory()).expect("app exports memory");
    m.data_mut(c)
}
fn span(m: &[u8], p: u32, len: usize) -> wasmtime::Result<std::ops::Range<usize>> {
    let (a, b) = (p as usize, p as usize + len);
    if b > m.len() { wasmtime::bail!("out of bounds guest access {a}..{b} (memory {})", m.len()) }
    Ok(a..b)
}

/// Embed one query through omnid (nomic, search_query prefix) → f32[768].
pub fn embed(texts: &[String], prefix: &str) -> Option<Vec<Vec<f32>>> {
    let s = std::os::unix::net::UnixStream::connect("/tmp/omnid.sock").ok()?;
    let mut w = s.try_clone().ok()?; let mut r = BufReader::new(s);
    let t: Vec<String> = texts.iter().map(|x| format!("{prefix}{x}")).collect();
    w.write_all(format!("{}\n", json!({"op": "embed", "texts": t, "model": "nomic"})).as_bytes()).ok()?;
    let mut line = String::new(); r.read_line(&mut line).ok()?;
    let v: serde_json::Value = serde_json::from_str(&line).ok()?;
    Some(v["vectors"].as_array()?.iter().map(|x| x.as_array().unwrap().iter().map(|f| f.as_f64().unwrap() as f32).collect()).collect())
}

/// int8 quantisation as the engines do it: g = 127 / max|x|, round, |q| ≤ 127.
pub fn quant_i8(x: &[f32]) -> Vec<i8> {
    let mx = x.iter().fold(1e-9f32, |a, &b| a.max(b.abs())); let g = 127.0 / mx;
    x.iter().map(|&v| (v * g).round().clamp(-127.0, 127.0) as i8).collect()
}

/// exact top-k of P2 row scores (ties → lower index first)
pub fn topk(scores: &[i32], k: usize) -> Vec<(u32, i32)> {
    let mut ix: Vec<u32> = (0..scores.len() as u32).collect();
    let k = k.min(ix.len());
    if k == 0 { return vec![]; }
    ix.select_nth_unstable_by(k - 1, |&a, &b| scores[b as usize].cmp(&scores[a as usize]).then(a.cmp(&b)));
    let mut t: Vec<(u32, i32)> = ix[..k].iter().map(|&i| (i, scores[i as usize])).collect();
    t.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    t
}

pub fn linker(engine: &Engine) -> Linker<Host> {
    let mut l = Linker::new(engine);
    l.func_wrap("omni", "print", |mut c: Caller<'_, Host>, p: u32, n: u32| -> wasmtime::Result<()> {
        let m = mem(&mut c); let r = span(m, p, n as usize)?; let s = String::from_utf8_lossy(&m[r]).to_string();
        println!("  [app] {s}"); c.data_mut().out.push(s); Ok(())
    }).unwrap();
    // read-only files under the granted root; no absolute paths, no `..`
    fn granted(c: &mut Caller<'_, Host>, p: u32, n: u32) -> wasmtime::Result<Option<std::path::PathBuf>> {
        let m = mem(c); let r = span(m, p, n as usize)?;
        let rel = String::from_utf8_lossy(&m[r]).to_string();
        if rel.starts_with('/') || rel.split('/').any(|s| s == "..") { return Ok(None); }
        Ok(Some(c.data().root.join(rel)))
    }
    l.func_wrap("omni", "file_size", |mut c: Caller<'_, Host>, p: u32, n: u32| -> wasmtime::Result<i64> {
        Ok(match granted(&mut c, p, n)? { Some(f) => std::fs::metadata(f).map(|m| m.len() as i64).unwrap_or(-1), None => -1 })
    }).unwrap();
    l.func_wrap("omni", "file_read", |mut c: Caller<'_, Host>, p: u32, n: u32, dst: u32, cap: u32| -> wasmtime::Result<i64> {
        let Some(f) = granted(&mut c, p, n)? else { return Ok(-1) };
        let Ok(b) = std::fs::read(f) else { return Ok(-1) };
        let m = mem(&mut c); let k = b.len().min(cap as usize); let r = span(m, dst, k)?; m[r].copy_from_slice(&b[..k]); Ok(k as i64)
    }).unwrap();
    l.func_wrap("omni", "embed_query", |mut c: Caller<'_, Host>, t: u32, n: u32, mean: u32, dim: u32, dst: u32| -> wasmtime::Result<i32> {
        let m = mem(&mut c);
        let text = String::from_utf8_lossy(&m[span(m, t, n as usize)?]).to_string();
        let mr = span(m, mean, 4 * dim as usize)?;
        let mu: Vec<f32> = m[mr].chunks(4).map(|b| f32::from_le_bytes(b.try_into().unwrap())).collect();
        let t0 = Instant::now();
        let Some(v) = embed(&[text], "search_query: ") else { return Ok(1) };
        let ms = t0.elapsed().as_secs_f64() * 1e3;
        let x: Vec<f32> = v[0].iter().zip(&mu).map(|(a, b)| a - b).collect();
        let q = quant_i8(&x);
        let m = mem(&mut c); let r = span(m, dst, dim as usize)?; for (d, s) in m[r].iter_mut().zip(&q) { *d = *s as u8; }
        c.data_mut().embed_ms += ms; Ok(0)
    }).unwrap();
    l.func_wrap("omni", "p2_topk", |mut c: Caller<'_, Host>, rows: u32, n: u32, dim: u32, x: u32, k: u32, out: u32| -> wasmtime::Result<u32> {
        if dim % 32 != 0 { wasmtime::bail!("dim must be a multiple of 32") }
        let t0 = Instant::now();
        let m = mem(&mut c);
        let rr = span(m, rows, p2::row_bytes(dim as usize) * n as usize)?; let xr = span(m, x, dim as usize)?;
        let xs: &[i8] = unsafe { std::slice::from_raw_parts(m[xr].as_ptr() as *const i8, dim as usize) };
        let mut sc = vec![0i32; n as usize];
        unsafe { p2::gemv_b1(&m[rr], n as usize, dim as usize, xs, &mut sc) };   // the kernel reads the app's memory in place
        let t = topk(&sc, k as usize);
        let or = span(m, out, 8 * t.len())?;
        for (j, (i, s)) in t.iter().enumerate() { m[or.start + 8 * j..or.start + 8 * j + 4].copy_from_slice(&(*i as i32).to_le_bytes()); m[or.start + 8 * j + 4..or.start + 8 * j + 8].copy_from_slice(&s.to_le_bytes()); }
        c.data_mut().kernel_us += t0.elapsed().as_secs_f64() * 1e6;
        Ok(t.len() as u32)
    }).unwrap();
    l.func_wrap("omni", "sift_topk", |mut c: Caller<'_, Host>, lat: u32, n: u32, d: u32, q: u32, k: u32, out: u32| -> wasmtime::Result<u32> {
        if d % 64 != 0 { wasmtime::bail!("d must be a multiple of 64") }
        let t0 = Instant::now();
        let k_ = c.data_mut().sifts.entry(d as usize).or_insert_with(|| sift::render(d as usize)) as *const sift::Sift;
        let m = mem(&mut c);
        let lr = span(m, lat, (n * d) as usize)?; let qr_ = span(m, q, d as usize)?;
        let (qr, qi) = sift::query(&m[qr_]);
        let mut sc = vec![0i32; n as usize];
        unsafe { (*k_).run(&m[lr], &qr, &qi, &mut sc, 1) };
        let t = topk(&sc, k as usize);
        let or = span(m, out, 4 * t.len())?;
        for (j, (i, _)) in t.iter().enumerate() { m[or.start + 4 * j..or.start + 4 * j + 4].copy_from_slice(&(*i as i32).to_le_bytes()); }
        c.data_mut().kernel_us += t0.elapsed().as_secs_f64() * 1e6;
        Ok(t.len() as u32)
    }).unwrap();
    l.func_wrap("omni", "rns_mul", |mut c: Caller<'_, Host>, a: i64, b: i64, out: u32| -> wasmtime::Result<i32> {
        // carry-free: each ring multiplies its own residues; exact for |a·b| below Π m (law rns.mul.exact_below_M)
        let m = mem(&mut c); let r = span(m, out, 10)?;
        for (k, md) in M10.iter().enumerate() { m[r.start + k] = ((a.rem_euclid(*md) * b.rem_euclid(*md)) % md) as u8; }
        Ok(10)
    }).unwrap();
    l.func_wrap("omni", "rotor2t", |mut c: Caller<'_, Host>, q: u32, v: u32, out: u32| -> wasmtime::Result<()> {
        let m = mem(&mut c);
        let rd = |m: &[u8], p: u32| -> wasmtime::Result<[i128; 4]> { let r = span(m, p, 32)?; let mut o = [0i128; 4]; for j in 0..4 { o[j] = i64::from_le_bytes(m[r.start + 8 * j..r.start + 8 * j + 8].try_into().unwrap()) as i128; } Ok(o) };
        let (q, v) = (rd(m, q)?, rd(m, v)?);
        let mul = |a: [i128; 4], b: [i128; 4]| [a[0] * b[0] - a[1] * b[1] - a[2] * b[2] - a[3] * b[3], a[0] * b[1] + a[1] * b[0] + a[2] * b[3] - a[3] * b[2],
                                                 a[0] * b[2] - a[1] * b[3] + a[2] * b[0] + a[3] * b[1], a[0] * b[3] + a[1] * b[2] - a[2] * b[1] + a[3] * b[0]];
        let o = mul(mul(q, v), [q[0], -q[1], -q[2], -q[3]]);
        let r = span(m, out, 32)?;
        for j in 0..4 { let x = i64::try_from(o[j]).map_err(|_| wasmtime::Error::msg("rotor overflow"))?; m[r.start + 8 * j..r.start + 8 * j + 8].copy_from_slice(&x.to_le_bytes()); }
        Ok(())
    }).unwrap();
    l
}

pub fn engine() -> Engine {
    let mut cfg = Config::new();
    cfg.consume_fuel(true);
    cfg.cranelift_opt_level(wasmtime::OptLevel::Speed);
    Engine::new(&cfg).unwrap()
}

pub fn new_store(engine: &Engine, root: &std::path::Path) -> Store<Host> {
    let mut s = Store::new(engine, Host { root: root.to_path_buf(), out: vec![], sifts: HashMap::new(), embed_ms: 0.0, kernel_us: 0.0 });
    s.set_fuel(20_000_000_000).unwrap();
    s
}

/// Write a Find index: every text file (.md) under `root` → nomic (search_document) → centred → absmean trits → P2 rows.
pub fn find_index(root: &str, out: &str) {
    let mut files = vec![];
    fn walk(d: &std::path::Path, v: &mut Vec<std::path::PathBuf>) {
        if let Ok(rd) = std::fs::read_dir(d) { for e in rd.flatten() { let p = e.path(); if p.is_dir() { walk(&p, v) } else if p.extension().map(|x| x == "md").unwrap_or(false) { v.push(p) } } }
    }
    walk(std::path::Path::new(root), &mut files);
    files.sort();
    let texts: Vec<String> = files.iter().map(|f| { let s = std::fs::read_to_string(f).unwrap_or_default(); s.chars().take(1200).collect() }).collect();
    let t0 = Instant::now();
    let mut vecs: Vec<Vec<f32>> = vec![];
    for ch in texts.chunks(16) { vecs.extend(embed(ch, "search_document: ").expect("omnid embed")); eprintln!("embedded {}/{} ({:.0} s)", vecs.len(), texts.len(), t0.elapsed().as_secs_f64()); }
    let embed_s = t0.elapsed().as_secs_f64();
    let dim = vecs[0].len(); let n = vecs.len();
    let mut mean = vec![0f32; dim]; for v in &vecs { for (m, x) in mean.iter_mut().zip(v) { *m += x / n as f32; } }
    let mut trits = vec![0i8; n * dim];
    for (r, v) in vecs.iter().enumerate() {
        let c: Vec<f32> = v.iter().zip(&mean).map(|(a, b)| a - b).collect();
        let s = c.iter().map(|x| x.abs()).sum::<f32>() / dim as f32;
        for (i, x) in c.iter().enumerate() { trits[r * dim + i] = (x / s.max(1e-9)).round().clamp(-1.0, 1.0) as i8; }
    }
    let rows = p2::pack(&trits, n, dim);
    let mut b = b"OFX1".to_vec(); b.extend((n as u32).to_le_bytes()); b.extend((dim as u32).to_le_bytes());
    for m in &mean { b.extend(m.to_le_bytes()); }
    b.extend(&rows);
    let base = std::path::Path::new(root);
    for f in &files { let p = f.strip_prefix(base).unwrap_or(f).to_string_lossy().to_string(); b.extend((p.len() as u16).to_le_bytes()); b.extend(p.as_bytes()); }
    std::fs::write(out, &b).unwrap();
    // float sidecar (centred, normalised) for the quality check only — never read by the app
    let mut fb = vec![]; for v in &vecs { let c: Vec<f32> = v.iter().zip(&mean).map(|(a, b)| a - b).collect(); let nn = c.iter().map(|x| x * x).sum::<f32>().sqrt(); for x in c { fb.extend((x / nn).to_le_bytes()); } }
    std::fs::write(format!("{out}.f32"), fb).unwrap();
    println!("find-index: {n} files, dim {dim}, embed {embed_s:.1} s, index {} KB (P2 rows {} KB vs f32 {} KB) -> {out}", b.len() / 1024, rows.len() / 1024, n * dim * 4 / 1024);
}

/// `omni-render runtime0 <docs-root> <index>`: both apps inside the engine + Find quality vs float cosine.
pub fn main(root: &str, index: &str) {
    let eng = engine();
    let lk = linker(&eng);
    let mut res = json!({});
    // ---- C app
    let t0 = Instant::now();
    let m = Module::from_file(&eng, "apps/out/rings.wasm").expect("apps/out/rings.wasm");
    let compile_ms = t0.elapsed().as_secs_f64() * 1e3;
    let mut st = new_store(&eng, std::path::Path::new(root));
    let t1 = Instant::now();
    let inst = lk.instantiate(&mut st, &m).unwrap();
    let inst_us = t1.elapsed().as_secs_f64() * 1e6;
    let run = inst.get_typed_func::<(), i32>(&mut st, "run").unwrap();
    let t2 = Instant::now();
    let pass = run.call(&mut st, ()).unwrap();
    let run_ms = t2.elapsed().as_secs_f64() * 1e3;
    let fuel = 20_000_000_000 - st.get_fuel().unwrap();
    println!("C app rings.wasm: checks {pass}/7 | compile {compile_ms:.1} ms, instantiate {inst_us:.0} µs, run {run_ms:.1} ms ({fuel} fuel, sift kernel {:.0} µs)", st.data().kernel_us);
    res["c_app"] = json!({"wasm_bytes": std::fs::metadata("apps/out/rings.wasm").unwrap().len(), "checks_mask": pass, "all_pass": pass == 7,
        "compile_ms": compile_ms, "instantiate_us": inst_us, "run_ms": run_ms, "fuel": fuel, "intrinsic_us": st.data().kernel_us, "lines": st.data().out});
    // ---- Rust app: Files + Find
    let t0 = Instant::now();
    let m = Module::from_file(&eng, "apps/out/find.wasm").expect("apps/out/find.wasm");
    let compile_ms = t0.elapsed().as_secs_f64() * 1e3;
    let ixp = std::path::Path::new(index);
    let mut st = new_store(&eng, ixp.parent().unwrap());
    let inst = lk.instantiate(&mut st, &m).unwrap();
    let mem = inst.get_memory(&mut st, "memory").unwrap();
    let open = inst.get_typed_func::<(u32, u32), i32>(&mut st, "find_open").unwrap();
    let find = inst.get_typed_func::<(u32, u32, u32), i32>(&mut st, "find").unwrap();
    // pass strings through a scratch page at the top of a freshly grown page
    let put = |st: &mut Store<Host>, s: &[u8]| -> u32 { let base = mem.data_size(&*st); mem.grow(&mut *st, 1).unwrap(); mem.data_mut(&mut *st)[base..base + s.len()].copy_from_slice(s); base as u32 };
    let name = ixp.file_name().unwrap().to_string_lossy().to_string();
    let p = put(&mut st, name.as_bytes());
    let t1 = Instant::now();
    let n_docs = open.call(&mut st, (p, name.len() as u32)).unwrap();
    let open_ms = t1.elapsed().as_secs_f64() * 1e3;
    println!("Rust app find.wasm: index opened in the app's memory: {n_docs} docs ({open_ms:.1} ms)");
    let queries = ["how do I run the omnid server", "devlog of the ring heads", "ternary trit packing kernel", "dispatcher task staging",
        "vault search indexer", "how does the launcher UI work", "gpu lane manual", "omniglyph laws ledger"];
    let mut qrows = vec![];
    for q in queries {
        println!("  query: {q}");
        let qp = put(&mut st, q.as_bytes());
        st.data_mut().embed_ms = 0.0; st.data_mut().kernel_us = 0.0;
        let t = Instant::now();
        let got = find.call(&mut st, (qp, q.len() as u32, 5)).unwrap();
        let ms = t.elapsed().as_secs_f64() * 1e3;
        qrows.push(json!({"q": q, "hits": got, "total_ms": ms, "embed_ms": st.data().embed_ms, "topk_kernel_us": st.data().kernel_us}));
    }
    let lines = st.data().out.clone();
    // quality: ternary top-5 vs float cosine top-5 (same centred vectors) + known-item (doc's own head as query)
    let f32b = std::fs::read(format!("{index}.f32")).unwrap();
    let ix = std::fs::read(index).unwrap();
    let n = u32::from_le_bytes(ix[4..8].try_into().unwrap()) as usize; let dim = u32::from_le_bytes(ix[8..12].try_into().unwrap()) as usize;
    let mean: Vec<f32> = ix[12..12 + 4 * dim].chunks(4).map(|b| f32::from_le_bytes(b.try_into().unwrap())).collect();
    let rows = &ix[12 + 4 * dim..12 + 4 * dim + p2::row_bytes(dim) * n];
    let fl: Vec<f32> = f32b.chunks(4).map(|b| f32::from_le_bytes(b.try_into().unwrap())).collect();
    let paths: Vec<String> = { let mut o = 12 + 4 * dim + p2::row_bytes(dim) * n; (0..n).map(|_| { let l = u16::from_le_bytes([ix[o], ix[o + 1]]) as usize; let s = String::from_utf8_lossy(&ix[o + 2..o + 2 + l]).to_string(); o += 2 + l; s }).collect() };
    let rootp = std::path::Path::new(root);
    let mut probes: Vec<(usize, String)> = vec![];
    for k in (0..n).step_by((n / 60).max(1)) {
        let s = std::fs::read_to_string(rootp.join(&paths[k])).unwrap_or_default();
        if let Some(h) = s.lines().find(|l| l.starts_with('#')) { probes.push((k, h.trim_start_matches('#').trim().chars().take(120).collect())); }
    }
    let qv = embed(&probes.iter().map(|p| p.1.clone()).collect::<Vec<_>>(), "search_query: ").unwrap();
    let (mut ov, mut kt, mut kf) = (0usize, 0usize, 0usize);
    for ((doc, _), v) in probes.iter().zip(&qv) {
        let c: Vec<f32> = v.iter().zip(&mean).map(|(a, b)| a - b).collect();
        let fs: Vec<i32> = (0..n).map(|r| (1e6 * (0..dim).map(|i| fl[r * dim + i] * c[i]).sum::<f32>()) as i32).collect();
        let x = quant_i8(&c); let mut ts = vec![0i32; n]; unsafe { p2::gemv_b1(rows, n, dim, &x, &mut ts) };
        let (tf, tt) = (topk(&fs, 5), topk(&ts, 5));
        ov += tt.iter().filter(|a| tf.iter().any(|b| b.0 == a.0)).count();
        kt += (tt[0].0 as usize == *doc) as usize; kf += (tf[0].0 as usize == *doc) as usize;
    }
    let np = probes.len();
    println!("Find quality ({np} known-item probes = each doc's first heading): top-1 ternary {kt}/{np} vs float {kf}/{np} | top-5 overlap ternary∩float {:.3}", ov as f64 / (5 * np) as f64);
    res["rust_app"] = json!({"wasm_bytes": std::fs::metadata("apps/out/find.wasm").unwrap().len(), "compile_ms": compile_ms, "docs": n_docs, "open_ms": open_ms,
        "queries": qrows, "lines": lines, "index_bytes": ix.len(), "p2_row_bytes": p2::row_bytes(dim), "dim": dim,
        "quality": {"probes": np, "known_item_top1_ternary": kt, "known_item_top1_float": kf, "top5_overlap": ov as f64 / (5 * np) as f64}});
    res["verdict"] = json!({"c_app_runs": pass == 7, "rust_app_runs": n_docs > 0 && qrows.iter().all(|r| r["hits"].as_i64().unwrap_or(0) > 0)});
    std::fs::write("results/runtime0.json", serde_json::to_string_pretty(&res).unwrap()).unwrap();
    println!("-> results/runtime0.json {}", res["verdict"]);
}
