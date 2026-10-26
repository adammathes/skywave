//! CPU-side mesh data and builders for low-poly primitives.

use crate::atlas::{tile_uv, uv_unorm, Tile};
use bytemuck::{Pod, Zeroable};

/// One mesh vertex, 32 bytes.
///
/// `color` is linear RGBA in unorm8 and is multiplied by the instance color and by the
/// atlas texel at `uv`. `uv` is a normalized position in the whole atlas (`0..=65535` maps
/// to `0.0..=1.0`, v down); build it with [`crate::tile_uv`] and [`crate::uv_unorm`].
/// The default uv `[0, 0]` lies in [`Tile::White`], so untextured vertices are unaffected.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Pod, Zeroable)]
pub struct Vertex {
    pub pos: [f32; 3],
    pub normal: [f32; 3],
    pub uv: [u16; 2],
    pub color: [u8; 4],
}

impl Vertex {
    /// An untextured vertex (uv in the white tile).
    pub const fn new(pos: [f32; 3], normal: [f32; 3], color: [u8; 4]) -> Vertex {
        Vertex {
            pos,
            normal,
            uv: [0, 0],
            color,
        }
    }
}

/// Indexed triangle list, counter-clockwise front faces.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Mesh {
    pub vertices: Vec<Vertex>,
    pub indices: Vec<u32>,
}

fn lerp3(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] {
    [
        a[0] + (b[0] - a[0]) * t,
        a[1] + (b[1] - a[1]) * t,
        a[2] + (b[2] - a[2]) * t,
    ]
}

fn dist3(a: [f32; 3], b: [f32; 3]) -> f32 {
    let d = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
    (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt()
}

/// The six faces of an axis-aligned box as (corners, normal); each face's corners are
/// bottom-left, bottom-right, top-right, top-left seen from outside (for the top and
/// bottom faces, "up" is -Z and +Z respectively).
fn box_faces(min: [f32; 3], max: [f32; 3]) -> [([[f32; 3]; 4], [f32; 3]); 6] {
    let [x0, y0, z0] = min;
    let [x1, y1, z1] = max;
    [
        (
            [[x1, y0, z1], [x1, y0, z0], [x1, y1, z0], [x1, y1, z1]],
            [1.0, 0.0, 0.0],
        ),
        (
            [[x0, y0, z0], [x0, y0, z1], [x0, y1, z1], [x0, y1, z0]],
            [-1.0, 0.0, 0.0],
        ),
        (
            [[x0, y1, z1], [x1, y1, z1], [x1, y1, z0], [x0, y1, z0]],
            [0.0, 1.0, 0.0],
        ),
        (
            [[x0, y0, z0], [x1, y0, z0], [x1, y0, z1], [x0, y0, z1]],
            [0.0, -1.0, 0.0],
        ),
        (
            [[x0, y0, z1], [x1, y0, z1], [x1, y1, z1], [x0, y1, z1]],
            [0.0, 0.0, 1.0],
        ),
        (
            [[x1, y0, z0], [x0, y0, z0], [x0, y1, z0], [x1, y1, z0]],
            [0.0, 0.0, -1.0],
        ),
    ]
}

impl Mesh {
    /// Appends an untextured planar quad with corners `a, b, c, d` in counter-clockwise
    /// order seen from the side `normal` points to.
    pub fn push_quad(&mut self, corners: [[f32; 3]; 4], normal: [f32; 3], color: [u8; 4]) {
        self.push_quad_uv(corners, normal, tile_uv(Tile::White), color);
    }

    /// Appends a planar quad with an explicit atlas uv per corner.
    pub fn push_quad_uv(
        &mut self,
        corners: [[f32; 3]; 4],
        normal: [f32; 3],
        uv: [[f32; 2]; 4],
        color: [u8; 4],
    ) {
        let base = self.vertices.len() as u32;
        for (pos, uv) in corners.into_iter().zip(uv) {
            self.vertices.push(Vertex {
                pos,
                normal,
                uv: uv_unorm(uv),
                color,
            });
        }
        self.indices
            .extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
    }

    /// Appends a quad showing one copy of `tile`. Corners are bottom-left, bottom-right,
    /// top-right, top-left, as for [`Mesh::push_quad`].
    pub fn push_tile_quad(
        &mut self,
        corners: [[f32; 3]; 4],
        normal: [f32; 3],
        color: [u8; 4],
        tile: Tile,
    ) {
        self.push_quad_uv(corners, normal, tile_uv(tile), color);
    }

    /// Appends a quad split into `repeat[0]` columns (along `a→b`) by `repeat[1]` rows
    /// (along `a→d`), each showing one copy of `tile`, so the pattern repeats across the
    /// surface. A zero count is treated as one.
    pub fn push_tiled_quad(
        &mut self,
        corners: [[f32; 3]; 4],
        normal: [f32; 3],
        color: [u8; 4],
        tile: Tile,
        repeat: [u32; 2],
    ) {
        let [a, b, c, d] = corners;
        let (nu, nv) = (repeat[0].max(1), repeat[1].max(1));
        let at = |u: u32, v: u32| {
            let (s, t) = (u as f32 / nu as f32, v as f32 / nv as f32);
            lerp3(lerp3(a, b, s), lerp3(d, c, s), t)
        };
        let uv = tile_uv(tile);
        for v in 0..nv {
            for u in 0..nu {
                let cell = [at(u, v), at(u + 1, v), at(u + 1, v + 1), at(u, v + 1)];
                self.push_quad_uv(cell, normal, uv, color);
            }
        }
    }

    /// An axis-aligned box from `min` to `max` with flat-shaded faces (24 vertices).
    pub fn cuboid(min: [f32; 3], max: [f32; 3], color: [u8; 4]) -> Mesh {
        let mut m = Mesh::default();
        for (corners, normal) in box_faces(min, max) {
            m.push_quad(corners, normal, color);
        }
        m
    }

    /// An axis-aligned box whose faces repeat `tile` about every `tile_size` meters
    /// (each face gets a whole number of copies, at least one, in each direction).
    pub fn cuboid_tiled(
        min: [f32; 3],
        max: [f32; 3],
        color: [u8; 4],
        tile: Tile,
        tile_size: f32,
    ) -> Mesh {
        let count = |len: f32| ((len / tile_size.max(1e-3)).round() as u32).max(1);
        let mut m = Mesh::default();
        for (corners, normal) in box_faces(min, max) {
            let [a, b, _, d] = corners;
            m.push_tiled_quad(
                corners,
                normal,
                color,
                tile,
                [count(dist3(a, b)), count(dist3(a, d))],
            );
        }
        m
    }

    /// A box spanning `[-0.5, 0.5]` in x and z and `[0, 1]` in y: scale it to any
    /// footprint and height with the instance transform.
    pub fn unit_block(color: [u8; 4]) -> Mesh {
        Mesh::cuboid([-0.5, 0.0, -0.5], [0.5, 1.0, 0.5], color)
    }

    /// An upward-facing rectangle at `y = 0` centred on the origin.
    pub fn ground_quad(half_x: f32, half_z: f32, color: [u8; 4]) -> Mesh {
        let mut m = Mesh::default();
        m.push_quad(
            [
                [-half_x, 0.0, half_z],
                [half_x, 0.0, half_z],
                [half_x, 0.0, -half_z],
                [-half_x, 0.0, -half_z],
            ],
            [0.0, 1.0, 0.0],
            color,
        );
        m
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use glam::Vec3;

    /// Every triangle's geometric normal (from CCW winding) agrees with its vertex normals.
    fn assert_well_formed(m: &Mesh) {
        assert_eq!(m.indices.len() % 3, 0);
        for tri in m.indices.chunks_exact(3) {
            let v = [tri[0], tri[1], tri[2]].map(|i| m.vertices[i as usize]);
            let [a, b, c] = v.map(|v| Vec3::from(v.pos));
            let face = (b - a).cross(c - a);
            assert!(face.length() > 0.0, "degenerate triangle");
            for vert in v {
                let n = Vec3::from(vert.normal);
                assert!((n.length() - 1.0).abs() < 1e-6);
                assert!(
                    face.normalize().dot(n) > 0.999,
                    "winding disagrees with normal"
                );
            }
        }
    }

    /// Every edge is shared by exactly two triangles in opposite directions (by position).
    fn assert_closed(m: &Mesh) {
        let mut edges = Vec::new();
        for tri in m.indices.chunks_exact(3) {
            for k in 0..3 {
                let p = m.vertices[tri[k] as usize].pos;
                let q = m.vertices[tri[(k + 1) % 3] as usize].pos;
                edges.push((p, q));
            }
        }
        for &(p, q) in &edges {
            assert_eq!(edges.iter().filter(|&&e| e == (q, p)).count(), 1);
        }
    }

    #[test]
    fn cuboid_is_closed_flat_and_outward() {
        let m = Mesh::cuboid([-1.0, 0.0, -2.0], [1.0, 3.0, 2.0], [255; 4]);
        assert_eq!(m.vertices.len(), 24);
        assert_eq!(m.indices.len(), 36);
        assert_well_formed(&m);
        let center = Vec3::new(0.0, 1.5, 0.0);
        for v in &m.vertices {
            assert!((Vec3::from(v.pos) - center).dot(Vec3::from(v.normal)) > 0.0);
        }
        assert_closed(&m);
    }

    #[test]
    fn unit_block_and_ground_quad() {
        let b = Mesh::unit_block([10, 20, 30, 255]);
        assert_well_formed(&b);
        let ys: Vec<f32> = b.vertices.iter().map(|v| v.pos[1]).collect();
        assert!(ys.iter().all(|&y| y == 0.0 || y == 1.0));
        assert!(b.vertices.iter().all(|v| v.color == [10, 20, 30, 255]));

        let g = Mesh::ground_quad(5.0, 7.0, [255; 4]);
        assert_well_formed(&g);
        assert_eq!(g.indices.len(), 6);
        assert!(g.vertices.iter().all(|v| v.normal == [0.0, 1.0, 0.0]));
    }

    #[test]
    fn untextured_vertices_sample_the_white_tile() {
        let [u0, v0, u1, v1] = Tile::White.rect();
        let inside = |uv: [u16; 2]| {
            let (u, v) = (uv[0] as f32 / 65535.0, uv[1] as f32 / 65535.0);
            (u0 - 1e-4..=u1 + 1e-4).contains(&u) && (v0 - 1e-4..=v1 + 1e-4).contains(&v)
        };
        for v in Mesh::unit_block([255; 4]).vertices {
            assert!(inside(v.uv), "{:?}", v.uv);
        }
        // The default and zeroed vertices sit on the white tile's corner (white with
        // clamp-to-edge sampling).
        assert_eq!(Vertex::new([0.0; 3], [0.0, 1.0, 0.0], [255; 4]).uv, [0, 0]);
        assert_eq!(Vertex::zeroed().uv, [0, 0]);
    }

    #[test]
    fn tiled_quad_repeats_the_tile() {
        let mut m = Mesh::default();
        m.push_tiled_quad(
            [
                [0.0, 0.0, 0.0],
                [6.0, 0.0, 0.0],
                [6.0, 4.0, 0.0],
                [0.0, 4.0, 0.0],
            ],
            [0.0, 0.0, 1.0],
            [255; 4],
            Tile::BrickRed,
            [3, 2],
        );
        assert_well_formed(&m);
        assert_eq!(m.vertices.len(), 3 * 2 * 4);
        let uv = tile_uv(Tile::BrickRed).map(uv_unorm);
        for (k, v) in m.vertices.iter().enumerate() {
            assert_eq!(v.uv, uv[k % 4]);
        }
        // Cells are 2 m × 2 m and cover the quad exactly.
        let xs: Vec<f32> = m.vertices.iter().map(|v| v.pos[0]).collect();
        assert!(xs.iter().all(|x| [0.0, 2.0, 4.0, 6.0].contains(x)));
    }

    #[test]
    fn tiled_cuboid_is_closed_and_counts_repeats() {
        let m = Mesh::cuboid_tiled([0.0; 3], [6.0, 3.0, 2.0], [255; 4], Tile::Stucco, 1.0);
        assert_well_formed(&m);
        assert_closed(&m);
        // Faces: 2×(2×3) + 2×(6×3) + 2×(6×2) quads.
        assert_eq!(m.vertices.len(), (2 * 6 + 2 * 18 + 2 * 12) * 4);
    }
}
