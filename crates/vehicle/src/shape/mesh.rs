//! Plain mesh output and the triangle-soup builder every shape part writes into.
//!
//! Builders append triangles with a [`Mat`] (color, emissive, smoothing group). `finish`
//! computes normals with a crease angle: a corner averages the normals of the faces that
//! share its exact position, are in the same smoothing group and lie within the crease
//! angle of its own face. Rounded panels come out smooth, seams, creases and box edges
//! hard. Identical corners are then welded into indexed vertices.

use super::math::*;
use alloc::vec::Vec;

/// One mesh as plain arrays, one entry per vertex, plus a triangle list.
///
/// The attributes mirror the renderer's vertex (`render::Vertex`) so `app` can convert by
/// zipping: positions and normals in model space (see [`super::CarMesh`]); `uvs` in meters
/// along the surface for panels that want an atlas pattern (glass, wood, grille; zero
/// elsewhere), which `app` maps into an atlas tile;
/// `colors` linear RGBA unorm8; `emissive` linear RGB unorm8 with a 7-bit intensity scale
/// in alpha (`0..=127` → `0..=8×`, the same encoding as `render::emissive`). Triangles are
/// counter-clockwise seen from the front.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MeshData {
    pub positions: Vec<[f32; 3]>,
    pub normals: Vec<[f32; 3]>,
    pub uvs: Vec<[f32; 2]>,
    pub colors: Vec<[u8; 4]>,
    pub emissive: Vec<[u8; 4]>,
    pub indices: Vec<u32>,
}

impl MeshData {
    pub fn triangle_count(&self) -> usize {
        self.indices.len() / 3
    }

    pub fn vertex_count(&self) -> usize {
        self.positions.len()
    }
}

/// Encodes a self-lit color like `render::emissive`: linear rgb and a `0..=8` scale.
pub fn emissive(rgb: [u8; 3], scale: f32) -> [u8; 4] {
    let a = clamp(scale, 0.0, 8.0) / 8.0 * 127.0 + 0.5;
    [rgb[0], rgb[1], rgb[2], a as u8]
}

/// How a triangle's uvs are made from its positions.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Uv {
    None,
    /// `(z, y)`: along the car and up, for side panels.
    Side,
    /// `(z, x)`: along the car and across, for top panels.
    Top,
    /// `(x, y)`: across and up, for the nose and tail.
    Face,
}

/// Surface material of a triangle.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Mat {
    pub color: [u8; 4],
    pub emissive: [u8; 4],
    /// Faces only smooth with faces of the same group.
    pub group: u8,
    pub uv: Uv,
}

impl Mat {
    pub const fn new(color: [u8; 4]) -> Mat {
        Mat {
            color,
            emissive: [0; 4],
            group: 0,
            uv: Uv::None,
        }
    }

    pub const fn glow(self, e: [u8; 4]) -> Mat {
        Mat {
            emissive: e,
            ..self
        }
    }

    pub const fn group(self, group: u8) -> Mat {
        Mat { group, ..self }
    }

    pub const fn uv(self, uv: Uv) -> Mat {
        Mat { uv, ..self }
    }

    /// The same material with rgb scaled by `k` (alpha kept).
    pub fn shade(self, k: f32) -> Mat {
        let f = |c: u8| clamp(c as f32 * k + 0.5, 0.0, 255.0) as u8;
        Mat {
            color: [
                f(self.color[0]),
                f(self.color[1]),
                f(self.color[2]),
                self.color[3],
            ],
            ..self
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct Tri {
    p: [V3; 3],
    mat: Mat,
}

/// Triangles waiting for normals.
#[derive(Clone, Debug)]
pub struct Soup {
    tris: Vec<Tri>,
    crease_cos: f32,
}

/// `-0.0` becomes `0.0` so mirrored centerline points weld.
fn canon(p: V3) -> V3 {
    let f = |v: f32| if v == 0.0 { 0.0 } else { v };
    [f(p[0]), f(p[1]), f(p[2])]
}

/// Faces smaller than this (m², doubled) are dropped as degenerate.
const MIN_AREA2: f32 = 1e-9;

impl Soup {
    /// A builder that smooths across edges sharper than `crease_deg` only.
    pub fn new(crease_deg: f32) -> Soup {
        Soup {
            tris: Vec::new(),
            crease_cos: cos(deg(crease_deg)),
        }
    }

    /// Triangle `a, b, c`, counter-clockwise seen from the front. Degenerate ones are
    /// skipped.
    pub fn tri(&mut self, a: V3, b: V3, c: V3, mat: Mat) {
        let n = cross(sub(b, a), sub(c, a));
        let a2 = dot(n, n);
        if a2.is_nan() || a2 < MIN_AREA2 * MIN_AREA2 {
            return;
        }
        self.tris.push(Tri {
            p: [canon(a), canon(b), canon(c)],
            mat,
        });
    }

    /// Quad `a, b, c, d` in counter-clockwise order seen from the front.
    pub fn quad(&mut self, a: V3, b: V3, c: V3, d: V3, mat: Mat) {
        self.tri(a, b, c, mat);
        self.tri(a, c, d, mat);
    }

    /// Quad `a, b, c, d` (either winding), turned to face along `dir`.
    pub fn quad_dir(&mut self, a: V3, b: V3, c: V3, d: V3, mat: Mat, dir: V3) {
        let n = add(cross(sub(b, a), sub(c, a)), cross(sub(c, a), sub(d, a)));
        if dot(n, dir) >= 0.0 {
            self.quad(a, b, c, d, mat);
        } else {
            self.quad(a, d, c, b, mat);
        }
    }

    /// Quad turned to face away from `center`.
    pub fn quad_out(&mut self, a: V3, b: V3, c: V3, d: V3, mat: Mat, center: V3) {
        let mid = scale(add(add(a, b), add(c, d)), 0.25);
        self.quad_dir(a, b, c, d, mat, sub(mid, center));
    }

    /// Triangle turned to face along `dir`.
    pub fn tri_dir(&mut self, a: V3, b: V3, c: V3, mat: Mat, dir: V3) {
        let n = cross(sub(b, a), sub(c, a));
        if dot(n, dir) >= 0.0 {
            self.tri(a, b, c, mat);
        } else {
            self.tri(a, c, b, mat);
        }
    }

    /// Box with center `c`, unit axes `ax, ay, az` and half extents `h`.
    pub fn obox(&mut self, c: V3, axes: [V3; 3], h: V3, mat: Mat) {
        let corner = |sx: f32, sy: f32, sz: f32| {
            let p = madd(c, axes[0], sx * h[0]);
            let p = madd(p, axes[1], sy * h[1]);
            madd(p, axes[2], sz * h[2])
        };
        for (axis, other) in [(0usize, (1usize, 2usize)), (1, (2, 0)), (2, (0, 1))] {
            for sgn in [-1.0f32, 1.0] {
                let mut q = [[0.0; 3]; 4];
                let uv = [(-1.0, -1.0), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)];
                for (k, &(u, v)) in uv.iter().enumerate() {
                    let mut s = [0.0f32; 3];
                    s[axis] = sgn;
                    s[other.0] = u;
                    s[other.1] = v;
                    q[k] = corner(s[0], s[1], s[2]);
                }
                self.quad_dir(q[0], q[1], q[2], q[3], mat, scale(axes[axis], sgn));
            }
        }
    }

    /// Axis-aligned box between two opposite corners (in any order).
    pub fn aabb(&mut self, a: V3, b: V3, mat: Mat) {
        let min = [a[0].min(b[0]), a[1].min(b[1]), a[2].min(b[2])];
        let max = [a[0].max(b[0]), a[1].max(b[1]), a[2].max(b[2])];
        let c = scale(add(min, max), 0.5);
        let h = scale(sub(max, min), 0.5);
        self.obox(
            c,
            [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
            h,
            mat,
        );
    }

    /// Box spanning from `a` to `b` (its long axis) with cross-section half sizes
    /// `(hw, hh)`; `up` picks the orientation of the `hh` side.
    pub fn beam(&mut self, a: V3, b: V3, up: V3, hw: f32, hh: f32, mat: Mat) {
        let d = sub(b, a);
        let l = len(d);
        if l < 1e-6 {
            return;
        }
        let ax = scale(d, 1.0 / l);
        let side = norm(cross(up, ax));
        let upn = cross(ax, side);
        self.obox(
            scale(add(a, b), 0.5),
            [ax, side, upn],
            [l * 0.5, hw, hh],
            mat,
        );
    }

    /// Ring of `segs` points of radius `r` around `c` in the plane with normal `n`.
    pub fn circle(c: V3, n: V3, r: f32, segs: usize, phase: f32) -> Vec<V3> {
        let (u, v) = basis(n);
        (0..segs)
            .map(|i| {
                let a = phase + 2.0 * PI * i as f32 / segs as f32;
                let (s, co) = sin_cos(a);
                madd(madd(c, u, co * r), v, s * r)
            })
            .collect()
    }

    /// Flat disc facing `n`.
    pub fn disc(&mut self, c: V3, n: V3, r: f32, segs: usize, mat: Mat) {
        let ring = Soup::circle(c, n, r, segs, 0.0);
        for i in 0..segs {
            self.tri_dir(c, ring[i], ring[(i + 1) % segs], mat, n);
        }
    }

    /// Flat ring between radii `r0 < r1` facing `n`.
    pub fn annulus(&mut self, c: V3, n: V3, r0: f32, r1: f32, segs: usize, mat: Mat) {
        let a = Soup::circle(c, n, r0, segs, 0.0);
        let b = Soup::circle(c, n, r1, segs, 0.0);
        for i in 0..segs {
            let j = (i + 1) % segs;
            self.quad_dir(a[i], b[i], b[j], a[j], mat, n);
        }
    }

    /// Open or capped cylinder from `a` to `b`. `inward` turns the side to face the axis.
    #[allow(clippy::too_many_arguments)]
    pub fn cylinder(
        &mut self,
        a: V3,
        b: V3,
        r: f32,
        segs: usize,
        caps: bool,
        inward: bool,
        mat: Mat,
    ) {
        let axis = norm(sub(b, a));
        let ra = Soup::circle(a, axis, r, segs, 0.0);
        let rb = Soup::circle(b, axis, r, segs, 0.0);
        let mid = scale(add(a, b), 0.5);
        for i in 0..segs {
            let j = (i + 1) % segs;
            let q = scale(add(ra[i], rb[j]), 0.5);
            let out = sub(q, mid);
            let out = sub(out, scale(axis, dot(out, axis)));
            let dir = if inward { scale(out, -1.0) } else { out };
            self.quad_dir(ra[i], rb[i], rb[j], ra[j], mat, dir);
        }
        if caps {
            self.disc(a, scale(axis, -1.0), r, segs, mat);
            self.disc(b, axis, r, segs, mat);
        }
    }

    /// Surface of revolution around the axis through `c` along unit `axis`: `profile` is a
    /// list of `(along, radius)` points. Each quad faces away from `away`, a point in the
    /// same `(along, radius)` space (e.g. the middle of a tire's cross-section).
    #[allow(clippy::too_many_arguments)]
    pub fn lathe(
        &mut self,
        c: V3,
        axis: V3,
        profile: &[(f32, f32)],
        segs: usize,
        phase: f32,
        away: (f32, f32),
        mat: Mat,
    ) {
        let (u, v) = basis(axis);
        let pt = |along: f32, r: f32, a: f32| {
            let (s, co) = sin_cos(a);
            madd(madd(madd(c, axis, along), u, co * r), v, s * r)
        };
        for k in 0..profile.len().saturating_sub(1) {
            let (a0, r0) = profile[k];
            let (a1, r1) = profile[k + 1];
            let (ma, mr) = ((a0 + a1) * 0.5, (r0 + r1) * 0.5);
            for i in 0..segs {
                let ang0 = phase + 2.0 * PI * i as f32 / segs as f32;
                let ang1 = phase + 2.0 * PI * (i + 1) as f32 / segs as f32;
                let p0 = pt(a0, r0, ang0);
                let p1 = pt(a1, r1, ang0);
                let p2 = pt(a1, r1, ang1);
                let p3 = pt(a0, r0, ang1);
                let mid = (ang0 + ang1) * 0.5;
                let from = pt(away.0, away.1, mid);
                let to = pt(ma, mr, mid);
                self.quad_dir(p0, p1, p2, p3, mat, sub(to, from));
            }
        }
    }

    /// Torus around `c` with unit `axis`, major radius `big`, tube radius `small`.
    #[allow(clippy::too_many_arguments)]
    pub fn torus(
        &mut self,
        c: V3,
        axis: V3,
        big: f32,
        small: f32,
        segs: usize,
        tube: usize,
        mat: Mat,
    ) {
        let (u, v) = basis(axis);
        let p = |i: usize, k: usize| {
            let a = 2.0 * PI * i as f32 / segs as f32;
            let b = 2.0 * PI * k as f32 / tube as f32;
            let (sa, ca) = sin_cos(a);
            let (sb, cb) = sin_cos(b);
            let radial = add(scale(u, ca), scale(v, sa));
            let center = madd(c, radial, big);
            (
                madd(madd(center, radial, cb * small), axis, sb * small),
                center,
            )
        };
        for i in 0..segs {
            for k in 0..tube {
                let (p0, c0) = p(i, k);
                let (p1, _) = p(i + 1, k);
                let (p2, c2) = p(i + 1, k + 1);
                let (p3, _) = p(i, k + 1);
                let center = scale(add(c0, c2), 0.5);
                self.quad_out(p0, p1, p2, p3, mat, center);
            }
        }
    }

    /// Sweeps a closed `profile` of `(u, v)` offsets along `path`: point `k` of the path
    /// places the profile in the plane spanned by `us[k]` and `vs[k]`. Quads face away from
    /// the path; `caps` closes both ends.
    pub fn sweep(
        &mut self,
        path: &[V3],
        us: &[V3],
        vs: &[V3],
        profile: &[(f32, f32)],
        caps: bool,
        mat: Mat,
    ) {
        let n = profile.len();
        if path.len() < 2 || n < 3 {
            return;
        }
        let ring = |k: usize| -> Vec<V3> {
            profile
                .iter()
                .map(|&(a, b)| madd(madd(path[k], us[k], a), vs[k], b))
                .collect()
        };
        // Profile centroid, so the outward test works for an offset profile.
        let (mut cu, mut cv) = (0.0, 0.0);
        for &(a, b) in profile {
            cu += a;
            cv += b;
        }
        cu /= n as f32;
        cv /= n as f32;
        let center = |k: usize| madd(madd(path[k], us[k], cu), vs[k], cv);
        let mut prev = ring(0);
        for k in 1..path.len() {
            let cur = ring(k);
            let c = scale(add(center(k - 1), center(k)), 0.5);
            for i in 0..n {
                let j = (i + 1) % n;
                self.quad_out(prev[i], cur[i], cur[j], prev[j], mat, c);
            }
            prev = cur;
        }
        if caps {
            for (k, toward) in [(0usize, 1usize), (path.len() - 1, path.len() - 2)] {
                let r = ring(k);
                let c = center(k);
                let dir = sub(path[k], path[toward]);
                for i in 0..n {
                    self.tri_dir(c, r[i], r[(i + 1) % n], mat, dir);
                }
            }
        }
    }

    /// Appends every triangle of `other`.
    pub fn extend(&mut self, other: Soup) {
        self.tris.extend(other.tris);
    }

    /// Normals, welding and indexing.
    pub fn finish(self) -> MeshData {
        let tris = self.tris;
        let n = tris.len();
        let mut face_n: Vec<V3> = Vec::with_capacity(n);
        let mut face_u: Vec<V3> = Vec::with_capacity(n);
        for t in &tris {
            let fnrm = cross(sub(t.p[1], t.p[0]), sub(t.p[2], t.p[0]));
            face_n.push(fnrm);
            face_u.push(norm(fnrm));
        }
        // Corners sorted by exact position; ties keep triangle order (stable sort).
        let key = |p: V3| [p[0].to_bits(), p[1].to_bits(), p[2].to_bits()];
        let mut corners: Vec<(u32, u8)> = Vec::with_capacity(n * 3);
        for (ti, _) in tris.iter().enumerate() {
            for c in 0..3u8 {
                corners.push((ti as u32, c));
            }
        }
        corners.sort_by(|a, b| {
            key(tris[a.0 as usize].p[a.1 as usize]).cmp(&key(tris[b.0 as usize].p[b.1 as usize]))
        });

        let mut out = MeshData::default();
        let mut index_of = alloc::vec![0u32; n * 3];
        let mut start = 0;
        while start < corners.len() {
            let k0 = key(tris[corners[start].0 as usize].p[corners[start].1 as usize]);
            let mut end = start + 1;
            while end < corners.len()
                && key(tris[corners[end].0 as usize].p[corners[end].1 as usize]) == k0
            {
                end += 1;
            }
            let first_vertex = out.positions.len();
            for ci in start..end {
                let (ti, c) = corners[ci];
                let t = &tris[ti as usize];
                let mine = face_u[ti as usize];
                let mut acc = [0.0f32; 3];
                for &(tj, _) in &corners[start..end] {
                    let o = &tris[tj as usize];
                    if o.mat.group == t.mat.group
                        && dot(face_u[tj as usize], mine) >= self.crease_cos
                    {
                        acc = add(acc, face_n[tj as usize]);
                    }
                }
                let nrm = norm(acc);
                let nrm = canon(nrm);
                let pos = t.p[c as usize];
                let uv = match t.mat.uv {
                    Uv::None => [0.0, 0.0],
                    Uv::Side => [pos[2], pos[1]],
                    Uv::Top => [pos[2], pos[0]],
                    Uv::Face => [pos[0], pos[1]],
                };
                // Weld with an identical vertex already emitted at this position.
                let mut found = None;
                for v in first_vertex..out.positions.len() {
                    if out.normals[v] == nrm
                        && out.colors[v] == t.mat.color
                        && out.emissive[v] == t.mat.emissive
                        && out.uvs[v] == uv
                    {
                        found = Some(v);
                        break;
                    }
                }
                let vi = match found {
                    Some(v) => v,
                    None => {
                        out.positions.push(pos);
                        out.normals.push(nrm);
                        out.uvs.push(uv);
                        out.colors.push(t.mat.color);
                        out.emissive.push(t.mat.emissive);
                        out.positions.len() - 1
                    }
                };
                index_of[ti as usize * 3 + c as usize] = vi as u32;
            }
            start = end;
        }
        out.indices = index_of;
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RED: Mat = Mat::new([200, 0, 0, 255]);

    #[test]
    fn box_is_closed_and_hard_edged() {
        let mut s = Soup::new(40.0);
        s.aabb([0.0; 3], [1.0, 2.0, 3.0], RED);
        let m = s.finish();
        assert_eq!(m.triangle_count(), 12);
        // Hard edges: 4 vertices per face.
        assert_eq!(m.vertex_count(), 24);
        // Every face points away from the center.
        for t in m.indices.chunks(3) {
            let [a, b, c] = [0, 1, 2].map(|k| m.positions[t[k] as usize]);
            let n = cross(sub(b, a), sub(c, a));
            let mid = scale(add(add(a, b), c), 1.0 / 3.0);
            assert!(dot(n, sub(mid, [0.5, 1.0, 1.5])) > 0.0);
        }
    }

    #[test]
    fn cylinder_sides_are_smooth() {
        let mut s = Soup::new(40.0);
        s.cylinder([0.0; 3], [0.0, 1.0, 0.0], 0.5, 16, false, false, RED);
        let m = s.finish();
        // 16 columns of 2 vertices shared by neighbors.
        assert_eq!(m.vertex_count(), 32);
        for (p, n) in m.positions.iter().zip(&m.normals) {
            assert!(n[1].abs() < 1e-5);
            assert!(dot(*n, [p[0], 0.0, p[2]]) > 0.0);
        }
    }

    #[test]
    fn degenerate_triangles_are_dropped() {
        let mut s = Soup::new(40.0);
        s.tri([0.0; 3], [1.0, 0.0, 0.0], [2.0, 0.0, 0.0], RED);
        s.tri([0.0; 3], [0.0; 3], [0.0, 1.0, 0.0], RED);
        assert!(s.tris.is_empty());
    }
}
