//! Building meshes with facade geometry: plinth, inset windows with sills on a floor grid,
//! storefront bays with doors, awnings and sign bands, cornices and parapets, roof clutter.
//!
//! Each building is first *planned* ([`plan_building`]: floor heights, window grid per
//! facade, where the storefront and door go), then *built* from the plan. The plan is pure
//! data, so tests can check the mesh against it.

use alloc::vec::Vec;

use place::{Building, BuildingUse, Roof, StyleTag};
use skywave_core::{Rng, Seed};

use crate::math::{abs, clamp, floor, offset_polygon, V2, V3};
use crate::mesh::{
    boxf, face, frustum, polygon_cap, quad, rect, ring_band, tri, Frame, Glow, Paint, Pattern,
    WorldMesh,
};
use crate::palette::{mix, shade, Palette, Rgb};
use crate::props::{Prop, PropKind};
use crate::LOT_Y;

/// Depth of window insets.
pub const WINDOW_INSET_M: f32 = 0.15;
/// Depth of the storefront recess.
const STOREFRONT_INSET_M: f32 = 0.25;
/// Triangle budget per building; window bays widen until the estimate fits.
pub const BUILDING_TRIANGLE_BUDGET: usize = 2000;
/// Share of windows lit (emissive), out of 256.
const LIT_WINDOWS_256: u32 = 102;
/// Estimated triangles per window (glass, reveals, sill, surrounding wall pieces).
const TRIS_PER_WINDOW: usize = 20;
/// Estimated triangles of everything but windows; generous so the budget holds.
const TRIS_FIXED: usize = 560;
/// Rng key for per-building detail.
const KEY_BUILDING: u64 = 0x4255_494C;

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
    /// Floors with a row of windows (the storefront floor has none).
    pub window_rows: u32,
    /// The ground floor is a storefront.
    pub storefront: bool,
    /// Ground-floor bay replaced by the entrance door.
    pub door: Option<u32>,
    /// Ground-floor bay replaced by a garage door (houses).
    pub garage: Option<u32>,
}

impl FacadePlan {
    /// Window insets this facade gets.
    pub fn windows(&self) -> u32 {
        let openings = u32::from(self.door.is_some()) + u32::from(self.garage.is_some());
        let ground = if !self.storefront && self.window_rows > 0 {
            openings
        } else {
            0
        };
        self.columns * self.window_rows - ground
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
}

impl BuildingPlan {
    pub fn floors(&self) -> usize {
        self.floor_y.len() - 1
    }

    pub fn windows(&self) -> u32 {
        self.facades.iter().map(FacadePlan::windows).sum()
    }

    /// Sill height of the windows on floor `k`.
    fn sill(&self, k: usize) -> f32 {
        if k == 0 {
            self.plinth_top + 0.65
        } else {
            self.floor_y[k] + 0.9
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

fn parapet(b: &Building) -> f32 {
    if is_pitched(b) {
        return 0.0;
    }
    match (b.style, b.usage) {
        (StyleTag::StuccoBox, BuildingUse::Residential) => 0.5,
        (StyleTag::StuccoBox, _) => 0.8,
        (StyleTag::StripMall, _) => 1.8,
        // Brick, Deco and the styles that have no look of their own yet (see `look`).
        _ => 1.0,
    }
}

/// Decides floors, the window grid of every facade, and the storefront and door.
/// `door_pick` chooses the door bay on houses (any number; it is reduced modulo the bays).
pub fn plan_building(b: &Building, env: &[FacadeEnv], door_pick: u32) -> BuildingPlan {
    let fp = &b.footprint;
    let n = fp.len();
    let floors = usize::from(b.floors.clamp(1, 40));
    let residential = b.usage == BuildingUse::Residential;
    let top = b.height_m;
    let roof_y = top - parapet(b).min(top * 0.3);
    let plinth = match (residential, b.style) {
        (true, _) => 0.12,
        (_, StyleTag::StripMall) => 0.25,
        _ => 0.45,
    };
    let plinth_top = LOT_Y + plinth;
    let span = roof_y - LOT_Y;
    let (ground_h, upper_h) = if residential {
        (span / floors as f32, span / floors as f32)
    } else if floors == 1 {
        (span, span)
    } else {
        let g = clamp(span - (floors - 1) as f32 * 3.3, 3.8, 4.8).min(span * 0.6);
        (g, (span - g) / (floors - 1) as f32)
    };
    let mut floor_y = Vec::with_capacity(floors + 1);
    floor_y.push(LOT_Y);
    for k in 1..=floors {
        floor_y.push(LOT_Y + ground_h + (k - 1) as f32 * upper_h);
    }
    floor_y[floors] = roof_y;

    let bay = match (b.usage, b.style) {
        (BuildingUse::Residential, _) => 3.6,
        (_, StyleTag::Brick) => 2.7,
        (BuildingUse::Office, _) => 2.8,
        _ => 3.0,
    };
    let (window_w, window_h) = match (residential, b.style) {
        (true, _) => (1.3, (upper_h - 1.35).min(1.3)),
        (false, StyleTag::Brick) => (1.1, (upper_h - 1.3).min(1.8)),
        (false, StyleTag::Deco) => (bay * 0.62, (upper_h - 1.45).min(1.5)),
        _ => (clamp(bay * 0.5, 0.9, 1.6), (upper_h - 1.3).min(1.7)),
    };
    let (door_w, door_h) = if residential { (1.0, 2.2) } else { (2.0, 2.6) };

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
    let storefront_use = b.usage == BuildingUse::Commercial && b.style != StyleTag::StripMall;

    let columns_for = |i: usize, bay: f32| -> u32 {
        let l = len(i);
        if env[i].blocked || l < 2.4 || b.style == StyleTag::StripMall {
            0
        } else {
            (floor((l - 0.8) / bay) as u32).max(1)
        }
    };
    let make = |bays: &[f32]| -> Vec<FacadePlan> {
        (0..n)
            .map(|i| {
                let columns = columns_for(i, bays[i]);
                let storefront = i == frontage && storefront_use && len(i) >= 4.0;
                let window_rows = if columns == 0 {
                    0
                } else {
                    floors as u32 - u32::from(storefront)
                };
                let front = i == frontage && !storefront && columns > 0;
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
                FacadePlan {
                    len: len(i),
                    columns,
                    window_rows,
                    storefront,
                    door,
                    garage,
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
    let mut facades = make(&bays);
    let per_floor = if b.style == StyleTag::Brick { 64 } else { 40 };
    let estimate = |fs: &[FacadePlan]| -> usize {
        TRIS_FIXED
            + floors * per_floor
            + fs.iter()
                .map(|f| f.windows() as usize * TRIS_PER_WINDOW + 24)
                .sum::<usize>()
    };
    let mut round = 0;
    while estimate(&facades) > BUILDING_TRIANGLE_BUDGET && round < 40 {
        for (i, b) in bays.iter_mut().enumerate() {
            if round < 6 && env[i].road_dist < 25.0 {
                continue;
            }
            *b *= 1.1;
        }
        facades = make(&bays);
        round += 1;
    }
    BuildingPlan {
        plinth_top,
        roof_y,
        top,
        floor_y,
        facades,
        frontage,
        window_w,
        window_h: window_h.max(0.6),
        door_w,
        door_h,
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
}

fn choose<T: Copy>(rng: &mut Rng, items: &[T]) -> T {
    items[rng.range(0, items.len() as u32) as usize]
}

fn look(b: &Building, pal: &Palette, rng: &mut Rng) -> Look {
    // Styles without a look of their own yet borrow the closest one.
    let (wall_rgb, pattern, trim_rgb) = match b.style {
        StyleTag::StuccoBox
        | StyleTag::Dingbat
        | StyleTag::Craftsman
        | StyleTag::Ranch
        | StyleTag::Tract70s
        | StyleTag::Motel => {
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
        StyleTag::Brick | StyleTag::Warehouse | StyleTag::Barn => (
            choose(rng, &[pal.brick_red, pal.brick_brown]),
            Pattern::Brick,
            choose(rng, &[pal.stucco_cream, pal.concrete]),
        ),
        StyleTag::Deco | StyleTag::Glass => (
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
    let start = m.indices.len();
    let mut rng = Rng::new(Seed(seed), &[KEY_BUILDING, u64::from(index)]);
    let look = look(b, pal, &mut rng);
    let fp: Vec<V2> = b.footprint.iter().map(|&p| V2::from_array(p)).collect();
    let n = fp.len();
    let mut stats = BuildingStats::default();

    // Plinth from below the lot surface.
    ring_band(
        m,
        &fp,
        0.06,
        LOT_Y - 0.1,
        plan.plinth_top,
        look.plinth,
        Some(look.plinth),
        None,
    );

    for i in 0..n {
        let (a, bb) = (fp[i], fp[(i + 1) % n]);
        let dir = (bb - a).normalize();
        let f = Frame::upright(a.at(0.0), dir);
        facade(
            m,
            &f,
            plan,
            &plan.facades[i],
            i == plan.frontage,
            b,
            &look,
            pal,
            &mut rng,
            &mut stats,
        );
    }

    if is_pitched(b) {
        pitched_roof(m, b, &fp, plan.top, &look, pal, &mut rng);
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

    style_details(m, b, plan, &fp, &look);
    roof_clutter(m, b, plan, &inner, &look, pal, &mut rng);

    stats.triangles = (m.indices.len() - start) / 3;
    stats
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
        if k == 0 && fp.storefront {
            storefront(
                m,
                f,
                len,
                y0,
                y1,
                look,
                rng,
                b.usage != BuildingUse::Residential,
            );
            continue;
        }
        if fp.columns == 0 {
            rect(m, f, 0.0, y0, len, y1, 0.0, false, look.wall);
            continue;
        }
        let bay = len / fp.columns as f32;
        let sill = plan.sill(k);
        let head = (sill + plan.window_h).min(y1 - 0.25);
        let door_floor = k == 0 && fp.door.is_some();
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
            let w = if is_garage {
                bay - 0.6
            } else if is_door {
                plan.door_w
            } else {
                plan.window_w
            }
            .min(bay - 0.5);
            let (x0, x1) = (cx - w * 0.5, cx + w * 0.5);
            if door_floor {
                rect(m, f, x, y0, x0, y1, 0.0, false, look.wall);
            } else {
                rect(m, f, x, sill, x0, head, 0.0, false, look.wall);
            }
            if is_garage {
                let top = (y0 + 2.1).min(y1 - 0.2);
                garage_door(m, f, x0, x1, y0, top, look);
                rect(m, f, x0, top, x1, y1, 0.0, false, look.wall);
            } else if is_door {
                let top = (y0 + plan.door_h).min(y1 - 0.2);
                door(m, f, x0, x1, y0, top, plan, b, look);
                rect(m, f, x0, top, x1, y1, 0.0, false, look.wall);
            } else {
                if door_floor {
                    rect(m, f, x0, y0, x1, sill, 0.0, false, look.wall);
                    rect(m, f, x0, head, x1, y1, 0.0, false, look.wall);
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
                window(m, f, x0, sill, x1, head, pane, k == 0, look);
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
                stats.windows += 1;
                stats.lit_windows += u32::from(lit);
            }
            x = x1;
        }
        if door_floor {
            rect(m, f, x, y0, len, y1, 0.0, false, look.wall);
        } else {
            rect(m, f, x, sill, len, head, 0.0, false, look.wall);
        }
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
    b: &Building,
    look: &Look,
) {
    let d = 0.18;
    inset(m, f, x0, y0, x1, y1, d, false, look.reveal);
    let residential = b.usage == BuildingUse::Residential;
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

/// Ground-floor shopfront: recessed glazing with mullions and a door, an awning, and a
/// sign band with glowing letters above.
#[allow(clippy::too_many_arguments)]
fn storefront(
    m: &mut WorldMesh,
    f: &Frame,
    len: f32,
    y0: f32,
    y1: f32,
    look: &Look,
    rng: &mut Rng,
    signs: bool,
) {
    let margin = 0.5;
    let (x0, x1) = (margin, len - margin);
    let glass_top = (y0 + 2.6).min(y1 - 0.9);
    let d = STOREFRONT_INSET_M;
    // Piers and the wall above the opening.
    rect(m, f, 0.0, y0, x0, y1, 0.0, false, look.wall);
    rect(m, f, x1, y0, len, y1, 0.0, false, look.wall);
    rect(m, f, x0, glass_top, x1, y1, 0.0, false, look.wall);
    inset(m, f, x0, y0, x1, glass_top, d, true, look.reveal);
    // Door near one end (or the middle on narrow fronts), glass elsewhere.
    let door_w = 1.1;
    let door_x = if x1 - x0 < 5.0 {
        (x0 + x1 - door_w) * 0.5
    } else if rng.range(0, 2) == 0 {
        x0 + 0.9
    } else {
        x1 - 0.9 - door_w
    };
    let door_top = (y0 + 2.25).min(glass_top);
    let back = -d;
    rect(
        m,
        f,
        x0,
        y0,
        door_x,
        glass_top,
        back,
        false,
        look.shop_glass,
    );
    rect(
        m,
        f,
        door_x + door_w,
        y0,
        x1,
        glass_top,
        back,
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
        back,
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
        back,
        false,
        look.frame,
    );
    rect(
        m,
        f,
        door_x + 0.1,
        y0 + 0.12,
        door_x + door_w - 0.1,
        door_top - 0.1,
        back + 0.01,
        false,
        look.shop_glass,
    );
    // Mullions every ~2 m, skipping the door, and a transom bar.
    let panes = ((x1 - x0) / 2.0) as u32;
    for k in 1..panes {
        let xm = x0 + (x1 - x0) * k as f32 / panes as f32;
        if xm > door_x - 0.2 && xm < door_x + door_w + 0.2 {
            continue;
        }
        boxf(
            m,
            f,
            [xm - 0.05, y0, back],
            [xm + 0.05, glass_top, back + 0.12],
            face::NEG_X | face::POS_X | face::POS_Z,
            look.frame,
        );
    }
    boxf(
        m,
        f,
        [x0, door_top, back],
        [x1, door_top + 0.08, back + 0.1],
        face::POS_Y | face::NEG_Y | face::POS_Z,
        look.frame,
    );
    // Stoop in front of the door when the plinth is high.
    if y0 - LOT_Y > 0.05 {
        boxf(
            m,
            f,
            [door_x - 0.2, LOT_Y - 0.05, 0.0],
            [door_x + door_w + 0.2, y0, 0.6],
            face::NO_BOTTOM & !face::NEG_Z,
            look.plinth,
        );
    }
    awning(m, f, x0 - 0.2, x1 + 0.2, glass_top + 0.15, 1.4, 0.6, look);
    if signs {
        let sy0 = glass_top + 0.4;
        let sy1 = (sy0 + 0.8).min(y1 - 0.15);
        if sy1 - sy0 > 0.35 {
            sign_band(m, f, 0.8, len - 0.8, sy0, sy1, look, rng);
        }
    }
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
    let stripes = (((x1 - x0) / 0.55) as u32).max(1);
    let w = (x1 - x0) / stripes as f32;
    let (yf, val) = (y - drop, 0.28);
    for k in 0..stripes {
        let (a, b) = (x0 + w * k as f32, x0 + w * (k + 1) as f32);
        let paint = look.awning[(k % 2) as usize];
        let under = paint.with_rgb(shade(paint.rgb(), 150));
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

/// The strip mall's front: a row of shop units under one deep canopy on posts, signs on
/// the fascia above.
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
    for u in 0..units {
        let ux = unit * u as f32;
        let sub = Frame {
            o: f.p(ux, 0.0, 0.0),
            ..*f
        };
        let neon = choose(rng, &pal.neon());
        let unit_look = Look { neon, ..*look };
        storefront_unit(m, &sub, unit, y0, canopy_y, &unit_look, rng);
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
    rect(m, f, 0.0, canopy_y, len, plan.top, 0.0, false, look.wall);
    // Canopy slab with a fascia, on square posts.
    let depth = 3.2;
    boxf(
        m,
        f,
        [-0.3, canopy_y, 0.0],
        [len + 0.3, canopy_y + 0.4, depth],
        face::ALL & !face::NEG_Z,
        look.trim,
    );
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

/// Cornices, belt courses, speed lines, stepped parapets and fins by style.
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
            ring_band(
                m,
                fp,
                0.24,
                plan.roof_y - 0.2,
                plan.roof_y + 0.18,
                look.sill,
                Some(look.sill),
                Some(look.sill),
            );
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
            // Stepped parapet centered on the frontage.
            let i = plan.frontage;
            let (a, c) = (fp[i], fp[(i + 1) % n]);
            let len = (c - a).length();
            let f = Frame::upright(a.at(0.0), (c - a).normalize());
            let mid = len * 0.5;
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
        // No details of their own yet.
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

/// Vents, air conditioners, a stair bulkhead and a water tank on taller roofs.
fn roof_clutter(
    m: &mut WorldMesh,
    b: &Building,
    plan: &BuildingPlan,
    inner: &[V2],
    look: &Look,
    pal: &Palette,
    rng: &mut Rng,
) {
    let (mut lo, mut hi) = (V2::new(f32::MAX, f32::MAX), V2::new(f32::MIN, f32::MIN));
    for p in inner {
        lo = V2::new(lo.x.min(p.x), lo.y.min(p.y));
        hi = V2::new(hi.x.max(p.x), hi.y.max(p.y));
    }
    let size = hi - lo;
    if size.x < 3.0 || size.y < 3.0 {
        return;
    }
    let y = plan.roof_y;
    let metal = Paint::new(pal.metal_light, Pattern::Metal);
    let dark = Paint::new(pal.metal_dark, Pattern::Metal);
    let spot = |rng: &mut Rng, margin: f32| {
        let mx = margin.min(size.x * 0.4);
        let mz = margin.min(size.y * 0.4);
        V2::new(
            rng.range_f32(lo.x + mx, hi.x - mx),
            rng.range_f32(lo.y + mz, hi.y - mz),
        )
    };
    let residential = b.usage == BuildingUse::Residential;
    let vents = if residential {
        rng.range(1, 3)
    } else {
        rng.range(2, 5)
    };
    for _ in 0..vents {
        let p = spot(rng, 1.0);
        let f = Frame::at(p.at(y));
        if rng.range(0, 2) == 0 {
            boxf(
                m,
                &f,
                [-0.3, 0.0, -0.25],
                [0.3, 0.45, 0.25],
                face::NO_BOTTOM,
                metal,
            );
        } else {
            frustum(m, &f, 0.12, 0.12, 0.6, 6, true, false, dark);
        }
    }
    if !residential {
        let units = rng.range(1, 3);
        for _ in 0..units {
            let p = spot(rng, 2.0);
            let f = Frame::at(p.at(y));
            boxf(
                m,
                &f,
                [-0.65, 0.0, -0.5],
                [0.65, 1.0, 0.5],
                face::NO_BOTTOM,
                metal,
            );
            frustum(
                m,
                &f.offset(0.0, 1.0, 0.0),
                0.4,
                0.4,
                0.02,
                8,
                true,
                false,
                dark,
            );
        }
    }
    if plan.floors() >= 4 {
        let p = spot(rng, 3.0);
        boxf(
            m,
            &Frame::at(p.at(y)),
            [-1.4, 0.0, -1.2],
            [1.4, 2.6, 1.2],
            face::NO_BOTTOM,
            look.parapet_in,
        );
        if rng.range(0, 2) == 0 && size.x > 9.0 && size.y > 9.0 {
            water_tank(m, spot(rng, 3.5).at(y), pal);
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
    let (overhang, pitch) = (0.5, 0.42);
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
        let plan = plan_building(&b, &env(4), 1);
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
        let plan = plan_building(&b, &env(4), 0);
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
            height_m: 8.0 * 3.7 + 1.4,
            floors: 8,
            roof: Roof::Flat,
            style: StyleTag::Deco,
            built_year: None,
            usage: BuildingUse::Office,
        };
        let plan = plan_building(&b, &env(4), 0);
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
    fn blocked_facades_are_blank() {
        let b = house(1);
        let mut e = env(4);
        e[0].blocked = true;
        let plan = plan_building(&b, &e, 0);
        assert_eq!(plan.facades[0].columns, 0);
        assert_eq!(plan.facades[0].windows(), 0);
    }
}
