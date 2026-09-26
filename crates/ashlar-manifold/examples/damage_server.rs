//! A damage server that links no Bevy.
//!
//! The point of this example is what it does not link: no Bevy, no renderer. A
//! server that must know the shape of a hole depends on `ashlar` and
//! `ashlar-manifold` and nothing else, which is the deployment choice ADR 0005
//! keeps open. A client derives the same hole from the same log, and a server
//! that only forwards the log never compiles a kernel at all.
//!
//! It builds a small merged wall, reads a [`DamageLog`] from RON, applies each
//! record to one kept solid per group, prints what each hit changed and what
//! collision is left, then replays the whole log through the stateless mesher
//! and checks the two agree. Run it from the workspace root:
//!
//! ```sh
//! cargo run -p ashlar-manifold --example damage_server
//! ```
//!
//! An optional first argument names the RON log to replay; the default is
//! `examples/damage_log.ron` beside this file.
#![allow(
    clippy::print_stdout,
    reason = "an example whose entire output is the report it prints"
)]

use std::path::PathBuf;
use std::process::ExitCode;

use ashlar::{
    Building, Collision, DamageLog, Element, Geometry, Instance, MergeGroup, MergedGroup, Part,
    Pose, UvMode,
};
use ashlar_manifold::{GroupSolids, ManifoldMesher, mesh_building_with_damage};

fn main() -> ExitCode {
    match run() {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::FAILURE,
        Err(error) => {
            println!("damage_server: {error}");
            ExitCode::FAILURE
        }
    }
}

/// Replay the log, print the report, and answer whether the two paths agreed.
fn run() -> Result<bool, Box<dyn std::error::Error>> {
    let path = std::env::args_os().nth(1).map_or_else(
        || {
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("examples")
                .join("damage_log.ron")
        },
        PathBuf::from,
    );
    let log: DamageLog = ron::from_str(&std::fs::read_to_string(&path)?)?;

    let building = building()?;
    let mesher = ManifoldMesher::default();
    let (mut solids, mut kept) = GroupSolids::build(&building, &DamageLog::default(), mesher)?;

    println!("replaying {} damage records", log.len());
    for (index, record) in log.iter().enumerate() {
        // Standalone elements are never cut, only reported: whether glass
        // shatters is the game's business, and the server's report is what a
        // game decides it from.
        let standalone = kept.standalone_touched(record);
        let hits = solids.apply(record)?;
        let groups: Vec<&str> = hits.iter().map(|hit| hit.group.id.as_str()).collect();
        let pieces: usize = hits.iter().map(|hit| hit.debris.len()).sum();
        let volume: f64 = hits
            .iter()
            .flat_map(|hit| &hit.debris)
            .map(|piece| piece.volume)
            .sum::<f64>()
            // An empty sum is negative zero, which prints as `-0.000000`;
            // adding `0.0` normalises it, because `-0.0 + 0.0 == 0.0`.
            + 0.0;
        let changed = if groups.is_empty() {
            "nothing".to_owned()
        } else {
            format!("{groups:?}")
        };
        println!(
            "damage[{index}]: changed {changed}, debris {pieces} pieces, {volume:.6} m³, \
             standalone {standalone:?}"
        );
        for hit in hits {
            if !kept.replace_group(hit.group) {
                return Err("a re-meshed group has no match in the kept building".into());
            }
        }
        kept.record(record.clone());
    }

    // What a physics server would load: the convex proxies still standing, and
    // a triangle mesh for each group a hole has made a proxy unable to
    // describe.
    let proxies = kept.colliders();
    println!("physics: {} convex proxies left", proxies.len());
    for (id, collider) in kept.mesh_colliders() {
        let vertices = collider.positions.len();
        let triangles = collider.indices.len() / 3;
        println!("  {id}: triangle collider, {vertices} vertices, {triangles} triangles");
    }

    // The same log, from nothing, through the stateless path a load or a
    // late-joining client takes. Every group's volume must match the kept solid
    // to 1e-9, or one of the two is wrong.
    let replay = mesh_building_with_damage(&building, solids.log(), &mesher)?;
    let mut consistent = replay.groups.len() == kept.groups.len();
    for (incremental, replayed) in kept.groups.iter().zip(&replay.groups) {
        let kept_volume = group_volume(incremental);
        let replay_volume = group_volume(replayed);
        if (kept_volume - replay_volume).abs() > 1e-9 {
            consistent = false;
            println!(
                "  {}: incremental {kept_volume:.9} against replay {replay_volume:.9}",
                incremental.id
            );
        }
    }
    if consistent {
        println!("replay agrees with the incremental result to 1e-9");
    } else {
        println!("replay DIFFERS from the incremental result");
    }
    Ok(consistent)
}

/// The wall the example damages: a shell with an opening and a standalone pane,
/// three bays in two groups, and a palette slot for the faces a blast exposes.
fn building() -> Result<Building, ashlar::ValidationError> {
    Building::builder("damage_server")
        .part(bay()?)
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
}

/// One wall bay: a four-by-three shell with a doorway, and a pane that is
/// standalone and so is reported rather than cut.
fn bay() -> Result<Part, ashlar::ValidationError> {
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
}

/// The volume of a merged group's surface, from the divergence theorem.
fn group_volume(group: &MergedGroup) -> f64 {
    group.batches.iter().map(|batch| volume(&batch.mesh)).sum()
}

/// The volume a closed triangle mesh encloses, from the divergence theorem.
fn volume(mesh: &ashlar::TriangleMesh) -> f64 {
    mesh.triangles()
        .map(|[a, b, c]| a.dot(b.cross(c)) / 6.0)
        .sum()
}
