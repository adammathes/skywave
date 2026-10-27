//! The town as the car sim collides with it (`docs/VEHICLES.md`, "Physics model" item 6).
//!
//! [`WorldCollider::build`] turns a [`Place`] into static plain-data shapes binned on a
//! uniform [`CELL_M`] grid, once:
//! - **Ground patches**: polygons at the heights the meshes put them: asphalt (road
//!   ribbons and junctions) on the road profiles, sidewalks [`CURB_M`] above them, lot pads
//!   at [`LOT_Y`]. Ribbons and strips follow their edge's profile ([`Patch::edge`]),
//!   junction fans and corners are planar triangles; the flat test town's are flat (asphalt
//!   at `y = 0`). Earlier patches win where they overlap, unless they lie a deck's height
//!   apart (a road over another): then the probe takes the layer the wheel is on.
//! - **Open ground** wherever no patch is: the apron around the town at [`TERRAIN_Y`], or
//!   on a terrain place the graded terrain ([`TerrainGrid`], on the mesh's triangles).
//! - **Steps**: every place two patches of different height meet (curbs, sidewalk backs),
//!   as segments with the height of their upper side.
//! - **Walls**: one vertical segment per building footprint edge, facing out. Exact for
//!   extruded footprints, concave ones included.
//! - **Posts and boxes**: props as vertical cylinders (lamps, trees, hydrants, poles) and
//!   oriented boxes (benches, hedges, fences, parked cars, a pier's deck): the test town's
//!   from its own prop passes, a terrain place's from its `Props` table.
//!
//! The collider is a [`vehicle::Ground`]: a wheel ray reads the patch height under it, and
//! near a step it reads the height at which a wheel of [`WHEEL_RADIUS_M`] would touch the
//! step's edge, so a tire rolls up a 15 cm curb over ~27 cm instead of hitting a wall.
//! Sloped ground reports its own normal, and a slope is no curb to the curb kick.
//! [`resolve`], called after each `vehicle::step`, pushes the car's body box out of walls,
//! posts and boxes with an impulse and a damping pulse (bump and stop, never bounce), and
//! kicks the body when a wheel hits a curb fast.
//!
//! Water: the car rolls on the sea floor under it; [`WorldCollider::water_depth`] says how
//! deep a point is under the surface, and the driver's side (`app::drive`) drags a car
//! that wades in deep and puts it back at its start.
//!
//! Queries allocate nothing and use only IEEE arithmetic in a fixed order, so the same
//! inputs give bit-identical results everywhere.

use alloc::vec::Vec;

use place::Place;
use vehicle::sim::{DT, WHEEL_RADIUS_M};
use vehicle::{Ground, Hit, VehicleSpec, VehicleState};

use crate::buildings;
use crate::ground::{self, Surface as LotSurface};
use crate::math::{floor, signed_area, sqrt, V2};
use crate::mesh::terrain::TerrainGrid;
use crate::mesh::WorldMesh;
use crate::props::{self, Prop, PropKind};
use crate::roads::Network;
use crate::{MeshSet, Palette, CURB_M, LOT_Y, TERRAIN_Y};

/// Grid cell edge, m.
pub const CELL_M: f32 = 16.0;
/// Height difference below which two patches count as level (no step), m.
const STEP_MIN_M: f32 = 0.02;
/// Patch edges are cut into pieces at most this long before they are tested for steps, m.
const STEP_PIECE_M: f32 = 4.0;
/// How far either side of an edge its heights are sampled, m.
const STEP_SAMPLE_M: f32 = 0.05;

/// Height of the car body's lowest face above the ground at rest, m. The sim's body-corner
/// contacts use the same 0.25 m.
const BODY_CLEARANCE_M: f32 = 0.25;
/// Contact response (`docs/VEHICLES.md` item 6).
const RESTITUTION: f32 = 0.2;
const FRICTION: f32 = 0.6;
/// After a hit the body may separate from what it hit at no more than this, m/s: the damping
/// pulse that turns restitution's bounce into a bump.
const REBOUND_MAX_MPS: f32 = 0.3;
/// Share of the remaining horizontal velocity the damping pulse removes from a hit at
/// [`HIT_DAMP_SPEED`] or harder (scaled down linearly for softer hits).
const HIT_DAMP: f32 = 0.2;
const HIT_DAMP_SPEED: f32 = 10.0;
/// Gauss-Seidel passes over the contacts per call.
const CONTACT_PASSES: usize = 4;
/// Wheels climb curbs quietly up to this speed (30 mph), m/s; above it the body is kicked.
pub const CURB_CLIMB_MPS: f32 = 13.4112;
/// Upward speed given to the wheel's corner of the body per m/s above [`CURB_CLIMB_MPS`],
/// per meter of step height, and its cap, m/s.
const CURB_KICK_PER_MPS_M: f32 = 1.0;
const CURB_KICK_MAX: f32 = 2.0;
/// Clearance the continuous check leaves between a clamped center and the wall, m.
const SWEEP_MARGIN_M: f32 = 0.05;

/// What a patch is paved with.
#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Surface {
    Asphalt = 0,
    Concrete = 1,
    Grass = 2,
}

impl Surface {
    /// Tire grip multiplier ([`Hit::friction`]).
    pub fn friction(self) -> f32 {
        match self {
            Surface::Asphalt => 1.0,
            Surface::Concrete => 0.9,
            Surface::Grass => 0.6,
        }
    }
}

/// A polygon of ground: `WorldCollider::points[start..start + len]`, any winding. Its
/// height at a point is `y` plus, when `edge` names a road edge, that edge's profile
/// height there ([`crate::roads::profile_height`], as the meshes lay the road), else plus
/// the plane `grad · (p - at)` (zero for the flat patches of the test town).
#[derive(Clone, Copy, Debug, PartialEq)]
#[repr(C)]
pub struct Patch {
    pub start: u32,
    pub len: u32,
    pub y: f32,
    pub surface: Surface,
    /// Bounds on the ground plane, `[x, z]`.
    pub lo: [f32; 2],
    pub hi: [f32; 2],
    /// Road edge whose profile the height follows, or [`NO_EDGE`].
    pub edge: u32,
    /// Height change per meter along x and z, and the point it is measured from.
    pub grad: [f32; 2],
    pub at: [f32; 2],
}

/// [`Patch::edge`] of a patch that does not follow a road profile.
pub const NO_EDGE: u32 = u32::MAX;

/// How a patch's height varies, while building.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Height {
    Flat(f32),
    /// Offset above edge `.0`'s profile.
    Edge(u32, f32),
    /// The plane through three points.
    Plane([f32; 3], [f32; 3], [f32; 3]),
}

/// Where two patches of different height meet: the segment `a..b`, the unit normal `up`
/// pointing to the higher side, and that side's height.
#[derive(Clone, Copy, Debug, PartialEq)]
#[repr(C)]
pub struct Step {
    pub a: [f32; 2],
    pub b: [f32; 2],
    pub up: [f32; 2],
    pub y_high: f32,
}

/// A building wall: the segment `a..b` on the ground, the outward unit normal `n`, and the
/// heights it spans.
#[derive(Clone, Copy, Debug, PartialEq)]
#[repr(C)]
pub struct Wall {
    pub a: [f32; 2],
    pub b: [f32; 2],
    pub n: [f32; 2],
    pub y0: f32,
    pub y1: f32,
}

/// A vertical cylinder.
#[derive(Clone, Copy, Debug, PartialEq)]
#[repr(C)]
pub struct Post {
    pub c: [f32; 2],
    pub r: f32,
    pub y0: f32,
    pub y1: f32,
    pub kind: PropKind,
}

/// An upright box: center `c`, unit axis `u` (the other is `u` turned a quarter), half
/// extents along `u` and across it.
#[derive(Clone, Copy, Debug, PartialEq)]
#[repr(C)]
pub struct BoxSolid {
    pub c: [f32; 2],
    pub u: [f32; 2],
    pub half: [f32; 2],
    pub y0: f32,
    pub y1: f32,
    pub kind: PropKind,
}

/// The ground under a point.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GroundSample {
    /// Height a wheel there rests at, m (above the patch height next to a step).
    pub y: f32,
    /// Unit contact normal (tilted toward the wheel at a step edge).
    pub normal: [f32; 3],
    pub surface: Surface,
}

/// Indices of the shapes overlapping each grid cell, compressed row storage.
#[derive(Clone, Debug, Default, PartialEq)]
struct Bins {
    /// `start[cell]..start[cell + 1]` indexes `items`; `cells + 1` entries.
    start: Vec<u32>,
    items: Vec<u32>,
}

/// Static collision shapes of a place. Built once; every query is allocation-free.
#[derive(Clone, Debug, PartialEq)]
pub struct WorldCollider {
    /// South-west corner of the grid, `[x, z]`, and its size in cells.
    pub origin: [f32; 2],
    pub cells: [u32; 2],
    /// Height and surface wherever no patch is (the apron around the town).
    pub apron_y: f32,
    /// The graded terrain of a terrain place: the ground wherever no patch is (instead of
    /// the apron).
    pub terrain: Option<TerrainGrid>,
    /// Road edge profiles patches follow: edge `e`'s polyline is
    /// `profile_xz[r[0]..r[1]]` (heights `profile_y`) for `r = profile_range[e]`.
    pub profile_xz: Vec<V2>,
    pub profile_y: Vec<f32>,
    pub profile_range: Vec<[u32; 2]>,
    pub points: Vec<[f32; 2]>,
    pub patches: Vec<Patch>,
    pub steps: Vec<Step>,
    pub walls: Vec<Wall>,
    pub posts: Vec<Post>,
    pub boxes: Vec<BoxSolid>,
    patch_bins: Bins,
    step_bins: Bins,
    wall_bins: Bins,
    post_bins: Bins,
    box_bins: Bins,
}

fn v2(p: [f32; 2]) -> V2 {
    V2::new(p[0], p[1])
}

fn arr(p: V2) -> [f32; 2] {
    [p.x, p.y]
}

fn bounds_of(pts: &[[f32; 2]]) -> ([f32; 2], [f32; 2]) {
    let mut lo = [f32::MAX, f32::MAX];
    let mut hi = [f32::MIN, f32::MIN];
    for p in pts {
        for k in 0..2 {
            lo[k] = lo[k].min(p[k]);
            hi[k] = hi[k].max(p[k]);
        }
    }
    (lo, hi)
}

fn point_in(p: [f32; 2], poly: &[[f32; 2]]) -> bool {
    let mut inside = false;
    let n = poly.len();
    let mut j = n - 1;
    for i in 0..n {
        let (a, b) = (poly[i], poly[j]);
        if (a[1] > p[1]) != (b[1] > p[1])
            && p[0] < (b[0] - a[0]) * (p[1] - a[1]) / (b[1] - a[1]) + a[0]
        {
            inside = !inside;
        }
        j = i;
    }
    inside
}

/// Closest point to `p` on the segment `a..b`.
fn closest_on_segment(p: [f32; 2], a: [f32; 2], b: [f32; 2]) -> [f32; 2] {
    let ab = [b[0] - a[0], b[1] - a[1]];
    let l2 = ab[0] * ab[0] + ab[1] * ab[1];
    let mut t = 0.0;
    if l2 > 0.0 {
        t = ((p[0] - a[0]) * ab[0] + (p[1] - a[1]) * ab[1]) / l2;
        t = t.clamp(0.0, 1.0);
    }
    [a[0] + ab[0] * t, a[1] + ab[1] * t]
}

fn dot2(a: [f32; 2], b: [f32; 2]) -> f32 {
    a[0] * b[0] + a[1] * b[1]
}

fn abs(x: f32) -> f32 {
    if x < 0.0 {
        -x
    } else {
        x
    }
}

/// Builder state: shapes plus their bounds, before binning.
struct Shapes {
    points: Vec<[f32; 2]>,
    patches: Vec<Patch>,
}

impl Shapes {
    fn patch(&mut self, poly: &[V2], y: f32, surface: Surface) {
        self.patch_with(poly, Height::Flat(y), surface);
    }

    fn patch_with(&mut self, poly: &[V2], height: Height, surface: Surface) {
        if poly.len() < 3 || abs(signed_area(poly)) < 1e-4 {
            return;
        }
        let (y, edge, grad, at) = match height {
            Height::Flat(y) => (y, NO_EDGE, [0.0; 2], [0.0; 2]),
            Height::Edge(e, y) => (y, e, [0.0; 2], [0.0; 2]),
            Height::Plane(a, b, c) => {
                let e1 = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
                let e2 = [c[0] - a[0], c[1] - a[1], c[2] - a[2]];
                let det = e1[0] * e2[2] - e2[0] * e1[2];
                if abs(det) < 1e-6 {
                    return;
                }
                let gx = (e1[1] * e2[2] - e2[1] * e1[2]) / det;
                let gz = (e1[0] * e2[1] - e2[0] * e1[1]) / det;
                (a[1], NO_EDGE, [gx, gz], [a[0], a[2]])
            }
        };
        let start = self.points.len() as u32;
        self.points.extend(poly.iter().map(|&p| arr(p)));
        let (lo, hi) = bounds_of(&self.points[start as usize..]);
        self.patches.push(Patch {
            start,
            len: poly.len() as u32,
            y,
            surface,
            lo,
            hi,
            edge,
            grad,
            at,
        });
    }
}

/// Which ground a height came from: a patch (by index) or the terrain/apron.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Under {
    Patch(u32),
    Open,
}

/// Patches more than this far apart in height under one point are layered (a road over
/// another): the probe then picks by height instead of taking the first.
const LAYER_M: f32 = crate::roads::DECK_CLEARANCE_M;
/// A wheel takes the highest layer at most this far above it.
const LAYER_STEP_M: f32 = 1.0;
/// Steps higher than this above the ground under a wheel are another layer, not a curb.
const MAX_STEP_UP_M: f32 = 0.5;

impl WorldCollider {
    /// Collision shapes of `place`, consistent with what [`crate::build_chunk_meshes`] draws.
    pub fn build(place: &Place) -> WorldCollider {
        let net = Network::new(place);
        let mut shapes = Shapes {
            points: Vec::new(),
            patches: Vec::new(),
        };
        // Patches in priority order: asphalt, then sidewalks, then lot pads.
        road_patches(place, &net, &mut shapes);
        let terrain_place = crate::is_terrain_place(place);
        let blocks = if terrain_place {
            Vec::new()
        } else {
            ground::blocks(&net)
        };
        for b in &blocks {
            let surface = match ground::block_surface(place, b) {
                LotSurface::Paved => Surface::Concrete,
                LotSurface::Lawn | LotSurface::ParkLawn => Surface::Grass,
            };
            shapes.patch(&b.poly, LOT_Y, surface);
        }

        let walls = building_walls(place);
        let (posts, boxes) = if terrain_place {
            place_prop_solids(place)
        } else {
            prop_solids(&props_of(place, &net, &blocks))
        };

        // Grid over everything, with a cell of margin.
        let mut lo = [f32::MAX, f32::MAX];
        let mut hi = [f32::MIN, f32::MIN];
        let mut grow = |l: [f32; 2], h: [f32; 2]| {
            for k in 0..2 {
                lo[k] = lo[k].min(l[k]);
                hi[k] = hi[k].max(h[k]);
            }
        };
        for p in &shapes.patches {
            grow(p.lo, p.hi);
        }
        for w in &walls {
            grow(w.a, w.a);
            grow(w.b, w.b);
        }
        for p in &posts {
            grow(p.c, p.c);
        }
        for b in &boxes {
            grow(b.c, b.c);
        }
        if terrain_place {
            // The terrain covers the whole place.
            let (o, e) = (place.header.origin_m(), place.header.extent_m());
            grow(o, [o[0] + e[0], o[1] + e[1]]);
        }
        if lo[0] > hi[0] {
            lo = [0.0, 0.0];
            hi = [0.0, 0.0];
        }
        let origin = [lo[0] - CELL_M, lo[1] - CELL_M];
        let cells = [
            ((hi[0] - origin[0]) / CELL_M) as u32 + 2,
            ((hi[1] - origin[1]) / CELL_M) as u32 + 2,
        ];

        let mut c = WorldCollider {
            origin,
            cells,
            apron_y: TERRAIN_Y,
            terrain: terrain_place.then(|| TerrainGrid::new(place, &net)),
            profile_xz: Vec::new(),
            profile_y: Vec::new(),
            profile_range: Vec::new(),
            points: shapes.points,
            patches: shapes.patches,
            steps: Vec::new(),
            walls,
            posts,
            boxes,
            patch_bins: Bins::default(),
            step_bins: Bins::default(),
            wall_bins: Bins::default(),
            post_bins: Bins::default(),
            box_bins: Bins::default(),
        };
        for path in &net.paths {
            let start = c.profile_xz.len() as u32;
            c.profile_xz.extend_from_slice(&path.raw);
            c.profile_y.extend_from_slice(&path.raw_y);
            c.profile_range.push([start, c.profile_xz.len() as u32]);
        }
        let patch_boxes: Vec<_> = c.patches.iter().map(|p| (p.lo, p.hi)).collect();
        c.patch_bins = c.bin(&patch_boxes, 0.0);
        c.steps = c.find_steps();
        let reach = WHEEL_RADIUS_M;
        let step_boxes: Vec<_> = c.steps.iter().map(|s| bounds_of(&[s.a, s.b])).collect();
        c.step_bins = c.bin(&step_boxes, reach);
        let wall_boxes: Vec<_> = c.walls.iter().map(|w| bounds_of(&[w.a, w.b])).collect();
        c.wall_bins = c.bin(&wall_boxes, 0.0);
        let post_boxes: Vec<_> = c
            .posts
            .iter()
            .map(|p| ([p.c[0] - p.r, p.c[1] - p.r], [p.c[0] + p.r, p.c[1] + p.r]))
            .collect();
        c.post_bins = c.bin(&post_boxes, 0.0);
        let box_boxes: Vec<_> = c
            .boxes
            .iter()
            .map(|b| {
                let r = sqrt(b.half[0] * b.half[0] + b.half[1] * b.half[1]);
                ([b.c[0] - r, b.c[1] - r], [b.c[0] + r, b.c[1] + r])
            })
            .collect();
        c.box_bins = c.bin(&box_boxes, 0.0);
        c
    }

    /// Cell index range `[x0, x1] x [z0, z1]` covering `lo..hi`, clamped to the grid; `None`
    /// if it misses the grid.
    fn cell_range(&self, lo: [f32; 2], hi: [f32; 2]) -> Option<[u32; 4]> {
        let cell = |v: f32, k: usize| floor((v - self.origin[k]) / CELL_M);
        let (x0, x1) = (cell(lo[0], 0), cell(hi[0], 0));
        let (z0, z1) = (cell(lo[1], 1), cell(hi[1], 1));
        let (nx, nz) = (self.cells[0] as f32, self.cells[1] as f32);
        if x1 < 0.0 || z1 < 0.0 || x0 >= nx || z0 >= nz || x0.is_nan() || z0.is_nan() {
            return None;
        }
        Some([
            x0.max(0.0) as u32,
            x1.min(nx - 1.0) as u32,
            z0.max(0.0) as u32,
            z1.min(nz - 1.0) as u32,
        ])
    }

    fn bin(&self, boxes: &[([f32; 2], [f32; 2])], margin: f32) -> Bins {
        let n_cells = (self.cells[0] * self.cells[1]) as usize;
        let mut count = alloc::vec![0u32; n_cells + 1];
        let range = |(lo, hi): &([f32; 2], [f32; 2])| {
            self.cell_range(
                [lo[0] - margin, lo[1] - margin],
                [hi[0] + margin, hi[1] + margin],
            )
        };
        for b in boxes {
            if let Some([x0, x1, z0, z1]) = range(b) {
                for z in z0..=z1 {
                    for x in x0..=x1 {
                        count[(z * self.cells[0] + x) as usize + 1] += 1;
                    }
                }
            }
        }
        for i in 0..n_cells {
            count[i + 1] += count[i];
        }
        let mut fill = count.clone();
        let mut items = alloc::vec![0u32; count[n_cells] as usize];
        for (i, b) in boxes.iter().enumerate() {
            if let Some([x0, x1, z0, z1]) = range(b) {
                for z in z0..=z1 {
                    for x in x0..=x1 {
                        let c = (z * self.cells[0] + x) as usize;
                        items[fill[c] as usize] = i as u32;
                        fill[c] += 1;
                    }
                }
            }
        }
        Bins {
            start: count,
            items,
        }
    }

    /// The items of `bins` in the cell containing `p` (empty outside the grid).
    fn cell_items<'a>(&self, bins: &'a Bins, p: [f32; 2]) -> &'a [u32] {
        match self.cell_range(p, p) {
            Some([x, _, z, _]) => {
                let c = (z * self.cells[0] + x) as usize;
                &bins.items[bins.start[c] as usize..bins.start[c + 1] as usize]
            }
            None => &[],
        }
    }

    /// Height of `patch` at `p` (also outside its polygon: the same surface extended).
    fn patch_height(&self, patch: &Patch, p: [f32; 2]) -> f32 {
        if patch.edge != NO_EDGE {
            let [a, b] = self.profile_range[patch.edge as usize];
            let (a, b) = (a as usize, b as usize);
            let h =
                crate::roads::profile_height(&self.profile_xz[a..b], &self.profile_y[a..b], v2(p));
            return h + patch.y;
        }
        if patch.grad == [0.0, 0.0] {
            return patch.y;
        }
        patch.y + patch.grad[0] * (p[0] - patch.at[0]) + patch.grad[1] * (p[1] - patch.at[1])
    }

    /// Height of the ground where no patch is.
    fn open_height(&self, p: [f32; 2]) -> f32 {
        match &self.terrain {
            Some(t) => t.height_at(p[0], p[1]),
            None => self.apron_y,
        }
    }

    fn under_height(&self, u: Under, p: [f32; 2]) -> f32 {
        match u {
            Under::Patch(i) => self.patch_height(&self.patches[i as usize], p),
            Under::Open => self.open_height(p),
        }
    }

    /// The ground under `p` seen from height `y_from`: the first patch containing it; where
    /// patches (or a deck and the open ground) lie a deck's height apart, the highest at
    /// most [`LAYER_STEP_M`] above `y_from`, else the lowest.
    fn under(&self, p: [f32; 2], y_from: f32) -> (Under, f32) {
        let mut first: Option<(u32, f32)> = None;
        let mut layered = false;
        let (mut reach, mut low) = (None::<(u32, f32)>, None::<(u32, f32)>);
        for &i in self.cell_items(&self.patch_bins, p) {
            let patch = &self.patches[i as usize];
            if p[0] < patch.lo[0] || p[0] > patch.hi[0] || p[1] < patch.lo[1] || p[1] > patch.hi[1]
            {
                continue;
            }
            let poly = &self.points[patch.start as usize..(patch.start + patch.len) as usize];
            if !point_in(p, poly) {
                continue;
            }
            let h = self.patch_height(patch, p);
            match first {
                None => first = Some((i, h)),
                Some((_, h0)) => layered |= abs(h - h0) > LAYER_M,
            }
            if h <= y_from + LAYER_STEP_M && reach.is_none_or(|r| h > r.1) {
                reach = Some((i, h));
            }
            if low.is_none_or(|l| h < l.1) {
                low = Some((i, h));
            }
        }
        let Some((i0, h0)) = first else {
            return (Under::Open, self.open_height(p));
        };
        if self.terrain.is_some() && h0 > y_from + LAYER_STEP_M {
            // Under a deck: the open ground, if that is below.
            let g = self.open_height(p);
            if g <= y_from + LAYER_STEP_M && h0 - g > LAYER_M {
                return match reach {
                    Some((i, h)) if h > g => (Under::Patch(i), h),
                    _ => (Under::Open, g),
                };
            }
        }
        if !layered {
            return (Under::Patch(i0), h0);
        }
        match reach.or(low) {
            Some((i, h)) => (Under::Patch(i), h),
            None => (Under::Patch(i0), h0),
        }
    }

    fn surface_of(&self, u: Under) -> Surface {
        match u {
            Under::Patch(i) => self.patches[i as usize].surface,
            Under::Open => Surface::Grass,
        }
    }

    /// How far a point at height `y` over ground point `p` (`[x, z]`) is under the water:
    /// 0 above the surface, on dry land (the open ground there above the water level) and
    /// on places without terrain.
    pub fn water_depth(&self, p: [f32; 2], y: f32) -> f32 {
        let Some(g) = &self.terrain else {
            return 0.0;
        };
        if g.height_at(p[0], p[1]) >= g.water_level {
            return 0.0;
        }
        (g.water_level - y).max(0.0)
    }

    /// Height and surface of the ground at `[x, z]` seen from above everything, steps
    /// ignored.
    pub fn level_at(&self, p: [f32; 2]) -> (f32, Surface) {
        self.level_from(p, f32::INFINITY)
    }

    /// Height and surface of the ground at `[x, z]` seen from height `y_from` (which layer
    /// of a bridge), steps ignored.
    pub fn level_from(&self, p: [f32; 2], y_from: f32) -> (f32, Surface) {
        let (u, y) = self.under(p, y_from);
        (y, self.surface_of(u))
    }

    /// Unit normal of the ground `u` at `p`.
    fn normal_of(&self, u: Under, p: [f32; 2]) -> [f32; 3] {
        let flat = match u {
            Under::Patch(i) => {
                let patch = &self.patches[i as usize];
                patch.edge == NO_EDGE && patch.grad == [0.0, 0.0]
            }
            Under::Open => self.terrain.is_none(),
        };
        if flat {
            return [0.0, 1.0, 0.0];
        }
        let e = 0.25;
        let h = |dx: f32, dz: f32| self.under_height(u, [p[0] + dx, p[1] + dz]);
        let gx = (h(e, 0.0) - h(-e, 0.0)) / (2.0 * e);
        let gz = (h(0.0, e) - h(0.0, -e)) / (2.0 * e);
        let inv = 1.0 / sqrt(gx * gx + 1.0 + gz * gz);
        [-gx * inv, inv, -gz * inv]
    }

    /// The ground a wheel of [`WHEEL_RADIUS_M`] centered over `[x, z]` rests on, seen from
    /// above everything.
    pub fn ground_at(&self, p: [f32; 2]) -> GroundSample {
        self.ground_from(p, f32::INFINITY)
    }

    /// The ground a wheel of [`WHEEL_RADIUS_M`] centered over `[x, z]` rests on, seen from
    /// height `y_from`.
    pub fn ground_from(&self, p: [f32; 2], y_from: f32) -> GroundSample {
        let (u, y0) = self.under(p, y_from);
        let surface = self.surface_of(u);
        let mut y = y0;
        let mut normal = self.normal_of(u, p);
        let r = WHEEL_RADIUS_M;
        for &i in self.cell_items(&self.step_bins, p) {
            let s = &self.steps[i as usize];
            if s.y_high <= y0 + STEP_MIN_M * 0.5 || s.y_high > y0 + MAX_STEP_UP_M {
                continue;
            }
            // Only from the low side: on top, the wheel sits on the upper patch.
            if (p[0] - s.a[0]) * s.up[0] + (p[1] - s.a[1]) * s.up[1] > 0.0 {
                continue;
            }
            let e = closest_on_segment(p, s.a, s.b);
            let (dx, dz) = (p[0] - e[0], p[1] - e[1]);
            let d2 = dx * dx + dz * dz;
            if d2 >= r * r {
                continue;
            }
            let h = sqrt(r * r - d2);
            let touch = s.y_high - r + h;
            if touch > y {
                y = touch;
                normal = [dx / r, h / r, dz / r];
            }
        }
        GroundSample { y, normal, surface }
    }

    /// The rise from `before` to `now` across whatever step lies between them: the ground
    /// at `now` minus the ground under `before` carried on to `now` (so a slope is no
    /// rise), both seen from `y_from`.
    fn rise(&self, before: [f32; 2], now: [f32; 2], y_from: f32) -> f32 {
        let (u, _) = self.under(before, y_from);
        let (_, y_now) = self.under(now, y_from);
        y_now - self.under_height(u, now)
    }

    /// Every patch edge piece across which the level changes, emitted once: from its lower
    /// patch, or from the upper one where the lower side is the apron.
    fn find_steps(&self) -> Vec<Step> {
        let mut out = Vec::new();
        for patch in &self.patches {
            let poly = &self.points[patch.start as usize..(patch.start + patch.len) as usize];
            let own = |p: [f32; 2]| self.patch_height(patch, p) + LAYER_STEP_M * 0.5;
            let v2s: Vec<V2> = poly.iter().map(|&p| v2(p)).collect();
            let ccw = signed_area(&v2s) > 0.0;
            let n = poly.len();
            for i in 0..n {
                let (a, b) = (v2(poly[i]), v2(poly[(i + 1) % n]));
                let len = (b - a).length();
                if len < 1e-4 {
                    continue;
                }
                let d = (b - a) * (1.0 / len);
                let out_n = if ccw { d.right() } else { d.left() };
                let pieces = (len / STEP_PIECE_M) as u32 + 1;
                for k in 0..pieces {
                    let p0 = a.lerp(b, k as f32 / pieces as f32);
                    let p1 = a.lerp(b, (k + 1) as f32 / pieces as f32);
                    let mid = (p0 + p1) * 0.5;
                    let inside = arr(mid - out_n * STEP_SAMPLE_M);
                    let (_, y_in) = self.under(inside, own(inside));
                    let outside = arr(mid + out_n * STEP_SAMPLE_M);
                    let (u_out, y_out) = self.under(outside, own(outside));
                    let apron = u_out == Under::Open;
                    let up = if y_out > y_in + STEP_MIN_M {
                        out_n
                    } else if apron && y_in > y_out + STEP_MIN_M {
                        -out_n
                    } else {
                        continue;
                    };
                    out.push(Step {
                        a: arr(p0),
                        b: arr(p1),
                        up: arr(up),
                        y_high: y_in.max(y_out),
                    });
                }
            }
        }
        out
    }
}

impl Ground for WorldCollider {
    fn probe(&self, origin: [f32; 3], dir: [f32; 3], max_len: f32) -> Option<Hit> {
        if dir[1] > -1e-6 {
            return None;
        }
        // The ground is flat between steps: intersect at the height under the origin, then
        // once more at the height under that first hit (for tilted rays).
        let g0 = self.ground_from([origin[0], origin[2]], origin[1]);
        let t0 = (origin[1] - g0.y) / -dir[1];
        let at = [origin[0] + dir[0] * t0, origin[2] + dir[2] * t0];
        let g = self.ground_from(at, origin[1]);
        let t = (origin[1] - g.y) / -dir[1];
        if t > max_len {
            return None;
        }
        Some(Hit {
            t: if t > 0.0 { t } else { 0.0 },
            normal: g.normal,
            friction: g.surface.friction(),
        })
    }
}

/// Asphalt ribbons and junctions, then sidewalk strips and corners, as the road meshes lay
/// them out (`roads::build`): flat where a road's profile is, else following it (ribbons
/// and strips) or as planar triangles (junction fans and corners).
fn road_patches(place: &Place, net: &Network, out: &mut Shapes) {
    let height = |i: usize, lift: f32| match net.paths[i].flat {
        Some(y) => Height::Flat(y + lift),
        None => Height::Edge(i as u32, lift),
    };
    let xz = |p: &crate::math::V3| V2::new(p.x, p.z);
    // A polygon of 3D points: one flat patch when level, else a fan of planar triangles.
    let fan = |out: &mut Shapes, pts: &[crate::math::V3], surface: Surface| {
        if pts.iter().all(|p| p.y == pts[0].y) {
            let poly: Vec<V2> = pts.iter().map(xz).collect();
            out.patch(&poly, pts[0].y, surface);
        } else {
            for k in 1..pts.len() - 1 {
                let (a, b, c) = (pts[0], pts[k], pts[k + 1]);
                let h = Height::Plane(a.to_array(), b.to_array(), c.to_array());
                out.patch_with(&[xz(&a), xz(&b), xz(&c)], h, surface);
            }
        }
    };
    for i in 0..place.roads.edges.len() {
        let path = &net.paths[i];
        let (ea, eb) = net.ends(i);
        let (s0, s1, hw) = (ea.trim, path.len() - eb.trim, ea.hw);
        if s1 > s0 {
            path.pieces(s0, s1, |a, b| {
                let q = [
                    path.xz(a, -hw),
                    path.xz(b, -hw),
                    path.xz(b, hw),
                    path.xz(a, hw),
                ];
                out.patch_with(&q, height(i, 0.0), Surface::Asphalt);
            });
        }
    }
    for j in &net.junctions {
        let ring = net.junction_ring(j);
        if ring.is_empty() {
            continue;
        }
        if ring.iter().all(|p| p.y == j.y) {
            let poly: Vec<V2> = ring.iter().map(xz).collect();
            out.patch(&poly, j.y, Surface::Asphalt);
        } else {
            let c = j.pos.at(j.y);
            for k in 0..ring.len() {
                let (a, b) = (ring[k], ring[(k + 1) % ring.len()]);
                fan(out, &[c, a, b], Surface::Asphalt);
            }
        }
    }
    for j in &net.junctions {
        for mg in &j.merges {
            let w = net.merge_wedge(j, mg);
            for k in 0..w.len().saturating_sub(1) {
                let q = [w[k].0, w[k + 1].0, w[k + 1].1, w[k].1];
                fan(out, &q, Surface::Asphalt);
            }
        }
    }
    for i in 0..place.roads.edges.len() {
        let path = &net.paths[i];
        let (ea, _) = net.ends(i);
        // Without a sidewalk the curb top is still a 0.2 m strip.
        let sw = if ea.sw > 0.0 { ea.sw } else { 0.2 };
        let hw = ea.hw;
        let [left, right] = net.side_spans(i);
        let spans = left
            .into_iter()
            .map(|s| (1.0, s))
            .chain(right.into_iter().map(|s| (-1.0, s)));
        for (sigma, [s0, s1]) in spans {
            if s1 > s0 {
                let (t0, t1) = (sigma * hw, sigma * (hw + sw));
                path.pieces(s0, s1, |a, b| {
                    let q = [
                        path.xz(a, t0),
                        path.xz(b, t0),
                        path.xz(b, t1),
                        path.xz(a, t1),
                    ];
                    out.patch_with(&q, height(i, CURB_M), Surface::Concrete);
                });
            }
        }
    }
    for j in &net.junctions {
        for i in 0..j.corners.len() {
            fan(out, &net.corner_walk(j, i), Surface::Concrete);
        }
    }
}

/// One outward-facing wall per footprint edge, from below the ground to the wall top.
fn building_walls(place: &Place) -> Vec<Wall> {
    let mut walls = Vec::new();
    let terrain = crate::is_terrain_place(place);
    for b in &place.buildings {
        let fp = ground::to_v2(&b.footprint);
        // On a terrain place the walls stand on the ground under them.
        let (y0, y1) = if terrain {
            let (base, drop) = buildings::footing(&b.footprint, |x, z| {
                place
                    .terrain
                    .height_at(&place.header, [x, z])
                    .unwrap_or(0.0)
            });
            (base - drop - 1.0, base + LOT_Y + b.height_m)
        } else {
            (-1.0, LOT_Y + b.height_m)
        };
        let ccw = signed_area(&fp) > 0.0;
        let n = fp.len();
        for i in 0..n {
            let (a, c) = (fp[i], fp[(i + 1) % n]);
            let d = (c - a).normalize();
            if d.length() == 0.0 {
                continue;
            }
            let normal = if ccw { d.right() } else { d.left() };
            walls.push(Wall {
                a: arr(a),
                b: arr(c),
                n: arr(normal),
                y0,
                y1,
            });
        }
    }
    walls
}

/// Every prop [`crate::build_chunk_meshes`] places, in the same passes: parked cars in
/// strip-mall lots, house sites, then street and park furniture. The meshes those passes
/// also emit go to scratch buffers and are dropped.
fn props_of(place: &Place, net: &Network, blocks: &[ground::Block]) -> Vec<Prop> {
    let seed = place.header.seed;
    let pal = Palette::socal_1985();
    let lookup = crate::Lookup::new(place, net);
    let mut scratch = MeshSet::new(place.header.chunk_size_m, crate::Keep::Nothing);
    let mut mesh = WorldMesh::default();
    let mut out = Vec::new();
    let mut house = Vec::new();
    for (i, b) in place.buildings.iter().enumerate() {
        let env = lookup.facade_envs(i);
        let door_pick = skywave_core::hash64(seed, &[0x444F_4F52, i as u64]) as u32;
        let plan = buildings::plan_building(b, &env, door_pick);
        if b.style == place::StyleTag::StripMall {
            let fp = ground::to_v2(&b.footprint);
            let c = ground::centroid(&fp);
            if let Some(blk) = blocks
                .iter()
                .find(|blk| crate::math::point_in_polygon(c, &blk.poly))
            {
                ground::parking_lot(
                    &fp,
                    plan.frontage,
                    blk,
                    &pal,
                    seed ^ i as u64,
                    &mut scratch,
                    &mut out,
                );
            }
        }
        mesh.vertices.clear();
        mesh.indices.clear();
        house.extend(buildings::house_site(
            &mut mesh, b, &plan, &env, &pal, seed, i as u32,
        ));
    }
    out.extend(house);
    let side_zone = crate::side_zones(place, net);
    let parks: Vec<Vec<V2>> = blocks
        .iter()
        .filter(|b| ground::is_park(place, b))
        .map(|b| b.poly.clone())
        .collect();
    out.extend(props::place(place, net, &side_zone, &parks, seed));
    out
}

/// Collision shapes of props: radius and height of each post, half extents of each box.
fn prop_solids(props: &[Prop]) -> (Vec<Post>, Vec<BoxSolid>) {
    let mut posts = Vec::new();
    let mut boxes = Vec::new();
    for p in props {
        let post = |r: f32, h: f32| Post {
            c: arr(p.pos),
            r,
            y0: p.y,
            y1: p.y + h,
            kind: p.kind,
        };
        match p.kind {
            PropKind::StreetLight => posts.push(post(0.15, 8.0)),
            PropKind::PalmTree => posts.push(post(0.3, 8.0)),
            PropKind::Oak => posts.push(post(0.35, 3.0)),
            PropKind::Hydrant => posts.push(post(0.2, 0.72)),
            PropKind::StopSign => posts.push(post(0.06, 2.4)),
            PropKind::TrafficSignal { .. } => posts.push(post(0.16, 6.3)),
            PropKind::TrashCan => posts.push(post(0.3, 1.0)),
            PropKind::Fountain => posts.push(post(4.2, 0.6)),
            PropKind::Bench => boxes.push(BoxSolid {
                c: arr(p.pos),
                u: arr(p.dir.left()),
                half: [0.95, 0.3],
                y0: p.y,
                y1: p.y + 0.86,
                kind: p.kind,
            }),
            PropKind::ParkedCar { .. } => boxes.push(BoxSolid {
                c: arr(p.pos),
                u: arr(p.dir),
                half: [2.25, 0.88],
                y0: p.y + 0.1,
                y1: p.y + 1.42,
                kind: p.kind,
            }),
            PropKind::Eucalyptus => posts.push(post(0.32, 12.0)),
            PropKind::PowerPole => posts.push(post(0.15, 10.4)),
            PropKind::Mailbox => posts.push(post(0.12, 1.3)),
            PropKind::StreetSign => posts.push(post(0.05, 3.0)),
            PropKind::Billboard => {
                let along = p.dir.left();
                for x in [-3.2f32, 3.2] {
                    let c = p.pos + along * x - p.dir * 0.4;
                    posts.push(Post {
                        c: arr(c),
                        ..post(0.22, 9.2)
                    });
                }
            }
            PropKind::Hedge { len_dm } | PropKind::Fence { len_dm } => boxes.push(BoxSolid {
                c: arr(p.pos),
                u: arr(p.dir),
                half: [f32::from(len_dm) * 0.05, 0.45],
                y0: p.y - 0.5,
                y1: p.y + 1.6,
                kind: p.kind,
            }),
            PropKind::Pier { len_m } => {
                // The deck from its land end out, to the head's far end.
                let len = f32::from(len_m) + props::PIER_HEAD_M;
                boxes.push(BoxSolid {
                    c: arr(p.pos + p.dir * (len * 0.5)),
                    u: arr(p.dir),
                    half: [len * 0.5, props::PIER_HALF_WIDTH_M],
                    y0: p.y - 9.0,
                    y1: p.y + 1.1,
                    kind: p.kind,
                })
            }
            // Above the sidewalk, out of the car's way.
            PropKind::NeonSign { .. } => {}
        }
    }
    (posts, boxes)
}

/// Collision shapes of a terrain place's props, from its `Props` table: [`prop_solids`]
/// for all but the parked cars, which take their shape's size.
fn place_prop_solids(place: &Place) -> (Vec<Post>, Vec<BoxSolid>) {
    let seed = place.header.seed;
    let items = &place.props.items;
    let props: Vec<Prop> = items
        .iter()
        .filter(|p| !matches!(p.kind, place::PropKind::ParkedCar { .. }))
        .map(|p| props::from_place(p, seed))
        .collect();
    let (posts, mut boxes) = prop_solids(&props);
    for p in items {
        if let place::PropKind::ParkedCar { vehicle_class } = p.kind {
            let w = props::from_place(p, seed);
            let [len, width, height] = props::parked_car_size(vehicle_class);
            boxes.push(BoxSolid {
                c: arr(w.pos),
                u: arr(w.dir),
                half: [len * 0.5, width * 0.5],
                y0: w.y + 0.1,
                y1: w.y + height,
                kind: w.kind,
            });
        }
    }
    (posts, boxes)
}

// ---------------------------------------------------------------------------------------
// Body response.

type V3 = [f32; 3];

fn add3(a: V3, b: V3) -> V3 {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}

fn sub3(a: V3, b: V3) -> V3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn scale3(a: V3, s: f32) -> V3 {
    [a[0] * s, a[1] * s, a[2] * s]
}

fn dot3(a: V3, b: V3) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn cross3(a: V3, b: V3) -> V3 {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

/// `q` (unit, `[x, y, z, w]`) applied to `v`.
fn rotate(q: [f32; 4], v: V3) -> V3 {
    let u = [q[0], q[1], q[2]];
    let t = scale3(cross3(u, v), 2.0);
    add3(add3(v, scale3(t, q[3])), cross3(u, t))
}

fn rotate_inv(q: [f32; 4], v: V3) -> V3 {
    rotate([-q[0], -q[1], -q[2], q[3]], v)
}

/// The car's body box seen from above: a rectangle around `c` with unit axes `f`
/// (forward) and `r`, half extents `hf`, `hr`, spanning heights `y0..y1`.
#[derive(Clone, Copy)]
struct Footprint {
    c: [f32; 2],
    f: [f32; 2],
    r: [f32; 2],
    hf: f32,
    hr: f32,
    y0: f32,
    y1: f32,
}

impl Footprint {
    fn new(state: &VehicleState, spec: &VehicleSpec) -> Footprint {
        let [len, wid, hgt] = spec.dims_m;
        let q = state.rot;
        let right = rotate(q, [1.0, 0.0, 0.0]);
        let up = rotate(q, [0.0, 1.0, 0.0]);
        let fwd = rotate(q, [0.0, 0.0, -1.0]);
        let low = -spec.cg_height_m + BODY_CLEARANCE_M;
        let center = add3(state.pos, scale3(up, low + 0.5 * hgt));
        let half = [0.5 * wid, 0.5 * hgt, 0.5 * len];
        let axes = [right, up, fwd];
        let mut f = [fwd[0], fwd[2]];
        let fl = sqrt(f[0] * f[0] + f[1] * f[1]);
        if fl > 1e-3 {
            f = [f[0] / fl, f[1] / fl];
        } else {
            // Nose straight up or down: the roof points along the car's heading.
            let ul = sqrt(up[0] * up[0] + up[2] * up[2]);
            f = if ul > 1e-6 {
                [up[0] / ul, up[2] / ul]
            } else {
                [0.0, -1.0]
            };
        }
        let r = [-f[1], f[0]];
        let extent = |dir: [f32; 2]| {
            let mut e = 0.0;
            for k in 0..3 {
                e += half[k] * abs(axes[k][0] * dir[0] + axes[k][2] * dir[1]);
            }
            e
        };
        let mut ey = 0.0;
        for k in 0..3 {
            ey += half[k] * abs(axes[k][1]);
        }
        Footprint {
            c: [center[0], center[2]],
            f,
            r,
            hf: extent(f),
            hr: extent(r),
            y0: center[1] - ey,
            y1: center[1] + ey,
        }
    }

    fn local(&self, p: [f32; 2]) -> [f32; 2] {
        let d = [p[0] - self.c[0], p[1] - self.c[1]];
        [dot2(d, self.f), dot2(d, self.r)]
    }

    fn world(&self, l: [f32; 2]) -> [f32; 2] {
        [
            self.c[0] + self.f[0] * l[0] + self.r[0] * l[1],
            self.c[1] + self.f[1] * l[0] + self.r[1] * l[1],
        ]
    }

    fn world_dir(&self, l: [f32; 2]) -> [f32; 2] {
        [
            self.f[0] * l[0] + self.r[0] * l[1],
            self.f[1] * l[0] + self.r[1] * l[1],
        ]
    }

    /// Half the rectangle's width along the unit direction `n`.
    fn radius_along(&self, n: [f32; 2]) -> f32 {
        self.hf * abs(dot2(self.f, n)) + self.hr * abs(dot2(self.r, n))
    }

    fn overlaps_height(&self, y0: f32, y1: f32) -> bool {
        self.y0 < y1 && self.y1 > y0
    }
}

/// One contact: move the car by `depth` along `n` (unit, horizontal); `p` is the contact
/// point on the ground plane and `y0..y1` the height band the two bodies share.
#[derive(Clone, Copy)]
struct Contact {
    n: [f32; 2],
    depth: f32,
    p: [f32; 2],
    y0: f32,
    y1: f32,
}

/// The part of `a..b` inside the footprint, in footprint-local coordinates (Liang-Barsky).
fn clip_to_rect(fp: &Footprint, a: [f32; 2], b: [f32; 2]) -> Option<([f32; 2], [f32; 2])> {
    let (la, lb) = (fp.local(a), fp.local(b));
    let d = [lb[0] - la[0], lb[1] - la[1]];
    let (mut t0, mut t1) = (0.0f32, 1.0f32);
    let half = [fp.hf, fp.hr];
    for k in 0..2 {
        for (p, q) in [(-d[k], la[k] + half[k]), (d[k], half[k] - la[k])] {
            if p == 0.0 {
                if q < 0.0 {
                    return None;
                }
            } else {
                let t = q / p;
                if p < 0.0 {
                    t0 = t0.max(t);
                } else {
                    t1 = t1.min(t);
                }
            }
        }
    }
    if t0 >= t1 {
        return None;
    }
    let at = |t: f32| [la[0] + d[0] * t, la[1] + d[1] * t];
    Some((at(t0), at(t1)))
}

/// A one-sided wall against the footprint: the least push that clears it without moving
/// the car toward the building's side.
fn wall_contact(fp: &Footprint, w: &Wall) -> Option<Contact> {
    if !fp.overlaps_height(w.y0, w.y1) {
        return None;
    }
    let (l0, l1) = clip_to_rect(fp, w.a, w.b)?;
    let mid = fp.world([(l0[0] + l1[0]) * 0.5, (l0[1] + l1[1]) * 0.5]);
    // Along the wall normal: the footprint's deepest point behind the wall plane.
    let mut best_n = w.n;
    let mut best = dot2(w.a, w.n) - (dot2(fp.c, w.n) - fp.radius_along(w.n));
    // Along the footprint's own axes, where the wall's end pokes into a side.
    let (la, lb) = (fp.local(w.a), fp.local(w.b));
    let half = [fp.hf, fp.hr];
    for k in 0..2 {
        let (smin, smax) = (la[k].min(lb[k]), la[k].max(lb[k]));
        for (sign, depth) in [(1.0, smax + half[k]), (-1.0, half[k] - smin)] {
            let mut l = [0.0, 0.0];
            l[k] = sign;
            let dir = fp.world_dir(l);
            if dot2(dir, w.n) > 0.1 && depth < best {
                best = depth;
                best_n = dir;
            }
        }
    }
    if best <= 0.0 {
        return None;
    }
    Some(Contact {
        n: best_n,
        depth: best,
        p: mid,
        y0: w.y0.max(fp.y0),
        y1: w.y1.min(fp.y1),
    })
}

/// A cylinder against the footprint. `prev` is the cylinder's center in footprint-local
/// coordinates one step ago: a post that got inside is pushed back out the side it came in.
fn post_contact(fp: &Footprint, p: &Post, prev: [f32; 2]) -> Option<Contact> {
    if !fp.overlaps_height(p.y0, p.y1) {
        return None;
    }
    let l = fp.local(p.c);
    let half = [fp.hf, fp.hr];
    let q = [l[0].clamp(-half[0], half[0]), l[1].clamp(-half[1], half[1])];
    let (dx, dz) = (q[0] - l[0], q[1] - l[1]);
    let d2 = dx * dx + dz * dz;
    let (n_local, depth, at) = if d2 > 1e-12 {
        if d2 >= p.r * p.r {
            return None;
        }
        let d = sqrt(d2);
        ([dx / d, dz / d], p.r - d, q)
    } else {
        // Center inside: leave through the face it entered by, else the nearest one.
        let mut k = if half[0] - abs(l[0]) < half[1] - abs(l[1]) {
            0
        } else {
            1
        };
        let out0 = abs(prev[0]) - half[0];
        let out1 = abs(prev[1]) - half[1];
        if out0 > 0.0 || out1 > 0.0 {
            k = if out0 / half[0] > out1 / half[1] {
                0
            } else {
                1
            };
        }
        let side = if (if k == 0 { prev[0] } else { prev[1] }) > 0.0 {
            1.0
        } else {
            -1.0
        };
        let mut n = [0.0, 0.0];
        n[k] = -side;
        let depth = half[k] - side * l[k] + p.r;
        (n, depth, l)
    };
    Some(Contact {
        n: fp.world_dir(n_local),
        depth,
        p: fp.world(at),
        y0: p.y0.max(fp.y0),
        y1: p.y1.min(fp.y1),
    })
}

/// An upright box against the footprint: separating-axis test over both rectangles' axes;
/// the contact point is the mean of the corners each holds of the other.
fn box_contact(fp: &Footprint, b: &BoxSolid) -> Option<Contact> {
    if !fp.overlaps_height(b.y0, b.y1) {
        return None;
    }
    let bv = [-b.u[1], b.u[0]];
    let axes = [fp.f, fp.r, b.u, bv];
    let d = [b.c[0] - fp.c[0], b.c[1] - fp.c[1]];
    let mut best = f32::MAX;
    let mut best_n = [0.0, 0.0];
    for a in axes {
        let rb = b.half[0] * abs(dot2(b.u, a)) + b.half[1] * abs(dot2(bv, a));
        let sep = dot2(d, a);
        let overlap = fp.radius_along(a) + rb - abs(sep);
        if overlap <= 0.0 {
            return None;
        }
        if overlap < best {
            best = overlap;
            // Push the car away from the box.
            best_n = if sep > 0.0 { [-a[0], -a[1]] } else { a };
        }
    }
    let mut sum = [0.0, 0.0];
    let mut count = 0.0;
    for (sx, sy) in [(1.0, 1.0), (1.0, -1.0), (-1.0, 1.0), (-1.0, -1.0)] {
        let corner = [
            b.c[0] + b.u[0] * b.half[0] * sx + bv[0] * b.half[1] * sy,
            b.c[1] + b.u[1] * b.half[0] * sx + bv[1] * b.half[1] * sy,
        ];
        let l = fp.local(corner);
        if abs(l[0]) <= fp.hf && abs(l[1]) <= fp.hr {
            sum = [sum[0] + corner[0], sum[1] + corner[1]];
            count += 1.0;
        }
        let corner = fp.world([fp.hf * sx, fp.hr * sy]);
        let rel = [corner[0] - b.c[0], corner[1] - b.c[1]];
        if abs(dot2(rel, b.u)) <= b.half[0] && abs(dot2(rel, bv)) <= b.half[1] {
            sum = [sum[0] + corner[0], sum[1] + corner[1]];
            count += 1.0;
        }
    }
    let p = if count > 0.0 {
        [sum[0] / count, sum[1] / count]
    } else {
        [(fp.c[0] + b.c[0]) * 0.5, (fp.c[1] + b.c[1]) * 0.5]
    };
    Some(Contact {
        n: best_n,
        depth: best,
        p,
        y0: b.y0.max(fp.y0),
        y1: b.y1.min(fp.y1),
    })
}

/// The car's rigid-body numbers for impulses.
struct Body {
    inv_mass: f32,
    inertia: [f32; 3],
}

impl Body {
    /// World-space inverse inertia applied to `v`.
    fn inv_inertia(&self, q: [f32; 4], v: V3) -> V3 {
        let b = rotate_inv(q, v);
        rotate(
            q,
            [
                b[0] / self.inertia[0],
                b[1] / self.inertia[1],
                b[2] / self.inertia[2],
            ],
        )
    }

    /// Inverse effective mass at offset `r` from the center of mass along unit `n`.
    fn k(&self, q: [f32; 4], r: V3, n: V3) -> f32 {
        self.inv_mass + dot3(n, cross3(self.inv_inertia(q, cross3(r, n)), r))
    }

    fn apply(&self, state: &mut VehicleState, r: V3, impulse: V3) {
        state.vel = add3(state.vel, scale3(impulse, self.inv_mass));
        let dw = self.inv_inertia(state.rot, cross3(r, impulse));
        state.ang_vel = add3(state.ang_vel, dw);
    }
}

/// Positional correction, then restitution and friction impulses at the contact, then the
/// damping pulse that keeps the car from bouncing off. Returns the speed at which the
/// contact point approached (0 if it was already separating).
fn respond(state: &mut VehicleState, body: &Body, c: &Contact) -> f32 {
    let n = [c.n[0], 0.0, c.n[1]];
    state.pos[0] += c.n[0] * c.depth;
    state.pos[2] += c.n[1] * c.depth;
    let y = state.pos[1].clamp(c.y0, c.y1.max(c.y0));
    let r = sub3([c.p[0], y, c.p[1]], state.pos);
    let v_p = add3(state.vel, cross3(state.ang_vel, r));
    let vn = dot3(v_p, n);
    if vn >= 0.0 {
        return 0.0;
    }
    let j = -(1.0 + RESTITUTION) * vn / body.k(state.rot, r, n);
    let mut impulse = scale3(n, j);
    // Friction along the horizontal sliding direction, within the friction cone.
    let vt = [v_p[0] - n[0] * vn, 0.0, v_p[2] - n[2] * vn];
    let vt_len = sqrt(dot3(vt, vt));
    if vt_len > 1e-6 {
        let t = scale3(vt, 1.0 / vt_len);
        let jt = (vt_len / body.k(state.rot, r, t)).min(FRICTION * j);
        impulse = add3(impulse, scale3(t, -jt));
    }
    body.apply(state, r, impulse);

    // Damping pulse: cap the rebound, and bleed horizontal speed in proportion to the hit.
    let sep = dot3(state.vel, n);
    if sep > REBOUND_MAX_MPS {
        state.vel = add3(state.vel, scale3(n, REBOUND_MAX_MPS - sep));
    }
    let keep = 1.0 - HIT_DAMP * (-vn / HIT_DAMP_SPEED).min(1.0);
    state.vel[0] *= keep;
    state.vel[2] *= keep;
    -vn
}

/// What [`resolve`] did this step, for sound, camera shake and tests.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Impact {
    /// The body touched a wall, post or box.
    pub contact: bool,
    /// Fastest approach of a contact point this step, m/s (0 for resting contact).
    pub speed: f32,
    /// A wheel hit a step above [`CURB_CLIMB_MPS`] and kicked the body.
    pub curb_kick: bool,
}

/// Pushes the car out of walls, posts and boxes and answers each contact (restitution
/// [`RESTITUTION`], friction [`FRICTION`], then a damping pulse), and kicks the body up when
/// a wheel hits a curb above [`CURB_CLIMB_MPS`]. Call after every `vehicle::step` with the
/// same collider that step probed.
///
/// The car's center never crosses a wall between steps: the move since the last step is
/// swept against walls first and clamped at the first one crossed.
pub fn resolve(state: &mut VehicleState, spec: &VehicleSpec, collider: &WorldCollider) -> Impact {
    let mut impact = Impact::default();
    let body = Body {
        inv_mass: 1.0 / spec.mass_kg,
        inertia: spec.inertia_box(),
    };
    let moved = scale3(state.vel, DT);
    let prev = sub3(state.pos, moved);
    sweep_walls(state, prev, collider);

    let reach = 0.5 * sqrt(spec.dims_m[0] * spec.dims_m[0] + spec.dims_m[1] * spec.dims_m[1])
        + 0.5 * spec.dims_m[2]
        + 1.0;
    let lo = [state.pos[0] - reach, state.pos[2] - reach];
    let hi = [state.pos[0] + reach, state.pos[2] + reach];
    if let Some([x0, x1, z0, z1]) = collider.cell_range(lo, hi) {
        for _ in 0..CONTACT_PASSES {
            let mut touched = false;
            for z in z0..=z1 {
                for x in x0..=x1 {
                    let cell = (z * collider.cells[0] + x) as usize;
                    if let Some(v) = resolve_cell(state, spec, &body, collider, cell, moved) {
                        touched = true;
                        impact.speed = impact.speed.max(v);
                    }
                }
            }
            if !touched {
                break;
            }
            impact.contact = true;
        }
    }
    impact.curb_kick = curb_kick(state, spec, &body, collider);
    state.speed_mps = dot3(state.vel, rotate(state.rot, [0.0, 0.0, -1.0]));
    impact
}

fn bin_slice(bins: &Bins, cell: usize) -> &[u32] {
    &bins.items[bins.start[cell] as usize..bins.start[cell + 1] as usize]
}

/// One pass over the solids binned in `cell`: the fastest approach speed if any touched.
fn resolve_cell(
    state: &mut VehicleState,
    spec: &VehicleSpec,
    body: &Body,
    collider: &WorldCollider,
    cell: usize,
    moved: V3,
) -> Option<f32> {
    let mut touched = None;
    for &i in bin_slice(&collider.wall_bins, cell) {
        let fp = Footprint::new(state, spec);
        if let Some(c) = wall_contact(&fp, &collider.walls[i as usize]) {
            let v = respond(state, body, &c);
            touched = Some(touched.unwrap_or(0.0f32).max(v));
        }
    }
    for &i in bin_slice(&collider.post_bins, cell) {
        let fp = Footprint::new(state, spec);
        let post = &collider.posts[i as usize];
        let l = fp.local(post.c);
        let prev = [
            l[0] + dot2([moved[0], moved[2]], fp.f),
            l[1] + dot2([moved[0], moved[2]], fp.r),
        ];
        if let Some(c) = post_contact(&fp, post, prev) {
            let v = respond(state, body, &c);
            touched = Some(touched.unwrap_or(0.0f32).max(v));
        }
    }
    for &i in bin_slice(&collider.box_bins, cell) {
        let fp = Footprint::new(state, spec);
        if let Some(c) = box_contact(&fp, &collider.boxes[i as usize]) {
            let v = respond(state, body, &c);
            touched = Some(touched.unwrap_or(0.0f32).max(v));
        }
    }
    touched
}

/// Continuous check: if the center went through a wall's outside face since `prev`, put it
/// back just outside the first such wall.
fn sweep_walls(state: &mut VehicleState, prev: V3, collider: &WorldCollider) {
    let (p0, p1) = ([prev[0], prev[2]], [state.pos[0], state.pos[2]]);
    let d = [p1[0] - p0[0], p1[1] - p0[1]];
    if d[0] == 0.0 && d[1] == 0.0 {
        return;
    }
    let (lo, hi) = bounds_of(&[p0, p1]);
    let Some([x0, x1, z0, z1]) = collider.cell_range(lo, hi) else {
        return;
    };
    let mut first = 1.0f32;
    let mut normal = [0.0, 0.0];
    for z in z0..=z1 {
        for x in x0..=x1 {
            let cell = (z * collider.cells[0] + x) as usize;
            for &i in bin_slice(&collider.wall_bins, cell) {
                let w = &collider.walls[i as usize];
                if state.pos[1] < w.y0 || state.pos[1] > w.y1 {
                    continue;
                }
                let s0 = dot2([p0[0] - w.a[0], p0[1] - w.a[1]], w.n);
                let s1 = dot2([p1[0] - w.a[0], p1[1] - w.a[1]], w.n);
                if s0 < 0.0 || s1 >= 0.0 {
                    continue;
                }
                let t = s0 / (s0 - s1);
                let hit = [p0[0] + d[0] * t, p0[1] + d[1] * t];
                let e = [w.b[0] - w.a[0], w.b[1] - w.a[1]];
                let along = dot2([hit[0] - w.a[0], hit[1] - w.a[1]], e);
                if along < 0.0 || along > dot2(e, e) {
                    continue;
                }
                if t < first {
                    first = t;
                    normal = w.n;
                }
            }
        }
    }
    if first < 1.0 {
        state.pos[0] = p0[0] + d[0] * first + normal[0] * SWEEP_MARGIN_M;
        state.pos[2] = p0[1] + d[1] * first + normal[1] * SWEEP_MARGIN_M;
    }
}

/// A wheel that went up a step faster than [`CURB_CLIMB_MPS`] kicks its corner of the body
/// upward, harder the faster and higher.
fn curb_kick(
    state: &mut VehicleState,
    spec: &VehicleSpec,
    body: &Body,
    collider: &WorldCollider,
) -> bool {
    let wheels = spec.wheel_positions();
    let mut kicked = false;
    for (i, mount) in wheels.iter().enumerate() {
        if !state.wheels[i].grounded {
            continue;
        }
        let r = rotate(state.rot, *mount);
        let at = add3(state.pos, r);
        let v = add3(state.vel, cross3(state.ang_vel, r));
        let speed = sqrt(v[0] * v[0] + v[2] * v[2]);
        if speed <= CURB_CLIMB_MPS {
            continue;
        }
        let before = [at[0] - v[0] * DT, at[2] - v[2] * DT];
        let rise = collider.rise(before, [at[0], at[2]], at[1]);
        if rise < STEP_MIN_M {
            continue;
        }
        let up = [0.0, 1.0, 0.0];
        let kick = (CURB_KICK_PER_MPS_M * rise * (speed - CURB_CLIMB_MPS)).min(CURB_KICK_MAX);
        let j = kick / body.k(state.rot, r, up);
        body.apply(state, r, scale3(up, j));
        kicked = true;
    }
    kicked
}

/// Compile-time proof that the collider's records are plain data.
const _: () = {
    const fn copy<T: Copy>() {}
    copy::<Patch>();
    copy::<Step>();
    copy::<Wall>();
    copy::<Post>();
    copy::<BoxSolid>();
    copy::<GroundSample>();
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clip_keeps_the_inside_part() {
        let fp = Footprint {
            c: [0.0, 0.0],
            f: [1.0, 0.0],
            r: [0.0, 1.0],
            hf: 2.0,
            hr: 1.0,
            y0: 0.0,
            y1: 1.0,
        };
        let (a, b) = clip_to_rect(&fp, [-5.0, 0.5], [5.0, 0.5]).unwrap();
        assert_eq!(a, [-2.0, 0.5]);
        assert_eq!(b, [2.0, 0.5]);
        assert!(clip_to_rect(&fp, [-5.0, 1.5], [5.0, 1.5]).is_none());
        // A wall across the nose, facing the car: pushed straight back by the overlap.
        let w = Wall {
            a: [1.5, 3.0],
            b: [1.5, -3.0],
            n: [-1.0, 0.0],
            y0: -1.0,
            y1: 5.0,
        };
        let c = wall_contact(&fp, &w).unwrap();
        assert_eq!(c.n, [-1.0, 0.0]);
        assert!((c.depth - 0.5).abs() < 1e-6);
        assert_eq!(c.p, [1.5, 0.0]);
    }

    #[test]
    fn post_inside_leaves_the_way_it_came() {
        let fp = Footprint {
            c: [0.0, 0.0],
            f: [1.0, 0.0],
            r: [0.0, 1.0],
            hf: 2.0,
            hr: 1.0,
            y0: 0.0,
            y1: 1.0,
        };
        let post = Post {
            c: [1.4, 0.3],
            r: 0.15,
            y0: 0.0,
            y1: 8.0,
            kind: PropKind::StreetLight,
        };
        // Came in through the nose, though the right side is nearer.
        let c = post_contact(&fp, &post, [2.3, 0.3]).unwrap();
        assert_eq!(c.n, [-1.0, 0.0]);
        assert!((c.depth - (0.6 + 0.15)).abs() < 1e-6);
    }
}
