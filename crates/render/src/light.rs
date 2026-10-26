//! Fitting the sun's orthographic shadow projections: one per cascade, each a texel-snapped
//! box around the bounding sphere of a slice of the camera frustum.
//!
//! Bounding spheres make the box size depend only on the projection and the split
//! distances, never on the camera's orientation, so the texel grid does not change scale as
//! the camera turns; snapping the box to whole texels then removes shimmer when it moves.

use glam::camera::rh::{proj::directx, view};
use glam::{Mat4, Vec3, Vec4Swizzles};

/// Number of shadow cascades.
pub const CASCADES: usize = 3;

/// Light-space view-projection for a directional light toward `to_sun` (unit) whose shadow
/// map of `resolution` texels covers the sphere (`center`, `radius`). Casters up to
/// `caster_margin` meters beyond the sphere toward the sun are kept.
///
/// The light-space position of the box is snapped to whole texels so the map does not
/// shimmer as the camera moves.
pub fn sun_view_proj(
    to_sun: Vec3,
    center: Vec3,
    radius: f32,
    caster_margin: f32,
    resolution: u32,
) -> Mat4 {
    let forward = -to_sun.normalize();
    let up = if forward.y.abs() > 0.99 {
        Vec3::Z
    } else {
        Vec3::Y
    };
    let light_view = view::look_to_mat4(Vec3::ZERO, forward, up);
    let c = (light_view * center.extend(1.0)).xyz();
    let texel = 2.0 * radius / resolution as f32;
    let cx = (c.x / texel).round() * texel;
    let cy = (c.y / texel).round() * texel;
    // View space looks down -Z: the sphere spans z in [c.z - r, c.z + r].
    let near = -(c.z + radius + caster_margin);
    let far = -(c.z - radius);
    let proj = directx::orthographic(
        cx - radius,
        cx + radius,
        cy - radius,
        cy + radius,
        near,
        far,
    );
    proj * light_view
}

/// One fitted cascade.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CascadeFit {
    /// World-to-clip of the cascade's orthographic sun projection (depth in `[0, 1]`).
    pub view_proj: Mat4,
    /// Bounding-sphere center in world space, meters.
    pub center: Vec3,
    /// Bounding-sphere radius, meters; the ortho box is `2 * radius` wide.
    pub radius: f32,
    /// World size of one shadow-map texel, meters.
    pub texel: f32,
    /// View-depth range `[near, far]` the sphere covers, meters.
    pub depth_range: [f32; 2],
}

/// Camera near and far view depths (positive meters) of a perspective `proj` with depth in
/// `[0, 1]`. An infinite far plane comes back as `f32::INFINITY`.
pub fn camera_depth_range(proj: Mat4) -> (f32, f32) {
    let inv = proj.inverse();
    let near = -inv.project_point3(Vec3::ZERO).z;
    let far = -inv.project_point3(Vec3::Z).z;
    let far = if far.is_finite() && far > near {
        far
    } else {
        f32::INFINITY
    };
    (near.max(1e-4), far)
}

/// View-space directions of the four frustum corner rays, scaled to unit view depth
/// (`z = -1`), in the order (-x,-y), (+x,-y), (+x,+y), (-x,+y).
fn corner_rays(proj: Mat4) -> [Vec3; 4] {
    let inv = proj.inverse();
    [(-1.0, -1.0), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)].map(|(x, y)| {
        let p = inv.project_point3(Vec3::new(x, y, 0.5));
        p / -p.z
    })
}

/// World-space corners of the frustum slice between view depths `near` and `far`: the four
/// near corners, then the four far corners.
pub fn slice_corners(view: Mat4, proj: Mat4, near: f32, far: f32) -> [Vec3; 8] {
    let inv_view = view.inverse();
    let rays = corner_rays(proj);
    let mut out = [Vec3::ZERO; 8];
    for (k, ray) in rays.iter().enumerate() {
        out[k] = inv_view.transform_point3(*ray * near);
        out[k + 4] = inv_view.transform_point3(*ray * far);
    }
    out
}

/// View-space center (on the view axis) and radius of a sphere holding the frustum slice.
/// Depends only on the projection and the depths, so it is identical every frame.
fn slice_sphere(rays: &[Vec3; 4], near: f32, far: f32) -> (Vec3, f32) {
    // For a symmetric frustum the smallest enclosing sphere has its center on the axis at
    // depth (n + f)(1 + k²)/2, clamped to f, where k is the corner ray's lateral slope.
    let k2 = rays
        .iter()
        .map(|r| r.x * r.x + r.y * r.y)
        .fold(0.0, f32::max);
    let d = (0.5 * (near + far) * (1.0 + k2)).min(far);
    let center = Vec3::new(0.0, 0.0, -d);
    // Take the farthest corner so off-axis projections stay covered as well.
    let mut r2: f32 = 0.0;
    for ray in rays {
        r2 = r2.max((*ray * near - center).length_squared());
        r2 = r2.max((*ray * far - center).length_squared());
    }
    // Round up to 1/64 m so float noise can never change the texel size.
    let radius = (r2.sqrt() * 64.0).ceil() / 64.0;
    (center, radius)
}

/// Fits [`CASCADES`] sun projections to the camera. `splits` are the far view depths of each
/// cascade in meters (clamped to the camera's far plane); cascade `i + 1` starts `blend` of
/// cascade `i`'s length before cascade `i` ends, so the shader can cross-fade in that band.
/// Each map is `resolution` texels square; casters up to `caster_margin` meters beyond a
/// cascade's sphere toward the sun still cast into it.
pub fn fit_cascades(
    view: Mat4,
    proj: Mat4,
    to_sun: Vec3,
    splits: [f32; CASCADES],
    blend: f32,
    resolution: u32,
    caster_margin: f32,
) -> [CascadeFit; CASCADES] {
    let (cam_near, cam_far) = camera_depth_range(proj);
    let rays = corner_rays(proj);
    let inv_view = view.inverse();
    let mut near = cam_near;
    let mut start = 0.0;
    core::array::from_fn(|i| {
        let far = splits[i].min(cam_far).max(near + 1e-3);
        let (c, radius) = slice_sphere(&rays, near, far);
        let center = inv_view.transform_point3(c);
        let fit = CascadeFit {
            view_proj: sun_view_proj(to_sun, center, radius, caster_margin, resolution),
            center,
            radius,
            texel: 2.0 * radius / resolution as f32,
            depth_range: [near, far],
        };
        // The shader blends over the last `blend` of [start, far], where `start` is the
        // previous split (not the overlapped near), so the next slice reaches back that far.
        near = (far - blend.clamp(0.0, 1.0) * (far - start)).max(cam_near);
        start = far;
        fit
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ndc(m: Mat4, p: Vec3) -> Vec3 {
        m.project_point3(p)
    }

    fn camera() -> (Mat4, Mat4) {
        let view = view::look_at_mat4(
            Vec3::new(-75.0, 24.0, 165.0),
            Vec3::new(0.0, 0.0, 100.0),
            Vec3::Y,
        );
        let proj = directx::perspective(62f32.to_radians(), 16.0 / 9.0, 0.25, 3000.0);
        (view, proj)
    }

    #[test]
    fn camera_depth_range_reads_near_and_far() {
        let (_, proj) = camera();
        let (n, f) = camera_depth_range(proj);
        assert!((n - 0.25).abs() < 1e-4, "{n}");
        assert!((f / 3000.0 - 1.0).abs() < 2e-3, "{f}");
        let inf = directx::perspective_infinite(1.0, 1.5, 0.1);
        assert_eq!(camera_depth_range(inf).1, f32::INFINITY);
    }

    #[test]
    fn every_slice_corner_is_inside_its_cascade_box() {
        let (_, proj) = camera();
        let splits = [40.0, 160.0, 600.0];
        let suns = [
            Vec3::new(0.70, 0.55, 0.45),
            Vec3::new(-0.2, 0.98, 0.1),
            Vec3::new(0.9, 0.06, -0.4),
            Vec3::Y,
        ];
        for (s, to_sun) in suns.iter().enumerate() {
            let to_sun = to_sun.normalize();
            for k in 0..24 {
                // Look around in yaw and pitch from a moving eye.
                let yaw = k as f32 * 0.7 + s as f32;
                let pitch = (k as f32 * 0.37).sin() * 1.2;
                let dir = Vec3::new(
                    -yaw.sin() * pitch.cos(),
                    pitch.sin(),
                    -yaw.cos() * pitch.cos(),
                );
                let eye = Vec3::new(k as f32 * 13.1, 1.5 + k as f32, k as f32 * -7.3);
                let view = view::look_to_mat4(eye, dir, Vec3::Y);
                let fits = fit_cascades(view, proj, to_sun, splits, 0.1, 2048, 400.0);
                for fit in &fits {
                    let [n, f] = fit.depth_range;
                    for p in slice_corners(view, proj, n, f) {
                        let q = ndc(fit.view_proj, p);
                        assert!(q.x.abs() <= 1.0 && q.y.abs() <= 1.0, "{q} sun {s} k {k}");
                        assert!((0.0..=1.0).contains(&q.z), "{q}");
                        // Casters 300 m toward the sun from any corner are not clipped.
                        let c = ndc(fit.view_proj, p + to_sun * 300.0);
                        assert!(c.z >= 0.0, "{c}");
                    }
                }
            }
        }
    }

    #[test]
    fn cascades_cover_the_splits_with_blend_overlap() {
        let (view, proj) = camera();
        let fits = fit_cascades(view, proj, Vec3::Y, [40.0, 160.0, 600.0], 0.1, 2048, 400.0);
        assert!((fits[0].depth_range[0] - 0.25).abs() < 1e-4);
        assert_eq!(fits[0].depth_range[1], 40.0);
        // Cascade 1 starts 10% of cascade 0's 40 m before 40 m.
        assert!((fits[1].depth_range[0] - 36.0).abs() < 1e-3);
        assert_eq!(fits[1].depth_range[1], 160.0);
        assert!((fits[2].depth_range[0] - 148.0).abs() < 1e-3);
        assert_eq!(fits[2].depth_range[1], 600.0);
        // Near cascades are sharper.
        assert!(fits[0].texel < fits[1].texel && fits[1].texel < fits[2].texel);
        assert!(fits[0].texel < 0.06, "{}", fits[0].texel);
    }

    #[test]
    fn cascade_size_does_not_change_when_the_camera_turns() {
        let (_, proj) = camera();
        let to_sun = Vec3::new(0.3, 0.6, 0.7).normalize();
        let eye = Vec3::new(3.3, 1.4, -8.0);
        let fit = |dir: Vec3| {
            fit_cascades(
                view::look_to_mat4(eye, dir, Vec3::Y),
                proj,
                to_sun,
                [40.0, 160.0, 600.0],
                0.1,
                2048,
                400.0,
            )
        };
        let base = fit(Vec3::NEG_Z);
        let texel_ndc = 2.0 / 2048.0;
        for k in 1..40 {
            let yaw = k as f32 * 0.161;
            let fits = fit(Vec3::new(-yaw.sin(), -0.1, -yaw.cos()));
            for (a, b) in base.iter().zip(&fits) {
                assert_eq!(a.radius, b.radius);
                // The world origin stays on the same sub-texel phase: no shimmer.
                let o = ndc(b.view_proj, Vec3::ZERO) / texel_ndc;
                assert!((o.x - o.x.round()).abs() < 2e-2, "{}", o.x);
                assert!((o.y - o.y.round()).abs() < 2e-2, "{}", o.y);
            }
        }
    }

    #[test]
    fn sphere_fits_inside_clip_volume() {
        let to_sun = Vec3::new(0.4, 0.8, 0.3).normalize();
        let center = Vec3::new(12.0, 3.0, -40.0);
        let r = 100.0;
        let m = sun_view_proj(to_sun, center, r, 50.0, 2048);
        let c = ndc(m, center);
        assert!(c.x.abs() < 2.0 / 2048.0 && c.y.abs() < 2.0 / 2048.0, "{c}");
        // Sample the sphere surface densely; everything must be inside the clip volume.
        for i in 0..64 {
            for j in 0..32 {
                let (th, ph) = (
                    i as f32 / 64.0 * std::f32::consts::TAU,
                    j as f32 / 31.0 * std::f32::consts::PI,
                );
                let d = Vec3::new(ph.sin() * th.cos(), ph.cos(), ph.sin() * th.sin());
                let p = ndc(m, center + d * (r * 0.999));
                assert!(p.x.abs() <= 1.0 + 2.0 / 2048.0 && p.y.abs() <= 1.0 + 2.0 / 2048.0);
                assert!((0.0..=1.0).contains(&p.z), "{p}");
            }
        }
    }

    #[test]
    fn closer_to_sun_is_shallower() {
        let to_sun = Vec3::new(-0.3, 0.9, 0.2).normalize();
        let m = sun_view_proj(to_sun, Vec3::ZERO, 50.0, 30.0, 1024);
        let low = ndc(m, Vec3::ZERO);
        let high = ndc(m, to_sun * 40.0);
        assert!(high.z < low.z);
        // A caster in the margin above the sphere is still in front of the near plane.
        assert!(ndc(m, to_sun * 75.0).z >= 0.0);
    }

    #[test]
    fn snapping_moves_the_map_in_whole_texels() {
        let to_sun = Vec3::new(0.5, 0.7, -0.5).normalize();
        let res = 2048;
        let r = 80.0;
        let texel_ndc = 2.0 / res as f32;
        for k in 0..50 {
            let center = Vec3::new(k as f32 * 0.37, 0.0, k as f32 * -0.21);
            let m = sun_view_proj(to_sun, center, r, 20.0, res);
            // A fixed world point must land on the same sub-texel phase for every center.
            let o = ndc(m, Vec3::ZERO) / texel_ndc;
            assert!((o.x - o.x.round()).abs() < 1e-2, "{}", o.x);
            assert!((o.y - o.y.round()).abs() < 1e-2, "{}", o.y);
        }
    }

    #[test]
    fn straight_down_sun_is_handled() {
        let m = sun_view_proj(Vec3::Y, Vec3::ZERO, 10.0, 0.0, 512);
        assert!(m.is_finite());
        assert!(ndc(m, Vec3::ZERO).z.is_finite());
    }
}
