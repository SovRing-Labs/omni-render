// rings — a C app compiled to WASM (clang --target=wasm32, no libc) running inside the omni-render runtime.
// It builds a small phase lattice in ITS OWN memory and asks the runtime's sift intrinsic (the JIT-rendered AVX2 kernel) to
// score it in place; checks RNS exact multiplication and an integer 2T rotor against its own arithmetic. Returns a bitmask of
// passed checks (7 = all).
typedef unsigned char u8; typedef signed char i8; typedef int i32; typedef unsigned int u32;
typedef long long i64; typedef unsigned long long u64;

#define IMPORT(name) __attribute__((import_module("omni"), import_name(#name)))
IMPORT(sift_topk) u32 sift_topk(const u8 *lat, u32 n, u32 d, const u8 *q, u32 k, i32 *out);
IMPORT(rns_mul) i32 rns_mul(i64 a, i64 b, u8 *residues_out);      // residues of a*b over the 10 ring moduli; returns count
IMPORT(rotor2t) void rotor2t(const i64 *q, const i64 *v, i64 *out); // integer quaternion sandwich q v q* (exact)
IMPORT(print) void print(const u8 *s, u32 len);

static const i32 M10[10] = {3, 5, 7, 11, 13, 17, 19, 23, 29, 31};
#define D 1024
#define N 512
static u8 lat[N * D];
static i32 out[16];

static u64 splitmix(u64 *x) { u64 z = (*x += 0x9E3779B97F4A7C15ULL); z = (z ^ (z >> 30)) * 0xBF58476D1CE4E5B9ULL; z = (z ^ (z >> 27)) * 0x94D049BB133111EBULL; return z ^ (z >> 31); }
static u32 slen(const char *s) { u32 n = 0; while (s[n]) n++; return n; }
static void say(const char *s) { print((const u8 *)s, slen(s)); }

__attribute__((export_name("run"))) i32 run(void) {
    i32 pass = 0;
    u64 st = 42;
    for (u32 i = 0; i < N * D; i += 16) { u64 z = splitmix(&st); for (int j = 0; j < 16; j++) lat[i + j] = (u8)((z >> (4 * j)) & 15); }
    // 1. sift: every 64th row as the query must come back top-1 with the maximal score 127*127*D... (C8²+S8²≈127² per dim)
    i32 ok = 1;
    for (u32 r = 0; r < N; r += 64) { sift_topk(lat, N, D, lat + r * D, 1, out); if ((u32)out[0] != r) ok = 0; }
    if (ok) { pass |= 1; say("rings: sift intrinsic top-1 self-recall 8/8"); }
    // 2. RNS: a*b mod each ring modulus must match the app's own arithmetic
    u8 res[10]; ok = 1;
    for (i64 a = 12345; a < 12345 + 50; a++) {
        i64 b = a * 7919 + 13;
        rns_mul(a, b, res);
        for (int k = 0; k < 10; k++) if (res[k] != (u8)(((a % M10[k]) * (b % M10[k])) % M10[k])) ok = 0;
    }
    if (ok) { pass |= 2; say("rings: rns_mul exact on 50 products x 10 rings"); }
    // 3. rotor: q = 1 + i (unnormalised), v = j → q v q* = |q|^2 * (rotated v) = 2 * k  (90° about i, scaled by |q|^2)
    i64 q[4] = {1, 1, 0, 0}, v[4] = {0, 0, 1, 0}, o[4];
    rotor2t(q, v, o);
    if (o[0] == 0 && o[1] == 0 && o[2] == 0 && o[3] == 2) { pass |= 4; say("rings: rotor2t (1+i) j (1-i) = 2k exact"); }
    return pass;
}
