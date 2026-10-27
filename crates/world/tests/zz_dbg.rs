use skywave_core::Seed;
use world::procgen::build_place;
use world::template::RegionTemplate;
#[test]
fn zz() {
    let t: RegionTemplate =
        ron::from_str(include_str!("../../../assets/regions/california_coast.ron")).unwrap();
    let p = build_place(Seed(3), &t);
    for (i, n) in p.roads.nodes.iter().enumerate() {
        if (n.pos[0] - 991.7).hypot(n.pos[1] - 788.7) < 45.0 {
            println!("node {i} {:?}", n.pos);
            for (k, e) in p.roads.edges.iter().enumerate() {
                if e.a as usize == i || e.b as usize == i {
                    println!(
                        "   edge {k} {:?} w {} {:?}",
                        e.class,
                        e.width_m,
                        e.profile
                            .iter()
                            .take(6)
                            .map(|q| [q[0] as i32, q[2] as i32])
                            .collect::<Vec<_>>()
                    );
                }
            }
        }
    }
}
