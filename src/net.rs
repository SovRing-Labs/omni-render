// SPDX-License-Identifier: MIT OR Apache-2.0
//! net — minimal HTTP/1.1 client (own code): GET / POST with Content-Length / chunked / close-delimited bodies (RFC 9112 §6.3),
//! redirects (≤20, Fetch §4.4; 303 and POST-after-301/302 turn into GET), connection reuse per host, gzip / deflate / br
//! content codings (RFC 9110 §8.4), https through rustls (TLS 1.3 + 1.2, ring crypto, Mozilla roots from webpki-roots — no
//! system store, no OCSP/CRL network calls). Privacy: no referer, a generic UA; cookies only through the jar (cookie.rs:
//! FIRST-PARTY ONLY, memory unless the person keeps a site). Nothing else is sent but the request line + Host (+ SNI, which
//! TLS needs to reach the right site).
use crate::cookie::{self, Jar};
use std::cell::RefCell;
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::sync::Arc;
use std::time::Duration;

/// A connection: plain TCP or TLS over TCP.
trait Conn: Read + Write {}
impl<T: Read + Write> Conn for T {}

fn tls_config() -> Arc<rustls::ClientConfig> {
    static CFG: std::sync::OnceLock<Arc<rustls::ClientConfig>> = std::sync::OnceLock::new();
    CFG.get_or_init(|| {
        let roots = rustls::RootCertStore { roots: webpki_roots::TLS_SERVER_ROOTS.to_vec() };
        let mut c = rustls::ClientConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
            .with_safe_default_protocol_versions().expect("tls versions")
            .with_root_certificates(roots)
            .with_no_client_auth();
        c.alpn_protocols = vec![b"http/1.1".to_vec()];
        Arc::new(c)
    }).clone()
}

/// One response after redirects and content decoding.
pub struct Resp { pub url: String, pub status: u16, pub headers: Vec<(String, String)>, pub body: Vec<u8> }

impl Resp {
    pub fn header(&self, k: &str) -> Option<&str> { self.headers.iter().find(|(h, _)| h == k).map(|(_, v)| v.as_str()) }
}

pub const ACCEPT_DOC: &str = "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8";

pub struct Client {
    conns: RefCell<HashMap<String, BufReader<Box<dyn Conn>>>>,
    pub jar: RefCell<Jar>,
    /// the top-level page's site (registrable domain): cookies flow only to and from it (cookie.rs policy)
    pub site: RefCell<String>,
    pub bytes: std::cell::Cell<usize>, pub requests: std::cell::Cell<usize>, pub wait_s: std::cell::Cell<f64>,
    /// why the last request got no answer (DNS, connect, TLS certificate, protocol), for the person to read
    pub last_error: RefCell<String>,
}

/// (tls, "host:port" key part, host for SNI, port, path-and-query)
fn split_url(url: &str) -> Option<(bool, String, String, u16, String)> {
    let (tls, rest) = match url.strip_prefix("https://") { Some(r) => (true, r), None => (false, url.strip_prefix("http://")?) };
    let rest = rest.split('#').next().unwrap_or("");
    let (hostport, path) = match rest.find(['/', '?']) { Some(i) => (&rest[..i], &rest[i..]), None => (rest, "/") };
    let path = if path.starts_with('?') { format!("/{path}") } else { path.to_string() };
    let (host, port) = if let Some(h) = hostport.strip_prefix('[') {
        let (h, p) = h.split_once(']')?;
        (h.to_string(), p.strip_prefix(':').and_then(|p| p.parse().ok()))
    } else {
        match hostport.rsplit_once(':') { Some((h, p)) => (h.to_string(), p.parse().ok()), None => (hostport.to_string(), None) }
    };
    if host.is_empty() { return None; }
    Some((tls, hostport.to_string(), host, port.unwrap_or(if tls { 443 } else { 80 }), path))
}

/// The User-Agent (requests and navigator.userAgent): Chrome-compatible, as every browser's is — sites sniff it, and a bare
/// product token gets the "your browser is deprecated" page (music.youtube.com, YTMUSIC-0 Y1); an extra product token is also refused there. `OMR_UA` overrides.
pub fn user_agent() -> String {
    std::env::var("OMR_UA").ok().filter(|v| !v.is_empty())
        .unwrap_or_else(|| "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/130.0.0.0 Safari/537.36".to_string())
}

pub fn host_of(url: &str) -> String { cookie::parts(url).map(|p| p.1).unwrap_or_default() }

impl Client {
    pub fn new() -> Client {
        Client { conns: Default::default(), jar: Default::default(), site: Default::default(), bytes: Default::default(), requests: Default::default(), wait_s: Default::default(), last_error: Default::default() }
    }

    /// A subresource GET: the body of a 2xx answer (after redirects), else None.
    pub fn get(&self, url: &str) -> Option<(String, Vec<u8>)> {
        let r = self.fetch("GET", url, None, "*/*", false)?;
        if (200..300).contains(&r.status) { Some((r.url, r.body)) } else { None }
    }

    /// Any request: follows redirects, stores / sends first-party cookies, decodes the body. `nav` = a top-level navigation
    /// (each hop becomes the page's site). Every status is returned (the caller shows error pages); None = no answer.
    pub fn fetch(&self, method: &str, url: &str, body: Option<(String, Vec<u8>)>, accept: &str, nav: bool) -> Option<Resp> {
        let t = std::time::Instant::now();
        self.last_error.borrow_mut().clear();
        let r = self.fetch_inner(method, url, body, accept, nav);
        // OMR_NET_TRACE=1: one stderr line per request (method, status, ms, bytes, url) — finds the request a page waits on
        if std::env::var_os("OMR_NET_TRACE").is_some() {
            eprintln!("net {method} {} {:.0} ms {} B {}", r.as_ref().map(|x| x.status.to_string()).unwrap_or_else(|| format!("ERR {}", self.last_error.borrow())), t.elapsed().as_secs_f64() * 1e3, r.as_ref().map(|x| x.body.len()).unwrap_or(0), url.chars().take(140).collect::<String>());
        }
        if r.is_none() && self.last_error.borrow().is_empty() { *self.last_error.borrow_mut() = "too many redirects".into(); }
        self.wait_s.set(self.wait_s.get() + t.elapsed().as_secs_f64());
        r
    }

    fn fetch_inner(&self, method: &str, url: &str, mut body: Option<(String, Vec<u8>)>, accept: &str, nav: bool) -> Option<Resp> {
        let mut u = url.to_string();
        let mut method = method.to_ascii_uppercase();
        for _ in 0..=20 {
            let host = host_of(&u);
            if nav { *self.site.borrow_mut() = cookie::site_of(&host); }
            let site = { let s = self.site.borrow(); if s.is_empty() { cookie::site_of(&host) } else { s.clone() } };
            let ck = self.jar.borrow_mut().header(&u, &site, false);
            let (status, headers, raw) = self.once(&method, &u, body.as_ref(), accept, &ck)?;
            for (k, v) in &headers { if k == "set-cookie" { self.jar.borrow_mut().set(&u, v, &site, false); } }
            if (300..400).contains(&status) && status != 304 {
                if let Some(loc) = headers.iter().find(|(k, _)| k == "location").map(|(_, v)| v.clone()) {
                    if status == 303 || ((status == 301 || status == 302) && method == "POST") { method = "GET".into(); body = None; }
                    u = crate::page::resolve(&u, &loc);
                    continue;
                }
            }
            let enc = headers.iter().find(|(k, _)| k == "content-encoding").map(|(_, v)| v.to_ascii_lowercase()).unwrap_or_default();
            let body = decode_body(&enc, raw);
            return Some(Resp { url: u, status, headers, body });
        }
        None
    }

    fn once(&self, method: &str, url: &str, body: Option<&(String, Vec<u8>)>, accept: &str, cookie: &str) -> Option<(u16, Vec<(String, String)>, Vec<u8>)> {
        if let Some(rp) = replay() {
            let r = rp.lock().ok()?.take(method, url, body.map(|b| &b.1[..]));
            if r.is_none() { *self.last_error.borrow_mut() = format!("replay: {method} {url} was not recorded"); eprintln!("replay miss {method} {}", url.chars().take(160).collect::<String>()); }
            return r;
        }
        let r = self.once_net(method, url, body, accept, cookie);
        if let (Some(rec), Some((st, h, b))) = (recorder(), &r) { if let Ok(mut g) = rec.lock() { g.put(method, url, body.map(|b| &b.1[..]), *st, h, b); } }
        r
    }

    fn once_net(&self, method: &str, url: &str, body: Option<&(String, Vec<u8>)>, accept: &str, cookie: &str) -> Option<(u16, Vec<(String, String)>, Vec<u8>)> {
        let err = |m: String| { *self.last_error.borrow_mut() = m; };
        let Some((tls, hostport, host, port, path)) = split_url(url) else { err(format!("not an http(s) address: {url}")); return None };
        let key = format!("{}{hostport}", if tls { "s:" } else { "" });
        let ua = user_agent();
        let mut req = format!("{method} {path} HTTP/1.1\r\nHost: {hostport}\r\nUser-Agent: {ua}\r\nAccept: {accept}\r\n\
                               Accept-Language: en-US,en;q=0.8\r\nAccept-Encoding: gzip, deflate, br\r\nConnection: keep-alive\r\n");
        if !cookie.is_empty() { req.push_str(&format!("Cookie: {cookie}\r\n")); }
        if let Some((ct, b)) = body { req.push_str(&format!("Content-Type: {ct}\r\nContent-Length: {}\r\n", b.len())); }
        else if method == "POST" || method == "PUT" { req.push_str("Content-Length: 0\r\n"); }
        req.push_str("\r\n");
        let mut msg = req.into_bytes();
        if let Some((_, b)) = body { msg.extend_from_slice(b); }
        for attempt in 0..2 {
            let mut conns = self.conns.borrow_mut();
            let pooled = attempt == 0 && conns.contains_key(&key);
            if !pooled {
                // RFC 9112 leaves connect time to the client: 10 s per resolved address, so an unreachable host cannot hang a page
                use std::net::ToSocketAddrs;
                let addrs: Vec<std::net::SocketAddr> = match (host.as_str(), port).to_socket_addrs() { Ok(a) => a.collect(), Err(_) => { err(format!("no such host: {host}")); return None } };
                let Some(s) = addrs.iter().find_map(|a| TcpStream::connect_timeout(a, Duration::from_secs(10)).ok()) else { err(format!("{host} did not answer (connection refused or timed out)")); return None };
                s.set_read_timeout(Some(Duration::from_secs(15))).ok();
                s.set_nodelay(true).ok();
                let c: Box<dyn Conn> = if tls {
                    let name = rustls::pki_types::ServerName::try_from(host.clone()).ok()?;
                    let conn = rustls::ClientConnection::new(tls_config(), name).ok()?;
                    Box::new(rustls::StreamOwned::new(conn, s))
                } else { Box::new(s) };
                conns.insert(key.clone(), BufReader::new(c));
            }
            let r = conns.get_mut(&key)?;
            if let Err(e) = r.get_mut().write_all(&msg).and_then(|_| r.get_mut().flush()) { conns.remove(&key); if pooled { continue } else { err(format!("{host}: {e}")); return None } }
            match read_response(r, method == "HEAD") {
                Some((status, h, b, reuse)) => {
                    if !reuse { conns.remove(&key); }
                    self.requests.set(self.requests.get() + 1);
                    self.bytes.set(self.bytes.get() + b.len());
                    return Some((status, h, b));
                }
                None => { conns.remove(&key); if pooled { continue } else { err(format!("{host} sent no valid HTTP answer (or timed out)")); return None } }
            }
        }
        None
    }
}

// ---------------- WebSocket client (RFC 6455): the broker owns the socket; the page talks to it through pageproc ----------------

trait WsStream: Read + Write + Send {}
impl<T: Read + Write + Send> WsStream for T {}

/// What a socket reports to its page: Open, Message(text), Binary(bytes), Close(code, reason), Error(text).
pub enum WsEvent { Open, Message(String), Binary(Vec<u8>), Close(u16, String), Error(String) }

pub struct Ws { stream: Arc<std::sync::Mutex<Box<dyn WsStream>>>, pub rx: std::sync::mpsc::Receiver<WsEvent>, closed: Arc<std::sync::atomic::AtomicBool> }

fn rand_bytes<const N: usize>() -> [u8; N] { let mut b = [0u8; N]; let _ = ring::rand::SecureRandom::fill(&ring::rand::SystemRandom::new(), &mut b); b }

fn b64_enc(b: &[u8]) -> String {
    const T: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut o = String::new();
    for c in b.chunks(3) {
        let n = (c[0] as u32) << 16 | (*c.get(1).unwrap_or(&0) as u32) << 8 | *c.get(2).unwrap_or(&0) as u32;
        for k in 0..4 { if k <= c.len() { o.push(T[(n >> (18 - 6 * k) & 63) as usize] as char); } else { o.push('='); } }
    }
    o
}

/// One frame, client → server (RFC 6455 §5.2): FIN, opcode, MASKED payload with a fresh key (§5.3).
fn ws_frame(op: u8, payload: &[u8]) -> Vec<u8> {
    let mut f = vec![0x80 | op];
    let n = payload.len();
    if n < 126 { f.push(0x80 | n as u8); } else if n < 65536 { f.push(0x80 | 126); f.extend_from_slice(&(n as u16).to_be_bytes()); } else { f.push(0x80 | 127); f.extend_from_slice(&(n as u64).to_be_bytes()); }
    let key: [u8; 4] = rand_bytes();
    f.extend_from_slice(&key);
    f.extend(payload.iter().enumerate().map(|(i, b)| b ^ key[i % 4]));
    f
}

impl Client {
    /// RFC 6455 §4.1 opening handshake to a ws:// / wss:// URL (first-party cookies as for any request), then a reader thread
    /// that turns frames into events (§5.4 fragments joined, §5.5.2 pings answered, §5.5.1 close echoed).
    pub fn websocket(&self, url: &str, origin: &str) -> Result<Ws, String> {
        if replay().is_some() { return Err("replay: websockets are not recorded".into()); }
        let http = if let Some(r) = url.strip_prefix("wss://") { format!("https://{r}") } else if let Some(r) = url.strip_prefix("ws://") { format!("http://{r}") } else { url.to_string() };
        let (tls, hostport, host, port, path) = split_url(&http).ok_or("not a ws:// or wss:// address")?;
        let site = { let s = self.site.borrow(); if s.is_empty() { cookie::site_of(&host) } else { s.clone() } };
        let ck = self.jar.borrow_mut().header(&http, &site, false);
        use std::net::ToSocketAddrs;
        let addrs: Vec<std::net::SocketAddr> = (host.as_str(), port).to_socket_addrs().map_err(|_| format!("no such host: {host}"))?.collect();
        let tcp = addrs.iter().find_map(|a| TcpStream::connect_timeout(a, Duration::from_secs(10)).ok()).ok_or(format!("{host} did not answer"))?;
        tcp.set_nodelay(true).ok();
        tcp.set_read_timeout(Some(Duration::from_secs(15))).ok();
        let tcp2 = tcp.try_clone().map_err(|e| e.to_string())?;
        let mut s: Box<dyn WsStream> = if tls {
            let name = rustls::pki_types::ServerName::try_from(host.clone()).map_err(|e| e.to_string())?;
            let conn = rustls::ClientConnection::new(tls_config(), name).map_err(|e| e.to_string())?;
            Box::new(rustls::StreamOwned::new(conn, tcp))
        } else { Box::new(tcp) };
        let key = b64_enc(&rand_bytes::<16>());
        let ua = user_agent();
        let mut req = format!("GET {path} HTTP/1.1\r\nHost: {hostport}\r\nUser-Agent: {ua}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: {key}\r\nSec-WebSocket-Version: 13\r\nOrigin: {origin}\r\n");
        if !ck.is_empty() { req.push_str(&format!("Cookie: {ck}\r\n")); }
        req.push_str("\r\n");
        s.write_all(req.as_bytes()).and_then(|_| s.flush()).map_err(|e| format!("{host}: {e}"))?;
        // the answer's head, byte by byte (frames may follow it at once)
        let mut head = Vec::new(); let mut b = [0u8; 1];
        while !head.ends_with(b"\r\n\r\n") { if s.read(&mut b).map_err(|e| e.to_string())? == 0 { return Err("closed during the handshake".into()); } head.push(b[0]); if head.len() > 16384 { return Err("handshake too long".into()); } }
        let head = String::from_utf8_lossy(&head).to_string();
        let status = head.split_whitespace().nth(1).unwrap_or("");
        if status != "101" { return Err(format!("handshake refused: HTTP {status}")); }
        let accept = head.lines().find_map(|l| l.split_once(':').filter(|(k, _)| k.trim().eq_ignore_ascii_case("sec-websocket-accept")).map(|(_, v)| v.trim().to_string())).unwrap_or_default();
        let want = b64_enc(ring::digest::digest(&ring::digest::SHA1_FOR_LEGACY_USE_ONLY, format!("{key}258EAFA5-E914-47DA-95CA-C5AB0DC85B11").as_bytes()).as_ref());
        if accept != want { return Err("handshake: wrong Sec-WebSocket-Accept".into()); }
        for l in head.lines() { if let Some((k, v)) = l.split_once(':') { if k.trim().eq_ignore_ascii_case("set-cookie") { self.jar.borrow_mut().set(&http, v.trim(), &site, false); } } }
        tcp2.set_read_timeout(Some(Duration::from_millis(50))).ok();
        let stream = Arc::new(std::sync::Mutex::new(s));
        let (tx, rx) = std::sync::mpsc::channel();
        let closed = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let _ = tx.send(WsEvent::Open);
        let (st, cl) = (stream.clone(), closed.clone());
        std::thread::spawn(move || {
            let mut buf: Vec<u8> = Vec::new();
            let mut msg: Vec<u8> = Vec::new(); let mut msg_op = 0u8;
            let mut chunk = [0u8; 16384];
            loop {
                if cl.load(std::sync::atomic::Ordering::Relaxed) { break; }
                let n = { let mut g = match st.lock() { Ok(g) => g, Err(_) => break }; match g.read(&mut chunk) {
                    Ok(0) => { let _ = tx.send(WsEvent::Close(1006, String::new())); break; }
                    Ok(n) => n,
                    Err(e) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) => 0,
                    Err(e) => { let _ = tx.send(WsEvent::Error(e.to_string())); let _ = tx.send(WsEvent::Close(1006, String::new())); break; }
                } };
                if n == 0 { std::thread::sleep(Duration::from_millis(5)); continue; }
                buf.extend_from_slice(&chunk[..n]);
                // whole frames out of the buffer (§5.2)
                loop {
                    if buf.len() < 2 { break; }
                    let (fin, op, masked) = (buf[0] & 0x80 != 0, buf[0] & 0x0f, buf[1] & 0x80 != 0);
                    let mut len = (buf[1] & 0x7f) as usize; let mut at = 2;
                    if len == 126 { if buf.len() < 4 { break; } len = u16::from_be_bytes([buf[2], buf[3]]) as usize; at = 4; }
                    else if len == 127 { if buf.len() < 10 { break; } len = u64::from_be_bytes(buf[2..10].try_into().unwrap()) as usize; at = 10; }
                    let mk = if masked { at += 4; Some([buf[at - 4], buf[at - 3], buf[at - 2], buf[at - 1]]) } else { None };
                    if len > 64 << 20 { let _ = tx.send(WsEvent::Error("message too large".into())); cl.store(true, std::sync::atomic::Ordering::Relaxed); break; }
                    if buf.len() < at + len { break; }
                    let mut p: Vec<u8> = buf[at..at + len].to_vec();
                    if let Some(k) = mk { for (i, b) in p.iter_mut().enumerate() { *b ^= k[i % 4]; } }
                    buf.drain(..at + len);
                    match op {
                        0 | 1 | 2 => {
                            if op != 0 { msg_op = op; msg.clear(); }
                            msg.extend_from_slice(&p);
                            if fin { let m = std::mem::take(&mut msg); let _ = tx.send(if msg_op == 1 { WsEvent::Message(String::from_utf8_lossy(&m).into_owned()) } else { WsEvent::Binary(m) }); }
                        }
                        8 => {
                            let code = if p.len() >= 2 { u16::from_be_bytes([p[0], p[1]]) } else { 1005 };
                            let reason = if p.len() > 2 { String::from_utf8_lossy(&p[2..]).into_owned() } else { String::new() };
                            if let Ok(mut g) = st.lock() { let _ = g.write_all(&ws_frame(8, &p[..p.len().min(2)])); let _ = g.flush(); }
                            let _ = tx.send(WsEvent::Close(code, reason));
                            cl.store(true, std::sync::atomic::Ordering::Relaxed);
                        }
                        9 => { if let Ok(mut g) = st.lock() { let _ = g.write_all(&ws_frame(10, &p)); let _ = g.flush(); } }
                        _ => {}
                    }
                }
            }
        });
        Ok(Ws { stream, rx, closed })
    }
}

impl Ws {
    pub fn send_text(&self, t: &str) -> bool { self.stream.lock().map(|mut g| g.write_all(&ws_frame(1, t.as_bytes())).and_then(|_| g.flush()).is_ok()).unwrap_or(false) }
    pub fn send_binary(&self, b: &[u8]) -> bool { self.stream.lock().map(|mut g| g.write_all(&ws_frame(2, b)).and_then(|_| g.flush()).is_ok()).unwrap_or(false) }
    /// §5.5.1 close handshake: send Close(code, reason); the reader stops after the peer's Close.
    pub fn close(&self, code: u16, reason: &str) {
        let mut p = code.to_be_bytes().to_vec(); p.extend_from_slice(&reason.as_bytes()[..reason.len().min(123)]);
        if let Ok(mut g) = self.stream.lock() { let _ = g.write_all(&ws_frame(8, &p)); let _ = g.flush(); }
    }
}
impl Drop for Ws { fn drop(&mut self) { self.closed.store(true, std::sync::atomic::Ordering::Relaxed); } }

/// RFC 9112 §6.3 message body length. Returns (status, headers, body, connection reusable).
fn read_response(r: &mut BufReader<Box<dyn Conn>>, head: bool) -> Option<(u16, Vec<(String, String)>, Vec<u8>, bool)> {
    let mut line = String::new();
    loop {
        line.clear();
        if r.read_line(&mut line).ok()? == 0 { return None; }
        let status: u16 = line.split_whitespace().nth(1)?.parse().ok()?;
        let mut h: Vec<(String, String)> = Vec::new();
        loop {
            line.clear();
            r.read_line(&mut line).ok()?;
            let l = line.trim_end();
            if l.is_empty() { break; }
            if let Some((k, v)) = l.split_once(':') { h.push((k.trim().to_ascii_lowercase(), v.trim().to_string())); }
        }
        // 1xx interim answers (100 Continue, 103 Early Hints) precede the real one
        if (100..200).contains(&status) && status != 101 { continue; }
        let hv = |k: &str| h.iter().find(|(x, _)| x == k).map(|(_, v)| v.to_ascii_lowercase());
        let mut reuse = !hv("connection").map(|v| v.contains("close")).unwrap_or(false);
        let mut body = Vec::new();
        if head || status == 204 || status == 304 || (100..200).contains(&status) {
            // rule 1: no content, whatever the headers say
        } else if hv("transfer-encoding").map(|v| v.contains("chunked")).unwrap_or(false) {
            loop {
                line.clear();
                r.read_line(&mut line).ok()?;
                let n = usize::from_str_radix(line.trim().split(';').next()?.trim(), 16).ok()?;
                if n == 0 {
                    // trailer section ends at an empty line
                    loop { line.clear(); if r.read_line(&mut line).ok()? == 0 || line.trim_end().is_empty() { break; } }
                    break;
                }
                let st = body.len();
                body.resize(st + n, 0);
                r.read_exact(&mut body[st..]).ok()?;
                line.clear();
                r.read_line(&mut line).ok()?;
            }
        } else if let Some(n) = hv("content-length").and_then(|v| v.split(',').next().and_then(|x| x.trim().parse::<usize>().ok())) {
            let cap = 64 << 20;
            if n > cap { reuse = false; }
            body.resize(n.min(cap), 0);
            r.read_exact(&mut body).ok()?;
        } else {
            // rule 8: close-delimited. A TLS peer that closes without close_notify reads as UnexpectedEof: the body still ended.
            reuse = false;
            match r.by_ref().take(64 << 20).read_to_end(&mut body) {
                Ok(_) => {}
                Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => {}
                Err(_) if (300..400).contains(&status) => {} // a redirect's body is never read
                Err(_) => return None,
            }
        }
        return Some((status, h, body, reuse));
    }
}

/// RFC 9110 §8.4 content codings, applied in the reverse of the order listed. An undecodable body is passed through.
fn decode_body(enc: &str, mut b: Vec<u8>) -> Vec<u8> {
    for c in enc.split(',').map(|s| s.trim()).rev() {
        let out = match c {
            "gzip" | "x-gzip" => gunzip(&b),
            "deflate" => miniz_oxide::inflate::decompress_to_vec_zlib(&b).ok().or_else(|| miniz_oxide::inflate::decompress_to_vec(&b).ok()),
            "br" => { let mut o = Vec::new(); brotli_decompressor::Decompressor::new(&b[..], 4096).read_to_end(&mut o).ok().map(|_| o) }
            "" | "identity" => None,
            _ => None,
        };
        match out { Some(o) => b = o, None => if !matches!(c, "" | "identity") { return b; } }
    }
    b
}

/// RFC 1952 member header (ID1 ID2 CM FLG MTIME XFL OS [FEXTRA] [FNAME] [FCOMMENT] [FHCRC]) then raw deflate.
fn gunzip(b: &[u8]) -> Option<Vec<u8>> {
    if b.len() < 18 || b[0] != 0x1f || b[1] != 0x8b || b[2] != 8 { return None; }
    let flg = b[3];
    let mut i = 10;
    if flg & 4 != 0 { let xlen = u16::from_le_bytes([*b.get(i)?, *b.get(i + 1)?]) as usize; i += 2 + xlen; }
    if flg & 8 != 0 { while *b.get(i)? != 0 { i += 1; } i += 1; }
    if flg & 16 != 0 { while *b.get(i)? != 0 { i += 1; } i += 1; }
    if flg & 2 != 0 { i += 2; }
    miniz_oxide::inflate::decompress_to_vec(b.get(i..)?).ok()
}

/// windows-1252 bytes 0x80–0x9F (WHATWG Encoding: the labels iso-8859-1 / latin1 / ascii all mean windows-1252).
const W1252: [u16; 32] = [0x20AC, 0x81, 0x201A, 0x0192, 0x201E, 0x2026, 0x2020, 0x2021, 0x02C6, 0x2030, 0x0160, 0x2039, 0x0152, 0x8D, 0x017D, 0x8F,
    0x90, 0x2018, 0x2019, 0x201C, 0x201D, 0x2022, 0x2013, 0x2014, 0x02DC, 0x2122, 0x0161, 0x203A, 0x0153, 0x9D, 0x017E, 0x0178];

fn charset_label(s: &str) -> Option<String> {
    let l = s.to_ascii_lowercase();
    let i = l.find("charset")?;
    let rest = l[i + 7..].trim_start().strip_prefix('=')?.trim_start().trim_start_matches(['"', '\'']);
    let end = rest.find(|c: char| c == '"' || c == '\'' || c == ';' || c == ' ' || c == '>' || c == '/').unwrap_or(rest.len());
    let v = rest[..end].trim();
    if v.is_empty() { None } else { Some(v.to_string()) }
}

/// HTML §13.2.3.2 prescan (simplified): the first `<meta charset>` or `<meta http-equiv=content-type content=…charset=…>`
/// in the first 1024 bytes.
fn prescan(b: &[u8]) -> Option<String> {
    let head = String::from_utf8_lossy(&b[..b.len().min(1024)]).to_ascii_lowercase();
    let mut from = 0;
    while let Some(i) = head[from..].find("<meta") {
        let s = from + i;
        let e = head[s..].find('>').map(|x| s + x).unwrap_or(head.len());
        if let Some(c) = charset_label(&head[s..e]) { return Some(c); }
        from = e;
    }
    None
}

/// A document's bytes as UTF-8 (HTML §13.2.3 encoding sniffing: BOM → HTTP charset → meta prescan → UTF-8 if valid, else
/// windows-1252). Encodings beyond UTF-8 / UTF-16 / windows-1252 / ISO-8859-15 pass through as UTF-8 (lossy).
pub fn to_utf8(b: Vec<u8>, content_type: Option<&str>) -> Vec<u8> {
    if b.starts_with(&[0xEF, 0xBB, 0xBF]) { return b[3..].to_vec(); }
    let utf16 = |be: bool| -> Vec<u8> {
        let u: Vec<u16> = b[2..].chunks_exact(2).map(|c| if be { u16::from_be_bytes([c[0], c[1]]) } else { u16::from_le_bytes([c[0], c[1]]) }).collect();
        String::from_utf16_lossy(&u).into_bytes()
    };
    if b.starts_with(&[0xFF, 0xFE]) { return utf16(false); }
    if b.starts_with(&[0xFE, 0xFF]) { return utf16(true); }
    let label = content_type.and_then(charset_label).or_else(|| prescan(&b)).unwrap_or_default();
    let single = |hi: &dyn Fn(u8) -> char| -> Vec<u8> { b.iter().map(|&c| if c < 0x80 { c as char } else { hi(c) }).collect::<String>().into_bytes() };
    match label.as_str() {
        "iso-8859-1" | "latin1" | "l1" | "us-ascii" | "ascii" | "windows-1252" | "cp1252" | "x-cp1252" | "iso8859-1" | "iso_8859-1" | "cp819" | "ibm819" =>
            single(&|c| if c < 0xA0 { char::from_u32(W1252[(c - 0x80) as usize] as u32).unwrap_or('\u{FFFD}') } else { c as char }),
        "iso-8859-15" | "latin9" | "iso8859-15" | "l9" => single(&|c| match c {
            0xA4 => '€', 0xA6 => 'Š', 0xA8 => 'š', 0xB4 => 'Ž', 0xB8 => 'ž', 0xBC => 'Œ', 0xBD => 'œ', 0xBE => 'Ÿ', _ => c as char }),
        _ => {
            if std::str::from_utf8(&b).is_ok() || label.contains("utf") || !label.is_empty() { b }
            else { single(&|c| if c < 0xA0 { char::from_u32(W1252[(c - 0x80) as usize] as u32).unwrap_or('\u{FFFD}') } else { c as char }) }
        }
    }
}

/// RFC 2397 data: URL → bytes (base64 or percent-encoded).
pub fn data_url(u: &str) -> Option<Vec<u8>> {
    let rest = u.strip_prefix("data:")?;
    let (meta, data) = rest.split_once(',')?;
    if meta.to_ascii_lowercase().ends_with(";base64") {
        let s: Vec<u8> = data.bytes().filter(|c| !c.is_ascii_whitespace()).collect();
        let s = pct(&s);
        b64(&s)
    } else { Some(pct(data.as_bytes())) }
}

fn pct(b: &[u8]) -> Vec<u8> {
    let mut o = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            if let Ok(v) = u8::from_str_radix(std::str::from_utf8(&b[i + 1..i + 3]).unwrap_or("zz"), 16) { o.push(v); i += 3; continue; }
        }
        o.push(b[i]); i += 1;
    }
    o
}

/// RFC 4648 base64 (standard or URL alphabet, padding optional).
pub fn b64(s: &[u8]) -> Option<Vec<u8>> {
    let val = |c: u8| -> Option<u32> { Some(match c { b'A'..=b'Z' => c - b'A', b'a'..=b'z' => c - b'a' + 26, b'0'..=b'9' => c - b'0' + 52, b'+' | b'-' => 62, b'/' | b'_' => 63, _ => return None } as u32) };
    let s: Vec<u8> = s.iter().copied().filter(|&c| c != b'=').collect();
    let mut o = Vec::with_capacity(s.len() * 3 / 4);
    for ch in s.chunks(4) {
        let mut n = 0u32;
        for (k, &c) in ch.iter().enumerate() { n |= val(c)? << (18 - 6 * k); }
        o.push((n >> 16) as u8);
        if ch.len() > 2 { o.push((n >> 8) as u8); }
        if ch.len() > 3 { o.push(n as u8); }
        if ch.len() == 1 { return None; }
    }
    Some(o)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn codings_and_charsets() {
        assert_eq!(to_utf8(b"caf\xe9".to_vec(), Some("text/html; charset=ISO-8859-1")), "café".as_bytes());
        assert_eq!(to_utf8(b"<meta charset=\"windows-1252\">\x93q\x94".to_vec(), None), "<meta charset=\"windows-1252\">\u{201C}q\u{201D}".as_bytes());
        assert_eq!(to_utf8("ok é".as_bytes().to_vec(), None), "ok é".as_bytes());
        assert_eq!(data_url("data:text/plain;base64,aGk=").unwrap(), b"hi");
        assert_eq!(data_url("data:,a%20b").unwrap(), b"a b");
        assert_eq!(split_url("http://[::1]:8080/x?y").map(|p| (p.2, p.3, p.4)), Some(("::1".into(), 8080, "/x?y".into())));
    }
}

// ---------------- record / replay (JS-COMPILE-0 step 1): a page's exact network bytes, kept as a doctor-format bundle ----------------
// OMR_RECORD=DIR: every HTTP exchange (each redirect hop) is written to DIR/res/NNN.<kind> (raw body, before content
// decoding) + DIR/exchanges.jsonl (method, url, request-body hash, status, headers, file), and DIR/manifest.json keeps the
// doctor corpus shape ({url, final_url, res: {url: {file, type, ctype, bytes}}}, tools/fetch_corpus.py) so doctor tools read
// it. OMR_REPLAY=DIR: answers come only from the bundle — same request (method + url + body hash) gets its recorded answers in
// order (the last one repeats); a body-hash miss falls back to method + url; anything else is a miss (stderr "replay miss").
// Websockets are refused under replay. Both run in the broker, the process that owns the network.

fn body_key(b: Option<&[u8]>) -> String {
    match b { None => "-".into(), Some(b) => { let d = ring::digest::digest(&ring::digest::SHA256, b); d.as_ref()[..8].iter().map(|x| format!("{x:02x}")).collect() } }
}
fn kind_of(ctype: &str, url: &str) -> &'static str {
    let c = ctype.to_ascii_lowercase(); let u = url.split('?').next().unwrap_or("").to_ascii_lowercase();
    if c.contains("javascript") || c.contains("ecmascript") || u.ends_with(".js") || u.ends_with(".mjs") { "script" }
    else if c.contains("text/css") || u.ends_with(".css") { "css" }
    else if c.contains("html") { "doc" }
    else if c.starts_with("image/") { "img" }
    else if c.contains("font") || u.ends_with(".woff2") || u.ends_with(".woff") || u.ends_with(".ttf") { "font" }
    else if c.contains("json") { "json" } else { "data" }
}

pub struct Recorder { dir: std::path::PathBuf, n: usize, url: String, final_url: String, res: serde_json::Map<String, serde_json::Value>, ex: std::fs::File }
impl Recorder {
    fn put(&mut self, method: &str, url: &str, body: Option<&[u8]>, status: u16, headers: &[(String, String)], raw: &[u8]) {
        let ctype = headers.iter().find(|(k, _)| k == "content-type").map(|(_, v)| v.clone()).unwrap_or_default();
        let kind = kind_of(&ctype, url);
        let file = format!("{:04}.{kind}", self.n); self.n += 1;
        let _ = std::fs::write(self.dir.join("res").join(&file), raw);
        let row = serde_json::json!({"method": method, "url": url, "body": body_key(body), "status": status, "headers": headers, "file": file});
        let _ = writeln!(self.ex, "{row}");
        if self.url.is_empty() { self.url = url.to_string(); }
        if kind == "doc" && self.final_url.is_empty() && (200..300).contains(&status) { self.final_url = url.to_string(); }
        self.res.entry(url.to_string()).or_insert(serde_json::json!({"file": file, "type": kind, "ctype": ctype, "bytes": raw.len()}));
        let m = serde_json::json!({"url": self.url, "final_url": if self.final_url.is_empty() { &self.url } else { &self.final_url }, "res": self.res, "recorded_by": "omni-render OMR_RECORD", "exchanges": "exchanges.jsonl"});
        let _ = std::fs::write(self.dir.join("manifest.json"), serde_json::to_vec_pretty(&m).unwrap_or_default());
    }
}
pub fn recorder() -> Option<&'static std::sync::Mutex<Recorder>> {
    static R: std::sync::OnceLock<Option<std::sync::Mutex<Recorder>>> = std::sync::OnceLock::new();
    R.get_or_init(|| {
        let dir = std::path::PathBuf::from(std::env::var_os("OMR_RECORD")?);
        std::fs::create_dir_all(dir.join("res")).ok()?;
        let ex = std::fs::OpenOptions::new().create(true).append(true).open(dir.join("exchanges.jsonl")).ok()?;
        let n = std::fs::read_dir(dir.join("res")).map(|d| d.count()).unwrap_or(0);
        Some(std::sync::Mutex::new(Recorder { dir, n, url: String::new(), final_url: String::new(), res: Default::default(), ex }))
    }).as_ref()
}

pub struct Replay { dir: std::path::PathBuf, rows: Vec<serde_json::Value>, exact: HashMap<String, (Vec<usize>, usize)>, loose: HashMap<String, (Vec<usize>, usize)> }
impl Replay {
    fn take(&mut self, method: &str, url: &str, body: Option<&[u8]>) -> Option<(u16, Vec<(String, String)>, Vec<u8>)> {
        let m = method.to_ascii_uppercase();
        let pick = |t: &mut HashMap<String, (Vec<usize>, usize)>, k: &str| -> Option<usize> { let e = t.get_mut(k)?; let i = e.0[e.1.min(e.0.len() - 1)]; e.1 += 1; Some(i) };
        let i = pick(&mut self.exact, &format!("{m} {url} {}", body_key(body))).or_else(|| pick(&mut self.loose, &format!("{m} {url}")))?;
        let r = &self.rows[i];
        let headers = r["headers"].as_array()?.iter().filter_map(|h| Some((h[0].as_str()?.to_string(), h[1].as_str()?.to_string()))).collect();
        let raw = std::fs::read(self.dir.join("res").join(r["file"].as_str()?)).ok()?;
        Some((r["status"].as_u64()? as u16, headers, raw))
    }
}
pub fn replay() -> Option<&'static std::sync::Mutex<Replay>> {
    static R: std::sync::OnceLock<Option<std::sync::Mutex<Replay>>> = std::sync::OnceLock::new();
    R.get_or_init(|| {
        let dir = std::path::PathBuf::from(std::env::var_os("OMR_REPLAY")?);
        let text = std::fs::read_to_string(dir.join("exchanges.jsonl")).ok()?;
        let rows: Vec<serde_json::Value> = text.lines().filter_map(|l| serde_json::from_str(l).ok()).collect();
        let (mut exact, mut loose) = (HashMap::<String, (Vec<usize>, usize)>::new(), HashMap::<String, (Vec<usize>, usize)>::new());
        for (i, r) in rows.iter().enumerate() {
            let (m, u, b) = (r["method"].as_str().unwrap_or(""), r["url"].as_str().unwrap_or(""), r["body"].as_str().unwrap_or("-"));
            exact.entry(format!("{m} {u} {b}")).or_default().0.push(i);
            loose.entry(format!("{m} {u}")).or_default().0.push(i);
        }
        eprintln!("replay: {} exchanges from {}", rows.len(), dir.display());
        Some(std::sync::Mutex::new(Replay { dir, rows, exact, loose }))
    }).as_ref()
}
