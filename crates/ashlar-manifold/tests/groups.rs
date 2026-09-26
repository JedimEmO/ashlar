//! Group meshing: unions of placed solids, damage cuts and per-operand provenance.

use ashlar::{
    FaceOrigin, FaceSource, Geometry, GeometryMesher, GroupMesh, MeshError, PlacedCut,
    PlacedGeometry, Pose, TriangleMesh,
};
use ashlar_manifold::ManifoldMesher;
use glam::DVec3;

fn mesh_group(
    solids: &[(&Geometry, Pose)],
    cuts: &[(&Geometry, Pose)],
) -> Result<GroupMesh, MeshError> {
    let solids: Vec<PlacedGeometry<'_>> = solids
        .iter()
        .map(|(geometry, pose)| PlacedGeometry {
            geometry,
            pose: *pose,
        })
        .collect();
    let cuts: Vec<PlacedCut<'_>> = cuts
        .iter()
        .map(|(geometry, pose)| PlacedCut {
            geometry,
            pose: *pose,
            collapse: false,
        })
        .collect();
    ManifoldMesher::default().mesh_group(&solids, &cuts)
}

fn volume(mesh: &TriangleMesh) -> f64 {
    mesh.triangles()
        .map(|triangle| triangle[0].dot(triangle[1].cross(triangle[2])) / 6.0)
        .sum()
}

fn triangle_area([a, b, c]: [DVec3; 3]) -> f64 {
    (b - a).cross(c - a).length() * 0.5
}

fn area(mesh: &TriangleMesh) -> f64 {
    mesh.triangles().map(triangle_area).sum()
}

fn centroid(triangle: [DVec3; 3]) -> DVec3 {
    (triangle[0] + triangle[1] + triangle[2]) / 3.0
}

#[test]
fn cubes_face_to_face_union_without_an_internal_wall() {
    let cube = Geometry::cuboid([1.0; 3]);
    let group = mesh_group(
        &[(&cube, Pose::default()), (&cube, Pose::at([1.0, 0.0, 0.0]))],
        &[],
    )
    .expect("a union of two cubes");
    assert!(
        (volume(&group.mesh) - 2.0).abs() < 1e-9,
        "{}",
        volume(&group.mesh)
    );
    assert!(
        (area(&group.mesh) - 10.0).abs() < 1e-9,
        "{}",
        area(&group.mesh)
    );
    for triangle in group.mesh.triangles() {
        assert!(
            !triangle.iter().all(|point| (point.x - 1.0).abs() < 1e-9),
            "a triangle lies in the shared plane"
        );
    }
}

#[test]
fn every_face_names_the_operand_it_came_from() {
    let cube = Geometry::cuboid([1.0; 3]);
    let group = mesh_group(
        &[(&cube, Pose::default()), (&cube, Pose::at([1.0, 0.0, 0.0]))],
        &[],
    )
    .expect("a union of two cubes");
    let mut seen = [false; 2];
    for (face, triangle) in group.mesh.triangles().enumerate() {
        let x = centroid(triangle).x;
        let operand = group.mesh.sources[face].operand;
        if x < 1.0 {
            assert_eq!(operand, 0, "a left face names the first operand");
            seen[0] = true;
        } else if x > 1.0 {
            assert_eq!(operand, 1, "a right face names the second operand");
            seen[1] = true;
        }
    }
    assert_eq!(seen, [true, true], "both operands occur");
}

#[test]
fn a_shared_parts_cutters_are_reported_per_operand() {
    let part = Geometry::cuboid([4.0, 3.0, 0.3])
        .subtract(Geometry::cuboid([1.0, 2.0, 0.5]).placed(Pose::at([1.5, 0.0, -0.1])));
    let group = mesh_group(
        &[(&part, Pose::default()), (&part, Pose::at([4.0, 0.0, 0.0]))],
        &[],
    )
    .expect("a union of two shared parts");
    for operand in 0..2u32 {
        let mut total = 0.0;
        for (face, triangle) in group.mesh.triangles().enumerate() {
            let source = group.mesh.sources[face];
            if source.operand != operand {
                continue;
            }
            match source.origin {
                FaceOrigin::Body => {}
                FaceOrigin::Cutter(0) => total += triangle_area(triangle),
                other => panic!("unexpected origin {other:?}"),
            }
        }
        assert!(
            (total - 1.5).abs() < 1e-9,
            "operand {operand} reveal area {total}"
        );
    }
}

#[test]
fn a_gap_is_two_components_and_not_an_error() {
    let cube = Geometry::cuboid([1.0; 3]);
    let group = mesh_group(
        &[
            (&cube, Pose::default()),
            (&cube, Pose::at([1.0 + 1e-6, 0.0, 0.0])),
        ],
        &[],
    )
    .expect("a gap is not an error");
    assert!(
        (volume(&group.mesh) - 2.0).abs() < 1e-6,
        "{}",
        volume(&group.mesh)
    );
}

#[test]
fn a_cut_exposes_damage_faces_that_belong_to_no_operand() {
    let cube = Geometry::cuboid([1.0; 3]);
    let cut = Geometry::cuboid([0.5, 0.5, 0.5]);
    let group = mesh_group(
        &[(&cube, Pose::default()), (&cube, Pose::at([1.0, 0.0, 0.0]))],
        &[(&cut, Pose::at([0.75, 0.25, -0.25]))],
    )
    .expect("a cut union");
    assert!(
        (volume(&group.mesh) - (2.0 - 0.0625)).abs() < 1e-9,
        "{}",
        volume(&group.mesh)
    );
    let mut damage = 0;
    for (face, _) in group.mesh.triangles().enumerate() {
        let source = group.mesh.sources[face];
        if source.origin == FaceOrigin::Damage(0) {
            damage += 1;
            assert_eq!(source.operand, FaceSource::NO_OPERAND);
        } else {
            assert!(source.operand == 0 || source.operand == 1, "{source:?}");
        }
    }
    assert!(damage > 0, "the cut exposes faces");
}

#[test]
fn cuts_apply_in_order_and_are_numbered_by_position() {
    let cube = Geometry::cuboid([1.0; 3]);
    let first = Geometry::cuboid([0.2, 0.2, 0.4]);
    let second = Geometry::cuboid([0.2, 0.2, 0.4]);
    let group = mesh_group(
        &[(&cube, Pose::default()), (&cube, Pose::at([1.0, 0.0, 0.0]))],
        &[
            (&first, Pose::at([0.2, 0.2, -0.1])),
            (&second, Pose::at([1.5, 0.5, -0.1])),
        ],
    )
    .expect("two disjoint cuts");
    let bounds = [
        (DVec3::new(0.2, 0.2, -0.1), DVec3::new(0.4, 0.4, 0.3)),
        (DVec3::new(1.5, 0.5, -0.1), DVec3::new(1.7, 0.7, 0.3)),
    ];
    let mut seen = [false; 2];
    for (face, triangle) in group.mesh.triangles().enumerate() {
        let FaceOrigin::Damage(index) = group.mesh.sources[face].origin else {
            continue;
        };
        let index = index as usize;
        seen[index] = true;
        let (min, max) = bounds[index];
        for point in triangle {
            assert!(
                point.cmpge(min - DVec3::splat(1e-6)).all()
                    && point.cmple(max + DVec3::splat(1e-6)).all(),
                "damage {index} face at {point} escapes {min}..{max}"
            );
        }
    }
    assert_eq!(seen, [true, true]);
}

#[test]
fn normals_are_unit_and_faceted_on_a_box() {
    let cube = Geometry::cuboid([1.0; 3]);
    let group = mesh_group(
        &[(&cube, Pose::default()), (&cube, Pose::at([1.0, 0.0, 0.0]))],
        &[],
    )
    .expect("a union of two cubes");
    for normal in &group.mesh.normals {
        assert!((normal.length() - 1.0).abs() < 1e-9, "{normal}");
        assert!(
            [
                DVec3::X,
                DVec3::NEG_X,
                DVec3::Y,
                DVec3::NEG_Y,
                DVec3::Z,
                DVec3::NEG_Z,
            ]
            .iter()
            .any(|axis| normal.abs_diff_eq(*axis, 1e-9)),
            "{normal} is not an axis"
        );
    }
}

#[test]
fn a_cylinder_in_a_group_is_smooth_around_and_creased_at_the_caps() {
    let cylinder = Geometry::cylinder(1.0, 2.0, 32);
    let group = mesh_group(&[(&cylinder, Pose::default())], &[]).expect("one cylinder");
    for (position, normal) in group.mesh.positions.iter().zip(&group.mesh.normals) {
        if (normal - DVec3::Y).length() < 1e-6 || (normal + DVec3::Y).length() < 1e-6 {
            continue;
        }
        assert!(normal.y.abs() < 1e-9, "{normal} leans off the side");
        let radial = DVec3::new(position.x, 0.0, position.z).normalize();
        let side = DVec3::new(normal.x, 0.0, normal.z).normalize();
        assert!(side.abs_diff_eq(radial, 1e-6), "{side} != {radial}");
    }
}

#[test]
fn an_empty_group_is_an_empty_mesh() {
    let group = mesh_group(&[], &[]).expect("an empty group");
    assert_eq!(group.mesh.triangle_count(), 0);
}

#[test]
fn an_invalid_member_is_refused_by_path() {
    let good = Geometry::cuboid([1.0; 3]);
    let bad = Geometry::cuboid([1.0, 0.0, 1.0]);
    let error = mesh_group(&[(&good, Pose::default()), (&bad, Pose::default())], &[])
        .expect_err("a zero extent is refused");
    assert!(error.path.starts_with("solids[1]"), "{}", error.path);
}

#[test]
fn the_result_is_consistent_and_welded() {
    let cube = Geometry::cuboid([1.0; 3]);
    let group = mesh_group(
        &[(&cube, Pose::default()), (&cube, Pose::at([1.0, 0.0, 0.0]))],
        &[],
    )
    .expect("a union of two cubes");
    assert!(group.mesh.is_consistent());
    assert!(group.mesh.positions.len() < group.mesh.triangle_count() * 3);
}
