//! A damage log applied from nothing: the stateless path a load, a late-joining
//! client and a server replaying a list all take.

use ashlar::{
    Building, Collision, Damage, DamageLog, Element, FaceOrigin, FaceSource, Geometry, Instance,
    MergeGroup, MeshedBuilding, Part, Pose, Side, TriangleCollider, TriangleMesh, UvMode,
};
use ashlar_manifold::{ManifoldMesher, mesh_building, mesh_building_with_damage};
use glam::DVec3;

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

fn collider_volume(collider: &TriangleCollider) -> f64 {
    collider
        .indices
        .as_chunks::<3>()
        .0
        .iter()
        .map(|triangle| {
            let [a, b, c] = triangle.map(|index| collider.positions[index as usize]);
            a.dot(b.cross(c)) / 6.0
        })
        .sum()
}

fn total_volume(meshed: &MeshedBuilding) -> f64 {
    meshed
        .groups
        .iter()
        .flat_map(|group| &group.batches)
        .map(|batch| volume(&batch.mesh))
        .sum()
}

/// The part every wall test places: a wall with one opening whose reveal wears
/// its own slot, and a pane that stays its own mesh.
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

/// Three bays in a row, the middle one wearing a brick face instead of paint,
/// and a palette slot for damage.
fn wall(merged: bool) -> Building {
    let mut builder = Building::builder("wall")
        .part(bay())
        .material("surface", "paint")
        .material("reveal", "steel")
        .material("glass", "glass")
        .material("rubble", "metro:rubble")
        .instance(Instance::new("a", "bay"))
        .instance(
            Instance::new("b", "bay")
                .placed(Pose::at([4.0, 0.0, 0.0]))
                .material("surface", "brick"),
        )
        .instance(Instance::new("c", "bay").placed(Pose::at([8.0, 0.0, 0.0])));
    if merged {
        builder = builder.merged();
    }
    builder.build().expect("valid wall")
}

/// Three bays in two groups: `a` and `b` in `"low"`, `c` in `"high"`.
fn grouped_wall() -> Building {
    Building::builder("wall")
        .part(bay())
        .material("surface", "paint")
        .material("reveal", "steel")
        .material("glass", "glass")
        .material("rubble", "metro:rubble")
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

/// A box that opens a square hole in bay `a`: it spans x 0.2 to 1.2 and y 1.5
/// to 2.5, clear of the doorway (x 1.5 to 2.5, y 0 to 2) and of the top edge at
/// y = 3, and it fully covers the wall's 0.3 thickness in z.
fn square_hole() -> Damage {
    Damage::new(Geometry::cuboid([1.0, 1.0, 1.0]), "rubble").placed(Pose::at([0.2, 1.5, -0.35]))
}

#[test]
fn an_empty_log_is_mesh_building() {
    let backend = ManifoldMesher::default();
    let plain = mesh_building(&wall(true), &backend).expect("meshes");
    let replayed =
        mesh_building_with_damage(&wall(true), &DamageLog::default(), &backend).expect("meshes");
    assert_eq!(plain.groups.len(), replayed.groups.len());
    for (before, after) in plain.groups.iter().zip(&replayed.groups) {
        assert!(after.collider.is_none(), "nothing was hit");
        assert_eq!(before.batches.len(), after.batches.len());
        for (left, right) in before.batches.iter().zip(&after.batches) {
            assert_eq!(left.mesh, right.mesh);
        }
    }
    assert!(replayed.damage.is_empty(), "no hit was recorded");
}

#[test]
fn a_blast_removes_exactly_what_it_overlaps() {
    let backend = ManifoldMesher::default();
    let before = total_volume(&mesh_building(&wall(true), &backend).expect("meshes"));
    let log = DamageLog(vec![square_hole()]);
    let meshed = mesh_building_with_damage(&wall(true), &log, &backend).expect("meshes");
    let after = total_volume(&meshed);
    assert!(
        (before - after - 0.3).abs() < 1e-9,
        "removed {}",
        before - after
    );
}

#[test]
fn exposed_faces_wear_the_damage_slot_and_are_interior() {
    let backend = ManifoldMesher::default();
    let log = DamageLog(vec![square_hole()]);
    let meshed = mesh_building_with_damage(&wall(true), &log, &backend).expect("meshes");
    let group = &meshed.groups[0];
    let rubble = group
        .batches
        .iter()
        .find(|batch| batch.binding.material == "metro:rubble")
        .expect("a rubble batch");
    assert_eq!(rubble.side, Side::Interior);
    assert!(
        (area(&rubble.mesh) - 1.2).abs() < 1e-9,
        "the four inner walls of the square hole are 4 * 1.0 * 0.3, area {}",
        area(&rubble.mesh)
    );
    for source in &rubble.mesh.sources {
        assert_eq!(source.origin, FaceOrigin::Damage(0));
        assert_eq!(source.operand, FaceSource::NO_OPERAND);
    }
    for batch in &group.batches {
        if batch.binding.material == "metro:rubble" {
            continue;
        }
        for source in &batch.mesh.sources {
            assert!(
                !matches!(source.origin, FaceOrigin::Damage(_)),
                "only the rubble batch carries damage faces"
            );
        }
    }
}

#[test]
fn untouched_faces_keep_their_bindings() {
    let backend = ManifoldMesher::default();
    let log = DamageLog(vec![square_hole()]);
    let meshed = mesh_building_with_damage(&wall(true), &log, &backend).expect("meshes");
    let brick = meshed.groups[0]
        .batches
        .iter()
        .find(|batch| batch.binding.material == "brick")
        .expect("a brick batch");
    for source in &brick.mesh.sources {
        assert_eq!(source.operand, 1, "only the middle instance wears brick");
    }
}

#[test]
fn damage_indices_are_the_logs_not_the_groups() {
    let backend = ManifoldMesher::default();
    let log = DamageLog(vec![
        square_hole(),
        Damage::new(Geometry::cuboid([1.0, 1.0, 1.0]), "rubble")
            .placed(Pose::at([8.2, 1.5, -0.35])),
    ]);
    let meshed = mesh_building_with_damage(&grouped_wall(), &log, &backend).expect("meshes");
    let high = meshed
        .groups
        .iter()
        .find(|group| group.id == "high")
        .expect("the high group");
    let mut seen = false;
    for batch in &high.batches {
        for source in &batch.mesh.sources {
            if let FaceOrigin::Damage(index) = source.origin {
                assert_eq!(index, 1, "the second record, though the group's only cut");
                seen = true;
            }
        }
    }
    assert!(seen, "the second record cut the high group");
}

#[test]
fn only_touched_groups_get_a_collider() {
    let backend = ManifoldMesher::default();
    let log = DamageLog(vec![
        Damage::new(Geometry::cuboid([1.0, 1.0, 1.0]), "rubble")
            .placed(Pose::at([8.2, 1.5, -0.35])),
    ]);
    let meshed = mesh_building_with_damage(&grouped_wall(), &log, &backend).expect("meshes");
    let high = meshed
        .groups
        .iter()
        .find(|group| group.id == "high")
        .expect("the high group");
    let low = meshed
        .groups
        .iter()
        .find(|group| group.id == "low")
        .expect("the low group");
    assert!(
        high.collider.is_some(),
        "the hit group answers with its mesh"
    );
    assert!(
        low.collider.is_none(),
        "the untouched group keeps its proxies"
    );
    let plain = mesh_building(&grouped_wall(), &backend).expect("meshes");
    let plain_low = plain
        .groups
        .iter()
        .find(|group| group.id == "low")
        .expect("the low group");
    assert_eq!(plain_low.batches.len(), low.batches.len());
    for (left, right) in plain_low.batches.iter().zip(&low.batches) {
        assert_eq!(left.mesh, right.mesh);
    }
}

#[test]
fn a_hit_groups_proxies_give_way_to_its_mesh_collider() {
    let backend = ManifoldMesher::default();
    let log = DamageLog(vec![
        Damage::new(Geometry::cuboid([1.0, 1.0, 1.0]), "rubble")
            .placed(Pose::at([8.2, 1.5, -0.35])),
    ]);
    let meshed = mesh_building_with_damage(&grouped_wall(), &log, &backend).expect("meshes");
    let colliders = meshed.colliders();
    let placed: Vec<&str> = colliders
        .iter()
        .filter(|collider| collider.element == "shell")
        .map(|collider| collider.instance.as_str())
        .collect();
    assert_eq!(placed, ["a", "b"], "c's convex proxy gave way");
    let mesh_colliders: Vec<&str> = meshed.mesh_colliders().map(|(id, _)| id).collect();
    assert_eq!(mesh_colliders, ["high"]);

    let high = meshed
        .groups
        .iter()
        .find(|group| group.id == "high")
        .expect("the high group");
    let collider = high.collider.as_ref().expect("the hit group's collider");
    let batch_volume: f64 = high.batches.iter().map(|batch| volume(&batch.mesh)).sum();
    assert!(
        (collider_volume(collider) - batch_volume).abs() < 1e-9,
        "collider {} against batches {batch_volume}",
        collider_volume(collider)
    );
}

#[test]
fn overlapping_hits_are_both_recorded() {
    let backend = ManifoldMesher::default();
    let before = total_volume(&mesh_building(&wall(true), &backend).expect("meshes"));
    // The second box starts inside the first, so the union spans x 0.2 to 1.5,
    // clear of the doorway at x 1.5: 1.3 * 1.0 * 0.3 = 0.39 is removed, and
    // both records leave faces.
    let log = DamageLog(vec![
        square_hole(),
        Damage::new(Geometry::cuboid([1.0, 1.0, 1.0]), "rubble")
            .placed(Pose::at([0.5, 1.5, -0.35])),
    ]);
    let meshed = mesh_building_with_damage(&wall(true), &log, &backend).expect("meshes");
    let after = total_volume(&meshed);
    assert!(
        (before - after - 0.39).abs() < 1e-9,
        "removed {}",
        before - after
    );
    let mut seen = [false; 2];
    for batch in &meshed.groups[0].batches {
        for source in &batch.mesh.sources {
            if let FaceOrigin::Damage(index) = source.origin {
                seen[index as usize] = true;
            }
        }
    }
    assert_eq!(seen, [true, true], "both records left damage faces");
}

#[test]
fn a_miss_changes_nothing_but_the_log() {
    let backend = ManifoldMesher::default();
    let plain = mesh_building(&wall(true), &backend).expect("meshes");
    let log = DamageLog(vec![
        Damage::new(Geometry::cuboid([1.0, 1.0, 1.0]), "rubble").placed(Pose::at([10.0, 1.5, 2.0])),
    ]);
    let meshed = mesh_building_with_damage(&wall(true), &log, &backend).expect("meshes");
    assert_eq!(plain.groups.len(), meshed.groups.len());
    for (before, after) in plain.groups.iter().zip(&meshed.groups) {
        assert!(after.collider.is_none());
        assert_eq!(before.batches.len(), after.batches.len());
        for (left, right) in before.batches.iter().zip(&after.batches) {
            assert_eq!(left.mesh, right.mesh);
        }
    }
    assert_eq!(
        meshed.damage.len(),
        1,
        "the log is kept though nothing moved"
    );
}

#[test]
fn damage_on_an_unmerged_building_is_refused() {
    let backend = ManifoldMesher::default();
    let log = DamageLog(vec![square_hole()]);
    let error = mesh_building_with_damage(&wall(false), &log, &backend).expect_err("refused");
    assert!(error.reason.contains("merged"), "{}", error.reason);
}

#[test]
fn an_unbound_damage_slot_is_refused_by_path() {
    let backend = ManifoldMesher::default();
    let log = DamageLog(vec![Damage::new(
        Geometry::cuboid([1.0, 1.0, 1.0]),
        "missing",
    )]);
    let error = mesh_building_with_damage(&wall(true), &log, &backend).expect_err("refused");
    assert_eq!(error.path, "damage[0].slot", "{}", error.path);
}

#[test]
fn a_ball_blast_leaves_a_closed_surface() {
    let backend = ManifoldMesher::default();
    let before = total_volume(&mesh_building(&wall(true), &backend).expect("meshes"));
    let log = DamageLog(vec![Damage::blast([6.0, 1.5, 0.15], 1.0, "rubble")]);
    let meshed = mesh_building_with_damage(&wall(true), &log, &backend).expect("meshes");
    let after = total_volume(&meshed);
    assert!(after > 0.0, "the wall keeps a volume, got {after}");
    assert!(
        after < before,
        "the blast removed something: {after} < {before}"
    );
    for group in &meshed.groups {
        for batch in &group.batches {
            assert!(batch.mesh.is_consistent(), "{:?}", group.id);
        }
    }
    for elements in meshed.parts.values() {
        for element in elements {
            assert!(element.mesh.is_consistent(), "{}", element.id);
        }
    }
}
