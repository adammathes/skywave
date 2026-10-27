//! The cabin: inner trim shell (door cards, pillars, headliner), a two-tone dash with an
//! applique band, a hooded cluster with bezelled dials, louvered vents, a center stack and
//! console, steering wheel, column and stalks, upholstered seats with inserts, piping and
//! head restraints, door cards with armrests, mirror, visors and pedals.
//! Left-hand drive: the driver sits at `x = -driver_offset`.

use super::body::{Body, Slot};
use super::math::*;
use super::mesh::{Mat, Soup};
use super::params::Applique;
use super::{Cockpit, Dial, Materials};
use alloc::vec::Vec;

const UP: V3 = [0.0, 1.0, 0.0];
const BACK: V3 = [0.0, 0.0, 1.0];
const RIGHT: V3 = [1.0, 0.0, 0.0];
/// Width of a dial's bezel ring outside the face, m.
const BEZEL_W: f32 = 0.008;
/// Half thickness of piping and pleat lines, m.
const PIPE: f32 = 0.0045;

pub struct Interior<'a, 'b> {
    b: &'a Body<'b>,
    m: &'a Materials,
    pub soup: Soup,
    pub wheel: Soup,
    pub cockpit: Cockpit,
}

impl<'a, 'b> Interior<'a, 'b> {
    pub fn new(b: &'a Body<'b>, m: &'a Materials) -> Self {
        Interior {
            b,
            m,
            soup: Soup::new(35.0),
            wheel: Soup::new(40.0),
            cockpit: Cockpit::default(),
        }
    }

    fn z(&self, s: f32) -> f32 {
        s - self.b.axle[0]
    }

    fn lod0(&self) -> bool {
        self.b.lod == 0
    }

    /// Cabin floor height.
    fn floor_y(&self, s: f32) -> f32 {
        self.b.frame(s).yb + 0.09
    }

    /// Inner half-width of the cabin at `s`, below the belt.
    fn inner_w(&self, s: f32) -> f32 {
        self.b.frame(s).z_ledge - 0.06
    }

    pub fn build(&mut self) {
        let spec = self.b.spec;
        let i = &spec.interior;
        let x_d = -i.driver_offset;
        let eye = [x_d, i.seat.1 + 0.60, self.z(i.seat.0 + 0.16)];
        self.cockpit.eye = eye;
        if self.lod0() {
            self.shell();
        }
        self.floor_and_bulkheads();
        self.dash();
        self.steering();
        self.seats();
        if self.lod0() {
            self.stack();
            self.console();
            self.doors();
            self.mirror_and_visors();
            self.pedals();
        }
    }

    // ---------------------------------------------------------------- helpers

    /// A rounded box: a rounded-rectangle section (corner radius `r`) in the plane of
    /// `axes[2]` and `axes[1]`, swept along `axes[0]` over `±h[0]`, with flat ends.
    fn pillow(&mut self, c: V3, axes: [V3; 3], h: V3, r: f32, mat: Mat) {
        let r = r.min(h[1] * 0.95).min(h[2] * 0.95);
        let mut prof: Vec<(f32, f32)> = Vec::with_capacity(12);
        for (k, (cu, cv)) in [(1.0f32, 1.0f32), (-1.0, 1.0), (-1.0, -1.0), (1.0, -1.0)]
            .iter()
            .enumerate()
        {
            let (ou, ov) = (cu * (h[2] - r), cv * (h[1] - r));
            for t in 0..3 {
                let a = PI * 0.5 * (k as f32 + t as f32 * 0.5);
                let (sn, cs) = sin_cos(a);
                prof.push((ou + cs * r, ov + sn * r));
            }
        }
        let path = [madd(c, axes[0], -h[0]), madd(c, axes[0], h[0])];
        self.soup
            .sweep(&path, &[axes[2]; 2], &[axes[1]; 2], &prof, true, mat);
    }

    /// A thin raised line (piping, pleat, seam) from `a` to `b`.
    fn pipe(&mut self, a: V3, b: V3, up: V3, mat: Mat) {
        self.soup.beam(a, b, up, PIPE, PIPE, mat);
    }

    /// A vent: a dark mouth on a face looking along `n` (`u` across, `v` up), with a bright
    /// surround, horizontal louvers and vertical vanes.
    #[allow(clippy::too_many_arguments)]
    fn vent(&mut self, c: V3, u: V3, v: V3, n: V3, hu: f32, hv: f32, rim: Mat) {
        let m = self.m;
        let at = |a: f32, b: f32, d: f32| madd(madd(madd(c, u, a), v, b), n, d);
        self.soup.quad_dir(
            at(-hu, -hv, 0.0),
            at(hu, -hv, 0.0),
            at(hu, hv, 0.0),
            at(-hu, hv, 0.0),
            m.vent,
            n,
        );
        let t = 0.007;
        for (a0, b0, a1, b1) in [
            (-hu - t, hv, hu + t, hv + t),
            (-hu - t, -hv - t, hu + t, -hv),
            (-hu - t, -hv, -hu, hv),
            (hu, -hv, hu + t, hv),
        ] {
            let lo = madd(madd(madd(c, u, a0), v, b0), n, -0.004);
            let hi = madd(madd(madd(c, u, a1), v, b1), n, 0.004);
            self.soup.obox(
                scale(add(lo, hi), 0.5),
                [u, v, n],
                [(a1 - a0) * 0.5, (b1 - b0) * 0.5, 0.004],
                rim,
            );
        }
        let nl = 4;
        for k in 0..nl {
            let b = -hv + 2.0 * hv * (k as f32 + 0.5) / nl as f32;
            self.soup.obox(
                at(0.0, b, 0.002),
                [u, norm(add(v, scale(n, 0.6))), n],
                [hu - 0.002, 0.0025, 0.004],
                m.int_trim.shade(1.2),
            );
        }
        for k in 0..3 {
            let a = -hu + 2.0 * hu * (k as f32 + 1.0) / 4.0;
            self.soup.obox(
                at(a, 0.0, -0.002),
                [u, v, n],
                [0.002, hv - 0.002, 0.005],
                m.vent.shade(1.6),
            );
        }
    }

    /// Lateral position of the door card at `(s, y)`, right side.
    fn card_x(&self, s: f32, y: f32) -> f32 {
        let f = self.b.frame(s);
        let t = clamp(
            (y - f.yb - 0.09) / (f.y_ledge - f.yb - 0.1).max(0.05),
            0.0,
            1.0,
        );
        f.z_ledge - 0.06 + 0.01 * t
    }

    /// A panel on the door card of side `sx` from `s0` to `s1`, `y0` to `y1`, `inset` in from
    /// the card, following its curve.
    #[allow(clippy::too_many_arguments)]
    fn card_panel(&mut self, sx: f32, s0: f32, s1: f32, y0: f32, y1: f32, inset: f32, mat: Mat) {
        let n = 4;
        for k in 0..n {
            let sa = lerp(s0, s1, k as f32 / n as f32);
            let sb = lerp(s0, s1, (k + 1) as f32 / n as f32);
            let p = |s: f32, y: f32| [sx * (self.card_x(s, y) - inset), y, self.z(s)];
            self.soup.quad_dir(
                p(sa, y0),
                p(sb, y0),
                p(sb, y1),
                p(sa, y1),
                mat,
                [-sx, 0.0, 0.0],
            );
        }
    }

    // ---------------------------------------------------------------- shell

    fn cabin_end(&self) -> f32 {
        let spec = self.b.spec;
        if self.b.convertible {
            // The front wall of the well the top folds into.
            spec.glass.backlight.0
        } else if spec.interior.rear_seat {
            self.b.length - 0.06
        } else {
            spec.glass.backlight.1.min(self.b.length - 0.1)
        }
    }

    /// Inward-facing trim over the inside of the body, skipping the glass. A convertible
    /// keeps only the door cards: no pillars or headliner over the open cabin; a targa has
    /// no headliner under its opening.
    fn shell(&mut self) {
        let open = self.b.convertible;
        let b = self.b;
        let g = &b.spec.glass;
        let s0 = g.windshield.0 + 0.02;
        let s1 = self.cabin_end();
        let st: Vec<f32> = b
            .stations
            .iter()
            .filter(|t| t.clamp.is_none() && t.seam.is_none() && t.s >= s0 && t.s <= s1)
            .map(|t| t.s)
            .collect();
        let mut sts: Vec<f32> = Vec::new();
        for s in st {
            if sts.last().is_none_or(|&l| s - l > 0.01) {
                sts.push(s);
            }
        }
        let ring = |s: f32| -> [[f32; 2]; 7] {
            let (p, f) = b.ring_plain(s);
            let gb = p[b.idx(Slot::GlassB)];
            let rr = p[b.idx(Slot::RoofRail)];
            let re = p[b.idx(Slot::RoofEdge)];
            let fl = f.yb + 0.09;
            let inset = 0.035;
            [
                [f.z_ledge - 0.06, fl],
                [f.z_ledge - 0.05, f.y_ledge - 0.012],
                [gb[0] - inset, gb[1] - 0.005],
                [rr[0] - inset, rr[1] - 0.012],
                [re[0] - inset, re[1] - 0.03],
                [re[0] * 0.5, f.top_y(re[0] * 0.5) - 0.03],
                [0.0, f.yt - 0.03],
            ]
        };
        let m = self.m;
        for k in 0..sts.len().saturating_sub(1) {
            let (sa, sb) = (sts[k], sts[k + 1]);
            let (ra, rb) = (ring(sa), ring(sb));
            let within = |r: (f32, f32)| sa >= r.0 - 1e-4 && sb <= r.1 + 1e-4;
            let side_glass = g.side.iter().any(|&r| within(r));
            let top_glass = within(g.windshield) || within(g.backlight);
            let targa = b.targa.is_some_and(within);
            for j in 0..6 {
                let mat: Mat = match j {
                    _ if open && j >= 2 => continue,
                    _ if targa && j >= 3 => continue,
                    0 => m.door_card,
                    1 => m.int_trim,
                    2 if side_glass => continue,
                    2 => m.int_trim,
                    3 => m.int_trim.shade(0.8),
                    _ if top_glass => continue,
                    _ => m.headliner,
                };
                let q = [
                    [ra[j][0], ra[j][1], self.z(sa)],
                    [ra[j + 1][0], ra[j + 1][1], self.z(sa)],
                    [rb[j + 1][0], rb[j + 1][1], self.z(sb)],
                    [rb[j][0], rb[j][1], self.z(sb)],
                ];
                let yc = (ra[0][1] + ra[6][1]) * 0.5;
                for sx in [1.0f32, -1.0] {
                    let qq = q.map(|p| [p[0] * sx, p[1], p[2]]);
                    let mid = scale(add(add(qq[0], qq[1]), add(qq[2], qq[3])), 0.25);
                    let dir = [-mid[0], yc - mid[1], 0.0];
                    self.soup.quad_dir(qq[0], qq[1], qq[2], qq[3], mat, dir);
                }
            }
        }
    }

    fn floor_and_bulkheads(&mut self) {
        let b = self.b;
        let spec = b.spec;
        let i = &spec.interior;
        let m = self.m;
        let s_fw = i.dash_x - 0.55;
        let fl = self.floor_y(i.seat.0);
        let s_end = self.cabin_end();
        let s_floor_end = if i.rear_seat {
            i.seat.0 + 1.1
        } else {
            i.seat.0 + 0.32
        };
        let w = self.inner_w(i.seat.0);
        let ax0 = b.axle[0];
        let z = |s: f32| s - ax0;
        self.soup.quad_dir(
            [-w, fl, z(s_fw)],
            [w, fl, z(s_fw)],
            [w, fl, z(s_floor_end)],
            [-w, fl, z(s_floor_end)],
            m.carpet,
            UP,
        );
        // Firewall.
        let wf = self.inner_w(s_fw);
        let fy1 = i.dash_height - 0.2;
        self.soup.quad_dir(
            [-wf, fl, z(s_fw)],
            [wf, fl, z(s_fw)],
            [wf, fy1, z(s_fw)],
            [-wf, fy1, z(s_fw)],
            m.carpet,
            BACK,
        );
        // Transmission tunnel.
        self.soup.aabb(
            [-0.12, fl, z(s_fw)],
            [0.12, fl + 0.12, z(i.dash_x + 0.1)],
            m.carpet,
        );
        if self.lod0() {
            // Floor mats with a heel pad, sill plates along the door openings.
            let mut wells = alloc::vec![i.seat.0 - 0.45];
            if i.rear_seat {
                wells.push(i.seat.0 + 0.45);
            }
            for (k, &sm) in wells.iter().enumerate() {
                for sx in [-1.0f32, 1.0] {
                    let x = sx * i.driver_offset;
                    let (h0, h1) = if k == 0 { (0.28, 0.12) } else { (0.16, 0.12) };
                    self.soup.aabb(
                        [x - 0.19, fl, z(sm - h0)],
                        [x + 0.19, fl + 0.008, z(sm + h1)],
                        m.carpet.shade(0.62),
                    );
                    if k == 0 && sx < 0.0 {
                        self.soup.aabb(
                            [x - 0.09, fl + 0.008, z(sm - 0.05)],
                            [x + 0.09, fl + 0.012, z(sm + 0.08)],
                            m.switch,
                        );
                    }
                }
            }
            let (s0, s1) = (i.dash_x + 0.05, i.seat.0 + 0.3);
            for sx in [-1.0f32, 1.0] {
                let x = sx * (self.inner_w((s0 + s1) * 0.5) - 0.02);
                self.soup.aabb(
                    [x - 0.03, fl, z(s0)],
                    [x + 0.03, fl + 0.025, z(s1)],
                    m.bezel.shade(0.8),
                );
            }
        }
        let f_end = b.frame(s_floor_end);
        if i.rear_seat {
            // Raised cargo floor over the rear axle and the tailgate's inner panel.
            let cy = (fl + 0.22)
                .max(b.wy + b.ra + 0.03)
                .min(f_end.y_ledge - 0.15);
            let we = self.inner_w(s_end);
            self.soup.aabb(
                [-w, fl, z(s_floor_end)],
                [w, cy, z(s_floor_end + 0.02)],
                m.carpet,
            );
            self.soup.quad_dir(
                [-we, cy, z(s_floor_end)],
                [we, cy, z(s_floor_end)],
                [we, cy, z(s_end)],
                [-we, cy, z(s_end)],
                m.carpet.shade(0.9),
                UP,
            );
            let ty = b.frame(s_end).y_ledge;
            self.soup.quad_dir(
                [-we, cy, z(s_end)],
                [we, cy, z(s_end)],
                [we, ty, z(s_end)],
                [-we, ty, z(s_end)],
                m.door_card,
                [0.0, 0.0, -1.0],
            );
        } else {
            // Bulkhead behind the seats and a parcel shelf back to the rear glass.
            let sy = f_end.y_ledge + 0.02;
            self.soup.quad_dir(
                [-w, fl, z(s_floor_end)],
                [w, fl, z(s_floor_end)],
                [w, sy, z(s_floor_end)],
                [-w, sy, z(s_floor_end)],
                m.int_trim,
                [0.0, 0.0, -1.0],
            );
            let we = self.inner_w(s_end).min(w);
            self.soup.quad_dir(
                [-w, sy, z(s_floor_end)],
                [w, sy, z(s_floor_end)],
                [we, sy, z(s_end)],
                [-we, sy, z(s_end)],
                m.carpet,
                UP,
            );
            if self.lod0() {
                // Rear speakers on the shelf.
                for sx in [-1.0f32, 1.0] {
                    let c = [sx * w * 0.6, sy + 0.012, z((s_floor_end + s_end) * 0.5)];
                    self.soup.aabb(
                        sub(c, [0.09, 0.012, 0.06]),
                        add(c, [0.09, 0.012, 0.06]),
                        m.int_trim,
                    );
                    for k in 0..4 {
                        let zz = c[2] - 0.045 + k as f32 * 0.03;
                        self.soup.aabb(
                            [c[0] - 0.08, c[1] + 0.012, zz - 0.004],
                            [c[0] + 0.08, c[1] + 0.015, zz + 0.004],
                            m.vent,
                        );
                    }
                }
            }
        }
    }

    // ---------------------------------------------------------------- dash

    fn dash(&mut self) {
        let b = self.b;
        let spec = b.spec;
        let i = &spec.interior;
        let m = self.m;
        let lod0 = self.lod0();
        let ws = spec.glass.windshield.0 + 0.03;
        let belt = b.frame(ws).y_ledge;
        let dh = i.dash_height;
        let dx = i.dash_x;
        let s_fw = dx - 0.55;
        let w = self.inner_w(dx) + 0.02;
        // Two closed profiles in (s, y), extruded across the car: the padded top in the
        // lighter tone and the face and underside below it.
        let top_sy = [
            (ws, belt - 0.004),
            (dx - 0.12, dh),
            (dx - 0.01, dh - 0.008),
            (dx + 0.015, dh - 0.05),
            (dx - 0.06, dh - 0.07),
            (ws + 0.04, belt - 0.05),
        ];
        let low_sy = [
            (dx + 0.0145, dh - 0.048),
            (dx - 0.01, dh - 0.24),
            (dx - 0.3, dh - 0.3),
            (s_fw, dh - 0.22),
            (s_fw + 0.02, belt - 0.06),
            (dx - 0.06, dh - 0.07),
        ];
        let (rs, ry) = (dx - 0.2, dh - 0.1);
        let path = [[-w, ry, self.z(rs)], [w, ry, self.z(rs)]];
        for (prof_sy, mat) in [(&top_sy, m.dash_top), (&low_sy, m.dash)] {
            let prof: Vec<(f32, f32)> = prof_sy.iter().map(|&(s, y)| (s - rs, y - ry)).collect();
            self.soup
                .sweep(&path, &[BACK, BACK], &[UP, UP], &prof, true, mat);
        }
        // The face between the pad's lip and the lower edge, as a plane in (y, s).
        let face_s = |y: f32| lerp(dx + 0.015, dx - 0.01, (dh - 0.05 - y) / 0.19);
        if lod0 {
            self.applique(w, dh, &face_s);
            self.glovebox(w, dh, &face_s);
        }

        // Hooded cluster in front of the driver.
        let xd = -i.driver_offset;
        let n_dials = i.dials as usize;
        let hw = 0.19f32;
        // The face stands just behind the dash's rear lip (dx + 0.015), under the hood: set
        // into the dash (dx - 0.10) its lower half was below the dash top as the driver sees it.
        let zc = self.z(dx + 0.025);
        let tilt = norm([0.0, 0.22, 1.0]);
        if lod0 {
            // A curved brow: an arched shell from the pad forward of the cluster to a lip
            // over its face.
            let (za, zb) = (self.z(dx - 0.17), self.z(dx + 0.04));
            let mut prof: Vec<(f32, f32)> = Vec::new();
            let n = 6;
            for k in 0..=n {
                let t = k as f32 / n as f32;
                let (sn, _) = sin_cos(PI * t);
                prof.push((lerp(za, zb, t), dh + 0.045 + 0.03 * sn + 0.005 * t));
            }
            for k in (0..=n).rev() {
                let t = k as f32 / n as f32;
                let (sn, _) = sin_cos(PI * t);
                prof.push((lerp(za, zb, t) + 0.004, dh + 0.033 + 0.03 * sn));
            }
            let prof: Vec<(f32, f32)> = prof.iter().map(|&(z, y)| (z - zc, y - dh)).collect();
            let path = [[xd - hw - 0.012, dh, zc], [xd + hw + 0.012, dh, zc]];
            self.soup
                .sweep(&path, &[BACK, BACK], &[UP, UP], &prof, true, m.dash_top);
        } else {
            self.soup.aabb(
                [xd - hw, dh + 0.06, self.z(dx - 0.17)],
                [xd + hw, dh + 0.075, self.z(dx + 0.035)],
                m.dash,
            );
        }
        for sx in [-1.0f32, 1.0] {
            let x = xd + sx * hw;
            self.soup.aabb(
                [x - 0.012, dh - 0.07, self.z(dx - 0.17)],
                [x + 0.012, dh + 0.07, self.z(dx + 0.03)],
                m.dash,
            );
        }
        let face_c = [xd, dh - 0.005, zc];
        let (fu, fv) = (RIGHT, cross(tilt, RIGHT));
        let fq = [
            madd(madd(face_c, fu, -hw + 0.012), fv, -0.065),
            madd(madd(face_c, fu, hw - 0.012), fv, -0.065),
            madd(madd(face_c, fu, hw - 0.012), fv, 0.065),
            madd(madd(face_c, fu, -hw + 0.012), fv, 0.065),
        ];
        self.soup
            .quad_dir(fq[0], fq[1], fq[2], fq[3], m.cluster, tilt);
        // The dial and its bezel ring (`BEZEL_W` wide) stay inside the face's 0.065 m
        // half-height, under the brow.
        let r = (0.36 / (2.0 * n_dials as f32) - 0.012).min(0.065 - BEZEL_W - 0.004);
        let segs = if lod0 { 24 } else { 8 };
        let mut dials = [Dial::default(); 3];
        for (k, dial) in dials.iter_mut().enumerate().take(n_dials) {
            let off = (k as f32 - (n_dials as f32 - 1.0) * 0.5) * (2.0 * r + 0.024);
            let c = madd(madd(face_c, fu, off), tilt, 0.004);
            self.soup.disc(c, tilt, r, segs, m.dial);
            if lod0 {
                // A chrome bezel ring standing toward the driver, a dark inner wall and an
                // odometer window under the hub on the first dial.
                self.soup.lathe(
                    c,
                    tilt,
                    &[
                        (0.0, r),
                        (0.005, r + 0.002),
                        (0.008, r + 0.004),
                        (0.009, r + 0.007),
                        (0.002, r + BEZEL_W),
                    ],
                    segs,
                    0.0,
                    (0.0, r + 0.015),
                    m.bezel,
                );
                self.soup.annulus(
                    madd(c, tilt, 0.0005),
                    tilt,
                    r * 0.93,
                    r,
                    segs,
                    m.cluster.shade(1.4),
                );
                if k == 0 {
                    let oc = madd(madd(c, fv, -0.42 * r), tilt, 0.0012);
                    let (ow, oh) = (0.36 * r, 0.09 * r);
                    self.soup.quad_dir(
                        madd(madd(oc, fu, -ow), fv, -oh),
                        madd(madd(oc, fu, ow), fv, -oh),
                        madd(madd(oc, fu, ow), fv, oh),
                        madd(madd(oc, fu, -ow), fv, oh),
                        m.cluster,
                        tilt,
                    );
                }
            }
            *dial = Dial {
                center: madd(c, tilt, 0.001),
                normal: tilt,
                up: fv,
                radius: r,
            };
        }
        self.cockpit.dials = dials;
        self.cockpit.dial_count = n_dials as u8;
        if lod0 {
            self.cluster_extras(face_c, fu, fv, tilt, hw, n_dials, r);
            // Square 80s vents at the dash ends.
            for sx in [-1.0f32, 1.0] {
                let x = sx * (w - 0.12);
                let y = dh - 0.08;
                let c = [x, y, self.z(face_s(y)) + 0.004];
                let n = norm([0.0, 0.13, 1.0]);
                self.vent(c, RIGHT, cross(n, RIGHT), n, 0.05, 0.032, m.int_trim);
            }
        }
    }

    /// The applique band across the dash face, with grain lines (wood) or a brushed sheen,
    /// and bright edges.
    fn applique(&mut self, w: f32, dh: f32, face_s: &dyn Fn(f32) -> f32) {
        let m = self.m;
        let (y0, y1) = (dh - 0.165, dh - 0.105);
        let x0 = w - 0.03;
        let ax0 = self.b.axle[0];
        let p = |x: f32, y: f32, out: f32| [x, y, face_s(y) - ax0 + out];
        self.soup.quad_dir(
            p(-x0, y0, 0.003),
            p(x0, y0, 0.003),
            p(x0, y1, 0.003),
            p(-x0, y1, 0.003),
            m.applique,
            BACK,
        );
        let applique = self.b.spec.interior.applique;
        let lines: &[(f32, f32)] = match applique {
            Applique::Wood => &[(0.18, 0.72), (0.41, 0.6), (0.63, 0.78), (0.86, 0.66)],
            Applique::Brushed => &[(0.3, 1.25), (0.7, 1.25)],
            Applique::None => &[],
        };
        for &(t, k) in lines {
            let y = lerp(y0, y1, t);
            self.soup.quad_dir(
                p(-x0, y - 0.0018, 0.0034),
                p(x0, y - 0.0018, 0.0034),
                p(x0, y + 0.0018, 0.0034),
                p(-x0, y + 0.0018, 0.0034),
                m.applique.shade(k),
                BACK,
            );
        }
        for y in [y0, y1] {
            let a = p(-x0, y, 0.004);
            let b = p(x0, y, 0.004);
            self.soup.beam(a, b, UP, 0.003, 0.003, m.bezel);
        }
    }

    /// A glovebox door with its seam and latch on the passenger side of the face.
    fn glovebox(&mut self, w: f32, dh: f32, face_s: &dyn Fn(f32) -> f32) {
        let m = self.m;
        let (x0, x1) = (0.2f32, (w - 0.1).max(0.35));
        let (y0, y1) = (dh - 0.235, dh - 0.175);
        let ax0 = self.b.axle[0];
        let p = |x: f32, y: f32, out: f32| [x, y, face_s(y) - ax0 + out];
        self.soup.quad_dir(
            p(x0 - 0.006, y0 - 0.006, 0.0015),
            p(x1 + 0.006, y0 - 0.006, 0.0015),
            p(x1 + 0.006, y1 + 0.006, 0.0015),
            p(x0 - 0.006, y1 + 0.006, 0.0015),
            m.vent,
            BACK,
        );
        self.soup.quad_dir(
            p(x0, y0, 0.0035),
            p(x1, y0, 0.0035),
            p(x1, y1, 0.0035),
            p(x0, y1, 0.0035),
            m.dash.shade(1.08),
            BACK,
        );
        let c = p((x0 + x1) * 0.5, y1 - 0.016, 0.004);
        self.soup.aabb(
            sub(c, [0.025, 0.006, 0.0]),
            add(c, [0.025, 0.006, 0.008]),
            m.bezel,
        );
    }

    /// Warning lamps under the dials, and for a single-dial cluster two square side gauges.
    #[allow(clippy::too_many_arguments)]
    fn cluster_extras(
        &mut self,
        face_c: V3,
        fu: V3,
        fv: V3,
        tilt: V3,
        hw: f32,
        n_dials: usize,
        r: f32,
    ) {
        let m = self.m;
        let n = 8;
        let y = -0.056;
        for k in 0..n {
            let x = lerp(-hw + 0.04, hw - 0.04, k as f32 / (n - 1) as f32);
            let c = madd(madd(madd(face_c, fu, x), fv, y), tilt, 0.0015);
            let (a, b) = (0.009, 0.004);
            self.soup.quad_dir(
                madd(madd(c, fu, -a), fv, -b),
                madd(madd(c, fu, a), fv, -b),
                madd(madd(c, fu, a), fv, b),
                madd(madd(c, fu, -a), fv, b),
                m.warn[k % 4],
                tilt,
            );
        }
        if n_dials == 1 {
            for sx in [-1.0f32, 1.0] {
                let c = madd(madd(face_c, fu, sx * (r + 0.065)), tilt, 0.003);
                let h = 0.04;
                self.soup.quad_dir(
                    madd(madd(c, fu, -h), fv, -h),
                    madd(madd(c, fu, h), fv, -h),
                    madd(madd(c, fu, h), fv, h),
                    madd(madd(c, fu, -h), fv, h),
                    m.dial,
                    tilt,
                );
                let rim = madd(c, tilt, 0.003);
                self.soup.obox(
                    madd(rim, fv, h + 0.003),
                    [fu, fv, tilt],
                    [h + 0.006, 0.003, 0.004],
                    m.bezel,
                );
                self.soup.obox(
                    madd(rim, fv, -h - 0.003),
                    [fu, fv, tilt],
                    [h + 0.006, 0.003, 0.004],
                    m.bezel,
                );
                for e in [-1.0f32, 1.0] {
                    self.soup.obox(
                        madd(rim, fu, e * (h + 0.003)),
                        [fu, fv, tilt],
                        [0.003, h, 0.004],
                        m.bezel,
                    );
                }
            }
        }
    }

    fn stack(&mut self) {
        let spec = self.b.spec;
        let i = &spec.interior;
        let m = self.m;
        let dx = i.dash_x;
        let dh = i.dash_height;
        let fl = self.floor_y(dx);
        let face = self.z(dx + 0.05);
        self.soup.aabb(
            [-0.13, fl + 0.12, self.z(dx - 0.08)],
            [0.13, dh - 0.04, face],
            m.dash.shade(0.9),
        );
        // A bezel plate round the radio and climate controls.
        self.soup.aabb(
            [-0.122, dh - 0.31, face],
            [0.122, dh - 0.045, face + 0.004],
            m.applique,
        );
        // Center vents.
        for sx in [-1.0f32, 1.0] {
            let c = [sx * 0.06, dh - 0.085, face + 0.006];
            let n = BACK;
            self.vent(c, RIGHT, UP, n, 0.042, 0.024, m.int_trim);
        }
        // Radio head unit with a display quad for the station text.
        let ry = dh - 0.18;
        self.soup.aabb(
            [-0.095, ry, face - 0.01],
            [0.095, ry + 0.055, face + 0.018],
            m.radio,
        );
        let dz = face + 0.0185;
        let d = [
            [-0.06, ry + 0.017, dz],
            [0.035, ry + 0.017, dz],
            [0.035, ry + 0.04, dz],
            [-0.06, ry + 0.04, dz],
        ];
        self.soup.quad_dir(d[0], d[1], d[2], d[3], m.display, BACK);
        self.cockpit.radio_display = d;
        for k in 0..5 {
            let x = -0.06 + k as f32 * 0.019;
            self.soup.aabb(
                [x, ry + 0.005, dz - 0.004],
                [x + 0.015, ry + 0.012, dz + 0.004],
                m.int_trim.shade(1.4),
            );
        }
        // Cassette door over the display.
        self.soup.aabb(
            [-0.055, ry + 0.044, dz - 0.002],
            [0.03, ry + 0.051, dz + 0.001],
            m.vent,
        );
        for x in [-0.077f32, 0.06] {
            self.soup.cylinder(
                [x, ry + 0.028, dz - 0.002],
                [x, ry + 0.028, dz + 0.012],
                0.011,
                12,
                true,
                false,
                m.bezel.shade(0.8),
            );
        }
        // Climate: three rotary knobs with pointer marks and a fan rocker.
        let cy = ry - 0.07;
        self.soup.aabb(
            [-0.1, cy, face - 0.005],
            [0.1, cy + 0.05, face + 0.008],
            m.int_trim,
        );
        for k in 0..3 {
            let x = -0.06 + k as f32 * 0.06;
            let c = [x, cy + 0.025, face + 0.008];
            self.soup.cylinder(
                c,
                add(c, [0.0, 0.0, 0.014]),
                0.015,
                12,
                true,
                false,
                m.switch,
            );
            self.soup.aabb(
                [x - 0.002, cy + 0.028, face + 0.022],
                [x + 0.002, cy + 0.038, face + 0.0235],
                m.accent_int,
            );
            for t in [-1.0f32, 0.0, 1.0] {
                let (sn, cs) = sin_cos(t * 0.9);
                let p = [x + sn * 0.021, cy + 0.025 + cs * 0.021, face + 0.0085];
                self.soup.aabb(
                    sub(p, [0.0015, 0.0015, 0.0]),
                    add(p, [0.0015, 0.0015, 0.001]),
                    m.display,
                );
            }
        }
        // Ashtray and lighter.
        let ay = cy - 0.045;
        self.soup.aabb(
            [-0.07, ay, face - 0.004],
            [0.07, ay + 0.03, face + 0.008],
            m.int_trim.shade(0.85),
        );
        self.soup.aabb(
            [-0.03, ay + 0.022, face + 0.008],
            [0.03, ay + 0.027, face + 0.013],
            m.bezel,
        );
        self.soup.cylinder(
            [0.09, ay + 0.015, face],
            [0.09, ay + 0.015, face + 0.012],
            0.009,
            10,
            true,
            false,
            m.bezel,
        );
    }

    /// The console between the seats: the shifter in a bright gate with its boot and knob,
    /// cup wells and a padded armrest lid.
    fn console(&mut self) {
        let spec = self.b.spec;
        let i = &spec.interior;
        let m = self.m;
        let dx = i.dash_x;
        let fl = self.floor_y(dx);
        let face = self.z(dx + 0.05);
        let s_end = i.seat.0 + 0.05;
        let top = fl + 0.22;
        self.soup.aabb(
            [-0.1, fl, face - 0.02],
            [0.1, top, self.z(s_end)],
            m.dash.shade(0.85),
        );
        // Side panels in the applique, a pinstripe of piping along the top edges.
        for sx in [-1.0f32, 1.0] {
            self.soup.aabb(
                [sx * 0.1, top - 0.08, face],
                [sx * 0.1015, top - 0.03, self.z(s_end) - 0.02],
                m.applique,
            );
        }
        let sz = self.z(dx + 0.38);
        // Gate plate.
        self.soup.aabb(
            [-0.055, top, sz - 0.08],
            [0.055, top + 0.006, sz + 0.08],
            m.applique,
        );
        let t = 0.006;
        for (a, b) in [
            ([-0.058, sz - 0.083], [0.058, sz - 0.077]),
            ([-0.058, sz + 0.077], [0.058, sz + 0.083]),
            ([-0.058, sz - 0.08], [-0.052, sz + 0.08]),
            ([0.052, sz - 0.08], [0.058, sz + 0.08]),
        ] {
            self.soup
                .aabb([a[0], top, a[1]], [b[0], top + t + 0.002, b[1]], m.bezel);
        }
        if !i.manual {
            // The selector's lit P R N D strip.
            for k in 0..4 {
                let z = sz - 0.06 + k as f32 * 0.017;
                self.soup.aabb(
                    [0.028, top + 0.006, z - 0.004],
                    [0.04, top + 0.0075, z + 0.004],
                    m.display,
                );
            }
        }
        // Boot, shaft and knob.
        let base = [0.0, top + 0.006, sz];
        let up = norm([0.0, 1.0, -0.2]);
        self.soup.lathe(
            base,
            up,
            &[(0.0, 0.045), (0.02, 0.04), (0.045, 0.026), (0.075, 0.014)],
            12,
            0.0,
            (0.03, 0.0),
            m.int_trim.shade(0.7),
        );
        let knob_c = madd(base, up, 0.15);
        self.soup
            .beam(madd(base, up, 0.07), knob_c, BACK, 0.007, 0.007, m.bezel);
        let knob = if i.applique == Applique::Wood {
            m.applique
        } else {
            m.switch
        };
        self.soup.lathe(
            knob_c,
            up,
            &[
                (-0.01, 0.0),
                (-0.008, 0.012),
                (0.004, 0.02),
                (0.018, 0.018),
                (0.028, 0.01),
                (0.031, 0.0),
            ],
            12,
            0.0,
            (0.01, 0.0),
            knob,
        );
        // Cup wells.
        let cz = self.z(dx + 0.58);
        for sx in [-1.0f32, 1.0] {
            let c = [sx * 0.045, top, cz];
            self.soup.cylinder(
                add(c, [0.0, -0.06, 0.0]),
                add(c, [0.0, 0.001, 0.0]),
                0.034,
                12,
                false,
                true,
                m.switch,
            );
            self.soup
                .disc(add(c, [0.0, -0.06, 0.0]), UP, 0.034, 12, m.switch);
            self.soup
                .annulus(add(c, [0.0, 0.002, 0.0]), UP, 0.034, 0.04, 12, m.int_trim);
        }
        // Armrest lid with piping.
        let lz = self.z(s_end) - 0.14;
        let lc = [0.0, top + 0.03, lz];
        self.pillow(lc, [BACK, UP, RIGHT], [0.13, 0.03, 0.09], 0.02, m.seat);
        for sx in [-1.0f32, 1.0] {
            self.pipe(
                [sx * 0.09, top + 0.06, lz - 0.13],
                [sx * 0.09, top + 0.06, lz + 0.13],
                UP,
                m.piping,
            );
        }
    }

    // ---------------------------------------------------------------- wheel

    fn steering(&mut self) {
        let spec = self.b.spec;
        let i = &spec.interior;
        let m = self.m;
        let lod0 = self.lod0();
        let (sn, cs) = sin_cos(deg(i.column_angle_deg));
        let axis = [0.0, sn, cs];
        let p = [-i.driver_offset, i.wheel_pos.1, self.z(i.wheel_pos.0)];
        self.cockpit.wheel_pivot = p;
        self.cockpit.wheel_axis = axis;
        let r = i.wheel_radius;
        let up = norm(sub(UP, scale(axis, dot(UP, axis))));
        let right = norm(cross(up, axis));
        let (segs, tube) = if lod0 { (36, 8) } else { (10, 4) };
        self.wheel.torus(p, axis, r, 0.016, segs, tube, m.wheel_rim);
        let two = r >= 0.19;
        let spokes: &[f32] = if two {
            &[-0.25, PI + 0.25]
        } else {
            &[0.0, PI, -PI * 0.5]
        };
        for &a in spokes {
            let (s, c) = sin_cos(a);
            let dir = add(scale(right, c), scale(up, s));
            let p0 = madd(madd(p, axis, -0.03), dir, 0.05);
            let p1 = madd(p, dir, r - 0.008);
            self.wheel.beam(
                p0,
                p1,
                axis,
                if two { 0.03 } else { 0.02 },
                0.007,
                m.wheel_spoke,
            );
            if lod0 {
                // A bright insert along the spoke.
                self.wheel.beam(
                    madd(madd(p0, axis, 0.0075), dir, 0.01),
                    madd(madd(p1, axis, 0.0075), dir, -0.02),
                    axis,
                    if two { 0.008 } else { 0.005 },
                    0.0015,
                    m.bezel,
                );
            }
        }
        let hub_r = if two { 0.075 } else { 0.055 };
        if lod0 {
            // A padded horn pad with a small bright emblem ring (no maker's mark).
            self.wheel.lathe(
                p,
                axis,
                &[
                    (-0.07, hub_r),
                    (-0.03, hub_r),
                    (-0.022, hub_r * 0.92),
                    (-0.016, hub_r * 0.7),
                    (-0.014, hub_r * 0.35),
                    (-0.0135, 0.0),
                ],
                16,
                0.0,
                (-0.05, 0.0),
                m.wheel_spoke,
            );
            self.wheel.annulus(
                madd(p, axis, -0.0128),
                scale(axis, -1.0),
                0.012,
                0.018,
                12,
                m.bezel,
            );
        } else {
            self.wheel.cylinder(
                madd(p, axis, -0.07),
                madd(p, axis, -0.02),
                hub_r,
                6,
                true,
                false,
                m.wheel_spoke,
            );
        }
        // Column, shroud and stalks (fixed).
        self.soup.cylinder(
            madd(p, axis, -0.07),
            madd(p, axis, -0.42),
            0.03,
            if lod0 { 12 } else { 8 },
            false,
            false,
            m.int_trim,
        );
        if lod0 {
            self.pillow(
                madd(p, axis, -0.12),
                [right, up, axis],
                [0.06, 0.05, 0.05],
                0.02,
                m.int_trim,
            );
            // Ignition lock on the shroud's right side.
            let ig = madd(madd(madd(p, axis, -0.12), right, 0.06), up, -0.01);
            self.soup
                .cylinder(ig, madd(ig, right, 0.008), 0.012, 10, true, false, m.bezel);
            for sx in [-1.0f32, 1.0] {
                let a = madd(madd(p, axis, -0.11), right, sx * 0.05);
                let bb = madd(madd(a, right, sx * 0.12), up, -0.02);
                self.soup
                    .beam(a, bb, up, 0.006, 0.006, m.int_trim.shade(0.7));
                let tip = madd(bb, right, sx * 0.035);
                self.soup.cylinder(bb, tip, 0.01, 10, true, false, m.switch);
            }
        } else {
            self.soup.obox(
                madd(p, axis, -0.12),
                [right, up, axis],
                [0.06, 0.05, 0.05],
                m.int_trim,
            );
        }
    }

    // ---------------------------------------------------------------- seats

    /// A front seat at LOD1: blocks.
    fn seat_low(&mut self, cx: f32, s_h: f32, y_h: f32, half_w: f32) {
        let m = self.m;
        let fl = self.floor_y(s_h);
        let ax0 = self.b.axle[0];
        let z = |s: f32| s - ax0;
        let cw = half_w * 0.62;
        self.soup.aabb(
            [cx - half_w * 0.8, fl, z(s_h - 0.38)],
            [cx + half_w * 0.8, y_h - 0.09, z(s_h)],
            m.int_trim,
        );
        let (ts, tc) = sin_cos(deg(-6.0));
        let cush_ax = [RIGHT, [0.0, tc, -ts], [0.0, ts, tc]];
        let cc = [cx, y_h - 0.05, z(s_h - 0.17)];
        self.soup.obox(cc, cush_ax, [cw, 0.045, 0.24], m.seat);
        let (rs, rc) = sin_cos(deg(18.0));
        let d = [0.0, rc, rs];
        let nb = [0.0, -rs, rc];
        let hinge = [cx, y_h - 0.02, z(s_h + 0.05)];
        let ax = [RIGHT, d, nb];
        self.soup.obox(
            madd(madd(hinge, d, 0.3), nb, 0.05),
            ax,
            [cw, 0.29, 0.05],
            m.seat,
        );
        // A convertible shows its headrests over the doors even at LOD1.
        if self.b.convertible {
            self.soup.obox(
                madd(madd(hinge, d, 0.68), nb, 0.06),
                ax,
                [0.12, 0.085, 0.045],
                m.seat,
            );
        }
    }

    /// A front bucket at LOD0: a velour or vinyl insert between rolled bolsters, piping
    /// along the seams, pleats across the insert, a head restraint on two posts, the
    /// recliner knob, seat rails and the belt.
    fn seat(&mut self, cx: f32, s_h: f32, y_h: f32, half_w: f32) {
        let m = self.m;
        let fl = self.floor_y(s_h);
        let ax0 = self.b.axle[0];
        let z = |s: f32| s - ax0;
        let cw = half_w * 0.62;
        let bw = (half_w - cw) * 0.5;
        // Outboard: toward the nearer door.
        let so = if cx < 0.0 { -1.0f32 } else { 1.0 };
        // Base and rails.
        self.soup.aabb(
            [cx - half_w * 0.8, fl + 0.03, z(s_h - 0.38)],
            [cx + half_w * 0.8, y_h - 0.09, z(s_h)],
            m.int_trim,
        );
        for e in [-0.55f32, 0.55] {
            let x = cx + e * half_w;
            self.soup.aabb(
                [x - 0.012, fl, z(s_h - 0.55)],
                [x + 0.012, fl + 0.03, z(s_h + 0.05)],
                m.switch,
            );
        }
        // Cushion, tipped back 6°.
        let (ts, tc) = sin_cos(deg(-6.0));
        let (cu, cf) = ([0.0, tc, -ts], [0.0, ts, tc]);
        let cush_ax = [RIGHT, cu, cf];
        let cc = [cx, y_h - 0.05, z(s_h - 0.17)];
        self.pillow(cc, cush_ax, [cw, 0.045, 0.24], 0.03, m.seat_insert);
        for sx in [-1.0f32, 1.0] {
            let c = [cx + sx * (cw + bw), y_h - 0.03, z(s_h - 0.17)];
            self.pillow(c, cush_ax, [bw, 0.065, 0.245], 0.04, m.seat);
            let a = madd(madd([cx + sx * cw, cc[1], cc[2]], cu, 0.048), cf, -0.23);
            self.pipe(a, madd(a, cf, 0.46), cu, m.piping);
        }
        // The front roll's piping and pleats across the insert.
        let front = madd(madd(cc, cu, 0.04), cf, -0.24);
        self.pipe(
            madd(front, RIGHT, -half_w + 0.01),
            madd(front, RIGHT, half_w - 0.01),
            cu,
            m.piping,
        );
        for k in 0..4 {
            let c = madd(madd(cc, cu, 0.046), cf, -0.16 + k as f32 * 0.1);
            self.pipe(
                madd(c, RIGHT, -cw + 0.008),
                madd(c, RIGHT, cw - 0.008),
                cu,
                m.seat_insert.shade(0.72),
            );
        }
        // Back, reclined 18°.
        let (rs, rc) = sin_cos(deg(18.0));
        let d = [0.0, rc, rs];
        let nb = [0.0, -rs, rc];
        let hinge = [cx, y_h - 0.02, z(s_h + 0.05)];
        let ax = [RIGHT, d, nb];
        let bc = madd(madd(hinge, d, 0.3), nb, 0.05);
        self.pillow(bc, ax, [cw, 0.29, 0.05], 0.03, m.seat_insert);
        for sx in [-1.0f32, 1.0] {
            let c = madd(
                madd([cx + sx * (cw + bw), hinge[1], hinge[2]], d, 0.28),
                nb,
                0.035,
            );
            self.pillow(c, ax, [bw, 0.27, 0.07], 0.04, m.seat);
            let a = madd(madd([cx + sx * cw, bc[1], bc[2]], nb, -0.051), d, -0.27);
            self.pipe(a, madd(a, d, 0.54), nb, m.piping);
        }
        for k in 0..5 {
            let c = madd(madd(bc, nb, -0.049), d, -0.2 + k as f32 * 0.09);
            self.pipe(
                madd(c, RIGHT, -cw + 0.008),
                madd(c, RIGHT, cw - 0.008),
                nb,
                m.seat_insert.shade(0.72),
            );
        }
        // Back of the seat: a hard shell in the trim color.
        self.soup.obox(
            madd(madd(hinge, d, 0.28), nb, 0.11),
            ax,
            [half_w - 0.01, 0.27, 0.008],
            m.int_trim,
        );
        // Head restraint on two posts.
        let hc = madd(madd(hinge, d, 0.7), nb, 0.06);
        self.pillow(hc, ax, [0.12, 0.08, 0.045], 0.035, m.seat);
        self.pipe(
            madd(madd(hc, nb, -0.047), RIGHT, -0.1),
            madd(madd(hc, nb, -0.047), RIGHT, 0.1),
            nb,
            m.piping,
        );
        for e in [-0.06f32, 0.06] {
            let top = madd(madd(hc, d, -0.075), RIGHT, e);
            self.soup
                .cylinder(top, madd(top, d, -0.07), 0.006, 6, false, false, m.bezel);
        }
        // Recliner knob.
        let kc = [cx + so * half_w * 0.82, y_h - 0.1, z(s_h + 0.02)];
        self.soup.cylinder(
            kc,
            add(kc, [so * 0.02, 0.0, 0.0]),
            0.024,
            10,
            true,
            false,
            m.switch,
        );
        // Belt: stowed down the pillar on the outboard side; the buckle stalk inboard.
        let sb = s_h + 0.14;
        let xb = so * (self.inner_w(sb) - 0.012);
        let (y_top, y_bot) = (y_h + 0.62, fl + 0.12);
        self.soup.beam(
            [xb, y_top, z(sb)],
            [xb, y_bot, z(sb + 0.06)],
            [-so, 0.0, 0.0],
            0.022,
            0.002,
            m.belt,
        );
        self.soup.aabb(
            [xb - 0.012, y_top - 0.02, z(sb) - 0.025],
            [xb + 0.012, y_top + 0.03, z(sb) + 0.025],
            m.int_trim.shade(0.7),
        );
        let xi = cx - so * (half_w + 0.01);
        self.soup.beam(
            [xi, fl + 0.06, z(s_h - 0.02)],
            [xi, y_h + 0.02, z(s_h - 0.06)],
            RIGHT,
            0.012,
            0.006,
            m.belt,
        );
        self.soup.aabb(
            [xi - 0.01, y_h + 0.01, z(s_h - 0.09)],
            [xi + 0.01, y_h + 0.045, z(s_h - 0.04)],
            m.bezel,
        );
    }

    fn seats(&mut self) {
        let i = &self.b.spec.interior;
        let (s, y) = i.seat;
        let off = i.driver_offset;
        if self.lod0() {
            self.seat(-off, s, y, 0.25);
            self.seat(off, s, y, 0.25);
        } else {
            self.seat_low(-off, s, y, 0.25);
            self.seat_low(off, s, y, 0.25);
        }
        if i.rear_seat {
            self.rear_seat();
        }
    }

    fn rear_seat(&mut self) {
        let i = &self.b.spec.interior;
        let (s, y) = i.seat;
        let off = i.driver_offset;
        let sr = s + 0.9;
        let w = self.inner_w(sr) - 0.03;
        let m = self.m;
        let fl = self.floor_y(sr);
        let ax0 = self.b.axle[0];
        let z = |v: f32| v - ax0;
        let lod0 = self.lod0();
        let (rs, rc) = sin_cos(deg(16.0));
        let d = [0.0, rc, rs];
        let nb = [0.0, -rs, rc];
        let hinge = [0.0, y, z(sr + 0.04)];
        let ax = [RIGHT, d, nb];
        if !lod0 {
            self.soup
                .aabb([-w, fl, z(sr - 0.42)], [w, y, z(sr + 0.02)], m.seat);
            self.soup.obox(
                madd(madd(hinge, d, 0.27), nb, 0.05),
                ax,
                [w, 0.27, 0.06],
                m.seat,
            );
            return;
        }
        // A bench: a cushion and a back in the seat color with an insert at each place,
        // pleated and piped.
        self.soup.aabb(
            [-w, fl, z(sr - 0.4)],
            [w, y - 0.1, z(sr + 0.02)],
            m.int_trim,
        );
        let cc = [0.0, y - 0.05, z(sr - 0.2)];
        self.pillow(cc, [RIGHT, UP, BACK], [w, 0.05, 0.215], 0.035, m.seat);
        let bc = madd(madd(hinge, d, 0.27), nb, 0.05);
        self.pillow(bc, ax, [w, 0.27, 0.06], 0.035, m.seat);
        for sx in [-1.0f32, 1.0] {
            let cx = sx * off;
            let ic = [cx, y - 0.047, z(sr - 0.2)];
            self.pillow(
                ic,
                [RIGHT, UP, BACK],
                [0.17, 0.05, 0.19],
                0.03,
                m.seat_insert,
            );
            let ib = madd(madd([cx, bc[1], bc[2]], nb, -0.004), d, -0.01);
            self.pillow(ib, ax, [0.17, 0.24, 0.06], 0.03, m.seat_insert);
            for e in [-0.17f32, 0.17] {
                let a = madd(madd([cx + e, bc[1], bc[2]], nb, -0.062), d, -0.24);
                self.pipe(a, madd(a, d, 0.48), nb, m.piping);
                let a = [cx + e, y + 0.002, z(sr - 0.39)];
                self.pipe(a, add(a, [0.0, 0.0, 0.38]), UP, m.piping);
            }
            for k in 0..4 {
                let c = madd(
                    madd([cx, bc[1], bc[2]], nb, -0.063),
                    d,
                    -0.18 + k as f32 * 0.1,
                );
                self.pipe(
                    madd(c, RIGHT, -0.16),
                    madd(c, RIGHT, 0.16),
                    nb,
                    m.seat_insert.shade(0.72),
                );
                let c = [cx, y + 0.003, z(sr - 0.33 + k as f32 * 0.09)];
                self.pipe(
                    add(c, [-0.16, 0.0, 0.0]),
                    add(c, [0.16, 0.0, 0.0]),
                    UP,
                    m.seat_insert.shade(0.72),
                );
            }
        }
        if self.b.convertible {
            // In plain view under the open sky: headrests, a fold-down center armrest and
            // armrests in the quarter trim.
            for sx in [-1.0f32, 1.0] {
                let cx = sx * off;
                self.pillow(
                    madd(madd([cx, hinge[1], hinge[2]], d, 0.6), nb, 0.07),
                    ax,
                    [0.11, 0.075, 0.04],
                    0.03,
                    m.seat,
                );
                let c = [sx * (w - 0.04), y + 0.12, z(sr - 0.12)];
                self.pillow(
                    c,
                    [BACK, UP, RIGHT],
                    [0.22, 0.03, 0.04],
                    0.015,
                    m.door_card.shade(0.85),
                );
            }
            let c = madd(madd(hinge, d, 0.2), nb, 0.0);
            self.pillow(c, ax, [0.09, 0.2, 0.05], 0.03, m.seat_bolster);
        }
    }

    // ---------------------------------------------------------------- small parts

    /// Door cards: an insert panel in the seat insert with piping, an applique strip, a
    /// padded armrest, a pull handle, the release lever, a window crank, a speaker grille,
    /// a map pocket and the lock knob.
    fn doors(&mut self) {
        let b = self.b;
        let spec = b.spec;
        let i = &spec.interior;
        let m = self.m;
        let s0 = i.dash_x + 0.08;
        let s1 = b.door_seams.get(1).copied().unwrap_or(i.seat.0 + 0.3) - 0.12;
        if s1 - s0 < 0.2 {
            return;
        }
        let y_arm = b.frame(s0).y_ledge - 0.2;
        let y_belt = b.frame((s0 + s1) * 0.5).y_ledge;
        let fl = self.floor_y(s0);
        for sx in [-1.0f32, 1.0] {
            // Insert between the armrest and the belt, bordered by piping.
            let (ya, yb) = (y_arm + 0.045, y_belt - 0.07);
            if yb - ya > 0.04 {
                self.card_panel(sx, s0 + 0.1, s1 - 0.04, ya, yb, 0.002, m.seat_insert);
                for y in [ya, yb] {
                    let p = |s: f32| [sx * (self.card_x(s, y) - 0.003), y, self.z(s)];
                    self.pipe(p(s0 + 0.1), p(s1 - 0.04), [-sx, 0.0, 0.0], m.piping);
                }
            }
            // The applique strip under the belt.
            let y = y_belt - 0.05;
            self.card_panel(sx, s0 + 0.04, s1, y - 0.012, y + 0.012, 0.003, m.applique);
            // Armrest.
            let len = (s1 - s0 - 0.2) * 0.5;
            let sc = (s0 + 0.15 + s1 - 0.05) * 0.5;
            let x = sx * (self.card_x(sc, y_arm) - 0.035);
            let ac = [x, y_arm, self.z(sc)];
            self.pillow(
                ac,
                [BACK, UP, RIGHT],
                [len, 0.022, 0.035],
                0.016,
                m.door_card.shade(0.8),
            );
            self.pipe(
                [x - sx * 0.035, y_arm + 0.02, self.z(sc - len)],
                [x - sx * 0.035, y_arm + 0.02, self.z(sc + len)],
                UP,
                m.piping,
            );
            // Pull handle: a bright bar over a dark cup, ahead of the armrest.
            let hs = s0 + 0.12;
            let hx = sx * (self.card_x(hs, y_arm + 0.09) - 0.004);
            self.soup.aabb(
                [hx - sx * 0.006, y_arm + 0.075, self.z(hs - 0.06)],
                [hx, y_arm + 0.105, self.z(hs + 0.06)],
                m.switch,
            );
            self.soup.beam(
                [hx - sx * 0.022, y_arm + 0.09, self.z(hs - 0.055)],
                [hx - sx * 0.022, y_arm + 0.09, self.z(hs + 0.055)],
                UP,
                0.006,
                0.006,
                m.bezel,
            );
            for e in [-0.05f32, 0.05] {
                self.soup.beam(
                    [hx, y_arm + 0.09, self.z(hs + e)],
                    [hx - sx * 0.022, y_arm + 0.09, self.z(hs + e)],
                    UP,
                    0.005,
                    0.005,
                    m.bezel,
                );
            }
            // Release lever in a recess near the front, over the handle.
            let ls = s0 + 0.08;
            let lx = sx * (self.card_x(ls, y_arm + 0.16) - 0.003);
            self.soup.aabb(
                [lx - sx * 0.004, y_arm + 0.14, self.z(ls - 0.04)],
                [lx, y_arm + 0.18, self.z(ls + 0.04)],
                m.switch,
            );
            self.soup.aabb(
                [lx - sx * 0.012, y_arm + 0.152, self.z(ls - 0.025)],
                [lx - sx * 0.004, y_arm + 0.168, self.z(ls + 0.03)],
                m.bezel,
            );
            // Window crank: a hub, an arm and a knob, behind the armrest.
            let cs = (s1 - 0.12).max(sc);
            let cy = y_arm - 0.07;
            let cxp = sx * (self.card_x(cs, cy) - 0.004);
            let hub = [cxp, cy, self.z(cs)];
            let inw = [-sx, 0.0, 0.0];
            self.soup
                .cylinder(hub, madd(hub, inw, 0.018), 0.014, 10, true, false, m.bezel);
            let arm_end = add(madd(hub, inw, 0.02), [0.0, -0.045, 0.03]);
            self.soup
                .beam(madd(hub, inw, 0.018), arm_end, inw, 0.006, 0.003, m.bezel);
            self.soup.cylinder(
                arm_end,
                madd(arm_end, inw, 0.03),
                0.009,
                8,
                true,
                false,
                m.switch,
            );
            // Speaker grille low at the front: a bright ring and grille rings.
            let c = [
                sx * (self.card_x(s0 + 0.12, fl + 0.14) - 0.005),
                fl + 0.14,
                self.z(s0 + 0.12),
            ];
            self.soup.disc(c, inw, 0.06, 16, m.vent);
            for (r0, r1) in [(0.02, 0.026), (0.038, 0.044)] {
                self.soup
                    .annulus(madd(c, inw, 0.001), inw, r0, r1, 16, m.vent.shade(1.8));
            }
            self.soup.annulus(
                madd(c, inw, 0.002),
                inw,
                0.06,
                0.068,
                16,
                m.bezel.shade(0.8),
            );
            // Map pocket along the bottom behind the speaker.
            let (ps0, ps1) = (s0 + 0.2, (s0 + 0.6).min(s1 - 0.05));
            if ps1 > ps0 + 0.1 {
                let px = sx * (self.card_x((ps0 + ps1) * 0.5, fl + 0.1) - 0.02);
                self.soup.aabb(
                    [px - 0.012, fl + 0.05, self.z(ps0)],
                    [px + 0.012, fl + 0.17, self.z(ps1)],
                    m.door_card.shade(0.78),
                );
            }
            // Lock knob on the sill.
            let ks = s1 - 0.06;
            let kx = sx * (b.frame(ks).z_ledge - 0.08);
            let ky = b.frame(ks).y_ledge - 0.012;
            self.soup.cylinder(
                [kx, ky, self.z(ks)],
                [kx, ky + 0.03, self.z(ks)],
                0.005,
                6,
                true,
                false,
                m.bezel,
            );
        }
    }

    fn mirror_and_visors(&mut self) {
        let b = self.b;
        let spec = b.spec;
        let i = &spec.interior;
        let m = self.m;
        let (mx, my) = i.mirror_pos;
        let z = self.z(mx);
        self.pillow(
            [0.0, my, z - 0.003],
            [RIGHT, UP, BACK],
            [0.12, 0.034, 0.015],
            0.012,
            m.int_trim,
        );
        self.soup.quad_dir(
            [-0.112, my - 0.027, z + 0.0125],
            [0.112, my - 0.027, z + 0.0125],
            [0.112, my + 0.027, z + 0.0125],
            [-0.112, my + 0.027, z + 0.0125],
            m.mirror,
            BACK,
        );
        // Day/night tab.
        self.soup.aabb(
            [-0.012, my - 0.046, z - 0.004],
            [0.012, my - 0.034, z + 0.008],
            m.switch,
        );
        let top = b.frame(mx - 0.02).yt - 0.02;
        self.soup.beam(
            [0.0, my + 0.03, z - 0.005],
            [0.0, top, z - 0.03],
            BACK,
            0.008,
            0.006,
            m.int_trim,
        );
        if b.convertible || b.targa.is_some() {
            self.header_visors();
            return;
        }
        // Sun visors folded up under the headliner, a vanity mirror under the passenger's,
        // hinge rods, a dome lamp between them and a grab handle over the passenger door.
        let s = spec.glass.windshield.1 + 0.07;
        let f = b.frame(s);
        let y = f.yt - 0.05;
        for sx in [-1.0f32, 1.0] {
            let cx = sx * i.driver_offset;
            let hx = (0.17f32).min(f.z_re * 0.5);
            self.pillow(
                [cx, y - 0.002, self.z(s)],
                [RIGHT, UP, BACK],
                [hx, 0.01, 0.085],
                0.008,
                m.headliner.shade(0.9),
            );
            if sx > 0.0 {
                self.soup.quad_dir(
                    [cx - hx * 0.5, y - 0.0125, self.z(s - 0.04)],
                    [cx + hx * 0.5, y - 0.0125, self.z(s - 0.04)],
                    [cx + hx * 0.5, y - 0.0125, self.z(s + 0.04)],
                    [cx - hx * 0.5, y - 0.0125, self.z(s + 0.04)],
                    m.mirror,
                    [0.0, -1.0, 0.0],
                );
            }
            let hinge = [cx + sx * (hx + 0.012), y + 0.004, self.z(s - 0.06)];
            self.soup.cylinder(
                hinge,
                add(hinge, [-sx * 0.03, 0.0, 0.0]),
                0.006,
                6,
                true,
                false,
                m.int_trim,
            );
        }
        let dz = self.z(s + 0.22);
        let dy = b.frame(s + 0.22).yt - 0.035;
        self.soup.aabb(
            [-0.06, dy - 0.012, dz - 0.04],
            [0.06, dy + 0.004, dz + 0.04],
            m.int_trim,
        );
        self.soup.aabb(
            [-0.05, dy - 0.014, dz - 0.03],
            [0.05, dy - 0.012, dz + 0.03],
            m.reverse,
        );
        let gs = spec.interior.seat.0;
        let gf = b.frame(gs);
        if gf.h > 0.15 {
            let (p, _) = b.ring_plain(gs);
            let rr = p[b.idx(Slot::RoofRail)];
            let x = rr[0] - 0.06;
            let y = rr[1] - 0.05;
            let a = [x, y, self.z(gs - 0.12)];
            let c = [x, y, self.z(gs + 0.12)];
            let mid = [x - 0.03, y - 0.03, self.z(gs)];
            self.soup.beam(
                a,
                add(mid, [0.0, 0.0, -0.06]),
                RIGHT,
                0.008,
                0.008,
                m.int_trim,
            );
            self.soup.beam(
                add(mid, [0.0, 0.0, -0.06]),
                add(mid, [0.0, 0.0, 0.06]),
                RIGHT,
                0.008,
                0.008,
                m.int_trim,
            );
            self.soup.beam(
                add(mid, [0.0, 0.0, 0.06]),
                c,
                RIGHT,
                0.008,
                0.008,
                m.int_trim,
            );
        }
    }

    /// An open roof's sun visors (a convertible's, a targa's): short, folded flat against the windshield under the header
    /// so they keep out of the open view.
    fn header_visors(&mut self) {
        let b = self.b;
        let spec = b.spec;
        let w1 = spec.glass.windshield.1;
        let (s_top, s_low) = (w1 - 0.03, w1 - 0.2);
        let (ft, fl) = (b.frame(s_top), b.frame(s_low));
        // Along the glass (down and forward) and its inward normal.
        let along = norm([0.0, fl.yt - ft.yt, s_low - s_top]);
        let inward = norm(cross(along, RIGHT));
        let inward = if inward[1] < 0.0 {
            inward
        } else {
            scale(inward, -1.0)
        };
        for sx in [-1.0f32, 1.0] {
            let cx = sx * spec.interior.driver_offset;
            let hx = (0.17f32).min(ft.z_re * 0.5);
            let c = [cx, ft.yt, self.z(s_top)];
            let c = madd(madd(c, along, 0.045), inward, 0.03);
            self.pillow(
                c,
                [RIGHT, along, inward],
                [hx * 0.8, 0.042, 0.008],
                0.006,
                self.m.headliner.shade(0.9),
            );
        }
    }

    /// Pedals on arms from under the dash, with ribbed rubber pads; a clutch with a manual
    /// gearbox, and a dead pedal for the left foot.
    fn pedals(&mut self) {
        let i = &self.b.spec.interior;
        let m = self.m;
        let xd = -i.driver_offset;
        let s = i.dash_x - 0.42;
        let fl = self.floor_y(s);
        let mut set: Vec<(f32, f32)> = alloc::vec![(0.08, 0.025), (-0.03, 0.045)];
        if i.manual {
            set.push((-0.13, 0.035));
        }
        let pad_y = fl + 0.1;
        for &(x, w) in &set {
            let x = xd + x;
            let z0 = self.z(s);
            self.soup.aabb(
                [x - w, pad_y - 0.035, z0 - 0.01],
                [x + w, pad_y + 0.035, z0 + 0.008],
                m.switch,
            );
            for k in 0..4 {
                let y = pad_y - 0.026 + k as f32 * 0.017;
                self.soup.aabb(
                    [x - w + 0.004, y - 0.002, z0 + 0.008],
                    [x + w - 0.004, y + 0.002, z0 + 0.011],
                    m.rubber,
                );
            }
            if x < xd + 0.07 {
                // Suspended pedals hang on arms; the throttle stands on the floor.
                self.soup.beam(
                    [x, pad_y + 0.03, z0 - 0.004],
                    [x, i.dash_height - 0.3, self.z(s - 0.1)],
                    BACK,
                    0.006,
                    0.01,
                    m.steel,
                );
            }
        }
        let rest = [xd - 0.22, fl + 0.06, self.z(s + 0.02)];
        self.soup.obox(
            rest,
            [RIGHT, norm([0.0, 0.6, 0.8]), norm([0.0, -0.8, 0.6])],
            [0.035, 0.07, 0.006],
            m.switch,
        );
    }
}
