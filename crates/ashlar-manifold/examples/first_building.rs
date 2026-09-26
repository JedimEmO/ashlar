//! The building from the manual's "Getting started" chapter: two bays of wall,
//! each with a door-sized opening, meshed and counted.
//!
//! `cargo run --example first_building -p ashlar-manifold`
use ashlar::{Building, Collision, Element, Geometry, Instance, Part, Pose};
use ashlar_manifold::{ManifoldMesher, mesh_building};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let bay = Part::builder("example:bay")
        .element(
            Element::new(
                "shell",
                Geometry::cuboid([4.0, 3.0, 0.3])
                    .subtract(Geometry::cuboid([1.2, 2.2, 0.5]).placed(Pose::at([1.4, 0.0, -0.1]))),
                "wall",
            )
            .collision(Collision::Bounds),
        )
        .build()?;

    let building = Building::builder("example:wall")
        .part(bay)
        .material("wall", "library:brick")
        .instance(Instance::new("first", "example:bay"))
        .instance(Instance::new("second", "example:bay").placed(Pose::at([4.0, 0.0, 0.0])))
        .build()?;

    let meshed = mesh_building(&building, &ManifoldMesher::default())?;
    let triangles: usize = meshed
        .pieces()
        .map(|piece| piece.mesh.triangle_count())
        .sum();
    println!(
        "{triangles} triangles, {} collision proxies",
        meshed.colliders().len()
    );
    Ok(())
}
