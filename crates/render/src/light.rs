//! Fitting the sun's orthographic shadow projection.

use glam::camera::rh::{proj::directx, view};
use glam::{Mat4, Vec3, Vec4Swizzles};

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

#[cfg(test)]
mod tests {
    use super::*;

    fn ndc(m: Mat4, p: Vec3) -> Vec3 {
        m.project_point3(p)
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
