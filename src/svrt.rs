// SPDX-License-Identifier: MIT OR Apache-2.0
//! svrt — Svelte components on the JS engine (SVELTE-T3). One engine for everything a component does: its script (imports,
//! props, state, functions, onMount fetches), its reactive statements (`$:` and `$derived`, re-run after every change),
//! `$store` auto-subscriptions, view expressions, event handlers and component events — all on jsi, so the launcher's real
//! logic runs as written. The view walk emits markup for the engine; elements with handlers carry `data-ir`.
use crate::jsi::{st, Ctl, Interp, Kind, Scope, Val};
use std::io::Write;
use serde_json::Value as J;
use std::collections::HashMap;
use std::rc::Rc;

fn s<'j>(n: &'j J, k: &str) -> &'j str { n.get(k).and_then(|x| x.as_str()).unwrap_or("") }
fn arr(n: &J) -> &[J] { n.as_array().map(|v| v.as_slice()).unwrap_or(&[]) }
fn esc(t: &str) -> String { t.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;") }
fn esc_attr(t: &str) -> String { esc(t).replace('"', "&quot;") }

/// A reactive statement with the names it reads and their values when it last ran (Svelte re-runs on change only).
struct Reactive { stmt: Rc<J>, deps: Vec<String>, last: Option<Vec<Val>> }
struct Inst { scope: Scope, comp: String, reactive: Vec<Reactive>, stores: Vec<String>, last_props: HashMap<String, Val> }

/// Identifiers a statement reads (not property names, not object keys, not the assigned target of `x = …`).
fn reads(n: &J, out: &mut Vec<String>) {
    match n {
        J::Object(o) => {
            match o.get("type").and_then(|t| t.as_str()) {
                Some("Identifier") => { if let Some(nm) = o.get("name").and_then(|x| x.as_str()) { if !out.iter().any(|x| x == nm) { out.push(nm.to_string()); } } return; }
                Some("MemberExpression") => { if let Some(ob) = o.get("object") { reads(ob, out); } if o.get("computed").and_then(|c| c.as_bool()).unwrap_or(false) { if let Some(p) = o.get("property") { reads(p, out); } } return; }
                Some("Property") => { if o.get("computed").and_then(|c| c.as_bool()).unwrap_or(false) { if let Some(k) = o.get("key") { reads(k, out); } } if let Some(v) = o.get("value") { reads(v, out); } return; }
                Some("AssignmentExpression") if o.get("left").map_or(false, |l| l["type"] == "Identifier") && o.get("operator").and_then(|x| x.as_str()) == Some("=") => { if let Some(r) = o.get("right") { reads(r, out); } return; }
                _ => {}
            }
            for (k, v) in o { if k != "type" { reads(v, out); } }
        }
        J::Array(a) => for x in a { reads(x, out); },
        _ => {}
    }
}
// handlers are borrowed from the long-lived IR (`irs`): the JS engine caches lowered code by node address, so IR nodes
// must never be cloned per render (a freed clone's address would alias another node's code)
pub struct Wire<'a> { pub path: String, pub scope: Scope, pub on: Vec<(String, &'a J)> }
struct Meta<'a> { parent_scope: Scope, on: Vec<(String, &'a J)> }

pub struct Rt<'a> {
    irs: &'a HashMap<String, J>, pub it: Interp, inst: HashMap<String, Inst>, meta: HashMap<String, Meta<'a>>,
    pub wires: Vec<Wire<'a>>, pub css: Vec<String>, seen_css: std::collections::HashSet<String>, cur: Vec<(String, usize)>,
    pub exprs: usize, pub errors: usize, pub error_kinds: HashMap<String, usize>, pub elements: usize,
    handles: HashMap<String, Val>, el_seq: usize,
}

impl<'a> Rt<'a> {
    pub fn new(irs: &'a HashMap<String, J>, js_dir: Option<&str>) -> Rt<'a> {
        let mut it = Interp::new();
        crate::jsi_lib::install_fetch(&mut it);
        it.module_dir = js_dir.map(std::path::PathBuf::from);
        // browser-ish globals for a first launch: empty hash route, empty storage, a document/window that answers politely
        let store = Val::plain(vec![]);
        let ls = Val::plain(vec![
            ("getItem", { let st_ = store.clone(); Val::native("getItem", move |it, _t, a| { let k = it.to_str(&a.first().cloned().unwrap_or(Val::Undef))?; let v = it.get(&st_, &k)?; Ok(if matches!(v, Val::Undef) { Val::Null } else { v }) }) }),
            ("setItem", { let st_ = store.clone(); Val::native("setItem", move |it, _t, a| { let k = it.to_str(&a.first().cloned().unwrap_or(Val::Undef))?; let v = it.to_str(&a.get(1).cloned().unwrap_or(Val::Undef))?; it.set(&st_, &k, Val::Str(v))?; Ok(Val::Undef) }) }),
            ("removeItem", Val::native("removeItem", |_it, _t, _a| Ok(Val::Undef))),
        ]);
        let noop = Val::native("noop", |_it, _t, _a| Ok(Val::Undef));
        let location = Val::plain(vec![("hash", st("")), ("search", st("")), ("pathname", st("/")), ("href", st("http://127.0.0.1:47811/")), ("origin", st("http://127.0.0.1:47811")), ("reload", noop.clone())]);
        let doc = Val::plain(vec![("addEventListener", noop.clone()), ("removeEventListener", noop.clone()), ("querySelector", Val::native("querySelector", |_it, _t, _a| Ok(Val::Null))),
            ("getElementById", Val::native("getElementById", |_it, _t, _a| Ok(Val::Null))), ("visibilityState", st("visible")), ("hidden", Val::Bool(false)),
            ("documentElement", Val::plain(vec![("style", Val::plain(vec![("setProperty", noop.clone())])), ("dataset", Val::plain(vec![])), ("classList", Val::plain(vec![("add", noop.clone()), ("remove", noop.clone()), ("toggle", noop.clone()), ("contains", Val::native("contains", |_it, _t, _a| Ok(Val::Bool(false))))]))])),
            ("body", Val::plain(vec![("classList", Val::plain(vec![("add", noop.clone()), ("remove", noop.clone()), ("toggle", noop.clone())]))]))]);
        let win = Val::plain(vec![("location", location.clone()), ("localStorage", ls.clone()), ("addEventListener", noop.clone()), ("removeEventListener", noop.clone()),
            ("innerWidth", Val::Num(1280.0)), ("innerHeight", Val::Num(800.0)), ("matchMedia", Val::native("matchMedia", |_it, _t, _a| Ok(Val::plain(vec![("matches", Val::Bool(false)), ("addEventListener", Val::native("n", |_i, _t, _a| Ok(Val::Undef)))])))),
            ("requestAnimationFrame", noop.clone()), ("cancelAnimationFrame", noop.clone()), ("open", Val::native("open", |_it, _t, _a| Ok(Val::Null))), ("dispatchEvent", noop.clone())]);
        for (k, v) in [("window", win.clone()), ("globalThis", win), ("document", doc), ("localStorage", ls), ("location", location),
                       ("navigator", Val::plain(vec![("userAgent", st("omni-render")), ("clipboard", Val::plain(vec![])), ("onLine", Val::Bool(true))])),
                       ("URLSearchParams", Val::native("URLSearchParams", |_it, _t, _a| Ok(Val::plain(vec![("get", Val::native("get", |_i, _t, _a| Ok(Val::Null))), ("has", Val::native("has", |_i, _t, _a| Ok(Val::Bool(false))))])))),
                       ("EventSource", Val::native("EventSource", |_it, _t, _a| Ok(Val::plain(vec![("close", Val::native("close", |_i, _t, _a| Ok(Val::Undef))), ("addEventListener", Val::native("ael", |_i, _t, _a| Ok(Val::Undef)))])))),
                       ("WebSocket", Val::native("WebSocket", |_it, _t, _a| Ok(Val::plain(vec![("close", Val::native("close", |_i, _t, _a| Ok(Val::Undef))), ("send", Val::native("send", |_i, _t, _a| Ok(Val::Undef))), ("addEventListener", Val::native("ael", |_i, _t, _a| Ok(Val::Undef)))])))),
                       ("requestAnimationFrame", noop.clone()), ("ResizeObserver", Val::native("RO", |_it, _t, _a| Ok(Val::plain(vec![("observe", Val::native("o", |_i, _t, _a| Ok(Val::Undef))), ("disconnect", Val::native("d", |_i, _t, _a| Ok(Val::Undef)))])))),
                       ("IntersectionObserver", Val::native("IO", |_it, _t, _a| Ok(Val::plain(vec![("observe", Val::native("o", |_i, _t, _a| Ok(Val::Undef))), ("disconnect", Val::native("d", |_i, _t, _a| Ok(Val::Undef)))]))))] {
            it.global.borrow_mut().vars.insert(Rc::from(k), v);
        }
        Rt { irs, it, inst: HashMap::new(), meta: HashMap::new(), wires: Vec::new(), css: Vec::new(), seen_css: Default::default(), cur: Vec::new(), exprs: 0, errors: 0, error_kinds: HashMap::new(), elements: 0, handles: HashMap::new(), el_seq: 0 }
    }

    fn note_err(&mut self, e: Ctl) {
        self.errors += 1;
        let k = match e { Ctl::Throw(v) => self.it.to_str(&v).map(|x| x.chars().take(80).collect::<String>()).unwrap_or_default(), _ => "control".into() };
        *self.error_kinds.entry(k).or_default() += 1;
    }

    fn ev(&mut self, e: &J, scope: &Scope) -> Val {
        self.exprs += 1;
        match self.it.eval(e, scope) { Ok(v) => v, Err(x) => { self.note_err(x); Val::Undef } }
    }

    fn refresh_stores(&mut self, path: &str) {
        let (scope, stores) = match self.inst.get(path) { Some(i) => (i.scope.clone(), i.stores.clone()), None => return };
        for n in stores {
            if let Ok(store) = self.it.lookup(&scope, &n) {
                let v = crate::jsi_lib::store_value(&mut self.it, &store).unwrap_or(Val::Undef);
                Interp::declare(&scope, &format!("${n}"), v);
            }
        }
    }

    fn run_reactive(&mut self, path: &str) {
        let scope = match self.inst.get(path) { Some(i) => i.scope.clone(), None => return };
        let n = self.inst[path].reactive.len();
        // re-run a statement only when a value it reads changed (chains settle within a few rounds)
        for _ in 0..4 {
            let mut any = false;
            for k in 0..n {
                let deps = self.inst[path].reactive[k].deps.clone();
                let now: Vec<Val> = deps.iter().map(|d| self.it.lookup(&scope, d).unwrap_or(Val::Undef)).collect();
                // Svelte 4 safe_not_equal: an object value counts as changed (it may have been mutated in place)
                let changed = match &self.inst[path].reactive[k].last { None => true, Some(prev) => prev.len() != now.len() || prev.iter().zip(&now).any(|(a, b)| !Interp::strict_eq(a, b) || (matches!(b, Val::Obj(_)) && !b.is_callable())) };
                if !changed { continue; }
                any = true;
                let stmt = self.inst[path].reactive[k].stmt.clone(); // Rc: same node address every run
                self.inst.get_mut(path).unwrap().reactive[k].last = Some(now);
                if let Err(e) = self.it.exec(&stmt, &scope) { self.note_err(e); }
            }
            if !any { break; }
            // objects always look changed; a second round is only worth it when a primitive dep moved
            if !self.inst[path].reactive.iter().any(|r| r.last.as_ref().map_or(false, |l| l.iter().any(|v| !matches!(v, Val::Obj(_))))) { break; }
        }
    }

    /// Create (once) or update an instance; returns its scope.
    fn mount(&mut self, name: &str, props: Vec<(String, Val)>, path: &str) -> Option<Scope> {
        let irs: &'a HashMap<String, J> = self.irs;
        let ir: &'a J = irs.get(name)?;
        if let Some(c) = ir["css"].as_str() { if !c.is_empty() && self.seen_css.insert(name.to_string()) { self.css.push(c.to_string()); } }
        if self.inst.contains_key(path) {
            // props: the parent's value wins only when it changed since last time (Svelte prop ownership)
            let scope = self.inst[path].scope.clone();
            for (k, v) in props {
                let changed = self.inst[path].last_props.get(&k).map_or(true, |old| !Interp::strict_eq(old, &v) || matches!(v, Val::Obj(_)));
                if changed { Interp::declare(&scope, &k, v.clone()); self.inst.get_mut(path).unwrap().last_props.insert(k, v); }
            }
            self.refresh_stores(path);
            self.run_reactive(path);
            return Some(scope);
        }
        let scope = Interp::child(&self.it.global.clone());
        let pmap: HashMap<String, Val> = props.iter().cloned().collect();
        let body: Vec<&'a J> = arr(&ir["module_body"]).iter().chain(arr(&ir["instance_body"]).iter()).collect();
        let stores: Vec<String> = arr(&ir["stores"]).iter().filter_map(|x| x.as_str().map(|x| x.to_string())).collect();
        self.inst.insert(path.to_string(), Inst { scope: scope.clone(), comp: name.to_string(), reactive: Vec::new(), stores: stores.clone(), last_props: props.iter().cloned().collect() });
        // imports first (stores must exist before `$store` reads), then a per-instance dispatcher
        for stm in body.iter().filter(|x| x["type"] == "ImportDeclaration") { if let Err(e) = self.it.exec(*stm, &scope) { self.note_err(e); } }
        let me = path.to_string();
        let disp = Val::native("createEventDispatcher", move |_it, _t, _a| { let me = me.clone(); Ok(Val::native("dispatch", move |it, _t, a| {
            let ev = it.to_str(&a.first().cloned().unwrap_or(Val::Undef))?.to_string();
            let detail = a.get(1).cloned().unwrap_or(Val::Undef);
            DISPATCH.with(|q| q.borrow_mut().push((me.clone(), ev, detail)));
            Ok(Val::Bool(true))
        })) });
        Interp::declare(&scope, "createEventDispatcher", disp);
        self.refresh_stores(path);
        let mut reactive: Vec<J> = Vec::new();
        for stm in &body { self.it.hoist(std::slice::from_ref(*stm), &scope); }
        for &stm in &body {
            match s(stm, "type") {
                "ImportDeclaration" | "FunctionDeclaration" => {}
                "ExportNamedDeclaration" if stm["declaration"]["type"] == "VariableDeclaration" => {
                    for d in arr(&stm["declaration"]["declarations"]) {
                        let n = s(&d["id"], "name").to_string();
                        let v = match pmap.get(&n) { Some(v) => v.clone(), None => if d["init"].is_null() { Val::Undef } else { self.ev(&d["init"], &scope) } };
                        Interp::declare(&scope, &n, v);
                    }
                }
                "LabeledStatement" if s(&stm["label"], "name") == "$" => {
                    // `$: x = …` declares x; every reactive statement re-runs after changes
                    if stm["body"]["type"] == "ExpressionStatement" && stm["body"]["expression"]["type"] == "AssignmentExpression" && stm["body"]["expression"]["left"]["type"] == "Identifier" {
                        let n = s(&stm["body"]["expression"]["left"], "name");
                        if self.it.lookup(&scope, n).is_err() { Interp::declare(&scope, n, Val::Undef); }
                    }
                    reactive.push(stm["body"].clone());
                }
                "VariableDeclaration" if arr(&stm["declarations"]).iter().any(|d| d["init"]["type"] == "CallExpression" && s(&d["init"]["callee"], "name").starts_with('$')) => {
                    for d in arr(&stm["declarations"]) {
                        let rune = if d["init"]["type"] == "CallExpression" { s(&d["init"]["callee"], "name") } else { "" };
                        match rune {
                            "$state" | "$state.raw" => { let v = match arr(&d["init"]["arguments"]).first() { Some(a) => self.ev(a, &scope), None => Val::Undef }; if let Err(e) = self.it.bind(&d["id"], v, &scope) { self.note_err(e); } }
                            "$derived" => {
                                let n = s(&d["id"], "name").to_string();
                                Interp::declare(&scope, &n, Val::Undef);
                                let a = arr(&d["init"]["arguments"]).first().cloned().unwrap_or(J::Null);
                                reactive.push(serde_json::json!({"type": "ExpressionStatement", "expression": {"type": "AssignmentExpression", "operator": "=", "left": {"type": "Identifier", "name": n}, "right": a}}));
                            }
                            "$props" => {
                                let po = Val::obj(Kind::Plain);
                                for (k, v) in &props { let _ = self.it.set(&po, k, v.clone()); }
                                if let Err(e) = self.it.bind(&d["id"], po, &scope) { self.note_err(e); }
                            }
                            _ => { let v = if d["init"].is_null() { Val::Undef } else { self.ev(&d["init"], &scope) }; if let Err(e) = self.it.bind(&d["id"], v, &scope) { self.note_err(e); } }
                        }
                    }
                }
                "ExpressionStatement" if stm["expression"]["type"] == "CallExpression" && s(&stm["expression"]["callee"], "name") == "$effect" => {
                    if let Some(f) = arr(&stm["expression"]["arguments"]).first() { let fv = self.ev(f, &scope); if fv.is_callable() { if let Err(e) = self.it.call(&fv, Val::Undef, vec![]) { self.note_err(e); } } }
                }
                _ => { if let Err(e) = self.it.exec(stm, &scope) { self.note_err(e); } }
            }
        }
        self.inst.get_mut(path).unwrap().reactive = reactive.into_iter().map(|stmt| { let mut deps = Vec::new(); reads(&stmt, &mut deps); Reactive { stmt: Rc::new(stmt), deps, last: None } }).collect();
        self.refresh_stores(path);
        self.run_reactive(path);
        Some(scope)
    }

    fn child_path(&mut self) -> String { match self.cur.last_mut() { Some((p, k)) => { *k += 1; format!("{p}.{k}") } None => "0".into() } }

    pub fn component(&mut self, name: &str, props: Vec<(String, Val)>, path: &str, out: &mut String) {
        let d = DEPTH.with(|c| { c.set(c.get() + 1); c.get() });
        MAXD.with(|m| if d > m.get() { m.set(d); });
        let verbose = VERBOSE.load(std::sync::atomic::Ordering::Relaxed) || d <= 10;
        if verbose {
            eprintln!("svrt[depth={}] mount {} @ {}", d, name, path);
            let _ = std::io::stderr().flush();
        }
        if d > 200 {
            eprintln!("svrt: component nesting > 200 at path {path} (component {name}, {} instances mounted, {} mounts this pass)", self.inst.len(), MOUNTS.with(|m| m.get()));
            eprintln!("svrt: deepest chain: {}", self.cur.iter().map(|(p, _)| p.as_str()).collect::<Vec<_>>().join(" > "));
            DEPTH.with(|c| c.set(0));
            std::process::exit(70);
        }
        MOUNTS.with(|m| m.set(m.get() + 1));
        let scope = match self.mount(name, props, path) { Some(s) => s, None => { DEPTH.with(|c| c.set(c.get().saturating_sub(1))); out.push_str(&format!("<div data-comp=\"{}\"></div>", esc_attr(name))); return; } };
        let irs: &'a HashMap<String, J> = self.irs;
        let view: &'a J = &irs[name]["view"];
        self.cur.push((path.to_string(), 0));
        self.nodes(view, &scope, path, out);
        self.cur.pop();
        DEPTH.with(|c| c.set(c.get().saturating_sub(1)));
    }

    fn nodes(&mut self, list: &'a J, scope: &Scope, path: &str, out: &mut String) { for n in arr(list) { self.node(n, scope, path, out); } }

    fn parts(&mut self, v: &'a J, scope: &Scope) -> Option<String> {
        if v["k"] == "bool" { return Some(String::new()); }
        let ps: &'a [J] = arr(&v["parts"]);
        let mut out = String::new();
        for p in ps {
            if let Some(t) = p.get("t").and_then(|x| x.as_str()) { out += t; }
            else { let x = self.ev(&p["e"], scope); if ps.len() == 1 && matches!(x, Val::Undef | Val::Null | Val::Bool(false)) { return None; } out += &self.it.to_str(&x).map(|x| x.to_string()).unwrap_or_default(); }
        }
        Some(out)
    }

    fn prop_vals(&mut self, n: &'a J, scope: &Scope) -> Vec<(String, Val)> {
        let mut props = Vec::new();
        for p in arr(&n["props"]) {
            let v = &p["v"];
            let ps: &'a [J] = arr(&v["parts"]);
            let pv = if v["k"] == "bool" { Val::Bool(true) } else if ps.len() == 1 && ps[0].get("e").is_some() { self.ev(&ps[0]["e"], scope) } else { st(&self.parts(v, scope).unwrap_or_default()) };
            props.push((s(p, "name").to_string(), pv));
        }
        props
    }

    fn node(&mut self, n: &'a J, scope: &Scope, path: &str, out: &mut String) {
        match s(n, "k") {
            "text" => out.push_str(&esc(s(n, "t"))),
            "expr" => { let v = self.ev(&n["e"], scope); if !matches!(v, Val::Undef | Val::Null) { let t = self.it.to_str(&v).map(|x| x.to_string()).unwrap_or_default(); out.push_str(&esc(&t)); } }
            "html" => { let v = self.ev(&n["e"], scope); if !matches!(v, Val::Undef | Val::Null) { out.push_str(&self.it.to_str(&v).map(|x| x.to_string()).unwrap_or_default()); } }
            "frag" => self.nodes(&n["children"], scope, path, out),
            "const" => { let v = self.ev(&n["e"], scope); Interp::declare(scope, s(n, "name"), v); }
            "el" => {
                self.elements += 1;
                let tag = s(n, "tag");
                let mut classes: Vec<String> = Vec::new(); let mut attrs = String::new();
                for a in arr(&n["attrs"]) {
                    if a.get("spread").is_some() {
                        let o = self.ev(&a["spread"], scope);
                        for k in self.it.keys(&o) { let v = self.it.get(&o, &k).unwrap_or(Val::Undef); if !matches!(v, Val::Undef | Val::Null | Val::Bool(false)) && !v.is_callable() { let t = self.it.to_str(&v).map(|x| x.to_string()).unwrap_or_default(); attrs += &format!(" {}=\"{}\"", k, esc_attr(&t)); } }
                        continue;
                    }
                    let name = s(a, "name");
                    if let Some(v) = self.parts(&a["v"], scope) { if name == "class" { classes.extend(v.split_whitespace().map(|x| x.to_string())); } else { attrs += &format!(" {}=\"{}\"", name, esc_attr(&v)); } }
                }
                for c in arr(&n["cls"]) { if self.ev(&c["e"], scope).truthy() { classes.push(s(c, "name").to_string()); } }
                let mut style = String::new();
                for sd in arr(&n["style"]) { if !sd["v"].is_null() { if let Some(v) = self.parts(&sd["v"], scope) { style += &format!("{}:{};", s(sd, "name"), v); } } }
                self.el_seq += 1;
                for b in arr(&n["bind"]) {
                    if s(b, "name") == "this" {
                        // a stable element handle: scripts read/write scrollTop, call scrollTo/focus; layout honours scrollTop
                        let key = format!("{path}#{}", self.el_seq);
                        let h = self.handles.entry(key).or_insert_with(|| {
                            let noop = Val::native("noop", |_i, _t, _a| Ok(Val::Undef));
                            let h = Val::plain(vec![("scrollTop", Val::Num(0.0)), ("scrollLeft", Val::Num(0.0)), ("scrollHeight", Val::Num(1e7)), ("clientHeight", Val::Num(0.0)), ("offsetHeight", Val::Num(0.0)),
                                ("focus", noop.clone()), ("blur", noop.clone()), ("scrollIntoView", noop.clone()), ("addEventListener", noop.clone()), ("removeEventListener", noop.clone()),
                                ("classList", Val::plain(vec![("add", noop.clone()), ("remove", noop.clone()), ("toggle", noop.clone()), ("contains", Val::native("c", |_i, _t, _a| Ok(Val::Bool(false))))])),
                                ("style", Val::plain(vec![("setProperty", noop.clone())])), ("dataset", Val::plain(vec![])),
                                ("getBoundingClientRect", Val::native("gbcr", |_i, _t, _a| Ok(Val::plain(vec![("top", Val::Num(0.0)), ("left", Val::Num(0.0)), ("width", Val::Num(0.0)), ("height", Val::Num(0.0)), ("bottom", Val::Num(0.0)), ("right", Val::Num(0.0))]))))]);
                            let hh = h.clone();
                            if let Val::Obj(o) = &h { o.borrow_mut().props.push((Rc::from("scrollTo"), Val::native("scrollTo", move |it, _t, a| {
                                let first = a.first().cloned().unwrap_or(Val::Undef);
                                let top = if matches!(first, Val::Obj(_)) { it.get(&first, "top")? } else { a.get(1).cloned().unwrap_or(Val::Undef) };
                                if !matches!(top, Val::Undef) { it.set(&hh, "scrollTop", top)?; }
                                Ok(Val::Undef)
                            }))); }
                            h
                        }).clone();
                        let target = &b["e"];
                        if target["type"] == "Identifier" { let _ = self.it.store_pub(target, h.clone(), scope); }
                        else if target["type"] == "MemberExpression" { let _ = self.it.store_pub(target, h.clone(), scope); }
                        let st_v = self.it.get(&h, "scrollTop").unwrap_or(Val::Num(0.0));
                        if let Val::Num(t) = st_v { if t > 0.0 { attrs += &format!(" data-scroll-top=\"{}\"", t.min(1e7) as i64); } }
                        continue;
                    }
                    let v = self.ev(&b["e"], scope);
                    match s(b, "name") { "value" if !matches!(v, Val::Undef | Val::Null) => { let t = self.it.to_str(&v).map(|x| x.to_string()).unwrap_or_default(); attrs += &format!(" value=\"{}\"", esc_attr(&t)); } "checked" if v.truthy() => attrs += " checked", _ => {} }
                }
                let on: Vec<(String, &'a J)> = arr(&n["on"]).iter().filter(|o| !o["h"].is_null()).map(|o| (s(o, "ev").to_string(), &o["h"])).collect();
                if !on.is_empty() { attrs += &format!(" data-ir=\"{}\"", self.wires.len()); self.wires.push(Wire { path: path.to_string(), scope: scope.clone(), on }); }
                if !classes.is_empty() { attrs += &format!(" class=\"{}\"", esc_attr(&classes.join(" "))); }
                if !style.is_empty() { attrs += &format!(" style=\"{}\"", esc_attr(&style)); }
                out.push_str(&format!("<{tag}{attrs}>"));
                if !matches!(tag, "input" | "img" | "br" | "hr" | "meta" | "link") { self.nodes(&n["children"], scope, path, out); out.push_str(&format!("</{tag}>")); }
            }
            "if" => {
                for b in arr(&n["branches"]) {
                    let take = b["test"].is_null() || self.ev(&b["test"], scope).truthy();
                    if take { self.nodes(&b["children"], scope, path, out); break; }
                }
            }
            "each" => {
                let list = self.ev(&n["list"], scope);
                let items = if matches!(list, Val::Undef | Val::Null) { vec![] } else { match self.it.iterate(&list) { Ok(v) => v, Err(e) => { self.note_err(e); vec![] } } };
                if items.is_empty() { self.nodes(&n["fallback"], scope, path, out); }
                for (i, item) in items.into_iter().enumerate() {
                    let sc = Interp::child(scope);
                    if !n["item"].is_null() { if let Err(e) = self.it.bind(&n["item"], item, &sc) { self.note_err(e); } }
                    if let Some(ix) = n["index"].as_str() { Interp::declare(&sc, ix, Val::Num(i as f64)); }
                    self.nodes(&n["children"], &sc, path, out);
                }
            }
            "comp" | "dyncomp" => {
                let name = if s(n, "k") == "comp" { s(n, "name").to_string() } else {
                    let t = self.ev(&n["this"], scope);
                    match t { Val::Str(x) if x.starts_with("\u{1}comp:") => x.trim_start_matches("\u{1}comp:").to_string(), _ => { out.push_str("<div data-comp=\"dynamic\"></div>"); return; } }
                };
                let props = self.prop_vals(n, scope);
                let child = self.child_path();
                let on: Vec<(String, &'a J)> = arr(&n["on"]).iter().filter(|o| !o["h"].is_null()).map(|o| (s(o, "ev").to_string(), &o["h"])).collect();
                self.meta.insert(child.clone(), Meta { parent_scope: scope.clone(), on });
                self.component(&name, props, &child, out);
            }
            _ => {}
        }
    }

    /// Fire an event on a wire; then deliver queued component events, re-run reactive statements everywhere.
    pub fn fire(&mut self, wire: usize, ev: &str) -> usize {
        let (scope, hs) = match self.wires.get(wire) { Some(w) => (w.scope.clone(), w.on.iter().filter(|(e, _)| e == ev).map(|x| x.1).collect::<Vec<&'a J>>()), None => return 0 };
        let evt = Val::plain(vec![("type", st(ev)), ("preventDefault", Val::native("pd", |_i, _t, _a| Ok(Val::Undef))), ("stopPropagation", Val::native("sp", |_i, _t, _a| Ok(Val::Undef))), ("target", Val::plain(vec![("value", st(""))])), ("currentTarget", Val::plain(vec![])), ("detail", Val::Undef)]);
        let mut ran = 0;
        for h in hs {
            let f = self.ev(h, &scope);
            if f.is_callable() { if let Err(e) = self.it.call(&f, Val::Undef, vec![evt.clone()]) { self.note_err(e); } ran += 1; }
        }
        // component events raised by dispatch()
        for _ in 0..8 {
            let q: Vec<(String, String, Val)> = DISPATCH.with(|q| std::mem::take(&mut *q.borrow_mut()));
            if q.is_empty() { break; }
            for (from, evn, detail) in q {
                if let Some(m) = self.meta.get(&from) {
                    let (ps, hs): (Scope, Vec<&'a J>) = (m.parent_scope.clone(), m.on.iter().filter(|(e, _)| *e == evn).map(|x| x.1).collect());
                    for h in hs { let f = self.ev(h, &ps); if f.is_callable() { let ce = Val::plain(vec![("type", st(&evn)), ("detail", detail.clone())]); if let Err(e) = self.it.call(&f, Val::Undef, vec![ce]) { self.note_err(e); } ran += 1; } }
                }
            }
        }
        let paths: Vec<String> = self.inst.keys().cloned().collect();
        for p in paths { self.refresh_stores(&p); self.run_reactive(&p); }
        ran
    }

    pub fn render(&mut self, root: &str) -> String {
        if VERBOSE.load(std::sync::atomic::Ordering::Relaxed) {
            eprintln!("svrt[render ENTER] root={} inst={}", root, self.inst.len());
            let _ = std::io::stderr().flush();
        }
        let mut out = String::new();
        for pass in 0..3 {
            if VERBOSE.load(std::sync::atomic::Ordering::Relaxed) {
                eprintln!("svrt[render pass={}] inst={} wires={} el_seq={}", pass, self.inst.len(), self.wires.len(), self.el_seq);
                let _ = std::io::stderr().flush();
            }
            self.wires.clear(); self.meta.clear(); self.el_seq = 0;
            out.clear();
            self.component(root, Vec::new(), "0", &mut out);
            let mut cbs: Vec<Val> = std::mem::take(&mut self.it.mount_queue);
            cbs.extend(self.it.after_update.iter().cloned());
            if VERBOSE.load(std::sync::atomic::Ordering::Relaxed) {
                eprintln!("svrt[pass={}] cbs={} mount_queue={} after_update={}", pass, cbs.len(), self.it.mount_queue.len(), self.it.after_update.len());
                let _ = std::io::stderr().flush();
            }
            if cbs.is_empty() || pass == 2 { break; }
            for f in cbs {
                if VERBOSE.load(std::sync::atomic::Ordering::Relaxed) {
                    eprintln!("svrt[pass={}] calling callback", pass);
                    let _ = std::io::stderr().flush();
                }
                if let Err(e) = self.it.call(&f, Val::Undef, vec![]) { self.note_err(e); }
            }
            let paths: Vec<String> = self.inst.keys().cloned().collect();
            if VERBOSE.load(std::sync::atomic::Ordering::Relaxed) {
                eprintln!("svrt[pass={}] reactive over {} paths", pass, paths.len());
                let _ = std::io::stderr().flush();
            }
            for p in paths { self.refresh_stores(&p); self.run_reactive(&p); }
            if self.it.mount_queue.is_empty() && pass >= 1 {
                self.wires.clear(); self.meta.clear(); self.el_seq = 0; out.clear();
                self.component(root, Vec::new(), "0", &mut out);
                break;
            }
        }
        out
    }
}

thread_local! { static DISPATCH: std::cell::RefCell<Vec<(String, String, Val)>> = const { std::cell::RefCell::new(Vec::new()) }; }
thread_local! {
    static DEPTH: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    static MAXD: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    static MOUNTS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}
static VERBOSE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

fn init_verbose() {
    if std::env::var("VERBOSE").is_ok() { VERBOSE.store(true, std::sync::atomic::Ordering::Relaxed); }
}

/// `omni-render sv-run <ir-dir> <Root> <out-dir> --js <js-ir-dir> [--css f]… [--click sel]… [--tick ms]`
pub fn cli(args: &[String]) {
    init_verbose();
    let (dir, root, outd) = (&args[0], &args[1], &args[2]);
    let (mut clicks, mut gcss, mut jsdir, mut tick) = (Vec::new(), Vec::new(), None, 0.0f64);
    let mut i = 3;
    while i < args.len() { match args[i].as_str() { "--click" => { clicks.push(args[i + 1].clone()); i += 2; } "--css" => { gcss.push(args[i + 1].clone()); i += 2; } "--js" => { jsdir = Some(args[i + 1].clone()); i += 2; } "--tick" => { tick = args[i + 1].parse().unwrap_or(0.0); i += 2; } _ => i += 1 } }
    let mut irs = HashMap::new();
    for e in std::fs::read_dir(dir).expect("ir dir").filter_map(|e| e.ok()) { let p = e.path(); if p.extension().map_or(false, |x| x == "json") { if let Ok(j) = serde_json::from_slice::<J>(&std::fs::read(&p).unwrap()) { if let Some(n) = j["component"].as_str() { irs.insert(n.to_string(), j); } } } }
    let gtext: String = gcss.iter().map(|g| std::fs::read_to_string(g).unwrap_or_default() + "\n").collect();
    let mut rt = Rt::new(&irs, jsdir.as_deref());
    let write = |rt: &Rt, body: &str, step: usize| {
        let css: String = gtext.clone() + &rt.css.join("\n");
        let d = format!("{outd}/step{step}"); std::fs::create_dir_all(&d).ok();
        let html = format!("<!doctype html><html><head><meta charset=\"utf-8\"><style>{css}</style></head><body><div id=\"app\">{body}</div></body></html>");
        std::fs::write(format!("{d}/page.html"), &html).ok();
        std::fs::write(format!("{d}/manifest.json"), "{\"url\":\"http://127.0.0.1:47811/\",\"final_url\":\"http://127.0.0.1:47811/\",\"res\":{}}").ok();
        html
    };
    let t = std::time::Instant::now();
    let mut body = rt.render(root);
    if tick > 0.0 { rt.it.run_timers(tick); let paths: Vec<String> = rt.inst.keys().cloned().collect(); for p in paths { rt.refresh_stores(&p); rt.run_reactive(&p); } body = rt.render(root); }
    let mut kinds: Vec<_> = rt.error_kinds.iter().collect(); kinds.sort_by(|a, b| b.1.cmp(a.1));
    eprintln!("step0: {} instances, {} elements, {} wires, {} expressions, {} errors, render {:.1} ms · requests {} · top errors {:?}", rt.inst.len(), rt.elements, rt.wires.len(), rt.exprs, rt.errors, t.elapsed().as_secs_f64() * 1e3, rt.it.fetch_log.len(), kinds.iter().take(6).collect::<Vec<_>>());
    let mut sess = std::env::var("OMR_INCR").ok().map(|_| crate::incr::Session::new());
    { let h = write(&rt, &body, 0); if let Some(s) = sess.as_mut() { s.frame(&h, "step0"); } }
    if std::env::var("OMR_SV_REQ").is_ok() { for r in &rt.it.fetch_log { eprintln!("  request {r}"); } }
    for (k, sel) in clicks.iter().enumerate() {
        let dom = crate::dom::Dom::parse(format!("<html><body>{body}</body></html>").as_bytes());
        let f = crate::css::ElemFacts::new(&dom);
        let parsed = match crate::css::parse_complex(&crate::css::tokenize(sel)) { Some(x) => x, None => continue };
        let hit = (0..dom.nodes.len() as u32).find(|&i| dom.nodes[i as usize].kind == crate::dom::Kind::Element && crate::css::match_parts(&dom, &f, i, &parsed.parts));
        let mut wire = None; let mut n = hit.unwrap_or(crate::dom::NONE);
        while n != crate::dom::NONE { if let Some(w) = dom.attr(n, "data-ir").and_then(|v| v.parse::<usize>().ok()) { wire = Some(w); break; } n = dom.nodes[n as usize].parent; }
        let t = std::time::Instant::now();
        let ran = wire.map_or(0, |w| rt.fire(w, "click"));
        rt.elements = 0;
        body = rt.render(root);
        eprintln!("step{}: click {sel:?} → wire {:?} · handlers run {ran} · re-render {:.1} ms · errors {}", k + 1, wire, t.elapsed().as_secs_f64() * 1e3, rt.errors);
        { let h = write(&rt, &body, k + 1); if let Some(s) = sess.as_mut() { s.frame(&h, &format!("step{}", k + 1)); } }
    }
    if let Some(s) = &sess { s.finish(&format!("{outd}/incr-session.json")); }
}
