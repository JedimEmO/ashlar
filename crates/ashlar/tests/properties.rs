//! Properties of the pure geometry, over generated input.
//!
//! The example-based tests fix the cases that were once wrong. These fix the
//! invariants that must hold for every input, which is where a validator that
//! panics on an adversarial polygon or a projection that quietly stretches an
//! angled face would show up. Case counts stay modest: these run on every
//! `cargo test`, not on a fuzzing budget.
#![allow(
    clippy::unwrap_used,
    reason = "generated fixtures; a failure is a test failure"
)]
use ashlar::{
    FaceSource, Geometry, Pose, TriangleMesh, WELD_TOLERANCE,
    glam::{DQuat, DVec2, DVec3},
    planar_uvs,
};
use proptest::prelude::*;

/// A strictly convex polygon: distinct angles at least three degrees apart on
/// one circle, so no edge is zero length and no three vertices are collinear.
fn convex_profile() -> impl Strategy<Value = Vec<[f64; 2]>> {
    (3usize..=10, 0.25f64..40.0).prop_flat_map(|(count, radius)| {
        proptest::collection::hash_set(0u32..120, count).prop_map(move |steps| {
            let mut steps: Vec<u32> = steps.into_iter().collect();
            steps.sort_unstable();
            steps
                .into_iter()
                .map(|step| {
                    let angle = f64::from(step) * std::f64::consts::TAU / 120.0;
                    [radius * angle.cos(), radius * angle.sin()]
                })
                .collect()
        })
    })
}

/// Anything at all, including NaN, infinities and repeated points.
fn arbitrary_profile() -> impl Strategy<Value = Vec<[f64; 2]>> {
    let coordinate = prop_oneof![
        4 => -10.0f64..10.0,
        1 => Just(0.0),
        1 => Just(f64::NAN),
        1 => Just(f64::INFINITY),
        1 => Just(f64::MAX),
    ];
    proptest::collection::vec([coordinate.clone(), coordinate], 0..8)
        .prop_map(|points| points.into_iter().map(|p| [p[0], p[1]]).collect())
}

/// Small integer coordinates, so a weld bucket holds bit-identical values and
/// the round trip can be compared exactly rather than within a tolerance.
fn lattice_mesh() -> impl Strategy<Value = TriangleMesh> {
    let corner = (-4i8..4, -4i8..4, -4i8..4, 0usize..6, -2i8..2, -2i8..2);
    proptest::collection::vec(corner, 3..=24).prop_map(|corners| {
        let mut mesh = TriangleMesh::default();
        for face in corners.as_chunks::<3>().0 {
            let point = |c: &(i8, i8, i8, usize, i8, i8)| {
                DVec3::new(f64::from(c.0), f64::from(c.1), f64::from(c.2))
            };
            let normal = |c: &(i8, i8, i8, usize, i8, i8)| {
                [
                    DVec3::X,
                    DVec3::Y,
                    DVec3::Z,
                    DVec3::NEG_X,
                    DVec3::NEG_Y,
                    DVec3::NEG_Z,
                ][c.3]
            };
            let uv = |c: &(i8, i8, i8, usize, i8, i8)| [f64::from(c.4), f64::from(c.5)];
            mesh.push_triangle(
                std::array::from_fn(|i| point(&face[i])),
                std::array::from_fn(|i| normal(&face[i])),
                std::array::from_fn(|i| uv(&face[i])),
                FaceSource::BODY,
            );
        }
        mesh
    })
}

/// Every corner of every triangle, dereferenced through the index buffer.
fn corner_stream(mesh: &TriangleMesh) -> Vec<(DVec3, DVec3, [f64; 2])> {
    mesh.corners()
        .flatten()
        .map(|i| (mesh.positions[i], mesh.normals[i], mesh.uvs[i]))
        .collect()
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 96, ..ProptestConfig::default() })]

    /// Validation is a predicate over authored data, so it has to answer for
    /// anything a file or an editor can hold, including NaN and duplicates.
    #[test]
    fn profile_validation_answers_rather_than_panicking(points in arbitrary_profile()) {
        let _ = Geometry::extrude(points.clone(), 1.0).check();
        let _ = Geometry::extrude(points, f64::NAN).check();
    }

    /// A strictly convex ring is the case the validator must never refuse:
    /// no zero-length edge, no collinear neighbour, no self-intersection.
    #[test]
    fn every_strictly_convex_profile_is_accepted(points in convex_profile()) {
        let extrusion = Geometry::extrude(points.clone(), 2.5);
        prop_assert!(extrusion.check().is_ok(), "refused {points:?}");
        // Winding is authored, not normalized, so the reverse is equally valid.
        let mut reversed = points;
        reversed.reverse();
        prop_assert!(Geometry::extrude(reversed, 2.5).check().is_ok());
    }

    /// The planar frame is orthonormal, so it is a rigid motion of the face
    /// into two dimensions: an angled wall is not foreshortened, and a texture
    /// laid on it keeps its physical scale.
    #[test]
    fn planar_uvs_preserve_every_edge_length(
        points in proptest::collection::vec(
            (-50.0f64..50.0, -50.0f64..50.0, -50.0f64..50.0),
            3..=3,
        ),
    ) {
        let points: [DVec3; 3] =
            std::array::from_fn(|i| DVec3::new(points[i].0, points[i].1, points[i].2));
        let area = (points[1] - points[0]).cross(points[2] - points[0]).length();
        prop_assume!(area > 1e-3);

        let uvs = planar_uvs(points).expect("a non-degenerate triangle projects");
        for [a, b] in [[0, 1], [1, 2], [2, 0]] {
            let edge = points[a].distance(points[b]);
            let mapped = DVec2::from_array(uvs[a]).distance(DVec2::from_array(uvs[b]));
            prop_assert!(
                (edge - mapped).abs() <= 1e-9 * edge.max(1.0),
                "edge {edge} became {mapped}",
            );
        }
    }

    /// Welding is an optimization. Unwelding undoes the sharing, so the two in
    /// sequence must give back exactly the triangles that went in.
    #[test]
    fn welding_then_unwelding_is_the_identity_on_the_triangles(mesh in lattice_mesh()) {
        // A triangle two of whose corners are the same vertex — position,
        // normal and UV — has no area, and welding drops it by design. The
        // identity is claimed for every triangle that is a triangle.
        prop_assume!(mesh.indices.as_chunks::<3>().0.iter().all(|face| {
            let corner = |i: u32| {
                let i = i as usize;
                (mesh.positions[i], mesh.normals[i], mesh.uvs[i])
            };
            corner(face[0]) != corner(face[1])
                && corner(face[1]) != corner(face[2])
                && corner(face[0]) != corner(face[2])
        }));
        let before = corner_stream(&mesh);
        let triangles = mesh.triangle_count();

        let mut welded = mesh.clone();
        welded.weld(WELD_TOLERANCE);
        prop_assert_eq!(welded.triangle_count(), triangles, "welding lost a face");
        prop_assert!(welded.is_consistent());
        prop_assert!(welded.positions.len() <= mesh.positions.len());
        prop_assert_eq!(corner_stream(&welded), before.clone(), "welding moved a corner");

        welded.unweld();
        prop_assert_eq!(welded.triangle_count(), triangles);
        prop_assert!(welded.is_consistent());
        prop_assert_eq!(welded.positions.len(), triangles * 3, "unwelding shares nothing");
        prop_assert_eq!(corner_stream(&welded), before);
    }

    /// A pose is rigid by construction, with no scale to hide in it, so any
    /// distance measured in a part frame survives placement into a building.
    #[test]
    fn transforming_by_a_pose_is_an_isometry(
        rotation in (-1.0f64..1.0, -1.0f64..1.0, -1.0f64..1.0, -1.0f64..1.0),
        translation in (-1e4f64..1e4, -1e4f64..1e4, -1e4f64..1e4),
        a in (-100.0f64..100.0, -100.0f64..100.0, -100.0f64..100.0),
        b in (-100.0f64..100.0, -100.0f64..100.0, -100.0f64..100.0),
    ) {
        let raw = glam::DVec4::new(rotation.0, rotation.1, rotation.2, rotation.3);
        prop_assume!(raw.length() > 0.25);
        let rotation = DQuat::from_vec4(raw.normalize());
        let pose = Pose {
            translation: DVec3::new(translation.0, translation.1, translation.2),
            rotation,
        };
        let (a, b) = (
            DVec3::new(a.0, a.1, a.2),
            DVec3::new(b.0, b.1, b.2),
        );

        let moved = a.distance(b);
        let placed = pose.transform_point(a).distance(pose.transform_point(b));
        // The translation is large next to the points, so the tolerance is
        // relative to the magnitudes the subtraction actually cancelled.
        let scale = pose.translation.length().max(100.0);
        prop_assert!((moved - placed).abs() <= 1e-10 * scale, "{moved} became {placed}");
        // And the inverse is exact enough to round trip through it.
        prop_assert!(pose.inverse().transform_point(pose.transform_point(a)).abs_diff_eq(a, 1e-8));
    }
}
