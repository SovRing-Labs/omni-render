// SPDX-License-Identifier: MIT OR Apache-2.0
//! Derivation recorder for `omni-render explain`: layout reports how it computed ONE watched node's size and position,
//! term by term, and every later move applied to it. Off by default; a disabled check is one thread-local load.

use std::cell::{Cell, RefCell};

thread_local! {
    static WATCH: Cell<u32> = const { Cell::new(u32::MAX) };
    static LOG: RefCell<Vec<(&'static str, String)>> = const { RefCell::new(Vec::new()) };
}

/// Start recording for node `n` (u32::MAX = off).
pub fn watch(n: u32) { WATCH.with(|w| w.set(n)); LOG.with(|l| l.borrow_mut().clear()); }
pub fn watched() -> u32 { WATCH.with(|w| w.get()) }
#[inline] pub fn on(n: u32) -> bool { WATCH.with(|w| w.get() == n) }
pub fn note(stage: &'static str, msg: String) { LOG.with(|l| l.borrow_mut().push((stage, msg))); }
pub fn take() -> Vec<(&'static str, String)> { LOG.with(|l| std::mem::take(&mut *l.borrow_mut())) }

/// Fixed-point (1/64 px) → px for messages.
pub fn px(v: i32) -> String {
    if v == i32::MAX { return "∞".into(); }
    let f = v as f64 / 64.0;
    if (f - f.round()).abs() < 1e-9 { format!("{}", f.round() as i64) } else { format!("{f:.2}") }
}

/// `why!(node, "stage", "fmt", args…)` — formats only when `node` is the watched one.
#[macro_export]
macro_rules! why {
    ($n:expr, $stage:expr, $($a:tt)*) => { if $crate::why::on($n) { $crate::why::note($stage, format!($($a)*)); } };
}
