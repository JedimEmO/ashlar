//! The sci-fi kit: every piece is a watertight solid, every scene builds, meshes
//! and binds every slot it draws, and the scenes stay inside a triangle budget.
#![allow(clippy::unwrap_used, reason = "test fixtures")]
use std::collections::BTreeMap;

use ashlar::{GeometryMesher, TriangleMesh};
use ashlar_manifold::{ManifoldMesher, mesh_building};
use ashlar_showcase::{Scene, building, library, scifi};

/// Every undirected edge of an unwelded mesh, keyed by its quantised end
/// points, appears exactly twice and in opposite directions.
fn closed(mesh: &TriangleMesh) -> bool {
    let key = |p: glam::DVec3| {
        #[allow(clippy::cast_possible_truncation, reason = "quantised millimicrons")]
        let q = |v: f64| (v * 1e6).round() as i64;
        [q(p.x), q(p.y), q(p.z)]
    };
    let mut edges: BTreeMap<([i64; 3], [i64; 3]), i32> = BTreeMap::new();
    for corners in mesh.corners() {
        let points = corners.map(|index| key(mesh.positions[index]));
        for (a, b) in [(0, 1), (1, 2), (2, 0)] {
            let (from, to) = (points[a], points[b]);
            if from < to {
                *edges.entry((from, to)).or_default() += 1;
            } else {
                *edges.entry((to, from)).or_default() -= 1;
            }
        }
    }
    edges.values().all(|count| *count == 0)
}

#[test]
fn every_piece_of_the_kit_is_a_closed_solid() {
    let mesher = ManifoldMesher::default();
    for part in scifi::parts().unwrap() {
        for element in &part.elements {
            let mut mesh = mesher
                .mesh(&element.geometry)
                .unwrap_or_else(|error| panic!("{}/{}: {error}", part.id, element.id));
            mesh.unweld();
            assert!(
                mesh.triangle_count() > 0,
                "{}/{} is empty",
                part.id,
                element.id
            );
            assert!(closed(&mesh), "{}/{} is not closed", part.id, element.id);
        }
    }
}

#[test]
fn every_scene_builds_meshes_and_binds_its_slots() {
    let definitions = library::materials();
    for scene in [Scene::ScifiKit, Scene::ScifiOutpost, Scene::ScifiColony] {
        let building = building(scene).unwrap_or_else(|error| panic!("{scene:?}: {error}"));
        definitions
            .check_for(&building)
            .unwrap_or_else(|error| panic!("{scene:?}: {error}"));
        let meshed = mesh_building(&building, &ManifoldMesher::default())
            .unwrap_or_else(|error| panic!("{scene:?}: {error}"));
        let unique: usize = meshed
            .parts
            .values()
            .flatten()
            .map(|element| element.mesh.triangle_count())
            .sum();
        let instances = building.recipe().instances.len();
        println!("{scene:?}: {unique} unique triangles, {instances} instances");
        assert!(unique < 400_000, "{scene:?}: {unique}");
    }
}
