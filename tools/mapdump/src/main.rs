//! Renders a generated macro map to a PNG: hillshaded zone colors, water shaded by depth,
//! 20 m contours, and markers for the downtown seed, strip centerline, highway, pass and
//! points of interest. One pixel per 4 m height sample, times `--scale`.
//!
//! With `--roads`, also generates the arterial network and highway (stage 3) and draws its
//! edges by class on top, nodes as dots. With `--streets` (implies `--roads`), also
//! generates the local streets of every chunk (stage 4), merges them with the arterials and
//! draws locals thin and alleys faint under the arterials, with timings per chunk. With
//! `--parcels` (implies `--streets`), also builds the blocks and parcels of every chunk
//! (stage 5) and draws them under the roads: a faint outline per parcel, tinted by lot kind
//! (lots pale, parks green, empty lots and fields tan).
//!
//! Usage: `cargo run -p mapdump -- --seed 1 --template assets/regions/california_coast.ron
//! --out map.png [--size 16x16] [--scale 1] [--roads] [--streets] [--parcels]`

mod png;

use std::time::Instant;

use place::{LotKind, Parcel, RoadClass, RoadGraph, ZoneId};
use skywave_core::Seed;
use world::procgen::{
    arterials, hash_graph, hash_parcels, MacroMap, MacroPoiKind, ParcelPlan, StreetPlan,
};
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

    fn blend(&mut self, x: i64, y: i64, c: [u8; 3], a: f32) {
        if x >= 0 && y >= 0 && (x as usize) < self.w && (y as usize) < self.h {
            let i = (y as usize * self.w + x as usize) * 3;
            for (px, &ch) in self.rgb[i..i + 3].iter_mut().zip(&c) {
                let v = *px as f32 * (1.0 - a) + ch as f32 * a;
                *px = v.round().clamp(0.0, 255.0) as u8;
            }
        }
    }

    /// Fills a polygon (pixel coordinates, even-odd) blended at `a`.
    fn fill(&mut self, poly: &[[f32; 2]], c: [u8; 3], a: f32) {
        let y0 = poly
            .iter()
            .map(|p| p[1])
            .fold(f32::INFINITY, f32::min)
            .floor() as i64;
        let y1 = poly
            .iter()
            .map(|p| p[1])
            .fold(f32::NEG_INFINITY, f32::max)
            .ceil() as i64;
        let mut xs: Vec<f32> = Vec::new();
        for y in y0.max(0)..=y1.min(self.h as i64 - 1) {
            let yc = y as f32 + 0.5;
            xs.clear();
            for i in 0..poly.len() {
                let (p, q) = (poly[i], poly[(i + 1) % poly.len()]);
                if (p[1] > yc) != (q[1] > yc) {
                    xs.push(p[0] + (yc - p[1]) / (q[1] - p[1]) * (q[0] - p[0]));
                }
            }
            xs.sort_by(|a, b| a.total_cmp(b));
            for pair in xs.chunks(2) {
                if let [l, r] = pair {
                    for x in (l - 0.5).ceil() as i64..=(r - 0.5).floor() as i64 {
                        self.blend(x, y, c, a);
                    }
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

/// Road color and line width in meters by class; ramps (one-way edges) are drawn
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
        RoadClass::Local => ([235, 235, 235], 6.0),
        RoadClass::Alley => ([150, 150, 150], 2.5),
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
            let minor = matches!(e.class, RoadClass::Local | RoadClass::Alley);
            let wpx = if minor {
                (width * s).max(1.0)
            } else {
                (width * s).max(1.5)
            };
            for seg in e.profile.windows(2) {
                if pass == 0 {
                    // Alleys get no casing: faint lines.
                    if e.class != RoadClass::Alley {
                        let casing = if minor { 1.0 } else { 2.0 };
                        img.line(px(seg[0]), px(seg[1]), wpx + casing, [20, 20, 20]);
                    }
                } else {
                    img.line(px(seg[0]), px(seg[1]), wpx, c);
                }
            }
        }
    }
    // Nodes of the major roads only (street nodes would clutter the grids).
    let mut major = vec![false; g.nodes.len()];
    for e in &g.edges {
        if !matches!(e.class, RoadClass::Local | RoadClass::Alley) {
            major[e.a as usize] = true;
            major[e.b as usize] = true;
        }
    }
    for (n, _) in g.nodes.iter().zip(&major).filter(|(_, &m)| m) {
        let p = [n.pos[0] * s, n.pos[1] * s];
        img.disc(p[0], p[1], 1.2 * scale as f32, [0, 0, 0]);
    }
    rgb.copy_from_slice(&img.rgb);
}

/// Parcel tint by lot kind.
fn lot_rgb(kind: LotKind) -> [u8; 3] {
    match kind {
        LotKind::Lot => [255, 250, 235],
        LotKind::Park => [40, 190, 80],
        LotKind::LotEmpty => [165, 120, 70],
    }
}

/// Draws parcels over a rendered map: a tinted fill and a faint dark outline each.
pub fn draw_parcels(rgb: &mut [u8], w: usize, h: usize, scale: usize, parcels: &[Parcel]) {
    let mut img = Image {
        w,
        h,
        rgb: rgb.to_vec(),
    };
    let s = scale as f32 / 4.0;
    for p in parcels {
        let poly: Vec<[f32; 2]> = p.polygon.iter().map(|q| [q[0] * s, q[1] * s]).collect();
        img.fill(&poly, lot_rgb(p.lot_kind), 0.45);
    }
    for p in parcels {
        let n = p.polygon.len();
        for i in 0..n {
            let (a, b) = (p.polygon[i], p.polygon[(i + 1) % n]);
            let (a, b) = ([a[0] * s, a[1] * s], [b[0] * s, b[1] * s]);
            let len = ((b[0] - a[0]).powi(2) + (b[1] - a[1]).powi(2)).sqrt();
            let steps = (len * 2.0).ceil().max(1.0) as usize;
            for k in 0..=steps {
                let t = k as f32 / steps as f32;
                let q = [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t];
                img.blend(q[0] as i64, q[1] as i64, [50, 45, 40], 0.5);
            }
        }
    }
    rgb.copy_from_slice(&img.rgb);
}

/// Builds every chunk's parcels with timings, prints counts, returns them in chunk order.
pub fn generate_parcels(
    seed: Seed,
    t: &RegionTemplate,
    map: &MacroMap,
    merged: &RoadGraph,
) -> Vec<Parcel> {
    let start = Instant::now();
    let plan = ParcelPlan::new(seed, t, map, merged);
    let plan_ms = start.elapsed().as_secs_f64() * 1000.0;
    let mut times: Vec<f64> = Vec::new();
    let mut parcels: Vec<Parcel> = Vec::new();
    for z in 0..i32::from(map.size_chunks[1]) {
        for x in 0..i32::from(map.size_chunks[0]) {
            let s = Instant::now();
            parcels.extend(plan.chunk([x, z]).parcels);
            times.push(s.elapsed().as_secs_f64() * 1000.0);
        }
    }
    let mut sorted = times.clone();
    sorted.sort_by(|a, b| a.total_cmp(b));
    let st = plan.stats();
    println!(
        "parcels: plan {plan_ms:.1} ms, {} chunks in {:.1} ms (median {:.2} ms, max {:.2} ms)",
        times.len(),
        times.iter().sum::<f64>(),
        sorted[sorted.len() / 2],
        sorted[sorted.len() - 1],
    );
    println!(
        "  faces: {} walked, {} kept ({} open); dropped {} outer, {} islands, {} > 1 km², {} beach, {} with holes; {} with water built open; {} crossings split, {} cell cuts",
        st.walked, st.kept, st.open, st.outer, st.islands, st.too_big, st.beach, st.holes, st.water, st.crossings, st.cuts
    );
    let mut count = [0usize; 3];
    for p in &parcels {
        count[p.lot_kind as usize] += 1;
    }
    println!(
        "  {} parcels: {} lots (pale), {} parks (green), {} empty lots and fields (tan), hash {:016x}",
        parcels.len(),
        count[0],
        count[1],
        count[2],
        hash_parcels(&parcels)
    );
    parcels
}

fn usage() -> ! {
    eprintln!(
        "usage: mapdump --seed N --template <region.ron> --out <map.png> [--size WxH] [--scale S] [--roads] [--streets] [--parcels]"
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
    let mut streets = false;
    let mut parcels = false;
    let mut k = 1;
    while k < args.len() {
        let val = args.get(k + 1).cloned();
        match args[k].as_str() {
            "--roads" => {
                roads = true;
                k += 1;
                continue;
            }
            "--streets" => {
                roads = true;
                streets = true;
                k += 1;
                continue;
            }
            "--parcels" => {
                roads = true;
                streets = true;
                parcels = true;
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
        // One-way edges are ramps whatever their class, and drawn as ramps, so count them apart.
        let mut count = [0usize; 7];
        let mut ramps = 0;
        for e in &g.edges {
            if e.oneway {
                ramps += 1;
            } else {
                count[e.class as usize] += 1;
            }
        }
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
            count[RoadClass::Collector as usize],
            count[RoadClass::Beachfront as usize],
            count[RoadClass::Rural as usize],
        );
        if streets {
            let merged = generate_streets(Seed(seed), &t, &map, &g);
            if parcels {
                let ps = generate_parcels(Seed(seed), &t, &map, &merged);
                draw_parcels(&mut rgb, w, h, scale.max(1), &ps);
            }
            draw_roads(&mut rgb, w, h, scale.max(1), &merged);
        } else {
            draw_roads(&mut rgb, w, h, scale.max(1), &g);
        }
    }
    std::fs::write(&out, png::encode(w as u32, h as u32, &rgb)).unwrap_or_else(|e| {
        eprintln!("{out}: {e}");
        std::process::exit(1);
    });
}

/// Generates every chunk's streets with timings, prints counts, returns the merged graph.
pub fn generate_streets(
    seed: Seed,
    t: &RegionTemplate,
    map: &MacroMap,
    g: &RoadGraph,
) -> RoadGraph {
    let start = Instant::now();
    let plan = StreetPlan::new(seed, t, map, g);
    let plan_ms = start.elapsed().as_secs_f64() * 1000.0;
    let mut times: Vec<f64> = Vec::new();
    let mut chunks = Vec::new();
    for z in 0..i32::from(map.size_chunks[1]) {
        for x in 0..i32::from(map.size_chunks[0]) {
            let s = Instant::now();
            chunks.push(plan.chunk([x, z]));
            times.push(s.elapsed().as_secs_f64() * 1000.0);
        }
    }
    let merged = plan.merge(&chunks);
    let total: f64 = times.iter().sum();
    let mut sorted = times.clone();
    sorted.sort_by(|a, b| a.total_cmp(b));
    let max_at = times
        .iter()
        .enumerate()
        .max_by(|a, b| a.1.total_cmp(b.1))
        .map_or(0, |m| m.0);
    let sx = usize::from(map.size_chunks[0]);
    let mut count = [0usize; 7];
    let mut len = [0.0f32; 7];
    for e in &merged.edges {
        count[e.class as usize] += 1;
        len[e.class as usize] += e
            .profile
            .windows(2)
            .map(|w| ((w[1][0] - w[0][0]).powi(2) + (w[1][2] - w[0][2]).powi(2)).sqrt())
            .sum::<f32>();
    }
    let farms: usize = chunks.iter().map(|c| c.farmsteads.len()).sum();
    println!(
        "streets: plan {plan_ms:.1} ms, {} chunks in {total:.1} ms (median {:.2} ms, max {:.2} ms at chunk {:?})",
        times.len(),
        sorted[sorted.len() / 2],
        sorted[sorted.len() - 1],
        [max_at % sx, max_at / sx],
    );
    println!(
        "  merged: {} nodes, {} edges, hash {:016x}",
        merged.nodes.len(),
        merged.edges.len(),
        hash_graph(&merged),
    );
    println!(
        "  local {} edges {:.1} km (light gray), alley {} edges {:.1} km (faint gray), rural {} edges {:.1} km (brown, incl. arterial farm roads), {farms} farmsteads",
        count[RoadClass::Local as usize],
        len[RoadClass::Local as usize] / 1000.0,
        count[RoadClass::Alley as usize],
        len[RoadClass::Alley as usize] / 1000.0,
        count[RoadClass::Rural as usize],
        len[RoadClass::Rural as usize] / 1000.0,
    );
    merged
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
        let merged = generate_streets(Seed(1), &t, &map, &g);
        let ps = generate_parcels(Seed(1), &t, &map, &merged);
        draw_parcels(&mut rgb, w, h, 1, &ps);
        draw_roads(&mut rgb, w, h, 1, &merged);
    }
}
