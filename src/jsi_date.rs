// SPDX-License-Identifier: MIT OR Apache-2.0
// Date support for src/jsi.rs: calendar maths in UTC, local-time offsets via libc::localtime_r.
//
// The launcher formats timestamps in every feed row (`toLocaleTimeString`, `toISOString`), so a Date
// without instance methods was worth ~26 runtime errors across AgentManagerPopout, ControlCenter and
// FlightMatrix. Formatting must agree with a real browser to compare boxes, hence local time via libc
// rather than a UTC-only shortcut.
use crate::jsi::{st, Ctl, Interp, Kind, Val};
use std::rc::Rc;

const MS_DAY: i64 = 86_400_000;

/// Broken-down local time, as JS exposes it.
#[derive(Clone, Copy)]
pub struct Parts { pub year: i64, pub mon: i64, pub day: i64, pub h: i64, pub mi: i64, pub s: i64, pub ms: i64, pub wday: i64 }

/// Days since the epoch for a proleptic-Gregorian civil date (Howard Hinnant's algorithm).
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// Split a UTC millisecond timestamp into UTC calendar fields.
pub fn utc_parts(ms: i64) -> Parts {
    let (days, rem) = (ms.div_euclid(MS_DAY), ms.rem_euclid(MS_DAY));
    let (year, mon, day) = civil_from_days(days);
    // 1970-01-01 was a Thursday (wday 4)
    Parts { year, mon, day, h: rem / 3_600_000, mi: (rem / 60_000) % 60, s: (rem / 1000) % 60, ms: rem % 1000, wday: (days + 4).rem_euclid(7) }
}

/// Local calendar fields, honouring the host timezone (the launcher prints local wall-clock times).
pub fn local_parts(ms: i64) -> Parts {
    let secs = ms.div_euclid(1000);
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    let t = secs as libc::time_t;
    if unsafe { libc::localtime_r(&t, &mut tm) }.is_null() { return utc_parts(ms); }
    Parts {
        year: tm.tm_year as i64 + 1900, mon: tm.tm_mon as i64 + 1, day: tm.tm_mday as i64,
        h: tm.tm_hour as i64, mi: tm.tm_min as i64, s: tm.tm_sec as i64, ms: ms.rem_euclid(1000),
        wday: tm.tm_wday as i64 % 7,
    }
}

/// Days in a month (1-based), honouring the proleptic Gregorian leap rule.
fn days_in_month(y: i64, m: i64) -> i64 {
    match m {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 => if (y % 4 == 0 && y % 100 != 0) || y % 400 == 0 { 29 } else { 28 },
        _ => 30,
    }
}

/// Normalise a 0-based month index and a possibly out-of-range day into a real (y, m, d).
/// Day 0 means the last day of the previous month; day 32 means the 1st of the next.
fn normalize_ymd(y: i64, m0: i64, d: i64) -> (i64, i64, i64) {
    let total = y * 12 + m0;
    let (yy, mm) = (total.div_euclid(12), total.rem_euclid(12) + 1);
    // per spec, out-of-range days roll into the next month rather than clamping
    let mut dd = d;
    let (mut yy, mut mm) = (yy, mm);
    if dd < 1 {
        mm -= 1;
        if mm < 1 { mm = 12; yy -= 1; }
        dd += days_in_month(yy, mm);
    } else {
        let dim = days_in_month(yy, mm);
        while dd > dim {
            dd -= dim;
            mm += 1;
            if mm > 12 { mm = 1; yy += 1; }
        }
    }
    (yy, mm, dd)
}

/// JS `new Date(y, m, d, ...)` applies the local offset; `Date.UTC` and `new Date(ms)` do not.
fn local_offset_ms(ms: i64) -> i64 {
    let secs = ms.div_euclid(1000);
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    let t = secs as libc::time_t;
    if unsafe { libc::localtime_r(&t, &mut tm) }.is_null() { return 0; }
    tm.tm_gmtoff as i64 * 1000
}

/// Parse the shapes JS must accept: ISO 8601 with Z or ±hh:mm, `YYYY-MM-DD[ HH:MM[:SS[.sss]]]`,
/// and `Date.parse`-style fallbacks. Returns ms since epoch.
pub fn parse_date(s: &str) -> Option<i64> {
    let s = s.trim();
    let b = s.as_bytes();
    let num = |a: usize, z: usize| -> Option<i64> { s.get(a..z)?.trim().parse::<i64>().ok() };
    if b.len() < 4 || !b[0].is_ascii_digit() { return None; }
    // Date Time String Format always carries a `-` after the year. A bare run of digits (or anything
    // shaped like a timestamp) is Invalid Date in a real browser, so it must not parse here either.
    if b.len() >= 5 && b[4] != b'-' { return None; }
    let y = num(0, 4)?;
    if b.len() == 4 { return Some(days_from_civil(y, 1, 1) * MS_DAY); }
    if b[4] != b'-' { return None; }
    let mo = num(5, 7)?;
    if b.len() < 10 { return None; }
    if b[7] != b'-' { return None; }
    let d = num(8, 10)?;
    let mut ms = days_from_civil(y, mo, d) * MS_DAY;
    if b.len() == 10 { return Some(ms); }
    if b[10] != b'T' && b[10] != b' ' { return None; }
    ms += num(11, 13)? * 3_600_000 + num(14, 16)? * 60_000;
    if b.len() >= 19 && b[16] == b':' { ms += num(17, 19)? * 1000; }
    if b.len() >= 23 && b[19] == b'.' { ms += num(20, 23)?; }
    // zone: absent = local wall-clock (spec); Z / ±hh:mm = absolute, no local conversion
    let tail = &s[(if b.len() >= 23 && b[19] == b'.' { 23 } else if b.len() >= 19 && b[16] == b':' { 19 } else { 16 })..];
    let tail = tail.trim();
    if tail.is_empty() { return Some(ms - local_offset_ms(ms)); }
    if tail == "Z" || tail == "z" { return Some(ms); }
    let (sign, rest) = tail.split_at(1);
    let sign = if sign == "+" { 1 } else if sign == "-" { -1 } else { return None };
    let (hh, mm) = match rest.split_once(':') { Some((h, m)) => (h, m), None => (rest, "0") };
    let off = sign * (hh.parse::<i64>().ok()? * 3_600_000 + mm.parse::<i64>().ok()? * 60_000);
    Some(ms - off)
}

const MONTHS: [&str; 12] = ["January", "February", "March", "April", "May", "June", "July", "August", "September", "October", "November", "December"];
const DAYS: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];

fn ord(n: i64) -> String { let o = ["th", "st", "nd", "rd"]; let v = n % 100; let s = if (11..=13).contains(&v) { "th" } else { o[(v as usize % 10).min(3)] }; format!("{n}{s}") }

/// `toLocaleDateString` / `toLocaleTimeString` / `toLocaleString` honour the few options the launcher
/// actually passes (`hour`/`minute`/`2-digit`) and otherwise mirror the en-US default.
pub fn fmt_local(ms: i64, kind: u8) -> String {
    let p = local_parts(ms);
    match kind {
        0 => format!("{}/{}/{}", p.mon, p.day, p.year),
        1 => {
            let h12 = if p.h % 12 == 0 { 12 } else { p.h % 12 };
            let ap = if p.h < 12 { "AM" } else { "PM" };
            format!("{h12}:{:02} {ap}", p.mi)
        }
        _ => format!("{}/{}/{}, {}", p.mon, p.day, p.year, fmt_local(ms, 1)),
    }
}


fn num(n: f64) -> Val { Val::Num(n) }
fn mon3(m: i64) -> &'static str { &MONTHS[((m - 1).clamp(0, 11)) as usize][..3] }

/// TimeClip (ECMA-262 §21.4.1.31).
fn time_clip(t: f64) -> f64 { if !t.is_finite() || t.abs() > 8.64e15 { f64::NAN } else { t.trunc() + 0.0 } }
/// MakeDay + MakeTime + MakeDate (§21.4.1.28–30): month and day overflow roll over arithmetically.
fn make(y: f64, m0: f64, d: f64, h: f64, mi: f64, s: f64, ms: f64) -> f64 {
    if ![y, m0, d, h, mi, s, ms].iter().all(|x| x.is_finite()) { return f64::NAN; }
    let (y, m0, d) = (y.trunc(), m0.trunc(), d.trunc());
    let ym = y + (m0 / 12.0).floor();
    let mn = m0.rem_euclid(12.0);
    let days = days_from_civil(ym as i64, mn as i64 + 1, 1) as f64 + d - 1.0;
    days * MS_DAY as f64 + h.trunc() * 3_600_000.0 + mi.trunc() * 60_000.0 + s.trunc() * 1000.0 + ms.trunc()
}
fn local_to_utc(t: f64) -> f64 { if t.is_finite() { t - local_offset_ms(t as i64) as f64 } else { t } }

/// A Date object (§21.4.4): [[DateValue]] in Kind::Date; every method lives on Date.prototype.
pub fn date_obj(ms: i64) -> Val { Val::obj(Kind::Date(if ms == i64::MIN { f64::NAN } else { ms as f64 })) }

fn tv(it: &mut Interp, this: &Val) -> Result<f64, Ctl> {
    if let Val::Obj(o) = this { if let Kind::Date(t) = o.borrow().kind { return Ok(t); } }
    Err(Ctl::Throw(it.error("TypeError", "this is not a Date object.")))
}
fn set_tv(this: &Val, t: f64) -> Val { let t = time_clip(t); if let Val::Obj(o) = this { if let Kind::Date(x) = &mut o.borrow_mut().kind { *x = t; } } num(t) }
fn iso(t: f64) -> String { let p = utc_parts(t as i64); let y = if (0..=9999).contains(&p.year) { format!("{:04}", p.year) } else if p.year < 0 { format!("-{:06}", -p.year) } else { format!("+{:06}", p.year) }; format!("{y}-{:02}-{:02}T{:02}:{:02}:{:02}.{:03}Z", p.mon, p.day, p.h, p.mi, p.s, p.ms) }
fn tz_name(t: i64) -> String {
    let off = local_offset_ms(t) / 60_000;
    let sign = if off < 0 { '-' } else { '+' };
    format!("GMT{sign}{:02}{:02}{}", off.abs() / 60, off.abs() % 60, if off == 0 { " (Coordinated Universal Time)" } else { "" })
}
fn to_string(t: f64) -> String { if t.is_nan() { return "Invalid Date".into(); } let p = local_parts(t as i64); format!("{} {} {:02} {:04} {:02}:{:02}:{:02} {}", DAYS[p.wday as usize], mon3(p.mon), p.day, p.year, p.h, p.mi, p.s, tz_name(t as i64)) }

/// Date.prototype methods (§21.4.4). Local-time fields use the host timezone; UTC ones do not.
pub fn date_proto() -> Vec<(&'static str, Val)> {
    let g = |name: &'static str, utc: bool, f: fn(&Parts) -> i64| (name, Val::native(name, move |it, this, _a| { let t = tv(it, &this)?; if t.is_nan() { return Ok(num(f64::NAN)); } let p = if utc { utc_parts(t as i64) } else { local_parts(t as i64) }; Ok(num(f(&p) as f64)) }));
    // setters: which fields the arguments replace (0 year, 1 month, 2 date, 3 hours, 4 minutes, 5 seconds, 6 ms) and how many
    let s = |name: &'static str, utc: bool, first: usize, max: usize| (name, Val::native(name, move |it, this, a| {
        let t = tv(it, &this)?;
        let base = if t.is_nan() { if first == 0 { 0.0 } else { return Ok(num(f64::NAN)); } } else { t };
        let p = if utc { utc_parts(base as i64) } else { local_parts(base as i64) };
        let mut f = [p.year as f64, (p.mon - 1) as f64, p.day as f64, p.h as f64, p.mi as f64, p.s as f64, p.ms as f64];
        for i in 0..max.min(a.len().max(1)) { f[first + i] = it.to_num(a.get(i).unwrap_or(&Val::Undef))?; }
        let lt = make(f[0], f[1], f[2], f[3], f[4], f[5], f[6]);
        Ok(set_tv(&this, if utc { lt } else { local_to_utc(lt) }))
    }));
    vec![
        ("getTime", Val::native("getTime", |it, this, _a| Ok(num(tv(it, &this)?)))),
        ("valueOf", Val::native("valueOf", |it, this, _a| Ok(num(tv(it, &this)?)))),
        ("setTime", Val::native("setTime", |it, this, a| { tv(it, &this)?; let t = it.to_num(a.first().unwrap_or(&Val::Undef))?; Ok(set_tv(&this, t)) })),
        ("getTimezoneOffset", Val::native("getTimezoneOffset", |it, this, _a| { let t = tv(it, &this)?; Ok(num(if t.is_nan() { f64::NAN } else { -(local_offset_ms(t as i64) / 60_000) as f64 })) })),
        g("getFullYear", false, |p| p.year), g("getUTCFullYear", true, |p| p.year), g("getYear", false, |p| p.year - 1900),
        g("getMonth", false, |p| p.mon - 1), g("getUTCMonth", true, |p| p.mon - 1),
        g("getDate", false, |p| p.day), g("getUTCDate", true, |p| p.day),
        g("getDay", false, |p| p.wday), g("getUTCDay", true, |p| p.wday),
        g("getHours", false, |p| p.h), g("getUTCHours", true, |p| p.h),
        g("getMinutes", false, |p| p.mi), g("getUTCMinutes", true, |p| p.mi),
        g("getSeconds", false, |p| p.s), g("getUTCSeconds", true, |p| p.s),
        g("getMilliseconds", false, |p| p.ms), g("getUTCMilliseconds", true, |p| p.ms),
        s("setFullYear", false, 0, 3), s("setUTCFullYear", true, 0, 3), s("setMonth", false, 1, 2), s("setUTCMonth", true, 1, 2),
        s("setDate", false, 2, 1), s("setUTCDate", true, 2, 1), s("setHours", false, 3, 4), s("setUTCHours", true, 3, 4),
        s("setMinutes", false, 4, 3), s("setUTCMinutes", true, 4, 3), s("setSeconds", false, 5, 2), s("setUTCSeconds", true, 5, 2),
        s("setMilliseconds", false, 6, 1), s("setUTCMilliseconds", true, 6, 1),
        ("toISOString", Val::native("toISOString", |it, this, _a| { let t = tv(it, &this)?; if t.is_nan() { return Err(Ctl::Throw(it.error("RangeError", "Invalid time value"))); } Ok(st(&iso(t))) })),
        ("toJSON", Val::native("toJSON", |it, this, _a| { let t = match &this { Val::Obj(o) => match o.borrow().kind { Kind::Date(t) => t, _ => f64::NAN }, _ => f64::NAN }; if t.is_nan() { let f = it.get(&this, "toISOString")?; if f.is_callable() && !matches!(&this, Val::Obj(o) if matches!(o.borrow().kind, Kind::Date(_))) { return it.call(&f, this, vec![]); } return Ok(Val::Null); } Ok(st(&iso(t))) })),
        ("toDateString", Val::native("toDateString", |it, this, _a| { let t = tv(it, &this)?; if t.is_nan() { return Ok(st("Invalid Date")); } let p = local_parts(t as i64); Ok(st(&format!("{} {} {:02} {:04}", DAYS[p.wday as usize], mon3(p.mon), p.day, p.year))) })),
        ("toUTCString", Val::native("toUTCString", |it, this, _a| { let t = tv(it, &this)?; if t.is_nan() { return Ok(st("Invalid Date")); } let p = utc_parts(t as i64); Ok(st(&format!("{}, {:02} {} {:04} {:02}:{:02}:{:02} GMT", DAYS[p.wday as usize], p.day, mon3(p.mon), p.year, p.h, p.mi, p.s))) })),
        ("toGMTString", Val::native("toGMTString", |it, this, _a| { let t = tv(it, &this)?; if t.is_nan() { return Ok(st("Invalid Date")); } let p = utc_parts(t as i64); Ok(st(&format!("{}, {:02} {} {:04} {:02}:{:02}:{:02} GMT", DAYS[p.wday as usize], p.day, mon3(p.mon), p.year, p.h, p.mi, p.s))) })),
        ("toLocaleDateString", Val::native("toLocaleDateString", |it, this, _a| { let t = tv(it, &this)?; Ok(st(&if t.is_nan() { "Invalid Date".into() } else { fmt_local(t as i64, 0) })) })),
        ("toLocaleTimeString", Val::native("toLocaleTimeString", |it, this, _a| { let t = tv(it, &this)?; Ok(st(&if t.is_nan() { "Invalid Date".into() } else { fmt_local(t as i64, 1) })) })),
        ("toLocaleString", Val::native("toLocaleString", |it, this, _a| { let t = tv(it, &this)?; Ok(st(&if t.is_nan() { "Invalid Date".into() } else { fmt_local(t as i64, 2) })) })),
        ("toString", Val::native("toString", |it, this, _a| { let t = tv(it, &this)?; Ok(st(&to_string(t))) })),
        ("toTimeString", Val::native("toTimeString", |it, this, _a| { let t = tv(it, &this)?; if t.is_nan() { return Ok(st("Invalid Date")); } let p = local_parts(t as i64); Ok(st(&format!("{:02}:{:02}:{:02} {}", p.h, p.mi, p.s, tz_name(t as i64)))) })),
    ]
}

/// OMR_CLOCK=virtual (JS-COMPILE-0 step 2): page time becomes a pure function of the run — a fixed epoch, plus the
/// timer clock the page has advanced (`run_timers`), plus 1 µs per clock read — so two runs of one build on the same
/// replayed bytes (OMR_REPLAY) do the same work. Busy-waits on Date.now still end (time moves with every read).
pub fn vclock() -> bool { static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new(); *ON.get_or_init(|| std::env::var("OMR_CLOCK").map_or(false, |v| v == "virtual")) }
thread_local! { static VT: std::cell::Cell<(f64, u64)> = const { std::cell::Cell::new((0.0, 0)) }; }
/// the timer clock moved by `ms` (virtual mode only)
pub fn vclock_advance(ms: f64) { if vclock() { VT.with(|v| { let (a, n) = v.get(); v.set((a + ms, n)); }); } }
/// ms since the page's time origin on the virtual clock; each read advances it by 1 µs
pub fn vclock_ms() -> f64 { VT.with(|v| { let (a, n) = v.get(); v.set((a, n + 1)); a + (n + 1) as f64 * 0.001 }) }
/// 2026-01-01T00:00:00Z: the virtual clock's epoch
pub const VEPOCH_MS: f64 = 1_767_225_600_000.0;

pub fn now_ms() -> i64 {
    if vclock() { return (VEPOCH_MS + vclock_ms()) as i64; }
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}

/// `Date(...)` (§21.4.2.1): called as a function → the current time as a string; constructed: no args → now;
/// one arg → a time value, a Date's value, or a parseable string; 2+ args → local wall-clock fields (0-based month).
pub fn date_ctor(it: &mut Interp, this: Val, a: Vec<Val>) -> Result<Val, Ctl> {
    if !crate::jsi::is_new(&this) { return Ok(st(&to_string(now_ms() as f64))); }
    let t = match a.len() {
        0 => now_ms() as f64,
        1 => {
            let v = match &a[0] { Val::Obj(o) => match o.borrow().kind { Kind::Date(t) => Some(t), _ => None }, _ => None };
            match v {
                Some(t) => t,
                None => match it.to_primitive(&a[0], 0)? {
                    // a String is parsed only in Date Time String Format; anything else is Invalid Date
                    Val::Str(s) => parse_date(&s).map_or(f64::NAN, |m| m as f64),
                    p => it.to_num(&p)?,
                },
            }
        }
        _ => {
            let mut f = [f64::NAN, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0];
            for (i, slot) in f.iter_mut().enumerate() { if i < a.len() { *slot = it.to_num(&a[i])?; } }
            if f[0].is_finite() { let y = f[0].trunc(); if (0.0..=99.0).contains(&y) { f[0] = 1900.0 + y; } }
            local_to_utc(make(f[0], f[1], f[2], f[3], f[4], f[5], f[6]))
        }
    };
    Ok(Val::obj(Kind::Date(time_clip(t))))
}

pub fn date_statics() -> Vec<(Rc<str>, Val)> {
    vec![
        (Rc::from("now"), Val::native("now", |_i, _t, _a| Ok(num(now_ms() as f64)))),
        (Rc::from("parse"), Val::native("parse", |it, _t, a| Ok(match arg_str(it, &a, 0) { Some(s) => num(parse_date(&s).map_or(f64::NAN, |m| m as f64)), None => num(f64::NAN) }))),
        (Rc::from("UTC"), Val::native("UTC", |it, _t, a| {
            let mut f = [f64::NAN, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0];
            for (i, slot) in f.iter_mut().enumerate() { if i < a.len() { *slot = it.to_num(&a[i])?; } }
            if f[0].is_finite() { let y = f[0].trunc(); if (0.0..=99.0).contains(&y) { f[0] = 1900.0 + y; } }
            Ok(num(time_clip(make(f[0], f[1], f[2], f[3], f[4], f[5], f[6]))))
        })),
    ]
}

fn arg_str(it: &mut Interp, a: &[Val], i: usize) -> Option<String> {
    match a.get(i) { Some(Val::Str(s)) => Some(s.to_string()), Some(v) => it.to_str(v).ok().map(|x| x.to_string()), None => None }
}

pub fn date_native() -> Val {
    Val::native("Date", |it, t, a| date_ctor(it, t, a))
}
