//! CPU-side mesh data and builders for low-poly primitives.

use bytemuck::{Pod, Zeroable};

/// One mesh vertex. `color` is linear RGBA in unorm8 and is multiplied by the instance color.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Pod, Zeroable)]
pub struct Vertex {
    pub pos: [f32; 3],
    pub normal: [f32; 3],
    pub color: [u8; 4],
}

/// Indexed triangle list, counter-clockwise front faces.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Mesh {
    pub vertices: Vec<Vertex>,
    pub indices: Vec<u32>,
}

impl Mesh {
    /// Appends a planar quad with corners `a, b, c, d` in counter-clockwise order seen
    /// from the side `normal` points to.
    pub fn push_quad(&mut self, corners: [[f32; 3]; 4], normal: [f32; 3], color: [u8; 4]) {
        let base = self.vertices.len() as u32;
        for pos in corners {
            self.vertices.push(Vertex { pos, normal, color });
        }
        self.indices
            .extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
    }

    /// An axis-aligned box from `min` to `max` with flat-shaded faces (24 vertices).
    pub fn cuboid(min: [f32; 3], max: [f32; 3], color: [u8; 4]) -> Mesh {
        let [x0, y0, z0] = min;
        let [x1, y1, z1] = max;
        let mut m = Mesh::default();
        m.push_quad(
            [[x1, y0, z1], [x1, y0, z0], [x1, y1, z0], [x1, y1, z1]],
            [1.0, 0.0, 0.0],
            color,
        );
        m.push_quad(
            [[x0, y0, z0], [x0, y0, z1], [x0, y1, z1], [x0, y1, z0]],
            [-1.0, 0.0, 0.0],
            color,
        );
        m.push_quad(
            [[x0, y1, z1], [x1, y1, z1], [x1, y1, z0], [x0, y1, z0]],
            [0.0, 1.0, 0.0],
            color,
        );
        m.push_quad(
            [[x0, y0, z0], [x1, y0, z0], [x1, y0, z1], [x0, y0, z1]],
            [0.0, -1.0, 0.0],
            color,
        );
        m.push_quad(
            [[x0, y0, z1], [x1, y0, z1], [x1, y1, z1], [x0, y1, z1]],
            [0.0, 0.0, 1.0],
            color,
        );
        m.push_quad(
            [[x1, y0, z0], [x0, y0, z0], [x0, y1, z0], [x1, y1, z0]],
            [0.0, 0.0, -1.0],
            color,
        );
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
        // Closed: every edge is shared by exactly two triangles (compare by position).
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
}
