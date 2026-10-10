// SPDX-License-Identifier: MIT OR Apache-2.0
//! No-op stand-ins for the two hooks `Layout::run` calls. The real T25 core (PolyForm side, `t25.rs`) is built only with `--features t25`.
pub fn fire(_lay: &crate::layout::Layout, _layout_us: f64) {}
pub fn fire_pristine(_lay: &crate::layout::Layout, _layout_us: f64) {}
