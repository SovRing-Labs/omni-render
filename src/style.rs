// SPDX-License-Identifier: MIT OR Apache-2.0
//! Computed style — cascade + inheritance + custom properties, then every distinct computed style is
//! interned once: a node carries a u16 *style codeword*, the table is shared across the document (and
//! across tabs). Lengths are integer fixed point (1/64 px) so layout is exact integer arithmetic.
use crate::css::{self, Decl, ElemFacts, Media, Sheet, Tok};
use crate::dom::{Dom, Kind, NONE};
use std::collections::HashMap;
use std::rc::Rc;

pub const FX: i32 = 64; // fixed-point scale: 1 px = 64 units

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub enum Len {
    #[default]
    Auto,
    Px(i32),  // fixed-point px
    Pct(i32), // percent * 100
    None,
    MinContent,
    MaxContent,
    FitContent,
    /// calc() mixing length and percentage: px (fixed point) + percent*100 of the base
    Mix(i32, i32),
    /// min()/max()/clamp() comparing percentages with lengths: kept symbolic, evaluated at layout against the base
    Expr(u16),
}

thread_local! {
    /// symbolic math expressions of the current document (index = Len::Expr id)
    static EXPRS: std::cell::RefCell<Vec<Vec<Tok>>> = std::cell::RefCell::new(Vec::new());
    static MATH_BASE: std::cell::Cell<Option<f32>> = std::cell::Cell::new(None);
    static MATH_MIXED: std::cell::Cell<bool> = std::cell::Cell::new(false);
    /// a container-relative length (cqw …) was met while styling: the value stays symbolic until layout knows the container
    static MATH_CQ: std::cell::Cell<bool> = std::cell::Cell::new(false);
    /// query containers on the current layout path, innermost last: (content width, definite content height for `size`) px
    static CQ_STACK: std::cell::RefCell<Vec<(f32, Option<f32>)>> = const { std::cell::RefCell::new(Vec::new()) };
    /// INCR-R3: in an incremental session the expression table persists across documents and is deduplicated, so the
    /// same expression gets the same id in a full and an incremental compute seeded from the same table
    static EXPRS_DEDUPE: std::cell::Cell<bool> = std::cell::Cell::new(false);
}
/// The current document's symbolic math table, saved with a cached page and restored before that page is laid out again
/// (the table is per document; loading another page replaces it — the repair core's page cache found this, 2026-10-04).
pub struct ExprTable(Vec<Vec<Tok>>);
pub fn exprs_save() -> ExprTable { ExprTable(EXPRS.with(|e| e.borrow().clone())) }
pub fn exprs_restore(t: &ExprTable) { EXPRS.with(|e| *e.borrow_mut() = t.0.clone()); }

/// Layout enters / leaves a query container (an element with container-type ≠ normal): its content box is the base of
/// the cq units of everything laid out inside it (CSS Contain 3 §6).
pub fn cq_push(w: i32, h: Option<i32>) { CQ_STACK.with(|c| c.borrow_mut().push((w as f32 / FX as f32, h.map(|h| h as f32 / FX as f32)))); }
pub fn cq_pop() { CQ_STACK.with(|c| { c.borrow_mut().pop(); }); }
pub fn cq_unit(u: &str) -> bool { matches!(u, "cqw" | "cqi" | "cqh" | "cqb" | "cqmin" | "cqmax") }
/// One container-relative unit in px: the nearest query container's content box for its axis (cqh / cqb need a `size`
/// container), else the small viewport (CSS Contain 3 §6: no eligible container → the small viewport units).
fn cq_px(v: f32, u: &str) -> f32 {
    let (vw, vh) = (1280.0f32, 800.0f32);
    let (w, h) = CQ_STACK.with(|c| { let c = c.borrow();
        (c.last().map(|e| e.0).unwrap_or(vw), c.iter().rev().find_map(|e| e.1).unwrap_or(vh)) });
    v / 100.0 * match u { "cqw" | "cqi" => w, "cqh" | "cqb" => h, "cqmin" => w.min(h), _ => w.max(h) }
}

/// Evaluate a symbolic Len::Expr against its percentage base (fixed-point px in and out).
pub fn expr_eval(id: u16, base: i32) -> Option<i32> {
    let toks = EXPRS.with(|e| e.borrow().get(id as usize).cloned())?;
    MATH_BASE.with(|b| b.set(Some(base as f32 / FX as f32)));
    let empty = VarScope { parent: None, map: HashMap::new() };
    let cx = Ctx { parent_fs: 16 * FX, root_fs: 16 * FX, vars: &empty, current_color: 0 };
    let f = match toks.first() { Some(Tok::Func(f)) => f.clone(), _ => return None };
    let mut i = 0;
    let v = math_fn(&f, &toks[1..], &mut i, 16 * FX, &cx);
    MATH_BASE.with(|b| b.set(None));
    v.map(|v| ((v.px + v.pct * 0.0) * FX as f32).round() as i32)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
#[repr(u8)]
pub enum Display {
    None, Block, #[default] Inline, InlineBlock, ListItem, Table, InlineTable, TableRow, TableCell, TableRowGroup,
    TableHeaderGroup, TableFooterGroup, TableCaption, TableColumn, TableColumnGroup, Flex, InlineFlex, Grid, InlineGrid, Contents,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Style {
    pub display: Display,
    pub position: u8,   // 0 static 1 relative 2 absolute 3 fixed 4 sticky
    pub float: u8,      // 0 none 1 left 2 right
    pub clear: u8,
    pub width: Len, pub height: Len, pub min_w: Len, pub max_w: Len, pub min_h: Len, pub max_h: Len,
    pub inset: [Len; 4],   // top right bottom left
    pub margin: [Len; 4],
    pub padding: [Len; 4],
    pub border_w: [i32; 4],
    pub border_style: [u8; 4], // 0 none 1 solid 2 dashed 3 dotted 4 double 5 other
    pub border_color: [u32; 4],
    pub color: u32,      // 0xRRGGBBAA
    pub bg: u32,
    pub bg_img: u16,     // interned raw background-image value (url / gradient); 0 = none
    pub font_family: u16,
    pub font_size: i32,  // fixed-point px
    pub font_weight: u16,
    pub italic: bool,
    pub line_height: Len, // Auto = normal; Pct = number*100; Px = absolute
    pub text_align: u8,  // 0 start 1 left 2 right 3 center 4 justify
    pub decoration: u8,  // bit0 underline bit1 line-through bit2 overline
    pub white_space: u8, // 0 normal 1 pre 2 nowrap 3 pre-wrap 4 pre-line
    pub list_style: u8,  // 0 none 1 disc 2 circle 3 square 4 decimal 5 other
    pub visibility: u8,  // 0 visible 1 hidden
    pub overflow: u8,    // 0 visible 1 hidden 2 scroll/auto
    pub border_box: bool,
    pub vertical_align: u8,
    pub table_fixed: bool, // table-layout: fixed (CSS 2.2 §17.5.2.1)
    pub aspect: u32,       // aspect-ratio as width/height × 1000; 0 = auto (CSS Sizing 4 §5.1)
    pub aspect_auto: bool, // `auto <ratio>`: a replaced element's natural ratio wins when it has one
    pub border_spacing: i32,   // horizontal border-spacing in FX units, inherited, initial 0 (CSS 2.2 §17.6.1)
    pub border_collapse: bool, // border-collapse: collapse, inherited (CSS 2.2 §17.6.2)
    pub text_transform: u8, // 0 none 1 upper 2 lower 3 capitalize
    pub flex_dir: u8,    // 0 row 1 row-rev 2 column 3 column-rev
    pub flex_wrap: bool,
    pub justify: u8,     // 0 start 1 end 2 center 3 between 4 around 5 evenly
    pub align_items: u8, // 0 stretch 1 start 2 end 3 center 4 baseline
    pub align_content: u8, // 0 normal/stretch 1 start 2 end 3 center 4 space-between 5 space-around 6 space-evenly (Box Alignment 3 §5.1)
    pub justify_items: u8, // grid items' default justify-self: 0 normal/stretch 1 start 2 end 3 center (CSS Box Alignment 3 §6.1)
    pub grow: u16, pub shrink: u16, // *100
    pub order: i16,
    pub basis: Len,
    pub gap: [Len; 2],   // row, column
    pub z: i16,
    pub opacity: u8,
    pub text_indent: Len,
    // grid: interned raw values (0 = unset), parsed at layout time
    pub grid_cols: u16, pub grid_rows: u16, pub grid_areas: u16, pub grid_auto_rows: u16, pub grid_auto_cols: u16,
    pub grid_flow: u8,   // grid-auto-flow: 0 row 1 column (+2 dense)
    pub grid_col: u16, pub grid_row: u16, pub grid_area: u16,
    pub bg_size: u8,     // 0 auto 1 cover 2 contain
    pub bg_repeat: u8,   // 0 repeat 1 no-repeat 2 repeat-x 3 repeat-y
    pub bg_dim: [Len; 2], // explicit background-size lengths (Auto = natural)
    pub bg_center: bool,
    pub justify_self: u8, pub align_self: u8, // 0 auto/stretch 1 start 2 end 3 center
    pub translate: [Len; 2], // transform: translate(x, y) — % of the element's own border box
    pub was_inline: bool,
    pub line_clamp: u8,       // -webkit-line-clamp / line-clamp: N lines (CSS Overflow 4 §5), 0 = none
    pub webkit_box: bool,     // display: -webkit-box / -webkit-inline-box (legacy box; orient + line-clamp apply)
    pub box_vertical: bool,   // -webkit-box-orient: vertical
    pub container: u8,       // container-type (CSS Contain 3 §3.1): 0 normal, 1 inline-size, 2 size — a query container for cq units
    pub sb_gutter: bool,     // scrollbar-gutter: stable (CSS Overflow 4 §3.1) — the root reserves the classic scrollbar width
    pub svg_fill: Option<u32>,   // CSS fill / stroke / stroke-width on SVG content (SVG 2 §13: inherited), None = the SVG's own
    pub svg_stroke: Option<u32>,
    pub svg_sw: Option<i32>,     // stroke-width, fixed-point px (user units)
    pub radius: [Len; 4],    // border-radius per corner tl tr br bl (CSS Backgrounds 3 §5; circular — the `/` vertical radii are not kept)
    pub shadow: u16,         // interned raw box-shadow (0 = none), resolved at layout time by paint::parse_shadows
    pub backdrop: u16,       // interned raw backdrop-filter (0 = none), paint::parse_backdrop
    pub letter_spacing: i32, // fixed-point px added after every character (inherited)    // inline-level before blockification (abs static position uses the original display)
}

impl Default for Style {
    fn default() -> Style {
        Style {
            display: Display::Inline, position: 0, float: 0, clear: 0,
            width: Len::Auto, height: Len::Auto, min_w: Len::Auto, max_w: Len::None, min_h: Len::Auto, max_h: Len::None,
            inset: [Len::Auto; 4], margin: [Len::Px(0); 4], padding: [Len::Px(0); 4],
            border_w: [0; 4], border_style: [0; 4], border_color: [0x000000ff; 4],
            color: 0x000000ff, bg: 0, bg_img: 0, font_family: 0, font_size: 16 * FX, font_weight: 400, italic: false,
            line_height: Len::Auto, text_align: 0, decoration: 0, white_space: 0, list_style: 1, visibility: 0, overflow: 0,
            border_box: false, vertical_align: 0, table_fixed: false, aspect: 0, aspect_auto: false, border_spacing: 0, border_collapse: false, text_transform: 0, flex_dir: 0, flex_wrap: false, justify: 0, align_items: 0, justify_items: 0, align_content: 0,
            grow: 0, shrink: 100, order: 0, basis: Len::Auto, gap: [Len::Auto; 2], z: 0, opacity: 255, text_indent: Len::Px(0),
            grid_cols: 0, grid_rows: 0, grid_areas: 0, grid_auto_rows: 0, grid_auto_cols: 0, grid_flow: 0, grid_col: 0, grid_row: 0, grid_area: 0, bg_size: 0, bg_dim: [Len::Auto; 2], bg_repeat: 0, bg_center: false, translate: [Len::Px(0); 2], was_inline: false, line_clamp: 0, webkit_box: false, box_vertical: false, container: 0, sb_gutter: false, svg_fill: None, svg_stroke: None, svg_sw: None, radius: [Len::Px(0); 4], shadow: 0, backdrop: 0, letter_spacing: 0, justify_self: 0, align_self: 0,
        }
    }
}

impl Style {
    /// Start a child's style: inherited properties copied, the rest reset to initial values.
    fn inherit(p: &Style) -> Style {
        Style {
            color: p.color, font_family: p.font_family, font_size: p.font_size, font_weight: p.font_weight, italic: p.italic,
            line_height: p.line_height, text_align: p.text_align, white_space: p.white_space, list_style: p.list_style,
            visibility: p.visibility, text_transform: p.text_transform, text_indent: p.text_indent, letter_spacing: p.letter_spacing, svg_fill: p.svg_fill, svg_stroke: p.svg_stroke, svg_sw: p.svg_sw, border_spacing: p.border_spacing, border_collapse: p.border_collapse,
            border_color: [p.color; 4],
            ..Style::default()
        }
    }
}

/// Our own user-agent sheet, written from the HTML Living Standard §15 "Rendering" suggested defaults.
/// Quirks-mode additions (HTML §15.3.8 "Tables" quirks block; §15.3.3 form/body quirks).
pub const UA_QUIRKS: &str = r#"
table { font-weight: initial; font-style: initial; font-size: initial; line-height: initial; white-space: initial; text-align: initial }
input:not([type=image]), textarea { box-sizing: border-box }
img[align=left] { margin-right: 3px } img[align=right] { margin-left: 3px }
"#;

pub const UA: &str = r#"
html, address, blockquote, body, center, dialog, div, figure, figcaption, footer, form, header, hr, legend, listing, main,
p, plaintext, pre, search, xmp, details, summary, article, aside, h1, h2, h3, h4, h5, h6, hgroup, nav, section, dir, dd, dl,
dt, menu, ol, ul, fieldset, optgroup { display: block }
button, select { box-sizing: border-box }
head, link, meta, script, style, title, template, noembed, noframes, param, rp, area, base, basefont, datalist, [hidden],
dialog:not([open]), input[type=hidden] { display: none }
li { display: list-item }
table { display: table; border-collapse: separate; border-spacing: 2px; box-sizing: border-box; text-indent: initial }
caption { display: table-caption; text-align: center }
colgroup { display: table-column-group } col { display: table-column }
thead { display: table-header-group; vertical-align: middle } tbody { display: table-row-group; vertical-align: middle }
tfoot { display: table-footer-group; vertical-align: middle } tr { display: table-row; vertical-align: inherit }
td, th { display: table-cell; vertical-align: inherit; padding: 1px } th { font-weight: bold; text-align: -internal-center }
body { margin: 8px }
p, blockquote, figure, listing, plaintext, pre, xmp, dl, menu, ol, ul, dir { margin-top: 1em; margin-bottom: 1em }
blockquote, figure { margin-left: 40px; margin-right: 40px }
dd { margin-left: 40px }
ol, ul, menu, dir { padding-left: 40px }
ol { list-style-type: decimal } ul, menu, dir { list-style-type: disc }
ul ul, ol ul, ul ol ul { list-style-type: circle } ul ul ul, ol ul ul { list-style-type: square }
ul ul, ul ol, ol ol, ol ul { margin-top: 0; margin-bottom: 0 }
h1 { font-size: 2em; margin-top: 0.67em; margin-bottom: 0.67em; font-weight: bold }
h2 { font-size: 1.5em; margin-top: 0.83em; margin-bottom: 0.83em; font-weight: bold }
h3 { font-size: 1.17em; margin-top: 1em; margin-bottom: 1em; font-weight: bold }
h4 { margin-top: 1.33em; margin-bottom: 1.33em; font-weight: bold }
h5 { font-size: 0.83em; margin-top: 1.67em; margin-bottom: 1.67em; font-weight: bold }
h6 { font-size: 0.67em; margin-top: 2.33em; margin-bottom: 2.33em; font-weight: bold }
article h1, aside h1, nav h1, section h1 { font-size: 1.5em; margin-top: 0.83em; margin-bottom: 0.83em }
hr { color: gray; border-style: inset; border-width: 1px; margin: 0.5em auto }
fieldset { margin-left: 2px; margin-right: 2px; border: 2px groove ThreeDFace; padding: 0.35em 0.75em 0.625em }
pre, listing, plaintext, xmp, code, kbd, samp, tt { font-family: monospace }
pre, listing, plaintext, xmp { white-space: pre }
textarea { white-space: pre-wrap }
b, strong { font-weight: bolder } i, cite, em, var, dfn { font-style: italic }
big { font-size: larger } small { font-size: smaller }
sub { vertical-align: sub; font-size: smaller } sup { vertical-align: super; font-size: smaller }
u, ins { text-decoration: underline } s, strike, del { text-decoration: line-through }
a:link { color: #0000EE; text-decoration: underline }
mark { background-color: yellow; color: black }
center { text-align: -webkit-center }
nobr { white-space: nowrap }
input, button, select, textarea, meter, progress { display: inline-block }
button, input, select, textarea { font-size: 13.333px; border: 2px solid #767676; padding: 1px 2px; font-family: sans-serif }
textarea { font-family: monospace; padding: 2px; border-width: 1px }
button, input[type=submit], input[type=button], input[type=reset] { padding: 1px 6px; border: 2px outset #767676; background-color: #efefef }
input[type=checkbox], input[type=radio] { margin: 3px 3px 3px 4px; padding: 0; border: 0 }
summary { display: block }
"#;

#[derive(Default, Clone)]
pub struct Styled {
    pub ids: Vec<u16>,        // per node: style codeword (0 = no box: text inherits parent's)
    pub table: Vec<Style>,    // codeword -> computed style
    pub families: Vec<String>,
    pub rules: usize,
    pub stats: StyleStats,
    /// generated boxes: (element, 1 = ::before | 2 = ::after, style codeword, content text)
    pub pseudo: Vec<(u32, u8, u16, String)>,
}

#[derive(Default, Debug, Clone)]
pub struct StyleStats { pub matched: usize, pub decls: usize, pub vars: usize, pub ms: f64, pub match_ms: f64, pub shared: usize, pub fp_ms: f64, pub apply_ms: f64, pub intern_ms: f64, pub setup_ms: f64, pub vars_ms: f64, pub gather_ms: f64 }

/// Custom properties as a parent-chained scope: an element that declares variables adds one small map
/// pointing at its parent's scope (no copying of the inherited set — CSS-variable-heavy pages stay flat).
/// Values are borrowed from the stylesheet (no copies); only inline-style variables own their tokens.
pub enum VarVal<'a> { B(&'a [Tok]), O(Rc<Vec<Tok>>) }
impl VarVal<'_> { fn toks(&self) -> &[Tok] { match self { VarVal::B(t) => t, VarVal::O(r) => r } } }

pub struct VarScope<'a> { parent: Option<Rc<VarScope<'a>>>, map: HashMap<std::borrow::Cow<'a, str>, VarVal<'a>> }

impl<'a> VarScope<'a> {
    pub fn get(&self, k: &str) -> Option<&[Tok]> {
        let mut cur = Some(self);
        while let Some(s) = cur { if let Some(v) = s.map.get(k) { return Some(v.toks()); } cur = s.parent.as_deref(); }
        None
    }
}

struct Ctx<'a, 'v> {
    parent_fs: i32,
    root_fs: i32,
    vars: &'a VarScope<'v>,
    current_color: u32,
}

fn named_color(n: &str) -> Option<u32> {
    let rgb: u32 = match n.to_ascii_lowercase().as_str() {
        "black" => 0x000000, "white" => 0xffffff, "red" => 0xff0000, "green" => 0x008000, "blue" => 0x0000ff,
        "yellow" => 0xffff00, "gray" | "grey" => 0x808080, "silver" => 0xc0c0c0, "maroon" => 0x800000, "purple" => 0x800080,
        "fuchsia" | "magenta" => 0xff00ff, "lime" => 0x00ff00, "olive" => 0x808000, "navy" => 0x000080, "teal" => 0x008080,
        "aqua" | "cyan" => 0x00ffff, "orange" => 0xffa500, "darkgray" | "darkgrey" => 0xa9a9a9, "lightgray" | "lightgrey" => 0xd3d3d3,
        "dimgray" | "dimgrey" => 0x696969, "gainsboro" => 0xdcdcdc, "whitesmoke" => 0xf5f5f5, "darkblue" => 0x00008b,
        "darkred" => 0x8b0000, "darkgreen" => 0x006400, "lightblue" => 0xadd8e6, "steelblue" => 0x4682b4, "royalblue" => 0x4169e1,
        "slategray" | "slategrey" => 0x708090, "lightyellow" => 0xffffe0, "beige" => 0xf5f5dc, "brown" => 0xa52a2a,
        "pink" => 0xffc0cb, "gold" => 0xffd700, "indigo" => 0x4b0082, "violet" => 0xee82ee, "tomato" => 0xff6347,
        "coral" => 0xff7f50, "salmon" => 0xfa8072, "crimson" => 0xdc143c, "darkorange" => 0xff8c00, "khaki" => 0xf0e68c,
        "lavender" => 0xe6e6fa, "ivory" => 0xfffff0, "linen" => 0xfaf0e6, "snow" => 0xfffafa, "honeydew" => 0xf0fff0,
        "aliceblue" => 0xf0f8ff, "azure" => 0xf0ffff, "mintcream" => 0xf5fffa, "ghostwhite" => 0xf8f8ff, "seashell" => 0xfff5ee,
        "darkslategray" | "darkslategrey" => 0x2f4f4f, "lightslategray" => 0x778899, "midnightblue" => 0x191970,
        "dodgerblue" => 0x1e90ff, "deepskyblue" => 0x00bfff, "skyblue" => 0x87ceeb, "forestgreen" => 0x228b22,
        "seagreen" => 0x2e8b57, "limegreen" => 0x32cd32, "firebrick" => 0xb22222, "chocolate" => 0xd2691e, "tan" => 0xd2b48c,
        "wheat" => 0xf5deb3, "orchid" => 0xda70d6, "plum" => 0xdda0dd, "turquoise" => 0x40e0d0, "cornflowerblue" => 0x6495ed,
        "darkcyan" => 0x008b8b, "lightgreen" => 0x90ee90, "palegreen" => 0x98fb98, "lightcyan" => 0xe0ffff, "mistyrose" => 0xffe4e1,
        "oldlace" => 0xfdf5e6, "floralwhite" => 0xfffaf0, "papayawhip" => 0xffefd5, "blanchedalmond" => 0xffebcd,
        "cornsilk" => 0xfff8dc, "lemonchiffon" => 0xfffacd, "lightgoldenrodyellow" => 0xfafad2, "darkviolet" => 0x9400d3,
        "darkmagenta" => 0x8b008b, "slateblue" => 0x6a5acd, "mediumblue" => 0x0000cd, "cadetblue" => 0x5f9ea0,
        "canvas" | "field" | "buttonface" => 0xffffff, "canvastext" | "fieldtext" | "buttontext" => 0x000000,
        "linktext" => 0x0000ee, "visitedtext" => 0x551a8b, "graytext" => 0x6d6d6d, "threedface" => 0xc0c0c0,
        "highlight" => 0x3390ff, "highlighttext" => 0xffffff,
        "transparent" => return Some(0),
        _ => return None,
    };
    Some((rgb << 8) | 0xff)
}

fn hexc(h: &str) -> Option<u32> {
    let v = |s: &str| u32::from_str_radix(s, 16).ok();
    let d = |c: char| v(&c.to_string()).map(|x| x * 17);
    let c: Vec<char> = h.chars().collect();
    match c.len() {
        3 => Some((d(c[0])? << 24) | (d(c[1])? << 16) | (d(c[2])? << 8) | 0xff),
        4 => Some((d(c[0])? << 24) | (d(c[1])? << 16) | (d(c[2])? << 8) | d(c[3])?),
        6 => Some((v(h)? << 8) | 0xff),
        8 => v(h),
        _ => None,
    }
}

fn args(v: &[Tok]) -> Vec<f32> {
    // numeric args of rgb()/hsl(): numbers, percentages (keep as raw), in comma or space syntax
    v.iter().filter_map(|t| match t { Tok::Num(x) => Some(*x), Tok::Pct(x) => Some(*x), Tok::Dim(x, _) => Some(*x), _ => None }).collect()
}

fn hsl(h: f32, s: f32, l: f32) -> (f32, f32, f32) {
    let h = ((h % 360.0) + 360.0) % 360.0 / 360.0;
    let (s, l) = (s / 100.0, l / 100.0);
    let q = if l < 0.5 { l * (1.0 + s) } else { l + s - l * s };
    let p = 2.0 * l - q;
    let f = |t: f32| { let t = (t + 1.0) % 1.0; if t < 1.0 / 6.0 { p + (q - p) * 6.0 * t } else if t < 0.5 { q } else if t < 2.0 / 3.0 { p + (q - p) * (2.0 / 3.0 - t) * 6.0 } else { p } };
    (f(h + 1.0 / 3.0) * 255.0, f(h) * 255.0, f(h - 1.0 / 3.0) * 255.0)
}

pub fn parse_color(v: &[Tok], cc: u32) -> Option<u32> { color(v, cc) }

fn color(v: &[Tok], cc: u32) -> Option<u32> {
    match v.first()? {
        Tok::Hash(h) => hexc(h),
        Tok::Ident(n) if n.eq_ignore_ascii_case("currentcolor") => Some(cc),
        Tok::Ident(n) => named_color(n),
        Tok::Func(f) if f == "rgb" || f == "rgba" || f == "hsl" || f == "hsla" => {
            let inner = &v[1..];
            let a = args(inner);
            if a.len() < 3 { return None; }
            let pct: Vec<bool> = inner.iter().filter(|t| matches!(t, Tok::Num(_) | Tok::Pct(_) | Tok::Dim(..))).map(|t| matches!(t, Tok::Pct(_))).collect();
            let (r, g, b) = if f.starts_with("rgb") {
                let ch = |i: usize| if pct[i] { a[i] * 2.55 } else { a[i] };
                (ch(0), ch(1), ch(2))
            } else { hsl(a[0], a[1], a[2]) };
            let al = if a.len() > 3 { if pct[3] { a[3] / 100.0 } else { a[3] } } else { 1.0 };
            let c = |x: f32| x.round().clamp(0.0, 255.0) as u32;
            Some((c(r) << 24) | (c(g) << 16) | (c(b) << 8) | c(al * 255.0))
        }
        _ => None,
    }
}

/// Length resolution. em → own font size (or parent's for font-size itself), rem → root.
fn len(t: &[Tok], fs: i32, cx: &Ctx<'_, '_>) -> Option<Len> {
    match t.first()? {
        Tok::Num(v) if *v == 0.0 => Some(Len::Px(0)),
        Tok::Pct(v) => Some(Len::Pct((v * 100.0) as i32)),
        Tok::Ident(x) if x.eq_ignore_ascii_case("auto") => Some(Len::Auto),
        Tok::Ident(x) if x.eq_ignore_ascii_case("none") => Some(Len::None),
        Tok::Ident(x) if x.eq_ignore_ascii_case("min-content") => Some(Len::MinContent),
        Tok::Ident(x) if x.eq_ignore_ascii_case("max-content") => Some(Len::MaxContent),
        Tok::Ident(x) if x.eq_ignore_ascii_case("fit-content") || x.eq_ignore_ascii_case("-webkit-fit-content") || x.eq_ignore_ascii_case("-moz-fit-content") => Some(Len::FitContent),
        Tok::Dim(_, u) if cq_unit(u) && crate::layout::patch_on(crate::layout::PATCH_CQ_UNITS) => calc("calc", &[t[0].clone(), Tok::RParen], fs, cx),
        Tok::Dim(v, u) => {
            let px = match u.as_str() {
                "px" => *v, "em" => v * fs as f32 / FX as f32, "rem" => v * cx.root_fs as f32 / FX as f32,
                "pt" => v * 4.0 / 3.0, "pc" => v * 16.0, "in" => v * 96.0, "cm" => v * 96.0 / 2.54, "mm" => v * 96.0 / 25.4,
                "q" => v * 96.0 / 101.6,
                "vw" | "dvw" | "svw" | "lvw" => v * 12.8, "vh" | "dvh" | "svh" | "lvh" => v * 8.0,
                "vmin" => v * 8.0, "vmax" => v * 12.8, "ex" => v * fs as f32 / FX as f32 * 0.5, "ch" => v * fs as f32 / FX as f32 * 0.5,
                _ => return None,
            };
            Some(Len::Px((px * FX as f32).round() as i32))
        }
        Tok::Func(f) if f == "calc" || f == "min" || f == "max" || f == "clamp" => calc(f, &t[1..], fs, cx),
        _ => None,
    }
}

/// CSS Values 4 math: calc / min / max / clamp with + − × ÷ and parentheses, nested freely. A value is a linear
/// combination px + pct% (+ a bare number); the result is a Len (Px, Pct, or Mix when both parts are present).
#[derive(Clone, Copy, Default)]
struct MathV { px: f32, pct: f32, num: f32, is_num: bool }

fn calc(f: &str, t: &[Tok], fs: i32, cx: &Ctx<'_, '_>) -> Option<Len> {
    // `t` starts just after the function token; evaluate up to its matching ')'
    let mut i = 0;
    MATH_MIXED.with(|m| m.set(false));
    MATH_CQ.with(|m| m.set(false));
    let v = math_fn(f, t, &mut i, fs, cx)?;
    if (MATH_MIXED.with(|m| m.get()) || MATH_CQ.with(|m| m.get())) && MATH_BASE.with(|b| b.get()).is_none() {
        // a min/max/clamp compared % with lengths: keep it symbolic (lengths already resolved to px)
        let mut toks = vec![Tok::Func(f.to_string())];
        toks.extend(t[..i.min(t.len())].iter().map(|x| match x { Tok::Dim(_, u) if cq_unit(u) => x.clone(), Tok::Dim(..) => match len(std::slice::from_ref(x), fs, cx) { Some(Len::Px(p)) => Tok::Dim(p as f32 / FX as f32, "px".into()), _ => x.clone() }, _ => x.clone() }));
        let id = EXPRS.with(|e| { let mut e = e.borrow_mut();
            if EXPRS_DEDUPE.with(|x| x.get()) { if let Some(k) = e.iter().position(|x| *x == toks) { return k.min(u16::MAX as usize) as u16; } }
            e.push(toks); (e.len() - 1).min(u16::MAX as usize) as u16 });
        return Some(Len::Expr(id));
    }
    let px = (v.px * FX as f32).round() as i32;
    let pct = (v.pct * 100.0).round() as i32;
    Some(if v.is_num { Len::Px((v.num * FX as f32) as i32) } else if pct == 0 { Len::Px(px) } else if px == 0 { Len::Pct(pct) } else { Len::Mix(px, pct) })
}

/// Resolve a length expression (incl. calc/min/max/clamp) against a base width; for layout-time values
/// stored as text (grid tracks). Variables are already substituted at cascade time.
pub fn eval_len_px(t: &[Tok], fs: i32, base: i32) -> Option<i32> {
    let empty = VarScope { parent: None, map: HashMap::new() };
    let cx = Ctx { parent_fs: fs, root_fs: 16 * FX, vars: &empty, current_color: 0 };
    let t: Vec<Tok> = t.iter().filter(|x| **x != Tok::Ws).cloned().collect();
    match len(&t, fs, &cx)? {
        Len::Px(v) => Some(v),
        Len::Pct(p) => Some((base as i64 * p as i64 / 10000) as i32),
        Len::Mix(v, p) => Some(v + (base as i64 * p as i64 / 10000) as i32),
        // min()/max()/clamp() comparing % with lengths stays symbolic until the base is known — here it is (CSS Values 4 §10.2;
        // grid track `min(720px, 100% - 40px)`, codinghorror .gh-canvas: Chrome 720 px, ours fell back to auto = 8 px)
        Len::Expr(id) if crate::layout::patch_on(crate::layout::PATCH_TRACK_MATH) => expr_eval(id, base),
        _ => None,
    }
}

fn skip_ws(t: &[Tok], i: &mut usize) { while *i < t.len() && t[*i] == Tok::Ws { *i += 1; } }

/// Arguments of calc/min/max/clamp until the matching ')' (consumed).
fn math_fn(f: &str, t: &[Tok], i: &mut usize, fs: i32, cx: &Ctx<'_, '_>) -> Option<MathV> {
    let mut args = Vec::new();
    loop {
        args.push(math_sum(t, i, fs, cx)?);
        skip_ws(t, i);
        match t.get(*i) {
            Some(Tok::Comma) => { *i += 1; }
            Some(Tok::RParen) => { *i += 1; break; }
            None => break,
            _ => return None,
        }
    }
    if args.iter().any(|a| a.pct != 0.0) && args.iter().any(|a| a.px != 0.0) && f != "calc" { MATH_MIXED.with(|m| m.set(true)); }
    let cmp = |v: &MathV| v.px + v.pct * 0.0; // with a known base, percentages were already folded into px
    Some(match f {
        "min" => *args.iter().min_by(|a, b| cmp(a).partial_cmp(&cmp(b)).unwrap_or(std::cmp::Ordering::Equal))?,
        "max" => *args.iter().max_by(|a, b| cmp(a).partial_cmp(&cmp(b)).unwrap_or(std::cmp::Ordering::Equal))?,
        "clamp" if args.len() == 3 => { let (lo, v, hi) = (args[0], args[1], args[2]); if cmp(&v) < cmp(&lo) { lo } else if cmp(&v) > cmp(&hi) { hi } else { v } }
        _ => *args.first()?,
    })
}

fn math_sum(t: &[Tok], i: &mut usize, fs: i32, cx: &Ctx<'_, '_>) -> Option<MathV> {
    let mut acc = math_prod(t, i, fs, cx)?;
    loop {
        skip_ws(t, i);
        let sign = match t.get(*i) { Some(Tok::Delim('+')) => 1.0, Some(Tok::Delim('-')) => -1.0,
            // "-" glued to the next number by the tokenizer: `a -1px` is a subtraction in calc only with spaces; accept it
            Some(Tok::Dim(v, _)) | Some(Tok::Num(v)) | Some(Tok::Pct(v)) if *v < 0.0 => 0.0, _ => break };
        if sign != 0.0 { *i += 1; }
        let r = math_prod(t, i, fs, cx)?;
        let s = if sign == 0.0 { 1.0 } else { sign };
        acc = MathV { px: acc.px + s * r.px, pct: acc.pct + s * r.pct, num: acc.num + s * r.num, is_num: acc.is_num && r.is_num };
    }
    Some(acc)
}

fn math_prod(t: &[Tok], i: &mut usize, fs: i32, cx: &Ctx<'_, '_>) -> Option<MathV> {
    let mut acc = math_unit(t, i, fs, cx)?;
    loop {
        skip_ws(t, i);
        let op = match t.get(*i) { Some(Tok::Delim('*')) => '*', Some(Tok::Delim('/')) => '/', _ => break };
        *i += 1;
        let r = math_unit(t, i, fs, cx)?;
        acc = match op {
            '*' if r.is_num => MathV { px: acc.px * r.num, pct: acc.pct * r.num, num: acc.num * r.num, is_num: acc.is_num },
            '*' if acc.is_num => MathV { px: r.px * acc.num, pct: r.pct * acc.num, num: 0.0, is_num: false },
            '/' if r.is_num && r.num != 0.0 => MathV { px: acc.px / r.num, pct: acc.pct / r.num, num: acc.num / r.num, is_num: acc.is_num },
            _ => return None,
        };
    }
    Some(acc)
}

fn math_unit(t: &[Tok], i: &mut usize, fs: i32, cx: &Ctx<'_, '_>) -> Option<MathV> {
    skip_ws(t, i);
    let tok = t.get(*i)?.clone();
    *i += 1;
    match tok {
        Tok::Num(v) => Some(MathV { num: v, is_num: true, ..Default::default() }),
        Tok::Pct(v) => Some(match MATH_BASE.with(|b| b.get()) { Some(base) => MathV { px: base * v / 100.0, ..Default::default() }, None => MathV { pct: v, ..Default::default() } }),
        // container units: resolved only at layout (MATH_BASE set by expr_eval); while styling they mark the value symbolic
        Tok::Dim(v, ref u) if cq_unit(u) && crate::layout::patch_on(crate::layout::PATCH_CQ_UNITS) =>
            Some(if MATH_BASE.with(|b| b.get()).is_some() { MathV { px: cq_px(v, u), ..Default::default() } } else { MATH_CQ.with(|m| m.set(true)); MathV::default() }),
        Tok::Dim(..) => match len(std::slice::from_ref(&tok), fs, cx)? { Len::Px(p) => Some(MathV { px: p as f32 / FX as f32, ..Default::default() }), _ => None },
        Tok::LParen => { let v = math_sum(t, i, fs, cx)?; skip_ws(t, i); if t.get(*i) == Some(&Tok::RParen) { *i += 1; } Some(v) }
        Tok::Func(f) if matches!(f.as_str(), "calc" | "min" | "max" | "clamp" | "-webkit-calc") => math_fn(f.trim_start_matches("-webkit-"), t, i, fs, cx),
        _ => None,
    }
}

/// Replace var(--x[, fallback]) recursively.
fn subst(v: &[Tok], vars: &VarScope<'_>, depth: u32) -> Option<Vec<Tok>> {
    let _ = depth;
    subst_in(v, vars, &mut Vec::new())
}

/// `active` = custom properties being expanded; meeting one again is a cycle, which makes it
/// guaranteed-invalid, so the referencing var() takes its fallback (CSS Variables 1 §2.3)
fn subst_in(v: &[Tok], vars: &VarScope<'_>, active: &mut Vec<String>) -> Option<Vec<Tok>> {
    if active.len() > 16 { return None; }
    if !v.iter().any(|t| matches!(t, Tok::Func(f) if f == "var")) { return Some(v.to_vec()); }
    let mut out = Vec::with_capacity(v.len());
    let mut i = 0;
    while i < v.len() {
        if let Tok::Func(f) = &v[i] {
            if f == "var" {
                let mut d = 1;
                let mut j = i + 1;
                while j < v.len() { match v[j] { Tok::Func(_) | Tok::LParen => d += 1, Tok::RParen => { d -= 1; if d == 0 { break; } } _ => {} } j += 1; }
                let inner = &v[i + 1..j.min(v.len())];
                let name = inner.iter().find_map(|t| if let Tok::Ident(n) = t { Some(n.clone()) } else { None })?;
                let fb = inner.iter().position(|t| *t == Tok::Comma).map(|k| &inner[k + 1..]);
                // `initial` is the guaranteed-invalid value: treat as undefined (space-toggle idiom)
                let valid = vars.get(&name).filter(|v| !(v.len() == 1 && matches!(&v[0], Tok::Ident(x) if x.eq_ignore_ascii_case("initial"))));
                let expanded = match valid {
                    Some(val) if !active.contains(&name) => {
                        active.push(name.clone());
                        let r = subst_in(val, vars, active);
                        active.pop();
                        r
                    }
                    _ => None,
                };
                match expanded {
                    Some(e) => out.extend(e),
                    None => out.extend(subst_in(fb?, vars, active)?),
                }
                i = j + 1;
                continue;
            }
        }
        out.push(v[i].clone());
        i += 1;
    }
    Some(out)
}

/// Serialize tokens back to CSS text (for values parsed later at layout time).
pub fn tok_str(v: &[Tok]) -> String {
    let mut o = String::new();
    for t in v {
        match t {
            Tok::Ident(x) => o.push_str(x), Tok::Func(f) => { o.push_str(f); o.push('('); } Tok::At(x) => { o.push('@'); o.push_str(x); }
            Tok::Hash(x) => { o.push('#'); o.push_str(x); } Tok::Str(x) => { o.push('"'); o.push_str(x); o.push('"'); }
            Tok::Url(x) => { o.push_str("url("); o.push_str(x); o.push(')'); }
            Tok::Num(n) => o.push_str(&format!("{n}")), Tok::Pct(n) => o.push_str(&format!("{n}%")), Tok::Dim(n, u) => o.push_str(&format!("{n}{u}")),
            Tok::Delim(c) => o.push(*c), Tok::Ws => o.push(' '), Tok::Colon => o.push(':'), Tok::Semi => o.push(';'), Tok::Comma => o.push(','),
            Tok::LBrack => o.push('['), Tok::RBrack => o.push(']'), Tok::LParen => o.push('('), Tok::RParen => o.push(')'),
            Tok::LBrace => o.push('{'), Tok::RBrace => o.push('}'),
        }
    }
    o
}

fn intern(fam: &mut Vec<String>, v: String) -> u16 {
    match fam.iter().position(|f| *f == v) { Some(i) => i as u16, None => { fam.push(v); (fam.len() - 1).min(u16::MAX as usize) as u16 } }
}

fn idents(v: &[Tok]) -> Vec<String> {
    v.iter().filter_map(|t| if let Tok::Ident(x) = t { Some(x.to_ascii_lowercase()) } else { None }).collect()
}

fn parts(v: &[Tok]) -> Vec<Vec<Tok>> {
    // split a value on top-level whitespace (functions kept whole)
    let mut out: Vec<Vec<Tok>> = Vec::new();
    let mut cur = Vec::new();
    let mut d = 0;
    for t in v {
        match t {
            Tok::Ws if d == 0 => { if !cur.is_empty() { out.push(std::mem::take(&mut cur)); } }
            Tok::Func(_) | Tok::LParen => { d += 1; cur.push(t.clone()); }
            Tok::RParen => { d -= 1; cur.push(t.clone()); }
            Tok::Comma if d == 0 => { if !cur.is_empty() { out.push(std::mem::take(&mut cur)); } }
            _ => cur.push(t.clone()),
        }
    }
    if !cur.is_empty() { out.push(cur); }
    out
}

fn four<T: Copy>(v: &[T]) -> Option<[T; 4]> {
    Some(match v.len() { 1 => [v[0]; 4], 2 => [v[0], v[1], v[0], v[1]], 3 => [v[0], v[1], v[2], v[1]], 4 => [v[0], v[1], v[2], v[3]], _ => return None })
}

fn bstyle(s: &str) -> Option<u8> {
    Some(match s { "none" | "hidden" => 0, "solid" => 1, "dashed" => 2, "dotted" => 3, "double" => 4, "groove" | "ridge" | "inset" | "outset" => 5, _ => return None })
}

fn bwidth(t: &[Tok], fs: i32, cx: &Ctx<'_, '_>) -> Option<i32> {
    match t.first()? {
        Tok::Ident(x) => Some(match x.to_ascii_lowercase().as_str() { "thin" => FX, "medium" => 3 * FX, "thick" => 5 * FX, _ => return None }),
        _ => match len(t, fs, cx)? { Len::Px(v) => Some(v), _ => None },
    }
}

fn apply(s: &mut Style, d: &Decl, cx: &Ctx<'_, '_>, fam: &mut Vec<String>, parent: &Style) {
    // no allocation on the common path: values without var() are borrowed; keyword/part lists are built on demand
    let has_var = d.value.iter().any(|t| matches!(t, Tok::Func(f) if f == "var"));
    let owned;
    let v: &[Tok] = if has_var { owned = match subst(&d.value, cx.vars, 0) { Some(v) => v, None => return }; &owned } else { &d.value };
    let v: &[Tok] = { let k = v.iter().take_while(|t| **t == Tok::Ws).count(); &v[k..] };
    if v.is_empty() { return; }
    let id_cell: std::cell::OnceCell<Vec<String>> = std::cell::OnceCell::new();
    let idv = || id_cell.get_or_init(|| idents(v));
    let kw_low;
    let kw: &str = match v.iter().find_map(|t| if let Tok::Ident(x) = t { Some(x.as_str()) } else { None }) {
        Some(x) if x.bytes().any(|c| c.is_ascii_uppercase()) => { kw_low = x.to_ascii_lowercase(); &kw_low }
        Some(x) => x,
        None => "",
    };
    let fs = s.font_size;
    if kw == "inherit" || (kw == "unset" && !is_inherited(&d.name)) || kw == "initial" || kw == "revert" || kw == "unset" {
        // CSS Cascade 4 §7.3: `unset` = inherit for an inherited property, initial otherwise (it took the parent's value for every
        // property — theguardian `flex-basis: unset` kept the earlier 50 % where Chrome uses auto → the 122.5 px width)
        let src: Style = if kw == "inherit" || (kw == "unset" && is_inherited(&d.name)) { parent.clone() } else { Style::default() };
        match d.name.as_str() {
            "color" => s.color = src.color, "display" => s.display = src.display, "width" => s.width = src.width, "height" => s.height = src.height,
            "font-size" => s.font_size = src.font_size, "background-color" | "background" => s.bg = src.bg,
            "vertical-align" => s.vertical_align = src.vertical_align, "text-align" => s.text_align = src.text_align,
            "box-sizing" => s.border_box = src.border_box, "table-layout" => s.table_fixed = src.table_fixed, "position" => s.position = src.position, "float" => s.float = src.float,
            "font-weight" => s.font_weight = src.font_weight, "font-style" => s.italic = src.italic, "font-family" => s.font_family = src.font_family,
            "line-height" => s.line_height = src.line_height, "white-space" => s.white_space = src.white_space,
            "text-decoration" | "text-decoration-line" => s.decoration = src.decoration, "margin" => s.margin = src.margin,
            "padding" => s.padding = src.padding, "list-style" | "list-style-type" => s.list_style = src.list_style,
            "visibility" => s.visibility = src.visibility, "text-transform" => s.text_transform = src.text_transform,
            "border" => { s.border_w = src.border_w; s.border_style = src.border_style; }
            "margin-top" => s.margin[0] = src.margin[0], "margin-right" => s.margin[1] = src.margin[1], "margin-bottom" => s.margin[2] = src.margin[2], "margin-left" => s.margin[3] = src.margin[3],
            "padding-top" => s.padding[0] = src.padding[0], "padding-right" => s.padding[1] = src.padding[1], "padding-bottom" => s.padding[2] = src.padding[2], "padding-left" => s.padding[3] = src.padding[3],
            "overflow" => s.overflow = src.overflow, "opacity" => s.opacity = src.opacity,
            "font" => { s.font_size = src.font_size; s.font_family = src.font_family; s.font_weight = src.font_weight; s.italic = src.italic; s.line_height = src.line_height; }
            "letter-spacing" => s.letter_spacing = src.letter_spacing,
            "border-radius" => s.radius = src.radius, "box-shadow" => s.shadow = src.shadow,
            "fill" => s.svg_fill = src.svg_fill, "stroke" => s.svg_stroke = src.svg_stroke, "stroke-width" => s.svg_sw = src.svg_sw,
            "backdrop-filter" | "-webkit-backdrop-filter" => s.backdrop = src.backdrop,
            "border-top-left-radius" => s.radius[0] = src.radius[0], "border-top-right-radius" => s.radius[1] = src.radius[1],
            "border-bottom-right-radius" => s.radius[2] = src.radius[2], "border-bottom-left-radius" => s.radius[3] = src.radius[3],
            "border-color" => s.border_color = src.border_color, "border-width" => s.border_w = src.border_w, "border-style" => s.border_style = src.border_style,
            "min-width" => s.min_w = src.min_w, "max-width" => s.max_w = src.max_w, "min-height" => s.min_h = src.min_h, "max-height" => s.max_h = src.max_h,
            "flex" => { s.grow = src.grow; s.shrink = src.shrink; s.basis = src.basis; }
            "flex-basis" => s.basis = src.basis, "flex-grow" => s.grow = src.grow, "flex-shrink" => s.shrink = src.shrink,
            "flex-direction" => s.flex_dir = src.flex_dir, "flex-wrap" => s.flex_wrap = src.flex_wrap, "order" => s.order = src.order,
            "align-self" => s.align_self = src.align_self, "justify-self" => s.justify_self = src.justify_self,
            "align-items" => s.align_items = src.align_items, "justify-content" => s.justify = src.justify,
            "top" => s.inset[0] = src.inset[0], "right" => s.inset[1] = src.inset[1], "bottom" => s.inset[2] = src.inset[2], "left" => s.inset[3] = src.inset[3],
            "gap" => s.gap = src.gap, "z-index" => s.z = src.z, "text-indent" => s.text_indent = src.text_indent,
            _ => {}
        }
        return;
    }
    let l = || len(v, fs, cx);
    let p_cell: std::cell::OnceCell<Vec<Vec<Tok>>> = std::cell::OnceCell::new();
    let p = || p_cell.get_or_init(|| parts(v));
    macro_rules! side_len { ($f:ident, $i:expr) => { if let Some(x) = l() { s.$f[$i] = x; } }; }
    match d.name.as_str() {
        "display" => {
            let d2 = match (kw, idv().get(1).map(|x| x.as_str())) {
                ("none", _) => Display::None, ("block", _) | ("flow-root", _) | ("flow", _) => Display::Block,
                ("inline", Some("flex")) => Display::InlineFlex, ("inline", Some("grid")) => Display::InlineGrid,
                ("inline", Some("block")) | ("inline", Some("flow-root")) => Display::InlineBlock, ("inline", _) => Display::Inline,
                ("inline-block", _) => Display::InlineBlock, ("list-item", _) => Display::ListItem,
                ("table", _) => Display::Table, ("inline-table", _) => Display::InlineTable, ("table-row", _) => Display::TableRow,
                ("table-cell", _) => Display::TableCell, ("table-row-group", _) => Display::TableRowGroup,
                ("table-header-group", _) => Display::TableHeaderGroup, ("table-footer-group", _) => Display::TableFooterGroup,
                ("table-caption", _) => Display::TableCaption, ("table-column", _) => Display::TableColumn,
                ("table-column-group", _) => Display::TableColumnGroup, ("flex", _) | ("-webkit-box", _) | ("-webkit-flex", _) | ("-ms-flexbox", _) => Display::Flex,
                ("inline-flex", _) | ("-webkit-inline-flex", _) | ("-webkit-inline-box", _) => Display::InlineFlex, ("grid", _) => Display::Grid,
                ("inline-grid", _) => Display::InlineGrid, ("contents", _) => Display::Contents,
                _ => return,
            };
            s.display = d2;
            s.webkit_box = matches!(kw, "-webkit-box" | "-webkit-inline-box");
        }
        "position" => s.position = match kw { "static" => 0, "relative" => 1, "absolute" => 2, "fixed" => 3, "sticky" | "-webkit-sticky" => 4, _ => return },
        "float" => s.float = match kw { "none" => 0, "left" | "inline-start" => 1, "right" | "inline-end" => 2, _ => return },
        "clear" => s.clear = match kw { "none" => 0, "left" => 1, "right" => 2, "both" => 3, _ => return },
        "width" => if let Some(x) = l() { s.width = x },
        "height" => if let Some(x) = l() { s.height = x },
        "min-width" => if let Some(x) = l() { s.min_w = x },
        "max-width" => if let Some(x) = l() { s.max_w = x },
        "min-height" => if let Some(x) = l() { s.min_h = x },
        "max-height" => if let Some(x) = l() { s.max_h = x },
        "top" => side_len!(inset, 0), "right" => side_len!(inset, 1), "bottom" => side_len!(inset, 2), "left" => side_len!(inset, 3),
        "inset" => { let ls: Vec<Len> = p().iter().filter_map(|x| len(x, fs, cx)).collect(); if let Some(f) = four(&ls) { s.inset = f } }
        // CSS Logical Properties L1 §4.3 (clean-room, from the spec): flow-relative insets; horizontal-tb + ltr is the only
        // writing mode this engine lays out, so block-start = top, inline-end = right, block-end = bottom, inline-start = left.
        // Found by the DOCTOR missing-feature station (14 broken boxes, 0 agreeing, 2 pages).
        "inset-block-start" => side_len!(inset, 0), "inset-inline-end" => side_len!(inset, 1),
        "inset-block-end" => side_len!(inset, 2), "inset-inline-start" => side_len!(inset, 3),
        "inset-block" | "inset-inline" => {
            let ls: Vec<Len> = p().iter().filter_map(|x| len(x, fs, cx)).collect();
            if let Some(&a) = ls.first() { let b = ls.get(1).copied().unwrap_or(a); let (i, j) = if d.name == "inset-block" { (0, 2) } else { (3, 1) }; s.inset[i] = a; s.inset[j] = b; }
        }
        "margin" | "padding" => {
            let ls: Vec<Len> = p().iter().filter_map(|x| len(x, fs, cx)).collect();
            if ls.len() == p().len() { if let Some(f) = four(&ls) { if d.name == "margin" { s.margin = f } else { s.padding = f } } }
        }
        "margin-top" | "margin-block-start" => side_len!(margin, 0), "margin-right" | "margin-inline-end" => side_len!(margin, 1),
        "margin-bottom" | "margin-block-end" => side_len!(margin, 2), "margin-left" | "margin-inline-start" => side_len!(margin, 3),
        "padding-top" | "padding-block-start" => side_len!(padding, 0), "padding-right" | "padding-inline-end" => side_len!(padding, 1),
        "padding-bottom" | "padding-block-end" => side_len!(padding, 2), "padding-left" | "padding-inline-start" => side_len!(padding, 3),
        "margin-inline" | "margin-block" | "padding-inline" | "padding-block" => {
            let ls: Vec<Len> = p().iter().filter_map(|x| len(x, fs, cx)).collect();
            if ls.is_empty() { return; }
            let (a, b) = (ls[0], *ls.get(1).unwrap_or(&ls[0]));
            match d.name.as_str() {
                "margin-inline" => { s.margin[3] = a; s.margin[1] = b } "margin-block" => { s.margin[0] = a; s.margin[2] = b }
                "padding-inline" => { s.padding[3] = a; s.padding[1] = b } _ => { s.padding[0] = a; s.padding[2] = b }
            }
        }
        "border" | "border-top" | "border-right" | "border-bottom" | "border-left" => {
            let sides: &[usize] = match d.name.as_str() { "border-top" => &[0], "border-right" => &[1], "border-bottom" => &[2], "border-left" => &[3], _ => &[0, 1, 2, 3] };
            let (mut w, mut st, mut c) = (None, None, None);
            for x in p() {
                if let Some(b) = x.first().and_then(|t| if let Tok::Ident(i) = t { bstyle(&i.to_ascii_lowercase()) } else { None }) { st = Some(b) }
                else if let Some(bw) = bwidth(x, fs, cx) { w = Some(bw) }
                else if let Some(cc) = color(x, cx.current_color) { c = Some(cc) }
            }
            for &i in sides {
                s.border_style[i] = st.unwrap_or(0);
                s.border_w[i] = w.unwrap_or(3 * FX);
                s.border_color[i] = c.unwrap_or(s.color);
            }
        }
        "border-width" => { let ws: Vec<i32> = p().iter().filter_map(|x| bwidth(x, fs, cx)).collect(); if let Some(f) = four(&ws) { s.border_w = f } }
        "border-style" => { let ws: Vec<u8> = idv().iter().filter_map(|x| bstyle(x)).collect(); if let Some(f) = four(&ws) { s.border_style = f } }
        "border-color" => { let ws: Vec<u32> = p().iter().filter_map(|x| color(x, cx.current_color)).collect(); if let Some(f) = four(&ws) { s.border_color = f } }
        n if n.starts_with("border-") && (n.ends_with("-width") || n.ends_with("-style") || n.ends_with("-color")) => {
            let side = match n.split('-').nth(1).unwrap_or("") { "top" | "block" => 0, "right" | "inline" => 1, "bottom" => 2, "left" => 3, _ => return };
            let sides: Vec<usize> = match n.split('-').nth(1).unwrap_or("") { "block" => vec![0, 2], "inline" => vec![1, 3], _ => vec![side] };
            for i in sides {
                if n.ends_with("-width") { if let Some(w) = bwidth(v, fs, cx) { s.border_w[i] = w } }
                else if n.ends_with("-style") { if let Some(b) = bstyle(kw) { s.border_style[i] = b } }
                else if let Some(c) = color(v, cx.current_color) { s.border_color[i] = c }
            }
        }
        "color" => if let Some(c) = color(v, parent.color) { s.color = c },
        "background-color" => if let Some(c) = color(v, cx.current_color) { s.bg = c },
        "background" => {
            for x in p() {
                if let Some(c) = color(x, cx.current_color) { s.bg = c; }
                if matches!(x.first(), Some(Tok::Url(_))) || matches!(x.first(), Some(Tok::Func(f)) if f == "url" || f.contains("gradient")) {
                    if s.bg_img == 0 { s.bg_img = intern(fam, tok_str(x)); }
                }
                if let Some(Tok::Ident(k)) = x.first() { match k.as_str() { "cover" => s.bg_size = 1, "contain" => s.bg_size = 2,
                    "no-repeat" => s.bg_repeat = 1, "repeat-x" => s.bg_repeat = 2, "repeat-y" => s.bg_repeat = 3, "center" => s.bg_center = true, _ => {} } }
            }
            if kw == "none" { s.bg = 0; s.bg_img = 0; }
        }
        "background-image" => s.bg_img = if kw == "none" { 0 } else { p().first().map(|x| intern(fam, tok_str(x))).unwrap_or(0) },
        "background-size" => {
            s.bg_size = match kw { "cover" => 1, "contain" => 2, _ => 0 };
            if s.bg_size == 0 { let ls: Vec<Len> = p().iter().filter_map(|x| len(x, fs, cx)).collect(); s.bg_dim = [*ls.first().unwrap_or(&Len::Auto), *ls.get(1).unwrap_or(&Len::Auto)]; }
        }
        "background-repeat" => s.bg_repeat = match kw { "no-repeat" => 1, "repeat-x" => 2, "repeat-y" => 3, _ => 0 },
        "background-position" => s.bg_center = idv().iter().any(|x| x == "center") || p().iter().any(|x| matches!(x.first(), Some(Tok::Pct(v)) if *v == 50.0)),
        "grid-template-columns" => s.grid_cols = if kw == "none" { 0 } else { intern(fam, tok_str(v)) },
        "grid-template-rows" => s.grid_rows = if kw == "none" { 0 } else { intern(fam, tok_str(v)) },
        "grid-template-areas" => s.grid_areas = if kw == "none" { 0 } else { intern(fam, tok_str(v)) },
        "grid-auto-rows" => s.grid_auto_rows = intern(fam, tok_str(v)),
        "grid-auto-columns" => s.grid_auto_cols = intern(fam, tok_str(v)),
        "grid-auto-flow" => { let ids = idv(); s.grid_flow = ids.iter().any(|x| x == "column") as u8 + if ids.iter().any(|x| x == "dense") { 2 } else { 0 }; }
        "grid-template" | "grid" => {
            // rows / columns form (areas strings handled when present)
            let txt = tok_str(v);
            if let Some((r, c)) = txt.split_once('/') {
                if txt.contains('"') { s.grid_areas = intern(fam, r.to_string()); } else { s.grid_rows = intern(fam, r.trim().to_string()); }
                s.grid_cols = intern(fam, c.trim().to_string());
            }
        }
        "grid-column" => s.grid_col = intern(fam, tok_str(v)),
        "grid-row" => s.grid_row = intern(fam, tok_str(v)),
        "grid-column-start" => s.grid_col = intern(fam, tok_str(v)),
        "grid-row-start" => s.grid_row = intern(fam, tok_str(v)),
        "grid-area" => s.grid_area = intern(fam, tok_str(v)),
        "justify-self" => s.justify_self = match kw { "start" | "flex-start" | "self-start" | "left" => 1, "end" | "flex-end" | "self-end" | "right" => 2, "center" => 3, _ => 0 },
        "align-self" => s.align_self = match kw { "start" | "flex-start" | "self-start" => 1, "end" | "flex-end" | "self-end" => 2, "center" => 3, _ => 0 },
        "place-self" => { let a = match kw { "start" => 1, "end" => 2, "center" => 3, _ => 0 }; s.align_self = a; s.justify_self = a; }
        "font-size" => {
            let pfs = cx.parent_fs;
            let nv = match kw {
                "xx-small" => 9 * FX, "x-small" => 10 * FX, "small" => 13 * FX, "medium" => 16 * FX, "large" => 18 * FX,
                "x-large" => 24 * FX, "xx-large" => 32 * FX, "xxx-large" => 48 * FX,
                "larger" => pfs * 6 / 5, "smaller" => pfs * 5 / 6,
                _ => match len(v, pfs, cx) { Some(Len::Px(x)) => x, Some(Len::Pct(pc)) => (pfs as i64 * pc as i64 / 10000) as i32, _ => return },
            };
            s.font_size = nv;
        }
        "font-weight" => s.font_weight = match v.first() {
            Some(Tok::Num(n)) => *n as u16,
            _ => match kw { "normal" => 400, "bold" => 700, "bolder" => if parent.font_weight < 400 { 400 } else if parent.font_weight < 600 { 700 } else { 900 },
                            "lighter" => if parent.font_weight < 600 { 100 } else if parent.font_weight < 800 { 400 } else { 700 }, _ => return },
        },
        "font-style" => s.italic = matches!(kw, "italic" | "oblique"),
        "font-family" => {
            let names: Vec<String> = v.split(|t| *t == Tok::Comma).map(|g| g.iter().filter_map(|t| match t {
                Tok::Ident(x) => Some(x.clone()), Tok::Str(x) => Some(x.clone()), _ => None }).collect::<Vec<_>>().join(" ")).collect();
            let key = names.join(",");
            s.font_family = match fam.iter().position(|f| *f == key) { Some(i) => i as u16, None => { fam.push(key); (fam.len() - 1) as u16 } };
        }
        "font" => {
            // [style] [weight] size[/line-height] family — the shorthand resets its longhands to their initial values first
            // (CSS Fonts 4 §2.8): style normal, weight 400, line-height normal; only a value that parses (has a size) resets
            let mut seen_size = false;
            let fsh = crate::layout::patch_on(crate::layout::PATCH_FONT_SHORTHAND);
            if fsh && p().iter().any(|x| { let szs: Vec<&[Tok]> = x.split(|t| *t == Tok::Delim('/')).collect(); matches!(len(szs[0], cx.parent_fs, cx), Some(Len::Px(_))) }) {
                s.italic = false; s.font_weight = 400; s.line_height = Len::Auto;
            }
            for (k, x) in p().iter().enumerate() {
                if seen_size { break; }
                match x.first() {
                    Some(Tok::Ident(i)) if matches!(i.to_ascii_lowercase().as_str(), "italic" | "oblique") => s.italic = true,
                    Some(Tok::Ident(i)) if i.eq_ignore_ascii_case("bold") => s.font_weight = 700,
                    Some(Tok::Num(n)) if *n >= 100.0 => s.font_weight = *n as u16,
                    _ => {
                        let szs: Vec<&[Tok]> = x.split(|t| *t == Tok::Delim('/')).collect();
                        if let Some(Len::Px(px)) = len(szs[0], cx.parent_fs, cx) {
                            s.font_size = px;
                            seen_size = true;
                            if let Some(lh) = szs.get(1) { line_height(s, lh, cx); }
                            // the family list is everything after the size part (CSS Fonts 4 §2.8: `font: 12.8px DejaVu Sans Mono`
                            // — unquoted multi-word names included); it was parsed but never applied (ours fell back to serif)
                            // the raw tokens after part k (parts() drops commas, which separate the families)
                            let rest: Vec<Tok> = { let (mut n, mut d, mut i, mut in_part) = (0usize, 0i32, 0usize, false);
                                while i < v.len() { match &v[i] { Tok::Ws | Tok::Comma if d == 0 => { if in_part { n += 1; in_part = false; if n == k + 1 { break; } } }
                                    Tok::Func(_) | Tok::LParen => { d += 1; in_part = true; } Tok::RParen => { d -= 1; } _ => { in_part = true; } } i += 1; }
                                v[i.min(v.len())..].to_vec() };
                            let names: Vec<String> = rest.split(|t| *t == Tok::Comma).map(|g| g.iter().filter_map(|t| match t {
                                Tok::Ident(x) => Some(x.clone()), Tok::Str(x) => Some(x.clone()), _ => None }).collect::<Vec<_>>().join(" ")).filter(|n| !n.is_empty()).collect();
                            // the family on its own switch (BROWSER-PRODUCT-0 P2: `font: 600 28px sans-serif` drew serif): measured on the
                            // doctor corpus +11 / −1 (weather-gov node 1331) → dormant until the doctor lane clears the −1
                            if (fsh || crate::layout::patch_on("patch.font_family")) && !names.is_empty() {
                                let key = names.join(",");
                                s.font_family = match fam.iter().position(|f| *f == key) { Some(i) => i as u16, None => { fam.push(key); (fam.len() - 1) as u16 } };
                            }
                        }
                    }
                }
            }
        }
        "line-height" => line_height(s, v, cx),
        "text-align" => s.text_align = match kw { "left" => 1, "right" => 2, "center" | "-internal-center" => 3, "-webkit-center" | "-moz-center" => 5, "justify" => 4, "start" => 0, "end" => 2, _ => return },
        "text-decoration" | "text-decoration-line" => {
            let mut b = 0;
            for x in idv() { b |= match x.as_str() { "underline" => 1, "line-through" => 2, "overline" => 4, _ => 0 }; }
            s.decoration = b;
        }
        "white-space" | "white-space-collapse" | "text-wrap-mode" => {
            if d.name == "text-wrap-mode" { if kw == "nowrap" { s.white_space = 2 } return; }
            s.white_space = match kw { "normal" | "collapse" => 0, "pre" | "preserve" => 1, "nowrap" => 2, "pre-wrap" | "break-spaces" => 3, "pre-line" | "preserve-breaks" => 4, _ => return }
        }
        "list-style-type" | "list-style" => {
            for x in idv() {
                s.list_style = match x.as_str() { "none" => 0, "disc" => 1, "circle" => 2, "square" => 3, "decimal" | "decimal-leading-zero" => 4,
                    "lower-alpha" | "upper-alpha" | "lower-roman" | "upper-roman" | "lower-latin" | "upper-latin" => 5, _ => continue };
            }
        }
        "visibility" => s.visibility = if kw == "hidden" || kw == "collapse" { 1 } else { 0 },
        "overflow" | "overflow-x" | "overflow-y" => s.overflow = match kw { "visible" => 0, "hidden" | "clip" => 1, "scroll" | "auto" => 2, _ => return },
        "box-sizing" => s.border_box = kw == "border-box",
        "table-layout" => s.table_fixed = kw == "fixed",
        // CSS 2.2 §17.6.1 / §17.6.2 (clean-room; DOCTOR missing-feature card): the first length is the horizontal spacing
        "border-spacing" => { if let Some(Len::Px(x)) = v.iter().find_map(|t| len(std::slice::from_ref(t), fs, cx)) { s.border_spacing = x.max(0) } }
        "border-collapse" => s.border_collapse = kw == "collapse",
        // CSS Sizing 4 §5.1 (clean-room): `<ratio>` = number [ / number ]; `auto` alone keeps 0; `auto <ratio>` keeps the ratio
        "aspect-ratio" => {
            let nums: Vec<f32> = d.value.iter().filter_map(|t| match t { Tok::Num(v) => Some(*v as f32), Tok::Dim(v, _) => Some(*v as f32), _ => None }).collect();
            s.aspect = match nums.as_slice() { [a] if *a > 0.0 => (a * 1000.0).round() as u32, [a, b] if *a > 0.0 && *b > 0.0 => (a / b * 1000.0).round() as u32, _ => 0 };
            s.aspect_auto = d.value.iter().any(|t| matches!(t, Tok::Ident(k) if k.eq_ignore_ascii_case("auto")));
        }
        "vertical-align" => s.vertical_align = match kw { "baseline" => 0, "middle" => 1, "top" => 2, "bottom" => 3, "sub" => 4, "super" => 5, "text-top" => 6, "text-bottom" => 7, _ => 0 },
        "text-transform" => s.text_transform = match kw { "uppercase" => 1, "lowercase" => 2, "capitalize" => 3, _ => 0 },
        "flex-direction" => s.flex_dir = match kw { "row" => 0, "row-reverse" => 1, "column" => 2, "column-reverse" => 3, _ => return },
        "flex-wrap" => s.flex_wrap = kw == "wrap" || kw == "wrap-reverse",
        "flex-flow" => for x in idv() { match x.as_str() { "row" => s.flex_dir = 0, "column" => s.flex_dir = 2, "wrap" => s.flex_wrap = true, "nowrap" => s.flex_wrap = false, _ => {} } },
        "justify-content" => s.justify = match kw { "flex-start" | "start" | "left" | "normal" => 0, "flex-end" | "end" | "right" => 1, "center" => 2, "space-between" => 3, "space-around" => 4, "space-evenly" => 5, _ => return },
        // CSS Box Alignment 3 §6.1 / §6.3 (clean-room; DOCTOR missing-feature card): justify-items is the default justify-self
        // of the container's items; place-items = <align-items> [<justify-items>]
        // CSS Box Alignment 3 §5.1 (clean-room; DOCTOR missing-feature card): align-content; place-content = <align> [<justify>]
        "align-content" => s.align_content = ac_kw(kw),
        "place-content" => { let ids = idv(); if let Some(w) = ids.first() { s.align_content = ac_kw(w); } }
        "justify-items" => s.justify_items = match kw { "start" | "flex-start" | "self-start" | "left" => 1, "end" | "flex-end" | "self-end" | "right" => 2, "center" => 3, _ => 0 },
        "place-items" => {
            let ids = idv(); let k = |w: &str| -> u8 { match w { "start" | "flex-start" | "self-start" => 1, "end" | "flex-end" | "self-end" => 2, "center" => 3, "baseline" => 4, _ => 0 } };
            let a = ids.first().map(|w| k(w)).unwrap_or(0); let j = ids.get(1).map(|w| k(w)).unwrap_or(a);
            s.align_items = a; s.justify_items = if j == 4 { 0 } else { j };
        }
        "align-items" => s.align_items = match kw { "stretch" | "normal" => 0, "flex-start" | "start" | "self-start" => 1, "flex-end" | "end" | "self-end" => 2, "center" => 3, "baseline" => 4, _ => return },
        "flex-grow" => if let Some(Tok::Num(n)) = v.first() { s.grow = (*n * 100.0) as u16 },
        "order" => if let Some(Tok::Num(n)) = v.first() { s.order = (*n as i32).clamp(-32768, 32767) as i16 },
        "flex-shrink" => if let Some(Tok::Num(n)) = v.first() { s.shrink = (*n * 100.0) as u16 },
        "flex-basis" => if let Some(x) = l() { s.basis = x },
        "flex" => {
            // keyword forms only when the value IS that one keyword (`flex: 0 1 auto` is not `flex: auto`)
            if p().len() == 1 { match kw { "none" => { s.grow = 0; s.shrink = 0; s.basis = Len::Auto; return } "auto" => { s.grow = 100; s.shrink = 100; s.basis = Len::Auto; return } "initial" => { s.grow = 0; s.shrink = 100; s.basis = Len::Auto; return } _ => {} } }
            let nums: Vec<f32> = p().iter().filter_map(|x| if let Some(Tok::Num(n)) = x.first() { Some(*n) } else { None }).collect();
            // Flexbox §7.1.1: an omitted basis is 0%; an explicit `auto` / `content` / length is kept; shrink defaults to 1
            if let Some(g) = nums.first() { s.grow = (g * 100.0) as u16; s.basis = Len::Pct(0); s.shrink = 100; }
            if let Some(sh) = nums.get(1) { s.shrink = (sh * 100.0) as u16; }
            if idv().iter().any(|x| x == "auto" || x == "content") { s.basis = Len::Auto; }
            for x in p() { if matches!(x.first(), Some(Tok::Dim(..)) | Some(Tok::Pct(_))) { if let Some(b) = len(x, fs, cx) { s.basis = b } } }
        }
        "gap" | "grid-gap" => { let ls: Vec<Len> = p().iter().filter_map(|x| len(x, fs, cx)).collect(); if !ls.is_empty() { s.gap = [ls[0], *ls.get(1).unwrap_or(&ls[0])] } }
        "row-gap" | "grid-row-gap" => if let Some(x) = l() { s.gap[0] = x }, "column-gap" | "grid-column-gap" => if let Some(x) = l() { s.gap[1] = x },
        "transform" | "translate" => {
            if kw == "none" { s.translate = [Len::Px(0); 2]; return; }
            let mut t = [Len::Px(0); 2];
            if d.name == "translate" {
                let ls: Vec<Len> = p().iter().filter_map(|x| len(x, fs, cx)).collect();
                if let Some(x) = ls.first() { t[0] = *x; } if let Some(y) = ls.get(1) { t[1] = *y; }
            } else {
                let mut k = 0;
                while k < v.len() {
                    if let Tok::Func(f) = &v[k] {
                        let mut d2 = 1; let mut j = k + 1;
                        while j < v.len() { match v[j] { Tok::Func(_) | Tok::LParen => d2 += 1, Tok::RParen => { d2 -= 1; if d2 == 0 { break; } } _ => {} } j += 1; }
                        let args: Vec<Len> = v[k + 1..j.min(v.len())].split(|x| *x == Tok::Comma).filter_map(|a| { let a: Vec<Tok> = a.iter().filter(|x| **x != Tok::Ws).cloned().collect(); len(&a, fs, cx) }).collect();
                        match f.as_str() {
                            "translate" | "translate3d" => { if let Some(x) = args.first() { t[0] = *x; } if let Some(y) = args.get(1) { t[1] = *y; } }
                            "translatex" => { if let Some(x) = args.first() { t[0] = *x; } }
                            "translatey" => { if let Some(y) = args.first() { t[1] = *y; } }
                            _ => {}
                        }
                        k = j + 1;
                    } else { k += 1; }
                }
            }
            s.translate = t;
        }
        "scrollbar-gutter" => s.sb_gutter = kw.starts_with("stable"),
        // `container: <name> / <type>` or `container-type: <type>`; the type is the only part layout needs (the cq units' base)
        "container-type" | "container" => {
            let ty = if d.name == "container" { v.iter().skip_while(|t| **t != Tok::Delim('/')).cloned().collect::<Vec<_>>() } else { v.to_vec() };
            let has = |w: &str| ty.iter().any(|t| matches!(t, Tok::Ident(x) if x.eq_ignore_ascii_case(w)));
            s.container = if has("size") { 2 } else if has("inline-size") { 1 } else { 0 };
        }
        "-webkit-line-clamp" | "line-clamp" => s.line_clamp = match v.iter().find_map(|t| if let Tok::Num(n) = t { Some(*n) } else { None }) { Some(n) if n >= 1.0 => n.min(255.0) as u8, _ => 0 },
        "-webkit-box-orient" | "box-orient" => s.box_vertical = matches!(kw, "vertical" | "block-axis"),
        "border-radius" => {
            // up to 4 radii before an optional `/` (vertical radii, not kept): 1 → all, 2 → tl+br / tr+bl, 3 → tl / tr+bl / br, 4 → each
            let head: Vec<Len> = v.split(|t| matches!(t, Tok::Delim('/'))).next().unwrap_or(&[]).split(|t| *t == Tok::Ws)
                .filter(|x| !x.is_empty()).filter_map(|x| len(x, fs, cx)).collect();
            s.radius = match head.len() { 1 => [head[0]; 4], 2 => [head[0], head[1], head[0], head[1]], 3 => [head[0], head[1], head[2], head[1]],
                n if n >= 4 => [head[0], head[1], head[2], head[3]], _ => return };
        }
        "border-top-left-radius" | "border-top-right-radius" | "border-bottom-right-radius" | "border-bottom-left-radius" => {
            let i = match d.name.as_str() { "border-top-left-radius" => 0, "border-top-right-radius" => 1, "border-bottom-right-radius" => 2, _ => 3 };
            if let Some(l) = v.split(|t| *t == Tok::Ws).find(|x| !x.is_empty()).and_then(|x| len(x, fs, cx)) { s.radius[i] = l; }
        }
        "box-shadow" => s.shadow = if kw == "none" { 0 } else { intern(fam, tok_str(v)) },
        "fill" => s.svg_fill = if kw == "none" { Some(0) } else { color(v, s.color).or(s.svg_fill) },
        "stroke" => s.svg_stroke = if kw == "none" { Some(0) } else { color(v, s.color).or(s.svg_stroke) },
        "stroke-width" => s.svg_sw = match v.first() { Some(Tok::Num(n)) => Some((n * FX as f32) as i32), _ => match len(v, fs, cx) { Some(Len::Px(x)) => Some(x), _ => s.svg_sw } },
        "backdrop-filter" | "-webkit-backdrop-filter" => s.backdrop = if kw == "none" { 0 } else { intern(fam, tok_str(v)) },
        "letter-spacing" => s.letter_spacing = if kw == "normal" { 0 } else { match len(v, fs, cx) { Some(Len::Px(x)) => x, _ => return } },
        "z-index" => if let Some(Tok::Num(n)) = v.first() { s.z = *n as i16 },
        "opacity" => if let Some(Tok::Num(n)) = v.first() { s.opacity = (n.clamp(0.0, 1.0) * 255.0) as u8 },
        "text-indent" => if let Some(x) = l() { s.text_indent = x },
        _ => {}
    }
}

fn is_inherited(n: &str) -> bool {
    matches!(n, "color" | "font" | "font-size" | "font-family" | "font-weight" | "font-style" | "line-height" | "text-align" | "white-space" | "list-style" | "list-style-type" | "visibility" | "text-transform" | "text-indent" | "letter-spacing" | "border-spacing" | "border-collapse" | "word-spacing" | "fill" | "stroke" | "stroke-width")
}

fn line_height(s: &mut Style, v: &[Tok], cx: &Ctx<'_, '_>) {
    s.line_height = match v.first() {
        Some(Tok::Ident(x)) if x.eq_ignore_ascii_case("normal") => Len::Auto,
        Some(Tok::Num(n)) => Len::Pct((n * 10000.0) as i32),
        _ => match len(v, s.font_size, cx) { Some(Len::Pct(p)) => Len::Px((s.font_size as i64 * p as i64 / 10000) as i32), Some(x) => x, None => return },
    };
}

/// Resolve every element's computed style and intern the distinct ones.
pub fn compute<'s>(d: &Dom, sheet: &'s Sheet, viewport: (f32, f32)) -> Styled {
    EXPRS.with(|e| e.borrow_mut().clear()); // symbolic expressions are per document
    compute_inner(d, sheet, viewport, None)
}

/// INCR-R3 S1 — the style change trit. A node's KEY hashes every input of its computed style: own tag + attributes +
/// :empty, the parent's key (ancestors, inherited style, vars, root font size, closed <details>, flex/grid parent), the
/// parent's whole element-child sequence (+ ~ :nth-* :*-of-type :last-*) and the node's index in it. Key hit = trit 0:
/// the previous document's style, vars and generated boxes are reused (no matching, no cascade); miss = trit +: computed
/// and stored. The memo keeps only the current document's keys. The tables Style indexes into (families, EXPRS) persist
/// across the session, so a full compute seeded from the same tables gives bit-identical styles (the gate).
/// Sheet must be built WITHOUT the load-time KeyFilter (it drops rules for classes absent at load).
pub struct StyleMemo<'s> {
    pub families: Vec<String>,
    pub exprs: ExprTable,
    map: HashMap<u128, MemoEnt<'s>>,
    /// S1b: the interned style table persists across the session (append-only), so a hit is `ids[i] = id` — no Style hashing
    table: Vec<Style>,
    intern: HashMap<Style, u16>,
    pub hits: usize, pub misses: usize, pub shared: usize, pub resets: usize,
    /// svrt/uiir number wired elements `data-ir="N"` in render order, so one new wire renumbers every later element; the
    /// attribute is left out of the key when no selector or declaration in the sheet mentions it (decided once per sheet)
    skip_ir: Option<bool>,
    /// which elements can depend on their siblings at all (decided once per sheet; see SibIndex)
    sib: Option<Rc<SibIndex>>,
}
/// Necessary condition for "this element's style can depend on its siblings": some compound that is sibling-sensitive
/// (structural pseudo-class, or the right side of a + / ~ combinator, also inside :not / :is) could match it. Indexed by
/// the compound's own tag / #id / .class; a compound with none of them makes every element sensitive (`any`).
#[derive(Default)]
pub struct SibIndex { any: bool, tags: std::collections::HashSet<String>, ids: std::collections::HashSet<String>, classes: std::collections::HashSet<String> }
impl SibIndex {
    pub fn of(sheet: &Sheet) -> SibIndex {
        fn sens(c: &css::Compound) -> bool {
            matches!(c.comb, Some(css::Comb::Next) | Some(css::Comb::Later)) || c.simples.iter().any(|x| match x {
                css::Simple::FirstChild | css::Simple::LastChild | css::Simple::OnlyChild | css::Simple::FirstOfType | css::Simple::LastOfType | css::Simple::NthChild(..) => true,
                css::Simple::Not(l) | css::Simple::Is(l) => l.iter().any(|p| p.iter().any(sens)),
                _ => false,
            })
        }
        let mut ix = SibIndex::default();
        for r in &sheet.rules {
            for c in &r.sel.parts {
                if !sens(c) { continue; }
                let key = c.simples.iter().find_map(|x| match x { css::Simple::Id(v) => Some((1, v.clone())), _ => None })
                    .or_else(|| c.simples.iter().find_map(|x| match x { css::Simple::Class(v) => Some((2, v.clone())), _ => None }))
                    .or_else(|| c.simples.iter().find_map(|x| match x { css::Simple::Tag(v) => Some((0, v.to_ascii_lowercase())), _ => None }));
                match key { Some((0, v)) => { ix.tags.insert(v); } Some((1, v)) => { ix.ids.insert(v); } Some((_, v)) => { ix.classes.insert(v); } None => ix.any = true }
            }
        }
        ix
    }
    fn sensitive(&self, d: &Dom, e: u32) -> bool {
        if self.any || self.tags.contains(&d.tag(e).to_ascii_lowercase()) { return true; }
        if let Some(id) = d.attr(e, "id") { if self.ids.contains(id) { return true; } }
        d.attr(e, "class").map_or(false, |c| c.split_ascii_whitespace().any(|k| self.classes.contains(k)))
    }
}
#[derive(Clone)]
struct MemoEnt<'s> { style: Rc<Style>, id: u16, vars: Rc<VarScope<'s>>, pseudo: Vec<(u8, u16, String)> }
const MEMO_TABLE_CAP: usize = 40_000; // u16 ids: reset the session before the table can overflow
impl<'s> StyleMemo<'s> {
    pub fn new() -> Self { StyleMemo { families: vec!["serif".into()], exprs: ExprTable(Vec::new()), map: HashMap::new(), table: vec![Style::default()],
        intern: [(Style::default(), 0u16)].into_iter().collect(), hits: 0, misses: 0, shared: 0, resets: 0, skip_ir: None, sib: None } }
    pub fn len(&self) -> usize { self.map.len() }
    fn reset(&mut self) { *self = StyleMemo { resets: self.resets + 1, hits: self.hits, misses: self.misses, shared: self.shared, skip_ir: self.skip_ir, sib: self.sib.clone(), ..StyleMemo::new() }; }
}
/// Incremental restyle against `memo` (updated in place: tables extended, map = this document's keys).
pub fn compute_incr<'s>(d: &Dom, sheet: &'s Sheet, viewport: (f32, f32), memo: &mut StyleMemo<'s>) -> Styled {
    if memo.table.len() > MEMO_TABLE_CAP { memo.reset(); }
    if memo.skip_ir.is_none() { memo.skip_ir = Some(!sheet_mentions_attr(sheet, "data-ir")); }
    if memo.sib.is_none() { memo.sib = Some(Rc::new(SibIndex::of(sheet))); }
    EXPRS_DEDUPE.with(|x| x.set(true)); exprs_restore(&memo.exprs);
    let st = compute_inner(d, sheet, viewport, Some(memo));
    memo.exprs = exprs_save(); EXPRS_DEDUPE.with(|x| x.set(false));
    st
}
/// The gate's reference: a FULL compute (no memo) seeded with the memo's tables; leaves the memo untouched.
pub fn compute_seeded<'s>(d: &Dom, sheet: &'s Sheet, viewport: (f32, f32), memo: &StyleMemo<'s>) -> Styled {
    EXPRS_DEDUPE.with(|x| x.set(true)); exprs_restore(&memo.exprs);
    let mut seed = StyleMemo { families: memo.families.clone(), table: memo.table.clone(), intern: memo.intern.clone(), skip_ir: memo.skip_ir, sib: memo.sib.clone(), ..StyleMemo::new() };
    if memo.table.len() > MEMO_TABLE_CAP { seed = StyleMemo::new(); exprs_restore(&ExprTable(Vec::new())); }
    let st = compute_inner(d, sheet, viewport, Some(&mut seed)); // empty map: every node a miss = the full path
    EXPRS_DEDUPE.with(|x| x.set(false));
    st
}

/// splitmix64 finaliser: full avalanche, a few cycles
#[inline] fn mix64(mut z: u64) -> u64 { z = (z ^ (z >> 30)).wrapping_mul(0xbf58476d1ce4e5b9); z = (z ^ (z >> 27)).wrapping_mul(0x94d049bb133111eb); z ^ (z >> 31) }
/// 128-bit key from parts: two independently seeded streams, each part absorbed through the finaliser
fn h128(parts: &[u64]) -> u128 {
    let (mut a, mut b) = (0x243f6a8885a308d3u64, 0x13198a2e03707344u64);
    for &p in parts { a = mix64(a ^ p.wrapping_mul(0x9e3779b97f4a7c15)); b = mix64(b.rotate_left(17) ^ p.wrapping_add(0xa4093822299f31d0)); }
    ((a as u128) << 64) | b as u128
}
/// byte string → 64 bits (8-byte words through the finaliser; length folded in)
#[inline] fn hbytes(h: u64, s: &[u8]) -> u64 {
    let mut h = mix64(h ^ s.len() as u64);
    let mut c = s.chunks_exact(8);
    for w in &mut c { h = mix64(h ^ u64::from_le_bytes(w.try_into().unwrap())); }
    let r = c.remainder(); if !r.is_empty() { let mut t = [0u8; 8]; t[..r.len()].copy_from_slice(r); h = mix64(h ^ u64::from_le_bytes(t) ^ 0xff); }
    h
}
/// Does any selector (including inside :not / :is) or any declaration of the sheet mention attribute `name`?
pub fn sheet_mentions_attr(sheet: &Sheet, name: &str) -> bool {
    fn sel(parts: &[css::Compound], name: &str) -> bool {
        parts.iter().any(|c| c.simples.iter().any(|x| match x {
            css::Simple::Attr(n, ..) => n == name,
            css::Simple::Not(l) | css::Simple::Is(l) => l.iter().any(|p| sel(p, name)),
            _ => false,
        }))
    }
    sheet.rules.iter().any(|r| sel(&r.sel.parts, name) || format!("{:?}", r.decls).contains(name))
}
/// tag + every attribute (name, value) in order + :empty (css.rs Simple::Empty definition); `skip_ir` leaves out data-ir
fn facts_hash(d: &Dom, e: u32, skip_ir: bool) -> u64 {
    let mut h = hbytes(0x51ed27, d.tag(e).as_bytes());
    for a in d.attr_list(e) { let nm = &*d.names[a.name as usize].local; if skip_ir && nm == "data-ir" { continue; } h = hbytes(h, nm.as_bytes()); h = hbytes(h, d.span_str(a.val).as_bytes()); }
    let empty = d.children(e).all(|c| d.nodes[c as usize].kind == Kind::Comment || (d.nodes[c as usize].kind == Kind::Text && d.text(c).is_empty()));
    mix64(h ^ empty as u64)
}

fn compute_inner<'s>(d: &Dom, sheet: &'s Sheet, viewport: (f32, f32), mut memo: Option<&mut StyleMemo<'s>>) -> Styled {
    let t0 = std::time::Instant::now();
    let _ = Media { width: viewport.0, height: viewport.1 };
    let f = ElemFacts::new(d);
    let n = d.nodes.len();
    let mut st = Styled { ids: vec![0; n], table: vec![Style::default()], families: vec!["serif".into()], rules: sheet.rules.len(), ..Default::default() };
    // INCR-R3: per-node style keys (only with a memo); old map consulted, new map = this document's keys
    let use_memo = memo.is_some() && !sheet.rules.iter().any(|r| r.sib && css::has_relative(&r.sel));
    let mut keys: Vec<u128> = Vec::new();
    let mut old_map: HashMap<u128, MemoEnt<'s>> = HashMap::new();
    let mut new_map: HashMap<u128, MemoEnt<'s>> = HashMap::new();
    let mut nofp: Vec<bool> = Vec::new();
    if let Some(m) = memo.as_mut() {
        st.families = m.families.clone();
        st.table = std::mem::take(&mut m.table);
        old_map = std::mem::take(&mut m.map);
        let skip_ir = m.skip_ir.unwrap_or(false);
        let sib = m.sib.clone();
        let facts: Vec<u64> = (0..n as u32).map(|i| if d.nodes[i as usize].kind == Kind::Element { facts_hash(d, i, skip_ir) } else { 0 }).collect();
        let mut seq: Vec<u64> = vec![0; n];
        for i in 0..n { let nd = d.nodes[i]; if nd.kind == Kind::Element && nd.parent != NONE { seq[nd.parent as usize] = h128(&[seq[nd.parent as usize], facts[i]]) as u64; } }
        keys = vec![0; n];
        let mut idx: Vec<u64> = vec![0; n]; let mut cnt: Vec<u64> = vec![0; n];
        for i in 0..n {
            let nd = d.nodes[i]; if nd.kind != Kind::Element { continue; }
            let (pk, mut ps, mut k) = if nd.parent != NONE { let p = nd.parent as usize; cnt[p] += 1; idx[i] = cnt[p]; (keys[p], seq[p], idx[i]) } else { (0, 0, 0) };
            // sibling context only for elements some sibling-sensitive compound could match (descendants inherit via pk)
            if let Some(sx) = &sib { if !sx.sensitive(d, i as u32) { ps = 0; k = u64::MAX; } }
            keys[i] = h128(&[(pk >> 64) as u64, pk as u64, ps, k, facts[i], (viewport.0.to_bits() as u64) << 32 | viewport.1.to_bits() as u64]);
        }
        nofp = vec![false; n];
    }
    let mut intern: HashMap<Style, u16> = match memo.as_mut() { Some(m) => std::mem::take(&mut m.intern), None => HashMap::new() };
    intern.insert(Style::default(), 0);
    // per-node full style + vars, kept only along the current ancestor path (preorder ⇒ parent seen first)
    let mut node_style: Vec<Option<Rc<Style>>> = vec![None; n];
    let mut node_vars: Vec<Option<Rc<VarScope<'s>>>> = vec![None; n];
    // ancestor Bloom per element: parent's set ∪ parent's own keys
    let mut bloom: Vec<css::Bloom> = vec![[0; 4]; n];
    let mut sibfp: Vec<u64> = vec![0; n];
    let mut last_elem: Vec<u32> = vec![NONE; n]; // most recent element child per parent (O(1) previous-sibling lookup)
    let root_style = Rc::new(Style { display: Display::Block, ..Style::default() });
    let empty_vars: Rc<VarScope<'s>> = Rc::new(VarScope { parent: None, map: HashMap::new() });
    let mut matched = Vec::new();
    let mut root_fs = 16 * FX;
    let t_setup = t0.elapsed().as_secs_f64() * 1e3;
    for i in 0..n as u32 {
        let node = d.nodes[i as usize];
        if node.kind != Kind::Element { continue; }
        let p = node.parent;
        let (pst, pvars) = if p != NONE && node_style[p as usize].is_some() {
            (node_style[p as usize].clone().unwrap(), node_vars[p as usize].clone().unwrap())
        } else { (root_style.clone(), empty_vars.clone()) };
        matched.clear();
        if p != NONE && d.nodes[p as usize].kind == Kind::Element {
            let mut b = bloom[p as usize];
            css::bloom_add(&mut b, b't', &d.tag(p).to_ascii_lowercase());
            if let Some(id) = &f.id[p as usize] { css::bloom_add(&mut b, b'#', id); }
            for c in &f.classes[p as usize] { css::bloom_add(&mut b, b'.', c); }
            bloom[i as usize] = b;
        }
        // style sharing: an identical previous element sibling (same tag, same attributes, no id / inline style)
        // has the same ancestors, so unless a candidate rule is sibling-position-sensitive its style is ours
        let prev = if p != NONE { last_elem[p as usize] } else { NONE };
        if p != NONE { last_elem[p as usize] = i; }
        // INCR-R3: style trit 0 — same inputs as a node of the previous document → reuse its result
        if use_memo {
            if let Some(ent) = old_map.remove(&keys[i as usize]).or_else(|| new_map.get(&keys[i as usize]).cloned()) {
                st.ids[i as usize] = ent.id;
                for (which, pid, text) in &ent.pseudo { st.pseudo.push((i, *which, *pid, text.clone())); }
                if d.tag(i) == "html" { root_fs = ent.style.font_size; }
                node_style[i as usize] = Some(ent.style.clone());
                node_vars[i as usize] = Some(ent.vars.clone());
                nofp[i as usize] = true;
                if let Some(m) = memo.as_mut() { m.hits += 1; }
                new_map.insert(keys[i as usize], ent);
                continue;
            }
        }
        let tf = std::time::Instant::now();
        let fp = sheet.sibling_fingerprint(d, &f, i, &bloom[i as usize]);
        st.stats.fp_ms += tf.elapsed().as_secs_f64() * 1e3;
        sibfp[i as usize] = fp;
        if let Some(sib) = Some(prev).filter(|&x| x != NONE) {
            if st.ids[sib as usize] != 0 && !(use_memo && nofp[sib as usize]) && same_shape(d, sib, i) && sibfp[sib as usize] == fp && node_style[sib as usize].is_some() {
                st.ids[i as usize] = st.ids[sib as usize];
                node_style[i as usize] = node_style[sib as usize].clone();
                node_vars[i as usize] = node_vars[sib as usize].clone();
                let extra: Vec<(u32, u8, u16, String)> = st.pseudo.iter().filter(|p| p.0 == sib).map(|p| (i, p.1, p.2, p.3.clone())).collect();
                if use_memo {
                    let pseudo = extra.iter().map(|(_, w, pid, t)| (*w, *pid, t.clone())).collect();
                    new_map.insert(keys[i as usize], MemoEnt { style: node_style[i as usize].clone().unwrap(), id: st.ids[i as usize], vars: node_vars[i as usize].clone().unwrap(), pseudo });
                    if let Some(m) = memo.as_mut() { m.shared += 1; }
                }
                st.pseudo.extend(extra);
                st.stats.shared += 1;
                continue;
            }
        }
        let tm = std::time::Instant::now();
        sheet.matching_bloom(d, &f, i, &bloom[i as usize], &mut matched);
        if sheet.scoped {
            // a shadow-scoped rule matches only inside its own shadow tree (the element's nearest ::shadow root)
            let mut q = d.nodes[i as usize].parent; while q != NONE && d.tag(q) != "::shadow" { q = d.nodes[q as usize].parent; }
            let root = if q == NONE { u32::MAX } else { q };
            // encapsulation: inside a shadow tree only the UA sheet and that tree's own rules apply; a shadow rule reaches out
            // of its tree only to its host (`:host`, rewritten to the host's tag)
            let ua = sheet.ua_rules;
            matched.retain(|&(_, o, ri)| { let sc = sheet.rules[ri as usize].scope;
                o < ua || sc == root || (sc != u32::MAX && d.nodes[sc as usize].parent == i) || (sc == u32::MAX && root == u32::MAX) });
        }
        st.stats.match_ms += tm.elapsed().as_secs_f64() * 1e3;
        st.stats.matched += matched.len();
        // cascade sort: origin (UA < author), then specificity, then order; important pass reverses origin
        matched.sort_unstable_by_key(|&(sp, o, _)| (o >= sheet.ua_rules, sp, o));
        let tg = std::time::Instant::now();
        let inline: Vec<Decl> = d.attr(i, "style").map(|s| css::parse_decls(&css::tokenize(s))).unwrap_or_default();
        // ::before / ::after rules style generated boxes; the element itself uses only pseudo-less rules
        let has_pseudo = matched.iter().any(|&(_, _, ri)| sheet.rules[ri as usize].sel.pseudo != 0);
        let pseudo_matched: Vec<(u32, u32, u32)> = if has_pseudo { matched.iter().copied().filter(|&(_, _, ri)| sheet.rules[ri as usize].sel.pseudo != 0).collect() } else { Vec::new() };
        if has_pseudo { matched.retain(|&(_, _, ri)| sheet.rules[ri as usize].sel.pseudo == 0); }
        let mut sheet_var_decls: Vec<&'s Decl> = Vec::new();
        for imp in [false, true] { for &(_, _, ri) in &matched { for dd in sheet.rules[ri as usize].decls.iter() { if dd.important == imp && dd.name.starts_with("--") { sheet_var_decls.push(dd); } } } }
        // gather declarations in cascade order: normal (spec, order), inline normal, important, inline important
        let mut decls: Vec<&Decl> = Vec::new();
        for imp in [false, true] {
            if imp {
                // important: author before UA (UA !important wins)
                for &(_, o, ri) in &matched { if o >= sheet.ua_rules { for dd in sheet.rules[ri as usize].decls.iter() { if dd.important { decls.push(dd); } } } }
                for &(_, o, ri) in &matched { if o < sheet.ua_rules { for dd in sheet.rules[ri as usize].decls.iter() { if dd.important { decls.push(dd); } } } }
            } else {
                for &(_, _, ri) in &matched { for dd in sheet.rules[ri as usize].decls.iter() { if !dd.important { decls.push(dd); } } }
            }
            for dd in &inline { if dd.important == imp { decls.push(dd); } }
        }
        st.stats.decls += decls.len();
        st.stats.gather_ms += tg.elapsed().as_secs_f64() * 1e3;
        // custom properties first (inherited)
        let tv = std::time::Instant::now();
        let mut vars = pvars.clone();
        let n_vars = sheet_var_decls.len() + inline.iter().filter(|dd| dd.name.starts_with("--")).count();
        if n_vars > 0 {
            let mut m: HashMap<std::borrow::Cow<'s, str>, VarVal<'s>> = HashMap::with_capacity(n_vars);
            // cascade order matters (later wins): sheet decls in order, then inline
            for dd in sheet_var_decls.iter() { m.insert(std::borrow::Cow::Borrowed(dd.name.as_str()), VarVal::B(&dd.value)); }
            for dd in &inline { if dd.name.starts_with("--") { m.insert(std::borrow::Cow::Owned(dd.name.clone()), VarVal::O(Rc::new(dd.value.clone()))); } }
            st.stats.vars += n_vars;
            vars = Rc::new(VarScope { parent: Some(pvars.clone()), map: m });
        }
        st.stats.vars_ms += tv.elapsed().as_secs_f64() * 1e3;
        let ta = std::time::Instant::now();
        let mut s = Style::inherit(&pst);
        let cx0 = Ctx { parent_fs: pst.font_size, root_fs, vars: &vars, current_color: pst.color };
        // font-size and color first: em units and currentColor depend on them
        for dd in &decls { if dd.name == "font-size" || dd.name == "font" { apply(&mut s, dd, &cx0, &mut st.families, &pst); } }
        for dd in &decls { if dd.name == "color" { apply(&mut s, dd, &cx0, &mut st.families, &pst); } }
        s.border_color = [s.color; 4];
        let cx = Ctx { parent_fs: pst.font_size, root_fs, vars: &vars, current_color: s.color };
        for dd in &decls {
            if dd.name.starts_with("--") || dd.name == "font-size" || dd.name == "font" || dd.name == "color" { continue; }
            apply(&mut s, dd, &cx, &mut st.families, &pst);
        }
        // presentational hints (HTML §15.3), only where no CSS set them
        // presentational hints yield only to AUTHOR declarations (HTML §15.3: they sit just above the UA sheet)
        let author: Vec<&Decl> = matched.iter().filter(|&&(_, o, _)| o >= sheet.ua_rules).flat_map(|&(_, _, ri)| sheet.rules[ri as usize].decls.iter()).chain(inline.iter()).collect();
        hints(d, i, &mut s, &author, &mut st.families, pst.font_size);
        // closed <details>: only its <summary> child is rendered (HTML §15.5.4)
        if p != NONE && d.tag(p) == "details" && d.attr(p, "open").is_none() && d.tag(i) != "summary" { s.display = Display::None; }
        // borders with style none have zero width (CSS Backgrounds §3)
        for k in 0..4 { if s.border_style[k] == 0 { s.border_w[k] = 0; } }
        // blockification: floats / absolute boxes are block-level (CSS Display §2.7)
        s.was_inline = matches!(s.display, Display::Inline | Display::InlineBlock | Display::InlineFlex | Display::InlineGrid | Display::InlineTable);
        let pflex = matches!(pst.display, Display::Flex | Display::InlineFlex | Display::Grid | Display::InlineGrid) && p != NONE && node_style[p as usize].is_some();
        if (s.float != 0 || matches!(s.position, 2 | 3) || pflex) && s.position != 3 || (matches!(s.position, 2 | 3)) {
            s.display = match s.display { Display::Inline | Display::InlineBlock => Display::Block, Display::InlineTable => Display::Table,
                Display::InlineFlex => Display::Flex, Display::InlineGrid => Display::Grid, x => x };
        }
        if d.tag(i) == "html" { root_fs = s.font_size; }
        st.stats.apply_ms += ta.elapsed().as_secs_f64() * 1e3;
        let ti = std::time::Instant::now();
        let id = match intern.get(&s) { Some(&x) => x, None => { let x = st.table.len() as u16; st.table.push(s.clone()); intern.insert(s.clone(), x); x } };
        st.stats.intern_ms += ti.elapsed().as_secs_f64() * 1e3;
        st.ids[i as usize] = id;
        if has_pseudo && s.display != Display::None {
            for which in [1u8, 2] {
                let mut pd: Vec<&Decl> = Vec::new();
                for imp in [false, true] {
                    for &(_, _, ri) in &pseudo_matched { let r = &sheet.rules[ri as usize]; if r.sel.pseudo == which { for dd in r.decls.iter() { if dd.important == imp { pd.push(dd); } } } }
                }
                if pd.is_empty() { continue; }
                let text = match pd.iter().rev().find(|dd| dd.name == "content") { Some(c) => content_text(&c.value, d, i), None => None };
                let text = match text { Some(t) => t, None => continue };
                let mut ps = Style::inherit(&s);
                // custom properties declared in the ::before/::after rule itself form the pseudo-element's own var scope
                // (CSS Variables §2: they apply to the generated box like any element; MDN sidebar chevron: --calc-width-height)
                let own: Vec<&&Decl> = pd.iter().filter(|dd| dd.name.starts_with("--")).collect();
                let pvars = if own.is_empty() || !crate::layout::patch_on(crate::layout::PATCH_PSEUDO_VARS) { vars.clone() } else {
                    let mut m: HashMap<std::borrow::Cow<'s, str>, VarVal<'s>> = HashMap::with_capacity(own.len());
                    for dd in own { m.insert(std::borrow::Cow::Borrowed(dd.name.as_str()), VarVal::B(&dd.value)); }
                    Rc::new(VarScope { parent: Some(vars.clone()), map: m })
                };
                let pcx = Ctx { parent_fs: s.font_size, root_fs, vars: &pvars, current_color: s.color };
                for dd in &pd { if dd.name == "font-size" || dd.name == "color" { apply(&mut ps, dd, &pcx, &mut st.families, &s); } }
                ps.border_color = [ps.color; 4];
                for dd in &pd { if dd.name.starts_with("--") || dd.name == "font-size" || dd.name == "color" || dd.name == "content" { continue; } apply(&mut ps, dd, &pcx, &mut st.families, &s); }
                for k in 0..4 { if ps.border_style[k] == 0 { ps.border_w[k] = 0; } }
                if ps.display == Display::None { continue; }
                // generated boxes inside a flex/grid container are flex/grid items: blockified like any child
                let in_fg = matches!(s.display, Display::Flex | Display::InlineFlex | Display::Grid | Display::InlineGrid);
                if (ps.float != 0 || matches!(ps.position, 2 | 3) || in_fg) && matches!(ps.display, Display::Inline | Display::InlineBlock) { ps.display = Display::Block; }
                let pid = match intern.get(&ps) { Some(&x) => x, None => { let x = st.table.len() as u16; st.table.push(ps.clone()); intern.insert(ps.clone(), x); x } };
                st.pseudo.push((i, which, pid, text));
            }
        }
        let s = Rc::new(s);
        if use_memo {
            let mut pseudo: Vec<(u8, u16, String)> = st.pseudo.iter().rev().take_while(|q| q.0 == i).map(|(_, w, pid, t)| (*w, *pid, t.clone())).collect();
            pseudo.reverse();
            new_map.insert(keys[i as usize], MemoEnt { style: s.clone(), id: st.ids[i as usize], vars: vars.clone(), pseudo });
            if let Some(m) = memo.as_mut() { m.misses += 1; }
        }
        node_style[i as usize] = Some(s);
        node_vars[i as usize] = Some(vars);
        // free parent state once its subtree is done is implicit: Rc drops when no child refers to it
    }
    st.stats.ms = t0.elapsed().as_secs_f64() * 1e3;
    st.stats.setup_ms = t_setup;
    if let Some(m) = memo { m.families = st.families.clone(); m.map = new_map; m.table = st.table.clone(); m.intern = intern; }
    st
}

fn prev_elem_sibling(d: &Dom, e: u32) -> Option<u32> {
    let p = d.nodes[e as usize].parent;
    if p == NONE { return None; }
    let mut last = None;
    for c in d.children(p) { if c == e { return last; } if d.nodes[c as usize].kind == Kind::Element { last = Some(c); } }
    None
}

/// Same tag and identical attributes (names + values), and nothing that styles one element alone.
fn same_shape(d: &Dom, a: u32, b: u32) -> bool {
    if d.nodes[a as usize].name != d.nodes[b as usize].name { return false; }
    let (la, lb) = (d.attr_list(a), d.attr_list(b));
    if la.len() != lb.len() { return false; }
    for (x, y) in la.iter().zip(lb) {
        let n = &*d.names[x.name as usize].local;
        if x.name != y.name || n == "id" || n == "style" || d.span_str(x.val) != d.span_str(y.val) { return false; }
    }
    let tag = d.tag(b);
    !matches!(tag, "details" | "summary" | "input" | "svg" | "html" | "body")
}

/// `content` → text of the generated box (None = no box: none / normal).
fn content_text(v: &[Tok], d: &Dom, e: u32) -> Option<String> {
    let mut out = String::new();
    let mut any = false;
    let mut k = 0;
    while k < v.len() {
        match &v[k] {
            Tok::Ident(x) if x == "none" || x == "normal" => return None,
            Tok::Ident(x) if x == "open-quote" => { out.push('\u{201C}'); any = true; }
            Tok::Ident(x) if x == "close-quote" => { out.push('\u{201D}'); any = true; }
            Tok::Ident(x) if x == "no-open-quote" || x == "no-close-quote" => any = true,
            Tok::Str(x) => { out.push_str(x); any = true; }
            Tok::Url(_) => any = true,
            Tok::Func(f) => {
                let mut j = k + 1;
                let mut dd = 1;
                while j < v.len() { match v[j] { Tok::Func(_) | Tok::LParen => dd += 1, Tok::RParen => { dd -= 1; if dd == 0 { break; } } _ => {} } j += 1; }
                if f == "attr" { if let Some(Tok::Ident(a)) = v.get(k + 1) { out.push_str(d.attr(e, a).unwrap_or("")); } }
                any = true; // url()/counter()/image-set(): a box exists even if we draw no text for it
                k = j;
            }
            Tok::Delim('/') => break, // alt text follows
            _ => {}
        }
        k += 1;
    }
    if any { Some(out) } else { None }
}

fn hints(d: &Dom, i: u32, s: &mut Style, decls: &[&Decl], fam: &mut Vec<String>, parent_fs: i32) {
    let has = |n: &str| decls.iter().any(|dd| dd.name == n || dd.name.starts_with(&format!("{n}-")));
    let tag = d.tag(i);
    if tag == "::shadow" { s.display = Display::Contents; return; } // declarative shadow root: its tree renders in place
    let px = |v: &str| v.trim().trim_end_matches("px").parse::<f32>().ok().map(|x| Len::Px((x * FX as f32) as i32))
        .or_else(|| v.trim().strip_suffix('%').and_then(|p| p.parse::<f32>().ok()).map(|p| Len::Pct((p * 100.0) as i32)));
    if let Some(w) = d.attr(i, "width") { if !has("width") && matches!(tag, "img" | "svg" | "table" | "td" | "th" | "col" | "iframe" | "video" | "canvas" | "embed" | "object" | "hr") { if let Some(l) = px(w) { s.width = l } } }
    if let Some(h) = d.attr(i, "height") { if !has("height") && matches!(tag, "img" | "svg" | "td" | "th" | "tr" | "iframe" | "video" | "canvas" | "embed" | "object" | "table") { if let Some(l) = px(h) { s.height = l } } }
    if let Some(a) = d.attr(i, "align") {
        let a = a.to_ascii_lowercase();
        if !has("text-align") && matches!(tag, "p" | "div" | "h1" | "h2" | "h3" | "h4" | "h5" | "h6" | "td" | "th" | "tr" | "caption") {
            s.text_align = match a.as_str() { "center" | "middle" => 5, "right" => 2, "left" => 1, "justify" => 4, _ => s.text_align };
        }
        if tag == "img" || tag == "table" { if !has("float") { s.float = match a.as_str() { "left" => 1, "right" => 2, _ => s.float }; } }
    }
    if let Some(c) = d.attr(i, "bgcolor") { if !has("background") { if let Some(x) = hexc(c.trim_start_matches('#')).or_else(|| named_color(c)) { s.bg = x } } }
    if tag == "font" {
        // HTML §15.3.4: <font size> 1–7 (or ±n relative to 3) → xx-small … xxx-large; face → font-family
        if let Some(sz) = d.attr(i, "size") {
            let t = sz.trim();
            let n: Option<i32> = if let Some(r) = t.strip_prefix('+') { r.parse::<i32>().ok().map(|v| 3 + v) } else if let Some(r) = t.strip_prefix('-') { r.parse::<i32>().ok().map(|v| 3 - v) } else { t.parse().ok() };
            if let (Some(n), false) = (n, has("font-size")) {
                s.font_size = [10, 13, 16, 18, 24, 32, 48][(n.clamp(1, 7) - 1) as usize] * FX;
                let _ = parent_fs;
            }
        }
        if let Some(face) = d.attr(i, "face") { if !has("font-family") { s.font_family = intern(fam, face.to_string()); } }
        if let Some(c) = d.attr(i, "color") { if !has("color") { if let Some(x) = hexc(c.trim_start_matches('#')).or_else(|| named_color(c)) { s.color = x } } }
    }
    if matches!(tag, "td" | "th" | "tr" | "tbody" | "thead" | "tfoot") && !has("vertical-align") {
        if let Some(v) = d.attr(i, "valign") { s.vertical_align = match v.trim().to_ascii_lowercase().as_str() { "top" => 2, "bottom" => 3, "baseline" => 0, _ => 1 }; }
    }
    if (tag == "td" || tag == "th") && !has("padding") {
        let mut p = d.nodes[i as usize].parent;
        while p != NONE && d.tag(p) != "table" { p = d.nodes[p as usize].parent; }
        if p != NONE { if let Some(cp) = d.attr(p, "cellpadding").and_then(|v| v.trim().parse::<f32>().ok()) { s.padding = [Len::Px((cp * FX as f32) as i32); 4]; } }
    }
    if tag == "table" {
        if let Some(b) = d.attr(i, "border") { if let Ok(w) = b.trim().parse::<i32>() { if w > 0 && !has("border") { s.border_w = [w * FX; 4]; s.border_style = [5; 4]; } } }
        if let Some(cp) = d.attr(i, "cellpadding") { let _ = cp; }
    }
}

fn ac_kw(w: &str) -> u8 { match w { "start" | "flex-start" => 1, "end" | "flex-end" => 2, "center" => 3, "space-between" => 4, "space-around" => 5, "space-evenly" => 6, _ => 0 } }
