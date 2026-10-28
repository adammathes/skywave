//! Landmarks built from kits (`docs/WORLD_GENERATION.md`, "Landmarks";
//! `docs/adr/0007-landmark-kits.md`): a [`Kit`] is a small RON file
//! (`assets/landmarks/<id>.ron`) of boxes, struts, cylinders, lathed profiles and lamps in
//! named paints, with lettered variants that each restate or extend an earlier one. This
//! module defines the types (serde derives behind the `serde` feature), checks a kit
//! ([`Kit::validate`]) and builds a variant's mesh, collision boxes and lights; `app`
//! parses the files and attaches them to the grammar ([`crate::Grammar::landmarks`]).
//!
//! It also lays out the landmark gallery ([`gallery`]): every kit's variants side by side
//! on flat lots along a road, each with a lettered lot label, so a screenshot tool can put
//! a variant's look in front of the owner beside its siblings.
//!
//! Kit frame: `x` across the lot (to the right seen from the road), `y` up from the lot,
//! `z` toward the road. A lot spans `x` in `-w/2..w/2` and `z` in `-d..0`; its prop's
//! point is the middle of its front edge.

use alloc::string::String;
use alloc::vec::Vec;
use core::fmt;

use place::{
    Edge, Era, Light as PlaceLight, LightKind, Node, Place, PlaceHeader, Prop, PropKind,
    RegionId, RoadClass, RoadGraph, Spawn, SpawnKind, Terrain, ZoneId, Zones,
};

use crate::math::{abs, floor, V2, V3};
use crate::mesh::{boxf, face, lathe, quad, rect, Frame, Glow, Paint, Pattern, WorldMesh};
use crate::palette::{shade, srgb, Palette, Rgb};
use crate::setpieces::Slab;
use crate::signs::letters::{self, LetterStyle};

/// The most variants a kit may have: one per capital letter.
pub const MAX_VARIANTS: usize = 26;
/// A lot's smallest and largest side, m.
pub const LOT_M: (f32, f32) = (4.0, 80.0);
/// The tallest a landmark may stand over its lot, m.
pub const MAX_HEIGHT_M: f32 = 60.0;
/// The most triangles a variant may build to (lot pad included): a landmark is drawn
/// once, so it may be richer than a building (`docs/RENDERING.md` budgets), not unbounded.
pub const MAX_TRIANGLES: usize = 8000;
/// Facets around a cylinder or lathe.
pub const SIDES: (u8, u8) = (3, 48);
/// Parts below this height over the lot block a car ([`slabs`]), m.
const SOLID_BELOW_M: f32 = 2.0;

/// What a landmark is like (the owner's taste default: mostly grounded, a few pieces of
/// roadside kitsch).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum Tone {
    /// A real kind of structure a place would have: a water tower, a sign pylon.
    Grounded,
    /// Roadside exaggeration: something oversized or playful built to be seen from a car.
    Kitsch,
}

/// A color of the place's palette, by its field name.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum Swatch {
    StuccoCream,
    StuccoWhite,
    Salmon,
    PastelPink,
    Peach,
    Mint,
    SkyBlue,
    Lavender,
    Sand,
    BrickRed,
    BrickBrown,
    Teal,
    Terracotta,
    Navy,
    Concrete,
    ConcreteDark,
    Asphalt,
    PaintWhite,
    PaintYellow,
    RoofGravel,
    Lawn,
    Bark,
    Glass,
    MetalDark,
    MetalLight,
    HydrantRed,
    Sodium,
    WindowWarm,
    NeonPink,
    NeonCyan,
    NeonRed,
    NeonGreen,
}

impl Swatch {
    pub fn rgb(self, p: &Palette) -> Rgb {
        match self {
            Swatch::StuccoCream => p.stucco_cream,
            Swatch::StuccoWhite => p.stucco_white,
            Swatch::Salmon => p.salmon,
            Swatch::PastelPink => p.pastel_pink,
            Swatch::Peach => p.peach,
            Swatch::Mint => p.mint,
            Swatch::SkyBlue => p.sky_blue,
            Swatch::Lavender => p.lavender,
            Swatch::Sand => p.sand,
            Swatch::BrickRed => p.brick_red,
            Swatch::BrickBrown => p.brick_brown,
            Swatch::Teal => p.teal,
            Swatch::Terracotta => p.terracotta,
            Swatch::Navy => p.navy,
            Swatch::Concrete => p.concrete,
            Swatch::ConcreteDark => p.concrete_dark,
            Swatch::Asphalt => p.asphalt,
            Swatch::PaintWhite => p.paint_white,
            Swatch::PaintYellow => p.paint_yellow,
            Swatch::RoofGravel => p.roof_gravel,
            Swatch::Lawn => p.lawn,
            Swatch::Bark => p.bark,
            Swatch::Glass => p.glass,
            Swatch::MetalDark => p.metal_dark,
            Swatch::MetalLight => p.metal_light,
            Swatch::HydrantRed => p.hydrant_red,
            Swatch::Sodium => p.sodium,
            Swatch::WindowWarm => p.window_warm,
            Swatch::NeonPink => p.neon_pink,
            Swatch::NeonCyan => p.neon_cyan,
            Swatch::NeonRed => p.neon_red,
            Swatch::NeonGreen => p.neon_green,
        }
    }
}

/// A color: the palette's (so a landmark dresses like its world) or a fixed one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum Color {
    Pal(Swatch),
    /// `0xRRGGBB` in sRGB, as a painter picks it.
    Srgb(u32),
    /// A palette color scaled by `n / 256` (darker below 256, brighter above).
    Shade(Swatch, u16),
}

impl Color {
    pub fn rgb(self, p: &Palette) -> Rgb {
        match self {
            Color::Pal(s) => s.rgb(p),
            Color::Srgb(hex) => srgb(hex),
            Color::Shade(s, n) => shade(s.rgb(p), u32::from(n)),
        }
    }
}

/// The surface pattern a paint has (the renderer's [`Pattern`]).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum Surface {
    #[default]
    Plain,
    Stucco,
    Brick,
    Concrete,
    Asphalt,
    Glass,
    Metal,
    Paint,
    RoofTile,
    RoofGravel,
    Awning,
    Lawn,
    Wood,
}

impl Surface {
    fn pattern(self) -> Pattern {
        match self {
            Surface::Plain => Pattern::Plain,
            Surface::Stucco => Pattern::Stucco,
            Surface::Brick => Pattern::Brick,
            Surface::Concrete => Pattern::Concrete,
            Surface::Asphalt => Pattern::Asphalt,
            Surface::Glass => Pattern::Glass,
            Surface::Metal => Pattern::Metal,
            Surface::Paint => Pattern::Paint,
            Surface::RoofTile => Pattern::RoofTile,
            Surface::RoofGravel => Pattern::RoofGravel,
            Surface::Awning => Pattern::Awning,
            Surface::Lawn => Pattern::Lawn,
            Surface::Wood => Pattern::Bark,
        }
    }
}

/// When a glowing paint is lit (the renderer's [`Glow`] classes).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum Lit {
    /// As windows are, by the time of day.
    Window,
    /// As neon and sign boxes are.
    Neon,
    /// Dusk to dawn, as street lamps are.
    Lamp,
    /// Always (a beacon).
    Always,
}

/// A named paint.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(deny_unknown_fields))]
pub struct PaintSpec {
    pub color: Color,
    #[cfg_attr(feature = "serde", serde(default))]
    pub surface: Surface,
    /// The light it gives off and when.
    #[cfg_attr(feature = "serde", serde(default))]
    pub glow: Option<(Color, Lit)>,
}

impl PaintSpec {
    fn paint(&self, pal: &Palette) -> Paint {
        let p = Paint::new(self.color.rgb(pal), self.surface.pattern());
        match self.glow {
            None => p,
            Some((c, lit)) => {
                let glow = match lit {
                    Lit::Window => Glow::Window,
                    Lit::Neon => Glow::Neon,
                    Lit::Lamp => Glow::Lamp,
                    Lit::Always => Glow::Signal,
                };
                p.glowing(c.rgb(pal), glow)
            }
        }
    }
}

/// Which way a cylinder's or a lathe's axis runs in the kit frame.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum Axis {
    #[default]
    Up,
    /// Along `x`, across the lot.
    Across,
    /// Along `z`, toward the road.
    Along,
}

fn default_sides() -> u8 {
    12
}

/// One piece of a landmark, in the kit frame, m.
#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum Part {
    /// An axis-aligned box from corner `lo` to corner `hi`.
    Box {
        lo: [f32; 3],
        hi: [f32; 3],
        paint: String,
    },
    /// A square beam `w` wide from `a` to `b` (legs, braces, masts).
    Strut {
        a: [f32; 3],
        b: [f32; 3],
        w: f32,
        paint: String,
    },
    /// A cylinder of radius `r` and length `h` from `at` along `axis`.
    Cylinder {
        at: [f32; 3],
        r: f32,
        h: f32,
        #[cfg_attr(feature = "serde", serde(default = "default_sides"))]
        sides: u8,
        #[cfg_attr(feature = "serde", serde(default))]
        axis: Axis,
        /// No end caps: a band or a tube seen from outside.
        #[cfg_attr(feature = "serde", serde(default))]
        open: bool,
        paint: String,
    },
    /// A surface of revolution about `axis` through `at`: the profile's points are
    /// `(radius, distance along the axis)`, in order.
    Lathe {
        at: [f32; 3],
        profile: Vec<[f32; 2]>,
        #[cfg_attr(feature = "serde", serde(default = "default_sides"))]
        sides: u8,
        #[cfg_attr(feature = "serde", serde(default))]
        axis: Axis,
        /// No end caps: a band or a tube seen from outside.
        #[cfg_attr(feature = "serde", serde(default))]
        open: bool,
        paint: String,
    },
    /// A lamp: a small glowing bulb at `at` that also lights the ground round it (a flood
    /// light of `radius` m in `color`, on whenever the street lamps are).
    Lamp {
        at: [f32; 3],
        color: Color,
        radius: f32,
    },
}

/// One look of a landmark, lettered by its place in the kit (A, B, C...).
#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(deny_unknown_fields))]
pub struct Variant {
    /// A few words for the contact sheet and the docs.
    pub name: String,
    #[cfg_attr(feature = "serde", serde(default))]
    pub about: String,
    /// An earlier variant's letter: this one starts as that one (its parts and paints).
    #[cfg_attr(feature = "serde", serde(default))]
    pub from: Option<char>,
    /// Paints added, or replacing the kit's (or the `from` variant's) of the same name.
    #[cfg_attr(feature = "serde", serde(default))]
    pub paints: Vec<(String, PaintSpec)>,
    /// Parts added after the `from` variant's.
    #[cfg_attr(feature = "serde", serde(default))]
    pub parts: Vec<Part>,
}

/// A landmark and its variants (`assets/landmarks/<id>.ron`).
#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(deny_unknown_fields))]
pub struct Kit {
    /// File stem: lowercase letters, digits and `_`.
    pub id: String,
    /// What it is, in plain words (an invented thing, never a real one's name).
    pub about: String,
    pub tone: Tone,
    /// The lot: width across and depth back from the road, m.
    pub lot: [f32; 2],
    /// The paint of the lot's pad, if it has one.
    #[cfg_attr(feature = "serde", serde(default))]
    pub ground: Option<String>,
    pub paints: Vec<(String, PaintSpec)>,
    pub variants: Vec<Variant>,
}

/// Why a kit was refused.
#[derive(Clone, Debug, PartialEq)]
pub enum KitError {
    BadId(String),
    Lot,
    VariantCount(usize),
    DuplicatePaint(String),
    /// A part, the ground or a variant names a paint nobody defines.
    NoPaint { variant: char, paint: String },
    /// `from` names itself, a later variant or none.
    BadFrom { variant: char },
    /// A part with a non-finite, empty or out-of-range size.
    BadPart { variant: char, part: usize },
    /// A part outside its lot or above [`MAX_HEIGHT_M`].
    OffLot { variant: char, part: usize },
    TooManyTriangles { variant: char, triangles: usize },
}

impl fmt::Display for KitError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            KitError::BadId(id) => write!(f, "id `{id}`: lowercase letters, digits and _ only"),
            KitError::Lot => write!(f, "lot sides must be {}..{} m", LOT_M.0, LOT_M.1),
            KitError::VariantCount(n) => {
                write!(f, "{n} variants: a kit has 1 to {MAX_VARIANTS}")
            }
            KitError::DuplicatePaint(p) => write!(f, "paint `{p}` is defined twice"),
            KitError::NoPaint { variant, paint } => {
                write!(f, "variant {variant}: no paint `{paint}`")
            }
            KitError::BadFrom { variant } => {
                write!(f, "variant {variant}: `from` must name an earlier variant")
            }
            KitError::BadPart { variant, part } => {
                write!(f, "variant {variant}, part {part}: bad size or facet count")
            }
            KitError::OffLot { variant, part } => write!(
                f,
                "variant {variant}, part {part}: outside the lot or over {MAX_HEIGHT_M} m"
            ),
            KitError::TooManyTriangles { variant, triangles } => write!(
                f,
                "variant {variant}: {triangles} triangles (at most {MAX_TRIANGLES})"
            ),
        }
    }
}

/// The letter of variant `k`.
pub fn letter(k: usize) -> char {
    (b'A' + k.min(MAX_VARIANTS - 1) as u8) as char
}

/// A variant with its `from` chain folded in.
pub struct Resolved<'a> {
    pub paints: Vec<(&'a str, &'a PaintSpec)>,
    pub parts: Vec<&'a Part>,
}

impl Resolved<'_> {
    fn paint(&self, name: &str) -> Option<&PaintSpec> {
        self.paints.iter().find(|(n, _)| *n == name).map(|(_, p)| *p)
    }
}

fn set_paints<'a>(into: &mut Vec<(&'a str, &'a PaintSpec)>, from: &'a [(String, PaintSpec)]) {
    for (name, spec) in from {
        match into.iter_mut().find(|(n, _)| *n == name.as_str()) {
            Some(slot) => slot.1 = spec,
            None => into.push((name.as_str(), spec)),
        }
    }
}

impl Kit {
    /// Variant `v` (an index) with what it takes from earlier ones; `None` past the end or
    /// for a `from` that is not an earlier variant.
    pub fn resolve(&self, v: usize) -> Option<Resolved<'_>> {
        // The chain, newest first.
        let mut chain = Vec::new();
        let mut at = v;
        loop {
            let var = self.variants.get(at)?;
            chain.push(var);
            match var.from {
                None => break,
                Some(c) => {
                    let k = (c as usize).checked_sub('A' as usize)?;
                    if k >= at {
                        return None;
                    }
                    at = k;
                }
            }
        }
        let mut r = Resolved {
            paints: Vec::new(),
            parts: Vec::new(),
        };
        set_paints(&mut r.paints, &self.paints);
        for var in chain.iter().rev() {
            set_paints(&mut r.paints, &var.paints);
            r.parts.extend(var.parts.iter());
        }
        Some(r)
    }

    /// Checks the kit (see [`KitError`]).
    pub fn validate(&self) -> Result<(), KitError> {
        let id_ok = !self.id.is_empty()
            && self
                .id
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_');
        if !id_ok {
            return Err(KitError::BadId(self.id.clone()));
        }
        let [w, d] = self.lot;
        if !(LOT_M.0..=LOT_M.1).contains(&w) || !(LOT_M.0..=LOT_M.1).contains(&d) {
            return Err(KitError::Lot);
        }
        let n = self.variants.len();
        if n == 0 || n > MAX_VARIANTS {
            return Err(KitError::VariantCount(n));
        }
        let mut names: Vec<&str> = Vec::new();
        for (name, _) in &self.paints {
            if names.contains(&name.as_str()) {
                return Err(KitError::DuplicatePaint(name.clone()));
            }
            names.push(name);
        }
        let pal = Palette::socal_1985();
        for k in 0..n {
            let variant = letter(k);
            let r = self.resolve(k).ok_or(KitError::BadFrom { variant })?;
            if let Some(g) = &self.ground {
                if r.paint(g).is_none() {
                    let paint = g.clone();
                    return Err(KitError::NoPaint { variant, paint });
                }
            }
            for (i, part) in r.parts.iter().enumerate() {
                let name = match part {
                    Part::Box { paint, .. }
                    | Part::Strut { paint, .. }
                    | Part::Cylinder { paint, .. }
                    | Part::Lathe { paint, .. } => Some(paint),
                    Part::Lamp { .. } => None,
                };
                if let Some(paint) = name {
                    if r.paint(paint).is_none() {
                        let paint = paint.clone();
                        return Err(KitError::NoPaint { variant, paint });
                    }
                }
                if !part_ok(part) {
                    return Err(KitError::BadPart { variant, part: i });
                }
                let (lo, hi) = part_bounds(part);
                let e = 0.01;
                let inside = lo[0] >= -0.5 * w - e
                    && hi[0] <= 0.5 * w + e
                    && lo[2] >= -d - e
                    && hi[2] <= e
                    && lo[1] >= -1.0
                    && hi[1] <= MAX_HEIGHT_M;
                if !inside {
                    return Err(KitError::OffLot { variant, part: i });
                }
            }
            let mut m = WorldMesh::default();
            build(&mut m, self, k, V3::ZERO, V2::new(0.0, 1.0), &pal);
            let triangles = m.triangle_count();
            if triangles > MAX_TRIANGLES {
                return Err(KitError::TooManyTriangles { variant, triangles });
            }
        }
        Ok(())
    }

    /// Variant `v`'s bounds in the kit frame, its lot pad included: `(lo, hi)`.
    pub fn bounds(&self, v: usize) -> ([f32; 3], [f32; 3]) {
        let [w, d] = self.lot;
        let (mut lo, mut hi) = ([-0.5 * w, 0.0, -d], [0.5 * w, 0.0, 0.0]);
        if let Some(r) = self.resolve(v) {
            for part in &r.parts {
                let (a, b) = part_bounds(part);
                for i in 0..3 {
                    lo[i] = lo[i].min(a[i]);
                    hi[i] = hi[i].max(b[i]);
                }
            }
        }
        (lo, hi)
    }
}

fn finite(v: &[f32]) -> bool {
    v.iter().all(|x| x.is_finite())
}

fn part_ok(part: &Part) -> bool {
    let sides_ok = |s: u8| (SIDES.0..=SIDES.1).contains(&s);
    match part {
        Part::Box { lo, hi, .. } => {
            finite(lo) && finite(hi) && (0..3).all(|i| hi[i] - lo[i] >= 0.005)
        }
        Part::Strut { a, b, w, .. } => {
            let len = v3(*b) - v3(*a);
            finite(a) && finite(b) && *w > 0.0 && *w < 10.0 && len.length() > 0.01
        }
        Part::Cylinder { at, r, h, sides, .. } => {
            finite(at) && *r > 0.0 && *h > 0.0 && r.is_finite() && h.is_finite() && sides_ok(*sides)
        }
        Part::Lathe {
            at, profile, sides, ..
        } => {
            finite(at)
                && profile.len() >= 2
                && profile.iter().all(|p| finite(p) && p[0] >= 0.0)
                && profile.iter().any(|p| p[0] > 0.0)
                && sides_ok(*sides)
        }
        Part::Lamp { at, radius, .. } => finite(at) && *radius > 0.0 && *radius <= 60.0,
    }
}

fn v3(a: [f32; 3]) -> V3 {
    V3::new(a[0], a[1], a[2])
}

/// The unit axes `(axis, ring x, ring z)` of `axis` in the kit frame.
fn axes(axis: Axis) -> (V3, V3, V3) {
    match axis {
        Axis::Up => (V3::Y, V3::X, V3::Z),
        Axis::Across => (V3::X, V3::Z, V3::Y),
        Axis::Along => (V3::Z, V3::X, V3::Y),
    }
}

/// A part's bounds in the kit frame.
fn part_bounds(part: &Part) -> ([f32; 3], [f32; 3]) {
    let grow = |lo: &mut [f32; 3], hi: &mut [f32; 3], p: V3| {
        for (i, v) in p.to_array().into_iter().enumerate() {
            lo[i] = lo[i].min(v);
            hi[i] = hi[i].max(v);
        }
    };
    let (mut lo, mut hi) = ([f32::MAX; 3], [f32::MIN; 3]);
    match part {
        Part::Box { lo: a, hi: b, .. } => return (*a, *b),
        Part::Strut { a, b, w, .. } => {
            let h = 0.75 * w;
            for p in [v3(*a), v3(*b)] {
                grow(&mut lo, &mut hi, p - V3::new(h, h, h));
                grow(&mut lo, &mut hi, p + V3::new(h, h, h));
            }
        }
        Part::Cylinder { at, r, h, axis, .. } => {
            let (u, x, z) = axes(*axis);
            let o = v3(*at);
            for (along, rad) in [(0.0, *r), (*h, *r)] {
                for (sx, sz) in [(-1.0, -1.0), (1.0, 1.0)] {
                    grow(&mut lo, &mut hi, o + u * along + x * (sx * rad) + z * (sz * rad));
                }
            }
        }
        Part::Lathe {
            at, profile, axis, ..
        } => {
            let (u, x, z) = axes(*axis);
            let o = v3(*at);
            for p in profile {
                for (sx, sz) in [(-1.0, -1.0), (1.0, 1.0)] {
                    grow(&mut lo, &mut hi, o + u * p[1] + x * (sx * p[0]) + z * (sz * p[0]));
                }
            }
        }
        Part::Lamp { at, .. } => {
            let o = v3(*at);
            grow(&mut lo, &mut hi, o - V3::new(0.2, 0.2, 0.2));
            grow(&mut lo, &mut hi, o + V3::new(0.2, 0.2, 0.2));
        }
    }
    (lo, hi)
}

/// The kit frame of a landmark whose lot's front middle is at `base`, facing `facing`
/// (unit, out of the lot toward the road).
pub fn frame(base: V3, facing: V2) -> Frame {
    Frame::upright(base, facing.left())
}

/// A frame at kit point `o` whose `y` runs along `u` and whose ring spans `x` and `z`.
fn axis_frame(f: &Frame, o: [f32; 3], axis: Axis) -> Frame {
    let (u, x, z) = axes(axis);
    let w = |v: V3| f.v(v.x, v.y, v.z);
    Frame {
        o: f.p(o[0], o[1], o[2]),
        x: w(x),
        y: w(u),
        z: w(z),
    }
}

/// A square beam `w` wide from `a` to `b`, world points.
fn strut(m: &mut WorldMesh, a: V3, b: V3, w: f32, paint: Paint) {
    let u = (b - a).normalize();
    let side = if abs(u.y) > 0.95 { V3::X } else { V3::Y };
    let p = u.cross(side).normalize();
    let q = u.cross(p).normalize();
    let h = 0.5 * w;
    let corner = |s: f32, t: f32| p * (s * h) + q * (t * h);
    let ring = [
        corner(-1.0, -1.0),
        corner(1.0, -1.0),
        corner(1.0, 1.0),
        corner(-1.0, 1.0),
    ];
    for i in 0..4 {
        let (c0, c1) = (ring[i], ring[(i + 1) % 4]);
        let out = (c0 + c1) * 0.5;
        quad(m, [a + c0, a + c1, b + c1, b + c0], out, paint);
    }
    quad(m, ring.map(|c| a + c), -u, paint);
    quad(m, ring.map(|c| b + c), u, paint);
}

/// Builds variant `v` of `kit` on a lot whose front middle is at `base`, facing `facing`
/// (unit, out of the lot toward the road). Nothing for a variant [`Kit::resolve`] refuses.
pub fn build(m: &mut WorldMesh, kit: &Kit, v: usize, base: V3, facing: V2, pal: &Palette) {
    let Some(r) = kit.resolve(v) else {
        return;
    };
    let f = frame(base, facing);
    let paint = |name: &str| r.paint(name).map(|s| s.paint(pal));
    let [w, d] = kit.lot;
    if let Some(p) = kit.ground.as_deref().and_then(paint) {
        let faces = face::ALL & !face::NEG_Y;
        boxf(m, &f, [-0.5 * w, -0.3, -d], [0.5 * w, 0.06, 0.0], faces, p);
    }
    for part in &r.parts {
        match part {
            Part::Box { lo, hi, paint: p } => {
                if let Some(p) = paint(p) {
                    boxf(m, &f, *lo, *hi, face::ALL, p);
                }
            }
            Part::Strut { a, b, w, paint: p } => {
                if let Some(p) = paint(p) {
                    let (a, b) = (f.p(a[0], a[1], a[2]), f.p(b[0], b[1], b[2]));
                    strut(m, a, b, *w, p);
                }
            }
            Part::Cylinder {
                at,
                r,
                h,
                sides,
                axis,
                open,
                paint: p,
            } => {
                if let Some(p) = paint(p) {
                    let af = axis_frame(&f, *at, *axis);
                    let profile = [[*r, 0.0], [*r, *h]];
                    lathe(m, &af, &profile, u32::from(*sides), !open, true, p);
                }
            }
            Part::Lathe {
                at,
                profile,
                sides,
                axis,
                open,
                paint: p,
            } => {
                if let Some(p) = paint(p) {
                    let af = axis_frame(&f, *at, *axis);
                    lathe(m, &af, profile, u32::from(*sides), !open, true, p);
                }
            }
            Part::Lamp { at, color, .. } => {
                let rgb = color.rgb(pal);
                let bulb = Paint::new(rgb, Pattern::Plain).glowing(rgb, Glow::Lamp);
                let [x, y, z] = *at;
                boxf(
                    m,
                    &f,
                    [x - 0.15, y - 0.15, z - 0.15],
                    [x + 0.15, y + 0.15, z + 0.15],
                    face::ALL,
                    bulb,
                );
            }
        }
    }
}

/// Variant `v`'s lamps as the place's lights, for a lot at `base` facing `facing`.
pub fn lights(kit: &Kit, v: usize, base: V3, facing: V2, pal: &Palette, out: &mut Vec<PlaceLight>) {
    let Some(r) = kit.resolve(v) else {
        return;
    };
    let f = frame(base, facing);
    for part in &r.parts {
        if let Part::Lamp { at, color, radius } = part {
            let rgb = color.rgb(pal);
            out.push(PlaceLight {
                pos: f.p(at[0], at[1], at[2]).to_array(),
                color: rgb.map(|c| f32::from(c) / 255.0),
                radius: *radius,
                kind: LightKind::Flood,
            });
        }
    }
}

/// The boxes variant `v` blocks a car with, for a lot at ground point `pos` (height `y`)
/// facing `facing`: each part that starts within [`SOLID_BELOW_M`] of the lot, as its
/// bounds (a strut as a post round its low end).
pub fn slabs(kit: &Kit, v: usize, pos: V2, y: f32, facing: V2, out: &mut Vec<Slab>) {
    let Some(r) = kit.resolve(v) else {
        return;
    };
    let x = facing.left();
    let at = |lx: f32, lz: f32| pos + x * lx + facing * lz;
    for part in &r.parts {
        let (lo, hi) = match part {
            Part::Lamp { .. } => continue,
            Part::Strut { a, b, w, .. } => {
                let low = if a[1] <= b[1] { a } else { b };
                let h = 0.5 * w;
                (
                    [low[0] - h, low[1], low[2] - h],
                    [low[0] + h, low[1] + SOLID_BELOW_M, low[2] + h],
                )
            }
            _ => part_bounds(part),
        };
        if lo[1] > SOLID_BELOW_M || hi[1] < 0.2 {
            continue;
        }
        let c = at(0.5 * (lo[0] + hi[0]), 0.5 * (lo[2] + hi[2]));
        out.push(Slab {
            c,
            u: x,
            half: [0.5 * (hi[0] - lo[0]), 0.5 * (hi[2] - lo[2])],
            y0: y + lo[1].min(0.0) - 0.5,
            y1: y + hi[1],
        });
    }
}

/// The gallery's lot label: a lit plate on a post lettered with `letter`, at `base`, the
/// plate facing `facing`.
pub fn lot_label(m: &mut WorldMesh, base: V3, facing: V2, letter: u8, pal: &Palette) {
    let f = frame(base, facing);
    let post = Paint::new(pal.metal_dark, Pattern::Metal);
    boxf(m, &f, [-0.08, 0.0, -0.18], [0.08, 1.2, -0.02], face::NO_BOTTOM, post);
    let edge = Paint::new(shade(pal.metal_dark, 160), Pattern::Metal);
    boxf(m, &f, [-0.85, 1.1, -0.12], [0.85, 2.8, 0.0], face::ALL, edge);
    let face_rgb: Rgb = [250, 246, 232];
    let plate = Paint::new(face_rgb, Pattern::Plain).glowing([110, 106, 96], Glow::Neon);
    rect(m, &f, -0.75, 1.2, 0.75, 2.7, 0.01, false, plate);
    let mut s = letters::layout(&[letter], LetterStyle::Block);
    let w = s.ink_w().max(1e-3);
    let size = 1.1f32.min(1.2 / w);
    s.place(size, -0.5 * w * size, 1.4);
    let ink = Paint::new([24, 22, 30], Pattern::Plain);
    letters::flat(m, &f, &s, 0.02, false, ink);
}

/// The gallery's road runs east along `z = row_z(k)` for kit `k`'s row...
pub const GALLERY_ROW_M: f32 = 160.0;
/// ...the lots' fronts this far north of its middle...
pub const GALLERY_SETBACK_M: f32 = 12.0;
/// ...this far apart, side to side...
pub const GALLERY_GAP_M: f32 = 16.0;
/// ...and the road runs this far past the first and last lot.
const GALLERY_RUNOUT_M: f32 = 80.0;
/// A gallery road's width, m.
const GALLERY_ROAD_M: f32 = 10.0;

/// One lot of the gallery.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GalleryLot {
    /// The kit's index and the variant's.
    pub kit: usize,
    pub variant: usize,
    /// The middle of the lot's front edge, `[x, z]`; the lot faces south (`+z`), toward
    /// its row's road.
    pub front: [f32; 2],
    /// The lot's label post, `[x, z]`, at its front right corner seen from the road.
    pub label: [f32; 2],
}

/// The middle of kit `k`'s road, `z`.
pub fn row_z(k: usize) -> f32 {
    -(k as f32) * GALLERY_ROW_M
}

/// The gallery's lots: kit by kit from south to north, variant by variant from west to
/// east (A westmost).
pub fn gallery_lots(kits: &[Kit]) -> Vec<GalleryLot> {
    let mut out = Vec::new();
    for (k, kit) in kits.iter().enumerate() {
        let [w, _] = kit.lot;
        let pitch = w + GALLERY_GAP_M;
        let z = row_z(k) - GALLERY_SETBACK_M;
        for v in 0..kit.variants.len() {
            let x = v as f32 * pitch;
            out.push(GalleryLot {
                kit: k,
                variant: v,
                front: [x, z],
                label: [x + 0.5 * w + 1.5, z + 1.0],
            });
        }
    }
    out
}

/// Yaw (the [`Prop`] convention) of something facing south, `+z`.
const SOUTH_YAW: f32 = core::f32::consts::PI;

/// The landmark gallery (`place=gallery`): every kit's variants side by side on flat lots
/// north of an east-west road, one road per kit, each lot with its letter on a lit label.
/// Flat lawn, no buildings, no traffic lanes; the car starts on the first row's road in
/// front of its first lot, facing it.
pub fn gallery(seed: u64, kits: &[Kit], region: RegionId, era: Era, pal: &Palette) -> Place {
    let lots = gallery_lots(kits);
    let rows = kits.len().max(1);
    let chunk = 128.0f32;
    let mut lo = [-GALLERY_RUNOUT_M - 40.0, row_z(rows - 1) - GALLERY_SETBACK_M - 120.0];
    let mut hi = [GALLERY_RUNOUT_M + 40.0, 220.0];
    for l in &lots {
        hi[0] = hi[0].max(l.front[0] + GALLERY_RUNOUT_M + 40.0);
        lo[1] = lo[1].min(l.front[1] - kits[l.kit].lot[1] - 60.0);
    }
    let c0 = [floor(lo[0] / chunk) as i32, floor(lo[1] / chunk) as i32];
    let c1 = [floor(hi[0] / chunk) as i32, floor(hi[1] / chunk) as i32];
    let header = PlaceHeader {
        version: place::PLACE_VERSION,
        seed,
        chunk_size_m: chunk,
        size_chunks: [(c1[0] - c0[0] + 1) as u16, (c1[1] - c0[1] + 1) as u16],
        chunk_origin: [c0[0] as i16, c0[1] as i16],
        region,
        era,
        origin_geo: None,
    };
    let res = 8;
    let terrain = Terrain::flat(&header, res, 0.0);
    let [zw, zd] = Zones::cells_per_side(&header, res);
    let zones = Zones {
        cells: alloc::vec![ZoneId::Park; zw * zd],
    };

    let mut roads = RoadGraph::default();
    for k in 0..kits.len() {
        let z = row_z(k);
        let last = lots
            .iter()
            .filter(|l| l.kit == k)
            .map(|l| l.front[0])
            .fold(0.0f32, f32::max);
        let (x0, x1) = (-GALLERY_RUNOUT_M, last + GALLERY_RUNOUT_M);
        let a = roads.nodes.len() as u32;
        for x in [x0, x1] {
            roads.nodes.push(Node {
                pos: [x, z],
                elevation: 0.0,
            });
        }
        roads.edges.push(Edge {
            a,
            b: a + 1,
            class: RoadClass::Local,
            lanes_fwd: 1,
            lanes_back: 1,
            width_m: GALLERY_ROAD_M,
            sidewalk: true,
            speed_kph: 40,
            oneway: false,
            profile: alloc::vec![[x0, 0.0, z], [x1, 0.0, z]],
        });
        // A road north from the last row's west end ties the rows together.
        if k > 0 {
            let prev = a - 2;
            let z0 = row_z(k - 1);
            roads.edges.push(Edge {
                a: prev,
                b: a,
                class: RoadClass::Local,
                lanes_fwd: 1,
                lanes_back: 1,
                width_m: GALLERY_ROAD_M,
                sidewalk: false,
                speed_kph: 40,
                oneway: false,
                profile: alloc::vec![[x0, 0.0, z0], [x0, 0.0, z]],
            });
        }
    }

    let south = V2::new(0.0, 1.0);
    let mut props = Vec::new();
    let mut lights = Vec::new();
    for l in &lots {
        let kit = &kits[l.kit];
        props.push(Prop {
            pos: [l.front[0], 0.0, l.front[1]],
            yaw: SOUTH_YAW,
            kind: PropKind::Landmark {
                kit: l.kit as u8,
                variant: l.variant as u8,
            },
        });
        props.push(Prop {
            pos: [l.label[0], 0.0, l.label[1]],
            yaw: SOUTH_YAW,
            kind: PropKind::LotLabel {
                letter: letter(l.variant) as u8,
            },
        });
        let base = V3::new(l.front[0], 0.0, l.front[1]);
        self::lights(kit, l.variant, base, south, pal, &mut lights);
    }
    let start = lots.first().map_or([0.0, 0.0], |l| l.front);
    let mut place = Place {
        header,
        terrain,
        zones,
        roads,
        parcels: Vec::new(),
        buildings: Vec::new(),
        pois: Vec::new(),
        spawns: alloc::vec![Spawn {
            pos: [start[0], 0.0, row_z(0) + 2.5],
            heading: 0.0,
            kind: SpawnKind::PlayerStart,
        }],
        lanes: Default::default(),
        sidewalks: Default::default(),
        facades: Default::default(),
        props: place::Props { items: props },
        lights: place::Lights { items: lights },
    };
    place.sort_by_chunk();
    place
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::string::ToString;
    use alloc::vec;

    fn spec(color: Color) -> PaintSpec {
        PaintSpec {
            color,
            surface: Surface::Plain,
            glow: None,
        }
    }

    fn kit() -> Kit {
        Kit {
            id: "test_post".to_string(),
            about: "A post.".to_string(),
            tone: Tone::Grounded,
            lot: [10.0, 10.0],
            ground: Some("pad".to_string()),
            paints: vec![
                ("pad".to_string(), spec(Color::Pal(Swatch::Concrete))),
                ("post".to_string(), spec(Color::Srgb(0x808080))),
            ],
            variants: vec![
                Variant {
                    name: "Plain".to_string(),
                    about: String::new(),
                    from: None,
                    paints: Vec::new(),
                    parts: vec![
                        Part::Box {
                            lo: [-1.0, 0.0, -6.0],
                            hi: [1.0, 3.0, -4.0],
                            paint: "post".to_string(),
                        },
                        Part::Lamp {
                            at: [0.0, 3.5, -5.0],
                            color: Color::Srgb(0xFFFFFF),
                            radius: 10.0,
                        },
                    ],
                },
                Variant {
                    name: "Red with a cap".to_string(),
                    about: String::new(),
                    from: Some('A'),
                    paints: vec![("post".to_string(), spec(Color::Pal(Swatch::NeonRed)))],
                    parts: vec![Part::Cylinder {
                        at: [0.0, 3.0, -5.0],
                        r: 1.2,
                        h: 0.3,
                        sides: 12,
                        axis: Axis::Up,
                        open: false,
                        paint: "post".to_string(),
                    }],
                },
            ],
        }
    }

    #[test]
    fn variants_inherit_parts_and_override_paints() {
        let k = kit();
        assert_eq!(k.validate(), Ok(()));
        let a = k.resolve(0).unwrap();
        let b = k.resolve(1).unwrap();
        assert_eq!((a.parts.len(), b.parts.len()), (2, 3));
        assert_eq!(b.paint("post").unwrap().color, Color::Pal(Swatch::NeonRed));
        assert_eq!(a.paint("post").unwrap().color, Color::Srgb(0x808080));
        let pal = Palette::socal_1985();
        let (mut ma, mut mb) = (WorldMesh::default(), WorldMesh::default());
        build(&mut ma, &k, 0, V3::ZERO, V2::new(0.0, 1.0), &pal);
        build(&mut mb, &k, 1, V3::ZERO, V2::new(0.0, 1.0), &pal);
        assert!(mb.triangle_count() > ma.triangle_count());
        crate::mesh::tests::assert_well_formed(&ma, 0.2);
        assert_eq!(k.bounds(1).1[1], 3.7);
    }

    #[test]
    fn bad_kits_are_refused() {
        let mut k = kit();
        k.variants[0].from = Some('B');
        assert_eq!(k.validate(), Err(KitError::BadFrom { variant: 'A' }));
        let mut k = kit();
        k.variants[1].from = Some('B');
        assert_eq!(k.validate(), Err(KitError::BadFrom { variant: 'B' }));
        let mut k = kit();
        k.paints.push(("post".to_string(), spec(Color::Srgb(0))));
        assert_eq!(
            k.validate(),
            Err(KitError::DuplicatePaint("post".to_string()))
        );
        let mut k = kit();
        k.ground = Some("tar".to_string());
        assert!(matches!(k.validate(), Err(KitError::NoPaint { .. })));
        let mut k = kit();
        k.variants[0].parts[0] = Part::Box {
            lo: [-1.0, 0.0, -6.0],
            hi: [1.0, 3.0, 2.0],
            paint: "post".to_string(),
        };
        assert_eq!(
            k.validate(),
            Err(KitError::OffLot {
                variant: 'A',
                part: 0
            })
        );
        let mut k = kit();
        k.variants[1].parts[0] = Part::Cylinder {
            at: [0.0, 3.0, -5.0],
            r: 1.2,
            h: 0.3,
            sides: 2,
            axis: Axis::Up,
            open: false,
            paint: "post".to_string(),
        };
        assert!(matches!(k.validate(), Err(KitError::BadPart { .. })));
        let mut k = kit();
        k.id = "Post".to_string();
        assert!(matches!(k.validate(), Err(KitError::BadId(_))));
        let mut k = kit();
        k.lot = [2.0, 10.0];
        assert_eq!(k.validate(), Err(KitError::Lot));
        let mut k = kit();
        k.variants[1].parts = vec![
            Part::Lathe {
                at: [0.0, 3.0, -5.0],
                profile: vec![[1.0, 0.0], [1.0, 0.2]],
                sides: 48,
                axis: Axis::Up,
                open: true,
                paint: "post".to_string(),
            };
            100
        ];
        assert!(matches!(
            k.validate(),
            Err(KitError::TooManyTriangles { variant: 'B', .. })
        ));
    }

    #[test]
    fn low_parts_block_cars_and_lamps_light_the_lot() {
        let k = kit();
        let mut s = Vec::new();
        slabs(&k, 1, V2::new(100.0, 50.0), 1.0, V2::new(0.0, 1.0), &mut s);
        // The post; the cap sits above the car.
        assert_eq!(s.len(), 1);
        assert!((s[0].c.x - 100.0).abs() < 1e-4 && (s[0].c.y - 45.0).abs() < 1e-4);
        assert_eq!(s[0].half, [1.0, 1.0]);
        let mut l = Vec::new();
        let pal = Palette::socal_1985();
        lights(&k, 0, V3::new(100.0, 1.0, 50.0), V2::new(0.0, 1.0), &pal, &mut l);
        assert_eq!(l.len(), 1);
        assert_eq!(l[0].pos, [100.0, 4.5, 45.0]);
    }

    #[test]
    fn the_gallery_lines_up_every_variant_on_its_own_lot() {
        let kits = [kit(), kit()];
        let pal = Palette::socal_1985();
        let p = gallery(1, &kits, RegionId::CaliforniaCoast, Era::Eighties, &pal);
        assert_eq!(p.validate(), Ok(()));
        let lots = gallery_lots(&kits);
        assert_eq!(lots.len(), 4);
        // Lots do not overlap and stand clear of their road.
        for (i, a) in lots.iter().enumerate() {
            let w = kits[a.kit].lot[0];
            for b in &lots[i + 1..] {
                if a.kit == b.kit {
                    assert!((a.front[0] - b.front[0]).abs() >= w + GALLERY_GAP_M - 1e-3);
                }
            }
            assert!(row_z(a.kit) - a.front[1] >= 0.5 * GALLERY_ROAD_M + 4.0);
        }
        let landmarks = p
            .props
            .items
            .iter()
            .filter(|q| matches!(q.kind, PropKind::Landmark { .. }))
            .count();
        let labels: Vec<u8> = p
            .props
            .items
            .iter()
            .filter_map(|q| match q.kind {
                PropKind::LotLabel { letter } => Some(letter),
                _ => None,
            })
            .collect();
        assert_eq!(landmarks, 4);
        assert_eq!(labels.iter().filter(|&&c| c == b'B').count(), 2);
        assert_eq!(p.lights.items.len(), 4);
        // The whole gallery lies on the place.
        let [w, d] = p.header.extent_m();
        let o = p.header.origin_m();
        for q in &p.props.items {
            assert!(q.pos[0] > o[0] && q.pos[0] < o[0] + w);
            assert!(q.pos[2] > o[1] && q.pos[2] < o[1] + d);
        }
        assert_eq!(
            p,
            gallery(1, &kits, RegionId::CaliforniaCoast, Era::Eighties, &pal)
        );
    }
}
