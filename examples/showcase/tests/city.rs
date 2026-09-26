//! The city kit: every piece is a closed solid, every scene builds, meshes and
//! binds, the interiors are rooms a proxy does not fill, and a metropolis is
//! the same city for the same seed.
#![allow(clippy::unwrap_used, reason = "test fixtures")]
use std::collections::BTreeMap;

use ashlar::{GeometryMesher, Side, TriangleMesh};
use ashlar_manifold::{ManifoldMesher, mesh_building};
use ashlar_showcase::{Scene, building, city, library};
use glam::{DVec2, DVec3};

/// Every undirected edge of an unwelded mesh, keyed by its quantised end
/// points, appears exactly twice and in opposite directions.
fn closed(mesh: &TriangleMesh) -> bool {
    #[allow(clippy::cast_possible_truncation, reason = "quantised micrometres")]
    let key = |p: glam::DVec3| {
        let q = |v: f64| (v * 1e6).round() as i64;
        [q(p.x), q(p.y), q(p.z)]
    };
    let mut edges: BTreeMap<([i64; 3], [i64; 3]), i32> = BTreeMap::new();
    for corners in mesh.corners() {
        let points = corners.map(|index| key(mesh.positions[index]));
        for (a, b) in [(0, 1), (1, 2), (2, 0)] {
            let (from, to) = (points[a], points[b]);
            if from < to {
                *edges.entry((from, to)).or_default() += 1;
            } else {
                *edges.entry((to, from)).or_default() -= 1;
            }
        }
    }
    edges.values().all(|count| *count == 0)
}

#[test]
fn every_piece_of_the_kit_is_a_closed_solid() {
    let mesher = ManifoldMesher::default();
    for part in city::parts().unwrap() {
        for element in &part.elements {
            let mut mesh = mesher
                .mesh(&element.geometry)
                .unwrap_or_else(|error| panic!("{}/{}: {error}", part.id, element.id));
            mesh.unweld();
            assert!(
                mesh.triangle_count() > 0,
                "{}/{} is empty",
                part.id,
                element.id
            );
            assert!(closed(&mesh), "{}/{} is not closed", part.id, element.id);
        }
    }
}

#[test]
fn every_scene_builds_meshes_and_binds_its_slots() {
    let definitions = library::materials();
    for scene in [
        Scene::CityKit,
        Scene::CityBlock,
        Scene::CityTower,
        Scene::CityLandmark,
        Scene::Metropolis,
    ] {
        let started = std::time::Instant::now();
        let building = building(scene).unwrap_or_else(|error| panic!("{scene:?}: {error}"));
        definitions
            .check_for(&building)
            .unwrap_or_else(|error| panic!("{scene:?}: {error}"));
        let meshed = mesh_building(&building, &ManifoldMesher::default())
            .unwrap_or_else(|error| panic!("{scene:?}: {error}"));
        let unique: usize = meshed
            .parts
            .values()
            .flatten()
            .map(|element| element.mesh.triangle_count())
            .sum();
        let recipe = building.recipe();
        println!(
            "{scene:?}: {unique} unique triangles, {} instances, {} rooms, {:.1?}",
            recipe.instances.len(),
            recipe.rooms.len(),
            started.elapsed()
        );
        assert!(unique < 400_000, "{scene:?}: {unique}");
    }
}

#[test]
#[ignore = "an eight by eight city; run it by name to measure it"]
fn the_large_metropolis_builds_and_meshes() {
    let started = std::time::Instant::now();
    let building = building(Scene::MetropolisLarge).unwrap();
    library::materials().check_for(&building).unwrap();
    let meshed = mesh_building(&building, &ManifoldMesher::default()).unwrap();
    let unique: usize = meshed
        .parts
        .values()
        .flatten()
        .map(|element| element.mesh.triangle_count())
        .sum();
    let placed: usize = building
        .recipe()
        .instances
        .iter()
        .map(|instance| {
            meshed.parts.get(&instance.part).map_or(0, |elements| {
                elements
                    .iter()
                    .map(|element| element.mesh.triangle_count())
                    .sum()
            })
        })
        .sum();
    println!(
        "MetropolisLarge: {unique} unique triangles, {placed} drawn, {} instances, {} rooms, {:.1?}",
        building.recipe().instances.len(),
        building.recipe().rooms.len(),
        started.elapsed()
    );
}

/// Whether `point` lies in the X/Z shadow of a triangle.
fn point_in_triangle_xz(point: DVec2, triangle: [DVec3; 3]) -> bool {
    let [a, b, c] = triangle.map(|v| DVec2::new(v.x, v.z));
    let side = |p: DVec2, q: DVec2, r: DVec2| (q - p).perp_dot(r - p);
    let (d1, d2, d3) = (side(a, b, point), side(b, c, point), side(c, a, point));
    let negative = d1 < 0.0 || d2 < 0.0 || d3 < 0.0;
    let positive = d1 > 0.0 || d2 > 0.0 || d3 > 0.0;
    !(negative && positive)
}

/// Whether an interior face at `level`, facing `up` or down, lies under or
/// over `point` on X/Z.
fn interior_face(meshed: &ashlar::MeshedBuilding, point: DVec2, level: f64, up: bool) -> bool {
    meshed
        .pieces()
        .filter(|piece| piece.side == Side::Interior)
        .any(|piece| {
            piece.mesh.corners().any(|corners| {
                let triangle =
                    corners.map(|index| piece.pose.transform_point(piece.mesh.positions[index]));
                let normal = (triangle[1] - triangle[0]).cross(triangle[2] - triangle[0]);
                normal.y.abs() > 0.99 * normal.length()
                    && (normal.y > 0.0) == up
                    && triangle.iter().all(|v| (v.y - level).abs() < 1e-4)
                    && point_in_triangle_xz(point, triangle)
            })
        })
}

#[test]
fn every_room_has_an_interior_floor_and_ceiling_and_no_proxy_fills_it() {
    for family in [
        city::Kind::Tower,
        city::Kind::Tenement,
        city::Kind::Landmark,
    ] {
        let building = city::tower_scene(family).unwrap();
        let meshed = mesh_building(&building, &ManifoldMesher::default()).unwrap();
        let colliders = meshed.colliders();
        let rooms = building.rooms();
        assert!(rooms.len() >= 8, "{family:?} has {} rooms", rooms.len());
        for room in rooms {
            let size = DVec3::from_array(room.size);
            let centre = room.pose.transform_point(size / 2.0);
            let floor = room
                .pose
                .transform_point(DVec3::new(size.x / 2.0, 0.0, size.z / 2.0));
            let ceiling = room
                .pose
                .transform_point(DVec3::new(size.x / 2.0, size.y, size.z / 2.0));
            let at = DVec2::new(centre.x, centre.z);
            assert!(
                interior_face(&meshed, at, floor.y, true),
                "{family:?}: {} has no interior floor at {floor}",
                room.id
            );
            assert!(
                interior_face(&meshed, at, ceiling.y, false),
                "{family:?}: {} has no interior ceiling at {ceiling}",
                room.id
            );
            for collider in &colliders {
                let [low, high] = collider.solid.bounds().unwrap();
                let inside =
                    (0..3).all(|axis| low[axis] < centre[axis] && centre[axis] < high[axis]);
                assert!(
                    !inside,
                    "{family:?}: {}/{} fills room {}",
                    collider.instance, collider.element, room.id
                );
            }
            assert!(
                building.room_storey(&room.id).is_some(),
                "{} has a storey",
                room.id
            );
        }
    }
}

#[test]
fn a_metropolis_is_the_same_city_for_the_same_seed() {
    let one = city::metropolis(11, [3, 2]).unwrap();
    let two = city::metropolis(11, [3, 2]).unwrap();
    let other = city::metropolis(12, [3, 2]).unwrap();
    assert_eq!(one.recipe(), two.recipe());
    assert_ne!(one.recipe().instances, other.recipe().instances);
}

#[test]
fn every_piece_scene_builds_and_binds() {
    let definitions = library::materials();
    let names = city::piece_names();
    assert!(names.len() >= 40, "{names:?}");
    for name in names {
        let building = city::piece(&name).unwrap_or_else(|error| panic!("{name}: {error}"));
        definitions
            .check_for(&building)
            .unwrap_or_else(|error| panic!("{name}: {error}"));
    }
}

#[test]
fn glazed_lobby_leaves_leave_the_portal_clear_of_static_collision() {
    let mut checked = 0;
    for part in city::parts()
        .unwrap()
        .into_iter()
        .filter(|part| part.elements.iter().any(|element| element.id == "portal"))
    {
        checked += 1;
        let leaves: Vec<_> = part
            .elements
            .iter()
            .filter(|element| element.id.starts_with("door-glass-"))
            .collect();
        assert_eq!(leaves.len(), 2, "{} needs a pair of glazed leaves", part.id);
        for element in part
            .elements
            .iter()
            .filter(|element| element.id.starts_with("door-"))
        {
            assert!(
                element.standalone,
                "{}/{} must move with its leaf",
                part.id, element.id
            );
            assert_eq!(element.collision, ashlar::Collision::None);
        }
        let building = city::piece(part.id.strip_prefix("city:").unwrap()).unwrap();
        let meshed = mesh_building(&building, &ManifoldMesher::default()).unwrap();
        // Sample both leaves' walking lanes, through the full surround depth.
        // Testing the published proxies catches a future surround or threshold
        // proxy that would fill the opening even if the leaves stay nonblocking.
        for leaf in leaves {
            assert_eq!(leaf.material_slot, "glass");
            let [low, high] = leaf.geometry.bounds().unwrap();
            let x = f64::midpoint(low.x, high.x);
            for z in [-0.4, 0.0, 0.3, 0.6, 0.9] {
                let point = DVec3::new(x, 1.5, z);
                for proxy in &meshed.part_colliders[&part.id] {
                    let [low, high] = proxy.solid.bounds().unwrap();
                    assert!(
                        !(point.cmpgt(low).all() && point.cmplt(high).all()),
                        "{}/{} blocks the entrance at {point}",
                        part.id,
                        proxy.id
                    );
                }
            }
        }
    }
    assert_eq!(
        checked, 5,
        "every tower family, landmark and tenement lobby"
    );
}

/// The 4 x 4 metropolis draws no more than its budget at any level of detail.
///
/// Detail is cheap to add one part at a time and expensive in a city of five
/// thousand instances, so the drawn totals are held here: level 0, what is
/// drawn within sixty metres, at 4.29 M triangles, and the coarse levels at
/// what they cost when the budget was set (2026-09-26). Raising a limit is a
/// decision to make on purpose, with the numbers in the commit message.
#[test]
fn metropolis_retains_the_recovered_draw_budget_at_every_level() {
    let city = building(Scene::Metropolis).unwrap();
    let limits = [4_290_131, 1_230_146, 785_082];
    for (lod, (policy, limit)) in ashlar::LodPolicy::ladder().iter().zip(limits).enumerate() {
        let level = city.simplified(policy).unwrap().unwrap();
        let mesh = mesh_building(&level, &ManifoldMesher::default()).unwrap();
        let drawn: usize = mesh.pieces().map(|piece| piece.mesh.triangle_count()).sum();
        assert!(drawn <= limit, "LOD {lod}: {drawn} exceeds {limit}");
    }
}
