// SPDX-License-Identifier: MIT OR Apache-2.0
//! pageproc — the page process (BROWSER-BODY-0 step 2): one locked-down renderer per shown page, kept alive while the page
//! is on screen. The broker (the browser window's process) owns the network and the window; the renderer owns the untrusted
//! page: parse, style, layout, raster tiles, hit test, find, reader text. Same shape as sandbox.rs's one-shot `safe`
//! (and the same frames), but the renderer stays up and answers requests, so scrolling, resize and clicks never re-parse.
//!
//! Frames (sandbox::write_frame: tag u8, len u32 LE, bytes):
//!   broker → renderer, start:  'U' url · 'D' document bytes · 'V' "w h" (viewport px)
//!   renderer → broker, start:  'S' lockdown report + probes · 'L' meta JSON (title, page_h, links, timings)
//!   requests (broker → renderer) and replies:
//!     'Y' "y"      → 'F' frame at scroll y: [y u32][page_h u32][w u32][h u32] then w*h pixels (u32 LE, 0xRRGGBBAA)
//!     'V' "w h"    → 'L' meta after re-layout at the new width (scroll kept)
//!     'H' "x y"    → 'A' JSON: what is under page point (x, y): {link, text} | {input, name, form} | {}
//!     'Q' text     → 'A' JSON: {hits, boxes:[[x,y,w,h]…]} page boxes of the elements whose text contains `text`
//!     'J'          → 'A' JSON {links:[[href, text, y]…]} every <a href> in document order (follow by index; AT-SPI)
//!     'T' frag     → 'A' JSON {y} top of the element with id = frag (or <a name = frag>), -1 if none
//!     'E'          → 'A' JSON {fields:[{input, tag, type, name, value, placeholder, form, box}…]} text-entry controls in order
//!     'M'          → 'R' the page's main content as markdown (agent::reader — the reader view)
//!     'K' JSON     → 'A' JSON {method, url[, body, ctype]} for a form submit: {form, values:{name: text}} (GET / POST)
//!     'k' "ms"     → 'A' JSON {pending, navigate?, open?, relayout?} — real time passed: the page's timers due in it run (HTML
//!                    §8.1.7 event loop, paced by the broker), observers get their records; a changed tree re-lays out ('L' follows)
//!     'I' JSON     → 'A' JSON {prevented, navigate?, relayout?} — the person's values go into the form's controls (input /
//!                    change events), then a cancelable submit event: {form, values, submit} (page scripts see what was typed)
//!     'Z' "pct"    → 'L' meta after re-layout with every font size (and absolute line height) scaled — text zoom
//!     'W'          → 'A' JSON {nodes:[[role, name, parent, x, y, w, h, extra]…]} the page's accessibility tree (HTML-AAM roles,
//!                    accname, states) in document order; parent = index of the nearest kept ancestor (-1 = the document)
//!     'X'          → exit
//!   at any time while working, renderer → broker 'G' url, answered 'B' bytes | 'N' (refused) — subresources;
//!   'P' JSON {method, url, ctype, body} (script fetch / XHR) → 'B' [status u16][url len u32][url][headers JSON len u32][headers][body] | 'N';
//!   'O' "g" | "s<line>" (document.cookie read / write for the page URL) → 'B' cookie string;
//!   'w' JSON {op: open|send|close|poll, id, url, data, code, reason} (WebSocket, RFC 6455 — the broker owns the socket) → 'B' JSON;
//!   'g' JSON [url…] preload: the broker fetches them in parallel (≤ 6 at once) into its cache → 'B' (later 'G's hit the cache).
use crate::dom::{self, Dom, NONE};
use crate::sandbox::{read_frame, write_frame, PipeSource};
use crate::{agent, font, hit, img, layout, page, raster, sandbox, scroll, style, svg};
use serde_json::{json, Value};
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::time::Instant;

/// Subresource bytes are asked for once per page (intrinsic size at layout, decode at raster, re-layout on resize).
struct Cached<'a> { inner: &'a PipeSource, map: RefCell<HashMap<String, Option<Rc<Vec<u8>>>>> }
impl page::Source for Cached<'_> {
    fn url(&self) -> String { self.inner.url.clone() }
    fn html(&self) -> Vec<u8> { self.inner.doc.clone() }
    fn get(&self, url: &str) -> Option<Vec<u8>> {
        if let Some(v) = self.map.borrow().get(url) { return v.as_ref().map(|b| b.to_vec()); }
        let v = self.inner.get(url).map(Rc::new);
        self.map.borrow_mut().insert(url.to_string(), v.clone());
        v.map(|b| b.to_vec())
    }
}

fn reply(tag: u8, data: &[u8]) { let o = std::io::stdout(); let _ = write_frame(&mut o.lock(), tag, data); }
fn request() -> Option<(u8, Vec<u8>)> { let i = std::io::stdin(); read_frame(&mut i.lock()) }
fn two(b: &[u8]) -> (i64, i64) {
    let s = String::from_utf8_lossy(b);
    let mut it = s.split_whitespace().map(|x| x.parse::<i64>().unwrap_or(0));
    (it.next().unwrap_or(0), it.next().unwrap_or(0))
}

/// Nearest ancestor-or-self element with tag `t`.
fn up(d: &Dom, mut n: u32, t: &[&str]) -> Option<u32> {
    while n != NONE && (n as usize) < d.nodes.len() {
        if d.nodes[n as usize].kind == dom::Kind::Element && t.contains(&d.tag(n)) { return Some(n); }
        n = d.nodes[n as usize].parent;
    }
    None
}

/// All text under `n`, whitespace collapsed.
pub fn text_of(d: &Dom, n: u32) -> String {
    let end = d.nodes[n as usize].end.min(d.nodes.len() as u32);
    let mut s = String::new();
    for i in n..end.max(n + 1) {
        if (i as usize) < d.nodes.len() && d.nodes[i as usize].kind == dom::Kind::Text { s.push_str(d.text(i)); s.push(' '); }
    }
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Form submission (HTML §4.10.21.3): action resolved against the base; successful controls (named, not disabled,
/// text-like / hidden / checked / selected) in tree order; GET = "mutate action URL", POST = "submit as entity body"
/// (application/x-www-form-urlencoded, multipart/form-data without files, text/plain). `values` = what the person typed,
/// keyed by control name, overriding the page's values (checkboxes: "on"/"off"; selects: the option value).
pub fn form_submit(d: &Dom, form: u32, base: &str, values: &HashMap<String, String>) -> Result<Value, String> {
    let method = d.attr(form, "method").unwrap_or("get").to_ascii_lowercase();
    let method = if method == "post" { "post" } else if method == "dialog" { return Err("dialog forms close a dialog (no request)".into()) } else { "get" };
    let action = page::resolve(base, d.attr(form, "action").filter(|a| !a.is_empty()).unwrap_or(base));
    let end = d.nodes[form as usize].end.min(d.nodes.len() as u32);
    let mut pairs: Vec<(String, String)> = vec![];
    for i in form + 1..end {
        if d.nodes[i as usize].kind != dom::Kind::Element { continue; }
        let tag = d.tag(i);
        let Some(name) = d.attr(i, "name").filter(|n| !n.is_empty()) else { continue };
        if d.attr(i, "disabled").is_some() { continue; }
        let ty = d.attr(i, "type").unwrap_or("text").to_ascii_lowercase();
        let v = match (tag, ty.as_str()) {
            ("input", "submit" | "button" | "image" | "reset" | "file") => continue,
            ("input", "checkbox") => {
                let on = match values.get(name).map(|s| s.as_str()) { Some("on") | Some("true") => true, Some("off") | Some("false") => false, _ => d.attr(i, "checked").is_some() };
                if !on { continue; } d.attr(i, "value").unwrap_or("on").to_string()
            }
            ("input", "radio") => {
                let val = d.attr(i, "value").unwrap_or("on");
                let on = match values.get(name) { Some(v) => v == val, None => d.attr(i, "checked").is_some() };
                if !on { continue; } val.to_string()
            }
            ("input", _) | ("textarea", _) => values.get(name).cloned().unwrap_or_else(|| d.attr(i, "value").unwrap_or("").to_string()),
            ("select", _) => {
                let e2 = d.nodes[i as usize].end.min(d.nodes.len() as u32);
                let opts: Vec<u32> = (i + 1..e2).filter(|&o| d.nodes[o as usize].kind == dom::Kind::Element && d.tag(o) == "option").collect();
                let ov = |o: u32| d.attr(o, "value").map(|s| s.to_string()).unwrap_or_else(|| text_of(d, o));
                let pick = match values.get(name) { Some(v) => opts.iter().copied().find(|&o| &ov(o) == v), None => None }
                    .or_else(|| opts.iter().copied().find(|&o| d.attr(o, "selected").is_some())).or(opts.first().copied());
                match pick { Some(o) => d.attr(o, "value").map(|s| s.to_string()).unwrap_or_else(|| text_of(d, o)), None => continue }
            }
            _ => continue,
        };
        pairs.push((name.to_string(), v));
    }
    let enc = |s: &str| -> String {
        let mut o = String::new();
        for b in s.bytes() {
            match b { b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'*' | b'-' | b'.' | b'_' => o.push(b as char), b' ' => o.push('+'), _ => o.push_str(&format!("%{b:02X}")) }
        }
        o
    };
    let q = pairs.iter().map(|(k, v)| format!("{}={}", enc(k), enc(v))).collect::<Vec<_>>().join("&");
    if method == "get" {
        let a = action.split('#').next().unwrap_or("").split('?').next().unwrap_or("").to_string();
        return Ok(json!({"method": "GET", "url": format!("{a}?{q}")}));
    }
    let action = action.split('#').next().unwrap_or("").to_string();
    let enctype = d.attr(form, "enctype").unwrap_or("").to_ascii_lowercase();
    if enctype == "multipart/form-data" {
        let bnd = format!("----omniform{:016x}", pairs.iter().fold(0xcbf29ce484222325u64, |h, (k, v)| (h ^ (k.len() as u64 * 31 + v.len() as u64)).wrapping_mul(0x100000001b3)));
        let mut body = String::new();
        for (k, v) in &pairs { body.push_str(&format!("--{bnd}\r\nContent-Disposition: form-data; name=\"{}\"\r\n\r\n{v}\r\n", k.replace('"', "%22"))); }
        body.push_str(&format!("--{bnd}--\r\n"));
        return Ok(json!({"method": "POST", "url": action, "ctype": format!("multipart/form-data; boundary={bnd}"), "body": body}));
    }
    if enctype == "text/plain" {
        let body: String = pairs.iter().map(|(k, v)| format!("{k}={v}\r\n")).collect();
        return Ok(json!({"method": "POST", "url": action, "ctype": "text/plain", "body": body}));
    }
    Ok(json!({"method": "POST", "url": action, "ctype": "application/x-www-form-urlencoded", "body": q}))
}

/// %XX → bytes (URL fragment → id, URL spec "percent-decode").
fn pct_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut o = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() && s.is_char_boundary(i + 1) && s.is_char_boundary(i + 3) {
            if let Ok(v) = u8::from_str_radix(&s[i + 1..i + 3], 16) { o.push(v); i += 3; continue; }
        }
        o.push(b[i]); i += 1;
    }
    String::from_utf8_lossy(&o).into_owned()
}

/// The renderer side. Called as the process's whole job (`omni-render page-renderer`, `omni-ui web-renderer`).
pub fn serve() {
    let _ = crate::jscomp::FEED.set(broker_compiled);
    // the doctor's program (OMR_PROGRAM) is read BEFORE lockdown: Landlock then denies the file (the page process ran with
    // every gated patch off however OMR_PROGRAM was set)
    let _ = crate::layout::program();
    let mode = sandbox::lockdown();
    let probes = sandbox::probes();
    let Some((_, u)) = request() else { return };
    let Some((_, doc)) = request() else { return };
    // the viewport frame first: it is already in the pipe (open writes U, D, V together) — reading it after the prefetch
    // exchange let the prefetch reply consume it (width parsed as 0 → clamped to 64: every remote page drew 64×64 until resized)
    let Some((_, v)) = request() else { return };
    // the preload scan runs before the page's own sequential loads (stylesheets, scripts, images)
    let pre = preload_scan(&doc, &String::from_utf8_lossy(&u));
    broker_prefetch(&pre);
    let url = String::from_utf8_lossy(&u).into_owned();
    let (mut w, mut vh) = two(&v);
    w = w.clamp(64, 8192); vh = vh.clamp(64, 8192);
    reply(b'S', format!("{}\n{}", match &mode { Ok(m) => m.clone(), Err(e) => format!("LOCKDOWN FAILED: {e}") },
        probes.iter().map(|(k, ok)| format!("{}: {}", k, if *ok { "PASS" } else { "FAIL" })).collect::<Vec<_>>().join("\n")).as_bytes());
    let pipe = PipeSource { url: url.clone(), doc, io: RefCell::new((std::io::stdin(), std::io::stdout())) };
    let src = Cached { inner: &pipe, map: Default::default() };
    let fonts = font::Fonts::new();
    let mut zoom: i64 = 100;
    let mut y: i64 = 0;
    // JS-0: page scripts run in this (locked-down) process on jsi; OMR_JS=0 from the broker turns them off
    let js_on = std::env::var("OMR_JS").map(|v| v != "0").unwrap_or(true);
    let mut js: Option<(crate::jsi::Interp, crate::jsdom::LiveRef)> = None;
    let mut js_report = Value::Null;
    let mut html_now: Option<Vec<u8>> = None;
    let mut first = true;
    let mut run_scripts = false;
    // the first layout is answered before the page's scripts run; they run on the first 'k' tick (the window never waits)
    let mut scripts_pending = false;
    // timer-driven re-layouts are paced by what a layout costs here: at most one per max(1 s, 5 × the last layout)
    let mut last_relayout = Instant::now();
    let mut last_layout_ms = 0.0f64;
    'document: loop {
    let t0 = Instant::now();
    let over = Over { inner: &src, html: html_now.clone() };
    let page::Loaded { dom, mut styled, fonts: web_fonts, .. } = page::load(&over, (w as f32, vh as f32));
    let load_ms = t0.elapsed().as_secs_f64() * 1e3;
    for (fam, lo, hi, it, bytes) in &web_fonts { if !fonts.has_web(fam) { fonts.add_web(fam, *lo, *hi, *it, bytes.clone()); } }
    drop(web_fonts);
    let base_table = styled.table.clone();
    if zoom != 100 { zoom_table(&mut styled.table, &base_table, zoom); }
    if first && js_on {
        let live = Rc::new(RefCell::new(crate::jsdom::Live::from_dom(&dom, &url)));
        live.borrow_mut().viewport = (w as i32, vh as i32);
        live.borrow_mut().cookie_io = Some(Rc::new(|op: Option<&str>| broker_cookie(op)));
        live.borrow_mut().req_io = Some(Rc::new(|m: &str, u: &str, ct: &str, b: Option<&str>| broker_req(m, u, ct, b)));
        live.borrow_mut().ws_io = Some(Rc::new(|v: &Value| broker_ws(v)));
        let mut it = crate::jsi::Interp::new();
        // ES modules (HTML §8.1.3.1 fetch a module script graph): a specifier resolves against its referrer's URL (or the page's);
        // bare names have no import map here and fail like an unmapped import; the text arrives through the broker
        let page_url = url.clone();
        // HTML §8.1.5 import maps: <script type="importmap"> {"imports": {"react": "https://…", "lib/": "https://…/lib/"}} —
        // a bare specifier maps by exact key, else by the longest "prefix/" key (scopes not modelled)
        let mut imap: Vec<(String, String)> = vec![];
        for (e, txt) in &dom.scripts {
            if dom.attr(*e, "type").map(|t| t.trim().eq_ignore_ascii_case("importmap")).unwrap_or(false) {
                if let Ok(v) = serde_json::from_str::<Value>(txt) { if let Some(o) = v["imports"].as_object() { for (k, t) in o { if let Some(t) = t.as_str() { imap.push((k.clone(), page::resolve(&url, t))); } } } }
            }
        }
        imap.sort_by(|a, b| b.0.len().cmp(&a.0.len()));
        it.module_loader = Some(Rc::new(move |spec: &str, referrer: &str| {
            let base = if referrer.is_empty() { page_url.as_str() } else { referrer };
            let mapped = imap.iter().find(|(k, _)| k == spec).map(|(_, t)| t.clone())
                .or_else(|| imap.iter().find(|(k, _)| k.ends_with('/') && spec.starts_with(k.as_str())).map(|(k, t)| format!("{t}{}", &spec[k.len()..])));
            let spec = mapped.as_deref().unwrap_or(spec);
            if !(spec.starts_with("./") || spec.starts_with("../") || spec.starts_with('/') || spec.contains("://")) { return None; }
            let u = page::resolve(base, spec);
            if !(u.starts_with("http://") || u.starts_with("https://")) { return None; }
            let prog = match crate::jscomp::feed(&u, true) { Some(p) => p, None => { let bytes = broker_get(&u)?; crate::jsparse::parse(&String::from_utf8_lossy(&bytes), true).ok()? } };
            // the next level of the module graph in one parallel batch (their own imports follow level by level)
            broker_prefetch(&module_deps(&prog, &u));
            Some((u, prog))
        }));
        crate::jsdom::install(&mut it, live.clone(), js_fetch());
        // the scripts run after the first layout (below), so geometry reads (getBoundingClientRect, offsetWidth) see real boxes
        run_scripts = true;
        js = Some((it, live));
    }
    first = false;
    // packed node → live node: the identity on the page as loaded; after a script re-render, by data-omr
    let to_live: Vec<u32> = if html_now.is_some() {
        (0..dom.nodes.len() as u32).map(|i| dom.attr(i, "data-omr").and_then(|v| v.parse().ok()).unwrap_or(NONE)).collect()
    } else { (0..dom.nodes.len() as u32).collect() };
    let d = &dom;
    let base = d.nodes.iter().enumerate().find(|(i, n)| n.kind == dom::Kind::Element && d.tag(*i as u32) == "base")
        .and_then(|(i, _)| d.attr(i as u32, "href")).map(|h| page::resolve(&url, h)).unwrap_or(url.clone());
    let title = d.nodes.iter().enumerate().find(|(i, n)| n.kind == dom::Kind::Element && d.tag(*i as u32) == "title")
        .map(|(i, _)| text_of(d, i as u32)).unwrap_or_default();
    // HTML §4.2.5.3 "refresh" pragma: <meta http-equiv=refresh content="N; url=U"> (the broker decides whether to follow)
    let refresh = (0..d.nodes.len() as u32).find(|&i| d.nodes[i as usize].kind == dom::Kind::Element && d.tag(i) == "meta"
            && d.attr(i, "http-equiv").map(|v| v.eq_ignore_ascii_case("refresh")).unwrap_or(false))
        .and_then(|i| d.attr(i, "content")).and_then(|c| {
            let (n, rest) = c.split_once([';', ',']).unwrap_or((c, ""));
            let delay = n.trim().parse::<f64>().ok()?;
            let r = rest.trim();
            let r = if r.len() >= 4 && r[..4].eq_ignore_ascii_case("url=") { &r[4..] } else { r };
            let r = r.trim().trim_matches(['\'', '"']);
            if r.is_empty() { None } else { Some(json!({"delay": delay, "url": page::resolve(&base, r)})) }
        });
    let links = (0..d.nodes.len() as u32).filter(|&i| d.in_doc(i) && d.nodes[i as usize].kind == dom::Kind::Element && d.tag(i) == "a" && d.attr(i, "href").is_some()).count();
    let sizes = |n: u32| -> Option<(u32, u32)> {
        if d.tag(n) == "svg" { return svg::intrinsic(d.outer_xml(n).as_bytes()).map(|(w, h, _)| (w.round() as u32, h.round() as u32)); }
        // HTML §15.4.2 (width / height attributes map to the aspect ratio + size): when both are given, layout needs no
        // bytes — the image is fetched only when its tile row is first rastered (scrolled into view)
        let num = |k: &str| d.attr(n, k).and_then(|v| v.trim().trim_end_matches("px").parse::<f32>().ok()).filter(|v| *v > 0.0);
        if let (Some(iw), Some(ih)) = (num("width"), num("height")) { return Some((iw.round() as u32, ih.round() as u32)); }
        usable_img(d, n, &base, &src).and_then(|b| img::size(&b))
    };
    let pcache: RefCell<HashMap<(u32, i32, i32), Option<Rc<(u32, u32, Vec<u32>)>>>> = Default::default();
    // inline <svg> elements with their CSS fill / stroke / colour, taken before zoom may rewrite the style table
    let svg_css: HashMap<u32, svg::Css> = (0..d.nodes.len() as u32).filter(|&i| d.nodes[i as usize].kind == dom::Kind::Element && d.tag(i) == "svg")
        .map(|i| { let st = &styled.table[styled.ids[i as usize] as usize];
            (i, svg::Css { fill: st.svg_fill, stroke: st.svg_stroke, sw: st.svg_sw.map(|x| x as f32 / style::FX as f32), color: st.color }) }).collect();
    let imgf = |n: u32, mode: u8, iw: i32, ih: i32| -> Option<Rc<(u32, u32, Vec<u32>)>> {
        if mode != 0 { return None; }
        pcache.borrow_mut().entry((n, iw, ih)).or_insert_with(|| {
            // inline <svg>: drawn from its own markup with the element's CSS fill / stroke / colour (P2 A5)
            if let Some(css) = svg_css.get(&n) { return svg::render_css(d.outer_xml(n).as_bytes(), iw.max(1) as u32, ih.max(1) as u32, *css).map(Rc::new); }
            usable_img(d, n, &base, &src).and_then(|b| img::decode(&b, iw.max(1) as u32, ih.max(1) as u32)).map(Rc::new)
        }).clone()
    };
    // getComputedStyle: the engine's cascade for the live nodes of this document (CSSOM §9 resolved values)
    if let Some((_, live)) = &js {
        let back: HashMap<u32, u32> = to_live.iter().enumerate().filter(|&(_, &l)| l != NONE).map(|(p, &l)| (l, p as u32)).collect();
        let tab = Rc::new((styled.ids.clone(), styled.table.clone(), back));
        live.borrow_mut().style_io = Some(Rc::new(move |n: u32, prop: &str| {
            let p = *tab.2.get(&n)?;
            let cw = *tab.0.get(p as usize)? as usize;
            computed(tab.1.get(cw)?, prop)
        }));
    }
    'relayout: loop {
        let t1 = Instant::now();
        let mut lay = layout::Layout::new(d, &styled, &fonts, &sizes, w as i32, vh as i32);
        lay.run();
        let out = std::mem::take(&mut lay.out);
        drop(lay);
        let layout_ms = t1.elapsed().as_secs_f64() * 1e3;
        last_layout_ms = layout_ms; last_relayout = Instant::now();
        let fx = style::FX;
        let page_h = out.boxes.iter().map(|(_, b)| (b[1] + b[3]) / fx).max().unwrap_or(0).max(vh as i32) as usize;
        // inline fragments (links inside paragraphs) join the block boxes: the deepest node under the pointer wins
        let hit_boxes: Vec<(u32, [i32; 4])> = out.boxes.iter().copied().chain(out.inline_abs()).collect();
        let grid = hit::HitGrid::build(&hit_boxes, w as i32 * fx, (page_h as i32 + 1) * fx);
        let mut first_box: HashMap<u32, [i32; 4]> = HashMap::new();
        for (nd, b) in &out.boxes { first_box.entry(*nd).or_insert([b[0] / fx, b[1] / fx, b[2] / fx, b[3] / fx]); }
        if let Some((_, live)) = &js {
            let mut lb = live.borrow_mut();
            lb.boxes = first_box.iter().filter_map(|(k, b)| to_live.get(*k as usize).filter(|x| **x != NONE).map(|x| (*x, *b))).collect();
            lb.viewport = (w as i32, vh as i32);
        }
        if std::mem::take(&mut run_scripts) { scripts_pending = true; }
        let mut store = raster::Store::default();
        let t2 = Instant::now();
        let mut sc = scroll::Scroller::new(&out.items, &fonts, &imgf, w as usize, vh as usize, page_h, &mut store);
        y = y.clamp(0, (page_h as i64 - vh).max(0));
        sc.scroll(y, &mut store);
        let raster_ms = t2.elapsed().as_secs_f64() * 1e3;
        reply(b'L', json!({"url": url, "base": base, "title": title, "w": w, "h": vh, "page_h": page_h, "links": links,
            "dom_nodes": d.nodes.len(), "items": out.items.len(), "zoom": zoom, "refresh": refresh, "js": js_report,
            "navigate": js.as_ref().and_then(|(_, l)| l.borrow_mut().navigate.take()), "load_ms": load_ms, "layout_ms": layout_ms, "raster_ms": raster_ms,
            "scripts_pending": scripts_pending}).to_string().as_bytes());
        while let Some((tag, data)) = request() {
            match tag {
                b'Y' => {
                    let (ny, _) = two(&data);
                    sc.scroll(ny - sc.y as i64, &mut store);
                    y = sc.y as i64;
                    let mut b = Vec::with_capacity(16 + sc.fb.len() * 4);
                    for x in [sc.y as u32, page_h as u32, w as u32, vh as u32] { b.extend_from_slice(&x.to_le_bytes()); }
                    for p in &sc.fb { b.extend_from_slice(&p.to_le_bytes()); }
                    reply(b'F', &b);
                }
                b'V' => {
                    let (nw, nh) = two(&data);
                    let (nw, nh) = (nw.clamp(64, 8192), nh.clamp(64, 8192));
                    if nw != w || nh != vh { w = nw; vh = nh; continue 'relayout; }
                    reply(b'L', json!({"url": url, "title": title, "w": w, "h": vh, "page_h": page_h, "links": links}).to_string().as_bytes());
                }
                b'H' => { let (x, py) = two(&data); reply(b'A', hit_json(d, &first_box, &base, grid.hit(x as i32 * fx, py as i32 * fx)).to_string().as_bytes()); }
                b'C' => {
                    // a pointer click: DOM click event first (bubbling, listeners + on* handlers), then the broker does the
                    // default action (follow the link / edit the field) unless a handler called preventDefault()
                    let (x, py) = two(&data);
                    let n = grid.hit(x as i32 * fx, py as i32 * fx);
                    let mut r = hit_json(d, &first_box, &base, n);
                    if let (Some(n), Some((it, live))) = (n, js.as_mut()) {
                        let ln = to_live.get(n as usize).copied().unwrap_or(NONE);
                        if ln != NONE {
                            it.limit = it.steps + 20_000_000;
                            live.borrow_mut().opened.clear(); // only a click may open a tab (popups during load are dropped)
                            let ok = crate::jsdom::dispatch(it, ln, crate::jsdom::event_obj("click", true)).unwrap_or(true);
                            crate::jsdom::settle(it, live, &|u| page::Source::get(&src, u), 300.0);
                            r["prevented"] = json!(!ok);
                            if let Some(u) = live.borrow_mut().navigate.take() { r["navigate"] = json!(u); }
                            let opened = std::mem::take(&mut live.borrow_mut().opened);
                            if !opened.is_empty() { r["open"] = json!(opened); }
                            let log = std::mem::take(&mut live.borrow_mut().log);
                            if !log.is_empty() { r["js_errors"] = json!(log); }
                            if std::mem::replace(&mut live.borrow_mut().dirty, false) {
                                r["relayout"] = json!(true);
                                reply(b'A', r.to_string().as_bytes());
                                html_now = Some(live.borrow().serialize(d.quirks).into_bytes());
                                continue 'document;
                            }
                        }
                    }
                    reply(b'A', r.to_string().as_bytes());
                }
                b'k' if std::mem::take(&mut scripts_pending) => {
                    // the page's scripts (first tick after load): a changed tree re-lays out ('L' follows the reply)
                    let mut r = json!({"pending": 0});
                    if let Some((it, live)) = js.as_mut() {
                        // OMR_JS_ROUNDTRIP=1 (a test): no scripts, only the live-tree → HTML → load round trip, to tell script effects from it
                        if std::env::var_os("OMR_JS_ROUNDTRIP").is_some() { live.borrow_mut().dirty = true; }
                        else { js_report = crate::jsdom::run_page(it, live, &|u| page::Source::get(&src, u), 1000.0, 50_000_000); }
                        r["js"] = js_report.clone();
                        r["pending"] = json!(it.pending_timers());
                        if let Some(u) = live.borrow_mut().navigate.take() { r["navigate"] = json!(u); }
                        if std::mem::replace(&mut live.borrow_mut().dirty, false) {
                            r["relayout"] = json!(true);
                            reply(b'A', r.to_string().as_bytes());
                            html_now = Some(live.borrow().serialize(d.quirks).into_bytes());
                            continue 'document;
                        }
                    }
                    reply(b'A', r.to_string().as_bytes());
                }
                b'k' => {
                    let (ms, _) = two(&data);
                    let mut r = json!({"pending": 0});
                    if let Some((it, live)) = js.as_mut() {
                        crate::jsdom::settle(it, live, &|u| page::Source::get(&src, u), ms.clamp(0, 10_000) as f64);
                        r["pending"] = json!(it.timers.iter().filter(|x| x.0.is_callable()).count());
                        if let Some(u) = live.borrow_mut().navigate.take() { r["navigate"] = json!(u); }
                        let log = std::mem::take(&mut live.borrow_mut().log);
                        if !log.is_empty() { r["js_errors"] = json!(log); }
                        let due = last_relayout.elapsed().as_secs_f64() * 1e3 >= (5.0 * last_layout_ms).max(1000.0);
                        if live.borrow().dirty && !due { r["deferred"] = json!(true); r["pending"] = json!(r["pending"].as_u64().unwrap_or(0).max(1)); }
                        if due && std::mem::replace(&mut live.borrow_mut().dirty, false) {
                            r["relayout"] = json!(true);
                            reply(b'A', r.to_string().as_bytes());
                            html_now = Some(live.borrow().serialize(d.quirks).into_bytes());
                            continue 'document;
                        }
                    }
                    reply(b'A', r.to_string().as_bytes());
                }
                b'I' => {
                    let v: Value = serde_json::from_slice(&data).unwrap_or(json!({}));
                    let mut r = json!({"prevented": false});
                    let form = v["form"].as_u64().map(|x| x as u32).and_then(|f| to_live.get(f as usize).copied()).filter(|&f| f != NONE);
                    if let (Some(f), Some((it, live))) = (form, js.as_mut()) {
                        it.limit = it.steps + 20_000_000;
                        let vals: Vec<(String, String)> = v["values"].as_object().map(|o| o.iter().map(|(k, x)| (k.clone(), x.as_str().unwrap_or("").to_string())).collect()).unwrap_or_default();
                        let changed = crate::jsdom::fill_form(&mut live.borrow_mut(), f, &vals);
                        for c in changed { let _ = crate::jsdom::dispatch(it, c, crate::jsdom::event_obj("input", true)); let _ = crate::jsdom::dispatch(it, c, crate::jsdom::event_obj("change", true)); }
                        if v["submit"] == true {
                            let ok = crate::jsdom::dispatch(it, f, crate::jsdom::event_obj("submit", true)).unwrap_or(true);
                            r["prevented"] = json!(!ok);
                        }
                        crate::jsdom::settle(it, live, &|u| page::Source::get(&src, u), 300.0);
                        if let Some(u) = live.borrow_mut().navigate.take() { r["navigate"] = json!(u); }
                        let log = std::mem::take(&mut live.borrow_mut().log);
                        if !log.is_empty() { r["js_errors"] = json!(log); }
                        if std::mem::replace(&mut live.borrow_mut().dirty, false) && r["prevented"] == true {
                            r["relayout"] = json!(true);
                            reply(b'A', r.to_string().as_bytes());
                            html_now = Some(live.borrow().serialize(d.quirks).into_bytes());
                            continue 'document;
                        }
                    }
                    reply(b'A', r.to_string().as_bytes());
                }
                b'Q' => {
                    let q = String::from_utf8_lossy(&data).to_lowercase();
                    let mut boxes: Vec<[i32; 4]> = vec![];
                    let mut hits = 0;
                    if !q.trim().is_empty() {
                        for i in 0..d.nodes.len() as u32 {
                            if d.nodes[i as usize].kind != dom::Kind::Text || !d.in_doc(i) { continue; }
                            let k = d.text(i).to_lowercase().matches(q.as_str()).count();
                            if k == 0 { continue; }
                            // the nearest ancestor that has a box (text nodes have none of their own)
                            if up(d, i, &["head", "script", "style", "noscript", "template"]).is_some() { continue; }
                            let mut p = d.nodes[i as usize].parent;
                            while p != NONE && !first_box.contains_key(&p) { p = d.nodes[p as usize].parent; }
                            // a run inside body / html only (no tighter box) is not drawn where we can point at it
                            if p == NONE || matches!(d.tag(p), "body" | "html") { continue; }
                            if let Some(b) = first_box.get(&p) { if b[2] > 0 && b[3] > 0 { hits += k; if boxes.last() != Some(b) { boxes.push(*b); } } }
                        }
                    }
                    boxes.sort_by_key(|b| (b[1], b[0]));
                    boxes.dedup();
                    reply(b'A', json!({"hits": hits, "boxes": boxes}).to_string().as_bytes());
                }
                b'J' => {
                    let ls: Vec<Value> = (0..d.nodes.len() as u32).filter(|&i| d.in_doc(i) && d.nodes[i as usize].kind == dom::Kind::Element && d.tag(i) == "a")
                        .filter_map(|i| d.attr(i, "href").map(|h| (i, h)))
                        .filter(|(_, h)| !h.starts_with("javascript:"))
                        .map(|(i, h)| json!([page::resolve_frag(&base, h), text_of(d, i), first_box.get(&i).map(|b| b[1]).unwrap_or(-1)])).collect();
                    reply(b'A', json!({"links": ls}).to_string().as_bytes());
                }
                b'T' => {
                    let f = String::from_utf8_lossy(&data).into_owned();
                    let f = pct_decode(&f);
                    let n = (0..d.nodes.len() as u32).find(|&i| d.nodes[i as usize].kind == dom::Kind::Element && (d.attr(i, "id") == Some(f.as_str()) || (d.tag(i) == "a" && d.attr(i, "name") == Some(f.as_str()))));
                    // the element's own box, or the first box after it in tree order (an empty <a name> has none)
                    let y = n.and_then(|n| (n..d.nodes.len() as u32).find_map(|i| first_box.get(&i).map(|b| b[1]))).unwrap_or(-1);
                    reply(b'A', json!({"y": y}).to_string().as_bytes());
                }
                b'E' => {
                    let fs: Vec<Value> = (0..d.nodes.len() as u32).filter(|&i| d.in_doc(i) && d.nodes[i as usize].kind == dom::Kind::Element && matches!(d.tag(i), "input" | "textarea"))
                        .filter(|&i| d.tag(i) == "textarea" || matches!(d.attr(i, "type").unwrap_or("text").to_ascii_lowercase().as_str(), "text" | "search" | "email" | "url" | "tel" | "number" | "password"))
                        .filter(|i| first_box.get(i).map(|b| b[2] > 0 && b[3] > 0).unwrap_or(false))
                        .map(|i| json!({"input": i, "tag": d.tag(i), "type": d.attr(i, "type").unwrap_or("text"), "name": d.attr(i, "name").unwrap_or(""),
                                         "value": d.attr(i, "value").unwrap_or(""), "placeholder": d.attr(i, "placeholder").unwrap_or(""),
                                         "form": up(d, i, &["form"]), "box": first_box.get(&i)})).collect();
                    reply(b'A', json!({"fields": fs}).to_string().as_bytes());
                }
                b'M' => reply(b'R', agent::reader(d, &styled, &base).as_bytes()),
                b'W' => reply(b'A', json!({"nodes": a11y_tree(d, &styled, &first_box, &base)}).to_string().as_bytes()),
                b'Z' => {
                    let (z, _) = two(&data);
                    let z = z.clamp(50, 300);
                    if z != zoom {
                        zoom = z;
                        zoom_table(&mut styled.table, &base_table, z);
                        continue 'relayout;
                    }
                    reply(b'L', json!({"url": url, "title": title, "w": w, "h": vh, "page_h": page_h, "links": links, "zoom": zoom}).to_string().as_bytes());
                }
                b'K' => {
                    let v: Value = serde_json::from_slice(&data).unwrap_or(json!({}));
                    let form = v["form"].as_u64().map(|x| x as u32).filter(|&f| (f as usize) < d.nodes.len());
                    let vals: HashMap<String, String> = v["values"].as_object().map(|o| o.iter().map(|(k, x)| (k.clone(), x.as_str().unwrap_or("").to_string())).collect()).unwrap_or_default();
                    let r = match form { Some(f) => match form_submit(d, f, &base, &vals) { Ok(v) => v, Err(e) => json!({"error": e}) }, None => json!({"error": "no form"}) };
                    reply(b'A', r.to_string().as_bytes());
                }
                _ => return,
            }
        }
        return;
    }
    }
}

/// The first image candidate this engine can decode (AVIF / JPEG XL sources in a <picture> fall through to the JPEG / PNG /
/// WebP ones; the corpus path keeps page::img_bytes — Chrome's own choice).
fn usable_img(d: &Dom, n: u32, base: &str, src: &dyn page::Source) -> Option<Vec<u8>> {
    page::img_srcs(d, n).iter().find_map(|u| src.get(&page::resolve(base, u)).filter(|b| img::size(b).is_some()))
}

/// The page as assistive technology reads it (HTML-AAM §4 element mappings, WAI-ARIA 1.2 roles, accname 1.2 name
/// computation — the parts screen readers use: landmarks, headings, links, lists, tables, images, form controls, text).
/// Generic containers (div, span…) are transparent: their content joins the nearest kept ancestor. Hidden content
/// (display:none, visibility:hidden, aria-hidden, hidden) is left out. Each row: [role, name, parent, x, y, w, h, extra].
pub fn a11y_tree(d: &Dom, styled: &style::Styled, first_box: &HashMap<u32, [i32; 4]>, base: &str) -> Vec<Value> {
    let n = d.nodes.len() as u32;
    let el = |i: u32| d.nodes[i as usize].kind == dom::Kind::Element;
    let sty = |i: u32| styled.ids.get(i as usize).and_then(|&c| styled.table.get(c as usize));
    let ids: HashMap<&str, u32> = (0..n).filter(|&i| el(i) && d.in_doc(i)).filter_map(|i| d.attr(i, "id").map(|v| (v, i))).collect();
    let text = |i: u32| text_of(d, i);
    // accname 1.2 §4.3 (simplified): aria-labelledby, aria-label, host-language label (alt, <label>, title, placeholder), content
    let label_for: HashMap<&str, u32> = (0..n).filter(|&i| el(i) && d.tag(i) == "label").filter_map(|i| d.attr(i, "for").map(|f| (f, i))).collect();
    let name_of = |i: u32, from_content: bool| -> String {
        if let Some(ls) = d.attr(i, "aria-labelledby") { let t: Vec<String> = ls.split_whitespace().filter_map(|x| ids.get(x)).map(|&k| text(k)).collect(); if !t.is_empty() { return t.join(" "); } }
        if let Some(v) = d.attr(i, "aria-label").map(str::trim).filter(|v| !v.is_empty()) { return v.to_string(); }
        let tag = d.tag(i);
        if matches!(tag, "img" | "area") || (tag == "input" && d.attr(i, "type") == Some("image")) { if let Some(a) = d.attr(i, "alt") { return a.trim().to_string(); } }
        if matches!(tag, "input" | "select" | "textarea" | "meter" | "progress") {
            if let Some(l) = d.attr(i, "id").and_then(|id| label_for.get(id)) { return text(*l); }
            if let Some(l) = up(d, i, &["label"]) { return text(l); }
            if let Some(v) = d.attr(i, "title").or(d.attr(i, "placeholder")) { return v.to_string(); }
            if tag == "input" && matches!(d.attr(i, "type").unwrap_or(""), "submit" | "button" | "reset") { return d.attr(i, "value").unwrap_or(if d.attr(i, "type") == Some("reset") { "Reset" } else { "Submit" }).to_string(); }
            return String::new();
        }
        if from_content { let t = text(i); if !t.is_empty() { return t; } let alt: Vec<&str> = (i + 1..d.nodes[i as usize].end).filter(|&k| el(k) && d.tag(k) == "img").filter_map(|k| d.attr(k, "alt")).collect(); if !alt.is_empty() { return alt.join(" "); } }
        d.attr(i, "title").unwrap_or("").to_string()
    };
    let named_landmark = |i: u32| d.attr(i, "aria-label").is_some() || d.attr(i, "aria-labelledby").is_some();
    // HTML-AAM implicit roles; header / footer are banner / contentinfo only outside article / aside / main / nav / section
    let role_of = |i: u32| -> Option<&'static str> {
        if let Some(r) = d.attr(i, "role").and_then(|r| r.split_whitespace().next()) {
            return Some(match r { "presentation" | "none" => return None, "link" => "link", "button" => "button", "heading" => "heading", "navigation" => "navigation",
                "main" => "main", "banner" => "banner", "contentinfo" => "contentinfo", "complementary" => "complementary", "search" => "search", "form" => "form",
                "region" => "region", "article" => "article", "list" => "list", "listitem" => "listitem", "img" | "image" => "image", "checkbox" | "switch" => "checkbox",
                "radio" => "radio", "textbox" => "textbox", "searchbox" => "searchbox", "combobox" | "listbox" => "combobox", "tab" => "tab", "tablist" => "tablist",
                "tabpanel" => "tabpanel", "menu" | "menubar" => "menu", "menuitem" | "menuitemcheckbox" | "menuitemradio" => "menuitem", "dialog" | "alertdialog" => "dialog",
                "alert" | "status" | "log" => "status", "table" | "grid" => "table", "row" => "row", "cell" | "gridcell" => "cell", "columnheader" => "columnheader",
                "rowheader" => "rowheader", "figure" => "figure", "group" => "group", "separator" => "separator", "paragraph" => "paragraph", _ => return None });
        }
        let tag = d.tag(i);
        Some(match tag {
            "a" | "area" if d.attr(i, "href").is_some() => "link",
            "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => "heading",
            "p" => "paragraph",
            "ul" | "ol" | "menu" => "list", "li" => "listitem", "dl" => "list", "dt" | "dd" => "listitem",
            "img" => { if d.attr(i, "alt").map(|a| a.trim().is_empty()).unwrap_or(false) { return None; } "image" }
            "svg" if named_landmark(i) || d.attr(i, "role") == Some("img") => "image",
            "button" | "summary" => "button",
            "input" => match d.attr(i, "type").unwrap_or("text").to_ascii_lowercase().as_str() {
                "hidden" => return None, "checkbox" => "checkbox", "radio" => "radio", "submit" | "button" | "reset" | "image" => "button",
                "search" => "searchbox", "range" => "slider", _ => "textbox" },
            "textarea" => "textbox", "select" => "combobox",
            "nav" => "navigation", "main" => "main", "aside" => "complementary", "form" => if named_landmark(i) { "form" } else { return None },
            "section" => if named_landmark(i) { "region" } else { return None }, "article" => "article", "search" => "search",
            "header" | "footer" => {
                if up(d, d.nodes[i as usize].parent, &["article", "aside", "main", "nav", "section"]).is_some() { return None; }
                if tag == "header" { "banner" } else { "contentinfo" }
            }
            "table" => "table", "tr" => "row", "td" => "cell", "th" => "columnheader", "caption" => "caption",
            "figure" => "figure", "figcaption" => "caption", "blockquote" => "blockquote", "pre" | "code" => "code", "dialog" => "dialog",
            "hr" => "separator", "details" => "group", "fieldset" => "group", "legend" => "caption",
            _ => return None,
        })
    };
    // ARIA 1.2 §5.2.8.4 roles named from content (containers whose text is also read as children are not: no double reading)
    let name_from_content = |r: &str| matches!(r, "link" | "button" | "heading" | "tab" | "menuitem" | "columnheader" | "rowheader" | "caption" | "label" | "checkbox" | "radio");
    // leaf-ish roles: their text is their name — descendants' text is not repeated as children
    let swallows = |r: &str| matches!(r, "link" | "button" | "heading" | "tab" | "menuitem" | "columnheader" | "rowheader" | "caption" | "label" | "image" | "checkbox" | "radio" | "textbox" | "searchbox" | "combobox" | "slider");
    let bx = |mut i: u32| -> [i32; 4] { while i != NONE && (i as usize) < d.nodes.len() { if let Some(b) = first_box.get(&i) { return *b; } i = d.nodes[i as usize].parent; } [0; 4] };
    let mut out: Vec<Value> = vec![];
    // stack of (node range end, a11y index, swallow) for the kept ancestors
    let mut kept: Vec<(u32, i64, bool)> = vec![];
    let body = (0..n).find(|&i| el(i) && d.tag(i) == "body" && d.in_doc(i)).unwrap_or(0);
    let end = d.nodes[body as usize].end.min(n);
    let mut i = body + 1;
    while i < end {
        while let Some(&(e, _, _)) = kept.last() { if i >= e { kept.pop(); } else { break; } }
        let parent = kept.last().map(|k| k.1).unwrap_or(-1);
        let swallowed = kept.last().map(|k| k.2).unwrap_or(false);
        let node = &d.nodes[i as usize];
        match node.kind {
            dom::Kind::Element => {
                let tag = d.tag(i);
                let hidden_style = sty(i).map(|s| s.display == style::Display::None || s.visibility == 1).unwrap_or(false);
                if matches!(tag, "script" | "style" | "noscript" | "template" | "head" | "meta" | "link" | "title") || tag.starts_with("::")
                    || d.attr(i, "aria-hidden") == Some("true") || d.attr(i, "hidden").is_some() || hidden_style {
                    i = node.end.max(i + 1); continue;
                }
                if swallowed { i += 1; continue; }
                if let Some(r) = role_of(i) {
                    let name = name_of(i, name_from_content(r));
                    let name: String = name.chars().take(400).collect();
                    let b = bx(i);
                    let mut extra = serde_json::Map::new();
                    if r == "heading" { let lv = d.attr(i, "aria-level").and_then(|v| v.parse().ok()).unwrap_or_else(|| tag[1..].parse::<u32>().unwrap_or(2)); extra.insert("level".into(), json!(lv)); }
                    if r == "link" { if let Some(h) = d.attr(i, "href") { extra.insert("href".into(), json!(page::resolve_frag(base, h))); } }
                    if matches!(r, "checkbox" | "radio") { extra.insert("checked".into(), json!(d.attr(i, "checked").is_some() || d.attr(i, "aria-checked") == Some("true"))); }
                    if matches!(r, "textbox" | "searchbox") {
                        let v = if tag == "textarea" { text(i) } else { d.attr(i, "value").unwrap_or("").to_string() };
                        extra.insert("value".into(), json!(if d.attr(i, "type") == Some("password") { "•".repeat(v.chars().count()) } else { v }));
                        if let Some(p) = d.attr(i, "placeholder") { extra.insert("placeholder".into(), json!(p)); }
                        if d.attr(i, "type") == Some("password") { extra.insert("protected".into(), json!(true)); }
                        if tag == "textarea" { extra.insert("multiline".into(), json!(true)); }
                    }
                    if r == "combobox" && tag == "select" {
                        let e2 = d.nodes[i as usize].end.min(n);
                        let opts: Vec<u32> = (i + 1..e2).filter(|&o| el(o) && d.tag(o) == "option").collect();
                        let pick = opts.iter().copied().find(|&o| d.attr(o, "selected").is_some()).or(opts.first().copied());
                        if let Some(o) = pick { extra.insert("value".into(), json!(text(o))); }
                    }
                    if let Some(x) = d.attr(i, "aria-expanded") { extra.insert("expanded".into(), json!(x == "true")); }
                    if tag == "details" { extra.insert("expanded".into(), json!(d.attr(i, "open").is_some())); }
                    if d.attr(i, "disabled").is_some() || d.attr(i, "aria-disabled") == Some("true") { extra.insert("disabled".into(), json!(true)); }
                    if d.attr(i, "required").is_some() { extra.insert("required".into(), json!(true)); }
                    if matches!(r, "textbox" | "searchbox" | "combobox" | "checkbox" | "radio" | "button" | "slider") || d.attr(i, "tabindex").map(|t| t.trim() != "-1").unwrap_or(false) {
                        extra.insert("focusable".into(), json!(true));
                        extra.insert("node".into(), json!(i));
                    }
                    if r == "link" { extra.insert("focusable".into(), json!(true)); extra.insert("node".into(), json!(i)); }
                    out.push(json!([r, name, parent, b[0], b[1], b[2], b[3], Value::Object(extra)]));
                    kept.push((node.end, (out.len() - 1) as i64, swallows(r)));
                }
            }
            dom::Kind::Text => {
                if !swallowed {
                    let t = d.text(i).split_whitespace().collect::<Vec<_>>().join(" ");
                    if !t.is_empty() {
                        let b = bx(d.nodes[i as usize].parent);
                        // adjacent runs under the same parent read as one text node (inline markup inside a sentence)
                        let merge = out.last().map(|l| l[0] == "text" && l[2] == json!(parent)).unwrap_or(false);
                        if merge { let l = out.last_mut().unwrap(); let s = format!("{} {}", l[1].as_str().unwrap_or(""), t); l[1] = json!(s.chars().take(2000).collect::<String>()); }
                        else { out.push(json!(["text", t.chars().take(2000).collect::<String>(), parent, b[0], b[1], b[2], b[3], {}])); }
                    }
                }
            }
            _ => {}
        }
        i += 1;
    }
    out
}

/// What is under a page point: a link, a form control, or the node.
fn hit_json(d: &Dom, first_box: &HashMap<u32, [i32; 4]>, base: &str, hit: Option<u32>) -> Value {
    let Some(n) = hit else { return json!({}) };
    if let Some(a) = up(d, n, &["a"]).filter(|&a| d.attr(a, "href").is_some()) {
        let href = d.attr(a, "href").unwrap_or("");
        if href.trim_start().to_ascii_lowercase().starts_with("javascript:") { return json!({"node": n, "script_link": true}); }
        return json!({"link": page::resolve_frag(base, href), "text": text_of(d, a), "target": d.attr(a, "target").unwrap_or("")});
    }
    if let Some(i) = up(d, n, &["select"]) {
        // a select: its options (value, label) and the chosen one; the broker cycles / picks among them
        let e2 = d.nodes[i as usize].end.min(d.nodes.len() as u32);
        let opts: Vec<Value> = (i + 1..e2).filter(|&o| d.nodes[o as usize].kind == dom::Kind::Element && d.tag(o) == "option")
            .map(|o| json!([d.attr(o, "value").map(|s| s.to_string()).unwrap_or_else(|| text_of(d, o)), text_of(d, o), d.attr(o, "selected").is_some()])).collect();
        return json!({"input": i, "tag": "select", "type": "select", "name": d.attr(i, "name").unwrap_or(""), "options": opts, "form": up(d, i, &["form"]), "box": first_box.get(&i)});
    }
    if let Some(i) = up(d, n, &["input", "textarea", "button"]) {
        let ty = d.attr(i, "type").unwrap_or(if d.tag(i) == "button" { "submit" } else { "text" }).to_ascii_lowercase();
        let form = up(d, i, &["form"]);
        return json!({"input": i, "tag": d.tag(i), "type": ty, "name": d.attr(i, "name").unwrap_or(""),
               "value": d.attr(i, "value").unwrap_or(""), "placeholder": d.attr(i, "placeholder").unwrap_or(""),
               "checked": d.attr(i, "checked").is_some(), "form": form, "box": first_box.get(&i)});
    }
    json!({"node": n})
}

/// One computed property of an element as CSSOM serialises it (the properties scripts read; others fall back to inline style).
fn computed(s: &style::Style, prop: &str) -> Option<String> {
    use style::{Display as D, Len};
    let px = |v: i32| format!("{}px", (v as f64 / style::FX as f64 * 100.0).round() / 100.0);
    let len = |l: &Len| match l { Len::Px(v) => px(*v), Len::Pct(p) => format!("{}%", *p as f64 / 100.0), _ => "auto".to_string() };
    let rgba = |c: u32| { let (r, g, b, a) = (c >> 24, (c >> 16) & 255, (c >> 8) & 255, c & 255); if a == 255 { format!("rgb({r}, {g}, {b})") } else { format!("rgba({r}, {g}, {b}, {})", (a as f64 / 255.0 * 1000.0).round() / 1000.0) } };
    let side = |p: &str| match p.rsplit('-').next() { Some("top") => Some(0), Some("right") => Some(1), Some("bottom") => Some(2), Some("left") => Some(3), _ => None };
    Some(match prop {
        "display" => match s.display { D::None => "none", D::Block => "block", D::Inline => "inline", D::InlineBlock => "inline-block", D::ListItem => "list-item",
            D::Table => "table", D::InlineTable => "inline-table", D::TableRow => "table-row", D::TableCell => "table-cell", D::TableRowGroup => "table-row-group",
            D::TableHeaderGroup => "table-header-group", D::TableFooterGroup => "table-footer-group", D::TableCaption => "table-caption", D::TableColumn => "table-column",
            D::TableColumnGroup => "table-column-group", D::Flex => "flex", D::InlineFlex => "inline-flex", D::Grid => "grid", D::InlineGrid => "inline-grid", D::Contents => "contents" }.into(),
        "position" => ["static", "relative", "absolute", "fixed", "sticky"].get(s.position as usize).unwrap_or(&"static").to_string(),
        "float" => ["none", "left", "right"].get(s.float as usize).unwrap_or(&"none").to_string(),
        "visibility" => if s.visibility == 1 { "hidden" } else { "visible" }.into(),
        "overflow" | "overflow-x" | "overflow-y" => ["visible", "hidden", "auto"].get(s.overflow as usize).unwrap_or(&"visible").to_string(),
        "color" => rgba(s.color),
        "background-color" => rgba(s.bg),
        "font-size" => px(s.font_size),
        "font-weight" => s.font_weight.to_string(),
        "font-style" => if s.italic { "italic" } else { "normal" }.into(),
        "line-height" => match s.line_height { Len::Auto => "normal".into(), Len::Px(v) => px(v), Len::Pct(p) => px(s.font_size * p / 100) , _ => "normal".into() },
        "text-align" => ["start", "left", "right", "center", "justify"].get(s.text_align as usize).unwrap_or(&"start").to_string(),
        "white-space" => ["normal", "pre", "nowrap", "pre-wrap", "pre-line"].get(s.white_space as usize).unwrap_or(&"normal").to_string(),
        "box-sizing" => if s.border_box { "border-box" } else { "content-box" }.into(),
        "flex-direction" => ["row", "row-reverse", "column", "column-reverse"].get(s.flex_dir as usize).unwrap_or(&"row").to_string(),
        "flex-wrap" => if s.flex_wrap { "wrap" } else { "nowrap" }.into(),
        "min-width" => len(&s.min_w), "max-width" => match s.max_w { Len::Auto => "none".into(), ref l => len(l) },
        "min-height" => len(&s.min_h), "max-height" => match s.max_h { Len::Auto => "none".into(), ref l => len(l) },
        p if p.starts_with("margin-") => len(&s.margin[side(p)?]),
        p if p.starts_with("padding-") => len(&s.padding[side(p)?]),
        p if p.starts_with("border-") && p.ends_with("-width") => px(s.border_w[side(&p[..p.len() - 6])?]),
        "top" | "right" | "bottom" | "left" => len(&s.inset[side(prop)?]),
        "opacity" => "1".into(),
        _ => return None,
    })
}

fn zoom_table(t: &mut [style::Style], base: &[style::Style], z: i64) {
    for (s, b) in t.iter_mut().zip(base) {
        s.font_size = (b.font_size as i64 * z / 100) as i32;
        if let style::Len::Px(v) = b.line_height { s.line_height = style::Len::Px((v as i64 * z / 100) as i32); }
    }
}

/// The page as loaded, or the live tree's re-serialisation after scripts changed it (subresources from the same cache).
struct Over<'a> { inner: &'a Cached<'a>, html: Option<Vec<u8>> }
impl page::Source for Over<'_> {
    fn url(&self) -> String { self.inner.url() }
    fn html(&self) -> Vec<u8> { match &self.html { Some(h) => h.clone(), None => self.inner.html() } }
    fn get(&self, url: &str) -> Option<Vec<u8>> { self.inner.get(url) }
}

/// PARSE-DOCK P3: a script the broker has parsed ('c' frame {url, module} → 'B' blob | 'W' still parsing, ask again | 'N' parse it yourself).
pub fn broker_compiled(url: &str, module: bool) -> Option<Value> {
    let t0 = Instant::now();
    loop {
        let o = std::io::stdout();
        write_frame(&mut o.lock(), b'c', json!({"url": url, "module": module}).to_string().as_bytes()).ok()?;
        let i = std::io::stdin();
        let (t, d) = read_frame(&mut i.lock())?;
        match t {
            b'B' => { let t1 = Instant::now(); let r = crate::jscomp::load(&d); if std::env::var_os("OMR_FNSTAT").is_some() { eprintln!("feed: {} KB blob, waited {:.0} ms, loaded in {:.0} ms · {}", d.len() / 1024, (t1 - t0).as_secs_f64() * 1e3, t1.elapsed().as_secs_f64() * 1e3, &url[url.len().saturating_sub(40)..]); } return r; }
            b'W' if t0.elapsed().as_secs() < 120 => std::thread::sleep(std::time::Duration::from_millis(20)),
            _ => return None,
        }
    }
}

/// One GET through the broker, from code that has no Source at hand.
pub fn broker_get(url: &str) -> Option<Vec<u8>> {
    let o = std::io::stdout();
    write_frame(&mut o.lock(), b'G', url.as_bytes()).ok()?;
    let i = std::io::stdin();
    let (t, d) = read_frame(&mut i.lock())?;
    if t == b'B' { Some(d) } else { None }
}

/// A script's request (fetch / XHR) through the broker: (status, final url, headers, body). The renderer has no network.
pub fn broker_req(method: &str, url: &str, ctype: &str, body: Option<&str>) -> Option<(u16, String, Vec<(String, String)>, Vec<u8>)> {
    let o = std::io::stdout();
    write_frame(&mut o.lock(), b'P', json!({"method": method, "url": url, "ctype": ctype, "body": body}).to_string().as_bytes()).ok()?;
    let i = std::io::stdin();
    let (t, d) = read_frame(&mut i.lock())?;
    if t != b'B' || d.len() < 6 { return None; }
    let status = u16::from_le_bytes([d[0], d[1]]);
    let ul = u32::from_le_bytes([d[2], d[3], d[4], d[5]]) as usize;
    let u = String::from_utf8_lossy(d.get(6..6 + ul)?).into_owned();
    let p = 6 + ul;
    let hl = u32::from_le_bytes(d.get(p..p + 4)?.try_into().ok()?) as usize;
    let hs: Vec<(String, String)> = serde_json::from_slice(d.get(p + 4..p + 4 + hl)?).unwrap_or_default();
    Some((status, u, hs, d[p + 4 + hl..].to_vec()))
}

/// Ask the broker to preload `urls` in parallel ('g' frame); returns when they are cached.
pub fn broker_prefetch(urls: &[String]) {
    if urls.len() < 2 { return; }
    let o = std::io::stdout();
    if write_frame(&mut o.lock(), b'g', serde_json::to_string(urls).unwrap_or_default().as_bytes()).is_err() { return; }
    let i = std::io::stdin();
    let _ = read_frame(&mut i.lock());
}

/// The static import / re-export specifiers of a module (estree Program), resolved against its URL.
fn module_deps(prog: &Value, base: &str) -> Vec<String> {
    prog["body"].as_array().map(|b| b.iter().filter(|s| matches!(s["type"].as_str(), Some("ImportDeclaration" | "ExportNamedDeclaration" | "ExportAllDeclaration")))
        .filter_map(|s| s["source"]["value"].as_str()).filter(|sp| sp.starts_with("./") || sp.starts_with("../") || sp.starts_with('/') || sp.contains("://"))
        .map(|sp| page::resolve(base, sp)).collect()).unwrap_or_default()
}

/// A preload scan of the document (HTML §8.1.3 speculative parsing): stylesheets, classic + module scripts, the first
/// candidate of every image — fetched in parallel before the page loads them one by one.
fn preload_scan(doc: &[u8], url: &str) -> Vec<String> {
    let d = Dom::parse(doc);
    let base = (0..d.nodes.len() as u32).find(|&i| d.nodes[i as usize].kind == dom::Kind::Element && d.tag(i) == "base")
        .and_then(|i| d.attr(i, "href")).map(|h| page::resolve(url, h)).unwrap_or(url.to_string());
    let mut v = vec![];
    for i in 0..d.nodes.len() as u32 {
        if d.nodes[i as usize].kind != dom::Kind::Element || !d.in_doc(i) { continue; }
        match d.tag(i) {
            "link" if d.attr(i, "rel").map(|r| r.split_whitespace().any(|w| w.eq_ignore_ascii_case("stylesheet") || w.eq_ignore_ascii_case("modulepreload"))).unwrap_or(false) => { if let Some(h) = d.attr(i, "href") { v.push(page::resolve(&base, h)); } }
            "script" => { if let Some(h) = d.attr(i, "src") { v.push(page::resolve(&base, h)); } }
            "img" => { if let Some(h) = page::img_srcs(&d, i).first() { if !h.starts_with("data:") { v.push(page::resolve(&base, h)); } } }
            _ => {}
        }
    }
    v.truncate(300);
    v
}

/// A WebSocket operation through the broker ('w' frame): the page never holds a socket.
pub fn broker_ws(v: &Value) -> Value {
    let o = std::io::stdout();
    if write_frame(&mut o.lock(), b'w', v.to_string().as_bytes()).is_err() { return json!({"error": "broker"}); }
    let i = std::io::stdin();
    match read_frame(&mut i.lock()) { Some((b'B', d)) => serde_json::from_slice(&d).unwrap_or(json!({})), _ => json!({"error": "broker"}) }
}

/// document.cookie through the broker's jar: None = read (HttpOnly hidden), Some(line) = write.
pub fn broker_cookie(op: Option<&str>) -> String {
    let o = std::io::stdout();
    let msg = match op { None => b"g".to_vec(), Some(l) => [b"s".as_slice(), l.as_bytes()].concat() };
    if write_frame(&mut o.lock(), b'O', &msg).is_err() { return String::new(); }
    let i = std::io::stdin();
    match read_frame(&mut i.lock()) { Some((b'B', d)) => String::from_utf8_lossy(&d).into_owned(), _ => String::new() }
}

/// A Response-like object for fetch() (Fetch §6.4: ok, status, statusText, url, headers.get, text(), json()).
pub fn response_obj(status: u16, url: &str, headers: Vec<(String, String)>, body: Vec<u8>) -> crate::jsi::Val {
    use crate::jsi::{st, Val};
    use crate::jsi_lib::{json_parse, rejected, resolved};
    let ct = headers.iter().find(|(k, _)| k == "content-type").map(|(_, v)| v.clone());
    let text = String::from_utf8_lossy(&crate::net::to_utf8(body, ct.as_deref())).into_owned();
    let t2 = text.clone();
    let hs = Rc::new(headers);
    let h2 = hs.clone();
    let headers_obj = Val::plain(vec![
        ("get", Val::native("get", move |it, _t, a| { let k = it.to_str(&a.first().cloned().unwrap_or(Val::Undef))?.to_ascii_lowercase();
            Ok(match hs.iter().filter(|(x, _)| *x == k).map(|(_, v)| v.clone()).collect::<Vec<_>>() { v if v.is_empty() => Val::Null, v => st(&v.join(", ")) }) })),
        ("has", Val::native("has", move |it, _t, a| { let k = it.to_str(&a.first().cloned().unwrap_or(Val::Undef))?.to_ascii_lowercase(); Ok(Val::Bool(h2.iter().any(|(x, _)| *x == k))) })),
    ]);
    Val::plain(vec![("ok", Val::Bool((200..300).contains(&status))), ("status", Val::Num(status as f64)), ("statusText", st(if (200..300).contains(&status) { "OK" } else { "" })),
        ("url", st(url)), ("redirected", Val::Bool(false)), ("type", st("basic")), ("headers", headers_obj),
        ("json", Val::native("json", move |it, _t, _a| Ok(match json_parse(&t2) { Some(v) => resolved(v), None => rejected(it.error("SyntaxError", "Unexpected token in JSON")) }))),
        ("text", Val::native("text", move |_it, _t, _a| Ok(resolved(st(&text)))))])
}

/// fetch(url[, {method, body, headers}]) for page scripts (Fetch §5.1, the parts pages use): through the broker, which
/// applies the cookie policy; non-2xx statuses resolve (only network failure rejects).
fn js_fetch() -> crate::jsi::Val {
    use crate::jsi::Val;
    use crate::jsi_lib::{rejected, resolved};
    Val::native("fetch", |it, _t, a| {
        let target = a.first().cloned().unwrap_or(Val::Undef);
        // fetch(Request-like object) reads its url
        let u = if matches!(target, Val::Obj(_)) { match it.get(&target, "url")? { Val::Undef => it.to_str(&target)?.to_string(), v => it.to_str(&v)?.to_string() } } else { it.to_str(&target)?.to_string() };
        let base = it.dom.as_ref().map(|l| l.borrow().url.clone()).unwrap_or_default();
        let url = page::resolve(&base, &u);
        let opts = a.get(1).cloned().unwrap_or(Val::Undef);
        let (mut method, mut body, mut ctype) = ("GET".to_string(), None, String::new());
        if matches!(opts, Val::Obj(_)) {
            if let m @ Val::Str(_) = it.get(&opts, "method")? { method = it.to_str(&m)?.to_uppercase(); }
            match it.get(&opts, "body")? {
                Val::Undef | Val::Null => {},
                // Fetch §5.2 BodyInit: FormData → an entry list (sent urlencoded here), URLSearchParams → urlencoded, else text
                b @ Val::Obj(_) if it.get(&b, "__omrFormData")?.truthy() => { let f = it.get(&b, "_enc")?; let v = it.call(&f, b.clone(), vec![])?; body = Some(it.to_str(&v)?.to_string()); ctype = "application/x-www-form-urlencoded;charset=UTF-8".into(); }
                b @ Val::Obj(_) if it.get(&b, "__omrParams")?.truthy() || (it.get(&b, "getAll")?.is_callable() && it.get(&b, "append")?.is_callable() && !it.get(&b, "__omrFormData")?.truthy()) => { body = Some(it.to_str(&b)?.to_string()); ctype = "application/x-www-form-urlencoded;charset=UTF-8".into(); }
                b => { body = Some(it.to_str(&b)?.to_string()); ctype = "text/plain;charset=UTF-8".into(); }
            }
            let h = it.get(&opts, "headers")?;
            if matches!(h, Val::Obj(_)) {
                for k in ["Content-Type", "content-type"] { if let v @ Val::Str(_) = it.get(&h, k)? { ctype = it.to_str(&v)?.to_string(); } }
            }
        }
        if !url.starts_with("http://") && !url.starts_with("https://") && !url.starts_with("data:") { let e = it.error("TypeError", "Failed to fetch"); return Ok(rejected(e)); }
        if let Some(d) = url.strip_prefix("data:").and(crate::net::data_url(&url)) { return Ok(resolved(response_obj(200, &url, vec![], d))); }
        match broker_req(&method, &url, &ctype, body.as_deref()) {
            Some((status, fu, hs, b)) => Ok(resolved(response_obj(status, &fu, hs, b))),
            None => { let e = it.error("TypeError", "Failed to fetch"); Ok(rejected(e)) }
        }
    })
}

/// The broker's subresource cache: a page's images / CSS / fonts are fetched once per session (back / forward / reload of
/// the same site re-use them). Bounded: cleared when it passes `cap` bytes.
pub struct Broker { pub client: crate::net::Client, cache: RefCell<HashMap<String, Option<Rc<Vec<u8>>>>>, cached: std::cell::Cell<usize>, pub cap: usize,
    pub served: std::cell::Cell<usize>, pub refused: std::cell::Cell<usize>,
    /// the last navigation's HTTP status, and its body when it was not a page (a file the person may save)
    pub status: std::cell::Cell<u16>, pub download: RefCell<Option<(String, String, Vec<u8>)>>,
    /// open WebSockets: id → (owning page, socket); a page's sockets close when it goes (drop_page)
    ws: RefCell<HashMap<u32, (u64, crate::net::Ws)>>, ws_next: std::cell::Cell<u32>,
    /// PARSE-DOCK P3: (script url, module) → its parsed blob, or the background parse making it
    comp: std::sync::Arc<std::sync::Mutex<HashMap<(String, bool), CState>>> }

#[derive(Clone)]
enum CState { Running, Ready(std::sync::Arc<Vec<u8>>), Failed }

impl Broker {
    pub fn new() -> Broker { Broker { client: crate::net::Client::new(), cache: Default::default(), cached: Default::default(), cap: 48 << 20, served: Default::default(), refused: Default::default(),
        status: Default::default(), download: Default::default(), ws: Default::default(), ws_next: std::cell::Cell::new(1), comp: Default::default() } }
    /// Close every WebSocket page `page` opened (it navigated away or its tab closed).
    pub fn drop_page(&self, page: u64) {
        let mut w = self.ws.borrow_mut();
        let ids: Vec<u32> = w.iter().filter(|(_, (p, _))| *p == page).map(|(k, _)| *k).collect();
        for k in ids { if let Some((_, s)) = w.remove(&k) { s.close(1001, "page gone"); } }
    }
    /// Preload (HTML §8.1.3 "speculative fetch" / a preload scanner): fetch `urls` in parallel, ≤ 6 connections, into the cache.
    /// Each worker has its own client (no shared connections); preloads carry no cookies (scripts / styles / images do not need them).
    pub fn prefetch(&self, urls: Vec<String>) {
        let mut want: Vec<String> = urls.into_iter().filter(|u| (u.starts_with("http://") || u.starts_with("https://")) && !self.cache.borrow().contains_key(u)).collect();
        want.sort(); want.dedup();
        if want.len() < 2 { return; }
        let next = std::sync::atomic::AtomicUsize::new(0);
        let got: std::sync::Mutex<Vec<(String, Option<Vec<u8>>)>> = Default::default();
        std::thread::scope(|s| for _ in 0..6.min(want.len()) {
            s.spawn(|| { let c = crate::net::Client::new(); loop {
                let k = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                let Some(u) = want.get(k) else { break };
                let r = c.get(u).map(|x| x.1);
                got.lock().unwrap().push((u.clone(), r));
            } });
        });
        for (u, v) in got.into_inner().unwrap() {
            let n = v.as_ref().map(|b| b.len()).unwrap_or(0);
            if self.cached.get() + n > self.cap { break; }
            self.cached.set(self.cached.get() + n);
            if v.is_some() { self.cache.borrow_mut().insert(u, v.map(Rc::new)); }
        }
    }
    /// A page's WebSocket request ('w' frame).
    fn ws_req(&self, page: u64, page_url: &str, v: &Value) -> Value {
        match v["op"].as_str().unwrap_or("") {
            "open" => {
                let url = v["url"].as_str().unwrap_or("");
                if !(url.starts_with("ws://") || url.starts_with("wss://")) { return json!({"error": "SyntaxError: not a ws:// or wss:// URL"}); }
                let origin = crate::cookie::parts(page_url).map(|(t, h, _)| format!("{}://{h}", if t { "https" } else { "http" })).unwrap_or_default();
                match self.client.websocket(url, &origin) {
                    Ok(s) => { let id = self.ws_next.get(); self.ws_next.set(id + 1); self.ws.borrow_mut().insert(id, (page, s)); json!({"id": id}) }
                    Err(e) => json!({"error": e}),
                }
            }
            "send" => { let id = v["id"].as_u64().unwrap_or(0) as u32; let ok = self.ws.borrow().get(&id).filter(|(p, _)| *p == page).map(|(_, s)| s.send_text(v["data"].as_str().unwrap_or(""))).unwrap_or(false); json!({"ok": ok}) }
            "close" => { let id = v["id"].as_u64().unwrap_or(0) as u32; if let Some((_, s)) = self.ws.borrow().get(&id).filter(|(p, _)| *p == page) { s.close(v["code"].as_u64().unwrap_or(1000) as u16, v["reason"].as_str().unwrap_or("")); } json!({"ok": true}) }
            "poll" => {
                let mut ev = vec![]; let mut gone = vec![];
                for (id, (p, s)) in self.ws.borrow().iter() {
                    if *p != page { continue; }
                    while let Ok(e) = s.rx.try_recv() {
                        ev.push(match e {
                            crate::net::WsEvent::Open => json!([id, "open"]),
                            crate::net::WsEvent::Message(t) => json!([id, "message", t]),
                            crate::net::WsEvent::Binary(b) => json!([id, "binary", b.iter().map(|x| *x as u32).collect::<Vec<_>>()]),
                            crate::net::WsEvent::Close(c, r) => { gone.push(*id); json!([id, "close", c, r]) }
                            crate::net::WsEvent::Error(t) => json!([id, "error", t]),
                        });
                    }
                }
                for g in gone { self.ws.borrow_mut().remove(&g); }
                json!({"events": ev})
            }
            _ => json!({"error": "unknown op"}),
        }
    }
    /// A request a page script made ('P' frame): http(s) only, the cookie policy applies in net.rs.
    pub fn script_req(&self, v: &Value) -> Option<crate::net::Resp> {
        let url = v["url"].as_str()?;
        if !(url.starts_with("http://") || url.starts_with("https://")) { return None; }
        let method = v["method"].as_str().unwrap_or("GET").to_ascii_uppercase();
        if !matches!(method.as_str(), "GET" | "POST" | "PUT" | "DELETE" | "PATCH" | "HEAD" | "OPTIONS") { return None; }
        let body = v["body"].as_str().map(|b| (v["ctype"].as_str().filter(|c| !c.is_empty()).unwrap_or("text/plain;charset=UTF-8").to_string(), b.as_bytes().to_vec()));
        self.client.fetch(&method, url, body, "*/*", false)
    }
    /// Subresource policy: http(s) only, no local files (the renderer never names a path it can open itself).
    /// A script parsed ahead: the compact feed (jscomp::dump) from the memory map, the disk cache (keyed by the script's bytes),
    /// or a background parse started now. Scripts under 24 KB are not worth a round trip: Failed = the renderer parses them.
    fn compiled(&self, url: &str, module: bool) -> CState {
        if let Some(st) = self.comp.lock().unwrap().get(&(url.to_string(), module)) { return st.clone(); }
        let Some(bytes) = self.sub(url) else { return CState::Failed };
        if bytes.len() < 24 * 1024 { return CState::Failed; }
        let key = { let mut h = crate::jscomp::Fx::default(); use std::hash::Hasher; h.write(&bytes); h.write_u8(module as u8); h.finish() };
        let dir = std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h).join(".cache/omni-web/js"));
        let file = dir.as_ref().map(|d| d.join(format!("{key:016x}.omj2")));
        if let Some(f) = &file { if let Ok(b) = std::fs::read(f) { if b.starts_with(b"OMJ2") { let st = CState::Ready(std::sync::Arc::new(b)); self.comp.lock().unwrap().insert((url.to_string(), module), st.clone()); return st; } } }
        let comp = self.comp.clone(); let id = (url.to_string(), module); let text = bytes.as_ref().clone();
        comp.lock().unwrap().insert(id.clone(), CState::Running);
        let _ = std::thread::Builder::new().stack_size(512 << 20).spawn(move || {
            crate::jscomp::begin();
            let st = match crate::jsparse::parse(&String::from_utf8_lossy(&text), module) {
                Ok(prog) => { let blob = crate::jscomp::dump(&prog); if let (Some(d), Some(f)) = (&dir, &file) { let _ = std::fs::create_dir_all(d); let tmp = f.with_extension("tmp"); if std::fs::write(&tmp, &blob).is_ok() { let _ = std::fs::rename(&tmp, f); } } CState::Ready(std::sync::Arc::new(blob)) }
                Err(_) => CState::Failed,
            };
            comp.lock().unwrap().insert(id, st);
        });
        CState::Running
    }
    pub fn sub(&self, u: &str) -> Option<Rc<Vec<u8>>> {
        if !(u.starts_with("http://") || u.starts_with("https://")) { self.refused.set(self.refused.get() + 1); return None; }
        if let Some(v) = self.cache.borrow().get(u) { return v.clone(); }
        let v = self.client.get(u).map(|(_, b)| Rc::new(b));
        let n = v.as_ref().map(|b| b.len()).unwrap_or(0);
        if self.cached.get() + n > self.cap { self.cache.borrow_mut().clear(); self.cached.set(0); }
        self.cached.set(self.cached.get() + n);
        self.cache.borrow_mut().insert(u.to_string(), v.clone());
        if v.is_some() { self.served.set(self.served.get() + 1) } else { self.refused.set(self.refused.get() + 1) }
        v
    }
}

/// One page shown by the broker: the renderer child + its pipes.
/// The renderer's frames arrive on `rx` from a reader thread, so the window can poll a long request (`tick_start` /
/// `tick_poll`) instead of blocking on it; `busy` = such a request is in flight (other requests wait until it answers).
pub struct PageProc { child: std::process::Child, cin: std::process::ChildStdin, rx: std::sync::mpsc::Receiver<(u8, Vec<u8>)>,
    pub url: String, pub report: String, pub meta: Value, pub id: u64, pub busy: bool, tick_r: Option<Value> }

pub struct Shot { pub y: i32, pub page_h: i32, pub w: usize, pub h: usize, pub px: Vec<u32> }

impl PageProc {
    /// Spawn `cmd` (a renderer: it must call `serve()`), hand it the document, wait for its first layout.
    pub fn open(mut cmd: std::process::Command, b: &Broker, url: &str, doc: Vec<u8>, w: usize, h: usize) -> Result<PageProc, String> {
        let mut child = cmd.stdin(std::process::Stdio::piped()).stdout(std::process::Stdio::piped()).spawn().map_err(|e| format!("spawn renderer: {e}"))?;
        let cin = child.stdin.take().ok_or("stdin")?;
        let mut cout = child.stdout.take().ok_or("stdout")?;
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || { while let Some(f) = read_frame(&mut cout) { if tx.send(f).is_err() { break; } } });
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        let mut p = PageProc { child, cin, rx, url: url.to_string(), report: String::new(), meta: json!({}), id: NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed), busy: false, tick_r: None };
        write_frame(&mut p.cin, b'U', url.as_bytes()).map_err(|e| e.to_string())?;
        write_frame(&mut p.cin, b'D', &doc).map_err(|e| e.to_string())?;
        write_frame(&mut p.cin, b'V', format!("{w} {h}").as_bytes()).map_err(|e| e.to_string())?;
        let m = p.wait(b, b'L').ok_or("renderer ended before its first layout")?;
        p.meta = serde_json::from_slice(&m).unwrap_or(json!({}));
        Ok(p)
    }

    /// Read frames until `want`, serving subresource requests meanwhile.
    fn wait(&mut self, b: &Broker, want: u8) -> Option<Vec<u8>> {
        loop {
            let (tag, data) = self.rx.recv().ok()?;
            if tag == want { return Some(data); }
            if !self.serve(b, tag, data)? { return None; }
        }
    }

    /// A renderer request ('G' / 'P' / 'g' / 'w' / 'O') or report ('S'): answered here. Some(false) = an unexpected frame.
    fn serve(&mut self, b: &Broker, tag: u8, data: Vec<u8>) -> Option<bool> {
        {
            match tag {
                b'G' => {
                    let u = String::from_utf8_lossy(&data).into_owned();
                    match b.sub(&u) { Some(v) => write_frame(&mut self.cin, b'B', &v).ok()?, None => write_frame(&mut self.cin, b'N', &[]).ok()? }
                }
                b'P' => {
                    let v: Value = serde_json::from_slice(&data).unwrap_or(json!({}));
                    match b.script_req(&v) {
                        Some(r) => {
                            let hj = serde_json::to_vec(&r.headers.iter().filter(|(k, _)| k != "set-cookie").collect::<Vec<_>>()).unwrap_or_default();
                            let mut out = Vec::with_capacity(10 + r.url.len() + hj.len() + r.body.len());
                            out.extend_from_slice(&r.status.to_le_bytes());
                            out.extend_from_slice(&(r.url.len() as u32).to_le_bytes()); out.extend_from_slice(r.url.as_bytes());
                            out.extend_from_slice(&(hj.len() as u32).to_le_bytes()); out.extend_from_slice(&hj);
                            out.extend_from_slice(&r.body);
                            write_frame(&mut self.cin, b'B', &out).ok()?
                        }
                        None => write_frame(&mut self.cin, b'N', &[]).ok()?,
                    }
                }
                b'c' => {
                    let v: Value = serde_json::from_slice(&data).unwrap_or(json!({}));
                    match b.compiled(v["url"].as_str().unwrap_or(""), v["module"] == true) {
                        CState::Ready(blob) => write_frame(&mut self.cin, b'B', &blob).ok()?,
                        CState::Running => write_frame(&mut self.cin, b'W', &[]).ok()?,
                        CState::Failed => write_frame(&mut self.cin, b'N', &[]).ok()?,
                    }
                }
                b'g' => {
                    let v: Vec<String> = serde_json::from_slice(&data).unwrap_or_default();
                    b.prefetch(v);
                    write_frame(&mut self.cin, b'B', &[]).ok()?
                }
                b'w' => {
                    let v: Value = serde_json::from_slice(&data).unwrap_or(json!({}));
                    let r = b.ws_req(self.id, &self.url, &v);
                    write_frame(&mut self.cin, b'B', r.to_string().as_bytes()).ok()?
                }
                b'O' => {
                    let site = b.client.site.borrow().clone();
                    let out = match data.split_first() {
                        Some((b'g', _)) => b.client.jar.borrow_mut().header(&self.url, &site, true),
                        Some((b's', line)) => { b.client.jar.borrow_mut().set(&self.url, &String::from_utf8_lossy(line), &site, true); String::new() }
                        _ => String::new(),
                    };
                    write_frame(&mut self.cin, b'B', out.as_bytes()).ok()?
                }
                b'S' => self.report = String::from_utf8_lossy(&data).into_owned(),
                _ => return Some(false),
            }
        }
        Some(true)
    }

    /// Start a page tick without waiting for it (the first one runs the page's scripts).
    pub fn tick_start(&mut self, ms: i64) -> bool {
        if self.busy { return true; }
        if write_frame(&mut self.cin, b'k', format!("{ms} 0").as_bytes()).is_err() { return false; }
        self.busy = true; self.tick_r = None;
        true
    }
    /// Serve whatever the renderer asked for so far; Some(reply) when the tick finished (after its relayout's 'L', if any).
    pub fn tick_poll(&mut self, b: &Broker) -> Option<Value> {
        if !self.busy { return None; }
        loop {
            let (tag, data) = match self.rx.try_recv() { Ok(f) => f, Err(std::sync::mpsc::TryRecvError::Empty) => return None,
                Err(_) => { self.busy = false; return Some(json!({"dead": true})); } };
            match tag {
                b'A' if self.tick_r.is_none() => {
                    let r: Value = serde_json::from_slice(&data).unwrap_or(json!({}));
                    if r["relayout"] == true { self.tick_r = Some(r); continue; }
                    self.busy = false; return Some(r);
                }
                b'L' if self.tick_r.is_some() => { self.meta = serde_json::from_slice(&data).unwrap_or(json!({})); self.busy = false; return self.tick_r.take(); }
                _ => { if self.serve(b, tag, data) != Some(true) { self.busy = false; return Some(json!({"dead": true})); } }
            }
        }
    }

    fn ask(&mut self, b: &Broker, tag: u8, data: &[u8], want: u8) -> Option<Vec<u8>> {
        write_frame(&mut self.cin, tag, data).ok()?;
        self.wait(b, want)
    }

    pub fn frame(&mut self, b: &Broker, y: i32) -> Option<Shot> {
        let f = self.ask(b, b'Y', format!("{} 0", y.max(0)).as_bytes(), b'F')?;
        if f.len() < 16 { return None; }
        let u = |i: usize| u32::from_le_bytes([f[i], f[i + 1], f[i + 2], f[i + 3]]);
        let (y, page_h, w, h) = (u(0) as i32, u(4) as i32, u(8) as usize, u(12) as usize);
        let px: Vec<u32> = f[16..].chunks_exact(4).map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]])).collect();
        if px.len() != w * h { return None; }
        Some(Shot { y, page_h, w, h, px })
    }
    pub fn resize(&mut self, b: &Broker, w: usize, h: usize) -> Option<Value> {
        let m = self.ask(b, b'V', format!("{w} {h}").as_bytes(), b'L')?;
        self.meta = serde_json::from_slice(&m).ok()?;
        Some(self.meta.clone())
    }
    /// A click at page point (x, y): the DOM click event runs first; the reply says what the broker should still do
    /// (link / input as `hit`, unless `prevented`; `navigate` = a script asked for a URL; `relayout` = the page changed).
    pub fn click(&mut self, b: &Broker, x: i32, y: i32) -> Value {
        let Some(r) = self.ask(b, b'C', format!("{x} {y}").as_bytes(), b'A').and_then(|v| serde_json::from_slice::<Value>(&v).ok()) else { return json!({}) };
        if r["relayout"] == true { if let Some(m) = self.wait(b, b'L') { self.meta = serde_json::from_slice(&m).unwrap_or(json!({})); } }
        r
    }
    pub fn hit(&mut self, b: &Broker, x: i32, y: i32) -> Value { self.ask(b, b'H', format!("{x} {y}").as_bytes(), b'A').and_then(|v| serde_json::from_slice(&v).ok()).unwrap_or(json!({})) }
    pub fn find(&mut self, b: &Broker, q: &str) -> Value { self.ask(b, b'Q', q.as_bytes(), b'A').and_then(|v| serde_json::from_slice(&v).ok()).unwrap_or(json!({"hits": 0, "boxes": []})) }
    pub fn zoom(&mut self, b: &Broker, pct: i32) -> Option<Value> {
        let m = self.ask(b, b'Z', format!("{pct} 0").as_bytes(), b'L')?;
        self.meta = serde_json::from_slice(&m).ok()?;
        Some(self.meta.clone())
    }
    pub fn links(&mut self, b: &Broker) -> Vec<(String, String, i32)> {
        self.ask(b, b'J', &[], b'A').and_then(|v| serde_json::from_slice::<Value>(&v).ok()).and_then(|v| v["links"].as_array().cloned()).unwrap_or_default()
            .iter().map(|l| (l[0].as_str().unwrap_or("").to_string(), l[1].as_str().unwrap_or("").to_string(), l[2].as_i64().unwrap_or(-1) as i32)).collect()
    }
    pub fn anchor(&mut self, b: &Broker, frag: &str) -> i32 {
        self.ask(b, b'T', frag.as_bytes(), b'A').and_then(|v| serde_json::from_slice::<Value>(&v).ok()).and_then(|v| v["y"].as_i64()).unwrap_or(-1) as i32
    }
    pub fn fields(&mut self, b: &Broker) -> Vec<Value> {
        self.ask(b, b'E', &[], b'A').and_then(|v| serde_json::from_slice::<Value>(&v).ok()).and_then(|v| v["fields"].as_array().cloned()).unwrap_or_default()
    }
    pub fn a11y(&mut self, b: &Broker) -> Vec<Value> {
        self.ask(b, b'W', &[], b'A').and_then(|v| serde_json::from_slice::<Value>(&v).ok()).and_then(|v| v["nodes"].as_array().cloned()).unwrap_or_default()
    }
    pub fn markdown(&mut self, b: &Broker) -> String { self.ask(b, b'M', &[], b'R').map(|v| String::from_utf8_lossy(&v).into_owned()).unwrap_or_default() }
    /// `ms` of real time passed: run the page's due timers; the reply says what changed (relayout → new meta read here).
    pub fn tick(&mut self, b: &Broker, ms: i64) -> Value {
        let Some(r) = self.ask(b, b'k', format!("{ms} 0").as_bytes(), b'A').and_then(|v| serde_json::from_slice::<Value>(&v).ok()) else { return json!({}) };
        if r["relayout"] == true { if let Some(m) = self.wait(b, b'L') { self.meta = serde_json::from_slice(&m).unwrap_or(json!({})); } }
        r
    }
    /// The person's form values into the page's controls + input / change events, then (`submit`) a cancelable submit
    /// event; the reply says whether a script took over (prevented / navigate / relayout).
    pub fn form_event(&mut self, b: &Broker, form: u64, values: &HashMap<String, String>, submit: bool) -> Value {
        let Some(r) = self.ask(b, b'I', json!({"form": form, "values": values, "submit": submit}).to_string().as_bytes(), b'A').and_then(|v| serde_json::from_slice::<Value>(&v).ok()) else { return json!({}) };
        if r["relayout"] == true { if let Some(m) = self.wait(b, b'L') { self.meta = serde_json::from_slice(&m).unwrap_or(json!({})); } }
        r
    }
    pub fn submit(&mut self, b: &Broker, form: u64, values: &HashMap<String, String>) -> Value {
        self.ask(b, b'K', json!({"form": form, "values": values}).to_string().as_bytes(), b'A').and_then(|v| serde_json::from_slice(&v).ok()).unwrap_or(json!({"error": "renderer"}))
    }
    /// Resident set of the renderer child (kB), for the RAM table.
    pub fn rss_kb(&self) -> u64 { proc_rss_kb(self.child.id()) }
}

impl Drop for PageProc {
    // a renderer still busy with a request cannot read 'X': it is stopped instead
    fn drop(&mut self) { if self.busy { let _ = self.child.kill(); } else { let _ = write_frame(&mut self.cin, b'X', &[]); } let _ = self.child.wait(); }
}

pub fn proc_rss_kb(pid: u32) -> u64 {
    std::fs::read_to_string(format!("/proc/{pid}/status")).ok().and_then(|s| s.lines().find(|l| l.starts_with("VmRSS:"))
        .and_then(|l| l.split_whitespace().nth(1)).and_then(|v| v.parse().ok())).unwrap_or(0)
}
pub fn proc_hwm_kb(pid: u32) -> u64 {
    std::fs::read_to_string(format!("/proc/{pid}/status")).ok().and_then(|s| s.lines().find(|l| l.starts_with("VmHWM:"))
        .and_then(|l| l.split_whitespace().nth(1)).and_then(|v| v.parse().ok())).unwrap_or(0)
}

/// A navigation's answer. `doc` is always HTML: error pages are shown as the server sent them; other content types are
/// wrapped so they still read (text → <pre>, images → <img>, anything else → a "save this file" page; the bytes wait in
/// `Broker::download`).
pub struct Nav { pub url: String, pub status: u16, pub doc: Vec<u8>, pub mime: String }

fn esc(s: &str) -> String { s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;") }

pub fn navigate(b: &Broker, method: &str, url: &str, body: Option<(String, Vec<u8>)>) -> Option<Nav> {
    if let Some(bytes) = url.strip_prefix("data:").and(crate::net::data_url(url)) {
        let mime = url[5..].split([',', ';']).next().unwrap_or("").to_ascii_lowercase();
        return Some(Nav { url: url.to_string(), status: 200, doc: wrap_doc(b, url, &mime, None, bytes), mime });
    }
    let r = b.client.fetch(method, url, body, crate::net::ACCEPT_DOC, true)?;
    b.status.set(r.status);
    let ct = r.header("content-type").map(|s| s.to_string());
    let mime = ct.as_deref().unwrap_or("").split(';').next().unwrap_or("").trim().to_ascii_lowercase();
    let doc = wrap_doc(b, &r.url, &mime, ct.as_deref(), r.body);
    Some(Nav { url: r.url, status: r.status, doc, mime })
}

fn wrap_doc(b: &Broker, url: &str, mime: &str, ct: Option<&str>, body: Vec<u8>) -> Vec<u8> {
    *b.download.borrow_mut() = None;
    let name = url.split(['?', '#']).next().unwrap_or("").rsplit('/').find(|s| !s.is_empty()).unwrap_or("file").to_string();
    let looks_html = body[..body.len().min(512)].iter().any(|&c| c == b'<');
    match mime {
        "text/html" | "application/xhtml+xml" => crate::net::to_utf8(body, ct),
        "" if looks_html || body.is_empty() => crate::net::to_utf8(body, ct),
        m if m.starts_with("image/") => {
            b.cache.borrow_mut().insert(url.to_string(), Some(Rc::new(body)));
            format!("<!doctype html><title>{}</title><body style=\"margin:0;background:#1e1e1e;text-align:center\"><img src=\"{}\" alt=\"{}\" style=\"max-width:100%\"></body>", esc(&name), esc(url), esc(&name)).into_bytes()
        }
        m if m.is_empty() || m.starts_with("text/") || m.contains("json") || m.contains("javascript") || m.ends_with("xml") => {
            let txt = String::from_utf8_lossy(&crate::net::to_utf8(body, ct)).into_owned();
            format!("<!doctype html><title>{}</title><pre style=\"white-space:pre-wrap\">{}</pre>", esc(&name), esc(&txt)).into_bytes()
        }
        m => {
            let n = body.len();
            *b.download.borrow_mut() = Some((url.to_string(), m.to_string(), body));
            format!("<!doctype html><title>{0}</title><h1>{0}</h1><p>This address is a file, not a page: <b>{1}</b>, {2} bytes.</p>\
                     <p>Press <b>s</b> or choose <b>Save</b> to keep it in your Downloads folder.</p>", esc(&name), esc(m), n).into_bytes()
        }
    }
}

/// The document a navigation shows: (final URL after redirects, bytes) — any status (error pages read as sent).
pub fn fetch_doc(b: &Broker, url: &str) -> Option<(String, Vec<u8>)> { navigate(b, "GET", url, None).map(|n| (n.url, n.doc)) }
