//! A convertible's open top: the windshield frame (A-pillars and a header rail), padded
//! caps on the door tops, and either the folded top's boot (a padded ridge over the well
//! behind the rear seat) or the raised fabric top with its plastic rear window and seams.
//! A hardtop gets nothing here; the loft leaves a convertible's cabin open above the belt
//! (`Body::open_cut`).

use super::body::{Body, Slot};
use super::math::*;
use super::mesh::{Mat, Soup, Uv};
use super::params::{Detail, Top};
use super::Materials;
use alloc::vec::Vec;

const UP: V3 = [0.0, 1.0, 0.0];
const BACK: V3 = [0.0, 0.0, 1.0];

/// Width of the door-top cap inboard of the belt ledge, m.
pub const CAP_W: f32 = 0.055;
/// Boot height over the deck: a plain folded top, and a conversion shop's padded boot, m.
const BOOT_H: f32 = 0.12;
const PADDED_BOOT_H: f32 = 0.17;

pub struct OpenTop<'a, 'b> {
    b: &'a Body<'b>,
    m: &'a Materials,
    pub body: Soup,
    pub glass: Soup,
}

impl<'a, 'b> OpenTop<'a, 'b> {
    pub fn new(b: &'a Body<'b>, m: &'a Materials) -> Self {
        OpenTop {
            b,
            m,
            body: Soup::new(35.0),
            glass: Soup::new(30.0),
        }
    }

    fn lod0(&self) -> bool {
        self.b.lod == 0
    }

    fn p(&self, s: f32, z: f32, y: f32) -> V3 {
        self.b.p(s, z, y)
    }

    pub fn build(&mut self) {
        let Some(top) = self.b.spec.roof.top() else {
            return;
        };
        self.door_caps();
        self.windshield_frame();
        match top {
            Top::Down => self.boot(),
            Top::Up => self.soft_top(),
        }
    }

    /// The body's stations strictly inside `s0..s1`, at least `gap` apart, with both ends.
    fn stations(&self, s0: f32, s1: f32, gap: f32) -> Vec<f32> {
        let mut v = alloc::vec![s0];
        for t in &self.b.stations {
            let last = v[v.len() - 1];
            if t.s > s0 + gap && t.s < s1 - gap && t.s - last >= gap {
                v.push(t.s);
            }
        }
        v.push(s1);
        v
    }

    /// Black with `BlackTrim`, bright with `ChromeTrim`, else black.
    fn frame_mat(&self) -> Mat {
        if self.b.has(Detail::ChromeTrim) && !self.b.has(Detail::BlackTrim) {
            self.m.chrome
        } else {
            self.m.trim
        }
    }

    /// Fabric for the top and boot: the accent color.
    fn fabric(&self) -> Mat {
        self.m.accent.uv(Uv::Top)
    }

    // ---------------------------------------------------------------- doors

    /// A cap on each door top from the windshield's foot to the boot: painted on top, trim
    /// facing into the cabin. At LOD1 (no door cards in the cabin) its inner face runs down
    /// to the floor so the doors do not look hollow.
    fn door_caps(&mut self) {
        let b = self.b;
        let g = &b.spec.glass;
        let lod0 = self.lod0();
        let sts = self.stations(g.windshield.0, g.backlight.0, if lod0 { 0.02 } else { 0.1 });
        let ring = |s: f32| {
            let f = b.frame(s);
            let low = if lod0 {
                f.y_ledge - 0.06
            } else {
                // Down to the floor, but over the wheel arches.
                let mut y = f.yb + 0.09;
                for &ax in &b.axle {
                    let ds = s - ax;
                    if ds.abs() < b.ra + 0.02 {
                        y = y.max(b.wy + sqrt((b.ra * b.ra - ds * ds).max(0.0)) + 0.03);
                    }
                }
                y.min(f.y_ledge - 0.06)
            };
            [
                [f.z_ledge, f.y_ledge],
                [f.z_ledge - CAP_W, f.y_ledge - 0.004],
                [f.z_ledge - CAP_W, low],
            ]
        };
        let (top, inner) = (self.m.paint, self.m.door_card);
        for k in 0..sts.len() - 1 {
            let (sa, sb) = (sts[k], sts[k + 1]);
            let (ra, rb) = (ring(sa), ring(sb));
            for sx in [-1.0f32, 1.0] {
                let q = |r: &[[f32; 2]; 3], i: usize, s: f32| self.p(s, sx * r[i][0], r[i][1]);
                let (a0, a1, a2) = (q(&ra, 0, sa), q(&ra, 1, sa), q(&ra, 2, sa));
                let (b0, b1, b2) = (q(&rb, 0, sb), q(&rb, 1, sb), q(&rb, 2, sb));
                self.body.quad_dir(a0, a1, b1, b0, top, UP);
                self.body.quad_dir(a1, a2, b2, b1, inner, [-sx, 0.0, 0.0]);
            }
        }
    }

    // ---------------------------------------------------------------- windshield

    /// A-pillars swept along the windshield's side edges and a header rail across its top.
    fn windshield_frame(&mut self) {
        let b = self.b;
        let (w0, w1) = b.spec.glass.windshield;
        let mat = self.frame_mat();
        let lod0 = self.lod0();
        let sts = self.stations(w0, w1, if lod0 { 0.03 } else { 0.12 });
        // Centerline on the frame strip between the roof edge and the glass.
        let line: Vec<(f32, f32, f32)> = sts
            .iter()
            .map(|&s| {
                let (p, f) = b.ring_plain(s);
                let z = p[b.idx(Slot::RoofEdge)][0] - 0.02;
                (s, z, f.top_y(z) + 0.008)
            })
            .collect();
        let prof = [
            (-0.034, -0.016),
            (0.024, -0.016),
            (0.024, 0.024),
            (-0.034, 0.024),
        ];
        for sx in [-1.0f32, 1.0] {
            let path: Vec<V3> = line.iter().map(|&(s, z, y)| self.p(s, sx * z, y)).collect();
            let u = [sx, 0.0, 0.0];
            let us: Vec<V3> = path.iter().map(|_| u).collect();
            let vs: Vec<V3> = (0..path.len())
                .map(|k| {
                    let a = path[k.saturating_sub(1)];
                    let c = path[(k + 1).min(path.len() - 1)];
                    let v = norm(cross(norm(sub(c, a)), u));
                    if v[1] < 0.0 {
                        scale(v, -1.0)
                    } else {
                        v
                    }
                })
                .collect();
            self.body.sweep(&path, &us, &vs, &prof, true, mat);
        }
        // Header rail, following the windshield's crown.
        let f1 = b.frame(w1);
        let ze = line[line.len() - 1].1 + 0.025;
        let n = if lod0 { 8 } else { 2 };
        let path: Vec<V3> = (0..=n)
            .map(|k| {
                let x = lerp(-ze, ze, k as f32 / n as f32);
                self.p(w1 - 0.022, x, f1.top_y(x.abs()) + 0.008)
            })
            .collect();
        let us: Vec<V3> = path.iter().map(|_| BACK).collect();
        let vs: Vec<V3> = path.iter().map(|_| UP).collect();
        let prof = [
            (-0.045, -0.034),
            (0.026, -0.034),
            (0.026, 0.024),
            (-0.045, 0.024),
        ];
        self.body.sweep(&path, &us, &vs, &prof, true, mat);
    }

    // ---------------------------------------------------------------- boot

    /// Deck height at `(s, x)`.
    fn deck_y(&self, s: f32, x: f32) -> f32 {
        self.b.frame(s).top_y(x.abs())
    }

    /// The folded top under its boot: a padded ridge across the car over the backlight
    /// range, six segments wide with rounded ends and pleat seams. `VinylRoof` makes it a
    /// conversion shop's padded boot: taller, squarer, with a bright base molding.
    fn boot(&mut self) {
        let b = self.b;
        let (s0, s1) = b.spec.glass.backlight;
        let padded = b.has(Detail::VinylRoof);
        let lod0 = self.lod0();
        let sc = (s0 + s1) * 0.5;
        let half = (s1 - s0) * 0.5;
        let fc = b.frame(sc);
        let zw = fc.z_ledge - if padded { 0.01 } else { 0.035 };
        let h = if padded { PADDED_BOOT_H } else { BOOT_H };
        // Profile `(along s from the middle, height over the deck)`, front to back over
        // the top; the closing segment runs along the deck.
        let prof: Vec<(f32, f32)> = if !lod0 {
            alloc::vec![
                (-half, -0.015),
                (-half * 0.85, h),
                (half * 0.6, h * 0.9),
                (half, -0.015),
            ]
        } else if padded {
            alloc::vec![
                (-half, -0.015),
                (-half * 0.99, h * 0.72),
                (-half * 0.9, h * 0.97),
                (-half * 0.6, h),
                (half * 0.4, h),
                (half * 0.8, h * 0.86),
                (half * 0.97, h * 0.4),
                (half, -0.015),
            ]
        } else {
            alloc::vec![
                (-half, -0.015),
                (-half * 0.96, h * 0.45),
                (-half * 0.78, h * 0.86),
                (-half * 0.4, h),
                (half * 0.15, h * 0.97),
                (half * 0.6, h * 0.76),
                (half * 0.92, h * 0.3),
                (half, -0.015),
            ]
        };
        // Lateral sections `(x, height scale, length scale)`: rounded ends.
        let secs: Vec<(f32, f32, f32)> = if lod0 {
            alloc::vec![
                (-zw, 0.4, 0.86),
                (-zw * 0.9, 0.88, 0.97),
                (-zw * 0.45, 1.0, 1.0),
                (0.0, 1.0, 1.0),
                (zw * 0.45, 1.0, 1.0),
                (zw * 0.9, 0.88, 0.97),
                (zw, 0.4, 0.86),
            ]
        } else {
            alloc::vec![(-zw, 0.6, 0.95), (0.0, 1.0, 1.0), (zw, 0.6, 0.95)]
        };
        let pt = |this: &Self, k: usize, i: usize, lift: f32| {
            let (x, hs, ls) = secs[k];
            let (a, hgt) = prof[i];
            let s = sc + a * ls;
            this.p(s, x, this.deck_y(s, x) + hgt * hs + lift)
        };
        let mat = self.fabric();
        let n = prof.len();
        for k in 0..secs.len() - 1 {
            let xm = (secs[k].0 + secs[k + 1].0) * 0.5;
            let c = self.p(sc, xm, self.deck_y(sc, xm) + h * 0.3);
            for i in 0..n {
                let j = (i + 1) % n;
                let q = [pt(self, k, i, 0.0), pt(self, k, j, 0.0)];
                let r = [pt(self, k + 1, j, 0.0), pt(self, k + 1, i, 0.0)];
                self.body.quad_out(q[0], q[1], r[0], r[1], mat, c);
            }
        }
        for (k, dir) in [(0usize, -1.0f32), (secs.len() - 1, 1.0)] {
            let x = secs[k].0;
            let c = self.p(sc, x, self.deck_y(sc, x) + h * 0.3 * secs[k].1);
            for i in 0..n {
                let j = (i + 1) % n;
                self.body.tri_dir(
                    c,
                    pt(self, k, i, 0.0),
                    pt(self, k, j, 0.0),
                    mat,
                    [dir, 0.0, 0.0],
                );
            }
        }
        if !lod0 {
            return;
        }
        // Pleat seams over the top at the inner sections.
        let seam = mat.shade(0.62);
        for &(x, hs, ls) in &secs[2..secs.len() - 2] {
            for i in 0..n - 1 {
                let at = |i: usize, dx: f32| {
                    let (a, hgt) = prof[i];
                    let s = sc + a * ls;
                    self.p(s, x + dx, self.deck_y(s, x) + hgt * hs + 0.003)
                };
                let (a0, a1) = (at(i, -0.007), at(i, 0.007));
                let (b0, b1) = (at(i + 1, -0.007), at(i + 1, 0.007));
                let out = sub(scale(add(a0, b1), 0.5), c_of(self, sc, x, h));
                self.body.quad_dir(a0, a1, b1, b0, seam, out);
            }
        }
        if padded {
            // Bright molding along the boot's front foot.
            let mold = self.m.chrome;
            let y = |s: f32, x: f32| self.deck_y(s, x) + 0.006;
            let sf = s0 + 0.02;
            let xs = zw * 0.92;
            let n = 6;
            let path: Vec<V3> = (0..=n)
                .map(|k| {
                    let x = lerp(-xs, xs, k as f32 / n as f32);
                    self.p(sf, x, y(sf, x))
                })
                .collect();
            let us: Vec<V3> = path.iter().map(|_| [0.0, 0.0, -1.0]).collect();
            let vs: Vec<V3> = path.iter().map(|_| UP).collect();
            let prof = [
                (-0.006, -0.008),
                (0.008, -0.008),
                (0.008, 0.01),
                (-0.006, 0.01),
            ];
            self.body.sweep(&path, &us, &vs, &prof, true, mold);
        }
    }

    // ---------------------------------------------------------------- soft top

    /// The raised fabric top from the header to the deck: a slightly crowned roof over the
    /// seats, open over the doors (the door glass is down), a solid quarter behind them,
    /// a rear slope with a plastic window, and seams.
    fn soft_top(&mut self) {
        let b = self.b;
        let spec = b.spec;
        let g = &spec.glass;
        let lod0 = self.lod0();
        let (sa, sb) = (g.windshield.1, g.backlight.1);
        let y_h = b.frame(sa).yt;
        let crest = (spec.interior.seat.0 + 0.15).clamp(sa + 0.1, g.backlight.0 - 0.1);
        let slope = g.backlight.0 - 0.05;
        let deck_end = self.deck_y(sb, 0.0);
        let y_top = |s: f32| {
            if s <= crest {
                lerp(y_h, y_h + 0.02, (s - sa) / (crest - sa))
            } else if s <= slope {
                y_h + 0.02
            } else {
                lerp(y_h + 0.02, deck_end, smoothstep(slope, sb, s))
            }
        };
        let quarter = spec.interior.seat.0 + 0.45;
        let gf = spec.section.greenhouse_frac;
        // Half ring: belt foot, rail, roof edge, crown, center.
        let ring = |s: f32| -> [[f32; 2]; 5] {
            let f = b.frame(s);
            let yt = y_top(s).max(f.y_ledge);
            let zl = f.z_ledge - 0.02;
            let zt = (gf * f.w - 0.02).min(zl);
            let rail = (0.06f32).min((yt - f.y_ledge) * 0.5);
            let crown = (0.014f32).min((yt - f.y_ledge) * 0.3);
            [
                [zl, f.y_ledge],
                [zt, yt - rail],
                [zt * 0.86, yt - crown * 0.5],
                [zt * 0.45, yt + crown * 0.4],
                [0.0, yt + crown * 0.5],
            ]
        };
        let n_s = if lod0 { 12 } else { 4 };
        let mut sts: Vec<f32> = (0..=n_s)
            .map(|k| lerp(sa, sb, k as f32 / n_s as f32))
            .collect();
        for extra in [quarter, slope] {
            if extra > sa && extra < sb {
                sts.push(extra);
            }
        }
        sts.sort_by(|a, c| a.partial_cmp(c).unwrap_or(core::cmp::Ordering::Equal));
        sts.dedup_by(|a, c| (*a - *c).abs() < 0.01);
        let mat = self.fabric();
        for k in 0..sts.len() - 1 {
            let (s0, s1) = (sts[k], sts[k + 1]);
            let (r0, r1) = (ring(s0), ring(s1));
            let mid_s = (s0 + s1) * 0.5;
            for sx in [-1.0f32, 1.0] {
                let c = self.p(mid_s, 0.0, b.frame(mid_s).y_ledge);
                for i in 0..4 {
                    if i == 0 && mid_s < quarter {
                        continue;
                    }
                    let q = |r: &[[f32; 2]; 5], i: usize, s: f32| self.p(s, sx * r[i][0], r[i][1]);
                    self.body.quad_out(
                        q(&r0, i, s0),
                        q(&r0, i + 1, s0),
                        q(&r1, i + 1, s1),
                        q(&r1, i, s1),
                        mat,
                        c,
                    );
                }
            }
        }
        // Top surface height at lateral `x`, by the ring's segments.
        let surf = |s: f32, x: f32| -> f32 {
            let r = ring(s);
            let x = x.abs();
            for i in (1..4).rev() {
                let (a, c) = (r[i + 1], r[i]);
                if x >= a[0] && x <= c[0] {
                    return lerp(a[1], c[1], (x - a[0]) / (c[0] - a[0]).max(1e-6));
                }
            }
            r[4][1]
        };
        // Plastic rear window on the slope.
        let (w0, w1) = (slope + 0.07, sb - 0.1);
        if w1 > w0 + 0.1 {
            let nx = if lod0 { 4 } else { 1 };
            let ns = if lod0 { 3 } else { 1 };
            let xw = |s: f32| (gf * b.frame(s).w - 0.02) * 0.62;
            for i in 0..ns {
                for k in 0..nx {
                    let s_a = lerp(w0, w1, i as f32 / ns as f32);
                    let s_b = lerp(w0, w1, (i + 1) as f32 / ns as f32);
                    let pt = |s: f32, t: f32| {
                        let x = lerp(-xw(s), xw(s), t);
                        self.p(s, x, surf(s, x) + 0.004)
                    };
                    let (t0, t1) = (k as f32 / nx as f32, (k + 1) as f32 / nx as f32);
                    let out = [0.0, 1.0, 1.0];
                    self.glass.quad_dir(
                        pt(s_a, t0),
                        pt(s_a, t1),
                        pt(s_b, t1),
                        pt(s_b, t0),
                        self.m.glass,
                        out,
                    );
                }
            }
        }
        if !lod0 {
            return;
        }
        // Seams: two along the roof, bows across it over the seats and at the slope.
        let seam = mat.shade(0.62);
        let strip = |this: &mut Self, a: (f32, f32), c: (f32, f32), w: f32, across: bool| {
            let n = 6;
            for k in 0..n {
                let t0 = k as f32 / n as f32;
                let t1 = (k + 1) as f32 / n as f32;
                let at = |t: f32, d: f32| {
                    let (s, x) = (lerp(a.0, c.0, t), lerp(a.1, c.1, t));
                    let (s, x) = if across { (s + d, x) } else { (s, x + d) };
                    this.p(s, x, surf(s, x) + 0.003)
                };
                let q = [at(t0, -w), at(t0, w), at(t1, w), at(t1, -w)];
                this.body.quad_dir(q[0], q[1], q[2], q[3], seam, UP);
            }
        };
        let zs = |s: f32| (gf * b.frame(s).w - 0.02) * 0.45;
        for sx in [-1.0f32, 1.0] {
            let (a, c) = (sa + 0.03, slope);
            strip(self, (a, sx * zs(a)), (c, sx * zs(c)), 0.006, false);
        }
        for s in [crest, (crest + slope) * 0.5] {
            let x = gf * b.frame(s).w - 0.04;
            strip(self, (s, -x), (s, x), 0.008, true);
        }
    }
}

/// A point inside the boot's middle at lateral `x`, for facing seams outward.
fn c_of(t: &OpenTop, s: f32, x: f32, h: f32) -> V3 {
    t.p(s, x, t.deck_y(s, x) + h * 0.3)
}
