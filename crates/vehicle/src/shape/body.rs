//! The lofted body shell: stations along the car, a ring per station, arches, seams, glass.
//!
//! Each station `s` (m from the nose) gets a half ring of `(z, y)` points (`z` lateral,
//! right side) from the bottom centerline round the side and over the top to the top
//! centerline. Every ring has the same slots, so neighboring rings connect into quads. A
//! ring is a lower body (floor, rocker, side, shoulder, belt ledge) plus a greenhouse (side
//! glass leaning in by the tumblehome, roof rail, roof) whose height is the side
//! silhouette above the belt: where that is zero (hood, deck) the greenhouse lies flat and
//! its top is the hood. Wheel arches lift ring points above the arch circle; seams are
//! V-grooves made by extra stations (door gaps, hood and trunk edges) or extra ring slots
//! (hood/trunk sides, creases).

use super::math::*;
use super::mesh::{Mat, Soup};
use super::params::{Detail, ShapeSpec};
use super::Materials;
use alloc::vec::Vec;

/// Half-width of a seam groove, m.
pub const SEAM_HALF: f32 = 0.007;
/// Seam depth, m.
pub const SEAM_DEPTH: f32 = 0.012;
/// How far the hood and trunk feature lines stand up, m.
const LINE_RISE: f32 = 0.016;
/// Width of the windshield frame on the top surface, m.
const FRAME_W: f32 = 0.055;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Slot {
    FloorC,
    FloorIn,
    RockerB,
    SideLow,
    /// Crease `k`, part -1 (lower edge), 0 (groove), 1 (upper edge).
    Crease(u8, i8),
    ShoulderLow,
    ShoulderMid,
    Ledge,
    GlassB,
    RoofRail,
    RoofEdge,
    Frame,
    /// Hood/trunk side seam, part 1 (outer edge), 0 (groove), -1 (inner edge).
    TopSeam(i8),
    /// A raised feature line along the hood and the trunk lid (LOD0 with top seams).
    HoodLine,
    Crown,
    Center,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SeamKind {
    Door,
    HoodRear,
    TrunkFront,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Station {
    pub s: f32,
    /// Arch clamp for axle 0 (front) or 1 (rear).
    pub clamp: Option<u8>,
    pub seam: Option<SeamKind>,
    /// Sort tiebreak for doubled stations at an arch edge.
    order: u8,
}

/// Scalars of the cross-section at one station.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Frame {
    pub yt: f32,
    pub yb: f32,
    pub w: f32,
    pub y_ledge: f32,
    pub y_edge: f32,
    pub z_ledge: f32,
    /// Roof (or hood) edge half-width.
    pub z_re: f32,
    pub crown: f32,
    /// Greenhouse height above the belt.
    pub h: f32,
}

impl Frame {
    /// Height of the top surface at lateral `z`.
    pub fn top_y(&self, z: f32) -> f32 {
        let t = clamp(z / self.z_re, 0.0, 1.0);
        self.yt - self.crown * t * t
    }
}

pub struct Ring {
    pub pts: Vec<[f32; 2]>,
    /// Each point's height before the arches lifted it.
    pub plain_y: Vec<f32>,
    pub raised: Vec<bool>,
    pub displaced: Vec<bool>,
    pub f: Frame,
}

pub struct Body<'a> {
    pub spec: &'a ShapeSpec,
    pub top: Vec<(f32, f32)>,
    pub bot: Vec<(f32, f32)>,
    pub length: f32,
    pub lod: u8,
    pub slots: Vec<Slot>,
    /// Ring points outboard of this lateral position are lifted over the arches.
    pub z_in: f32,
    tan_tumble: f32,
    /// Axle positions along the car.
    pub axle: [f32; 2],
    /// Arch radius and wheel center height.
    pub ra: f32,
    pub wy: f32,
    /// Hood and trunk lid ranges along the car, when they have seams.
    pub hood: Option<(f32, f32)>,
    pub trunk: Option<(f32, f32)>,
    /// Trunk seams drawn on the tail as a tailgate outline instead.
    pub tailgate: bool,
    pub door_seams: Vec<f32>,
    pub stations: Vec<Station>,
    /// `spec.details` plus the palette entry's `color_details`.
    pub details: Vec<Detail>,
    /// A convertible: the cabin is open above the belt from the windshield header to the
    /// boot, and the windshield range has no side faces (the frame is a part).
    pub convertible: bool,
    /// A pickup's open bed: the top is left out over this range (`bed.rs` lines it).
    pub bed: Option<(f32, f32)>,
}

impl<'a> Body<'a> {
    /// The shell of `spec` at level of detail `lod` in palette entry `color` (which picks
    /// the `color_details`).
    pub fn new(spec: &'a ShapeSpec, lod: u8, color: usize) -> Body<'a> {
        let tail = spec.tail_index();
        let top: Vec<(f32, f32)> = spec.side[..=tail].to_vec();
        let mut bot: Vec<(f32, f32)> = spec.side[tail + 1..].to_vec();
        bot.reverse();
        let length = top[top.len() - 1].0;
        let wh = &spec.wheels;
        let seams = lod == 0;

        let mut slots = alloc::vec![Slot::FloorC, Slot::FloorIn, Slot::RockerB, Slot::SideLow];
        if seams {
            for k in 0..spec.seams.creases.len() as u8 {
                slots.extend([Slot::Crease(k, -1), Slot::Crease(k, 0), Slot::Crease(k, 1)]);
            }
        }
        slots.push(Slot::ShoulderLow);
        if spec.section.shoulder < 0.92 || lod < 2 {
            slots.push(Slot::ShoulderMid);
        }
        slots.extend([
            Slot::Ledge,
            Slot::GlassB,
            Slot::RoofRail,
            Slot::RoofEdge,
            Slot::Frame,
        ]);
        let top_seam = seams && (spec.seams.hood || spec.seams.trunk);
        if top_seam {
            slots.extend([
                Slot::TopSeam(1),
                Slot::TopSeam(0),
                Slot::TopSeam(-1),
                Slot::HoodLine,
            ]);
        }
        slots.extend([Slot::Crown, Slot::Center]);

        let g = &spec.glass;
        let hood = (seams && spec.seams.hood).then_some((0.05, g.windshield.0 - 0.05));
        let deck = (g.backlight.1 + 0.05, length - 0.05);
        let deck_ok = deck.1 - deck.0 >= 0.25;
        // A pickup's deck is its open bed: the trunk seams outline the tailgate instead.
        let bed = spec.bed();
        let trunk = (seams && spec.seams.trunk && deck_ok && bed.is_none()).then_some(deck);
        let tailgate = seams && spec.seams.trunk && (!deck_ok || bed.is_some());

        let ra = wh.radius + wh.arch_clearance;
        let n_colors = spec.palette.len().max(1);
        let mut details = spec.details.clone();
        for &(k, d) in &spec.color_details {
            if k as usize == color % n_colors && !details.contains(&d) {
                details.push(d);
            }
        }
        let mut body = Body {
            spec,
            top,
            bot,
            length,
            lod,
            slots,
            z_in: wh.track * 0.5 - wh.width * 0.5 - 0.03,
            tan_tumble: tan(deg(spec.section.tumblehome_deg)),
            axle: [wh.front_x, wh.rear_x],
            ra,
            wy: wh.radius,
            hood,
            trunk,
            tailgate,
            door_seams: Vec::new(),
            stations: Vec::new(),
            details,
            convertible: spec.roof.convertible(),
            bed,
        };
        if seams {
            body.door_seams = body.door_lines();
        }
        body.stations = body.make_stations();
        body
    }

    pub fn idx(&self, slot: Slot) -> usize {
        self.slots.iter().position(|&s| s == slot).unwrap_or(0)
    }

    pub fn has(&self, d: Detail) -> bool {
        self.details.contains(&d)
    }

    /// Whether ring segment `j → j+1` between stations `a` and `b` is left out because the
    /// car is a convertible: everything above the belt over the open cabin, and the side
    /// faces (where the door glass would be) beside the windshield.
    fn open_cut(&self, j: usize, a: f32, b: f32) -> bool {
        let within = |r: (f32, f32)| a >= r.0 - 1e-4 && b <= r.1 + 1e-4;
        let i_ledge = self.idx(Slot::Ledge);
        if self.bed.is_some_and(within) {
            return j >= i_ledge;
        }
        if !self.convertible {
            return false;
        }
        let g = &self.spec.glass;
        if within((g.windshield.1, g.backlight.0)) {
            return j >= i_ledge;
        }
        within(g.windshield) && (j == i_ledge || j == self.idx(Slot::GlassB))
    }

    /// Model-space point for station `s` and ring point `(z, y)`: x right, y up, z rearward
    /// from the front axle.
    pub fn p(&self, s: f32, z: f32, y: f32) -> V3 {
        [z, y, s - self.axle[0]]
    }

    /// Door gap positions along the car.
    fn door_lines(&self) -> Vec<f32> {
        let g = &self.spec.glass;
        let front_arch_end = self.axle[0] + self.ra;
        let mut v = Vec::new();
        v.push((g.windshield.0 + 0.04).max(front_arch_end + 0.06));
        let side = &g.side;
        if self.spec.seams.doors == 2 {
            let rear = side
                .first()
                .map_or(self.spec.interior.seat.0 + 0.45, |w| w.1 + 0.03);
            v.push(rear);
        } else {
            let b = match side.len() {
                0 => self.spec.interior.seat.0 + 0.2,
                1 => side[0].1 - 0.4,
                _ => (side[0].1 + side[1].0) * 0.5,
            };
            v.push(b);
            let c = match side.len() {
                0 | 1 => b + 0.9,
                2 => side[1].1 + 0.03,
                _ => (side[1].1 + side[2].0) * 0.5,
            };
            v.push(c);
        }
        v
    }

    fn make_stations(&self) -> Vec<Station> {
        let spec = self.spec;
        let l = self.length;
        let (n_base, gap, n_arc) = match self.lod {
            0 => (26, 0.04, 9),
            1 => (24, 0.07, 6),
            _ => (8, 0.14, 3),
        };
        // (s, hard) candidates.
        let mut cand: Vec<(f32, bool)> = Vec::new();
        for i in 0..=n_base {
            cand.push((l * i as f32 / n_base as f32, false));
        }
        // Silhouette corners on top must be stations; the underside's only matter a little.
        for &(x, _) in &self.top {
            cand.push((x, true));
        }
        for &(x, _) in &self.bot {
            cand.push((x, false));
        }
        let g = &spec.glass;
        for r in [g.windshield, g.backlight].iter().chain(g.side.iter()) {
            cand.push((r.0, true));
            cand.push((r.1, true));
        }
        for &(x, _) in &spec.plan {
            cand.push((x, false));
        }
        cand.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(core::cmp::Ordering::Equal));
        let mut kept: Vec<(f32, bool)> = Vec::new();
        for c in cand {
            if !(c.0 >= 0.0 && c.0 <= l) {
                continue;
            }
            match kept.last_mut() {
                Some(last) if c.0 - last.0 < gap => {
                    if c.1 && !last.1 {
                        *last = c;
                    } else if c.1 && last.1 && c.0 - last.0 > 0.004 {
                        kept.push(c);
                    }
                }
                _ => kept.push(c),
            }
        }
        // The tail must be a station.
        if kept.last().is_none_or(|k| k.0 < l) {
            if let Some(last) = kept.last_mut() {
                if l - last.0 < 0.01 {
                    last.0 = l;
                } else {
                    kept.push((l, true));
                }
            }
        }
        let mut st: Vec<Station> = kept
            .iter()
            .map(|&(s, _)| Station {
                s,
                clamp: None,
                seam: None,
                order: 0,
            })
            .collect();

        // Arches.
        for (a, &ax) in self.axle.iter().enumerate() {
            let (s0, s1) = (ax - self.ra, ax + self.ra);
            st.retain(|t| t.s < s0 - 0.03 || t.s > s1 + 0.03);
            let a = Some(a as u8);
            st.push(Station {
                s: s0,
                clamp: None,
                seam: None,
                order: 0,
            });
            st.push(Station {
                s: s0,
                clamp: a,
                seam: None,
                order: 1,
            });
            for k in 1..n_arc {
                let ang = PI * k as f32 / n_arc as f32;
                st.push(Station {
                    s: ax - self.ra * cos(ang),
                    clamp: a,
                    seam: None,
                    order: 0,
                });
            }
            st.push(Station {
                s: s1,
                clamp: a,
                seam: None,
                order: 0,
            });
            st.push(Station {
                s: s1,
                clamp: None,
                seam: None,
                order: 1,
            });
        }

        // Seams.
        let mut centers: Vec<(f32, SeamKind)> = Vec::new();
        if let Some(h) = self.hood {
            centers.push((h.1, SeamKind::HoodRear));
        }
        if let Some(t) = self.trunk {
            centers.push((t.0, SeamKind::TrunkFront));
        }
        for &d in &self.door_seams {
            centers.push((d, SeamKind::Door));
        }
        for (mut c, kind) in centers {
            for &ax in &self.axle {
                for edge in [ax - self.ra, ax + self.ra] {
                    if (c - edge).abs() < 0.03 {
                        c = if c < edge { edge - 0.03 } else { edge + 0.03 };
                    }
                }
            }
            let clamp = self.axle_over(c);
            st.retain(|t| {
                (t.s - c).abs() > 0.02 || (t.clamp.is_some() && t.order == 1) || {
                    // Keep doubled arch-edge stations.
                    let e = self.axle.iter().any(|&ax| {
                        (t.s - (ax - self.ra)).abs() < 1e-6 || (t.s - (ax + self.ra)).abs() < 1e-6
                    });
                    e
                }
            });
            st.push(Station {
                s: c - SEAM_HALF,
                clamp,
                seam: None,
                order: 0,
            });
            st.push(Station {
                s: c,
                clamp,
                seam: Some(kind),
                order: 0,
            });
            st.push(Station {
                s: c + SEAM_HALF,
                clamp,
                seam: None,
                order: 0,
            });
        }
        // A convertible's opening (or a pickup's bed) must start and end on a station, even
        // over an arch: a plain station close by gives way to it.
        let mut open = Vec::new();
        if spec.roof.convertible() {
            open.extend([g.windshield.1, g.backlight.0]);
        }
        if let Some((s0, s1)) = self.bed {
            open.extend([s0, s1]);
        }
        for s in open {
            let edge = |t: &Station| {
                self.axle.iter().any(|&ax| {
                    (t.s - (ax - self.ra)).abs() < 1e-6 || (t.s - (ax + self.ra)).abs() < 1e-6
                })
            };
            st.retain(|t| (t.s - s).abs() > 0.01 || t.seam.is_some() || edge(t) || t.s == s);
            if st.iter().all(|t| (t.s - s).abs() > 1e-6) {
                st.push(Station {
                    s,
                    clamp: self.axle_over(s),
                    seam: None,
                    order: 0,
                });
            }
        }
        st.sort_by(|a, b| {
            a.s.partial_cmp(&b.s)
                .unwrap_or(core::cmp::Ordering::Equal)
                .then(a.order.cmp(&b.order))
        });
        st
    }

    /// The axle whose arch covers `s`, if any.
    fn axle_over(&self, s: f32) -> Option<u8> {
        self.axle
            .iter()
            .position(|&ax| s > ax - self.ra && s < ax + self.ra)
            .map(|a| a as u8)
    }

    pub fn frame(&self, s: f32) -> Frame {
        let spec = self.spec;
        let sec = &spec.section;
        let yt = polyline_at(&self.top, s);
        let yb = polyline_at(&self.bot, s);
        let w = polyline_at(&spec.plan, s);
        let belt = polyline_at(&spec.belt, s);
        let span = yt - yb;
        let crown = sec.crown.min(span * 0.15);
        let y_edge = yt - crown;
        let y_ledge = clamp(belt, yb + 0.02, y_edge);
        let h = y_edge - y_ledge;
        let k = smoothstep(0.0, 0.08, h);
        let r_sh = ((1.0 - sec.shoulder) * w).min((y_ledge - yb) * 0.4);
        let lean = 0.015;
        let z_ledge = w - lean - r_sh;
        let z_gb = z_ledge - 0.025 * k;
        let rail = (0.04f32).min(h * 0.3) * k;
        let y_rr = y_edge - rail;
        let z_rr = z_gb - (y_rr - y_ledge) * self.tan_tumble;
        let roof_w = sec.greenhouse_frac * w;
        let z_re = (z_rr - 0.012 * k).min(lerp(z_rr, roof_w, k)).max(0.2);
        Frame {
            yt,
            yb,
            w,
            y_ledge,
            y_edge,
            z_ledge,
            z_re,
            crown,
            h,
        }
    }

    /// The un-cut half ring at `s`.
    pub fn ring_plain(&self, s: f32) -> (Vec<[f32; 2]>, Frame) {
        let spec = self.spec;
        let f = self.frame(s);
        let k = smoothstep(0.0, 0.08, f.h);
        let w = f.w;
        let lean = 0.015;
        let tuck = spec.section.tuck.min(w * 0.1);
        let r_sh = w - lean - f.z_ledge;
        let rocker_h = (0.07f32).min((f.y_ledge - f.yb) * 0.25);
        let y_sl = f.yb + rocker_h;
        let y_shl = f.y_ledge - r_sh;
        let side_z = |y: f32| w - lean * clamp((y - y_sl) / (y_shl - y_sl).max(1e-4), 0.0, 1.0);
        let z_gb = f.z_ledge - 0.025 * k;
        let rail = (0.04f32).min(f.h * 0.3) * k;
        let y_rr = f.y_edge - rail;
        let z_rr = z_gb - (y_rr - f.y_ledge) * self.tan_tumble;
        let z_f = (f.z_re - FRAME_W).max(f.z_re * 0.5);
        let z_seam = (spec.seams.hood_width * f.z_re).min(z_f - SEAM_HALF - 0.01);
        let has_line = self.slots.contains(&Slot::HoodLine);
        let z_line = z_seam * 0.7;
        let z_crown = if has_line {
            z_line * 0.5
        } else {
            f.z_re * 0.45
        };
        let mut pts = Vec::with_capacity(self.slots.len());
        for &slot in &self.slots {
            let p = match slot {
                Slot::FloorC => [0.0, f.yb],
                Slot::FloorIn => [self.z_in.min(w - tuck - 0.02), f.yb],
                Slot::RockerB => [w - tuck, f.yb + 0.01],
                Slot::SideLow => [w, y_sl],
                Slot::Crease(c, part) => {
                    let (x0, y0, x1, y1) = spec.seams.creases[c as usize];
                    let t = clamp((s - x0) / (x1 - x0), 0.0, 1.0);
                    let lo = y_sl + 2.0 * SEAM_HALF;
                    let hi = y_shl - 2.0 * SEAM_HALF;
                    let yc = if hi > lo {
                        clamp(lerp(y0, y1, t), lo, hi)
                    } else {
                        (y_sl + y_shl) * 0.5
                    };
                    let y = yc + part as f32 * SEAM_HALF;
                    [side_z(y), y]
                }
                Slot::ShoulderLow => [w - lean, y_shl],
                Slot::ShoulderMid => [w - lean - r_sh * 0.293, f.y_ledge - r_sh * 0.293],
                Slot::Ledge => [f.z_ledge, f.y_ledge],
                Slot::GlassB => [z_gb, f.y_ledge],
                Slot::RoofRail => [z_rr, y_rr],
                Slot::RoofEdge => [f.z_re, f.y_edge],
                Slot::Frame => [z_f, f.top_y(z_f)],
                Slot::TopSeam(part) => {
                    let z = z_seam + part as f32 * SEAM_HALF;
                    [z, f.top_y(z)]
                }
                Slot::HoodLine => [z_line, f.top_y(z_line)],
                Slot::Crown => [z_crown, f.top_y(z_crown)],
                Slot::Center => [0.0, f.yt],
            };
            pts.push(p);
        }
        (pts, f)
    }

    /// Outward 2D normal of ring point `j` from its neighbors.
    fn ring_normal(pts: &[[f32; 2]], j: usize) -> [f32; 2] {
        let a = pts[j.saturating_sub(1)];
        let b = pts[(j + 1).min(pts.len() - 1)];
        let t = [b[0] - a[0], b[1] - a[1]];
        let l = sqrt(t[0] * t[0] + t[1] * t[1]);
        if l < 1e-9 {
            return [1.0, 0.0];
        }
        [t[1] / l, -t[0] / l]
    }

    /// Arch height over the wheel at `s` for `axle`.
    pub fn arch_y(&self, axle: u8, s: f32, f: &Frame) -> f32 {
        let ds = s - self.axle[axle as usize];
        let y = self.wy + sqrt((self.ra * self.ra - ds * ds).max(0.0));
        y.min(f.y_ledge - 0.03)
    }

    /// The ring at a station with arches and seams cut in.
    pub fn ring(&self, st: &Station) -> Ring {
        let (mut pts, f) = self.ring_plain(st.s);
        let plain_y: Vec<f32> = pts.iter().map(|p| p[1]).collect();
        let n = pts.len();
        let mut raised = alloc::vec![false; n];
        let mut displaced = alloc::vec![false; n];
        if let Some(a) = st.clamp {
            let ya = self.arch_y(a, st.s, &f);
            for j in 0..n {
                if pts[j][0] > self.z_in + 0.001 && pts[j][1] < ya {
                    pts[j][1] = ya;
                    raised[j] = true;
                }
            }
        }
        let in_lid = |r: Option<(f32, f32)>| r.is_some_and(|(a, b)| st.s > a && st.s < b);
        if in_lid(self.hood) || in_lid(self.trunk) {
            if let Some(j) = self.slots.iter().position(|&s| s == Slot::HoodLine) {
                pts[j][1] += LINE_RISE;
            }
        }
        // Seam grooves: which slots get pushed in at this station.
        let plain = pts.clone();
        let mut push = |j: usize, raised: &[bool]| {
            if raised[j] {
                return;
            }
            let nrm = Self::ring_normal(&plain, j);
            pts[j][0] -= nrm[0] * SEAM_DEPTH;
            pts[j][1] -= nrm[1] * SEAM_DEPTH;
            displaced[j] = true;
        };
        let in_range = |r: Option<(f32, f32)>| r.is_some_and(|(a, b)| st.s > a && st.s < b);
        for (j, &slot) in self.slots.iter().enumerate() {
            match slot {
                Slot::Crease(c, 0) => {
                    let (x0, _, x1, _) = self.spec.seams.creases[c as usize];
                    if st.s >= x0 && st.s <= x1 {
                        push(j, &raised);
                    }
                }
                Slot::TopSeam(0) if in_range(self.hood) || in_range(self.trunk) => push(j, &raised),
                _ => {}
            }
        }
        match st.seam {
            Some(SeamKind::Door) => {
                if f.h > 0.02 {
                    for j in self.idx(Slot::SideLow)..=self.idx(Slot::Ledge) {
                        push(j, &raised);
                    }
                } else {
                    for j in self.idx(Slot::SideLow)..self.idx(Slot::ShoulderLow) {
                        push(j, &raised);
                    }
                }
            }
            Some(SeamKind::HoodRear) | Some(SeamKind::TrunkFront) => {
                let from = if self.slots.contains(&Slot::TopSeam(0)) {
                    self.idx(Slot::TopSeam(0))
                } else {
                    self.idx(Slot::Frame)
                };
                for j in from..n {
                    push(j, &raised);
                }
            }
            None => {}
        }
        Ring {
            pts,
            plain_y,
            raised,
            displaced,
            f,
        }
    }

    /// Height of a two-tone paint split at `s`: the first crease, else partway up the side.
    pub fn split_y(&self, s: f32) -> f32 {
        match self.spec.seams.creases.first() {
            Some(&(x0, y0, x1, y1)) => lerp(y0, y1, clamp((s - x0) / (x1 - x0), 0.0, 1.0)),
            None => {
                let f = self.frame(s);
                lerp(f.yb, f.y_ledge, 0.55)
            }
        }
    }

    /// The paint at height `y` of station `s`: a two-tone's lower color under the split.
    pub fn paint_at(&self, m: &Materials, s: f32, y: f32) -> Mat {
        if m.two_tone && y < self.split_y(s) {
            m.paint_low
        } else {
            m.paint
        }
    }

    /// Lateral position of the body side at `(s, y)` (un-cut shell, right side).
    pub fn side_z(&self, s: f32, y: f32) -> f32 {
        let (pts, _) = self.ring_plain(s);
        let lo = self.idx(Slot::RockerB);
        let hi = self.idx(Slot::RoofEdge);
        if y <= pts[lo][1] {
            return pts[lo][0];
        }
        for j in lo..hi {
            let (a, b) = (pts[j], pts[j + 1]);
            if y >= a[1] && y <= b[1] && b[1] > a[1] {
                return lerp(a[0], b[0], (y - a[1]) / (b[1] - a[1]));
            }
        }
        pts[hi][0]
    }

    /// Whether ring segment `j → j+1` between stations `a` and `b` is glass.
    fn glass_kind(&self, j: usize, a: f32, b: f32) -> bool {
        let g = &self.spec.glass;
        let within = |r: (f32, f32)| a >= r.0 - 1e-4 && b <= r.1 + 1e-4;
        let slot = self.slots[j];
        if slot == Slot::GlassB {
            return g.side.iter().any(|&r| within(r));
        }
        if j >= self.idx(Slot::Frame) && j + 1 < self.slots.len() {
            // A convertible's backlight range is the boot over a deck, not glass.
            return within(g.windshield) || (!self.convertible && within(g.backlight));
        }
        false
    }

    /// Builds the shell into `body` and `glass`.
    pub fn emit(&self, m: &Materials, body: &mut Soup, glass: &mut Soup) {
        let rings: Vec<Ring> = self.stations.iter().map(|st| self.ring(st)).collect();
        let n = self.slots.len();
        let black = self.has(Detail::BlackTrim);
        let chrome = self.has(Detail::ChromeTrim);
        let i_ledge = self.idx(Slot::Ledge);
        let i_gb = self.idx(Slot::GlassB);
        let i_rr = self.idx(Slot::RoofRail);
        let i_re = self.idx(Slot::RoofEdge);
        let i_rb = self.idx(Slot::RockerB);
        let crease = self
            .slots
            .contains(&Slot::Crease(0, 0))
            .then(|| (self.idx(Slot::Crease(0, -1)), self.idx(Slot::Crease(0, 1))));
        let g = &self.spec.glass;
        for i in 0..rings.len() - 1 {
            let (sa, sb) = (self.stations[i].s, self.stations[i + 1].s);
            let (ra, rb) = (&rings[i], &rings[i + 1]);
            let gh = ra.f.h.min(rb.f.h) > 0.02;
            for j in 0..n - 1 {
                if self.open_cut(j, sa, sb) {
                    continue;
                }
                let pa = ra.pts[j];
                let pb = ra.pts[j + 1];
                let pc = rb.pts[j + 1];
                let pd = rb.pts[j];
                let a = self.p(sa, pa[0], pa[1]);
                let b = self.p(sa, pb[0], pb[1]);
                let c = self.p(sb, pc[0], pc[1]);
                let d = self.p(sb, pd[0], pd[1]);
                let is_glass = self.glass_kind(j, sa, sb);
                let disp = ra.displaced[j]
                    || ra.displaced[j + 1]
                    || rb.displaced[j]
                    || rb.displaced[j + 1];
                let rais = ra.raised[j] || ra.raised[j + 1] || rb.raised[j] || rb.raised[j + 1];
                let nrm = norm(add(
                    cross(sub(b, a), sub(c, a)),
                    cross(sub(c, a), sub(d, a)),
                ));
                // Two-tone: by the crease slots when the ring has them, else by height (the
                // height before the arches lift the points, so the lower color runs over
                // the arches as it does between crease slots).
                let mut mat: Mat = match crease {
                    Some((lo, _)) if m.two_tone && j < lo => m.paint_low,
                    Some((lo, hi)) if m.two_tone && j >= lo && j < hi => m.pinstripe,
                    Some(_) => m.paint,
                    None => {
                        let y =
                            (ra.plain_y[j] + ra.plain_y[j + 1] + rb.plain_y[j] + rb.plain_y[j + 1])
                                * 0.25;
                        self.paint_at(m, (sa + sb) * 0.5, y)
                    }
                };
                let stripe = mat == m.pinstripe && m.two_tone;
                if j < i_rb {
                    mat = m.chassis;
                } else if rais && (nrm[2].abs() > 0.5 || nrm[1] < -0.3 || nrm[0] < 0.3) {
                    mat = m.liner;
                } else if disp && !stripe {
                    mat = m.seam;
                } else if gh && j == i_ledge && chrome {
                    mat = m.chrome;
                } else if gh && black && (j == i_gb || j == i_rr) && !is_glass {
                    mat = m.trim;
                } else if gh && chrome && !black && j == i_rr {
                    mat = m.chrome;
                } else if black && j == i_re && (sa >= g.windshield.0 && sb <= g.windshield.1) {
                    mat = m.trim;
                }
                let target: &mut Soup = if is_glass { &mut *glass } else { &mut *body };
                let mat = if is_glass { m.glass } else { mat };
                target.quad(a, b, c, d, mat);
                let mir = |p: V3| [-p[0], p[1], p[2]];
                target.quad(mir(a), mir(d), mir(c), mir(b), mat);
            }
        }
        // Nose and tail caps, sliced into horizontal slabs between mirrored ring points.
        for (ri, dir) in [(0usize, -1.0f32), (rings.len() - 1, 1.0)] {
            let r = &rings[ri];
            let s = self.stations[ri].s;
            for j in 0..n - 1 {
                let (p, q) = (r.pts[j], r.pts[j + 1]);
                let a = self.p(s, -p[0], p[1]);
                let b = self.p(s, p[0], p[1]);
                let c = self.p(s, q[0], q[1]);
                let d = self.p(s, -q[0], q[1]);
                let mat = if j < i_rb {
                    m.chassis
                } else {
                    self.paint_at(m, s, (p[1] + q[1]) * 0.5)
                        .uv(super::mesh::Uv::Face)
                };
                body.quad_dir(a, b, c, d, mat, [0.0, 0.0, dir]);
            }
        }
    }
}
