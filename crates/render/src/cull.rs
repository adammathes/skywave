//! CPU frustum culling: local bounding boxes of meshes and chunk parts, carried to world
//! space by each instance's transform, tested against the planes of a view-projection
//! (the camera's perspective, or a shadow cascade's orthographic box, whose near plane
//! already sits the caster margin toward the sun). Plain arithmetic, no allocation.

use glam::{Mat4, Vec3, Vec4};

/// An axis-aligned box. [`Aabb::EMPTY`] (min above max) holds nothing and is never visible.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Aabb {
    pub min: Vec3,
    pub max: Vec3,
}

impl Aabb {
    pub const EMPTY: Aabb = Aabb {
        min: Vec3::splat(f32::INFINITY),
        max: Vec3::splat(f32::NEG_INFINITY),
    };

    pub fn is_empty(&self) -> bool {
        !(self.min.x <= self.max.x && self.min.y <= self.max.y && self.min.z <= self.max.z)
    }

    /// Grows the box to hold `p`.
    pub fn extend(&mut self, p: Vec3) {
        self.min = self.min.min(p);
        self.max = self.max.max(p);
    }

    /// The box round `points`.
    pub fn of_points(points: impl IntoIterator<Item = Vec3>) -> Aabb {
        let mut b = Aabb::EMPTY;
        for p in points {
            b.extend(p);
        }
        b
    }

    /// World-space center and half extents of this (model-space) box under the affine
    /// `model`: the box round the transformed box.
    pub fn transformed(&self, model: &Mat4) -> (Vec3, Vec3) {
        let c = 0.5 * (self.min + self.max);
        let e = 0.5 * (self.max - self.min);
        let center = model.transform_point3(c);
        let half = model.x_axis.truncate().abs() * e.x
            + model.y_axis.truncate().abs() * e.y
            + model.z_axis.truncate().abs() * e.z;
        (center, half)
    }
}

/// The six clip planes of a view-projection with depth in `[0, 1]`, as `n·p + d ≥ 0`
/// inside (not normalized: only signs are compared).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Frustum {
    planes: [Vec4; 6],
}

impl Frustum {
    /// Planes of `view_proj` (Gribb–Hartmann): left, right, bottom, top, near, far.
    pub fn from_matrix(view_proj: Mat4) -> Frustum {
        let (r0, r1, r2, r3) = (
            view_proj.row(0),
            view_proj.row(1),
            view_proj.row(2),
            view_proj.row(3),
        );
        Frustum {
            planes: [r3 + r0, r3 - r0, r3 + r1, r3 - r1, r2, r3 - r2],
        }
    }

    /// Whether the box (`center`, `half` extents) reaches inside. Conservative: a box
    /// outside the frustum near one of its corners can still count as inside.
    pub fn intersects(&self, center: Vec3, half: Vec3) -> bool {
        self.planes.iter().all(|p| {
            let n = p.truncate();
            n.dot(center) + p.w + n.abs().dot(half) >= 0.0
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use glam::camera::rh::{proj::directx, view};

    #[test]
    fn boxes_transform_to_their_world_bounds() {
        let b = Aabb::of_points([Vec3::new(-1.0, 0.0, -2.0), Vec3::new(1.0, 3.0, 2.0)]);
        let m = Mat4::from_translation(Vec3::new(10.0, 0.0, 0.0))
            * Mat4::from_rotation_y(core::f32::consts::FRAC_PI_2)
            * Mat4::from_scale(Vec3::splat(2.0));
        let (c, h) = b.transformed(&m);
        assert!((c - Vec3::new(10.0, 3.0, 0.0)).length() < 1e-5, "{c}");
        assert!((h - Vec3::new(4.0, 3.0, 2.0)).length() < 1e-5, "{h}");
        assert!(Aabb::EMPTY.is_empty() && !b.is_empty());
    }

    #[test]
    fn the_camera_frustum_keeps_what_it_sees() {
        let v = view::look_at_mat4(Vec3::new(0.0, 2.0, 0.0), Vec3::new(0.0, 2.0, -1.0), Vec3::Y);
        let p = directx::perspective(1.0, 16.0 / 9.0, 0.25, 1000.0);
        let f = Frustum::from_matrix(p * v);
        let unit = Vec3::splat(0.5);
        assert!(f.intersects(Vec3::new(0.0, 0.0, -20.0), unit), "ahead");
        assert!(!f.intersects(Vec3::new(0.0, 0.0, 20.0), unit), "behind");
        assert!(
            !f.intersects(Vec3::new(200.0, 0.0, -20.0), unit),
            "far right"
        );
        assert!(
            !f.intersects(Vec3::new(0.0, 0.0, -1200.0), unit),
            "past far"
        );
        // Straddling the left plane: kept.
        assert!(f.intersects(Vec3::new(-17.0, 0.0, -20.0), Vec3::splat(4.0)));
        // A long box behind the camera reaching into the view: kept.
        assert!(f.intersects(Vec3::new(0.0, 0.0, 5.0), Vec3::new(1.0, 1.0, 10.0)));
    }
}
