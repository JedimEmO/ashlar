//! Analytic surfaces exercise fitting independently of world generation or rendering.
#![allow(
    clippy::unwrap_used,
    clippy::float_cmp,
    reason = "exact analytic fixtures; errors fail the test"
)]
use ashlar::*;
use glam::{DQuat, DVec3};

fn patch(f: impl Fn(f64, f64) -> f64) -> TerrainPatch {
    let heights = (0..81)
        .flat_map(|z| (0..81).map(move |x| (x, z)))
        .map(|(x, z)| f(f64::from(x) - 40.0, f64::from(z) - 40.0))
        .collect();
    TerrainPatch::new([-40.0, -40.0], 1.0, [81, 81], heights).unwrap()
}
fn spec() -> GroundingSpec {
    GroundingSpec {
        footprint: GroundRect::new([-4.0, 0.0], [4.0, 6.0]),
        floor: 0.0,
        slab_bottom: -0.3,
        entrance: Socket::new(
            "entrance",
            Pose::at([0.0, 0.0, 0.0]).rotated(DQuat::from_rotation_y(std::f64::consts::PI)),
        ),
    }
}
fn policy() -> GroundingPolicy {
    GroundingPolicy {
        clearance: 0.4,
        embed: 0.3,
        max_depth: 5.0,
        support_width: 0.3,
        max_span: 2.0,
        landing_length: 1.5,
        ramp_width: 2.0,
        max_grade: 0.3,
        max_ramp_length: 18.0,
        search_step: 0.5,
        vegetation_margin: 0.75,
        max_toe_step: 0.2,
    }
}
#[test]
fn flat_foundations_and_ramp_share_outward_convex_geometry() {
    let fit = fit_ground(&spec(), policy(), &patch(|_, _| 10.0), [0.0, 0.0], 0.0).unwrap();
    assert!((fit.pose.translation.y - 10.4).abs() < 1e-9);
    assert!(fit.ramp_grade <= policy().max_grade);
    assert!((fit.toe.y - 10.015).abs() < 1e-9);
    for solid in &fit.solids {
        let center = solid.vertices.iter().sum::<DVec3>() / 8.0;
        for [a, b, c] in solid.triangles() {
            assert!(
                (b - a).cross(c - a).dot(a - center) > 0.0,
                "inward or degenerate face"
            );
        }
        assert!(
            solid
                .vertices
                .iter()
                .take(4)
                .all(|p| p.y + fit.pose.translation.y <= 9.7 + 1e-9)
        );
    }
    for solid in &fit.solids {
        for local in solid.vertices {
            let world = fit.pose.transform_point(local);
            assert!(fit.exclusions.iter().any(|r| world.x >= r.min.x - 1e-9
                && world.x <= r.max.x + 1e-9
                && world.z >= r.min.y - 1e-9
                && world.z <= r.max.y + 1e-9));
        }
    }
}
#[test]
fn interior_extrema_are_not_missed_and_deep_sites_reject() {
    let ground = patch(|x, z| if x == 0.0 && z == 3.0 { 2.0 } else { 0.0 });
    let fit = fit_ground(&spec(), policy(), &ground, [0.0, 0.0], 0.0).unwrap();
    assert!((fit.pose.translation.y - 2.4).abs() < 1e-9);
    let depression = patch(|x, z| if x == 0.0 && z == 0.0 { -8.0 } else { 0.0 });
    assert!(matches!(
        fit_ground(&spec(), policy(), &depression, [0.0, 0.0], 0.0),
        Err(GroundError::SupportDepth { .. })
    ));
}
#[test]
fn sloped_surface_fits_with_yaw_and_world_translation() {
    let terrain = patch(|x, z| 12.0 + 0.02 * x + 0.08 * z);
    let fit = fit_ground(&spec(), policy(), &terrain, [2.25, -1.5], 0.65).unwrap();
    let shifted = TerrainPatch::new(
        [4960.0, -8040.0],
        1.0,
        [81, 81],
        (0..81)
            .flat_map(|z| {
                (0..81).map(move |x| {
                    12.0 + 0.02 * (f64::from(x) - 40.0) + 0.08 * (f64::from(z) - 40.0)
                })
            })
            .collect(),
    )
    .unwrap();
    let other = fit_ground(&spec(), policy(), &shifted, [5002.25, -8001.5], 0.65).unwrap();
    assert!(
        (other.pose.translation - fit.pose.translation - DVec3::new(5000.0, 0.0, -8000.0)).length()
            < 1e-9
    );
    assert!((other.toe - fit.toe - DVec3::new(5000.0, 0.0, -8000.0)).length() < 1e-9);
    for (a, b) in fit.solids.iter().zip(other.solids.iter()) {
        for (a, b) in a.vertices.iter().zip(b.vertices.iter()) {
            assert!((*a - *b).length() < 1e-9);
        }
    }
}
#[test]
fn invalid_inputs_and_unreachable_approach_report_errors() {
    assert!(TerrainPatch::new([0.0, 0.0], 1.0, [2, 2], vec![f64::NAN; 4]).is_err());
    let ground = patch(|_, _| 0.0);
    assert!(matches!(
        fit_ground(&spec(), policy(), &ground, [100.0, 0.0], 0.0),
        Err(GroundError::OutsidePatch)
    ));
    let mut limits = policy();
    limits.max_ramp_length = 0.5;
    assert!(matches!(
        fit_ground(&spec(), limits, &ground, [0.0, 0.0], 0.0),
        Err(GroundError::NoApproach)
    ));
    limits.search_step = 0.0;
    assert!(matches!(
        fit_ground(&spec(), limits, &ground, [0.0, 0.0], 0.0),
        Err(GroundError::Invalid(_))
    ));
}
#[test]
fn triangle_interpolation_and_clipping_use_collision_diagonal() {
    let terrain = TerrainPatch::new([0.0, 0.0], 1.0, [2, 2], vec![0.0, 1.0, 2.0, 10.0]).unwrap();
    let range = terrain
        .range(GroundRect::new([0.2, 0.2], [0.3, 0.3]), Pose::default())
        .unwrap();
    assert!((range[0] - 0.6).abs() < 1e-9 && (range[1] - 0.9).abs() < 1e-9);
    let range = terrain
        .range(GroundRect::new([0.7, 0.7], [0.8, 0.8]), Pose::default())
        .unwrap();
    assert!((range[0] - 4.9).abs() < 1e-9 && (range[1] - 6.6).abs() < 1e-9);
}

#[test]
fn an_obstacle_between_threshold_and_toe_blocks_the_approach() {
    let terrain = patch(|_, z| if z == -2.0 { 1.0 } else { 0.0 });
    let mut limits = policy();
    limits.max_ramp_length = 2.5;
    assert!(matches!(
        fit_ground(&spec(), limits, &terrain, [0.0, 0.0], 0.0),
        Err(GroundError::NoApproach)
    ));
}

#[test]
fn support_segment_embeds_below_a_depression_between_its_endpoints() {
    let terrain = patch(|x, z| if x == -3.0 && z == 0.0 { -0.8 } else { 0.0 });
    let fit = fit_ground(&spec(), policy(), &terrain, [0.0, 0.0], 0.0).unwrap();
    let segment = &fit.solids[0];
    assert!(
        segment
            .vertices
            .iter()
            .take(4)
            .all(|v| (v.y + fit.pose.translation.y + 1.1).abs() < 1e-9)
    );
}

#[test]
fn the_other_diagonal_reads_the_same_samples_as_a_different_surface() {
    let heights = vec![0.0, 1.0, 2.0, 10.0];
    let rect = GroundRect::new([0.2, 0.2], [0.3, 0.3]);
    let anti = TerrainPatch::new([0.0, 0.0], 1.0, [2, 2], heights.clone()).unwrap();
    assert_eq!(anti.diagonal(), Diagonal::Anti);
    let main =
        TerrainPatch::with_diagonal([0.0, 0.0], 1.0, [2, 2], heights, Diagonal::Main).unwrap();
    assert_eq!(main.diagonal(), Diagonal::Main);
    let range = anti.range(rect, Pose::default()).unwrap();
    assert!((range[0] - 0.6).abs() < 1e-9 && (range[1] - 0.9).abs() < 1e-9);
    // The same four samples, split the other way, put the low corner under a
    // face that climbs towards the high one instead of away from it.
    let range = main.range(rect, Pose::default()).unwrap();
    assert!((range[0] - 2.0).abs() < 1e-9 && (range[1] - 3.0).abs() < 1e-9);
}

#[test]
fn a_fitted_prism_converts_into_the_same_convex_proxy_an_element_declares() {
    let solid = GroundSolid::cuboid([2.0, 3.0, 4.0], Pose::at([1.0, 0.0, -1.0]));
    let convex = ConvexSolid::from(&solid);
    assert_eq!(convex.vertices, solid.vertices.to_vec());
    let [min, max] = convex.bounds().unwrap();
    assert_eq!(min.to_array(), [1.0, 0.0, -1.0]);
    assert_eq!(max.to_array(), [3.0, 3.0, 3.0]);
}
