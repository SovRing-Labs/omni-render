// SPDX-License-Identifier: MIT OR Apache-2.0
//! Page loading from a frozen corpus bundle (offline): URL resolution, stylesheet collection in
//! document order (<link rel=stylesheet> + <style>, @import depth 3), cascade.
use crate::css::{Media, Sheet};
use crate::dom::{Dom, Kind};
use crate::style::{self, Styled};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// RFC 3986 reference resolution (the subset browsers need for subresources).
/// `resolve` for link targets: the fragment is kept (in-page anchors scroll instead of reloading; WHATWG URL fragment state).
pub fn resolve_frag(base: &str, href: &str) -> String {
    let h = href.trim();
    if let Some(f) = h.strip_prefix('#') { return format!("{}#{f}", base.split('#').next().unwrap_or("")); }
    match h.split_once('#') { Some((a, f)) if !(h.len() > 5 && h[..5].eq_ignore_ascii_case("data:")) => format!("{}#{f}", resolve(base, a)), _ => resolve(base, h) }
}

pub fn resolve(base: &str, href: &str) -> String {
    let href = href.trim();
    // RFC 2397: a data: URL is whole in itself ('#' may be part of its payload, e.g. SVG colours)
    if href.len() > 5 && href[..5].eq_ignore_ascii_case("data:") { return href.to_string(); }
    let href = href.split('#').next().unwrap_or("");
    if href.is_empty() { return base.split('#').next().unwrap_or("").to_string(); }
    if let Some(p) = href.find(':') {
        if href[..p].chars().all(|c| c.is_ascii_alphanumeric() || c == '+' || c == '-' || c == '.') && p > 1 { return href.to_string(); }
    }
    let scheme_end = base.find("://").map(|x| x + 3).unwrap_or(0);
    let scheme = &base[..scheme_end.saturating_sub(3)];
    if let Some(rest) = href.strip_prefix("//") { return format!("{scheme}://{rest}"); }
    let host_end = base[scheme_end..].find('/').map(|x| x + scheme_end).unwrap_or(base.len());
    let origin = &base[..host_end];
    if href.starts_with('/') { return format!("{origin}{}", norm(href)); }
    let bpath = &base[host_end..];
    let bpath = bpath.split(['?', '#']).next().unwrap_or("");
    if href.starts_with('?') { return format!("{origin}{bpath}{href}"); }
    let dir = match bpath.rfind('/') { Some(i) => &bpath[..=i], None => "/" };
    format!("{origin}{}", norm(&format!("{dir}{href}")))
}

fn norm(path: &str) -> String {
    let (p, q) = match path.find('?') { Some(i) => (&path[..i], &path[i..]), None => (path, "") };
    let mut out: Vec<&str> = Vec::new();
    let segs: Vec<&str> = p.split('/').collect();
    for (k, s) in segs.iter().enumerate() {
        match *s {
            "." => { if k == segs.len() - 1 { out.push(""); } }
            ".." => { if out.len() > 1 { out.pop(); } if k == segs.len() - 1 { out.push(""); } }
            _ => out.push(s),
        }
    }
    let j = out.join("/");
    format!("{}{q}", if j.starts_with('/') { j } else { format!("/{j}") })
}

/// Where a page's bytes come from: a frozen corpus bundle or the network.
pub trait Source {
    fn url(&self) -> String;
    fn html(&self) -> Vec<u8>;
    fn get(&self, url: &str) -> Option<Vec<u8>>;
}

pub struct Bundle {
    pub dir: PathBuf,
    pub url: String,
    pub res: HashMap<String, (String, String)>, // url -> (file, type)
}

impl Bundle {
    pub fn open(dir: &Path) -> Option<Bundle> {
        let m: serde_json::Value = serde_json::from_slice(&std::fs::read(dir.join("manifest.json")).ok()?).ok()?;
        let url = m["final_url"].as_str().unwrap_or(m["url"].as_str()?).to_string();
        let mut res = HashMap::new();
        if let Some(o) = m["res"].as_object() {
            for (k, v) in o { res.insert(k.clone(), (v["file"].as_str()?.to_string(), v["type"].as_str()?.to_string())); }
        }
        Some(Bundle { dir: dir.to_path_buf(), url, res })
    }
    pub fn get(&self, url: &str) -> Option<Vec<u8>> {
        let (f, _) = self.res.get(url)?;
        std::fs::read(self.dir.join("res").join(f)).ok()
    }
    pub fn html(&self) -> Vec<u8> { std::fs::read(self.dir.join("page.html")).unwrap_or_default() }
}

impl Source for Bundle {
    fn url(&self) -> String { self.url.clone() }
    fn html(&self) -> Vec<u8> { Bundle::html(self) }
    fn get(&self, url: &str) -> Option<Vec<u8>> { Bundle::get(self, url) }
}

/// A live page over HTTP: the document is fetched once; subresources on demand.
/// A local .html file; its relative resources are read from disk (file:// URLs only, nothing fetched).
pub struct Local { pub path: PathBuf, pub url: String }

impl Local {
    pub fn open(p: &Path) -> Option<Local> {
        let abs = std::fs::canonicalize(p).ok()?;
        Some(Local { url: format!("file://{}", abs.display()), path: abs })
    }
}

impl Source for Local {
    fn url(&self) -> String { self.url.clone() }
    fn html(&self) -> Vec<u8> { std::fs::read(&self.path).unwrap_or_default() }
    fn get(&self, url: &str) -> Option<Vec<u8>> { std::fs::read(url.strip_prefix("file://")?.split(['?', '#']).next()?).ok() }
}

pub struct Live<'a> { pub client: &'a crate::net::Client, pub url: String, pub doc: Vec<u8> }

impl<'a> Live<'a> {
    pub fn fetch(client: &'a crate::net::Client, url: &str) -> Option<Live<'a>> {
        let (final_url, doc) = client.get(url)?;
        Some(Live { client, url: final_url, doc })
    }
}

impl Source for Live<'_> {
    fn url(&self) -> String { self.url.clone() }
    fn html(&self) -> Vec<u8> { self.doc.clone() }
    fn get(&self, url: &str) -> Option<Vec<u8>> { self.client.get(url).map(|x| x.1) }
}

pub struct Loaded { pub dom: Dom, pub sheet: Sheet, pub styled: Styled, pub css_bytes: usize, pub missing_css: usize, pub parse_ms: f64, pub css_ms: f64,
    /// web fonts the page actually uses: (family, weight lo, weight hi, italic, bytes)
    pub fonts: Vec<(String, u16, u16, bool, Vec<u8>)> }

/// The document's sheet (UA + quirks + <style> + <link> with @import and media). `filter` = drop rules whose id / class /
/// tag keys never occur in `dom` (load-time speed-up); an incremental session (INCR-R3) builds it with filter = false,
/// because a class added later must still find its rules.
pub fn build_sheet(dom: &Dom, b: &dyn Source, viewport: (f32, f32), filter: bool) -> (Sheet, usize, usize) {
    let m = Media { width: viewport.0, height: viewport.1 };
    let mut sheet = Sheet::new();
    if filter { sheet.filter = Some(crate::css::KeyFilter::from_dom(dom)); }
    let mut order = 0u32;
    sheet.add(style::UA, &m, &mut order);
    if dom.quirks { sheet.add(style::UA_QUIRKS, &m, &mut order); }
    sheet.ua_rules = order;
    let mut base = b.url();
    for i in 0..dom.nodes.len() as u32 {
        if dom.tag(i) == "base" { if let Some(h) = dom.attr(i, "href") { base = resolve(&b.url(), h); break; } }
    }
    let style_text: HashMap<u32, &str> = dom.sheets.iter().map(|(n, s)| (*n, s.as_str())).collect();
    let (mut css_bytes, mut missing) = (0, 0);
    // stylesheets inside a declarative shadow root are scoped to it; until scoping exists they are left out of the
    // document cascade so they cannot restyle the page
    let in_shadow = |mut n: u32| { while n != crate::dom::NONE { if dom.tag(n) == "::shadow" { return true; } n = dom.nodes[n as usize].parent; } false };
    let shadow_root = |mut n: u32| { while n != crate::dom::NONE { if dom.tag(n) == "::shadow" { return n; } n = dom.nodes[n as usize].parent; } crate::dom::NONE };
    // patch.shadow_styles: a declarative shadow root's stylesheets apply INSIDE that shadow tree (scoped rules; DOM §4.2.2 /
    // CSS Scoping 1 §3) — nytimes' horizontal feed `._feed_…{display:flex}` lives in one
    let scoped_on = crate::layout::patch_on(crate::layout::PATCH_SHADOW_STYLES);
    for i in 0..dom.nodes.len() as u32 {
        if dom.nodes[i as usize].kind != Kind::Element { continue; }
        sheet.cur_scope = u32::MAX;
        let mut host_tag: Option<String> = None;
        if matches!(dom.tag(i), "style" | "link") && in_shadow(i) {
            if !scoped_on { continue; }
            let sr = shadow_root(i); sheet.cur_scope = sr; sheet.scoped = true;
            let h = dom.nodes[sr as usize].parent; if h != crate::dom::NONE { host_tag = Some(dom.tag(h).to_string()); }
        }
        // `:host` / `:host(sel)` in a shadow stylesheet select the host element (CSS Scoping 1 §3.2): rewritten to the host's tag
        let host_css = |t: &str| -> String { match &host_tag { None => t.to_string(), Some(h) => {
            let mut out = String::with_capacity(t.len()); let mut rest = t;
            while let Some(k) = rest.find(":host") { out.push_str(&rest[..k]); let after = &rest[k + 5..];
                if after.starts_with('(') { if let Some(e) = after.find(')') { out.push_str(h); out.push_str(&after[1..e]); rest = &after[e + 1..]; continue; } }
                if after.starts_with('-') { out.push_str(":host"); rest = after; continue; } // :host-context: not modelled
                out.push_str(h); rest = after; }
            out.push_str(rest); out } } };
        match dom.tag(i) {
            "style" => {
                if let Some(t) = style_text.get(&i) {
                    let hc = host_css(t); let t: &str = &hc;
                    if media_attr_ok(dom.attr(i, "media"), &m) { css_bytes += t.len(); sheet.base = base.clone(); add_with_imports(&mut sheet, t, &base, b, &m, &mut order, 0, &mut css_bytes, &mut missing); }
                }
            }
            "link" => {
                let rel = dom.attr(i, "rel").unwrap_or("").to_ascii_lowercase();
                if rel.split_ascii_whitespace().any(|r| r == "stylesheet") && !rel.contains("alternate") && media_attr_ok(dom.attr(i, "media"), &m) {
                    if let Some(h) = dom.attr(i, "href") {
                        let u = resolve(&base, h);
                        match b.get(&u) {
                            Some(css) => { let t = String::from_utf8_lossy(&css).into_owned(); css_bytes += t.len(); add_with_imports(&mut sheet, &t, &u, b, &m, &mut order, 0, &mut css_bytes, &mut missing); }
                            None => missing += 1,
                        }
                    }
                }
            }
            _ => {}
        }
    }
    // DOCTOR fix sheet (doctor-core PROCEDURE §4 / P2): the site's page-code rewrites, last in cascade order (doctor trial or
    // OMR_FIXES=<dir> holding <slug(url)>.css); absent = the page exactly as published
    sheet.cur_scope = u32::MAX;
    #[cfg(feature = "t25")] if let Some(css) = crate::repair::doctor::fix::sheet_for(&b.url()) { sheet.base = base.clone(); sheet.add(&css, &m, &mut order); }
    (sheet, css_bytes, missing)
}

/// ::before / ::after: rebuild the DOM with the generated element nodes and remap style ids onto it (layout's input).
pub fn apply_pseudo(dom: Dom, styled: &mut Styled) -> Dom {
    if styled.pseudo.is_empty() { return dom; }
    let boxes: Vec<(u32, u8, String)> = styled.pseudo.iter().map(|(e, w, _, t)| (*e, *w, t.clone())).collect();
    let (nd, map, bix) = dom.with_pseudo(&boxes);
    let mut ids = vec![0u16; nd.nodes.len()];
    for (o, &nn) in map.iter().enumerate() { if nn != crate::dom::NONE { ids[nn as usize] = styled.ids[o]; } }
    for (k, &nn) in bix.iter().enumerate() { if nn != crate::dom::NONE { ids[nn as usize] = styled.pseudo[k].2; } }
    styled.ids = ids;
    nd
}

pub fn load(b: &dyn Source, viewport: (f32, f32)) -> Loaded {
    let t0 = std::time::Instant::now();
    let dom = Dom::parse(&b.html());
    let parse_ms = t0.elapsed().as_secs_f64() * 1e3;
    let t1 = std::time::Instant::now();
    let (sheet, css_bytes, missing) = build_sheet(&dom, b, viewport, true);
    let css_ms = t1.elapsed().as_secs_f64() * 1e3;
    let mut styled = style::compute(&dom, &sheet, viewport);
    let dom = apply_pseudo(dom, &mut styled);
    // fetch only the @font-face sources whose family some computed style names
    let used: std::collections::HashSet<String> = styled.families.iter().flat_map(|f| f.split(',').map(|x| x.trim().trim_matches('"').trim_matches('\'').to_ascii_lowercase()).collect::<Vec<_>>()).collect();
    let mut fonts = Vec::new();
    let mut seen = std::collections::HashSet::new();
    // one face per (family, weight, style): prefer the subset whose unicode-range covers basic Latin (patch.unicode_range)
    let latin_first = crate::layout::patch_on(crate::layout::PATCH_UNICODE_RANGE);
    let mut faces: Vec<&crate::css::FontFaceRule> = sheet.faces.iter().collect();
    if latin_first { faces.sort_by_key(|f| !f.latin); }
    for f in faces {
        let fam = f.family.to_ascii_lowercase();
        if !used.contains(&fam) || !seen.insert((fam.clone(), f.w_lo, f.italic)) { continue; }
        if let Some(bytes) = f.srcs.iter().find_map(|u| b.get(u)) { crate::font::note_size_adjust(&fam, f.w_lo, f.italic, f.size_adjust); fonts.push((fam, f.w_lo, f.w_hi, f.italic, bytes)); }
    }
    #[cfg(feature = "t25")] crate::repair::doctor::fix::arm_geo_for(&b.url(), &dom); // doctor route 2: the site's stored exact geometry (OMR_FIXES)
    Loaded { dom, sheet, styled, css_bytes, missing_css: missing, parse_ms, css_ms, fonts }
}

fn media_attr_ok(m: Option<&str>, media: &Media) -> bool {
    match m {
        None => true,
        Some(q) => {
            let mut s = Sheet::new();
            let mut o = 0u32;
            s.add(&format!("@media {q} {{ x {{ y: z }} }}"), media, &mut o);
            !s.rules.is_empty()
        }
    }
}

fn add_with_imports(sheet: &mut Sheet, css: &str, base: &str, b: &dyn Source, m: &Media, order: &mut u32, depth: u32, bytes: &mut usize, missing: &mut usize) {
    sheet.base = base.to_string();
    // @import may only precede all other rules (CSS Cascade §2.1): scan just the sheet's head, raw, no full parse
    if depth < 3 {
        let mut probe = Sheet::new();
        let mut o = 0u32;
        let head_end = import_head_end(css);
        probe.add(&css[..head_end], m, &mut o);
        if std::env::var("OMR_DEBUG_IMPORT").is_ok() { eprintln!("import-head {base}: {} bytes head, imports {:?}", head_end, probe.imports); }
        for imp in probe.imports.iter() {
            let u = resolve(base, imp);
            match b.get(&u) {
                Some(c) => { let t = String::from_utf8_lossy(&c).into_owned(); *bytes += t.len(); add_with_imports(sheet, &t, &u, b, m, order, depth + 1, bytes, missing); }
                None => *missing += 1,
            }
        }
    }
    let before = sheet.imports.len();
    sheet.base = base.to_string();
    sheet.add(css, m, order);
    sheet.imports.truncate(before);
}

/// Byte offset where the leading run of @charset/@import/@layer statements (and comments/whitespace) ends.
fn import_head_end(css: &str) -> usize {
    let b = css.as_bytes();
    let mut i = 0;
    loop {
        while i < b.len() && b[i].is_ascii_whitespace() { i += 1; }
        if css[i..].starts_with("/*") { i = css[i + 2..].find("*/").map(|k| i + 2 + k + 2).unwrap_or(b.len()); continue; }
        let lower = css[i..css.len().min(i + 8)].to_ascii_lowercase();
        if lower.starts_with("@import") || lower.starts_with("@charset") || (lower.starts_with("@layer") && css[i..].find(';').map_or(false, |s| css[i..].find('{').map_or(true, |br| s < br))) {
            let mut q = 0u8;
            while i < b.len() { let c = b[i]; if q != 0 { if c == q { q = 0; } } else if c == b'"' || c == b'\'' { q = c; } else if c == b';' { i += 1; break; } i += 1; }
            continue;
        }
        return i;
    }
}

/// Every URL an <img> may load: src, each srcset candidate, data-src (lazy loaders), <picture><source srcset>.
pub fn img_srcs(d: &crate::dom::Dom, n: u32) -> Vec<String> {
    let mut v = Vec::new();
    let mut push_set = |s: &str| for c in s.split(',') { if let Some(u) = c.trim().split_whitespace().next() { if !u.starts_with("data:") { v.push(u.to_string()); } } };
    if let Some(s) = d.attr(n, "srcset") { push_set(s); }
    let p = d.nodes[n as usize].parent;
    if p != crate::dom::NONE && d.tag(p) == "picture" { for c in d.children(p) { if d.tag(c) == "source" { if let Some(s) = d.attr(c, "srcset") { push_set(s); } } } }
    for a in ["src", "data-src", "data-lazy-src"] { if let Some(s) = d.attr(n, a) { if !s.is_empty() && !s.starts_with("data:") { v.insert(0, s.to_string()); } } }
    // data: images (RFC 2397) last: lazy loaders park a 1-px placeholder in src while the real URL waits in data-src
    if let Some(s) = d.attr(n, "src").map(str::trim).filter(|s| s.starts_with("data:")) { v.push(s.to_string()); }
    if let Some(s) = d.attr(n, "srcset") { for c in s.split(", ") { if let Some(u) = c.trim().split_whitespace().next() { if u.starts_with("data:") { v.push(u.to_string()); } } } }
    v
}

/// First candidate the source actually has (the corpus froze whichever size Chrome chose).
pub fn img_bytes(d: &crate::dom::Dom, n: u32, base: &str, b: &dyn Source) -> Option<Vec<u8>> {
    img_srcs(d, n).iter().find_map(|u| b.get(&resolve(base, u)))
}

#[cfg(test)]
mod tests {
    use super::resolve;
    #[test]
    fn resolves() {
        assert_eq!(resolve("https://a.com/x/y.html", "z.css"), "https://a.com/x/z.css");
        assert_eq!(resolve("https://a.com/x/y.html", "../z.css"), "https://a.com/z.css");
        assert_eq!(resolve("https://a.com/x/y.html", "/s/z.css?v=1"), "https://a.com/s/z.css?v=1");
        assert_eq!(resolve("https://a.com/x/", "//cdn.b.com/q.css"), "https://cdn.b.com/q.css");
        assert_eq!(resolve("https://a.com", "q.css"), "https://a.com/q.css");
    }
}
