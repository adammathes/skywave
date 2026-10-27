//! `shape-preview`: renders a `ShapeSpec` from four fixed angles into one PNG so a car can
//! be looked at without a GPU.
//!
//! Usage: `cargo run -p shape-preview -- assets/vehicles/corsair_gtx.shape.ron out.png
//! [--color N] [--lod L] [--popups] [--view 0-3] [--zoom Z]`
//!
//! Panels, left to right: front three-quarter, side, rear three-quarter, cockpit from the
//! driver's eye. Prints triangle counts.
//!
//! `cargo run -p shape-preview -- --figures out.png [--seed S]` instead draws a contact
//! sheet of twelve seeded pedestrian figures mid-stride, seventies above, eighties below
//! (seeds `S..S + 6` in each era).

mod figures;
mod png;
mod raster;

use raster::{Camera, Item, Target};
use vehicle::shape::{BuildOptions, CarMesh, MeshData, ShapeSpec};

/// Pixels per panel side in the output.
pub const PANEL: usize = 400;
/// Supersampling factor per axis.
const SS: usize = 2;

pub fn load(text: &str) -> Result<ShapeSpec, String> {
    let spec: ShapeSpec = ron::from_str(text).map_err(|e| format!("shape spec: {e}"))?;
    spec.validate()
        .map_err(|e| format!("shape spec `{}`: {e:?}", spec.id))?;
    Ok(spec)
}

/// A ground grid under the car with a soft contact shadow baked into its colors.
fn ground(car: &CarMesh, length: f32, half_w: f32) -> MeshData {
    let mut m = MeshData::default();
    let n = 48;
    let size = 40.0f32;
    let cz = car.wheel_centers[2][2] * 0.5;
    for j in 0..=n {
        for i in 0..=n {
            let x = -size * 0.5 + size * i as f32 / n as f32;
            let z = cz - size * 0.5 + size * j as f32 / n as f32;
            let dx = (x.abs() - half_w).max(0.0);
            let dz = ((z - cz).abs() - length * 0.5).max(0.0);
            let d = (dx * dx + dz * dz).sqrt();
            let inside = x.abs() < half_w && (z - cz).abs() < length * 0.5;
            let shadow = if inside {
                0.45
            } else {
                1.0 - 0.55 * (-d * 3.0).exp()
            };
            let g = (120.0 * shadow) as u8;
            m.positions.push([x, 0.0, z]);
            m.normals.push([0.0, 1.0, 0.0]);
            m.uvs.push([0.0, 0.0]);
            m.colors.push([g, g, (g as f32 * 1.02) as u8, 255]);
            m.emissive.push([0; 4]);
        }
    }
    for j in 0..n {
        for i in 0..n {
            let a = (j * (n + 1) + i) as u32;
            let b = a + 1;
            let c = a + (n + 1) as u32 + 1;
            let d = a + (n + 1) as u32;
            m.indices.extend_from_slice(&[a, d, c, a, c, b]);
        }
    }
    m
}

/// Renders the four panels into one `4 * PANEL` by `PANEL` sRGB image.
pub fn render(spec: &ShapeSpec, car: &CarMesh, panel: usize) -> (usize, usize, Vec<u8>) {
    render_views(spec, car, panel, None)
}

/// Renders all four panels, or only panel `only` (0–3).
pub fn render_views(
    spec: &ShapeSpec,
    car: &CarMesh,
    panel: usize,
    only: Option<usize>,
) -> (usize, usize, Vec<u8>) {
    render_zoomed(spec, car, panel, only, 1.0)
}

/// Like [`render_views`] with the exterior cameras' field of view divided by `zoom`.
pub fn render_zoomed(
    spec: &ShapeSpec,
    car: &CarMesh,
    panel: usize,
    only: Option<usize>,
    zoom: f32,
) -> (usize, usize, Vec<u8>) {
    let length = spec.length();
    let half_w = spec.plan.iter().fold(0.0f32, |m, p| m.max(p.1));
    let ground = ground(car, length, half_w);
    let mut items: Vec<Item> = Vec::new();
    let mut g = Item::new(&ground);
    g.two_sided = true;
    items.push(g);
    for m in [&car.body, &car.lights, &car.interior, &car.steering_wheel] {
        items.push(Item::new(m));
    }
    let mut glass = Item::new(&car.glass);
    glass.blend = true;
    items.push(glass);
    for (k, w) in car.wheels.iter().enumerate() {
        let mut it = Item::new(w);
        if k % 2 == 0 {
            it.rot = [[-1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, -1.0]];
        }
        it.offset = car.wheel_centers[k];
        items.push(it);
    }

    let front_z = -spec.wheels.front_x;
    let c = [0.0, 0.55, front_z + length * 0.5];
    let dist = length * 2.05;
    let at = |d: [f32; 3], k: f32| {
        let d = raster::norm(d);
        [
            c[0] + d[0] * dist * k,
            c[1] + d[1] * dist * k,
            c[2] + d[2] * dist * k,
        ]
    };
    let eye = car.cockpit.eye;
    let cams = [
        Camera {
            eye: at([-0.62, 0.28, -0.74], 1.0),
            target: c,
            fov_deg: 30.0 / zoom,
            near: 0.1,
        },
        Camera {
            eye: at([1.0, 0.06, 0.0], 1.3),
            target: c,
            fov_deg: 30.0 / zoom,
            near: 0.1,
        },
        Camera {
            eye: at([0.66, 0.32, 0.70], 1.0),
            target: c,
            fov_deg: 30.0 / zoom,
            near: 0.1,
        },
        Camera {
            eye,
            target: [eye[0], eye[1] - 0.13, eye[2] - 1.0],
            fov_deg: 75.0,
            near: 0.02,
        },
    ];
    let cams: Vec<Camera> = match only {
        Some(k) => vec![cams[k.min(3)]],
        None => cams.to_vec(),
    };
    let (w, h) = (panel * cams.len(), panel);
    let mut out = vec![0u8; w * h * 3];
    for (k, cam) in cams.iter().enumerate() {
        let mut t = Target::new(panel * SS, panel * SS);
        t.draw(cam, &items, false);
        t.draw(cam, &items, true);
        let (pw, ph, px) = t.to_srgb(SS);
        for y in 0..ph {
            for x in 0..pw {
                let o = (y * w + k * panel + x) * 3;
                let i = (y * pw + x) * 3;
                let edge = x == 0 && k > 0;
                for ch in 0..3 {
                    out[o + ch] = if edge { 20 } else { px[i + ch] };
                }
            }
        }
    }
    (w, h, out)
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let mut pos: Vec<&str> = Vec::new();
    let mut opts = BuildOptions::default();
    let mut view: Option<usize> = None;
    let mut zoom = 1.0f32;
    let mut figures_out: Option<String> = None;
    // The first of six seeds per era; from 126 the twelve wear every outfit in both eras.
    let mut seed = 126u64;
    let mut k = 1;
    while k < args.len() {
        match args[k].as_str() {
            "--color" => {
                k += 1;
                opts.color_index = args.get(k).and_then(|v| v.parse().ok()).unwrap_or(0);
            }
            "--lod" => {
                k += 1;
                opts.lod = args.get(k).and_then(|v| v.parse().ok()).unwrap_or(0);
            }
            "--popups" => opts.popups_up = true,
            "--figures" => {
                k += 1;
                figures_out = args.get(k).cloned();
            }
            "--seed" => {
                k += 1;
                seed = args.get(k).and_then(|v| v.parse().ok()).unwrap_or(seed);
            }
            "--view" => {
                k += 1;
                view = args.get(k).and_then(|v| v.parse().ok());
            }
            "--zoom" => {
                k += 1;
                zoom = args.get(k).and_then(|v| v.parse().ok()).unwrap_or(1.0);
            }
            a => pos.push(a),
        }
        k += 1;
    }
    if let Some(out) = figures_out {
        let (w, h, rgb) = figures::sheet(seed, 220, 360, SS);
        let png = png::encode(w as u32, h as u32, &rgb);
        std::fs::write(&out, png).unwrap_or_else(|e| {
            eprintln!("{out}: {e}");
            std::process::exit(1);
        });
        println!("wrote {out} ({w}x{h})");
        return;
    }
    if pos.len() != 2 {
        eprintln!("usage: shape-preview <spec.shape.ron> <out.png> [--color N] [--lod L] [--popups] [--view 0-3] [--zoom Z]");
        std::process::exit(2);
    }
    let text = std::fs::read_to_string(pos[0]).unwrap_or_else(|e| {
        eprintln!("{}: {e}", pos[0]);
        std::process::exit(1);
    });
    let spec = load(&text).unwrap_or_else(|e| {
        eprintln!("{e}");
        std::process::exit(1);
    });
    let car = vehicle::shape::build_with(&spec, &opts).unwrap_or_else(|e| {
        eprintln!("build: {e:?}");
        std::process::exit(1);
    });
    println!(
        "{} lod{}: exterior {} tris (body+parts {}, glass {}, lights {}, wheel {} x4), interior {} tris (cabin {}, steering wheel {})",
        spec.id,
        opts.lod,
        car.exterior_triangles(),
        car.body.triangle_count(),
        car.glass.triangle_count(),
        car.lights.triangle_count(),
        car.wheels[0].triangle_count(),
        car.interior_triangles(),
        car.interior.triangle_count(),
        car.steering_wheel.triangle_count(),
    );
    let panel = if view.is_some() { PANEL * 2 } else { PANEL };
    let (w, h, rgb) = render_zoomed(&spec, &car, panel, view, zoom);
    let png = png::encode(w as u32, h as u32, &rgb);
    std::fs::write(pos[1], png).unwrap_or_else(|e| {
        eprintln!("{}: {e}", pos[1]);
        std::process::exit(1);
    });
    println!("wrote {} ({w}x{h})", pos[1]);
}

#[cfg(test)]
mod tests {
    use super::*;

    const CORSAIR: &str = include_str!("../../../assets/vehicles/corsair_gtx.shape.ron");

    #[test]
    fn renders_the_corsair() {
        let spec = load(CORSAIR).unwrap();
        let car = vehicle::shape::build(&spec, 0).unwrap();
        let panel = 96;
        let (w, h, rgb) = render(&spec, &car, panel);
        assert_eq!((w, h), (panel * 4, panel));
        assert_eq!(rgb.len(), w * h * 3);
        // Not blank: many distinct colors.
        let mut seen = std::collections::BTreeSet::new();
        for p in rgb.chunks(3) {
            seen.insert((p[0] / 8, p[1] / 8, p[2] / 8));
        }
        assert!(seen.len() > 40, "only {} colors", seen.len());
        // In each exterior panel, the middle (where the car is) differs from an empty
        // render of the same view.
        let empty = vehicle::shape::CarMesh::default();
        let mut blank = empty.clone();
        blank.wheel_centers = car.wheel_centers;
        blank.cockpit = car.cockpit;
        let (_, _, bg) = render(&spec, &blank, panel);
        for k in 0..3 {
            let mut differ = 0;
            let mut total = 0;
            for y in panel * 3 / 8..panel * 5 / 8 {
                for x in panel * 3 / 8..panel * 5 / 8 {
                    let o = (y * w + k * panel + x) * 3;
                    total += 1;
                    let d: i32 = (0..3)
                        .map(|c| (rgb[o + c] as i32 - bg[o + c] as i32).abs())
                        .sum();
                    if d > 24 {
                        differ += 1;
                    }
                }
            }
            assert!(
                differ * 2 > total,
                "panel {k}: {differ}/{total} pixels show the car"
            );
        }
    }

    #[test]
    fn renders_a_contact_sheet_of_figures() {
        let (pw, ph) = (40, 64);
        let (w, h, rgb) = figures::sheet(126, pw, ph, 1);
        assert_eq!((w, h), (pw * figures::COLS, ph * 2));
        // Every panel shows a figure: its middle column differs from the sky beside it.
        for row in 0..2 {
            for col in 0..figures::COLS {
                let px = |x: usize, y: usize| {
                    let o = ((row * ph + y) * w + col * pw + x) * 3;
                    [rgb[o], rgb[o + 1], rgb[o + 2]]
                };
                let differ = (ph / 6..ph / 2)
                    .filter(|&y| px(pw / 2, y) != px(2, y))
                    .count();
                assert!(differ > ph / 8, "panel {row},{col}: {differ}");
            }
        }
    }
}
