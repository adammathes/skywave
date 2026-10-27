//! `ShapeSpec`: the few curves and choices a car body is lofted from, as plain data.
//!
//! Shape space: `x` is meters from the nose, growing rearward; `y` is meters above the
//! ground; widths are half-widths from the centerline. The loft is symmetric. RON struct
//! names are the Rust type names. Layout: `docs/VEHICLES.md` ("Parametric mesh").

use alloc::string::String;
use alloc::vec::Vec;

/// Body cross-section controls.
#[derive(Clone, Copy, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(deny_unknown_fields))]
pub struct Section {
    /// Where the side turns into the top, as a fraction of the half-width, in `[0.6, 1]`:
    /// `1.0` is a sharp shoulder, lower is a rounder one.
    pub shoulder: f32,
    /// Inward lean of the side glass from vertical, degrees, in `[0, 40]`.
    pub tumblehome_deg: f32,
    /// Roof half-width as a fraction of the body half-width, in `[0.4, 1]`.
    pub greenhouse_frac: f32,
    /// How much higher the centerline is than the edges of the hood, roof and deck, m.
    pub crown: f32,
    /// How far the rocker tucks in under the side, m.
    pub tuck: f32,
}

/// Where the glass is, as `x` ranges along the car.
#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(deny_unknown_fields))]
pub struct Glass {
    pub windshield: (f32, f32),
    /// Rear window (on a wagon, the tailgate glass).
    pub backlight: (f32, f32),
    /// Side windows, front to back; the gaps between them are the B, C, ... pillars.
    pub side: Vec<(f32, f32)>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(deny_unknown_fields))]
pub struct Wheels {
    /// Tire outer radius, m.
    pub radius: f32,
    /// Tire width, m.
    pub width: f32,
    /// Gap between tire and arch, m.
    pub arch_clearance: f32,
    /// Axle positions along the car, m from the nose.
    pub front_x: f32,
    pub rear_x: f32,
    /// Distance between left and right wheel centers, m (`VehicleSpec::track_m`).
    pub track: f32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum HeadLights {
    /// Lids in the fender tops; closed unless built with `popups_up`.
    PopUp,
    /// One rectangular lamp per side.
    Rect,
    /// Two square sealed beams per side.
    Quad,
    /// One round sealed beam per side.
    Round,
    /// A slim strip behind the grille.
    Hidden,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum Grille {
    Slot,
    Egg,
    Wide,
    None,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum Bumper {
    Chrome,
    Body,
    Rubber,
}

#[derive(Clone, Copy, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(deny_unknown_fields))]
pub struct Front {
    pub lights: HeadLights,
    pub grille: Grille,
    pub bumper: Bumper,
    /// Bumper bottom and top above the ground, m.
    pub bumper_y: (f32, f32),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum TailLights {
    FullWidth,
    Dual,
    Triple,
    Round,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum Spoiler {
    None,
    Lip,
    Wing,
}

#[derive(Clone, Copy, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(deny_unknown_fields))]
pub struct Rear {
    pub lights: TailLights,
    pub spoiler: Spoiler,
    pub bumper: Bumper,
    pub bumper_y: (f32, f32),
}

/// Inset lines cut into the loft (6 mm deep).
#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(deny_unknown_fields))]
pub struct Seams {
    pub hood: bool,
    /// Trunk lid on the deck, or a tailgate outline on the tail when the deck is short.
    pub trunk: bool,
    /// 2 or 4.
    pub doors: u8,
    /// Hood (and trunk lid) half-width as a fraction of the top's half-width, in
    /// `[0.3, 0.95]`; the rest is fender.
    pub hood_width: f32,
    /// Side character lines `(x0, y0, x1, y1)` in side view; at most 2.
    pub creases: Vec<(f32, f32, f32, f32)>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum Mirrors {
    Door,
    Fender,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum Handles {
    Flush,
    Pull,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum Exhaust {
    Single,
    Dual,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum Antenna {
    Mast,
    None,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum Rims {
    Steel,
    Alloy5,
    Mesh,
    Turbine,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(deny_unknown_fields))]
pub struct Parts {
    pub mirrors: Mirrors,
    pub handles: Handles,
    /// Wiper blades, 0–3.
    pub wipers: u8,
    pub exhaust: Exhaust,
    pub antenna: Antenna,
    pub rims: Rims,
    /// Full wheel covers over the rims.
    pub hubcaps: bool,
    /// A white band on the outer sidewalls. Optional (default off).
    #[cfg_attr(feature = "serde", serde(default))]
    pub whitewalls: bool,
}

/// Generic emblem outlines (never a real maker's mark).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum BadgeShape {
    Diamond,
    Circle,
    Bar,
    Chevron,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum Detail {
    /// Slats over the rear window.
    Louvers,
    HoodScoop,
    /// Tape stripe along the sides in the accent color.
    SideStripe,
    /// Wood-grain side panels with a light surround.
    WoodPanel,
    /// Chrome belt molding and arch lips.
    ChromeTrim,
    /// Textured roof cover in the accent color.
    VinylRoof,
    /// Twin stripes over hood, roof and deck in the accent color.
    RallyStripes,
    MudFlaps,
    RoofRack,
    /// Black pillars, window frames and arch lips.
    BlackTrim,
    Badge(BadgeShape),
    /// A narrow wood-grain band along the doors and quarters with a light surround.
    WoodApplique,
    /// Clear lamps in the front fender sides, behind the headlamps.
    CorneringLamps,
}

/// Where a convertible's fabric top is.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum Top {
    /// Folded into the well behind the rear seat under a padded boot.
    #[default]
    Down,
    /// Raised over the cabin, with a plastic rear window.
    Up,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum RoofKind {
    /// A fixed roof over the greenhouse.
    #[default]
    Hardtop,
    /// No fixed roof: a framed windshield with A-pillars, a cabin open above the belt, no
    /// side or rear glass, and a fabric top that folds into the `glass.backlight` range
    /// (the boot when down; the top's rear window when up).
    Convertible { top: Top },
}

/// The roof over the cabin.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(deny_unknown_fields))]
pub struct Roof {
    pub kind: RoofKind,
}

impl Roof {
    /// Whether this is a convertible (top up or down).
    pub fn convertible(&self) -> bool {
        matches!(self.kind, RoofKind::Convertible { .. })
    }

    /// The convertible's top position, if it is one.
    pub fn top(&self) -> Option<Top> {
        match self.kind {
            RoofKind::Hardtop => None,
            RoofKind::Convertible { top } => Some(top),
        }
    }
}

/// The band across the dash face under the cluster and over the glovebox.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum Applique {
    /// Dash color, a shade darker.
    #[default]
    None,
    /// Wood grain.
    Wood,
    /// Brushed metal.
    Brushed,
}

/// Cockpit layout. `x` values are m from the nose, `y` above the ground.
#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(deny_unknown_fields))]
pub struct InteriorShape {
    /// Height of the dash top, m.
    pub dash_height: f32,
    /// Where the dash's driver-facing edge is.
    pub dash_x: f32,
    /// Steering wheel center `(x, y)`; it sits on the driver's centerline.
    pub wheel_pos: (f32, f32),
    pub wheel_radius: f32,
    /// Steering column angle above horizontal, degrees (the wheel's tilt from vertical).
    pub column_angle_deg: f32,
    /// Rear-view mirror center `(x, y)`.
    pub mirror_pos: (f32, f32),
    /// Driver hip point `(x, y)`.
    pub seat: (f32, f32),
    /// Distance from the car's centerline to the driver's (left-hand drive).
    pub driver_offset: f32,
    pub rear_seat: bool,
    /// Big dials in the cluster, 1–3.
    pub dials: u8,
    /// Dash, seats, carpet, headliner, trim, accent: `#rrggbb` sRGB.
    pub colors: Vec<String>,
    /// Radio, display and dial backlight glow, `#rrggbb`.
    pub display: String,
    /// Seat center insert (two-tone velour or vinyl), `#rrggbb`; empty: the seat color a
    /// shade lighter.
    #[cfg_attr(feature = "serde", serde(default))]
    pub insert: String,
    /// Seam piping on the seats and door cards, `#rrggbb`; empty: the seat color darker.
    #[cfg_attr(feature = "serde", serde(default))]
    pub piping: String,
    /// Dash top (the lighter upper pad), `#rrggbb`; empty: the dash color.
    #[cfg_attr(feature = "serde", serde(default))]
    pub dash_top: String,
    #[cfg_attr(feature = "serde", serde(default))]
    pub applique: Applique,
    /// A clutch pedal and a stick in a gate (default: an automatic's two pedals and a
    /// selector).
    #[cfg_attr(feature = "serde", serde(default))]
    pub manual: bool,
}

/// Everything the mesh generator needs for one car body.
#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(deny_unknown_fields))]
pub struct ShapeSpec {
    /// Matches the `VehicleSpec` id.
    pub id: String,
    /// Side silhouette `(x, y)`: along the top from the nose (`x = 0`) to the tail, then
    /// along the underside back to the nose. Top `x` strictly increases, the underside's
    /// strictly decreases; both chains start and end at the same `x`.
    pub side: Vec<(f32, f32)>,
    /// Top-down half outline `(x, half_width)`, `x` strictly increasing from 0 to the
    /// length.
    pub plan: Vec<(f32, f32)>,
    /// Beltline `(x, y)` where the glass starts, `x` strictly increasing; held flat beyond
    /// its ends.
    pub belt: Vec<(f32, f32)>,
    pub section: Section,
    pub glass: Glass,
    pub wheels: Wheels,
    pub front: Front,
    pub rear: Rear,
    pub seams: Seams,
    pub parts: Parts,
    pub details: Vec<Detail>,
    /// Extra details for single palette entries, `(index, detail)`: e.g. wood-grain trim
    /// on the second color only. Optional.
    #[cfg_attr(feature = "serde", serde(default))]
    pub color_details: Vec<(u32, Detail)>,
    /// Hardtop (the default when left out) or convertible.
    #[cfg_attr(feature = "serde", serde(default))]
    pub roof: Roof,
    /// Era body colors `#rrggbb` (sRGB), or `#upper/#lower` for two-tone paint split at the
    /// first crease (or halfway up the side) with a pinstripe in `accent`; one is chosen per
    /// instance.
    pub palette: Vec<String>,
    /// Stripe, vinyl and badge-field color, `#rrggbb`.
    pub accent: String,
    pub interior: InteriorShape,
}

/// Why [`ShapeSpec::validate`] rejected a spec; the first failure is reported.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShapeError {
    EmptyId,
    NonFinite,
    /// The side polyline needs 6–48 points.
    SidePointCount,
    /// The side polyline must start at `x = 0`.
    SideNotAtNose,
    /// Top chain point `index` does not have a larger `x` than the one before it.
    SideTopNotIncreasing {
        index: u32,
    },
    /// Underside point `index` (counted in the whole polyline) does not have a smaller `x`.
    SideBottomNotDecreasing {
        index: u32,
    },
    /// The underside does not start at the tail and end at the nose.
    SideEnds,
    /// The underside is not below the top somewhere.
    SideInverted,
    /// The plan outline needs 2–32 points.
    PlanPointCount,
    /// Plan point `index` does not have a larger `x` than the one before it.
    PlanNotIncreasing {
        index: u32,
    },
    /// The plan outline does not span `0..=length`.
    PlanSpan,
    NonPositiveWidth {
        index: u32,
    },
    BeltPointCount,
    BeltNotIncreasing {
        index: u32,
    },
    SectionOutOfRange,
    /// A glass range is empty, reversed or outside the car.
    GlassRange,
    /// Side window `index` overlaps the one before it or leaves the car.
    SideWindow {
        index: u32,
    },
    WheelSize,
    /// Axles out of order, outside the body or arches overlapping.
    WheelPlacement,
    /// The track puts the tires outside the body.
    TrackTooWide,
    BumperRange,
    DoorCount,
    HoodWidth,
    TooManyCreases,
    Crease {
        index: u32,
    },
    TooManyWipers,
    EmptyPalette,
    /// Palette entry `index` is not `#rrggbb` or `#rrggbb/#rrggbb`.
    BadColor {
        index: u32,
    },
    BadAccent,
    /// The cockpit does not fit in the cabin, or a cockpit color is bad.
    Interior,
    /// A convertible with side windows, a boot (`glass.backlight`) not between the rear
    /// seat and the deck, or a greenhouse behind the windshield header.
    OpenTop,
    /// A `color_details` index past the end of the palette.
    ColorDetail,
}

/// Parses `#rrggbb` into sRGB bytes.
pub fn parse_hex(s: &str) -> Option<[u8; 3]> {
    let b = s.as_bytes();
    if b.len() != 7 || b[0] != b'#' {
        return None;
    }
    let nib = |c: u8| match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'a'..=b'f' => Some(c - b'a' + 10),
        b'A'..=b'F' => Some(c - b'A' + 10),
        _ => None,
    };
    let mut out = [0u8; 3];
    for i in 0..3 {
        out[i] = nib(b[1 + 2 * i])? * 16 + nib(b[2 + 2 * i])?;
    }
    Some(out)
}

/// Parses a palette entry: `#rrggbb`, or `#upper/#lower` for two-tone paint.
pub fn parse_paint(s: &str) -> Option<([u8; 3], Option<[u8; 3]>)> {
    match s.split_once('/') {
        Some((a, b)) => Some((parse_hex(a)?, Some(parse_hex(b)?))),
        None => Some((parse_hex(s)?, None)),
    }
}

/// An optional color: empty, or `#rrggbb`.
fn opt_hex_ok(s: &str) -> bool {
    s.is_empty() || parse_hex(s).is_some()
}

fn finite2(p: &[(f32, f32)]) -> bool {
    p.iter().all(|&(a, b)| a.is_finite() && b.is_finite())
}

fn within(v: f32, lo: f32, hi: f32) -> bool {
    v >= lo && v <= hi
}

impl ShapeSpec {
    /// Index of the rearmost point of the top chain (the tail top).
    pub(crate) fn tail_index(&self) -> usize {
        let mut k = 0;
        while k + 1 < self.side.len() && self.side[k + 1].0 > self.side[k].0 {
            k += 1;
        }
        k
    }

    /// Body length, m (the side polyline's largest `x`).
    pub fn length(&self) -> f32 {
        self.side.get(self.tail_index()).map_or(0.0, |p| p.0)
    }

    fn all_finite(&self) -> bool {
        let s = &self.section;
        let w = &self.wheels;
        let i = &self.interior;
        let g = &self.glass;
        let scalars = [
            s.shoulder,
            s.tumblehome_deg,
            s.greenhouse_frac,
            s.crown,
            s.tuck,
            w.radius,
            w.width,
            w.arch_clearance,
            w.front_x,
            w.rear_x,
            w.track,
            self.front.bumper_y.0,
            self.front.bumper_y.1,
            self.rear.bumper_y.0,
            self.rear.bumper_y.1,
            self.seams.hood_width,
            g.windshield.0,
            g.windshield.1,
            g.backlight.0,
            g.backlight.1,
            i.dash_height,
            i.dash_x,
            i.wheel_pos.0,
            i.wheel_pos.1,
            i.wheel_radius,
            i.column_angle_deg,
            i.mirror_pos.0,
            i.mirror_pos.1,
            i.seat.0,
            i.seat.1,
            i.driver_offset,
        ];
        scalars.iter().all(|v| v.is_finite())
            && finite2(&self.side)
            && finite2(&self.plan)
            && finite2(&self.belt)
            && finite2(&g.side)
            && self
                .seams
                .creases
                .iter()
                .all(|c| c.0.is_finite() && c.1.is_finite() && c.2.is_finite() && c.3.is_finite())
    }

    /// Checks that the curves are well-formed and the parts fit. Does not judge looks.
    pub fn validate(&self) -> Result<(), ShapeError> {
        if self.id.is_empty() {
            return Err(ShapeError::EmptyId);
        }
        if !self.all_finite() {
            return Err(ShapeError::NonFinite);
        }

        // Side silhouette.
        let side = &self.side;
        if side.len() < 6 || side.len() > 48 {
            return Err(ShapeError::SidePointCount);
        }
        if side[0].0 != 0.0 {
            return Err(ShapeError::SideNotAtNose);
        }
        let tail = self.tail_index();
        if tail < 2 {
            return Err(ShapeError::SideTopNotIncreasing { index: 1 });
        }
        let length = side[tail].0;
        for k in tail + 1..side.len() {
            let ok = if k == tail + 1 {
                side[k].0 == length
            } else {
                side[k].0 < side[k - 1].0
            };
            if !ok {
                if k == tail + 1 {
                    // The top chain stopped increasing before the tail.
                    return Err(ShapeError::SideTopNotIncreasing { index: k as u32 });
                }
                return Err(ShapeError::SideBottomNotDecreasing { index: k as u32 });
            }
        }
        if side.len() - tail < 3 || side[side.len() - 1].0 != 0.0 {
            return Err(ShapeError::SideEnds);
        }
        let top: Vec<(f32, f32)> = side[..=tail].to_vec();
        let mut bottom: Vec<(f32, f32)> = side[tail + 1..].to_vec();
        bottom.reverse();
        for &(x, y) in side {
            let t = super::math::polyline_at(&top, x);
            let b = super::math::polyline_at(&bottom, x);
            if t <= b + 0.05 || y < 0.0 {
                return Err(ShapeError::SideInverted);
            }
        }

        // Plan.
        let plan = &self.plan;
        if plan.len() < 2 || plan.len() > 32 {
            return Err(ShapeError::PlanPointCount);
        }
        for (k, &(x, w)) in plan.iter().enumerate() {
            if k > 0 && x <= plan[k - 1].0 {
                return Err(ShapeError::PlanNotIncreasing { index: k as u32 });
            }
            if w <= 0.0 {
                return Err(ShapeError::NonPositiveWidth { index: k as u32 });
            }
        }
        if plan[0].0 != 0.0 || plan[plan.len() - 1].0 != length {
            return Err(ShapeError::PlanSpan);
        }

        // Belt.
        if self.belt.is_empty() || self.belt.len() > 32 {
            return Err(ShapeError::BeltPointCount);
        }
        for k in 1..self.belt.len() {
            if self.belt[k].0 <= self.belt[k - 1].0 {
                return Err(ShapeError::BeltNotIncreasing { index: k as u32 });
            }
        }

        let s = &self.section;
        if !within(s.shoulder, 0.6, 1.0)
            || !within(s.tumblehome_deg, 0.0, 40.0)
            || !within(s.greenhouse_frac, 0.4, 1.0)
            || !within(s.crown, 0.0, 0.15)
            || !within(s.tuck, 0.0, 0.15)
        {
            return Err(ShapeError::SectionOutOfRange);
        }

        // Glass.
        let g = &self.glass;
        let range_ok = |r: (f32, f32)| r.0 > 0.0 && r.1 > r.0 && r.1 <= length;
        if !range_ok(g.windshield) || !range_ok(g.backlight) || g.backlight.0 < g.windshield.1 {
            return Err(ShapeError::GlassRange);
        }
        for (k, &r) in g.side.iter().enumerate() {
            let after_prev = k == 0 || r.0 > g.side[k - 1].1;
            if !range_ok(r) || !after_prev || g.side.len() > 6 {
                return Err(ShapeError::SideWindow { index: k as u32 });
            }
        }

        // Wheels.
        let w = &self.wheels;
        if !(w.radius > 0.15 && w.radius < 0.6)
            || !(w.width > 0.08 && w.width < 0.5)
            || !within(w.arch_clearance, 0.0, 0.2)
        {
            return Err(ShapeError::WheelSize);
        }
        let ra = w.radius + w.arch_clearance;
        if !(w.front_x - ra > 0.0
            && w.rear_x + ra < length
            && w.rear_x - w.front_x > 2.0 * ra + 0.3)
        {
            return Err(ShapeError::WheelPlacement);
        }
        let max_half = plan.iter().fold(0.0f32, |m, p| m.max(p.1));
        if w.track <= 0.5 || w.track * 0.5 + w.width * 0.5 > max_half + 0.05 {
            return Err(ShapeError::TrackTooWide);
        }

        for b in [self.front.bumper_y, self.rear.bumper_y] {
            if !(b.0 >= 0.0 && b.1 > b.0 + 0.03 && b.1 < 1.2) {
                return Err(ShapeError::BumperRange);
            }
        }

        let sm = &self.seams;
        if sm.doors != 2 && sm.doors != 4 {
            return Err(ShapeError::DoorCount);
        }
        if !within(sm.hood_width, 0.3, 0.95) {
            return Err(ShapeError::HoodWidth);
        }
        if sm.creases.len() > 2 {
            return Err(ShapeError::TooManyCreases);
        }
        for (k, c) in sm.creases.iter().enumerate() {
            if !(c.0 >= 0.0 && c.2 > c.0 + 0.2 && c.2 <= length && c.1 > 0.0 && c.3 > 0.0) {
                return Err(ShapeError::Crease { index: k as u32 });
            }
        }
        if self.parts.wipers > 3 {
            return Err(ShapeError::TooManyWipers);
        }

        if self.palette.is_empty() {
            return Err(ShapeError::EmptyPalette);
        }
        for (k, c) in self.palette.iter().enumerate() {
            if parse_paint(c).is_none() {
                return Err(ShapeError::BadColor { index: k as u32 });
            }
        }
        if parse_hex(&self.accent).is_none() {
            return Err(ShapeError::BadAccent);
        }
        if self
            .color_details
            .iter()
            .any(|&(k, _)| k as usize >= self.palette.len())
        {
            return Err(ShapeError::ColorDetail);
        }

        let i = &self.interior;
        let inside = |x: f32| x > g.windshield.0 && x < length;
        let ok = inside(i.dash_x)
            && inside(i.wheel_pos.0)
            && inside(i.seat.0)
            && i.seat.0 > i.wheel_pos.0 + 0.2
            && i.wheel_pos.0 > i.dash_x
            && within(i.wheel_radius, 0.12, 0.25)
            && within(i.column_angle_deg, 0.0, 60.0)
            && i.dash_height > 0.3
            && i.wheel_pos.1 > 0.3
            && i.seat.1 > 0.1
            && within(i.driver_offset, 0.2, max_half - 0.2)
            && (1..=3).contains(&i.dials)
            && i.colors.len() == 6
            && i.colors.iter().all(|c| parse_hex(c).is_some())
            && parse_hex(&i.display).is_some()
            && [&i.insert, &i.piping, &i.dash_top]
                .iter()
                .all(|c| opt_hex_ok(c));
        if !ok {
            return Err(ShapeError::Interior);
        }
        if self.roof.convertible() {
            // The cabin opens from the header back to the boot well; the rear seat sits
            // ahead of the well and the deck behind the header is at the belt (there is no
            // greenhouse to fold away).
            let at = super::math::polyline_at;
            let header_y = at(&top, g.windshield.1);
            let mid = (g.windshield.1 + g.backlight.0) * 0.5;
            let ok = g.side.is_empty()
                && g.backlight.0 > i.seat.0 + 0.5
                && g.backlight.1 < length - 0.25
                && header_y > at(&self.belt, g.windshield.1) + 0.25
                && at(&top, mid) < at(&self.belt, mid) + 0.08
                && at(&top, g.backlight.1) < at(&self.belt, g.backlight.1) + 0.08;
            if !ok {
                return Err(ShapeError::OpenTop);
            }
        }
        Ok(())
    }
}
