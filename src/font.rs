// SPDX-License-Identifier: MIT OR Apache-2.0
//! Fonts — system faces mmapped (shared with every other process using them; zero private RAM),
//! outlines read by ttf-parser (safe Rust), rasterized by our own signed-area coverage accumulator,
//! stored as 2-bit coverage masks (4 levels: 0, ⅓, ⅔, 1) in a glyph cache keyed by (face, glyph, px).
use memmap2::Mmap;
use std::cell::RefCell;
use std::collections::HashMap;

/// Owned face bytes (a private mmap of a system file, or decoded web-font bytes). Tables are parsed on demand —
/// ttf-parser only reads the table directory — and every per-glyph result is cached, so this is off the hot path.
pub enum Src { Map(Mmap), Owned(Box<[u8]>) }
pub struct Face { src: Src, k: u32, pub upem: f32, adjust: u32, wght: f32 }
impl Face {
    /// units per em as used for sizing: size-adjust 95 % makes every glyph and metric 0.95× (CSS Fonts 5 §4.11)
    pub fn eff_upem(&self) -> f32 { if self.adjust != 1000 && crate::layout::patch_on(crate::layout::PATCH_SIZE_ADJUST) { self.upem * 1000.0 / self.adjust as f32 } else { self.upem } }
    fn bytes(&self) -> &[u8] { match &self.src { Src::Map(m) => &m[..], Src::Owned(b) => b } }
    fn with<R>(&self, f: impl FnOnce(&ttf_parser::Face) -> R) -> Option<R> {
        let mut x = ttf_parser::Face::parse(self.bytes(), self.k).ok()?;
        // variable fonts (OpenType fvar / HVAR): the weight instance, and the optical size Chrome picks for the used font size
        // (CSS Fonts 4 font-optical-sizing: auto → opsz = font-size in px)
        if x.is_variable() {
            if self.wght > 0.0 { x.set_variation(ttf_parser::Tag::from_bytes(b"wght"), self.wght); }
            let o = OPSZ.with(|o| o.get()); if o > 0.0 { x.set_variation(ttf_parser::Tag::from_bytes(b"opsz"), o); }
        }
        Some(f(&x))
    }
}

/// Font class chosen from the CSS family list.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Class { Serif, Sans, Mono }

const FILES: &[(Class, bool, bool, &[&str])] = &[
    (Class::Serif, false, false, &["/usr/share/fonts/truetype/liberation/LiberationSerif-Regular.ttf", "/usr/share/fonts/truetype/dejavu/DejaVuSerif.ttf"]),
    (Class::Serif, true, false, &["/usr/share/fonts/truetype/liberation/LiberationSerif-Bold.ttf", "/usr/share/fonts/truetype/dejavu/DejaVuSerif-Bold.ttf"]),
    (Class::Serif, false, true, &["/usr/share/fonts/truetype/liberation/LiberationSerif-Italic.ttf"]),
    (Class::Serif, true, true, &["/usr/share/fonts/truetype/liberation/LiberationSerif-BoldItalic.ttf"]),
    (Class::Sans, false, false, &["/usr/share/fonts/truetype/liberation/LiberationSans-Regular.ttf", "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf"]),
    (Class::Sans, true, false, &["/usr/share/fonts/truetype/liberation/LiberationSans-Bold.ttf", "/usr/share/fonts/truetype/dejavu/DejaVuSans-Bold.ttf"]),
    (Class::Sans, false, true, &["/usr/share/fonts/truetype/liberation/LiberationSans-Italic.ttf"]),
    (Class::Sans, true, true, &["/usr/share/fonts/truetype/liberation/LiberationSans-BoldItalic.ttf"]),
    (Class::Mono, false, false, &["/usr/share/fonts/truetype/dejavu/DejaVuSansMono.ttf", "/usr/share/fonts/truetype/liberation/LiberationMono-Regular.ttf"]),
    (Class::Mono, true, false, &["/usr/share/fonts/truetype/dejavu/DejaVuSansMono-Bold.ttf", "/usr/share/fonts/truetype/liberation/LiberationMono-Bold.ttf"]),
    (Class::Mono, false, true, &["/usr/share/fonts/truetype/dejavu/DejaVuSansMono-Oblique.ttf", "/usr/share/fonts/truetype/liberation/LiberationMono-Italic.ttf"]),
    (Class::Mono, true, true, &["/usr/share/fonts/truetype/dejavu/DejaVuSansMono-BoldOblique.ttf", "/usr/share/fonts/truetype/liberation/LiberationMono-BoldItalic.ttf"]),
];

/// Map a CSS font-family list to a class. Metric-compatible substitutes: Arial/Helvetica → Liberation Sans,
/// Times → Liberation Serif, Courier → Liberation Mono (same advances, so line breaks match).
pub fn classify(families: &str) -> Class {
    for f in families.split(',') {
        let f = f.trim().trim_matches('"').trim_matches('\'').to_ascii_lowercase();
        if f.is_empty() { continue; }
        if f.contains("mono") || f.contains("courier") || f.contains("consol") || f.contains("menlo") || f.contains("code") || f == "monaco" || f.contains("fixed") { return Class::Mono; }
        if f == "serif" || f.contains("times") || f.contains("georgia") || f.contains("garamond") || f.contains("cambria") || f.contains("palatino")
            || f.contains("baskerville") || f.contains("book") || f.contains("merriweather") || f.contains("charter") || (f.contains("serif") && !f.contains("sans")) { return Class::Serif; }
        if f == "sans-serif" || f == "system-ui" || f == "-apple-system" || f.contains("arial") || f.contains("helvetica") || f.contains("verdana")
            || f.contains("sans") || f.contains("segoe") || f.contains("roboto") || f.contains("inter") || f.contains("tahoma") || f.contains("lato")
            || f.contains("ubuntu") || f.contains("blinkmacsystemfont") || f.contains("trebuchet") || f.contains("lucida") || f.contains("open") { return Class::Sans; }
    }
    Class::Serif // CSS initial font is the UA default (serif)
}

#[derive(Clone)]
pub struct Mask { pub w: u16, pub h: u16, pub left: i16, pub top: i16, pub bits: Vec<u8>, pub depth8: bool } // 2 bits/px (or 8 for the A/B), row-major

impl Mask {
    /// Coverage 0..=255.
    #[inline] pub fn at(&self, x: usize, y: usize) -> u8 {
        let i = y * self.w as usize + x;
        if self.depth8 { self.bits[i] } else { [0, 85, 170, 255][((self.bits[i >> 2] >> ((i & 3) * 2)) & 3) as usize] }
    }
}

pub type FontId = u16;

struct WebFace { family: String, w_lo: u16, w_hi: u16, italic: bool, bytes: Option<Vec<u8>>, id: Option<FontId>, adjust: u32 }

/// Installed fonts: family (lowercase) → (path, face index, weight, italic). Built once, cached on disk.
pub struct SysIndex { pub fam: HashMap<String, Vec<(String, u32, u16, bool)>> }

impl SysIndex {
    pub fn load() -> SysIndex {
        let home = std::env::var("HOME").unwrap_or_default();
        let dirs = ["/usr/share/fonts".to_string(), "/usr/local/share/fonts".to_string(), format!("{home}/.local/share/fonts"), format!("{home}/.fonts")];
        let stamp: u64 = dirs.iter().filter_map(|d| std::fs::metadata(d).ok()).filter_map(|m| m.modified().ok())
            .map(|t| t.duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)).sum();
        let cache = format!("{home}/.cache/omni-render/fontindex-{stamp}.tsv");
        let mut fam: HashMap<String, Vec<(String, u32, u16, bool)>> = HashMap::new();
        if let Ok(t) = std::fs::read_to_string(&cache) {
            for l in t.lines() {
                let c: Vec<&str> = l.split('\t').collect();
                if c.len() == 5 { fam.entry(c[0].to_string()).or_default().push((c[1].to_string(), c[2].parse().unwrap_or(0), c[3].parse().unwrap_or(400), c[4] == "1")); }
            }
            return SysIndex { fam };
        }
        let mut stack: Vec<std::path::PathBuf> = dirs.iter().map(std::path::PathBuf::from).collect();
        let mut out = String::new();
        while let Some(d) = stack.pop() {
            let rd = match std::fs::read_dir(&d) { Ok(r) => r, Err(_) => continue };
            for e in rd.filter_map(|e| e.ok()) {
                let p = e.path();
                if p.is_dir() { stack.push(p); continue; }
                let ext = p.extension().and_then(|x| x.to_str()).unwrap_or("").to_ascii_lowercase();
                if !matches!(ext.as_str(), "ttf" | "otf" | "ttc") { continue; }
                let data = match std::fs::read(&p) { Ok(b) => b, Err(_) => continue };
                let n = ttf_parser::fonts_in_collection(&data).unwrap_or(1);
                for k in 0..n.min(16) {
                    let f = match ttf_parser::Face::parse(&data, k) { Ok(f) => f, Err(_) => continue };
                    let name = f.names().into_iter().filter(|nm| (nm.name_id == 16 || nm.name_id == 1) && nm.is_unicode())
                        .min_by_key(|nm| if nm.name_id == 16 { 0 } else { 1 }).and_then(|nm| nm.to_string());
                    let name = match name { Some(x) => x.to_ascii_lowercase(), None => continue };
                    let w = f.weight().to_number();
                    let it = f.is_italic() || f.is_oblique();
                    let ps = p.to_string_lossy().to_string();
                    out.push_str(&format!("{name}\t{ps}\t{k}\t{w}\t{}\n", it as u8));
                    fam.entry(name).or_default().push((ps, k, w, it));
                }
            }
        }
        let _ = std::fs::create_dir_all(format!("{home}/.cache/omni-render"));
        let _ = std::fs::write(&cache, out);
        SysIndex { fam }
    }
}

pub struct Fonts {
    faces: RefCell<Vec<std::rc::Rc<Face>>>,
    sys: RefCell<HashMap<(Class, bool, bool), Option<FontId>>>,
    web: RefCell<Vec<WebFace>>,
    resolved: RefCell<HashMap<(String, u16, bool), FontId>>,
    pub glyphs: RefCell<HashMap<(FontId, u16, u16), Mask>>, // (face, glyph, px*4) → mask
    adv: RefCell<HashMap<(FontId, char, u16), (FontId, u16, u16)>>, // (face, char, opsz quarter-px or 0) → (face used, glyph, advance in upem)
    inst: RefCell<HashMap<(FontId, u16), Option<FontId>>>, // variable face × font-weight → its weight instance
    pub web_bytes: std::cell::Cell<usize>,
    index: std::cell::OnceCell<SysIndex>,
    installed: RefCell<HashMap<(String, u32), Option<FontId>>>,
    kern: RefCell<HashMap<(FontId, u16, u16), i16>>,
    primary: RefCell<HashMap<(String, u16, bool), bool>>, // resolved from the FIRST listed family (not a fallback)? // (face, left glyph, right glyph) → pair adjustment in font units
}

impl Fonts {
    pub fn new() -> Fonts {
        Fonts { faces: RefCell::new(Vec::new()), sys: RefCell::new(HashMap::new()), web: RefCell::new(Vec::new()), resolved: RefCell::new(HashMap::new()),
                glyphs: RefCell::new(HashMap::new()), adv: RefCell::new(HashMap::new()), inst: RefCell::new(HashMap::new()), web_bytes: std::cell::Cell::new(0),
                index: std::cell::OnceCell::new(), installed: RefCell::new(HashMap::new()), kern: RefCell::new(HashMap::new()), primary: RefCell::new(HashMap::new()) }
    }

    /// An installed family by exact (case-insensitive) name, nearest weight/style (CSS Fonts §5.2, simplified).
    fn installed_face(&self, family: &str, weight: u16, italic: bool) -> Option<FontId> {
        let idx = self.index.get_or_init(SysIndex::load);
        let list = idx.fam.get(family)?;
        let (path, k, _, _) = list.iter().min_by_key(|(_, _, w, it)| ((*it != italic) as u32) * 10000 + (*w as i32 - weight as i32).unsigned_abs())?.clone();
        if let Some(x) = self.installed.borrow().get(&(path.clone(), k)) { return *x; }
        // SAFETY: font files are read-only system files; the map is owned by the Face and freed with Fonts.
        let id = std::fs::File::open(&path).ok().and_then(|file| unsafe { Mmap::map(&file).ok() }).and_then(|m| self.push(Src::Map(m), k));
        self.installed.borrow_mut().insert((path, k), id);
        id
    }

    fn push(&self, src: Src, k: u32) -> Option<FontId> { self.push_adj(src, k, 1000) }
    fn push_adj(&self, src: Src, k: u32, adjust: u32) -> Option<FontId> {
        let face = Face { src, k, upem: 0.0, adjust, wght: 0.0 };
        let upem = face.with(|f| f.units_per_em() as f32)?;
        let mut v = self.faces.borrow_mut();
        v.push(std::rc::Rc::new(Face { upem, ..face }));
        Some((v.len() - 1) as FontId)
    }

    /// System face for a class (mmapped file, shared page cache).
    pub fn sys_face(&self, c: Class, bold: bool, italic: bool) -> Option<FontId> {
        if let Some(x) = self.sys.borrow().get(&(c, bold, italic)) { return *x; }
        let mut id = None;
        if let Some((_, _, _, paths)) = FILES.iter().find(|(cc, b, i, _)| *cc == c && *b == bold && *i == italic) {
            for p in paths.iter() {
                if let Ok(file) = std::fs::File::open(p) {
                    // SAFETY: system font files are read-only; the map is owned by the Face.
                    if let Ok(m) = unsafe { Mmap::map(&file) } {
                        if let Some(x) = self.push(Src::Map(m), 0) { id = Some(x); break; }
                    }
                }
            }
        }
        self.sys.borrow_mut().insert((c, bold, italic), id);
        id
    }

    /// Register an @font-face source (sfnt/WOFF/WOFF2 bytes); decoded on first use.
    pub fn add_web(&self, family: &str, w_lo: u16, w_hi: u16, italic: bool, bytes: Vec<u8>) {
        let fam = family.trim().trim_matches('"').trim_matches('\'').to_ascii_lowercase();
        let adjust = SIZE_ADJUST.with(|m| m.borrow().get(&(fam.clone(), w_lo, italic)).copied()).unwrap_or(1000);
        self.web.borrow_mut().push(WebFace { family: fam, w_lo, w_hi, italic, bytes: Some(bytes), id: None, adjust });
    }

    pub fn has_web(&self, family: &str) -> bool { let f = family.to_ascii_lowercase(); self.web.borrow().iter().any(|w| w.family == f) }

    fn web_face(&self, family: &str, weight: u16, italic: bool) -> Option<FontId> {
        let mut web = self.web.borrow_mut();
        // CSS Fonts §5.2 (simplified): same family; style match preferred; nearest weight
        let best = web.iter().enumerate().filter(|(_, w)| w.family == family)
            .min_by_key(|(_, w)| ((w.italic != italic) as u32) * 10000 + if weight < w.w_lo { (w.w_lo - weight) as u32 } else if weight > w.w_hi { (weight - w.w_hi) as u32 } else { 0 })
            .map(|(k, _)| k)?;
        let w = &mut web[best];
        if w.id.is_none() {
            let raw = w.bytes.take()?;
            let sf = crate::woff::to_sfnt(&raw)?;
            self.web_bytes.set(self.web_bytes.get() + sf.len());
            let adj = w.adjust;
            drop(web);
            let id = self.push_adj(Src::Owned(sf.into_boxed_slice()), 0, adj)?;
            self.web.borrow_mut()[best].id = Some(id);
            return Some(id);
        }
        w.id
    }

    /// Resolve a CSS font-family list + weight/style to a face.
    pub fn resolve(&self, families: &str, weight: u16, italic: bool) -> FontId {
        let var_on = crate::layout::patch_on(crate::layout::PATCH_VAR_FONT);
        if var_on {
            let base = self.resolve_inner(families, weight, italic);
            return self.instance(base, weight).unwrap_or(base);
        }
        self.resolve_inner(families, weight, italic)
    }

    /// The weight instance of a variable face (fvar `wght`), shared bytes copied once per weight.
    fn instance(&self, id: FontId, weight: u16) -> Option<FontId> {
        if let Some(x) = self.inst.borrow().get(&(id, weight)) { return *x; }
        let f = self.face_of(id)?;
        let has_wght = f.with(|t| t.is_variable() && t.variation_axes().into_iter().any(|a| a.tag == ttf_parser::Tag::from_bytes(b"wght"))).unwrap_or(false);
        let out = if has_wght && f.wght == 0.0 {
            let bytes: Box<[u8]> = f.bytes().to_vec().into_boxed_slice();
            let upem = f.upem;
            let mut v = self.faces.borrow_mut();
            v.push(std::rc::Rc::new(Face { src: Src::Owned(bytes), k: f.k, upem, adjust: f.adjust, wght: weight as f32 }));
            Some((v.len() - 1) as FontId)
        } else { None };
        self.inst.borrow_mut().insert((id, weight), out);
        out
    }

    fn resolve_inner(&self, families: &str, weight: u16, italic: bool) -> FontId {
        let key = (families.to_string(), weight, italic);
        if let Some(&id) = self.resolved.borrow().get(&key) { return id; }
        let bold = weight >= 600;
        let mut id = None;
        let mut nth = 0usize;
        for f in families.split(',') {
            let f = f.trim().trim_matches('"').trim_matches('\'').to_ascii_lowercase();
            if f.is_empty() { continue; }
            nth += 1;
            if let Some(x) = self.web_face(&f, weight, italic) { id = Some(x); break; }
            // metric-compatible aliases (what fontconfig + Chrome accept on Linux), then installed families, then generics
            let alias = match f.as_str() {
                "arial" | "helvetica" | "arimo" => Some("liberation sans"), "times" | "times new roman" | "tinos" => Some("liberation serif"),
                "courier" | "courier new" | "cousine" => Some("liberation mono"),
                "serif" => Some("liberation serif"), "sans-serif" => Some("liberation sans"), "monospace" | "ui-monospace" => Some("dejavu sans mono"),
                "system-ui" | "ui-sans-serif" => Some("noto sans"), "ui-serif" => Some("noto serif"), _ => None,
            };
            if let Some(x) = self.installed_face(alias.unwrap_or(&f), weight, italic) { id = Some(x); break; }
            if matches!(f.as_str(), "serif" | "sans-serif" | "monospace" | "cursive" | "fantasy") { id = self.sys_face(classify(&f), bold, italic); if id.is_some() { break; } }
        }
        // no family available: the UA default font (standard font = Times New Roman → Liberation Serif), as Chrome on Linux
        self.primary.borrow_mut().insert((families.to_string(), weight, italic), if id.is_some() { nth == 1 } else { nth == 0 });
        let id = id.or_else(|| self.installed_face("liberation serif", weight, italic)).or_else(|| self.sys_face(Class::Serif, bold, italic)).or_else(|| self.sys_face(Class::Sans, false, false)).unwrap_or(0);
        self.resolved.borrow_mut().insert(key, id);
        id
    }

    fn face_of(&self, id: FontId) -> Option<std::rc::Rc<Face>> { self.faces.borrow().get(id as usize).cloned() }

    /// (face used, glyph id, advance in 1/64 px). Characters missing from a (subset) web font fall back to the system face.
    pub fn advance(&self, id: FontId, ch: char, px64: i32) -> (FontId, u16, i32) {
        let var = crate::layout::patch_on(crate::layout::PATCH_VAR_FONT) && self.face_of(id).map_or(false, |f| f.with(|t| t.is_variable()).unwrap_or(false));
        let oq: u16 = if var { (px64 / 16).clamp(1, u16::MAX as i32) as u16 } else { 0 };
        if var { OPSZ.with(|o| o.set(px64 as f32 / 64.0)); }
        let r = *self.adv.borrow_mut().entry((id, ch, oq)).or_insert_with(|| {
            let look = |fid: FontId| -> Option<(FontId, u16, u16)> {
                let f = self.face_of(fid)?;
                let up = f.upem;
                f.with(|t| { let g = t.glyph_index(ch)?.0; Some((fid, g, t.glyph_hor_advance(ttf_parser::GlyphId(g)).unwrap_or((up / 2.0) as u16))) })?
            };
            look(id).or_else(|| self.sys_face(Class::Sans, false, false).and_then(look)).or_else(|| self.sys_face(Class::Serif, false, false).and_then(|s| {
                // DejaVu has the widest coverage among the system faces
                let _ = s; None })).unwrap_or((id, 0, 1024))
        });
        if var { OPSZ.with(|o| o.set(0.0)); }
        let (fid, g, a) = r;
        let upem = self.face_of(fid).map(|f| f.eff_upem()).unwrap_or(2048.0);
        (fid, g, (a as f32 * px64 as f32 / upem).round() as i32)
    }

    /// Pair kerning of two glyphs of one face, in 1/64 px (OpenType GPOS `kern` PairPos, else the legacy `kern` table),
    /// read by omni-spec `otshape` (written from the OpenType spec). Collections and fonts without kerning give 0.
    pub fn kern(&self, id: FontId, l: u16, r: u16, px64: i32) -> i32 {
        let k = *self.kern.borrow_mut().entry((id, l, r)).or_insert_with(|| {
            let Some(f) = self.face_of(id) else { return 0 };
            if f.k != 0 { return 0; }
            // variable fonts: Chrome shapes the instance at the used size (opsz, `font-optical-sizing: auto`); we read the default
            // instance only, so its kerning would add error, not remove it — 0 until font variations land (fvar/gvar/HVAR)
            if f.with(|t| t.is_variable()).unwrap_or(false) { return 0; }
            omni_spec::otshape::parse(f.bytes()).and_then(|t| t.pair_advance(l, r)).unwrap_or(0)
        });
        if k == 0 { return 0; }
        let upem = self.face_of(id).map(|f| f.eff_upem()).unwrap_or(2048.0);
        (k as f32 * px64 as f32 / upem).round() as i32
    }

    /// Was this family list resolved from its first family (the face the page asked for), not a fallback? Shaping
    /// features such as kerning only move us toward Chrome when we hold the same face (a missing web font = a substitute).
    pub fn is_primary(&self, families: &str, weight: u16, italic: bool) -> bool {
        self.resolve(families, weight, italic);
        self.primary.borrow().get(&(families.to_string(), weight, italic)).copied().unwrap_or(false)
    }

    pub fn normal_line(&self, id: FontId, px64: i32) -> i32 {
        self.face_of(id).and_then(|f| { let s = px64 as f32 / 64.0 / f.eff_upem(); f.with(|t| {
            let r = |v: f32| (v * s).round() as i32;
            let (a, d, g) = vmetrics(t);
            (r(a) + r(-d) + r(g)) * 64 }) }).unwrap_or(px64 * 115 / 100)
    }

    pub fn metrics(&self, id: FontId, px64: i32) -> (i32, i32) {
        self.face_of(id).and_then(|f| { let s = px64 as f32 / f.eff_upem(); f.with(|t| { let (a, d, _) = vmetrics(t); ((a * s).round() as i32, (-d * s).round() as i32) }) }).unwrap_or((px64 * 4 / 5, px64 / 5))
    }

    /// Rasterize (cached). Size quantized to 1/4 px.
    pub fn glyph(&self, id: FontId, g: u16, px64: i32) -> Option<Mask> {
        let q = ((px64 + 8) / 16).clamp(1, u16::MAX as i32) as u16;
        let k = (id, g, q);
        if let Some(m) = self.glyphs.borrow().get(&k) { return Some(m.clone()); }
        let f = self.face_of(id)?;
        let scale = q as f32 / 4.0 / f.eff_upem();
        let mut pb = PathBuilder { pts: Vec::new(), cur: (0.0, 0.0), start: (0.0, 0.0), s: scale };
        let bb = f.with(|t| t.outline_glyph(ttf_parser::GlyphId(g), &mut pb)).flatten();
        let m = match bb {
            None => Mask { w: 0, h: 0, left: 0, top: 0, bits: Vec::new(), depth8: false },
            Some(b) => {
                let x0 = (b.x_min as f32 * scale).floor() as i32;
                let x1 = (b.x_max as f32 * scale).ceil() as i32 + 1;
                let y0 = (-(b.y_max as f32) * scale).floor() as i32;
                let y1 = (-(b.y_min as f32) * scale).ceil() as i32 + 1;
                let (w, h) = ((x1 - x0).clamp(1, 2048) as usize, (y1 - y0).clamp(1, 2048) as usize);
                let cov = raster_coverage(&pb.pts, x0 as f32, y0 as f32, w, h);
                let depth8 = std::env::var("OMR_GLYPH8").is_ok(); // A/B switch: measure what 2-bit costs
                let bits = if depth8 { cov.iter().map(|a| (a.min(1.0) * 255.0 + 0.5) as u8).collect() } else {
                    let mut bits = vec![0u8; (w * h + 3) / 4];
                    for (i, &a) in cov.iter().enumerate() { let lvl = ((a.min(1.0) * 3.0) + 0.5) as u8; bits[i >> 2] |= lvl.min(3) << ((i & 3) * 2); }
                    bits
                };
                Mask { w: w as u16, h: h as u16, left: x0 as i16, top: y0 as i16, bits, depth8 }
            }
        };
        self.glyphs.borrow_mut().insert(k, m.clone());
        Some(m)
    }

    pub fn cache_bytes(&self) -> usize { self.glyphs.borrow().values().map(|m| m.bits.len() + 16).sum() }
}

/// Vertical metrics as browsers pick them: OS/2 typo metrics when the font sets USE_TYPO_METRICS (fsSelection bit 7),
/// else hhea. (ascender, descender (negative), line gap) in font units.
fn vmetrics(t: &ttf_parser::Face) -> (f32, f32, f32) {
    if let Some(os2) = t.tables().os2 {
        if os2.use_typographic_metrics() {
            return (os2.typographic_ascender() as f32, os2.typographic_descender() as f32, os2.typographic_line_gap() as f32);
        }
    }
    (t.ascender() as f32, t.descender() as f32, t.line_gap() as f32)
}

/// Family names we map onto metric-compatible system faces instead of treating them as unknown web families.
const KNOWN_SYSTEM: &[&str] = &["arial", "helvetica", "times", "courier", "verdana", "georgia", "liberation", "dejavu", "noto", "tahoma", "trebuchet", "segoe", "roboto", "ubuntu", "consolas", "menlo", "monaco", "lucida"];

/// Collects the outline as line segments (curves flattened), in pixel space with y down.
struct PathBuilder { pts: Vec<(f32, f32, f32, f32)>, cur: (f32, f32), start: (f32, f32), s: f32 }

impl PathBuilder {
    fn p(&self, x: f32, y: f32) -> (f32, f32) { (x * self.s, -y * self.s) }
    fn line(&mut self, to: (f32, f32)) { if to != self.cur { self.pts.push((self.cur.0, self.cur.1, to.0, to.1)); } self.cur = to; }
}

impl ttf_parser::OutlineBuilder for PathBuilder {
    fn move_to(&mut self, x: f32, y: f32) { let p = self.p(x, y); self.cur = p; self.start = p; }
    fn line_to(&mut self, x: f32, y: f32) { let p = self.p(x, y); self.line(p); }
    fn quad_to(&mut self, x1: f32, y1: f32, x: f32, y: f32) {
        let (a, b, c) = (self.cur, self.p(x1, y1), self.p(x, y));
        let n = (((a.0 - 2.0 * b.0 + c.0).abs() + (a.1 - 2.0 * b.1 + c.1).abs()).sqrt() * 1.5).ceil().clamp(1.0, 16.0) as usize;
        for i in 1..=n { let t = i as f32 / n as f32; let u = 1.0 - t;
            self.line((u * u * a.0 + 2.0 * u * t * b.0 + t * t * c.0, u * u * a.1 + 2.0 * u * t * b.1 + t * t * c.1)); }
    }
    fn curve_to(&mut self, x1: f32, y1: f32, x2: f32, y2: f32, x: f32, y: f32) {
        let (a, b, c, d) = (self.cur, self.p(x1, y1), self.p(x2, y2), self.p(x, y));
        let n = 12;
        for i in 1..=n { let t = i as f32 / n as f32; let u = 1.0 - t;
            self.line((u * u * u * a.0 + 3.0 * u * u * t * b.0 + 3.0 * u * t * t * c.0 + t * t * t * d.0,
                       u * u * u * a.1 + 3.0 * u * u * t * b.1 + 3.0 * u * t * t * c.1 + t * t * t * d.1)); }
    }
    fn close(&mut self) { let s = self.start; self.line(s); }
}

/// Signed-area coverage accumulation (non-zero winding approximated by |area|, which is exact for
/// TrueType's non-overlapping contours). Each edge deposits, per crossed pixel row, its area to the
/// right; a running prefix sum per row yields coverage. Own implementation of the textbook method.
pub(crate) fn raster_coverage(segs: &[(f32, f32, f32, f32)], ox: f32, oy: f32, w: usize, h: usize) -> Vec<f32> {
    let st = w + 1; // one spill column per row so deposits never leak into the next row
    let mut acc = vec![0f32; st * h];
    for &(x0, y0, x1, y1) in segs {
        let (x0, y0, x1, y1) = (x0 - ox, y0 - oy, x1 - ox, y1 - oy);
        if (y0 - y1).abs() < 1e-6 { continue; }
        let (dir, xa, ya, xb, yb) = if y0 < y1 { (1.0f32, x0, y0, x1, y1) } else { (-1.0, x1, y1, x0, y0) };
        let dxdy = (xb - xa) / (yb - ya);
        let mut x = xa;
        let ys = ya.max(0.0);
        if ya < 0.0 { x += dxdy * (0.0 - ya); }
        let ye = yb.min(h as f32);
        let mut y = ys;
        while y < ye {
            let row = y as usize;
            let ny = ((row + 1) as f32).min(ye);
            let dy = ny - y;
            let xn = x + dxdy * dy;
            let d = dy * dir;
            let (xl, xr) = if x < xn { (x, xn) } else { (xn, x) };
            let base = row * st;
            let xl0 = xl.max(0.0).min(w as f32 - 1e-3);
            let xr0 = xr.max(0.0).min(w as f32 - 1e-3);
            let ix = xl0 as usize;
            if (xr0 as usize) == ix {
                let mid = (xl0 + xr0) * 0.5 - ix as f32;
                acc[base + ix] += d * (1.0 - mid);
                acc[base + ix + 1] += d * mid;
            } else {
                // spread across the span of pixels the edge crosses in this row (box approximation)
                let span = xr0 - xl0;
                let mut xx = xl0;
                while xx < xr0 {
                    let px = xx as usize;
                    let nx = ((px + 1) as f32).min(xr0);
                    let frac = (nx - xx) / span;
                    let mid = (xx + nx) * 0.5 - px as f32;
                    acc[base + px] += d * frac * (1.0 - mid);
                    acc[base + px + 1] += d * frac * mid;
                    xx = nx;
                }
            }
            x = xn;
            y = ny;
        }
    }
    let mut out = vec![0f32; w * h];
    for r in 0..h {
        let mut s = 0f32;
        for c in 0..w { s += acc[r * st + c]; out[r * w + c] = s.abs(); }
    }
    out
}

thread_local! { static SIZE_ADJUST: std::cell::RefCell<HashMap<(String, u16, bool), u32>> = std::cell::RefCell::new(HashMap::new()); }
/// The page loader records each @font-face's size-adjust (‰) before the faces are handed to `Fonts::add_web`.
pub fn note_size_adjust(family: &str, w_lo: u16, italic: bool, adjust: u32) { SIZE_ADJUST.with(|m| { m.borrow_mut().insert((family.to_ascii_lowercase(), w_lo, italic), adjust); }); }

thread_local! { static OPSZ: std::cell::Cell<f32> = const { std::cell::Cell::new(0.0) }; }
