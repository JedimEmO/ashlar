//! The corporate tower and block unioned by storey: the grouping the research
//! spike measured, the materials it keeps, and what the merge costs.
use std::collections::BTreeSet;
use std::time::Instant;

use ashlar::{Damage, DamageLog};
use ashlar_manifold::{GroupSolids, ManifoldMesher, mesh_building};
use ashlar_showcase::corporate;
use glam::DVec3;

/// The tower every test here merges: four upper storeys over the podium.
fn tower() -> ashlar::Building {
    corporate::building(4).expect("tower")
}

#[test]
fn the_tower_merges_into_storey_groups() {
    // The prescribed band of a pose at the top of the top storey is one above
    // the storey itself, so the roof cap is its own group: the podium, the four
    // upper storeys and the roof are six bands, not five.
    let merged = corporate::merged_by_storey(tower()).expect("merged tower");
    let meshed = mesh_building(&merged, &ManifoldMesher::default()).expect("meshed");
    assert_eq!(meshed.groups.len(), 6, "one group per storey band");
    let storeys: BTreeSet<i32> = meshed
        .groups
        .iter()
        .filter_map(|group| group.storey)
        .collect();
    assert_eq!(
        storeys,
        BTreeSet::from([0, 1, 2, 3, 4, 5]),
        "storeys 0 to 5"
    );
    for group in &meshed.groups {
        assert!(group.storey.is_some(), "{} has no storey", group.id);
        assert!(
            !group.batches.is_empty(),
            "{} is empty of batches",
            group.id
        );
    }
}

#[test]
fn glazing_stays_standalone() {
    let merged = corporate::merged_by_storey(tower()).expect("merged tower");
    let meshed = mesh_building(&merged, &ManifoldMesher::default()).expect("meshed");
    let banned = ["metro:glass", "metro:light", "metro:signal"];
    for group in &meshed.groups {
        for batch in &group.batches {
            assert!(
                !banned.contains(&batch.binding.material.as_str()),
                "{} wears {}",
                group.id,
                batch.binding.material
            );
        }
    }
    assert!(
        meshed
            .parts
            .values()
            .flatten()
            .any(|element| element.material_slot == "glass"),
        "at least one standalone piece is glazing"
    );
}

#[test]
fn merging_keeps_the_towers_material_coverage() {
    let unmerged_building = tower();
    let unmerged = mesh_building(&unmerged_building, &ManifoldMesher::default()).expect("meshed");
    let expected: BTreeSet<String> = unmerged
        .pieces()
        .map(|piece| piece.binding.material.clone())
        .collect();

    let merged_building = corporate::merged_by_storey(tower()).expect("merged tower");
    let merged = mesh_building(&merged_building, &ManifoldMesher::default()).expect("meshed");
    let mut actual: BTreeSet<String> = BTreeSet::new();
    for group in &merged.groups {
        for batch in &group.batches {
            actual.insert(batch.binding.material.clone());
        }
    }
    // A standalone piece is drawn from `parts`, so its material is the one its
    // placing instance resolves: the same lookup the unmerged path makes.
    for instance in &merged_building.recipe().instances {
        let Some(elements) = merged.parts.get(&instance.part) else {
            continue;
        };
        for element in elements {
            let binding = merged_building
                .binding(&instance.id, &element.material_slot)
                .expect("validated binding");
            actual.insert(binding.material.clone());
        }
    }
    assert_eq!(actual, expected);
}

#[test]
fn colliders_are_the_unmerged_towers() {
    let unmerged = mesh_building(&tower(), &ManifoldMesher::default()).expect("meshed");
    let merged_building = corporate::merged_by_storey(tower()).expect("merged tower");
    let merged = mesh_building(&merged_building, &ManifoldMesher::default()).expect("meshed");
    assert_eq!(
        merged.colliders().len(),
        unmerged.colliders().len(),
        "merging changes what is drawn, not what is walked into"
    );
}

#[test]
fn every_group_face_has_an_operand() {
    let merged = corporate::merged_by_storey(tower()).expect("merged tower");
    let meshed = mesh_building(&merged, &ManifoldMesher::default()).expect("meshed");
    for group in &meshed.groups {
        let operands = u32::try_from(group.operands.len()).expect("operand count fits");
        for batch in &group.batches {
            for source in &batch.mesh.sources {
                assert!(
                    source.operand < operands,
                    "{} names operand {} of {operands}",
                    group.id,
                    source.operand
                );
            }
        }
    }
}

/// Every corner of an axis-aligned face must wear that face's normal: a wall
/// that meets a 45 degree chamfer is creased there, not smoothed by rounding.
fn flat_axis_faces(mesh: &ashlar::TriangleMesh) {
    for (face, corners) in mesh.corners().enumerate() {
        let points = corners.map(|index| mesh.positions[index]);
        let normal = (points[1] - points[0])
            .cross(points[2] - points[0])
            .normalize();
        if normal.abs().max_element() <= 0.999_999 {
            continue;
        }
        for corner in corners {
            let dot = mesh.normals[corner].dot(normal);
            assert!(
                dot > 0.9999,
                "face {face} corner {corner} leans {dot} off {normal}"
            );
        }
    }
}

#[test]
fn flat_faces_of_the_merged_tower_have_flat_normals() {
    let merged = corporate::merged_by_storey(tower()).expect("merged tower");
    let meshed = mesh_building(&merged, &ManifoldMesher::default()).expect("meshed");
    for group in &meshed.groups {
        for batch in &group.batches {
            flat_axis_faces(&batch.mesh);
        }
    }
    let unmerged = mesh_building(&tower(), &ManifoldMesher::default()).expect("meshed");
    for element in unmerged.parts.values().flatten() {
        flat_axis_faces(&element.mesh);
    }
}

/// The minimum of three runs of `run`, in milliseconds.
fn best_of(mut run: impl FnMut()) -> f64 {
    let mut best = f64::INFINITY;
    for _ in 0..3 {
        let started = std::time::Instant::now();
        run();
        best = best.min(started.elapsed().as_secs_f64() * 1000.0);
    }
    best
}

#[test]
#[ignore = "timing; run with --ignored --nocapture"]
fn merge_timing() {
    let mesher = ManifoldMesher::default();
    for (name, unmerged) in [
        ("tower", corporate::building(4).expect("tower")),
        ("block", corporate::block().expect("block")),
    ] {
        let merged_building = corporate::merged_by_storey(unmerged.clone()).expect("merged");
        let unmerged_ms = best_of(|| {
            mesh_building(&unmerged, &mesher).expect("meshed");
        });
        let merged_ms = best_of(|| {
            mesh_building(&merged_building, &mesher).expect("meshed");
        });
        let unmerged_meshed = mesh_building(&unmerged, &mesher).expect("meshed");
        let merged_meshed = mesh_building(&merged_building, &mesher).expect("meshed");

        let placed: usize = unmerged_meshed
            .pieces()
            .map(|piece| piece.mesh.triangle_count())
            .sum();
        println!("{name}: mesh_building unmerged {unmerged_ms:.1} ms, merged {merged_ms:.1} ms");
        println!("  {:<28} {:>8} {:>10}", "group", "operands", "triangles");
        let mut merged_total = 0;
        for group in &merged_meshed.groups {
            let triangles: usize = group
                .batches
                .iter()
                .map(|batch| batch.mesh.triangle_count())
                .sum();
            merged_total += triangles;
            println!(
                "  {:<28} {:>8} {:>10}",
                group.id,
                group.operands.len(),
                triangles
            );
        }
        println!("  {:<28} {:>8} {:>10}", "merged total", "", merged_total);
        println!("  {name}: {merged_total} merged triangles vs {placed} unmerged placed");
    }
}

/// What a hit on a kept storey group costs, and what it leaves.
///
/// The tower is unioned by storey; the group with the most triangles is the
/// largest chunk a hit can re-mesh, so it is the worst case this path has to
/// answer. The blast is placed on the group's own vertex nearest the middle of
/// its lowest-Z face, so the bite is real rather than a tangent miss.
#[test]
#[ignore = "timing; run with --ignored --nocapture"]
fn hit_timing() {
    let mesher = ManifoldMesher::default();
    let merged = corporate::merged_by_storey(tower()).expect("merged tower");
    assert!(
        merged.recipe().materials.contains_key("stone"),
        "the tower's palette binds the blast's slot"
    );

    // One build only to choose the target and the bite points; the fresh builds
    // below answer the same geometry and are what the timing runs on.
    let (_, first) = GroupSolids::build(&merged, &DamageLog::default(), mesher).expect("solids");
    let (target_id, centre, warm_centre) = {
        let (_, group) = first
            .groups
            .iter()
            .enumerate()
            .max_by_key(|(_, group)| {
                group
                    .batches
                    .iter()
                    .map(|batch| batch.mesh.triangle_count())
                    .sum::<usize>()
            })
            .expect("a group");
        let bounds = group.bounds.expect("the group has bounds");
        let nearest_to = |target: DVec3| {
            group
                .batches
                .iter()
                .flat_map(|batch| &batch.mesh.positions)
                .copied()
                .min_by(|a, b| {
                    a.distance_squared(target)
                        .total_cmp(&b.distance_squared(target))
                })
                .expect("the group has vertices")
        };
        let centre = nearest_to(DVec3::new(
            f64::midpoint(bounds[0].x, bounds[1].x),
            f64::midpoint(bounds[0].y, bounds[1].y),
            bounds[0].z,
        ));
        // A second bite on the opposite face, clear of the first, to be the
        // untimed hit that settles the contacts the timed hit then finds.
        let warm_centre = nearest_to(DVec3::new(
            f64::midpoint(bounds[0].x, bounds[1].x),
            f64::midpoint(bounds[0].y, bounds[1].y),
            bounds[1].z,
        ));
        (group.id.clone(), centre, warm_centre)
    };
    let damage = Damage::blast(centre.to_array(), 1.5, "stone").collapsing();
    let warmup = Damage::blast(warm_centre.to_array(), 1.5, "stone").collapsing();

    let mut cold_best = f64::INFINITY;
    let mut warm_best = f64::INFINITY;
    let mut debris_pieces = 0;
    let mut debris_volume = 0.0;
    let mut triangles_before = 0;
    let mut triangles_after = 0;
    for _ in 0..5 {
        // The old-style cold number: the hit is the first thing a fresh build
        // does, so every contact is asked cold.
        let (mut solids, answer) =
            GroupSolids::build(&merged, &DamageLog::default(), mesher).expect("solids");
        let group = answer
            .groups
            .iter()
            .find(|group| group.id == target_id)
            .expect("the target group");
        triangles_before = group
            .batches
            .iter()
            .map(|batch| batch.mesh.triangle_count())
            .sum();

        let started = Instant::now();
        let hits = solids.apply(&damage).expect("a hit");
        cold_best = cold_best.min(started.elapsed().as_secs_f64() * 1000.0);

        debris_pieces = 0;
        debris_volume = 0.0;
        triangles_after = triangles_before;
        for hit in &hits {
            if hit.group.id != target_id {
                continue;
            }
            triangles_after = hit
                .group
                .batches
                .iter()
                .map(|batch| batch.mesh.triangle_count())
                .sum();
            for debris in &hit.debris {
                debris_pieces += 1;
                debris_volume += debris.volume;
            }
        }

        // The warm number: one untimed collapsing hit elsewhere on the same
        // group settles the contacts between the groups the timed hit does not
        // change, so the timed hit finds those answers cached.
        let (mut solids, _) =
            GroupSolids::build(&merged, &DamageLog::default(), mesher).expect("solids");
        solids.apply(&warmup).expect("a warm-up hit");
        let started = Instant::now();
        solids.apply(&damage).expect("a hit");
        warm_best = warm_best.min(started.elapsed().as_secs_f64() * 1000.0);
    }
    println!(
        "hit_timing: cold {cold_best:.1} ms, warm {warm_best:.1} ms, \
         debris {debris_pieces} pieces {debris_volume:.6} m^3"
    );
    println!("  group {target_id}: {triangles_before} triangles before, {triangles_after} after");
}
