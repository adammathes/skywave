//! Offline importer for real places (`docs/LAYOUTS.md`, "Real data"): reads the primary and
//! secondary roads of US Census TIGER/Line and the USGS 3DEP elevation model for a real box,
//! squeezes them into an authored layout's frame and rewrites the layout's generated
//! sections (between `// realmap:<name> begin` and `// realmap:<name> end` markers):
//! `terrain` (a height grid), `water` (the sea, traced from the elevation model) and
//! `roads` (the main road network, trimmed so every road ends on another).
//!
//! Both sources are public domain (`docs/adr/0005-reference-sources.md`). The raw files are
//! downloaded with `curl` into `target/realmap/` and never committed; only the small layout
//! is. Nothing here runs in the game.
//!
//! Usage: `cargo run -p realmap -- tools/realmap/<id>.ron`.

mod net;
mod sources;
mod water;

use std::fmt::Write as _;
use std::process::ExitCode;

use serde::Deserialize;

/// One import: which layout to rewrite and how the real box maps into it.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Import {
    /// The layout whose generated sections are rewritten.
    pub layout: String,
    /// The state's FIPS code, for the TIGER file ("06" is California).
    pub state: String,
    /// Longitude to x: (degrees, meters) breakpoints, increasing. The first maps to x = 0 and
    /// the last to the layout's east edge; between them it is linear, so a city can get
    /// more room than the land around it.
    pub x_warp: Vec<(f64, f32)>,
    /// Latitude to z, the same way, from the north edge (z = 0) down.
    pub z_warp: Vec<(f64, f32)>,
    /// A county's FIPS code: read its all-roads file, and take the `names` too.
    #[serde(default)]
    pub county: Option<String>,
    /// TIGER `FULLNAME`s of main streets to import as arterials (with `county`).
    #[serde(default)]
    pub names: Vec<String>,
    /// The real height that maps to the layout's ground (a lake city's shore).
    #[serde(default)]
    pub datum_m: f32,
    /// Slopes are this many times steeper than the real ones at the local squeeze.
    pub slope: f32,
    /// Low ground is flattened: a real height `h` counts as `h * h / (h + foothill_m)`, so
    /// the land under about this height (where the suburbs climb) stays gentle enough for
    /// streets while the ridges keep nearly their full height. Hilly districts keep their
    /// full relief. 0 leaves heights as they are.
    #[serde(default)]
    pub foothill_m: f32,
    /// Passes of a 3 x 3 box blur over the height grid: rounds off hills too small to drive
    /// at the frame's scale.
    #[serde(default)]
    pub smooth: u32,
    /// Heights are capped here, after scaling.
    #[serde(default = "max_height")]
    pub max_height_m: f32,
    /// Land at or under this real height that the sea reaches is water.
    pub sea_below_m: f32,
    /// `Sea` (the default) or `Lake`.
    #[serde(default = "sea")]
    pub water_kind: String,
    /// Spacing of the height grid.
    pub terrain_cell_m: f32,
    /// Road rasterization cell: roads closer than this merge (divided highways become one).
    pub merge_m: f32,
    /// Roads with a TIGER `MTFCC` in this list are imported: S1100 primary (as highways),
    /// S1200 secondary (as arterials).
    #[serde(default = "classes")]
    pub classes: Vec<String>,
    /// Real-world boxes (west, south, east, north) whose highways are cut out (where one
    /// runs too close to the streets or a river for its overpasses at the frame's squeeze).
    #[serde(default)]
    pub skip: Vec<(f64, f64, f64, f64)>,
    /// Invented links, (lon, lat) polylines imported like secondary roads: a stand-in for a
    /// real connection that runs outside the box, so the roads it joins are not trimmed.
    #[serde(default)]
    pub links: Vec<Vec<(f64, f64)>>,
    /// Points on suspension bridges, in degrees: the chain through each stays its own road
    /// and stands towers.
    #[serde(default)]
    pub suspension: Vec<(f64, f64)>,
    /// Districts, in degrees.
    pub districts: Vec<District>,
    /// The player starts on the road nearest this point...
    pub start: (f64, f64),
    /// ...driving toward this one (the road's points are turned round to suit).
    #[serde(default)]
    pub start_toward: Option<(f64, f64)>,
    /// Channels, in degrees.
    #[serde(default)]
    pub channels: Vec<Channel>,
}

/// A channel or river (`docs/LAYOUTS.md`), its line in degrees.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Channel {
    pub name: String,
    pub line: Vec<(f64, f64)>,
    pub width_m: f32,
    pub depth_m: f32,
    #[serde(default)]
    pub wet: bool,
}

/// A district: a polygon, or a band along a line `band_m` either side.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct District {
    pub name: String,
    pub zone: String,
    #[serde(default)]
    pub hilly: bool,
    #[serde(default)]
    pub polygon: Vec<(f64, f64)>,
    #[serde(default)]
    pub band: Vec<(f64, f64)>,
    #[serde(default)]
    pub band_m: f32,
}

fn sea() -> String {
    "Sea".into()
}

fn max_height() -> f32 {
    400.0
}

fn classes() -> Vec<String> {
    vec!["S1100".into(), "S1200".into()]
}

/// Piecewise-linear maps between the real box and the layout's frame.
pub struct Warp {
    x: Vec<(f64, f32)>,
    z: Vec<(f64, f32)>,
}

fn lerp_pw(t: &[(f64, f32)], v: f64) -> f32 {
    for w in t.windows(2) {
        let ((a, xa), (b, xb)) = (w[0], w[1]);
        let lo = a.min(b);
        let hi = a.max(b);
        if v >= lo && v <= hi {
            let f = (v - a) / (b - a);
            return xa + (xb - xa) * f as f32;
        }
    }
    if (v - t[0].0).abs() < (v - t[t.len() - 1].0).abs() {
        t[0].1
    } else {
        t[t.len() - 1].1
    }
}

fn inv_pw(t: &[(f64, f32)], m: f32) -> f64 {
    for w in t.windows(2) {
        let ((a, xa), (b, xb)) = (w[0], w[1]);
        if m >= xa && m <= xb {
            let f = f64::from((m - xa) / (xb - xa));
            return a + (b - a) * f;
        }
    }
    if m < t[0].1 {
        t[0].0
    } else {
        t[t.len() - 1].0
    }
}

/// Real meters per output meter along a warp at output coordinate `m`.
fn squeeze(t: &[(f64, f32)], m: f32, deg_m: f64) -> f32 {
    let w = t
        .windows(2)
        .find(|w| m <= w[1].1)
        .unwrap_or(&t[t.len() - 2..]);
    ((w[1].0 - w[0].0).abs() * deg_m) as f32 / (w[1].1 - w[0].1)
}

impl Warp {
    pub fn west(&self) -> f64 {
        self.x[0].0
    }
    pub fn east(&self) -> f64 {
        self.x[self.x.len() - 1].0
    }
    pub fn north(&self) -> f64 {
        self.z[0].0
    }
    pub fn south(&self) -> f64 {
        self.z[self.z.len() - 1].0
    }
    /// (lon, lat) to (x, z) meters.
    pub fn fwd(&self, lon: f64, lat: f64) -> (f32, f32) {
        (lerp_pw(&self.x, lon), lerp_pw(&self.z, lat))
    }
    /// (x, z) meters to (lon, lat).
    pub fn inv(&self, x: f32, z: f32) -> (f64, f64) {
        (inv_pw(&self.x, x), inv_pw(&self.z, z))
    }
    /// How many real meters one output meter stands for at (x, z): the geometric mean of
    /// the two axes' squeeze.
    pub fn squeeze(&self, x: f32, z: f32) -> f32 {
        let lat = (self.north() + self.south()) * 0.5;
        let lon_m = 111_320.0 * lat.to_radians().cos();
        (squeeze(&self.x, x, lon_m) * squeeze(&self.z, z, 110_574.0)).sqrt()
    }
}

/// The layout's extent in meters, from its `size_chunks: (x, z)` line (the layout may not
/// parse before its generated sections are filled).
fn extent(text: &str) -> Option<[f32; 2]> {
    let i = text.find("size_chunks:")? + "size_chunks:".len();
    let s = &text[i..i + text[i..].find(')')?];
    let s = s.trim().trim_start_matches('(');
    let (a, b) = s.split_once(',')?;
    let c = place::CHUNK_SIZE_M;
    Some([
        a.trim().parse::<f32>().ok()? * c,
        b.trim().parse::<f32>().ok()? * c,
    ])
}

/// Whether (x, y) lies inside `poly` (even-odd).
fn point_in(x: f64, y: f64, poly: &[(f64, f64)]) -> bool {
    let mut inside = false;
    let n = poly.len();
    for i in 0..n {
        let (a, b) = (poly[i], poly[(i + n - 1) % n]);
        if (a.1 > y) != (b.1 > y) && x < (b.0 - a.0) * (y - a.1) / (b.1 - a.1) + a.0 {
            inside = !inside;
        }
    }
    inside
}

/// A polygon `half` meters either side of `line`.
fn band(line: &[(f32, f32)], half: f32) -> Vec<(f32, f32)> {
    let n = line.len();
    let normal = |i: usize| {
        let (a, b) = (line[i.saturating_sub(1)], line[(i + 1).min(n - 1)]);
        let (dx, dz) = (b.0 - a.0, b.1 - a.1);
        let l = dx.hypot(dz).max(1e-3);
        (-dz / l, dx / l)
    };
    let left = (0..n).map(|i| {
        (
            line[i].0 + normal(i).0 * half,
            line[i].1 + normal(i).1 * half,
        )
    });
    let right = (0..n).rev().map(|i| {
        (
            line[i].0 - normal(i).0 * half,
            line[i].1 - normal(i).1 * half,
        )
    });
    left.chain(right).collect()
}

/// Rewrites the text between `// realmap:<name> begin` and `// realmap:<name> end`.
fn splice(text: &str, name: &str, body: &str) -> Result<String, String> {
    let b = format!("// realmap:{name} begin");
    let e = format!("// realmap:{name} end");
    let i = text.find(&b).ok_or(format!("layout has no `{b}` marker"))?;
    let j = text.find(&e).ok_or(format!("layout has no `{e}` marker"))?;
    let line_end = i + text[i..].find('\n').unwrap() + 1;
    let indent_start = text[..j].rfind('\n').unwrap() + 1;
    let mut out = String::new();
    out.push_str(&text[..line_end]);
    out.push_str(body);
    out.push_str(&text[indent_start..]);
    Ok(out)
}

fn run(path: &str) -> Result<(), String> {
    let cfg_text = std::fs::read_to_string(path).map_err(|e| format!("{path}: {e}"))?;
    let cfg: Import = ron::from_str(&cfg_text).map_err(|e| format!("{path}: {e}"))?;
    let layout_text =
        std::fs::read_to_string(&cfg.layout).map_err(|e| format!("{}: {e}", cfg.layout))?;
    let ext = extent(&layout_text).ok_or(format!("{}: no `size_chunks: (x, z)`", cfg.layout))?;
    let warp = Warp {
        x: cfg.x_warp.clone(),
        z: cfg.z_warp.clone(),
    };
    let last = |t: &[(f64, f32)]| t[t.len() - 1].1;
    if cfg.x_warp.len() < 2
        || cfg.z_warp.len() < 2
        || cfg.x_warp[0].1 != 0.0
        || cfg.z_warp[0].1 != 0.0
        || last(&cfg.x_warp) != ext[0]
        || last(&cfg.z_warp) != ext[1]
    {
        return Err(format!(
            "x_warp and z_warp must run from 0 to the layout's extent {ext:?}"
        ));
    }

    let cache = std::path::Path::new("target/realmap");
    std::fs::create_dir_all(cache).map_err(|e| e.to_string())?;
    let roads = sources::tiger_roads(
        cache,
        &cfg.state,
        cfg.county.as_deref(),
        &cfg.names,
        &warp,
        &cfg.classes,
    )?;
    let dem = sources::dem(cache, &warp, 1024)?;
    eprintln!("realmap: {} TIGER lines in the box", roads.len());

    // Terrain: the real heights, scaled down where the frame squeezes the land.
    let cell = cfg.terrain_cell_m;
    let cols = (ext[0] / cell).ceil() as usize + 1;
    let rows = (ext[1] / cell).ceil() as usize + 1;
    let hilly: Vec<&Vec<(f64, f64)>> = cfg
        .districts
        .iter()
        .filter(|d| d.hilly)
        .map(|d| &d.polygon)
        .collect();
    let mut hs = vec![0.0f32; cols * rows];
    for r in 0..rows {
        for c in 0..cols {
            let (x, z) = (c as f32 * cell, r as f32 * cell);
            let (lon, lat) = warp.inv(x, z);
            let h = (dem.at(lon, lat) - cfg.datum_m).max(0.0);
            let hilly = hilly.iter().any(|p| point_in(lon, lat, p));
            let h = if cfg.foothill_m > 0.0 && !hilly {
                h * h / (h + cfg.foothill_m)
            } else {
                h
            };
            hs[r * cols + c] = (h * cfg.slope / warp.squeeze(x, z)).min(cfg.max_height_m);
        }
    }
    for _ in 0..cfg.smooth {
        let src = hs.clone();
        for r in 0..rows {
            for c in 0..cols {
                let (mut sum, mut n) = (0.0, 0.0);
                for rr in r.saturating_sub(1)..(r + 2).min(rows) {
                    for cc in c.saturating_sub(1)..(c + 2).min(cols) {
                        sum += src[rr * cols + cc];
                        n += 1.0;
                    }
                }
                hs[r * cols + c] = sum / n;
            }
        }
    }
    let mut t = String::new();
    writeln!(
        t,
        "    terrain: Some(Terrain(cell_m: {cell}, cols: {cols}, rows: {rows}, heights_m: ["
    )
    .unwrap();
    for r in 0..rows {
        t.push_str("        ");
        for c in 0..cols {
            let y = (hs[r * cols + c] * 2.0).round() / 2.0;
            write!(t, "{y},").unwrap();
        }
        t.push('\n');
    }
    t.push_str("    ])),\n");

    // Water: the sea and the bays it reaches.
    let polys = water::sea(&dem, &warp, ext, cfg.sea_below_m);
    let mut w = String::new();
    for p in &polys {
        writeln!(
            w,
            "        Water(kind: {}, depth_m: 4.2, polygon: [",
            cfg.water_kind
        )
        .unwrap();
        for chunk in p.chunks(8) {
            w.push_str("            ");
            for &(x, z) in chunk {
                write!(w, "({}, {}), ", x.round(), z.round()).unwrap();
            }
            w.push('\n');
        }
        w.push_str("        ]),\n");
    }

    // Roads.
    let skip: Vec<[f64; 4]> = cfg.skip.iter().map(|&(a, b, c, d)| [a, b, c, d]).collect();
    // Cut out the parts of highways inside a skip box, splitting them there.
    let skipped = |(lon, lat): (f64, f64)| {
        skip.iter()
            .any(|s| lon >= s[0] && lat >= s[1] && lon <= s[2] && lat <= s[3])
    };
    let mut lines: Vec<sources::Line> = Vec::new();
    for l in roads {
        let mut part: Vec<(f64, f64)> = Vec::new();
        for &p in l
            .lonlat
            .iter()
            .chain(core::iter::once(&(f64::NAN, f64::NAN)))
        {
            if p.0.is_nan() || (l.mtfcc == "S1100" && skipped(p)) {
                if part.len() >= 2 {
                    lines.push(sources::Line {
                        mtfcc: l.mtfcc.clone(),
                        lonlat: core::mem::take(&mut part),
                    });
                }
                part.clear();
            } else {
                part.push(p);
            }
        }
    }
    lines.extend(cfg.links.iter().map(|l| sources::Line {
        mtfcc: "S1200".into(),
        lonlat: l.clone(),
    }));
    let susp: Vec<(f32, f32)> = cfg
        .suspension
        .iter()
        .map(|&(a, b)| warp.fwd(a, b))
        .collect();
    let mut net = net::network(&lines, &warp, ext, cfg.merge_m, &susp);
    // Start: the nearest point on any road.
    let sp = warp.fwd(cfg.start.0, cfg.start.1);
    let mut best = (f32::INFINITY, 0usize, 0.0f32, (0.0f32, 0.0f32));
    for (ri, road) in net.iter().enumerate() {
        let mut s = 0.0;
        for w in road.points.windows(2) {
            let (a, b) = (w[0], w[1]);
            let len = (b.0 - a.0).hypot(b.1 - a.1);
            let t = if len > 0.0 {
                (((sp.0 - a.0) * (b.0 - a.0) + (sp.1 - a.1) * (b.1 - a.1)) / (len * len))
                    .clamp(0.0, 1.0)
            } else {
                0.0
            };
            let q = (a.0 + (b.0 - a.0) * t, a.1 + (b.1 - a.1) * t);
            let dd = (q.0 - sp.0).hypot(q.1 - sp.1);
            if dd < best.0 {
                best = (dd, ri, s + t * len, (b.0 - a.0, b.1 - a.1));
            }
            s += len;
        }
    }
    // Facing away from `start_toward`: turn the road round.
    let (ri, mut along) = (best.1, best.2);
    if let Some((lon, lat)) = cfg.start_toward {
        let to = warp.fwd(lon, lat);
        let dir = best.3;
        if dir.0 * (to.0 - sp.0) + dir.1 * (to.1 - sp.1) < 0.0 {
            let pts = &mut net[ri].points;
            let len: f32 = pts
                .windows(2)
                .map(|w| (w[1].0 - w[0].0).hypot(w[1].1 - w[0].1))
                .sum();
            pts.reverse();
            along = len - along;
        }
    }
    let start = format!(
        "    start: Start(road: \"{}\", along_m: {}),\n",
        net[ri].name,
        along.floor()
    );

    let mut r = String::new();
    for road in &net {
        let pts: Vec<String> = road
            .points
            .iter()
            .map(|&(x, z)| format!("({}, {})", x.round(), z.round()))
            .collect();
        let ramps = if road.ramps.is_empty() {
            String::new()
        } else {
            let names: Vec<String> = road.ramps.iter().map(|n| format!("\"{n}\"")).collect();
            format!(", ramps: [{}]", names.join(", "))
        };
        let ramps = if road.suspension {
            format!("{ramps}, suspension: true")
        } else {
            ramps
        };
        let class = if road.highway {
            "class: Highway, lanes: 2".to_string()
        } else {
            "class: Arterial, lanes: 2, max_grade: Some(0.16)".to_string()
        };
        writeln!(
            r,
            "        Road(name: \"{}\", {class}{ramps}, points: [",
            road.name
        )
        .unwrap();
        for chunk in pts.chunks(8) {
            writeln!(r, "            {},", chunk.join(", ")).unwrap();
        }
        r.push_str("        ]),\n");
    }

    // Districts.
    let clamp = |(x, z): (f32, f32)| (x.clamp(0.0, ext[0]).round(), z.clamp(0.0, ext[1]).round());
    let mut d = String::new();
    for dist in &cfg.districts {
        let poly: Vec<(f32, f32)> = if dist.band.is_empty() {
            dist.polygon.iter().map(|&(a, b)| warp.fwd(a, b)).collect()
        } else {
            let line: Vec<(f32, f32)> = dist.band.iter().map(|&(a, b)| warp.fwd(a, b)).collect();
            band(&line, dist.band_m)
        };
        let pts: Vec<String> = poly
            .into_iter()
            .map(clamp)
            .map(|(x, z)| format!("({x}, {z})"))
            .collect();
        let hilly = if dist.hilly { ", hilly: true" } else { "" };
        writeln!(
            d,
            "        District(name: \"{}\", zone: {}{hilly}, polygon: [{}]),",
            dist.name,
            dist.zone,
            pts.join(", ")
        )
        .unwrap();
    }

    // Channels.
    let mut ch = String::new();
    for c in &cfg.channels {
        let pts: Vec<String> = c
            .line
            .iter()
            .map(|&(a, b)| clamp(warp.fwd(a, b)))
            .map(|(x, z)| format!("({x}, {z})"))
            .collect();
        let wet = if c.wet { ", wet: true" } else { "" };
        writeln!(
            ch,
            "        Channel(name: \"{}\", line: [{}], width_m: {}, depth_m: {}{wet}),",
            c.name,
            pts.join(", "),
            c.width_m,
            c.depth_m
        )
        .unwrap();
    }

    let mut text = layout_text.clone();
    if !cfg.channels.is_empty() {
        text = splice(&text, "channels", &ch)?;
    }
    text = splice(&text, "districts", &d)?;
    text = splice(&text, "start", &start)?;
    text = splice(&text, "terrain", &t)?;
    text = splice(&text, "water", &w)?;
    text = splice(&text, "roads", &r)?;
    let new: world::layout::Layout =
        ron::from_str(&text).map_err(|e| format!("generated layout does not parse: {e}"))?;
    std::fs::write(&cfg.layout, &text).map_err(|e| e.to_string())?;
    eprintln!(
        "realmap: wrote {} ({} roads, {} water polygons, {cols}x{rows} heights)",
        cfg.layout,
        net.len(),
        polys.len()
    );
    new.validate()
        .map_err(|e| format!("{} does not validate: {e}", cfg.layout))
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().collect();
    let Some(path) = args.get(1) else {
        eprintln!("usage: realmap <import.ron>");
        return ExitCode::FAILURE;
    };
    match run(path) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("realmap: {e}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splice_replaces_only_between_markers() {
        let t = "a\n    // realmap:x begin\n    old\n    // realmap:x end\nb\n";
        let s = splice(t, "x", "    new\n").unwrap();
        assert_eq!(
            s,
            "a\n    // realmap:x begin\n    new\n    // realmap:x end\nb\n"
        );
        assert!(splice(t, "y", "").is_err());
    }

    #[test]
    fn warp_round_trips_and_squeezes_per_piece() {
        let w = Warp {
            x: vec![(-122.0, 0.0), (-121.9, 1000.0), (-121.5, 2000.0)],
            z: vec![(38.0, 0.0), (37.0, 2000.0)],
        };
        let (x, z) = w.fwd(-121.7, 37.5);
        assert!((x - 1500.0).abs() < 0.01 && (z - 1000.0).abs() < 0.01);
        let (lon, lat) = w.inv(x, z);
        assert!((lon + 121.7).abs() < 1e-6 && (lat - 37.5).abs() < 1e-6);
        // The second x piece holds four times the longitude in the same meters.
        let ratio = squeeze(&w.x, 1500.0, 1.0) / squeeze(&w.x, 500.0, 1.0);
        assert!((ratio - 4.0).abs() < 1e-3);
    }

    #[test]
    fn extent_reads_size_chunks() {
        assert_eq!(extent("  size_chunks: (32, 16),"), Some([4096.0, 2048.0]));
    }
}
