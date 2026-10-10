// SPDX-License-Identifier: MIT OR Apache-2.0
//! SVELTE-T1 back-end: Omniglyph UI IR (JSON from tools/svelte2ir.mjs) → markup for the engine.
//! State, props and derived values are evaluated exactly (an estree subset: literals, names, member/optional chains,
//! templates, unary/binary/logical/conditional, array/object literals, a few string/array methods). Anything the IR cannot
//! know statically (stores fed by the network, handler results) is `Unknown`: an unknown test takes the else branch, an
//! unknown text renders empty — and every unknown is counted, so coverage is measured, never assumed.
use serde_json::Value as J;
use std::collections::HashMap;

#[derive(Clone, Debug, PartialEq)]
pub enum V { Undef, Null, Bool(bool), Num(f64), Str(String), Arr(Vec<V>), Obj(Vec<(String, V)>), Fn(Box<J>), Unknown }

impl V {
    fn truthy(&self) -> Option<bool> {
        Some(match self { V::Undef | V::Null => false, V::Bool(b) => *b, V::Num(n) => *n != 0.0 && !n.is_nan(), V::Str(s) => !s.is_empty(), V::Arr(_) | V::Obj(_) | V::Fn(_) => true, V::Unknown => return None })
    }
    fn to_str(&self) -> String {
        match self {
            V::Undef => "undefined".into(), V::Null => "null".into(), V::Bool(b) => b.to_string(),
            V::Num(n) => if n.fract() == 0.0 && n.abs() < 1e15 { format!("{}", *n as i64) } else { format!("{n}") },
            V::Str(s) => s.clone(), V::Arr(a) => a.iter().map(|x| if matches!(x, V::Undef | V::Null) { String::new() } else { x.to_str() }).collect::<Vec<_>>().join(","),
            V::Obj(_) => "[object Object]".into(), V::Fn(_) => "function".into(), V::Unknown => String::new(),
        }
    }
    fn num(&self) -> f64 { match self { V::Num(n) => *n, V::Bool(b) => *b as i32 as f64, V::Null => 0.0, V::Str(s) => s.trim().parse().unwrap_or(f64::NAN), _ => f64::NAN } }
    fn get(&self, k: &str) -> V {
        match self {
            V::Obj(kv) => kv.iter().find(|(a, _)| a == k).map(|x| x.1.clone()).unwrap_or(V::Undef),
            V::Arr(a) => if k == "length" { V::Num(a.len() as f64) } else { k.parse::<usize>().ok().and_then(|i| a.get(i).cloned()).unwrap_or(V::Undef) },
            V::Str(s) => if k == "length" { V::Num(s.chars().count() as f64) } else { V::Undef },
            V::Unknown => V::Unknown,
            _ => V::Undef,
        }
    }
}

#[derive(Default)]
pub struct Stats { pub exprs: usize, pub unknown: usize, pub elements: usize, pub comps: usize, pub missing_comps: usize, pub unsupported: usize }

/// A rendered element's event wiring: component instance path, the env it was rendered in, and its handlers.
pub struct Wire { pub path: String, pub comp: String, pub env: Vec<HashMap<String, V>>, pub on: Vec<(String, J)> }
/// Parent link of a component instance: where its `dispatch(ev)` lands.
pub struct Meta { pub parent: String, pub parent_comp: String, pub parent_env: Vec<HashMap<String, V>>, pub on: Vec<(String, J)> }

pub struct Ctx<'a> { pub irs: &'a HashMap<String, J>, pub stats: Stats, pub unsupported_kinds: HashMap<String, usize>,
    pub js: Option<crate::jsi::Interp>,
    pub inst: HashMap<String, HashMap<String, V>>, last_props: HashMap<String, V>, cur: Vec<(String, String, usize)>, pub wires: Vec<Wire>, pub meta: HashMap<String, Meta>, pub host_calls: usize, pub ran: usize, pub css: Vec<String>, seen_css: std::collections::HashSet<String> }

fn s<'j>(n: &'j J, k: &str) -> &'j str { n.get(k).and_then(|x| x.as_str()).unwrap_or("") }

fn eval(e: &J, env: &[HashMap<String, V>]) -> V {
    let look = |name: &str| -> V { for f in env.iter().rev() { if let Some(v) = f.get(name) { return v.clone(); } } match name { "undefined" => V::Undef, "NaN" => V::Num(f64::NAN), "Infinity" => V::Num(f64::INFINITY), _ => V::Unknown } };
    match s(e, "type") {
        "Literal" => match &e["value"] { J::Null => V::Null, J::Bool(b) => V::Bool(*b), J::Number(n) => V::Num(n.as_f64().unwrap_or(0.0)), J::String(x) => V::Str(x.clone()), _ => V::Unknown },
        "Identifier" => look(s(e, "name")),
        "TemplateLiteral" => {
            let q = e["quasis"].as_array().cloned().unwrap_or_default();
            let x = e["expressions"].as_array().cloned().unwrap_or_default();
            let mut out = String::new();
            for (i, qq) in q.iter().enumerate() {
                out += qq["value"]["cooked"].as_str().unwrap_or("");
                if let Some(ex) = x.get(i) { let v = eval(ex, env); if v == V::Unknown { return V::Unknown; } out += &v.to_str(); }
            }
            V::Str(out)
        }
        "ChainExpression" => eval(&e["expression"], env),
        "MemberExpression" => {
            let o = eval(&e["object"], env);
            if matches!(o, V::Undef | V::Null) { return if e["optional"].as_bool().unwrap_or(false) { V::Undef } else { V::Unknown }; }
            let key = if e["computed"].as_bool().unwrap_or(false) { eval(&e["property"], env).to_str() } else { s(&e["property"], "name").to_string() };
            o.get(&key)
        }
        "UnaryExpression" => {
            let a = eval(&e["argument"], env);
            match s(e, "operator") {
                "!" => a.truthy().map(|b| V::Bool(!b)).unwrap_or(V::Unknown),
                "-" => if a == V::Unknown { V::Unknown } else { V::Num(-a.num()) },
                "+" => if a == V::Unknown { V::Unknown } else { V::Num(a.num()) },
                "typeof" => V::Str(match a { V::Undef => "undefined", V::Fn(_) => "function", V::Null | V::Obj(_) | V::Arr(_) => "object", V::Bool(_) => "boolean", V::Num(_) => "number", V::Str(_) => "string", V::Unknown => return V::Unknown }.into()),
                _ => V::Unknown,
            }
        }
        "LogicalExpression" => {
            let a = eval(&e["left"], env);
            match s(e, "operator") {
                "&&" => match a.truthy() { Some(false) => a, Some(true) => eval(&e["right"], env), None => V::Unknown },
                "||" => match a.truthy() { Some(true) => a, Some(false) => eval(&e["right"], env), None => V::Unknown },
                "??" => match a { V::Undef | V::Null => eval(&e["right"], env), V::Unknown => V::Unknown, x => x },
                _ => V::Unknown,
            }
        }
        "ConditionalExpression" => match eval(&e["test"], env).truthy() { Some(true) => eval(&e["consequent"], env), Some(false) => eval(&e["alternate"], env), None => V::Unknown },
        "BinaryExpression" => {
            let (a, b) = (eval(&e["left"], env), eval(&e["right"], env));
            if a == V::Unknown || b == V::Unknown { return V::Unknown; }
            match s(e, "operator") {
                "===" | "==" => V::Bool(a == b || (matches!(a, V::Num(_)) && matches!(b, V::Num(_)) && a.num() == b.num())),
                "!==" | "!=" => V::Bool(!(a == b || (matches!(a, V::Num(_)) && matches!(b, V::Num(_)) && a.num() == b.num()))),
                "+" => if matches!(a, V::Str(_)) || matches!(b, V::Str(_)) { V::Str(a.to_str() + &b.to_str()) } else { V::Num(a.num() + b.num()) },
                "-" => V::Num(a.num() - b.num()), "*" => V::Num(a.num() * b.num()), "/" => V::Num(a.num() / b.num()), "%" => V::Num(a.num() % b.num()),
                "in" => V::Bool(!matches!(b.get(&a.to_str()), V::Undef)),
                "<" => V::Bool(a.num() < b.num()), ">" => V::Bool(a.num() > b.num()), "<=" => V::Bool(a.num() <= b.num()), ">=" => V::Bool(a.num() >= b.num()),
                _ => V::Unknown,
            }
        }
        "NewExpression" if s(&e["callee"], "name") == "URLSearchParams" => {
            let q = e["arguments"].as_array().and_then(|a| a.first()).map(|a| eval(a, env)).unwrap_or(V::Str(String::new()));
            if q == V::Unknown { V::Unknown } else { V::Obj(vec![("\u{1}params".into(), V::Str(q.to_str().trim_start_matches('?').to_string()))]) }
        }
        "ArrayExpression" => { let mut v = Vec::new(); for x in e["elements"].as_array().into_iter().flatten() { v.push(eval(x, env)); } V::Arr(v) }
        "ObjectExpression" => {
            let mut kv = Vec::new();
            for p in e["properties"].as_array().into_iter().flatten() {
                if s(p, "type") != "Property" { return V::Unknown; }
                let k = if p["key"]["type"] == "Identifier" && !p["computed"].as_bool().unwrap_or(false) { s(&p["key"], "name").to_string() } else { eval(&p["key"], env).to_str() };
                kv.push((k, eval(&p["value"], env)));
            }
            V::Obj(kv)
        }
        "ArrowFunctionExpression" | "FunctionExpression" => V::Fn(Box::new(e.clone())),
        "CallExpression" if e["callee"]["type"] == "Identifier" && matches!(look(s(&e["callee"], "name")), V::Fn(_)) => {
            // calling a function VALUE (e.g. `$: visible = (k) => …`): pure evaluation of its body in a new frame
            let f = match look(s(&e["callee"], "name")) { V::Fn(f) => f, _ => unreachable!() };
            let mut frame = HashMap::new();
            for (i, p) in f["params"].as_array().into_iter().flatten().enumerate() {
                if p["type"] == "Identifier" { frame.insert(s(p, "name").to_string(), e["arguments"].get(i).map(|a| eval(a, env)).unwrap_or(V::Undef)); }
            }
            let mut env2: Vec<HashMap<String, V>> = env.to_vec();
            env2.push(frame);
            if f["body"]["type"] == "BlockStatement" {
                // a single `return expr;` is the only block form evaluated purely
                let body = f["body"]["body"].as_array().cloned().unwrap_or_default();
                match body.as_slice() { [r] if r["type"] == "ReturnStatement" => eval(&r["argument"], &env2), _ => V::Unknown }
            } else { eval(&f["body"], &env2) }
        }
        "CallExpression" => {
            // string / array methods with exact semantics; anything else is a host call → unknown at build time
            let c = &e["callee"];
            let args: Vec<V> = e["arguments"].as_array().into_iter().flatten().map(|x| eval(x, env)).collect();
            if s(c, "type") == "Identifier" && s(c, "name") == "String" { return args.first().map(|a| V::Str(a.to_str())).unwrap_or(V::Str(String::new())); }
            if s(c, "type") == "MemberExpression" && !c["computed"].as_bool().unwrap_or(false) {
                let o = eval(&c["object"], env);
                let m = s(&c["property"], "name");
                if matches!(o, V::Undef | V::Null) && c["optional"].as_bool().unwrap_or(false) { return V::Undef; }
                match (&o, m) {
                    (V::Str(x), "toUpperCase") => return V::Str(x.to_uppercase()),
                    (V::Str(x), "toLowerCase") => return V::Str(x.to_lowercase()),
                    (V::Str(x), "trim") => return V::Str(x.trim().into()),
                    (V::Str(x), "includes") => return V::Bool(args.first().map_or(false, |a| x.contains(&a.to_str()))),
                    (V::Str(x), "startsWith") => return V::Bool(args.first().map_or(false, |a| x.starts_with(&a.to_str()))),
                    (V::Str(x), "slice") => { let ch: Vec<char> = x.chars().collect(); let n = ch.len() as i64; let fix = |v: f64| { let v = v as i64; (if v < 0 { (n + v).max(0) } else { v.min(n) }) as usize }; let a = args.first().map(|a| fix(a.num())).unwrap_or(0); let b = args.get(1).map(|b| fix(b.num())).unwrap_or(n as usize); return V::Str(ch[a.min(b)..b].iter().collect()); }
                    (V::Str(x), "replace") => {
                        // regex literal of the anchored "^c+" / "c+$" kind, or a plain string — the forms seen in app code
                        let a0 = &e["arguments"][0];
                        let to = args.get(1).map(|v| v.to_str()).unwrap_or_default();
                        if let Some(pat) = a0["regex"]["pattern"].as_str() {
                            let lit = pat.trim_start_matches('^').trim_end_matches('$').trim_end_matches('+').replace('\\', "");
                            if lit.chars().count() == 1 {
                                let c = lit.chars().next().unwrap();
                                if pat.starts_with('^') { let t = x.trim_start_matches(c); return V::Str(if t.len() < x.len() { format!("{to}{t}") } else { x.clone() }); }
                                if pat.ends_with('$') { return V::Str(x.trim_end_matches(c).to_string()); }
                            }
                            return V::Unknown;
                        }
                        return V::Str(x.replacen(&args.first().map(|v| v.to_str()).unwrap_or_default(), &to, 1));
                    }
                    (V::Obj(kv), "get") if kv.first().map_or(false, |x| x.0 == "\u{1}params") => {
                        let q = kv[0].1.to_str(); let want = args.first().map(|a| a.to_str()).unwrap_or_default();
                        return q.split('&').filter_map(|p| p.split_once('=').or(Some((p, "")))).find(|(k, _)| *k == want).map(|(_, v)| V::Str(v.to_string())).unwrap_or(V::Null);
                    }
                    (V::Obj(_), "getItem") => return V::Null, // localStorage on first launch: empty
                    (V::Arr(x), "includes") => return V::Bool(args.first().map_or(false, |a| x.contains(a))),
                    (V::Arr(x), "join") => return V::Str(x.iter().map(|v| v.to_str()).collect::<Vec<_>>().join(&args.first().map(|a| a.to_str()).unwrap_or(",".into()))),
                    (V::Num(x), "toFixed") => return V::Str(format!("{:.*}", args.first().map(|a| a.num() as usize).unwrap_or(0), x)),
                    _ => {}
                }
            }
            V::Unknown
        }
        _ => V::Unknown,
    }
}

fn esc(t: &str) -> String { t.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;") }
fn esc_attr(t: &str) -> String { esc(t).replace('"', "&quot;") }

impl<'a> Ctx<'a> {
    pub fn new(irs: &'a HashMap<String, J>) -> Ctx<'a> { Ctx { irs, stats: Stats::default(), unsupported_kinds: HashMap::new(), js: None, inst: HashMap::new(), last_props: HashMap::new(), cur: Vec::new(), wires: Vec::new(), meta: HashMap::new(), host_calls: 0, ran: 0, css: Vec::new(), seen_css: Default::default() } }

    fn val(&mut self, e: &J, env: &[HashMap<String, V>]) -> V { self.stats.exprs += 1; let v = eval(e, env); if v == V::Unknown { self.stats.unknown += 1; } v }

    fn parts(&mut self, v: &J, env: &[HashMap<String, V>]) -> Option<String> {
        if v["k"] == "bool" { return Some(String::new()); }
        let mut out = String::new();
        for p in v["parts"].as_array().into_iter().flatten() {
            if let Some(t) = p.get("t").and_then(|x| x.as_str()) { out += t; } else { let x = self.val(&p["e"], env); if matches!(x, V::Undef | V::Null | V::Bool(false)) && v["parts"].as_array().map_or(0, |a| a.len()) == 1 { return None; } out += &x.to_str(); }
        }
        Some(out)
    }

    /// Instantiate a component: its own scope frame (props from the parent, then state, then derived), then its view.
    pub fn component(&mut self, name: &str, props: Vec<(String, V)>, out: &mut String) { self.component_at(name, props, "0", out) }

    pub fn component_at(&mut self, name: &str, props: Vec<(String, V)>, path: &str, out: &mut String) {
        let ir = match self.irs.get(name) { Some(x) => x.clone(), None => { self.stats.missing_comps += 1; out.push_str(&format!("<div data-comp=\"{}\"></div>", esc_attr(name))); return; } };
        self.stats.comps += 1;
        if let Some(c) = ir["css"].as_str() { if !c.is_empty() && self.seen_css.insert(name.to_string()) { self.css.push(c.to_string()); } }
        let mut frame: HashMap<String, V> = HashMap::new();
        // host frame for a first launch: no hash route, empty storage
        let loc = V::Obj(vec![("hash".into(), V::Str(String::new())), ("pathname".into(), V::Str("/".into())), ("search".into(), V::Str(String::new()))]);
        let host: HashMap<String, V> = [("window".to_string(), V::Obj(vec![("location".into(), loc.clone()), ("localStorage".into(), V::Obj(vec![("__store".into(), V::Bool(true))]))])),
            ("location".to_string(), loc), ("localStorage".to_string(), V::Obj(vec![("__store".into(), V::Bool(true))]))].into_iter().collect();
        let mut env = vec![host, frame.clone()];
        // imported components are values (for <svelte:component this={…}> and route maps)
        for im in ir["imports"].as_array().into_iter().flatten() {
            if s(im, "from").ends_with(".svelte") { for nm in im["names"].as_array().into_iter().flatten() { if let Some(x) = nm.as_str() { frame.insert(x.to_string(), V::Str(format!("\u{1}comp:{x}"))); env[1] = frame.clone(); } } }
        }
        for p in ir["props"].as_array().into_iter().flatten() {
            let n = s(p, "name").to_string();
            let passed = props.iter().find(|(k, _)| *k == n).map(|x| x.1.clone());
            let key = format!("{path}\u{1}{n}");
            // a component's own assignment to a prop holds until the parent passes a DIFFERENT value (Svelte semantics)
            let own = self.inst.get(path).and_then(|fr| fr.get(&n)).cloned();
            let v = match (&passed, own) {
                (Some(pv), Some(ov)) if self.last_props.get(&key) == Some(pv) => ov,
                (Some(pv), _) => pv.clone(),
                (None, Some(ov)) => ov,
                (None, None) => if p["init"].is_null() { V::Undef } else { eval(&p["init"], &env) },
            };
            if let Some(pv) = passed { self.last_props.insert(key, pv); }
            frame.insert(n, v); env[1] = frame.clone();
        }
        if let Some(saved) = self.inst.get(path) {
            // state persists across renders; props were refreshed above
            for st in ir["state"].as_array().into_iter().flatten() { let n = s(st, "name"); if let Some(v) = saved.get(n) { frame.insert(n.to_string(), v.clone()); } }
            env[1] = frame.clone();
        } else {
            for st in ir["state"].as_array().into_iter().flatten() { let v = if st["init"].is_null() { V::Undef } else { eval(&st["init"], &env) }; frame.insert(s(st, "name").to_string(), v); env[1] = frame.clone(); }
        }
        for _ in 0..2 { for d in ir["derived"].as_array().into_iter().flatten() { let v = eval(&d["e"], &env); frame.insert(s(d, "name").to_string(), v); env[1] = frame.clone(); } }
        // data modules (.js) run on the JS engine; their stores feed `$name`, plain exports feed `name`
        if self.js.is_some() {
            for im in ir["imports"].as_array().into_iter().flatten() {
                let from = s(im, "from");
                if !(from.ends_with(".js") || from.starts_with("svelte/store")) { continue; }
                let names: Vec<String> = im["names"].as_array().into_iter().flatten().filter_map(|x| x.as_str().map(|x| x.to_string())).collect();
                for (n, val, store) in self.js_values(from, &names) {
                    if let Some(sv) = store { frame.insert(format!("${n}"), sv); }
                    frame.entry(n).or_insert(val);
                }
                env[1] = frame.clone();
            }
        }
        for sname in ir["stores"].as_array().into_iter().flatten() { if let Some(n) = sname.as_str() { frame.entry(format!("${n}")).or_insert(V::Unknown); } }
        env[1] = frame.clone();
        self.inst.insert(path.to_string(), frame);
        let view = ir["view"].clone();
        self.cur.push((path.to_string(), name.to_string(), 0));
        self.nodes(&view, &mut env, out);
        self.cur.pop();
    }

    fn nodes(&mut self, list: &J, env: &mut Vec<HashMap<String, V>>, out: &mut String) { for n in list.as_array().into_iter().flatten() { self.node(n, env, out); } }

    fn node(&mut self, n: &J, env: &mut Vec<HashMap<String, V>>, out: &mut String) {
        match s(n, "k") {
            "text" => out.push_str(&esc(s(n, "t"))),
            "expr" => { let v = self.val(&n["e"], env); if !matches!(v, V::Undef | V::Null) { out.push_str(&esc(&v.to_str())); } }
            "html" => { let v = self.val(&n["e"], env); out.push_str(&v.to_str()); }
            "frag" => self.nodes(&n["children"], env, out),
            "const" => { let v = self.val(&n["e"], env); if let Some(f) = env.last_mut() { f.insert(s(n, "name").to_string(), v); } }
            "el" => {
                self.stats.elements += 1;
                let tag = s(n, "tag");
                let mut classes: Vec<String> = Vec::new();
                let mut attrs = String::new();
                for a in n["attrs"].as_array().into_iter().flatten() {
                    if a.get("spread").is_some() { continue; }
                    let name = s(a, "name");
                    if let Some(v) = self.parts(&a["v"], env) { if name == "class" { classes.extend(v.split_whitespace().map(|x| x.to_string())); } else { attrs += &format!(" {}=\"{}\"", name, esc_attr(&v)); } }
                }
                for c in n["cls"].as_array().into_iter().flatten() { if self.val(&c["e"], env).truthy() == Some(true) { classes.push(s(c, "name").to_string()); } }
                let mut style = String::new();
                for st in n["style"].as_array().into_iter().flatten() { if let Some(v) = (!st["v"].is_null()).then(|| self.parts(&st["v"], env)).flatten() { style += &format!("{}:{};", s(st, "name"), v); } }
                for b in n["bind"].as_array().into_iter().flatten() { if s(b, "name") == "value" { let v = self.val(&b["e"], env); if !matches!(v, V::Undef | V::Null | V::Unknown) { attrs += &format!(" value=\"{}\"", esc_attr(&v.to_str())); } } }
                let on: Vec<(String, J)> = n["on"].as_array().into_iter().flatten().filter(|o| !o["h"].is_null()).map(|o| (s(o, "ev").to_string(), o["h"].clone())).collect();
                if !on.is_empty() {
                    let (path, comp, _) = self.cur.last().cloned().unwrap_or_default();
                    attrs += &format!(" data-ir=\"{}\"", self.wires.len());
                    self.wires.push(Wire { path, comp, env: env.clone(), on });
                }
                if !classes.is_empty() { attrs += &format!(" class=\"{}\"", esc_attr(&classes.join(" "))); }
                if !style.is_empty() { attrs += &format!(" style=\"{}\"", esc_attr(&style)); }
                out.push_str(&format!("<{tag}{attrs}>"));
                if !matches!(tag, "input" | "img" | "br" | "hr" | "meta" | "link") { self.nodes(&n["children"], env, out); out.push_str(&format!("</{tag}>")); }
            }
            "if" => {
                for b in n["branches"].as_array().into_iter().flatten() {
                    let take = if b["test"].is_null() { true } else { self.val(&b["test"], env).truthy().unwrap_or(false) };
                    if take { self.nodes(&b["children"], env, out); break; }
                }
            }
            "each" => {
                let list = self.val(&n["list"], env);
                match list {
                    V::Arr(items) if !items.is_empty() => {
                        for (i, it) in items.into_iter().enumerate() {
                            let mut f = HashMap::new();
                            if n["item"]["type"] == "Identifier" { f.insert(s(&n["item"], "name").to_string(), it); }
                            if let Some(ix) = n["index"].as_str() { f.insert(ix.to_string(), V::Num(i as f64)); }
                            env.push(f); self.nodes(&n["children"], env, out); env.pop();
                        }
                    }
                    _ => self.nodes(&n["fallback"], env, out),
                }
            }
            "comp" => {
                let mut props = Vec::new();
                for p in n["props"].as_array().into_iter().flatten() {
                    let v = &p["v"];
                    let pv = if v["k"] == "bool" { V::Bool(true) } else { let parts = v["parts"].as_array().cloned().unwrap_or_default(); if parts.len() == 1 && parts[0].get("e").is_some() { self.val(&parts[0]["e"], env) } else { V::Str(self.parts(v, env).unwrap_or_default()) } };
                    props.push((s(p, "name").to_string(), pv));
                }
                let name = s(n, "name").to_string();
                let child = self.child_path();
                let (pp, pc, _) = self.cur.last().cloned().unwrap_or_default();
                let on: Vec<(String, J)> = n["on"].as_array().into_iter().flatten().filter(|o| !o["h"].is_null()).map(|o| (s(o, "ev").to_string(), o["h"].clone())).collect();
                self.meta.insert(child.clone(), Meta { parent: pp, parent_comp: pc, parent_env: env.clone(), on });
                self.component_at(&name, props, &child, out);
            }
            "dyncomp" => {
                let target = self.val(&n["this"], env);
                if std::env::var("OMR_IR_DEBUG").is_ok() { eprintln!("dyncomp this = {:?}; route = {:?}; routes.triad = {:?}", target, eval(&serde_json::json!({"type":"Identifier","name":"route"}), env), eval(&serde_json::json!({"type":"Identifier","name":"routes"}), env).get("triad")); }
                match target {
                    V::Str(t) if t.starts_with("\u{1}comp:") => {
                        let name = t.trim_start_matches("\u{1}comp:").to_string();
                        let mut props = Vec::new();
                        for p in n["props"].as_array().into_iter().flatten() { let v = &p["v"]; let parts = v["parts"].as_array().cloned().unwrap_or_default(); let pv = if v["k"] == "bool" { V::Bool(true) } else if parts.len() == 1 && parts[0].get("e").is_some() { self.val(&parts[0]["e"], env) } else { V::Str(self.parts(v, env).unwrap_or_default()) }; props.push((s(p, "name").to_string(), pv)); }
                        let child = self.child_path();
                        self.component_at(&name, props, &child, out);
                    }
                    _ => { self.stats.missing_comps += 1; out.push_str("<div data-comp=\"dynamic\"></div>"); }
                }
            }
            other => { self.stats.unsupported += 1; *self.unsupported_kinds.entry(format!("{other}:{}", s(n, "what"))).or_default() += 1; }
        }
    }
}

fn json_to_v(j: &J) -> V {
    match j { J::Null => V::Null, J::Bool(b) => V::Bool(*b), J::Number(n) => V::Num(n.as_f64().unwrap_or(0.0)), J::String(x) => V::Str(x.clone()),
        J::Array(a) => V::Arr(a.iter().map(json_to_v).collect()), J::Object(o) => V::Obj(o.iter().map(|(k, v)| (k.clone(), json_to_v(v))).collect()) }
}

impl<'a> Ctx<'a> {
    /// Values of `names` exported by a data module, through the JS engine: (name, value, store value if a store).
    fn js_values(&mut self, from: &str, names: &[String]) -> Vec<(String, V, Option<V>)> {
        let it = match self.js.as_mut() { Some(i) => i, None => return vec![] };
        let m = match it.import(from) { Ok(m) => m, Err(_) => return names.iter().map(|n| (n.clone(), V::Unknown, Some(V::Unknown))).collect() };
        let mut out = Vec::new();
        for n in names {
            let v = it.get(&m, n).unwrap_or(crate::jsi::Val::Undef);
            let is_store = matches!(&v, crate::jsi::Val::Obj(_)) && it.get(&v, "subscribe").map_or(false, |f| f.is_callable());
            let conv = |it: &mut crate::jsi::Interp, x: &crate::jsi::Val| -> V { if x.is_callable() { return V::Unknown; } serde_json::from_str::<J>(&crate::jsi_lib::stringify_val(it, x)).map(|j| json_to_v(&j)).unwrap_or(V::Unknown) };
            let store = if is_store { let sv = crate::jsi_lib::store_value(it, &v).unwrap_or(crate::jsi::Val::Undef); Some(conv(it, &sv)) } else { None };
            let plain = if is_store { V::Unknown } else { conv(it, &v) };
            out.push((n.clone(), plain, store));
        }
        out
    }

    fn child_path(&mut self) -> String {
        match self.cur.last_mut() { Some((p, _, k)) => { *k += 1; format!("{p}.{k}") } None => "0".into() }
    }

    /// Run an event: element `wire` index, event name. Returns true when some state changed.
    pub fn fire(&mut self, wire: usize, ev: &str) -> bool {
        let (path, comp, mut env, handlers) = match self.wires.get(wire) { Some(w) => (w.path.clone(), w.comp.clone(), w.env.clone(), w.on.clone()), None => return false };
        let mut changed = false;
        for (e, h) in handlers { if e == ev { changed |= self.call_value(&h, &path, &comp, &mut env, &[V::Unknown]); } }
        changed
    }

    /// Invoke a handler value: a named function, an arrow, or a call expression.
    fn call_value(&mut self, h: &J, path: &str, comp: &str, env: &mut Vec<HashMap<String, V>>, args: &[V]) -> bool {
        self.ran += 1;
        match s(h, "type") {
            "Identifier" => self.call_named(s(h, "name"), path, comp, env, args),
            "ArrowFunctionExpression" | "FunctionExpression" => {
                let mut f = HashMap::new();
                for (i, p) in h["params"].as_array().into_iter().flatten().enumerate() { if p["type"] == "Identifier" { f.insert(s(p, "name").to_string(), args.get(i).cloned().unwrap_or(V::Undef)); } }
                env.push(f);
                let ch = if h["body"]["type"] == "BlockStatement" { self.exec_block(&h["body"], path, comp, env) } else { self.expr_stmt(&h["body"], path, comp, env) };
                env.pop();
                ch
            }
            _ => self.expr_stmt(h, path, comp, env),
        }
    }

    fn call_named(&mut self, name: &str, path: &str, comp: &str, env: &mut Vec<HashMap<String, V>>, args: &[V]) -> bool {
        let ir = match self.irs.get(comp) { Some(x) => x.clone(), None => return false };
        if let Some(hd) = ir["handlers"].as_array().into_iter().flatten().find(|x| s(x, "name") == name) {
            let mut f = HashMap::new();
            for (i, p) in hd["params"].as_array().into_iter().flatten().enumerate() { if p["type"] == "Identifier" { f.insert(s(p, "name").to_string(), args.get(i).cloned().unwrap_or(V::Undef)); } }
            env.push(f);
            let ch = self.exec_block(&hd["body"], path, comp, env);
            env.pop();
            return ch;
        }
        if name == "dispatch" {
            // component event: run the parent's on:<ev> handler in the parent's scope
            let ev = args.first().map(|a| a.to_str()).unwrap_or_default();
            if let Some(m) = self.meta.get(path) {
                let (pp, pc, mut penv) = (m.parent.clone(), m.parent_comp.clone(), m.parent_env.clone());
                let hs: Vec<J> = m.on.iter().filter(|(e, _)| *e == ev).map(|x| x.1.clone()).collect();
                let mut ch = false;
                for h in hs { ch |= self.call_value(&h, &pp, &pc, &mut penv, &[V::Unknown]); }
                return ch;
            }
            return false;
        }
        self.host_calls += 1; // fetch / setTimeout / store set … — the host table (T3)
        false
    }

    fn exec_block(&mut self, b: &J, path: &str, comp: &str, env: &mut Vec<HashMap<String, V>>) -> bool {
        let mut ch = false;
        env.push(HashMap::new());
        for st in b["body"].as_array().into_iter().flatten() {
            match self.exec(st, path, comp, env) { Flow::Next(c) => ch |= c, Flow::Return(c) => { ch |= c; break; } }
        }
        env.pop();
        ch
    }

    fn exec(&mut self, st: &J, path: &str, comp: &str, env: &mut Vec<HashMap<String, V>>) -> Flow {
        match s(st, "type") {
            "ExpressionStatement" => Flow::Next(self.expr_stmt(&st["expression"], path, comp, env)),
            "BlockStatement" => Flow::Next(self.exec_block(st, path, comp, env)),
            "IfStatement" => match eval(&st["test"], env).truthy() {
                Some(true) => self.exec(&st["consequent"], path, comp, env),
                Some(false) => if st["alternate"].is_null() { Flow::Next(false) } else { self.exec(&st["alternate"], path, comp, env) },
                None => Flow::Next(false),
            },
            "ReturnStatement" => Flow::Return(if st["argument"].is_null() { false } else { self.expr_stmt(&st["argument"], path, comp, env) }),
            "VariableDeclaration" => {
                for d in st["declarations"].as_array().into_iter().flatten() { if d["id"]["type"] == "Identifier" { let v = if d["init"].is_null() { V::Undef } else { eval(&d["init"], env) }; if let Some(f) = env.last_mut() { f.insert(s(&d["id"], "name").to_string(), v); } } }
                Flow::Next(false)
            }
            "TryStatement" => Flow::Next(self.exec_block(&st["block"], path, comp, env)),
            _ => Flow::Next(false),
        }
    }

    /// Expression in statement position: assignments and calls have effects; the rest is evaluated and dropped.
    fn expr_stmt(&mut self, e: &J, path: &str, comp: &str, env: &mut Vec<HashMap<String, V>>) -> bool {
        match s(e, "type") {
            "AssignmentExpression" if e["left"]["type"] == "Identifier" => {
                let name = s(&e["left"], "name").to_string();
                let rhs = eval(&e["right"], env);
                let cur = eval(&e["left"], env);
                let v = match s(e, "operator") {
                    "=" => rhs, "+=" => if matches!(cur, V::Str(_)) || matches!(rhs, V::Str(_)) { V::Str(cur.to_str() + &rhs.to_str()) } else { V::Num(cur.num() + rhs.num()) },
                    "-=" => V::Num(cur.num() - rhs.num()), _ => V::Unknown,
                };
                self.assign(&name, v, path, env)
            }
            "UpdateExpression" if e["argument"]["type"] == "Identifier" => {
                let name = s(&e["argument"], "name").to_string();
                let cur = eval(&e["argument"], env).num();
                self.assign(&name, V::Num(if s(e, "operator") == "++" { cur + 1.0 } else { cur - 1.0 }), path, env)
            }
            "CallExpression" => {
                let args: Vec<V> = e["arguments"].as_array().into_iter().flatten().map(|a| eval(a, env)).collect();
                if e["callee"]["type"] == "Identifier" { self.call_named(s(&e["callee"], "name"), path, comp, env, &args) } else { self.host_calls += 1; false }
            }
            "SequenceExpression" => { let mut c = false; for x in e["expressions"].as_array().into_iter().flatten() { c |= self.expr_stmt(x, path, comp, env); } c }
            "LogicalExpression" => match (s(e, "operator"), eval(&e["left"], env).truthy()) { ("&&", Some(true)) | ("||", Some(false)) => self.expr_stmt(&e["right"], path, comp, env), _ => false },
            "ConditionalExpression" => match eval(&e["test"], env).truthy() { Some(true) => self.expr_stmt(&e["consequent"], path, comp, env), Some(false) => self.expr_stmt(&e["alternate"], path, comp, env), None => false },
            "AwaitExpression" => self.expr_stmt(&e["argument"], path, comp, env),
            _ => false,
        }
    }

    /// Assign to the innermost scope that holds `name`: a local, else the component instance's persistent state.
    fn assign(&mut self, name: &str, v: V, path: &str, env: &mut Vec<HashMap<String, V>>) -> bool {
        for f in env.iter_mut().skip(2).rev() { if f.contains_key(name) { f.insert(name.to_string(), v); return false; } }
        let changed = self.inst.get(path).and_then(|fr| fr.get(name)).map_or(true, |old| *old != v);
        self.inst.entry(path.to_string()).or_default().insert(name.to_string(), v.clone());
        if env.len() > 1 { env[1].insert(name.to_string(), v); }
        changed
    }

    /// Re-render from the root with the current instance state.
    pub fn render(&mut self, root: &str) -> String {
        self.wires.clear(); self.meta.clear(); self.css.clear(); self.seen_css.clear();
        let mut out = String::new();
        self.component(root, Vec::new(), &mut out);
        out
    }
}

enum Flow { Next(bool), Return(bool) }

/// `omni-render ir-run <ir-dir> <Root> <out-dir> [--click "<selector>"]… [--css file]…` — render, click (our own selector
/// engine finds the element in our own DOM of the markup), re-render; one HTML + bundle per step.
pub fn run_cli(args: &[String]) {
    let (dir, root, outd) = (&args[0], &args[1], &args[2]);
    let mut clicks = Vec::new(); let mut gcss = Vec::new();
    let mut i = 3;
    let mut jsdir: Option<String> = None;
    while i < args.len() { match args[i].as_str() { "--click" => { clicks.push(args[i + 1].clone()); i += 2; } "--css" => { gcss.push(args[i + 1].clone()); i += 2; } "--js" => { jsdir = Some(args[i + 1].clone()); i += 2; } _ => i += 1 } }
    let mut irs = HashMap::new();
    for e in std::fs::read_dir(dir).expect("ir dir").filter_map(|e| e.ok()) {
        let p = e.path();
        if p.extension().map_or(false, |x| x == "json") { if let Ok(j) = serde_json::from_slice::<J>(&std::fs::read(&p).unwrap()) { if let Some(n) = j["component"].as_str() { irs.insert(n.to_string(), j); } } }
    }
    let gtext: String = gcss.iter().map(|g| std::fs::read_to_string(g).unwrap_or_default() + "\n").collect();
    let mut ctx = Ctx::new(&irs);
    if let Some(d) = &jsdir { let mut it = crate::jsi::Interp::new(); crate::jsi_lib::install_fetch(&mut it); it.module_dir = Some(std::path::PathBuf::from(d)); ctx.js = Some(it); }
    let write = |ctx: &Ctx, body: &str, step: usize| {
        let css: String = gtext.clone() + &ctx.css.join("\n");
        let d = format!("{outd}/step{step}");
        std::fs::create_dir_all(&d).ok();
        let html = format!("<!doctype html><html><head><meta charset=\"utf-8\"><style>{css}</style></head><body><div id=\"app\">{body}</div></body></html>");
        std::fs::write(format!("{d}/page.html"), &html).ok();
        std::fs::write(format!("{d}/manifest.json"), "{\"url\":\"file:///launcher/\",\"final_url\":\"file:///launcher/\",\"res\":{}}").ok();
        html
    };
    let t = std::time::Instant::now();
    let mut body = ctx.render(root);
    eprintln!("step0: {} wires, {} components, render {:.2} ms · expressions {} ({} unknown) · data requests {:?}", ctx.wires.len(), ctx.inst.len(), t.elapsed().as_secs_f64() * 1e3, ctx.stats.exprs, ctx.stats.unknown, ctx.js.as_ref().map(|j| j.fetch_log.clone()).unwrap_or_default());
    let mut sess = std::env::var("OMR_INCR").ok().map(|_| crate::incr::Session::new());
    { let h = write(&ctx, &body, 0); if let Some(s) = sess.as_mut() { s.frame(&h, "step0"); } }
    for (k, sel) in clicks.iter().enumerate() {
        let dom = crate::dom::Dom::parse(format!("<html><body>{body}</body></html>").as_bytes());
        let f = crate::css::ElemFacts::new(&dom);
        let parsed = match crate::css::parse_complex(&crate::css::tokenize(sel)) { Some(x) => x, None => { eprintln!("bad selector {sel}"); continue; } };
        let hit = (0..dom.nodes.len() as u32).find(|&i| dom.nodes[i as usize].kind == crate::dom::Kind::Element && crate::css::match_parts(&dom, &f, i, &parsed.parts));
        // the click lands on the nearest element (self or ancestor) that carries handlers
        let mut wire = None;
        let mut n = hit.unwrap_or(crate::dom::NONE);
        while n != crate::dom::NONE { if let Some(w) = dom.attr(n, "data-ir").and_then(|v| v.parse::<usize>().ok()) { wire = Some(w); break; } n = dom.nodes[n as usize].parent; }
        let t = std::time::Instant::now();
        let changed = wire.map_or(false, |w| ctx.fire(w, "click"));
        body = ctx.render(root);
        eprintln!("step{}: click {sel:?} → element {:?} wire {:?} changed {changed} · handlers run {} · host calls {} · re-render {:.2} ms", k + 1, hit, wire, ctx.ran, ctx.host_calls, t.elapsed().as_secs_f64() * 1e3);
        { let h = write(&ctx, &body, k + 1); if let Some(s) = sess.as_mut() { s.frame(&h, &format!("step{}", k + 1)); } }
    }
    if let Some(s) = &sess { s.finish(&format!("{outd}/incr-session.json")); }
}

/// `omni-render ir-html <ir-dir> <Root> [global.css …]` → a full HTML document on stdout; coverage on stderr.
pub fn cli(dir: &str, root: &str, global_css: &[&str]) {
    let mut irs = HashMap::new();
    for e in std::fs::read_dir(dir).expect("ir dir").filter_map(|e| e.ok()) {
        let p = e.path();
        if p.extension().map_or(false, |x| x == "json") { if let Ok(j) = serde_json::from_slice::<J>(&std::fs::read(&p).unwrap()) { if let Some(n) = j["component"].as_str() { irs.insert(n.to_string(), j); } } }
    }
    let mut ctx = Ctx::new(&irs);
    let mut body = String::new();
    ctx.component(root, Vec::new(), &mut body);
    let mut css = String::new();
    for g in global_css { css += &std::fs::read_to_string(g).unwrap_or_default(); css.push('\n'); }
    for c in &ctx.css { css += c; css.push('\n'); }
    println!("<!doctype html><html><head><meta charset=\"utf-8\"><style>{css}</style></head><body><div id=\"app\">{body}</div></body></html>");
    let st = &ctx.stats;
    eprintln!("ir-html {root}: {} components instantiated ({} missing IR), {} elements, {} expressions ({} unknown = {:.1}%), {} unsupported nodes",
        st.comps, st.missing_comps, st.elements, st.exprs, st.unknown, st.unknown as f64 * 100.0 / st.exprs.max(1) as f64, st.unsupported);
    if !ctx.unsupported_kinds.is_empty() { eprintln!("  unsupported: {:?}", ctx.unsupported_kinds); }
}
