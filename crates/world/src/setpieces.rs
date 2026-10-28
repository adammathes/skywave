//! The set pieces' props (`docs/WORLD_GENERATION.md`, "Set pieces"): the meshes of a
//! tunnel's tiled box, a drag strip's start tree, timing board and painted lines, guard
//! rail, chain-link, a crew's garage, a taco stand and its picnic tables, and the
//! collision boxes the collider gives them. `procgen::setpieces` sites and places them.
//!
//! The start tree's lit lamps and the timing board's digits change as the player runs the
//! strip, so the app draws them as instances over the props: [`tree_lamps`] and
//! [`digit`] build those meshes at the prop's origin facing yaw 0, and [`TREE_LAMPS`] and
//! [`board_slot`] say where each goes in the prop's frame.

use alloc::vec::Vec;

use crate::math::{V2, V3};
use crate::mesh::{boxf, face, quad, rect, Frame, Glow, Paint, Pattern, WorldMesh};
use crate::palette::{mix, shade, Palette, Rgb};
use crate::props::{Prop, PropKind};
use crate::signs::letters::{self, LetterStyle};
use crate::signs::lexicon::Lexicon;

/// A tunnel's inner height over the road, its walls' and roof's thickness, m.
pub const TUNNEL_H_M: f32 = 6.0;
const TUNNEL_WALL_M: f32 = 0.6;
const TUNNEL_ROOF_M: f32 = 1.2;
/// Height of a portal's headwall over the roof, m.
const PORTAL_M: f32 = 1.6;
/// A tunnel's ceiling lamps: a strip this far in from each wall, m.
const TUNNEL_LAMP_IN_M: f32 = 2.5;

/// A crew garage's front width, depth and wall height, m (three bays).
pub const GARAGE_M: (f32, f32, f32) = (20.0, 14.0, 5.4);
/// Its bays' clear width and height, and the piers between them, m.
const BAY_W_M: f32 = 5.6;
const BAY_H_M: f32 = 3.9;
/// A taco stand's width, depth and height, m.
pub const STAND_M: (f32, f32, f32) = (5.0, 3.2, 3.0);

/// Whether props of `kind` are a set piece's (built by [`build`]).
pub fn is_set_piece(kind: PropKind) -> bool {
    matches!(
        kind,
        PropKind::TunnelPiece { .. }
            | PropKind::StartTree
            | PropKind::TimingBoard
            | PropKind::StripLine { .. }
            | PropKind::GuardRail { .. }
            | PropKind::ChainLink { .. }
            | PropKind::CrewGarage { .. }
            | PropKind::TacoStand { .. }
            | PropKind::PicnicTable
            | PropKind::CrossingGate { .. }
    )
}

/// Builds set piece prop `p` (see [`is_set_piece`]); signs are lettered from `lex` (none
/// with an empty lexicon).
pub fn build(m: &mut WorldMesh, p: &Prop, pal: &Palette, lex: &Lexicon) {
    let base = p.pos.at(p.y);
    match p.kind {
        PropKind::TunnelPiece {
            len_dm,
            width_dm,
            portals,
        } => tunnel(
            m,
            base,
            p.dir,
            f32::from(len_dm) * 0.1,
            f32::from(width_dm) * 0.1,
            portals,
            pal,
        ),
        PropKind::StartTree => start_tree(m, base, p.dir, pal),
        PropKind::TimingBoard => timing_board(m, base, p.dir, pal),
        PropKind::StripLine { len_dm } => strip_line(m, base, p.dir, f32::from(len_dm) * 0.1, pal),
        PropKind::GuardRail { len_dm } => guard_rail(m, base, p.dir, f32::from(len_dm) * 0.1, pal),
        PropKind::ChainLink { len_dm } => chain_link(m, base, p.dir, f32::from(len_dm) * 0.1, pal),
        PropKind::CrewGarage { name_seed } => garage(m, base, p.dir, name_seed, pal, lex),
        PropKind::TacoStand { name_seed } => taco_stand(m, base, p.dir, name_seed, pal, lex),
        PropKind::PicnicTable => picnic_table(m, base, p.dir, pal),
        PropKind::CrossingGate { .. } => crossing_gate(m, base, p.dir, pal),
        _ => {}
    }
}

/// An upright box a set piece's collider holds: center `c`, unit axis `u`, half extents
/// along `u` and across it, from `y0` to `y1`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Slab {
    pub c: V2,
    pub u: V2,
    pub half: [f32; 2],
    pub y0: f32,
    pub y1: f32,
}

/// The boxes set piece prop `p` blocks a car with (none for paint and the tree's and
/// board's posts, which the collider gives posts).
pub fn slabs(p: &Prop, out: &mut Vec<Slab>) {
    let y = p.y;
    let along = p.dir;
    let across = p.dir.right();
    let mut slab =
        |c: V2, u: V2, half: [f32; 2], y0: f32, y1: f32| out.push(Slab { c, u, half, y0, y1 });
    match p.kind {
        PropKind::TunnelPiece {
            len_dm, width_dm, ..
        } => {
            let (len, w) = (f32::from(len_dm) * 0.1, f32::from(width_dm) * 0.1);
            for sigma in [-1.0f32, 1.0] {
                let c = p.pos + across * (sigma * (0.5 * w + 0.5 * TUNNEL_WALL_M));
                let half = [0.5 * len, 0.5 * TUNNEL_WALL_M];
                slab(c, along, half, y - 1.5, y + TUNNEL_H_M);
            }
        }
        PropKind::GuardRail { len_dm } => slab(
            p.pos,
            along,
            [f32::from(len_dm) * 0.05, 0.15],
            y - 0.5,
            y + 0.85,
        ),
        PropKind::ChainLink { len_dm } => slab(
            p.pos,
            along,
            [f32::from(len_dm) * 0.05, 0.1],
            y - 0.5,
            y + 1.9,
        ),
        PropKind::CrewGarage { .. } => {
            // Frame: x across the front, z out of the bays.
            let (w, d, h) = GARAGE_M;
            let x = p.dir.left();
            let at = |lx: f32, lz: f32| p.pos + x * lx + p.dir * lz;
            let (y0, y1) = (y - 1.0, y + h);
            slab(at(0.0, -d + 0.15), x, [0.5 * w, 0.15], y0, y1);
            for sx in [-1.0f32, 1.0] {
                slab(
                    at(sx * (0.5 * w - 0.15), -0.5 * d),
                    p.dir,
                    [0.5 * d, 0.15],
                    y0,
                    y1,
                );
            }
            for px in piers() {
                let half = [0.5 * pier_w(), 0.25];
                slab(at(px, -0.25), x, half, y0, y1);
            }
        }
        PropKind::TacoStand { .. } => {
            let (w, d, h) = STAND_M;
            let c = p.pos - p.dir * (0.5 * d);
            slab(c, p.dir.left(), [0.5 * w, 0.5 * d], y - 0.5, y + h);
        }
        PropKind::PicnicTable => slab(p.pos, along, [1.0, 0.85], y, y + 0.78),
        _ => {}
    }
}

/// The x of each pier's middle across a garage's front (the two corners and the two
/// between the bays).
fn piers() -> [f32; 4] {
    let (w, _, _) = GARAGE_M;
    let pw = pier_w();
    let half = 0.5 * w - 0.5 * pw;
    let step = BAY_W_M + pw;
    [-half, -half + step, -half + 2.0 * step, half]
}

/// The x of bay `k`'s middle across a garage's front (its frame: x to the left looking out
/// of the bays).
pub fn bay_x(k: usize) -> f32 {
    piers()[k.min(2)] + 0.5 * pier_w() + 0.5 * BAY_W_M
}

/// A garage pier's width: what the front leaves between three bays.
fn pier_w() -> f32 {
    (GARAGE_M.0 - 3.0 * BAY_W_M) / 4.0
}

/// The glazed tile the tunnel is lined with, its darker band, the trim.
fn tile(pal: &Palette) -> (Paint, Paint, Paint) {
    let white = Paint::new(
        shade(mix(pal.stucco_white, pal.sky_blue, 30), 280),
        Pattern::Brick,
    );
    let band = Paint::new(shade(pal.teal, 190), Pattern::Brick);
    let trim = Paint::new(pal.paint_yellow, Pattern::Paint);
    (white, band, trim)
}

/// A length of tunnel: tiled walls (a dark band at the bottom, a trim line), the ceiling
/// with a strip of lamps along each side, the roof slab and outer walls in concrete; a
/// portal's headwall over each open end `portals` says.
fn tunnel(m: &mut WorldMesh, base: V3, along: V2, len: f32, w: f32, portals: u8, pal: &Palette) {
    let f = Frame::upright(base, along);
    let (hl, hw) = (0.5 * len, 0.5 * w);
    let (white, band, trim) = tile(pal);
    let concrete = Paint::new(pal.concrete, Pattern::Concrete);
    let ceiling = Paint::new(shade(pal.concrete_dark, 150), Pattern::Concrete);
    let lamp_rgb: Rgb = [255, 246, 228];
    let lamp = Paint::new(lamp_rgb, Pattern::Plain).glowing(lamp_rgb, Glow::Signal);
    let h = TUNNEL_H_M;
    let t = TUNNEL_WALL_M;
    for sigma in [-1.0f32, 1.0] {
        let z = sigma * hw;
        // Inner face: band, trim, tile; `back` faces the road.
        let back = sigma > 0.0;
        rect(m, &f, -hl, -0.3, hl, 1.2, z, back, band);
        rect(m, &f, -hl, 1.2, hl, 1.35, z, back, trim);
        rect(m, &f, -hl, 1.35, hl, h, z, back, white);
        // Outer face, down into the ground.
        rect(
            m,
            &f,
            -hl,
            -1.5,
            hl,
            h + TUNNEL_ROOF_M,
            z + sigma * t,
            !back,
            concrete,
        );
        // The lamp strip under the ceiling.
        let lz = sigma * (hw - TUNNEL_LAMP_IN_M);
        let (lo, hi) = ([-hl + 0.3, h - 0.12, lz - 0.2], [hl - 0.3, h, lz + 0.2]);
        boxf(m, &f, lo, hi, face::NEG_Y | face::NEG_Z | face::POS_Z, lamp);
    }
    // Ceiling, roof.
    quad(
        m,
        [
            f.p(-hl, h, -hw),
            f.p(hl, h, -hw),
            f.p(hl, h, hw),
            f.p(-hl, h, hw),
        ],
        -f.y,
        ceiling,
    );
    let (ro, rt) = (hw + t, h + TUNNEL_ROOF_M);
    quad(
        m,
        [
            f.p(-hl, rt, -ro),
            f.p(hl, rt, -ro),
            f.p(hl, rt, ro),
            f.p(-hl, rt, ro),
        ],
        f.y,
        concrete,
    );
    for (bit, x, out) in [(1u8, hl, 1.0f32), (2, -hl, -1.0)] {
        if portals & bit == 0 {
            continue;
        }
        // The headwall over the opening and the wall ends, facing out.
        let fx = f.x * out;
        let q = |z: f32, y: f32| f.p(x, y, z);
        quad(
            m,
            [
                q(-ro, h),
                q(ro, h),
                q(ro, rt + PORTAL_M),
                q(-ro, rt + PORTAL_M),
            ],
            fx,
            concrete,
        );
        for sigma in [-1.0f32, 1.0] {
            let (z0, z1) = (sigma * hw, sigma * ro);
            quad(
                m,
                [q(z0, -0.3), q(z1, -0.3), q(z1, h), q(z0, h)],
                fx,
                concrete,
            );
        }
        // A tiled band across the headwall and its coping.
        let qb = |z: f32, y: f32| f.p(x + out * 0.02, y, z);
        quad(
            m,
            [
                qb(-hw, h + 0.3),
                qb(hw, h + 0.3),
                qb(hw, h + 0.9),
                qb(-hw, h + 0.9),
            ],
            fx,
            band,
        );
        let x0 = x.min(x + out * 0.3);
        boxf(
            m,
            &f,
            [x0 - 0.15, rt + PORTAL_M, -ro - 0.1],
            [x0 + 0.45, rt + PORTAL_M + 0.25, ro + 0.1],
            face::NO_BOTTOM,
            concrete,
        );
    }
}

/// Lamps of a start tree, top to bottom: the two staging lamps, three ambers, the green,
/// the red. Each `(y, color)`.
pub const TREE_LAMPS: [(f32, Rgb); 7] = [
    (3.05, [255, 240, 200]),
    (2.82, [255, 240, 200]),
    (2.5, [255, 160, 30]),
    (2.2, [255, 160, 30]),
    (1.9, [255, 160, 30]),
    (1.55, [60, 255, 90]),
    (1.25, [255, 40, 30]),
];
/// The tree's two columns of lamps (one per lane) stand this far either side of its post.
const TREE_COL_M: f32 = 0.24;
/// Half the size of a lamp's lens, m.
const LENS_M: f32 = 0.085;

/// The tree's frame: x across the strip (to the left looking down it), z down the strip.
fn tree_frame(base: V3, facing: V2) -> Frame {
    Frame::upright(base, facing.left())
}

/// A start tree: a post, a black housing and the dark lenses of its [`TREE_LAMPS`] facing
/// back up the strip at the staged cars.
fn start_tree(m: &mut WorldMesh, base: V3, facing: V2, pal: &Palette) {
    let f = tree_frame(base, facing);
    let post = Paint::new(pal.metal_light, Pattern::Metal);
    let housing = Paint::new(shade(pal.metal_dark, 120), Pattern::Metal);
    boxf(
        m,
        &f,
        [-0.08, 0.0, -0.08],
        [0.08, 1.1, 0.08],
        face::NO_BOTTOM,
        post,
    );
    boxf(
        m,
        &f,
        [-0.42, 1.08, -0.1],
        [0.42, 3.25, 0.1],
        face::ALL,
        housing,
    );
    for &(y, rgb) in &TREE_LAMPS {
        let lens = Paint::new(shade(rgb, 70), Pattern::Glass);
        for x in [-TREE_COL_M, TREE_COL_M] {
            let c = f.p(x, y, -0.11);
            let (u, v) = (f.x * LENS_M, f.y * LENS_M);
            quad(m, [c - u - v, c + u - v, c + u + v, c - u + v], -f.z, lens);
        }
    }
}

/// The lit lenses of the tree's lamp `k` (an index into [`TREE_LAMPS`]), both columns, for
/// a tree at the origin facing yaw 0 (the app moves it onto the tree).
pub fn tree_lamps(m: &mut WorldMesh, k: usize) {
    let f = tree_frame(V3::ZERO, crate::props::dir_of_yaw(0.0));
    let (y, rgb) = TREE_LAMPS[k % TREE_LAMPS.len()];
    let lit = Paint::new(rgb, Pattern::Plain).glowing(rgb, Glow::Signal);
    for x in [-TREE_COL_M, TREE_COL_M] {
        let c = f.p(x, y, -0.125);
        let (u, v) = (f.x * (LENS_M * 1.15), f.y * (LENS_M * 1.15));
        quad(m, [c - u - v, c + u - v, c + u + v, c - u + v], -f.z, lit);
    }
}

/// The timing board's panel: its half width, its bottom and top, m.
const BOARD_HALF_M: f32 = 2.6;
const BOARD_Y: (f32, f32) = (2.6, 5.0);
/// Digit cells: height and the step between cells, m.
const DIGIT_H_M: f32 = 0.9;
const DIGIT_STEP_M: f32 = 0.62;
/// The board shows the elapsed time in four digits (two after the point) and the speed
/// at the line in three: [`board_slot`] 0..4 and 4..7.
pub const BOARD_SLOTS: usize = 7;

/// The board's frame: x across its face (left to right as it is read), z out of it.
fn board_frame(base: V3, facing: V2) -> Frame {
    Frame::upright(base, facing.left())
}

/// The x of digit cell `k`'s left edge and its baseline on the board.
fn slot_xy(k: usize) -> (f32, f32) {
    let (y_et, y_spd) = (BOARD_Y.0 + 1.25, BOARD_Y.0 + 0.2);
    if k < 4 {
        // Two digits, the point, two digits.
        let gap = if k >= 2 { 0.25 } else { 0.0 };
        (-1.75 + k as f32 * DIGIT_STEP_M + gap, y_et)
    } else {
        (-1.45 + (k - 4) as f32 * DIGIT_STEP_M, y_spd)
    }
}

/// Where digit cell `k` sits on a board at the origin facing yaw 0: [`digit`] meshes go
/// there.
pub fn board_slot(k: usize) -> V3 {
    let f = board_frame(V3::ZERO, crate::props::dir_of_yaw(0.0));
    let (x, y) = slot_xy(k);
    f.p(x, y, 0.13) - f.p(0.0, 0.0, 0.0)
}

/// A timing board: two legs, a black panel, the point between the time's digits and the
/// time's and speed's labels lettered beside their cells.
fn timing_board(m: &mut WorldMesh, base: V3, facing: V2, pal: &Palette) {
    let f = board_frame(base, facing);
    let leg = Paint::new(pal.metal_light, Pattern::Metal);
    for x in [-BOARD_HALF_M + 0.4, BOARD_HALF_M - 0.4] {
        boxf(
            m,
            &f,
            [x - 0.1, 0.0, -0.1],
            [x + 0.1, BOARD_Y.0, 0.1],
            face::NO_BOTTOM,
            leg,
        );
    }
    let panel = Paint::new([18, 18, 20], Pattern::Plain);
    boxf(
        m,
        &f,
        [-BOARD_HALF_M, BOARD_Y.0, -0.12],
        [BOARD_HALF_M, BOARD_Y.1, 0.12],
        face::ALL,
        panel,
    );
    let amber: Rgb = [255, 170, 40];
    let ink = Paint::new(amber, Pattern::Plain).glowing(amber, Glow::Signal);
    let (x, y) = slot_xy(1);
    let c = f.p(x + DIGIT_STEP_M - 0.02, y + 0.06, 0.125);
    let (u, v) = (f.x * 0.06, f.y * 0.06);
    quad(m, [c - u - v, c + u - v, c + u + v, c - u + v], f.z, ink);
    let label = Paint::new([220, 220, 220], Pattern::Plain).glowing([200, 200, 200], Glow::Signal);
    for (text, k) in [(&b"SEC"[..], 3usize), (&b"MPH"[..], 6)] {
        let (x, y) = slot_xy(k);
        let mut s = letters::layout(text, LetterStyle::Block);
        s.place(0.3, x + DIGIT_STEP_M + 0.1, y);
        letters::flat(m, &f, &s, 0.125, false, label);
    }
}

/// Segments of a seven-segment digit, `[x0, y0, x1, y1]` in a cell 0.5 wide and
/// [`DIGIT_H_M`] tall: a, b, c, d, e, f, g.
const SEGMENTS: [[f32; 4]; 7] = [
    [0.06, 0.82, 0.44, 0.9],
    [0.42, 0.47, 0.5, 0.86],
    [0.42, 0.04, 0.5, 0.43],
    [0.06, 0.0, 0.44, 0.08],
    [0.0, 0.04, 0.08, 0.43],
    [0.0, 0.47, 0.08, 0.86],
    [0.06, 0.41, 0.44, 0.49],
];
/// Which segments each digit lights (bit 0 is a).
const DIGITS: [u8; 10] = [0x3F, 0x06, 0x5B, 0x4F, 0x66, 0x6D, 0x7D, 0x07, 0x7F, 0x6F];

/// Digit `d` (0..10) in lit amber segments, its cell's lower left at the origin of a board
/// facing yaw 0 (the app moves it onto [`board_slot`]).
pub fn digit(m: &mut WorldMesh, d: usize) {
    let f = board_frame(V3::ZERO, crate::props::dir_of_yaw(0.0));
    let amber: Rgb = [255, 170, 40];
    let lit = Paint::new(amber, Pattern::Plain).glowing(amber, Glow::Signal);
    let bits = DIGITS[d % 10];
    let s = DIGIT_H_M / 0.9;
    for (k, seg) in SEGMENTS.iter().enumerate() {
        if bits >> k & 1 == 1 {
            let [x0, y0, x1, y1] = seg.map(|v| v * s);
            rect(m, &f, x0, y0, x1, y1, 0.0, false, lit);
        }
    }
}

/// A painted line across the road, 0.3 m deep along `along`, `len` across it.
fn strip_line(m: &mut WorldMesh, base: V3, along: V2, len: f32, pal: &Palette) {
    let f = Frame::upright(base, along);
    let white = Paint::new(pal.paint_white, Pattern::Paint);
    let (hx, hz) = (0.15, 0.5 * len);
    let y = 0.03;
    quad(
        m,
        [
            f.p(-hx, y, -hz),
            f.p(hx, y, -hz),
            f.p(hx, y, hz),
            f.p(-hx, y, hz),
        ],
        f.y,
        white,
    );
}

/// Steel beam on wood posts every 2 m, along `along`.
fn guard_rail(m: &mut WorldMesh, base: V3, along: V2, len: f32, pal: &Palette) {
    let f = Frame::upright(base, along);
    let half = 0.5 * len;
    let wood = Paint::new(shade(pal.bark, 300), Pattern::Plain);
    let steel = Paint::new(pal.metal_light, Pattern::Metal);
    let posts = ((len / 2.0) as u32).max(1);
    for k in 0..=posts {
        let x = (-half + len * k as f32 / posts as f32).clamp(-half + 0.08, half - 0.08);
        boxf(
            m,
            &f,
            [x - 0.08, -0.5, -0.2],
            [x + 0.08, 0.75, -0.05],
            face::NO_BOTTOM,
            wood,
        );
    }
    boxf(
        m,
        &f,
        [-half, 0.48, -0.05],
        [half, 0.8, 0.05],
        face::ALL,
        steel,
    );
}

/// Chain-link on steel posts every 3 m with a top rail, along `along`: the fabric a dull
/// grey sheet seen from both sides.
fn chain_link(m: &mut WorldMesh, base: V3, along: V2, len: f32, pal: &Palette) {
    let f = Frame::upright(base, along);
    let half = 0.5 * len;
    let steel = Paint::new(pal.metal_light, Pattern::Metal);
    let fabric = Paint::new(mix(pal.metal_light, pal.metal_dark, 140), Pattern::Metal);
    let h = 1.85;
    let posts = ((len / 3.0) as u32).max(1);
    for k in 0..=posts {
        let x = (-half + len * k as f32 / posts as f32).clamp(-half + 0.04, half - 0.04);
        boxf(
            m,
            &f,
            [x - 0.04, -0.4, -0.04],
            [x + 0.04, h + 0.05, 0.04],
            face::NO_BOTTOM,
            steel,
        );
    }
    boxf(
        m,
        &f,
        [-half, h - 0.03, -0.03],
        [half, h + 0.03, 0.03],
        face::NO_BOTTOM,
        steel,
    );
    rect(m, &f, -half, 0.05, half, h, 0.0, false, fabric);
    rect(m, &f, -half, 0.05, half, h, 0.0, true, fabric);
}

/// The words of a sign named by `seed` from `lex`: a name, then the first of `trades`
/// the lexicon lists (picked by the seed); nothing from an empty lexicon.
fn sign_words(
    lex: &Lexicon,
    seed: u64,
    trades: &[&str],
    surname: bool,
) -> Option<(Vec<u8>, Vec<u8>)> {
    let names: &[alloc::string::String] = if surname { &lex.surnames } else { &lex.given };
    if names.is_empty() {
        return None;
    }
    let name = &names[(seed % names.len() as u64) as usize];
    let have: Vec<&&str> = trades
        .iter()
        .filter(|t| lex.trades.iter().any(|w| w == *t))
        .collect();
    let trade = have.get(((seed >> 20) % have.len().max(1) as u64) as usize)?;
    Some((name.as_bytes().to_vec(), trade.as_bytes().to_vec()))
}

/// The trades a crew's garage may letter, as the 2001 lexicon has them.
const GARAGE_TRADES: [&str; 5] = ["GARAGE", "BODYWORK", "WHEELS", "TINT", "CAR AUDIO"];
/// A taco stand's.
const STAND_TRADES: [&str; 2] = ["TACOS", "TAQUERIA"];

/// Letters `text` centered on `x = 0` in the frame's plane at depth `z`, at most `cap`
/// tall and `max_w` wide, its baseline at `y`.
#[allow(clippy::too_many_arguments)]
fn centered(
    m: &mut WorldMesh,
    f: &Frame,
    text: &[u8],
    cap: f32,
    max_w: f32,
    y: f32,
    z: f32,
    paint: Paint,
) {
    let mut s = letters::layout(text, LetterStyle::Block);
    let w = s.ink_w().max(1e-3);
    let size = cap.min(max_w / w);
    s.place(size, -0.5 * w * size, y);
    letters::flat(m, f, &s, z, false, paint);
}

/// A three-bay block garage: stucco walls, the bays open with their roll-up doors drawn
/// up, a lit interior (pale floor and back wall, fluorescent tubes) and a backlit box sign
/// over the bays lettered with the crew's shop name. `facing`: out of the bays.
fn garage(m: &mut WorldMesh, base: V3, facing: V2, seed: u64, pal: &Palette, lex: &Lexicon) {
    let f = Frame::upright(base, facing.left());
    let (w, d, h) = GARAGE_M;
    let hw = 0.5 * w;
    let wall = Paint::new(mix(pal.stucco_cream, pal.concrete, 110), Pattern::Stucco);
    let trim = Paint::new(shade(pal.teal, 160), Pattern::Paint);
    let floor = Paint::new(shade(pal.concrete, 230), Pattern::Concrete);
    let inside = Paint::new(shade(pal.concrete, 250), Pattern::Concrete);
    let door = Paint::new(shade(pal.metal_light, 200), Pattern::Metal);
    let roof = Paint::new(pal.roof_gravel, Pattern::RoofGravel);
    // Floor slab and walls (outer faces, inner faces lit pale).
    boxf(
        m,
        &f,
        [-hw, -0.4, -d],
        [hw, 0.08, 0.0],
        face::POS_Y | face::POS_Z,
        floor,
    );
    boxf(
        m,
        &f,
        [-hw, -1.0, -d],
        [hw, h, -d + 0.3],
        face::ALL & !face::NEG_Y,
        wall,
    );
    for sx in [-1.0f32, 1.0] {
        let (x0, x1) = if sx < 0.0 {
            (-hw, -hw + 0.3)
        } else {
            (hw - 0.3, hw)
        };
        boxf(
            m,
            &f,
            [x0, -1.0, -d + 0.3],
            [x1, h, 0.0],
            face::ALL & !face::NEG_Y,
            wall,
        );
    }
    rect(
        m,
        &f,
        -hw + 0.3,
        0.08,
        hw - 0.3,
        h - 0.3,
        -d + 0.31,
        false,
        inside,
    );
    // Piers and the header over the bays.
    let pw = pier_w();
    for px in piers() {
        boxf(
            m,
            &f,
            [px - 0.5 * pw, -1.0, -0.5],
            [px + 0.5 * pw, h, 0.0],
            face::ALL & !face::NEG_Y,
            wall,
        );
    }
    boxf(
        m,
        &f,
        [-hw, BAY_H_M, -0.5],
        [hw, h, 0.0],
        face::ALL & !face::NEG_Y,
        wall,
    );
    boxf(
        m,
        &f,
        [-hw, BAY_H_M + 0.3, 0.0],
        [hw, BAY_H_M + 0.5, 0.06],
        face::NO_BOTTOM,
        trim,
    );
    // Roll-up doors drawn up into their drums under the header.
    for k in 0..3 {
        let x0 = piers()[k] + 0.5 * pw;
        boxf(
            m,
            &f,
            [x0, BAY_H_M - 0.45, -0.95],
            [x0 + BAY_W_M, BAY_H_M, -0.5],
            face::NEG_Y | face::POS_Z,
            door,
        );
    }
    // Ceiling and roof; the parapet's cap.
    quad(
        m,
        [
            f.p(-hw, h - 0.3, -d),
            f.p(hw, h - 0.3, -d),
            f.p(hw, h - 0.3, 0.0),
            f.p(-hw, h - 0.3, 0.0),
        ],
        -f.y,
        inside,
    );
    quad(
        m,
        [
            f.p(-hw, h, -d),
            f.p(hw, h, -d),
            f.p(hw, h, 0.0),
            f.p(-hw, h, 0.0),
        ],
        f.y,
        roof,
    );
    // Fluorescent tubes across the ceiling, one per bay.
    let tube_rgb: Rgb = [235, 245, 255];
    let tube = Paint::new(tube_rgb, Pattern::Plain).glowing(tube_rgb, Glow::Window);
    for k in 0..3 {
        let x = piers()[k] + 0.5 * pw + 0.5 * BAY_W_M;
        for z in [-0.3 * d, -0.7 * d] {
            boxf(
                m,
                &f,
                [x - 1.2, h - 0.42, z - 0.08],
                [x + 1.2, h - 0.3, z + 0.08],
                face::NEG_Y,
                tube,
            );
        }
    }
    // The box sign over the bays: a lit panel, dark letters.
    let sign_rgb: Rgb = [250, 246, 232];
    let panel = Paint::new(sign_rgb, Pattern::Plain).glowing([110, 106, 96], Glow::Neon);
    let (sx, sy0, sy1) = (0.36 * w, h + 0.1, h + 1.7);
    let frame = Paint::new(shade(pal.metal_dark, 140), Pattern::Metal);
    boxf(
        m,
        &f,
        [-sx - 0.1, sy0 - 0.1, -0.35],
        [sx + 0.1, sy1 + 0.1, 0.05],
        face::ALL,
        frame,
    );
    rect(m, &f, -sx, sy0, sx, sy1, 0.06, false, panel);
    boxf(
        m,
        &f,
        [-0.1, h - 0.1, -0.3],
        [0.1, sy0, -0.1],
        face::NO_BOTTOM,
        frame,
    );
    if let Some((name, trade)) = sign_words(lex, seed, &GARAGE_TRADES, true) {
        let ink = Paint::new([30, 26, 40], Pattern::Plain);
        centered(m, &f, &name, 0.75, 2.0 * sx - 0.6, sy0 + 0.7, 0.07, ink);
        let red = Paint::new(pal.neon_red, Pattern::Plain);
        centered(m, &f, &trade, 0.4, 2.0 * sx - 0.6, sy0 + 0.15, 0.07, red);
    }
}

/// A walk-up taco stand: a small stucco box, a serving window under a striped awning, a
/// lit menu beside it and a lit sign on the roof lettered with its name.
fn taco_stand(m: &mut WorldMesh, base: V3, facing: V2, seed: u64, pal: &Palette, lex: &Lexicon) {
    let f = Frame::upright(base, facing.left());
    let (w, d, h) = STAND_M;
    let hw = 0.5 * w;
    let wall = Paint::new(pal.salmon, Pattern::Stucco);
    boxf(
        m,
        &f,
        [-hw, -0.5, -d],
        [hw, h, 0.0],
        face::ALL & !face::NEG_Y,
        wall,
    );
    // The window: a warm lit opening with a counter.
    let warm = pal.window_warm;
    let window = Paint::new(warm, Pattern::Glass).glowing(warm, Glow::Window);
    rect(m, &f, -1.6, 1.0, 0.6, 2.1, 0.01, false, window);
    let counter = Paint::new(pal.metal_light, Pattern::Metal);
    boxf(
        m,
        &f,
        [-1.7, 0.95, 0.0],
        [0.7, 1.02, 0.35],
        face::ALL,
        counter,
    );
    // The awning over it, striped in two paints.
    let stripes = [
        Paint::new(pal.paint_white, Pattern::Awning),
        Paint::new(pal.neon_green, Pattern::Awning),
    ];
    for k in 0..6 {
        let x0 = -1.8 + k as f32 * 0.45;
        quad(
            m,
            [
                f.p(x0, 2.4, 0.0),
                f.p(x0 + 0.45, 2.4, 0.0),
                f.p(x0 + 0.45, 2.15, 1.1),
                f.p(x0, 2.15, 1.1),
            ],
            f.y + f.z,
            stripes[k % 2],
        );
    }
    // The lit menu board.
    let menu_rgb: Rgb = [255, 236, 190];
    let menu = Paint::new(menu_rgb, Pattern::Plain).glowing([120, 108, 84], Glow::Neon);
    rect(m, &f, 1.0, 1.1, 2.3, 2.3, 0.01, false, menu);
    let ink = Paint::new([60, 30, 20], Pattern::Plain);
    for k in 0..4 {
        let y = 1.3 + k as f32 * 0.25;
        rect(
            m,
            &f,
            1.15,
            y,
            2.15 - (k % 2) as f32 * 0.3,
            y + 0.06,
            0.02,
            false,
            ink,
        );
    }
    // The roof sign.
    let sign_rgb: Rgb = [255, 210, 80];
    let panel = Paint::new(sign_rgb, Pattern::Plain).glowing([130, 104, 40], Glow::Neon);
    let post = Paint::new(pal.metal_dark, Pattern::Metal);
    boxf(
        m,
        &f,
        [-hw + 0.2, h, -0.6],
        [hw - 0.2, h + 1.3, -0.4],
        face::ALL,
        post,
    );
    rect(
        m,
        &f,
        -hw + 0.3,
        h + 0.1,
        hw - 0.3,
        h + 1.2,
        -0.39,
        false,
        panel,
    );
    if let Some((name, trade)) = sign_words(lex, seed, &STAND_TRADES, false) {
        let red = Paint::new(pal.neon_red, Pattern::Plain);
        centered(m, &f, &name, 0.42, w - 0.9, h + 0.65, -0.38, red);
        let dark = Paint::new([40, 30, 20], Pattern::Plain);
        centered(m, &f, &trade, 0.3, w - 0.9, h + 0.22, -0.38, dark);
    }
}

/// A picnic table with its two benches, along `along`.
fn picnic_table(m: &mut WorldMesh, base: V3, along: V2, pal: &Palette) {
    let f = Frame::upright(base, along);
    let wood = Paint::new(mix(pal.bark, pal.sand, 110), Pattern::Plain);
    boxf(m, &f, [-0.9, 0.72, -0.4], [0.9, 0.78, 0.4], face::ALL, wood);
    for x in [-0.7f32, 0.7] {
        boxf(
            m,
            &f,
            [x - 0.05, 0.0, -0.05],
            [x + 0.05, 0.72, 0.05],
            face::NO_BOTTOM,
            wood,
        );
    }
    for z in [-0.75f32, 0.75] {
        boxf(
            m,
            &f,
            [-0.9, 0.42, z - 0.13],
            [0.9, 0.47, z + 0.13],
            face::ALL,
            wood,
        );
        for x in [-0.7f32, 0.7] {
            boxf(
                m,
                &f,
                [x - 0.04, 0.0, z - 0.04],
                [x + 0.04, 0.42, z + 0.04],
                face::NO_BOTTOM,
                wood,
            );
        }
    }
}

/// A crossing gate's frame: x across the road (its middle toward -x), z toward the drivers
/// it stops.
fn gate_frame(base: V3, facing: V2) -> Frame {
    Frame::upright(base, facing.left())
}

/// Where a gate's arm pivots in its frame, and the height of a lowered arm.
const GATE_PIVOT: [f32; 3] = [-0.35, 1.05, 0.25];
/// A gate's two flashing lamps in its frame (their middles), both facing the drivers.
const GATE_LAMPS: [[f32; 3]; 2] = [[-0.42, 2.6, 0.12], [0.42, 2.6, 0.12]];

/// Where a gate at the origin facing yaw 0 pivots its arm: the app turns [`gate_arm`]
/// about the frame's z (the way the gate faces) there, up from lowered.
pub fn gate_pivot() -> V3 {
    let f = gate_frame(V3::ZERO, crate::props::dir_of_yaw(0.0));
    f.p(GATE_PIVOT[0], GATE_PIVOT[1], GATE_PIVOT[2])
}

/// The way a lowered arm points for a gate facing yaw 0 (across the road), and the axis it
/// turns up about (the way the gate faces).
pub fn gate_axes() -> (V3, V3) {
    let f = gate_frame(V3::ZERO, crate::props::dir_of_yaw(0.0));
    (-f.x, f.z)
}

/// A crossing gate's mast: a post on a base, the crossed white blades, the twin lamps on
/// their crossarm (dark: the app lights them), a bell on top, and the arm's pivot box with
/// its counterweight. The arm itself is [`gate_arm`].
fn crossing_gate(m: &mut WorldMesh, base: V3, facing: V2, pal: &Palette) {
    let f = gate_frame(base, facing);
    let post = Paint::new(pal.metal_light, Pattern::Metal);
    let dark = Paint::new(shade(pal.metal_dark, 120), Pattern::Metal);
    let concrete = Paint::new(pal.concrete, Pattern::Concrete);
    boxf(
        m,
        &f,
        [-0.3, -0.3, -0.3],
        [0.3, 0.12, 0.3],
        face::NO_BOTTOM,
        concrete,
    );
    boxf(
        m,
        &f,
        [-0.07, 0.12, -0.07],
        [0.07, 4.1, 0.07],
        face::NO_BOTTOM,
        post,
    );
    let white = Paint::new(pal.paint_white, Pattern::Paint);
    let c = f.p(0.0, 3.55, 0.08);
    for (dx, dy) in [(0.69f32, 0.72f32), (0.69, -0.72)] {
        let (u, v) = (f.x * dx + f.y * dy, f.x * -dy + f.y * dx);
        let (u, v) = (u * 0.8, v * 0.1);
        quad(m, [c - u - v, c + u - v, c + u + v, c - u + v], f.z, white);
        quad(m, [c - u - v, c - u + v, c + u + v, c + u - v], -f.z, post);
    }
    boxf(m, &f, [-0.6, 2.55, 0.0], [0.6, 2.65, 0.08], face::ALL, post);
    let lens = Paint::new(shade(pal.neon_red, 90), Pattern::Glass);
    for l in GATE_LAMPS {
        boxf(
            m,
            &f,
            [l[0] - 0.17, l[1] - 0.17, 0.0],
            [l[0] + 0.17, l[1] + 0.17, 0.1],
            face::NO_BOTTOM,
            dark,
        );
        let q = f.p(l[0], l[1], l[2] + 0.0);
        let (u, v) = (f.x * 0.12, f.y * 0.12);
        quad(m, [q - u - v, q + u - v, q + u + v, q - u + v], f.z, lens);
    }
    // The bell on top.
    let bell = Paint::new(shade(pal.metal_dark, 200), Pattern::Metal);
    boxf(
        m,
        &f,
        [-0.14, 4.1, -0.14],
        [0.14, 4.3, 0.14],
        face::NO_BOTTOM,
        bell,
    );
    // The pivot's box and the counterweight behind the post.
    let [px, py, pz] = GATE_PIVOT;
    boxf(
        m,
        &f,
        [px - 0.18, py - 0.25, pz - 0.15],
        [px + 0.18, py + 0.25, pz + 0.15],
        face::ALL,
        dark,
    );
    boxf(
        m,
        &f,
        [px + 0.15, py - 0.12, pz - 0.08],
        [px + 0.95, py + 0.12, pz + 0.08],
        face::ALL,
        dark,
    );
}

/// A gate's arm `len` long for a gate at the origin facing yaw 0, lowered: from its pivot
/// ([`gate_pivot`] is the app's to add) out along [`gate_axes`]'s first axis, in red and
/// white bands with three lamps along its top.
pub fn gate_arm(m: &mut WorldMesh, len: f32, pal: &Palette) {
    let f = gate_frame(V3::ZERO, crate::props::dir_of_yaw(0.0));
    let red = Paint::new(pal.neon_red, Pattern::Paint);
    let white = Paint::new(pal.paint_white, Pattern::Paint);
    let bands = ((len / 0.6) as usize).max(1);
    let step = len / bands as f32;
    for k in 0..bands {
        let (x0, x1) = (-(k as f32) * step, -((k + 1) as f32) * step);
        let paint = if k % 2 == 0 { red } else { white };
        boxf(
            m,
            &f,
            [x1, -0.05, -0.04],
            [x0, 0.05, 0.04],
            face::ALL,
            paint,
        );
    }
    let lamp = Paint::new(shade(pal.neon_red, 90), Pattern::Glass);
    for t in [0.33f32, 0.66, 0.97] {
        let x = -t * len;
        boxf(
            m,
            &f,
            [x - 0.06, 0.05, -0.05],
            [x + 0.06, 0.15, 0.05],
            face::NO_BOTTOM,
            lamp,
        );
    }
}

/// Gate lamp `k` (0 or 1) lit, for a gate at the origin facing yaw 0.
pub fn gate_lamp(m: &mut WorldMesh, k: usize) {
    let f = gate_frame(V3::ZERO, crate::props::dir_of_yaw(0.0));
    let red: Rgb = [255, 40, 30];
    let lit = Paint::new(red, Pattern::Plain).glowing(red, Glow::Signal);
    let l = GATE_LAMPS[k % 2];
    let q = f.p(l[0], l[1], l[2] + 0.01);
    let (u, v) = (f.x * 0.13, f.y * 0.13);
    quad(m, [q - u - v, q + u - v, q + u + v, q - u + v], f.z, lit);
}

/// The train's body kinds: the locomotive, then the freight cars behind it.
pub const TRAIN_LOCO: u8 = 0;
pub const TRAIN_BOXCAR: u8 = 1;
pub const TRAIN_TANK: u8 = 2;
pub const TRAIN_HOPPER: u8 = 3;

/// A train body of `kind`, `len` long, standing on the rail top at the origin with its
/// front toward yaw 0's facing (`-z`): a hood-and-cab locomotive with its headlight lit, a
/// boxcar, a tank car or a covered hopper, each on two trucks; in freight colors.
pub fn train_body(m: &mut WorldMesh, kind: u8, len: f32, pal: &Palette) {
    let f = Frame::upright(V3::ZERO, crate::props::dir_of_yaw(0.0));
    let hl = 0.5 * len;
    let truck = Paint::new(shade(pal.metal_dark, 110), Pattern::Metal);
    let frame = Paint::new(shade(pal.metal_dark, 150), Pattern::Metal);
    // Trucks and wheels' shadow boxes, the frame over them.
    for x in [hl - 3.0, -hl + 3.0] {
        boxf(
            m,
            &f,
            [x - 1.4, 0.0, -1.2],
            [x + 1.4, 0.9, 1.2],
            face::NO_BOTTOM,
            truck,
        );
    }
    boxf(m, &f, [-hl, 0.9, -1.45], [hl, 1.25, 1.45], face::ALL, frame);
    let w = 1.55;
    match kind {
        TRAIN_LOCO => {
            let body = Paint::new([196, 120, 40], Pattern::Paint);
            let stripe = Paint::new(pal.paint_yellow, Pattern::Paint);
            let glass = Paint::new(pal.glass, Pattern::Glass);
            // The cab at the front, the long hood behind it.
            boxf(
                m,
                &f,
                [hl - 5.0, 1.25, -w],
                [hl - 0.6, 4.4, w],
                face::NO_BOTTOM,
                body,
            );
            boxf(
                m,
                &f,
                [-hl + 0.6, 1.25, -1.2],
                [hl - 5.0, 4.0, 1.2],
                face::NO_BOTTOM,
                body,
            );
            boxf(
                m,
                &f,
                [hl - 0.6, 1.25, -w],
                [hl, 2.6, w],
                face::NO_BOTTOM,
                body,
            );
            for z in [-w - 0.01, w + 0.01] {
                rect(m, &f, -hl + 0.6, 1.6, hl - 0.6, 1.8, z, z < 0.0, stripe);
            }
            let fx = hl - 0.6 + 0.01;
            let q = |y: f32, z: f32| f.p(fx, y, z);
            quad(
                m,
                [q(3.3, -1.3), q(3.3, 1.3), q(4.1, 1.3), q(4.1, -1.3)],
                f.x,
                glass,
            );
            let lamp: Rgb = [255, 244, 210];
            let lit = Paint::new(lamp, Pattern::Plain).glowing(lamp, Glow::Signal);
            let fx = hl + 0.01;
            let q = |y: f32, z: f32| f.p(fx, y, z);
            quad(
                m,
                [q(2.0, -0.25), q(2.0, 0.25), q(2.4, 0.25), q(2.4, -0.25)],
                f.x,
                lit,
            );
        }
        TRAIN_TANK => {
            let body = Paint::new([74, 76, 82], Pattern::Metal);
            crate::mesh::frustum(
                m,
                &Frame {
                    o: f.p(-hl + 0.6, 2.6, 0.0),
                    x: f.y,
                    y: f.x,
                    z: -f.z,
                },
                1.4,
                1.4,
                len - 1.2,
                10,
                true,
                true,
                body,
            );
        }
        TRAIN_HOPPER => {
            let body = Paint::new([150, 146, 136], Pattern::Metal);
            boxf(
                m,
                &f,
                [-hl + 0.4, 1.25, -w],
                [hl - 0.4, 4.2, w],
                face::NO_BOTTOM,
                body,
            );
            let rib = body.with_rgb(shade(body.rgb(), 200));
            let ribs = ((len / 2.0) as usize).max(1);
            for k in 1..ribs {
                let x = -hl + 0.4 + k as f32 * (len - 0.8) / ribs as f32;
                boxf(
                    m,
                    &f,
                    [x - 0.06, 1.25, -w - 0.06],
                    [x + 0.06, 4.2, w + 0.06],
                    face::NEG_Z | face::POS_Z,
                    rib,
                );
            }
        }
        _ => {
            let body = Paint::new([150, 78, 48], Pattern::Metal);
            boxf(
                m,
                &f,
                [-hl + 0.3, 1.25, -w],
                [hl - 0.3, 4.3, w],
                face::NO_BOTTOM,
                body,
            );
            let door = body.with_rgb(shade(body.rgb(), 170));
            for z in [-w - 0.02, w + 0.02] {
                rect(m, &f, -1.5, 1.4, 1.5, 3.9, z, z < 0.0, door);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mesh::tests::assert_well_formed;

    fn prop(kind: PropKind) -> Prop {
        Prop {
            pos: V2::new(10.0, -4.0),
            y: 2.0,
            dir: V2::new(0.6, 0.8),
            kind,
            seed: 7,
        }
    }

    const KINDS: [PropKind; 10] = [
        PropKind::TunnelPiece {
            len_dm: 160,
            width_dm: 250,
            portals: 3,
        },
        PropKind::StartTree,
        PropKind::TimingBoard,
        PropKind::StripLine { len_dm: 280 },
        PropKind::GuardRail { len_dm: 80 },
        PropKind::ChainLink { len_dm: 120 },
        PropKind::CrewGarage { name_seed: 11 },
        PropKind::TacoStand { name_seed: 12 },
        PropKind::PicnicTable,
        PropKind::CrossingGate { arm_dm: 80 },
    ];

    #[test]
    fn every_set_piece_builds_a_well_formed_mesh() {
        let pal = Palette::socal_1985();
        let lex = Lexicon::default();
        for kind in KINDS {
            assert!(is_set_piece(kind));
            let mut m = WorldMesh::default();
            build(&mut m, &prop(kind), &pal, &lex);
            assert!(!m.indices.is_empty(), "{kind:?}");
            assert_well_formed(&m, 0.99);
        }
        for k in 0..TREE_LAMPS.len() {
            let mut m = WorldMesh::default();
            tree_lamps(&mut m, k);
            assert_eq!(m.triangle_count(), 4);
        }
        for d in 0..10 {
            let mut m = WorldMesh::default();
            digit(&mut m, d);
            assert!(m.triangle_count() >= 4);
        }
        for kind in [TRAIN_LOCO, TRAIN_BOXCAR, TRAIN_TANK, TRAIN_HOPPER] {
            let mut m = WorldMesh::default();
            train_body(&mut m, kind, 16.5, &pal);
            assert!(m.triangle_count() > 20);
            // Its surfaces face out: away from its middle line.
            for v in &m.vertices {
                let out = [v.pos[0], v.pos[1] - 2.6, 0.0];
                let dot = out[0] * v.normal[0] + out[1] * v.normal[1];
                if kind == TRAIN_TANK && v.pos[1] > 1.3 && v.normal[2].abs() < 0.5 {
                    assert!(dot > 0.0, "an inward face at {:?} {:?}", v.pos, v.normal);
                }
            }
            // Within its length and the loading gauge.
            for v in &m.vertices {
                assert!(v.pos[2].abs() <= 8.26 && v.pos[0].abs() <= 1.62 && v.pos[1] <= 4.41);
            }
        }
        let mut m = WorldMesh::default();
        gate_arm(&mut m, 7.4, &pal);
        assert_well_formed(&m, 0.99);
        for k in 0..2 {
            let mut m = WorldMesh::default();
            gate_lamp(&mut m, k);
            assert_eq!(m.triangle_count(), 2);
        }
        // A lowered arm runs level across the road from its pivot, a meter up.
        let (along, up_axis) = gate_axes();
        assert!(along.y.abs() < 1e-6 && (up_axis.length() - 1.0).abs() < 1e-5);
        assert!((gate_pivot().y - 1.05).abs() < 1e-6);
    }

    #[test]
    fn a_tunnel_walls_its_road_and_a_garage_its_bays() {
        let mut out = Vec::new();
        slabs(&prop(KINDS[0]), &mut out);
        assert_eq!(out.len(), 2);
        // The walls stand just outside the clear width, either side.
        for s in &out {
            let off = (s.c - V2::new(10.0, -4.0)).length();
            assert!((off - 12.8).abs() < 1e-3, "{off}");
        }
        out.clear();
        slabs(&prop(KINDS[6]), &mut out);
        // Back wall, two side walls, four piers; the bays between the piers are open.
        assert_eq!(out.len(), 7);
        let bays = BAY_W_M * 3.0 + 4.0 * pier_w();
        assert!((bays - GARAGE_M.0).abs() < 1e-4);
    }

    #[test]
    fn the_board_slots_read_left_to_right() {
        let x = |k: usize| board_slot(k).dot(V3::new(-1.0, 0.0, 0.0));
        for k in 1..4 {
            assert!(x(k) > x(k - 1));
        }
        assert!(board_slot(4).y < board_slot(0).y);
    }
}
