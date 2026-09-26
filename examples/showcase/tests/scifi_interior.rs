//! The sci-fi kit's interiors, held to what the house's are held to: every
//! room has a floor under it and a ceiling or dome over it, a point in each
//! room is found as that room, no collision proxy stands in a room, every
//! portal is a flat rectangle, the openings a room names are published, and
//! glazing and furniture stay out of merged storeys.
#![allow(clippy::unwrap_used, reason = "fixtures built in the test")]
use ashlar::{Building, Room, Side};
use ashlar_manifold::{ManifoldMesher, mesh_building};
use ashlar_showcase::{Scene, building, scifi};
use glam::{DVec2, DVec3};

/// Every building the kit publishes with rooms: each enterable piece alone,
/// and the three scenes.
fn buildings() -> Vec<(String, Building)> {
    let mut all: Vec<(String, Building)> = scifi::Habitat::ALL
        .iter()
        .map(|habitat| {
            (
                habitat.name().to_owned(),
                scifi::piece(habitat.name()).unwrap(),
            )
        })
        .collect();
    for scene in [Scene::ScifiKit, Scene::ScifiOutpost, Scene::ScifiColony] {
        all.push((format!("{scene:?}"), building(scene).unwrap()));
    }
    all
}

/// The centre of a room, in building space.
fn centre(room: &Room) -> DVec3 {
    room.pose
        .transform_point(DVec3::from_array(room.size) / 2.0)
}

/// The floor under a room's centre, in building space.
fn floor(room: &Room) -> DVec3 {
    room.pose
        .transform_point(DVec3::new(room.size[0] / 2.0, 0.0, room.size[2] / 2.0))
}

fn inside_xz(point: DVec2, triangle: [DVec3; 3]) -> bool {
    let [a, b, c] = triangle.map(|corner| DVec2::new(corner.x, corner.z));
    let side = |from: DVec2, to: DVec2| {
        (to.x - from.x) * (point.y - from.y) - (to.y - from.y) * (point.x - from.x)
    };
    let (ab, bc, ca) = (side(a, b), side(b, c), side(c, a));
    (ab >= 0.0 && bc >= 0.0 && ca >= 0.0) || (ab <= 0.0 && bc <= 0.0 && ca <= 0.0)
}

#[test]
fn every_room_is_found_and_has_a_floor_and_a_ceiling() {
    for (name, building) in buildings() {
        assert!(
            !building.rooms().is_empty() || name == "Tube",
            "{name} has no rooms"
        );
        let meshed = mesh_building(&building, &ManifoldMesher::default()).unwrap();
        let triangles: Vec<[DVec3; 3]> = meshed
            .pieces()
            .filter(|piece| piece.side == Side::Interior)
            .flat_map(|piece| {
                piece
                    .mesh
                    .triangles()
                    .map(move |triangle| triangle.map(|corner| piece.pose.transform_point(corner)))
            })
            .collect();
        for room in building.rooms() {
            let middle = centre(room);
            assert!(room.contains(middle), "{name}/{}", room.id);
            assert_eq!(
                building.room_at(middle).map(|found| found.id.as_str()),
                Some(room.id.as_str()),
                "{name}: the centre of {} is found in another room",
                room.id
            );
            let under = floor(room);
            let point = DVec2::new(under.x, under.z);
            let normal = |triangle: &[DVec3; 3]| {
                (triangle[1] - triangle[0])
                    .cross(triangle[2] - triangle[0])
                    .normalize()
            };
            assert!(
                triangles.iter().any(|triangle| {
                    normal(triangle).y > 0.9
                        && triangle
                            .iter()
                            .all(|corner| (corner.y - under.y).abs() < 0.08)
                        && inside_xz(point, *triangle)
                }),
                "{name}/{} has no interior floor under its centre",
                room.id
            );
            assert!(
                triangles.iter().any(|triangle| {
                    normal(triangle).y < -0.3
                        && triangle.iter().all(|corner| corner.y > middle.y)
                        && inside_xz(point, *triangle)
                }),
                "{name}/{} has no interior ceiling over its centre",
                room.id
            );
        }
    }
}

#[test]
fn no_collision_proxy_stands_in_a_room() {
    // Conservative: each proxy's box in its own part's frame is larger than
    // the proxy, so a room centre outside every such box is outside every
    // proxy, however the instance is turned.
    for (name, building) in buildings() {
        let meshed = mesh_building(&building, &ManifoldMesher::default()).unwrap();
        for room in building.rooms() {
            let middle = centre(room);
            for instance in &building.recipe().instances {
                let local = instance.pose.inverse().transform_point(middle);
                for collider in meshed
                    .part_colliders
                    .get(&instance.part)
                    .into_iter()
                    .flatten()
                {
                    let Some([low, high]) = collider.solid.bounds() else {
                        continue;
                    };
                    let inside =
                        (0..3).all(|axis| local[axis] > low[axis] && local[axis] < high[axis]);
                    assert!(
                        !inside,
                        "{name}: {}/{}'s proxy stands in {}",
                        instance.id, collider.id, room.id
                    );
                }
            }
        }
    }
}

#[test]
fn portals_are_flat_and_every_named_opening_is_published() {
    for (name, building) in buildings() {
        let meshed = mesh_building(&building, &ManifoldMesher::default()).unwrap();
        let portals = meshed.portals();
        for portal in &portals {
            let normal = (portal.corners[1] - portal.corners[0])
                .cross(portal.corners[2] - portal.corners[0])
                .normalize();
            for corner in portal.corners {
                let off = (corner - portal.corners[0]).dot(normal).abs();
                assert!(
                    off < 1e-6,
                    "{name}: {} corner is {off} off its plane",
                    portal.id
                );
            }
            // A door is at least the house's 1.0 by 2.1.
            if portal.id.contains("door") || portal.id.contains("lock") || portal.id == "hatch" {
                let width = (portal.corners[1] - portal.corners[0]).length();
                let height = (portal.corners[3] - portal.corners[0]).length();
                let (short, long) = (width.min(height), width.max(height));
                assert!(
                    short >= 0.99 && long >= 2.09,
                    "{name}: {} is {short} by {long}",
                    portal.id
                );
            }
        }
        for room in building.rooms() {
            for id in &room.portals {
                assert!(
                    portals.iter().any(|portal| &portal.id == id),
                    "{name}: {} names {id}, which no cutter publishes",
                    room.id
                );
            }
        }
    }
}

#[test]
fn glazing_and_furniture_are_standalone() {
    for part in scifi::parts().unwrap() {
        for element in &part.elements {
            if element.material_slot == "glass" || part.id.starts_with("scifi:crate-small") {
                assert!(
                    element.standalone,
                    "{}/{} is not standalone",
                    part.id, element.id
                );
            }
        }
    }
    for part in scifi::furniture::parts().unwrap() {
        for element in &part.elements {
            assert!(element.standalone, "{}/{}", part.id, element.id);
            assert_eq!(element.side, Side::Interior, "{}/{}", part.id, element.id);
        }
    }
}
