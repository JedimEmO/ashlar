//! Damage data: hulls and balls, bounds, validation and serialization.

use std::f64::consts::FRAC_PI_4;

use ashlar::glam::{DQuat, DVec3};
use ashlar::{
    Axis, Building, Damage, DamageLog, Element, Geometry, Instance, MirrorPlane, Part, Pose, Shape,
};

/// A wall of one bay, merged when asked, binding the slots a damage record may
/// name.
fn wall(merged: bool) -> Building {
    let part = Part::builder("bay")
        .element(Element::new(
            "shell",
            Geometry::cuboid([4.0, 3.0, 0.3]),
            "surface",
        ))
        .build()
        .expect("valid part");
    let mut builder = Building::builder("wall")
        .part(part)
        .material("surface", "paint")
        .instance(Instance::new("a", "bay"));
    if merged {
        builder = builder.merged();
    }
    builder.build().expect("valid building")
}

fn assert_bounds(bounds: Option<[DVec3; 2]>, min: [f64; 3], max: [f64; 3]) {
    let [low, high] = bounds.expect("bounds");
    let low = low.to_array();
    let high = high.to_array();
    for axis in 0..3 {
        assert!(
            (low[axis] - min[axis]).abs() < 1e-12,
            "low {low:?}, expected {min:?}"
        );
        assert!(
            (high[axis] - max[axis]).abs() < 1e-12,
            "high {high:?}, expected {max:?}"
        );
    }
}

#[test]
fn a_ball_is_the_same_points_every_time() {
    let first = Geometry::ball(1.5, 8);
    let second = Geometry::ball(1.5, 8);
    assert_eq!(first, second, "the same arguments build the same solid");
    let Shape::Hull { points } = &first.shape else {
        panic!("a ball is a hull");
    };
    assert_eq!(points.len(), 114, "two poles plus seven rings of sixteen");
    assert!(
        DVec3::from_array(points[0]).distance(DVec3::new(0.0, -1.5, 0.0)) < 1e-12,
        "the south pole comes first"
    );
    assert!(
        DVec3::from_array(points[113]).distance(DVec3::new(0.0, 1.5, 0.0)) < 1e-12,
        "the north pole comes last"
    );
    for point in points {
        let distance = DVec3::from_array(*point).length();
        assert!((distance - 1.5).abs() < 1e-12, "point {point:?}");
    }
}

#[test]
fn a_bad_ball_fails_check() {
    assert!(Geometry::ball(1.5, 1).check().is_err());
    assert!(Geometry::ball(0.0, 8).check().is_err());
    assert!(Geometry::ball(f64::NAN, 8).check().is_err());
}

#[test]
fn a_flat_hull_is_refused() {
    let flat = [
        [0.0, 0.0, 0.0],
        [1.0, 0.0, 0.0],
        [0.0, 1.0, 0.0],
        [1.0, 1.0, 0.0],
    ];
    assert!(Geometry::hull(flat).check().is_err());
}

#[test]
fn a_three_point_hull_is_refused() {
    let triangle = [[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]];
    assert!(Geometry::hull(triangle).check().is_err());
}

#[test]
fn a_tetrahedron_is_accepted() {
    let tetra = [
        [0.0, 0.0, 0.0],
        [1.0, 0.0, 0.0],
        [0.0, 1.0, 0.0],
        [0.0, 0.0, 1.0],
    ];
    assert!(Geometry::hull(tetra).check().is_ok());
}

#[test]
fn bounds_of_primitives_are_tight() {
    let cuboid = Geometry::cuboid([2.0, 3.0, 4.0]).placed(Pose::at([1.0, 2.0, 3.0]));
    assert_bounds(cuboid.bounds(), [1.0, 2.0, 3.0], [3.0, 5.0, 7.0]);

    let cylinder = Geometry::cylinder(0.5, 2.0, 12).placed(Pose::at([1.0, 2.0, 3.0]));
    assert_bounds(cylinder.bounds(), [0.5, 2.0, 2.5], [1.5, 4.0, 3.5]);

    let gable = [[0.0, 0.0], [2.0, 0.0], [1.0, 3.0]];
    let extrusion = Geometry::extrude(gable, 1.0);
    assert_bounds(extrusion.bounds(), [0.0, 0.0, 0.0], [2.0, 1.0, 3.0]);

    let ball = Geometry::ball(1.5, 8);
    assert_bounds(ball.bounds(), [-1.5, -1.5, -1.5], [1.5, 1.5, 1.5]);
}

#[test]
fn bounds_of_a_rotated_box_are_conservative() {
    let pose = Pose::default().rotated(DQuat::from_rotation_y(FRAC_PI_4));
    let box_geometry = Geometry::cuboid([2.0, 1.0, 1.0]).placed(pose);
    let [low, high] = box_geometry.bounds().expect("bounds");
    for x in [0.0, 2.0] {
        for y in [0.0, 1.0] {
            for z in [0.0, 1.0] {
                let corner = pose.transform_point(DVec3::new(x, y, z));
                assert!(
                    corner.cmpge(low).all() && corner.cmple(high).all(),
                    "corner {corner:?} outside {low:?}..{high:?}"
                );
            }
        }
    }
    let extent = high.x - low.x;
    assert!(
        (extent - 3.0 / 2.0_f64.sqrt()).abs() < 1e-9,
        "x extent {extent}"
    );
}

#[test]
fn bounds_follow_arrays_mirrors_unions_and_ignore_cutters() {
    let array = Geometry::cuboid([1.0, 1.0, 1.0]).arrayed(3, Pose::at([2.0, 0.0, 0.0]));
    assert_bounds(array.bounds(), [0.0, 0.0, 0.0], [5.0, 1.0, 1.0]);

    let mirror = Geometry::cuboid([1.0, 1.0, 1.0])
        .placed(Pose::at([4.0, 0.0, 0.0]))
        .mirrored(MirrorPlane::new(Axis::X, 0.0));
    assert_bounds(mirror.bounds(), [-5.0, 0.0, 0.0], [-4.0, 1.0, 1.0]);

    let union = Geometry::cuboid([1.0, 1.0, 1.0])
        .union(Geometry::cuboid([1.0, 1.0, 1.0]).placed(Pose::at([3.0, 0.0, 0.0])));
    assert_bounds(union.bounds(), [0.0, 0.0, 0.0], [4.0, 1.0, 1.0]);

    let difference = Geometry::cuboid([4.0, 4.0, 4.0])
        .subtract(Geometry::cuboid([10.0, 10.0, 10.0]).placed(Pose::at([-5.0, -5.0, -5.0])));
    assert_bounds(difference.bounds(), [0.0, 0.0, 0.0], [4.0, 4.0, 4.0]);
}

#[test]
fn damage_round_trips_through_ron() {
    let damage =
        Damage::new(Geometry::cuboid([1.0, 1.0, 1.0]), "rubble").placed(Pose::at([1.0, 2.0, 3.0]));
    let encoded = ron::to_string(&damage).expect("serialize");
    let decoded: Damage = ron::from_str(&encoded).expect("deserialize");
    assert_eq!(damage, decoded);

    let log = DamageLog(vec![damage]);
    let encoded = ron::to_string(&log).expect("serialize");
    assert!(
        encoded.trim_start().starts_with('['),
        "the log is a bare list: {encoded}"
    );
    let decoded: DamageLog = ron::from_str(&encoded).expect("deserialize");
    assert_eq!(log, decoded);
}

#[test]
fn damage_is_checked_against_the_building() {
    let unmerged = DamageLog(vec![Damage::new(
        Geometry::cuboid([1.0, 1.0, 1.0]),
        "surface",
    )]);
    let error = unmerged
        .check(&wall(false))
        .expect_err("unmerged is refused");
    assert_eq!(error.path, "damage[0]");
    assert_eq!(error.reason, "damage needs a merged building");

    let unknown = DamageLog(vec![Damage::new(
        Geometry::cuboid([1.0, 1.0, 1.0]),
        "missing",
    )]);
    let error = unknown.check(&wall(true)).expect_err("unknown slot");
    assert_eq!(error.path, "damage[0].slot");

    let invalid = DamageLog(vec![Damage::new(Geometry::ball(0.0, 8), "surface")]);
    let error = invalid.check(&wall(true)).expect_err("invalid solid");
    assert_eq!(error.path, "damage[0].solid");

    let good = DamageLog(vec![Damage::new(
        Geometry::cuboid([1.0, 1.0, 1.0]),
        "surface",
    )]);
    assert!(good.check(&wall(true)).is_ok());
}

#[test]
fn a_blast_is_a_ball_at_its_centre() {
    let blast = Damage::blast([5.0, 1.0, 2.0], 1.5, "rubble");
    assert_bounds(blast.bounds(), [3.5, -0.5, 0.5], [6.5, 2.5, 3.5]);
}

#[test]
fn collapse_is_not_serialised_when_false() {
    let plain = Damage::new(Geometry::cuboid([1.0, 1.0, 1.0]), "rubble");
    let encoded = ron::to_string(&plain).expect("serialize");
    assert!(
        !encoded.contains("collapse"),
        "a false collapse must not be written: {encoded}"
    );
    let decoded: Damage = ron::from_str(&encoded).expect("deserialize");
    assert_eq!(decoded, plain);

    let collapsing = plain.collapsing();
    let encoded = ron::to_string(&collapsing).expect("serialize");
    assert!(encoded.contains("collapse"), "{encoded}");
    let decoded: Damage = ron::from_str(&encoded).expect("deserialize");
    assert_eq!(decoded, collapsing);
}
