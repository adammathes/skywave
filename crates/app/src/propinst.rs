//! The props a terrain place draws as instances of shared meshes rather than inside its
//! chunks' meshes ([`world::props::instanced`]): its trees, parked cars, street lights,
//! power poles and small street furniture. A tree costs a chunk ~1100 vertices and a car
//! ~2000 triangles, and the coast has thousands of lamps, hydrants, benches and poles:
//! baked into the chunks (and cut into their light tiles) they would not fit the chunk
//! pool. As instances there are a few dozen meshes, and each frame draws the ones near the
//! camera in live chunks. Street lights draw out past the resident ring, so a boulevard's
//! row of lamp heads reaches the horizon at night; their lenses glow with the photocell
//! (the instance's `glow`).
//!
//! Trees come in [`VARIANTS`] shapes per kind, each turned and scaled per tree; mailboxes
//! and poles in a few shapes too; parked cars are the built-in body shapes at level of
//! detail 1 in their palette colors. Every instance is placed by a fixed transform from
//! the Place's `Props` table. Each frame [`PropInstances::push`] appends the ones within
//! their kind's draw distance of the eye whose chunk is drawn, grouped by mesh so the
//! renderer draws each mesh's run as one instanced draw, each with the lights that reach
//! it when it is within [`LIT_RADIUS_M`]. Nothing allocates per frame.

use glam::{Mat4, Quat, Vec3};
use place::{Place, PropKind};
use render::{LightSet, Mesh, MeshId, MeshInstance, PointLight};
use world::{MeshKind, Palette, WorldMesh};

use crate::lighting::{light_set, Bounds};
use crate::worldmesh::atlas_mesh;
use crate::{car, convert, lighttiles, vehicles, Part};

/// Shapes of a kind that has several.
pub const VARIANTS: u32 = 4;
/// How far from the eye parked cars are drawn, m.
pub const CAR_DRAW_RADIUS_M: f32 = 280.0;
/// Instances farther than this from the eye are lit by the sun and sky only, m.
pub const LIT_RADIUS_M: f32 = 160.0;
/// Most prop instances a frame draws.
pub const MAX_PROP_INSTANCES: usize = 4096;

/// How an instanced kind is drawn: its mesh, how many shapes, whether each turns at random
/// (trees) or faces its yaw, its draw distance (m), and the half size and center height of
/// its light-culling box (m).
struct Look {
    kind: world::props::PropKind,
    variants: u32,
    tree: bool,
    reach: f32,
    half: f32,
    up: f32,
}

const fn look(
    kind: world::props::PropKind,
    variants: u32,
    tree: bool,
    reach: f32,
    half: f32,
    up: f32,
) -> Look {
    Look {
        kind,
        variants,
        tree,
        reach,
        half,
        up,
    }
}

/// The instanced kinds but cars, in mesh order.
const LOOKS: [Look; 10] = {
    use world::props::PropKind as K;
    [
        look(K::PalmTree, VARIANTS, true, 420.0, 4.0, 7.0),
        look(K::Oak, VARIANTS, true, 420.0, 4.0, 3.5),
        look(K::Eucalyptus, VARIANTS, true, 420.0, 5.0, 9.0),
        look(K::Hydrant, 1, false, 200.0, 0.5, 0.4),
        look(K::Bench, 1, false, 200.0, 1.0, 0.5),
        look(K::TrashCan, 1, false, 200.0, 0.5, 0.5),
        look(K::Mailbox, VARIANTS, false, 200.0, 0.5, 0.7),
        look(K::PowerPole, VARIANTS, false, 380.0, 1.5, 5.5),
        look(K::StopSign, 1, false, 250.0, 0.6, 1.5),
        look(K::StreetLight, 1, false, 900.0, 1.5, 4.0),
    ]
};

/// The street lights' entry in [`LOOKS`]: their lenses switch with the photocell.
const LAMP_LOOK: usize = 9;

/// Which of [`LOOKS`] draws Place kind `k` (`None`: a parked car, or not instanced).
fn look_of(k: PropKind) -> Option<usize> {
    Some(match k {
        PropKind::PalmTree => 0,
        PropKind::Oak => 1,
        PropKind::Eucalyptus => 2,
        PropKind::Hydrant => 3,
        PropKind::Bench => 4,
        PropKind::TrashCan => 5,
        PropKind::Mailbox => 6,
        PropKind::PowerPole => 7,
        PropKind::StopSign => 8,
        PropKind::StreetLight => LAMP_LOOK,
        _ => return None,
    })
}

/// One instance: its mesh, where it stands and what it covers.
#[derive(Clone, Copy, Debug)]
struct Entry {
    mesh: u16,
    transform: [[f32; 4]; 4],
    bounds: Bounds,
    chunk: [i32; 2],
    /// Squared draw distance.
    reach2: f32,
}

/// The instanced props of a place (see the module docs).
pub struct PropInstances {
    meshes: Vec<Mesh>,
    ids: Vec<MeshId>,
    /// Sorted by mesh.
    entries: Vec<Entry>,
    /// The street lights' mesh.
    lamp_mesh: u16,
}

impl PropInstances {
    /// The instanced props of `place`, each filed under the chunk `chunk_of` gives for its
    /// ground point. `None` when it has none.
    pub fn new(
        place: &Place,
        palette: &Palette,
        chunk_of: impl Fn([f32; 2]) -> [i32; 2],
    ) -> Result<Option<PropInstances>, String> {
        let items: Vec<&place::Prop> = place
            .props
            .items
            .iter()
            .filter(|p| world::props::instanced(p.kind))
            .collect();
        if items.is_empty() {
            return Ok(None);
        }
        let seed = place.header.seed;
        let kit = world::props::PropKit::new();
        // Each kind's shapes, built at the origin facing yaw 0 (`-z`).
        let mut meshes = Vec::new();
        let mut first = [0u16; LOOKS.len()];
        for (k, look) in LOOKS.iter().enumerate() {
            first[k] = meshes.len() as u16;
            for v in 0..look.variants {
                let mut m = WorldMesh::default();
                let p = world::props::Prop {
                    pos: world::math::V2::new(0.0, 0.0),
                    y: 0.0,
                    dir: world::props::dir_of_yaw(0.0),
                    kind: look.kind,
                    seed: skywave_core::hash64(0x5452_4545, &[k as u64, u64::from(v)]) as u32,
                };
                world::props::build(&mut m, &p, palette, &kit);
                let m = atlas_mesh(&m, MeshKind::Prop);
                let mut mesh = convert(&m, Part::Base);
                // A lamp's lens (left out of a chunk's lit geometry, which keeps lenses
                // apart for the photocell) is part of its mesh.
                lighttiles::append(&mut mesh, &convert(&m, Part::Lamps));
                meshes.push(mesh);
            }
        }
        let lamp_mesh = first[LAMP_LOOK];
        let shapes: Vec<vehicle::shape::ShapeSpec> = vehicles::BUILTIN_SHAPES
            .iter()
            .map(|(_, text)| vehicles::load_shape(text))
            .collect::<Result<_, _>>()?;
        // Car meshes are made for the (shape, paint) pairs the place uses.
        let mut car_mesh: Vec<Vec<Option<u16>>> =
            shapes.iter().map(|s| vec![None; s.palette.len()]).collect();
        let mut entries = Vec::with_capacity(items.len());
        for p in items {
            let pos = Vec3::from(p.pos);
            let h = skywave_core::hash64(
                seed,
                &[u64::from(p.pos[0].to_bits()), u64::from(p.pos[2].to_bits())],
            );
            let chunk = chunk_of([p.pos[0], p.pos[2]]);
            let (mesh, transform, bounds, reach) = match p.kind {
                PropKind::ParkedCar { vehicle_class } => {
                    let s = world::props::parked_car_shape(vehicle_class) % shapes.len();
                    let paint = usize::from(vehicle_class >> 4) % shapes[s].palette.len();
                    let id = match car_mesh[s][paint] {
                        Some(id) => id,
                        None => {
                            meshes.push(car::parked_mesh(&shapes[s], paint)?);
                            let id = (meshes.len() - 1) as u16;
                            car_mesh[s][paint] = Some(id);
                            id
                        }
                    };
                    let m = Mat4::from_rotation_translation(Quat::from_rotation_y(p.yaw), pos);
                    let b = Bounds::cube(pos + Vec3::Y, 2.8);
                    (id, m, b, CAR_DRAW_RADIUS_M)
                }
                kind => {
                    let Some(k) = look_of(kind) else {
                        continue;
                    };
                    let look = &LOOKS[k];
                    let v = (h % u64::from(look.variants)) as u16;
                    let (turn, scale) = if look.tree {
                        let turn = ((h >> 8) & 1023) as f32 / 1024.0 * core::f32::consts::TAU;
                        (turn, 0.85 + 0.3 * ((h >> 20) & 255) as f32 / 255.0)
                    } else {
                        (p.yaw, 1.0)
                    };
                    let m = Mat4::from_scale_rotation_translation(
                        Vec3::splat(scale),
                        Quat::from_rotation_y(turn),
                        pos,
                    );
                    let (half, up) = (look.half * scale, look.up * scale);
                    let b = Bounds::cube(pos + Vec3::Y * up, half.max(up));
                    (first[k] + v, m, b, look.reach)
                }
            };
            entries.push(Entry {
                mesh,
                transform: transform.to_cols_array_2d(),
                bounds,
                chunk,
                reach2: reach * reach,
            });
        }
        // Stable, so each mesh's instances keep the table's order.
        entries.sort_by_key(|e| e.mesh);
        Ok(Some(PropInstances {
            meshes,
            ids: Vec::new(),
            entries,
            lamp_mesh,
        }))
    }

    /// The shared meshes, to register with the renderer.
    pub fn meshes(&self) -> &[Mesh] {
        &self.meshes
    }

    /// The ids the meshes were registered under, in [`PropInstances::meshes`] order.
    pub fn set_ids(&mut self, ids: Vec<MeshId>) {
        self.ids = ids;
    }

    /// Instanced props in all.
    pub fn count(&self) -> usize {
        self.entries.len()
    }

    /// Appends the instances near `eye` whose chunk `live` says is drawn, at most
    /// [`MAX_PROP_INSTANCES`], each with the nearest of `lights` that reach it when within
    /// [`LIT_RADIUS_M`]; street lamp lenses glow by `lamp_glow`. Returns how many.
    pub fn push(
        &self,
        eye: Vec3,
        live: impl Fn([i32; 2]) -> bool,
        lights: &[PointLight],
        lamp_glow: f32,
        out: &mut Vec<MeshInstance>,
    ) -> usize {
        let mut n = 0;
        let lit2 = LIT_RADIUS_M * LIT_RADIUS_M;
        for e in &self.entries {
            if n == MAX_PROP_INSTANCES {
                break;
            }
            let d2 = e.bounds.distance_squared(eye);
            if d2 > e.reach2 || !live(e.chunk) {
                continue;
            }
            let lights = if d2 < lit2 {
                light_set(lights, &e.bounds)
            } else {
                LightSet::EMPTY
            };
            let glow = if e.mesh == self.lamp_mesh {
                lamp_glow
            } else {
                1.0
            };
            out.push(MeshInstance {
                lights,
                glow,
                ..MeshInstance::new(self.ids[usize::from(e.mesh)], e.transform)
            });
            n += 1;
        }
        n
    }
}
