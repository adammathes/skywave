//! Renders a generated macro map to a PNG: hillshaded zone colors, water shaded by depth,
//! 20 m contours, and markers for the downtown seed, strip centerline, highway, pass and
//! points of interest. One pixel per 4 m height sample, times `--scale`.
//!
//! With `--roads`, also generates the arterial network and highway (stage 3) and draws its
//! edges by class on top, nodes as dots.
//!
//! Usage: `cargo run -p mapdump -- --seed 1 --template assets/regions/california_coast.ron
//! --out map.png [--size 16x16] [--scale 1] [--roads]`

mod png;

use std::time::Instant;

use place::{RoadClass, RoadGraph, ZoneId};
use skywave_core::Seed;
use world::procgen::{arterials, hash_graph, MacroMap, MacroPoiKind};
use world::template::RegionTemplate;

/// Zone colors by `ZoneId` discriminant (Water is shaded separately).
const ZONE_RGB: [[u8; 3]; 12] = [
    [205, 70, 70],   // Downtown: brick red
    [255, 60, 200],  // Strip: neon magenta
    [235, 215, 145], // Residential: pale yellow
    [185, 225, 165], // Suburb: mint
    [150, 140, 170], // Industrial: slate violet
    [240, 145, 60],  // Commercial: orange
    [40, 135, 60],   // Park: dark green
    [250, 238, 190], // Beach: sand
    [130, 175, 50],  // Farmland: crop green
    [170, 140, 105], // Hills: dry brown
    [70, 70, 80],    // Highway: asphalt
    [40, 90, 170],   // Water
];

const ZONE_NAMES: [&str; 12] = [
    "Downtown",
    "Strip",
    "Residential",
    "Suburb",
    "Industrial",
    "Commercial",
    "Park",
    "Beach",
    "Farmland",
    "Hills",
    "Highway",
    "Water",
];

pub fn load(text: &str) -> Result<RegionTemplate, String> {
    let t: RegionTemplate = ron::from_str(text).map_err(|e| format!("region template: {e}"))?;
    t.validate()
        .map_err(|e| format!("region template {}: {e}", t.name))?;
    Ok(t)
}

struct Image {
    w: usize,
    h: usize,
    rgb: Vec<u8>,
}

impl Image {
    fn put(&mut self, x: i64, y: i64, c: [u8; 3]) {
        if x >= 0 && y >= 0 && (x as usize) < self.w && (y as usize) < self.h {
            let i = (y as usize * self.w + x as usize) * 3;
            self.rgb[i..i + 3].copy_from_slice(&c);
        }
    }

    fn disc(&mut self, cx: f32, cy: f32, r: f32, c: [u8; 3]) {
        let ri = r.ceil() as i64;
        for dy in -ri..=ri {
            for dx in -ri..=ri {
                if ((dx * dx + dy * dy) as f32) <= r * r {
                    self.put(cx as i64 + dx, cy as i64 + dy, c);
                }
            }
        }
    }

    fn line(&mut self, a: [f32; 2], b: [f32; 2], width: f32, c: [u8; 3]) {
        let len = ((b[0] - a[0]).powi(2) + (b[1] - a[1]).powi(2)).sqrt();
        let steps = (len * 2.0).ceil().max(1.0) as usize;
        for k in 0..=steps {
            let t = k as f32 / steps as f32;
            let p = [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t];
            if width <= 1.0 {
                self.put(p[0] as i64, p[1] as i64, c);
            } else {
                self.disc(p[0], p[1], width * 0.5, c);
            }
        }
    }
}

pub fn render(map: &MacroMap, scale: usize) -> (usize, usize, Vec<u8>) {
    let (w, h) = (map.width as usize, map.height as usize);
    let mut img = Image {
        w: w * scale,
        h: h * scale,
        rgb: vec![0; w * h * scale * scale * 3],
    };
    let at = |x: usize, z: usize| map.heights[z.min(h - 1) * w + x.min(w - 1)];
    // Light from the northwest (-x, -z), 45 degrees up.
    let light = {
        let l = [-1.0f32, 1.4, -1.0];
        let n = (l[0] * l[0] + l[1] * l[1] + l[2] * l[2]).sqrt();
        [l[0] / n, l[1] / n, l[2] / n]
    };
    for z in 0..h {
        for x in 0..w {
            let hgt = at(x, z);
            let px = x as f32 * 4.0;
            let pz = z as f32 * 4.0;
            let zone = map.zone_at(px, pz);
            let c = if zone == ZoneId::Water || hgt < map.water_level {
                let depth = (map.water_level - hgt).max(0.0);
                let t = (depth / 12.0).min(1.0);
                let shallow = [90.0, 170.0, 210.0];
                let deep = [20.0, 55.0, 120.0];
                [
                    shallow[0] + (deep[0] - shallow[0]) * t,
                    shallow[1] + (deep[1] - shallow[1]) * t,
                    shallow[2] + (deep[2] - shallow[2]) * t,
                ]
            } else {
                let gx = (at(x + 1, z) - at(x.saturating_sub(1), z)) / 8.0;
                let gz = (at(x, z + 1) - at(x, z.saturating_sub(1))) / 8.0;
                let n = [-gx, 1.0, -gz];
                let len = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
                let lit = ((n[0] * light[0] + n[1] * light[1] + n[2] * light[2]) / len).max(0.0);
                let shade = 0.45 + 0.75 * lit;
                let contour = (hgt / 20.0).floor() != (at(x + 1, z) / 20.0).floor()
                    || (hgt / 20.0).floor() != (at(x, z + 1) / 20.0).floor();
                let k = if contour { shade * 0.8 } else { shade };
                let base = ZONE_RGB[zone as usize];
                [base[0] as f32 * k, base[1] as f32 * k, base[2] as f32 * k]
            };
            let c = [
                c[0].clamp(0.0, 255.0) as u8,
                c[1].clamp(0.0, 255.0) as u8,
                c[2].clamp(0.0, 255.0) as u8,
            ];
            for sy in 0..scale {
                for sx in 0..scale {
                    img.put((x * scale + sx) as i64, (z * scale + sy) as i64, c);
                }
            }
        }
    }
    let s = scale as f32 / 4.0; // world meters to pixels
    let px = |p: [f32; 2]| [p[0] * s, p[1] * s];
    for seg in map.highway.windows(2) {
        img.line(px(seg[0]), px(seg[1]), 1.0, [255, 220, 60]);
    }
    for (k, seg) in map.strip.centerline.windows(2).enumerate() {
        if k % 2 == 0 {
            img.line(px(seg[0]), px(seg[1]), 1.0, [255, 255, 255]);
        }
    }
    if let (Some(a), Some(b)) = (map.strip.centerline.first(), map.strip.centerline.last()) {
        img.disc(px(*a)[0], px(*a)[1], 3.0 * scale as f32, [255, 255, 255]);
        img.disc(px(*b)[0], px(*b)[1], 3.0 * scale as f32, [255, 255, 255]);
    }
    let d = px(map.downtown);
    img.disc(d[0], d[1], 7.0 * scale as f32, [0, 0, 0]);
    img.disc(d[0], d[1], 4.0 * scale as f32, [255, 255, 255]);
    if let Some(p) = map.pass {
        let p = px(p);
        let r = 7.0 * scale as f32;
        img.line([p[0] - r, p[1] - r], [p[0] + r, p[1] + r], 3.0, [220, 0, 0]);
        img.line([p[0] - r, p[1] + r], [p[0] + r, p[1] - r], 3.0, [220, 0, 0]);
    }
    for poi in &map.pois {
        let p = px(poi.pos);
        match poi.kind {
            MacroPoiKind::Pier => {
                let len = 160.0 * s;
                let e = [p[0] + poi.facing[0] * len, p[1] + poi.facing[1] * len];
                img.line(p, e, 2.0 * scale as f32, [120, 80, 40]);
            }
            MacroPoiKind::Harbor => {
                let r = 5.0 * scale as f32;
                for k in 0..4 {
                    let o = k as f32 - 1.5;
                    img.line(
                        [p[0] - r, p[1] + o],
                        [p[0] + r, p[1] + o],
                        1.0,
                        [0, 230, 230],
                    );
                }
            }
            MacroPoiKind::Overlook => {
                img.disc(p[0], p[1], 5.0 * scale as f32, [0, 0, 0]);
                img.disc(p[0], p[1], 3.0 * scale as f32, [255, 150, 0]);
            }
        }
    }
    (img.w, img.h, img.rgb)
}

/// Road color and line width in meters by class; ramps (one-way collectors) are drawn
/// in their own color.
fn road_style(class: RoadClass, oneway: bool) -> ([u8; 3], f32) {
    if oneway {
        return ([255, 140, 0], 8.0);
    }
    match class {
        RoadClass::Highway => ([200, 30, 30], 22.0),
        RoadClass::Arterial => ([255, 255, 255], 14.0),
        RoadClass::Collector => ([255, 225, 90], 10.0),
        RoadClass::Beachfront => ([0, 210, 255], 12.0),
        RoadClass::Rural => ([110, 60, 20], 8.0),
        RoadClass::Local | RoadClass::Alley => ([200, 200, 200], 6.0),
    }
}

/// Draws the road graph over a rendered map: a dark casing, the class color, nodes.
pub fn draw_roads(rgb: &mut [u8], w: usize, h: usize, scale: usize, g: &RoadGraph) {
    let mut img = Image {
        w,
        h,
        rgb: rgb.to_vec(),
    };
    let s = scale as f32 / 4.0;
    let px = |p: [f32; 3]| [p[0] * s, p[2] * s];
    let mut order: Vec<usize> = (0..g.edges.len()).collect();
    // Minor roads first, the highway on top.
    let rank = |c: RoadClass| match c {
        RoadClass::Rural | RoadClass::Local | RoadClass::Alley => 0,
        RoadClass::Collector | RoadClass::Beachfront => 1,
        RoadClass::Arterial => 2,
        RoadClass::Highway => 3,
    };
    order.sort_by_key(|&i| rank(g.edges[i].class));
    for pass in 0..2 {
        for &i in &order {
            let e = &g.edges[i];
            let (c, width) = road_style(e.class, e.oneway);
            let wpx = (width * s).max(1.5);
            for seg in e.profile.windows(2) {
                if pass == 0 {
                    img.line(px(seg[0]), px(seg[1]), wpx + 2.0, [20, 20, 20]);
                } else {
                    img.line(px(seg[0]), px(seg[1]), wpx, c);
                }
            }
        }
    }
    for n in &g.nodes {
        let p = [n.pos[0] * s, n.pos[1] * s];
        img.disc(p[0], p[1], 1.2 * scale as f32, [0, 0, 0]);
    }
    rgb.copy_from_slice(&img.rgb);
}

fn usage() -> ! {
    eprintln!(
        "usage: mapdump --seed N --template <region.ron> --out <map.png> [--size WxH] [--scale S] [--roads]"
    );
    std::process::exit(2);
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let mut seed = 1u64;
    let mut template: Option<String> = None;
    let mut out: Option<String> = None;
    let mut size = [16u16, 16];
    let mut scale = 1usize;
    let mut roads = false;
    let mut k = 1;
    while k < args.len() {
        let val = args.get(k + 1).cloned();
        match args[k].as_str() {
            "--roads" => {
                roads = true;
                k += 1;
                continue;
            }
            "--seed" => seed = val.and_then(|v| v.parse().ok()).unwrap_or_else(|| usage()),
            "--template" => template = val,
            "--out" => out = val,
            "--size" => {
                let v = val.unwrap_or_else(|| usage());
                let parts: Vec<u16> = v.split('x').filter_map(|p| p.parse().ok()).collect();
                if parts.len() != 2 {
                    usage();
                }
                size = [parts[0], parts[1]];
            }
            "--scale" => scale = val.and_then(|v| v.parse().ok()).unwrap_or_else(|| usage()),
            _ => usage(),
        }
        k += 2;
    }
    let (Some(template), Some(out)) = (template, out) else {
        usage();
    };
    let text = std::fs::read_to_string(&template).unwrap_or_else(|e| {
        eprintln!("{template}: {e}");
        std::process::exit(1);
    });
    let t = load(&text).unwrap_or_else(|e| {
        eprintln!("{e}");
        std::process::exit(1);
    });
    let start = Instant::now();
    let map = MacroMap::generate(Seed(seed), &t, size);
    let ms = start.elapsed().as_secs_f64() * 1000.0;
    let mut counts = [0usize; 12];
    for &z in &map.zones {
        counts[z as usize] += 1;
    }
    let (lo, hi) = map
        .heights
        .iter()
        .fold((f32::INFINITY, f32::NEG_INFINITY), |(a, b), &h| {
            (a.min(h), b.max(h))
        });
    println!(
        "{} seed {seed}: {}x{} samples, generated in {ms:.1} ms, hash {:016x}",
        t.name,
        map.width,
        map.height,
        map.hash()
    );
    println!(
        "heights {lo:.1}..{hi:.1} m, water level {:.1} m",
        map.water_level
    );
    let total = map.zones.len() as f32;
    for (i, &c) in counts.iter().enumerate() {
        let rgb = ZONE_RGB[i];
        println!(
            "  {:<12} {:>5.1}%  rgb({},{},{})",
            ZONE_NAMES[i],
            100.0 * c as f32 / total,
            rgb[0],
            rgb[1],
            rgb[2]
        );
    }
    println!(
        "  markers: downtown black/white disc, strip centerline dashed white with end dots, highway yellow, pass red X, pier brown, harbor cyan, overlook orange"
    );
    println!(
        "downtown ({:.0}, {:.0}), strip {:.0} m, pass {:?}, water bodies {}",
        map.downtown[0],
        map.downtown[1],
        map.strip.length_m,
        map.pass.map(|p| [p[0].round(), p[1].round()]),
        map.water.len()
    );
    let (w, h, mut rgb) = render(&map, scale.max(1));
    if roads {
        let start = Instant::now();
        let g = arterials(Seed(seed), &t, &map);
        let ms = start.elapsed().as_secs_f64() * 1000.0;
        let mut count = [0usize; 7];
        for e in &g.edges {
            count[e.class as usize] += 1;
        }
        let ramps = g.edges.iter().filter(|e| e.oneway).count();
        println!(
            "roads: {} nodes, {} edges in {ms:.1} ms, hash {:016x}",
            g.nodes.len(),
            g.edges.len(),
            hash_graph(&g)
        );
        println!(
            "  highway {} (red), arterial {} (white), collector {} (yellow), ramps {ramps} (orange), beachfront {} (cyan), rural {} (brown)",
            count[RoadClass::Highway as usize],
            count[RoadClass::Arterial as usize],
            count[RoadClass::Collector as usize] - ramps,
            count[RoadClass::Beachfront as usize],
            count[RoadClass::Rural as usize],
        );
        draw_roads(&mut rgb, w, h, scale.max(1), &g);
    }
    std::fs::write(&out, png::encode(w as u32, h as u32, &rgb)).unwrap_or_else(|e| {
        eprintln!("{out}: {e}");
        std::process::exit(1);
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_seed_one_small() {
        let text = include_str!("../../../assets/regions/california_coast.ron");
        let t = load(text).expect("template");
        let map = MacroMap::generate(Seed(1), &t, [4, 4]);
        let (w, h, mut rgb) = render(&map, 1);
        assert_eq!((w, h), (128, 128));
        assert_eq!(rgb.len(), w * h * 3);
        let g = arterials(Seed(1), &t, &map);
        draw_roads(&mut rgb, w, h, 1, &g);
    }
}
