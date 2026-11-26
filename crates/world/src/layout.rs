//! Authored layouts (`docs/LAYOUTS.md`): a hand-written map of a place's structure (terrain
//! shapes, water, districts and main roads) that [`crate::procgen::authored`] turns into a
//! Place, the shared street, parcel, building and detail stages filling in the rest.
//!
//! Plain data with a check ([`Layout::validate`]); nothing here builds anything.

use alloc::string::String;
use alloc::vec::Vec;
use core::fmt;

use place::{RoadClass, ZoneId};

use crate::math::{segment_distance, sin_cos, PI, V2};
use crate::template::Edges;

/// A point `(x, z)` in meters in the place's frame: x east, z south, `(0, 0)` the corner of
/// chunk `(0, 0)`.
pub type Pt = (f32, f32);

/// A ridge along a polyline: its crest `height_m` over the ground (or `heights_m`, one per
/// point of `line`, interpolated between them), falling to nothing `falloff_m` either
/// side, roughened by up to `relief_m`.
#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(deny_unknown_fields))]
pub struct Ridge {
    pub name: String,
    pub line: Vec<Pt>,
    #[cfg_attr(feature = "serde", serde(default))]
    pub height_m: f32,
    #[cfg_attr(feature = "serde", serde(default))]
    pub heights_m: Vec<f32>,
    pub falloff_m: f32,
    #[cfg_attr(feature = "serde", serde(default))]
    pub relief_m: f32,
}

impl Ridge {
    /// The crest height at point `i` of the line.
    pub fn height_at(&self, i: usize) -> f32 {
        self.heights_m.get(i).copied().unwrap_or(self.height_m)
    }
}

/// A hill or mountain at a point, `height_m` over the ground, falling to nothing at
/// `falloff_m`.
#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(deny_unknown_fields))]
pub struct Peak {
    pub name: String,
    pub at: Pt,
    pub height_m: f32,
    pub falloff_m: f32,
}

/// Open water or a lake.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum WaterKind {
    /// Salt water: the ocean, a bay, a strait.
    Sea,
    Lake,
}

/// A body of water: the ground inside the polygon sinks to `depth_m` under the water level.
#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(deny_unknown_fields))]
pub struct Water {
    pub kind: WaterKind,
    pub depth_m: f32,
    pub polygon: Vec<Pt>,
}

/// A district: the land inside the polygon is `zone`. Later districts paint over earlier
/// ones. Urban zones smooth the ground under them so streets sit well; `hilly` keeps its
/// hills instead.
#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(deny_unknown_fields))]
pub struct District {
    pub name: String,
    pub zone: ZoneId,
    pub polygon: Vec<Pt>,
    #[cfg_attr(feature = "serde", serde(default))]
    pub hilly: bool,
}

/// A main road: a polyline (`points`), or a start (`from`) and a length (`length_m`)
/// along a bearing (`bearing_deg`) or toward a peak (`toward`). Each end must meet
/// another road (within [`SNAP_M`]) unless `dead_end` says the last point is a dead end
/// (an overlook, a gate). Over water the road is a bridge, its deck `deck_m` above the
/// water; a `suspension` bridge also stands two towers over its longest crossing, with
/// cables between them.
#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(deny_unknown_fields))]
pub struct Road {
    pub name: String,
    pub class: RoadClass,
    /// Lanes each way.
    pub lanes: u8,
    #[cfg_attr(feature = "serde", serde(default))]
    pub points: Vec<Pt>,
    #[cfg_attr(feature = "serde", serde(default))]
    pub from: Option<Pt>,
    #[cfg_attr(feature = "serde", serde(default))]
    pub toward: Option<String>,
    #[cfg_attr(feature = "serde", serde(default))]
    pub bearing_deg: Option<f32>,
    #[cfg_attr(feature = "serde", serde(default))]
    pub length_m: Option<f32>,
    #[cfg_attr(feature = "serde", serde(default))]
    pub speed_kph: Option<u8>,
    /// Steepest grade the road's profile may take (default [`MAX_GRADE`]).
    #[cfg_attr(feature = "serde", serde(default))]
    pub max_grade: Option<f32>,
    #[cfg_attr(feature = "serde", serde(default = "yes"))]
    pub sidewalk: bool,
    #[cfg_attr(feature = "serde", serde(default))]
    pub dead_end: bool,
    #[cfg_attr(feature = "serde", serde(default = "deck"))]
    pub deck_m: f32,
    #[cfg_attr(feature = "serde", serde(default))]
    pub suspension: bool,
}

#[cfg(feature = "serde")]
fn yes() -> bool {
    true
}

#[cfg(feature = "serde")]
fn deck() -> f32 {
    DECK_M
}

/// Where the player starts: on road `road`, `along_m` from its first point, driving away
/// from it.
#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(deny_unknown_fields))]
pub struct Start {
    pub road: String,
    pub along_m: f32,
}

/// A place's structure, authored. See the module docs and `docs/LAYOUTS.md`.
#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(deny_unknown_fields))]
pub struct Layout {
    pub id: String,
    /// Chunks of 128 m along x and z.
    pub size_chunks: (u16, u16),
    /// Land height where no shape says otherwise.
    pub ground_m: f32,
    pub water_level_m: f32,
    pub edges: Edges,
    #[cfg_attr(feature = "serde", serde(default))]
    pub ridges: Vec<Ridge>,
    #[cfg_attr(feature = "serde", serde(default))]
    pub peaks: Vec<Peak>,
    #[cfg_attr(feature = "serde", serde(default))]
    pub water: Vec<Water>,
    /// The zone of land no district covers.
    pub default_zone: ZoneId,
    #[cfg_attr(feature = "serde", serde(default))]
    pub districts: Vec<District>,
    pub roads: Vec<Road>,
    pub start: Start,
}

/// A road end this close to another road meets it.
pub const SNAP_M: f32 = 30.0;
/// Default steepest road grade.
pub const MAX_GRADE: f32 = 0.12;
/// Default bridge deck height over the water.
pub const DECK_M: f32 = 3.0;
/// How far a road profile may leave the ground; `place::validate` allows 8 m.
pub const PROFILE_BAND_M: f32 = 7.8;

/// Why [`Layout::validate`] refused a layout.
#[derive(Clone, Debug, PartialEq)]
pub enum LayoutError {
    Size,
    /// A point of the named thing lies outside the place.
    Outside(String),
    /// A polygon or line with too few points.
    TooFewPoints(String),
    /// Two things share a name.
    Duplicate(String),
    /// A road names no peak that exists, or has neither points nor `from` and a length.
    BadRoad(String),
    /// A road end meets no other road and the road is not a dead end.
    RoadToNowhere(String),
    /// A bridge's deck would stand more than the profile band over the water's bed.
    DeckTooHigh(String),
    /// The start names no road, or a point past its end.
    BadStart,
    /// A non-positive height, depth, falloff or lane count.
    Value(String),
}

impl fmt::Display for LayoutError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LayoutError::Size => write!(f, "size_chunks must be 1..=256 each way"),
            LayoutError::Outside(n) => write!(f, "`{n}` has a point outside the place"),
            LayoutError::TooFewPoints(n) => write!(f, "`{n}` has too few points"),
            LayoutError::Duplicate(n) => write!(f, "two things are named `{n}`"),
            LayoutError::BadRoad(n) => write!(f, "road `{n}` has no usable line"),
            LayoutError::RoadToNowhere(n) => {
                write!(f, "road `{n}` ends nowhere (meet a road or say dead_end)")
            }
            LayoutError::DeckTooHigh(n) => {
                write!(f, "road `{n}`: deck_m plus the water's depth is over 7.8 m")
            }
            LayoutError::BadStart => write!(f, "start names no road or runs past its end"),
            LayoutError::Value(n) => write!(f, "`{n}` has a bad value"),
        }
    }
}

impl Layout {
    /// The place's extent `[x, z]` in meters.
    pub fn extent(&self) -> [f32; 2] {
        [
            f32::from(self.size_chunks.0) * place::CHUNK_SIZE_M,
            f32::from(self.size_chunks.1) * place::CHUNK_SIZE_M,
        ]
    }

    /// The peak named `name`.
    pub fn peak(&self, name: &str) -> Option<&Peak> {
        self.peaks.iter().find(|p| p.name == name)
    }

    /// Road `r`'s polyline: its points, or the line from `from` along its bearing or toward
    /// its peak for `length_m`. `None` when it has neither.
    pub fn road_line(&self, r: &Road) -> Option<Vec<V2>> {
        if r.points.len() >= 2 {
            return Some(r.points.iter().map(|&(x, z)| V2::new(x, z)).collect());
        }
        let from = V2::new(r.from?.0, r.from?.1);
        let len = r.length_m?;
        let dir = match (&r.toward, r.bearing_deg) {
            (Some(name), _) => {
                let p = self.peak(name)?;
                (V2::new(p.at.0, p.at.1) - from).normalize()
            }
            (None, Some(deg)) => {
                // Clockwise from north (-z).
                let (s, c) = sin_cos(deg * PI / 180.0);
                V2::new(s, -c)
            }
            (None, None) => return None,
        };
        if dir.length() < 0.5 {
            return None;
        }
        Some(alloc::vec![from, from + dir * len])
    }

    /// Checks every rule in `docs/LAYOUTS.md`; generation assumes a layout that passed.
    pub fn validate<'a>(&'a self) -> Result<(), LayoutError> {
        let (sx, sz) = self.size_chunks;
        if sx == 0 || sz == 0 || sx > place::MAX_SIZE_CHUNKS || sz > place::MAX_SIZE_CHUNKS {
            return Err(LayoutError::Size);
        }
        let ext = self.extent();
        let inside = |p: V2| p.x >= 0.0 && p.y >= 0.0 && p.x <= ext[0] && p.y <= ext[1];
        let pts = |v: &[Pt]| v.iter().map(|&(x, z)| V2::new(x, z)).collect::<Vec<V2>>();
        let check = |name: &str, v: &[V2], min: usize| {
            if v.len() < min {
                return Err(LayoutError::TooFewPoints(name.into()));
            }
            if v.iter().any(|&p| !inside(p)) {
                return Err(LayoutError::Outside(name.into()));
            }
            Ok(())
        };
        let mut names: Vec<&str> = Vec::new();
        let mut unique = |n: &'a str| -> Result<(), LayoutError> {
            if names.contains(&n) {
                return Err(LayoutError::Duplicate(n.into()));
            }
            names.push(n);
            Ok(())
        };
        for r in &self.ridges {
            unique(&r.name)?;
            check(&r.name, &pts(&r.line), 2)?;
            let per_point = !r.heights_m.is_empty();
            if (per_point
                && (r.heights_m.len() != r.line.len() || r.heights_m.iter().any(|&h| h < 0.0)))
                || (!per_point && r.height_m <= 0.0)
                || r.falloff_m <= 0.0
                || r.relief_m < 0.0
            {
                return Err(LayoutError::Value(r.name.clone()));
            }
        }
        for p in &self.peaks {
            unique(&p.name)?;
            check(&p.name, &[V2::new(p.at.0, p.at.1)], 1)?;
            if p.height_m <= 0.0 || p.falloff_m <= 0.0 {
                return Err(LayoutError::Value(p.name.clone()));
            }
        }
        for (i, w) in self.water.iter().enumerate() {
            let n = alloc::format!("water {i}");
            check(&n, &pts(&w.polygon), 3)?;
            if w.depth_m <= 0.0 {
                return Err(LayoutError::Value(n));
            }
        }
        for d in &self.districts {
            unique(&d.name)?;
            check(&d.name, &pts(&d.polygon), 3)?;
        }
        let max_depth = self.water.iter().map(|w| w.depth_m).fold(0.0, f32::max);
        let mut lines: Vec<Vec<V2>> = Vec::new();
        for r in &self.roads {
            unique(&r.name)?;
            if r.lanes == 0 || r.lanes > 3 || r.deck_m <= 0.0 {
                return Err(LayoutError::Value(r.name.clone()));
            }
            if r.max_grade.is_some_and(|g| !(0.01..=0.35).contains(&g)) {
                return Err(LayoutError::Value(r.name.clone()));
            }
            if r.deck_m + max_depth > PROFILE_BAND_M {
                return Err(LayoutError::DeckTooHigh(r.name.clone()));
            }
            let line = self
                .road_line(r)
                .ok_or_else(|| LayoutError::BadRoad(r.name.clone()))?;
            check(&r.name, &line, 2)?;
            lines.push(line);
        }
        // Every road end meets another road (or itself, for a loop) unless it is a dead end.
        for (i, r) in self.roads.iter().enumerate() {
            let line = &lines[i];
            let ends = [line[0], line[line.len() - 1]];
            for (k, &p) in ends.iter().enumerate() {
                if k == 1 && r.dead_end {
                    continue;
                }
                let closed = (ends[0] - ends[1]).length() < 1.0;
                let met = closed
                    || lines
                        .iter()
                        .enumerate()
                        .any(|(j, l)| j != i && distance_to_line(p, l) <= SNAP_M);
                if !met {
                    return Err(LayoutError::RoadToNowhere(r.name.clone()));
                }
            }
        }
        let start = self
            .roads
            .iter()
            .position(|r| r.name == self.start.road)
            .ok_or(LayoutError::BadStart)?;
        if self.start.along_m < 0.0 || self.start.along_m > line_length(&lines[start]) {
            return Err(LayoutError::BadStart);
        }
        Ok(())
    }
}

/// Distance from `p` to the polyline `line`.
pub fn distance_to_line(p: V2, line: &[V2]) -> f32 {
    line.windows(2)
        .map(|s| segment_distance(p, s[0], s[1]).0)
        .fold(f32::INFINITY, f32::min)
}

/// Length of the polyline `line`.
pub fn line_length(line: &[V2]) -> f32 {
    line.windows(2).map(|s| (s[1] - s[0]).length()).sum()
}
