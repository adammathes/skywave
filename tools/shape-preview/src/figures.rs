//! `--figures`: a contact sheet of seeded pedestrian figures (`traffic::figure`) mid-stride,
//! one row per era.

use crate::raster::{Camera, Item, Target};
use traffic::figure::{figure, Era, Xform, PARTS};
use vehicle::shape::MeshData;

/// Columns per row; one row per era.
pub const COLS: usize = 6;

/// A small square of ground with a soft shadow under the figure.
fn floor() -> MeshData {
    let mut m = MeshData::default();
    let n = 16;
    let size = 3.0f32;
    for j in 0..=n {
        for i in 0..=n {
            let x = -size * 0.5 + size * i as f32 / n as f32;
            let z = -size * 0.5 + size * j as f32 / n as f32;
            let d = (x * x + z * z).sqrt();
            let g = (120.0 * (1.0 - 0.5 * (-d * d * 9.0).exp())) as u8;
            m.positions.push([x, 0.0, z]);
            m.normals.push([0.0, 1.0, 0.0]);
            m.uvs.push([0.0, 0.0]);
            m.colors.push([g, g, (g as f32 * 1.02) as u8, 255]);
            m.emissive.push([0; 4]);
        }
    }
    for j in 0..n {
        for i in 0..n {
            let a = (j * (n + 1) + i) as u32;
            let (b, c, d) = (a + 1, a + (n + 1) as u32 + 1, a + (n + 1) as u32);
            m.indices.extend_from_slice(&[a, d, c, a, c, b]);
        }
    }
    m
}

/// Renders `COLS` figures per era (seeds `seed..seed + COLS`), each `pw` by `ph`.
pub fn sheet(seed: u64, pw: usize, ph: usize, ss: usize) -> (usize, usize, Vec<u8>) {
    let (w, h) = (pw * COLS, ph * 2);
    let mut out = vec![0u8; w * h * 3];
    let ground = floor();
    for (row, era) in [Era::Seventies, Era::Eighties].into_iter().enumerate() {
        for col in 0..COLS {
            let s = seed + col as u64;
            let fig = figure(s, era);
            // Mid-stride, at a spread of walking speeds.
            let speed = [1.4, 1.2, 1.6, 1.0, 1.3, 1.5][col];
            let joints = fig.skeleton.walk(0.12 + 0.07 * col as f32, speed);
            let mut xf = [Xform::IDENTITY; PARTS];
            fig.skeleton.pose(&joints, &mut xf);
            println!(
                "{era:?} seed {s}: {:.2} m, build {:.2}, {:?}, {:?}, {} tris",
                fig.spec.height_m,
                fig.spec.build,
                fig.spec.outfit,
                fig.spec.hair,
                fig.triangle_count()
            );
            let mut items = vec![{
                let mut g = Item::new(&ground);
                g.two_sided = true;
                g
            }];
            for (k, m) in fig.parts.iter().enumerate() {
                let mut it = Item::new(m);
                it.rot = xf[k].rot;
                it.offset = xf[k].pos;
                items.push(it);
            }
            let cam = Camera {
                eye: [-2.6, 1.45, -3.6],
                target: [0.0, 0.88, 0.0],
                fov_deg: 30.0,
                near: 0.1,
            };
            let mut t = Target::new(pw * ss, ph * ss);
            t.draw(&cam, &items, false);
            let (tw, th, px) = t.to_srgb(ss);
            for y in 0..th {
                for x in 0..tw {
                    let o = ((row * ph + y) * w + col * pw + x) * 3;
                    let i = (y * tw + x) * 3;
                    let edge = (x == 0 && col > 0) || (y == 0 && row > 0);
                    for ch in 0..3 {
                        out[o + ch] = if edge { 20 } else { px[i + ch] };
                    }
                }
            }
        }
    }
    (w, h, out)
}
