//! Hulls, balls and kernel-free bounds, meshed through the manifold backend.

use ashlar::{
    Axis, Building, Collision, Damage, Element, Geometry, GeometryMesher, Instance, MergeGroup,
    MirrorPlane, Part, Pose, TriangleMesh, UvMode,
};
use ashlar_manifold::{ManifoldMesher, mesh_building};
use glam::{DQuat, DVec3};

fn mesh_bounds(mesh: &TriangleMesh) -> [DVec3; 2] {
    let mut min = DVec3::splat(f64::INFINITY);
    let mut max = DVec3::splat(f64::NEG_INFINITY);
    for point in &mesh.positions {
        min = min.min(*point);
        max = max.max(*point);
    }
    [min, max]
}

fn volume(mesh: &TriangleMesh) -> f64 {
    mesh.triangles()
        .map(|triangle| triangle[0].dot(triangle[1].cross(triangle[2])) / 6.0)
        .sum()
}

/// The part every wall test places: a wall with one opening, and a pane that
/// stays its own mesh.
fn bay() -> Part {
    Part::builder("bay")
        .element(
            Element::new(
                "shell",
                Geometry::cuboid([4.0, 3.0, 0.3])
                    .subtract(Geometry::cuboid([1.0, 2.0, 0.5]).placed(Pose::at([1.5, 0.0, -0.1]))),
                "surface",
            )
            .cut_material("reveal")
            .uv(UvMode::Box)
            .collision(Collision::Bounds),
        )
        .element(
            Element::new(
                "glass",
                Geometry::cuboid([1.0, 2.0, 0.02]).placed(Pose::at([1.5, 0.0, 0.14])),
                "glass",
            )
            .standalone(),
        )
        .build()
        .expect("valid bay")
}

/// Three bays in a row: `a` and `b` in `"low"`, `c` in `"high"`.
fn grouped_wall() -> Building {
    Building::builder("wall")
        .part(bay())
        .material("surface", "paint")
        .material("reveal", "steel")
        .material("glass", "glass")
        .merged()
        .group(MergeGroup::new("low"))
        .group(MergeGroup::new("high"))
        .instance(Instance::new("a", "bay").group("low"))
        .instance(
            Instance::new("b", "bay")
                .placed(Pose::at([4.0, 0.0, 0.0]))
                .group("low"),
        )
        .instance(
            Instance::new("c", "bay")
                .placed(Pose::at([8.0, 0.0, 0.0]))
                .group("high"),
        )
        .build()
        .expect("valid wall")
}

/// At least eight varied expressions, so the bounds check is not one shape's
/// accident.
fn varied() -> Vec<Geometry> {
    let triangle = [[0.0, 0.0], [2.0, 0.0], [0.0, 3.0]];
    vec![
        Geometry::cuboid([2.0, 1.0, 1.0])
            .placed(Pose::default().rotated(DQuat::from_rotation_y(0.7))),
        Geometry::chamfered_cuboid([2.0, 1.0, 1.0], 0.1)
            .placed(Pose::at([1.0, 2.0, 3.0]).rotated(DQuat::from_rotation_x(0.4))),
        Geometry::cylinder(0.4, 2.0, 16).arrayed(3, Pose::at([1.5, 0.0, 0.0])),
        Geometry::extrude(triangle, 1.0).mirrored(MirrorPlane::new(Axis::Z, 0.5)),
        Geometry::cuboid([4.0, 3.0, 0.3])
            .subtract(Geometry::cuboid([1.0, 1.0, 0.5]).placed(Pose::at([1.0, 1.0, -0.1]))),
        Geometry::cuboid([1.0, 1.0, 1.0])
            .union(Geometry::cuboid([1.0, 1.0, 1.0]).placed(Pose::at([2.0, 1.0, 0.0])))
            .union(Geometry::cuboid([1.0, 1.0, 1.0]).placed(Pose::at([4.0, 0.0, 0.0]))),
        Geometry::hull([
            [0.0, 0.0, 0.0],
            [1.0, 0.0, 0.0],
            [0.0, 1.0, 0.0],
            [0.0, 0.0, 1.0],
        ]),
        Geometry::ball(1.0, 6),
        Geometry::cylinder(0.5, 1.0, 8)
            .subtract(Geometry::cylinder(0.2, 2.0, 8).placed(Pose::at([0.0, -0.5, 0.0]))),
    ]
}

#[test]
fn a_ball_meshes_to_a_closed_solid_of_about_the_right_volume() {
    let ball = Geometry::ball(1.5, 8);
    let mesh = ManifoldMesher::default().mesh(&ball).expect("meshes");
    let measured = volume(&mesh);
    let expected = 4.0 / 3.0 * std::f64::consts::PI * 1.5_f64.powi(3);
    assert!(
        measured > expected * 0.9 && measured < expected,
        "volume {measured} of {expected}"
    );

    let bounds = ball.bounds().expect("bounds");
    let mesh = mesh_bounds(&mesh);
    assert!(
        (mesh[0].y - bounds[0].y).abs() < 1e-9 && (mesh[1].y - bounds[1].y).abs() < 1e-9,
        "the poles touch the analytic box: mesh {mesh:?}, bounds {bounds:?}"
    );
    let grown = [
        bounds[0] - DVec3::splat(1e-9),
        bounds[1] + DVec3::splat(1e-9),
    ];
    assert!(
        mesh[0].cmpge(grown[0]).all() && mesh[1].cmple(grown[1]).all(),
        "mesh {mesh:?} outside bounds {bounds:?} on X or Z"
    );
}

#[test]
fn bounds_contain_the_mesh_for_every_shape() {
    let mesher = ManifoldMesher::default();
    for geometry in varied() {
        let mesh = mesher.mesh(&geometry).expect("meshes");
        let bounds = geometry.bounds().expect("bounds");
        let mesh = mesh_bounds(&mesh);
        let grown = [
            bounds[0] - DVec3::splat(1e-9),
            bounds[1] + DVec3::splat(1e-9),
        ];
        assert!(
            mesh[0].cmpge(grown[0]).all() && mesh[1].cmple(grown[1]).all(),
            "mesh {mesh:?} outside bounds {bounds:?} for {geometry:?}"
        );
    }
}

#[test]
fn groups_touched_names_only_the_overlapping_group() {
    let building = grouped_wall();
    let meshed = mesh_building(&building, &ManifoldMesher::default()).expect("meshes");

    let in_c = Damage::blast([10.0, 1.5, 0.15], 1.0, "rubble");
    assert_eq!(meshed.groups_touched(&in_c), ["high"]);

    let joint = Damage::blast([8.0, 1.5, 0.15], 1.0, "rubble");
    assert_eq!(meshed.groups_touched(&joint), ["low", "high"]);

    let far = Damage::blast([100.0, 0.0, 0.0], 1.0, "rubble");
    assert!(meshed.groups_touched(&far).is_empty());
}

#[test]
fn standalone_touched_names_the_glass_in_the_blast() {
    let building = grouped_wall();
    let meshed = mesh_building(&building, &ManifoldMesher::default()).expect("meshes");
    let blast = Damage::blast([2.0, 1.0, 0.15], 0.6, "rubble");
    assert_eq!(meshed.standalone_touched(&blast), [("a", "glass")]);
}
