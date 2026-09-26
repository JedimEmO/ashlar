//! Portals derived from marked cutters: their rectangle, their frame and how a
//! building places them.
use ashlar::{Building, Element, Geometry, GeometryMesher, Instance, MergeGroup, Part, Pose};
use ashlar_manifold::{ManifoldMesher, mesh_building};
use glam::DQuat;

fn doorway() -> Geometry {
    Geometry::cuboid([4.0, 3.0, 0.3]).subtract(
        Geometry::cuboid([1.0, 2.2, 0.5])
            .placed(Pose::at([1.5, -0.2, -0.1]))
            .portal("door"),
    )
}

fn wall_part() -> Part {
    Part::builder("wall")
        .element(Element::new("panel", doorway(), "surface"))
        .build()
        .expect("part")
}

fn span(values: [f64; 4]) -> (f64, f64) {
    (
        values.iter().copied().fold(f64::INFINITY, f64::min),
        values.iter().copied().fold(f64::NEG_INFINITY, f64::max),
    )
}

#[test]
fn a_doorway_is_a_rectangle_at_the_walls_mid_plane() {
    let portals = ManifoldMesher::default()
        .portals(&doorway())
        .expect("portal");
    assert_eq!(portals.len(), 1);
    let portal = &portals[0];
    assert_eq!(portal.id, "door");
    assert!(
        (portal.normal.z.abs() - 1.0).abs() < 1e-9,
        "{:?}",
        portal.normal
    );
    // The rectangle lies at the middle of the 0.3 wall, not on either face.
    for corner in portal.corners {
        assert!((corner.z - 0.15).abs() < 1e-9, "{corner}");
    }
    let (x0, x1) = span(portal.corners.map(|corner| corner.x));
    let (y0, y1) = span(portal.corners.map(|corner| corner.y));
    assert!(
        (x0 - 1.5).abs() < 1e-9 && (x1 - 2.5).abs() < 1e-9,
        "{x0}..{x1}"
    );
    // Clipped to the wall: the cutter runs 0.2 below the floor, the portal does not.
    assert!(
        (y0 - 0.0).abs() < 1e-9 && (y1 - 2.0).abs() < 1e-9,
        "{y0}..{y1}"
    );
}

#[test]
fn a_rotated_cutter_gives_a_tight_rectangle() {
    let geometry = Geometry::cuboid([4.0, 3.0, 0.3]).subtract(
        Geometry::cuboid([1.0, 1.0, 1.0])
            .placed(
                Pose::at([1.5, 1.0, -0.35])
                    .rotated(DQuat::from_rotation_z(std::f64::consts::FRAC_PI_6)),
            )
            .portal("window"),
    );
    let portals = ManifoldMesher::default()
        .portals(&geometry)
        .expect("portal");
    assert_eq!(portals.len(), 1);
    let corners = portals[0].corners;
    // Tight in the cutter's frame: a rotated unit square would measure larger
    // against the loose axis-aligned box of its own rotation.
    let first = corners[0].distance(corners[1]);
    let second = corners[1].distance(corners[2]);
    assert!((first - 1.0).abs() < 1e-9, "edge {first}");
    assert!((second - 1.0).abs() < 1e-9, "edge {second}");
}

#[test]
fn a_recess_is_not_a_portal() {
    let geometry = Geometry::cuboid([4.0, 3.0, 0.3]).subtract(
        Geometry::cuboid([1.0, 1.0, 0.1])
            .placed(Pose::at([1.0, 1.0, -0.05]))
            .portal("niche"),
    );
    let error = ManifoldMesher::default()
        .portals(&geometry)
        .expect_err("a recess pierces nothing");
    assert!(
        error.reason.contains("does not pass through"),
        "{}",
        error.reason
    );
}

#[test]
fn portals_are_placed_per_instance_in_building_space() {
    let pose =
        Pose::at([10.0, 0.0, 0.0]).rotated(DQuat::from_rotation_y(std::f64::consts::FRAC_PI_2));
    let building = Building::builder("doorway")
        .part(wall_part())
        .material("surface", "brick")
        .instance(Instance::new("first", "wall"))
        .instance(Instance::new("second", "wall").placed(pose))
        .build()
        .expect("building");
    let meshed = mesh_building(&building, &ManifoldMesher::default()).expect("meshed");
    let portals = meshed.portals();
    assert_eq!(portals.len(), 2);
    assert_eq!(portals[0].instance, "first");
    assert_eq!(portals[0].element, "panel");
    assert_eq!(portals[1].instance, "second");
    for (first, second) in portals[0].corners.iter().zip(&portals[1].corners) {
        assert!((pose.transform_point(*first) - *second).length() < 1e-9);
    }
    assert!(
        (portals[1].normal.x.abs() - 1.0).abs() < 1e-9,
        "{:?}",
        portals[1].normal
    );
}

#[test]
fn a_portal_inside_a_placed_union_is_lifted_to_the_root_frame() {
    let placed = doorway().placed(Pose::at([0.0, 5.0, 0.0]));
    let other = Geometry::cuboid([1.0, 1.0, 1.0]).placed(Pose::at([20.0, 0.0, 0.0]));
    let geometry = Geometry::union_all([placed, other]);
    let portals = ManifoldMesher::default()
        .portals(&geometry)
        .expect("portal");
    assert_eq!(portals.len(), 1);
    let (y0, y1) = span(portals[0].corners.map(|corner| corner.y));
    assert!((y0 - 5.0).abs() < 1e-9, "{y0}");
    assert!((y1 - 7.0).abs() < 1e-9, "{y1}");
}

#[test]
fn portals_carry_their_storey() {
    let building = Building::builder("storeyed")
        .part(wall_part())
        .group(MergeGroup::new("upper").storey(2))
        .material("surface", "brick")
        .instance(Instance::new("first", "wall").group("upper"))
        .build()
        .expect("building");
    let meshed = mesh_building(&building, &ManifoldMesher::default()).expect("meshed");
    let portals = meshed.portals();
    assert_eq!(portals.len(), 1);
    assert_eq!(portals[0].storey, Some(2));
}

#[test]
fn merging_does_not_change_portals() {
    let build = |merged: bool| {
        let builder = Building::builder("doorway")
            .part(wall_part())
            .material("surface", "brick")
            .instance(Instance::new("first", "wall"));
        let builder = if merged { builder.merged() } else { builder };
        builder.build().expect("building")
    };
    let plain = mesh_building(&build(false), &ManifoldMesher::default()).expect("plain");
    let merged = mesh_building(&build(true), &ManifoldMesher::default()).expect("merged");
    assert_eq!(plain.portals(), merged.portals());
}

/// An L-shaped element: a wall along X and, from its end, one along Z.
fn l_shape() -> Geometry {
    Geometry::union_all([
        Geometry::cuboid([6.0, 3.0, 0.3]),
        Geometry::cuboid([0.3, 3.0, 5.0]).placed(Pose::at([0.0, 0.0, 0.3])),
    ])
}

#[test]
fn a_doorway_through_one_leg_of_an_l_is_a_portal() {
    let geometry = l_shape().subtract(
        Geometry::cuboid([1.0, 2.2, 0.5])
            .placed(Pose::at([3.0, -0.2, -0.1]))
            .portal("door"),
    );
    let portals = ManifoldMesher::default()
        .portals(&geometry)
        .expect("portal");
    assert_eq!(portals.len(), 1);
    let portal = &portals[0];
    assert_eq!(portal.id, "door");
    assert!(
        (portal.normal.z.abs() - 1.0).abs() < 1e-9,
        "{:?}",
        portal.normal
    );
    // The middle of leg A's 0.3 thickness, not of the L's 5.3 m bounds.
    for corner in portal.corners {
        assert!((corner.z - 0.15).abs() < 1e-9, "{corner}");
    }
    let (x0, x1) = span(portal.corners.map(|corner| corner.x));
    let (y0, y1) = span(portal.corners.map(|corner| corner.y));
    assert!(
        (x0 - 3.0).abs() < 1e-9 && (x1 - 4.0).abs() < 1e-9,
        "{x0}..{x1}"
    );
    assert!(
        (y0 - 0.0).abs() < 1e-9 && (y1 - 2.0).abs() < 1e-9,
        "{y0}..{y1}"
    );
}

#[test]
fn a_doorway_through_the_other_leg_too() {
    let geometry = l_shape().subtract(
        Geometry::cuboid([0.5, 2.2, 1.0])
            .placed(Pose::at([-0.1, -0.2, 2.0]))
            .portal("door"),
    );
    let portals = ManifoldMesher::default()
        .portals(&geometry)
        .expect("portal");
    assert_eq!(portals.len(), 1);
    let portal = &portals[0];
    assert!(
        (portal.normal.x.abs() - 1.0).abs() < 1e-9,
        "{:?}",
        portal.normal
    );
    for corner in portal.corners {
        assert!((corner.x - 0.15).abs() < 1e-9, "{corner}");
    }
    let (z0, z1) = span(portal.corners.map(|corner| corner.z));
    assert!(
        (z0 - 2.0).abs() < 1e-9 && (z1 - 3.0).abs() < 1e-9,
        "{z0}..{z1}"
    );
}

#[test]
fn a_door_facing_another_wall_is_still_a_portal() {
    // A U: leg A, a parallel wall a metre away, and a short leg B joining them.
    let geometry = Geometry::union_all([
        Geometry::cuboid([6.0, 3.0, 0.3]),
        Geometry::cuboid([6.0, 3.0, 0.3]).placed(Pose::at([0.0, 0.0, 1.0])),
        Geometry::cuboid([0.3, 3.0, 0.7]).placed(Pose::at([0.0, 0.0, 0.3])),
    ])
    .subtract(
        Geometry::cuboid([1.0, 2.2, 0.5])
            .placed(Pose::at([3.0, -0.2, -0.1]))
            .portal("door"),
    );
    let portals = ManifoldMesher::default()
        .portals(&geometry)
        .expect("portal");
    assert_eq!(portals.len(), 1);
    // The probe is too short to reach the parallel wall at z = 1.0.
    for corner in portals[0].corners {
        assert!((corner.z - 0.15).abs() < 1e-9, "{corner}");
    }
}

#[test]
fn a_recess_in_an_l_is_still_refused() {
    let geometry = l_shape().subtract(
        Geometry::cuboid([1.0, 1.0, 0.1])
            .placed(Pose::at([3.0, 1.0, 0.0]))
            .portal("niche"),
    );
    let error = ManifoldMesher::default()
        .portals(&geometry)
        .expect_err("a recess pierces nothing");
    assert!(
        error.reason.contains("does not pass through"),
        "{}",
        error.reason
    );
}

#[test]
fn an_opening_at_the_corner_of_the_l_is_refused_or_sane() {
    let geometry = l_shape().subtract(
        Geometry::cuboid([0.5, 1.0, 0.5])
            .placed(Pose::at([-0.1, 1.0, -0.1]))
            .portal("corner"),
    );
    // The cutter only notches the corner where the two legs overlap. Material
    // continues past the core on X (leg A) and on Z (leg B), and leg A carries
    // on above on Y, so no axis is pierced and the cutter is refused.
    let error = ManifoldMesher::default()
        .portals(&geometry)
        .expect_err("a corner notch pierces nothing");
    assert!(
        error.reason.contains("does not pass through"),
        "{}",
        error.reason
    );
}

#[test]
fn a_doorway_through_a_round_wall_is_a_portal_and_a_recess_in_it_is_not() {
    // A curved wall's inner face moves as the jambs do: the probe that asks
    // whether material carries on past the opening, held a hair inside the
    // jambs, meets that face a hair short of the core's own box. That is a
    // wall that stops, and the doorway through it is a portal.
    let drum = || Geometry::revolve([[2.85, 0.0], [3.2, 0.0], [3.2, 2.75], [2.85, 2.75]], 48);
    let through = drum().subtract(
        Geometry::cuboid([1.2, 2.3, 1.2])
            .placed(Pose::at([-0.6, 0.0, -3.6]))
            .portal("door"),
    );
    let portals = ManifoldMesher::default()
        .portals(&through)
        .expect("a doorway through a drum is a portal");
    assert_eq!(portals.len(), 1);
    assert!((portals[0].normal.z.abs() - 1.0).abs() < 1e-9);
    let recess = drum().subtract(
        Geometry::cuboid([1.2, 2.3, 0.6])
            .placed(Pose::at([-0.6, 0.0, -3.6]))
            .portal("door"),
    );
    assert!(ManifoldMesher::default().portals(&recess).is_err());
}
