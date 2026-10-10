// SPDX-License-Identifier: MIT OR Apache-2.0
//! agent — what an AI agent needs from a page: the visible content in reading order as compact markdown
//! (headings, paragraphs, lists, links, table rows, form fields), with CSS-hidden content dropped.
//! No raster, no layout: packed DOM + style codewords only — that is the whole RAM bill.
use crate::dom::{Dom, Kind};
use crate::style::{Display, Styled};

pub fn markdown(d: &Dom, st: &Styled, base: &str) -> String {
    let body = (0..d.nodes.len() as u32).find(|&i| d.tag(i) == "body").unwrap_or(0);
    render(d, st, body, base, Ctx::default())
}

/// Reader view (BROWSER-BODY-0 step 5): the main content only. Root = the page's own declaration of it (HTML `<main>`,
/// ARIA role=main, a single `<article>`), else the element whose own paragraphs carry the most text (the content-density
/// idea readability tools share; our code). Inside it, navigation / asides / page furniture and form controls are dropped.
pub fn reader(d: &Dom, st: &Styled, base: &str) -> String {
    let n = d.nodes.len() as u32;
    let el = |i: u32| d.nodes[i as usize].kind == Kind::Element && visible(d, st, i);
    let declared = (0..n).find(|&i| el(i) && (d.tag(i) == "main" || d.attr(i, "role") == Some("main")))
        .or_else(|| { let a: Vec<u32> = (0..n).filter(|&i| el(i) && d.tag(i) == "article").collect(); if a.len() == 1 { Some(a[0]) } else { None } });
    let root = declared.unwrap_or_else(|| {
        // text of each <p>, credited to its parent: the densest parent is the content column
        let mut score = vec![0usize; n as usize];
        for i in 0..n {
            if !(el(i) && d.tag(i) == "p") { continue; }
            let end = d.nodes[i as usize].end.min(n);
            let t: usize = (i..end).filter(|&k| d.nodes[k as usize].kind == Kind::Text).map(|k| d.text(k).trim().len()).sum();
            let par = d.nodes[i as usize].parent;
            if par != crate::dom::NONE { score[par as usize] += t; }
        }
        (0..n).max_by_key(|&i| score[i as usize]).filter(|&i| score[i as usize] > 0)
            .unwrap_or_else(|| (0..n).find(|&i| d.tag(i) == "body").unwrap_or(0))
    });
    render(d, st, root, base, Ctx { reader: true, ..Ctx::default() })
}

fn render(d: &Dom, st: &Styled, root: u32, base: &str, mut cx: Ctx) -> String {
    let mut out = String::with_capacity(d.bytes.len() / 2);
    walk(d, st, root, base, &mut out, &mut cx);
    // collapse blank lines
    let mut clean = String::with_capacity(out.len());
    let mut blank = 0;
    for l in out.lines() {
        let t = l.trim_end();
        if t.trim().is_empty() { blank += 1; if blank == 1 { clean.push('\n'); } continue; }
        blank = 0;
        clean.push_str(t);
        clean.push('\n');
    }
    clean
}

#[derive(Default)]
struct Ctx { list_depth: usize, in_pre: bool, reader: bool }

fn visible(d: &Dom, st: &Styled, n: u32) -> bool {
    if d.nodes[n as usize].kind != Kind::Element { return true; }
    let s = &st.table[st.ids[n as usize] as usize];
    s.display != Display::None && s.visibility == 0
}

fn block_tag(t: &str) -> bool {
    matches!(t, "p" | "div" | "section" | "article" | "main" | "header" | "footer" | "nav" | "aside" | "li" | "ul" | "ol" | "dl" | "dt" | "dd"
        | "h1" | "h2" | "h3" | "h4" | "h5" | "h6" | "pre" | "blockquote" | "table" | "tr" | "form" | "figure" | "figcaption" | "br" | "hr")
}

fn walk(d: &Dom, st: &Styled, n: u32, base: &str, out: &mut String, cx: &mut Ctx) {
    for c in d.children(n) {
        match d.nodes[c as usize].kind {
            Kind::Text => {
                let t = d.text(c);
                if cx.in_pre { out.push_str(t); continue; }
                let mut prev_ws = out.ends_with(|ch: char| ch.is_whitespace()) || out.is_empty();
                for ch in t.chars() {
                    if ch.is_whitespace() { if !prev_ws { out.push(' '); prev_ws = true; } } else { out.push(ch); prev_ws = false; }
                }
            }
            Kind::Element => {
                if !visible(d, st, c) { continue; }
                let tag = d.tag(c);
                if cx.reader && (matches!(tag, "nav" | "aside" | "footer" | "form" | "input" | "textarea" | "select" | "button" | "label" | "sup")
                    || matches!(d.attr(c, "role"), Some("navigation" | "complementary" | "contentinfo" | "search"))) { continue; }
                match tag {
                    "script" | "style" | "template" | "noscript" | "svg" | "canvas" | "iframe" | "head" => continue,
                    "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => {
                        out.push_str("\n\n");
                        out.push_str(&"#".repeat(tag[1..].parse::<usize>().unwrap_or(1)));
                        out.push(' ');
                        walk(d, st, c, base, out, cx);
                        out.push_str("\n\n");
                    }
                    "a" => {
                        let href = d.attr(c, "href").filter(|h| !h.starts_with("javascript:") && !h.is_empty());
                        // link text on one line: block children inside an <a> must not split the [text](url) form
                        if let Some(h) = href {
                            let mut inner = String::new();
                            walk(d, st, c, base, &mut inner, cx);
                            let inner = inner.split_whitespace().collect::<Vec<_>>().join(" ");
                            if inner.is_empty() { continue; }
                            if !out.ends_with(|ch: char| ch.is_whitespace()) && !out.is_empty() && inner.starts_with(|ch: char| ch.is_alphanumeric()) && out.ends_with(|ch: char| ch.is_alphanumeric()) { out.push(' '); }
                            out.push('['); out.push_str(&inner.replace(['[', ']'], "")); out.push_str("]("); out.push_str(&crate::page::resolve(base, h)); out.push(')');
                        }
                        else { walk(d, st, c, base, out, cx); }
                    }
                    "img" if cx.reader => {}
                    "img" => { if let Some(a) = d.attr(c, "alt").filter(|a| !a.trim().is_empty()) { out.push_str("![" ); out.push_str(a.trim()); out.push(']'); } }
                    "li" => { out.push('\n'); out.push_str(&"  ".repeat(cx.list_depth.saturating_sub(1))); out.push_str("- "); walk(d, st, c, base, out, cx); }
                    "ul" | "ol" => { cx.list_depth += 1; walk(d, st, c, base, out, cx); cx.list_depth -= 1; out.push('\n'); }
                    "pre" => { out.push_str("\n```\n"); cx.in_pre = true; walk(d, st, c, base, out, cx); cx.in_pre = false; out.push_str("\n```\n"); }
                    "code" if !cx.in_pre => { out.push('`'); walk(d, st, c, base, out, cx); out.push('`'); }
                    "strong" | "b" => { out.push_str("**"); walk(d, st, c, base, out, cx); out.push_str("**"); }
                    "tr" => { out.push_str("\n|"); for cell in d.children(c) { if matches!(d.tag(cell), "td" | "th") && visible(d, st, cell) { out.push(' '); walk(d, st, cell, base, out, cx); out.push_str(" |"); } } }
                    "br" => out.push('\n'),
                    "hr" => out.push_str("\n---\n"),
                    "input" | "textarea" | "select" | "button" => {
                        let label = d.attr(c, "aria-label").or(d.attr(c, "placeholder")).or(d.attr(c, "value")).or(d.attr(c, "name")).unwrap_or("");
                        if d.attr(c, "type") != Some("hidden") { out.push_str(&format!(" [{} {}] ", tag, label.trim())); }
                        if tag == "button" || tag == "select" { walk(d, st, c, base, out, cx); }
                    }
                    _ => {
                        let b = block_tag(tag) || matches!(st.table[st.ids[c as usize] as usize].display, Display::Block | Display::ListItem | Display::Flex | Display::Grid | Display::Table);
                        if b { out.push('\n'); }
                        walk(d, st, c, base, out, cx);
                        if b { out.push('\n'); }
                    }
                }
            }
            _ => {}
        }
    }
}
