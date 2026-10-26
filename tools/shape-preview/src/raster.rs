//! A tiny software rasterizer: perspective camera, near-plane clipping, z-buffer, back-face
//! culling, Gouraud-interpolated Lambert from one sun plus a sky/ground ambient, vertex
//! colors, emissive added, alpha-blended glass.

use vehicle::shape::MeshData;

pub type V3 = [f32; 3];

fn sub(a: V3, b: V3) -> V3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
fn dot(a: V3, b: V3) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
fn cross(a: V3, b: V3) -> V3 {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}
pub fn norm(a: V3) -> V3 {
    let l = dot(a, a).sqrt();
    if l > 0.0 {
        [a[0] / l, a[1] / l, a[2] / l]
    } else {
        a
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Camera {
    pub eye: V3,
    pub target: V3,
    pub fov_deg: f32,
    pub near: f32,
}

/// A mesh to draw with a rigid transform: `p' = rot * p + offset` (rot given as rows).
pub struct Item<'a> {
    pub mesh: &'a MeshData,
    pub rot: [V3; 3],
    pub offset: V3,
    pub blend: bool,
    /// Draw both faces (for the ground).
    pub two_sided: bool,
}

impl<'a> Item<'a> {
    pub fn new(mesh: &'a MeshData) -> Self {
        Item {
            mesh,
            rot: [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
            offset: [0.0; 3],
            blend: false,
            two_sided: false,
        }
    }
}

/// Linear RGB render target with a 1/w depth buffer.
pub struct Target {
    pub w: usize,
    pub h: usize,
    pub color: Vec<V3>,
    depth: Vec<f32>,
}

const SUN: V3 = [-0.45, 0.78, -0.43];

fn shade(n: V3, color: [u8; 4], e: [u8; 4]) -> V3 {
    let l = norm(SUN);
    let ndl = dot(n, l).max(0.0);
    let sky = 0.24 + 0.12 * n[1];
    let k = ndl * 0.85 + sky;
    let e_scale = (e[3] & 0x7f) as f32 / 127.0 * 8.0;
    let mut out = [0.0; 3];
    for i in 0..3 {
        let c = color[i] as f32 / 255.0;
        out[i] = c * k + e[i] as f32 / 255.0 * e_scale.min(2.0) * 0.35;
    }
    out
}

/// A clip-space vertex: view-space position plus lit color and alpha.
#[derive(Clone, Copy)]
struct CV {
    v: V3,
    c: V3,
    a: f32,
}

fn lerp_cv(a: CV, b: CV, t: f32) -> CV {
    let l = |x: f32, y: f32| x + (y - x) * t;
    CV {
        v: [l(a.v[0], b.v[0]), l(a.v[1], b.v[1]), l(a.v[2], b.v[2])],
        c: [l(a.c[0], b.c[0]), l(a.c[1], b.c[1]), l(a.c[2], b.c[2])],
        a: l(a.a, b.a),
    }
}

impl Target {
    pub fn new(w: usize, h: usize) -> Target {
        let mut color = vec![[0.0; 3]; w * h];
        // Sky gradient.
        for y in 0..h {
            let t = y as f32 / h as f32;
            let c = [0.30 + 0.35 * t, 0.42 + 0.30 * t, 0.62 + 0.18 * t];
            for x in 0..w {
                color[y * w + x] = c;
            }
        }
        Target {
            w,
            h,
            color,
            depth: vec![0.0; w * h],
        }
    }

    pub fn draw(&mut self, cam: &Camera, items: &[Item], blend_pass: bool) {
        let f = norm(sub(cam.target, cam.eye));
        let r = norm(cross(f, [0.0, 1.0, 0.0]));
        let u = cross(r, f);
        let tan = (cam.fov_deg.to_radians() * 0.5).tan();
        let aspect = self.w as f32 / self.h as f32;
        for it in items {
            if it.blend != blend_pass {
                continue;
            }
            let m = it.mesh;
            let xf = |p: V3| {
                let q = [dot(it.rot[0], p), dot(it.rot[1], p), dot(it.rot[2], p)];
                [
                    q[0] + it.offset[0],
                    q[1] + it.offset[1],
                    q[2] + it.offset[2],
                ]
            };
            let verts: Vec<CV> = (0..m.positions.len())
                .map(|i| {
                    let p = xf(m.positions[i]);
                    let n = m.normals[i];
                    let n = [dot(it.rot[0], n), dot(it.rot[1], n), dot(it.rot[2], n)];
                    let d = sub(p, cam.eye);
                    CV {
                        v: [dot(d, r), dot(d, u), -dot(d, f)],
                        c: shade(n, m.colors[i], m.emissive[i]),
                        a: m.colors[i][3] as f32 / 255.0,
                    }
                })
                .collect();
            for t in m.indices.chunks(3) {
                let tri = [
                    verts[t[0] as usize],
                    verts[t[1] as usize],
                    verts[t[2] as usize],
                ];
                // Clip against the near plane z = -near.
                let mut poly: Vec<CV> = Vec::with_capacity(4);
                for k in 0..3 {
                    let a = tri[k];
                    let b = tri[(k + 1) % 3];
                    let ina = a.v[2] <= -cam.near;
                    let inb = b.v[2] <= -cam.near;
                    if ina {
                        poly.push(a);
                    }
                    if ina != inb {
                        let tt = (-cam.near - a.v[2]) / (b.v[2] - a.v[2]);
                        poly.push(lerp_cv(a, b, tt));
                    }
                }
                if poly.len() < 3 {
                    continue;
                }
                let proj: Vec<(f32, f32, f32, CV)> = poly
                    .iter()
                    .map(|cv| {
                        let w = -cv.v[2];
                        let x = cv.v[0] / (w * tan * aspect);
                        let y = cv.v[1] / (w * tan);
                        let sx = (x * 0.5 + 0.5) * self.w as f32;
                        let sy = (0.5 - y * 0.5) * self.h as f32;
                        (sx, sy, 1.0 / w, *cv)
                    })
                    .collect();
                for k in 1..proj.len() - 1 {
                    self.raster(proj[0], proj[k], proj[k + 1], blend_pass, it.two_sided);
                }
            }
        }
    }

    fn raster(
        &mut self,
        a: (f32, f32, f32, CV),
        b: (f32, f32, f32, CV),
        c: (f32, f32, f32, CV),
        blend: bool,
        two_sided: bool,
    ) {
        let area = (b.0 - a.0) * (c.1 - a.1) - (b.1 - a.1) * (c.0 - a.0);
        // Screen y points down, so a counter-clockwise front face has negative area.
        if area >= 0.0 && !two_sided {
            return;
        }
        if area.abs() < 1e-12 {
            return;
        }
        let minx = a.0.min(b.0).min(c.0).floor().max(0.0) as usize;
        let maxx = (a.0.max(b.0).max(c.0).ceil() as usize).min(self.w);
        let miny = a.1.min(b.1).min(c.1).floor().max(0.0) as usize;
        let maxy = (a.1.max(b.1).max(c.1).ceil() as usize).min(self.h);
        for py in miny..maxy {
            for px in minx..maxx {
                let (x, y) = (px as f32 + 0.5, py as f32 + 0.5);
                let w0 = ((b.0 - x) * (c.1 - y) - (b.1 - y) * (c.0 - x)) / area;
                let w1 = ((c.0 - x) * (a.1 - y) - (c.1 - y) * (a.0 - x)) / area;
                let w2 = 1.0 - w0 - w1;
                if w0 < 0.0 || w1 < 0.0 || w2 < 0.0 {
                    continue;
                }
                let iw = w0 * a.2 + w1 * b.2 + w2 * c.2;
                let idx = py * self.w + px;
                if iw <= self.depth[idx] {
                    continue;
                }
                let pc = |k: usize| {
                    (w0 * a.3.c[k] * a.2 + w1 * b.3.c[k] * b.2 + w2 * c.3.c[k] * c.2) / iw
                };
                let col = [pc(0), pc(1), pc(2)];
                if blend {
                    let al = (w0 * a.3.a * a.2 + w1 * b.3.a * b.2 + w2 * c.3.a * c.2) / iw;
                    let d = &mut self.color[idx];
                    for k in 0..3 {
                        d[k] = d[k] * (1.0 - al) + (col[k] + 0.04) * al;
                    }
                } else {
                    self.depth[idx] = iw;
                    self.color[idx] = col;
                }
            }
        }
    }

    /// Tone-mapped sRGB bytes, box-downsampled by `ss`.
    pub fn to_srgb(&self, ss: usize) -> (usize, usize, Vec<u8>) {
        let (w, h) = (self.w / ss, self.h / ss);
        let mut out = Vec::with_capacity(w * h * 3);
        for y in 0..h {
            for x in 0..w {
                let mut acc = [0.0f32; 3];
                for dy in 0..ss {
                    for dx in 0..ss {
                        let c = self.color[(y * ss + dy) * self.w + x * ss + dx];
                        for k in 0..3 {
                            acc[k] += c[k];
                        }
                    }
                }
                for a in acc {
                    let v = a / (ss * ss) as f32;
                    let t = 1.0 - (-1.45 * v).exp();
                    out.push((t.powf(1.0 / 2.2) * 255.0 + 0.5).clamp(0.0, 255.0) as u8);
                }
            }
        }
        (w, h, out)
    }
}
