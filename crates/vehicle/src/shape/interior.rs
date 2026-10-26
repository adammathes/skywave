//! The cabin: inner trim shell (door cards, pillars, headliner), dash with a hooded
//! cluster and a center stack, steering wheel, column and stalks, seats, mirror, visors.
//! Left-hand drive: the driver sits at `x = -driver_offset`.

use super::body::{Body, Slot};
use super::math::*;
use super::mesh::{Mat, Soup};
use super::{Cockpit, Dial, Materials};
use alloc::vec::Vec;

const UP: V3 = [0.0, 1.0, 0.0];
const BACK: V3 = [0.0, 0.0, 1.0];
const RIGHT: V3 = [1.0, 0.0, 0.0];

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
            self.doors();
            self.mirror_and_visors();
            self.pedals();
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
    /// keeps only the door cards: no pillars or headliner over the open cabin.
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
            for j in 0..6 {
                let mat: Mat = match j {
                    _ if open && j >= 2 => continue,
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
        }
    }

    // ---------------------------------------------------------------- dash

    fn dash(&mut self) {
        let b = self.b;
        let spec = b.spec;
        let i = &spec.interior;
        let m = self.m;
        let ws = spec.glass.windshield.0 + 0.03;
        let belt = b.frame(ws).y_ledge;
        let dh = i.dash_height;
        let dx = i.dash_x;
        let s_fw = dx - 0.55;
        let w = self.inner_w(dx) + 0.02;
        // Closed profile in (s, y), extruded across the car.
        let prof_sy = [
            (ws, belt - 0.004),
            (dx - 0.12, dh),
            (dx - 0.01, dh - 0.008),
            (dx + 0.015, dh - 0.05),
            (dx - 0.01, dh - 0.24),
            (dx - 0.3, dh - 0.3),
            (s_fw, dh - 0.22),
            (s_fw + 0.02, belt - 0.06),
        ];
        let (rs, ry) = (dx - 0.2, dh - 0.1);
        let prof: Vec<(f32, f32)> = prof_sy.iter().map(|&(s, y)| (s - rs, y - ry)).collect();
        let path = [[-w, ry, self.z(rs)], [w, ry, self.z(rs)]];
        self.soup
            .sweep(&path, &[BACK, BACK], &[UP, UP], &prof, true, m.dash);

        // Hooded cluster in front of the driver.
        let xd = -i.driver_offset;
        let n_dials = i.dials as usize;
        let hw = 0.19f32;
        // The face stands just behind the dash's rear lip (dx + 0.015), under the hood: set
        // into the dash (dx - 0.10) its lower half was below the dash top as the driver sees it.
        let zc = self.z(dx + 0.025);
        let tilt = norm([0.0, 0.22, 1.0]);
        self.soup.aabb(
            [xd - hw, dh + 0.06, self.z(dx - 0.17)],
            [xd + hw, dh + 0.075, self.z(dx + 0.035)],
            m.dash,
        );
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
        let r = (0.36 / (2.0 * n_dials as f32) - 0.012).min(0.068);
        let segs = if self.lod0() { 16 } else { 8 };
        let mut dials = [Dial::default(); 3];
        for (k, dial) in dials.iter_mut().enumerate().take(n_dials) {
            let off = (k as f32 - (n_dials as f32 - 1.0) * 0.5) * (2.0 * r + 0.024);
            let c = madd(madd(face_c, fu, off), tilt, 0.004);
            self.soup.disc(c, tilt, r, segs, m.dial);
            if self.lod0() {
                self.soup.annulus(
                    madd(c, tilt, 0.002),
                    tilt,
                    r,
                    r + 0.008,
                    segs,
                    m.chrome.shade(0.7),
                );
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
        // Square 80s vents at the dash ends.
        if self.lod0() {
            for sx in [-1.0f32, 1.0] {
                let x = sx * (w - 0.12);
                let zf = self.z(dx + 0.016);
                self.soup.aabb(
                    [x - 0.05, dh - 0.11, zf - 0.01],
                    [x + 0.05, dh - 0.05, zf],
                    m.vent,
                );
                for k in 0..3 {
                    let y = dh - 0.095 + k as f32 * 0.016;
                    self.soup.aabb(
                        [x - 0.045, y - 0.002, zf - 0.006],
                        [x + 0.045, y + 0.002, zf + 0.003],
                        m.int_trim,
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
        // Center vents.
        for sx in [-1.0f32, 1.0] {
            let x = sx * 0.06;
            self.soup.aabb(
                [x - 0.045, dh - 0.11, face - 0.01],
                [x + 0.045, dh - 0.06, face + 0.003],
                m.vent,
            );
            for k in 0..3 {
                let y = dh - 0.098 + k as f32 * 0.014;
                self.soup.aabb(
                    [x - 0.042, y - 0.002, face],
                    [x + 0.042, y + 0.002, face + 0.006],
                    m.int_trim,
                );
            }
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
            let x = -0.06 + k as f32 * 0.024;
            self.soup.aabb(
                [x, ry + 0.005, dz - 0.004],
                [x + 0.016, ry + 0.012, dz + 0.004],
                m.int_trim.shade(1.4),
            );
        }
        for x in [-0.077f32, 0.06] {
            self.soup.cylinder(
                [x, ry + 0.028, dz - 0.002],
                [x, ry + 0.028, dz + 0.012],
                0.011,
                8,
                true,
                false,
                m.chrome.shade(0.6),
            );
        }
        // Climate sliders.
        let cy = ry - 0.07;
        self.soup.aabb(
            [-0.1, cy, face - 0.005],
            [0.1, cy + 0.05, face + 0.006],
            m.int_trim,
        );
        for k in 0..3 {
            let x = -0.06 + k as f32 * 0.06;
            self.soup.aabb(
                [x - 0.006, cy + 0.015, face + 0.006],
                [x + 0.006, cy + 0.035, face + 0.016],
                m.accent_int,
            );
        }
        // Console and shifter.
        let s_end = i.seat.0 + 0.05;
        self.soup.aabb(
            [-0.1, fl, face - 0.02],
            [0.1, fl + 0.22, self.z(s_end)],
            m.dash.shade(0.85),
        );
        let sz = self.z(dx + 0.38);
        self.soup.beam(
            [0.0, fl + 0.22, sz],
            [0.0, fl + 0.36, sz - 0.03],
            BACK,
            0.009,
            0.009,
            m.chrome.shade(0.6),
        );
        self.soup.aabb(
            [-0.022, fl + 0.35, sz - 0.05],
            [0.022, fl + 0.4, sz - 0.01],
            m.int_trim,
        );
        self.soup.aabb(
            [-0.05, fl + 0.22, sz - 0.07],
            [0.05, fl + 0.23, sz + 0.07],
            m.int_trim,
        );
    }

    // ---------------------------------------------------------------- wheel

    fn steering(&mut self) {
        let spec = self.b.spec;
        let i = &spec.interior;
        let m = self.m;
        let (sn, cs) = sin_cos(deg(i.column_angle_deg));
        let axis = [0.0, sn, cs];
        let p = [-i.driver_offset, i.wheel_pos.1, self.z(i.wheel_pos.0)];
        self.cockpit.wheel_pivot = p;
        self.cockpit.wheel_axis = axis;
        let r = i.wheel_radius;
        let up = norm(sub(UP, scale(axis, dot(UP, axis))));
        let right = norm(cross(up, axis));
        let (segs, tube) = if self.lod0() { (22, 6) } else { (10, 4) };
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
        }
        let hub_r = if two { 0.075 } else { 0.055 };
        self.wheel.cylinder(
            madd(p, axis, -0.07),
            madd(p, axis, -0.02),
            hub_r,
            if self.lod0() { 12 } else { 6 },
            true,
            false,
            m.wheel_spoke,
        );
        if self.lod0() {
            self.wheel.obox(
                madd(p, axis, -0.012),
                [right, up, axis],
                [hub_r * 0.95, hub_r * 0.7, 0.012],
                m.wheel_rim,
            );
        }
        // Column, shroud and stalks (fixed).
        self.soup.cylinder(
            madd(p, axis, -0.07),
            madd(p, axis, -0.42),
            0.03,
            8,
            false,
            false,
            m.int_trim,
        );
        self.soup.obox(
            madd(p, axis, -0.12),
            [right, up, axis],
            [0.06, 0.05, 0.05],
            m.int_trim,
        );
        if self.lod0() {
            for sx in [-1.0f32, 1.0] {
                let a = madd(madd(p, axis, -0.11), right, sx * 0.05);
                let bb = madd(madd(a, right, sx * 0.12), up, -0.02);
                self.soup
                    .beam(a, bb, up, 0.006, 0.006, m.int_trim.shade(0.7));
            }
        }
    }

    // ---------------------------------------------------------------- seats

    fn seat(&mut self, cx: f32, s_h: f32, y_h: f32, half_w: f32) {
        let m = self.m;
        let fl = self.floor_y(s_h);
        let ax0 = self.b.axle[0];
        let z = |s: f32| s - ax0;
        let cw = half_w * 0.62;
        let lod0 = self.lod0();
        // Base and cushion.
        self.soup.aabb(
            [cx - half_w * 0.8, fl, z(s_h - 0.38)],
            [cx + half_w * 0.8, y_h - 0.09, z(s_h)],
            m.int_trim,
        );
        let (ts, tc) = sin_cos(deg(-6.0));
        let cush_ax = [RIGHT, [0.0, tc, -ts], [0.0, ts, tc]];
        let cc = [cx, y_h - 0.05, z(s_h - 0.17)];
        self.soup.obox(cc, cush_ax, [cw, 0.045, 0.24], m.seat);
        if lod0 {
            for sx in [-1.0f32, 1.0] {
                let c = [
                    cx + sx * (cw + (half_w - cw) * 0.5),
                    y_h - 0.03,
                    z(s_h - 0.17),
                ];
                self.soup.obox(
                    c,
                    cush_ax,
                    [(half_w - cw) * 0.5, 0.065, 0.24],
                    m.seat_bolster,
                );
            }
        }
        // Back, reclined 18°.
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
        if lod0 {
            for sx in [-1.0f32, 1.0] {
                let c = madd(
                    madd(
                        [cx + sx * (cw + (half_w - cw) * 0.5), hinge[1], hinge[2]],
                        d,
                        0.28,
                    ),
                    nb,
                    0.035,
                );
                self.soup
                    .obox(c, ax, [(half_w - cw) * 0.5, 0.27, 0.07], m.seat_bolster);
            }
        }
        // A convertible shows its headrests over the doors even at LOD1.
        if lod0 || self.b.convertible {
            self.soup.obox(
                madd(madd(hinge, d, 0.68), nb, 0.06),
                ax,
                [0.12, 0.085, 0.045],
                m.seat,
            );
        }
    }

    fn seats(&mut self) {
        let i = &self.b.spec.interior;
        let (s, y) = i.seat;
        let off = i.driver_offset;
        self.seat(-off, s, y, 0.25);
        self.seat(off, s, y, 0.25);
        if i.rear_seat {
            let sr = s + 0.9;
            let w = self.inner_w(sr) - 0.03;
            let m = self.m;
            let fl = self.floor_y(sr);
            let ax0 = self.b.axle[0];
            let z = |v: f32| v - ax0;
            self.soup
                .aabb([-w, fl, z(sr - 0.42)], [w, y - 0.0, z(sr + 0.02)], m.seat);
            let (rs, rc) = sin_cos(deg(16.0));
            let d = [0.0, rc, rs];
            let nb = [0.0, -rs, rc];
            let hinge = [0.0, y, z(sr + 0.04)];
            self.soup.obox(
                madd(madd(hinge, d, 0.27), nb, 0.05),
                [RIGHT, d, nb],
                [w, 0.27, 0.06],
                m.seat,
            );
            if self.b.convertible && self.lod0() {
                // In plain view under the open sky: two pleated places with headrests,
                // a fold-down center armrest and armrests in the quarter trim.
                let ax = [RIGHT, d, nb];
                for sx in [-1.0f32, 1.0] {
                    let cx = sx * off;
                    self.soup.obox(
                        madd(madd([cx, hinge[1], hinge[2]], d, 0.6), nb, 0.07),
                        ax,
                        [0.11, 0.075, 0.04],
                        m.seat,
                    );
                    for k in [-1.0f32, 1.0] {
                        let c = [cx + k * 0.12, y + 0.01, z(sr - 0.2)];
                        self.soup.aabb(
                            sub(c, [0.012, 0.03, 0.2]),
                            add(c, [0.012, 0.03, 0.2]),
                            m.seat_bolster,
                        );
                    }
                    let c = [sx * (w - 0.04), y + 0.12, z(sr - 0.12)];
                    self.soup.aabb(
                        sub(c, [0.04, 0.03, 0.22]),
                        add(c, [0.04, 0.03, 0.22]),
                        m.door_card.shade(0.85),
                    );
                }
                let c = madd(madd(hinge, d, 0.2), nb, 0.015);
                self.soup.obox(c, ax, [0.09, 0.2, 0.05], m.seat_bolster);
            }
        }
    }

    // ---------------------------------------------------------------- small parts

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
        for sx in [-1.0f32, 1.0] {
            let w = self.inner_w((s0 + s1) * 0.5);
            let y = b.frame(s0).y_ledge - 0.2;
            let x = sx * (w - 0.02);
            self.soup.aabb(
                [x - 0.045 * sx, y - 0.025, self.z(s0 + 0.15)],
                [x + 0.01 * sx, y + 0.02, self.z(s1 - 0.05)],
                m.door_card.shade(0.8),
            );
            // Pull handle and speaker.
            self.soup.aabb(
                [x - 0.03 * sx, y + 0.08, self.z(s0 + 0.12)],
                [x - 0.0, y + 0.1, self.z(s0 + 0.24)],
                m.chrome.shade(0.6),
            );
            let c = [sx * (w - 0.005), self.floor_y(s0) + 0.14, self.z(s0 + 0.12)];
            self.soup.disc(c, [-sx, 0.0, 0.0], 0.06, 10, m.vent);
        }
    }

    fn mirror_and_visors(&mut self) {
        let b = self.b;
        let spec = b.spec;
        let i = &spec.interior;
        let m = self.m;
        let (mx, my) = i.mirror_pos;
        let z = self.z(mx);
        self.soup.aabb(
            [-0.12, my - 0.034, z - 0.018],
            [0.12, my + 0.034, z + 0.012],
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
        let top = b.frame(mx - 0.02).yt - 0.02;
        self.soup.beam(
            [0.0, my + 0.03, z - 0.005],
            [0.0, top, z - 0.03],
            BACK,
            0.008,
            0.006,
            m.int_trim,
        );
        if b.convertible {
            self.header_visors();
            return;
        }
        // Sun visors folded up under the headliner.
        let s = spec.glass.windshield.1 + 0.07;
        let f = b.frame(s);
        let y = f.yt - 0.05;
        for sx in [-1.0f32, 1.0] {
            let cx = sx * i.driver_offset;
            let hx = (0.17f32).min(f.z_re * 0.5);
            self.soup.aabb(
                [cx - hx, y - 0.012, self.z(s - 0.09)],
                [cx + hx, y + 0.008, self.z(s + 0.08)],
                m.headliner.shade(0.9),
            );
        }
    }

    /// A convertible's sun visors: short, folded flat against the windshield under the header
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
            self.soup.obox(
                c,
                [RIGHT, along, inward],
                [hx * 0.8, 0.042, 0.008],
                self.m.headliner.shade(0.9),
            );
        }
    }

    fn pedals(&mut self) {
        let i = &self.b.spec.interior;
        let xd = -i.driver_offset;
        let s = i.dash_x - 0.42;
        let fl = self.floor_y(s);
        for (k, x) in [0.07f32, -0.03].iter().enumerate() {
            let w = if k == 1 { 0.05 } else { 0.03 };
            self.soup.aabb(
                [xd + x - w, fl + 0.07, self.z(s) - 0.01],
                [xd + x + w, fl + 0.14, self.z(s) + 0.008],
                self.m.int_trim.shade(0.6),
            );
        }
    }
}
