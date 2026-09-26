//! The house's additive interior: the storey groups it merges into, the
//! finishes and provenance it keeps, and the rooms and portals it publishes.
use std::collections::BTreeSet;

use ashlar::{Collision, FaceOrigin, MergedGroup, Room, Side, TriangleMesh};
use ashlar_manifold::{ManifoldMesher, mesh_building};
use ashlar_showcase::interior;
use glam::{DVec2, DVec3};

/// The signed volume a closed surface encloses, exactly as
/// `crates/ashlar-manifold/tests/groups.rs` computes it.
fn volume(mesh: &TriangleMesh) -> f64 {
    mesh.triangles()
        .map(|triangle| triangle[0].dot(triangle[1].cross(triangle[2])) / 6.0)
        .sum()
}

#[test]
fn the_house_builds_and_merges_into_three_groups() {
    let building = interior::building().expect("the house");
    let meshed = mesh_building(&building, &ManifoldMesher::default()).expect("meshed");
    assert_eq!(meshed.groups.len(), 3, "one group per storey");
    let storeys: BTreeSet<i32> = meshed
        .groups
        .iter()
        .filter_map(|group| group.storey)
        .collect();
    assert_eq!(storeys, BTreeSet::from([0, 1, 2]), "storeys 0 to 2");
}

#[test]
fn liners_are_interior_batches() {
    let building = interior::building().expect("the house");
    let meshed = mesh_building(&building, &ManifoldMesher::default()).expect("meshed");
    for group in &meshed.groups {
        let has_wall = group
            .batches
            .iter()
            .any(|batch| batch.binding.material == "library:plaster");
        if !has_wall {
            continue;
        }
        assert!(
            group
                .batches
                .iter()
                .any(|batch| batch.side == Side::Interior),
            "{} has no interior finish",
            group.id
        );
        assert!(
            group
                .batches
                .iter()
                .any(|batch| batch.side == Side::Exterior),
            "{} has no exterior batch beside its wall",
            group.id
        );
    }
}

#[test]
fn the_doors_reveal_wears_its_own_slot() {
    let building = interior::building().expect("the house");
    let meshed = mesh_building(&building, &ManifoldMesher::default()).expect("meshed");
    let ground = meshed
        .groups
        .iter()
        .find(|group| group.id == "ground")
        .expect("a ground group");
    let mut metal = false;
    let mut reveal = false;
    for batch in &ground.batches {
        for source in &batch.mesh.sources {
            if matches!(source.origin, FaceOrigin::Cutter(_)) {
                metal |= batch.binding.material == "library:painted-metal";
                reveal |= batch.binding.material == "library:formed-concrete";
            }
        }
    }
    assert!(metal, "the door cutter wears its own metal reveal");
    assert!(reveal, "a window cutter falls back to the wall's cut slot");
}

#[test]
fn every_marked_opening_is_published_once_per_placement() {
    let building = interior::building().expect("the house");
    let meshed = mesh_building(&building, &ManifoldMesher::default()).expect("meshed");
    let portals = meshed.portals();
    let front: Vec<_> = portals
        .iter()
        .filter(|portal| portal.id == "front-door")
        .collect();
    assert_eq!(front.len(), 1, "one door, one ground placement");
    assert_eq!(front[0].storey, Some(0));
    assert_eq!(
        portals
            .iter()
            .filter(|portal| portal.id == "inner-door")
            .count(),
        2,
        "the partition is placed once per storey"
    );
}

#[test]
fn portals_lie_in_their_walls() {
    let building = interior::building().expect("the house");
    let meshed = mesh_building(&building, &ManifoldMesher::default()).expect("meshed");
    for portal in meshed.portals() {
        let normal = (portal.corners[1] - portal.corners[0])
            .cross(portal.corners[2] - portal.corners[0])
            .normalize();
        for corner in portal.corners {
            let out = (corner - portal.corners[0]).dot(normal).abs();
            assert!(out < 1e-9, "{} corner is {out} off its plane", portal.id);
        }
        assert!(
            (portal.normal.length() - 1.0).abs() < 1e-9,
            "{} normal is not unit",
            portal.id
        );
    }
}

#[test]
fn a_point_in_each_room_is_found() {
    let building = interior::building().expect("the house");
    for room in building.rooms() {
        let centre = room.pose.transform_point(DVec3::new(
            room.size[0] / 2.0,
            room.size[1] / 2.0,
            room.size[2] / 2.0,
        ));
        assert!(room.contains(centre), "{}", room.id);
        // The four rooms are disjoint, so the first room containing a point is
        // the one the point came from.
        assert_eq!(
            building.room_at(centre).map(|found| found.id.as_str()),
            Some(room.id.as_str())
        );
    }
}

#[test]
fn glass_is_standalone_and_walls_are_not() {
    let building = interior::building().expect("the house");
    let meshed = mesh_building(&building, &ManifoldMesher::default()).expect("meshed");
    for group in &meshed.groups {
        for batch in &group.batches {
            assert_ne!(
                batch.binding.material, "metro:glass",
                "{} drew its glazing from a group",
                group.id
            );
        }
    }
    assert!(
        meshed
            .parts
            .values()
            .flatten()
            .any(|element| element.material_slot == "glass"),
        "the glazing is drawn from the standalone parts"
    );
}

#[test]
fn colliders_are_convex_proxies_of_walls_and_floors() {
    let building = interior::building().expect("the house");
    let meshed = mesh_building(&building, &ManifoldMesher::default()).expect("meshed");
    let declared: usize = building
        .recipe()
        .instances
        .iter()
        .map(|instance| {
            building
                .part(&instance.part)
                .expect("a validated part")
                .elements
                .iter()
                .filter(|element| element.collision != Collision::None)
                .count()
        })
        .sum();
    assert!(declared > 0, "the house declares static collision");
    assert_eq!(meshed.colliders().len(), declared);
}

#[test]
fn the_whole_house_is_closed_per_group() {
    let building = interior::building().expect("the house");
    let meshed = mesh_building(&building, &ManifoldMesher::default()).expect("meshed");
    for group in &meshed.groups {
        let total: f64 = group.batches.iter().map(|batch| volume(&batch.mesh)).sum();
        assert!(total > 0.0, "{} encloses {total}, not a solid", group.id);
    }
}

/// The point at the centre of a room's footprint, in the XZ plane.
fn room_centre(room: &Room) -> DVec3 {
    room.pose
        .transform_point(DVec3::new(room.size[0] / 2.0, 0.0, room.size[2] / 2.0))
}

/// Whether a point in the XZ plane lies inside a triangle's XZ projection,
/// boundary included: the signs of the three edge cross products all agree.
fn point_in_triangle_xz(point: DVec2, triangle: [DVec3; 3]) -> bool {
    let [a, b, c] = triangle.map(|corner| DVec2::new(corner.x, corner.z));
    let side = |from: DVec2, to: DVec2| {
        (to.x - from.x) * (point.y - from.y) - (to.y - from.y) * (point.x - from.x)
    };
    let (ab, bc, ca) = (side(a, b), side(b, c), side(c, a));
    (ab >= 0.0 && bc >= 0.0 && ca >= 0.0) || (ab <= 0.0 && bc <= 0.0 && ca <= 0.0)
}

/// Whether the vertical line through `point` crosses an `Interior` face of
/// `group` that faces `along_y` and lies within 0.05 m of `level`.
fn crosses_face(group: &MergedGroup, point: DVec2, along_y: f64, level: f64) -> bool {
    group
        .batches
        .iter()
        .filter(|batch| batch.side == Side::Interior)
        .any(|batch| {
            batch.mesh.corners().any(|corners| {
                let triangle = corners.map(|index| batch.mesh.positions[index]);
                let normal = (triangle[1] - triangle[0])
                    .cross(triangle[2] - triangle[0])
                    .normalize();
                (normal.y - along_y).abs() < 1e-6
                    && (triangle[0].y - level).abs() < 0.05
                    && point_in_triangle_xz(point, triangle)
            })
        })
}

#[test]
fn every_room_has_an_interior_floor_and_ceiling() {
    let building = interior::building().expect("the house");
    let meshed = mesh_building(&building, &ManifoldMesher::default()).expect("meshed");
    for room in building.rooms() {
        let origin = room.pose.transform_point(DVec3::ZERO);
        let floor = origin.y;
        let ceiling = origin.y + room.size[1];
        let centre = room_centre(room);
        let group = meshed
            .groups
            .iter()
            .find(|group| Some(group.id.as_str()) == room.group.as_deref())
            .expect("the room's group is meshed");
        let storey = group.storey.expect("the room's group declares a storey");
        let point = DVec2::new(centre.x, centre.z);
        assert!(
            crosses_face(group, point, 1.0, floor),
            "{} has no interior floor",
            room.id
        );
        // A ceiling is the underside of the floor above, so it is meshed in the
        // group one storey up rather than in the room's own group.
        let ceiling_group = meshed
            .groups
            .iter()
            .find(|group| crosses_face(group, point, -1.0, ceiling))
            .unwrap_or_else(|| panic!("{} has no interior ceiling", room.id));
        assert_eq!(
            ceiling_group.storey,
            Some(storey + 1),
            "{} ceiling sits in {} rather than the storey above",
            room.id,
            ceiling_group.id
        );
    }
}

/// Whether an `Interior` face of `group` that faces up and lies more than
/// `margin` above `floor` covers `point`, the way a top-down camera looks
/// through the storey above it.
fn an_upward_face_covers(group: &MergedGroup, point: DVec2, floor: f64, margin: f64) -> bool {
    group
        .batches
        .iter()
        .filter(|batch| batch.side == Side::Interior)
        .any(|batch| {
            batch.mesh.corners().any(|corners| {
                let triangle = corners.map(|index| batch.mesh.positions[index]);
                let normal = (triangle[1] - triangle[0])
                    .cross(triangle[2] - triangle[0])
                    .normalize();
                normal.y > 1.0 - 1e-6
                    && triangle.iter().all(|corner| corner.y > floor + margin)
                    && point_in_triangle_xz(point, triangle)
            })
        })
}

/// Whether a triangle of `group` lies entirely in the horizontal plane at
/// `level` and covers `point` in the XZ plane: the seam where two flush solids
/// meet before they are unioned.
fn a_face_in_plane_covers(group: &MergedGroup, point: DVec2, level: f64) -> bool {
    group.batches.iter().any(|batch| {
        batch.mesh.corners().any(|corners| {
            let triangle = corners.map(|index| batch.mesh.positions[index]);
            triangle
                .iter()
                .all(|corner| (corner.y - level).abs() < 1e-6)
                && point_in_triangle_xz(point, triangle)
        })
    })
}

#[test]
fn a_top_down_game_can_see_into_the_storey_it_shows() {
    let building = interior::building().expect("the house");
    let meshed = mesh_building(&building, &ManifoldMesher::default()).expect("meshed");
    for room in building
        .rooms()
        .iter()
        .filter(|room| room.group.as_deref() == Some("ground"))
    {
        let floor = room.pose.transform_point(DVec3::ZERO).y;
        let centre = room_centre(room);
        let point = DVec2::new(centre.x, centre.z);
        for group in &meshed.groups {
            // Once the storeys above the ground one are hidden, only these
            // groups are drawn.
            if group.storey.is_none_or(|storey| storey > 0) {
                continue;
            }
            assert!(
                !an_upward_face_covers(group, point, floor, 0.1),
                "{} is covered from above in {}",
                room.id,
                group.id
            );
        }
    }
}

#[test]
fn a_ceiling_is_one_solid_with_its_slab() {
    let building = interior::building().expect("the house");
    let meshed = mesh_building(&building, &ManifoldMesher::default()).expect("meshed");
    let upper = meshed
        .groups
        .iter()
        .find(|group| group.id == "upper")
        .expect("the upper group");
    for room in building
        .rooms()
        .iter()
        .filter(|room| room.group.as_deref() == Some("ground"))
    {
        // The ceiling liner's top meets the upper floor slab's underside at the
        // ground room's ceiling level.
        let contact = room.pose.transform_point(DVec3::ZERO).y + room.size[1];
        let centre = room_centre(room);
        let point = DVec2::new(centre.x, centre.z);
        assert!(
            !a_face_in_plane_covers(upper, point, contact),
            "{} still has a seam between its ceiling and the slab above",
            room.id
        );
    }
}

#[test]
fn hiding_the_exterior_leaves_closed_rooms() {
    let building = interior::building().expect("the house");
    let meshed = mesh_building(&building, &ManifoldMesher::default()).expect("meshed");
    let ground = meshed
        .groups
        .iter()
        .find(|group| group.id == "ground")
        .expect("the ground group");
    // Projecting sills are exterior trim, not the footprint the floor must
    // cover. Measure against the structural slab in its instance frame.
    let floor = building.instance("floor-0").expect("ground floor");
    let slab = building
        .part(&floor.part)
        .expect("floor part")
        .elements
        .iter()
        .find(|element| element.id == "slab")
        .expect("structural slab");
    let footprint = slab
        .geometry
        .bounds()
        .expect("slab bounds")
        .map(|point| floor.pose.transform_point(point));
    let mut min = DVec2::splat(f64::INFINITY);
    let mut max = DVec2::splat(f64::NEG_INFINITY);
    for batch in &ground.batches {
        if batch.side != Side::Interior {
            continue;
        }
        for corners in batch.mesh.corners() {
            let triangle = corners.map(|index| batch.mesh.positions[index]);
            let normal = (triangle[1] - triangle[0])
                .cross(triangle[2] - triangle[0])
                .normalize();
            if (normal.y - 1.0).abs() >= 1e-6
                || triangle.iter().any(|point| (point.y - 0.02).abs() > 1e-6)
            {
                continue;
            }
            for corner in triangle {
                min = min.min(DVec2::new(corner.x, corner.z));
                max = max.max(DVec2::new(corner.x, corner.z));
            }
        }
    }
    let start = DVec2::new(footprint[0].x, footprint[0].z);
    let end = DVec2::new(footprint[1].x, footprint[1].z);
    assert!(
        (min.x - start.x).abs() < 0.35 && (min.y - start.y).abs() < 0.35,
        "the interior floor starts at {min}, not the house's {start}"
    );
    assert!(
        (max.x - end.x).abs() < 0.35 && (max.y - end.y).abs() < 0.35,
        "the interior floor ends at {max}, not the house's {end}"
    );
}
