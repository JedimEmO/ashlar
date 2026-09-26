//! Solid geometry regression tests using measured surfaces and volumes.
use ashlar::{
    Axis, Building, Element, FaceOrigin, Geometry, GeometryMesher, Instance, MirrorPlane, Part,
    Pose, TriangleMesh,
};
use ashlar_manifold::{ManifoldMesher, mesh_building};
use glam::{DQuat, DVec3};

/// The mesher shares vertices; these tests read triangles, so they expand the
/// result again. `welding_shares_vertices_without_changing_the_surface` is what
/// checks the indexed form itself.
fn mesh(geometry: &Geometry) -> TriangleMesh {
    let mut mesh = ManifoldMesher::default()
        .mesh(geometry)
        .expect("valid solid");
    mesh.unweld();
    mesh
}

fn volume(mesh: &TriangleMesh) -> f64 {
    mesh.positions
        .as_chunks::<3>()
        .0
        .iter()
        .map(|t| t[0].dot(t[1].cross(t[2])) / 6.0)
        .sum()
}

fn close(actual: f64, expected: f64) {
    assert!((actual - expected).abs() < 1e-7, "{actual} != {expected}");
}

fn area(mesh: &TriangleMesh) -> f64 {
    mesh.corners()
        .map(|corners| {
            let [a, b, c] = corners.map(|i| mesh.positions[i]);
            (b - a).cross(c - a).length() * 0.5
        })
        .sum()
}

fn cut_area(mesh: &TriangleMesh) -> f64 {
    mesh.corners()
        .zip(mesh.cut_faces())
        .filter(|(_, cut)| *cut)
        .map(|(corners, _)| {
            let [a, b, c] = corners.map(|i| mesh.positions[i]);
            (b - a).cross(c - a).length() * 0.5
        })
        .sum()
}

fn check_surface(mesh: &TriangleMesh) {
    let mut edges = std::collections::BTreeMap::new();
    let key = |p: DVec3| p.to_array().map(|v| if v == 0.0 { 0 } else { v.to_bits() });
    for triangle in mesh.positions.as_chunks::<3>().0 {
        for (a, b) in [(0, 1), (1, 2), (2, 0)] {
            let a = key(triangle[a]);
            let b = key(triangle[b]);
            let edge = edges.entry((a.min(b), a.max(b))).or_insert((0, 0));
            edge.0 += 1;
            edge.1 += if a < b { 1 } else { -1 };
        }
    }
    assert!(
        edges.values().all(|edge| *edge == (2, 0)),
        "surface must be closed and consistently wound"
    );
    assert_eq!(mesh.positions.len(), mesh.normals.len());
    assert_eq!(mesh.positions.len(), mesh.uvs.len());
    assert!(mesh.uvs.iter().flatten().all(|v| v.is_finite()));
    for (triangle, normals) in mesh
        .positions
        .as_chunks::<3>()
        .0
        .iter()
        .zip(mesh.normals.as_chunks::<3>().0.iter())
    {
        let normal = (triangle[1] - triangle[0])
            .cross(triangle[2] - triangle[0])
            .normalize();
        for shading in normals {
            close(shading.length(), 1.0);
            assert!(normal.dot(*shading) > 0.0, "shading normal points inward");
        }
    }
}

#[test]
fn box_has_outward_faces_and_correct_extent() {
    let mesh = mesh(&Geometry::cuboid([2.0, 3.0, 4.0]));
    close(volume(&mesh), 24.0);
    assert_eq!(mesh.positions.len(), 36);
    check_surface(&mesh);
    assert_eq!(
        mesh.positions.iter().copied().reduce(DVec3::min),
        Some(DVec3::ZERO)
    );
    assert_eq!(
        mesh.positions.iter().copied().reduce(DVec3::max),
        Some(DVec3::new(2.0, 3.0, 4.0))
    );
}

#[test]
fn cylinder_is_y_up_with_smooth_sides_and_creased_caps() {
    let mesh = mesh(&Geometry::cylinder(2.0, 3.0, 32));
    close(
        volume(&mesh),
        32.0 * 2.0 * (std::f64::consts::TAU / 32.0).sin() * 3.0,
    );
    check_surface(&mesh);
    for (p, n) in mesh.positions.iter().zip(&mesh.normals) {
        assert!(p.y >= -1e-8 && p.y <= 3.0 + 1e-8);
        if n.y.abs() < 0.5 {
            assert!(n.dot(DVec3::new(p.x, 0.0, p.z).normalize()) > 0.999);
        } else {
            close(n.y.abs(), 1.0);
        }
    }
}

#[test]
fn concave_extrusion_preserves_xz_coordinates_in_both_windings() {
    let profile = vec![
        [1.0, 2.0],
        [4.0, 2.0],
        [4.0, 3.0],
        [2.0, 3.0],
        [2.0, 5.0],
        [1.0, 5.0],
    ];
    for profile in [profile.clone(), profile.into_iter().rev().collect()] {
        let mesh = mesh(&Geometry::extrude(profile, 2.0));
        close(volume(&mesh), 10.0);
        check_surface(&mesh);
        let min = mesh
            .positions
            .iter()
            .copied()
            .reduce(DVec3::min)
            .expect("vertices");
        let max = mesh
            .positions
            .iter()
            .copied()
            .reduce(DVec3::max)
            .expect("vertices");
        assert!((min - DVec3::new(1.0, 0.0, 2.0)).length() < 1e-8);
        assert!((max - DVec3::new(4.0, 2.0, 5.0)).length() < 1e-8);
    }
}

#[test]
fn doorway_is_open_and_cut_reveals_are_present() {
    let mesh = mesh(
        &Geometry::cuboid([4.0, 4.0, 0.5])
            .subtract(Geometry::cuboid([2.0, 3.0, 1.0]).placed(Pose::at([1.0, -0.1, -0.2]))),
    );
    close(volume(&mesh), 8.0 - 2.0 * 2.9 * 0.5);
    check_surface(&mesh);
    // Every triangle's centroid is outside the removed doorway volume.
    for t in mesh.positions.as_chunks::<3>().0 {
        let p = (t[0] + t[1] + t[2]) / 3.0;
        assert!(!(p.x > 1.0 + 1e-8 && p.x < 3.0 - 1e-8 && p.y < 2.9 - 1e-8));
    }
    assert!(mesh.normals.iter().any(|n| n.x > 0.99));
    assert!(mesh.normals.iter().any(|n| n.x < -0.99));
}

#[test]
fn nested_rotated_subtraction_preserves_operand_and_parent_frames() {
    let base = Geometry::cuboid([4.0, 4.0, 4.0]);
    let hollow_cutter = Geometry::cuboid([2.0, 5.0, 2.0])
        .subtract(Geometry::cuboid([1.0, 6.0, 1.0]).placed(Pose::at([0.5, -0.5, 0.5])))
        .placed(Pose::at([1.0, -0.5, 1.0]));
    let cut = base.subtract(hollow_cutter);
    let pose = Pose::at([20.0, -4.0, 11.0]).rotated(DQuat::from_rotation_y(0.73));
    let plain = mesh(&cut);
    let transformed = mesh(&cut.placed(pose));
    close(volume(&plain), 52.0);
    close(volume(&transformed), 52.0);
    for p in &transformed.positions {
        let local = pose.rotation.inverse() * (*p - pose.translation);
        assert!(local.min_element() > -1e-7 && local.max_element() < 4.0 + 1e-7);
    }
    check_surface(&transformed);
}

#[test]
fn empty_and_disjoint_differences_have_defined_results() {
    let base = Geometry::cuboid([1.0; 3]);
    assert!(
        mesh(&base.clone().subtract(base.clone()))
            .positions
            .is_empty()
    );
    close(
        volume(&mesh(&base.subtract(
            Geometry::cuboid([1.0; 3]).placed(Pose::at([3.0; 3])),
        ))),
        1.0,
    );
}

#[test]
fn budgets_and_invalid_raw_geometry_return_errors() {
    let mesher = ManifoldMesher {
        max_segments: 8,
        ..Default::default()
    };
    assert!(
        mesher
            .mesh(&Geometry::cylinder(1.0, 1.0, 9))
            .expect_err("budget")
            .reason
            .contains("segment")
    );
    let mesher = ManifoldMesher {
        max_triangles: 1,
        ..Default::default()
    };
    assert!(mesher.mesh(&Geometry::cuboid([1.0; 3])).is_err());
    assert!(
        ManifoldMesher::default()
            .mesh(&Geometry::cuboid([f64::NAN; 3]))
            .is_err()
    );
}

#[test]
fn instances_share_part_meshes_and_retain_material_overrides() {
    let part = Part::builder("bay")
        .element(Element::new("solid", Geometry::cuboid([1.0; 3]), "surface"))
        .build()
        .expect("part");
    let building = Building::builder("test")
        .part(part)
        .material("surface", "a")
        .instance(Instance::new("first", "bay"))
        .instance(
            Instance::new("second", "bay")
                .material("surface", "b")
                .placed(Pose::at([10.0, 0.0, 0.0])),
        )
        .build()
        .expect("recipe");
    let output = mesh_building(&building, &ManifoldMesher::default()).expect("meshed");
    assert_eq!(output.parts.len(), 1);
    assert_eq!(output.building.material("first", "surface"), Some("a"));
    assert_eq!(output.building.material("second", "surface"), Some("b"));
    assert_eq!(output.building, building);
}

#[test]
fn chamfers_remove_corners_but_preserve_box_dimensions() {
    let geometry = Geometry::chamfered_cuboid([2.0, 3.0, 4.0], 0.1);
    let output = mesh(&geometry);
    check_surface(&output);
    assert!(volume(&output) < 24.0 && volume(&output) > 23.0);
    assert_eq!(
        output.positions.iter().copied().reduce(DVec3::min),
        Some(DVec3::ZERO)
    );
    assert_eq!(
        output.positions.iter().copied().reduce(DVec3::max),
        Some(DVec3::new(2.0, 3.0, 4.0))
    );
    assert!(output.positions.iter().all(|p| p.x + p.y >= 0.1 - 1e-9));
    assert!(
        output
            .positions
            .iter()
            .all(|p| p.x + p.y + p.z >= 0.2 - 1e-9)
    );
    assert!(Geometry::chamfered_cuboid([1.0; 3], 0.5).check().is_err());
    assert!(
        Geometry::chamfered_cuboid([1.0; 3], f64::NAN)
            .check()
            .is_err()
    );
}

#[test]
fn cut_material_batches_follow_boolean_provenance() {
    let geometry = Geometry::cuboid([4.0, 4.0, 0.5])
        .subtract(Geometry::cuboid([2.0, 3.0, 1.0]).placed(Pose::at([1.0, -0.1, -0.2])));
    let output = mesh(&geometry);
    let cut_area: f64 = output
        .positions
        .as_chunks::<3>()
        .0
        .iter()
        .zip(output.cut_faces())
        .filter(|(_, cut)| *cut)
        .map(|(t, _)| (t[1] - t[0]).cross(t[2] - t[0]).length() * 0.5)
        .sum();
    close(cut_area, 3.9);
    let part = Part::builder("wall")
        .element(Element::new("panel", geometry, "surface").cut_material("reveal"))
        .build()
        .expect("part");
    let building = Building::builder("cut")
        .part(part)
        .material("surface", "paint")
        .material("reveal", "concrete")
        .instance(Instance::new("wall", "wall").material("reveal", "steel"))
        .build()
        .expect("building");
    let result = mesh_building(&building, &ManifoldMesher::default()).expect("mesh");
    assert_eq!(result.parts["wall"].len(), 2);
    assert!(
        result.parts["wall"]
            .iter()
            .all(|batch| batch.mesh.cut_faces().all(|cut| cut == batch.is_cut))
    );
    assert_eq!(result.building.material("wall", "reveal"), Some("steel"));
}

#[test]
fn metre_uvs_preserve_surface_lengths_on_rotated_and_bevelled_faces() {
    let geometry = Geometry::chamfered_cuboid([0.18, 6.0, 3.7], 0.04)
        .subtract(Geometry::cuboid([0.4, 2.0, 1.0]).placed(Pose::at([-0.1, 1.0, 0.8])))
        .placed(Pose::at([11.0, -7.0, 3.0]).rotated(DQuat::from_euler(
            glam::EulerRot::XYZ,
            0.31,
            0.67,
            0.21,
        )));
    let mesh = mesh(&geometry);
    for (points, uvs) in mesh
        .positions
        .as_chunks::<3>()
        .0
        .iter()
        .zip(mesh.uvs.as_chunks::<3>().0)
    {
        for (a, b) in [(0, 1), (1, 2), (2, 0)] {
            let uv_length =
                glam::DVec2::from_array(uvs[a]).distance(glam::DVec2::from_array(uvs[b]));
            close(uv_length, points[a].distance(points[b]));
        }
    }
}

#[test]
fn a_union_fuses_overlapping_solids_into_one_closed_surface() {
    let left = Geometry::cuboid([2.0, 2.0, 2.0]);
    let right = Geometry::cuboid([2.0, 2.0, 2.0]).placed(Pose::at([1.0, 0.0, 0.0]));
    let output = mesh(&left.clone().union(right));
    close(volume(&output), 12.0);
    check_surface(&output);
    let disjoint = mesh(&Geometry::union_all([
        left,
        Geometry::cuboid([1.0; 3]).placed(Pose::at([10.0, 0.0, 0.0])),
    ]));
    close(volume(&disjoint), 9.0);
    check_surface(&disjoint);
}

#[test]
fn an_array_repeats_a_solid_along_a_translation_and_around_a_rotation() {
    let post = Geometry::cuboid([0.5, 3.0, 0.5]);
    let row = mesh(&post.clone().arrayed(4, Pose::at([2.0, 0.0, 0.0])));
    close(volume(&row), 4.0 * 0.75);
    check_surface(&row);
    assert!(
        row.positions
            .iter()
            .map(|p| p.x)
            .fold(f64::NEG_INFINITY, f64::max)
            > 6.4
    );
    let ring = mesh(
        &post
            .clone()
            .placed(Pose::at([4.0, 0.0, 0.0]))
            .arrayed(6, Pose::default().rotated(DQuat::from_rotation_y(1.0))),
    );
    close(volume(&ring), 6.0 * 0.75);
    check_surface(&ring);
    // One copy is the solid itself, and copies that overlap merge rather than
    // leaving an interior wall.
    close(
        volume(&mesh(&post.clone().arrayed(1, Pose::at([1.0; 3])))),
        0.75,
    );
    close(
        volume(&mesh(&post.arrayed(2, Pose::at([0.25, 0.0, 0.0])))),
        1.125,
    );
}

#[test]
fn a_mirror_reflects_a_solid_and_leaves_its_faces_pointing_outward() {
    let wedge = Geometry::extrude([[0.0, 0.0], [3.0, 0.0], [3.0, 1.0]], 2.0);
    let plain = mesh(&wedge);
    let reflected = mesh(&wedge.clone().mirrored(MirrorPlane::new(Axis::X, 5.0)));
    close(volume(&reflected), volume(&plain));
    check_surface(&reflected);
    for p in &reflected.positions {
        assert!(p.x >= 7.0 - 1e-9 && p.x <= 10.0 + 1e-9, "{p}");
    }
    // A mirrored pair is one symmetric solid, not two overlapping ones.
    let pair = mesh(&Geometry::union_all([
        wedge.clone(),
        wedge.mirrored(MirrorPlane::new(Axis::X, 3.0)),
    ]));
    close(volume(&pair), 2.0 * volume(&plain));
    check_surface(&pair);
}

#[test]
fn cut_provenance_survives_arrays_mirrors_and_unions() {
    let bay = Geometry::cuboid([4.0, 3.0, 0.5])
        .subtract(Geometry::cuboid([2.0, 2.0, 1.0]).placed(Pose::at([1.0, 0.5, -0.25])));
    let cut_area = |mesh: &TriangleMesh| -> f64 {
        mesh.positions
            .as_chunks::<3>()
            .0
            .iter()
            .zip(mesh.cut_faces())
            .filter(|(_, cut)| *cut)
            .map(|(t, _)| (t[1] - t[0]).cross(t[2] - t[0]).length() * 0.5)
            .sum()
    };
    let single = cut_area(&mesh(&bay));
    assert!(single > 0.0);
    close(
        cut_area(&mesh(&bay.clone().arrayed(3, Pose::at([5.0, 0.0, 0.0])))),
        3.0 * single,
    );
    close(
        cut_area(&mesh(&bay.clone().mirrored(MirrorPlane::new(Axis::Z, 4.0)))),
        single,
    );
    close(
        cut_area(&mesh(&Geometry::union_all([
            bay.clone(),
            bay.placed(Pose::at([0.0, 0.0, 8.0])),
        ]))),
        2.0 * single,
    );
}

#[test]
fn the_new_nodes_reject_empty_groups_and_oversized_repetition() {
    assert!(
        Geometry::union_all([Geometry::cuboid([1.0; 3])])
            .check()
            .is_err()
    );
    assert!(
        Geometry::cuboid([1.0; 3])
            .arrayed(0, Pose::at([1.0, 0.0, 0.0]))
            .check()
            .is_err()
    );
    assert!(
        Geometry::cuboid([1.0; 3])
            .arrayed(ashlar::MAX_ARRAY_COUNT + 1, Pose::at([1.0, 0.0, 0.0]))
            .check()
            .is_err()
    );
    assert!(
        Geometry::cuboid([1.0; 3])
            .mirrored(MirrorPlane::new(Axis::Y, f64::NAN))
            .check()
            .is_err()
    );
    assert!(
        Geometry::cuboid([1.0; 3])
            .arrayed(2, Pose::at([f64::INFINITY, 0.0, 0.0]))
            .check()
            .is_err()
    );
}

fn uv_mesh(geometry: Geometry, mode: ashlar::UvMode) -> TriangleMesh {
    let part = Part::builder("part")
        .element(Element::new("solid", geometry, "surface").uv(mode))
        .build()
        .expect("part");
    let building = Building::builder("uv")
        .part(part)
        .material("surface", "any")
        .instance(Instance::new("only", "part"))
        .build()
        .expect("building");
    let mut mesh = mesh_building(&building, &ManifoldMesher::default())
        .expect("meshed")
        .parts["part"][0]
        .mesh
        .clone();
    mesh.unweld();
    mesh
}

#[test]
fn box_uvs_share_one_grid_across_coplanar_elements_and_planar_uvs_do_not() {
    let far = Pose::at([7.0, 0.0, 5.0]);
    let plate = Geometry::cuboid([2.0, 0.5, 3.0]);
    let boxed = uv_mesh(plate.clone().placed(far), ashlar::UvMode::Box);
    let mut horizontal = 0;
    for (points, uvs) in boxed
        .positions
        .as_chunks::<3>()
        .0
        .iter()
        .zip(boxed.uvs.as_chunks::<3>().0)
    {
        let normal = (points[1] - points[0])
            .cross(points[2] - points[0])
            .normalize();
        if normal.y.abs() < 0.9 {
            continue;
        }
        horizontal += 1;
        // A top or bottom face reads X and Z straight out of part space, so a
        // neighbour placed anywhere on the same plane continues the same grid.
        for (point, uv) in points.iter().zip(uvs) {
            close(uv[0], point.x);
            close(uv[1], point.z);
        }
    }
    assert_eq!(horizontal, 4);
    // On an axis-aligned face the two modes agree, because a planar frame on
    // such a face is already the world grid. A tilted face is where they part:
    // planar keeps surface lengths, box reads the world axes and foreshortens.
    let planar = uv_mesh(plate.clone().placed(far), ashlar::UvMode::Planar);
    assert_eq!(planar.uvs, boxed.uvs);
    let tilt = Pose::at([7.0, 0.0, 5.0]).rotated(DQuat::from_rotation_z(0.6));
    let (planar, boxed) = (
        uv_mesh(plate.clone().placed(tilt), ashlar::UvMode::Planar),
        uv_mesh(plate.placed(tilt), ashlar::UvMode::Box),
    );
    let span = |mesh: &TriangleMesh, corner: usize| {
        let uvs = mesh.uvs.as_chunks::<3>().0;
        let points = mesh.positions.as_chunks::<3>().0;
        glam::DVec2::from_array(uvs[corner][0]).distance(glam::DVec2::from_array(uvs[corner][1]))
            / points[corner][0].distance(points[corner][1])
    };
    let tightest = (0..planar.positions.len() / 3)
        .map(|t| span(&boxed, t))
        .fold(f64::INFINITY, f64::min);
    for triangle in 0..planar.positions.len() / 3 {
        close(span(&planar, triangle), 1.0);
        assert!(span(&boxed, triangle) <= 1.0 + 1e-9);
    }
    assert!(tightest < 1.0 - 1e-6, "a tilted face has to foreshorten");
}

#[test]
fn cylindrical_uvs_are_metre_true_around_the_axis_with_one_seam() {
    let radius = 1.5;
    let mesh = uv_mesh(
        Geometry::cylinder(radius, 4.0, 64),
        ashlar::UvMode::Cylindrical { axis: Axis::Y },
    );
    let mut seam = 0.0_f64;
    for (points, uvs) in mesh
        .positions
        .as_chunks::<3>()
        .0
        .iter()
        .zip(mesh.uvs.as_chunks::<3>().0)
    {
        let normal = (points[1] - points[0])
            .cross(points[2] - points[0])
            .normalize();
        if normal.y.abs() > 0.7 {
            continue;
        }
        for (a, b) in [(0, 1), (1, 2), (2, 0)] {
            // V is height, and U is arc length, which is within a chord's
            // rounding of the straight distance at this subdivision.
            close(uvs[a][1], points[a].y);
            let arc = (uvs[a][0] - uvs[b][0]).abs();
            let chord = DVec3::new(points[a].x, 0.0, points[a].z).distance(DVec3::new(
                points[b].x,
                0.0,
                points[b].z,
            ));
            assert!(
                arc >= chord - 1e-9 && arc <= chord * 1.001 + 1e-9,
                "{arc} vs {chord}"
            );
        }
        seam = seam.max(
            uvs.iter().map(|uv| uv[0]).fold(f64::NEG_INFINITY, f64::max)
                - uvs.iter().map(|uv| uv[0]).fold(f64::INFINITY, f64::min),
        );
    }
    let span = mesh
        .uvs
        .iter()
        .map(|uv| uv[0])
        .fold(f64::NEG_INFINITY, f64::max)
        - mesh
            .uvs
            .iter()
            .map(|uv| uv[0])
            .fold(f64::INFINITY, f64::min);
    let turn = std::f64::consts::TAU * radius;
    // U covers exactly one turn: the wrap is a single seam, not a fan of them,
    // and no triangle straddles more than a fraction of the circumference.
    assert!(span > turn * 0.98 && span < turn * 1.05, "{span} vs {turn}");
    assert!(seam < turn / 8.0, "a triangle spans {seam} of {turn}");
    // The caps are perpendicular to the axis, so they keep a plane mapping
    // instead of collapsing onto the axis.
    for (points, uvs) in mesh
        .positions
        .as_chunks::<3>()
        .0
        .iter()
        .zip(mesh.uvs.as_chunks::<3>().0)
    {
        let normal = (points[1] - points[0])
            .cross(points[2] - points[0])
            .normalize();
        if normal.y.abs() > 0.7 {
            for (point, uv) in points.iter().zip(uvs) {
                close(uv[0], point.x);
                close(uv[1], point.z);
            }
        }
    }
}

#[test]
fn welding_shares_vertices_without_changing_the_surface() {
    let geometry = Geometry::chamfered_cuboid([4.0, 3.0, 0.8], 0.05)
        .subtract(Geometry::cuboid([2.0, 2.0, 1.0]).placed(Pose::at([1.0, 0.5, -0.1])));
    let welded = ManifoldMesher::default().mesh(&geometry).expect("solid");
    assert!(welded.is_consistent());
    let mut expanded = welded.clone();
    expanded.unweld();
    assert_eq!(expanded.positions.len(), welded.triangle_count() * 3);
    assert!(
        welded.positions.len() * 4 < expanded.positions.len() * 3,
        "{} of {} vertices survived welding",
        welded.positions.len(),
        expanded.positions.len()
    );
    // Welding is a change of storage only: the same triangles, in order.
    assert_eq!(
        welded.triangles().collect::<Vec<_>>(),
        expanded.triangles().collect::<Vec<_>>()
    );
    assert_eq!(welded.sources, expanded.sources);
    for (a, b) in welded.corners().zip(expanded.corners()) {
        for (a, b) in a.into_iter().zip(b) {
            assert_eq!(welded.normals[a], expanded.normals[b]);
            close(welded.uvs[a][0], expanded.uvs[b][0]);
            close(welded.uvs[a][1], expanded.uvs[b][1]);
        }
    }
    // A crease or a UV seam still splits: a closed box keeps more vertices than
    // its eight corners.
    let cube = ManifoldMesher::default()
        .mesh(&Geometry::cuboid([1.0; 3]))
        .expect("cube");
    assert_eq!(cube.triangle_count(), 12);
    assert_eq!(cube.positions.len(), 24);
    let unwelded = ManifoldMesher {
        weld_tolerance: 0.0,
        ..Default::default()
    }
    .mesh(&Geometry::cuboid([1.0; 3]))
    .expect("cube");
    assert_eq!(unwelded.positions.len(), 36);
}

#[test]
fn declared_collision_derives_a_box_or_a_hull_per_element_and_places_it() {
    use ashlar::Collision;
    let part = Part::builder("kit")
        .element(
            Element::new(
                "body",
                Geometry::chamfered_cuboid([4.0, 3.0, 2.0], 0.4),
                "s",
            )
            .collision(Collision::Hull),
        )
        .element(
            Element::new(
                "slab",
                Geometry::cuboid([4.0, 0.5, 2.0]).placed(Pose::at([0.0, 3.0, 0.0])),
                "s",
            )
            .collision(Collision::Bounds),
        )
        .element(Element::new(
            "trim",
            Geometry::cuboid([4.0, 0.1, 0.1]).placed(Pose::at([0.0, 3.5, 0.0])),
            "s",
        ))
        .build()
        .expect("part");
    let far = Pose::at([100.0, 0.0, 0.0]).rotated(DQuat::from_rotation_y(0.4));
    let building = Building::builder("collide")
        .part(part)
        .material("s", "any")
        .instance(Instance::new("here", "kit"))
        .instance(Instance::new("there", "kit").placed(far))
        .build()
        .expect("building");
    let meshed = mesh_building(&building, &ManifoldMesher::default()).expect("meshed");
    // Declared elements only, once per definition and once more per placement.
    let ids: Vec<&str> = meshed.part_colliders["kit"]
        .iter()
        .map(|c| c.id.as_str())
        .collect();
    assert_eq!(ids, ["body", "slab"]);
    let placed = meshed.colliders();
    assert_eq!(placed.len(), 4);
    let hull = &placed[0].solid;
    let slab = &placed[1].solid;
    assert_eq!(slab.vertices.len(), 8);
    // A hull follows the chamfer, so it has the corner points a box has not.
    assert!(hull.vertices.len() > 8);
    let [min, max] = hull.bounds().expect("hull bounds");
    assert!((min - DVec3::ZERO).length() < 1e-9);
    assert!((max - DVec3::new(4.0, 3.0, 2.0)).length() < 1e-9);
    assert!(
        hull.vertices
            .iter()
            .all(|p| p.x + p.y >= 0.4 - 1e-9 && p.x + p.y + p.z >= 0.8 - 1e-9)
    );
    // Placement is the instance pose, not a refitted axis-aligned box.
    for (local, world) in placed[0]
        .solid
        .vertices
        .iter()
        .zip(&placed[2].solid.vertices)
    {
        assert!((far.transform_point(*local) - *world).length() < 1e-9);
    }
}

#[test]
fn two_openings_in_one_wall_wear_two_reveals() {
    let geometry = Geometry::cuboid([6.0, 3.0, 0.3])
        .subtract(Geometry::cuboid([1.0, 2.0, 0.5]).placed(Pose::at([1.0, 0.0, -0.1])))
        .subtract(
            Geometry::cuboid([1.0, 1.0, 0.5])
                .placed(Pose::at([4.0, 1.0, -0.1]))
                .cut_material("sill"),
        );
    let part = Part::builder("wall")
        .element(Element::new("panel", geometry, "wall").cut_material("reveal"))
        .build()
        .expect("part");
    let building = Building::builder("openings")
        .part(part)
        .material("wall", "brick")
        .material("reveal", "plaster")
        .material("sill", "stone")
        .instance(Instance::new("only", "wall"))
        .build()
        .expect("building");
    let meshed = mesh_building(&building, &ManifoldMesher::default()).expect("meshed");
    let batches = &meshed.parts["wall"];
    let slots: Vec<&str> = batches
        .iter()
        .map(|batch| batch.material_slot.as_str())
        .collect();
    assert_eq!(slots, ["wall", "reveal", "sill"]);

    let wall = &batches[0];
    assert!(
        !wall.mesh.cut_faces().any(|cut| cut),
        "the wall batch holds no cut face"
    );

    let reveal = &batches[1];
    assert!(reveal.is_cut);
    assert!(
        reveal
            .mesh
            .sources
            .iter()
            .all(|source| source.origin == FaceOrigin::Cutter(0)),
        "the door reveal belongs to cutter zero"
    );
    // Two jambs two metres tall and a one-metre head, through a 0.3 wall.
    close(area(&reveal.mesh), 1.5);

    let sill = &batches[2];
    assert!(sill.is_cut);
    assert!(
        sill.mesh
            .sources
            .iter()
            .all(|source| source.origin == FaceOrigin::Cutter(1)),
        "the window reveal belongs to cutter one"
    );
    // All four sides of a one-metre window, through a 0.3 wall.
    close(area(&sill.mesh), 1.2);
}

#[test]
fn cutter_indices_follow_the_geometrys_own_numbering() {
    let block = Geometry::cuboid([6.0, 4.0, 1.0]);
    let cutter0 = Geometry::cuboid([1.0, 1.0, 2.0]).placed(Pose::at([0.5, 0.5, -0.5]));
    let solid = block.subtract(cutter0);
    let other = Geometry::cuboid([2.0, 2.0, 1.0]).placed(Pose::at([7.0, 0.0, 0.0]));
    let union = Geometry::union_all([solid, other]);

    // A cutter root that is itself a difference: its hole is part of this
    // cutter and takes no index of its own.
    let plug = Geometry::cuboid([1.0, 1.0, 2.0]).placed(Pose::at([2.5, 1.0, -0.5]));
    let hole = Geometry::cuboid([0.4, 0.4, 3.0]).placed(Pose::at([2.8, 1.3, -1.0]));
    let compound = plug.subtract(hole);

    let cutter2 = Geometry::cuboid([0.6, 0.6, 2.0]).placed(Pose::at([4.0, 2.0, -0.5]));
    let geometry = union.subtract(compound).subtract(cutter2);

    assert_eq!(geometry.cutters().len(), 3);
    let output = ManifoldMesher::default().mesh(&geometry).expect("solid");
    let indices: std::collections::BTreeSet<u32> = output
        .sources
        .iter()
        .filter_map(|source| match source.origin {
            FaceOrigin::Cutter(k) => Some(k),
            _ => None,
        })
        .collect();
    assert_eq!(indices, std::collections::BTreeSet::from([0, 1, 2]));

    // Absolute, unrotated poses in one frame: each cutter's bounds are its pose
    // translation plus its own size, and a compound is bounded by its outer
    // solid because the hole sits inside it.
    let bounds = [
        (DVec3::new(0.5, 0.5, -0.5), DVec3::new(1.0, 1.0, 2.0)),
        (DVec3::new(2.5, 1.0, -0.5), DVec3::new(1.0, 1.0, 2.0)),
        (DVec3::new(4.0, 2.0, -0.5), DVec3::new(0.6, 0.6, 2.0)),
    ];
    for (face, corners) in output.corners().enumerate() {
        let FaceOrigin::Cutter(k) = output.sources[face].origin else {
            continue;
        };
        let (min, size) = bounds[k as usize];
        let max = min + size;
        for corner in corners {
            let point = output.positions[corner];
            assert!(
                point.cmpge(min - DVec3::splat(1e-6)).all()
                    && point.cmple(max + DVec3::splat(1e-6)).all(),
                "cutter {k} face at {point} escapes {min}..{max}"
            );
        }
    }
}

#[test]
fn an_arrays_copies_share_cutter_numbers() {
    let wall = Geometry::cuboid([2.0, 2.0, 0.5]);
    let opening = Geometry::cuboid([0.5, 0.5, 1.0]).placed(Pose::at([0.5, 0.5, -0.25]));
    let single = wall.subtract(opening);
    let copies = mesh(&single.clone().arrayed(3, Pose::at([3.0, 0.0, 0.0])));
    let indices: std::collections::BTreeSet<u32> = copies
        .sources
        .iter()
        .filter_map(|source| match source.origin {
            FaceOrigin::Cutter(k) => Some(k),
            _ => None,
        })
        .collect();
    assert_eq!(indices, std::collections::BTreeSet::from([0]));
    close(cut_area(&copies), 3.0 * cut_area(&mesh(&single)));
}

#[test]
fn a_chamfered_boxs_faces_are_flat() {
    let output = ManifoldMesher::default()
        .mesh(&Geometry::chamfered_cuboid([2.6, 3.8, 6.7], 0.045))
        .expect("valid solid");
    let mut axis_faces = 0;
    for (face, corners) in output.corners().enumerate() {
        let points = corners.map(|index| output.positions[index]);
        let normal = (points[1] - points[0])
            .cross(points[2] - points[0])
            .normalize();
        if normal.abs().max_element() <= 0.999_999 {
            continue;
        }
        axis_faces += 1;
        for corner in corners {
            assert!(
                output.normals[corner].abs_diff_eq(normal, 1e-9),
                "face {face} corner {corner} wears {} not {normal}",
                output.normals[corner]
            );
        }
    }
    assert!(axis_faces > 0, "the chamfered box keeps its flat faces");
}

#[test]
fn a_cylinder_is_still_smooth_around_and_creased_at_its_caps() {
    let output = mesh(&Geometry::cylinder(1.0, 2.0, 32));
    for (position, normal) in output.positions.iter().zip(&output.normals) {
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
fn a_cutter_slot_equal_to_the_main_slot_is_one_batch() {
    let geometry = Geometry::cuboid([2.0, 2.0, 0.5]).subtract(
        Geometry::cuboid([0.5, 0.5, 1.0])
            .placed(Pose::at([0.5, 0.5, -0.25]))
            .cut_material("wall"),
    );
    let part = Part::builder("wall")
        .element(Element::new("panel", geometry, "wall"))
        .build()
        .expect("part");
    let building = Building::builder("shared")
        .part(part)
        .material("wall", "brick")
        .instance(Instance::new("only", "wall"))
        .build()
        .expect("building");
    let meshed = mesh_building(&building, &ManifoldMesher::default()).expect("meshed");
    let batches = &meshed.parts["wall"];
    assert_eq!(batches.len(), 1);
    assert_eq!(batches[0].material_slot, "wall");
    assert!(!batches[0].is_cut, "the batch mixes cut and uncut faces");
    assert!(batches[0].mesh.cut_faces().any(|cut| cut));
    assert!(batches[0].mesh.cut_faces().any(|cut| !cut));
}
