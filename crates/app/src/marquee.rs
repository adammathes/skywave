//! The landmarks' chasing bulbs (`docs/WORLD_GENERATION.md`, "Landmarks"): a kit's
//! `Bulbs` rows are built dim into the town's meshes, and over them every third bulb is lit
//! in turn ([`world::landmarks::chase`]), stepping [`CHASE_HZ`] times a second of game time,
//! so a marquee's lights run round it. Each landmark with bulbs has one mesh per phase,
//! built in place (an identity instance); the lit phase is drawn within [`DRAW_RADIUS_M`].

use glam::{Mat4, Vec3};
use place::{Place, PropKind};
use render::{Mesh, MeshId, MeshInstance};
use world::landmarks::{Kit, CHASE_PHASES};
use world::math::{V2, V3};
use world::{MeshKind, Palette, WorldMesh};

use crate::worldmesh::atlas_mesh;
use crate::{convert, Part};

/// The lit bulbs move on this many times a second.
pub const CHASE_HZ: f32 = 6.0;
/// The lit bulbs are drawn within this of the eye, m.
pub const DRAW_RADIUS_M: f32 = 500.0;

/// One landmark with bulbs: its prop and kit, then its phase meshes' indices.
struct Sign {
    at: Vec3,
    base: V3,
    facing: V2,
    kit: usize,
    variant: usize,
    mesh: [u16; CHASE_PHASES],
}

/// The place's chasing marquees.
pub struct Marquees {
    signs: Vec<Sign>,
    ids: Vec<MeshId>,
}

impl Marquees {
    /// The landmarks of `place` built from `kits` whose variant has bulbs; `None` if none.
    pub fn of(place: &Place, kits: &[Kit]) -> Option<Marquees> {
        let signs: Vec<Sign> = place
            .props
            .items
            .iter()
            .filter_map(|p| match p.kind {
                PropKind::Landmark { kit, variant } => {
                    let (kit, variant) = (usize::from(kit), usize::from(variant));
                    let k = kits.get(kit)?;
                    world::landmarks::chases(k, variant).then(|| Sign {
                        at: Vec3::from(p.pos),
                        base: V3::new(p.pos[0], p.pos[1], p.pos[2]),
                        facing: world::props::dir_of_yaw(p.yaw),
                        kit,
                        variant,
                        mesh: [0; CHASE_PHASES],
                    })
                }
                _ => None,
            })
            .collect();
        (!signs.is_empty()).then_some(Marquees {
            signs,
            ids: Vec::new(),
        })
    }

    /// The meshes its instances draw, in order; [`Marquees::set_ids`] takes their ids.
    pub fn meshes(&mut self, kits: &[Kit], pal: &Palette) -> Vec<Mesh> {
        let mut out = Vec::new();
        for s in &mut self.signs {
            for (phase, slot) in s.mesh.iter_mut().enumerate() {
                let mut m = WorldMesh::default();
                let kit = &kits[s.kit];
                world::landmarks::chase(&mut m, kit, s.variant, phase, s.base, s.facing, pal);
                out.push(convert(&atlas_mesh(&m, MeshKind::Prop), Part::Base));
                *slot = (out.len() - 1) as u16;
            }
        }
        out
    }

    pub fn set_ids(&mut self, ids: Vec<MeshId>) {
        self.ids = ids;
    }

    /// Most instances a frame adds: a lit phase per landmark.
    pub fn capacity(&self) -> usize {
        self.signs.len()
    }

    /// The phase lit `ticks` simulation steps in.
    pub fn phase(ticks: u64) -> usize {
        let steps = ticks as f64 * f64::from(CHASE_HZ) * skywave_core::SIM_DT;
        steps as u64 as usize % CHASE_PHASES
    }

    /// Appends each near landmark's lit bulbs for `ticks` steps in.
    pub fn push(&self, eye: Vec3, ticks: u64, out: &mut Vec<MeshInstance>) {
        if self.ids.is_empty() {
            return;
        }
        let phase = Self::phase(ticks);
        for s in self
            .signs
            .iter()
            .filter(|s| s.at.distance(eye) <= DRAW_RADIUS_M)
        {
            let id = self.ids[usize::from(s.mesh[phase])];
            out.push(MeshInstance {
                casts_shadow: false,
                ..MeshInstance::new(id, Mat4::IDENTITY.to_cols_array_2d())
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use place::Prop;

    fn place_with(kit: u8, variant: u8) -> Place {
        let mut place = Place::test_grid();
        place.props.items = vec![Prop {
            pos: [10.0, 0.0, 20.0],
            yaw: 0.0,
            kind: PropKind::Landmark { kit, variant },
        }];
        place
    }

    #[test]
    fn a_marquee_chases_in_thirds_and_only_near_the_eye() {
        let kits = crate::places::landmarks();
        let palace = kits.iter().position(|k| k.id == "movie_palace").unwrap();
        let tower = kits.iter().position(|k| k.id == "water_tower").unwrap();
        assert!(Marquees::of(&place_with(tower as u8, 0), kits).is_none());
        let mut m = Marquees::of(&place_with(palace as u8, 0), kits).unwrap();
        let meshes = m.meshes(kits, &Palette::socal_1985());
        assert_eq!(meshes.len(), CHASE_PHASES);
        assert!(meshes.iter().all(|x| !x.indices.is_empty()));
        m.set_ids((0..meshes.len() as u32).map(MeshId).collect());
        // A phase lasts 1 / CHASE_HZ s: 20 steps at 120 Hz, then the next, round again.
        let phases: Vec<usize> = [0, 19, 20, 40, 60].map(Marquees::phase).to_vec();
        assert_eq!(phases, vec![0, 0, 1, 2, 0]);
        let mut out = Vec::new();
        m.push(Vec3::new(10.0, 2.0, 30.0), 20, &mut out);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].mesh, MeshId(1));
        out.clear();
        m.push(Vec3::new(10.0, 2.0, 30.0 + DRAW_RADIUS_M), 20, &mut out);
        assert!(out.is_empty());
    }
}
