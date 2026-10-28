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
//! posts and boxes and answers each with a plain impulse (a nudge back, a speed loss in
//! proportion to the closing speed, a little yaw off center, a dip the springs rock out),
//! kicks the body when a wheel hits a curb fast, and reports the hits as [`Bump`]s for the
//! sound. Parked cars stay put; [`Rocking`] sways the ones the car knocks, for drawing.
//!
//! Water: the car rolls on the sea floor under it; [`WorldCollider::water_depth`] says how
//! deep a point is under the surface, and the driver's side (`app::drive`) drags a car
//! that wades in deep and puts it back at its start.
//!
//! Queries allocate nothing and use only IEEE arithmetic in a fixed order, so the same
//! inputs give bit-identical results everywhere.

use alloc::vec::Vec;

use place::{Place, ALL_HOURS};
use vehicle::sim::{self, DT, WHEEL_RADIUS_M};
use vehicle::{Ground, Hit, Rock, VehicleSpec, VehicleState};

use crate::buildings;
use crate::grammar::Grammar;
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
/// Contact response (`docs/VEHICLES.md` item 6): a plain impulse. The contact point leaves
/// what it hit at this share of the closing speed, at most [`REBOUND_MAX_MPS`] (a nudge
/// back, never a bounce)...
const RESTITUTION: f32 = 0.1;
const REBOUND_MAX_MPS: f32 = 0.4;
/// ...with light friction along the contact...
const FRICTION: f32 = 0.3;
/// ...then the car loses this share of its horizontal speed in a hit closing at
/// [`SCRUB_SPEED`] or harder (in proportion below it)...
const SCRUB: f32 = 0.4;
const SCRUB_SPEED: f32 = 12.0;
/// ...and the body pitches or rolls toward what it hit at this rate per m/s of closing
/// speed, rad/s, at most [`ROCK_MAX`]: the springs rock it back.
const ROCK_PER_MPS: f32 = 0.03;
const ROCK_MAX: f32 = 0.4;
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
/// [`Patch::edge`] of a patch that lies `y` above a terrain place's graded terrain (a
/// parking lot's pieces).
pub const TERRAIN_EDGE: u32 = u32::MAX - 1;

/// How a patch's height varies, while building.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Height {
    Flat(f32),
    /// Offset above the terrain.
    Terrain(f32),
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
    /// A parking lot's car: its index into [`WorldCollider::lot_cars`] (it is there only
    /// while that says so); [`NO_LOT`] for everything else.
    pub lot: u32,
}

/// [`BoxSolid::lot`] of every box but a lot's car.
pub const NO_LOT: u32 = u32::MAX;

/// A parking lot's car (a `place::PropKind::ParkedCar` whose hours are not
/// [`ALL_HOURS`]): where it stands, its hours, its box and whether it is there now.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LotCar {
    pub at: [f32; 3],
    pub hours: u32,
    /// Index into [`WorldCollider::boxes`].
    pub solid: u32,
    pub present: bool,
}

/// A lot car within this distance of the eye keeps whether it is there, whichever way the
/// eye looks, m.
pub const HOLD_M: f32 = 60.0;
/// Out to this distance a lot car keeps it while inside [`SEEN_COS`] of the way the eye
/// looks, m...
pub const SEEN_M: f32 = 150.0;
/// ...the cosine of the half-angle of that cone (90°: the half-plane ahead, wider than any
/// camera's view).
pub const SEEN_COS: f32 = 0.0;

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
    /// The parking lots' cars, in `Props` order: which are there now
    /// ([`WorldCollider::advance_lot_cars`]).
    pub lot_cars: Vec<LotCar>,
    /// Whether the lot cars have been put where the clock says, once, unseen.
    lots_settled: bool,
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
            Height::Terrain(y) => (y, TERRAIN_EDGE, [0.0; 2], [0.0; 2]),
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
    pub fn build(place: &Place, grammar: &Grammar) -> WorldCollider {
        let mut net = Network::new(place);
        let lots = crate::lots::plan(place, &net, &grammar.lots);
        net.cuts = crate::lots::cuts(&lots);
        let mut shapes = Shapes {
            points: Vec::new(),
            patches: Vec::new(),
        };
        // Patches in priority order: asphalt, then dropped curbs and sidewalks, then
        // parking lots (their raised pieces first), then lot pads.
        road_patches(place, &net, &mut shapes);
        let terrain_place = crate::is_terrain_place(place);
        let terrain = terrain_place.then(|| TerrainGrid::new(place, &net));
        let mut lot_walls = Vec::new();
        if let Some(g) = &terrain {
            lot_patches(&lots, &net, g, &mut shapes, &mut lot_walls);
        }
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

        let mut walls = building_walls(place);
        if terrain_place {
            walls.extend(edge_walls(place));
        }
        let (posts, mut boxes, lot_cars) = if terrain_place {
            place_prop_solids(place, &grammar.landmarks)
        } else {
            let props = props_of(place, grammar, &net, &blocks);
            let (posts, boxes) = prop_solids(&props, &grammar.landmarks);
            (posts, boxes, Vec::new())
        };
        boxes.extend(lot_walls);

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
            terrain,
            profile_xz: Vec::new(),
            profile_y: Vec::new(),
            profile_range: Vec::new(),
            points: shapes.points,
            patches: shapes.patches,
            steps: Vec::new(),
            walls,
            posts,
            boxes,
            lot_cars,
            lots_settled: false,
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
        if patch.edge == TERRAIN_EDGE {
            return self.open_height(p) + patch.y;
        }
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

    /// Whether box `b` is there now (a lot's car only while [`LotCar::present`]).
    pub fn present(&self, b: &BoxSolid) -> bool {
        b.lot == NO_LOT || self.lot_cars[b.lot as usize].present
    }

    /// Brings the lots' cars toward where the clock says they are at `seconds_of_day`
    /// ([`props::parked_now`]), without anyone seeing one come or go: a car within
    /// [`HOLD_M`] of `eye`, or within [`SEEN_M`] inside the cone round `forward`, stays as
    /// it is, and none arrives
    /// where the car `state` (of `spec`) stands. The first call puts them all where the
    /// clock says (but for the car's place). Allocates nothing.
    pub fn advance_lot_cars(
        &mut self,
        seconds_of_day: f32,
        eye: [f32; 3],
        forward: [f32; 3],
        state: &VehicleState,
        spec: &VehicleSpec,
    ) {
        let first = !self.lots_settled;
        self.lots_settled = true;
        let fp = Footprint::new(state, spec);
        let f = [forward[0], forward[2]];
        let fl = sqrt(dot2(f, f));
        let boxes = &self.boxes;
        for car in &mut self.lot_cars {
            let want = props::parked_now(car.hours, car.at, seconds_of_day);
            if want == car.present {
                continue;
            }
            let d = [car.at[0] - eye[0], car.at[2] - eye[2]];
            let dl = sqrt(dot2(d, d));
            let seen = dl < HOLD_M || (dl < SEEN_M && dot2(d, f) > SEEN_COS * dl * fl);
            if !first && seen {
                continue;
            }
            if want && box_contact(&fp, &boxes[car.solid as usize]).is_some() {
                continue;
            }
            car.present = want;
        }
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
        self.ground_and_step(p, y_from).0
    }

    /// [`Self::ground_from`], and whether a step's edge rather than the ground under `p`
    /// gave it.
    fn ground_and_step(&self, p: [f32; 2], y_from: f32) -> (GroundSample, bool) {
        let (u, y0) = self.under(p, y_from);
        let surface = self.surface_of(u);
        let mut y = y0;
        let mut normal = self.normal_of(u, p);
        let mut stepped = false;
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
                stepped = true;
            }
        }
        (GroundSample { y, normal, surface }, stepped)
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
        // Intersect the ground's tangent plane under the origin, then once more the plane
        // under that first hit (a Newton step each; exact on planar ground). Matching the
        // height under the origin alone does not converge on a slope as steep as the ray is
        // tilted: on a 45° hill a wheel ray along the ground's normal read the wheel as
        // buried, and the bump stop flung the car. A step's edge is the height a wheel
        // touches it at, not a surface: that stays a level plane at the touch height.
        let level =
            |g: &GroundSample, stepped: bool| if stepped { [0.0, 1.0, 0.0] } else { g.normal };
        let o = [origin[0], origin[2]];
        let (g0, s0) = self.ground_and_step(o, origin[1]);
        let t0 = plane_t(origin, dir, o, g0.y, level(&g0, s0));
        let at = [origin[0] + dir[0] * t0, origin[2] + dir[2] * t0];
        let (g, s) = self.ground_and_step(at, origin[1]);
        let t = plane_t(origin, dir, at, g.y, level(&g, s));
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

/// Distance along the ray `origin + t * dir` (pointing down) to the plane through
/// `[at[0], y, at[1]]` with unit normal `n`; to the level plane at `y` where the ray grazes
/// that one. For a level `n` both are bit-identical to `(origin[1] - y) / -dir[1]`.
fn plane_t(origin: [f32; 3], dir: [f32; 3], at: [f32; 2], y: f32, n: [f32; 3]) -> f32 {
    let den = dir[0] * n[0] + dir[1] * n[1] + dir[2] * n[2];
    if den > -0.1 {
        return (origin[1] - y) / -dir[1];
    }
    ((at[0] - origin[0]) * n[0] + (y - origin[1]) * n[1] + (at[1] - origin[2]) * n[2]) / den
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
    // Dropped curbs: the apron halfway up to the walk (level with the road where there is
    // no walk), ahead of the walks.
    for c in &net.cuts {
        let i = c.edge as usize;
        let path = &net.paths[i];
        let (ea, _) = net.ends(i);
        let sigma = if c.left { 1.0 } else { -1.0 };
        let (hw, walk) = (ea.hw, ea.sw > 0.0);
        let (t1, lift) = if walk {
            (hw + 0.2 + crate::roads::APRON_M, CURB_M * 0.5)
        } else {
            (hw + 0.2, 0.0)
        };
        path.pieces(c.s0, c.s1, |a, b| {
            let q = [
                path.xz(a, sigma * hw),
                path.xz(b, sigma * hw),
                path.xz(b, sigma * t1),
                path.xz(a, sigma * t1),
            ];
            out.patch_with(&q, height(i, lift), Surface::Concrete);
        });
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

/// Along every land side of a terrain place, the face of its wall ([`crate::edges`]) as
/// vertical walls 16 m long facing into the place, from under the ground to well above
/// the crest.
fn edge_walls(place: &Place) -> Vec<Wall> {
    use crate::edges::face_inside;
    use place::{NORTH, SOUTH, WEST};
    let mut walls = Vec::new();
    let o = place.header.origin_m();
    let ext = place.header.extent_m();
    let seed = place.header.seed;
    for (side, &kind) in place.terrain.edges.iter().enumerate() {
        let len = if side == NORTH || side == SOUTH {
            ext[0]
        } else {
            ext[1]
        };
        let n = (len / EDGE_WALL_M) as usize + 1;
        let at = |a: f32| {
            let d = face_inside(seed, side, kind, a)?;
            Some(match side {
                NORTH => [o[0] + a, o[1] + d],
                SOUTH => [o[0] + a, o[1] + ext[1] - d],
                WEST => [o[0] + d, o[1] + a],
                _ => [o[0] + ext[0] - d, o[1] + a],
            })
        };
        let normal = match side {
            NORTH => [0.0, 1.0],
            SOUTH => [0.0, -1.0],
            WEST => [1.0, 0.0],
            _ => [-1.0, 0.0],
        };
        for k in 0..n {
            let (a0, a1) = (
                k as f32 * EDGE_WALL_M,
                ((k + 1) as f32 * EDGE_WALL_M).min(len),
            );
            if a1 <= a0 {
                continue;
            }
            let (Some(a), Some(b)) = (at(a0), at(a1)) else {
                continue;
            };
            let ground = |p: [f32; 2]| place.terrain.height_at(&place.header, p).unwrap_or(0.0);
            let base = ground(a).min(ground(b));
            walls.push(Wall {
                a,
                b,
                n: normal,
                y0: base - 5.0,
                y1: base + EDGE_WALL_HIGH_M,
            });
        }
    }
    walls
}

/// Length of the pieces of an edge wall's collision face, and how high it reaches.
const EDGE_WALL_M: f32 = 16.0;
const EDGE_WALL_HIGH_M: f32 = 400.0;

/// The parking lots' ground as patches on the terrain: islands, wheel stops and walks at
/// their heights above it, ramps from their walk down to the lot, then the asphalt at the
/// meshes' height above it. Their low walls go to
/// `walls` as boxes.
fn lot_patches(
    lots: &[crate::lots::ParkingLot],
    net: &Network,
    grid: &TerrainGrid,
    out: &mut Shapes,
    walls: &mut Vec<BoxSolid>,
) {
    use crate::lots::{ISLAND_H_M, LOT_LIFT_M, STOP_H_M, WALL_H_M};
    for lot in lots {
        for i in &lot.islands {
            out.patch_with(&i.q, Height::Terrain(ISLAND_H_M), Surface::Grass);
        }
        for s in &lot.stops {
            let half = (s[1] - s[0]).normalize().left() * 0.075;
            let q = [s[0] - half, s[0] + half, s[1] + half, s[1] - half];
            out.patch_with(&q, Height::Terrain(STOP_H_M), Surface::Concrete);
        }
        for q in &lot.walks {
            out.patch_with(q, Height::Terrain(CURB_M), Surface::Concrete);
        }
    }
    for lot in lots {
        for r in &lot.ramps {
            // From the walk's top at its back edge down to the lot: two planes, reaching
            // 1 cm over the walk so no sliver of open ground lies between them.
            let path = &net.paths[r.edge as usize];
            let top = |p: V2| [p.x, path.y_at(p) + CURB_M, p.y];
            let low = |p: V2| [p.x, grid.height_at(p.x, p.y) + LOT_LIFT_M, p.y];
            let mut q = r.q;
            q[0] = q[0] + (q[0] - q[3]).normalize() * 0.01;
            q[1] = q[1] + (q[1] - q[2]).normalize() * 0.01;
            let (a, b, c, d) = (top(q[0]), top(q[1]), low(q[2]), low(q[3]));
            out.patch_with(
                &[q[0], q[1], q[2]],
                Height::Plane(a, b, c),
                Surface::Concrete,
            );
            out.patch_with(
                &[q[0], q[2], q[3]],
                Height::Plane(a, c, d),
                Surface::Concrete,
            );
        }
        for q in &lot.pave {
            out.patch_with(q, Height::Terrain(LOT_LIFT_M), Surface::Asphalt);
        }
        for s in &lot.walls {
            let d = s[1] - s[0];
            let c = (s[0] + s[1]) * 0.5;
            let (ya, yb) = (
                grid.height_at(s[0].x, s[0].y),
                grid.height_at(s[1].x, s[1].y),
            );
            walls.push(BoxSolid {
                c: arr(c),
                u: arr(d.normalize()),
                half: [d.length() * 0.5, 0.13],
                y0: ya.min(yb) - 0.3,
                y1: ya.max(yb) + WALL_H_M,
                kind: PropKind::Fence {
                    len_dm: (d.length() * 10.0) as u16,
                },
                lot: NO_LOT,
            });
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
fn props_of(
    place: &Place,
    grammar: &Grammar,
    net: &Network,
    blocks: &[ground::Block],
) -> Vec<Prop> {
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
        let zone = crate::building_zone(place, b);
        let plan = buildings::plan_building(b, zone, &env, door_pick, grammar);
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
                    grammar.lots.stall_w,
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
/// `kits`: the landmark kits a [`PropKind::Landmark`] indexes.
fn prop_solids(props: &[Prop], kits: &[crate::landmarks::Kit]) -> (Vec<Post>, Vec<BoxSolid>) {
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
                lot: NO_LOT,
            }),
            PropKind::ParkedCar { .. } => boxes.push(BoxSolid {
                c: arr(p.pos),
                u: arr(p.dir),
                half: [2.25, 0.88],
                y0: p.y + 0.1,
                y1: p.y + 1.42,
                kind: p.kind,
                lot: NO_LOT,
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
                lot: NO_LOT,
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
                    lot: NO_LOT,
                })
            }
            // Above the sidewalk, out of the car's way; track lies on the ground (or the
            // road) and a car rolls over it.
            PropKind::NeonSign { .. } | PropKind::Track { .. } => {}
            PropKind::Crossbuck => posts.push(post(0.08, 3.9)),
            PropKind::FaceBillboard { .. } => {
                let along = p.dir.left();
                for x in [-3.2f32, 3.2] {
                    let c = p.pos + along * x - p.dir * 0.4;
                    posts.push(Post {
                        c: arr(c),
                        ..post(0.22, 9.2)
                    });
                }
            }
            // The posts and the ad box; a person may sit inside.
            PropKind::BusShelter { .. } => {
                let (len, depth, h) = props::SHELTER_M;
                let along = p.dir.left();
                for x in [-0.5 * len + 0.05, 0.5 * len - 0.05] {
                    for z in [-depth + 0.05, -0.05] {
                        posts.push(Post {
                            c: arr(p.pos + along * x + p.dir * z),
                            ..post(0.06, h)
                        });
                    }
                }
                boxes.push(BoxSolid {
                    c: arr(p.pos + along * (0.5 * len - 0.5) - p.dir * (0.5 * depth)),
                    u: arr(along),
                    half: [0.12, 0.6],
                    y0: p.y,
                    y1: p.y + 2.0,
                    kind: p.kind,
                    lot: NO_LOT,
                });
            }
            // The tree's post and the board's legs; paint lies flat on the road.
            PropKind::StartTree => posts.push(post(0.42, 3.25)),
            PropKind::TimingBoard => {
                let across = p.dir.left();
                for x in [-2.2f32, 2.2] {
                    posts.push(Post {
                        c: arr(p.pos + across * x),
                        ..post(0.12, 5.0)
                    });
                }
            }
            PropKind::StripLine { .. } => {}
            PropKind::CrossingGate { .. } => posts.push(post(0.15, 4.2)),
            PropKind::LotLabel { .. } => posts.push(post(0.12, 2.8)),
            PropKind::Landmark { kit, variant } => {
                let Some(k) = kits.get(usize::from(kit)) else {
                    continue;
                };
                let mut slabs = Vec::new();
                crate::landmarks::slabs(k, usize::from(variant), p.pos, p.y, p.dir, &mut slabs);
                boxes.extend(slabs.iter().map(|s| BoxSolid {
                    c: arr(s.c),
                    u: arr(s.u),
                    half: s.half,
                    y0: s.y0,
                    y1: s.y1,
                    kind: p.kind,
                    lot: NO_LOT,
                }));
            }
            PropKind::TunnelPiece { .. }
            | PropKind::GuardRail { .. }
            | PropKind::ChainLink { .. }
            | PropKind::CrewGarage { .. }
            | PropKind::TacoStand { .. }
            | PropKind::PicnicTable => {
                let mut slabs = Vec::new();
                crate::setpieces::slabs(p, &mut slabs);
                boxes.extend(slabs.iter().map(|s| BoxSolid {
                    c: arr(s.c),
                    u: arr(s.u),
                    half: s.half,
                    y0: s.y0,
                    y1: s.y1,
                    kind: p.kind,
                    lot: NO_LOT,
                }));
            }
        }
    }
    (posts, boxes)
}

/// Collision shapes of a terrain place's props, from its `Props` table: [`prop_solids`]
/// for all but the parked cars, which take their shape's size.
/// The lots' cars ([`LotCar`]) come with their boxes, none there yet.
fn place_prop_solids(
    place: &Place,
    kits: &[crate::landmarks::Kit],
) -> (Vec<Post>, Vec<BoxSolid>, Vec<LotCar>) {
    let seed = place.header.seed;
    let items = &place.props.items;
    let props: Vec<Prop> = items
        .iter()
        .filter(|p| !matches!(p.kind, place::PropKind::ParkedCar { .. }))
        .map(|p| props::from_place(p, seed))
        .collect();
    let (posts, mut boxes) = prop_solids(&props, kits);
    let mut lots = Vec::new();
    for p in items {
        if let place::PropKind::ParkedCar {
            vehicle_class,
            hours,
        } = p.kind
        {
            let w = props::from_place(p, seed);
            let [len, width, height] = props::parked_car_size(vehicle_class);
            let lot = if hours == ALL_HOURS {
                NO_LOT
            } else {
                lots.push(LotCar {
                    at: p.pos,
                    hours,
                    solid: boxes.len() as u32,
                    present: false,
                });
                lots.len() as u32 - 1
            };
            boxes.push(BoxSolid {
                c: arr(w.pos),
                u: arr(w.dir),
                half: [len * 0.5, width * 0.5],
                y0: w.y + 0.1,
                y1: w.y + height,
                kind: w.kind,
                lot,
            });
        }
    }
    (posts, boxes, lots)
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

/// One contact's answer: nudge the car out along `n`, then a plain impulse at the contact
/// point (stop the approach, a small rebound, light friction; off center it yaws the car),
/// a speed loss in proportion to the closing speed, and a pitch or roll kick toward what
/// was hit that the springs rock out. The hit, if the point was approaching.
fn respond(state: &mut VehicleState, spec: &VehicleSpec, c: &Contact) -> Option<Bump> {
    let n = [c.n[0], 0.0, c.n[1]];
    state.pos[0] += c.n[0] * c.depth;
    state.pos[2] += c.n[1] * c.depth;
    let y = state.pos[1].clamp(c.y0, c.y1.max(c.y0));
    let r = sub3([c.p[0], y, c.p[1]], state.pos);
    let v_p = add3(state.vel, cross3(state.ang_vel, r));
    let vn = dot3(v_p, n);
    if vn >= 0.0 {
        return None;
    }
    let closing = -vn;
    let rebound = (RESTITUTION * closing).min(REBOUND_MAX_MPS);
    let j = (closing + rebound) / sim::inv_mass_at(spec, state, r, n);
    let mut impulse = scale3(n, j);
    // Friction along the horizontal sliding direction, within the friction cone.
    let vt = [v_p[0] - n[0] * vn, 0.0, v_p[2] - n[2] * vn];
    let vt_len = sqrt(dot3(vt, vt));
    if vt_len > 1e-6 {
        let t = scale3(vt, 1.0 / vt_len);
        let jt = (vt_len / sim::inv_mass_at(spec, state, r, t)).min(FRICTION * j);
        impulse = add3(impulse, scale3(t, -jt));
    }
    sim::knock(spec, state, r, impulse);
    scrub_and_dip(state, spec, n, closing);
    Some(Bump {
        speed: closing,
        normal: n,
        at: rotate_inv(state.rot, r),
    })
}

/// After a hit closing at `closing` along `n`: the speed loss and the dip toward what was
/// hit, for walls, posts, boxes and other cars alike.
fn scrub_and_dip(state: &mut VehicleState, spec: &VehicleSpec, n: V3, closing: f32) {
    // The speed loss, across the normal only, so the nudge back survives it.
    let keep = 1.0 - SCRUB * (closing / SCRUB_SPEED).min(1.0);
    let along = dot3(state.vel, n);
    let lost = [
        (state.vel[0] - n[0] * along) * (keep - 1.0),
        0.0,
        (state.vel[2] - n[2] * along) * (keep - 1.0),
    ];
    sim::knock(spec, state, [0.0; 3], scale3(lost, spec.mass_kg));

    // The side that hit dips: a spin about the horizontal axis across the normal. It sits
    // outside the impulse model and adds a little energy, capped at ROCK_MAX (0.4 rad/s, a
    // dip of 1-2 degrees); the springs and dampers take it straight back out, and it is what
    // makes a hit read as a bump rather than a stop.
    let rock = (ROCK_PER_MPS * closing).min(ROCK_MAX);
    state.ang_vel = add3(state.ang_vel, scale3([-n[2], 0.0, n[0]], rock));
}

/// A hit on the car, for sound and tests.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Bump {
    /// Closing speed of the contact point, m/s.
    pub speed: f32,
    /// Unit world-space normal of what was hit, pointing at the car (up for a curb).
    pub normal: [f32; 3],
    /// Where on the car, body space (`+x` right, `-z` forward, origin at the center of
    /// mass), m.
    pub at: [f32; 3],
}

/// What [`resolve`] did this step, for sound, camera shake and tests.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Impact {
    /// The body touched a wall, post or box.
    pub contact: bool,
    /// The hardest of those contacts this step (speed 0 for resting contact).
    pub hit: Bump,
    /// A wheel hit a step above [`CURB_CLIMB_MPS`] and kicked the body...
    pub curb_kick: bool,
    /// ...the fastest such wheel: its speed, up, and its contact patch.
    pub curb: Bump,
    /// The parked car hit hardest this step, an index into [`WorldCollider::boxes`], and
    /// that hit.
    pub parked: Option<u32>,
    pub parked_hit: Bump,
}

/// Pushes the car out of walls, posts and boxes and answers each contact with a plain
/// impulse ([`RESTITUTION`], [`FRICTION`], [`SCRUB`], [`ROCK_PER_MPS`]), and kicks the body
/// up when a wheel hits a curb above [`CURB_CLIMB_MPS`]. Call after every `vehicle::step`
/// with the same collider that step probed.
///
/// The car's center never crosses a wall between steps: the move since the last step is
/// swept against walls first and clamped at the first one crossed.
pub fn resolve(state: &mut VehicleState, spec: &VehicleSpec, collider: &WorldCollider) -> Impact {
    let mut impact = Impact::default();
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
                    let mut on = |hit: Option<Bump>, parked: Option<u32>| {
                        touched = true;
                        let Some(hit) = hit else {
                            return;
                        };
                        if hit.speed > impact.hit.speed {
                            impact.hit = hit;
                        }
                        if parked.is_some() && hit.speed > impact.parked_hit.speed {
                            impact.parked = parked;
                            impact.parked_hit = hit;
                        }
                    };
                    resolve_cell(state, spec, collider, cell, moved, &mut on);
                }
            }
            if !touched {
                break;
            }
            impact.contact = true;
        }
    }
    curb_kick(state, spec, collider, &mut impact);
    state.speed_mps = dot3(state.vel, rotate(state.rot, [0.0, 0.0, -1.0]));
    impact
}

/// Parked cars [`Rocking`] keeps swaying at once.
pub const ROCK_SLOTS: usize = 4;

/// A parked car swaying on its springs: the center of its box, [`BoxSolid::c`] (also
/// where the Place's prop stands), and its sway.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct RockingCar {
    pub c: [f32; 2],
    pub rock: Rock,
    pub on: bool,
}

/// The parked cars the player has knocked, swaying until they settle. Nothing moves
/// along the ground: this is only how they are drawn.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Rocking {
    pub cars: [RockingCar; ROCK_SLOTS],
}

impl Rocking {
    /// After each [`resolve`]: the parked car `impact` hit, if any, is knocked away from
    /// the player's car. It takes the slot it already has, else a free one, else the
    /// stillest.
    pub fn note(&mut self, impact: &Impact, collider: &WorldCollider) {
        let Some(i) = impact.parked else {
            return;
        };
        let c = collider.boxes[i as usize].c;
        let mut slot = 0;
        let mut best = f32::MAX;
        for (k, car) in self.cars.iter().enumerate() {
            let energy = if car.on && car.c == c {
                -1.0
            } else if !car.on {
                0.0
            } else {
                let r = &car.rock;
                abs(r.tilt[0]) + abs(r.tilt[1]) + abs(r.rate[0]) + abs(r.rate[1])
            };
            if energy < best {
                best = energy;
                slot = k;
            }
        }
        let car = &mut self.cars[slot];
        if !(car.on && car.c == c) {
            *car = RockingCar {
                c,
                rock: Rock::default(),
                on: true,
            };
        }
        let hit = &impact.parked_hit;
        car.rock.knock([-hit.normal[0], -hit.normal[2]], hit.speed);
    }

    /// One sim step of every swaying car; a settled one frees its slot.
    pub fn step(&mut self) {
        for car in &mut self.cars {
            if car.on {
                car.rock.step();
                car.on = !car.rock.settled();
            }
        }
    }

    /// The tilt toward world `+x` and `+z`, rad, of the parked car standing at `c`, if it
    /// is swaying.
    pub fn tilt_at(&self, c: [f32; 2]) -> Option<[f32; 2]> {
        self.cars
            .iter()
            .find(|car| car.on && car.c == c)
            .map(|car| car.rock.tilt)
    }
}

/// A car the player's can hit that moves (an NPC car, `docs/VEHICLES.md` item 6): its body
/// box, the ground velocity `[x, z]` of the box's center, m/s, its yaw rate (rad/s, the
/// vehicle sim's convention: positive turns left, about `+y`) and its mass.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct OtherCar {
    pub body: BoxSolid,
    pub vel: [f32; 2],
    pub spin: f32,
    pub mass_kg: f32,
}

/// What [`hit_car`] did: the hit on the player's car, and the change it made to the other
/// car's ground velocity (at its box's center) and yaw rate.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct CarHit {
    pub bump: Bump,
    pub dv: [f32; 2],
    pub dspin: f32,
}

/// Restitution between two cars: they part at this share of the closing speed...
const CAR_RESTITUTION: f32 = 0.2;
/// ...the player's car at most this fast, m/s: a hard hit is a thud, not a bounce.
const CAR_REBOUND_MAX_MPS: f32 = 1.0;
/// The fastest a hit sends the other car away from the player's (after its own approach is
/// stopped), and along the contact, m/s, and the most it changes its yaw rate, rad/s: past
/// them the other car's tires take the rest, so a hit at 100 km/h shoves it, never launches
/// it, and a car coming on is stopped rather than driving on into the player's.
const SHOVE_MAX_MPS: f32 = 4.0;
const SPIN_MAX: f32 = 0.6;
/// Gap left between the two bodies after the push apart, m.
const CAR_SKIN_M: f32 = 0.002;

/// The player's car against another car that moves: the footprints are the parked-car
/// box test's, the answer the wall's with a second body. The player's car is pushed out
/// along the contact normal; then an impulse exchanges momentum by the two masses (and
/// inertias, at the contact point) with [`CAR_RESTITUTION`] and light friction along the
/// contact. The other car's share is capped at [`SHOVE_MAX_MPS`] and [`SPIN_MAX`]; the
/// player's car still leaves at the other car's new speed less a rebound, so it never
/// drives on into it. Then the speed loss and the dip a wall gives, and the vertical speed
/// goes in proportion to the ground speed lost: the normal is horizontal, and on a hill
/// the climb would otherwise carry on upward once the car is stopped. `None` when the boxes
/// do not touch; a touch that is not closing is a hit of speed 0 that changes nothing but
/// the push apart.
pub fn hit_car(state: &mut VehicleState, spec: &VehicleSpec, other: &OtherCar) -> Option<CarHit> {
    let fp = Footprint::new(state, spec);
    let c = box_contact(&fp, &other.body)?;
    let n = [c.n[0], 0.0, c.n[1]];
    state.pos[0] += c.n[0] * (c.depth + CAR_SKIN_M);
    state.pos[2] += c.n[1] * (c.depth + CAR_SKIN_M);
    // At the center of mass's height: bumpers meet bumpers, and on a hill the other car's
    // box stands higher or lower than this one's, which would tip it.
    let r = sub3([c.p[0], state.pos[1], c.p[1]], state.pos);
    let at = rotate_inv(state.rot, r);
    // The other car: a box on the ground, spinning about its center.
    let m2 = other.mass_kg;
    let [l2, w2] = [2.0 * other.body.half[0], 2.0 * other.body.half[1]];
    let i2 = m2 * (l2 * l2 + w2 * w2) / 12.0;
    let r2 = [c.p[0] - other.body.c[0], c.p[1] - other.body.c[1]];
    let point2 = |vel: [f32; 2], spin: f32| [vel[0] + spin * r2[1], vel[1] - spin * r2[0]];
    // Turning moment of a unit ground push `d` at the contact point, about the other car.
    let arm = |d: [f32; 2]| r2[1] * d[0] - r2[0] * d[1];
    let inv2 = |d: [f32; 2]| 1.0 / m2 + arm(d) * arm(d) / i2;

    let v1 = add3(state.vel, cross3(state.ang_vel, r));
    let v2 = point2(other.vel, other.spin);
    let rel = [v1[0] - v2[0], v1[2] - v2[1]];
    let vn = dot2(rel, c.n);
    if vn >= 0.0 {
        return Some(CarHit {
            bump: Bump {
                speed: 0.0,
                normal: n,
                at,
            },
            ..CarHit::default()
        });
    }
    let closing = -vn;
    let inv1 = sim::inv_mass_at(spec, state, r, n);
    let j = (1.0 + CAR_RESTITUTION) * closing / (inv1 + inv2(c.n));
    // Friction along the sliding direction, within the friction cone.
    let vt = [rel[0] - c.n[0] * vn, rel[1] - c.n[1] * vn];
    let vt_len = sqrt(dot2(vt, vt));
    let t = if vt_len > 1e-6 {
        [vt[0] / vt_len, vt[1] / vt_len]
    } else {
        [0.0, 0.0]
    };
    let jt = if vt_len > 1e-6 {
        let inv1t = sim::inv_mass_at(spec, state, r, [t[0], 0.0, t[1]]);
        (vt_len / (inv1t + inv2(t))).min(FRICTION * j)
    } else {
        0.0
    };
    // The other car's share: pushed away along the normal, dragged along the slide.
    let push = [-c.n[0] * j + t[0] * jt, -c.n[1] * j + t[1] * jt];
    // Its approach is taken away in full, then it leaves at most SHOVE_MAX_MPS, less the
    // speed it came on at (but the rebound): a car standing is shoved, one coming on stops.
    let away = -dot2(other.vel, c.n);
    let leave = (SHOVE_MAX_MPS + away.min(0.0)).max(CAR_REBOUND_MAX_MPS);
    let dv_n = (j / m2).min((leave - away).max(0.0));
    let dv_t = (jt / m2).min(SHOVE_MAX_MPS);
    let dv = [-c.n[0] * dv_n + t[0] * dv_t, -c.n[1] * dv_n + t[1] * dv_t];
    let dspin = (arm(push) / i2).clamp(-SPIN_MAX, SPIN_MAX);
    // The player's car leaves the other car's new speed at the contact by the rebound.
    let v2 = point2(
        [other.vel[0] + dv[0], other.vel[1] + dv[1]],
        other.spin + dspin,
    );
    let closing_now = -dot2([v1[0] - v2[0], v1[2] - v2[1]], c.n);
    let rebound = (CAR_RESTITUTION * closing).min(CAR_REBOUND_MAX_MPS);
    let jn = ((closing_now + rebound) / inv1).max(0.0);
    let impulse = [c.n[0] * jn - t[0] * jt, 0.0, c.n[1] * jn - t[1] * jt];
    let ground0 = sqrt(state.vel[0] * state.vel[0] + state.vel[2] * state.vel[2]);
    sim::knock(spec, state, r, impulse);
    scrub_and_dip(state, spec, n, closing);
    let ground1 = sqrt(state.vel[0] * state.vel[0] + state.vel[2] * state.vel[2]);
    if ground1 < ground0 {
        state.vel[1] *= ground1 / ground0;
    }
    state.speed_mps = dot3(state.vel, rotate(state.rot, [0.0, 0.0, -1.0]));
    Some(CarHit {
        bump: Bump {
            speed: closing,
            normal: n,
            at,
        },
        dv,
        dspin,
    })
}

fn bin_slice(bins: &Bins, cell: usize) -> &[u32] {
    &bins.items[bins.start[cell] as usize..bins.start[cell + 1] as usize]
}

/// One pass over the solids binned in `cell`: `on` hears of every contact, with the hit if
/// it was one and the box's index if it is a parked car.
fn resolve_cell(
    state: &mut VehicleState,
    spec: &VehicleSpec,
    collider: &WorldCollider,
    cell: usize,
    moved: V3,
    on: &mut impl FnMut(Option<Bump>, Option<u32>),
) {
    for &i in bin_slice(&collider.wall_bins, cell) {
        let fp = Footprint::new(state, spec);
        if let Some(c) = wall_contact(&fp, &collider.walls[i as usize]) {
            on(respond(state, spec, &c), None);
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
            on(respond(state, spec, &c), None);
        }
    }
    for &i in bin_slice(&collider.box_bins, cell) {
        let fp = Footprint::new(state, spec);
        let b = &collider.boxes[i as usize];
        if !collider.present(b) {
            continue;
        }
        if let Some(c) = box_contact(&fp, b) {
            let parked = matches!(b.kind, PropKind::ParkedCar { .. }).then_some(i);
            on(respond(state, spec, &c), parked);
        }
    }
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
/// upward, harder the faster and higher; the fastest is `impact.curb`.
fn curb_kick(
    state: &mut VehicleState,
    spec: &VehicleSpec,
    collider: &WorldCollider,
    impact: &mut Impact,
) {
    let wheels = spec.wheel_positions();
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
        let j = kick / sim::inv_mass_at(spec, state, r, up);
        sim::knock(spec, state, r, scale3(up, j));
        impact.curb_kick = true;
        if speed > impact.curb.speed {
            impact.curb = Bump {
                speed,
                normal: up,
                at: *mount,
            };
        }
    }
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
    copy::<Bump>();
    copy::<Impact>();
    copy::<Rocking>();
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
