//! The props a terrain place draws as instances of shared meshes rather than inside its
//! chunks' meshes ([`world::props::instanced`]): its trees, parked cars, street lights,
//! power poles and small street furniture. A tree costs ~700–900 triangles and a car
//! 3–4k, and the coast has thousands of lamps, hydrants, benches and poles:
//! baked into the chunks (and cut into their light tiles) they would not fit the chunk
//! pool. As instances there are a few dozen meshes, and each frame draws the ones near the
//! camera in live chunks. Street lights draw out past the resident ring, so a boulevard's
//! row of lamp heads reaches the horizon at night; their lenses glow with the photocell
//! (the instance's `glow`).
//!
//! Trees come in [`VARIANTS`] shapes per kind, each turned and scaled per tree; mailboxes
//! and poles in a few shapes too; parked cars are the built-in body shapes at level of
//! detail 1, a third in their own palette colors and the rest in the era's street colors
//! (`vehicle::shape::ERA_1980S_PAINT`), each with a little dust and wear. Every instance is placed by a fixed transform from
//! the Place's `Props` table. Each frame [`PropInstances::push`] appends the ones within
//! their kind's draw distance of the eye whose chunk is drawn, grouped by mesh so the
//! renderer draws each mesh's run as one instanced draw, each with the lights that reach
//! it when it is within [`LIT_RADIUS_M`]. Nothing allocates per frame.

use glam::{Mat4, Quat, Vec3};
use place::{Place, PropKind};
use render::{LightSet, Mesh, MeshId, MeshInstance};
use vehicle::shape;
use world::{MeshKind, Palette, WorldMesh};

use crate::lighting::{Bounds, FrameLights};
use crate::worldmesh::atlas_mesh;
use crate::{car, convert, lighttiles, vehicles, Part};

/// Shapes of a kind that has several.
pub const VARIANTS: u32 = 4;
/// How far from the eye parked cars are drawn, m.
pub const CAR_DRAW_RADIUS_M: f32 = 280.0;
/// Parked cars nearer than this draw at level of detail 1 (3-5k triangles), farther at
/// level 2 (~2k), m.
pub const CAR_DETAIL_RADIUS_M: f32 = 80.0;
/// Parked cars whose paint nibble (0..16) is under this wear their own palette's colors.
const OWN_PAINT_NIBBLES: usize = 5;
/// Paints a place uses per shape, from its own palette and from the era's street colors:
/// each (shape, paint) pair is two meshes, so this bounds the parked-car mesh set (fourteen
/// shapes at 2 + 4 paints) whatever the place holds.
const OWN_PAINTS: usize = 2;
pub(crate) const ERA_PAINTS: usize = 4;
/// Most a parked car's dust and wear darkens it.
const MAX_WEAR: f32 = 0.14;

/// The entry of [`shape::ERA_1980S_PAINT`] that `roll` picks.
fn era_index(roll: u32) -> usize {
    let paint = shape::era_1980s_paint(roll);
    shape::ERA_1980S_PAINT
        .iter()
        .position(|e| e.0 == paint)
        .unwrap_or(0)
}

/// The built-in shapes a parked car of each `VehicleClass` (the low nibble of a Place
/// `ParkedCar`'s class, in `vehicle::VehicleClass` order) is drawn as, with weights. Each
/// list keeps to bodies within half a meter of the length of the class's collision box
/// (`world::props::parked_car_size`, by class, in `crates/world`) and no more than 10 cm
/// wider; a class past the end (`Utility`, which the world
/// does not park) parks as the wagon. The exotics and the sports cars' open versions never
/// park: the street sees the everyday cars and the two common sports cars, and in 2001 the
/// traffic cars that fit a stall (the box truck and the semi do not). The 2001 cars come
/// last in each list, so the 1984 coast, which has none of them, picks as before.
const PARKED: [&[(&str, u32)]; 10] = [
    &[
        ("gannet_gt", 3),
        ("halcyon_turbo", 3),
        ("pipit_hatch", 2),
        ("longbow_coupe", 2),
        ("appaloosa_gt", 2),
        ("linnet_hatch", 3),
    ], // Sports
    &[
        ("bancroft_sedan", 3),
        ("prescott_landau", 3),
        ("homestead_wagon", 1),
        ("buckboard_pickup", 2),
        ("plover_sedan", 3),
        ("sorrel_utility", 1),
        ("wren_minivan", 1),
        ("percheron_pickup", 1),
    ], // Sedan
    &[
        ("homestead_wagon", 3),
        ("meadow_minivan", 3),
        ("drayman_van", 1),
        ("buckboard_pickup", 1),
        ("sorrel_utility", 3),
        ("wren_minivan", 3),
        ("plover_sedan", 1),
        ("percheron_pickup", 1),
    ], // Wagon
    &[
        ("seabreeze_convertible", 3),
        ("halcyon_turbo", 2),
        ("burro_4x4", 2),
        ("plover_sedan", 2),
    ], // Coupe
    &[("buckboard_pickup", 1), ("percheron_pickup", 1)], // Pickup
    &[
        ("meadow_minivan", 1),
        ("drayman_van", 1),
        ("wren_minivan", 1),
    ], // Van
    &[("pipit_hatch", 1), ("linnet_hatch", 1)],          // Hatch
    &[("seabreeze_convertible", 1)],                     // Convertible
    &[
        ("regent_coupe_cabrio", 1),
        ("bancroft_sedan", 1),
        ("plover_sedan", 1),
    ], // Luxury
    &[("gannet_gt", 1), ("halcyon_turbo", 1), ("appaloosa_gt", 1)], // Muscle
];

/// The paints a place of `seed` parks shape `s` (with `n_own` palette entries) in: up to
/// [`OWN_PAINTS`] palette indices and [`ERA_PAINTS`] distinct [`shape::ERA_1980S_PAINT`]
/// indices, drawn by the era's weights so the common street colors are the likely ones.
pub(crate) fn place_paints(seed: u64, s: usize, n_own: usize) -> (Vec<usize>, Vec<usize>) {
    let roll = |j: u64| skywave_core::hash64(seed, &[0x5041_494e, s as u64, j]);
    let start = (roll(0) % n_own as u64) as usize;
    let own: Vec<usize> = (0..OWN_PAINTS.min(n_own))
        .map(|k| (start + k) % n_own)
        .collect();
    let mut era = Vec::with_capacity(ERA_PAINTS);
    for j in 1..64 {
        let k = era_index(roll(j) as u32);
        if !era.contains(&k) {
            era.push(k);
            if era.len() == ERA_PAINTS {
                break;
            }
        }
    }
    (own, era)
}

/// The [`vehicles::BUILTIN_SHAPES`] index a parked car of `vehicle_class` in `zone` is
/// drawn as, picked from its [`PARKED`] list by `roll` (any number), each entry's weight
/// times the world's era weight of the shape (`era`, per built-in shape:
/// [`crate::scenario::Lineup::shapes`], 0 for a car the world does not have) times the
/// shape's weight in that zone ([`vehicles::zone_weights`]). Equal weights pick as the list
/// alone does ([`crate::scenario::reduced`]); a list the world has none of falls back to the
/// world's heaviest car.
fn parked_shape(vehicle_class: u8, roll: u32, era: &[u32], zone: place::ZoneId) -> usize {
    parked_pick(
        PARKED
            .get(usize::from(vehicle_class & 15))
            .copied()
            .unwrap_or(&[("homestead_wagon", 1)]),
        roll,
        era,
        zone,
    )
}

/// The shape of the parked car of `vehicle_class` at `pos` (its seeded hash `h`), by the
/// zone it stands in.
fn parked_at(place: &Place, pos: [f32; 3], vehicle_class: u8, h: u64, era: &[u32]) -> usize {
    let zone = world::zone_at(place, world::math::V2::new(pos[0], pos[2]));
    parked_shape(vehicle_class, (h >> 44) as u32, era, zone)
}

/// [`parked_shape`] over one list.
fn parked_pick(list: &[(&str, u32)], roll: u32, era: &[u32], zone: place::ZoneId) -> usize {
    let index = |id: &str| {
        vehicles::BUILTIN_SHAPES
            .iter()
            .position(|(file, _)| file.strip_suffix(".shape.ron") == Some(id))
            .unwrap_or(0)
    };
    let mut weights = [0u32; 8];
    let weights = &mut weights[..list.len().min(8)];
    for (w, &(id, k)) in weights.iter_mut().zip(list) {
        let z = u32::from(vehicles::zone_weights(id)[zone as usize]);
        *w = k * z * era.get(index(id)).copied().unwrap_or(0);
    }
    crate::scenario::reduced(weights);
    let total: u32 = weights.iter().sum();
    if total == 0 {
        let heaviest = era.iter().copied().max().unwrap_or(0);
        return era.iter().position(|&w| w == heaviest).unwrap_or(0);
    }
    let mut r = roll % total;
    for (&w, &(id, _)) in weights.iter().zip(list) {
        if r < w {
            return index(id);
        }
        r -= w;
    }
    index(list[0].0)
}

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

/// The instanced kinds but cars, in mesh order. A bench or a trash can is one of a few
/// street-clutter designs (`world::props::PropKit::instanced`): eight shapes each, so the
/// designs come out in about their shares.
const LOOKS: [Look; 10] = {
    use world::props::PropKind as K;
    [
        look(K::PalmTree, VARIANTS, true, 420.0, 4.0, 7.0),
        look(K::Oak, VARIANTS, true, 420.0, 4.0, 3.5),
        look(K::Eucalyptus, VARIANTS, true, 420.0, 5.0, 9.0),
        look(K::Hydrant, 1, false, 200.0, 0.5, 0.4),
        look(K::Bench, 8, false, 200.0, 1.6, 1.3),
        look(K::TrashCan, 8, false, 200.0, 0.7, 0.6),
        look(K::Mailbox, 8, false, 200.0, 0.5, 0.7),
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
    /// The mesh beyond [`CAR_DETAIL_RADIUS_M`] (a parked car's level 2; `mesh` otherwise).
    far: u16,
    /// Vertex color multiplier (a parked car's wear).
    tint: [f32; 4],
    transform: [[f32; 4]; 4],
    bounds: Bounds,
    chunk: [i32; 2],
    /// Squared draw distance.
    reach2: f32,
    /// A parked car: it sways when knocked ([`world::collide::Rocking`]).
    car: bool,
    /// A parking lot's car: its index into the collider's lot cars
    /// ([`world::WorldCollider::lot_cars`], in `Props` order), drawn only while there;
    /// [`world::collide::NO_LOT`] for every other prop.
    lot: u32,
}

/// The instanced props of a place (see the module docs).
pub struct PropInstances {
    meshes: Vec<Mesh>,
    ids: Vec<MeshId>,
    /// Sorted by mesh.
    entries: Vec<Entry>,
    /// The street lights' mesh.
    lamp_mesh: u16,
    /// Per shape, per paint (its palette, then the era's): the parked car's level-1 mesh
    /// (level 2 is the next), when the place parks it.
    cars: Vec<Vec<Option<u16>>>,
}

impl PropInstances {
    /// The instanced props of `place`, each filed under the chunk `chunk_of` gives for its
    /// ground point, its parked cars picked by the world's era weights `era` (per built-in
    /// shape; see [`parked_shape`]), its hero cars from `heroes` (built-in shape indices,
    /// the world's heroes it has). `None` when it has none.
    pub fn new(
        place: &Place,
        palette: &Palette,
        era: &[u32],
        heroes: &[usize],
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
        let kit = world::props::PropKit::instanced();
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
        // Car meshes are made for the (shape, paint) pairs the place uses, from its few
        // paints per shape.
        let paints: Vec<(Vec<usize>, Vec<usize>)> = shapes
            .iter()
            .enumerate()
            .map(|(k, sh)| place_paints(seed, k, sh.palette.len()))
            .collect();
        let mut car_mesh: Vec<Vec<Option<u16>>> = shapes
            .iter()
            .map(|s| vec![None; s.palette.len() + shape::ERA_1980S_PAINT.len()])
            .collect();
        let mut entries = Vec::with_capacity(items.len());
        let mut lots = 0u32;
        for p in items {
            let pos = Vec3::from(p.pos);
            let h = skywave_core::hash64(
                seed,
                &[u64::from(p.pos[0].to_bits()), u64::from(p.pos[2].to_bits())],
            );
            let chunk = chunk_of([p.pos[0], p.pos[2]]);
            let car = matches!(
                p.kind,
                PropKind::ParkedCar { .. } | PropKind::HeroCar { .. }
            );
            let mut tint = [1.0; 4];
            let mut far = u16::MAX;
            let (mesh, transform, bounds, reach) = match p.kind {
                PropKind::ParkedCar { .. } | PropKind::HeroCar { .. } => {
                    let (s, paint, era) = match p.kind {
                        PropKind::HeroCar { slot } if !heroes.is_empty() => {
                            // A hero car in one of its own colors, kept clean.
                            let s = heroes[usize::from(slot) % heroes.len()] % shapes.len();
                            let n_own = shapes[s].palette.len();
                            (s, (h >> 32) as usize % n_own, None)
                        }
                        kind => {
                            let vehicle_class = match kind {
                                PropKind::ParkedCar { vehicle_class, .. } => vehicle_class,
                                _ => 0,
                            };
                            let s = parked_at(place, p.pos, vehicle_class, h, era) % shapes.len();
                            // A third in the car's own palette, the rest in the era's street
                            // colors.
                            let n_own = shapes[s].palette.len();
                            let nibble = usize::from(vehicle_class >> 4);
                            let (own, era_set) = &paints[s];
                            if nibble < OWN_PAINT_NIBBLES {
                                (s, own[nibble % own.len()], None)
                            } else {
                                let k = era_set[((h >> 32) as usize) % era_set.len()];
                                (s, n_own + k, Some(shape::ERA_1980S_PAINT[k].0))
                            }
                        }
                    };
                    let id = match car_mesh[s][paint] {
                        Some(id) => id,
                        None => {
                            let own = if era.is_some() { 0 } else { paint };
                            // Level 1, then level 2 right after it (`far` is `mesh + 1`).
                            for lod in [1, 2] {
                                meshes.push(car::parked_mesh(&shapes[s], own, era, lod)?);
                            }
                            let id = (meshes.len() - 2) as u16;
                            car_mesh[s][paint] = Some(id);
                            id
                        }
                    };
                    far = id + 1;
                    let m = Mat4::from_rotation_translation(Quat::from_rotation_y(p.yaw), pos);
                    let b = Bounds::cube(pos + Vec3::Y, 2.8);
                    // Road dust and sun wear: a little darker and warmer, up to 14%; a hero
                    // car is kept clean.
                    let wear = if matches!(p.kind, PropKind::HeroCar { .. }) {
                        0.0
                    } else {
                        MAX_WEAR
                    };
                    let d = ((h >> 12) & 255) as f32 / 255.0 * wear;
                    tint = [1.0 - d, 1.0 - d * 1.06, 1.0 - d * 1.2, 1.0];
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
                far: if far == u16::MAX { mesh } else { far },
                tint,
                transform: transform.to_cols_array_2d(),
                bounds,
                chunk,
                reach2: reach * reach,
                car,
                lot: match p.kind {
                    PropKind::ParkedCar { hours, .. } if hours != place::ALL_HOURS => {
                        lots += 1;
                        lots - 1
                    }
                    _ => world::collide::NO_LOT,
                },
            });
        }
        // Stable, so each mesh's instances keep the table's order.
        entries.sort_by_key(|e| e.mesh);
        Ok(Some(PropInstances {
            meshes,
            ids: Vec::new(),
            entries,
            lamp_mesh,
            cars: car_mesh,
        }))
    }

    /// The registered ids of the parked car `shape` in era paint `era` (an index into
    /// `shape::ERA_1980S_PAINT`) at levels 1 and 2, when the place parks one.
    pub fn car_ids(&self, shape: usize, era: usize) -> Option<[MeshId; 2]> {
        let paints = self.cars.get(shape)?;
        let n_own = paints.len() - shape::ERA_1980S_PAINT.len();
        let m = usize::from((*paints.get(n_own + era)?)?);
        Some([*self.ids.get(m)?, *self.ids.get(m + 1)?])
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
    /// [`LIT_RADIUS_M`]; street lamp lenses glow by `lamp_glow`; parked cars `rocking`
    /// knocked lean on their springs, and a lot's are there only while `lot_cars` (the
    /// collider's) says so. Returns how many.
    #[allow(clippy::too_many_arguments)]
    pub fn push(
        &self,
        eye: Vec3,
        live: impl Fn([i32; 2]) -> bool,
        lights: &FrameLights,
        lamp_glow: f32,
        rocking: &world::collide::Rocking,
        lot_cars: &[world::collide::LotCar],
        out: &mut Vec<MeshInstance>,
    ) -> usize {
        let mut n = 0;
        let lit2 = LIT_RADIUS_M * LIT_RADIUS_M;
        let near2 = CAR_DETAIL_RADIUS_M * CAR_DETAIL_RADIUS_M;
        // Two passes so each mesh's instances stay one run: everything near (and every
        // prop with one level), then the far parked cars at their lower level.
        for far in [false, true] {
            for e in &self.entries {
                if n == MAX_PROP_INSTANCES {
                    return n;
                }
                let d2 = e.bounds.distance_squared(eye);
                if d2 > e.reach2 || !live(e.chunk) {
                    continue;
                }
                if e.lot != world::collide::NO_LOT
                    && !lot_cars.get(e.lot as usize).is_some_and(|c| c.present)
                {
                    continue;
                }
                let lod_far = e.far != e.mesh && d2 > near2;
                if lod_far != far {
                    continue;
                }
                let mesh = if far { e.far } else { e.mesh };
                let lights = if d2 < lit2 {
                    lights.set_for(&e.bounds)
                } else {
                    LightSet::EMPTY
                };
                // A parked car's lamps (built lit for the traffic) are off.
                let glow = if e.mesh == self.lamp_mesh {
                    lamp_glow
                } else if e.car {
                    0.0
                } else {
                    1.0
                };
                // A knocked parked car leans the same at either level of detail.
                let mut transform = e.transform;
                if e.car {
                    let at = [transform[3][0], transform[3][2]];
                    if let Some(tilt) = rocking.tilt_at(at) {
                        transform = tilted(transform, tilt);
                    }
                }
                out.push(MeshInstance {
                    lights,
                    glow,
                    color: e.tint,
                    ..MeshInstance::new(self.ids[usize::from(mesh)], transform)
                });
                n += 1;
            }
        }
        n
    }
}

/// `transform` leaned about its ground point by `tilt` (rad, toward world `+x` and `+z`).
fn tilted(transform: [[f32; 4]; 4], tilt: [f32; 2]) -> [[f32; 4]; 4] {
    let m = Mat4::from_cols_array_2d(&transform);
    let at = m.w_axis.truncate();
    let lean = Mat4::from_rotation_x(tilt[1]) * Mat4::from_rotation_z(-tilt[0]);
    (Mat4::from_translation(at) * lean * Mat4::from_translation(-at) * m).to_cols_array_2d()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Any zone: the 1980s cars weigh alike in all of them.
    const Z: place::ZoneId = place::ZoneId::Residential;

    /// The built-in shapes no place parks: the exotics, the sports cars' open versions and
    /// the 2001 hero cars, which only the player drives, and the two trucks too long for a
    /// stall, which only traffic drives.
    const UNPARKED: [&str; 17] = [
        "basilisk_v12",
        "basilisk_roadster",
        "avocet_v8",
        "avocet_spider",
        "longbow_convertible",
        "appaloosa_convertible",
        "halberd_coupe",
        "halberd_targa",
        "tanager_awd",
        "destrier_coupe",
        "siskin_coupe",
        "sledge_box_truck",
        "anvil_semi",
        "shrike_coupe",
        "glaive_turbo",
        "falchion_sedan",
        "courser_v8",
    ];

    #[test]
    fn parked_cars_fit_their_class_and_every_shape_parks() {
        let shapes: Vec<_> = vehicles::BUILTIN_SHAPES
            .iter()
            .map(|(_, text)| vehicles::load_shape(text).unwrap())
            .collect();
        // The shapes that never park are the ones traffic leaves out, and its two trucks.
        let unparked: Vec<&str> = (0..shapes.len())
            .filter(|k| !vehicles::TRAFFIC_SHAPES.contains(k) || shapes[*k].length() > 7.0)
            .map(|k| shapes[k].id.as_str())
            .collect();
        assert_eq!(unparked, UNPARKED);
        let mut parked = vec![false; shapes.len()];
        for (class, list) in PARKED.iter().enumerate() {
            let [box_len, box_w, _] = world::props::parked_car_size(class as u8);
            for &(id, w) in *list {
                let k = shapes.iter().position(|s| s.id == id).expect(id);
                assert!(w > 0);
                assert!(
                    (shapes[k].length() - box_len).abs() <= 0.5,
                    "{id} ({} m) parks in a {box_len} m box",
                    shapes[k].length()
                );
                // The world's collision box is the class's; a body may overhang it by a few
                // centimeters a side at most.
                let w = 2.0 * shapes[k].plan.iter().fold(0.0f32, |m, p| m.max(p.1));
                assert!(
                    w <= box_w + 0.1,
                    "{id} ({w} m wide) parks in a {box_w} m box"
                );
                parked[k] = true;
            }
            // Every entry is reachable in the 2001 world, which has every car.
            let era = &crate::scenario::lineup(1).shapes;
            let picked: Vec<usize> = (0..20_000u32)
                .map(|r| parked_shape(class as u8, r * 7919, era, Z))
                .collect();
            for &(id, _) in *list {
                assert!(picked.iter().any(|&k| shapes[k].id == id), "{id}");
            }
        }
        for (k, s) in shapes.iter().enumerate() {
            assert_eq!(
                parked[k],
                !UNPARKED.contains(&s.id.as_str()),
                "{} parks somewhere unless it is one of UNPARKED",
                s.id
            );
        }
        let era = &crate::scenario::lineup(0).shapes;
        assert_eq!(shapes[parked_shape(15, 7, era, Z)].id, "homestead_wagon");
    }

    /// The pick before era weights: by the list's own weights alone, over the 1980s cars
    /// (the list as it was before the 2001 cars joined its end).
    fn unweighted(list: &[(&str, u32)], roll: u32) -> &'static str {
        let eighties = list
            .iter()
            .take_while(|e| vehicles::builtin_spec(e.0).unwrap().year < 1990)
            .count();
        assert!(list[eighties..]
            .iter()
            .all(|e| vehicles::builtin_spec(e.0).unwrap().year > 1990));
        let list = &list[..eighties];
        let total: u32 = list.iter().map(|e| e.1).sum();
        let mut r = roll % total;
        for &(id, w) in list {
            if r < w {
                return vehicles::BUILTIN.iter().find(|b| b.0 == id).unwrap().0;
            }
            r -= w;
        }
        unreachable!()
    }

    #[test]
    fn the_1984_world_parks_as_before_era_weights() {
        let era = &crate::scenario::lineup(0).shapes;
        for (class, list) in PARKED.iter().enumerate() {
            for roll in (0..5000).chain([u32::MAX - 3, u32::MAX]) {
                let k = parked_shape(class as u8, roll, era, Z);
                let file = vehicles::BUILTIN_SHAPES[k].0;
                assert_eq!(
                    file.strip_suffix(".shape.ron"),
                    Some(unweighted(list, roll)),
                    "class {class} roll {roll}"
                );
            }
        }
    }

    #[test]
    fn a_2001_car_never_parks_in_1984() {
        // The Halcyon dated 2001: it parks in the 2001 world, never in 1984, and the rest of
        // its lists take its place.
        let k = vehicles::BUILTIN_SHAPES
            .iter()
            .position(|b| b.0 == "halcyon_turbo.shape.ron")
            .unwrap();
        let l84 = crate::scenario::lineup_with_2001_car(0, "halcyon_turbo");
        let l01 = crate::scenario::lineup_with_2001_car(1, "halcyon_turbo");
        let (mut in84, mut in01) = (0, 0);
        for class in 0..16u8 {
            for roll in 0..2000 {
                in84 += usize::from(parked_shape(class, roll, &l84.shapes, Z) == k);
                in01 += usize::from(parked_shape(class, roll, &l01.shapes, Z) == k);
            }
        }
        assert_eq!(in84, 0);
        assert!(in01 > 0);
        // A list of only cars the world does not have parks its heaviest car.
        let mut only = vec![0; vehicles::BUILTIN_SHAPES.len()];
        only[3] = 7;
        assert_eq!(parked_pick(&[("halcyon_turbo", 1)], 5, &only, Z), 3);
    }

    #[test]
    fn a_world_cut_inside_the_roster_parks_only_its_cars() {
        // Years (1979, 1983): every shape of an allowed car that parks anywhere parks,
        // every shape of a newer car never does.
        let l = crate::scenario::lineup_with(0, Some((1979, 1983)), |_| None);
        let mut parked = vec![0u32; vehicles::BUILTIN_SHAPES.len()];
        for class in 0..16u8 {
            for roll in 0..4000 {
                parked[parked_shape(class, roll, &l.shapes, Z)] += 1;
            }
        }
        for (k, (file, _)) in vehicles::BUILTIN_SHAPES.iter().enumerate() {
            let id = file.strip_suffix(".shape.ron").unwrap();
            let listed = PARKED.iter().any(|list| list.iter().any(|e| e.0 == id));
            assert_eq!(
                parked[k] > 0,
                l.shapes[k] > 0 && listed,
                "{id}: parked {} times, weight {}",
                parked[k],
                l.shapes[k]
            );
        }
    }

    #[test]
    fn a_place_parks_each_shape_in_a_few_paints() {
        for (s, n_own) in [(0, 7), (3, 1), (11, 2)] {
            let (own, era) = place_paints(1, s, n_own);
            assert_eq!(own.len(), OWN_PAINTS.min(n_own));
            assert!(own.iter().all(|&k| k < n_own));
            assert_eq!(era.len(), ERA_PAINTS);
            assert!(era.iter().all(|&k| k < shape::ERA_1980S_PAINT.len()));
            assert!((1..era.len()).all(|i| !era[..i].contains(&era[i])));
        }
        // The mix varies from place to place.
        assert!((0..8).any(|seed| place_paints(seed, 0, 7) != place_paints(1, 0, 7)));
    }

    /// Bytes of parked- and NPC-car mesh on the coast (seed 1), vertices plus indices: 22.9 MB with
    /// the four first shapes in every paint the place used, 65 MB for twelve shapes that
    /// way, 21.5 MB for twelve in at most six paints each, 24.8 MB for the fourteen that
    /// park (the sports cars' two common coupes added). The ceiling is ~1.2x the four
    /// (`docs/PERFORMANCE.md`).
    const PARKED_MESH_BYTES_MAX: usize = 27_000_000;
    /// The same in the 2001 world (the coast's place until the basin's lands): the 1980s
    /// cars and the 2001 traffic cars, nineteen shapes parked and 21 driving, 36.3 MB
    /// (33.0 parked, 3.3 for the traffic's own, most of it the two trucks); the ceiling is
    /// ~1.2x that.
    const PARKED_MESH_BYTES_MAX_2001: usize = 44_000_000;

    /// Parked- and traffic-car meshes of scenario `scenario`'s world on the coast at seed 1:
    /// counts asserted against the paint caps, and the bytes.
    fn parked_and_traffic_bytes(scenario: usize) -> usize {
        let place =
            crate::places::build(crate::scene::PlaceKey::Coast, skywave_core::Seed(1)).unwrap();
        let era = &crate::scenario::lineup(scenario).shapes;
        let mut props = PropInstances::new(&place, &Palette::socal_1985(), era, &[], |_| [0, 0])
            .unwrap()
            .unwrap();
        let ids = (0..props.meshes().len() as u32).map(MeshId).collect();
        props.set_ids(ids);
        let first_car: usize = LOOKS.iter().map(|l| l.variants as usize).sum();
        let cars = &props.meshes()[first_car..];
        let bytes: usize = cars
            .iter()
            .map(|m| {
                m.vertices.len() * core::mem::size_of::<render::Vertex>()
                    + m.indices.len() * core::mem::size_of::<u32>()
            })
            .sum();
        let tris: usize = cars.iter().map(|m| m.indices.len() / 3).sum();
        eprintln!(
            "world {scenario}: parked cars: {} meshes, {tris} triangles, {bytes} bytes",
            cars.len()
        );
        let parking = (0..vehicles::BUILTIN_SHAPES.len())
            .filter(|&k| era[k] > 0 && !UNPARKED.contains(&shape_id(k)))
            .count();
        assert!(cars.len() <= parking * (OWN_PAINTS + ERA_PAINTS) * 2);
        // The traffic takes the parked cars' era paints, sharing their meshes; it builds
        // only the (shape, paint) pairs the place does not park, inside the same budget.
        let mut npcs = crate::npc::Npcs::new(&place, skywave_core::Seed(1), era);
        let mut built = 0;
        npcs.build_meshes(Some(&props), |_| {
            built += 1;
            MeshId(u32::MAX)
        })
        .unwrap();
        let npc = npcs.built_bytes();
        // The pedestrian figures' meshes have their own budget (`peds`).
        let built = built - crate::peds::VARIANTS * traffic::figure::PARTS;
        let driving = usize::from(npcs.budget_shapes());
        eprintln!("world {scenario}: traffic: {driving} shapes, {built} more meshes, {npc} bytes");
        // Plus each traffic shape's brake lamp overlay at both levels, and the meshes of the
        // shapes that drive and never park.
        let unparked_drivers = (0..vehicles::BUILTIN_SHAPES.len())
            .filter(|&k| era[k] > 0 && vehicles::TRAFFIC_SHAPES.contains(&k))
            .filter(|&k| UNPARKED.contains(&shape_id(k)))
            .count();
        assert!(
            cars.len() + built
                <= parking * (OWN_PAINTS + ERA_PAINTS) * 2
                    + driving * 2
                    + unparked_drivers * ERA_PAINTS * 2
        );
        bytes + npc
    }

    fn shape_id(k: usize) -> &'static str {
        vehicles::BUILTIN_SHAPES[k]
            .0
            .strip_suffix(".shape.ron")
            .unwrap()
    }

    #[test]
    fn parked_car_meshes_stay_within_budget() {
        let bytes = parked_and_traffic_bytes(0);
        assert!(bytes <= PARKED_MESH_BYTES_MAX, "{bytes} bytes");
        let bytes = parked_and_traffic_bytes(1);
        assert!(bytes <= PARKED_MESH_BYTES_MAX_2001, "{bytes} bytes in 2001");
    }

    /// Every parked car on the coast (seed 1) in scenario `scenario`'s world: its shape
    /// and zone, as [`PropInstances::new`] picks them.
    fn parked_census(scenario: usize) -> Vec<(usize, place::ZoneId)> {
        let place =
            crate::places::build(crate::scene::PlaceKey::Coast, skywave_core::Seed(1)).unwrap();
        let era = &crate::scenario::lineup(scenario).shapes;
        let seed = place.header.seed;
        place
            .props
            .items
            .iter()
            .filter_map(|p| match p.kind {
                PropKind::ParkedCar { vehicle_class, .. } => {
                    let h = skywave_core::hash64(
                        seed,
                        &[u64::from(p.pos[0].to_bits()), u64::from(p.pos[2].to_bits())],
                    );
                    let zone = world::zone_at(&place, world::math::V2::new(p.pos[0], p.pos[2]));
                    Some((parked_at(&place, p.pos, vehicle_class, h, era), zone))
                }
                _ => None,
            })
            .collect()
    }

    #[test]
    fn the_2001_traffic_cars_park_in_2001_by_zone_and_never_in_1984() {
        let ids = [
            "plover_sedan",
            "sorrel_utility",
            "wren_minivan",
            "percheron_pickup",
            "linnet_hatch",
        ];
        let index = |id: &str| {
            vehicles::BUILTIN_SHAPES
                .iter()
                .position(|b| b.0.strip_suffix(".shape.ron") == Some(id))
                .unwrap()
        };
        let c84 = parked_census(0);
        let c01 = parked_census(1);
        assert!(c84.len() > 100 && c84.len() == c01.len());
        for id in ids.iter().chain(&["sledge_box_truck", "anvil_semi"]) {
            assert!(c84.iter().all(|c| c.0 != index(id)), "{id} parks in 1984");
        }
        for id in ids {
            let n = c01.iter().filter(|c| c.0 == index(id)).count();
            eprintln!("{id}: {n} of {} parked", c01.len());
            assert!(n > 0, "{id} never parks in 2001");
        }
        for id in ["sledge_box_truck", "anvil_semi"] {
            assert!(c01.iter().all(|c| c.0 != index(id)), "{id} parks");
        }
        // The 2001 cars outnumber the old ones, which still park at their age weight.
        let new = c01.iter().filter(|c| c.0 >= 25).count();
        assert!(
            new * 2 > c01.len() && new < c01.len(),
            "{new} of {}",
            c01.len()
        );
        // By zone: the pickup's weight is 5 in industrial zones and 2 in residential ones,
        // the minivan's 1 and 4, against the old cars' 2 everywhere.
        let w = |id: &str, z: place::ZoneId| vehicles::zone_weights(id)[z as usize];
        use place::ZoneId::{Industrial, Residential};
        assert!(w("percheron_pickup", Industrial) > w("percheron_pickup", Residential));
        assert!(w("wren_minivan", Residential) > w("wren_minivan", Industrial));
        let picks = |id: &str, z: place::ZoneId| {
            let era = &crate::scenario::lineup(1).shapes;
            // Rolls spread over the weights' whole range.
            (0..20_000u32)
                .filter(|&r| parked_shape(1, r * 7919, era, z) == index(id))
                .count()
        };
        assert!(picks("percheron_pickup", Industrial) > 2 * picks("percheron_pickup", Residential));
        assert!(picks("wren_minivan", Residential) > 2 * picks("wren_minivan", Industrial));
    }
}
