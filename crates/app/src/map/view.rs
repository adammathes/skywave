//! Where the map lands on the HUD: the minimap circle (centered on the car, north-up or
//! turned with the heading) and the full-screen map (fit to the safe area, panned and
//! zoomed). Plain math over virtual HUD units; nothing here allocates.
//!
//! Screen axes are the HUD's: x right, y down. The map image has north (-z) up and east
//! (+x) right, so a world offset `[dx, dz]` is a screen offset `[dx, dz]` scaled, then
//! turned clockwise by the minimap's rotation. Headings are the sim's: radians, 0 faces
//! north, positive turns left.

use render::{HudImage, HudMask, HudTexture};

use super::MapImage;

/// Minimap diameter, virtual units.
pub const MINIMAP_DIAMETER: f32 = 200.0;
/// Minimap zoom settings: meters across the circle.
pub const MINIMAP_RANGES_M: [f32; 3] = [100.0, 150.0, 250.0];
/// The default zoom: 150 m across.
pub const DEFAULT_MINIMAP_RANGE: usize = 1;
/// The full map zooms from fitting the whole place (1) to this many times closer.
pub const FULL_MAP_MAX_ZOOM: f32 = 8.0;
/// Full-map pan speed at full deflection: this share of the visible width per second.
pub const PAN_VIEWS_PER_S: f32 = 0.6;
/// Full-map zoom speed at full deflection, doublings per second, and per wheel notch.
pub const ZOOM_OCTAVES_PER_S: f32 = 1.5;
pub const ZOOM_OCTAVES_PER_NOTCH: f32 = 0.25;

/// What the views need of the map image: where it lies in the world.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MapGeom {
    /// World `[x, z]` of the northwest corner.
    pub origin: [f32; 2],
    /// Meters covered along x and z.
    pub extent: [f32; 2],
}

impl MapGeom {
    pub fn of(image: &MapImage) -> MapGeom {
        MapGeom {
            origin: image.origin,
            extent: image.extent_m(),
        }
    }

    pub fn center(&self) -> [f32; 2] {
        [
            self.origin[0] + self.extent[0] * 0.5,
            self.origin[1] + self.extent[1] * 0.5,
        ]
    }

    /// Texture coordinates of the world point `p`.
    pub fn uv(&self, p: [f32; 2]) -> [f32; 2] {
        [
            (p[0] - self.origin[0]) / self.extent[0],
            (p[1] - self.origin[1]) / self.extent[1],
        ]
    }
}

/// Turns the screen vector `v` clockwise by `a` radians (y down), as the HUD shader does.
pub fn rotate_cw(v: [f32; 2], a: f32) -> [f32; 2] {
    let (s, c) = a.sin_cos();
    [v[0] * c - v[1] * s, v[0] * s + v[1] * c]
}

/// The sim's heading of a forward vector `[x, z]`.
pub fn heading_of(forward: [f32; 2]) -> f32 {
    (-forward[0]).atan2(-forward[1])
}

/// One frame's minimap.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Minimap {
    /// Center of the circle, virtual units.
    pub center: [f32; 2],
    /// Radius of the circle, virtual units.
    pub radius: f32,
    /// World point under the center (the car).
    pub focus: [f32; 2],
    /// The car's heading.
    pub heading: f32,
    /// Clockwise turn of the map content: the heading when it rotates with the car
    /// (so the car's forward points up), 0 north-up.
    pub rotation: f32,
    /// Meters across the circle.
    pub range_m: f32,
}

impl Minimap {
    pub fn new(
        center: [f32; 2],
        radius: f32,
        focus: [f32; 2],
        heading: f32,
        rotate: bool,
        range_m: f32,
    ) -> Minimap {
        Minimap {
            center,
            radius,
            focus,
            heading,
            rotation: if rotate { heading } else { 0.0 },
            range_m: range_m.max(1.0),
        }
    }

    /// Virtual units per meter.
    pub fn scale(&self) -> f32 {
        2.0 * self.radius / self.range_m
    }

    /// Where the world point `p` is drawn, virtual units.
    pub fn to_screen(&self, p: [f32; 2]) -> [f32; 2] {
        let s = self.scale();
        let d = rotate_cw(
            [(p[0] - self.focus[0]) * s, (p[1] - self.focus[1]) * s],
            self.rotation,
        );
        [self.center[0] + d[0], self.center[1] + d[1]]
    }

    /// Whether a screen point is inside the circle.
    pub fn contains(&self, q: [f32; 2]) -> bool {
        let (dx, dy) = (q[0] - self.center[0], q[1] - self.center[1]);
        dx * dx + dy * dy <= self.radius * self.radius
    }

    /// Screen direction (unit) of north.
    pub fn north(&self) -> [f32; 2] {
        rotate_cw([0.0, -1.0], self.rotation)
    }

    /// Clockwise rotation of the player arrow (which points up unrotated).
    pub fn arrow_rotation(&self) -> f32 {
        self.rotation - self.heading
    }

    /// The map quad: the circle, showing `range_m` around the focus, turned.
    pub fn image(&self, geom: &MapGeom) -> HudImage {
        let [u, v] = geom.uv(self.focus);
        let (hu, hv) = (
            self.range_m * 0.5 / geom.extent[0],
            self.range_m * 0.5 / geom.extent[1],
        );
        HudImage {
            x: self.center[0] - self.radius,
            y: self.center[1] - self.radius,
            w: 2.0 * self.radius,
            h: 2.0 * self.radius,
            uv: [u - hu, v - hv, u + hu, v + hv],
            rotation: self.rotation,
            texture: HudTexture::Map,
            mask: HudMask::Circle,
            color: [255; 4],
        }
    }
}

/// Virtual units per meter that fit `extent` (meters) inside `rect` (`[x, y, w, h]`).
pub fn fit_scale(extent: [f32; 2], rect: [f32; 4]) -> f32 {
    (rect[2] / extent[0].max(1.0)).min(rect[3] / extent[1].max(1.0))
}

/// The full-screen map's state: open or not, where it looks and how close.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FullMap {
    pub open: bool,
    /// World point at the middle of the view.
    pub center: [f32; 2],
    /// 1 fits the whole place; up to [`FULL_MAP_MAX_ZOOM`].
    pub zoom: f32,
}

impl Default for FullMap {
    fn default() -> FullMap {
        FullMap {
            open: false,
            center: [0.0; 2],
            zoom: 1.0,
        }
    }
}

/// The full map laid out for one frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FullMapView {
    /// The area it may use, virtual units `[x, y, w, h]` (the safe area).
    pub rect: [f32; 4],
    /// Virtual units per meter.
    pub scale: f32,
    /// World point at the middle of `rect`.
    pub center: [f32; 2],
}

impl FullMapView {
    pub fn to_screen(&self, p: [f32; 2]) -> [f32; 2] {
        [
            self.rect[0] + self.rect[2] * 0.5 + (p[0] - self.center[0]) * self.scale,
            self.rect[1] + self.rect[3] * 0.5 + (p[1] - self.center[1]) * self.scale,
        ]
    }

    pub fn to_world(&self, q: [f32; 2]) -> [f32; 2] {
        [
            self.center[0] + (q[0] - self.rect[0] - self.rect[2] * 0.5) / self.scale,
            self.center[1] + (q[1] - self.rect[1] - self.rect[3] * 0.5) / self.scale,
        ]
    }

    /// Whether a screen point is inside the view.
    pub fn contains(&self, q: [f32; 2]) -> bool {
        let r = self.rect;
        q[0] >= r[0] && q[1] >= r[1] && q[0] <= r[0] + r[2] && q[1] <= r[1] + r[3]
    }

    /// The map quad: the visible part of the place, `[x, y, w, h]` and its uv, or `None`
    /// when none of it is in view.
    pub fn image(&self, geom: &MapGeom, corner: f32) -> Option<HudImage> {
        let a = self.to_screen(geom.origin);
        let b = self.to_screen([
            geom.origin[0] + geom.extent[0],
            geom.origin[1] + geom.extent[1],
        ]);
        let r = self.rect;
        let (x0, y0) = (a[0].max(r[0]), a[1].max(r[1]));
        let (x1, y1) = (b[0].min(r[0] + r[2]), b[1].min(r[1] + r[3]));
        if x1 <= x0 || y1 <= y0 {
            return None;
        }
        let uv0 = geom.uv(self.to_world([x0, y0]));
        let uv1 = geom.uv(self.to_world([x1, y1]));
        Some(HudImage {
            x: x0,
            y: y0,
            w: x1 - x0,
            h: y1 - y0,
            uv: [uv0[0], uv0[1], uv1[0], uv1[1]],
            rotation: 0.0,
            texture: HudTexture::Map,
            mask: HudMask::Rounded(corner),
            color: [255; 4],
        })
    }
}

impl FullMap {
    /// Opens on the whole place.
    pub fn open(&mut self, geom: &MapGeom) {
        *self = FullMap {
            open: true,
            center: geom.center(),
            zoom: 1.0,
        };
    }

    pub fn close(&mut self) {
        self.open = false;
    }

    pub fn view(&self, geom: &MapGeom, rect: [f32; 4]) -> FullMapView {
        FullMapView {
            rect,
            scale: fit_scale(geom.extent, rect) * self.zoom,
            center: self.center,
        }
    }

    /// Pans by `dir` (each axis -1..1, screen directions) for `dt` seconds.
    pub fn pan(&mut self, dir: [f32; 2], dt: f32, geom: &MapGeom, rect: [f32; 4]) {
        let view = self.view(geom, rect);
        let step = PAN_VIEWS_PER_S * dt * rect[2] / view.scale;
        let clamp = |v: f32| {
            if v.is_finite() {
                v.clamp(-1.0, 1.0)
            } else {
                0.0
            }
        };
        self.center[0] += clamp(dir[0]) * step;
        self.center[1] += clamp(dir[1]) * step;
        self.clamp(geom, rect);
    }

    /// Zooms in by `octaves` doublings (negative zooms out), about the view's middle.
    pub fn zoom_by(&mut self, octaves: f32, geom: &MapGeom, rect: [f32; 4]) {
        if octaves.is_finite() {
            self.zoom = (self.zoom * octaves.exp2()).clamp(1.0, FULL_MAP_MAX_ZOOM);
        }
        self.clamp(geom, rect);
    }

    /// Keeps the view over the place: on an axis where the whole place fits, centered;
    /// otherwise with no space beyond the place's edges.
    pub fn clamp(&mut self, geom: &MapGeom, rect: [f32; 4]) {
        self.zoom = if self.zoom.is_finite() {
            self.zoom.clamp(1.0, FULL_MAP_MAX_ZOOM)
        } else {
            1.0
        };
        let scale = self.view(geom, rect).scale;
        let mid = geom.center();
        for k in 0..2 {
            let half_view = rect[2 + k] * 0.5 / scale;
            let half_place = geom.extent[k] * 0.5;
            let c = if self.center[k].is_finite() {
                self.center[k]
            } else {
                mid[k]
            };
            self.center[k] = if half_view >= half_place {
                mid[k]
            } else {
                c.clamp(
                    mid[k] - half_place + half_view,
                    mid[k] + half_place - half_view,
                )
            };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::f32::consts::{FRAC_PI_2, PI};
    use render::HudLayout;

    fn close(a: [f32; 2], b: [f32; 2]) -> bool {
        (a[0] - b[0]).abs() < 1e-3 && (a[1] - b[1]).abs() < 1e-3
    }

    const TOWN: MapGeom = MapGeom {
        origin: [-512.0, -512.0],
        extent: [1024.0, 1024.0],
    };

    #[test]
    fn minimap_turns_and_scales_the_world_around_the_car() {
        let car = [10.0, 20.0];
        // 150 m across a 200-unit circle: 4/3 units per meter.
        let s = 200.0 / 150.0;
        // (heading, rotate, world point, expected screen offset from the center)
        let east = -FRAC_PI_2;
        let cases: [(f32, bool, [f32; 2], [f32; 2]); 9] = [
            // North-up: the world as on the map, north up, east right.
            (east, false, [10.0, 0.0], [0.0, -20.0 * s]),
            (east, false, [40.0, 20.0], [30.0 * s, 0.0]),
            (1.0, false, [10.0, 50.0], [0.0, 30.0 * s]),
            // Heading-up, facing north: same as north-up.
            (0.0, true, [10.0, 0.0], [0.0, -20.0 * s]),
            // Facing east: ahead (east) is up, north is to the left.
            (east, true, [40.0, 20.0], [0.0, -30.0 * s]),
            (east, true, [10.0, 0.0], [-20.0 * s, 0.0]),
            // Facing west: ahead (west) is up, north is to the right.
            (FRAC_PI_2, true, [-20.0, 20.0], [0.0, -30.0 * s]),
            (FRAC_PI_2, true, [10.0, 0.0], [20.0 * s, 0.0]),
            // Facing south: north is down.
            (PI, true, [10.0, 0.0], [0.0, 20.0 * s]),
        ];
        for (heading, rotate, p, want) in cases {
            let m = Minimap::new([100.0, 600.0], 100.0, car, heading, rotate, 150.0);
            let q = m.to_screen(p);
            assert!(
                close([q[0] - 100.0, q[1] - 600.0], want),
                "heading {heading} rotate {rotate} {p:?}: {q:?}"
            );
            // The car stays at the center; the arrow points along the heading.
            assert!(close(m.to_screen(car), m.center));
            let ahead = [car[0] - heading.sin() * 30.0, car[1] - heading.cos() * 30.0];
            let d = m.to_screen(ahead);
            let dir = rotate_cw([0.0, -1.0], m.arrow_rotation());
            let (dx, dy) = ((d[0] - 100.0) / (30.0 * s), (d[1] - 600.0) / (30.0 * s));
            assert!(close([dx, dy], dir), "arrow {heading} {rotate}");
            // North on the screen is where the north of the world went.
            let n = m.to_screen([car[0], car[1] - 30.0]);
            let nd = [(n[0] - 100.0) / (30.0 * s), (n[1] - 600.0) / (30.0 * s)];
            assert!(close(nd, m.north()));
        }
        // Heading-up keeps the arrow pointing up.
        let m = Minimap::new([0.0; 2], 100.0, car, 0.7, true, 150.0);
        assert_eq!(m.arrow_rotation(), 0.0);
    }

    #[test]
    fn minimap_zoom_and_mask() {
        for (range, s) in [(100.0, 2.0), (150.0, 200.0 / 150.0), (250.0, 0.8)] {
            let m = Minimap::new([100.0, 600.0], 100.0, [0.0; 2], 0.0, false, range);
            assert!((m.scale() - s).abs() < 1e-6);
            // The circle's rim is half the range away.
            let rim = m.to_screen([range * 0.5, 0.0]);
            assert!(close(rim, [200.0, 600.0]));
            assert!(m.contains(rim) && !m.contains([200.1, 600.0]));
            let img = m.image(&TOWN);
            assert_eq!((img.x, img.y, img.w, img.h), (0.0, 500.0, 200.0, 200.0));
            assert_eq!(img.mask, HudMask::Circle);
            let half = range * 0.5 / 1024.0;
            assert!(close([img.uv[0], img.uv[1]], [0.5 - half, 0.5 - half]));
            assert!(close([img.uv[2], img.uv[3]], [0.5 + half, 0.5 + half]));
        }
        let m = Minimap::new([0.0; 2], 100.0, [0.0; 2], 0.3, true, 150.0);
        assert_eq!(m.image(&TOWN).rotation, 0.3);
        assert_eq!(heading_of([0.0, -1.0]), 0.0);
        assert!((heading_of([1.0, 0.0]) + FRAC_PI_2).abs() < 1e-6);
    }

    #[test]
    fn full_map_fits_every_aspect_inside_the_safe_area() {
        let mut full = FullMap::default();
        full.open(&TOWN);
        // 16:9, 16:10 (Deck) and 4:3: the square town fills the safe height of the 16:9 box
        // and stays square on screen.
        for target in [(1280, 720), (1280, 800), (1024, 768), (1920, 1080)] {
            for ui in [1.0, 1.5, 2.0] {
                let layout = HudLayout::new(target, ui);
                let safe = HudLayout::safe_rect(ui);
                let view = full.view(&TOWN, safe);
                let img = view.image(&TOWN, 8.0).expect("in view");
                let a = layout.to_pixels(img.x, img.y);
                let b = layout.to_pixels(img.x + img.w, img.y + img.h);
                let [bx, by, bw, bh] = layout.viewport;
                assert!((b[1] - a[1] - 0.94 * bh).abs() < 0.01, "{target:?}");
                assert!(
                    ((b[0] - a[0]) - (b[1] - a[1])).abs() < 0.01,
                    "square {target:?}"
                );
                assert!(a[0] >= bx + 0.03 * bw - 0.01 && b[0] <= bx + 0.97 * bw + 0.01);
                assert!(a[1] >= by + 0.03 * bh - 0.01 && b[1] <= by + 0.97 * bh + 0.01);
                assert!(close([(a[0] + b[0]) * 0.5, 0.0], [bx + bw * 0.5, 0.0]));
                assert!(close([img.uv[0], img.uv[1]], [0.0, 0.0]));
                assert!(close([img.uv[2], img.uv[3]], [1.0, 1.0]));
            }
        }
        // A wide place is limited by the width instead.
        let wide = MapGeom {
            origin: [0.0; 2],
            extent: [4000.0, 1000.0],
        };
        let s = fit_scale(wide.extent, [0.0, 0.0, 1203.2, 676.8]);
        assert!((s - 1203.2 / 4000.0).abs() < 1e-6);
    }

    #[test]
    fn full_map_pan_and_zoom_are_clamped() {
        let safe = HudLayout::safe_rect(1.0);
        let mut full = FullMap::default();
        full.open(&TOWN);
        assert_eq!((full.center, full.zoom), ([0.0, 0.0], 1.0));
        // At zoom 1 the whole town is in view: panning does nothing.
        full.pan([1.0, 1.0], 1.0, &TOWN, safe);
        assert_eq!(full.center, [0.0, 0.0]);
        // Zoom limits.
        full.zoom_by(-3.0, &TOWN, safe);
        assert_eq!(full.zoom, 1.0);
        full.zoom_by(10.0, &TOWN, safe);
        assert_eq!(full.zoom, FULL_MAP_MAX_ZOOM);
        full.zoom_by(-1.0, &TOWN, safe);
        assert_eq!(full.zoom, 4.0);
        // At 4× the view is 256 m tall: the center can go to 128 m from the edge, no
        // further; across, the view is wider (16:9), so less room.
        for _ in 0..100 {
            full.pan([-1.0, 1.0], 0.5, &TOWN, safe);
        }
        let view = full.view(&TOWN, safe);
        let half_w = safe[2] * 0.5 / view.scale;
        assert!(close(full.center, [-512.0 + half_w, 512.0 - 128.0]));
        // The view never shows past the town's corner.
        let img = view.image(&TOWN, 0.0).unwrap();
        assert!(close([img.uv[0], img.uv[3]], [0.0, 1.0]));
        assert!(close([img.x, img.y + img.h], [safe[0], safe[1] + safe[3]]));
        // Zooming back out recenters.
        full.zoom_by(-5.0, &TOWN, safe);
        assert_eq!((full.center, full.zoom), ([0.0, 0.0], 1.0));
        // Bad input is ignored.
        full.pan([f32::NAN, 0.0], 1.0, &TOWN, safe);
        full.zoom_by(f32::INFINITY, &TOWN, safe);
        assert_eq!((full.center, full.zoom), ([0.0, 0.0], 1.0));
        // World and screen round-trip.
        let v = full.view(&TOWN, safe);
        assert!(close(
            v.to_world(v.to_screen([100.0, -40.0])),
            [100.0, -40.0]
        ));
    }
}
