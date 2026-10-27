//! Building meshes with facade geometry: plinth, inset windows with sills on a floor grid,
//! storefront bays with doors, awnings and sign bands, cornices and parapets, roof clutter.
//!
//! Each building is first *planned* ([`plan_building`]: floor heights, window grid per
//! facade, where the storefront and door go), then *built* from the plan. The plan is pure
//! data, so tests can check the mesh against it.
//!
//! A street front is three bands: a base (the ground floor, a run of bays between piers:
//! storefronts of bulkhead, display glass, transom and sign band; lobbies, garages, stair
//! doors, blank wall), a body (windows of one kind per building on the bay grid, so they
//! line up over the piers) and a top (parapet, cornice or stepped parapet). The grammar
//! says which, how big and how often.
//!
//! Styles with a look of their own: dingbats (an open carport on posts under the flats),
//! bungalows (a porch on tapered piers, low gables), ranches (wide windows, a brick
//! wainscot, low hips), 70s tract houses (two-tone with a belt), motels (a door and window
//! per room and an exterior walkway on the court side, the name over the parapet),
//! warehouses (roll-up doors, clerestory windows, pilasters), glass towers (a ribbon
//! curtain wall with mullions), barns (gambrel roof, braced doors; octagons are silos) and
//! detached garages (`Parking`). Brick, deco and stucco towers of
//! [`STEP_BACK_MIN_FLOORS`] floors or more step back above [`STEP_BACK_FLOORS`].
//!
//! A terrain place's detailing gives its buildings [`Signage`]: invented names lettered on
//! the sign bands (or a backlit sign box) and an awning of one of several kinds; some shops
//! are open late (the detailing puts a warm fill light in front of them). Without it (the
//! test town) a building keeps its original look.

use alloc::boxed::Box;
use alloc::vec::Vec;

use place::{Building, BuildingUse, Roof, StyleTag, ZoneId};
use skywave_core::{Rng, Seed};

use crate::grammar::{pick, Awning, Base, Grammar, GroundBays, SignRules, StyleRules};
use crate::math::{abs, clamp, floor, offset_polygon, V2, V3};
use crate::mesh::{
    boxf, face, frustum, lathe, polygon_cap, quad, rect, ring_band, tri, Frame, Glow, Paint,
    Pattern, WorldMesh,
};
use crate::palette::{mix, shade, Palette, Rgb};
use crate::props::{Prop, PropKind};
use crate::LOT_Y;

/// Depth of window insets.
pub const WINDOW_INSET_M: f32 = 0.15;
/// Depth of the storefront recess.
const STOREFRONT_INSET_M: f32 = 0.25;
/// Triangle ceiling per building (`docs/backlog/ART.md`): the budget of a building whose
/// street front has a base band. Window bays widen until the estimate fits.
pub const BUILDING_TRIANGLE_BUDGET: usize = 3500;
/// The planner's target for a building without a base band.
pub const PLAIN_TRIANGLE_TARGET: usize = 2000;
/// Base bays a street front may have at most.
pub const MAX_BAYS: usize = 12;
/// Rng key for the facade choices made at planning.
const KEY_FACADE: u64 = 0x4641_4344;
/// Share of windows lit (emissive), out of 256.
const LIT_WINDOWS_256: u32 = 102;
/// Estimated triangles per window (glass, reveals, sill, surrounding wall pieces).
const TRIS_PER_WINDOW: usize = 20;
/// Estimated triangles of everything but windows, the top and the rooftop units (counted
/// apart: [`RoofPlan`]); generous so the budget holds.
const TRIS_FIXED: usize = 420;
/// Rng key for per-building detail.
const KEY_BUILDING: u64 = 0x4255_494C;
/// Towers of at least [`STEP_BACK_MIN_FLOORS`] step back above this many floors.
pub const STEP_BACK_FLOORS: u8 = 6;
pub const STEP_BACK_MIN_FLOORS: u8 = 8;
/// Share of the plain target a stepped-back tower's base may use (a base band adds what it
/// costs).
const BASE_BUDGET: usize = 1300;

/// What is in front of one facade (footprint edge `i`, from vertex `i` to `i + 1`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FacadeEnv {
    /// Distance to the nearest road centerline straight ahead, meters (`f32::MAX` if none).
    pub road_dist: f32,
    /// Open ground between the facade and the property line of that road, meters.
    pub yard: f32,
    /// Another building stands right in front of it (a party wall): no openings.
    pub blocked: bool,
    /// The road ahead is the frontage edge of the building's parcel.
    pub frontage: bool,
}

/// The opening layout of one facade.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FacadePlan {
    pub len: f32,
    /// Window bays across the facade (0: blank wall).
    pub columns: u32,
    /// Floors with a row of windows (the base band's floor has none).
    pub window_rows: u32,
    /// The ground floor is a base band of bays (the street front only).
    pub base: Option<BasePlan>,
    /// The base band has a storefront bay.
    pub storefront: bool,
    /// Ground-floor bay replaced by the entrance door.
    pub door: Option<u32>,
    /// Ground-floor bay replaced by a garage door (houses).
    pub garage: Option<u32>,
    /// The ground floor is an open carport (dingbats): no openings on it.
    pub carport: bool,
    /// An exterior walkway runs along the upper floor, a door per bay (motels).
    pub walkway: bool,
}

impl FacadePlan {
    /// Window insets this facade gets.
    pub fn windows(&self) -> u32 {
        let openings = u32::from(self.door.is_some()) + u32::from(self.garage.is_some());
        let ground = if self.base.is_none() && !self.carport && self.window_rows > 0 {
            openings
        } else {
            0
        };
        self.columns * self.window_rows - ground
    }
}

/// What a base bay is.
#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BayKind {
    Storefront = 0,
    Lobby = 1,
    Garage = 2,
    Wall = 3,
    /// The door to the stairs of the flats above.
    Door = 4,
}

impl BayKind {
    /// In [`GroundBays`] field order.
    const ALL: [BayKind; 5] = [
        BayKind::Storefront,
        BayKind::Lobby,
        BayKind::Garage,
        BayKind::Wall,
        BayKind::Door,
    ];
}

/// The base band of a street front: bays between piers, pier centres evenly spaced.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BasePlan {
    /// Bays across the facade; the window columns above are a whole number per bay.
    pub bays: u32,
    pub kinds: [BayKind; MAX_BAYS],
    /// Where each storefront bay's entry is: 0 at the left, 1 centred, 2 at the right.
    pub entries: [u8; MAX_BAYS],
    pub pier_w: f32,
    pub bulkhead_h: f32,
    /// Top of the display glass above the base's floor.
    pub glass_top: f32,
    pub transom_h: f32,
    /// The transoms are glazed (else panelled over).
    pub transom_glass: bool,
    /// How far the storefront entries sit back; 0 for flush.
    pub recess: f32,
    /// Storefront entry `(width, height)`.
    pub entry: (f32, f32),
}

impl BasePlan {
    pub fn kinds(&self) -> &[BayKind] {
        &self.kinds[..self.bays as usize]
    }
}

/// The kind of window on a building's upper floors, in [`crate::grammar::WindowKinds`]
/// field order.
#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WindowKind {
    Punched = 0,
    Paired = 1,
    Ribbon = 2,
    Bay = 3,
    Picture = 4,
}

impl WindowKind {
    const ALL: [WindowKind; 5] = [
        WindowKind::Punched,
        WindowKind::Paired,
        WindowKind::Ribbon,
        WindowKind::Bay,
        WindowKind::Picture,
    ];
}

/// The kind of top on a flat roof, in [`crate::grammar::TopKinds`] field order.
#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TopKind {
    Parapet = 0,
    Cornice = 1,
    Stepped = 2,
    Shaped = 3,
    /// One storey only (the planner gives taller buildings the plain parapet).
    FalseFront = 4,
    Bracketed = 5,
    Mansard = 6,
}

impl TopKind {
    const ALL: [TopKind; 7] = [
        TopKind::Parapet,
        TopKind::Cornice,
        TopKind::Stepped,
        TopKind::Shaped,
        TopKind::FalseFront,
        TopKind::Bracketed,
        TopKind::Mansard,
    ];
}

/// What stands on a flat roof, counted at planning (from the grammar's rates and the roof's
/// area) so the triangle estimate covers it; where each goes is drawn when it is built.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RoofPlan {
    /// Vent hoods and plumbing vents.
    pub vents: u8,
    /// Packaged rooftop units.
    pub units: u8,
    pub fans: u8,
    pub skylights: u8,
}

/// Most of each kind of rooftop unit on one roof (the triangle ceiling holds with all).
const MAX_ROOF_UNITS: u8 = 6;
const MAX_FANS: u8 = 3;
const MAX_SKYLIGHTS: u8 = 8;

impl RoofPlan {
    /// Estimated triangles.
    fn tris(&self) -> usize {
        12 * usize::from(self.vents)
            + 40 * usize::from(self.units)
            + 34 * usize::from(self.fans)
            + 10 * usize::from(self.skylights)
    }
}

/// Everything needed to build a building, decided up front.
#[derive(Clone, Debug, PartialEq)]
pub struct BuildingPlan {
    /// Top of the plinth; walls with openings start here.
    pub plinth_top: f32,
    /// Roof surface, behind the parapet.
    pub roof_y: f32,
    /// Top of the parapet.
    pub top: f32,
    /// Bottom of each floor (`floors + 1` entries; the last is the roof).
    pub floor_y: Vec<f32>,
    pub facades: Vec<FacadePlan>,
    /// Index of the street-facing facade (storefront, door, signs).
    pub frontage: usize,
    pub window_w: f32,
    pub window_h: f32,
    pub door_w: f32,
    pub door_h: f32,
    pub window_kind: WindowKind,
    pub top_kind: TopKind,
    /// Rooftop units (none on pitched roofs and stepped-back bases).
    pub roof: RoofPlan,
    /// The part of a tall building stepped back above [`STEP_BACK_FLOORS`]; this plan is
    /// then the base.
    pub tower: Option<Box<TowerPlan>>,
    /// What the detailing adds to the street front; `None` keeps the plain look.
    pub signage: Option<Signage>,
    /// The grammar's rules for the building's style, which the builders read too.
    pub rules: StyleRules,
}

/// What a terrain place's detailing adds to a building's street front.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Signage {
    /// Seeds the invented names on its signs.
    pub name_seed: u64,
    /// The awning over each run of storefronts ([`Awning`] as `u8`; the plain look's is
    /// striped).
    pub awning: u8,
    /// A strip mall's canopy ([`Awning::Mansard`] or else a flat canopy, as `u8`).
    pub canopy: u8,
    /// Its signs are backlit boxes with dark letters rather than neon letters.
    pub lit_box: bool,
    /// The shop is open late: the detailing lights the walk in front of it at night (its
    /// glass is not emissive: a big pane at a lit window's brightness reads as a white
    /// wall).
    pub shop_lit: bool,
}

/// Rng key for signage.
const KEY_SIGNAGE: u64 = 0x5349_474E;

/// The signage of building `index` of a place with detail seed `seed`.
pub fn signage(seed: u64, index: u32, rules: &SignRules) -> Signage {
    let h = skywave_core::hash64(seed, &[KEY_SIGNAGE, u64::from(index)]);
    Signage {
        name_seed: h,
        awning: rules.awning(h >> 8) as u8,
        canopy: rules.mall_canopy(h >> 40) as u8,
        lit_box: (h >> 16) & 255 < u64::from(rules.lit_box_256),
        shop_lit: (h >> 24) & 255 < u64::from(rules.shop_lit_256),
    }
}

/// The stepped-back upper part of a tower: a building of its own (inset footprint, the
/// upper floors), planned from the ground and lifted onto the base's roof.
#[derive(Clone, Debug, PartialEq)]
pub struct TowerPlan {
    pub building: Building,
    pub plan: BuildingPlan,
    /// How far it is lifted: the base's roof above the lot.
    pub lift: f32,
}

impl BuildingPlan {
    pub fn floors(&self) -> usize {
        self.floor_y.len() - 1
    }

    pub fn windows(&self) -> u32 {
        let tower = self.tower.as_ref().map_or(0, |t| t.plan.windows());
        self.facades.iter().map(FacadePlan::windows).sum::<u32>() + tower
    }

    /// Sill height of the windows on floor `k`.
    fn sill(&self, k: usize) -> f32 {
        if k == 0 {
            self.plinth_top + self.rules.body.sill_ground
        } else {
            self.floor_y[k] + self.rules.body.sill_upper
        }
    }
}

/// What came out of building one building.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct BuildingStats {
    pub windows: u32,
    pub lit_windows: u32,
    pub triangles: usize,
}

/// Whether the building gets a pitched roof (rectangular footprints only).
pub fn is_pitched(b: &Building) -> bool {
    b.roof != Roof::Flat && b.footprint.len() == 4
}

fn parapet(b: &Building, rules: &StyleRules) -> f32 {
    if is_pitched(b) {
        return 0.0;
    }
    rules.top.parapet_h.of(b.usage)
}

/// Decides floors, the window grid of every facade, the street front's base band and door,
/// the window and top kinds, by the grammar's rules for the building's style in `zone`
/// (the zone under its footprint's centroid). `door_pick` chooses the door bay on houses
/// (any number; it is reduced modulo the bays) and seeds the other choices.
/// Brick, Deco and stucco towers of [`STEP_BACK_MIN_FLOORS`] or more step back above
/// [`STEP_BACK_FLOORS`]: the plan is the base's, with the tower's in
/// [`BuildingPlan::tower`].
pub fn plan_building(
    b: &Building,
    zone: ZoneId,
    env: &[FacadeEnv],
    door_pick: u32,
    grammar: &Grammar,
) -> BuildingPlan {
    let rules = grammar.style(b.style, zone);
    if let Some(split) = step_back(b, env, door_pick, rules) {
        return split;
    }
    plan_one(b, env, door_pick, PLAIN_TRIANGLE_TARGET, rules, true)
}

/// The base and tower of a building that steps back, if it does.
fn step_back(
    b: &Building,
    env: &[FacadeEnv],
    door_pick: u32,
    rules: &StyleRules,
) -> Option<BuildingPlan> {
    let stepped = matches!(
        b.style,
        StyleTag::Deco | StyleTag::Brick | StyleTag::StuccoBox
    );
    if !stepped || b.floors < STEP_BACK_MIN_FLOORS || b.footprint.len() != 4 {
        return None;
    }
    let fp: Vec<V2> = b.footprint.iter().map(|&p| V2::from_array(p)).collect();
    let n = fp.len();
    let short = (0..n)
        .map(|i| (fp[(i + 1) % n] - fp[i]).length())
        .fold(f32::MAX, f32::min);
    let inset = clamp(short * 0.12, 2.0, 4.0);
    if short - 2.0 * inset < 8.0 {
        return None;
    }
    let full = plan_one(b, env, door_pick, PLAIN_TRIANGLE_TARGET, rules, true);
    let k = usize::from(STEP_BACK_FLOORS);
    let base_roof = full.floor_y[k];
    let base_b = Building {
        height_m: base_roof + 1.0,
        floors: STEP_BACK_FLOORS,
        ..b.clone()
    };
    let mut base = plan_one(&base_b, env, door_pick, BASE_BUDGET, rules, true);
    // The tower stands where the base's rooftop units would.
    base.roof = RoofPlan::default();
    let tower_fp = offset_polygon(&fp, |_| -inset);
    let tower_b = Building {
        footprint: tower_fp.iter().map(|p| [p.x, p.y]).collect(),
        height_m: full.top - base.roof_y + LOT_Y,
        floors: b.floors - STEP_BACK_FLOORS,
        usage: BuildingUse::Office,
        ..b.clone()
    };
    let tower_env: Vec<FacadeEnv> = env
        .iter()
        .map(|e| FacadeEnv {
            road_dist: e.road_dist + inset,
            yard: e.yard + inset,
            blocked: false,
            frontage: e.frontage,
        })
        .collect();
    // Both estimates count the generous fixed share; one is enough for the pair.
    let used = estimate(&base_b, &base, &base.facades, base.floors());
    let band = base.facades.iter().map(base_tris).sum::<usize>();
    let total = (PLAIN_TRIANGLE_TARGET + band).min(BUILDING_TRIANGLE_BUDGET);
    let left = (total + TRIS_FIXED / 2).saturating_sub(used);
    let mut plan = plan_one(&tower_b, &tower_env, door_pick, left, rules, false);
    // The tower has no entrance of its own.
    for f in &mut plan.facades {
        f.door = None;
        f.storefront = false;
        f.window_rows = if f.columns > 0 {
            plan.floor_y.len() as u32 - 1
        } else {
            0
        };
    }
    base.tower = Some(Box::new(TowerPlan {
        building: tower_b,
        plan,
        lift: base.roof_y - LOT_Y,
    }));
    Some(base)
}

/// Triangles per window inset by style and kind.
fn tris_per_window(b: &Building, kind: WindowKind) -> usize {
    match (b.style, kind) {
        (StyleTag::Glass | StyleTag::Warehouse, _) => 8,
        (StyleTag::Motel, _) => 34,
        (_, WindowKind::Paired) => TRIS_PER_WINDOW + 2,
        (_, WindowKind::Bay) => 34,
        _ => TRIS_PER_WINDOW,
    }
}

/// Triangles of a base band: piers, the belt course, and each bay (a storefront's sign
/// lettering and striped awning are most of it).
fn base_tris(f: &FacadePlan) -> usize {
    let Some(bp) = &f.base else {
        return 0;
    };
    let span = f.len / bp.bays as f32;
    let kinds = bp.kinds();
    // A lettered sign over each run of storefronts, up to ~210.
    let runs = (0..kinds.len())
        .filter(|&k| {
            kinds[k] == BayKind::Storefront && (k == 0 || kinds[k - 1] != BayKind::Storefront)
        })
        .count();
    let bays: usize = kinds
        .iter()
        .map(|k| match k {
            // Glass, frames and entry ~60; a striped awning ~14 a meter.
            BayKind::Storefront => 70 + (span * 14.0) as usize,
            BayKind::Lobby => 40,
            BayKind::Garage => 30,
            BayKind::Wall => 4,
            BayKind::Door => 36,
        })
        .sum();
    bays + 210 * runs + 8 * (bp.bays as usize + 1) + 16
}

/// The triangle estimate of a plan.
fn estimate(b: &Building, plan: &BuildingPlan, fs: &[FacadePlan], floors: usize) -> usize {
    estimate_parts(b, plan.window_kind, plan.top_kind, plan.roof, fs, floors)
}

fn estimate_parts(
    b: &Building,
    window_kind: WindowKind,
    top_kind: TopKind,
    roof: RoofPlan,
    fs: &[FacadePlan],
    floors: usize,
) -> usize {
    let per_floor = if b.style == StyleTag::Brick { 64 } else { 40 };
    let top = match top_kind {
        TopKind::Parapet => 0,
        TopKind::Cornice => 12 * b.footprint.len(),
        TopKind::Stepped => 36,
        TopKind::Shaped => 12 * SHAPED_SEGMENTS + 16,
        TopKind::FalseFront => 30,
        TopKind::Bracketed => 24 + 10 * MAX_BRACKETS,
        TopKind::Mansard => 10 * b.footprint.len(),
    };
    let per_window = tris_per_window(b, window_kind);
    TRIS_FIXED
        + top
        + roof.tris()
        + floors * per_floor
        + fs.iter()
            .map(|f| f.windows() as usize * per_window + 24 + base_tris(f))
            .sum::<usize>()
}

/// Rng key for the rooftop counts.
const KEY_ROOF: u64 = 0x524F_4F46;

/// How many of each rooftop unit a flat roof gets: the grammar's rate per 1000 m² times the
/// footprint's area, rounded at random (so the counts average the rate), at least one of
/// a kind whose rate is above zero, at most the per-kind cap.
fn plan_roof(b: &Building, rules: &StyleRules, door_pick: u32) -> RoofPlan {
    let fp = &b.footprint;
    let n = fp.len();
    let mut twice = 0.0;
    for i in 0..n {
        let (p, q) = (fp[i], fp[(i + 1) % n]);
        twice += p[0] * q[1] - q[0] * p[1];
    }
    let area = abs(twice) * 0.5;
    let mut rng = Rng::new(Seed(u64::from(door_pick)), &[KEY_ROOF]);
    let a = &rules.addenda;
    let mut count = |(lo, hi): (f32, f32), max: u8| -> u8 {
        if hi <= 0.0 {
            return 0;
        }
        let rate = rng.range_f32(lo, hi);
        let n = floor(rate * area / 1000.0 + rng.range_f32(0.0, 1.0));
        (n as u8).clamp(1, max)
    };
    let units = count(a.roof_units.of(b.usage), MAX_ROOF_UNITS);
    let fans = count(a.exhaust_fans.of(b.usage), MAX_FANS);
    let skylights = count(a.skylights.of(b.usage), MAX_SKYLIGHTS);
    let (lo, hi) = a.roof_vents.of(b.usage);
    let vents = if hi == 0 {
        0
    } else {
        rng.range(u32::from(lo), u32::from(hi) + 1) as u8
    };
    RoofPlan {
        vents,
        units,
        fans,
        skylights,
    }
}

/// The base band of a street front `len` long, from the grammar's `base` with `ground`
/// the bay weights for the building's use.
fn plan_base(base: &Base, ground: GroundBays, len: f32, floors: usize, rng: &mut Rng) -> BasePlan {
    let draw = |rng: &mut Rng, (lo, hi): (f32, f32)| rng.range_f32(lo, hi);
    let width = draw(rng, base.front_bay_w);
    let bays = ((len / width + 0.5) as u32).clamp(1, MAX_BAYS as u32);
    let s = &base.storefront;
    let weights = [
        ground.storefront,
        ground.lobby,
        ground.garage,
        ground.wall,
        ground.door,
    ];
    let mut kinds = [BayKind::Wall; MAX_BAYS];
    let mut entries = [1; MAX_BAYS];
    for k in 0..bays as usize {
        kinds[k] = BayKind::ALL[pick(&weights, rng.next_u32()).unwrap_or(3)];
        if rng.range(0, 256) < s.side_entry_256 {
            entries[k] = if rng.range(0, 2) == 0 { 0 } else { 2 };
        }
    }
    // One lobby at most, in the middle; one stair door at most, and none without flats
    // over the shops. What they displace becomes a storefront (or wall).
    let filler = if ground.storefront > 0 {
        BayKind::Storefront
    } else {
        BayKind::Wall
    };
    let mid = bays as usize / 2;
    let (mut lobby, mut door) = (None, false);
    for (k, kind) in kinds[..bays as usize].iter_mut().enumerate() {
        match *kind {
            BayKind::Lobby if lobby.is_some() => *kind = filler,
            BayKind::Lobby => lobby = Some(k),
            BayKind::Door if door || floors < 2 => *kind = filler,
            BayKind::Door => door = true,
            _ => {}
        }
    }
    if let Some(k) = lobby {
        kinds.swap(k, mid);
    }
    // A front with no way in gets one in the middle: the likeliest entrance.
    let entrance = |k: &BayKind| *k != BayKind::Wall;
    if !kinds[..bays as usize].iter().any(entrance) {
        let order = [
            (ground.lobby, BayKind::Lobby),
            (ground.storefront, BayKind::Storefront),
            (ground.garage, BayKind::Garage),
            (if floors > 1 { ground.door } else { 0 }, BayKind::Door),
        ];
        // The heaviest weight; the first listed on a tie.
        let best = order.iter().rev().filter(|o| o.0 > 0).max_by_key(|o| o.0);
        if let Some(&(_, k)) = best {
            kinds[mid] = k;
        }
    }
    let recessed = rng.range(0, 256) < s.recessed_256;
    BasePlan {
        bays,
        kinds,
        entries,
        pier_w: draw(rng, base.pier_w),
        bulkhead_h: draw(rng, s.bulkhead_h),
        glass_top: draw(rng, s.glass_top),
        transom_h: draw(rng, s.transom_h),
        transom_glass: rng.range(0, 256) < s.transom_glass_256,
        recess: if recessed { draw(rng, s.recess) } else { 0.0 },
        entry: s.entry,
    }
}

/// Whether the outward side of footprint edge `i` faces another part of the same
/// footprint within 40 m (the court of an L or U).
fn faces_court(fp: &[V2], i: usize) -> bool {
    let n = fp.len();
    let (a, b) = (fp[i], fp[(i + 1) % n]);
    let mid = (a + b) * 0.5;
    let out = (b - a).normalize().right();
    let far = mid + out * 40.0;
    (0..n).any(|j| {
        if j == i {
            return false;
        }
        let (c, d) = (fp[j], fp[(j + 1) % n]);
        crate::math::intersect_lines(mid, far - mid, c, d - c).is_some_and(|x| {
            let along = (x - mid).dot(out);
            let on = (x - c).dot(d - c);
            along > 0.5 && along < 40.0 && on > 0.0 && on < (d - c).dot(d - c)
        })
    })
}

/// [`plan_building`] for one block of walls within `budget` triangles (plus what a base
/// band costs, if `banded` lets its street front have one; a stepped-back tower's does not).
fn plan_one(
    b: &Building,
    env: &[FacadeEnv],
    door_pick: u32,
    budget: usize,
    rules: &StyleRules,
    banded: bool,
) -> BuildingPlan {
    let fp = &b.footprint;
    let n = fp.len();
    let floors = usize::from(b.floors.clamp(1, 40));
    let residential = b.usage == BuildingUse::Residential;
    let mass = &rules.massing;
    let even = mass.even_floors.of(b.usage);
    let top = b.height_m;
    let roof_y = top - parapet(b, rules).min(top * 0.3);
    let plinth_top = LOT_Y + rules.base.plinth_h.of(b.usage);
    let span = roof_y - LOT_Y;
    let (ground_h, upper_h) = if even {
        (span / floors as f32, span / floors as f32)
    } else if floors == 1 {
        (span, span)
    } else {
        let (lo, hi) = mass.ground_floor_h;
        let g = clamp(span - (floors - 1) as f32 * mass.floor_h, lo, hi)
            .min(span * mass.ground_share_max);
        (g, (span - g) / (floors - 1) as f32)
    };
    let mut floor_y = Vec::with_capacity(floors + 1);
    floor_y.push(LOT_Y);
    for k in 1..=floors {
        floor_y.push(LOT_Y + ground_h + (k - 1) as f32 * upper_h);
    }
    floor_y[floors] = roof_y;

    let bay = rules.base.bay_w.of(b.usage);
    let win = rules.body.window.of(b.usage);
    let window_w = clamp(bay * win.w_share, win.w.0, win.w.1);
    let window_h = (upper_h - win.head).min(win.h_max);
    let (door_w, door_h) = rules.base.door.of(b.usage);
    let mut rng = Rng::new(Seed(u64::from(door_pick)), &[KEY_FACADE]);
    let window_kind =
        WindowKind::ALL[pick(&rules.body.kinds.of(b.usage).weights(), rng.next_u32()).unwrap_or(0)];
    let top_kind = match TopKind::ALL[pick(&rules.top.kinds.weights(), rng.next_u32()).unwrap_or(0)]
    {
        // A false front is a one-storey thing; on one storey the shop signs reach the top
        // of the wall, where brackets and a mansard's eave would be.
        TopKind::FalseFront if floors > 1 => TopKind::Parapet,
        TopKind::Bracketed if floors == 1 => TopKind::Cornice,
        TopKind::Mansard if floors == 1 => TopKind::Parapet,
        k => k,
    };
    let roof = if is_pitched(b) {
        RoofPlan::default()
    } else {
        plan_roof(b, rules, door_pick)
    };

    let len = |i: usize| (V2::from_array(fp[(i + 1) % n]) - V2::from_array(fp[i])).length();
    // The frontage: the open facade facing the parcel's frontage road, else the one closest
    // to any road (longest wins ties).
    let mut frontage = 0;
    for i in 0..n {
        let (e, f) = (env[i], env[frontage]);
        let closer = e.road_dist + 0.5 < f.road_dist;
        let tie = abs(e.road_dist - f.road_dist) <= 0.5 && len(i) > len(frontage);
        let better = (e.frontage && !f.frontage) || (e.frontage == f.frontage && (closer || tie));
        if !e.blocked && (f.blocked || better) {
            frontage = i;
        }
    }
    let blank =
        matches!(b.style, StyleTag::StripMall | StyleTag::Barn) || b.usage == BuildingUse::Parking;
    // Styles that draw their own ground floor (carports, roll-up doors, curtain walls,
    // motel rooms) never get a base band.
    let own_openings = matches!(
        b.style,
        StyleTag::Dingbat | StyleTag::Warehouse | StyleTag::Glass | StyleTag::Motel
    );
    let ground = rules.base.ground.of(b.usage);
    let base = (banded
        && !blank
        && !own_openings
        && ground.total() > 0
        && !env[frontage].blocked
        && len(frontage) >= 4.0)
        .then(|| plan_base(&rules.base, ground, len(frontage), floors, &mut rng));
    let fpv: Vec<V2> = fp.iter().map(|&p| V2::from_array(p)).collect();
    let courts: Vec<bool> = (0..n)
        .map(|i| b.style == StyleTag::Motel && faces_court(&fpv, i))
        .collect();
    let any_court = courts.iter().any(|&c| c);
    let columns_for = |i: usize, bay: f32| -> u32 {
        let l = len(i);
        if env[i].blocked || l < 2.4 || blank {
            0
        } else if let (true, Some(bp)) = (i == frontage, &base) {
            // Whole window bays per base bay, so the windows line up over the piers.
            bp.bays * (floor(l / bp.bays as f32 / bay) as u32).max(1)
        } else {
            (floor((l - 0.8) / bay) as u32).max(1)
        }
    };
    let make = |bays: &[f32]| -> Vec<FacadePlan> {
        (0..n)
            .map(|i| {
                let columns = columns_for(i, bays[i]);
                let base = if i == frontage { base } else { None };
                let carport = b.style == StyleTag::Dingbat && floors > 1;
                let window_rows = match (columns, b.style) {
                    (0, _) => 0,
                    (_, StyleTag::Warehouse) => 1,
                    _ => floors as u32 - u32::from(base.is_some()) - u32::from(carport),
                };
                let front = i == frontage && base.is_none() && columns > 0 && !own_openings;
                // Houses with room for it get an attached garage at one end of the front.
                let garage =
                    if front && residential && columns >= 2 && len(i) / columns as f32 >= 3.3 {
                        Some(if door_pick & 1 == 0 { 0 } else { columns - 1 })
                    } else {
                        None
                    };
                let door = match (front, garage) {
                    (false, _) => None,
                    (true, Some(g)) => {
                        let k = (door_pick >> 1) % (columns - 1);
                        Some(if g == 0 { k + 1 } else { k })
                    }
                    (true, None) => Some(if residential {
                        door_pick % columns
                    } else {
                        columns / 2
                    }),
                };
                let walkway = b.style == StyleTag::Motel
                    && columns > 0
                    && floors > 1
                    && (courts[i] || (!any_court && i == frontage));
                let storefront = base.is_some_and(|bp| bp.kinds().contains(&BayKind::Storefront));
                FacadePlan {
                    len: len(i),
                    columns,
                    window_rows,
                    base,
                    storefront,
                    door,
                    garage,
                    carport,
                    walkway,
                }
            })
            .collect()
    };
    // Facades nobody sees from a street get sparser windows; then bays widen (side and
    // back facades first) until the triangle estimate fits the budget.
    let mut bays: Vec<f32> = (0..n)
        .map(|i| {
            if env[i].road_dist > 30.0 {
                bay * 1.5
            } else {
                bay
            }
        })
        .collect();
    let est = |fs: &[FacadePlan]| estimate_parts(b, window_kind, top_kind, roof, fs, floors);
    let mut facades = make(&bays);
    // A base band raises the budget by what it costs, up to the ceiling: the windows get
    // no more than they would without it.
    let band = base_tris(&facades[frontage]);
    let budget = (budget + band).min(budget + BUILDING_TRIANGLE_BUDGET - PLAIN_TRIANGLE_TARGET);
    let mut round = 0;
    while est(&facades) > budget && round < 40 {
        for (i, b) in bays.iter_mut().enumerate() {
            if round < 6 && env[i].road_dist < 25.0 {
                continue;
            }
            *b *= 1.1;
        }
        facades = make(&bays);
        round += 1;
    }
    // Still over (tall towers on many-sided lots): blank the facades least seen, farthest
    // from a road and shortest first, never the frontage.
    if est(&facades) > budget {
        let mut order: Vec<usize> = (0..n).filter(|&i| i != frontage).collect();
        order.sort_by(|&a, &c| {
            let key = |i: usize| (env[i].road_dist.min(1e6), -len(i));
            let (ka, kc) = (key(a), key(c));
            kc.0.total_cmp(&ka.0).then(kc.1.total_cmp(&ka.1))
        });
        for i in order {
            if est(&facades) <= budget {
                break;
            }
            let f = &mut facades[i];
            f.columns = 0;
            f.window_rows = 0;
            f.door = None;
            f.garage = None;
            f.walkway = false;
        }
    }
    // The base band itself never blanks; the frontage keeps it whatever its windows do.
    BuildingPlan {
        plinth_top,
        roof_y,
        top,
        floor_y,
        facades,
        frontage,
        window_w,
        window_h: window_h.max(rules.body.window_h_min),
        door_w,
        door_h,
        window_kind,
        top_kind,
        roof,
        tower: None,
        signage: None,
        rules: *rules,
    }
}

/// Colors of one building.
#[derive(Clone, Copy)]
struct Look {
    roof_tile: Paint,
    shop_glass: Paint,
    wall: Paint,
    reveal: Paint,
    trim: Paint,
    plinth: Paint,
    roof: Paint,
    parapet_in: Paint,
    glass: Paint,
    lit: Paint,
    sill: Paint,
    door: Paint,
    frame: Paint,
    awning: [Paint; 2],
    sign: Paint,
    neon: Rgb,
    sky: Rgb,
    /// Upper-floor siding of two-tone houses.
    upper: Paint,
    /// The building's signage, if the detailing gave it any.
    signage: Option<Signage>,
}

fn choose<T: Copy>(rng: &mut Rng, items: &[T]) -> T {
    items[rng.range(0, items.len() as u32) as usize]
}

fn look(b: &Building, pal: &Palette, rng: &mut Rng) -> Look {
    let (wall_rgb, pattern, trim_rgb) = match b.style {
        StyleTag::Craftsman => (
            choose(
                rng,
                &[
                    shade(pal.bark, 330),
                    mix(pal.lawn_dry, pal.stucco_cream, 110),
                    shade(pal.leaf_green, 300),
                    pal.sand,
                    mix(pal.brick_brown, pal.stucco_cream, 120),
                ],
            ),
            Pattern::Plain,
            choose(rng, &[pal.stucco_white, pal.stucco_cream]),
        ),
        StyleTag::Ranch | StyleTag::Tract70s => (
            choose(
                rng,
                &[
                    pal.stucco_white,
                    pal.stucco_cream,
                    pal.sand,
                    pal.peach,
                    mix(pal.lawn_dry, pal.stucco_white, 150),
                ],
            ),
            Pattern::Stucco,
            choose(
                rng,
                &[shade(pal.bark, 260), pal.brick_brown, pal.stucco_white],
            ),
        ),
        StyleTag::Warehouse => (
            choose(
                rng,
                &[pal.concrete, pal.sand, pal.stucco_white, pal.stucco_cream],
            ),
            Pattern::Concrete,
            choose(
                rng,
                &[pal.teal, pal.navy, pal.terracotta, pal.concrete_dark],
            ),
        ),
        StyleTag::Barn => (
            choose(
                rng,
                &[pal.brick_red, shade(pal.brick_red, 230), pal.terracotta],
            ),
            Pattern::Plain,
            pal.stucco_white,
        ),
        StyleTag::Glass => (
            choose(
                rng,
                &[pal.metal_dark, shade(pal.bark, 200), pal.navy, pal.concrete],
            ),
            Pattern::Metal,
            choose(rng, &[pal.metal_light, pal.concrete, pal.stucco_white]),
        ),
        StyleTag::StuccoBox | StyleTag::Dingbat | StyleTag::Motel => {
            let w = choose(rng, &pal.stucco_walls());
            (
                w,
                Pattern::Stucco,
                choose(
                    rng,
                    &[pal.stucco_white, pal.teal, pal.terracotta, pal.stucco_cream],
                ),
            )
        }
        StyleTag::Brick => (
            choose(rng, &[pal.brick_red, pal.brick_brown]),
            Pattern::Brick,
            choose(rng, &[pal.stucco_cream, pal.concrete]),
        ),
        StyleTag::Deco => (
            choose(
                rng,
                &[
                    pal.stucco_white,
                    pal.pastel_pink,
                    pal.mint,
                    pal.peach,
                    pal.sky_blue,
                    pal.stucco_cream,
                ],
            ),
            Pattern::Stucco,
            choose(
                rng,
                &[pal.teal, pal.salmon, pal.lavender, pal.navy, pal.terracotta],
            ),
        ),
        StyleTag::StripMall => (
            choose(rng, &[pal.sand, pal.stucco_cream, pal.peach]),
            Pattern::Stucco,
            choose(rng, &[pal.teal, pal.terracotta]),
        ),
    };
    let awning = choose(rng, &pal.awnings());
    let neon = choose(rng, &pal.neon());
    let glass = Paint::new(pal.glass, Pattern::Glass);
    let roof_rgb = choose(
        rng,
        &[
            pal.terracotta,
            pal.terracotta,
            shade(pal.terracotta, 210),
            pal.concrete_dark,
            shade(pal.bark, 300),
        ],
    );
    Look {
        roof_tile: Paint::new(roof_rgb, Pattern::RoofTile),
        shop_glass: Paint::new(mix(pal.glass, pal.sky_blue, 130), Pattern::Glass),
        wall: Paint::new(wall_rgb, pattern),
        reveal: Paint::new(shade(wall_rgb, 200), pattern),
        trim: Paint::new(trim_rgb, Pattern::Plain),
        plinth: Paint::new(
            mix(shade(wall_rgb, 170), pal.concrete_dark, 128),
            Pattern::Concrete,
        ),
        roof: Paint::new(pal.roof_gravel, Pattern::RoofGravel),
        parapet_in: Paint::new(shade(wall_rgb, 190), pattern),
        glass,
        lit: glass.glowing(pal.window_warm, Glow::Window),
        sill: Paint::new(
            if b.style == StyleTag::Brick {
                pal.stucco_cream
            } else {
                mix(wall_rgb, pal.stucco_white, 160)
            },
            Pattern::Concrete,
        ),
        door: Paint::new(
            choose(
                rng,
                &[
                    pal.teal,
                    pal.terracotta,
                    pal.navy,
                    pal.bark,
                    pal.stucco_white,
                ],
            ),
            Pattern::Plain,
        ),
        frame: Paint::new(pal.metal_dark, Pattern::Metal),
        awning: [
            Paint::new(awning, Pattern::Awning),
            Paint::new(pal.stucco_white, Pattern::Awning),
        ],
        sign: Paint::new(
            shade(
                choose(rng, &[pal.navy, pal.metal_dark, pal.terracotta, pal.teal]),
                200,
            ),
            Pattern::Plain,
        ),
        neon,
        sky: pal.sky_blue,
        upper: Paint::new(wall_rgb, pattern),
        signage: None,
    }
    .styled(b, pal, rng)
}

impl Look {
    /// The touches of the styles that have their own (drawn after the shared colors so
    /// the older styles keep theirs).
    fn styled(mut self, b: &Building, pal: &Palette, rng: &mut Rng) -> Look {
        match b.style {
            StyleTag::Craftsman | StyleTag::Ranch | StyleTag::Tract70s => {
                let roof = choose(
                    rng,
                    &[
                        pal.concrete_dark,
                        shade(pal.bark, 250),
                        shade(pal.metal_dark, 330),
                        shade(pal.terracotta, 180),
                    ],
                );
                self.roof_tile = Paint::new(roof, Pattern::RoofTile);
                let up = choose(
                    rng,
                    &[
                        shade(pal.bark, 320),
                        pal.sand,
                        shade(pal.lawn_dry, 280),
                        pal.stucco_white,
                    ],
                );
                self.upper = Paint::new(up, Pattern::Plain);
            }
            StyleTag::Barn => {
                let roof = choose(
                    rng,
                    &[pal.metal_light, pal.concrete_dark, shade(pal.bark, 220)],
                );
                self.roof_tile = Paint::new(roof, Pattern::Metal);
                self.door = Paint::new(shade(self.wall.rgb(), 200), Pattern::Plain);
                self.trim = Paint::new(pal.stucco_white, Pattern::Plain);
            }
            StyleTag::Glass => {
                let g = choose(
                    rng,
                    &[
                        pal.glass,
                        mix(pal.glass, pal.bark, 120),
                        mix(pal.glass, pal.teal, 90),
                        mix(pal.glass, pal.sky_blue, 110),
                    ],
                );
                self.glass = Paint::new(g, Pattern::Glass);
                self.lit = self.glass.glowing(pal.window_warm, Glow::Window);
            }
            _ => {}
        }
        self
    }
}

/// Builds `b` from `plan` into `m`. `index` keys the building's random detail.
pub fn build(
    m: &mut WorldMesh,
    b: &Building,
    plan: &BuildingPlan,
    pal: &Palette,
    seed: u64,
    index: u32,
) -> BuildingStats {
    build_on(m, b, plan, pal, seed, index, 0.0)
}

/// Where a building stands on uneven ground (`height_at(x, z)`): the ground height its
/// floor is built at and how far the ground under it falls below that. Uphill it sinks
/// in by up to 1 m; downhill its plinth runs down to the ground as a foundation wall.
pub fn footing(fp: &[[f32; 2]], height_at: impl Fn(f32, f32) -> f32) -> (f32, f32) {
    let (mut lo, mut hi) = (f32::MAX, f32::MIN);
    let (mut cx, mut cz) = (0.0, 0.0);
    for p in fp {
        let h = height_at(p[0], p[1]);
        lo = lo.min(h);
        hi = hi.max(h);
        cx += p[0] / fp.len() as f32;
        cz += p[1] / fp.len() as f32;
    }
    let h = height_at(cx, cz);
    lo = lo.min(h);
    hi = hi.max(h);
    let base = hi - ((hi - lo) * 0.5).min(1.0);
    (base, base - lo)
}

/// [`build`] for a building whose ground falls up to `drop` below its floor: the plinth
/// reaches down that far.
#[allow(clippy::too_many_arguments)]
pub fn build_on(
    m: &mut WorldMesh,
    b: &Building,
    plan: &BuildingPlan,
    pal: &Palette,
    seed: u64,
    index: u32,
    drop: f32,
) -> BuildingStats {
    let start = m.indices.len();
    let mut rng = Rng::new(Seed(seed), &[KEY_BUILDING, u64::from(index)]);
    let mut look = look(b, pal, &mut rng);
    look.signage = plan.signage;
    let fp: Vec<V2> = b.footprint.iter().map(|&p| V2::from_array(p)).collect();
    let n = fp.len();
    let mut stats = BuildingStats::default();
    if b.style == StyleTag::Barn && n == 8 {
        silo(m, b, &fp, &look, pal);
        stats.triangles = (m.indices.len() - start) / 3;
        return stats;
    }

    // Plinth from below the lot surface.
    ring_band(
        m,
        &fp,
        0.06,
        LOT_Y - 0.1 - drop,
        plan.plinth_top,
        look.plinth,
        Some(look.plinth),
        None,
    );

    for i in 0..n {
        let (a, bb) = (fp[i], fp[(i + 1) % n]);
        let dir = (bb - a).normalize();
        let f = Frame::upright(a.at(0.0), dir);
        let fpl = &plan.facades[i];
        let front = i == plan.frontage;
        match b.style {
            StyleTag::Glass if fpl.columns > 0 => {
                curtain_facade(m, &f, plan, fpl, front, &look, &mut rng, &mut stats)
            }
            StyleTag::Warehouse => {
                warehouse_facade(m, &f, plan, fpl, front, &look, &mut rng, &mut stats)
            }
            StyleTag::Motel if fpl.walkway => {
                motel_facade(m, &f, plan, fpl, &look, &mut rng, &mut stats)
            }
            _ => facade(m, &f, plan, fpl, front, b, &look, pal, &mut rng, &mut stats),
        }
    }

    if is_pitched(b) {
        if b.style == StyleTag::Barn {
            gambrel_roof(m, &fp, plan.top, &look);
        } else {
            pitched_roof(m, b, &fp, plan.top, &look, pal, &mut rng);
        }
        house_details(m, b, plan, &fp, &look);
        stats.triangles = (m.indices.len() - start) / 3;
        return stats;
    }

    // Parapet: coping band outside, inner walls, cap, roof.
    let inner = offset_polygon(&fp, |_| -0.25);
    ring_band(
        m,
        &fp,
        0.08,
        plan.top - 0.14,
        plan.top,
        look.trim,
        None,
        Some(look.trim),
    );
    let outer = offset_polygon(&fp, |_| 0.08);
    for i in 0..n {
        let j = (i + 1) % n;
        quad(
            m,
            [
                inner[i].at(plan.top),
                inner[j].at(plan.top),
                outer[j].at(plan.top),
                outer[i].at(plan.top),
            ],
            V3::Y,
            look.trim,
        );
        let into = -(inner[j] - inner[i]).right();
        quad(
            m,
            [
                inner[i].at(plan.roof_y),
                inner[j].at(plan.roof_y),
                inner[j].at(plan.top),
                inner[i].at(plan.top),
            ],
            into.at(0.0),
            look.parapet_in,
        );
    }
    polygon_cap(m, &inner, plan.roof_y, true, look.roof);

    top_details(m, plan, &fp, &look);
    style_details(m, b, plan, &fp, &look);
    house_details(m, b, plan, &fp, &look);
    if let Some(t) = &plan.tower {
        // The stepped-back tower, built from the ground and lifted onto the roof.
        let mark = m.vertices.len();
        let s = build(m, &t.building, &t.plan, pal, seed, index | 0x8000_0000);
        for v in &mut m.vertices[mark..] {
            v.pos[1] += t.lift;
        }
        stats.windows += s.windows;
        stats.lit_windows += s.lit_windows;
    } else {
        roof_clutter(m, plan, &fp, &inner, &look, pal, &mut rng);
    }
    if b.style == StyleTag::Motel {
        // The motel's name on the street side, over the parapet.
        let i = plan.frontage;
        let (a, c) = (fp[i], fp[(i + 1) % n]);
        let len = (c - a).length();
        let f = Frame::upright(a.at(0.0), (c - a).normalize());
        // Over a mansard it stands on the eave.
        let f = if plan.top_kind == TopKind::Mansard {
            f.offset(0.0, 0.0, MANSARD_D + 0.05)
        } else {
            f
        };
        let w = (len * 0.5).clamp(3.0, 9.0);
        let x0 = (len - w) * 0.5;
        sign_band(
            m,
            &f,
            x0,
            x0 + w,
            plan.top - 0.5,
            plan.top + 0.9,
            &look,
            &mut rng,
        );
    }

    stats.triangles = (m.indices.len() - start) / 3;
    stats
}

/// Dingbat fascia, ranch wainscots, the two-tone tract house's belt, a bungalow's porch,
/// the barn's doors.
fn house_details(m: &mut WorldMesh, b: &Building, plan: &BuildingPlan, fp: &[V2], look: &Look) {
    let n = fp.len();
    let frame = |i: usize| {
        let (a, c) = (fp[i], fp[(i + 1) % n]);
        Frame::upright(a.at(0.0), (c - a).normalize())
    };
    match b.style {
        StyleTag::Dingbat if plan.floors() > 1 => {
            // A fascia band where the flats overhang the carport.
            let y = plan.floor_y[1];
            ring_band(
                m,
                fp,
                0.06,
                y - 0.25,
                y + 0.05,
                look.trim,
                Some(look.trim),
                Some(look.trim),
            );
        }
        StyleTag::Ranch => {
            // A low brick wainscot between the openings.
            let brick = Paint::new(shade(look.trim.rgb(), 230), Pattern::Brick);
            for (i, fpl) in plan.facades.iter().enumerate() {
                if fpl.columns == 0 {
                    continue;
                }
                let f = frame(i);
                let bay = fpl.len / fpl.columns as f32;
                for c in 0..fpl.columns {
                    if fpl.door == Some(c) || fpl.garage == Some(c) {
                        continue;
                    }
                    let (x0, x1) = (c as f32 * bay, (c + 1) as f32 * bay);
                    boxf(
                        m,
                        &f,
                        [x0, plan.plinth_top, 0.0],
                        [x1, plan.plinth_top + 0.55, 0.05],
                        face::POS_Y | face::POS_Z,
                        brick,
                    );
                }
            }
        }
        StyleTag::Tract70s if plan.floors() > 1 => {
            let y = plan.floor_y[1];
            ring_band(
                m,
                fp,
                0.05,
                y - 0.08,
                y + 0.1,
                look.trim,
                Some(look.trim),
                Some(look.trim),
            );
        }
        StyleTag::Craftsman if b.usage == BuildingUse::Residential => {
            let i = plan.frontage;
            let fpl = &plan.facades[i];
            if fpl.columns > 0 && fpl.len > 6.0 {
                porch(m, &frame(i), plan, fpl, look);
            }
        }
        StyleTag::Barn => {
            // Big doors with a cross brace on the gable ends.
            for i in 0..n {
                let len = plan.facades[i].len;
                let other = plan.facades[(i + 1) % n].len;
                if n != 4 || len > other {
                    continue;
                }
                let f = frame(i);
                let w = (len * 0.4).min(4.5);
                let (x0, x1) = ((len - w) * 0.5, (len + w) * 0.5);
                let y1 = (LOT_Y + 3.8).min(plan.top - 0.4);
                barn_door(m, &f, x0, x1, LOT_Y, y1, look);
                // The hay loft door under the roof.
                let lw = (w * 0.4).min(1.6);
                barn_door(
                    m,
                    &f,
                    (len - lw) * 0.5,
                    (len + lw) * 0.5,
                    plan.top + 0.6,
                    plan.top + 0.6 + lw * 1.1,
                    look,
                );
            }
        }
        _ => {}
    }
}

/// A bungalow's front porch: a deck, square posts on tapered piers and a shed roof.
fn porch(m: &mut WorldMesh, f: &Frame, plan: &BuildingPlan, fpl: &FacadePlan, look: &Look) {
    let len = fpl.len;
    let bay = len / fpl.columns as f32;
    let door = fpl.door.unwrap_or(0) as f32;
    let w = (len * 0.65).max(4.5).min(len - 0.6);
    let cx = (door + 0.5) * bay;
    let x0 = clamp(cx - w * 0.5, 0.3, len - 0.3 - w);
    let x1 = x0 + w;
    let depth = 2.4;
    let deck = plan.plinth_top + 0.2;
    boxf(
        m,
        f,
        [x0, LOT_Y - 0.05, 0.0],
        [x1, deck, depth],
        face::NO_BOTTOM & !face::NEG_Z,
        look.plinth,
    );
    let top = if plan.floors() > 1 {
        plan.floor_y[1] - 0.1
    } else {
        plan.top - 0.15
    };
    let posts = if w > 7.0 { 3 } else { 2 };
    for k in 0..posts {
        let x = x0 + 0.35 + (w - 0.7) * k as f32 / (posts - 1) as f32;
        boxf(
            m,
            f,
            [x - 0.32, deck, depth - 0.65],
            [x + 0.32, deck + 0.95, depth - 0.05],
            face::NO_BOTTOM,
            look.wall,
        );
        boxf(
            m,
            f,
            [x - 0.16, deck + 0.95, depth - 0.51],
            [x + 0.16, top, depth - 0.19],
            face::NEG_X | face::POS_X | face::POS_Z | face::NEG_Z,
            look.trim,
        );
    }
    // Shed roof from the wall out over the posts, with a fascia.
    let (yi, yo) = (top + 0.55, top);
    let slope = [
        f.p(x0 - 0.3, yi, 0.0),
        f.p(x1 + 0.3, yi, 0.0),
        f.p(x1 + 0.3, yo, depth + 0.4),
        f.p(x0 - 0.3, yo, depth + 0.4),
    ];
    let up = f.y * (depth + 0.4) + f.z * 0.55;
    quad(m, slope, up, look.roof_tile);
    quad(m, slope, -up, look.trim);
    boxf(
        m,
        f,
        [x0 - 0.3, yo - 0.22, depth + 0.3],
        [x1 + 0.3, yo, depth + 0.42],
        face::POS_Z | face::NEG_X | face::POS_X | face::NEG_Y,
        look.trim,
    );
}

/// A board door with a white frame and cross brace.
#[allow(clippy::too_many_arguments)]
fn barn_door(m: &mut WorldMesh, f: &Frame, x0: f32, x1: f32, y0: f32, y1: f32, look: &Look) {
    let z = 0.04;
    rect(m, f, x0, y0, x1, y1, z, false, look.door);
    let t = 0.14;
    let trim = look.trim;
    rect(m, f, x0 - t, y0, x0, y1 + t, z, false, trim);
    rect(m, f, x1, y0, x1 + t, y1 + t, z, false, trim);
    rect(m, f, x0, y1, x1, y1 + t, z, false, trim);
    // The cross: two thin diagonal strips.
    for (a, b) in [((x0, y0), (x1, y1)), ((x0, y1), (x1, y0))] {
        let d = V2::new(b.0 - a.0, b.1 - a.1).normalize();
        let o = V2::new(-d.y, d.x) * (t * 0.5);
        quad(
            m,
            [
                f.p(a.0 - o.x, a.1 - o.y, z + 0.01),
                f.p(b.0 - o.x, b.1 - o.y, z + 0.01),
                f.p(b.0 + o.x, b.1 + o.y, z + 0.01),
                f.p(a.0 + o.x, a.1 + o.y, z + 0.01),
            ],
            f.z,
            trim,
        );
    }
}

/// A silo on an octagonal footprint: a concrete stave cylinder under a metal dome.
fn silo(m: &mut WorldMesh, b: &Building, fp: &[V2], look: &Look, pal: &Palette) {
    let mut c = V2::new(0.0, 0.0);
    for &p in fp {
        c = c + p * (1.0 / fp.len() as f32);
    }
    let r = (fp[0] - c).length() * 0.97;
    let f = Frame::at(c.at(LOT_Y - 0.2));
    let staves = Paint::new(mix(pal.concrete, pal.stucco_cream, 90), Pattern::Concrete);
    let h = b.height_m;
    frustum(m, &f, r, r, h, 12, false, false, staves);
    // Hoops.
    for k in 1..4 {
        let y = h * k as f32 / 4.0;
        frustum(
            m,
            &f.offset(0.0, y, 0.0),
            r + 0.06,
            r + 0.06,
            0.18,
            12,
            false,
            false,
            look.frame,
        );
    }
    let dome = Paint::new(pal.metal_light, Pattern::Metal);
    lathe(
        m,
        &f.offset(0.0, h, 0.0),
        &[
            [r + 0.1, 0.0],
            [r * 0.75, r * 0.45],
            [r * 0.3, r * 0.75],
            [0.0, r * 0.82],
        ],
        12,
        false,
        false,
        dome,
    );
}

/// A gambrel roof over a rectangular barn: steep lower slopes to a knee, shallow upper
/// slopes to the ridge, five-sided gable walls at the ends.
fn gambrel_roof(m: &mut WorldMesh, fp: &[V2], wall_top: f32, look: &Look) {
    let e0 = fp[1] - fp[0];
    let e1 = fp[2] - fp[1];
    let (u, short) = if e0.length() >= e1.length() {
        (e0.normalize(), e1.length())
    } else {
        (e1.normalize(), e0.length())
    };
    let v = u.left();
    let mut c = V2::new(0.0, 0.0);
    for &p in fp {
        c = c + p * 0.25;
    }
    let half_long = fp
        .iter()
        .map(|&p| abs((p - c).dot(u)))
        .fold(0.0f32, f32::max);
    let hw = short * 0.5;
    let over = 0.35;
    // Cross-section across the barn: (offset from the ridge line, height).
    let prof = [
        (hw + over, wall_top - over * 1.6),
        (hw * 0.62, wall_top + hw * 0.62),
        (0.0, wall_top + hw * 0.92),
    ];
    let pt = |side: f32, k: usize, end: f32, extra: f32| {
        let (o, y) = prof[k];
        (c + v * (side * o) + u * (end * (half_long + extra))).at(y)
    };
    for side in [-1.0f32, 1.0] {
        for k in 0..2 {
            let q = [
                pt(side, k, -1.0, over),
                pt(side, k, 1.0, over),
                pt(side, k + 1, 1.0, over),
                pt(side, k + 1, -1.0, over),
            ];
            let up = (v * side).at(0.0) + V3::Y;
            quad(m, q, up, look.roof_tile);
            quad(m, q, -up, look.trim);
        }
    }
    // Gable walls under the roof at both ends.
    for end in [-1.0f32, 1.0] {
        let out = (u * end).at(0.0);
        let at = |side: f32, k: usize| {
            let (o, y) = prof[k];
            let o = if k == 0 { hw } else { o };
            let y = if k == 0 { wall_top } else { y };
            (c + v * (side * o) + u * (end * half_long)).at(y)
        };
        let (l0, l1, top, r1, r0) = (at(-1.0, 0), at(-1.0, 1), at(1.0, 2), at(1.0, 1), at(1.0, 0));
        tri(m, l0, r0, l1, out, look.wall);
        tri(m, l1, r0, r1, out, look.wall);
        tri(m, l1, r1, top, out, look.wall);
    }
}

/// A curtain-walled facade: a glazed lobby, then a ribbon of glass over a spandrel on
/// every floor, split into panels that light up one by one, with mullions.
#[allow(clippy::too_many_arguments)]
fn curtain_facade(
    m: &mut WorldMesh,
    f: &Frame,
    plan: &BuildingPlan,
    fpl: &FacadePlan,
    is_front: bool,
    look: &Look,
    rng: &mut Rng,
    stats: &mut BuildingStats,
) {
    let len = fpl.len;
    let floors = plan.floors();
    let edge = 0.35;
    let (x0, x1) = (edge, len - edge);
    // Corner piers.
    rect(
        m,
        f,
        0.0,
        plan.plinth_top,
        x0,
        plan.top,
        0.0,
        false,
        look.wall,
    );
    rect(
        m,
        f,
        x1,
        plan.plinth_top,
        len,
        plan.top,
        0.0,
        false,
        look.wall,
    );
    let d = 0.14;
    let cols = fpl.columns.max(1);
    let w = (x1 - x0) / cols as f32;
    for k in 0..floors {
        let y0 = if k == 0 {
            plan.plinth_top
        } else {
            plan.floor_y[k]
        };
        let y1 = if k + 1 == floors {
            plan.roof_y
        } else {
            plan.floor_y[k + 1]
        };
        let spandrel = if k == 0 { 0.0 } else { (y1 - y0) * 0.35 };
        rect(m, f, x0, y0, x1, y0 + spandrel, 0.0, false, look.wall);
        for c in 0..cols {
            let (a, b) = (x0 + w * c as f32, x0 + w * (c + 1) as f32);
            let lit = k > 0 && rng.range(0, 256) < LIT_WINDOWS_256;
            let sky = rng.range(0, 110);
            let pane = look.glass.with_rgb(mix(look.glass.rgb(), look.sky, sky));
            let pane = if lit {
                pane.glowing(look.lit.rgb_emissive(), Glow::Window)
            } else {
                pane
            };
            rect(m, f, a, y0 + spandrel, b, y1, -d, false, pane);
            stats.windows += 1;
            stats.lit_windows += u32::from(lit);
        }
        if k == 0 && is_front {
            // Entrance doors in the middle of the lobby glass.
            let cx = len * 0.5;
            rect(
                m,
                f,
                cx - 1.2,
                y0,
                cx + 1.2,
                y0 + 2.5,
                -d + 0.02,
                false,
                look.frame,
            );
            rect(
                m,
                f,
                cx - 1.1,
                y0 + 0.1,
                cx - 0.05,
                y0 + 2.4,
                -d + 0.03,
                false,
                look.shop_glass,
            );
            rect(
                m,
                f,
                cx + 0.05,
                y0 + 0.1,
                cx + 1.1,
                y0 + 2.4,
                -d + 0.03,
                false,
                look.shop_glass,
            );
            boxf(
                m,
                f,
                [cx - 2.4, y0 + 3.0, 0.0],
                [cx + 2.4, y0 + 3.2, 2.2],
                face::ALL & !face::NEG_Z,
                look.trim,
            );
        }
    }
    // Mullions down the panel joints, and one through each panel.
    let n = cols * 2;
    for j in 1..n {
        let x = x0 + (x1 - x0) * j as f32 / n as f32;
        let t = if j % 2 == 0 { 0.09 } else { 0.05 };
        boxf(
            m,
            f,
            [x - t, plan.plinth_top, -d],
            [x + t, plan.roof_y, 0.04],
            face::NEG_X | face::POS_X | face::POS_Z,
            look.trim,
        );
    }
}

/// A warehouse wall: roll-up doors and a man door along the street, a band of clerestory
/// windows high up, pilasters on blank walls.
#[allow(clippy::too_many_arguments)]
fn warehouse_facade(
    m: &mut WorldMesh,
    f: &Frame,
    plan: &BuildingPlan,
    fpl: &FacadePlan,
    is_front: bool,
    look: &Look,
    rng: &mut Rng,
    stats: &mut BuildingStats,
) {
    let len = fpl.len;
    let (y0, top) = (plan.plinth_top, plan.top);
    // A painted stripe along the top of every wall.
    rect(
        m,
        f,
        0.0,
        top - 1.2,
        len,
        top - 0.8,
        0.012,
        false,
        look.trim,
    );
    if fpl.columns == 0 {
        rect(m, f, 0.0, y0, len, top, 0.0, false, look.wall);
        let piers = ((len / 7.0) as u32).max(1);
        for k in 1..piers {
            let x = len * k as f32 / piers as f32;
            boxf(
                m,
                f,
                [x - 0.2, y0, 0.0],
                [x + 0.2, top - 0.2, 0.22],
                face::NEG_X | face::POS_X | face::POS_Z,
                look.wall,
            );
        }
        return;
    }
    // Roll-up doors along the street: in the bottom band, between wall pieces.
    let doors: u32 = if is_front {
        ((len / 14.0) as u32).clamp(1, 4)
    } else {
        0
    };
    let door_w = 4.2f32.min(len * 0.3);
    let door_top = (y0 + 4.6).min(top - 2.6);
    let mut x = 0.0;
    let span = len / (doors + 1) as f32;
    for k in 0..doors {
        let cx = span * (k + 1) as f32;
        let (a, b) = (cx - door_w * 0.5, cx + door_w * 0.5);
        rect(m, f, x, y0, a, door_top, 0.0, false, look.wall);
        roll_door(m, f, a, b, LOT_Y, door_top, look);
        x = b;
    }
    if is_front && len > door_w + 4.0 {
        // The man door at the end, under a little canopy.
        let (a, b) = (len - 2.4, len - 1.4);
        rect(m, f, x, y0, a, door_top, 0.0, false, look.wall);
        inset(m, f, a, y0, b, y0 + 2.2, 0.15, false, look.reveal);
        rect(m, f, a, y0, b, y0 + 2.2, -0.15, false, look.door);
        rect(m, f, a, y0 + 2.2, b, door_top, 0.0, false, look.wall);
        boxf(
            m,
            f,
            [a - 0.4, y0 + 2.5, 0.0],
            [b + 0.4, y0 + 2.62, 1.0],
            face::ALL & !face::NEG_Z,
            look.trim,
        );
        x = b;
    }
    rect(m, f, x, y0, len, door_top, 0.0, false, look.wall);
    // Clerestory windows between the doors' tops and the stripe.
    let sill = door_top + 0.7;
    let head = (sill + 1.1).min(top - 1.4);
    rect(m, f, 0.0, door_top, len, sill, 0.0, false, look.wall);
    rect(m, f, 0.0, head, len, top, 0.0, false, look.wall);
    let bay = len / fpl.columns as f32;
    let mut x = 0.0;
    for c in 0..fpl.columns {
        let cx = (c as f32 + 0.5) * bay;
        let w = (bay * 0.6).min(3.6);
        let (a, b) = (cx - w * 0.5, cx + w * 0.5);
        rect(m, f, x, sill, a, head, 0.0, false, look.wall);
        let lit = rng.range(0, 256) < LIT_WINDOWS_256 / 2;
        let pane = if lit {
            look.glass.glowing(look.lit.rgb_emissive(), Glow::Window)
        } else {
            look.glass
        };
        inset(m, f, a, sill, b, head, 0.12, false, look.reveal);
        rect(m, f, a, sill, b, head, -0.12, false, pane);
        stats.windows += 1;
        stats.lit_windows += u32::from(lit);
        x = b;
    }
    rect(m, f, x, sill, len, head, 0.0, false, look.wall);
}

/// A ribbed roll-up door in an inset opening.
#[allow(clippy::too_many_arguments)]
fn roll_door(m: &mut WorldMesh, f: &Frame, x0: f32, x1: f32, y0: f32, y1: f32, look: &Look) {
    let d = 0.2;
    inset(m, f, x0, y0, x1, y1, d, false, look.reveal);
    let panel = Paint::new(shade(look.frame.rgb(), 330), Pattern::Metal);
    rect(m, f, x0, y0, x1, y1, -d, false, panel);
    let rib = panel.with_rgb(shade(panel.rgb(), 170));
    let ribs = (((y1 - y0) / 0.6) as u32).max(2);
    for k in 1..ribs {
        let y = y0 + (y1 - y0) * k as f32 / ribs as f32;
        rect(
            m,
            f,
            x0 + 0.05,
            y - 0.025,
            x1 - 0.05,
            y + 0.025,
            -d + 0.006,
            false,
            rib,
        );
    }
    // The hood the door rolls into.
    boxf(
        m,
        f,
        [x0 - 0.1, y1, 0.0],
        [x1 + 0.1, y1 + 0.45, 0.3],
        face::ALL & !face::NEG_Z,
        look.trim,
    );
}

/// A motel wing's court side: a door and a window per room on both floors, the upper
/// walkway on posts with its railing, and the roof overhang above it.
#[allow(clippy::too_many_arguments)]
fn motel_facade(
    m: &mut WorldMesh,
    f: &Frame,
    plan: &BuildingPlan,
    fpl: &FacadePlan,
    look: &Look,
    rng: &mut Rng,
    stats: &mut BuildingStats,
) {
    let len = fpl.len;
    let floors = plan.floors();
    let bay = len / fpl.columns as f32;
    let walk = 1.8;
    for k in 0..floors {
        let y0 = if k == 0 {
            plan.plinth_top
        } else {
            plan.floor_y[k]
        };
        let y1 = if k + 1 == floors {
            plan.top
        } else {
            plan.floor_y[k + 1]
        };
        let door_top = (y0 + 2.1).min(y1 - 0.3);
        let (sill, head) = (y0 + 0.95, (y0 + 2.0).min(y1 - 0.35));
        rect(m, f, 0.0, door_top, len, y1, 0.0, false, look.wall);
        let mut x = 0.0;
        for c in 0..fpl.columns {
            let bx = c as f32 * bay;
            let (d0, d1) = (bx + 0.35, bx + 1.3);
            let (w0, w1) = (bx + 1.75, (bx + bay - 0.45).min(bx + 3.3));
            rect(m, f, x, y0, d0, door_top, 0.0, false, look.wall);
            inset(m, f, d0, y0, d1, door_top, 0.12, false, look.reveal);
            rect(m, f, d0, y0, d1, door_top, -0.12, false, look.door);
            rect(m, f, d1, y0, w0, door_top, 0.0, false, look.wall);
            if w1 > w0 + 0.4 {
                rect(m, f, w0, y0, w1, sill, 0.0, false, look.wall);
                rect(m, f, w0, head, w1, door_top, 0.0, false, look.wall);
                let lit = rng.range(0, 256) < LIT_WINDOWS_256;
                let pane = if lit {
                    look.glass.glowing(look.lit.rgb_emissive(), Glow::Window)
                } else {
                    look.glass
                };
                window(m, f, w0, sill, w1, head, pane, false, look);
                stats.windows += 1;
                stats.lit_windows += u32::from(lit);
                x = w1;
            } else {
                x = d1;
            }
        }
        rect(m, f, x, y0, len, door_top, 0.0, false, look.wall);
    }
    if floors < 2 {
        return;
    }
    // The walkway deck, its railing and the posts holding it and the roof overhang.
    let y = plan.floor_y[1];
    boxf(
        m,
        f,
        [0.0, y - 0.3, 0.0],
        [len, y, walk],
        face::ALL & !face::NEG_Z,
        look.trim,
    );
    let rail = Paint::new(shade(look.trim.rgb(), 200), Pattern::Metal);
    rect(m, f, 0.0, y + 0.1, len, y + 1.0, walk - 0.05, false, rail);
    rect(m, f, 0.0, y + 0.1, len, y + 1.0, walk - 0.05, true, rail);
    boxf(
        m,
        f,
        [0.0, y + 0.95, walk - 0.1],
        [len, y + 1.05, walk],
        face::POS_Y | face::POS_Z | face::NEG_Z,
        look.trim,
    );
    boxf(
        m,
        f,
        [-0.2, plan.top - 0.35, 0.0],
        [len + 0.2, plan.top - 0.1, walk + 0.3],
        face::ALL & !face::NEG_Z,
        look.trim,
    );
    let posts = ((len / 6.0) as u32).max(1);
    for k in 0..=posts {
        let x = (len * k as f32 / posts as f32).clamp(0.15, len - 0.15);
        boxf(
            m,
            f,
            [x - 0.1, LOT_Y, walk - 0.25],
            [x + 0.1, plan.top - 0.35, walk - 0.05],
            face::NEG_X | face::POS_X | face::POS_Z | face::NEG_Z,
            look.trim,
        );
    }
}

/// One facade, floor by floor. `f`: origin at the facade's start corner on the ground, X
/// along the facade, Y up, Z out of the building.
#[allow(clippy::too_many_arguments)]
fn facade(
    m: &mut WorldMesh,
    f: &Frame,
    plan: &BuildingPlan,
    fp: &FacadePlan,
    is_front: bool,
    b: &Building,
    look: &Look,
    pal: &Palette,
    rng: &mut Rng,
    stats: &mut BuildingStats,
) {
    let len = fp.len;
    let floors = plan.floors();
    if b.style == StyleTag::StripMall && is_front {
        strip_mall_front(m, f, plan, len, look, pal, rng);
        return;
    }
    if b.usage == BuildingUse::Parking && is_front {
        // A detached garage: one wide door.
        let (x0, x1) = (0.5, (len - 0.5).max(1.0));
        let top = (plan.plinth_top + 2.2).min(plan.top - 0.3);
        rect(
            m,
            f,
            0.0,
            plan.plinth_top,
            x0,
            plan.top,
            0.0,
            false,
            look.wall,
        );
        rect(
            m,
            f,
            x1,
            plan.plinth_top,
            len,
            plan.top,
            0.0,
            false,
            look.wall,
        );
        rect(m, f, x0, top, x1, plan.top, 0.0, false, look.wall);
        garage_door(m, f, x0, x1, LOT_Y, top, look);
        return;
    }
    if fp.columns == 0 && !fp.storefront {
        rect(
            m,
            f,
            0.0,
            plan.plinth_top,
            len,
            plan.top,
            0.0,
            false,
            look.wall,
        );
        if b.style == StyleTag::Brick && len > 6.0 {
            // Drainpipes on blank brick walls.
            for x in [0.35, len - 0.35] {
                boxf(
                    m,
                    f,
                    [x - 0.06, plan.plinth_top, 0.0],
                    [x + 0.06, plan.roof_y, 0.12],
                    face::NEG_X | face::POS_X | face::POS_Z,
                    look.frame,
                );
            }
        }
        return;
    }
    for k in 0..floors {
        let y0 = if k == 0 {
            plan.plinth_top
        } else {
            plan.floor_y[k]
        };
        let y1 = if k + 1 == floors {
            plan.top
        } else {
            plan.floor_y[k + 1]
        };
        // Two-tone tract houses: siding on the upper floor.
        let upper = Look {
            wall: look.upper,
            reveal: look.upper.with_rgb(shade(look.upper.rgb(), 200)),
            ..*look
        };
        let look = if b.style == StyleTag::Tract70s && k > 0 {
            &upper
        } else {
            look
        };
        if k == 0 && fp.carport {
            if is_front {
                carport(m, f, len, y1, look);
            } else {
                rect(m, f, 0.0, y0, len, y1, 0.0, false, look.wall);
            }
            continue;
        }
        if k == 0 {
            if let Some(bp) = &fp.base {
                let signs = b.usage != BuildingUse::Residential;
                base_band(m, f, plan, fp, bp, y0, y1, look, rng, signs);
                continue;
            }
        }
        if fp.columns == 0 {
            rect(m, f, 0.0, y0, len, y1, 0.0, false, look.wall);
            continue;
        }
        let bay = len / fp.columns as f32;
        let sill = plan.sill(k);
        let head = (sill + plan.window_h).min(y1 - 0.25);
        let door_floor = k == 0 && fp.door.is_some();
        // Bay windows and ribbons are upper-floor kinds; a floor with doors is punched.
        let kind = match plan.window_kind {
            WindowKind::Bay if k == 0 => WindowKind::Punched,
            _ if door_floor => WindowKind::Punched,
            kind => kind,
        };
        if !door_floor {
            // Wall bands below and above the window row; piers between windows.
            rect(m, f, 0.0, y0, len, sill, 0.0, false, look.wall);
            rect(m, f, 0.0, head, len, y1, 0.0, false, look.wall);
        }
        let mut x = 0.0;
        for c in 0..fp.columns {
            let cx = (c as f32 + 0.5) * bay;
            let is_door = door_floor && fp.door == Some(c);
            let is_garage = door_floor && fp.garage == Some(c);
            if is_garage || is_door {
                let w = if is_garage { bay - 0.6 } else { plan.door_w }.min(bay - 0.5);
                let (x0, x1) = (cx - w * 0.5, cx + w * 0.5);
                rect(m, f, x, y0, x0, y1, 0.0, false, look.wall);
                let top = if is_garage {
                    let top = (y0 + 2.1).min(y1 - 0.2);
                    garage_door(m, f, x0, x1, y0, top, look);
                    top
                } else {
                    let top = (y0 + plan.door_h).min(y1 - 0.2);
                    let residential = b.usage == BuildingUse::Residential;
                    door(m, f, x0, x1, y0, top, plan, residential, look);
                    top
                };
                rect(m, f, x0, top, x1, y1, 0.0, false, look.wall);
                x = x1;
                continue;
            }
            let lit = rng.range(0, 256) < LIT_WINDOWS_256;
            // Panes differ in how much sky they reflect.
            let sky = rng.range(0, 90);
            let pane = look.glass.with_rgb(mix(look.glass.rgb(), look.sky, sky));
            let pane = if lit {
                pane.glowing(look.lit.rgb_emissive(), Glow::Window)
            } else {
                pane
            };
            let (x0, x1) = opening(kind, c, fp.columns, bay, len, plan.window_w);
            // A ribbon's panes meet at mullions; other windows have wall between.
            let between = if kind == WindowKind::Ribbon && c > 0 {
                look.frame
            } else {
                look.wall
            };
            if door_floor {
                rect(m, f, x, y0, x0, y1, 0.0, false, look.wall);
                rect(m, f, x0, y0, x1, sill, 0.0, false, look.wall);
                rect(m, f, x0, head, x1, y1, 0.0, false, look.wall);
            } else {
                rect(m, f, x, sill, x0, head, 0.0, false, between);
            }
            if kind == WindowKind::Bay {
                oriel(m, f, x0, sill, x1, head, pane, look);
            } else {
                window(m, f, x0, sill, x1, head, pane, k == 0, look);
            }
            if kind == WindowKind::Paired {
                // The pier between the pair, flush with the wall.
                let cx = (x0 + x1) * 0.5;
                rect(
                    m,
                    f,
                    cx - 0.09,
                    sill,
                    cx + 0.09,
                    head,
                    0.0,
                    false,
                    look.wall,
                );
            }
            if b.style == StyleTag::Brick {
                // Soldier-course lintel in the trim color.
                rect(
                    m,
                    f,
                    x0 - 0.06,
                    head,
                    x1 + 0.06,
                    (head + 0.2).min(y1),
                    0.012,
                    false,
                    look.sill,
                );
            }
            x = x1;
            stats.windows += 1;
            stats.lit_windows += u32::from(lit);
        }
        if door_floor {
            rect(m, f, x, y0, len, y1, 0.0, false, look.wall);
        } else {
            rect(m, f, x, sill, len, head, 0.0, false, look.wall);
        }
    }
}

/// The opening `(x0, x1)` of window column `c` of `columns`, each `bay` wide on a facade
/// `len` long, for windows `w` wide.
fn opening(kind: WindowKind, c: u32, columns: u32, bay: f32, len: f32, w: f32) -> (f32, f32) {
    let cx = (c as f32 + 0.5) * bay;
    let one = |w: f32| {
        let w = w.min(bay - 0.5);
        (cx - w * 0.5, cx + w * 0.5)
    };
    match kind {
        WindowKind::Punched | WindowKind::Bay => one(w),
        WindowKind::Picture => one(w * 1.7),
        // Two narrow windows in one opening, a pier between them.
        WindowKind::Paired => one(w * 1.24 + 0.18),
        WindowKind::Ribbon => {
            // Edge to edge but for the facade's ends, a mullion on every column line.
            let x0 = if c == 0 { 0.45 } else { c as f32 * bay + 0.05 };
            let x1 = if c + 1 == columns {
                len - 0.45
            } else {
                (c + 1) as f32 * bay - 0.05
            };
            (x0, x1.max(x0 + 0.3))
        }
    }
}

/// A projecting bay window over an opening: a glazed box on a panelled sill with a roof.
#[allow(clippy::too_many_arguments)]
fn oriel(
    m: &mut WorldMesh,
    f: &Frame,
    x0: f32,
    y0: f32,
    x1: f32,
    y1: f32,
    glass: Paint,
    look: &Look,
) {
    let (out, d) = (0.15, 0.5);
    let (a, c) = (x0 - out, x1 + out);
    boxf(
        m,
        f,
        [a, y0 - 0.35, 0.0],
        [c, y0, d],
        face::ALL & !face::NEG_Z,
        look.trim,
    );
    boxf(
        m,
        f,
        [a - 0.05, y1, 0.0],
        [c + 0.05, y1 + 0.2, d + 0.06],
        face::ALL & !face::NEG_Z,
        look.trim,
    );
    rect(m, f, a, y0, c, y1, d, false, glass);
    quad(
        m,
        [
            f.p(a, y0, 0.0),
            f.p(a, y0, d),
            f.p(a, y1, d),
            f.p(a, y1, 0.0),
        ],
        -f.x,
        glass,
    );
    quad(
        m,
        [
            f.p(c, y0, d),
            f.p(c, y0, 0.0),
            f.p(c, y1, 0.0),
            f.p(c, y1, d),
        ],
        f.x,
        glass,
    );
    // The opening behind it, in shadow.
    rect(m, f, x0, y0, x1, y1, 0.0, false, look.reveal);
}

/// The base band of a street front: piers on the bay lines, a belt course over it, and each
/// bay's infill.
#[allow(clippy::too_many_arguments)]
fn base_band(
    m: &mut WorldMesh,
    f: &Frame,
    plan: &BuildingPlan,
    fp: &FacadePlan,
    bp: &BasePlan,
    y0: f32,
    y1: f32,
    look: &Look,
    rng: &mut Rng,
    signs: bool,
) {
    let len = fp.len;
    let span = len / bp.bays as f32;
    let pw = bp.pier_w;
    let pier = |j: u32| {
        let x = j as f32 * span;
        if j == 0 {
            (0.0, pw)
        } else if j == bp.bays {
            (len - pw, len)
        } else {
            (x - pw * 0.5, x + pw * 0.5)
        }
    };
    // The piers' narrow returns in flat shade: a patterned face would be cut per tile.
    let side = Paint::new(shade(look.wall.rgb(), 215), Pattern::Plain);
    for j in 0..=bp.bays {
        let (a, c) = pier(j);
        rect(m, f, a, y0, c, y1, 0.08, false, look.wall);
        boxf(
            m,
            f,
            [a, y0, 0.0],
            [c, y1, 0.08],
            face::NEG_X | face::POS_X,
            side,
        );
    }
    if plan.floors() > 1 {
        // The belt course that caps the base.
        boxf(
            m,
            f,
            [0.0, y1 - 0.22, 0.0],
            [len, y1, 0.16],
            face::POS_Y | face::NEG_Y | face::POS_Z,
            look.trim,
        );
    }
    let kinds = bp.kinds();
    for (k, &kind) in kinds.iter().enumerate() {
        let (x0, x1) = (pier(k as u32).1, pier(k as u32 + 1).0);
        match kind {
            BayKind::Storefront => {
                storefront_bay(m, f, x0, x1, y0, y1, bp, bp.entries[k], look, signs);
                // One awning and one sign band over each run of storefront bays: one
                // business.
                let last = k + 1 == kinds.len() || kinds[k + 1] != BayKind::Storefront;
                let first = (0..=k)
                    .rev()
                    .take_while(|&j| kinds[j] == BayKind::Storefront)
                    .last()
                    .unwrap_or(k);
                let (_, tt) = storefront_heights(bp, y0, y1, signs);
                let (sy0, sy1) = (tt + 0.12, (tt + 0.92).min(y1 - 0.12));
                let sx0 = pier(first as u32).1;
                if last {
                    storefront_awning(m, f, sx0, x1, y0, tt, look);
                }
                if signs && last && sy1 - sy0 > 0.35 {
                    sign_band(m, f, sx0 + 0.1, x1 - 0.1, sy0, sy1, look, rng);
                }
            }
            BayKind::Wall => rect(m, f, x0, y0, x1, y1, 0.0, false, look.wall),
            BayKind::Lobby | BayKind::Door | BayKind::Garage => {
                let (w, h) = match kind {
                    BayKind::Lobby => (plan.door_w, plan.door_h),
                    BayKind::Door => plan.rules.base.door.residential,
                    _ => ((x1 - x0 - 0.8).min(4.2), 3.0),
                };
                let w = w.min(x1 - x0 - 0.6);
                let (a, c) = ((x0 + x1 - w) * 0.5, (x0 + x1 + w) * 0.5);
                let top = (y0 + h).min(y1 - 0.6);
                rect(m, f, x0, y0, a, y1, 0.0, false, look.wall);
                rect(m, f, c, y0, x1, y1, 0.0, false, look.wall);
                rect(m, f, a, top, c, y1, 0.0, false, look.wall);
                match kind {
                    BayKind::Garage => roll_door(m, f, a, c, y0, top, look),
                    _ => door(m, f, a, c, y0, top, plan, kind == BayKind::Door, look),
                }
            }
        }
    }
}

/// The tops of a storefront's display glass and transom from `y0` to `y1`: on a low ground
/// floor the glass and then the transom give way to the sign band, the glass never below
/// the entry.
fn storefront_heights(bp: &BasePlan, y0: f32, y1: f32, signs: bool) -> (f32, f32) {
    let gt = (y0 + bp.glass_top)
        .min(y1 - if signs { 1.1 } else { 0.6 })
        .max(y0 + bp.entry.1 + 0.05);
    let tt = (gt + 0.1 + bp.transom_h)
        .min(y1 - if signs { 0.75 } else { 0.2 })
        .max(gt + 0.1);
    (gt, tt)
}

/// One storefront bay between piers, bottom up: bulkhead, display glass with mullions,
/// transom (the awning and sign band over a run of them are [`base_band`]'s); the entry
/// flush or recessed.
#[allow(clippy::too_many_arguments)]
fn storefront_bay(
    m: &mut WorldMesh,
    f: &Frame,
    x0: f32,
    x1: f32,
    y0: f32,
    y1: f32,
    bp: &BasePlan,
    entry: u8,
    look: &Look,
    signs: bool,
) {
    // The glass line sits back of the piers' face.
    let g = 0.12;
    let (gt, tt) = storefront_heights(bp, y0, y1, signs);
    let bh = bp.bulkhead_h.min(gt - y0 - 1.6).max(0.2);
    rect(m, f, x0, tt, x1, y1, 0.0, false, look.wall);
    // Shallow returns in flat shade, uncut by the wall pattern's tiles, with a floor from
    // the wall line to the glass line.
    let reveal = Paint::new(look.reveal.rgb(), Pattern::Plain);
    inset(m, f, x0, y0, x1, tt, g, true, reveal);
    let ew = bp.entry.0.min((x1 - x0) * 0.45);
    let ex0 = match entry {
        0 => x0 + 0.3,
        2 => x1 - 0.3 - ew,
        _ => (x0 + x1 - ew) * 0.5,
    };
    let ex1 = ex0 + ew;
    let dt = (y0 + bp.entry.1).min(gt - 0.05);
    let bulkhead = look.plinth;
    // Each bulkhead closed on its side toward the entry (the reveal hides the other end).
    for (a, c, side) in [(x0, ex0, face::POS_X), (ex1, x1, face::NEG_X)] {
        if c - a < 0.05 {
            continue;
        }
        boxf(
            m,
            f,
            [a, y0, -g],
            [c, y0 + bh, -g + 0.06],
            face::POS_Y | face::POS_Z | side,
            bulkhead,
        );
        rect(m, f, a, y0 + bh, c, gt, -g, false, look.shop_glass);
        // Thin mullions every pane, and one at each edge of the entry.
        let lights = (((c - a) / 2.2) as u32).max(1);
        for q in 1..=lights {
            let xm = if q == lights {
                if c == x1 {
                    break;
                }
                c
            } else {
                a + (c - a) * q as f32 / lights as f32
            };
            rect(
                m,
                f,
                xm - 0.04,
                y0 + bh,
                xm + 0.04,
                gt,
                -g + 0.03,
                false,
                look.frame,
            );
        }
    }
    // The transom bar and the transom: prism glass, or panelled over in a remodel.
    rect(m, f, x0, gt, x1, gt + 0.1, -g + 0.03, false, look.frame);
    let transom = if bp.transom_glass {
        look.shop_glass
            .with_rgb(mix(look.shop_glass.rgb(), look.sky, 110))
    } else {
        look.trim
    };
    rect(m, f, x0, gt + 0.1, x1, tt, -g, false, transom);
    // The entry: a door in the glass line, or at the back of a recess lined with glass.
    let back = -g - bp.recess;
    if bp.recess > 0.0 {
        for (x, n) in [(ex0, f.x), (ex1, -f.x)] {
            quad(
                m,
                [
                    f.p(x, y0, -g),
                    f.p(x, y0, back),
                    f.p(x, gt, back),
                    f.p(x, gt, -g),
                ],
                n,
                look.shop_glass,
            );
        }
        quad(
            m,
            [
                f.p(ex0, gt, -g),
                f.p(ex1, gt, -g),
                f.p(ex1, gt, back),
                f.p(ex0, gt, back),
            ],
            -f.y,
            look.trim,
        );
        quad(
            m,
            [
                f.p(ex0, y0 + 0.01, back),
                f.p(ex1, y0 + 0.01, back),
                f.p(ex1, y0 + 0.01, -g),
                f.p(ex0, y0 + 0.01, -g),
            ],
            f.y,
            bulkhead,
        );
    }
    rect(m, f, ex0, dt, ex1, gt, back, false, look.shop_glass);
    rect(m, f, ex0, y0, ex1, dt, back, false, look.frame);
    rect(
        m,
        f,
        ex0 + 0.1,
        y0 + 0.12,
        ex1 - 0.1,
        dt - 0.1,
        back + 0.01,
        false,
        look.shop_glass,
    );
    // Stoop in front of the door when the plinth is high.
    if y0 - LOT_Y > 0.05 {
        boxf(
            m,
            f,
            [ex0 - 0.2, LOT_Y - 0.05, 0.0],
            [ex1 + 0.2, y0, 0.6],
            face::NO_BOTTOM & !face::NEG_Z,
            look.plinth,
        );
    }
}

/// Clear height under a storefront awning's lowest edge, m (`docs/backlog/BUILDINGS.md`).
const AWNING_CLEARANCE: f32 = 2.4;

/// The underside of an awning or canopy: shaded, and where the shop is open late, lit by
/// its spill (a glow at window strength, so it shows at night like the light the
/// detailing puts in front of the shop).
fn underside(look: &Look, paint: Paint) -> Paint {
    let under = paint.with_rgb(shade(paint.rgb(), 150));
    if look.signage.is_some_and(|s| s.shop_lit) {
        under.glowing(shade(look.lit.rgb_emissive(), 110), Glow::Window)
    } else {
        under
    }
}

/// The awning over a run of storefronts from `x0` to `x1` on a floor at `y0`, hung from
/// the transoms' top `tt`, its lowest edge clear of heads.
#[allow(clippy::too_many_arguments)]
fn storefront_awning(
    m: &mut WorldMesh,
    f: &Frame,
    x0: f32,
    x1: f32,
    y0: f32,
    tt: f32,
    look: &Look,
) {
    let (ax0, ax1) = (x0 - 0.05, x1 + 0.05);
    // How far an edge `val` below the awning's front may drop.
    let drop = |want: f32, val: f32| want.min(tt - y0 - AWNING_CLEARANCE - val).max(0.1);
    let kind = look.signage.map_or(Awning::Striped as u8, |s| s.awning);
    match kind {
        k if k == Awning::Striped as u8 => awning(m, f, ax0, ax1, tt, 1.4, drop(0.6, 0.28), look),
        k if k == Awning::Solid as u8 => {
            let solid = Look {
                awning: [look.awning[0], look.awning[0]],
                ..*look
            };
            awning(m, f, ax0, ax1, tt, 1.1, drop(0.45, 0.28), &solid);
        }
        k if k == Awning::Canopy as u8 => {
            // A flat stucco canopy with a fascia in the trim color.
            let d = 1.7;
            let (lo, hi) = ([ax0, tt - 0.05, 0.0], [ax1, tt + 0.23, d]);
            boxf(
                m,
                f,
                lo,
                hi,
                face::POS_Y | face::NEG_X | face::POS_X,
                look.wall,
            );
            boxf(m, f, lo, hi, face::NEG_Y, underside(look, look.wall));
            rect(m, f, ax0, tt - 0.05, ax1, tt + 0.23, d, false, look.trim);
        }
        k if k == Awning::Bullnose as u8 => {
            // A quarter round of vinyl from the wall over and down to its front edge.
            let r = 0.9;
            let yb = tt - drop(0.2, 0.0);
            let arc = |k: u32| {
                let (s, c) = crate::math::sin_cos(core::f32::consts::FRAC_PI_2 * k as f32 / 4.0);
                (r * s, yb + r * c)
            };
            let cloth = look.awning[0];
            for k in 0..4 {
                let ((za, ya), (zb, yb2)) = (arc(k), arc(k + 1));
                let q = [
                    f.p(ax0, ya, za),
                    f.p(ax1, ya, za),
                    f.p(ax1, yb2, zb),
                    f.p(ax0, yb2, zb),
                ];
                let out = f.v(0.0, (ya + yb2) * 0.5 - yb, (za + zb) * 0.5);
                quad(m, q, out, cloth);
                for (x, side) in [(ax0, -f.x), (ax1, f.x)] {
                    tri(
                        m,
                        f.p(x, yb, 0.0),
                        f.p(x, ya, za),
                        f.p(x, yb2, zb),
                        side,
                        cloth,
                    );
                }
            }
            let soffit = [
                f.p(ax0, yb, 0.0),
                f.p(ax1, yb, 0.0),
                f.p(ax1, yb, r),
                f.p(ax0, yb, r),
            ];
            quad(m, soffit, -f.y, underside(look, cloth));
        }
        k if k == Awning::Aluminum as u8 => {
            // A shallow ribbed metal shed with a deep fascia.
            let metal = Paint::new(mix(look.trim.rgb(), [236, 236, 230], 150), Pattern::Metal);
            let d = 1.3;
            let yf = tt - drop(0.25, 0.3);
            let slope = [
                f.p(ax0, tt, 0.0),
                f.p(ax1, tt, 0.0),
                f.p(ax1, yf, d),
                f.p(ax0, yf, d),
            ];
            let up = f.y * d + f.z * (tt - yf);
            quad(m, slope, up, metal);
            quad(m, slope, -up, underside(look, metal));
            boxf(
                m,
                f,
                [ax0, yf - 0.3, d],
                [ax1, yf + 0.06, d + 0.06],
                face::ALL,
                metal,
            );
        }
        k if k == Awning::Mansard as u8 => {
            // A shingled pent from above the transom down and out, a soffit under it.
            let (d, h) = (1.0, 0.9);
            let slope = [
                f.p(ax0, tt + h, 0.0),
                f.p(ax1, tt + h, 0.0),
                f.p(ax1, tt, d),
                f.p(ax0, tt, d),
            ];
            quad(m, slope, f.y * d + f.z * h, look.roof_tile);
            let soffit = [
                f.p(ax0, tt, 0.0),
                f.p(ax1, tt, 0.0),
                f.p(ax1, tt, d),
                f.p(ax0, tt, d),
            ];
            quad(m, soffit, -f.y, underside(look, look.reveal));
            for (x, side) in [(ax0, -f.x), (ax1, f.x)] {
                tri(
                    m,
                    f.p(x, tt, 0.0),
                    f.p(x, tt + h, 0.0),
                    f.p(x, tt, d),
                    side,
                    look.roof_tile,
                );
            }
        }
        _ => {}
    }
}

/// A dingbat's open carport under the flats: a deep shadowed recess across the front
/// on slim posts.
fn carport(m: &mut WorldMesh, f: &Frame, len: f32, y1: f32, look: &Look) {
    let (x0, x1) = (0.3, len - 0.3);
    let top = y1 - 0.3;
    let d = 4.5;
    rect(m, f, 0.0, LOT_Y, x0, y1, 0.0, false, look.wall);
    rect(m, f, x1, LOT_Y, len, y1, 0.0, false, look.wall);
    rect(m, f, x0, top, x1, y1, 0.0, false, look.wall);
    let shadow = look.reveal.with_rgb(shade(look.wall.rgb(), 110));
    inset(m, f, x0, LOT_Y, x1, top, d, false, shadow);
    rect(m, f, x0, LOT_Y, x1, top, -d, false, shadow);
    let posts = ((len / 3.2) as u32).max(1);
    for k in 1..posts {
        let x = len * k as f32 / posts as f32;
        boxf(
            m,
            f,
            [x - 0.08, LOT_Y, -0.3],
            [x + 0.08, top, -0.14],
            face::NEG_X | face::POS_X | face::POS_Z,
            look.frame,
        );
    }
}

/// An inset window: glass at the back, reveals, a sill below.
#[allow(clippy::too_many_arguments)]
fn window(
    m: &mut WorldMesh,
    f: &Frame,
    x0: f32,
    y0: f32,
    x1: f32,
    y1: f32,
    glass: Paint,
    bottom: bool,
    look: &Look,
) {
    let d = WINDOW_INSET_M;
    // The upward-facing bottom reveal only shows on windows near eye level.
    inset(m, f, x0, y0, x1, y1, d, bottom, look.reveal);
    rect(m, f, x0, y0, x1, y1, -d, false, glass);
    boxf(
        m,
        f,
        [x0 - 0.08, y0 - 0.07, 0.0],
        [x1 + 0.08, y0, 0.07],
        face::POS_Y | face::POS_Z,
        look.sill,
    );
}

/// The reveals (side walls) of a rectangular opening of depth `d`.
#[allow(clippy::too_many_arguments)]
fn inset(
    m: &mut WorldMesh,
    f: &Frame,
    x0: f32,
    y0: f32,
    x1: f32,
    y1: f32,
    d: f32,
    bottom: bool,
    paint: Paint,
) {
    quad(
        m,
        [
            f.p(x0, y0, 0.0),
            f.p(x0, y0, -d),
            f.p(x0, y1, -d),
            f.p(x0, y1, 0.0),
        ],
        f.x,
        paint,
    );
    quad(
        m,
        [
            f.p(x1, y0, 0.0),
            f.p(x1, y0, -d),
            f.p(x1, y1, -d),
            f.p(x1, y1, 0.0),
        ],
        -f.x,
        paint,
    );
    quad(
        m,
        [
            f.p(x0, y1, 0.0),
            f.p(x1, y1, 0.0),
            f.p(x1, y1, -d),
            f.p(x0, y1, -d),
        ],
        -f.y,
        paint,
    );
    if bottom {
        quad(
            m,
            [
                f.p(x0, y0, 0.0),
                f.p(x1, y0, 0.0),
                f.p(x1, y0, -d),
                f.p(x0, y0, -d),
            ],
            f.y,
            paint,
        );
    }
}

/// An entrance: inset door, a stoop up to the plinth and a little canopy.
#[allow(clippy::too_many_arguments)]
fn door(
    m: &mut WorldMesh,
    f: &Frame,
    x0: f32,
    x1: f32,
    y0: f32,
    y1: f32,
    plan: &BuildingPlan,
    residential: bool,
    look: &Look,
) {
    let d = 0.18;
    inset(m, f, x0, y0, x1, y1, d, false, look.reveal);
    if residential {
        rect(m, f, x0, y0, x1, y1, -d, false, look.door);
        // Small window in the door.
        rect(
            m,
            f,
            x0 + 0.25,
            y1 - 0.75,
            x1 - 0.25,
            y1 - 0.25,
            -d + 0.01,
            false,
            look.glass,
        );
    } else {
        rect(m, f, x0, y0, x1, y1, -d, false, look.frame);
        let mid = (x0 + x1) * 0.5;
        rect(
            m,
            f,
            x0 + 0.08,
            y0 + 0.1,
            mid - 0.04,
            y1 - 0.08,
            -d + 0.01,
            false,
            look.glass,
        );
        rect(
            m,
            f,
            mid + 0.04,
            y0 + 0.1,
            x1 - 0.08,
            y1 - 0.08,
            -d + 0.01,
            false,
            look.glass,
        );
    }
    let rise = plan.plinth_top - LOT_Y;
    if rise > 0.05 {
        boxf(
            m,
            f,
            [x0 - 0.3, LOT_Y - 0.05, 0.0],
            [x1 + 0.3, plan.plinth_top, 0.9],
            face::NO_BOTTOM & !face::NEG_Z,
            look.plinth,
        );
    }
    let (depth, over) = if residential { (0.9, 0.35) } else { (1.6, 0.6) };
    boxf(
        m,
        f,
        [x0 - over, y1 + 0.25, 0.0],
        [x1 + over, y1 + 0.37, depth],
        face::ALL & !face::NEG_Z,
        look.trim,
    );
}

/// A striped, double-sided sloped awning from `y` at the wall out to `depth`, dropping by
/// `drop`, with a valance.
#[allow(clippy::too_many_arguments)]
fn awning(
    m: &mut WorldMesh,
    f: &Frame,
    x0: f32,
    x1: f32,
    y: f32,
    depth: f32,
    drop: f32,
    look: &Look,
) {
    // A one-color awning is one panel; stripes are geometry.
    let stripes = if look.awning[0] == look.awning[1] {
        1
    } else {
        (((x1 - x0) / 0.55) as u32).max(1)
    };
    let w = (x1 - x0) / stripes as f32;
    let (yf, val) = (y - drop, 0.28);
    for k in 0..stripes {
        let (a, b) = (x0 + w * k as f32, x0 + w * (k + 1) as f32);
        let paint = look.awning[(k % 2) as usize];
        let under = underside(look, paint);
        let slope = [
            f.p(a, y, 0.0),
            f.p(b, y, 0.0),
            f.p(b, yf, depth),
            f.p(a, yf, depth),
        ];
        let up = f.y * depth + f.z * drop;
        quad(m, slope, up, paint);
        quad(m, slope, -up, under);
        let valance = [
            f.p(a, yf - val, depth),
            f.p(b, yf - val, depth),
            f.p(b, yf, depth),
            f.p(a, yf, depth),
        ];
        quad(m, valance, f.z, paint);
        quad(m, valance, -f.z, under);
    }
    // Side cheeks.
    for x in [x0, x1] {
        let (a, b, c) = (f.p(x, y, 0.0), f.p(x, yf, depth), f.p(x, yf - val, depth));
        tri(m, a, b, c, f.x, look.awning[0]);
        tri(m, a, b, c, -f.x, look.awning[0]);
    }
}

/// A sign box with a row of glowing letter blocks.
#[allow(clippy::too_many_arguments)]
fn sign_band(
    m: &mut WorldMesh,
    f: &Frame,
    x0: f32,
    x1: f32,
    y0: f32,
    y1: f32,
    look: &Look,
    rng: &mut Rng,
) {
    if x1 - x0 < 1.5 {
        return;
    }
    let depth = 0.14;
    if let Some(s) = look.signage {
        if named_sign(m, f, x0, x1, y0, y1, depth, look, s) {
            return;
        }
    }
    boxf(
        m,
        f,
        [x0, y0, 0.0],
        [x1, y1, depth],
        face::ALL & !face::NEG_Z,
        look.sign,
    );
    // Letters: blocks of varying width covering ~60% of the sign, centered.
    let h = y1 - y0;
    let (ly0, ly1) = (y0 + h * 0.28, y1 - h * 0.28);
    let lh = ly1 - ly0;
    let target = (x1 - x0) * 0.55;
    let mut widths: Vec<f32> = Vec::new();
    let mut total = 0.0;
    while total < target && widths.len() < 18 {
        // Mostly letter-sized blocks, now and then a word gap.
        let w = if widths.len() > 2 && rng.range(0, 6) == 0 {
            -lh * 0.5
        } else {
            lh * rng.range_f32(0.45, 0.8)
        };
        widths.push(w);
        total += w.abs() + lh * 0.22;
    }
    let mut x = (x0 + x1 - total) * 0.5;
    let neon = Paint::new(look.neon, Pattern::Plain).glowing(look.neon, Glow::Neon);
    for w in widths {
        if w > 0.0 {
            rect(m, f, x, ly0, x + w, ly1, depth + 0.01, false, neon);
        }
        x += w.abs() + lh * 0.22;
    }
}

/// A sign box lettered with an invented name: neon letters on a dark box, or dark letters
/// on a backlit one. False (nothing drawn) when no name fits.
#[allow(clippy::too_many_arguments)]
fn named_sign(
    m: &mut WorldMesh,
    f: &Frame,
    x0: f32,
    x1: f32,
    y0: f32,
    y1: f32,
    depth: f32,
    look: &Look,
    s: Signage,
) -> bool {
    let h = y1 - y0;
    let px = (h * 0.56 / 5.0).min(0.15);
    let max = ((x1 - x0 - 0.3) / (4.0 * px)) as usize;
    // Each sign of a building its own name, keyed by where it starts along the wall.
    let key = skywave_core::mix64(s.name_seed ^ u64::from(x0.to_bits()));
    let name = crate::signs::business_name(key, max.min(18));
    if name.is_empty() {
        return false;
    }
    let (box_paint, ink) = if s.lit_box {
        let face = shade(mix(look.neon, [255, 244, 214], 190), 140);
        let lit = Paint::new(face, Pattern::Plain).glowing(face, Glow::Neon);
        (lit, Paint::new(shade(look.sign.rgb(), 120), Pattern::Plain))
    } else {
        let neon = Paint::new(look.neon, Pattern::Plain).glowing(look.neon, Glow::Neon);
        (look.sign, neon)
    };
    boxf(
        m,
        f,
        [x0, y0, 0.0],
        [x1, y1, depth],
        face::ALL & !face::NEG_Z,
        look.sign,
    );
    if s.lit_box {
        rect(
            m,
            f,
            x0 + 0.06,
            y0 + 0.06,
            x1 - 0.06,
            y1 - 0.06,
            depth + 0.005,
            false,
            box_paint,
        );
    }
    let w = crate::signs::text_width(&name) as f32 * px;
    let lx = (x0 + x1 - w) * 0.5;
    let ly = (y0 + y1) * 0.5 - 2.5 * px;
    crate::props::letters(m, f, &name, lx, ly, px, depth + 0.012, false, ink);
    true
}

/// A strip mall's mansard canopy: tall `(min, max)` and projecting `(min, max)`, m
/// (`docs/backlog/BUILDINGS.md`, the SoCal table).
const MALL_MANSARD_H: (f32, f32) = (0.9, 1.5);
const MALL_MANSARD_D: (f32, f32) = (2.0, 3.0);

/// The strip mall's front: a row of shop units under one canopy on posts (a flat one, or
/// a shingled mansard), signs on the fascia above or on the mansard's face.
#[allow(clippy::too_many_arguments)]
fn strip_mall_front(
    m: &mut WorldMesh,
    f: &Frame,
    plan: &BuildingPlan,
    len: f32,
    look: &Look,
    pal: &Palette,
    rng: &mut Rng,
) {
    let units = ((len / 11.0) as u32).max(1);
    let unit = len / units as f32;
    let y0 = plan.plinth_top;
    let canopy_y = y0 + 3.3;
    let mansard = look
        .signage
        .is_some_and(|s| s.canopy == Awning::Mansard as u8);
    let (h, depth) = if mansard {
        (
            rng.range_f32(MALL_MANSARD_H.0, MALL_MANSARD_H.1)
                .min(plan.top - canopy_y - 0.2),
            rng.range_f32(MALL_MANSARD_D.0, MALL_MANSARD_D.1),
        )
    } else {
        (0.4, 3.2)
    };
    for u in 0..units {
        let ux = unit * u as f32;
        let sub = Frame {
            o: f.p(ux, 0.0, 0.0),
            ..*f
        };
        let neon = choose(rng, &pal.neon());
        let unit_look = Look { neon, ..*look };
        storefront_unit(m, &sub, unit, y0, canopy_y, &unit_look, rng);
        if mansard && h > 0.8 {
            // Letters on a raceway along the mansard's foot.
            let g = f.offset(0.0, 0.0, depth + 0.02);
            let (sy0, sy1) = (canopy_y + 0.3, canopy_y + h - 0.05);
            sign_band(m, &g, ux + 1.2, ux + unit - 1.2, sy0, sy1, &unit_look, rng);
        } else if !mansard {
            // Sign on the fascia above the canopy.
            sign_band(
                m,
                f,
                ux + 1.2,
                ux + unit - 1.2,
                canopy_y + 0.55,
                (canopy_y + 1.35).min(plan.top - 0.3),
                &unit_look,
                rng,
            );
        }
    }
    rect(m, f, 0.0, canopy_y, len, plan.top, 0.0, false, look.wall);
    let (x0, x1) = (-0.3, len + 0.3);
    if mansard {
        // Profile, wall to eave: soffit, a short fascia, the shingled slope, a flat top
        // back to the wall.
        let (yf, run) = (canopy_y + 0.25, 0.6 * h);
        let p = [
            (0.0, canopy_y),
            (depth, canopy_y),
            (depth, yf),
            (depth - run, canopy_y + h),
            (0.0, canopy_y + h),
        ];
        let under = underside(look, look.reveal);
        let paints = [under, look.trim, look.roof_tile, look.parapet_in];
        for k in 0..4 {
            let ((za, ya), (zb, yb)) = (p[k], p[k + 1]);
            let q = [
                f.p(x0, ya, za),
                f.p(x1, ya, za),
                f.p(x1, yb, zb),
                f.p(x0, yb, zb),
            ];
            let out = f.v(0.0, za - zb, yb - ya);
            quad(m, q, out, paints[k]);
        }
        for (x, side) in [(x0, -f.x), (x1, f.x)] {
            for k in 1..4 {
                let v = |i: usize| f.p(x, p[i].1, p[i].0);
                tri(m, v(0), v(k), v(k + 1), side, look.trim);
            }
        }
    } else {
        // Canopy slab with a fascia.
        let (lo, hi) = ([x0, canopy_y, 0.0], [x1, canopy_y + h, depth]);
        boxf(
            m,
            f,
            lo,
            hi,
            face::ALL & !face::NEG_Z & !face::NEG_Y,
            look.trim,
        );
        boxf(m, f, lo, hi, face::NEG_Y, underside(look, look.trim));
    }
    // Square posts.
    for u in 0..=units {
        let x = (unit * u as f32).clamp(0.3, len - 0.3);
        boxf(
            m,
            f,
            [x - 0.14, LOT_Y, depth - 0.45],
            [x + 0.14, canopy_y, depth - 0.17],
            face::NEG_X | face::POS_X | face::POS_Z | face::NEG_Z,
            look.trim,
        );
    }
}

/// One strip-mall unit: piers, recessed glazing and a door, no awning (the canopy covers it).
#[allow(clippy::too_many_arguments)]
fn storefront_unit(
    m: &mut WorldMesh,
    f: &Frame,
    len: f32,
    y0: f32,
    y1: f32,
    look: &Look,
    rng: &mut Rng,
) {
    let (x0, x1) = (0.4, len - 0.4);
    let glass_top = y1 - 0.4;
    let d = STOREFRONT_INSET_M;
    rect(m, f, 0.0, y0, x0, y1, 0.0, false, look.wall);
    rect(m, f, x1, y0, len, y1, 0.0, false, look.wall);
    rect(m, f, x0, glass_top, x1, y1, 0.0, false, look.wall);
    inset(m, f, x0, y0, x1, glass_top, d, true, look.reveal);
    let door_w = 1.8;
    let door_x = x0 + rng.range_f32(0.8, (x1 - x0 - door_w - 0.8).max(0.9));
    let door_top = y0 + 2.3;
    rect(m, f, x0, y0, door_x, glass_top, -d, false, look.shop_glass);
    rect(
        m,
        f,
        door_x + door_w,
        y0,
        x1,
        glass_top,
        -d,
        false,
        look.shop_glass,
    );
    rect(
        m,
        f,
        door_x,
        door_top,
        door_x + door_w,
        glass_top,
        -d,
        false,
        look.shop_glass,
    );
    rect(
        m,
        f,
        door_x,
        y0,
        door_x + door_w,
        door_top,
        -d,
        false,
        look.frame,
    );
    let mid = door_x + door_w * 0.5;
    rect(
        m,
        f,
        door_x + 0.08,
        y0 + 0.1,
        mid - 0.04,
        door_top - 0.08,
        -d + 0.01,
        false,
        look.shop_glass,
    );
    rect(
        m,
        f,
        mid + 0.04,
        y0 + 0.1,
        door_x + door_w - 0.08,
        door_top - 0.08,
        -d + 0.01,
        false,
        look.shop_glass,
    );
    let panes = ((x1 - x0) / 2.4) as u32;
    for k in 1..panes {
        let xm = x0 + (x1 - x0) * k as f32 / panes as f32;
        if xm > door_x - 0.2 && xm < door_x + door_w + 0.2 {
            continue;
        }
        boxf(
            m,
            f,
            [xm - 0.05, y0, -d],
            [xm + 0.05, glass_top, -d + 0.12],
            face::NEG_X | face::POS_X | face::POS_Z,
            look.frame,
        );
    }
}

/// Height of a stepped parapet's three steps.
const STEPS_H: f32 = 2.4;
/// Segments of a shaped parapet's outline.
const SHAPED_SEGMENTS: usize = 8;
/// Brackets under a bracketed cornice at most.
const MAX_BRACKETS: usize = 16;
/// How far a mansard reaches out at its foot, and how far down the wall it comes.
const MANSARD_D: f32 = 0.7;
const MANSARD_H: f32 = 1.1;

/// The street front's frame and length.
fn front_frame(plan: &BuildingPlan, fp: &[V2]) -> (Frame, f32) {
    let i = plan.frontage;
    let (a, c) = (fp[i], fp[(i + 1) % fp.len()]);
    (
        Frame::upright(a.at(0.0), (c - a).normalize()),
        (c - a).length(),
    )
}

/// The top of a flat roof: nothing more than the coping, a cornice all round, a parapet
/// stepped up or shaped over the middle of the street front, a false front, a cornice on
/// brackets along the street front, or a shingled mansard all round.
fn top_details(m: &mut WorldMesh, plan: &BuildingPlan, fp: &[V2], look: &Look) {
    let (f, len) = front_frame(plan, fp);
    let mid = len * 0.5;
    match plan.top_kind {
        TopKind::Parapet => {}
        TopKind::Cornice => {
            let d = plan.rules.top.cornice_d;
            ring_band(
                m,
                fp,
                d,
                plan.roof_y - 0.2,
                plan.roof_y + 0.18,
                look.sill,
                Some(look.sill),
                Some(look.sill),
            );
        }
        TopKind::Stepped => {
            let mut y = plan.top;
            for (k, frac) in [0.55f32, 0.32, 0.14].iter().enumerate() {
                let half = len * frac * 0.5;
                let rise = 0.7 + 0.1 * k as f32;
                let paint = if k == 2 { look.trim } else { look.wall };
                boxf(
                    m,
                    &f,
                    [mid - half, y, -0.5],
                    [mid + half, y + rise, 0.0],
                    face::NO_BOTTOM,
                    paint,
                );
                y += rise;
            }
        }
        TopKind::Shaped => {
            // Flat shoulders, then a curved crown; coped in the trim color.
            let half = (len * 0.3).clamp(2.0, 7.0);
            let rise = |t: f32| {
                let c = (abs(t) / 0.75).min(1.0);
                0.5 + 0.9 * (1.0 - c * c)
            };
            let (z0, z1) = (-0.3, 0.0);
            let n = SHAPED_SEGMENTS;
            for k in 0..n {
                let (ta, tb) = (
                    -1.0 + 2.0 * k as f32 / n as f32,
                    -1.0 + 2.0 * (k + 1) as f32 / n as f32,
                );
                let (xa, xb) = (mid + ta * half, mid + tb * half);
                let (ya, yb) = (plan.top + rise(ta), plan.top + rise(tb));
                for (z, out) in [(z1, f.z), (z0, -f.z)] {
                    let q = [
                        f.p(xa, plan.top, z),
                        f.p(xb, plan.top, z),
                        f.p(xb, yb, z),
                        f.p(xa, ya, z),
                    ];
                    quad(m, q, out, look.wall);
                }
                let up = f.y * (xb - xa) - f.x * (yb - ya);
                let cap = [
                    f.p(xa, ya, z1 + 0.05),
                    f.p(xb, yb, z1 + 0.05),
                    f.p(xb, yb, z0 - 0.05),
                    f.p(xa, ya, z0 - 0.05),
                ];
                quad(m, cap, up, look.trim);
            }
            for (x, side) in [(mid - half, -f.x), (mid + half, f.x)] {
                let y = plan.top + rise(1.0);
                let q = [
                    f.p(x, plan.top, z0),
                    f.p(x, plan.top, z1),
                    f.p(x, y, z1),
                    f.p(x, y, z0),
                ];
                quad(m, q, side, look.wall);
            }
        }
        TopKind::FalseFront => {
            // The street wall carried up across the whole front, with a coping.
            let h = (len * 0.12).clamp(1.2, 2.4);
            let y = plan.top + h;
            boxf(
                m,
                &f,
                [0.0, plan.top, -0.25],
                [len, y, 0.0],
                face::NO_BOTTOM & !face::POS_Y,
                look.wall,
            );
            boxf(
                m,
                &f,
                [-0.06, y, -0.31],
                [len + 0.06, y + 0.14, 0.08],
                face::ALL,
                look.trim,
            );
        }
        TopKind::Bracketed => {
            // A deep cornice crowning the street front on a row of brackets.
            let d = (plan.rules.top.cornice_d * 2.0).max(0.45);
            let (y0, y1) = (plan.top - 0.25, plan.top + 0.05);
            boxf(
                m,
                &f,
                [-d, y0, 0.0],
                [len + d, y1, d],
                face::ALL & !face::NEG_Z,
                look.sill,
            );
            let count = ((len / 1.2) as usize).clamp(2, MAX_BRACKETS);
            for k in 0..count {
                let x = 0.3 + (len - 0.6) * k as f32 / (count - 1) as f32;
                boxf(
                    m,
                    &f,
                    [x - 0.08, y0 - 0.45, 0.0],
                    [x + 0.08, y0, d * 0.8],
                    face::NEG_X | face::POS_X | face::POS_Z | face::NEG_Y,
                    look.sill,
                );
            }
        }
        TopKind::Mansard => {
            // Shingles from the coping down and out to an eave all round, a soffit under it.
            let n = fp.len();
            let eave = offset_polygon(fp, |_| MANSARD_D);
            let (y0, y1) = (plan.top - MANSARD_H, plan.top + 0.15);
            for i in 0..n {
                let j = (i + 1) % n;
                let out = (fp[j] - fp[i]).right();
                let slope = [eave[i].at(y0), eave[j].at(y0), fp[j].at(y1), fp[i].at(y1)];
                quad(m, slope, out.at(MANSARD_D / (y1 - y0)), look.roof_tile);
                let soffit = [fp[i].at(y0), fp[j].at(y0), eave[j].at(y0), eave[i].at(y0)];
                quad(m, soffit, -V3::Y, look.reveal);
            }
        }
    }
}

/// Belt courses, speed lines and fins by style.
fn style_details(m: &mut WorldMesh, b: &Building, plan: &BuildingPlan, fp: &[V2], look: &Look) {
    let n = fp.len();
    match b.style {
        StyleTag::StuccoBox if b.usage != BuildingUse::Residential => {
            ring_band(
                m,
                fp,
                0.05,
                plan.roof_y - 0.06,
                plan.roof_y + 0.08,
                look.trim,
                Some(look.trim),
                Some(look.trim),
            );
        }
        StyleTag::StuccoBox => {}
        StyleTag::Brick => {
            for k in 1..plan.floors() {
                let y = plan.floor_y[k];
                ring_band(
                    m,
                    fp,
                    0.04,
                    y - 0.07,
                    y + 0.07,
                    look.sill,
                    Some(look.sill),
                    Some(look.sill),
                );
            }
        }
        StyleTag::Deco => {
            // Three speed lines wrapping the top floor.
            for k in 0..3 {
                let y = plan.roof_y - 0.3 - k as f32 * 0.38;
                if y < plan.plinth_top + 2.5 {
                    break;
                }
                ring_band(
                    m,
                    fp,
                    0.06,
                    y - 0.07,
                    y + 0.07,
                    look.trim,
                    Some(look.trim),
                    Some(look.trim),
                );
            }
            let i = plan.frontage;
            let (a, c) = (fp[i], fp[(i + 1) % n]);
            let len = (c - a).length();
            let f = Frame::upright(a.at(0.0), (c - a).normalize());
            let mid = len * 0.5;
            let y = plan.top
                + if plan.top_kind == TopKind::Stepped {
                    STEPS_H
                } else {
                    0.0
                };
            // A fin up the middle of taller fronts.
            if plan.floors() >= 2 && len > 8.0 {
                boxf(
                    m,
                    &f,
                    [mid - 0.25, plan.floor_y[1] + 0.4, 0.0],
                    [mid + 0.25, y - 0.2, 0.55],
                    face::NO_BOTTOM & !face::NEG_Z,
                    look.trim,
                );
            }
        }
        StyleTag::StripMall => {}
        // Theirs are in `house_details` or their own facades.
        StyleTag::Dingbat
        | StyleTag::Craftsman
        | StyleTag::Ranch
        | StyleTag::Tract70s
        | StyleTag::Glass
        | StyleTag::Warehouse
        | StyleTag::Barn
        | StyleTag::Motel => {}
    }
}

/// What stands on a flat roof: plumbing vents and hoods, packaged rooftop units on curbs
/// with a duct elbow into the roof, exhaust fans, skylights ([`RoofPlan`]), set toward the
/// back so the parapet hides most of them from the street; a stair bulkhead and a water
/// tank on taller roofs.
fn roof_clutter(
    m: &mut WorldMesh,
    plan: &BuildingPlan,
    fp: &[V2],
    inner: &[V2],
    look: &Look,
    pal: &Palette,
    rng: &mut Rng,
) {
    // Placed in the street front's frame: x along it, z out of it (the roof is at z < 0).
    let (f, _) = front_frame(plan, fp);
    let (dir, out) = (V2::new(f.x.x, f.x.z), V2::new(f.z.x, f.z.z));
    let o = V2::new(f.o.x, f.o.z);
    let (mut lo, mut hi) = (V2::new(f32::MAX, f32::MAX), V2::new(f32::MIN, f32::MIN));
    for &p in inner {
        let l = V2::new((p - o).dot(dir), (p - o).dot(out));
        lo = V2::new(lo.x.min(l.x), lo.y.min(l.y));
        hi = V2::new(hi.x.max(l.x), hi.y.max(l.y));
    }
    let size = hi - lo;
    if size.x < 3.0 || size.y < 3.0 {
        return;
    }
    let y = plan.roof_y;
    // A frame on the roof at a spot `margin` in from the edges, in the back `share` of
    // the roof's depth; `None` when the spot falls off the roof (odd footprints).
    let spot = |rng: &mut Rng, margin: f32, share: f32| {
        let mx = margin.min(size.x * 0.4);
        let mz = margin.min(size.y * 0.4);
        let x = rng.range_f32(lo.x + mx, hi.x - mx);
        let z = rng.range_f32(lo.y + mz, (lo.y + size.y * share - mz).max(lo.y + mz));
        let p = o + dir * x + out * z;
        crate::math::point_in_polygon(p, inner).then(|| Frame { o: p.at(y), ..f })
    };
    let metal = Paint::new(pal.metal_light, Pattern::Metal);
    let dark = Paint::new(pal.metal_dark, Pattern::Metal);
    let cabinet = Paint::new(mix(pal.metal_light, pal.sand, 90), Pattern::Metal);
    let r = plan.roof;
    for _ in 0..r.vents {
        let Some(g) = spot(rng, 1.0, 1.0) else {
            continue;
        };
        if rng.range(0, 2) == 0 {
            boxf(
                m,
                &g,
                [-0.3, 0.0, -0.25],
                [0.3, 0.45, 0.25],
                face::NO_BOTTOM,
                metal,
            );
        } else {
            frustum(m, &g, 0.06, 0.06, 0.6, 5, false, false, dark);
        }
    }
    for _ in 0..r.units {
        let Some(g) = spot(rng, 2.0, 0.65) else {
            continue;
        };
        let sides = face::NO_BOTTOM & !face::POS_Y;
        boxf(m, &g, [-0.75, 0.0, -0.6], [0.75, 0.3, 0.6], sides, dark);
        boxf(
            m,
            &g,
            [-0.65, 0.3, -0.5],
            [0.65, 1.25, 0.5],
            face::NO_BOTTOM,
            cabinet,
        );
        // The condenser fan's grille on top.
        let grille = [
            g.p(-0.1, 1.26, -0.35),
            g.p(0.5, 1.26, -0.35),
            g.p(0.5, 1.26, 0.35),
            g.p(-0.1, 1.26, 0.35),
        ];
        quad(m, grille, V3::Y, dark);
        // The supply duct: out of the cabinet's side, then down through the roof.
        let elbow = face::POS_Y | face::NEG_Z | face::POS_Z | face::NEG_X;
        boxf(
            m,
            &g,
            [-1.15, 0.45, -0.25],
            [-0.65, 0.85, 0.25],
            elbow,
            metal,
        );
        let drop = face::NEG_X | face::POS_X | face::NEG_Z | face::POS_Z;
        boxf(m, &g, [-1.15, 0.0, -0.25], [-0.75, 0.45, 0.25], drop, metal);
    }
    for _ in 0..r.fans {
        let Some(g) = spot(rng, 1.5, 0.8) else {
            continue;
        };
        let sides = face::NO_BOTTOM & !face::POS_Y;
        boxf(m, &g, [-0.35, 0.0, -0.35], [0.35, 0.3, 0.35], sides, dark);
        frustum(
            m,
            &g.offset(0.0, 0.3, 0.0),
            0.18,
            0.18,
            0.3,
            6,
            false,
            false,
            metal,
        );
        frustum(
            m,
            &g.offset(0.0, 0.5, 0.0),
            0.45,
            0.06,
            0.28,
            6,
            false,
            false,
            metal,
        );
    }
    for _ in 0..r.skylights {
        let Some(g) = spot(rng, 1.5, 1.0) else {
            continue;
        };
        let sides = face::NO_BOTTOM & !face::POS_Y;
        boxf(
            m,
            &g,
            [-0.6, 0.0, -0.9],
            [0.6, 0.25, 0.9],
            sides,
            look.parapet_in,
        );
        let pane = [
            g.p(-0.6, 0.25, -0.9),
            g.p(0.6, 0.25, -0.9),
            g.p(0.6, 0.25, 0.9),
            g.p(-0.6, 0.25, 0.9),
        ];
        quad(m, pane, V3::Y, look.shop_glass);
    }
    let add = &plan.rules.addenda;
    if plan.floors() >= usize::from(add.bulkhead_floors) {
        if let Some(g) = spot(rng, 3.0, 1.0) {
            boxf(
                m,
                &g,
                [-1.4, 0.0, -1.2],
                [1.4, 2.6, 1.2],
                face::NO_BOTTOM,
                look.parapet_in,
            );
        }
        if rng.range(0, add.water_tank_1_in) == 0 && size.x > 9.0 && size.y > 9.0 {
            if let Some(g) = spot(rng, 3.5, 1.0) {
                water_tank(m, g.o, pal);
            }
        }
    }
}

/// A wooden water tank on a steel stand with a conical roof.
fn water_tank(m: &mut WorldMesh, base: V3, pal: &Palette) {
    let wood = Paint::new(pal.bark, Pattern::Bark);
    let steel = Paint::new(pal.metal_dark, Pattern::Metal);
    let legs = 1.8;
    for (x, z) in [(-1.1, -1.1), (1.1, -1.1), (1.1, 1.1), (-1.1, 1.1)] {
        let f = Frame::at(base + V3::new(x, 0.0, z));
        boxf(
            m,
            &f,
            [-0.08, 0.0, -0.08],
            [0.08, legs, 0.08],
            face::NO_BOTTOM & !face::POS_Y,
            steel,
        );
    }
    let f = Frame::at(base + V3::new(0.0, legs, 0.0));
    boxf(m, &f, [-1.3, -0.1, -1.3], [1.3, 0.0, 1.3], face::ALL, steel);
    frustum(m, &f, 1.6, 1.5, 2.6, 10, false, true, wood);
    frustum(
        m,
        &f.offset(0.0, 2.6, 0.0),
        1.7,
        0.0,
        1.0,
        10,
        true,
        false,
        steel,
    );
}

/// A sectional garage door, inset, with panel grooves.
#[allow(clippy::too_many_arguments)]
fn garage_door(m: &mut WorldMesh, f: &Frame, x0: f32, x1: f32, y0: f32, y1: f32, look: &Look) {
    let d = 0.12;
    inset(m, f, x0, y0, x1, y1, d, false, look.reveal);
    let panel = look.sill;
    rect(m, f, x0, y0, x1, y1, -d, false, panel);
    let groove = panel.with_rgb(shade(panel.rgb(), 190));
    for k in 1..4 {
        let y = y0 + (y1 - y0) * k as f32 / 4.0;
        rect(
            m,
            f,
            x0 + 0.05,
            y - 0.02,
            x1 - 0.05,
            y + 0.02,
            -d + 0.006,
            false,
            groove,
        );
    }
}

/// Hip or gable roof over a rectangular footprint, with eaves, fascia and soffit, and
/// sometimes a chimney.
fn pitched_roof(
    m: &mut WorldMesh,
    b: &Building,
    fp: &[V2],
    wall_top: f32,
    look: &Look,
    pal: &Palette,
    rng: &mut Rng,
) {
    let e0 = fp[1] - fp[0];
    let e1 = fp[2] - fp[1];
    let (u, long, short) = if e0.length() >= e1.length() {
        (e0.normalize(), e0.length(), e1.length())
    } else {
        (e1.normalize(), e1.length(), e0.length())
    };
    let mut c = V2::new(0.0, 0.0);
    for &p in fp {
        c = c + p * 0.25;
    }
    let (overhang, pitch) = match b.style {
        StyleTag::Craftsman => (0.7, 0.32),
        StyleTag::Ranch => (0.8, 0.28),
        _ => (0.5, 0.42),
    };
    let eave = offset_polygon(fp, |_| overhang);
    let rise = (short * 0.5 + overhang) * pitch;
    // Mansard roofs are drawn as hips until they get their own shape.
    let hip = matches!(b.roof, Roof::Hip | Roof::Mansard);
    let ridge_half = if hip {
        (long - short) * 0.5
    } else {
        long * 0.5 + overhang
    };
    let ridge = |p: V2| {
        let side = if (p - c).dot(u) > 0.0 { 1.0 } else { -1.0 };
        (c + u * (ridge_half * side)).at(wall_top + rise)
    };
    // Fascia board around the eaves and the soffit under them.
    let soffit = look.trim.with_rgb(shade(look.trim.rgb(), 210));
    ring_band(
        m,
        fp,
        overhang,
        wall_top - 0.22,
        wall_top,
        look.trim,
        None,
        Some(soffit),
    );
    let under = look.roof_tile.with_rgb(shade(look.roof_tile.rgb(), 140));
    for i in 0..4 {
        let (a, bb) = (eave[i], eave[(i + 1) % 4]);
        let along = (bb - a).normalize();
        let up = along.right().at(0.0) + V3::Y;
        if abs(along.dot(u)) > 0.7 {
            let q = [a.at(wall_top), bb.at(wall_top), ridge(bb), ridge(a)];
            quad(m, q, up, look.roof_tile);
            if !hip {
                // Underside, seen under the gable overhangs.
                quad(m, q, -up, under);
            }
        } else if hip {
            tri(
                m,
                a.at(wall_top),
                bb.at(wall_top),
                ridge(a),
                up,
                look.roof_tile,
            );
        }
    }
    if !hip {
        // Gable walls fill the triangles under the roof ends.
        for i in 0..4 {
            let (a, bb) = (fp[i], fp[(i + 1) % 4]);
            let along = (bb - a).normalize();
            if abs(along.dot(u)) < 0.3 {
                let apex = ((a + bb) * 0.5).at(wall_top + short * 0.5 * pitch);
                tri(
                    m,
                    a.at(wall_top),
                    bb.at(wall_top),
                    apex,
                    along.right().at(0.0),
                    look.wall,
                );
            }
        }
    }
    if rng.range(0, 256) < 90 {
        let v = u.left();
        let along = rng.range_f32(-0.3, 0.3) * long;
        let p = c + u * along + v * (short * 0.22);
        let f = Frame::upright(p.at(wall_top), u);
        let chimney = if rng.range(0, 2) == 0 {
            Paint::new(pal.brick_red, Pattern::Brick)
        } else {
            look.wall
        };
        boxf(
            m,
            &f,
            [-0.35, 0.0, -0.35],
            [0.35, rise + 0.7, 0.35],
            face::NO_BOTTOM,
            chimney,
        );
    }
}

/// A house's front yard: a walkway from the door to the sidewalk, a driveway beside the
/// house, and maybe a shade tree and a car in the driveway (returned as props).
pub fn house_site(
    m: &mut WorldMesh,
    b: &Building,
    plan: &BuildingPlan,
    env: &[FacadeEnv],
    pal: &Palette,
    seed: u64,
    index: u32,
) -> Vec<Prop> {
    let mut props = Vec::new();
    let i = plan.frontage;
    let fp = &plan.facades[i];
    let yard = env[i].yard;
    if b.usage != BuildingUse::Residential || fp.columns == 0 || !(2.0..20.0).contains(&yard) {
        return props;
    }
    let mut rng = Rng::new(Seed(seed), &[KEY_BUILDING, u64::from(index), 1]);
    let n = b.footprint.len();
    let a = V2::from_array(b.footprint[i]);
    let dir = (V2::from_array(b.footprint[(i + 1) % n]) - a).normalize();
    let f = Frame::upright(a.at(0.0), dir);
    let bay = fp.len / fp.columns as f32;
    let door = fp.door.unwrap_or(0);
    let cx = (door as f32 + 0.5) * bay;
    let y = LOT_Y + 0.008;
    let walk = Paint::new(pal.concrete, Pattern::Concrete);
    let drive = Paint::new(shade(pal.concrete, 230), Pattern::Concrete);
    let flat = |m: &mut WorldMesh, x0: f32, x1: f32, z0: f32, z1: f32, paint: Paint| {
        quad(
            m,
            [
                f.p(x0, y, z0),
                f.p(x1, y, z0),
                f.p(x1, y, z1),
                f.p(x0, y, z1),
            ],
            V3::Y,
            paint,
        );
    };
    flat(m, cx - 0.6, cx + 0.6, 0.9, yard, walk);
    // Driveway up to the garage, or along the side away from the door.
    let right = door * 2 < fp.columns;
    let (x0, x1, z0) = match fp.garage {
        Some(g) => (g as f32 * bay + 0.1, (g + 1) as f32 * bay - 0.1, 0.0),
        None if right => (fp.len + 0.6, fp.len + 3.4, -7.0),
        None => (-3.4, -0.6, -7.0),
    };
    flat(m, x0, x1, z0, yard, drive);
    let forward = -V2::new(f.z.x, f.z.z);
    if rng.range(0, 256) < 110 {
        let p = f
            .p(
                (x0 + x1) * 0.5,
                0.0,
                if fp.garage.is_some() { 3.2 } else { -2.0 },
            )
            .xz();
        let color = rng.range(0, 8) as u8;
        props.push(Prop {
            pos: p,
            y: LOT_Y,
            dir: if rng.range(0, 2) == 0 {
                forward
            } else {
                -forward
            },
            kind: PropKind::ParkedCar { color },
            seed: rng.next_u32(),
        });
    }
    if yard > 4.5 && rng.range(0, 256) < 90 {
        let (lo, hi) = if x0 > cx {
            (0.2, cx - 2.6)
        } else {
            (cx + 2.6, fp.len - 0.2)
        };
        if hi > lo {
            let x = rng.range_f32(lo, hi);
            props.push(Prop {
                pos: f.p(x, 0.0, yard * 0.5).xz(),
                y: LOT_Y,
                dir: V2::new(1.0, 0.0),
                kind: if rng.range(0, 4) == 0 {
                    PropKind::PalmTree
                } else {
                    PropKind::Oak
                },
                seed: rng.next_u32(),
            });
        }
    }
    props
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env(n: usize) -> Vec<FacadeEnv> {
        (0..n)
            .map(|i| FacadeEnv {
                road_dist: if i == 1 { 14.0 } else { 40.0 },
                yard: if i == 1 { 6.5 } else { 30.0 },
                blocked: false,
                frontage: false,
            })
            .collect()
    }

    fn house(floors: u8) -> Building {
        Building {
            // 12 m along x, 9 m along z.
            footprint: alloc::vec![[0.0, 0.0], [0.0, 9.0], [12.0, 9.0], [12.0, 0.0]],
            height_m: f32::from(floors) * 3.1 + 0.8,
            floors,
            roof: Roof::Flat,
            style: StyleTag::StuccoBox,
            built_year: None,
            usage: BuildingUse::Residential,
        }
    }

    #[test]
    fn window_insets_match_the_floor_grid() {
        let b = house(2);
        let plan = plan_building(&b, ZoneId::Strip, &env(4), 1, &crate::grammar::coast_80s());
        // The 12 m street front gets 3 bays of 3.6 m; the sides and back, with no road
        // within 30 m, get 1.5x wider bays: 1 on the 9 m sides, 2 on the 12 m back.
        let cols: Vec<u32> = plan.facades.iter().map(|f| f.columns).collect();
        assert_eq!(cols, [1, 3, 1, 2]);
        assert_eq!(plan.frontage, 1);
        // Odd door pick: garage at the far end, door in the first remaining bay.
        assert_eq!(plan.facades[1].garage, Some(2));
        assert_eq!(plan.facades[1].door, Some(0));
        assert!(plan.facades.iter().all(|f| f.window_rows == 2));
        // 2 floors x 7 bays, minus the door and garage bays.
        assert_eq!(plan.windows(), 12);
        let mut m = WorldMesh::default();
        let stats = build(&mut m, &b, &plan, &Palette::socal_1985(), 1, 0);
        assert_eq!(stats.windows, 12);
        assert!(stats.triangles <= BUILDING_TRIANGLE_BUDGET);
        // Each window's glass is a quad set back by the inset depth from its wall (the
        // footprint is the rectangle 0..12 x 0..9).
        let at_inset = |v: &&crate::mesh::WorldVertex| {
            let [x, _, z] = v.pos;
            let wall = x.min(12.0 - x).min(z).min(9.0 - z);
            v.color[3] == Pattern::Glass as u8 && (wall - WINDOW_INSET_M).abs() < 1e-3
        };
        assert_eq!(m.vertices.iter().filter(at_inset).count(), 12 * 4);
    }

    #[test]
    fn storefront_replaces_the_ground_floor_windows() {
        let mut b = house(3);
        b.usage = BuildingUse::Commercial;
        b.style = StyleTag::Brick;
        b.height_m = 3.0 * 3.7 + 1.4;
        let plan = plan_building(&b, ZoneId::Strip, &env(4), 0, &crate::grammar::coast_80s());
        let front = plan.facades[plan.frontage];
        assert!(front.storefront);
        assert_eq!(front.window_rows, 2);
        assert_eq!(front.door, None);
        // Ground floor taller than the upper floors.
        assert!(plan.floor_y[1] - plan.floor_y[0] > plan.floor_y[2] - plan.floor_y[1]);
        let mut m = WorldMesh::default();
        let stats = build(&mut m, &b, &plan, &Palette::socal_1985(), 3, 7);
        assert_eq!(stats.windows, plan.windows());
        assert!(m.vertices.iter().any(|v| v.emissive[3] == Glow::Neon as u8));
    }

    #[test]
    fn big_buildings_widen_bays_to_fit_the_budget() {
        let b = Building {
            footprint: alloc::vec![[0.0, 0.0], [0.0, 30.0], [28.0, 30.0], [28.0, 0.0]],
            height_m: 7.0 * 3.7 + 1.4,
            floors: 7,
            roof: Roof::Flat,
            style: StyleTag::Deco,
            built_year: None,
            usage: BuildingUse::Office,
        };
        let plan = plan_building(&b, ZoneId::Strip, &env(4), 0, &crate::grammar::coast_80s());
        let mut m = WorldMesh::default();
        let stats = build(&mut m, &b, &plan, &Palette::socal_1985(), 5, 2);
        assert_eq!(stats.windows, plan.windows());
        assert!(
            stats.triangles <= BUILDING_TRIANGLE_BUDGET,
            "{}",
            stats.triangles
        );
        assert!(stats.windows > 40);
    }

    #[test]
    fn new_styles_fit_their_budgets() {
        let mut worst = Vec::new();
        for (style, usage, floors, w, d) in [
            (StyleTag::Deco, BuildingUse::Office, 13u8, 32.0f32, 30.0f32),
            (StyleTag::Brick, BuildingUse::Office, 12, 22.0, 40.0),
            (StyleTag::StuccoBox, BuildingUse::Office, 9, 25.0, 25.0),
            (StyleTag::Glass, BuildingUse::Office, 30, 36.0, 30.0),
            (StyleTag::Glass, BuildingUse::Office, 6, 60.0, 30.0),
            (StyleTag::Warehouse, BuildingUse::Industrial, 1, 80.0, 40.0),
            (StyleTag::Motel, BuildingUse::Hotel, 2, 60.0, 10.0),
            (StyleTag::Dingbat, BuildingUse::Residential, 3, 18.0, 20.0),
            (StyleTag::Craftsman, BuildingUse::Residential, 1, 11.0, 16.0),
            (StyleTag::Ranch, BuildingUse::Residential, 1, 24.0, 14.0),
            (StyleTag::Tract70s, BuildingUse::Residential, 2, 16.0, 14.0),
            (StyleTag::Barn, BuildingUse::Industrial, 1, 14.0, 22.0),
        ] {
            let b = Building {
                footprint: alloc::vec![[0.0, 0.0], [0.0, d], [w, d], [w, 0.0]],
                height_m: f32::from(floors) * 3.6 + 1.5,
                floors,
                roof: if usage == BuildingUse::Residential || style == StyleTag::Barn {
                    Roof::Gable
                } else {
                    Roof::Flat
                },
                style,
                built_year: None,
                usage,
            };
            let plan = plan_building(&b, ZoneId::Strip, &env(4), 3, &crate::grammar::coast_80s());
            let mut m = WorldMesh::default();
            let stats = build(&mut m, &b, &plan, &Palette::socal_1985(), 5, 9);
            assert_eq!(stats.windows, plan.windows(), "{style:?}");
            let cap = if usage == BuildingUse::Residential {
                1200
            } else {
                BUILDING_TRIANGLE_BUDGET
            };
            assert!(stats.triangles <= cap, "{style:?}: {}", stats.triangles);
            worst.push((style, stats.triangles));
        }
        std::println!("{worst:?}");
        // Tall brick and deco step back.
        let tall = Building {
            footprint: alloc::vec![[0.0, 0.0], [0.0, 30.0], [32.0, 30.0], [32.0, 0.0]],
            height_m: 12.0 * 3.6 + 1.5,
            floors: 12,
            roof: Roof::Flat,
            style: StyleTag::Deco,
            built_year: None,
            usage: BuildingUse::Office,
        };
        let plan = plan_building(
            &tall,
            ZoneId::Strip,
            &env(4),
            0,
            &crate::grammar::coast_80s(),
        );
        let t = plan.tower.as_ref().expect("a tower");
        assert_eq!(plan.floors() + t.plan.floors(), 12);
    }

    /// A commercial front `w` long and `d` deep, `floors` high.
    fn shop(style: StyleTag, usage: BuildingUse, floors: u8, w: f32, d: f32) -> Building {
        Building {
            footprint: alloc::vec![[0.0, 0.0], [0.0, d], [w, d], [w, 0.0]],
            height_m: f32::from(floors) * 3.6 + 1.6,
            floors,
            roof: Roof::Flat,
            style,
            built_year: None,
            usage,
        }
    }

    #[test]
    fn upper_windows_line_up_over_the_piers() {
        let g = crate::grammar::coast_80s();
        let mut checked = 0;
        for pick in 0..64 {
            for style in [StyleTag::StuccoBox, StyleTag::Brick, StyleTag::Deco] {
                let b = shop(
                    style,
                    BuildingUse::Commercial,
                    3,
                    14.0 + pick as f32 * 0.5,
                    12.0,
                );
                let plan = plan_building(&b, ZoneId::Strip, &env(4), pick, &g);
                let f = &plan.facades[plan.frontage];
                let bp = f.base.expect("a commercial front has a base band");
                assert!(bp.bays >= 1 && bp.bays as usize <= MAX_BAYS);
                // Whole window bays per base bay: every pier line is a column line, and
                // no window crosses one.
                assert_eq!(f.columns % bp.bays, 0, "{style:?}");
                let per = f.columns / bp.bays;
                let (span, col) = (f.len / bp.bays as f32, f.len / f.columns as f32);
                for c in 0..f.columns {
                    let (x0, x1) =
                        opening(plan.window_kind, c, f.columns, col, f.len, plan.window_w);
                    let bay = c / per;
                    let (p0, p1) = (bay as f32 * span, (bay + 1) as f32 * span);
                    assert!(x0 >= p0 - 1e-3 && x1 <= p1 + 1e-3, "{style:?} column {c}");
                }
                assert_eq!(f.window_rows, 2);
                checked += 1;
            }
        }
        assert_eq!(checked, 192);
    }

    #[test]
    fn base_bays_follow_the_rules() {
        let g = crate::grammar::coast_80s();
        let (mut lobbies, mut doors, mut recessed, mut flush) = (0, 0, 0, 0);
        for pick in 0..200 {
            for (style, usage, floors) in [
                (StyleTag::StuccoBox, BuildingUse::Office, 4u8),
                (StyleTag::Deco, BuildingUse::Office, 1),
                (StyleTag::Brick, BuildingUse::Commercial, 3),
                (StyleTag::StuccoBox, BuildingUse::Commercial, 1),
            ] {
                let b = shop(style, usage, floors, 40.0, 20.0);
                let plan = plan_building(&b, ZoneId::Downtown, &env(4), pick, &g);
                let bp = plan.facades[plan.frontage].base.expect("base");
                let k = bp.kinds();
                let count = |kind| k.iter().filter(|&&x| x == kind).count();
                assert!(count(BayKind::Lobby) <= 1);
                assert!(count(BayKind::Door) <= usize::from(floors > 1));
                if count(BayKind::Lobby) == 1 {
                    assert_eq!(k[k.len() / 2], BayKind::Lobby);
                    lobbies += 1;
                }
                doors += count(BayKind::Door);
                assert!(k.iter().any(|&x| x != BayKind::Wall), "no way in");
                let s = &g.style(style, ZoneId::Downtown).base.storefront;
                assert!(bp.bulkhead_h >= s.bulkhead_h.0 && bp.bulkhead_h <= s.bulkhead_h.1);
                if bp.recess > 0.0 {
                    recessed += 1;
                } else {
                    flush += 1;
                }
                if style == StyleTag::Brick {
                    continue;
                }
                // Residential and pitched buildings have no base band.
                let mut house = b.clone();
                house.usage = BuildingUse::Residential;
                let plan = plan_building(&house, ZoneId::Downtown, &env(4), pick, &g);
                assert!(plan.facades.iter().all(|f| f.base.is_none()));
            }
        }
        assert!(lobbies > 50 && doors > 50 && recessed > 50 && flush > 50);
        // Industrial brick: loading doors, never a shop.
        let b = shop(StyleTag::Brick, BuildingUse::Industrial, 2, 30.0, 20.0);
        for pick in 0..50 {
            let plan = plan_building(&b, ZoneId::Industrial, &env(4), pick, &g);
            let bp = plan.facades[plan.frontage].base.expect("base");
            assert!(!bp.kinds().contains(&BayKind::Storefront));
            assert!(!plan.facades[plan.frontage].storefront);
        }
    }

    #[test]
    fn banded_buildings_stay_within_the_ceiling() {
        let g = crate::grammar::coast_80s();
        let mut worst = 0;
        for pick in 0..40u32 {
            for (style, usage, floors, w, d) in [
                (
                    StyleTag::StuccoBox,
                    BuildingUse::Commercial,
                    2u8,
                    48.0f32,
                    20.0f32,
                ),
                (StyleTag::Brick, BuildingUse::Commercial, 4, 40.0, 30.0),
                (StyleTag::Deco, BuildingUse::Office, 7, 36.0, 30.0),
                (StyleTag::Deco, BuildingUse::Office, 12, 32.0, 30.0),
                (StyleTag::Brick, BuildingUse::Office, 12, 22.0, 40.0),
                (StyleTag::StuccoBox, BuildingUse::Hotel, 9, 25.0, 25.0),
            ] {
                let b = shop(style, usage, floors, w, d);
                let mut plan = plan_building(&b, ZoneId::Downtown, &env(4), pick, &g);
                plan.signage = Some(signage(u64::from(pick), pick, &g.signs));
                let mut m = WorldMesh::default();
                let stats = build(&mut m, &b, &plan, &Palette::socal_1985(), 5, pick);
                assert_eq!(stats.windows, plan.windows(), "{style:?}");
                assert!(
                    stats.triangles <= BUILDING_TRIANGLE_BUDGET,
                    "{style:?} {floors}: {}",
                    stats.triangles
                );
                worst = worst.max(stats.triangles);
                // Without a base band the plain target holds.
                let mut plain = b.clone();
                plain.style = StyleTag::Glass;
                let plan = plan_building(&plain, ZoneId::Downtown, &env(4), pick, &g);
                let stats = build(&mut m, &plain, &plan, &Palette::socal_1985(), 5, pick);
                assert!(
                    stats.triangles <= PLAIN_TRIANGLE_TARGET,
                    "{}",
                    stats.triangles
                );
            }
        }
        std::println!("worst banded building: {worst} triangles");
    }

    #[test]
    fn blocked_facades_are_blank() {
        let b = house(1);
        let mut e = env(4);
        e[0].blocked = true;
        let plan = plan_building(&b, ZoneId::Strip, &e, 0, &crate::grammar::coast_80s());
        assert_eq!(plan.facades[0].columns, 0);
        assert_eq!(plan.facades[0].windows(), 0);
    }

    /// Signage with awning `awning`, strip-mall canopy `canopy`, open late.
    fn lit_shop(awning: Awning, canopy: Awning) -> Signage {
        Signage {
            name_seed: 7,
            awning: awning as u8,
            canopy: canopy as u8,
            lit_box: false,
            shop_lit: true,
        }
    }

    /// Underside faces glowing with the shop's spill.
    fn lit_undersides(m: &WorldMesh) -> usize {
        m.vertices
            .iter()
            .filter(|v| v.emissive[3] == Glow::Window as u8 && v.normal[1] < -0.9)
            .count()
    }

    #[test]
    fn every_awning_fits_and_lit_shops_light_its_underside() {
        let g = crate::grammar::coast_80s();
        let pal = Palette::socal_1985();
        let b = shop(StyleTag::StuccoBox, BuildingUse::Commercial, 2, 30.0, 18.0);
        for awning in [
            Awning::Striped,
            Awning::Solid,
            Awning::Canopy,
            Awning::None,
            Awning::Bullnose,
            Awning::Aluminum,
            Awning::Mansard,
        ] {
            for pick in 0..12u32 {
                let mut plan = plan_building(&b, ZoneId::Strip, &env(4), pick, &g);
                if !plan.facades[plan.frontage].storefront {
                    continue;
                }
                let mut s = lit_shop(awning, Awning::Canopy);
                for lit in [true, false] {
                    s.shop_lit = lit;
                    plan.signage = Some(s);
                    let mut m = WorldMesh::default();
                    let stats = build(&mut m, &b, &plan, &pal, 5, pick);
                    assert!(stats.triangles <= BUILDING_TRIANGLE_BUDGET, "{awning:?}");
                    let glow = lit_undersides(&m);
                    assert_eq!(glow > 0, lit && awning != Awning::None, "{awning:?} {lit}");
                }
            }
        }
    }

    #[test]
    fn strip_mall_canopies_and_their_rooftops() {
        let g = crate::grammar::coast_80s();
        let pal = Palette::socal_1985();
        let b = Building {
            height_m: 5.4,
            ..shop(StyleTag::StripMall, BuildingUse::Commercial, 1, 66.0, 22.0)
        };
        for canopy in [Awning::Mansard, Awning::Canopy] {
            for pick in 0..8u32 {
                let mut plan = plan_building(&b, ZoneId::Strip, &env(4), pick, &g);
                plan.signage = Some(lit_shop(Awning::None, canopy));
                // 1452 m² at 2-4 units and 3-6 skylights per 1000 m².
                assert!((2..=6).contains(&plan.roof.units), "{:?}", plan.roof);
                assert!((4..=MAX_SKYLIGHTS).contains(&plan.roof.skylights));
                let mut m = WorldMesh::default();
                let stats = build(&mut m, &b, &plan, &pal, 5, pick);
                assert!(
                    stats.triangles <= BUILDING_TRIANGLE_BUDGET,
                    "{}",
                    stats.triangles
                );
                assert!(lit_undersides(&m) > 0);
                // The parapet hides everything on the roof: nothing stands above its
                // coping.
                let top = m.vertices.iter().map(|v| v.pos[1]).fold(f32::MIN, f32::max);
                assert!(top <= plan.top + 1e-3, "{top} over {}", plan.top);
                // The mansard reaches 2-3 m out in front of the shops.
                let front = m.vertices.iter().map(|v| v.pos[2]).fold(f32::MIN, f32::max);
                let reach = front - 22.0;
                if canopy == Awning::Mansard {
                    assert!((2.0..=3.1).contains(&reach), "{reach}");
                } else {
                    assert!((3.1..=3.3).contains(&reach), "{reach}");
                }
            }
        }
    }

    #[test]
    fn roof_units_follow_the_roof_area() {
        let g = crate::grammar::coast_80s();
        let rules = g.style(StyleTag::StuccoBox, ZoneId::Strip);
        let small = shop(StyleTag::StuccoBox, BuildingUse::Commercial, 2, 12.0, 15.0);
        let big = shop(StyleTag::StuccoBox, BuildingUse::Commercial, 2, 40.0, 40.0);
        let flats = shop(StyleTag::StuccoBox, BuildingUse::Residential, 2, 40.0, 40.0);
        let (mut a, mut c) = (0u32, 0u32);
        for pick in 0..64 {
            let (s, l) = (plan_roof(&small, rules, pick), plan_roof(&big, rules, pick));
            // One unit at least on any commercial roof, 2-4 per 1000 m² on a big one.
            assert!(
                s.units >= 1 && (3..=MAX_ROOF_UNITS).contains(&l.units),
                "{l:?}"
            );
            assert_eq!((s.skylights, l.skylights), (0, 0));
            a += u32::from(s.units);
            c += u32::from(l.units);
            let r = plan_roof(&flats, rules, pick);
            assert_eq!((r.units, r.fans), (0, 0));
        }
        assert!(c > 3 * a, "{a} {c}");
    }

    #[test]
    fn top_kinds_build_where_they_belong() {
        let pal = Palette::socal_1985();
        for kind in TopKind::ALL {
            let mut g = crate::grammar::coast_80s();
            let mut w = [0u16; 7];
            w[kind as usize] = 1;
            for r in &mut g.styles {
                let t = &mut r.top.kinds;
                [
                    t.parapet,
                    t.cornice,
                    t.stepped,
                    t.shaped,
                    t.false_front,
                    t.bracketed,
                    t.mansard,
                ] = w;
            }
            for floors in [1u8, 3] {
                let b = shop(StyleTag::Brick, BuildingUse::Commercial, floors, 24.0, 20.0);
                let mut plan = plan_building(&b, ZoneId::Strip, &env(4), 3, &g);
                let want = match (kind, floors) {
                    (TopKind::FalseFront, 3) | (TopKind::Mansard, 1) => TopKind::Parapet,
                    (TopKind::Bracketed, 1) => TopKind::Cornice,
                    _ => kind,
                };
                assert_eq!(plan.top_kind, want);
                plan.signage = Some(lit_shop(Awning::Striped, Awning::Canopy));
                let mut m = WorldMesh::default();
                let stats = build(&mut m, &b, &plan, &pal, 5, 3);
                assert!(stats.triangles <= BUILDING_TRIANGLE_BUDGET, "{kind:?}");
                let top = m.vertices.iter().map(|v| v.pos[1]).fold(f32::MIN, f32::max);
                // Stepped, shaped and false fronts rise over the coping.
                let rises = matches!(
                    want,
                    TopKind::Stepped | TopKind::Shaped | TopKind::FalseFront
                );
                assert_eq!(top > plan.top + 1.0, rises, "{kind:?} {floors}: {top}");
            }
        }
    }
}
