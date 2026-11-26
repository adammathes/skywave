//! The two sources: TIGER/Line roads (a shapefile) and a 3DEP height grid, fetched with
//! `curl` into the cache once and read from there.

use std::path::Path;
use std::process::Command;

use crate::Warp;

/// TIGER vintage the roads come from.
const TIGER_YEAR: u32 = 2023;

/// A TIGER line: its class (`MTFCC`) and points in degrees.
pub struct Line {
    pub mtfcc: String,
    pub lonlat: Vec<(f64, f64)>,
}

fn fetch(url: &str, to: &Path) -> Result<(), String> {
    if to.exists() {
        return Ok(());
    }
    eprintln!("realmap: fetching {url}");
    let tmp = to.with_extension("part");
    let ok = Command::new("curl")
        .args(["-sSfL", "-o"])
        .arg(&tmp)
        .arg(url)
        .status()
        .map_err(|e| format!("curl: {e}"))?
        .success();
    if !ok {
        return Err(format!("could not fetch {url}"));
    }
    std::fs::rename(&tmp, to).map_err(|e| e.to_string())
}

fn u32le(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes(b[o..o + 4].try_into().unwrap())
}

fn f64le(b: &[u8], o: usize) -> f64 {
    f64::from_le_bytes(b[o..o + 8].try_into().unwrap())
}

/// The state's primary and secondary roads of the classes asked for that touch the warp's box.
pub fn tiger_roads(
    cache: &Path,
    state: &str,
    warp: &Warp,
    classes: &[String],
) -> Result<Vec<Line>, String> {
    let base = format!("tl_{TIGER_YEAR}_{state}_prisecroads");
    let zip = cache.join(format!("{base}.zip"));
    fetch(
        &format!("https://www2.census.gov/geo/tiger/TIGER{TIGER_YEAR}/PRISECROADS/{base}.zip"),
        &zip,
    )?;
    let shp = cache.join(format!("{base}.shp"));
    if !shp.exists() {
        let ok = Command::new("unzip")
            .args(["-o", "-q"])
            .arg(&zip)
            .arg(format!("{base}.shp"))
            .arg(format!("{base}.dbf"))
            .arg("-d")
            .arg(cache)
            .status()
            .map_err(|e| format!("unzip: {e}"))?
            .success();
        if !ok {
            return Err(format!("could not unzip {}", zip.display()));
        }
    }
    let shp = std::fs::read(&shp).map_err(|e| e.to_string())?;
    let dbf = std::fs::read(cache.join(format!("{base}.dbf"))).map_err(|e| e.to_string())?;

    // dBASE: header, field descriptors, fixed-width records.
    let n = u32le(&dbf, 4) as usize;
    let header = u16::from_le_bytes([dbf[8], dbf[9]]) as usize;
    let rec = u16::from_le_bytes([dbf[10], dbf[11]]) as usize;
    let mut fields = Vec::new();
    let (mut o, mut at) = (32, 1);
    while dbf[o] != 0x0d {
        let name = String::from_utf8_lossy(&dbf[o..o + 11])
            .trim_end_matches('\0')
            .to_string();
        let len = dbf[o + 16] as usize;
        fields.push((name, at, len));
        at += len;
        o += 32;
    }
    let mtfcc = fields
        .iter()
        .find(|f| f.0 == "MTFCC")
        .ok_or("TIGER file has no MTFCC field")?;
    let class_of = |i: usize| {
        let r = header + i * rec;
        String::from_utf8_lossy(&dbf[r + mtfcc.1..r + mtfcc.1 + mtfcc.2])
            .trim()
            .to_string()
    };

    // Shapefile: polyline records (type 3), each with parts.
    let (w, e, s, nn) = (warp.west(), warp.east(), warp.south(), warp.north());
    let mut out = Vec::new();
    let mut o = 100;
    let mut i = 0;
    while o + 8 <= shp.len() && i < n {
        let len = u32::from_be_bytes(shp[o + 4..o + 8].try_into().unwrap()) as usize * 2;
        let c = &shp[o + 8..o + 8 + len];
        o += 8 + len;
        let idx = i;
        i += 1;
        if u32le(c, 0) != 3 {
            continue;
        }
        let (x0, y0, x1, y1) = (f64le(c, 4), f64le(c, 12), f64le(c, 20), f64le(c, 28));
        if x1 < w || x0 > e || y1 < s || y0 > nn {
            continue;
        }
        let class = class_of(idx);
        if !classes.contains(&class) {
            continue;
        }
        let parts = u32le(c, 36) as usize;
        let points = u32le(c, 40) as usize;
        let starts: Vec<usize> = (0..parts).map(|k| u32le(c, 44 + 4 * k) as usize).collect();
        let pt0 = 44 + 4 * parts;
        for k in 0..parts {
            let a = starts[k];
            let b = if k + 1 < parts { starts[k + 1] } else { points };
            let lonlat = (a..b)
                .map(|j| (f64le(c, pt0 + 16 * j), f64le(c, pt0 + 16 * j + 8)))
                .collect();
            out.push(Line {
                mtfcc: class.clone(),
                lonlat,
            });
        }
    }
    Ok(out)
}

/// A height grid in degrees: `w` x `h` samples over the warp's box, rows from the north.
/// Samples the service had no data for (open water) are `-1`.
pub struct Dem {
    pub w: usize,
    pub h: usize,
    west: f64,
    north: f64,
    dlon: f64,
    dlat: f64,
    pub z: Vec<f32>,
}

impl Dem {
    /// Bilinear height at (lon, lat), clamped to the grid.
    pub fn at(&self, lon: f64, lat: f64) -> f32 {
        let fx = ((lon - self.west) / self.dlon - 0.5).clamp(0.0, (self.w - 1) as f64);
        let fy = ((self.north - lat) / self.dlat - 0.5).clamp(0.0, (self.h - 1) as f64);
        let (ix, iy) = ((fx as usize).min(self.w - 2), (fy as usize).min(self.h - 2));
        let (tx, ty) = ((fx - ix as f64) as f32, (fy - iy as f64) as f32);
        let g = |i: usize, j: usize| self.z[j * self.w + i];
        let a = g(ix, iy) + (g(ix + 1, iy) - g(ix, iy)) * tx;
        let b = g(ix, iy + 1) + (g(ix + 1, iy + 1) - g(ix, iy + 1)) * tx;
        a + (b - a) * ty
    }
}

/// The 3DEP elevation over the warp's box, `px` samples across its longer side, from the
/// National Map's elevation image service (raw float samples followed by a validity mask).
pub fn dem(cache: &Path, warp: &Warp, px: usize) -> Result<Dem, String> {
    let (w, e, s, n) = (warp.west(), warp.east(), warp.south(), warp.north());
    let aspect = (e - w) / (n - s);
    let (cw, ch) = if aspect >= 1.0 {
        (px, ((px as f64) / aspect).round() as usize)
    } else {
        (((px as f64) * aspect).round() as usize, px)
    };
    let file = cache.join(format!("3dep_{w:.4}_{s:.4}_{e:.4}_{n:.4}_{cw}x{ch}.bsq"));
    fetch(
        &format!(
            "https://elevation.nationalmap.gov/arcgis/rest/services/3DEPElevation/ImageServer/exportImage?bbox={w},{s},{e},{n}&bboxSR=4326&imageSR=4326&size={cw},{ch}&format=bsq&pixelType=F32&interpolation=RSP_BilinearInterpolation&f=image"
        ),
        &file,
    )?;
    let b = std::fs::read(&file).map_err(|e| e.to_string())?;
    let count = cw * ch;
    if b.len() < count * 4 {
        return Err(format!("{}: short elevation file", file.display()));
    }
    let mask = &b[count * 4..];
    let z = (0..count)
        .map(|i| {
            let v = f32::from_le_bytes(b[4 * i..4 * i + 4].try_into().unwrap());
            // The mask, when present, has one bit per sample, high bit first.
            let valid = mask.get(i / 8).is_none_or(|m| m & (0x80 >> (i % 8)) != 0);
            if valid && v.is_finite() && v > -500.0 {
                v
            } else {
                -1.0
            }
        })
        .collect();
    Ok(Dem {
        w: cw,
        h: ch,
        west: w,
        north: n,
        dlon: (e - w) / cw as f64,
        dlat: (n - s) / ch as f64,
        z,
    })
}
