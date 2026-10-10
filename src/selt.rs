// SPDX-License-Identifier: MIT OR Apache-2.0
//! SEL-T0 — ternary selector matching (an unpublished plan §4).
//! Every element is a bit vector over the page's feature dictionary (tag, #id, .class, [attr], [attr=value]). Every rule's
//! key (rightmost) compound is a ternary row over the same dictionary: +1 required, −1 forbidden (`:not(x)`), 0 don't
//! care. A row matches iff dot(row, x) == count(+1) and no −1 feature is present — an exact integer test (GEOMETRIC-
//! COMPUTING-LAWS A7: structure is exact), evaluated for ALL rules at once as a sparse ternary mat-vec: the element's few
//! active features add their posting rows into per-rule counters. Rows whose compound has parts the dictionary cannot
//! express (structural pseudos, substring attribute ops, combinators) are "residual": the ternary test is their exact
//! prefilter and the interpreter confirms the survivors. Same answer as the interpreter, by construction and by test.
use crate::css::{self, AttrOp, Bloom, ElemFacts, Sheet, Simple};
use crate::dom::{Dom, Kind};
use std::collections::HashMap;

pub struct Index {
    feat: HashMap<String, u32>,
    by_tag: HashMap<String, u32>, by_id: HashMap<String, u32>, by_class: HashMap<String, u32>, by_attr: HashMap<String, u32>,
    by_attr_val: HashMap<String, HashMap<String, u32>>,
    post: Vec<Vec<(u32, i8)>>,   // feature → (rule, +1 | −1)
    need: Vec<u8>,               // per rule: number of +1 features
    residual: Vec<bool>,         // per rule: interpreter must confirm
    at_least: Vec<bool>,         // per rule: has an :is()/:where() any-of group → counter ≥ need is the (necessary) test
    always: Vec<u32>,            // rules with no +1 feature (need = 0): candidates for every element
    pub rules: usize,
    pub residual_n: usize,
    pub features: usize,
    pub nonzeros: usize,
}

fn simple_key(x: &Simple) -> Option<String> {
    match x {
        Simple::Tag(t) => Some(format!("t:{}", t.to_ascii_lowercase())),
        Simple::Id(v) => Some(format!("#{v}")),
        Simple::Class(v) => Some(format!(".{v}")),
        Simple::Attr(n, AttrOp::Exists, _, _) => Some(format!("@{}", n.to_ascii_lowercase())),
        Simple::Attr(n, AttrOp::Eq, v, false) => Some(format!("@{}={v}", n.to_ascii_lowercase())),
        _ => None,
    }
}

impl Index {
    pub fn build(sheet: &Sheet) -> Index {
        let mut ix = Index { feat: HashMap::new(), by_tag: HashMap::new(), by_id: HashMap::new(), by_class: HashMap::new(), by_attr: HashMap::new(), by_attr_val: HashMap::new(), post: Vec::new(), need: Vec::new(), residual: Vec::new(), at_least: Vec::new(), always: Vec::new(), rules: sheet.rules.len(), residual_n: 0, features: 0, nonzeros: 0 };
        for (ri, r) in sheet.rules.iter().enumerate() {
            let key = r.sel.parts.last().unwrap();
            let mut need = 0u8;
            let mut residual = r.sel.parts.len() > 1;
            let mut row: Vec<(String, i8)> = Vec::new();
            let mut any_of = false;
            for x in &key.simples {
                match x {
                    Simple::Any => {}
                    Simple::Not(list) if list.len() == 1 && list[0].len() == 1 && list[0][0].simples.len() == 1 && list[0][0].comb.is_none() => {
                        match simple_key(&list[0][0].simples[0]) { Some(k) => row.push((k, -1)), None => residual = true }
                    }
                    // :is()/:where() whose every alternative has a keyable simple: any-of group, one unit of `need`
                    Simple::Is(list) if !list.is_empty() && list.iter().all(|alt| alt.last().map_or(false, |c| c.simples.iter().any(|y| simple_key(y).is_some()))) => {
                        for alt in list { let k = alt.last().unwrap().simples.iter().find_map(simple_key).unwrap(); row.push((k, 1)); }
                        need += 1; residual = true; any_of = true;
                    }
                    Simple::Attr(n, _, _, _) if simple_key(x).is_none() => { row.push((format!("@{}", n.to_ascii_lowercase()), 1)); need += 1; residual = true; }
                    s => match simple_key(s) { Some(k) => { row.push((k, 1)); need += 1; } None => residual = true },
                }
            }
            for (k, sgn) in row {
                let n = ix.feat.len() as u32;
                let f = *ix.feat.entry(k).or_insert(n);
                if f as usize == ix.post.len() { ix.post.push(Vec::new()); }
                ix.post[f as usize].push((ri as u32, sgn));
                ix.nonzeros += 1;
            }
            if need == 0 { ix.always.push(ri as u32); }
            if residual { ix.residual_n += 1; }
            ix.need.push(need);
            ix.residual.push(residual);
            ix.at_least.push(any_of);
        }
        ix.features = ix.feat.len();
        for (k, &v) in &ix.feat {
            if let Some(t) = k.strip_prefix("t:") { ix.by_tag.insert(t.to_string(), v); }
            else if let Some(t) = k.strip_prefix('#') { ix.by_id.insert(t.to_string(), v); }
            else if let Some(t) = k.strip_prefix('.') { ix.by_class.insert(t.to_string(), v); }
            else if let Some(t) = k.strip_prefix('@') {
                match t.split_once('=') { Some((n, val)) => { ix.by_attr_val.entry(n.to_string()).or_default().insert(val.to_string(), v); } None => { ix.by_attr.insert(t.to_string(), v); } }
            }
        }
        ix
    }

    /// The element's active features (only those present in the dictionary).
    fn active(&self, d: &Dom, f: &ElemFacts, e: u32, out: &mut Vec<u32>) {
        out.clear();
        let tag = d.tag(e);
        if let Some(&x) = self.by_tag.get(tag) { out.push(x); } else if tag.bytes().any(|b| b.is_ascii_uppercase()) { if let Some(&x) = self.by_tag.get(&tag.to_ascii_lowercase()) { out.push(x); } }
        if let Some(id) = &f.id[e as usize] { if let Some(&x) = self.by_id.get(id.as_str()) { out.push(x); } }
        for c in &f.classes[e as usize] { if let Some(&x) = self.by_class.get(c.as_str()) { out.push(x); } }
        for a in d.attr_list(e) {
            let n: &str = &d.names[a.name as usize].local;
            if let Some(&x) = self.by_attr.get(n) { out.push(x); }
            if let Some(m) = self.by_attr_val.get(n) { if let Some(&x) = m.get(d.span_str(a.val)) { out.push(x); } }
        }
    }

    /// Matching rules for `e` (rule indices), same set as `Sheet::matching_bloom`.
    pub fn matching(&self, sheet: &Sheet, d: &Dom, f: &ElemFacts, e: u32, anc: &Bloom, scratch: &mut Scratch, out: &mut Vec<u32>) {
        out.clear();
        self.active(d, f, e, &mut scratch.act);
        scratch.act.sort_unstable(); scratch.act.dedup(); // a repeated class is one feature
        for &ft in &scratch.act {
            for &(r, s) in &self.post[ft as usize] {
                let ru = r as usize;
                if !scratch.seen[ru] { scratch.seen[ru] = true; scratch.touched.push(r); }
                if s > 0 { scratch.cnt[ru] += 1; } else { scratch.bad[ru] = true; }
            }
        }
        let ok = |r: u32, sc: &Scratch| -> bool {
            let ru = r as usize;
            if sc.bad[ru] || if self.at_least[ru] { sc.cnt[ru] < self.need[ru] } else { sc.cnt[ru] != self.need[ru] } { return false; }
            let rule = &sheet.rules[ru];
            if (0..4).any(|k| rule.anc[k] & !anc[k] != 0) { return false; }
            !self.residual[ru] || css::match_parts(d, f, e, &rule.sel.parts)
        };
        for &r in &scratch.touched { if self.need[r as usize] > 0 && ok(r, scratch) { out.push(r); } }
        for &r in &self.always { if ok(r, scratch) { out.push(r); } }
        for &r in &scratch.touched { scratch.cnt[r as usize] = 0; scratch.bad[r as usize] = false; scratch.seen[r as usize] = false; }
        scratch.touched.clear();
        out.sort_unstable();
    }
}

pub struct Scratch { cnt: Vec<u8>, bad: Vec<bool>, seen: Vec<bool>, touched: Vec<u32>, act: Vec<u32> }
impl Scratch { pub fn new(rules: usize) -> Scratch { Scratch { cnt: vec![0; rules], bad: vec![false; rules], seen: vec![false; rules], touched: Vec::new(), act: Vec::new() } } }

/// `omni-render selt <page>`: ternary matcher vs the interpreter on every element — identical sets required; timings.
pub fn bench(target: &str) {
    let dir = std::fs::read_dir("corpus").unwrap().filter_map(|e| e.ok()).map(|e| e.path())
        .find(|p| p.file_name().map_or(false, |n| n.to_string_lossy().contains(target)) && p.join("manifest.json").exists()).expect("page");
    let b = crate::page::Bundle::open(&dir).unwrap();
    let l = crate::page::load(&b, (1280.0, 800.0));
    let d = &l.dom;
    let f = ElemFacts::new(d);
    let n = d.nodes.len();
    // ancestor Blooms exactly as the cascade builds them
    let mut bloom: Vec<Bloom> = vec![[0; 4]; n];
    for i in 0..n as u32 {
        let p = d.nodes[i as usize].parent;
        if p != crate::dom::NONE && d.nodes[p as usize].kind == Kind::Element {
            let mut bb = bloom[p as usize];
            css::bloom_add(&mut bb, b't', &d.tag(p).to_ascii_lowercase());
            if let Some(id) = &f.id[p as usize] { css::bloom_add(&mut bb, b'#', id); }
            for c in &f.classes[p as usize] { css::bloom_add(&mut bb, b'.', c); }
            bloom[i as usize] = bb;
        } else if p != crate::dom::NONE { bloom[i as usize] = bloom[p as usize]; }
    }
    let els: Vec<u32> = (0..n as u32).filter(|&i| d.nodes[i as usize].kind == Kind::Element && !d.tag(i).starts_with("::")).collect();
    let tb = std::time::Instant::now();
    let ix = Index::build(&l.sheet);
    let build_ms = tb.elapsed().as_secs_f64() * 1e3;
    // interpreter
    let mut interp: Vec<Vec<u32>> = Vec::with_capacity(els.len());
    let mut m = Vec::new();
    let t0 = std::time::Instant::now();
    for &e in &els { m.clear(); l.sheet.matching_bloom(d, &f, e, &bloom[e as usize], &mut m); let mut v: Vec<u32> = m.iter().map(|x| x.2).collect(); v.sort_unstable(); v.dedup(); interp.push(v); }
    let interp_ms = t0.elapsed().as_secs_f64() * 1e3;
    // ternary
    let mut sc = Scratch::new(ix.rules);
    let mut out = Vec::new();
    let mut agree = 0usize;
    let mut first_bad: Option<(u32, Vec<u32>, Vec<u32>)> = None;
    let t1 = std::time::Instant::now();
    let mut tern: Vec<Vec<u32>> = Vec::with_capacity(els.len());
    for &e in &els { ix.matching(&l.sheet, d, &f, e, &bloom[e as usize], &mut sc, &mut out); tern.push(out.clone()); }
    let tern_ms = t1.elapsed().as_secs_f64() * 1e3;
    for (k, &e) in els.iter().enumerate() { if interp[k] == tern[k] { agree += 1; } else if first_bad.is_none() { first_bad = Some((e, interp[k].clone(), tern[k].clone())); } }
    let matched: usize = interp.iter().map(|v| v.len()).sum();
    println!("{}: {} elements, {} rules ({} residual = {:.1}%), {} features, {} nonzeros",
        dir.file_name().unwrap().to_string_lossy(), els.len(), ix.rules, ix.residual_n, ix.residual_n as f64 * 100.0 / ix.rules.max(1) as f64, ix.features, ix.nonzeros);
    println!("  always-checked rules (no required feature): {}", ix.always.len());
    println!("  identical matched sets: {agree} of {} elements ({} rule matches total)", els.len(), matched);
    println!("  interpreter {interp_ms:.2} ms · ternary {tern_ms:.2} ms (+ build {build_ms:.2} ms) · ratio {:.2}×", interp_ms / tern_ms.max(1e-9));
    if let Some((e, a, b2)) = first_bad {
        let only_i: Vec<_> = a.iter().filter(|x| !b2.contains(x)).take(3).collect();
        let only_t: Vec<_> = b2.iter().filter(|x| !a.contains(x)).take(3).collect();
        println!("  first difference: node {e} <{}> interpreter-only {:?} ternary-only {:?}", d.tag(e), only_i, only_t);
        for &&r in only_i.iter().chain(only_t.iter()) { println!("    rule {r}: {:?} need {} residual {} anc-ok {}", l.sheet.rules[r as usize].sel.parts, ix.need[r as usize], ix.residual[r as usize], !(0..4).any(|k| l.sheet.rules[r as usize].anc[k] & !bloom[e as usize][k] != 0)); }
    }
}
