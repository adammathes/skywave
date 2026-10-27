//! Small meshes bolted onto the shell: lamps, grille, bumpers, mirrors, handles, wipers,
//! exhaust, antenna, arch lips and the `details` list.

use super::body::Body;
use super::math::*;
use super::mesh::{Mat, Soup, Uv};
use super::params::{
    BadgeShape, Bumper, Detail, Exhaust, Grille, Handles, HeadLights, Mirrors, Spoiler, TailLights,
};
use super::Materials;
use alloc::vec::Vec;

const UP: V3 = [0.0, 1.0, 0.0];
const FWD: V3 = [0.0, 0.0, -1.0];
const BACK: V3 = [0.0, 0.0, 1.0];

pub struct Parts<'a, 'b> {
    pub b: &'a Body<'b>,
    pub m: &'a Materials,
    pub popups_up: bool,
    pub body: Soup,
    pub lights: Soup,
}

/// A face band (nose or tail) to hang lamps and grilles on.
struct Band {
    /// Station of the face.
    s: f32,
    /// +1 at the tail, -1 at the nose: the direction the face looks along `s`.
    dir: f32,
    y0: f32,
    y1: f32,
}

impl<'a, 'b> Parts<'a, 'b> {
    pub fn new(b: &'a Body<'b>, m: &'a Materials, popups_up: bool) -> Self {
        Parts {
            b,
            m,
            popups_up,
            body: Soup::new(35.0),
            lights: Soup::new(35.0),
        }
    }

    fn lod0(&self) -> bool {
        self.b.lod == 0
    }

    fn p(&self, s: f32, x: f32, y: f32) -> V3 {
        self.b.p(s, x, y)
    }

    /// Outer face of the body side at `(s, y)`, right side.
    fn side(&self, s: f32, y: f32) -> f32 {
        self.b.side_z(s, y)
    }

    fn top(&self, s: f32, x: f32) -> f32 {
        self.b.frame(s).top_y(x.abs())
    }

    /// Lateral half-width usable for lamps at height `y` on the face at `s`.
    fn face_half(&self, s: f32, y: f32) -> f32 {
        self.side(s, y) - 0.035
    }

    fn band(&self, front: bool) -> Band {
        let spec = self.b.spec;
        let (s, bumper, dir) = if front {
            (0.0, spec.front.bumper_y, -1.0)
        } else {
            (self.b.length, spec.rear.bumper_y, 1.0)
        };
        let f = self.b.frame(s);
        let y0 = bumper.1 + 0.015;
        let y1 = (f.y_ledge - 0.025).max(y0 + 0.04);
        Band { s, dir, y0, y1 }
    }

    pub fn build(&mut self) {
        self.bumper(true);
        self.bumper(false);
        self.headlights();
        self.grille();
        self.tail_lights();
        self.mirrors();
        self.arch_lips();
        if self.lod0() {
            self.handles();
            self.wipers();
            self.exhaust();
            self.antenna();
            self.plates();
            self.trunk_outline();
            self.side_markers();
            self.fuel_door();
            if !self.b.convertible {
                self.drip_rails();
            }
        }
        self.spoiler();
        let details: Vec<Detail> = self.b.details.clone();
        // A convertible has no roof or rear glass to dress: its vinyl is the padded boot.
        let hardtop = !self.b.convertible;
        for d in details {
            match d {
                Detail::Louvers if hardtop => self.louvers(),
                Detail::HoodScoop => self.hood_scoop(),
                Detail::SideStripe if self.lod0() => self.side_stripe(),
                Detail::WoodPanel => self.wood_panel(false),
                Detail::WoodApplique => self.wood_panel(true),
                Detail::CorneringLamps => self.cornering_lamps(),
                Detail::ChromeTrim | Detail::BlackTrim if self.lod0() => {
                    self.rocker_trim();
                    if d == Detail::ChromeTrim {
                        self.side_molding();
                    }
                }
                Detail::VinylRoof if hardtop => self.vinyl_roof(),
                Detail::RallyStripes if self.lod0() => self.rally_stripes(),
                Detail::MudFlaps => self.mud_flaps(),
                Detail::RoofRack if hardtop => self.roof_rack(),
                Detail::SpareTire => self.spare_tire(),
                Detail::Badge(shape) if self.lod0() => {
                    self.badge(shape, true);
                    self.badge(shape, false);
                }
                _ => {}
            }
        }
    }

    // ---------------------------------------------------------------- bumpers

    fn bumper(&mut self, front: bool) {
        let spec = self.b.spec;
        let (kind, (y0, y1)) = if front {
            (spec.front.bumper, spec.front.bumper_y)
        } else {
            (spec.rear.bumper, spec.rear.bumper_y)
        };
        let mat = match kind {
            Bumper::Chrome => self.m.chrome,
            Bumper::Body => self.m.paint_low,
            Bumper::Rubber => self.m.rubber,
        };
        let l = self.b.length;
        let ym = (y0 + y1) * 0.5;
        let hh = (y1 - y0) * 0.5;
        let face = if front { 0.0 } else { l };
        let inward = if front { 1.0 } else { -1.0 };
        let wrap = 0.30f32.min(if front {
            self.b.axle[0] - self.b.ra - 0.05
        } else {
            l - self.b.axle[1] - self.b.ra - 0.05
        });
        // Path around the end in plan, from left side to right side, at bumper height.
        let mut plan: Vec<(f32, f32)> = Vec::new(); // (s, x)
        let ds = [wrap, wrap * 0.35, 0.0];
        for &d in &ds {
            let s = face + inward * d;
            let x = self.side(s.max(0.0).min(l), ym) + 0.012;
            plan.push((s, -x));
        }
        let corner = self.side(face, ym);
        let out = 0.035;
        plan.push((face - inward * out * 0.8, -(corner * 0.9)));
        plan.push((face - inward * out * 1.05, 0.0));
        let n = plan.len();
        for k in (0..n - 1).rev() {
            let (s, x) = plan[k];
            plan.push((s, -x));
        }
        let path: Vec<V3> = plan.iter().map(|&(s, x)| self.p(s, x, ym)).collect();
        let center = self.p(l * 0.5, 0.0, ym);
        let mut us = Vec::new();
        for k in 0..path.len() {
            let a = path[k.saturating_sub(1)];
            let b = path[(k + 1).min(path.len() - 1)];
            let t = norm(sub(b, a));
            let mut o = norm(cross(UP, t));
            if dot(o, sub(path[k], center)) < 0.0 {
                o = scale(o, -1.0);
            }
            us.push(o);
        }
        let vs: Vec<V3> = path.iter().map(|_| UP).collect();
        let prof: Vec<(f32, f32)> = if self.lod0() {
            alloc::vec![
                (-0.03, -hh),
                (0.012, -hh),
                (0.028, -hh * 0.55),
                (0.032, 0.0),
                (0.028, hh * 0.55),
                (0.012, hh),
                (-0.03, hh),
            ]
        } else {
            alloc::vec![(-0.03, -hh), (0.03, -hh), (0.03, hh), (-0.03, hh)]
        };
        self.body.sweep(&path, &us, &vs, &prof, true, mat);
        if self.lod0() {
            // Rubber strip along chrome bumpers, guards on big ones.
            if kind == Bumper::Chrome {
                let strip = [
                    (0.02, -0.016),
                    (0.046, -0.013),
                    (0.046, 0.013),
                    (0.02, 0.016),
                ];
                self.body.sweep(
                    &path[1..path.len() - 1],
                    &us[1..us.len() - 1],
                    &vs[1..vs.len() - 1],
                    &strip,
                    true,
                    self.m.rubber,
                );
                for sx in [-1.0f32, 1.0] {
                    let x = sx * corner * 0.42;
                    let s = face - inward * (out + 0.02);
                    let z = s - self.b.axle[0];
                    self.body.aabb(
                        [x - 0.03, y0 - 0.01, z - 0.03],
                        [x + 0.03, y1 + 0.01, z + 0.03],
                        self.m.rubber,
                    );
                }
            } else if kind == Bumper::Rubber && !self.b.has(Detail::ChromeTrim) {
                // A lighter gray rub strip along a black rubber bumper.
                let strip = [
                    (0.024, -0.014),
                    (0.042, -0.011),
                    (0.042, 0.011),
                    (0.024, 0.014),
                ];
                self.body.sweep(
                    &path[1..path.len() - 1],
                    &us[1..us.len() - 1],
                    &vs[1..vs.len() - 1],
                    &strip,
                    true,
                    self.m.steel.shade(1.6),
                );
            } else if self.b.has(Detail::ChromeTrim) {
                // A bright strip along a painted or rubber bumper.
                let strip = [
                    (0.024, -0.016),
                    (0.045, -0.013),
                    (0.045, 0.013),
                    (0.024, 0.016),
                ];
                self.body.sweep(
                    &path[1..path.len() - 1],
                    &us[1..us.len() - 1],
                    &vs[1..vs.len() - 1],
                    &strip,
                    true,
                    self.m.chrome,
                );
            }
        }
    }

    // ---------------------------------------------------------------- lamps

    /// A lamp lens on a face: a slightly proud box with an emissive front and a bezel.
    #[allow(clippy::too_many_arguments)]
    fn lamp_rect(
        &mut self,
        band_s: f32,
        dir: f32,
        x0: f32,
        x1: f32,
        y0: f32,
        y1: f32,
        lens: Mat,
        bezel: Option<Mat>,
    ) {
        let az = band_s - self.b.axle[0];
        let back = az - dir * 0.02;
        if let Some(bz) = bezel {
            let e = 0.012;
            let bf = az + dir * 0.010;
            self.body.aabb(
                [x0 - e, y0 - e, bf.min(back)],
                [x1 + e, y1 + e, bf.max(back)],
                bz,
            );
        }
        let f2 = az + dir * 0.016;
        self.lights
            .aabb([x0, y0, f2.min(back)], [x1, y1, f2.max(back)], lens);
    }

    /// Reflector facets over a lamp lens: a grid of small cells just proud of its face,
    /// alternately brighter and darker (LOD0).
    #[allow(clippy::too_many_arguments)]
    fn facets(&mut self, band_s: f32, dir: f32, x0: f32, x1: f32, y0: f32, y1: f32, lens: Mat) {
        if !self.lod0() {
            return;
        }
        let z = band_s - self.b.axle[0] + dir * 0.0172;
        let cell = 0.045;
        let nx = (((x1 - x0) / cell) as usize).max(2);
        let ny = (((y1 - y0) / cell) as usize).clamp(1, 4);
        let (dx, dy) = ((x1 - x0) / nx as f32, (y1 - y0) / ny as f32);
        for i in 0..nx {
            for k in 0..ny {
                let (a, b) = (x0 + dx * i as f32 + 0.002, x0 + dx * (i + 1) as f32 - 0.002);
                let (c, d) = (y0 + dy * k as f32 + 0.002, y0 + dy * (k + 1) as f32 - 0.002);
                let mat = if (i + k) % 2 == 0 {
                    lens.shade(1.1)
                } else {
                    lens.shade(0.9)
                };
                self.lights.quad_dir(
                    [a, c, z],
                    [b, c, z],
                    [b, d, z],
                    [a, d, z],
                    mat,
                    [0.0, 0.0, dir],
                );
            }
        }
    }

    /// A bright surround standing proud of a face round `x0..x1`, `y0..y1`: four bars `w`
    /// wide.
    #[allow(clippy::too_many_arguments)]
    fn surround(&mut self, band_s: f32, dir: f32, x0: f32, x1: f32, y0: f32, y1: f32, w: f32) {
        let az = band_s - self.b.axle[0];
        let (za, zb) = (az - dir * 0.004, az + dir * 0.026);
        let (za, zb) = (za.min(zb), za.max(zb));
        let m = self.m.chrome;
        for (a, b) in [
            ([x0 - w, y1], [x1 + w, y1 + w]),
            ([x0 - w, y0 - w], [x1 + w, y0]),
            ([x0 - w, y0], [x0, y1]),
            ([x1, y0], [x1 + w, y1]),
        ] {
            self.body.aabb([a[0], a[1], za], [b[0], b[1], zb], m);
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn lamp_round(&mut self, band_s: f32, dir: f32, x: f32, y: f32, r: f32, lens: Mat, bezel: Mat) {
        let az = band_s - self.b.axle[0];
        let segs = if self.lod0() { 12 } else { 8 };
        let n = [0.0, 0.0, dir];
        let c_back = [x, y, az - dir * 0.02];
        let c_front = [x, y, az + dir * 0.016];
        self.body.cylinder(
            c_back,
            [x, y, az + dir * 0.012],
            r + 0.014,
            segs,
            false,
            false,
            bezel,
        );
        self.body.annulus(
            [x, y, az + dir * 0.012],
            n,
            r - 0.002,
            r + 0.014,
            segs,
            bezel,
        );
        self.lights
            .cylinder(c_back, c_front, r, segs, false, false, lens);
        self.lights.disc(c_front, n, r, segs, lens);
        if self.lod0() {
            // Dome highlight ring.
            self.lights.annulus(
                add(c_front, scale(n, 0.002)),
                n,
                r * 0.35,
                r * 0.5,
                segs,
                lens.shade(1.15),
            );
        }
    }

    fn headlights(&mut self) {
        let spec = self.b.spec;
        let band = self.band(true);
        let m = self.m;
        let head = m.headlamp;
        let bezel = if self.b.has(Detail::BlackTrim) {
            m.trim
        } else {
            m.chrome
        };
        let (y0, y1) = (band.y0, band.y1);
        let bh = y1 - y0;
        let yc = (y0 + y1) * 0.5;
        let half = self.face_half(band.s, yc);
        match spec.front.lights {
            HeadLights::Rect => {
                let h = bh.min(0.13);
                for sx in [-1.0f32, 1.0] {
                    let (a, b) = (half - 0.27, half);
                    let (x0, x1) = if sx > 0.0 { (a, b) } else { (-b, -a) };
                    self.lamp_rect(
                        band.s,
                        band.dir,
                        x0,
                        x1,
                        yc - h * 0.5,
                        yc + h * 0.5,
                        head,
                        Some(bezel),
                    );
                    self.facets(band.s, band.dir, x0, x1, yc - h * 0.5, yc + h * 0.5, head);
                }
            }
            HeadLights::Quad => {
                let sz = bh.min(0.15);
                for sx in [-1.0f32, 1.0] {
                    for k in 0..2 {
                        let b = half - k as f32 * (sz + 0.03);
                        let a = b - sz;
                        let (x0, x1) = if sx > 0.0 { (a, b) } else { (-b, -a) };
                        let lens = if k == 0 { head } else { head.shade(0.92) };
                        self.lamp_rect(
                            band.s,
                            band.dir,
                            x0,
                            x1,
                            yc - sz * 0.45,
                            yc + sz * 0.45,
                            lens,
                            Some(bezel),
                        );
                        self.facets(
                            band.s,
                            band.dir,
                            x0 + 0.01,
                            x1 - 0.01,
                            yc - sz * 0.35,
                            yc + sz * 0.35,
                            lens,
                        );
                    }
                }
            }
            HeadLights::Round => {
                let r = (bh * 0.42).min(0.09);
                for sx in [-1.0f32, 1.0] {
                    self.lamp_round(band.s, band.dir, sx * (half - r - 0.01), yc, r, head, bezel);
                }
            }
            HeadLights::Hidden => {
                let h = (bh * 0.35).min(0.05);
                for sx in [-1.0f32, 1.0] {
                    let (a, b) = (half - 0.32, half - 0.02);
                    let (x0, x1) = if sx > 0.0 { (a, b) } else { (-b, -a) };
                    self.lamp_rect(
                        band.s,
                        band.dir,
                        x0,
                        x1,
                        yc - h * 0.5,
                        yc + h * 0.5,
                        head,
                        None,
                    );
                }
            }
            HeadLights::PopUp => {
                self.popups();
                // Parking and turn lamps in the nose band.
                let h = (bh * 0.6).min(0.06);
                for sx in [-1.0f32, 1.0] {
                    let (a, b) = (half - 0.20, half - 0.02);
                    let (x0, x1) = if sx > 0.0 { (a, b) } else { (-b, -a) };
                    self.lamp_rect(
                        band.s,
                        band.dir,
                        x0,
                        x1,
                        yc - h * 0.5,
                        yc + h * 0.5,
                        m.amber,
                        Some(m.trim),
                    );
                }
            }
        }
    }

    /// Pop-up lamp lids in the fender tops, beside the hood.
    fn popups(&mut self) {
        let b = self.b;
        let s0 = 0.10f32;
        let s1 = (s0 + 0.30).min(b.axle[0] - 0.05);
        let fm = b.frame((s0 + s1) * 0.5);
        let x1 = fm.z_re - 0.035;
        let x0 = (b.spec.seams.hood_width * fm.z_re + 0.03).min(x1 - 0.16);
        let m = self.m;
        for sx in [-1.0f32, 1.0] {
            if self.popups_up {
                // Raised: a box standing on the lid's front edge with the lamp facing forward.
                let y = self.top(s0, (x0 + x1) * 0.5);
                let h = 0.15;
                let za = s0 - b.axle[0];
                let (xa, xb) = if sx > 0.0 { (x0, x1) } else { (-x1, -x0) };
                self.body
                    .aabb([xa, y - 0.02, za], [xb, y + h, za + 0.13], m.paint);
                self.lights.aabb(
                    [xa + 0.02, y + 0.025, za - 0.006],
                    [xb - 0.02, y + h - 0.025, za + 0.01],
                    m.headlamp,
                );
                self.body.aabb(
                    [xa + 0.01, y + 0.015, za - 0.004],
                    [xb - 0.01, y + h - 0.015, za + 0.005],
                    m.trim,
                );
            } else {
                // Closed lid: a raised panel with a dark gap round it.
                self.surface_patch(
                    s0 - 0.008,
                    s1 + 0.008,
                    sx * (x0 - 0.008),
                    sx * (x1 + 0.008),
                    0.0015,
                    m.seam,
                    1,
                );
                self.lid(s0, s1, sx * x0, sx * x1, 0.005, m.paint);
            }
        }
    }

    /// A thin slab following the top surface between `s0..s1` and lateral `xa..xb`.
    fn lid(&mut self, s0: f32, s1: f32, xa: f32, xb: f32, lift: f32, mat: Mat) {
        let ns = 3;
        let nx = 2;
        let pt = |this: &Self, i: usize, k: usize, dy: f32| {
            let s = lerp(s0, s1, i as f32 / ns as f32);
            let x = lerp(xa, xb, k as f32 / nx as f32);
            this.p(s, x, this.top(s, x) + dy)
        };
        for i in 0..ns {
            for k in 0..nx {
                let (a, bb, c, d) = (
                    pt(self, i, k, lift),
                    pt(self, i + 1, k, lift),
                    pt(self, i + 1, k + 1, lift),
                    pt(self, i, k + 1, lift),
                );
                self.body.quad_dir(a, bb, c, d, mat, UP);
            }
        }
        // Edges down to the surface.
        let center = self.p(
            (s0 + s1) * 0.5,
            (xa + xb) * 0.5,
            self.top((s0 + s1) * 0.5, (xa + xb) * 0.5) - 0.05,
        );
        for i in 0..ns {
            for k in [0usize, nx] {
                let (a, bb) = (pt(self, i, k, lift), pt(self, i + 1, k, lift));
                let (c, d) = (pt(self, i + 1, k, -0.002), pt(self, i, k, -0.002));
                self.body.quad_out(a, bb, c, d, mat, center);
            }
        }
        for k in 0..nx {
            for i in [0usize, ns] {
                let (a, bb) = (pt(self, i, k, lift), pt(self, i, k + 1, lift));
                let (c, d) = (pt(self, i, k + 1, -0.002), pt(self, i, k, -0.002));
                self.body.quad_out(a, bb, c, d, mat, center);
            }
        }
    }

    /// A decal on the top surface, `lift` above it; `n` subdivisions along each axis.
    #[allow(clippy::too_many_arguments)]
    fn surface_patch(&mut self, s0: f32, s1: f32, xa: f32, xb: f32, lift: f32, mat: Mat, n: usize) {
        let ns = (n * 2).max(1);
        for i in 0..ns {
            for k in 0..n {
                let sa = lerp(s0, s1, i as f32 / ns as f32);
                let sb = lerp(s0, s1, (i + 1) as f32 / ns as f32);
                let xa0 = lerp(xa, xb, k as f32 / n as f32);
                let xb0 = lerp(xa, xb, (k + 1) as f32 / n as f32);
                let q = [
                    self.p(sa, xa0, self.top(sa, xa0) + lift),
                    self.p(sb, xa0, self.top(sb, xa0) + lift),
                    self.p(sb, xb0, self.top(sb, xb0) + lift),
                    self.p(sa, xb0, self.top(sa, xb0) + lift),
                ];
                self.body.quad_dir(q[0], q[1], q[2], q[3], mat, UP);
            }
        }
    }

    /// A decal on the right or left body side (`sx`), following the surface.
    #[allow(clippy::too_many_arguments)]
    fn side_patch(
        &mut self,
        sx: f32,
        s0: f32,
        s1: f32,
        y0: [f32; 2],
        y1: [f32; 2],
        lift: f32,
        mat: Mat,
    ) {
        let q = [[s0, y0[0]], [s1, y0[1]], [s1, y1[1]], [s0, y1[0]]]
            .map(|[s, y]| self.p(s, sx * (self.side(s, y) + lift), y));
        self.body
            .quad_dir(q[0], q[1], q[2], q[3], mat, [sx, 0.0, 0.0]);
    }

    fn grille(&mut self) {
        let spec = self.b.spec;
        let band = self.band(true);
        let m = self.m;
        let (y0, y1) = (band.y0, band.y1);
        let bh = y1 - y0;
        let yc = (y0 + y1) * 0.5;
        let half = self.face_half(band.s, yc);
        let lamp_w = match spec.front.lights {
            HeadLights::Rect => 0.30,
            HeadLights::Quad => 2.0 * bh.min(0.15) + 0.06,
            HeadLights::Round => 2.0 * (bh * 0.42).min(0.09) + 0.04,
            HeadLights::Hidden => 0.0,
            HeadLights::PopUp => 0.23,
        };
        let xg = (half - lamp_w - 0.02).max(0.15);
        let az = band.s - self.b.axle[0];
        let surround = if self.b.has(Detail::BlackTrim) {
            m.trim
        } else {
            m.chrome
        };
        match spec.front.grille {
            Grille::None => {}
            Grille::Slot => {
                let h = (bh * 0.4).min(0.05);
                self.body.aabb(
                    [-xg, yc - h * 0.5, az - 0.008],
                    [xg, yc + h * 0.5, az + 0.01],
                    m.grille,
                );
                if self.lod0() {
                    for k in 0..2 {
                        let y = yc - h * 0.25 + k as f32 * h * 0.5;
                        self.body.aabb(
                            [-xg + 0.01, y - 0.005, az - 0.012],
                            [xg - 0.01, y + 0.005, az + 0.0],
                            m.trim,
                        );
                    }
                }
            }
            Grille::Egg | Grille::Wide => {
                let h = (bh * 0.8).min(0.22);
                let (gy0, gy1) = (yc - h * 0.5, yc + h * 0.5);
                self.body.aabb(
                    [-xg, gy0, az - 0.006],
                    [xg, gy1, az + 0.01],
                    m.grille.uv(Uv::Face),
                );
                let fz = az - 0.022;
                let t = 0.012;
                // Surround.
                self.body.aabb(
                    [-xg - t, gy1 - t * 0.2, fz],
                    [xg + t, gy1 + t, az],
                    surround,
                );
                self.body.aabb(
                    [-xg - t, gy0 - t, fz],
                    [xg + t, gy0 + t * 0.2, az],
                    surround,
                );
                self.body
                    .aabb([-xg - t, gy0, fz], [-xg + t * 0.2, gy1, az], surround);
                self.body
                    .aabb([xg - t * 0.2, gy0, fz], [xg + t, gy1, az], surround);
                if self.lod0() {
                    let bar = if spec.front.grille == Grille::Wide {
                        surround
                    } else {
                        m.trim
                    };
                    let nv = if spec.front.grille == Grille::Wide {
                        15
                    } else {
                        11
                    };
                    // Bars: front faces only, standing proud of the dark back plate.
                    let bz = az - 0.012;
                    for k in 1..nv {
                        let x = lerp(-xg, xg, k as f32 / nv as f32);
                        self.body.quad_dir(
                            [x - 0.005, gy0, bz],
                            [x + 0.005, gy0, bz],
                            [x + 0.005, gy1, bz],
                            [x - 0.005, gy1, bz],
                            bar,
                            FWD,
                        );
                    }
                    let nh = if spec.front.grille == Grille::Wide {
                        3
                    } else {
                        4
                    };
                    for k in 1..nh {
                        let y = lerp(gy0, gy1, k as f32 / nh as f32);
                        let bz = bz - 0.002;
                        self.body.quad_dir(
                            [-xg, y - 0.005, bz],
                            [xg, y - 0.005, bz],
                            [xg, y + 0.005, bz],
                            [-xg, y + 0.005, bz],
                            bar,
                            FWD,
                        );
                    }
                }
            }
        }
    }

    fn tail_lights(&mut self) {
        let spec = self.b.spec;
        let band = self.band(false);
        let m = self.m;
        let (y0, y1) = (band.y0, band.y1);
        let bh = y1 - y0;
        let yc = (y0 + y1) * 0.5;
        let half = self.face_half(band.s, yc) + 0.01;
        let bezel = if self.b.has(Detail::ChromeTrim) {
            Some(m.chrome)
        } else {
            Some(m.trim)
        };
        match spec.rear.lights {
            TailLights::FullWidth => {
                let h = (bh * 0.75).min(0.14);
                let (ly0, ly1) = (y1 - 0.02 - h, y1 - 0.02);
                let pieces: [(f32, f32, Mat); 5] = [
                    (-half, -half * 0.62, m.tail),
                    (-half * 0.62, -0.08, m.tail.shade(0.85)),
                    (-0.08, 0.08, m.reverse),
                    (0.08, half * 0.62, m.tail.shade(0.85)),
                    (half * 0.62, half, m.tail),
                ];
                for &(a, b, lens) in pieces.iter() {
                    self.lamp_rect(band.s, band.dir, a, b, ly0, ly1, lens, None);
                    self.facets(band.s, band.dir, a, b, ly0, ly1, lens);
                }
                // A bright surround and dividers between the lenses.
                self.surround(band.s, band.dir, -half, half, ly0, ly1, 0.016);
                let az = band.s - self.b.axle[0];
                for &(a, _, _) in pieces[1..].iter() {
                    let z = az + band.dir * 0.022;
                    self.body.aabb(
                        [a - 0.006, ly0, z.min(az)],
                        [a + 0.006, ly1, z.max(az)],
                        m.chrome,
                    );
                }
                if let Some(bz) = bezel {
                    let az = band.s - self.b.axle[0];
                    self.body.aabb(
                        [-half - 0.012, ly0 - 0.012, az - 0.02],
                        [half + 0.012, ly1 + 0.012, az + 0.01],
                        bz,
                    );
                    if self.lod0() {
                        // Ribbed lens look: dark horizontal ribs.
                        for k in 1..3 {
                            let y = lerp(ly0, ly1, k as f32 / 3.0);
                            self.body.aabb(
                                [-half, y - 0.003, az + 0.012],
                                [half, y + 0.003, az + 0.02],
                                m.trim,
                            );
                        }
                    }
                }
            }
            TailLights::Dual => {
                // Tall corner lamps: two stacked lenses per side, reverse lamp in the lower.
                let w = 0.15f32;
                let gap = 0.012;
                let hh = (bh - 0.03 - gap) * 0.5;
                for sx in [-1.0f32, 1.0] {
                    let (a, b) = (half - w, half);
                    let (x0, x1) = if sx > 0.0 { (a, b) } else { (-b, -a) };
                    let top = y1 - 0.01;
                    self.lamp_rect(band.s, band.dir, x0, x1, top - hh, top, m.tail, bezel);
                    self.facets(band.s, band.dir, x0, x1, top - hh, top, m.tail);
                    self.surround(band.s, band.dir, x0, x1, top - 2.0 * hh - gap, top, 0.014);
                    let mid = top - hh - gap;
                    self.lamp_rect(
                        band.s,
                        band.dir,
                        x0,
                        x1,
                        mid - hh * 0.55,
                        mid,
                        m.tail.shade(0.9),
                        bezel,
                    );
                    self.lamp_rect(
                        band.s,
                        band.dir,
                        x0,
                        x1,
                        mid - hh,
                        mid - hh * 0.55 - 0.006,
                        m.reverse,
                        bezel,
                    );
                }
            }
            TailLights::Triple => {
                let h = (bh * 0.8).min(0.16);
                let w = 0.13f32;
                for sx in [-1.0f32, 1.0] {
                    for k in 0..3 {
                        let b = half - k as f32 * (w + 0.015);
                        let a = b - w;
                        let (x0, x1) = if sx > 0.0 { (a, b) } else { (-b, -a) };
                        let lens = if k == 2 { m.amber } else { m.tail };
                        self.lamp_rect(
                            band.s,
                            band.dir,
                            x0,
                            x1,
                            yc - h * 0.5,
                            yc + h * 0.5,
                            lens,
                            bezel,
                        );
                        self.facets(band.s, band.dir, x0, x1, yc - h * 0.5, yc + h * 0.5, lens);
                    }
                    let (a, b) = (half - 3.0 * w - 0.03, half);
                    let (x0, x1) = if sx > 0.0 { (a, b) } else { (-b, -a) };
                    self.surround(band.s, band.dir, x0, x1, yc - h * 0.5, yc + h * 0.5, 0.014);
                }
            }
            TailLights::Round => {
                let r = (bh * 0.4).min(0.065);
                let bz = bezel.unwrap_or(m.trim);
                for sx in [-1.0f32, 1.0] {
                    for k in 0..2 {
                        let x = sx * (half - r - 0.01 - k as f32 * (2.0 * r + 0.04));
                        self.lamp_round(band.s, band.dir, x, yc, r, m.tail, bz);
                    }
                }
            }
        }
    }

    // ---------------------------------------------------------------- side parts

    fn mirrors(&mut self) {
        let spec = self.b.spec;
        let m = self.m;
        let housing = if self.b.has(Detail::BlackTrim) {
            m.trim
        } else if self.b.has(Detail::ChromeTrim) {
            m.chrome
        } else {
            m.paint
        };
        let g = &spec.glass;
        let (s, y, stalk) = match spec.parts.mirrors {
            Mirrors::Door => {
                // Without door glass (a convertible) at the windshield's foot.
                let s = g.side.first().map_or(g.windshield.0 + 0.06, |w| w.0) + 0.09;
                let f = self.b.frame(s);
                (s, f.y_ledge + 0.06, 0.05)
            }
            Mirrors::Fender => {
                let s = self.b.axle[0] + 0.15;
                let f = self.b.frame(s);
                (s, f.y_edge + 0.1, 0.0)
            }
        };
        let z = s - self.b.axle[0];
        for sx in [-1.0f32, 1.0] {
            let base_x = match spec.parts.mirrors {
                Mirrors::Door => self.side(s, y - 0.04).max(self.b.frame(s).z_ledge),
                Mirrors::Fender => self.b.frame(s).z_re - 0.06,
            };
            let hx = base_x + stalk + 0.06;
            if spec.parts.mirrors == Mirrors::Door {
                // Sail mount on the door top.
                let f = self.b.frame(s);
                let x0 = f.z_ledge - 0.04;
                self.body.aabb(
                    [sx * x0, f.y_ledge - 0.01, z - 0.07],
                    [sx * (base_x + 0.02), y + 0.01, z + 0.03],
                    housing,
                );
                self.body.aabb(
                    [sx * (base_x + 0.02), y - 0.03, z - 0.02],
                    [sx * (base_x + stalk + 0.01), y - 0.0, z + 0.02],
                    housing,
                );
            } else {
                let fy = self.top(s, base_x);
                self.body.beam(
                    [sx * base_x, fy, z],
                    [sx * (base_x + 0.02), y - 0.02, z],
                    FWD,
                    0.008,
                    0.008,
                    m.chrome,
                );
            }
            let c = [sx * hx, y, z + 0.01];
            if self.lod0() {
                // A rounded shell: a bullet-nosed profile swept across, the glass behind.
                let prof: Vec<(f32, f32)> = (0..12)
                    .map(|k| {
                        let (sn, cs) = sin_cos(2.0 * PI * k as f32 / 12.0);
                        let fwd = if cs < 0.0 { 1.35 } else { 1.0 };
                        (cs * 0.035 * fwd, sn * 0.045)
                    })
                    .collect();
                let path = [
                    [sx * (hx - 0.06), y, z + 0.012],
                    [sx * hx, y, z + 0.01],
                    [sx * (hx + 0.06), y, z + 0.012],
                ];
                let us = [BACK; 3];
                let vs = [UP; 3];
                self.body.sweep(&path, &us, &vs, &prof, true, housing);
            } else {
                self.body.obox(
                    c,
                    [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
                    [0.06, 0.045, 0.035],
                    housing,
                );
            }
            // Glass faces rearward.
            self.body.aabb(
                [sx * hx - 0.05, y - 0.035, z + 0.042],
                [sx * hx + 0.05, y + 0.035, z + 0.047],
                m.mirror,
            );
        }
    }

    fn handles(&mut self) {
        let spec = self.b.spec;
        let m = self.m;
        let lines = &self.b.door_seams;
        if lines.len() < 2 {
            return;
        }
        let rear_edges: Vec<f32> = lines[1..].to_vec();
        for se in rear_edges {
            let s = se - 0.11;
            let y = self.b.frame(s).y_ledge - 0.075;
            let z = s - self.b.axle[0];
            for sx in [-1.0f32, 1.0] {
                let x = self.side(s, y);
                match spec.parts.handles {
                    Handles::Flush => {
                        self.side_patch(
                            sx,
                            s - 0.07,
                            s + 0.07,
                            [y - 0.014; 2],
                            [y + 0.014; 2],
                            0.002,
                            m.seam,
                        );
                        self.body.aabb(
                            [sx * (x + 0.001), y - 0.008, z - 0.045],
                            [sx * (x + 0.007), y + 0.008, z + 0.045],
                            m.trim,
                        );
                    }
                    Handles::Pull => {
                        self.body.aabb(
                            [sx * (x - 0.002), y - 0.016, z - 0.08],
                            [sx * (x + 0.004), y + 0.016, z + 0.08],
                            m.trim,
                        );
                        // A bar on two posts over a dark finger cup.
                        self.body.aabb(
                            [sx * (x + 0.004), y - 0.012, z - 0.06],
                            [sx * (x + 0.005), y + 0.012, z + 0.06],
                            m.liner,
                        );
                        for e in [-0.062f32, 0.062] {
                            self.body.aabb(
                                [sx * (x + 0.004), y - 0.008, z + e - 0.008],
                                [sx * (x + 0.02), y + 0.008, z + e + 0.008],
                                m.chrome,
                            );
                        }
                        self.body.beam(
                            [sx * (x + 0.024), y, z - 0.068],
                            [sx * (x + 0.024), y, z + 0.068],
                            [sx, 0.0, 0.0],
                            0.009,
                            0.005,
                            m.chrome,
                        );
                    }
                }
                // Key lock behind the handle.
                self.body.cylinder(
                    [sx * x, y, z + 0.11],
                    [sx * (x + 0.006), y, z + 0.11],
                    0.011,
                    10,
                    true,
                    false,
                    m.chrome,
                );
            }
        }
    }

    fn wipers(&mut self) {
        let spec = self.b.spec;
        let n = spec.parts.wipers as usize;
        if n == 0 {
            return;
        }
        let s = spec.glass.windshield.0 + 0.035;
        let f = self.b.frame(s + 0.03);
        let span = f.z_re * 2.0;
        let blade = (span / n as f32) * 0.85;
        for k in 0..n {
            let x0 = -f.z_re * 0.9 + span * 0.95 * (k as f32 / n as f32);
            let x1 = x0 + blade;
            let y0 = self.top(s, x0) + 0.012;
            let y1 = self.top(s + 0.03, x1) + 0.012;
            let a = self.p(s, x0, y0);
            let b = self.p(s + 0.03, x1, y1);
            // Blade frame with its rubber under it, the arm over it from the pivot.
            self.body.beam(a, b, UP, 0.008, 0.006, self.m.trim);
            self.body.beam(
                add(a, [0.0, -0.008, 0.0]),
                add(b, [0.0, -0.008, 0.0]),
                UP,
                0.003,
                0.003,
                self.m.rubber,
            );
            let mid = scale(add(a, b), 0.5);
            self.body.beam(
                add(a, [0.0, 0.012, 0.0]),
                add(mid, [0.0, 0.012, 0.0]),
                UP,
                0.006,
                0.004,
                self.m.trim,
            );
            for t in [0.25f32, 0.75] {
                let c = add(lerp3(a, b, t), [0.0, 0.006, 0.0]);
                self.body.aabb(
                    sub(c, [0.012, 0.006, 0.006]),
                    add(c, [0.012, 0.006, 0.006]),
                    self.m.trim,
                );
            }
            // Pivot.
            self.body.cylinder(
                add(a, [0.0, 0.006, 0.0]),
                add(a, [0.0, -0.02, 0.0]),
                0.012,
                8,
                true,
                false,
                self.m.trim,
            );
        }
    }

    fn exhaust(&mut self) {
        let spec = self.b.spec;
        let l = self.b.length;
        let y = spec.rear.bumper_y.0 - 0.03;
        let r = 0.034;
        let xs: &[f32] = match spec.parts.exhaust {
            Exhaust::Single => &[0.5],
            Exhaust::Dual => &[-0.48, 0.48],
        };
        let half = self.side(l, 0.4);
        for &x0 in xs {
            let x = x0 * half.max(0.5) / 0.9 * 0.75;
            let a = [x, y, l - 0.35 - self.b.axle[0]];
            let b = [x, y, l + 0.04 - self.b.axle[0]];
            self.body.cylinder(a, b, r, 16, false, false, self.m.chrome);
            // A rolled lip and the dark bore.
            self.body.lathe(
                b,
                BACK,
                &[(0.0, r), (0.004, r - 0.002), (0.0, r - 0.006)],
                16,
                0.0,
                (-0.01, r - 0.003),
                self.m.chrome,
            );
            self.body.cylinder(
                add(b, [0.0, 0.0, -0.005]),
                add(b, [0.0, 0.0, -0.06]),
                r - 0.006,
                16,
                false,
                true,
                self.m.chassis,
            );
            self.body.disc(
                add(b, [0.0, 0.0, -0.04]),
                BACK,
                r - 0.006,
                16,
                self.m.chassis,
            );
            // Muffler and the pipe ahead of it.
            let mz = a[2] - 0.05;
            self.body.cylinder(
                [x, y + 0.02, mz - 0.42],
                [x, y + 0.02, mz],
                0.075,
                12,
                true,
                false,
                self.m.steel,
            );
            self.body.cylinder(
                [x * 0.4, y + 0.04, mz - 1.2],
                [x, y + 0.02, mz - 0.42],
                0.025,
                8,
                false,
                false,
                self.m.steel,
            );
        }
    }

    fn antenna(&mut self) {
        if self.b.spec.parts.antenna == super::params::Antenna::None {
            return;
        }
        let s = self.b.spec.glass.windshield.0 - 0.12;
        let f = self.b.frame(s);
        let x = f.z_re - 0.05;
        let y = f.top_y(x);
        let z = s - self.b.axle[0];
        self.body.cylinder(
            [x, y - 0.01, z],
            [x, y + 0.015, z],
            0.012,
            6,
            true,
            false,
            self.m.chrome,
        );
        self.body.beam(
            [x, y, z],
            [x, y + 0.78, z + 0.05],
            BACK,
            0.003,
            0.003,
            self.m.chrome,
        );
    }

    /// Blank license plate (no text): in the tail band below the lamps when there is room,
    /// otherwise on the bumper.
    fn plates(&mut self) {
        let spec = self.b.spec;
        let (y0, y1) = spec.rear.bumper_y;
        let band = self.band(false);
        let lamp_bottom = match spec.rear.lights {
            TailLights::FullWidth => band.y1 - 0.02 - ((band.y1 - band.y0) * 0.75).min(0.14),
            _ => band.y1,
        };
        let pz = self.b.length - self.b.axle[0];
        let (mid, back) = if lamp_bottom - band.y0 > 0.17 {
            ((band.y0 + lamp_bottom) * 0.5, pz + 0.008)
        } else {
            ((y0 + y1) * 0.5, pz + 0.035 * 1.05 + 0.034)
        };
        let h = (0.075f32).min((y1 - y0) * 0.45);
        self.body.aabb(
            [-0.15, mid - h, back - 0.015],
            [0.15, mid + h, back],
            self.m.plate,
        );
        // Bright frame and the plate lamp's hood over it.
        let (e, f) = (0.012, back + 0.004);
        for (a, b) in [
            ([-0.15 - e, mid + h], [0.15 + e, mid + h + e]),
            ([-0.15 - e, mid - h - e], [0.15 + e, mid - h]),
            ([-0.15 - e, mid - h], [-0.15, mid + h]),
            ([0.15, mid - h], [0.15 + e, mid + h]),
        ] {
            self.body
                .aabb([a[0], a[1], back - 0.016], [b[0], b[1], f], self.m.chrome);
        }
        self.body.aabb(
            [-0.05, mid + h + e, back - 0.016],
            [0.05, mid + h + e + 0.018, back + 0.02],
            self.m.trim,
        );
        self.lights.quad_dir(
            [-0.04, mid + h + e - 0.0005, back + 0.004],
            [0.04, mid + h + e - 0.0005, back + 0.004],
            [0.04, mid + h + e - 0.0005, back + 0.016],
            [-0.04, mid + h + e - 0.0005, back + 0.016],
            self.m.reverse,
            [0.0, -1.0, 0.0],
        );
    }

    /// Tailgate outline on the tail when the deck is too short for a trunk lid.
    fn trunk_outline(&mut self) {
        if !self.b.tailgate {
            return;
        }
        let l = self.b.length;
        let spec = self.b.spec;
        let f = self.b.frame(l);
        let y0 = spec.rear.bumper_y.1 + 0.02;
        let y1 = f.y_ledge - 0.015;
        let x = self.side(l, (y0 + y1) * 0.5) - 0.05;
        let z = l - self.b.axle[0] + 0.0015;
        let t = 0.004;
        let mut q = |a: [f32; 2], b: [f32; 2]| {
            self.body.quad_dir(
                [a[0], a[1], z],
                [b[0], a[1], z],
                [b[0], b[1], z],
                [a[0], b[1], z],
                self.m.seam,
                BACK,
            );
        };
        q([-x, y0 - t], [x, y0 + t]);
        q([-x - t, y0], [-x + t, y1]);
        q([x - t, y0], [x + t, y1]);
    }

    /// Amber markers on the front fenders and red ones on the rear quarters, in bezels.
    fn side_markers(&mut self) {
        let b = self.b;
        let m = self.m;
        let l = b.length;
        let bezel = if b.has(Detail::ChromeTrim) {
            m.chrome
        } else {
            m.trim
        };
        let front_s = (b.axle[0] - b.ra - 0.12).max(0.1);
        let rear_s = (b.axle[1] + b.ra + 0.14).min(l - 0.1);
        for (s, lens) in [(front_s, m.amber), (rear_s, m.tail)] {
            let y = b.split_y(s).min(b.frame(s).y_ledge - 0.06);
            let z = s - b.axle[0];
            for sx in [-1.0f32, 1.0] {
                let x = self.side(s, y);
                self.body.aabb(
                    [sx * (x - 0.004), y - 0.026, z - 0.056],
                    [sx * (x + 0.003), y + 0.026, z + 0.056],
                    bezel,
                );
                self.lights.aabb(
                    [sx * (x - 0.002), y - 0.018, z - 0.046],
                    [sx * (x + 0.006), y + 0.018, z + 0.046],
                    lens,
                );
            }
        }
    }

    /// A fuel filler door on the left rear quarter: a lid inside a seam.
    fn fuel_door(&mut self) {
        let b = self.b;
        let l = b.length;
        let behind = b.axle[1] + b.ra + 0.22;
        let s = if behind + 0.1 < l - 0.12 {
            behind
        } else {
            b.axle[1] - b.ra - 0.16
        };
        let f = b.frame(s);
        let y = f.y_ledge - 0.13;
        if y - 0.06 < self.arch_floor(s, 0.03) {
            return;
        }
        let paint = b.paint_at(self.m, s, y);
        let (h, w) = (0.055, 0.06);
        self.side_patch(
            -1.0,
            s - w - 0.005,
            s + w + 0.005,
            [y - h - 0.005; 2],
            [y + h + 0.005; 2],
            0.0015,
            self.m.seam,
        );
        self.side_patch(-1.0, s - w, s + w, [y - h; 2], [y + h; 2], 0.003, paint);
    }

    /// Rain gutters along the roof edges, bright with `ChromeTrim`, black with `BlackTrim`.
    fn drip_rails(&mut self) {
        let b = self.b;
        let mat = if b.has(Detail::ChromeTrim) {
            self.m.chrome
        } else if b.has(Detail::BlackTrim) {
            self.m.trim
        } else {
            return;
        };
        let g = &b.spec.glass;
        let (s0, s1) = (g.windshield.1 - 0.02, g.backlight.0 + 0.02);
        let n = 10;
        let pts: Vec<(f32, [f32; 2])> = (0..=n)
            .map(|k| {
                let s = lerp(s0, s1, k as f32 / n as f32);
                let (p, _) = b.ring_plain(s);
                (s, p[b.idx(super::body::Slot::RoofEdge)])
            })
            .collect();
        for sx in [-1.0f32, 1.0] {
            for k in 0..n {
                let (sa, pa) = pts[k];
                let (sb, pb) = pts[k + 1];
                self.body.beam(
                    self.p(sa, sx * (pa[0] + 0.004), pa[1] - 0.006),
                    self.p(sb, sx * (pb[0] + 0.004), pb[1] - 0.006),
                    UP,
                    0.006,
                    0.007,
                    mat,
                );
            }
        }
    }

    fn arch_lips(&mut self) {
        let b = self.b;
        if b.lod > 1 {
            return;
        }
        let mat = if b.has(Detail::ChromeTrim) {
            self.m.chrome
        } else if b.has(Detail::BlackTrim) {
            self.m.trim
        } else {
            self.m.paint_low
        };
        let lip = 0.03;
        let out = 0.012;
        for a in 0..2 {
            let ax = b.axle[a];
            // Follow the body's own arch stations so the lip sits exactly on the cut edge:
            // (s, y, radial-out direction in (s, y)).
            let mut path: Vec<(f32, f32, f32, f32)> = Vec::new();
            let low = b.frame(ax).yb + 0.06;
            path.push((ax - b.ra, low, -1.0, 0.0));
            for st in b.stations.iter().filter(|t| t.clamp == Some(a as u8)) {
                let y = b.arch_y(a as u8, st.s, &b.frame(st.s));
                let (ds, dy) = (st.s - ax, y - b.wy);
                let l = sqrt(ds * ds + dy * dy).max(1e-6);
                path.push((st.s, y, ds / l, dy / l));
            }
            path.push((ax + b.ra, low, 1.0, 0.0));
            for sx in [-1.0f32, 1.0] {
                for k in 0..path.len() - 1 {
                    let (s0, y0, d0s, d0y) = path[k];
                    let (s1, y1, d1s, d1y) = path[k + 1];
                    let inner0 = (s0, y0);
                    let inner1 = (s1, y1);
                    let outer0 = (s0 + d0s * lip, y0 + d0y * lip);
                    let outer1 = (s1 + d1s * lip, y1 + d1y * lip);
                    let zof = |s: f32, y: f32, add_: f32| sx * (b.side_z(s, y) + add_);
                    let pi0 = self.p(inner0.0, zof(inner0.0, inner0.1.max(low), out), inner0.1);
                    let pi1 = self.p(inner1.0, zof(inner1.0, inner1.1.max(low), out), inner1.1);
                    let po0 = self.p(outer0.0, zof(outer0.0, outer0.1, 0.002), outer0.1);
                    let po1 = self.p(outer1.0, zof(outer1.0, outer1.1, 0.002), outer1.1);
                    self.body.quad_dir(pi0, po0, po1, pi1, mat, [sx, 0.0, 0.0]);
                    // Return face into the arch.
                    let back = |p: V3| [p[0] - sx * (out + 0.02), p[1], p[2]];
                    let toward = [0.0, -(d0y + d1y) * 0.5, -(d0s + d1s) * 0.5];
                    self.body
                        .quad_dir(pi0, pi1, back(pi1), back(pi0), mat, toward);
                }
            }
        }
    }

    // ---------------------------------------------------------------- details

    fn spoiler(&mut self) {
        let spec = self.b.spec;
        let l = self.b.length;
        let m = self.m;
        match spec.rear.spoiler {
            Spoiler::None => {}
            Spoiler::Lip => {
                let s = l - 0.05;
                let f = self.b.frame(s);
                let x = f.z_re * 0.95;
                let y = f.top_y(x) + 0.02;
                let a = self.p(s, -x, y);
                let b = self.p(s, x, y);
                self.body
                    .beam(a, b, norm([0.0, 1.0, 0.6]), 0.035, 0.008, m.paint);
            }
            Spoiler::Wing => {
                let s = l - 0.18;
                let f = self.b.frame(s);
                let y = f.yt + 0.17;
                for sx in [-1.0f32, 1.0] {
                    let x = sx * f.z_re * 0.7;
                    let y0 = f.top_y(x.abs());
                    self.body.beam(
                        self.p(s, x, y0),
                        self.p(s + 0.03, x, y),
                        FWD,
                        0.01,
                        0.05,
                        m.trim,
                    );
                }
                let a = self.p(s + 0.02, -f.z_re * 0.95, y + 0.01);
                let b = self.p(s + 0.02, f.z_re * 0.95, y + 0.01);
                self.body
                    .beam(a, b, norm([0.0, 1.0, 0.25]), 0.12, 0.012, m.paint);
                for sx in [-1.0f32, 1.0] {
                    let x = sx * f.z_re * 0.95;
                    self.body.aabb(
                        [x - 0.006, y - 0.04, s - 0.11 - self.b.axle[0]],
                        [x + 0.006, y + 0.06, s + 0.15 - self.b.axle[0]],
                        m.paint,
                    );
                }
            }
        }
    }

    fn louvers(&mut self) {
        let g = self.b.spec.glass.backlight;
        let m = if self.b.has(Detail::BlackTrim) {
            self.m.trim
        } else {
            self.m.paint
        };
        let n = if self.lod0() { 8 } else { 4 };
        let (s0, s1) = (g.0 + 0.07, g.1 - 0.06);
        for k in 0..n {
            let s = lerp(s0, s1, (k as f32 + 0.5) / n as f32);
            let f = self.b.frame(s);
            let x = (f.z_re - 0.07).max(0.2);
            let ya = self.b.frame(s - 0.03).yt;
            let yb = self.b.frame(s + 0.03).yt;
            // Glass tangent (rearward, down) and normal; fins lean 45° back from the normal.
            let t = norm([0.0, yb - ya, 0.06]);
            let nrm = norm(cross([1.0, 0.0, 0.0], t));
            let nrm = if nrm[1] < 0.0 { scale(nrm, -1.0) } else { nrm };
            let fin = norm(add(nrm, t));
            let up = norm(cross(fin, [1.0, 0.0, 0.0]));
            let c = madd(self.p(s, 0.0, f.top_y(0.0)), fin, 0.022);
            self.body
                .beam([-x, c[1], c[2]], [x, c[1], c[2]], up, 0.022, 0.004, m);
        }
        // Side frames.
        for sx in [-1.0f32, 1.0] {
            let fa = self.b.frame(s0 - 0.05);
            let fb = self.b.frame(s1 + 0.04);
            let xa = (fa.z_re - 0.05) * sx;
            let xb = (fb.z_re - 0.05) * sx;
            let a = self.p(s0 - 0.05, xa, fa.top_y(xa.abs()) + 0.012);
            let b = self.p(s1 + 0.04, xb, fb.top_y(xb.abs()) + 0.012);
            self.body.beam(a, b, UP, 0.025, 0.012, m);
        }
    }

    fn hood_scoop(&mut self) {
        let g = self.b.spec.glass.windshield;
        let (s0, s1) = (g.0 * 0.45, g.0 * 0.85);
        let f0 = self.b.frame(s0);
        let f1 = self.b.frame(s1);
        let w = 0.17;
        let a0 = self.p(s0, -w, f0.top_y(w) - 0.01);
        let b0 = self.p(s0, w, f0.top_y(w) - 0.01);
        let a1 = self.p(s1, -w, f1.top_y(w) - 0.01);
        let b1 = self.p(s1, w, f1.top_y(w) - 0.01);
        let up = [0.0, 0.07, 0.0];
        let (a0u, b0u) = (add(a0, up), add(b0, up));
        let (a1u, b1u) = (add(a1, [0.0, 0.03, 0.0]), add(b1, [0.0, 0.03, 0.0]));
        let m = self.m.paint;
        self.body.quad_dir(a0u, b0u, b1u, a1u, m, UP);
        self.body.quad_dir(a0, a0u, a1u, a1, m, [-1.0, 0.0, 0.0]);
        self.body.quad_dir(b0, b0u, b1u, b1, m, [1.0, 0.0, 0.0]);
        self.body.quad_dir(a1, a1u, b1u, b1, m, BACK);
        // Dark mouth.
        self.body.quad_dir(a0, b0, b0u, a0u, self.m.grille, FWD);
    }

    /// Station pairs along the side outside the arches, for side decals.
    fn side_spans(&self, s0: f32, s1: f32) -> Vec<(f32, f32)> {
        let st = &self.b.stations;
        let mut v: Vec<(f32, f32)> = Vec::new();
        for i in 0..st.len() - 1 {
            let (a, b) = (st[i].s, st[i + 1].s);
            if b - a < 0.02 || a < s0 || b > s1 {
                continue;
            }
            // Merge short neighbors (seams leave a gap, arches keep their own stations).
            match v.last_mut() {
                Some(last) if last.1 == a && b - last.0 < 0.3 && st[i].clamp.is_none() => {
                    last.1 = b
                }
                _ => v.push((a, b)),
            }
        }
        v
    }

    /// Lowest decal height at `s` that clears the arches by `margin`.
    fn arch_floor(&self, s: f32, margin: f32) -> f32 {
        let b = self.b;
        let mut y = 0.0f32;
        for &ax in &b.axle {
            let ds = s - ax;
            let r = b.ra + margin;
            if ds.abs() < r {
                y = y.max(b.wy + sqrt(r * r - ds * ds));
            }
        }
        y
    }

    fn side_stripe(&mut self) {
        let crease = self.b.spec.seams.creases.first().copied();
        let l = self.b.length;
        let spans = self.side_spans(0.12, l - 0.08);
        let mat = self.m.accent;
        for sx in [-1.0f32, 1.0] {
            for &(a, b) in &spans {
                let yat = |s: f32| match crease {
                    Some((x0, y0, x1, y1)) => {
                        lerp(y0, y1, clamp((s - x0) / (x1 - x0), 0.0, 1.0)) - 0.035
                    }
                    None => {
                        let f = self.b.frame(s);
                        lerp(f.yb, f.y_ledge, 0.6)
                    }
                };
                let (ya, yb) = (yat(a), yat(b));
                if ya - 0.02 < self.arch_floor(a, 0.03) || yb - 0.02 < self.arch_floor(b, 0.03) {
                    continue;
                }
                self.side_patch(
                    sx,
                    a,
                    b,
                    [ya - 0.016, yb - 0.016],
                    [ya + 0.016, yb + 0.016],
                    0.003,
                    mat,
                );
            }
        }
    }

    /// Wood-grain side panels: full height (a wagon's), or a narrow `applique` band under
    /// the belt.
    fn wood_panel(&mut self, applique: bool) {
        let b = self.b;
        let l = b.length;
        let s0 = b.axle[0] + b.ra + 0.06;
        let s1 = if applique {
            b.axle[1] + b.ra + 0.10
        } else {
            l - 0.10
        };
        let (lo_off, hi_off) = if applique { (0.25, 0.08) } else { (0.0, 0.07) };
        let spans = self.side_spans(s0, s1);
        let wood = self.m.wood;
        let surround = self.m.wood_trim;
        let lod0 = self.lod0();
        for sx in [-1.0f32, 1.0] {
            for &(a, c) in &spans {
                let y_lo = |s: f32| {
                    let f = b.frame(s);
                    let lo = if applique {
                        f.y_ledge - lo_off
                    } else {
                        f.yb + 0.16
                    };
                    lo.max(self.arch_floor(s, 0.05))
                };
                let y_hi = |s: f32| b.frame(s).y_ledge - hi_off;
                let (la, lc) = (y_lo(a), y_lo(c));
                let (ha, hc) = (y_hi(a), y_hi(c));
                if ha - la < 0.08 || hc - lc < 0.08 {
                    continue;
                }
                let t = 0.028;
                self.side_patch(sx, a, c, [la + t, lc + t], [ha - t, hc - t], 0.004, wood);
                self.side_patch(sx, a, c, [ha - t, hc - t], [ha, hc], 0.006, surround);
                self.side_patch(sx, a, c, [la, lc], [la + t, lc + t], 0.006, surround);
            }
            if lod0 {
                // End caps of the surround.
                for (s, ds) in [
                    (s0.max(spans.first().map_or(s0, |x| x.0)), 0.03),
                    (spans.last().map_or(s1, |x| x.1), -0.03),
                ] {
                    let f = b.frame(s);
                    let lo = if applique {
                        f.y_ledge - lo_off
                    } else {
                        f.yb + 0.16
                    };
                    let lo = lo.max(self.arch_floor(s, 0.05));
                    let hi = f.y_ledge - hi_off;
                    let (sa, sb) = if ds > 0.0 { (s, s + ds) } else { (s + ds, s) };
                    self.side_patch(sx, sa, sb, [lo, lo], [hi, hi], 0.0065, surround);
                }
            }
        }
    }

    /// Clear lenses in chrome bezels on the front fender sides, behind the headlamps. They
    /// are unlit (they would only light while turning).
    fn cornering_lamps(&mut self) {
        let b = self.b;
        let band = self.band(true);
        let yc = (band.y0 + band.y1) * 0.5;
        let s0 = 0.16f32;
        let s1 = (s0 + 0.16).min(b.axle[0] - b.ra - 0.03).max(s0 + 0.06);
        let (h, e) = (0.055f32.min((band.y1 - band.y0) * 0.45), 0.012);
        let (za, zb) = (s0 - b.axle[0], s1 - b.axle[0]);
        for sx in [-1.0f32, 1.0] {
            let x = self.side((s0 + s1) * 0.5, yc);
            self.body.aabb(
                [sx * (x - 0.01), yc - h - e, za - e],
                [sx * (x + 0.006), yc + h + e, zb + e],
                self.m.chrome,
            );
            self.lights.aabb(
                [sx * (x - 0.005), yc - h, za],
                [sx * (x + 0.009), yc + h, zb],
                self.m.reverse,
            );
        }
    }

    /// A bright molding along the first crease (or partway up the side), clear of the
    /// arches and the wood.
    fn side_molding(&mut self) {
        if self.b.has(Detail::WoodPanel) {
            return;
        }
        let l = self.b.length;
        let spans = self.side_spans(0.08, l - 0.08);
        for sx in [-1.0f32, 1.0] {
            for &(a, c) in &spans {
                let (ya, yc) = (self.b.split_y(a) - 0.02, self.b.split_y(c) - 0.02);
                if ya < self.arch_floor(a, 0.02) || yc < self.arch_floor(c, 0.02) {
                    continue;
                }
                self.side_patch(
                    sx,
                    a,
                    c,
                    [ya, yc],
                    [ya + 0.014, yc + 0.014],
                    0.005,
                    self.m.chrome,
                );
            }
        }
    }

    fn rocker_trim(&mut self) {
        let l = self.b.length;
        let spans = self.side_spans(0.1, l - 0.1);
        for sx in [-1.0f32, 1.0] {
            for &(a, c) in &spans {
                let fa = self.b.frame(a);
                let fc = self.b.frame(c);
                let ya = fa.yb + 0.09;
                let yc = fc.yb + 0.09;
                if ya < self.arch_floor(a, 0.04) || yc < self.arch_floor(c, 0.04) {
                    continue;
                }
                let mat = if self.b.has(Detail::ChromeTrim) {
                    self.m.chrome
                } else {
                    self.m.trim
                };
                self.side_patch(sx, a, c, [ya, yc], [ya + 0.03, yc + 0.03], 0.004, mat);
            }
        }
    }

    fn roof_spans(&self) -> (f32, f32) {
        let g = &self.b.spec.glass;
        (g.windshield.1 + 0.02, g.backlight.0 - 0.02)
    }

    fn vinyl_roof(&mut self) {
        let (s0, s1) = self.roof_spans();
        let f = self.b.frame((s0 + s1) * 0.5);
        let x = f.z_re - 0.01;
        self.surface_patch(s0, s1, -x, x, 0.004, self.m.accent.uv(Uv::Top), 4);
    }

    fn rally_stripes(&mut self) {
        let g = &self.b.spec.glass;
        let l = self.b.length;
        let roof = if self.b.convertible {
            (0.0, 0.0)
        } else {
            self.roof_spans()
        };
        let spans = [
            (0.03, g.windshield.0 - 0.01),
            roof,
            (g.backlight.1 + 0.01, l - 0.02),
        ];
        for (s0, s1) in spans {
            if s1 - s0 < 0.05 {
                continue;
            }
            for sx in [-1.0f32, 1.0] {
                self.surface_patch(s0, s1, sx * 0.07, sx * 0.19, 0.003, self.m.accent, 2);
            }
        }
    }

    fn mud_flaps(&mut self) {
        let b = self.b;
        let wh = &b.spec.wheels;
        for &ax in &b.axle {
            let z = ax + b.ra + 0.02 - b.axle[0];
            for sx in [-1.0f32, 1.0] {
                let x = sx * wh.track * 0.5;
                self.body.aabb(
                    [x - 0.1, 0.09, z],
                    [x + 0.1, b.wy + 0.08, z + 0.012],
                    self.m.rubber,
                );
            }
        }
    }

    fn roof_rack(&mut self) {
        let (s0, s1) = self.roof_spans();
        let (s0, s1) = (s0 + 0.12, s1 - 0.06);
        let m = if self.b.has(Detail::ChromeTrim) {
            self.m.chrome
        } else {
            self.m.trim
        };
        let lift = 0.05;
        let n = if self.lod0() { 3 } else { 1 };
        for sx in [-1.0f32, 1.0] {
            let pts: Vec<V3> = (0..=n)
                .map(|k| {
                    let s = lerp(s0, s1, k as f32 / n as f32);
                    let f = self.b.frame(s);
                    let x = f.z_re - 0.07;
                    self.p(s, sx * x, f.top_y(x) + lift)
                })
                .collect();
            for k in 0..n {
                self.body.beam(pts[k], pts[k + 1], UP, 0.014, 0.014, m);
            }
            // Feet.
            for &k in &[0usize, n] {
                let p = pts[k];
                self.body.aabb(
                    [p[0] - 0.02, p[1] - lift - 0.005, p[2] - 0.04],
                    [p[0] + 0.02, p[1], p[2] + 0.04],
                    self.m.trim,
                );
            }
        }
        if self.lod0() {
            for k in 0..3 {
                let s = lerp(s0 + 0.15, s1 - 0.15, k as f32 / 2.0);
                let f = self.b.frame(s);
                let x = f.z_re - 0.07;
                let y = f.top_y(x) + lift + 0.016;
                self.body.beam(
                    self.p(s, -x, y),
                    self.p(s, x, y),
                    UP,
                    0.012,
                    0.008,
                    self.m.trim,
                );
            }
        }
    }

    /// A spare wheel standing off the tail on a bracket, its tread showing round a cover
    /// in the accent color with a bright center ring.
    fn spare_tire(&mut self) {
        let b = self.b;
        let wh = &b.spec.wheels;
        let r = wh.radius * 0.92;
        let y = b.spec.rear.bumper_y.1 + r + 0.02;
        let z0 = b.length - b.axle[0] + 0.07;
        let z1 = z0 + wh.width * 0.9;
        let segs = match b.lod {
            0 => 20,
            1 => 12,
            _ => 8,
        };
        self.body.aabb(
            [-0.06, y - 0.06, z0 - 0.08],
            [0.06, y + 0.06, z0],
            self.m.trim,
        );
        self.body.cylinder(
            [0.0, y, z0],
            [0.0, y, z1],
            r,
            segs,
            true,
            false,
            self.m.tire,
        );
        // The cover: a band over the outer half of the tread and a flat face.
        let zc = (z0 + z1) * 0.5;
        let cover = self.m.accent;
        self.body.cylinder(
            [0.0, y, zc],
            [0.0, y, z1 + 0.012],
            r + 0.008,
            segs,
            false,
            false,
            cover,
        );
        self.body
            .disc([0.0, y, z1 + 0.012], BACK, r + 0.008, segs, cover);
        if self.lod0() {
            self.body.annulus(
                [0.0, y, z1 + 0.014],
                BACK,
                r * 0.3,
                r * 0.36,
                segs,
                self.m.chrome,
            );
        }
    }

    /// A trunk lock and a short bright name plate (no lettering) on the tail panel's left,
    /// over the lamps' line: what the chase camera sees of the car's identity.
    fn deck_badge(&mut self) {
        let band = self.band(false);
        let az = band.s - self.b.axle[0] + band.dir * 0.012;
        let y = band.y0 + 0.03;
        let x = -self.face_half(band.s, y) * 0.55;
        for k in 0..4 {
            let x0 = x - 0.075 + k as f32 * 0.04;
            self.body.aabb(
                [x0, y - 0.008, az - 0.004],
                [x0 + 0.03, y + 0.008 + 0.004 * (k % 2) as f32, az + 0.006],
                self.m.chrome,
            );
        }
        let lock = [0.0, band.y0 + 0.02, az];
        self.body.cylinder(
            sub(lock, [0.0, 0.0, band.dir * 0.006]),
            add(lock, [0.0, 0.0, band.dir * 0.008]),
            0.016,
            12,
            true,
            false,
            self.m.chrome,
        );
    }

    fn badge(&mut self, shape: BadgeShape, front: bool) {
        let band = self.band(front);
        // Front: top center of the nose. Rear: lower right of the tail panel.
        let (x, y) = if front {
            (0.0, band.y1 - 0.02)
        } else {
            (
                self.face_half(band.s, band.y0 + 0.05) * 0.55,
                band.y0 + 0.045,
            )
        };
        let az = band.s - self.b.axle[0] + band.dir * 0.02;
        let n = [0.0, 0.0, band.dir];
        let c = [x, y, az];
        let m = self.m.chrome;
        let r = 0.05;
        if !front {
            self.deck_badge();
        }
        match shape {
            BadgeShape::Circle => {
                self.body
                    .disc(add(c, scale(n, 0.001)), n, r * 0.7, 10, self.m.accent);
                self.body
                    .annulus(add(c, scale(n, 0.002)), n, r * 0.7, r, 12, m);
                self.body.cylinder(
                    sub(c, scale(n, 0.01)),
                    add(c, scale(n, 0.0015)),
                    r,
                    12,
                    false,
                    false,
                    m,
                );
            }
            BadgeShape::Diamond => {
                let s = core::f32::consts::FRAC_1_SQRT_2;
                self.body.obox(
                    c,
                    [[s, s, 0.0], [-s, s, 0.0], [0.0, 0.0, 1.0]],
                    [r * 0.7, r * 0.7, 0.006],
                    m,
                );
            }
            BadgeShape::Bar => {
                self.body.aabb(
                    [x - 0.11, y - 0.011, az - 0.006],
                    [x + 0.11, y + 0.011, az + 0.008],
                    m,
                );
                self.body.aabb(
                    [x - 0.1, y - 0.005, az + 0.008],
                    [x + 0.1, y + 0.005, az + 0.0095],
                    self.m.accent,
                );
            }
            BadgeShape::Chevron => {
                for (a, b) in [
                    ([x - 0.07, y + 0.028], [x, y - 0.02]),
                    ([x, y - 0.02], [x + 0.07, y + 0.028]),
                ] {
                    self.body
                        .beam([a[0], a[1], az], [b[0], b[1], az], n, 0.008, 0.012, m);
                }
            }
        }
    }
}
