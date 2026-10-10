// SPDX-License-Identifier: MIT OR Apache-2.0
fn js_mod(x: f64, y: f64) -> f64 {
    const LIM: f64 = 9007199254740992.0;
    if x.fract() == 0.0 && y.fract() == 0.0 && x.abs() < LIM && y.abs() < LIM && y != 0.0 {
        let r = (x as i64 % y as i64) as f64;
        return if r == 0.0 && x.is_sign_negative() { -0.0 } else { r };
    }
    if y.is_infinite() && x.is_finite() { x } else { x % y }
}
fn reference(x: f64, y: f64) -> f64 { if y.is_infinite() && x.is_finite() { x } else { x % y } }
fn main() {
    let mut s: u64 = 0x9e3779b97f4a7c15; let mut bad = 0u64; let mut n = 0u64;
    let specials = [0.0, -0.0, 1.0, -1.0, 2.0, -2.0, 3.0, 7.0, -7.0, 1e15, -1e15, 9007199254740991.0, -9007199254740991.0, 9007199254740992.0, 1e300, f64::INFINITY, f64::NEG_INFINITY, f64::NAN, 0.5, -0.5, 4294967296.0];
    for &x in &specials { for &y in &specials { n += 1; let (a, b) = (js_mod(x, y), reference(x, y)); if a.to_bits() != b.to_bits() && !(a.is_nan() && b.is_nan()) { bad += 1; println!("x={x} y={y} got={a} want={b}"); } } }
    for _ in 0..20_000_000 {
        s ^= s << 13; s ^= s >> 7; s ^= s << 17;
        let k = (s >> 60) as u32; let mut x = ((s >> 11) as i64 >> (k % 50)) as f64; let mut y = (((s.rotate_left(29)) as i64) >> (k % 60 + 3)) as f64;
        if k & 1 == 1 { x = -x; } if k & 2 == 2 { y = -y; }
        n += 1; let (a, b) = (js_mod(x, y), reference(x, y));
        if a.to_bits() != b.to_bits() && !(a.is_nan() && b.is_nan()) { bad += 1; if bad < 10 { println!("x={x} y={y} got={a} want={b}"); } }
    }
    println!("cases {n} mismatches {bad}");
}
