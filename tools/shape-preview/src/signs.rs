//! `--signs`: a sheet of every sign pictogram (extruded, outlined in neon, on a dark
//! panel) above the four letterforms, each lettered in one of the sign finishes, and a row
//! of blade signs as the coast grammar draws them.

use crate::raster::{Camera, Item, Target};
use vehicle::shape::MeshData;
use world::math::{V2, V3};
use world::mesh::{rect, Frame, Glow, Paint, Pattern, WorldMesh};
use world::props::{shop_sign, Prop, PropKind};
use world::signs::letters::{self, Finish, LetterStyle};
use world::signs::lexicon::ShopSign;
use world::signs::pictogram::{self, Pictogram, Tints};
use world::Palette;

/// Pictogram tiles per row.
pub const COLS: usize = 8;
/// Blade signs in the bottom row.
pub const BLADES: usize = 8;

const GRAMMAR: &str = include_str!("../../../assets/grammar/california_coast_80s.ron");
const LEXICON: &str = include_str!("../../../assets/grammar/lexicon.ron");

fn to_mesh(m: &WorldMesh) -> MeshData {
    let mut d = MeshData::default();
    for v in &m.vertices {
        d.positions.push(v.pos);
        d.normals.push(v.normal);
        d.uvs.push(v.uv);
        d.colors.push(v.color);
        // The rasterizer reads the emissive alpha as an intensity; neon at full.
        let lit = v.emissive[3] != Glow::None as u8;
        d.emissive.push([
            v.emissive[0],
            v.emissive[1],
            v.emissive[2],
            if lit { 40 } else { 0 },
        ]);
    }
    d.indices = m.indices.clone();
    d
}

/// Facing the camera at -Z, X to its right.
fn frame() -> Frame {
    Frame {
        o: V3::new(0.0, 0.0, 0.0),
        x: V3::new(-1.0, 0.0, 0.0),
        y: V3::new(0.0, 1.0, 0.0),
        z: V3::new(0.0, 0.0, -1.0),
    }
}

fn paint(rgb: [u8; 3]) -> Paint {
    Paint::new(rgb, Pattern::Plain)
}

fn neon(rgb: [u8; 3]) -> Paint {
    Paint::new(rgb, Pattern::Plain).glowing(rgb, Glow::Neon)
}

/// Renders `m` seen square on from `dist` meters, `view` meters tall, into `pw` by `ph`.
fn shoot(m: &WorldMesh, pw: usize, ph: usize, ss: usize, view: f32) -> (usize, usize, Vec<u8>) {
    let data = to_mesh(m);
    let dist = 6.0;
    let fov = 2.0 * (view * 0.5 / dist).atan().to_degrees();
    let cam = Camera {
        eye: [0.0, 0.0, -dist],
        target: [0.0, 0.0, 0.0],
        fov_deg: fov,
        near: 0.1,
    };
    let mut t = Target::new(pw * ss, ph * ss);
    t.draw(&cam, &[Item::new(&data)], false);
    t.to_srgb(ss)
}

fn blit(out: &mut [u8], w: usize, x0: usize, y0: usize, tile: (usize, usize, Vec<u8>)) {
    let (tw, th, px) = tile;
    for y in 0..th {
        for x in 0..tw {
            let o = ((y0 + y) * w + x0 + x) * 3;
            let i = (y * tw + x) * 3;
            let edge = x == 0 || y == 0;
            for c in 0..3 {
                out[o + c] = if edge { 20 } else { px[i + c] };
            }
        }
    }
}

/// The sheet: `COLS` pictograms a row, `tile` pixels square, then one row per letterform.
pub fn sheet(tile: usize, ss: usize) -> (usize, usize, Vec<u8>) {
    let rows = Pictogram::ALL.len().div_ceil(COLS);
    let line_h = tile * 3 / 4;
    let blade_h = tile * 3;
    let (w, h) = (tile * COLS, tile * rows + line_h * 4 + blade_h);
    let mut out = vec![0u8; w * h * 3];
    let f = frame();
    let tints = Tints {
        body: paint([230, 150, 90]),
        accent: paint([90, 190, 200]),
        dark: paint([30, 30, 40]),
        light: paint([240, 236, 220]),
    };
    let tube = neon([255, 60, 170]);
    for (i, &p) in Pictogram::ALL.iter().enumerate() {
        let parts = pictogram::parts(p);
        let mut m = WorldMesh::default();
        rect(
            &mut m,
            &f,
            -0.6,
            -0.6,
            0.6,
            0.6,
            0.0,
            false,
            paint([28, 34, 52]),
        );
        let a = pictogram::extrude(&mut m, &f, &parts, 0.0, 0.0, 1.0, 0.0, 0.05, false, &tints);
        let b = pictogram::neon_outline(&mut m, &f, &parts, 0.0, 0.0, 1.0, 0.07, false, 0.03, tube);
        println!(
            "{p:?}: {} outline vertices, {a} + {b} triangles one face",
            pictogram::outline_len(&parts)
        );
        let (cx, cy) = ((i % COLS) * tile, (i / COLS) * tile);
        blit(&mut out, w, cx, cy, shoot(&m, tile, tile, ss, 1.25));
    }
    // The letterforms, each in a finish it is drawn for.
    let samples: [(LetterStyle, Finish, &[u8]); 4] = [
        (
            LetterStyle::Block,
            Finish::Channel,
            b"BLOCK ABCDEFGHIJKLM NOPQRSTUVWXYZ 0123456789",
        ),
        (
            LetterStyle::Condensed,
            Finish::Backlit,
            b"CONDENSED ABCDEFGHIJKLM NOPQRSTUVWXYZ 0123456789",
        ),
        (
            LetterStyle::Script,
            Finish::Neon,
            b"Script Abcdefghijklm Nopqrstuvwxyz Holdrek's",
        ),
        (
            LetterStyle::Slab,
            Finish::Painted,
            b"SLAB ABCDEFGHIJKLM NOPQRSTUVWXYZ SINCE 1962",
        ),
    ];
    let aspect = w as f32 / line_h as f32;
    for (row, (style, finish, text)) in samples.into_iter().enumerate() {
        let mut s = letters::layout(text, style);
        let view_h = 1.0;
        let view_w = view_h * aspect;
        let scale = (view_w * 0.94 / s.ink_w()).min(0.5);
        s.place(scale, -s.ink_w() * scale * 0.5, -0.2);
        let mut m = WorldMesh::default();
        let (panel, ink) = match finish {
            Finish::Backlit => (neon([250, 236, 200]), paint([40, 40, 60])),
            Finish::Painted => (paint([150, 70, 52]), paint([236, 228, 200])),
            Finish::Neon => (paint([28, 34, 52]), neon([80, 230, 255])),
            Finish::Channel => (paint([200, 196, 186]), paint([200, 60, 40])),
        };
        rect(&mut m, &f, -view_w, -0.6, view_w, 0.6, 0.0, false, panel);
        let side = paint([120, 40, 30]);
        let n = letters::build(&mut m, &f, &s, finish, 0.0, false, ink, side, 0.04);
        println!(
            "{style:?} ({finish:?}): {} segments, {n} triangles",
            s.segments()
        );
        let y0 = tile * rows + row * line_h;
        blit(&mut out, w, 0, y0, shoot(&m, w, line_h, ss, view_h));
    }
    // Blade signs as the coast places them, square on.
    let mut g: world::Grammar = ron::from_str(GRAMMAR).expect("grammar");
    g.lexicon = ron::from_str(LEXICON).expect("lexicon");
    let pal = Palette::socal_1985();
    let bw = w / BLADES;
    for k in 0..BLADES {
        // A splitmix step, so neighbouring signs draw unlike trades.
        let mut h = (k as u64 + 3).wrapping_mul(0x9E37_79B9_7F4A_7C15);
        h = (h ^ (h >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        h = (h ^ (h >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        let s = ShopSign::draw(&g.signs, &g.lexicon, g.era, h ^ (h >> 31));
        let p = Prop {
            pos: V2::new(0.9, 0.0),
            y: -5.6,
            dir: V2::new(-1.0, 0.0),
            kind: PropKind::NeonSign {
                palette: (k % 5) as u8,
            },
            seed: k as u32,
        };
        let mut m = WorldMesh::default();
        shop_sign(&mut m, &p, &pal, &s, &g.lexicon);
        println!(
            "blade {k}: {:?} ({:?}, {:?}), {} triangles",
            String::from_utf8_lossy(&s.text(&g.lexicon)),
            s.style,
            s.pictogram,
            m.triangle_count()
        );
        let y0 = tile * rows + line_h * 4;
        blit(&mut out, w, k * bw, y0, shoot(&m, bw, blade_h, ss, 4.6));
    }
    (w, h, out)
}
