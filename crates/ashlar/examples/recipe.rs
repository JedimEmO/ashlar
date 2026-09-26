//! A reusable bay and an assembly, with no built-in architectural style.
use ashlar::{Building, Element, Geometry, Instance, Part, Pose, Socket};
use glam::DQuat;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // A full-depth cutter describes an opening; the mesh adapter evaluates this boolean.
    let bay = Part::builder("example:bay")
        .element(Element::new(
            "shell",
            Geometry::cuboid([4.0, 4.0, 0.4])
                .subtract(Geometry::cuboid([2.0, 3.0, 0.6]).placed(Pose::at([1.0, 0.0, -0.1]))),
            "surface",
        ))
        .socket(Socket::new("left", Pose::default()))
        .socket(Socket::new("right", Pose::at([4.0, 0.0, 0.0])))
        .build()?;

    let mut assembly = Building::builder("example:courtyard")
        .grid(0.25)
        .material("surface", "example:painted_metal")
        .part(bay);

    // Repetition is ordinary Rust. No macro or parser is needed.
    for index in 0..3 {
        assembly = assembly.instance(
            Instance::new(format!("front-{index}"), "example:bay").placed(Pose::at([
                f64::from(index) * 4.0,
                0.0,
                0.0,
            ])),
        );
    }
    assembly = assembly.instance(
        Instance::new("side", "example:bay")
            .placed(Pose::at([12.0, 0.0, 0.0]).rotated(DQuat::from_rotation_y(-0.8)))
            .material("surface", "example:ceramic"),
    );
    let building = assembly.build()?;
    // The recipe as RON: the authoring schema a tool edits and reads back.
    println!(
        "{}",
        ron::ser::to_string_pretty(building.recipe(), ron::ser::PrettyConfig::default())?
    );
    Ok(())
}
