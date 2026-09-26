//! Kept group solids: `GroupSolids` answers what the stateless replay does,
//! subtracts a hit from what is already unioned, and reports the debris.

use std::collections::{BTreeMap, BTreeSet};

use ashlar::{
    Building, Collision, Damage, DamageLog, Element, FaceOrigin, FaceSource, Geometry, Instance,
    MergeGroup, MergedGroup, Part, Pose, TriangleCollider, TriangleMesh, UvMode,
};
use ashlar_manifold::{Debris, DebrisKind, GroupSolids, ManifoldMesher, mesh_building_with_damage};
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

fn group_volume(group: &MergedGroup) -> f64 {
    group.batches.iter().map(|batch| volume(&batch.mesh)).sum()
}

fn group_area(group: &MergedGroup) -> f64 {
    group.batches.iter().map(|batch| area(&batch.mesh)).sum()
}

/// Group id and face source to summed face area over every batch of a meshed
/// building.
fn histogram(meshed: &ashlar::MeshedBuilding) -> BTreeMap<(String, FaceSource), f64> {
    let mut result = BTreeMap::new();
    for group in &meshed.groups {
        for batch in &group.batches {
            for (face, triangle) in batch.mesh.triangles().enumerate() {
                *result
                    .entry((group.id.clone(), batch.mesh.sources[face]))
                    .or_insert(0.0) += triangle_area(triangle);
            }
        }
    }
    result
}

/// Compare two histograms by tolerance rather than by key: a key present on one
/// side only is compared against zero.
fn assert_histograms_close(
    left: &BTreeMap<(String, FaceSource), f64>,
    right: &BTreeMap<(String, FaceSource), f64>,
) {
    let keys: BTreeSet<&(String, FaceSource)> = left.keys().chain(right.keys()).collect();
    for key in keys {
        let a = left.get(key).copied().unwrap_or(0.0);
        let b = right.get(key).copied().unwrap_or(0.0);
        assert!((a - b).abs() < 1e-9, "{key:?}: {a} vs {b}");
    }
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

/// A ball centred on the joint between bays `b` and `c` at x = 8.
fn ball_on_joint() -> Damage {
    Damage::blast([8.0, 1.5, 0.15], 1.0, "rubble")
}

/// A box that starts inside `square_hole`'s and reaches further right, so the
/// two removals overlap.
fn overlapping_box() -> Damage {
    Damage::new(Geometry::cuboid([1.0, 1.0, 1.0]), "rubble").placed(Pose::at([0.5, 1.5, -0.35]))
}

/// One plain 6 x 3 x 0.3 wall in one merged group, binding a damage slot.
fn plain_wall() -> Building {
    let part = Part::builder("slab")
        .element(Element::new(
            "shell",
            Geometry::cuboid([6.0, 3.0, 0.3]),
            "surface",
        ))
        .build()
        .expect("valid slab");
    Building::builder("wall")
        .part(part)
        .material("surface", "paint")
        .material("rubble", "metro:rubble")
        .merged()
        .instance(Instance::new("a", "slab"))
        .build()
        .expect("valid wall")
}

/// Two plain 4 x 3 x 0.3 bays in one merged group, the second wearing brick.
fn two_bay_wall() -> Building {
    let part = Part::builder("slab")
        .element(Element::new(
            "shell",
            Geometry::cuboid([4.0, 3.0, 0.3]),
            "surface",
        ))
        .build()
        .expect("valid slab");
    Building::builder("wall")
        .part(part)
        .material("surface", "paint")
        .material("rubble", "metro:rubble")
        .merged()
        .instance(Instance::new("a", "slab"))
        .instance(
            Instance::new("b", "slab")
                .placed(Pose::at([4.0, 0.0, 0.0]))
                .material("surface", "brick"),
        )
        .build()
        .expect("valid wall")
}

/// A frame cut through the whole wall that isolates the block x 2..4, y 1..2,
/// which reaches nothing, because the frame passes right through the 0.3
/// thickness on every side.
fn isolating_frame() -> Damage {
    Damage::new(
        Geometry::cuboid([3.0, 2.0, 1.0])
            .placed(Pose::at([1.5, 0.5, -0.35]))
            .subtract(Geometry::cuboid([2.0, 1.0, 2.0]).placed(Pose::at([2.0, 1.0, -0.85]))),
        "rubble",
    )
}

/// The same frame inside one 4 m bay, isolating x 1..3, y 1..2.
fn island_in_bay() -> Damage {
    Damage::new(
        Geometry::cuboid([3.0, 2.0, 1.0])
            .placed(Pose::at([0.5, 0.5, -0.35]))
            .subtract(Geometry::cuboid([2.0, 1.0, 2.0]).placed(Pose::at([1.0, 1.0, -0.85]))),
        "rubble",
    )
}

#[test]
fn build_answers_what_the_stateless_path_answers() {
    let mesher = ManifoldMesher::default();
    for log in [
        DamageLog::default(),
        DamageLog(vec![square_hole(), ball_on_joint()]),
    ] {
        let building = wall(true);
        let (_solids, kept) = GroupSolids::build(&building, &log, mesher).expect("build");
        let stateless = mesh_building_with_damage(&building, &log, &mesher).expect("stateless");
        let kept_ids: Vec<&str> = kept.groups.iter().map(|group| group.id.as_str()).collect();
        let replay_ids: Vec<&str> = stateless
            .groups
            .iter()
            .map(|group| group.id.as_str())
            .collect();
        assert_eq!(kept_ids, replay_ids);
        for (left, right) in kept.groups.iter().zip(&stateless.groups) {
            assert!(
                (group_volume(left) - group_volume(right)).abs() < 1e-9,
                "{} volume",
                left.id
            );
            assert!(
                (group_area(left) - group_area(right)).abs() < 1e-9,
                "{} area",
                left.id
            );
        }
        assert_histograms_close(&histogram(&kept), &histogram(&stateless));
    }
}

#[test]
fn hits_applied_one_by_one_equal_the_log_replayed() {
    let mesher = ManifoldMesher::default();
    let building = wall(true);
    let (mut solids, mut kept) =
        GroupSolids::build(&building, &DamageLog::default(), mesher).expect("build");
    let records = [square_hole(), ball_on_joint(), overlapping_box()];
    for record in &records {
        for hit in solids.apply(record).expect("apply") {
            assert!(kept.replace_group(hit.group), "the group exists");
        }
        kept.record(record.clone());
    }
    let replay = mesh_building_with_damage(&building, solids.log(), &mesher).expect("replay");
    assert_eq!(kept.groups.len(), replay.groups.len());
    assert_histograms_close(&histogram(&kept), &histogram(&replay));
    for (left, right) in kept.groups.iter().zip(&replay.groups) {
        assert_eq!(left.id, right.id);
        assert!(
            (group_volume(left) - group_volume(right)).abs() < 1e-9,
            "{} volume",
            left.id
        );
        assert!(
            (group_area(left) - group_area(right)).abs() < 1e-9,
            "{} area",
            left.id
        );
        match (left.bounds, right.bounds) {
            (Some(left), Some(right)) => {
                for axis in 0..3 {
                    assert!(
                        (left[0][axis] - right[0][axis]).abs() < 1e-9,
                        "{} min",
                        left[0]
                    );
                    assert!(
                        (left[1][axis] - right[1][axis]).abs() < 1e-9,
                        "{} max",
                        left[1]
                    );
                }
            }
            (None, None) => {}
            other => panic!("{} bounds differ: {other:?}", left.id),
        }
    }
    // Triangle counts are not compared: triangulation is not part of the
    // contract, because keeping a solid and re-unioning one may cut the same
    // surface into different triangles.
}

#[test]
fn debris_plus_what_is_left_is_what_there_was() {
    let mesher = ManifoldMesher::default();
    let building = wall(true);
    let (mut solids, mut kept) =
        GroupSolids::build(&building, &DamageLog::default(), mesher).expect("build");
    let before: f64 = kept.groups.iter().map(group_volume).sum();
    let hits = solids.apply(&square_hole()).expect("apply");
    let mut removed = 0.0;
    for hit in hits {
        for debris in &hit.debris {
            removed += debris.volume;
            let batch_volume: f64 = debris.batches.iter().map(|batch| volume(&batch.mesh)).sum();
            assert!(
                (batch_volume - debris.volume).abs() < 1e-9,
                "piece batches {batch_volume} against volume {}",
                debris.volume
            );
        }
        assert!(kept.replace_group(hit.group));
    }
    let after: f64 = kept.groups.iter().map(group_volume).sum();
    assert!(
        (before - after - removed).abs() < 1e-9,
        "removed {removed}, but volume fell from {before} to {after}"
    );
}

#[test]
fn debris_keeps_its_materials() {
    let mesher = ManifoldMesher::default();
    let (mut solids, _kept) =
        GroupSolids::build(&wall(true), &DamageLog::default(), mesher).expect("build");
    // The box starts outside the front face (z below 0) and ends inside the
    // wall, inside bay `b`, so the removed chunk is one piece with both the
    // wall's own front face and the blast's exposed faces on it.
    let hit = Damage::new(Geometry::cuboid([1.0, 1.0, 0.3]), "rubble")
        .placed(Pose::at([4.2, 1.5, -0.05]));
    let hits = solids.apply(&hit).expect("apply");
    assert_eq!(hits.len(), 1);
    let debris = &hits[0].debris;
    assert_eq!(debris.len(), 1, "one connected piece");
    let piece = &debris[0];
    let brick = piece
        .batches
        .iter()
        .find(|batch| batch.binding.material == "brick")
        .expect("the wall's brick face");
    for source in &brick.mesh.sources {
        assert_eq!(source.operand, 1, "bay b is the middle operand");
        assert_eq!(source.origin, FaceOrigin::Body);
    }
    let rubble = piece
        .batches
        .iter()
        .find(|batch| batch.binding.material == "metro:rubble")
        .expect("the blast's faces");
    for source in &rubble.mesh.sources {
        assert_eq!(source.operand, FaceSource::NO_OPERAND);
        assert_eq!(source.origin, FaceOrigin::Damage(0));
    }
    assert_eq!(piece.batches.len(), 2, "brick and rubble, nothing else");
}

#[test]
fn a_hit_across_a_doorway_makes_two_pieces_of_debris() {
    let mesher = ManifoldMesher::default();
    let (mut solids, _kept) =
        GroupSolids::build(&wall(true), &DamageLog::default(), mesher).expect("build");
    // x 1.0 to 3.0 crosses the opening at 1.5 to 2.5, so the wall's material
    // either side is removed and the opening's void splits it in two.
    let hit = Damage::new(Geometry::cuboid([2.0, 0.5, 1.0]), "rubble")
        .placed(Pose::at([1.0, 0.5, -0.35]));
    let hits = solids.apply(&hit).expect("apply");
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].debris.len(), 2);
}

#[test]
fn a_hit_in_one_group_leaves_the_others_alone() {
    let mesher = ManifoldMesher::default();
    let (mut solids, _kept) =
        GroupSolids::build(&grouped_wall(), &DamageLog::default(), mesher).expect("build");
    let hit = Damage::new(Geometry::cuboid([1.0, 1.0, 1.0]), "rubble")
        .placed(Pose::at([8.2, 1.5, -0.35]));
    let hits = solids.apply(&hit).expect("apply");
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].group.id, "high");
}

#[test]
fn a_miss_is_recorded_and_changes_nothing() {
    let mesher = ManifoldMesher::default();
    let (mut solids, _kept) =
        GroupSolids::build(&wall(true), &DamageLog::default(), mesher).expect("build");
    let miss =
        Damage::new(Geometry::cuboid([1.0, 1.0, 1.0]), "rubble").placed(Pose::at([10.0, 1.5, 2.0]));
    assert!(solids.apply(&miss).expect("apply").is_empty());
    assert_eq!(solids.log().len(), 1);

    let hits = solids.apply(&square_hole()).expect("apply");
    assert_eq!(hits.len(), 1);
    let mut seen = false;
    for batch in &hits[0].group.batches {
        for source in &batch.mesh.sources {
            if let FaceOrigin::Damage(index) = source.origin {
                assert_eq!(index, 1, "the second record, after the miss was logged");
                seen = true;
            }
        }
    }
    assert!(seen, "the real hit left damage faces");
}

#[test]
fn a_hit_group_has_a_collider_with_the_groups_volume() {
    let mesher = ManifoldMesher::default();
    let (mut solids, _kept) =
        GroupSolids::build(&wall(true), &DamageLog::default(), mesher).expect("build");
    let hits = solids.apply(&square_hole()).expect("apply");
    assert_eq!(hits.len(), 1);
    let group = &hits[0].group;
    let collider = group.collider.as_ref().expect("the hit group's collider");
    assert!(
        (collider_volume(collider) - group_volume(group)).abs() < 1e-9,
        "collider {} against batches {}",
        collider_volume(collider),
        group_volume(group)
    );
}

#[test]
fn an_invalid_record_is_refused_and_not_logged() {
    let mesher = ManifoldMesher::default();
    let (mut solids, _kept) =
        GroupSolids::build(&wall(true), &DamageLog::default(), mesher).expect("build");
    let bad = Damage::new(Geometry::cuboid([1.0, 1.0, 1.0]), "missing");
    let error = solids.apply(&bad).expect_err("an unbound slot is refused");
    assert_eq!(error.path, "damage[0].slot");
    assert_eq!(solids.log().len(), 0, "a refusal is not logged");
}

#[test]
fn a_failed_hit_changes_nothing() {
    // A budget that clears the undamaged wall's 76 kernel triangles and the
    // ball cutter's own 224, but not the 240 the subtraction leaves. The ball
    // then fails at the check after the subtraction, which is where a hit that
    // were not atomic would already have overwritten the solid and joined the
    // log.
    let mesher = ManifoldMesher {
        max_triangles: 230,
        ..ManifoldMesher::default()
    };
    let (mut solids, _kept) =
        GroupSolids::build(&wall(true), &DamageLog::default(), mesher).expect("build");
    let error = solids
        .apply(&ball_on_joint())
        .expect_err("the ball's crater exceeds the budget");
    assert_eq!(error.path, "damage[0]");
    assert_eq!(solids.log().len(), 0, "a failed hit is not logged");

    let hits = solids
        .apply(&square_hole())
        .expect("the small box stays under the budget");
    assert_eq!(solids.log().len(), 1);
    assert_eq!(hits.len(), 1);
    let mut seen = false;
    for batch in &hits[0].group.batches {
        for source in &batch.mesh.sources {
            if let FaceOrigin::Damage(index) = source.origin {
                assert_eq!(index, 0, "the hit is the first record in the log");
                seen = true;
            }
        }
    }
    assert!(seen, "the small hit left damage faces");
}

#[test]
fn group_solids_is_send() {
    fn assert_send<T: Send>() {}
    assert_send::<GroupSolids>();
}

#[test]
fn a_piece_cut_free_falls_when_the_hit_collapses() {
    let mesher = ManifoldMesher::default();

    // Without collapsing the island stays: 5.4 - (3*2 - 2*1)*0.3.
    let (mut solids, _kept) =
        GroupSolids::build(&plain_wall(), &DamageLog::default(), mesher).expect("build");
    let hits = solids.apply(&isolating_frame()).expect("apply");
    assert_eq!(hits.len(), 1);
    let volume = group_volume(&hits[0].group);
    assert!((volume - 4.2).abs() < 1e-9, "volume {volume}");

    // With collapsing the island goes too: 4.2 - 2*1*0.3.
    let (mut solids, _kept) =
        GroupSolids::build(&plain_wall(), &DamageLog::default(), mesher).expect("build");
    let hits = solids
        .apply(&isolating_frame().collapsing())
        .expect("apply");
    assert_eq!(hits.len(), 1);
    let volume = group_volume(&hits[0].group);
    assert!((volume - 3.6).abs() < 1e-9, "volume {volume}");

    let fallen: Vec<&Debris> = hits[0]
        .debris
        .iter()
        .filter(|piece| piece.kind == DebrisKind::Fallen)
        .collect();
    assert_eq!(fallen.len(), 1, "the island is one piece");
    assert!(
        (fallen[0].volume - 0.6).abs() < 1e-9,
        "volume {}",
        fallen[0].volume
    );
    for batch in &fallen[0].batches {
        for source in &batch.mesh.sources {
            assert!(
                source.operand == 0 || matches!(source.origin, FaceOrigin::Damage(_)),
                "unexpected falling source {source:?}"
            );
        }
    }
    assert!(
        hits[0]
            .debris
            .iter()
            .any(|piece| piece.kind == DebrisKind::Blasted),
        "the frame is debris too"
    );
}

#[test]
fn a_stub_standing_on_the_base_stays() {
    let mesher = ManifoldMesher::default();
    let (mut solids, _kept) =
        GroupSolids::build(&plain_wall(), &DamageLog::default(), mesher).expect("build");
    let slot = |x: f64| {
        Damage::new(Geometry::cuboid([0.5, 3.0, 1.0]), "rubble")
            .placed(Pose::at([x, 0.0, -0.35]))
            .collapsing()
    };
    let mut last = None;
    for damage in [slot(2.0), slot(3.5)] {
        let hits = solids.apply(&damage).expect("apply");
        assert_eq!(hits.len(), 1);
        last = hits.into_iter().next();
    }
    let hit = last.expect("both cuts applied");
    let volume = group_volume(&hit.group);
    assert!((volume - 4.5).abs() < 1e-9, "volume {volume}");
    assert!(
        hit.debris
            .iter()
            .all(|piece| piece.kind == DebrisKind::Blasted),
        "the stub on the base stayed"
    );
}

#[test]
fn sources_survive_a_collapse() {
    let mesher = ManifoldMesher::default();
    let (mut solids, _kept) =
        GroupSolids::build(&two_bay_wall(), &DamageLog::default(), mesher).expect("build");
    let hits = solids.apply(&island_in_bay().collapsing()).expect("apply");
    assert_eq!(hits.len(), 1);
    let brick = hits[0]
        .group
        .batches
        .iter()
        .find(|batch| batch.binding.material == "brick")
        .expect("bay b wears brick");
    assert!(!brick.mesh.sources.is_empty(), "bay b still has faces");
    for source in &brick.mesh.sources {
        assert_eq!(source.operand, 1, "bay b is the second operand");
        assert_eq!(source.origin, FaceOrigin::Body);
    }
}

#[test]
fn a_collapsing_log_replays_to_the_same_building() {
    let mesher = ManifoldMesher::default();
    let building = plain_wall();
    let (mut solids, mut kept) =
        GroupSolids::build(&building, &DamageLog::default(), mesher).expect("build");
    let records = [isolating_frame().collapsing(), island_in_bay().collapsing()];
    let mut dropped = false;
    for record in &records {
        for hit in solids.apply(record).expect("apply") {
            dropped |= hit
                .debris
                .iter()
                .any(|piece| piece.kind == DebrisKind::Fallen);
            assert!(kept.replace_group(hit.group), "the group exists");
        }
        kept.record(record.clone());
    }
    assert!(dropped, "at least one record cut a piece free");
    let replay = mesh_building_with_damage(&building, solids.log(), &mesher).expect("replay");
    assert_eq!(kept.groups.len(), replay.groups.len());
    assert_histograms_close(&histogram(&kept), &histogram(&replay));
    for (left, right) in kept.groups.iter().zip(&replay.groups) {
        assert_eq!(left.id, right.id);
        assert!(
            (group_volume(left) - group_volume(right)).abs() < 1e-9,
            "{} volume",
            left.id
        );
        assert!(
            (group_area(left) - group_area(right)).abs() < 1e-9,
            "{} area",
            left.id
        );
    }
}

/// One plain 6 x 3 x 0.3 wall with a 1 x 0.5 x 0.05 sign stood 0.2 m off its
/// front face, touching nothing and not on the base, in one merged group.
fn signed_wall() -> Building {
    let part = Part::builder("slab")
        .element(Element::new(
            "shell",
            Geometry::cuboid([6.0, 3.0, 0.3]),
            "surface",
        ))
        .element(Element::new(
            "sign",
            Geometry::cuboid([1.0, 0.5, 0.05]).placed(Pose::at([4.5, 2.0, 0.5])),
            "surface",
        ))
        .build()
        .expect("valid slab");
    Building::builder("wall")
        .part(part)
        .material("surface", "paint")
        .material("rubble", "metro:rubble")
        .merged()
        .instance(Instance::new("a", "slab"))
        .build()
        .expect("valid wall")
}

/// A box that opens a square hole through the wall at x 0.5 to 1.5, y 1 to 2,
/// clear of the sign.
fn wall_hole() -> Damage {
    Damage::new(Geometry::cuboid([1.0, 1.0, 1.0]), "rubble").placed(Pose::at([0.5, 1.0, -0.35]))
}

/// A box that overlaps the sign's left half and clears the wall. The sign
/// spans x 4.5 to 5.5, so this takes x 4.5 to 5.0.
fn sign_half() -> Damage {
    Damage::new(Geometry::cuboid([0.5, 0.5, 0.2]), "rubble").placed(Pose::at([4.5, 2.0, 0.4]))
}

#[test]
fn a_loose_piece_the_hit_never_touched_stays() {
    let mesher = ManifoldMesher::default();
    let (mut solids, _kept) =
        GroupSolids::build(&signed_wall(), &DamageLog::default(), mesher).expect("build");
    let hits = solids.apply(&wall_hole().collapsing()).expect("apply");
    assert_eq!(hits.len(), 1);
    let volume = group_volume(&hits[0].group);
    assert!(
        (volume - (5.4 + 0.025 - 0.3)).abs() < 1e-9,
        "volume {volume}"
    );
    assert!(
        hits[0]
            .debris
            .iter()
            .all(|piece| piece.kind != DebrisKind::Fallen),
        "the sign the hit never touched stayed"
    );
}

#[test]
fn a_loose_piece_the_hit_bit_into_falls() {
    let mesher = ManifoldMesher::default();
    let (mut solids, _kept) =
        GroupSolids::build(&signed_wall(), &DamageLog::default(), mesher).expect("build");
    let hits = solids.apply(&sign_half().collapsing()).expect("apply");
    assert_eq!(hits.len(), 1);
    let volume = group_volume(&hits[0].group);
    assert!((volume - 5.4).abs() < 1e-9, "volume {volume}");
    let fallen: f64 = hits[0]
        .debris
        .iter()
        .filter(|piece| piece.kind == DebrisKind::Fallen)
        .map(|piece| piece.volume)
        .sum();
    assert!((fallen - 0.0125).abs() < 1e-9, "fallen {fallen}");
}

#[test]
fn a_collapsing_log_with_a_loose_piece_replays_to_the_same_building() {
    let mesher = ManifoldMesher::default();
    let building = signed_wall();
    let (mut solids, mut kept) =
        GroupSolids::build(&building, &DamageLog::default(), mesher).expect("build");
    let records = [wall_hole().collapsing(), sign_half().collapsing()];
    for record in &records {
        for hit in solids.apply(record).expect("apply") {
            assert!(kept.replace_group(hit.group), "the group exists");
        }
        kept.record(record.clone());
    }
    let replay = mesh_building_with_damage(&building, solids.log(), &mesher).expect("replay");
    assert_eq!(kept.groups.len(), replay.groups.len());
    for (left, right) in kept.groups.iter().zip(&replay.groups) {
        assert_eq!(left.id, right.id);
        assert!(
            (group_volume(left) - group_volume(right)).abs() < 1e-9,
            "{} volume",
            left.id
        );
        assert!(
            (group_area(left) - group_area(right)).abs() < 1e-9,
            "{} area",
            left.id
        );
        match (left.bounds, right.bounds) {
            (Some(min_max), Some(other_min_max)) => {
                for axis in 0..3 {
                    assert!(
                        (min_max[0][axis] - other_min_max[0][axis]).abs() < 1e-9,
                        "{} min",
                        left.id
                    );
                    assert!(
                        (min_max[1][axis] - other_min_max[1][axis]).abs() < 1e-9,
                        "{} max",
                        left.id
                    );
                }
            }
            (None, None) => {}
            other => panic!("{} bounds differ: {other:?}", left.id),
        }
    }
}

/// A merged building of three storeys, each one instance of a 4 x 3 x 4 block,
/// stacked exactly face to face at y = 0, 3, 6, each its own group.
fn stack() -> Building {
    stack_of(false)
}

/// The same stack, with a 1 x 0.5 x 0.05 sign stood 0.2 m off `g2`'s block. The
/// sign touches nothing and does not reach the ground.
fn stack_with_sign() -> Building {
    stack_of(true)
}

/// Build the stack, optionally giving every storey's part a sign. A plain part
/// is always present so both variants have the same shape.
fn stack_of(sign: bool) -> Building {
    let plain = Part::builder("plain")
        .element(
            Element::new("box", Geometry::cuboid([4.0, 3.0, 4.0]), "surface")
                .collision(Collision::Bounds),
        )
        .build()
        .expect("valid block");
    let mut signed = Part::builder("signed").element(
        Element::new("box", Geometry::cuboid([4.0, 3.0, 4.0]), "surface")
            .collision(Collision::Bounds),
    );
    if sign {
        signed = signed.element(Element::new(
            "sign",
            Geometry::cuboid([1.0, 0.5, 0.05]).placed(Pose::at([4.2, 1.5, 1.0])),
            "surface",
        ));
    }
    Building::builder("stack")
        .part(plain)
        .part(signed.build().expect("valid signed block"))
        .material("surface", "paint")
        .material("rubble", "metro:rubble")
        .merged()
        .group(MergeGroup::new("g0"))
        .group(MergeGroup::new("g1"))
        .group(MergeGroup::new("g2"))
        .instance(Instance::new("s0", "plain").group("g0"))
        .instance(
            Instance::new("s1", "plain")
                .group("g1")
                .placed(Pose::at([0.0, 3.0, 0.0])),
        )
        .instance(
            Instance::new("s2", if sign { "signed" } else { "plain" })
                .group("g2")
                .placed(Pose::at([0.0, 6.0, 0.0])),
        )
        .build()
        .expect("valid stack")
}

/// The cut the storey tests share: a box that removes all of `g1` and a 0.1 m
/// sliver of `g0`'s top and `g2`'s bottom.
fn storey_cut() -> Damage {
    Damage::new(Geometry::cuboid([6.0, 3.2, 6.0]), "rubble")
        .placed(Pose::at([-1.0, 2.9, -1.0]))
        .collapsing()
}

/// The number of triangles in a hit's re-meshed group.
fn group_triangles(group: &MergedGroup) -> usize {
    group
        .batches
        .iter()
        .map(|batch| batch.mesh.triangle_count())
        .sum()
}

#[test]
fn cutting_the_middle_out_drops_everything_above() {
    let mesher = ManifoldMesher::default();
    let (mut solids, _kept) =
        GroupSolids::build(&stack(), &DamageLog::default(), mesher).expect("build");
    let hits = solids.apply(&storey_cut()).expect("apply");
    let find = |id: &str| {
        hits.iter()
            .find(|hit| hit.group.id == id)
            .unwrap_or_else(|| panic!("a hit for {id}"))
    };
    // g0 keeps 48 - 0.1 * 4 * 4.
    assert!((group_volume(&find("g0").group) - 46.4).abs() < 1e-9);
    // g1 is removed entirely, and g2's remainder has nothing holding it up.
    assert_eq!(group_triangles(&find("g1").group), 0);
    let g2 = find("g2");
    assert_eq!(group_triangles(&g2.group), 0);
    let fallen: Vec<&Debris> = g2
        .debris
        .iter()
        .filter(|piece| piece.kind == DebrisKind::Fallen)
        .collect();
    assert_eq!(fallen.len(), 1, "the storey is one piece");
    assert!(
        (fallen[0].volume - 46.4).abs() < 1e-9,
        "volume {}",
        fallen[0].volume
    );
}

#[test]
fn the_same_cut_without_collapse_leaves_the_top_floating() {
    let mesher = ManifoldMesher::default();
    let (mut solids, _kept) =
        GroupSolids::build(&stack(), &DamageLog::default(), mesher).expect("build");
    let damage = Damage::new(Geometry::cuboid([6.0, 3.2, 6.0]), "rubble")
        .placed(Pose::at([-1.0, 2.9, -1.0]));
    let hits = solids.apply(&damage).expect("apply");
    let g2 = hits
        .iter()
        .find(|hit| hit.group.id == "g2")
        .expect("a g2 hit");
    assert!((group_volume(&g2.group) - 46.4).abs() < 1e-9);
    assert!(
        g2.debris
            .iter()
            .all(|piece| piece.kind != DebrisKind::Fallen),
        "nothing collapses without the flag"
    );
}

#[test]
fn a_column_is_enough_to_hold_the_top_up() {
    let mesher = ManifoldMesher::default();
    let (mut solids, _kept) =
        GroupSolids::build(&stack(), &DamageLog::default(), mesher).expect("build");
    // The same box, with a 0.5 m column left through its middle. In the box's
    // own frame the building's centre x = z = 2 is (3, 3), and the hole runs
    // past both ends in y so a full-height column of g1 survives.
    let solid = Geometry::cuboid([6.0, 3.2, 6.0])
        .subtract(Geometry::cuboid([0.5, 4.0, 0.5]).placed(Pose::at([2.75, -0.4, 2.75])));
    let damage = Damage::new(solid, "rubble")
        .placed(Pose::at([-1.0, 2.9, -1.0]))
        .collapsing();
    let hits = solids.apply(&damage).expect("apply");
    let g2 = hits
        .iter()
        .find(|hit| hit.group.id == "g2")
        .expect("a g2 hit");
    assert!(
        g2.debris
            .iter()
            .all(|piece| piece.kind != DebrisKind::Fallen),
        "the column held the top up"
    );
    // The column's sliver of g2's bottom is 0.1 * 0.5 * 0.5.
    assert!(
        (group_volume(&g2.group) - 46.425).abs() < 1e-9,
        "volume {}",
        group_volume(&g2.group)
    );
}

#[test]
fn a_piece_loose_by_design_survives_a_collapse_elsewhere() {
    let mesher = ManifoldMesher::default();
    let (mut solids, _kept) =
        GroupSolids::build(&stack_with_sign(), &DamageLog::default(), mesher).expect("build");

    // A collapsing hit on g0's far corner digs a divot but frees nothing, so
    // the sign, metres away, is not this hit's business.
    let corner = Damage::blast([0.2, 0.2, 0.2], 0.5, "rubble").collapsing();
    let hits = solids.apply(&corner).expect("apply");
    assert!(
        hits.iter()
            .flat_map(|hit| &hit.debris)
            .all(|piece| piece.kind != DebrisKind::Fallen),
        "the corner blast freed nothing"
    );

    // The storey cut takes g2's block, and the sign, untouched, stays. A game
    // may well want a sign to fall with its storey; this is the documented
    // limit of a signature test, which geometry alone cannot see past.
    let hits = solids.apply(&storey_cut()).expect("apply");
    let g2 = hits
        .iter()
        .find(|hit| hit.group.id == "g2")
        .expect("a g2 hit");
    let fallen: f64 = g2
        .debris
        .iter()
        .filter(|piece| piece.kind == DebrisKind::Fallen)
        .map(|piece| piece.volume)
        .sum();
    assert!((fallen - 46.4).abs() < 1e-9, "fallen {fallen}");
    // The sign is 1.0 * 0.5 * 0.05.
    assert!(
        (group_volume(&g2.group) - 0.025).abs() < 1e-9,
        "the sign stayed: volume {}",
        group_volume(&g2.group)
    );
}

#[test]
fn replay_equals_play_with_a_falling_storey() {
    let mesher = ManifoldMesher::default();
    let building = stack();
    let (mut solids, mut kept) =
        GroupSolids::build(&building, &DamageLog::default(), mesher).expect("build");
    let records = [
        Damage::blast([0.2, 0.2, 0.2], 0.5, "rubble").collapsing(),
        storey_cut(),
    ];
    for record in &records {
        for hit in solids.apply(record).expect("apply") {
            assert!(kept.replace_group(hit.group), "the group exists");
        }
        kept.record(record.clone());
    }
    let replay = mesh_building_with_damage(&building, solids.log(), &mesher).expect("replay");
    assert_eq!(kept.groups.len(), replay.groups.len());
    for (left, right) in kept.groups.iter().zip(&replay.groups) {
        assert_eq!(left.id, right.id);
        assert!(
            (group_volume(left) - group_volume(right)).abs() < 1e-9,
            "{} volume",
            left.id
        );
        assert!(
            (group_area(left) - group_area(right)).abs() < 1e-9,
            "{} area",
            left.id
        );
        match (left.bounds, right.bounds) {
            (Some(min_max), Some(other_min_max)) => {
                for axis in 0..3 {
                    assert!(
                        (min_max[0][axis] - other_min_max[0][axis]).abs() < 1e-9,
                        "{} min",
                        left.id
                    );
                    assert!(
                        (min_max[1][axis] - other_min_max[1][axis]).abs() < 1e-9,
                        "{} max",
                        left.id
                    );
                }
            }
            (None, None) => {}
            other => panic!("{} bounds differ: {other:?}", left.id),
        }
    }
}

#[test]
fn a_fallen_groups_collider_and_proxies_follow() {
    let mesher = ManifoldMesher::default();
    let (mut solids, mut kept) =
        GroupSolids::build(&stack(), &DamageLog::default(), mesher).expect("build");
    for hit in solids.apply(&storey_cut()).expect("apply") {
        assert!(kept.replace_group(hit.group));
    }
    let g2 = kept
        .groups
        .iter()
        .find(|group| group.id == "g2")
        .expect("g2");
    // An empty group still answers with a triangle collider, empty because no
    // triangles are left.
    let collider = g2.collider.as_ref().expect("g2's collider");
    assert!(collider.positions.is_empty());
    // Its convex proxy is no longer listed.
    let colliders = kept.colliders();
    let proxies: Vec<&str> = colliders
        .iter()
        .filter(|collider| collider.instance == "s2")
        .map(|collider| collider.element.as_str())
        .collect();
    assert!(proxies.is_empty(), "g2's proxy gave way: {proxies:?}");
}

#[test]
fn contacts_are_asked_once() {
    let mesher = ManifoldMesher::default();
    let (mut solids, _kept) =
        GroupSolids::build(&stack(), &DamageLog::default(), mesher).expect("build");

    // A collapsing hit on one corner of g0 digs a divot and frees nothing, so
    // only g0's piece changes. The walk settles g0 against g1 and g1 against
    // g2, two pairs.
    let before_first = solids.contact_tests();
    solids
        .apply(&Damage::blast([0.2, 0.2, 0.2], 0.5, "rubble").collapsing())
        .expect("apply");
    let first = solids.contact_tests() - before_first;

    // The same-sized hit on another corner of the same storey. Only g0's piece
    // has changed since, so the only pair the walk has to ask about is g0's;
    // the g1/g2 answer from the first hit is still good.
    solids
        .apply(&Damage::blast([3.8, 0.2, 3.8], 0.5, "rubble").collapsing())
        .expect("apply");
    let second = solids.contact_tests() - before_first - first;

    assert_eq!(
        first, 1,
        "only g0/g1: the walk in `build` already asked about g1/g2, and neither changed"
    );
    assert_eq!(second, 1, "only g0/g1; g1/g2 was cached");
}

#[test]
fn a_cached_contact_is_forgotten_when_its_piece_changes() {
    let mesher = ManifoldMesher::default();
    let (mut solids, _kept) =
        GroupSolids::build(&stack(), &DamageLog::default(), mesher).expect("build");

    // A divot inside g1, clear of both interfaces, changes g1 while leaving it
    // stood on g0 and holding g2 up.
    let divot = Damage::blast([0.2, 4.5, 0.2], 0.5, "rubble").collapsing();
    let hits = solids.apply(&divot).expect("apply");
    assert!(
        hits.iter().all(|hit| hit.group.id == "g1"),
        "the divot changes only g1"
    );

    // A slab through the g0/g1 joint severs g1, and g1 can no longer hold g2.
    // The old "g0 touches g1" answer must not survive g1 changing.
    let sever = Damage::new(Geometry::cuboid([6.0, 0.2, 6.0]), "rubble")
        .placed(Pose::at([-1.0, 2.9, -1.0]))
        .collapsing();
    let hits = solids.apply(&sever).expect("apply");
    let g1 = hits
        .iter()
        .find(|hit| hit.group.id == "g1")
        .expect("a g1 hit");
    assert_eq!(group_triangles(&g1.group), 0, "g1 fell");
    let g2 = hits
        .iter()
        .find(|hit| hit.group.id == "g2")
        .expect("a g2 hit");
    assert_eq!(group_triangles(&g2.group), 0, "g2 fell with it");
    assert!(
        g2.debris
            .iter()
            .any(|piece| piece.kind == DebrisKind::Fallen),
        "the top storey dropped"
    );
}
