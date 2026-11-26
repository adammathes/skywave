//! Map skins: how a world's map looks, in the style of the maps people used in its era. A
//! scenario names one (`map_skin`); the rasterizer ([`super::MapImage::rasterize_with`])
//! takes its colors, road styles and paper, and the HUD its frame and marker colors.
//!
//! - `plain`: the neutral map (the test town and any world that names none).
//! - `atlas`: a spiral-bound street atlas of the 80s and 90s. Cream paper with a little
//!   grain, no buildings, white streets, yellow main roads, red-orange freeways, a pale
//!   blue page grid, an oxblood cover for a rim.
//! - `transit`: a fold-out city map crossed with a transit diagram. Warm off-white ground,
//!   a bold lake, main roads as heavy red lines, freeways in blue, a black rim.
//! - `nav`: a mid-2000s in-dash navigation screen. Dark night-mode ground, bright roads with
//!   the route-colored main roads, no buildings, a silver bezel, a blue-white car arrow.
//!
//! All plain constants; original designs, no trademarks.

use place::RoadClass;

use super::{road_style, Rgb, RoadStyle, CHANNEL, PARK, SHORE, WATER, ZONE_COLORS};

/// One map style.
#[derive(Clone, Copy, Debug)]
pub struct Skin {
    pub id: &'static str,
    /// Ground tint per zone, in `ZoneId` order.
    pub zones: [Rgb; 12],
    pub water: Rgb,
    pub shore: Rgb,
    pub park: Rgb,
    pub channel: Rgb,
    /// Footprints darken the ground to this share out of 256; 256 leaves them out.
    pub building_shade: u32,
    pub road: fn(RoadClass) -> RoadStyle,
    /// Paper grain: each pixel brightened or darkened by up to this much.
    pub grain: u8,
    /// A grid line every this many meters (0: none), in `grid` at `grid_opacity`.
    pub grid_m: f32,
    pub grid: Rgb,
    pub grid_opacity: f32,
    /// The HUD's colors around the map: the minimap rim and full-map frame, the player
    /// arrow, the halo under it, and the ring round landmark dots. sRGBA.
    pub rim: [u8; 4],
    pub player: [u8; 4],
    pub halo: [u8; 4],
    pub poi_ring: [u8; 4],
    /// The full map's place names and the shadow under them.
    pub label: [u8; 4],
    pub label_shadow: [u8; 4],
}

/// Skins are told apart by id.
impl PartialEq for Skin {
    fn eq(&self, other: &Skin) -> bool {
        self.id == other.id
    }
}

impl Skin {
    /// The skin named `id`.
    pub fn named(id: &str) -> Option<&'static Skin> {
        SKINS.iter().copied().find(|s| s.id == id)
    }
}

/// Every skin.
pub const SKINS: [&Skin; 4] = [&PLAIN, &ATLAS, &TRANSIT, &NAV];

pub const PLAIN: Skin = Skin {
    id: "plain",
    zones: ZONE_COLORS,
    water: WATER,
    shore: SHORE,
    park: PARK,
    channel: CHANNEL,
    building_shade: 216,
    road: road_style,
    grain: 0,
    grid_m: 0.0,
    grid: [0; 3],
    grid_opacity: 0.0,
    rim: [16, 20, 30, 235],
    player: [255, 178, 62, 255],
    halo: [255, 255, 255, 150],
    poi_ring: [24, 16, 8, 255],
    label: [238, 234, 222, 255],
    label_shadow: [0, 0, 0, 170],
};

const fn road(fill: Rgb, edge: Rgb, min_px: f32) -> RoadStyle {
    RoadStyle {
        fill,
        edge,
        min_px,
        opacity: 1.0,
        outline: true,
    }
}

const fn alley(fill: Rgb, opacity: f32) -> RoadStyle {
    RoadStyle {
        fill,
        edge: [0; 3],
        min_px: 1.5,
        opacity,
        outline: false,
    }
}

/// Every zone the same ground but a few.
const fn zones(ground: Rgb, park: Rgb, sand: Rgb, hills: Rgb, water: Rgb) -> [Rgb; 12] {
    [
        ground, ground, ground, ground, ground, ground, park, sand, ground, hills, ground, water,
    ]
}

const ATLAS_PAPER: Rgb = [246, 240, 218];
const ATLAS_WATER: Rgb = [164, 204, 228];

fn atlas_road(class: RoadClass) -> RoadStyle {
    match class {
        RoadClass::Highway => road([232, 104, 72], [122, 46, 32], 9.0),
        RoadClass::Arterial => road([252, 214, 96], [140, 104, 40], 6.0),
        RoadClass::Collector | RoadClass::Beachfront => road([255, 250, 236], [150, 138, 112], 4.0),
        RoadClass::Local => road([255, 253, 246], [176, 164, 140], 3.0),
        RoadClass::Rural => road([255, 236, 190], [150, 128, 90], 3.0),
        RoadClass::Alley => alley([255, 252, 242], 0.7),
    }
}

pub const ATLAS: Skin = Skin {
    id: "atlas",
    zones: {
        let mut z = zones(
            ATLAS_PAPER,
            [196, 222, 170],
            [244, 232, 196],
            [232, 222, 194],
            ATLAS_WATER,
        );
        // Downtown a shade pinker, as atlases tint the business district.
        z[0] = [240, 222, 210];
        z
    },
    water: ATLAS_WATER,
    shore: [104, 154, 196],
    park: [196, 222, 170],
    channel: [214, 212, 200],
    building_shade: 256,
    road: atlas_road,
    grain: 5,
    grid_m: 400.0,
    grid: [120, 150, 200],
    grid_opacity: 0.35,
    rim: [92, 34, 30, 245],
    player: [218, 40, 40, 255],
    halo: [255, 255, 255, 170],
    poi_ring: [60, 40, 30, 255],
    label: [58, 38, 30, 255],
    label_shadow: [250, 244, 226, 220],
};

const TRANSIT_GROUND: Rgb = [238, 235, 226];
const TRANSIT_WATER: Rgb = [62, 128, 196];

fn transit_road(class: RoadClass) -> RoadStyle {
    match class {
        RoadClass::Highway => road([44, 92, 176], [250, 248, 240], 10.0),
        RoadClass::Arterial => road([206, 54, 42], [250, 248, 240], 7.0),
        RoadClass::Collector | RoadClass::Beachfront => road([128, 124, 118], [238, 235, 226], 3.0),
        RoadClass::Local => road([196, 190, 180], [238, 235, 226], 2.0),
        RoadClass::Rural => road([150, 140, 120], [238, 235, 226], 2.0),
        RoadClass::Alley => alley([214, 208, 198], 0.6),
    }
}

pub const TRANSIT: Skin = Skin {
    id: "transit",
    zones: {
        let mut z = zones(
            TRANSIT_GROUND,
            [168, 204, 140],
            [236, 222, 186],
            [222, 214, 196],
            TRANSIT_WATER,
        );
        z[0] = [224, 214, 204];
        z[4] = [222, 220, 214];
        z
    },
    water: TRANSIT_WATER,
    shore: [36, 88, 152],
    park: [168, 204, 140],
    channel: [204, 200, 192],
    building_shade: 240,
    road: transit_road,
    grain: 0,
    grid_m: 0.0,
    grid: [0; 3],
    grid_opacity: 0.0,
    rim: [22, 22, 24, 245],
    player: [250, 190, 30, 255],
    halo: [255, 255, 255, 170],
    poi_ring: [22, 22, 24, 255],
    label: [22, 22, 24, 255],
    label_shadow: [255, 255, 255, 220],
};

const NAV_GROUND: Rgb = [30, 38, 54];
const NAV_WATER: Rgb = [20, 56, 98];

fn nav_road(class: RoadClass) -> RoadStyle {
    match class {
        RoadClass::Highway => road([250, 150, 60], [12, 14, 20], 9.0),
        RoadClass::Arterial => road([246, 214, 104], [12, 14, 20], 6.0),
        RoadClass::Collector | RoadClass::Beachfront => road([196, 204, 216], [12, 14, 20], 4.0),
        RoadClass::Local => road([136, 148, 166], [18, 22, 32], 3.0),
        RoadClass::Rural => road([170, 160, 130], [18, 22, 32], 3.0),
        RoadClass::Alley => alley([90, 100, 118], 0.8),
    }
}

pub const NAV: Skin = Skin {
    id: "nav",
    zones: {
        let mut z = zones(
            NAV_GROUND,
            [30, 66, 48],
            [64, 62, 50],
            [44, 46, 52],
            NAV_WATER,
        );
        z[0] = [42, 46, 64];
        z
    },
    water: NAV_WATER,
    shore: [52, 104, 160],
    park: [30, 66, 48],
    channel: [52, 56, 66],
    building_shade: 256,
    road: nav_road,
    grain: 0,
    grid_m: 0.0,
    grid: [0; 3],
    grid_opacity: 0.0,
    rim: [148, 152, 160, 250],
    player: [120, 200, 255, 255],
    halo: [10, 14, 22, 170],
    poi_ring: [240, 240, 244, 255],
    label: [238, 240, 246, 255],
    label_shadow: [0, 0, 0, 200],
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn skins_are_found_by_id_and_unique() {
        for s in SKINS {
            assert_eq!(Skin::named(s.id).map(|n| n.id), Some(s.id));
            assert_eq!(SKINS.iter().filter(|o| o.id == s.id).count(), 1);
        }
        assert!(Skin::named("glossy").is_none());
    }
}
