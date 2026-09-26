//! Declared room volumes: containment, corners, storey lookup and validation.

use ashlar::glam::{DQuat, DVec3};
use ashlar::*;
use std::f64::consts::FRAC_PI_2;

fn shell() -> Part {
    Part::builder("shell")
        .element(Element::new(
            "wall",
            Geometry::cuboid([4.0, 3.0, 0.3]),
            "surface",
        ))
        .build()
        .expect("valid shell")
}

fn building() -> BuildingBuilder {
    Building::builder("house")
        .part(shell())
        .material("surface", "paint")
        .instance(Instance::new("a", "shell"))
}

#[test]
fn a_room_contains_points_in_its_own_frame() {
    // A quarter turn about +Y maps the room's local x to building -z and its
    // local z to building +x. With the minimum corner at (10, 0, 0) the room
    // therefore occupies building x in [10, 15], y in [0, 3] and z in [-4, 0],
    // and a point is inside when the offset from (10, 0, 0), read back in the
    // room's frame, lies in [0, 4] x [0, 3] x [0, 5].
    let room = Room::new("hall", [4.0, 3.0, 5.0])
        .placed(Pose::at([10.0, 0.0, 0.0]).rotated(DQuat::from_rotation_y(FRAC_PI_2)));

    // (10.5, 1, -0.5) reads back as (0.5, 1, 0.5), inside all three extents.
    assert!(room.contains(DVec3::new(10.5, 1.0, -0.5)));

    // (10.5, 1, 4) would sit inside the room if the rotation were ignored,
    // since the unrotated box spans x in [10, 14] and z in [0, 5]. Turned back
    // it is (-4, 1, 0.5), and x = -4 is outside [0, 4].
    assert!(!room.contains(DVec3::new(10.5, 1.0, 4.0)));
}

#[test]
fn the_boundary_is_inside() {
    let room = Room::new("cell", [4.0, 3.0, 5.0]);
    for corner in room.corners() {
        assert!(room.contains(corner), "corner {corner} is on the boundary");
    }
    assert!(room.contains(DVec3::new(4.0, 3.0, 5.0)));
    assert!(!room.contains(DVec3::new(4.0 + 1e-6, 0.0, 0.0)));
    assert!(!room.contains(DVec3::new(0.0, 0.0, -1e-6)));
}

#[test]
fn room_at_takes_the_first_declared() {
    let building = building()
        .room(Room::new("first", [4.0, 3.0, 4.0]))
        .room(Room::new("second", [4.0, 3.0, 4.0]))
        .build()
        .expect("valid");
    let room = building
        .room_at(DVec3::new(1.0, 1.0, 1.0))
        .expect("inside both");
    assert_eq!(room.id, "first");
    assert!(building.room_at(DVec3::new(10.0, 0.0, 0.0)).is_none());
}

#[test]
fn a_rooms_storey_comes_from_its_group() {
    let building = building()
        .group(MergeGroup::new("upper").storey(2))
        .room(Room::new("loft", [4.0, 3.0, 4.0]).group("upper"))
        .room(Room::new("atrium", [4.0, 3.0, 4.0]))
        .build()
        .expect("valid");
    assert_eq!(building.room_storey("loft"), Some(2));
    assert_eq!(building.room_storey("atrium"), None);
    assert_eq!(building.room_storey("missing"), None);
}

#[test]
fn an_unknown_group_is_refused_by_path() {
    let error = building()
        .room(Room::new("loft", [4.0, 3.0, 4.0]).group("nowhere"))
        .build()
        .expect_err("unknown group");
    assert_eq!(error.path, "rooms[loft].group");
    assert_eq!(error.reason, "unknown group \"nowhere\"");
}

#[test]
fn a_duplicate_room_is_refused_by_path() {
    let error = building()
        .room(Room::new("hall", [4.0, 3.0, 4.0]))
        .room(Room::new("hall", [2.0, 2.0, 2.0]))
        .build()
        .expect_err("duplicate room");
    assert_eq!(error.path, "rooms[hall]");
}

#[test]
fn a_flat_room_is_refused_by_path() {
    let error = building()
        .room(Room::new("flat", [4.0, 0.0, 4.0]))
        .build()
        .expect_err("zero height");
    assert_eq!(error.path, "rooms[flat].size");
    assert_eq!(error.reason, "room dimensions must be finite and positive");
}

#[test]
fn corners_are_in_building_space() {
    let room = Room::new("hall", [4.0, 3.0, 5.0])
        .placed(Pose::at([10.0, 0.0, 0.0]).rotated(DQuat::from_rotation_y(FRAC_PI_2)));
    let corners = room.corners();
    for corner in corners {
        assert!(room.contains(corner), "corner {corner} is outside");
    }
    let mut min = corners[0];
    let mut max = corners[0];
    for corner in corners {
        min = min.min(corner);
        max = max.max(corner);
    }
    // A quarter turn about Y swaps the local x and z extents.
    let extent = max - min;
    assert!(
        extent.abs_diff_eq(DVec3::new(5.0, 3.0, 4.0), 1e-9),
        "{extent}"
    );
}

#[test]
fn an_old_recipe_has_no_rooms_field() {
    let plain = building().build().expect("valid");
    let text = ron::to_string(plain.recipe()).expect("serialize");
    assert!(!text.contains("rooms"), "old recipe wrote rooms: {text}");

    let with_room = building()
        .room(Room::new("hall", [4.0, 3.0, 5.0]).placed(Pose::at([1.0, 0.0, 0.0])))
        .build()
        .expect("valid");
    let text = ron::to_string(with_room.recipe()).expect("serialize");
    let decoded: BuildingRecipe = ron::from_str(&text).expect("deserialize");
    assert_eq!(decoded, *with_room.recipe());
}
