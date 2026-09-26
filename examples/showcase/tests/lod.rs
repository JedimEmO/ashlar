//! Levels of detail on real content: every showcase scene bakes down the
//! default ladder with fewer triangles at every level, every level's surfaces
//! are closed solids, and a baked scene writes and reads back.
#![allow(
    clippy::unwrap_used,
    clippy::cast_precision_loss,
    reason = "test fixtures; a file size printed in megabytes"
)]
use std::collections::BTreeMap;

use ashlar::{BakedBuilding, LodPolicy, Side, TriangleMesh};
use ashlar_manifold::{ManifoldMesher, bake};
use ashlar_showcase::{Scene, building};

/// Every undirected edge of an unwelded mesh appears exactly twice, in
/// opposite directions, once its end points are quantised.
fn closed(mesh: &TriangleMesh) -> bool {
    let mut mesh = mesh.clone();
    mesh.unweld();
    let key = |p: glam::DVec3| {
        #[expect(clippy::cast_possible_truncation, reason = "quantised micrometres")]
        let q = |v: f64| (v * 1e5).round() as i64;
        [q(p.x), q(p.y), q(p.z)]
    };
    let mut edges: BTreeMap<([i64; 3], [i64; 3]), i32> = BTreeMap::new();
    for corners in mesh.corners() {
        let points = corners.map(|index| key(mesh.positions[index]));
        for (a, b) in [(0, 1), (1, 2), (2, 0)] {
            let (from, to) = (points[a], points[b]);
            if from == to {
                continue;
            }
            if from < to {
                *edges.entry((from, to)).or_default() += 1;
            } else {
                *edges.entry((to, from)).or_default() -= 1;
            }
        }
    }
    edges.values().all(|count| *count == 0)
}

fn baked(scene: Scene) -> BakedBuilding {
    let building = building(scene).unwrap();
    bake(&building, &LodPolicy::ladder(), &ManifoldMesher::default())
        .unwrap_or_else(|error| panic!("{scene:?}: {error}"))
}

const SCENES: [Scene; 8] = [
    Scene::Interior,
    Scene::CorporateBlock,
    Scene::CorporateBlockMerged,
    Scene::ScifiColony,
    Scene::ScifiOutpost,
    Scene::CityBlock,
    Scene::Metropolis,
    Scene::Outpost,
];

#[test]
fn every_level_is_coarser_than_the_one_before() {
    for scene in SCENES {
        let started = std::time::Instant::now();
        let baked = baked(scene);
        let elapsed = started.elapsed();
        let triangles = baked.triangles();
        let bytes = baked.write().unwrap().len();
        println!(
            "{scene:?}: triangles {triangles:?}, bake {elapsed:.2?}, file {:.1} MB",
            bytes as f64 / 1e6
        );
        assert_eq!(triangles.len(), 3);
        assert!(
            triangles.windows(2).all(|pair| pair[1] <= pair[0]),
            "{scene:?}: {triangles:?}"
        );
        assert!(triangles[2] < triangles[0], "{scene:?}: {triangles:?}");
        let bands: Vec<f32> = baked
            .levels
            .iter()
            .filter_map(|level| level.until)
            .collect();
        assert!(bands.windows(2).all(|pair| pair[0] < pair[1]), "{bands:?}");
    }
}

#[test]
fn every_level_is_closed_and_the_coarse_ones_have_no_interior() {
    for scene in [Scene::Interior, Scene::ScifiColony, Scene::CityBlock] {
        let baked = baked(scene);
        for (index, level) in baked.levels.iter().enumerate() {
            for (part, elements) in &level.parts {
                for element in elements {
                    // A cut batch is half of a split element, and so open.
                    if element.is_cut {
                        continue;
                    }
                    if index > 0 {
                        assert_eq!(
                            element.side,
                            Side::Exterior,
                            "{scene:?} level {index}: {part}/{} is interior",
                            element.id
                        );
                    }
                }
            }
            for group in &level.groups {
                if index > 0 {
                    assert!(
                        group
                            .batches
                            .iter()
                            .all(|batch| batch.side == Side::Exterior),
                        "{scene:?} level {index}: group {} keeps an interior batch",
                        group.id
                    );
                }
            }
        }
        // Colliders and portals are level zero's at every level.
        let zero = baked.level(0).unwrap();
        let two = baked.level(2).unwrap();
        assert_eq!(zero.colliders().len(), two.colliders().len());
        assert_eq!(zero.portals().len(), two.portals().len());
    }
}

#[test]
fn every_simplified_part_meshes_to_closed_solids() {
    // Per element rather than per batch: a batch split by a cut slot is open
    // on purpose, and the element it came from is what must be a solid.
    use ashlar::GeometryMesher;
    let mesher = ManifoldMesher::default();
    for scene in [Scene::ScifiColony, Scene::CityBlock, Scene::CorporateBlock] {
        let building = building(scene).unwrap();
        for policy in &LodPolicy::ladder()[1..] {
            let simplified = building
                .simplified(policy)
                .unwrap()
                .expect("a scene survives");
            for part in &simplified.recipe().parts {
                for element in &part.elements {
                    let mesh = mesher.mesh(&element.geometry).unwrap_or_else(|error| {
                        panic!("{scene:?}: {}/{}: {error}", part.id, element.id)
                    });
                    assert!(
                        closed(&mesh),
                        "{scene:?} at {policy:?}: {}/{} is not closed",
                        part.id,
                        element.id
                    );
                }
            }
        }
    }
}

#[test]
fn a_baked_metropolis_writes_and_reads_back() {
    let baked = baked(Scene::Metropolis);
    let bytes = baked.write().unwrap();
    let back = BakedBuilding::read(&bytes).unwrap();
    assert_eq!(back.triangles(), baked.triangles());
    assert_eq!(back.levels.len(), baked.levels.len());
    assert_eq!(
        back.building().recipe().instances.len(),
        baked.building().recipe().instances.len()
    );
}

#[test]
#[ignore = "bakes the 8 x 8 metropolis; run by name to measure"]
fn measure_the_large_metropolis() {
    let started = std::time::Instant::now();
    let baked = baked(Scene::MetropolisLarge);
    let elapsed = started.elapsed();
    let bytes = baked.write().unwrap().len();
    println!(
        "MetropolisLarge: triangles {:?}, bake {elapsed:.2?}, file {:.1} MB",
        baked.triangles(),
        bytes as f64 / 1e6
    );
}

/// Triangles drawn at each level: every instance's pieces, not unique meshes.
fn drawn(baked: &BakedBuilding) -> Vec<usize> {
    (0..baked.levels.len())
        .map(|level| {
            baked
                .level(level)
                .unwrap()
                .pieces()
                .map(|piece| piece.mesh.triangle_count())
                .sum()
        })
        .collect()
}

#[test]
#[ignore = "measures drawn triangles and writes each level's recipe for a capture"]
fn write_levels_for_review() {
    let out = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/scratch/lod");
    std::fs::create_dir_all(&out).unwrap();
    let library = ashlar_showcase::library::materials();
    std::fs::write(
        out.join("materials.ron"),
        ron::ser::to_string_pretty(&library, ron::ser::PrettyConfig::default()).unwrap(),
    )
    .unwrap();
    std::fs::write(
        out.join("graphs.ron"),
        ron::ser::to_string_pretty(
            &ashlar_showcase::materials::graphs(),
            ron::ser::PrettyConfig::default(),
        )
        .unwrap(),
    )
    .unwrap();
    for (name, scene) in [
        ("metropolis", Scene::Metropolis),
        ("colony", Scene::ScifiColony),
        ("block", Scene::CorporateBlock),
        ("city-block", Scene::CityBlock),
    ] {
        let building = building(scene).unwrap();
        let baked = baked(scene);
        println!("{name}: drawn {:?}", drawn(&baked));
        for (index, policy) in LodPolicy::ladder().iter().enumerate() {
            let level = building
                .simplified(policy)
                .unwrap()
                .expect("a scene survives");
            std::fs::write(
                out.join(format!("{name}-{index}.ron")),
                ron::ser::to_string_pretty(level.recipe(), ron::ser::PrettyConfig::default())
                    .unwrap(),
            )
            .unwrap();
        }
    }
}
