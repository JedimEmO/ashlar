//! Many blasts on the interior house: every one has to apply.
use ashlar::{Damage, DamageLog, Geometry, MergedGroup, Pose, TriangleMesh};
use ashlar_manifold::{GroupHit, GroupSolids, ManifoldMesher};
use ashlar_showcase::interior;
use std::time::{Duration, Instant};

/// The volume a closed triangle mesh encloses, from the divergence theorem.
fn volume(mesh: &TriangleMesh) -> f64 {
    mesh.triangles()
        .map(|[a, b, c]| a.dot(b.cross(c)) / 6.0)
        .sum()
}

/// The volume of a merged group's surface.
fn group_volume(group: &MergedGroup) -> f64 {
    group.batches.iter().map(|batch| volume(&batch.mesh)).sum()
}

#[test]
fn forty_blasts_all_apply() {
    let building = interior::building().expect("house");
    let (mut solids, _) =
        GroupSolids::build(&building, &DamageLog::default(), ManifoldMesher::default())
            .expect("solids");
    let mut failures = Vec::new();
    for i in 0..200_u32 {
        // A low-discrepancy walk over the front wall and a little beyond it.
        let t = f64::from(i);
        // Clustered, so holes overlap holes, which is what clicking around does.
        let x = 2.0 + (t * 0.618_033_988_7).fract() * 3.0;
        let y = 0.5 + (t * 0.754_877_666_2).fract() * 4.5;
        let z = (t * 0.569_840_290_9).fract() * 0.3;
        let damage = Damage::blast([x, y, z], 1.2, "stone").collapsing();
        if let Err(error) = solids.apply(&damage) {
            failures.push(format!("{i}: {error}"));
        }
    }
    assert!(failures.is_empty(), "{failures:#?}");
}

#[test]
fn blasts_on_the_towers_own_surface_all_apply() {
    use ashlar_showcase::corporate;
    let building =
        corporate::merged_by_storey(corporate::building(4).expect("tower")).expect("merged");
    let (mut solids, meshed) =
        GroupSolids::build(&building, &DamageLog::default(), ManifoldMesher::default())
            .expect("solids");
    // Blast centres are surface points, as a click gives: every 97th vertex.
    let centres: Vec<[f64; 3]> = meshed
        .groups
        .iter()
        .flat_map(|group| &group.batches)
        .flat_map(|batch| &batch.mesh.positions)
        .step_by(97)
        .take(60)
        .map(ashlar::glam::DVec3::to_array)
        .collect();
    let mut failures = Vec::new();
    let count = centres.len();
    let mut first_ten = Duration::ZERO;
    let mut last_ten = Duration::ZERO;
    for (i, centre) in centres.into_iter().enumerate() {
        let started = Instant::now();
        let result = solids.apply(&Damage::blast(centre, 1.2, "stone").collapsing());
        let elapsed = started.elapsed();
        if let Err(error) = result {
            failures.push(format!("{i} at {centre:?}: {error}"));
        }
        if i < 10 {
            first_ten += elapsed;
        }
        if i >= count.saturating_sub(10) {
            last_ten += elapsed;
        }
    }
    assert!(failures.is_empty(), "{failures:#?}");
    // A ratio, not a wall-clock number: a kept solid whose run table multiplies
    // makes a late hit orders of magnitude slower than an early one, and the
    // ratio holds on a slow machine where an absolute bound would not.
    if count >= 20 {
        assert!(
            last_ten < first_ten * 5,
            "the last ten hits took {last_ten:?} against {first_ten:?} for the first ten"
        );
    }
}

#[test]
fn blasts_chasing_the_damaged_surface_all_apply() {
    let building = interior::building().expect("house");
    let (mut solids, meshed) =
        GroupSolids::build(&building, &DamageLog::default(), ManifoldMesher::default())
            .expect("solids");
    // Each blast is centred on a vertex of the surface the last one left, as a
    // player clicking on rims and rubble does.
    let mut surface: Vec<[f64; 3]> = meshed
        .groups
        .iter()
        .flat_map(|group| &group.batches)
        .flat_map(|batch| &batch.mesh.positions)
        .map(ashlar::glam::DVec3::to_array)
        .collect();
    let mut failures = Vec::new();
    // The hits the loop last saw, so the check after it can look at the final
    // mesh every group was left with.
    let mut last: Vec<GroupHit> = Vec::new();
    for i in 0..120_usize {
        if surface.is_empty() {
            break;
        }
        // Through f32 and back, as a renderer's hit position comes: a centre a
        // few 1e-7 off a wall plane is what makes near-coincident geometry.
        #[allow(clippy::cast_possible_truncation)]
        let centre = surface[(i * 7919) % surface.len()].map(|v| f64::from((v + 0.013) as f32));
        match solids.apply(&Damage::blast(centre, 1.2, "stone").collapsing()) {
            Ok(hits) => {
                let next: Vec<[f64; 3]> = hits
                    .iter()
                    .flat_map(|hit| &hit.group.batches)
                    .flat_map(|batch| &batch.mesh.positions)
                    .map(ashlar::glam::DVec3::to_array)
                    .collect();
                if !next.is_empty() {
                    surface = next;
                }
                if !hits.is_empty() {
                    last = hits;
                }
            }
            Err(error) => failures.push(format!("{i} at {centre:?}: {error}")),
        }
    }
    assert!(failures.is_empty(), "{failures:#?}");
    for hit in &last {
        for batch in &hit.group.batches {
            assert!(
                batch.mesh.is_consistent(),
                "{} was left with an inconsistent mesh",
                hit.group.id
            );
        }
    }
}

/// One collapsing slab across the whole footprint cuts the tower free of its
/// podium, and everything above it falls: support now crosses the storey
/// groups, so a storey whose base is removed goes with it.
#[test]
fn cutting_the_tower_in_two_drops_the_top() {
    use ashlar::glam::DVec3;
    use ashlar_showcase::corporate;
    let building =
        corporate::merged_by_storey(corporate::building(4).expect("tower")).expect("merged");
    let mesher = ManifoldMesher::default();
    let (mut solids, undamaged) =
        GroupSolids::build(&building, &DamageLog::default(), mesher).expect("solids");

    // The band of storey 1, read off the groups rather than the private storey
    // constants, and a slab of that thickness spanning the whole footprint
    // grown by a metre.
    let band = undamaged
        .groups
        .iter()
        .find(|group| group.storey == Some(1))
        .and_then(|group| group.bounds)
        .expect("storey 1's bounds");
    let mut footprint_min = DVec3::splat(f64::INFINITY);
    let mut footprint_max = DVec3::splat(f64::NEG_INFINITY);
    for group in &undamaged.groups {
        if let Some([min, max]) = group.bounds {
            footprint_min = footprint_min.min(min);
            footprint_max = footprint_max.max(max);
        }
    }
    let grown_min = footprint_min - DVec3::splat(1.0);
    let grown_max = footprint_max + DVec3::splat(1.0);
    let size = grown_max - grown_min;
    let slab = Damage::new(
        Geometry::cuboid([size.x, band[1].y - band[0].y, size.z]),
        "stone",
    )
    .placed(Pose::at([grown_min.x, band[0].y, grown_min.z]))
    .collapsing();

    // The volume of every group that stands wholly above the slab.
    let above = |groups: &[MergedGroup]| -> f64 {
        groups
            .iter()
            .filter(|group| {
                group
                    .bounds
                    .is_some_and(|bounds| bounds[0].y >= band[1].y - 1e-6)
            })
            .map(group_volume)
            .sum()
    };
    let before = above(&undamaged.groups);
    let hits = solids.apply(&slab).expect("apply");
    let mut after = undamaged.clone();
    for hit in hits {
        after.replace_group(hit.group);
    }
    let remaining = above(&after.groups);
    assert!(before > 0.0, "the tower has storeys above the slab");
    // What is left is the loose-by-design pieces the exemption list keeps.
    assert!(
        remaining < before * 0.02,
        "the top fell: {remaining} of {before} m^3 remained"
    );
}

/// A collapsing hit drops what *it* cut free. A group may already hold pieces
/// that touch nothing and do not reach its base, a rail or a sign stood off its
/// wall, and a blast at the other end of the storey has no business with them.
#[test]
fn a_collapsing_hit_leaves_distant_loose_pieces_alone() {
    use ashlar_manifold::DebrisKind;
    use ashlar_showcase::corporate;
    let building =
        corporate::merged_by_storey(corporate::building(4).expect("tower")).expect("merged");
    let (mut solids, meshed) =
        GroupSolids::build(&building, &DamageLog::default(), ManifoldMesher::default())
            .expect("solids");
    for group in &meshed.groups {
        let [min, max] = group.bounds.expect("bounds");
        // A small blast on one low corner of the group's bounds.
        let centre = [min.x, min.y + 0.5, min.z];
        let hits = solids
            .apply(&Damage::blast(centre, 0.6, "stone").collapsing())
            .expect("hit");
        for hit in &hits {
            for debris in &hit.debris {
                if debris.kind == DebrisKind::Fallen {
                    let reach = 0.6 + 1.0;
                    let near = (0..3).all(|axis| {
                        (debris.centre[axis] - centre[axis]).abs()
                            <= reach + (max - min)[axis] * 0.0
                    });
                    assert!(
                        near,
                        "{}: a piece at {:?} fell to a 0.6 m blast at {centre:?}",
                        hit.group.id, debris.centre
                    );
                }
            }
        }
    }
}
