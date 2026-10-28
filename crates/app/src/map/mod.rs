//! The map: the Place rasterized once at load into an RGBA8 image ([`MapImage`]) that the
//! HUD draws as the minimap and the full-screen map ([`view`]), plus invented names for the
//! points of interest ([`names`]).
//!
//! The image covers the whole place, north (-z) up and east (+x) right, [`MAP_SIZE`] pixels
//! along the longer side (the 1 km test town is ~1 m per pixel, a 2 km place 2 m). Layers,
//! bottom to top: a muted tint per zone, parks, water (with a darker shoreline), faint
//! building footprints, every road's 1 px darker outline, road fills by class (alleys
//! faint, locals thin, highways thick and light), and a dot per point of interest. Roads
//! and dots are anti-aliased by coverage; fills are scanline polygons sampled at pixel
//! centers. Everything is plain f32 arithmetic in a fixed order (no `mul_add`, no hashing
//! order), so the same Place gives bit-identical pixels on every platform.

pub mod names;
pub mod view;

use place::{LotKind, Place, PoiKind, RoadClass, ZoneId, Zones};
use skywave_core::hash_bytes;

/// Pixels along the longer side of the place.
pub const MAP_SIZE: u32 = 1024;

/// The rasterized place.
#[derive(Clone, Debug, PartialEq)]
pub struct MapImage {
    pub w: u32,
    pub h: u32,
    /// `w * h` sRGB RGBA8 pixels, row-major from the northwest corner, opaque.
    pub rgba: Vec<u8>,
    pub meters_per_px: f32,
    /// World `[x, z]` of the image's top-left (northwest) corner.
    pub origin: [f32; 2],
}

/// Colors, sRGB.
type Rgb = [u8; 3];

/// Muted ground tint per zone, in [`ZoneId`] order.
pub const ZONE_COLORS: [Rgb; 12] = [
    [176, 170, 172], // Downtown
    [204, 184, 160], // Strip
    [186, 196, 166], // Residential
    [194, 200, 172], // Suburb
    [170, 166, 158], // Industrial
    [196, 186, 170], // Commercial
    [140, 180, 120], // Park
    [230, 216, 176], // Beach
    [204, 198, 146], // Farmland
    [178, 170, 136], // Hills
    [184, 184, 182], // Highway
    WATER,           // Water
];
pub const WATER: Rgb = [104, 150, 190];
pub const SHORE: Rgb = [74, 112, 150];
pub const PARK: Rgb = [140, 180, 120];
/// A dry flood channel's concrete.
pub const CHANNEL: Rgb = [176, 174, 166];
/// Footprints darken the ground under them to this share (out of 256).
const BUILDING_SHADE: u32 = 216;

/// How one road class is drawn: fill, outline, the narrowest it is drawn (px) whatever its
/// real width, and the fill's opacity (alleys are faint and have no outline).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RoadStyle {
    pub fill: Rgb,
    pub edge: Rgb,
    pub min_px: f32,
    pub opacity: f32,
    pub outline: bool,
}

/// The style of a road class; classes are drawn in [`ROAD_ORDER`].
pub fn road_style(class: RoadClass) -> RoadStyle {
    let style = |fill, edge, min_px| RoadStyle {
        fill,
        edge,
        min_px,
        opacity: 1.0,
        outline: true,
    };
    match class {
        RoadClass::Highway => style([238, 238, 234], [62, 62, 70], 7.0),
        RoadClass::Arterial => style([252, 238, 196], [96, 88, 72], 5.0),
        RoadClass::Collector | RoadClass::Beachfront => {
            style([250, 250, 246], [104, 104, 108], 4.0)
        }
        RoadClass::Local => style([246, 246, 242], [124, 124, 126], 3.0),
        RoadClass::Rural => style([236, 226, 204], [124, 116, 100], 3.0),
        RoadClass::Alley => RoadStyle {
            fill: [240, 238, 232],
            edge: [0; 3],
            min_px: 1.5,
            opacity: 0.6,
            outline: false,
        },
    }
}

/// Bottom to top.
pub const ROAD_ORDER: [RoadClass; 7] = [
    RoadClass::Alley,
    RoadClass::Rural,
    RoadClass::Local,
    RoadClass::Beachfront,
    RoadClass::Collector,
    RoadClass::Arterial,
    RoadClass::Highway,
];

/// The dot color of a point of interest.
pub fn poi_color(kind: PoiKind) -> Rgb {
    match kind {
        PoiKind::Pier | PoiKind::Marina => [40, 110, 190],
        PoiKind::Lighthouse | PoiKind::Overlook => [120, 90, 170],
        PoiKind::Diner => [222, 84, 60],
        PoiKind::Gas => [232, 160, 30],
        PoiKind::Stadium => [60, 150, 80],
    }
}
/// A point of interest as the full map labels it.
#[derive(Clone, Debug, PartialEq)]
pub struct MapPoi {
    pub pos: [f32; 2],
    pub kind: PoiKind,
    /// Invented from the Poi's `name_seed` ([`names`]).
    pub name: String,
}

/// The full map's labels: every point of interest with its invented name.
pub fn labels(place: &Place) -> Vec<MapPoi> {
    place
        .pois
        .iter()
        .map(|p| MapPoi {
            pos: p.pos,
            kind: p.kind,
            name: names::poi_name(p.kind, p.name_seed),
        })
        .collect()
}

/// Radius of a point of interest's dot, px (plus a 1 px ring).
const POI_DOT_PX: f32 = 3.0;

impl MapImage {
    /// Rasterizes `place` (see the module docs). Allocates the image and a few scratch
    /// buffers; meant for load time.
    pub fn rasterize(place: &Place) -> MapImage {
        let extent = place.header.extent_m();
        let longest = extent[0].max(extent[1]).max(1.0);
        let meters_per_px = longest / MAP_SIZE as f32;
        let size = |e: f32| ((e / meters_per_px).round() as u32).clamp(1, MAP_SIZE);
        let mut c = Canvas {
            w: size(extent[0]),
            h: size(extent[1]),
            px: Vec::new(),
            water: Vec::new(),
            origin: place.header.origin_m(),
            mpp: meters_per_px,
            spans: Vec::new(),
        };
        c.zones(place);
        for p in &place.parcels {
            if p.lot_kind == LotKind::Park || p.zone == ZoneId::Park {
                c.polygon(&p.polygon, |_| PARK);
            }
        }
        for water in &place.terrain.water {
            // A dry flood channel is concrete, not water.
            if water.kind == place::WaterKind::Channel {
                c.polygon(&water.polygon, |_| CHANNEL);
                continue;
            }
            c.polygon(&water.polygon, |_| WATER);
            c.mark_water(&water.polygon);
        }
        c.shore();
        for b in &place.buildings {
            c.polygon(&b.footprint, shade);
        }
        c.roads(place);
        for poi in &place.pois {
            let [x, y] = c.to_px(poi.pos);
            c.disc(x, y, POI_DOT_PX + 1.0, [40, 40, 44], 1.0);
            c.disc(x, y, POI_DOT_PX, poi_color(poi.kind), 1.0);
        }
        let mut rgba = Vec::with_capacity(c.px.len() * 4);
        for p in &c.px {
            rgba.extend_from_slice(&[p[0], p[1], p[2], 255]);
        }
        MapImage {
            w: c.w,
            h: c.h,
            rgba,
            meters_per_px,
            origin: c.origin,
        }
    }

    /// Size of the area the image covers, meters `[x, z]`.
    pub fn extent_m(&self) -> [f32; 2] {
        [
            self.w as f32 * self.meters_per_px,
            self.h as f32 * self.meters_per_px,
        ]
    }

    /// Texture coordinates (0..1 over the image) of the world point `p = [x, z]`.
    pub fn uv(&self, p: [f32; 2]) -> [f32; 2] {
        let e = self.extent_m();
        [
            (p[0] - self.origin[0]) / e[0],
            (p[1] - self.origin[1]) / e[1],
        ]
    }

    /// The pixel at `(x, y)`, or `None` outside the image.
    pub fn pixel(&self, x: u32, y: u32) -> Option<[u8; 4]> {
        (x < self.w && y < self.h).then(|| {
            let i = ((y * self.w + x) * 4) as usize;
            [
                self.rgba[i],
                self.rgba[i + 1],
                self.rgba[i + 2],
                self.rgba[i + 3],
            ]
        })
    }

    /// The pixel under the world point `p`.
    pub fn pixel_at(&self, p: [f32; 2]) -> Option<[u8; 4]> {
        let x = (p[0] - self.origin[0]) / self.meters_per_px;
        let y = (p[1] - self.origin[1]) / self.meters_per_px;
        if x >= 0.0 && y >= 0.0 {
            self.pixel(x as u32, y as u32)
        } else {
            None
        }
    }

    /// Fingerprint of the size and pixels, for determinism pins.
    pub fn hash(&self) -> u64 {
        let head = hash_bytes(
            0x4D41_5049_4D47,
            &[self.w.to_le_bytes(), self.h.to_le_bytes()].concat(),
        );
        hash_bytes(head, &self.rgba)
    }
}

/// The ground under a footprint, darkened.
fn shade(under: Rgb) -> Rgb {
    under.map(|c| ((u32::from(c) * BUILDING_SHADE) >> 8) as u8)
}

/// `a` toward `b` by `t` (0..1), rounded.
fn blend(a: Rgb, b: Rgb, t: f32) -> Rgb {
    let mut out = a;
    for k in 0..3 {
        let (x, y) = (f32::from(a[k]), f32::from(b[k]));
        out[k] = (x + (y - x) * t).round().clamp(0.0, 255.0) as u8;
    }
    out
}

/// The image being drawn, in pixel coordinates (`(0, 0)` is the top-left pixel's corner).
struct Canvas {
    w: u32,
    h: u32,
    px: Vec<Rgb>,
    /// Pixels that are water (zone or body), for the shoreline.
    water: Vec<bool>,
    origin: [f32; 2],
    mpp: f32,
    /// Scanline crossings, reused.
    spans: Vec<f32>,
}

impl Canvas {
    fn to_px(&self, p: [f32; 2]) -> [f32; 2] {
        [
            (p[0] - self.origin[0]) / self.mpp,
            (p[1] - self.origin[1]) / self.mpp,
        ]
    }

    fn index(&self, x: u32, y: u32) -> usize {
        (y * self.w + x) as usize
    }

    /// The zone tint under every pixel center.
    fn zones(&mut self, place: &Place) {
        let header = &place.header;
        let res = place.terrain.res_per_chunk.max(1);
        let [cw, ch] = Zones::cells_per_side(header, res);
        let cells = &place.zones.cells;
        let cell_m = header.chunk_size_m / f32::from(res);
        let po = header.origin_m();
        let n = (self.w * self.h) as usize;
        self.px = vec![ZONE_COLORS[ZoneId::Residential as usize]; n];
        self.water = vec![false; n];
        if cells.len() != cw * ch {
            return;
        }
        for y in 0..self.h {
            let wz = self.origin[1] + (y as f32 + 0.5) * self.mpp;
            let cz = ((wz - po[1]) / cell_m).max(0.0) as usize;
            for x in 0..self.w {
                let wx = self.origin[0] + (x as f32 + 0.5) * self.mpp;
                let cx = ((wx - po[0]) / cell_m).max(0.0) as usize;
                let zone = cells[cz.min(ch - 1) * cw + cx.min(cw - 1)];
                let i = self.index(x, y);
                self.px[i] = ZONE_COLORS[zone as usize];
                self.water[i] = zone == ZoneId::Water;
            }
        }
    }

    /// Calls `f(row, x0, x1)` for every pixel run whose centers are inside `poly`
    /// (even-odd), world coordinates.
    fn scan(&mut self, poly: &[[f32; 2]], mut f: impl FnMut(&mut Canvas, u32, u32, u32)) {
        if poly.len() < 3 {
            return;
        }
        let pts: Vec<[f32; 2]> = poly.iter().map(|&p| self.to_px(p)).collect();
        let (mut lo, mut hi) = (f32::MAX, f32::MIN);
        for p in &pts {
            lo = lo.min(p[1]);
            hi = hi.max(p[1]);
        }
        let y0 = (lo - 0.5).ceil().max(0.0) as u32;
        let y1 = ((hi - 0.5).floor().min(self.h as f32 - 1.0)).max(-1.0);
        if y1 < 0.0 {
            return;
        }
        let mut spans = std::mem::take(&mut self.spans);
        for y in y0..=y1 as u32 {
            let cy = y as f32 + 0.5;
            spans.clear();
            for i in 0..pts.len() {
                let (a, b) = (pts[i], pts[(i + 1) % pts.len()]);
                if (a[1] > cy) != (b[1] > cy) {
                    spans.push(a[0] + (cy - a[1]) / (b[1] - a[1]) * (b[0] - a[0]));
                }
            }
            spans.sort_by(f32::total_cmp);
            for pair in spans.chunks_exact(2) {
                // Pixels whose centers x + 0.5 lie in [pair[0], pair[1]).
                let x0 = (pair[0] - 0.5).ceil().max(0.0);
                let x1 = (pair[1] - 0.5).ceil().min(self.w as f32);
                if x1 > x0 {
                    f(self, y, x0 as u32, x1 as u32);
                }
            }
        }
        self.spans = spans;
    }

    /// Fills `poly` with `color(color under it)`.
    fn polygon(&mut self, poly: &[[f32; 2]], color: impl Fn(Rgb) -> Rgb) {
        self.scan(poly, |c, y, x0, x1| {
            for x in x0..x1 {
                let i = c.index(x, y);
                c.px[i] = color(c.px[i]);
            }
        });
    }

    fn mark_water(&mut self, poly: &[[f32; 2]]) {
        self.scan(poly, |c, y, x0, x1| {
            for x in x0..x1 {
                let i = c.index(x, y);
                c.water[i] = true;
            }
        });
    }

    /// Water pixels next to land become the shoreline color.
    fn shore(&mut self) {
        for y in 0..self.h {
            for x in 0..self.w {
                let i = self.index(x, y);
                if !self.water[i] {
                    continue;
                }
                let land = |dx: i32, dy: i32| {
                    let (nx, ny) = (x as i32 + dx, y as i32 + dy);
                    nx >= 0
                        && ny >= 0
                        && (nx as u32) < self.w
                        && (ny as u32) < self.h
                        && !self.water[self.index(nx as u32, ny as u32)]
                };
                if land(-1, 0) || land(1, 0) || land(0, -1) || land(0, 1) {
                    self.px[i] = SHORE;
                }
            }
        }
    }

    /// Every road: all outlines first (so crossings stay open), then the fills by class.
    fn roads(&mut self, place: &Place) {
        for pass in 0..2 {
            for class in ROAD_ORDER {
                let style = road_style(class);
                if pass == 0 && !style.outline {
                    continue;
                }
                for e in place.roads.edges.iter().filter(|e| e.class == class) {
                    let r = (e.width_m / self.mpp).max(style.min_px) * 0.5;
                    let (r, color, opacity) = if pass == 0 {
                        (r + 1.0, style.edge, 1.0)
                    } else {
                        (r, style.fill, style.opacity)
                    };
                    for seg in e.profile.windows(2) {
                        let a = self.to_px([seg[0][0], seg[0][2]]);
                        let b = self.to_px([seg[1][0], seg[1][2]]);
                        self.capsule(a, b, r, color, opacity);
                    }
                }
            }
        }
    }

    /// An anti-aliased segment `a b` of radius `r` with round caps, pixel coordinates.
    fn capsule(&mut self, a: [f32; 2], b: [f32; 2], r: f32, color: Rgb, opacity: f32) {
        let pad = r + 1.0;
        let x0 = (a[0].min(b[0]) - pad).floor().max(0.0) as u32;
        let y0 = (a[1].min(b[1]) - pad).floor().max(0.0) as u32;
        let x1 = (a[0].max(b[0]) + pad).ceil().min(self.w as f32);
        let y1 = (a[1].max(b[1]) + pad).ceil().min(self.h as f32);
        if x1 <= x0 as f32 || y1 <= y0 as f32 {
            return;
        }
        let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
        let len2 = dx * dx + dy * dy;
        for y in y0..y1 as u32 {
            for x in x0..x1 as u32 {
                let (px, py) = (x as f32 + 0.5 - a[0], y as f32 + 0.5 - a[1]);
                let t = if len2 > 0.0 {
                    ((px * dx + py * dy) / len2).clamp(0.0, 1.0)
                } else {
                    0.0
                };
                let (ex, ey) = (px - t * dx, py - t * dy);
                let d = (ex * ex + ey * ey).sqrt();
                let cover = (r + 0.5 - d).clamp(0.0, 1.0) * opacity;
                if cover > 0.0 {
                    let i = self.index(x, y);
                    self.px[i] = blend(self.px[i], color, cover);
                }
            }
        }
    }

    fn disc(&mut self, x: f32, y: f32, r: f32, color: Rgb, opacity: f32) {
        self.capsule([x, y], [x, y], r, color, opacity);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use place::{Poi, WaterBody, WaterKind};

    fn rgb(p: Option<[u8; 4]>) -> Rgb {
        let p = p.expect("inside the map");
        [p[0], p[1], p[2]]
    }

    #[test]
    fn the_test_town_map_is_deterministic_and_pinned() {
        let place = Place::test_grid();
        let a = MapImage::rasterize(&place);
        let b = MapImage::rasterize(&place);
        assert_eq!(a, b);
        // 1024 m square town: 1024 px, 1 m per pixel, origin at its northwest corner.
        assert_eq!((a.w, a.h), (MAP_SIZE, MAP_SIZE));
        assert_eq!(a.meters_per_px, 1.0);
        assert_eq!(a.origin, [-512.0, -512.0]);
        assert_eq!(a.rgba.len(), (a.w * a.h * 4) as usize);
        assert!(a.rgba.chunks_exact(4).all(|p| p[3] == 255));
        assert_eq!(a.hash(), 0x8a97_cf1e_db61_56ae, "map image hash");
    }

    #[test]
    fn roads_lawns_parks_and_buildings_have_their_colors() {
        let place = Place::test_grid();
        let map = MapImage::rasterize(&place);
        // The north-south arterial on x = 0 and a local street on x = -300, mid-block.
        assert_eq!(
            rgb(map.pixel_at([0.5, -350.5])),
            road_style(RoadClass::Arterial).fill
        );
        assert_eq!(
            rgb(map.pixel_at([-299.5, -350.5])),
            road_style(RoadClass::Local).fill
        );
        // 14 m arterial at 1 m/px: fill out to 7 m, the outline just past it.
        assert_eq!(
            rgb(map.pixel_at([6.5, -350.5])),
            road_style(RoadClass::Arterial).fill
        );
        assert_eq!(
            rgb(map.pixel_at([7.5, -350.5])),
            road_style(RoadClass::Arterial).edge
        );
        // Lawn outside the town: the residential tint.
        let lawn = ZONE_COLORS[ZoneId::Residential as usize];
        assert_eq!(rgb(map.pixel_at([-480.5, -480.5])), lawn);
        // A building footprint is the ground darkened.
        let b = &place.buildings[0];
        let c = centroid(&b.footprint);
        let under = rgb(map.pixel_at(c));
        assert_ne!(under, lawn);
        assert!(ZONE_COLORS
            .iter()
            .any(|&z| shade(z) == under || shade(PARK) == under));
        // The park is green.
        let park = place
            .parcels
            .iter()
            .find(|p| p.lot_kind == LotKind::Park)
            .expect("the test town has a park");
        let inside = (0..park.polygon.len())
            .map(|i| {
                let (a, b) = (park.polygon[i], park.polygon[(i + 2) % park.polygon.len()]);
                [a[0] + (b[0] - a[0]) * 0.1, a[1] + (b[1] - a[1]) * 0.1]
            })
            .find(|&p| {
                !place
                    .buildings
                    .iter()
                    .any(|b| place::validate::point_in_polygon(p, &b.footprint))
            })
            .expect("open ground in the park");
        assert_eq!(rgb(map.pixel_at(inside)), PARK);
    }

    fn centroid(poly: &[[f32; 2]]) -> [f32; 2] {
        let n = poly.len() as f32;
        let s = poly
            .iter()
            .fold([0.0, 0.0], |s, p| [s[0] + p[0], s[1] + p[1]]);
        [s[0] / n, s[1] / n]
    }

    #[test]
    fn a_dry_channel_is_concrete_without_a_shore() {
        let mut place = Place::test_grid();
        let (x0, z0, s) = (-500.0, -500.0, 60.0);
        place.terrain.water.push(WaterBody {
            polygon: vec![[x0, z0], [x0, z0 + s], [x0 + s, z0 + s], [x0 + s, z0]],
            kind: WaterKind::Channel,
        });
        let map = MapImage::rasterize(&place);
        assert_eq!(rgb(map.pixel_at([-470.5, -470.5])), CHANNEL);
        assert_eq!(rgb(map.pixel_at([-470.5, -499.5])), CHANNEL);
    }

    #[test]
    fn a_lake_and_a_poi_are_drawn() {
        let mut place = Place::test_grid();
        // A 60 m square lake in the empty lawn of the northwest corner.
        let (x0, z0, s) = (-500.0, -500.0, 60.0);
        place.terrain.water.push(WaterBody {
            polygon: vec![[x0, z0], [x0, z0 + s], [x0 + s, z0 + s], [x0 + s, z0]],
            kind: WaterKind::Lake,
        });
        place.pois.push(Poi {
            pos: [-480.0, -400.0],
            kind: PoiKind::Diner,
            name_seed: 7,
        });
        let map = MapImage::rasterize(&place);
        assert_eq!(rgb(map.pixel_at([-470.5, -470.5])), WATER);
        // Its edge rows are the shoreline; just outside is lawn.
        assert_eq!(rgb(map.pixel_at([-470.5, -499.5])), SHORE);
        assert_eq!(rgb(map.pixel_at([-470.5, -440.5])), SHORE);
        let lawn = ZONE_COLORS[ZoneId::Residential as usize];
        assert_eq!(rgb(map.pixel_at([-470.5, -439.5])), lawn);
        assert_eq!(
            rgb(map.pixel_at([-479.5, -399.5])),
            poi_color(PoiKind::Diner)
        );
        assert_ne!(map.hash(), MapImage::rasterize(&Place::test_grid()).hash());
    }

    #[test]
    fn uv_maps_the_corners_and_pixels_round_trip() {
        let map = MapImage::rasterize(&Place::test_grid());
        assert_eq!(map.uv([-512.0, -512.0]), [0.0, 0.0]);
        assert_eq!(map.uv([512.0, 512.0]), [1.0, 1.0]);
        assert_eq!(map.uv([0.0, 0.0]), [0.5, 0.5]);
        assert_eq!(map.pixel_at([-600.0, 0.0]), None);
        assert_eq!(map.pixel(MAP_SIZE, 0), None);
    }
}
