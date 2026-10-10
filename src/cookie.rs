// SPDX-License-Identifier: MIT OR Apache-2.0
//! cookie — the browser's cookie jar (BROWSER-FUNCTIONAL-0 C6, author 10-07 "yes add the cookie jar"): RFC 6265 storage model
//! (§5.3), Cookie header (§5.4), domain / path matching (§5.1.3–5.1.4), cookie date parsing (§5.1.1), the RFC 6265bis
//! additions that matter today (Secure cookies only from https, __Secure- / __Host- prefixes, HttpOnly kept from scripts).
//! Policy (author): FIRST-PARTY ONLY — a cookie is stored or sent only when the request's site equals the top-level page's site
//! (stricter than SameSite=None: no third-party tracking cookies at all); MEMORY by default; persisted only for sites the
//! person chose to keep (`keep` set), never others. Sites = registrable domains from a small public-suffix table (the
//! publicsuffix.org concept; the table below lists common multi-label suffixes — any other host is treated as one label
//! under its TLD). Clean room: written from the RFC text; no code copied.
use std::collections::HashSet;
use std::path::PathBuf;

#[derive(Clone, Debug, PartialEq)]
pub struct Cookie {
    pub name: String, pub value: String, pub domain: String, pub path: String,
    /// unix seconds; None = session cookie (dies with the browser)
    pub expires: Option<i64>,
    pub host_only: bool, pub secure: bool, pub http_only: bool,
    pub created: i64, pub seq: u64,
}

#[derive(Default)]
pub struct Jar {
    pub cookies: Vec<Cookie>,
    seq: u64,
    /// sites whose persistent cookies survive a restart (the person opted in, per site)
    pub keep: HashSet<String>,
    /// sites whose cookies are refused outright: none stored, none sent (the third state of the site-panel switch)
    pub block: HashSet<String>,
    pub file: Option<PathBuf>,
    keep_file: Option<PathBuf>,
    block_file: Option<PathBuf>,
}

/// Multi-label public suffixes (registrations happen one label below them). Single-label TLDs need no entry.
const SUFFIX2: &[&str] = &["co.uk", "org.uk", "ac.uk", "gov.uk", "me.uk", "ltd.uk", "plc.uk", "net.uk", "sch.uk", "nhs.uk", "police.uk",
    "com.au", "net.au", "org.au", "edu.au", "gov.au", "asn.au", "id.au", "co.nz", "org.nz", "net.nz", "govt.nz", "ac.nz",
    "co.jp", "ne.jp", "or.jp", "ac.jp", "go.jp", "ad.jp", "co.kr", "or.kr", "ac.kr", "go.kr", "com.br", "net.br", "org.br", "gov.br",
    "com.cn", "net.cn", "org.cn", "gov.cn", "edu.cn", "com.mx", "org.mx", "gob.mx", "edu.mx", "co.in", "net.in", "org.in", "gov.in", "ac.in",
    "co.za", "org.za", "gov.za", "ac.za", "com.ar", "gob.ar", "com.tr", "gov.tr", "com.tw", "org.tw", "gov.tw", "edu.tw", "com.hk", "org.hk",
    "gov.hk", "com.sg", "gov.sg", "edu.sg", "com.my", "gov.my", "co.id", "or.id", "go.id", "ac.id", "co.il", "org.il", "ac.il", "gov.il",
    "com.ua", "co.th", "in.th", "go.th", "ac.th", "com.vn", "com.ph", "com.pk", "com.ng", "com.eg", "com.sa", "com.pe", "com.co",
    "co.ve", "com.ve", "com.es", "co.at", "or.at", "com.pl", "net.pl", "org.pl", "co.it", "com.ru", "co.ke", "com.bd", "com.np",
    // private registries (anyone can register under them: each customer is its own site)
    "github.io", "gitlab.io", "herokuapp.com", "blogspot.com", "appspot.com", "netlify.app", "vercel.app", "pages.dev", "workers.dev",
    "cloudfront.net", "azurewebsites.net", "firebaseapp.com", "web.app", "s3.amazonaws.com", "fly.dev", "onrender.com", "glitch.me",
    "neocities.org", "readthedocs.io", "substack.com", "wordpress.com", "tumblr.com", "myshopify.com"];

fn is_ip(h: &str) -> bool { h.parse::<std::net::IpAddr>().is_ok() || h.starts_with('[') }

/// Is `d` itself a public suffix (a TLD, or one of the multi-label suffixes)?
pub fn is_public_suffix(d: &str) -> bool { !d.contains('.') || SUFFIX2.contains(&d) }

/// The registrable domain ("site") of a host: suffix + one label. IPs and single labels are their own site.
pub fn site_of(host: &str) -> String {
    let h = host.trim_end_matches('.').to_ascii_lowercase();
    if is_ip(&h) { return h; }
    let labels: Vec<&str> = h.split('.').collect();
    if labels.len() <= 2 { return h; }
    let last2 = labels[labels.len() - 2..].join(".");
    let last3 = labels[labels.len() - 3..].join(".");
    if SUFFIX2.contains(&last3.as_str()) && labels.len() >= 4 { return labels[labels.len() - 4..].join("."); }
    if SUFFIX2.contains(&last2.as_str()) { return last3; }
    last2
}

/// (scheme is https, host, path) of an absolute http(s) URL.
pub fn parts(url: &str) -> Option<(bool, String, String)> {
    let (tls, rest) = match url.strip_prefix("https://") { Some(r) => (true, r), None => (false, url.strip_prefix("http://")?) };
    let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let hostport = &rest[..end];
    let host = if hostport.starts_with('[') { hostport.split(']').next().map(|h| format!("{h}]")).unwrap_or_default() } else { hostport.split(':').next().unwrap_or("").to_string() };
    let path = rest[end..].split(['?', '#']).next().unwrap_or("");
    Some((tls, host.to_ascii_lowercase(), if path.is_empty() { "/".into() } else { path.to_string() }))
}

/// §5.1.3 domain-match
fn domain_match(host: &str, domain: &str) -> bool { host == domain || (!is_ip(host) && host.ends_with(domain) && host[..host.len() - domain.len()].ends_with('.')) }

/// §5.1.4 default-path: the request path up to (not including) its last '/', or "/"
fn default_path(path: &str) -> String {
    if !path.starts_with('/') { return "/".into(); }
    match path.rfind('/') { Some(0) | None => "/".into(), Some(i) => path[..i].to_string() }
}

/// §5.1.4 path-match
fn path_match(req: &str, cp: &str) -> bool { req == cp || (req.starts_with(cp) && (cp.ends_with('/') || req[cp.len()..].starts_with('/'))) }

pub fn now() -> i64 { std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0) }

/// §5.1.1 cookie-date: tokens split on delimiters; first time (hh:mm:ss), day-of-month, month name, year win.
pub fn parse_date(s: &str) -> Option<i64> {
    let delim = |c: char| c == '\t' || (' '..='/').contains(&c) || (';'..='@').contains(&c) || ('['..='`').contains(&c) || ('{'..='~').contains(&c);
    let (mut time, mut day, mut month, mut year) = (None, None, None, None);
    for tok in s.split(delim).filter(|t| !t.is_empty()) {
        let digits_prefix = |t: &str, lo: usize, hi: usize| -> Option<i64> {
            let n = t.chars().take_while(|c| c.is_ascii_digit()).count();
            if n < lo || n > hi { return None; }
            t[..n].parse().ok()
        };
        if time.is_none() {
            let p: Vec<&str> = tok.split(':').collect();
            if p.len() == 3 { if let (Some(h), Some(m), Some(sec)) = (digits_prefix(p[0], 1, 2), digits_prefix(p[1], 1, 2), digits_prefix(p[2], 1, 2)) { time = Some((h, m, sec)); continue; } }
        }
        if day.is_none() { if let Some(d) = digits_prefix(tok, 1, 2) { if tok.len() <= 2 || !tok.as_bytes()[2].is_ascii_digit() { day = Some(d); continue; } } }
        if month.is_none() && tok.len() >= 3 {
            let m = ["jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec"].iter().position(|m| tok[..3].eq_ignore_ascii_case(m));
            if let Some(m) = m { month = Some(m as i64 + 1); continue; }
        }
        if year.is_none() { if let Some(y) = digits_prefix(tok, 2, 4) { year = Some(y); continue; } }
    }
    let (h, mi, sec) = time?;
    let (d, mo, mut y) = (day?, month?, year?);
    if (70..=99).contains(&y) { y += 1900; } else if (0..=69).contains(&y) { y += 2000; }
    if !(1..=31).contains(&d) || y < 1601 || h > 23 || mi > 59 || sec > 59 { return None; }
    // days from civil (proleptic Gregorian), Howard Hinnant's published algorithm
    let (yy, mm) = if mo <= 2 { (y - 1, mo + 9) } else { (y, mo - 3) };
    let era = yy.div_euclid(400);
    let yoe = yy - era * 400;
    let doy = (153 * mm + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146097 + doe - 719468;
    Some(days * 86400 + h * 3600 + mi * 60 + sec)
}

impl Jar {
    /// A jar that keeps the chosen sites' persistent cookies in `dir` (cookies.jsonl + keep-sites.txt).
    pub fn persistent(dir: PathBuf) -> Jar {
        let mut j = Jar { file: Some(dir.join("cookies.jsonl")), keep_file: Some(dir.join("keep-sites.txt")), block_file: Some(dir.join("block-sites.txt")), ..Default::default() };
        if let Ok(s) = std::fs::read_to_string(dir.join("block-sites.txt")) { j.block = s.lines().map(|l| l.trim().to_string()).filter(|l| !l.is_empty()).collect(); }
        if let Ok(s) = std::fs::read_to_string(dir.join("keep-sites.txt")) { j.keep = s.lines().map(|l| l.trim().to_string()).filter(|l| !l.is_empty()).collect(); }
        if let Ok(s) = std::fs::read_to_string(dir.join("cookies.jsonl")) {
            let t = now();
            for l in s.lines() {
                let Ok(v) = serde_json::from_str::<serde_json::Value>(l) else { continue };
                let c = Cookie { name: v["name"].as_str().unwrap_or("").into(), value: v["value"].as_str().unwrap_or("").into(),
                    domain: v["domain"].as_str().unwrap_or("").into(), path: v["path"].as_str().unwrap_or("/").into(), expires: v["expires"].as_i64(),
                    host_only: v["host_only"].as_bool().unwrap_or(true), secure: v["secure"].as_bool().unwrap_or(false),
                    http_only: v["http_only"].as_bool().unwrap_or(false), created: v["created"].as_i64().unwrap_or(t), seq: 0 };
                if c.expires.map(|e| e > t).unwrap_or(false) && j.keep.contains(&site_of(&c.domain)) { j.seq += 1; j.cookies.push(Cookie { seq: j.seq, ..c }); }
            }
        }
        j
    }

    fn save(&self) {
        let (Some(f), Some(kf)) = (&self.file, &self.keep_file) else { return };
        if let Some(d) = f.parent() { let _ = std::fs::create_dir_all(d); }
        let mut out = String::new();
        for c in self.cookies.iter().filter(|c| c.expires.is_some() && self.keep.contains(&site_of(&c.domain))) {
            out.push_str(&serde_json::json!({"name": c.name, "value": c.value, "domain": c.domain, "path": c.path, "expires": c.expires,
                "host_only": c.host_only, "secure": c.secure, "http_only": c.http_only, "created": c.created}).to_string());
            out.push('\n');
        }
        // write-then-rename: a crash never leaves half a jar
        let tmp = f.with_extension("tmp");
        if std::fs::write(&tmp, out).is_ok() { let _ = std::fs::rename(&tmp, f); }
        let mut ks: Vec<&String> = self.keep.iter().collect();
        ks.sort();
        let _ = std::fs::write(kf, ks.iter().map(|s| format!("{s}\n")).collect::<String>());
        if let Some(bf) = &self.block_file { let mut bs: Vec<&String> = self.block.iter().collect(); bs.sort(); let _ = std::fs::write(bf, bs.iter().map(|s| format!("{s}\n")).collect::<String>()); }
    }

    /// Keep (true) or stop keeping (false) a site's cookies across restarts.
    pub fn set_keep(&mut self, site: &str, keep: bool) {
        if keep { self.block.remove(site); self.keep.insert(site.to_string()); } else { self.keep.remove(site); }
        self.save();
    }

    /// Block (true) a site's cookies — forgets what it holds, refuses and withholds from now on — or lift the block.
    pub fn set_block(&mut self, site: &str, block: bool) {
        if block { self.block.insert(site.to_string()); self.keep.remove(site); self.cookies.retain(|c| site_of(&c.domain) != site); } else { self.block.remove(site); }
        self.save();
    }

    /// Forget every cookie of a site.
    pub fn clear_site(&mut self, site: &str) {
        self.cookies.retain(|c| site_of(&c.domain) != site);
        self.save();
    }

    pub fn count_site(&self, site: &str) -> usize { self.cookies.iter().filter(|c| site_of(&c.domain) == site).count() }

    /// §5.3: store one Set-Cookie line received for `url`. `top_site` = the page's site (first-party policy);
    /// `from_script` = document.cookie (cannot create or overwrite HttpOnly cookies).
    pub fn set(&mut self, url: &str, line: &str, top_site: &str, from_script: bool) -> bool {
        let Some((tls, host, path)) = parts(url) else { return false };
        if site_of(&host) != top_site || self.block.contains(top_site) { return false; } // third party or blocked site: never stored
        let mut it = line.split(';');
        let nv = it.next().unwrap_or("");
        let (name, value) = match nv.split_once('=') { Some((n, v)) => (n.trim(), v.trim()), None => ("", nv.trim()) };
        if name.is_empty() && value.is_empty() { return false; }
        if name.len() + value.len() > 4096 { return false; }
        let t = now();
        let (mut expires, mut max_age, mut domain, mut cpath, mut secure, mut http_only) = (None, None, String::new(), None, false, false);
        for av in it {
            let (k, v) = match av.split_once('=') { Some((k, v)) => (k.trim(), v.trim()), None => (av.trim(), "") };
            match k.to_ascii_lowercase().as_str() {
                "expires" => if let Some(e) = parse_date(v) { expires = Some(e); },
                "max-age" => if let Ok(n) = v.parse::<i64>() { max_age = Some(if n <= 0 { i64::MIN } else { t.saturating_add(n) }); },
                "domain" => if !v.is_empty() { domain = v.trim_start_matches('.').to_ascii_lowercase(); },
                "path" => if v.starts_with('/') { cpath = Some(v.to_string()); },
                "secure" => secure = true,
                "httponly" => http_only = true,
                _ => {}
            }
        }
        if http_only && from_script { return false; }
        if secure && !tls { return false; } // 6265bis: only secure origins set Secure cookies
        let expires = max_age.or(expires);
        let host_only;
        if !domain.is_empty() {
            if is_public_suffix(&domain) { if domain == host { domain.clear(); } else { return false; } }
        }
        if !domain.is_empty() {
            if !domain_match(&host, &domain) { return false; }
            host_only = false;
        } else { domain = host.clone(); host_only = true; }
        let cpath = cpath.unwrap_or_else(|| default_path(&path));
        // 6265bis §4.1.3 prefixes
        if name.starts_with("__Secure-") && !secure { return false; }
        if name.starts_with("__Host-") && (!secure || !host_only || cpath != "/") { return false; }
        let old = self.cookies.iter().position(|c| c.name == name && c.domain == domain && c.path == cpath);
        let mut created = t;
        if let Some(i) = old {
            if from_script && self.cookies[i].http_only { return false; }
            created = self.cookies[i].created;
            self.cookies.remove(i);
        }
        let persist = expires.is_some();
        if expires.map(|e| e <= t).unwrap_or(false) { if persist && self.keep.contains(&site_of(&domain)) { self.save(); } return true; } // expiry in the past = delete
        self.seq += 1;
        self.cookies.push(Cookie { name: name.into(), value: value.into(), domain: domain.clone(), path: cpath, expires, host_only, secure, http_only, created, seq: self.seq });
        // §6.1 limits: 50 per domain, 3000 in all — the least recently set go first
        let n_dom = self.cookies.iter().filter(|c| c.domain == domain).count();
        if n_dom > 50 { if let Some(i) = self.cookies.iter().position(|c| c.domain == domain) { self.cookies.remove(i); } }
        if self.cookies.len() > 3000 { self.cookies.remove(0); }
        if persist && self.keep.contains(&site_of(&domain)) { self.save(); }
        true
    }

    /// §5.4: the Cookie header value for a request to `url` (empty: send none). `for_script` hides HttpOnly cookies.
    pub fn header(&mut self, url: &str, top_site: &str, for_script: bool) -> String {
        let Some((tls, host, path)) = parts(url) else { return String::new() };
        if site_of(&host) != top_site || self.block.contains(top_site) { return String::new(); }
        let t = now();
        self.cookies.retain(|c| c.expires.map(|e| e > t).unwrap_or(true));
        let mut m: Vec<&Cookie> = self.cookies.iter().filter(|c| {
            (if c.host_only { host == c.domain } else { domain_match(&host, &c.domain) }) && path_match(&path, &c.path) && (!c.secure || tls) && !(for_script && c.http_only)
        }).collect();
        m.sort_by(|a, b| b.path.len().cmp(&a.path.len()).then(a.created.cmp(&b.created)).then(a.seq.cmp(&b.seq)));
        m.iter().map(|c| if c.name.is_empty() { c.value.clone() } else { format!("{}={}", c.name, c.value) }).collect::<Vec<_>>().join("; ")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rfc6265_basics() {
        let mut j = Jar::default();
        let s = site_of("www.example.co.uk");
        assert_eq!(s, "example.co.uk");
        assert_eq!(site_of("a.b.github.io"), "b.github.io");
        assert!(j.set("https://www.example.co.uk/a/b", "sid=1; Path=/", &s, false));
        assert!(j.set("https://www.example.co.uk/a/b", "loc=2", &s, false)); // default path /a
        assert!(!j.set("https://www.example.co.uk/", "x=1; Domain=co.uk", &s, false)); // public suffix
        assert!(!j.set("https://tracker.net/", "t=1", &s, false)); // third party
        assert!(j.set("https://www.example.co.uk/", "d=1; Domain=example.co.uk", &s, false));
        assert_eq!(j.header("https://shop.example.co.uk/", &s, false), "d=1"); // sid is host-only (www)
        assert_eq!(j.header("https://www.example.co.uk/a/c", &s, false), "loc=2; sid=1; d=1");
        assert!(j.set("https://www.example.co.uk/", "h=1; HttpOnly", &s, false));
        assert!(!j.set("https://www.example.co.uk/", "h=2", &s, true)); // script cannot overwrite HttpOnly
        assert!(!j.header("https://www.example.co.uk/", &s, true).contains("h="));
        assert!(!j.set("http://www.example.co.uk/", "s=1; Secure", &s, false));
        assert!(!j.set("https://www.example.co.uk/", "__Host-a=1; Secure; Domain=example.co.uk", &s, false));
        assert!(j.set("https://www.example.co.uk/", "sid=; Max-Age=0; Path=/", &s, false));
        assert!(!j.header("https://www.example.co.uk/", &s, false).contains("sid="));
        assert_eq!(parse_date("Wed, 21 Oct 2015 07:28:00 GMT"), Some(1445412480));
        assert_eq!(parse_date("Fri, 01-Jan-2038 00:00:00 GMT"), Some(2145916800));
        assert_eq!(j.header("https://tracker.net/", &s, false), "");
    }

    #[test]
    fn blocked_site_stores_and_sends_nothing() {
        let mut j = Jar::default();
        let s = site_of("www.example.com");
        assert!(j.set("https://www.example.com/", "a=1", &s, false));
        j.set_block(&s, true);
        assert_eq!(j.count_site(&s), 0); // blocking forgets what the site held
        assert!(!j.set("https://www.example.com/", "b=2", &s, false));
        assert_eq!(j.header("https://www.example.com/", &s, false), "");
        j.set_keep(&s, true); // keeping lifts the block
        assert!(!j.block.contains(&s));
        assert!(j.set("https://www.example.com/", "c=3", &s, false));
    }
}
