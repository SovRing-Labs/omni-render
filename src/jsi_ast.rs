// SPDX-License-Identifier: MIT OR Apache-2.0
//! Typed JS AST for jsi: the estree IR (JSON) is lowered once into enums with interned names and pre-parsed literals,
//! so evaluation never touches a string-keyed map or compares node-type strings (the JSON walk cost ~36x node).
//! Generator and async bodies are additionally marked: every node whose subtree holds a `yield`/`await` (not crossing a
//! function boundary) is wrapped in `Expr::Y` / `Stmt::Y`, so the resumable evaluator (jsi.rs, ECMA-262 §27.5 / §27.7)
//! walks only the path to a suspension point and runs every other subtree on the plain fast path.
use serde_json::Value as J;
use std::rc::Rc;

pub type Name = Rc<str>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Bin { Add, Sub, Mul, Div, Mod, Pow, StrictEq, StrictNe, Eq, Ne, Lt, Gt, Le, Ge, BitAnd, BitOr, BitXor, Shl, Shr, UShr, In, InstanceOf }
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Un { Not, Neg, Plus, BitNot, TypeOf, Void, Delete }
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Logic { And, Or, Nullish }
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AssignOp { Set, Bin(Bin), AndSet, OrSet, NullishSet }

#[derive(Debug)]
pub enum Lit { Undef, Null, Bool(bool), Num(f64), Str(Name), Regex(String, String), Big(Name) }

#[derive(Debug)]
pub enum Key { Static(Name), Computed(Expr) }

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PropKind { Init, Get, Set, Spread, Method, Proto }

#[derive(Debug)]
pub struct Prop { pub key: Key, pub value: Expr, pub kind: PropKind }

/// A tagged template's call site (ECMA-262 §13.2.8.4 GetTemplateObject): the strings array is built once per site.
pub struct TplSite { pub cooked: Vec<Option<Name>>, pub raw: Vec<Name>, pub exprs: Vec<Expr>, pub cache: std::cell::RefCell<Option<crate::jsi::Val>> }
impl std::fmt::Debug for TplSite { fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result { write!(f, "TplSite({:?})", self.raw) } }

#[derive(Debug)]
pub enum Expr {
    Lit(Lit),
    Ident(Name),
    /// an identifier resolved at lowering (JS-COMPILE-0 C1): the binding is slot `.1` of the frame `.0` hops up the chain
    Slot(u32, u32, Name),
    This,
    Template(Vec<Name>, Vec<Expr>),
    Array(Vec<Option<Expr>>, Vec<bool>), // elements, spread flags
    Object(Vec<Prop>),
    Func(Rc<Func>),
    Class(Rc<ClassDef>),
    Unary(Un, Box<Expr>),
    TypeOfIdent(Name),
    DeleteMember(Box<Expr>, Box<Key>),
    Update { inc: bool, prefix: bool, target: Box<Expr> },
    Binary(Bin, Box<Expr>, Box<Expr>),
    Logical(Logic, Box<Expr>, Box<Expr>),
    Cond(Box<Expr>, Box<Expr>, Box<Expr>),
    Assign(AssignOp, Box<Target>, Box<Expr>),
    Seq(Vec<Expr>),
    Member { obj: Box<Expr>, key: Box<Key>, optional: bool },
    Call { callee: Box<Expr>, args: Vec<Expr>, spread: Vec<bool>, optional: bool },
    New { callee: Box<Expr>, args: Vec<Expr>, spread: Vec<bool> },
    Chain(Box<Expr>),
    Await(Box<Expr>),
    /// `yield x` / `yield* x` (delegate)
    Yield(Option<Box<Expr>>, bool),
    /// subtree holding a suspension point (generator / async bodies only)
    Y(Box<Expr>),
    SuperMember(Box<Key>),
    SuperCall(Vec<Expr>, Vec<bool>),
    NewTarget,
    ImportMeta,
    Tagged(Box<Expr>, Rc<TplSite>),
    ImportCall(Box<Expr>),
    Unsupported(Name),
}

#[derive(Debug)]
pub enum Target { Pat(Pat), Member(Expr, Key), Super(Key), Slot(u32, u32, Name) }

#[derive(Debug)]
pub enum Pat {
    Ident(Name),
    Default(Box<Pat>, Box<Expr>),
    Array(Vec<Option<Pat>>, Option<Box<Pat>>),
    Object(Vec<(Key, Pat)>, Option<Box<Pat>>),
    Member(Box<Expr>, Box<Key>),
}

#[derive(Debug)]
pub enum Body { Block(Vec<Stmt>), Expr(Expr) }

/// What a call needs of a function: its parameters, body and hoisted names. Built when the function is first called
/// (PARSE-DOCK: a placeholder from jscomp stands for the function until then), or at once for an eagerly lowered one.
#[derive(Debug)]
pub struct FuncBody {
    pub params: Vec<Pat>, pub rest: Option<Pat>, pub body: Body,
    /// body holds a yield/await (marked with Y nodes): runs on the resumable evaluator
    pub resumable: bool,
    /// var-declared names anywhere in the body (ECMA-262 §10.2.11 FunctionDeclarationInstantiation step 27)
    pub vars: Vec<Name>,
    /// the activation frame's slots (JS-COMPILE-0 C1): this / arguments / %home / %fn / %nt (not for arrows), the
    /// parameters, the vars and the body's top-level lexical names; `pslots` / `vslots` index params and vars into it
    pub layout: Rc<Layout>, pub pslots: Vec<u32>, pub vslots: Vec<u32>,
    /// the source mentions `eval`: direct eval can add names to any frame, so nothing in or under it is resolved
    pub open: bool,
    /// calls so far and the closure-compiled body once hot (JS-COMPILE-0 C3, jsc.rs)
    pub calls: std::cell::Cell<u32>, pub code: std::cell::OnceCell<crate::jsc::Code>,
    /// the body holds a loop (not counting nested functions): compiled on its first call, since a once-called
    /// function (an init IIFE, a `main`) may still spend its time in a loop
    pub has_loop: bool,
}

/// Slot indices of the special bindings in a non-arrow activation frame.
pub const SL_THIS: usize = 0; pub const SL_ARGS: usize = 1; pub const SL_HOME: usize = 2; pub const SL_FN: usize = 3; pub const SL_NT: usize = 4;
const SPECIALS: [&str; 5] = ["this", "arguments", "%home", "%fn", "%nt"];

/// The names a frame holds, in slot order (Cytron et al. 1991 static scope resolution; clean-room).
#[derive(Debug, Default)]
pub struct Layout { pub names: Vec<Name>, idx: Option<crate::jsi::FxMap<Name, u32>> }
impl Layout {
    pub fn new(mut names: Vec<Name>) -> Layout {
        let mut seen = std::collections::HashSet::new(); names.retain(|n| seen.insert(n.clone()));
        let idx = if names.len() > 8 { Some(names.iter().enumerate().map(|(i, n)| (n.clone(), i as u32)).collect()) } else { None };
        Layout { names, idx }
    }
    #[inline] pub fn find(&self, name: &str) -> Option<usize> {
        match &self.idx { Some(m) => m.get(name).map(|&i| i as usize), None => self.names.iter().position(|n| std::ptr::eq(n.as_ptr(), name.as_ptr()) || &**n == name) }
    }
}

/// One frame of the static scope chain, mirroring the frames the interpreter creates at run time (jsi.rs `child`).
/// `slotted`: an activation frame whose bindings live in slots; otherwise a map frame whose names are only checked
/// for shadowing. `open`: may hold names not listed (with) — resolution stops there.
#[derive(Debug)]
pub struct SFrame { lay: Rc<Layout>, slotted: bool, open: bool, parent: Option<Rc<SFrame>> }

#[derive(Debug)]
pub struct Func {
    pub name: Name, pub arrow: bool, pub is_async: bool, pub uses_args: bool, pub uses_this: bool,
    pub is_gen: bool, pub uses_super: bool, pub uses_meta: bool,
    /// method / accessor / arrow: no own `prototype`, not a constructor
    pub method: bool,
    /// strict mode code (§11.2.2): a "use strict" directive here or around it, or class code — `this` is not coerced
    pub strict: bool,
    /// parameter count without a rest element / before the first default (Function `length`)
    pub np_all: usize, pub np_len: usize,
    cell: std::cell::OnceCell<FuncBody>,
    /// address of the stored node (jscomp) a lazy function is lowered from
    lazy: Option<usize>,
    /// an eagerly lowered body, resolved and moved into `cell` on first use
    eager: std::cell::RefCell<Option<FuncBody>>,
    /// the static scope chain where this function is created; set while the enclosing body is resolved
    env: std::cell::OnceCell<Rc<SFrame>>,
}
impl Func {
    pub fn lazy_addr(&self) -> Option<usize> { self.lazy }
    /// the parameters / body, lowering them now if this function was left as a placeholder
    pub fn b(&self) -> &FuncBody {
        self.cell.get_or_init(|| {
            if let Some(mut fb) = self.eager.borrow_mut().take() { resolve(&mut fb, self); return fb; }
            let pr = if crate::jsi::prof_on() { crate::jsi::prof_enter(); Some(std::time::Instant::now()) } else { None };
            let j = crate::jscomp::fetch(self.lazy.expect("lazy function without an address"));
            let t1 = std::time::Instant::now();
            crate::jsi::count_forced();
            let mut r = in_strict(self.strict, || lower_body(&j, self.is_async, self.is_gen, self.arrow));
            resolve(&mut r, self);
            if let Some(t) = pr { crate::jsi::prof_leave("<force:total>", t, 0); if t1.duration_since(t).as_millis() > 20 { eprintln!("force slow: fetch {:.0} ms, lower {:.0} ms", t1.duration_since(t).as_secs_f64() * 1e3, t1.elapsed().as_secs_f64() * 1e3); } }
            r
        })
    }
    fn set_env(&self, e: &Rc<SFrame>) { let _ = self.env.set(e.clone()); }
}

thread_local! { static STRICT: std::cell::Cell<u32> = const { std::cell::Cell::new(0) }; }
// an `eval` identifier was lowered (direct eval possible): set by `expr`, read per body by `lower_body`
thread_local! { static EVAL_SEEN: std::cell::Cell<bool> = const { std::cell::Cell::new(false) }; }
/// A Use Strict Directive (§11.2.1) at the head of a statement list.
pub fn has_use_strict(body: &J) -> bool {
    for st in arr(body) { if st["type"] != "ExpressionStatement" { break; } match st.get("directive").and_then(|d| d.as_str()) { Some("use strict") => return true, Some(_) => continue, None => break } }
    false
}
/// Lower `f` inside strict code (class bodies, "use strict" functions).
fn in_strict<T>(on: bool, f: impl FnOnce() -> T) -> T { if on { STRICT.with(|c| c.set(c.get() + 1)); } let r = f(); if on { STRICT.with(|c| c.set(c.get() - 1)); } r }

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MKind { Method, Get, Set, Field, StaticBlock }

#[derive(Debug)]
pub struct Member { pub key: Key, pub kind: MKind, pub is_static: bool, pub func: Option<Rc<Func>>, pub value: Option<Expr>, pub block: Vec<Stmt> }

#[derive(Debug)]
pub struct ClassDef { pub name: Name, pub ctor: Rc<Func>, pub sup: Option<Expr>, pub members: Vec<Member> }

#[derive(Debug)]
pub enum VarKind { Var, Let, Const }

#[derive(Debug)]
pub enum Stmt {
    Expr(Expr),
    Var(VarKind, Vec<(Pat, Option<Expr>)>),
    Func(Name, Rc<Func>),
    Class(Name, Rc<ClassDef>),
    Return(Option<Expr>),
    If(Expr, Box<Stmt>, Option<Box<Stmt>>),
    /// scoped = declares let/const/class/function (needs its own frame); `Lay` = that frame's slots once resolved
    Block(Vec<Stmt>, bool, Lay),
    /// per_iter = `let` loop whose body creates closures (each iteration needs its own copy of the bindings)
    For { init: Option<Box<Stmt>>, test: Option<Expr>, update: Option<Expr>, body: Box<Stmt>, per_iter: bool, lay: Lay },
    /// decl: 0 = assignment target, 1 = `var`, 2 = `let`/`const`; `lay` = the per-iteration frame
    ForOf { decl: u8, pat: Pat, right: Expr, body: Box<Stmt>, of: bool, is_await: bool, lay: Lay },
    While(Expr, Box<Stmt>),
    DoWhile(Box<Stmt>, Expr),
    Break(Option<Name>), Continue(Option<Name>),
    Throw(Expr),
    /// `lays`: the block / catch / finally frames
    Try { block: Vec<Stmt>, param: Option<Pat>, handler: Option<Vec<Stmt>>, finalizer: Option<Vec<Stmt>>, lays: [Lay; 3] },
    Switch(Expr, Vec<(Option<Expr>, Vec<Stmt>)>, Lay),
    Labeled(Name, Box<Stmt>),
    Empty,
    Import(Name, Vec<(ImportKind, Name)>),
    ExportDecl(Box<Stmt>, Vec<Name>),
    ExportDefault(Expr),
    ExportDefaultDecl(Box<Stmt>, Option<Name>),
    ExportNames(Option<Name>, Vec<(Name, Name)>),
    ExportAll(Name),
    /// statement holding a suspension point (generator / async bodies only)
    Y(Box<Stmt>),
    /// `with (obj) body` (sloppy mode, Annex-free §14.11)
    With(Expr, Box<Stmt>),
    Unsupported(Name),
}

/// A statically known frame's slot layout, set by the resolver (JS-COMPILE-0 C1b); None = a map frame.
pub type Lay = Option<Rc<Layout>>;

#[derive(Debug)]
pub enum ImportKind { Default, Namespace, Named(Name) }

fn s<'j>(n: &'j J, k: &str) -> &'j str { n.get(k).and_then(|x| x.as_str()).unwrap_or("") }
fn arr(n: &J) -> &[J] { n.as_array().map(|v| v.as_slice()).unwrap_or(&[]) }
fn nm(x: &str) -> Name { Rc::from(x) }
/// Private names (ECMA-262 §6.2.12) live under a key no source string can spell and no enumeration shows.
pub fn private_key(n: &str) -> Name { Rc::from(format!("\u{0}#{n}").as_str()) }

fn bin(op: &str) -> Option<Bin> {
    Some(match op { "+" => Bin::Add, "-" => Bin::Sub, "*" => Bin::Mul, "/" => Bin::Div, "%" => Bin::Mod, "**" => Bin::Pow,
        "===" => Bin::StrictEq, "!==" => Bin::StrictNe, "==" => Bin::Eq, "!=" => Bin::Ne, "<" => Bin::Lt, ">" => Bin::Gt, "<=" => Bin::Le, ">=" => Bin::Ge,
        "&" => Bin::BitAnd, "|" => Bin::BitOr, "^" => Bin::BitXor, "<<" => Bin::Shl, ">>" => Bin::Shr, ">>>" => Bin::UShr, "in" => Bin::In, "instanceof" => Bin::InstanceOf, _ => return None })
}

pub fn key_of(prop: &J, computed: bool) -> Key {
    if computed { return Key::Computed(expr(prop)); }
    match s(prop, "type") {
        "Identifier" => Key::Static(nm(s(prop, "name"))),
        "Literal" => Key::Static(nm(&match &prop["value"] { J::String(x) => x.clone(), J::Number(n) => crate::jsi::num_str(n.as_f64().unwrap_or(0.0)), J::Bool(b) => b.to_string(), J::Null => "null".into(), _ => String::new() })),
        "PrivateIdentifier" => Key::Static(private_key(s(prop, "name"))),
        _ => Key::Computed(expr(prop)),
    }
}

fn lit(n: &J) -> Lit {
    if n.get("regex").map_or(false, |r| !r.is_null()) { return Lit::Regex(s(&n["regex"], "pattern").into(), s(&n["regex"], "flags").into()); }
    if let Some(b) = n.get("bigint").and_then(|b| b.as_str()) { return Lit::Big(nm(b)); }
    match &n["value"] { J::Null => Lit::Null, J::Bool(b) => Lit::Bool(*b), J::Number(x) => Lit::Num(x.as_f64().unwrap_or(0.0)), J::String(x) => Lit::Str(nm(x)), _ => Lit::Undef }
}

fn args(a: &J) -> (Vec<Expr>, Vec<bool>) {
    let mut v = Vec::new(); let mut sp = Vec::new();
    for x in arr(a) { if x["type"] == "SpreadElement" { v.push(expr(&x["argument"])); sp.push(true); } else { v.push(expr(x)); sp.push(false); } }
    (v, sp)
}

fn is_type(o: &serde_json::Map<String, J>, t: &str) -> bool { o.get("type").and_then(|t| t.as_str()) == Some(t) }

pub fn func(n: &J) -> Rc<Func> { func_m(n, false) }

fn func_m(n: &J, method: bool) -> Rc<Func> {
    let own = if n.get("$t").is_some() { n["m"].as_u64().unwrap_or(0) >> 8 & 1 == 1 } else { n["body"]["type"] == "BlockStatement" && has_use_strict(&n["body"]["body"]) };
    in_strict(own, || func_inner(n, method))
}
/// params, rest, body, hoisted vars and the resumable mark of a function node
fn lower_body(n: &J, is_async: bool, is_gen: bool, arrow: bool) -> FuncBody {
    let outer_eval = EVAL_SEEN.with(|c| c.replace(false));
    let mut params = Vec::new(); let mut rest = None;
    for p in arr(&n["params"]) { if p["type"] == "RestElement" { rest = Some(pat(&p["argument"])); } else { params.push(pat(p)); } }
    let mut body = if n["body"]["type"] == "BlockStatement" { Body::Block(stmts(arr(&n["body"]["body"]))) } else { Body::Expr(expr(&n["body"])) };
    let mut vars = Vec::new();
    if let Body::Block(b) = &body { collect_vars(b, &mut vars); }
    let resumable = (is_gen || is_async) && match &mut body { Body::Block(b) => { let mut y = false; for st in b.iter_mut() { y |= wrap_s(st); } y } Body::Expr(e) => wrap_e(e) };
    // `eval` anywhere in what was lowered (params, body, eagerly lowered nested functions — conservative); restored for
    // an enclosing body still being lowered
    let open = EVAL_SEEN.with(|c| { let o = c.get(); c.set(o || outer_eval); o });
    finish_body(params, rest, body, vars, resumable, arrow, open)
}
/// FuncBody with its activation layout (see `FuncBody.layout`).
fn finish_body(params: Vec<Pat>, rest: Option<Pat>, body: Body, vars: Vec<Name>, resumable: bool, arrow: bool, open: bool) -> FuncBody {
    let mut names: Vec<Name> = if arrow { vec![] } else { SPECIALS.iter().map(|x| nm(x)).collect() };
    for p in params.iter().chain(rest.iter()) { decl_names(p, &mut names); }
    names.extend(vars.iter().cloned());
    if let Body::Block(b) = &body { lex_names(b, &mut names); }
    let layout = Layout::new(names);
    let pslots = params.iter().map(|p| match p { Pat::Ident(n) => layout.find(n).unwrap() as u32, _ => u32::MAX }).collect();
    let vslots = vars.iter().map(|v| layout.find(v).unwrap() as u32).collect();
    let has_loop = match &body { Body::Block(b) => b.iter().any(stmt_has_loop), Body::Expr(_) => false };
    FuncBody { params, rest, body, vars, resumable, layout: Rc::new(layout), pslots, vslots, open, calls: Default::default(), code: Default::default(), has_loop }
}
fn stmt_has_loop(s: &Stmt) -> bool {
    match s {
        Stmt::For { .. } | Stmt::ForOf { .. } | Stmt::While(..) | Stmt::DoWhile(..) => true,
        Stmt::If(_, c, a) => stmt_has_loop(c) || a.as_ref().map_or(false, |a| stmt_has_loop(a)),
        Stmt::Block(b, ..) => b.iter().any(stmt_has_loop),
        Stmt::Try { block, handler, finalizer, .. } => block.iter().any(stmt_has_loop) || handler.as_ref().map_or(false, |h| h.iter().any(stmt_has_loop)) || finalizer.as_ref().map_or(false, |f| f.iter().any(stmt_has_loop)),
        Stmt::Switch(_, cases, _) => cases.iter().any(|(_, b)| b.iter().any(stmt_has_loop)),
        Stmt::Labeled(_, b) | Stmt::Y(b) | Stmt::With(_, b) => stmt_has_loop(b),
        _ => false,
    }
}
/// LexicallyDeclaredNames of a statement list as the interpreter binds them into the list's frame: let / const
/// declarations, classes, function declarations (hoist_fns).
fn lex_names(b: &[Stmt], out: &mut Vec<Name>) {
    fn one(st: &Stmt, out: &mut Vec<Name>) {
        match st { Stmt::Var(VarKind::Let | VarKind::Const, d) => for (p, _) in d { decl_names(p, out) }, Stmt::Func(n, _) | Stmt::Class(n, _) => out.push(n.clone()),
            Stmt::Y(d) | Stmt::ExportDecl(d, _) | Stmt::ExportDefaultDecl(d, _) => one(d, out), _ => {} }
    }
    for st in b { one(st, out); }
}
fn func_inner(n: &J, method: bool) -> Rc<Func> {
    crate::jsi::count_fn();
    let strict = STRICT.with(|c| c.get()) > 0;
    let arrow = n["type"] == "ArrowFunctionExpression";
    if let Some(addr) = n.get("$t").and_then(|x| x.as_u64()) {
        let m = n["m"].as_u64().unwrap_or(0); let bit = |i: u32| m >> i & 1 == 1;
        let np = n["n"].as_u64().unwrap_or(0);
        return Rc::new(Func { name: nm(s(&n["id"], "name")), arrow, is_async: bit(9), is_gen: bit(10), method: method || arrow, strict, uses_args: bit(0), uses_this: bit(1), uses_super: bit(2), uses_meta: bit(3),
            np_all: (np & 0xffff) as usize, np_len: (np >> 16) as usize, cell: std::cell::OnceCell::new(), lazy: Some(addr as usize), eager: Default::default(), env: Default::default() });
    }
    let is_async = n["async"].as_bool().unwrap_or(false); let is_gen = n["generator"].as_bool().unwrap_or(false);
    let fb = lower_body(n, is_async, is_gen, arrow);
    let fl = body_flags(&n["body"]);
    let (np_all, np_len) = (fb.params.len(), fb.params.iter().take_while(|p| !matches!(p, Pat::Default(..))).count());
    Rc::new(Func { name: nm(s(&n["id"], "name")), arrow, is_async, is_gen, method: method || arrow, strict, uses_args: fl.0, uses_this: fl.1, uses_super: fl.2, uses_meta: fl.3,
        np_all, np_len, cell: std::cell::OnceCell::new(), lazy: None, eager: std::cell::RefCell::new(Some(fb)), env: Default::default() })
}

/// The constructor a class without one gets (ECMA-262 §15.7.14 ClassDefinitionEvaluation step 14):
/// base `constructor() {}`, derived `constructor(...args) { super(...args); }`.
fn default_ctor(derived: bool, name: &str) -> Rc<Func> {
    let (rest, body) = if derived { (Some(Pat::Ident(nm("args"))), vec![Stmt::Expr(Expr::SuperCall(vec![Expr::Ident(nm("args"))], vec![true]))]) } else { (None, vec![]) };
    Rc::new(Func { name: nm(name), arrow: false, is_async: false, is_gen: false, method: false, strict: true, uses_args: false, uses_this: true, uses_super: derived, uses_meta: false,
        np_all: 0, np_len: 0, cell: std::cell::OnceCell::from(finish_body(vec![], rest, Body::Block(body), vec![], false, false, false)), lazy: None, eager: Default::default(), env: Default::default() })
}

fn class(n: &J) -> Rc<ClassDef> { in_strict(true, || class_inner(n)) }
fn class_inner(n: &J) -> Rc<ClassDef> {
    let name = nm(s(&n["id"], "name"));
    let derived = !n["superClass"].is_null();
    let mut ctor = None; let mut members = Vec::new();
    for m in arr(&n["body"]["body"]) {
        let is_static = m["static"].as_bool().unwrap_or(false);
        let computed = m["computed"].as_bool().unwrap_or(false);
        match s(m, "type") {
            "MethodDefinition" => {
                let kind = s(m, "kind");
                // the class constructor carries the class's name (ECMA-262 §15.7.14 step 18 SetFunctionName)
                if kind == "constructor" { let mut v = m["value"].clone(); v["id"] = serde_json::json!({"type": "Identifier", "name": &*name}); ctor = Some(func_m(&v, false)); continue; }
                let mk = match kind { "get" => MKind::Get, "set" => MKind::Set, _ => MKind::Method };
                members.push(Member { key: key_of(&m["key"], computed), kind: mk, is_static, func: Some(func_m(&m["value"], true)), value: None, block: vec![] });
            }
            "PropertyDefinition" => members.push(Member { key: key_of(&m["key"], computed), kind: MKind::Field, is_static, func: None, value: if m["value"].is_null() { None } else { Some(expr(&m["value"])) }, block: vec![] }),
            "StaticBlock" => members.push(Member { key: Key::Static(nm("")), kind: MKind::StaticBlock, is_static: true, func: None, value: None, block: stmts(arr(&m["body"])) }),
            _ => {}
        }
    }
    Rc::new(ClassDef { ctor: ctor.unwrap_or_else(|| default_ctor(derived, &name)), name, members, sup: if derived { Some(expr(&n["superClass"])) } else { None } })
}

pub fn pat(p: &J) -> Pat {
    match s(p, "type") {
        "Identifier" => Pat::Ident(nm(s(p, "name"))),
        "AssignmentPattern" => Pat::Default(Box::new(pat(&p["left"])), Box::new(expr(&p["right"]))),
        "ArrayPattern" => {
            let mut els = Vec::new(); let mut rest = None;
            for e in arr(&p["elements"]) { if e.is_null() { els.push(None); } else if e["type"] == "RestElement" { rest = Some(Box::new(pat(&e["argument"]))); } else { els.push(Some(pat(e))); } }
            Pat::Array(els, rest)
        }
        "ObjectPattern" => {
            let mut props = Vec::new(); let mut rest = None;
            for pr in arr(&p["properties"]) {
                if pr["type"] == "RestElement" { rest = Some(Box::new(pat(&pr["argument"]))); continue; }
                props.push((key_of(&pr["key"], pr["computed"].as_bool().unwrap_or(false)), pat(&pr["value"])));
            }
            Pat::Object(props, rest)
        }
        "MemberExpression" => Pat::Member(Box::new(if p["object"]["type"] == "Super" { Expr::This } else { expr(&p["object"]) }), Box::new(key_of(&p["property"], p["computed"].as_bool().unwrap_or(false)))),
        _ => Pat::Ident(nm("")),
    }
}

/// Template raw strings: the IR keeps only `cooked` (the front-ends strip every `raw` key), so the raw text is re-spelled
/// from the cooked value with the escapes a source template would have needed (ECMA-262 §13.2.8.2 TRV).
fn raw_of(cooked: &str) -> String {
    let mut o = String::with_capacity(cooked.len());
    for c in cooked.chars() { match c { '\\' => o += "\\\\", '\n' => o += "\\n", '\r' => o += "\\r", '\t' => o += "\\t", '\u{8}' => o += "\\b", '\u{c}' => o += "\\f", '\u{b}' => o += "\\v", '\0' => o += "\\0", '`' => o += "\\`", c => o.push(c) } }
    o.replace("${", "\\${")
}

pub fn expr(n: &J) -> Expr {
    match s(n, "type") {
        "Literal" => Expr::Lit(lit(n)),
        "Identifier" => { let name = s(n, "name"); if name == "eval" { EVAL_SEEN.with(|c| c.set(true)); } Expr::Ident(nm(name)) }
        "ThisExpression" => Expr::This,
        "TemplateLiteral" => Expr::Template(arr(&n["quasis"]).iter().map(|q| nm(q["value"]["cooked"].as_str().unwrap_or(""))).collect(), arr(&n["expressions"]).iter().map(expr).collect()),
        "TaggedTemplateExpression" => {
            let q = &n["quasi"];
            let cooked: Vec<Option<Name>> = arr(&q["quasis"]).iter().map(|e| e["value"]["cooked"].as_str().map(nm)).collect();
            let raw = arr(&q["quasis"]).iter().map(|e| nm(&match e["value"]["raw"].as_str() { Some(r) => r.to_string(), None => raw_of(e["value"]["cooked"].as_str().unwrap_or("")) })).collect();
            Expr::Tagged(Box::new(expr(&n["tag"])), Rc::new(TplSite { cooked, raw, exprs: arr(&q["expressions"]).iter().map(expr).collect(), cache: std::cell::RefCell::new(None) }))
        }
        "ArrayExpression" => { let mut v = Vec::new(); let mut sp = Vec::new(); for e in arr(&n["elements"]) { if e.is_null() { v.push(None); sp.push(false); } else if e["type"] == "SpreadElement" { v.push(Some(expr(&e["argument"]))); sp.push(true); } else { v.push(Some(expr(e))); sp.push(false); } } Expr::Array(v, sp) }
        "ObjectExpression" => Expr::Object(arr(&n["properties"]).iter().map(|p| {
            if p["type"] == "SpreadElement" { return Prop { key: Key::Static(nm("")), value: expr(&p["argument"]), kind: PropKind::Spread }; }
            let computed = p["computed"].as_bool().unwrap_or(false);
            let kind = match s(p, "kind") { "get" => PropKind::Get, "set" => PropKind::Set, _ if p["method"].as_bool().unwrap_or(false) => PropKind::Method, _ => PropKind::Init };
            // ECMA-262 §13.2.5.5: a non-computed, non-shorthand `__proto__: v` sets the prototype
            let is_proto = kind == PropKind::Init && !computed && !p["shorthand"].as_bool().unwrap_or(false) && (s(&p["key"], "name") == "__proto__" || p["key"]["value"].as_str() == Some("__proto__"));
            let value = if p["value"].is_null() { Expr::Ident(nm(s(&p["key"], "name"))) } else if matches!(kind, PropKind::Get | PropKind::Set | PropKind::Method) { Expr::Func(func_m(&p["value"], true)) } else { expr(&p["value"]) };
            Prop { key: key_of(&p["key"], computed), value, kind: if is_proto { PropKind::Proto } else { kind } }
        }).collect()),
        "FunctionExpression" | "ArrowFunctionExpression" => Expr::Func(func(n)),
        "ClassExpression" => Expr::Class(class(n)),
        "UnaryExpression" => {
            let op = s(n, "operator");
            if op == "typeof" && n["argument"]["type"] == "Identifier" { return Expr::TypeOfIdent(nm(s(&n["argument"], "name"))); }
            if op == "delete" {
                let a = if n["argument"]["type"] == "ChainExpression" { &n["argument"]["expression"] } else { &n["argument"] };
                return if a["type"] == "MemberExpression" { Expr::DeleteMember(Box::new(expr(&a["object"])), Box::new(key_of(&a["property"], a["computed"].as_bool().unwrap_or(false)))) } else { Expr::Seq(vec![expr(a), Expr::Lit(Lit::Bool(true))]) };
            }
            let u = match op { "!" => Un::Not, "-" => Un::Neg, "+" => Un::Plus, "~" => Un::BitNot, "typeof" => Un::TypeOf, "void" => Un::Void, _ => Un::Void };
            Expr::Unary(u, Box::new(expr(&n["argument"])))
        }
        "UpdateExpression" => Expr::Update { inc: s(n, "operator") == "++", prefix: n["prefix"].as_bool().unwrap_or(false), target: Box::new(expr(&n["argument"])) },
        "BinaryExpression" => {
            // `#x in o` (ECMA-262 §13.10.1): the private name's key
            let l = if n["left"]["type"] == "PrivateIdentifier" { Expr::Lit(Lit::Str(private_key(s(&n["left"], "name")))) } else { expr(&n["left"]) };
            match bin(s(n, "operator")) { Some(b) => Expr::Binary(b, Box::new(l), Box::new(expr(&n["right"]))), None => Expr::Unsupported(nm(s(n, "operator"))) }
        }
        "LogicalExpression" => Expr::Logical(match s(n, "operator") { "&&" => Logic::And, "||" => Logic::Or, _ => Logic::Nullish }, Box::new(expr(&n["left"])), Box::new(expr(&n["right"]))),
        "ConditionalExpression" => Expr::Cond(Box::new(expr(&n["test"])), Box::new(expr(&n["consequent"])), Box::new(expr(&n["alternate"]))),
        "AssignmentExpression" => {
            let op = s(n, "operator");
            let aop = match op { "=" => AssignOp::Set, "&&=" => AssignOp::AndSet, "||=" => AssignOp::OrSet, "??=" => AssignOp::NullishSet, _ => bin(&op[..op.len().saturating_sub(1)]).map(AssignOp::Bin).unwrap_or(AssignOp::Set) };
            let l = &n["left"];
            let t = if l["type"] == "MemberExpression" { if l["object"]["type"] == "Super" { Target::Super(key_of(&l["property"], l["computed"].as_bool().unwrap_or(false))) } else { Target::Member(expr(&l["object"]), key_of(&l["property"], l["computed"].as_bool().unwrap_or(false))) } } else { Target::Pat(pat(l)) };
            Expr::Assign(aop, Box::new(t), Box::new(expr(&n["right"])))
        }
        "SequenceExpression" => Expr::Seq(arr(&n["expressions"]).iter().map(expr).collect()),
        "MemberExpression" => {
            let key = Box::new(key_of(&n["property"], n["computed"].as_bool().unwrap_or(false)));
            if n["object"]["type"] == "Super" { return Expr::SuperMember(key); }
            Expr::Member { obj: Box::new(expr(&n["object"])), key, optional: n["optional"].as_bool().unwrap_or(false) }
        }
        "CallExpression" => {
            let (a, sp) = args(&n["arguments"]);
            if n["callee"]["type"] == "Super" { return Expr::SuperCall(a, sp); }
            Expr::Call { callee: Box::new(expr(&n["callee"])), args: a, spread: sp, optional: n["optional"].as_bool().unwrap_or(false) }
        }
        "NewExpression" => { let (a, sp) = args(&n["arguments"]); Expr::New { callee: Box::new(expr(&n["callee"])), args: a, spread: sp } }
        "ChainExpression" => Expr::Chain(Box::new(expr(&n["expression"]))),
        "AwaitExpression" => Expr::Await(Box::new(expr(&n["argument"]))),
        "YieldExpression" => Expr::Yield(if n["argument"].is_null() { None } else { Some(Box::new(expr(&n["argument"]))) }, n["delegate"].as_bool().unwrap_or(false)),
        "ParenthesizedExpression" => expr(&n["expression"]),
        "MetaProperty" => if s(&n["meta"], "name") == "new" { Expr::NewTarget } else { Expr::ImportMeta },
        "ImportExpression" => Expr::ImportCall(Box::new(expr(&n["source"]))),
        "Super" => Expr::This,
        "PrivateIdentifier" => Expr::Lit(Lit::Str(private_key(s(n, "name")))),
        other => Expr::Unsupported(nm(other)),
    }
}

pub fn stmts(v: &[J]) -> Vec<Stmt> { v.iter().map(stmt).collect() }
/// A Program body: strict when it opens with "use strict".
pub fn program(body: &J) -> Vec<Stmt> { in_strict(has_use_strict(body), || stmts(arr(body))) }
/// Module code is always strict (§11.2.2).
pub fn module_program(body: &J) -> Vec<Stmt> { in_strict(true, || stmts(arr(body))) }

/// (uses arguments, uses this or an arrow, uses super, uses new.target/import.meta) of a body, in ONE walk (was four).
pub fn body_flags(n: &J) -> (bool, bool, bool, bool) {
    fn go(n: &J, f: &mut (bool, bool, bool, bool)) {
        match n {
            J::Object(o) => {
                match o.get("type").and_then(|t| t.as_str()) {
                    Some("Identifier") => if o.get("name").and_then(|x| x.as_str()) == Some("arguments") { f.0 = true },
                    Some("ThisExpression" | "ArrowFunctionExpression") => f.1 = true,
                    Some("Super") => f.2 = true,
                    Some("MetaProperty") => f.3 = true,
                    _ => {}
                }
                // a lazy function: its whole-node flags were computed when it was stored
                if o.contains_key("$t") { let m = o.get("m").and_then(|x| x.as_u64()).unwrap_or(0); f.0 |= m >> 4 & 1 == 1; f.1 |= m >> 5 & 1 == 1; f.2 |= m >> 6 & 1 == 1; f.3 |= m >> 7 & 1 == 1; return; }
                if !(f.0 && f.1 && f.2 && f.3) { for v in o.values() { go(v, f); } }
            }
            J::Array(a) => if !(f.0 && f.1 && f.2 && f.3) { for v in a { go(v, f); } },
            _ => {}
        }
    }
    let mut f = (false, false, false, false); go(n, &mut f); f
}

fn mentions(n: &J, pred: &dyn Fn(&serde_json::Map<String, J>) -> bool) -> bool {
    match n { J::Object(o) => pred(o) || o.values().any(|v| mentions(v, pred)), J::Array(a) => a.iter().any(|v| mentions(v, pred)), _ => false }
}

/// Does this subtree create a function (closure)?
fn has_fn(n: &J) -> bool {
    match n { J::Object(o) => matches!(o.get("type").and_then(|t| t.as_str()), Some("FunctionExpression" | "ArrowFunctionExpression" | "FunctionDeclaration" | "ClassExpression" | "ClassDeclaration")) || o.values().any(has_fn), J::Array(a) => a.iter().any(has_fn), _ => false }
}

pub fn decl_names(p: &Pat, out: &mut Vec<Name>) {
    match p { Pat::Ident(n) => out.push(n.clone()), Pat::Default(p, _) => decl_names(p, out),
        Pat::Array(els, rest) => { for e in els.iter().flatten() { decl_names(e, out); } if let Some(r) = rest { decl_names(r, out); } }
        Pat::Object(props, rest) => { for (_, p) in props { decl_names(p, out); } if let Some(r) = rest { decl_names(r, out); } }
        Pat::Member(..) => {} }
}

/// VarDeclaredNames (ECMA-262 §8.2.6): `var` bindings anywhere in a function body, not inside nested functions.
/// (Dedupe through a set: a bundle's init function holds thousands of vars, and `out.contains` made this quadratic —
/// the top frame of a YouTube renderer profile, JS-COMPILE-0.)
pub fn collect_vars(body: &[Stmt], out: &mut Vec<Name>) {
    let mut seen: std::collections::HashSet<Name> = out.iter().cloned().collect();
    for st in body { var_names(st, out, &mut seen); }
}
fn var_names(st: &Stmt, out: &mut Vec<Name>, seen: &mut std::collections::HashSet<Name>) {
    fn add(p: &Pat, out: &mut Vec<Name>, seen: &mut std::collections::HashSet<Name>) { let mut v = Vec::new(); decl_names(p, &mut v); for n in v { if seen.insert(n.clone()) { out.push(n); } } }
    fn list(b: &[Stmt], out: &mut Vec<Name>, seen: &mut std::collections::HashSet<Name>) { for st in b { var_names(st, out, seen); } }
    match st {
        Stmt::Var(VarKind::Var, decls) => for (p, _) in decls { add(p, out, seen) },
        Stmt::If(_, c, a) => { var_names(c, out, seen); if let Some(a) = a { var_names(a, out, seen); } }
        Stmt::Block(b, ..) => list(b, out, seen),
        Stmt::For { init, body, .. } => { if let Some(i) = init { var_names(i, out, seen); } var_names(body, out, seen); }
        Stmt::ForOf { decl, pat, body, .. } => { if *decl == 1 { add(pat, out, seen); } var_names(body, out, seen); }
        Stmt::While(_, b) | Stmt::DoWhile(b, _) | Stmt::Labeled(_, b) | Stmt::Y(b) | Stmt::With(_, b) => var_names(b, out, seen),
        Stmt::Try { block, handler, finalizer, .. } => { list(block, out, seen); if let Some(h) = handler { list(h, out, seen); } if let Some(f) = finalizer { list(f, out, seen); } }
        Stmt::Switch(_, cases, _) => for (_, b) in cases { list(b, out, seen); },
        Stmt::ExportDecl(d, _) => var_names(d, out, seen),
        _ => {}
    }
}

// ── suspension marking (generator / async bodies) ──
fn wrap_e(e: &mut Expr) -> bool {
    let y = mark_e(e);
    if y && !matches!(e, Expr::Y(_)) { let inner = std::mem::replace(e, Expr::Lit(Lit::Undef)); *e = Expr::Y(Box::new(inner)); }
    y
}
fn wrap_s(s: &mut Stmt) -> bool {
    let y = mark_s(s);
    if y && !matches!(s, Stmt::Y(_)) { let inner = std::mem::replace(s, Stmt::Empty); *s = Stmt::Y(Box::new(inner)); }
    y
}
fn wrap_key(k: &mut Key) -> bool { match k { Key::Computed(e) => wrap_e(e), _ => false } }
fn wrap_pat(p: &mut Pat) -> bool {
    match p {
        Pat::Ident(_) => false,
        Pat::Default(p, d) => wrap_pat(p) | wrap_e(d),
        Pat::Array(els, rest) => { let mut y = false; for e in els.iter_mut().flatten() { y |= wrap_pat(e); } if let Some(r) = rest { y |= wrap_pat(r); } y }
        Pat::Object(props, rest) => { let mut y = false; for (k, p) in props.iter_mut() { y |= wrap_key(k); y |= wrap_pat(p); } if let Some(r) = rest { y |= wrap_pat(r); } y }
        Pat::Member(o, k) => wrap_e(o) | wrap_key(k),
    }
}
fn wrap_list(v: &mut [Expr]) -> bool { let mut y = false; for e in v.iter_mut() { y |= wrap_e(e); } y }
fn wrap_block(v: &mut [Stmt]) -> bool { let mut y = false; for s in v.iter_mut() { y |= wrap_s(s); } y }
fn mark_e(e: &mut Expr) -> bool {
    match e {
        Expr::Await(x) => { wrap_e(x); true }
        Expr::Yield(x, _) => { if let Some(x) = x { wrap_e(x); } true }
        Expr::Y(_) => true,
        Expr::Template(_, ex) => wrap_list(ex),
        Expr::Array(els, _) => { let mut y = false; for x in els.iter_mut().flatten() { y |= wrap_e(x); } y }
        Expr::Object(props) => { let mut y = false; for p in props.iter_mut() { y |= wrap_key(&mut p.key); if !matches!(p.kind, PropKind::Get | PropKind::Set | PropKind::Method) { y |= wrap_e(&mut p.value); } } y }
        Expr::Unary(_, a) => wrap_e(a),
        Expr::DeleteMember(o, k) => wrap_e(o) | wrap_key(k),
        Expr::Update { target, .. } => wrap_e(target),
        Expr::Binary(_, a, b) | Expr::Logical(_, a, b) => wrap_e(a) | wrap_e(b),
        Expr::Cond(t, c, a) => wrap_e(t) | wrap_e(c) | wrap_e(a),
        Expr::Assign(_, t, r) => { let ty = match &mut **t { Target::Pat(p) => wrap_pat(p), Target::Member(o, k) => wrap_e(o) | wrap_key(k), Target::Super(k) => wrap_key(k), Target::Slot(..) => false }; ty | wrap_e(r) }
        Expr::Seq(v) => wrap_list(v),
        Expr::Member { obj, key, .. } => wrap_e(obj) | wrap_key(key),
        Expr::Call { callee, args, .. } | Expr::New { callee, args, .. } => wrap_e(callee) | wrap_list(args),
        Expr::Chain(x) => wrap_e(x),
        Expr::SuperMember(k) => wrap_key(k),
        Expr::SuperCall(a, _) => wrap_list(a),
        Expr::Tagged(t, _) => wrap_e(t),
        Expr::ImportCall(x) => wrap_e(x),
        _ => false,
    }
}
fn mark_s(s: &mut Stmt) -> bool {
    match s {
        Stmt::Expr(e) | Stmt::Throw(e) => wrap_e(e),
        Stmt::Var(_, decls) => { let mut y = false; for (p, init) in decls.iter_mut() { y |= wrap_pat(p); if let Some(i) = init { y |= wrap_e(i); } } y }
        Stmt::Return(e) => e.as_mut().map_or(false, wrap_e),
        Stmt::If(t, c, a) => wrap_e(t) | wrap_s(c) | a.as_mut().map_or(false, |a| wrap_s(a)),
        Stmt::Block(b, ..) => wrap_block(b),
        Stmt::For { init, test, update, body, .. } => init.as_mut().map_or(false, |i| wrap_s(i)) | test.as_mut().map_or(false, wrap_e) | update.as_mut().map_or(false, wrap_e) | wrap_s(body),
        Stmt::ForOf { pat, right, body, is_await, .. } => wrap_pat(pat) | wrap_e(right) | wrap_s(body) | *is_await,
        Stmt::While(t, b) => wrap_e(t) | wrap_s(b),
        Stmt::DoWhile(b, t) => wrap_s(b) | wrap_e(t),
        Stmt::Try { block, param, handler, finalizer, .. } => wrap_block(block) | param.as_mut().map_or(false, wrap_pat) | handler.as_mut().map_or(false, |h| wrap_block(h)) | finalizer.as_mut().map_or(false, |f| wrap_block(f)),
        Stmt::Switch(d, cases, _) => { let mut y = wrap_e(d); for (t, b) in cases.iter_mut() { if let Some(t) = t { y |= wrap_e(t); } y |= wrap_block(b); } y }
        Stmt::Labeled(_, b) => wrap_s(b),
        Stmt::With(o, b) => wrap_e(o) | wrap_s(b),
        Stmt::Y(_) => true,
        _ => false,
    }
}

pub fn stmt(n: &J) -> Stmt {
    match s(n, "type") {
        "ExpressionStatement" => Stmt::Expr(expr(&n["expression"])),
        "VariableDeclaration" => Stmt::Var(match s(n, "kind") { "var" => VarKind::Var, "let" => VarKind::Let, _ => VarKind::Const }, arr(&n["declarations"]).iter().map(|d| (pat(&d["id"]), if d["init"].is_null() { None } else { Some(expr(&d["init"])) })).collect()),
        "FunctionDeclaration" => Stmt::Func(nm(s(&n["id"], "name")), func(n)),
        "ClassDeclaration" => Stmt::Class(nm(s(&n["id"], "name")), class(n)),
        "ReturnStatement" => Stmt::Return(if n["argument"].is_null() { None } else { Some(expr(&n["argument"])) }),
        "IfStatement" => Stmt::If(expr(&n["test"]), Box::new(stmt(&n["consequent"])), if n["alternate"].is_null() { None } else { Some(Box::new(stmt(&n["alternate"]))) }),
        "BlockStatement" => { let scoped = arr(&n["body"]).iter().any(|x| matches!(s(x, "type"), "FunctionDeclaration" | "ClassDeclaration") || (x["type"] == "VariableDeclaration" && s(x, "kind") != "var")); Stmt::Block(stmts(arr(&n["body"])), scoped, None) }
        "ForStatement" => Stmt::For { per_iter: s(&n["init"], "kind") != "var" && !n["init"].is_null() && has_fn(&n["body"]), init: if n["init"].is_null() { None } else if n["init"]["type"] == "VariableDeclaration" { Some(Box::new(stmt(&n["init"]))) } else { Some(Box::new(Stmt::Expr(expr(&n["init"])))) }, test: if n["test"].is_null() { None } else { Some(expr(&n["test"])) }, update: if n["update"].is_null() { None } else { Some(expr(&n["update"])) }, body: Box::new(stmt(&n["body"])), lay: None },
        "ForOfStatement" | "ForInStatement" => {
            let l = &n["left"];
            let (decl, p) = if l["type"] == "VariableDeclaration" { (if s(l, "kind") == "var" { 1 } else { 2 }, pat(&l["declarations"][0]["id"])) } else { (0, pat(l)) };
            Stmt::ForOf { decl, pat: p, right: expr(&n["right"]), body: Box::new(stmt(&n["body"])), of: n["type"] == "ForOfStatement", is_await: n["await"].as_bool().unwrap_or(false), lay: None }
        }
        "WhileStatement" => Stmt::While(expr(&n["test"]), Box::new(stmt(&n["body"]))),
        "DoWhileStatement" => Stmt::DoWhile(Box::new(stmt(&n["body"])), expr(&n["test"])),
        "BreakStatement" => Stmt::Break(if n["label"].is_null() { None } else { Some(nm(s(&n["label"], "name"))) }),
        "ContinueStatement" => Stmt::Continue(if n["label"].is_null() { None } else { Some(nm(s(&n["label"], "name"))) }),
        "ThrowStatement" => Stmt::Throw(expr(&n["argument"])),
        "TryStatement" => Stmt::Try { block: stmts(arr(&n["block"]["body"])), param: if n["handler"].is_null() || n["handler"]["param"].is_null() { None } else { Some(pat(&n["handler"]["param"])) }, handler: if n["handler"].is_null() { None } else { Some(stmts(arr(&n["handler"]["body"]["body"]))) }, finalizer: if n["finalizer"].is_null() { None } else { Some(stmts(arr(&n["finalizer"]["body"]))) }, lays: [None, None, None] },
        "SwitchStatement" => Stmt::Switch(expr(&n["discriminant"]), arr(&n["cases"]).iter().map(|c| (if c["test"].is_null() { None } else { Some(expr(&c["test"])) }, stmts(arr(&c["consequent"])))).collect(), None),
        "WithStatement" => Stmt::With(expr(&n["object"]), Box::new(stmt(&n["body"]))),
        "LabeledStatement" => Stmt::Labeled(nm(s(&n["label"], "name")), Box::new(stmt(&n["body"]))),
        "EmptyStatement" | "DebuggerStatement" => Stmt::Empty,
        "ImportDeclaration" => Stmt::Import(nm(s(&n["source"], "value")), arr(&n["specifiers"]).iter().map(|sp| {
            let local = nm(s(&sp["local"], "name"));
            let k = match s(sp, "type") { "ImportDefaultSpecifier" => ImportKind::Default, "ImportNamespaceSpecifier" => ImportKind::Namespace, _ => ImportKind::Named(nm(if sp["imported"]["type"] == "Identifier" { s(&sp["imported"], "name") } else { s(&sp["imported"], "value") })) };
            (k, local)
        }).collect()),
        "ExportNamedDeclaration" => {
            let d = &n["declaration"];
            if !d.is_null() {
                let st = stmt(d);
                let mut names = Vec::new();
                match &st { Stmt::Var(_, decls) => for (p, _) in decls { decl_names(p, &mut names) }, Stmt::Func(nm_, _) | Stmt::Class(nm_, _) => names.push(nm_.clone()), _ => {} }
                return Stmt::ExportDecl(Box::new(st), names);
            }
            Stmt::ExportNames(if n["source"].is_null() { None } else { Some(nm(s(&n["source"], "value"))) }, arr(&n["specifiers"]).iter().map(|sp| (nm(if sp["local"]["type"] == "Identifier" { s(&sp["local"], "name") } else { s(&sp["local"], "value") }), nm(if sp["exported"]["type"] == "Identifier" { s(&sp["exported"], "name") } else { s(&sp["exported"], "value") }))).collect())
        }
        "ExportDefaultDeclaration" => {
            let d = &n["declaration"];
            match s(d, "type") {
                "FunctionDeclaration" | "ClassDeclaration" if !d["id"].is_null() => { let st = stmt(d); Stmt::ExportDefaultDecl(Box::new(st), Some(nm(s(&d["id"], "name")))) }
                "FunctionDeclaration" => Stmt::ExportDefault(Expr::Func(func(d))),
                "ClassDeclaration" => Stmt::ExportDefault(Expr::Class(class(d))),
                _ => Stmt::ExportDefault(expr(d)),
            }
        }
        "ExportAllDeclaration" => if n["exported"].is_null() { Stmt::ExportAll(nm(s(&n["source"], "value"))) } else { Stmt::ExportNames(Some(nm(s(&n["source"], "value"))), vec![(nm("*"), nm(if n["exported"]["type"] == "Identifier" { s(&n["exported"], "name") } else { s(&n["exported"], "value") }))]) },
        other => Stmt::Unsupported(nm(other)),
    }
}

// ── static scope resolution (JS-COMPILE-0 C1; Cytron et al. 1991, V8 scope analysis — ideas only) ──
// The walk below mirrors, construct by construct, the frames jsi.rs creates at run time (`Interp::child` in `ex`,
// `ex_loop`, `activation`, named function expressions). An identifier whose binding sits in a slotted (activation)
// frame, with only statically known frames in between, becomes `Expr::Slot(depth, slot)`; everything else keeps the
// by-name path (always correct). Not resolved: resumable (generator / async with await) bodies, bodies that mention
// `eval`, class bodies, destructuring patterns, tagged-template sites. OMR_SLOTCHECK=1 cross-checks every slot access.

fn sframe(names: Vec<Name>, parent: &Rc<SFrame>) -> Rc<SFrame> { Rc::new(SFrame { lay: Rc::new(Layout::new(names)), slotted: false, open: false, parent: Some(parent.clone()) }) }
/// a frame the interpreter will create with slots: its layout is stored on the node (`lay`)
fn slotted(names: Vec<Name>, parent: &Rc<SFrame>, lay: &mut Lay) -> Rc<SFrame> {
    let l = Rc::new(Layout::new(names)); *lay = Some(l.clone());
    Rc::new(SFrame { lay: l, slotted: true, open: false, parent: Some(parent.clone()) })
}

fn resolve_name(env: &Rc<SFrame>, name: &str) -> Option<(u32, u32)> {
    let mut d = 0u32; let mut cur = Some(env);
    while let Some(f) = cur {
        if let Some(i) = f.lay.find(name) { return if f.slotted { Some((d, i as u32)) } else { None }; }
        if f.open { return None; }
        d += 1; cur = f.parent.as_ref();
    }
    None
}

fn resolve(fb: &mut FuncBody, f: &Func) {
    if fb.resumable || fb.open || std::env::var_os("OMR_NOSLOT").is_some() { return; }
    let root = Rc::new(SFrame { lay: fb.layout.clone(), slotted: true, open: false, parent: f.env.get().cloned() });
    match &mut fb.body { Body::Block(b) => rs_block(b, &root), Body::Expr(e) => rs_e(e, &root) }
}

fn rs_block(b: &mut [Stmt], env: &Rc<SFrame>) { for st in b.iter_mut() { rs_s(st, env); } }

fn rs_s(st: &mut Stmt, env: &Rc<SFrame>) {
    match st {
        Stmt::Expr(e) | Stmt::Throw(e) => rs_e(e, env),
        Stmt::Return(e) => if let Some(e) = e { rs_e(e, env) },
        Stmt::Var(_, decls) => for (_, init) in decls.iter_mut() { if let Some(e) = init { rs_e(e, env); } },
        Stmt::Func(_, f) => f.set_env(env),
        Stmt::If(t, c, a) => { rs_e(t, env); rs_s(c, env); if let Some(a) = a { rs_s(a, env); } }
        Stmt::Block(b, scoped, lay) => if *scoped { let mut n = Vec::new(); lex_names(b, &mut n); let fr = slotted(n, env, lay); rs_block(b, &fr) } else { rs_block(b, env) },
        Stmt::For { init, test, update, body, per_iter, lay } => {
            let mut n = Vec::new();
            if let Some(i) = init { if let Stmt::Var(VarKind::Let | VarKind::Const, d) = &**i { for (p, _) in d { decl_names(p, &mut n); } } }
            let sc = slotted(n, env, lay);
            if let Some(i) = init { rs_s(i, &sc); }
            if let Some(t) = test { rs_e(t, &sc); }
            if let Some(u) = update { rs_e(u, &sc); }
            // the per-iteration frame is a copy of `sc` (same layout, one frame further down)
            if *per_iter { let it = Rc::new(SFrame { lay: sc.lay.clone(), slotted: true, open: false, parent: Some(sc.clone()) }); rs_s(body, &it); } else { rs_s(body, &sc); }
        }
        Stmt::ForOf { decl, pat, right, body, lay, .. } => {
            rs_e(right, env);
            let mut n = Vec::new(); if *decl == 2 { decl_names(pat, &mut n); }
            let it = slotted(n, env, lay); rs_s(body, &it);
        }
        Stmt::While(t, b) | Stmt::DoWhile(b, t) => { rs_e(t, env); rs_s(b, env); }
        Stmt::Try { block, param, handler, finalizer, lays } => {
            let [l0, l1, l2] = lays;
            let mut n = Vec::new(); lex_names(block, &mut n); let bf = slotted(n, env, l0); rs_block(block, &bf);
            if let Some(h) = handler { let mut n = Vec::new(); if let Some(p) = param { decl_names(p, &mut n); } lex_names(h, &mut n); let hf = slotted(n, env, l1); rs_block(h, &hf); }
            if let Some(fz) = finalizer { let mut n = Vec::new(); lex_names(fz, &mut n); let ff = slotted(n, env, l2); rs_block(fz, &ff); }
        }
        Stmt::Switch(d, cases, lay) => {
            rs_e(d, env);
            let mut n = Vec::new(); for (_, b) in cases.iter() { lex_names(b, &mut n); }
            let sf = slotted(n, env, lay);
            for (t, b) in cases.iter_mut() { if let Some(t) = t { rs_e(t, &sf); } rs_block(b, &sf); }
        }
        Stmt::Labeled(_, b) => rs_s(b, env),
        Stmt::With(o, b) => { rs_e(o, env); let wf = Rc::new(SFrame { lay: Rc::new(Layout::default()), slotted: false, open: true, parent: Some(env.clone()) }); rs_s(b, &wf); }
        _ => {}
    }
}

fn rs_key(k: &mut Key, env: &Rc<SFrame>) { if let Key::Computed(e) = k { rs_e(e, env); } }

fn rs_e(e: &mut Expr, env: &Rc<SFrame>) {
    match e {
        Expr::Ident(n) => if &**n != "eval" { if let Some((d, i)) = resolve_name(env, n) { let n = n.clone(); *e = Expr::Slot(d, i, n); } },
        Expr::Template(_, ex) | Expr::Seq(ex) => for x in ex.iter_mut() { rs_e(x, env) },
        Expr::Array(els, _) => for x in els.iter_mut().flatten() { rs_e(x, env) },
        Expr::Object(props) => for p in props.iter_mut() {
            rs_key(&mut p.key, env);
            match (&p.kind, &mut p.value) { (PropKind::Get | PropKind::Set | PropKind::Method, Expr::Func(f)) => f.set_env(env), (_, v) => rs_e(v, env) }
        },
        Expr::Func(f) => if f.arrow || f.name.is_empty() { f.set_env(env) } else { f.set_env(&sframe(vec![f.name.clone()], env)) },
        Expr::Unary(_, a) | Expr::Chain(a) | Expr::ImportCall(a) => rs_e(a, env),
        Expr::DeleteMember(o, k) => { rs_e(o, env); rs_key(k, env); }
        Expr::Update { target, .. } => rs_e(target, env),
        Expr::Binary(_, a, b) | Expr::Logical(_, a, b) => { rs_e(a, env); rs_e(b, env); }
        Expr::Cond(t, c, a) => { rs_e(t, env); rs_e(c, env); rs_e(a, env); }
        Expr::Assign(_, t, r) => {
            rs_e(r, env);
            match &mut **t {
                Target::Pat(Pat::Ident(n)) => if let Some((d, i)) = resolve_name(env, n) { let n = n.clone(); **t = Target::Slot(d, i, n); },
                Target::Member(o, k) => { rs_e(o, env); rs_key(k, env); }
                _ => {}
            }
        }
        Expr::Member { obj, key, .. } => { rs_e(obj, env); rs_key(key, env); }
        Expr::Call { callee, args, .. } | Expr::New { callee, args, .. } => { rs_e(callee, env); for a in args.iter_mut() { rs_e(a, env); } }
        Expr::SuperCall(a, _) => for x in a.iter_mut() { rs_e(x, env) },
        Expr::Tagged(t, _) => rs_e(t, env),
        _ => {}
    }
}
