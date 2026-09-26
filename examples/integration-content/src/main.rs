//! The content step of a small game: author its buildings in Rust, pick its
//! materials from the default library, and bake both into the game's assets.
//!
//! ```text
//! cargo run --release -p integration-content
//! cargo run --release -p integration-game
//! ```
//!
//! Everything here is the game's own. The kit is two parts, a cottage and a
//! lamp post; the village places them and gives each cottage its own adobe
//! colour, which the content step resolves into a material of its own.
use std::f64::consts::PI;

use anyhow::Result;
use ashlar::{
    Binding, Building, Collision, Element, Geometry, Instance, LodPolicy, ParamValue, Part, Pose,
    Room, glam::DQuat,
};
use ashlar_content::Content;
use ashlar_material::stdlib;

/// Where the game reads its assets, relative to this crate.
const ASSETS: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../integration-game/assets");

/// A cottage: adobe walls with a door and two windows cut through them, a
/// floor, a pitched roof of clay tiles, and glass. The door's cutter is a
/// portal, so a game knows where the way in is.
fn cottage() -> Result<Part> {
    let walls = Geometry::cuboid([6.0, 3.0, 5.0])
        .subtract(
            Geometry::cuboid([5.4, 3.2, 4.4])
                .placed(Pose::at([0.3, 0.2, 0.3]))
                .union(Geometry::cuboid([1.2, 1.2, 0.5]).placed(Pose::at([0.8, 1.1, -0.1])))
                .union(Geometry::cuboid([1.2, 1.2, 0.5]).placed(Pose::at([4.0, 1.1, -0.1]))),
        )
        .subtract(
            Geometry::cuboid([1.0, 2.1, 0.6])
                .placed(Pose::at([2.5, 0.2, -0.15]))
                .portal("door"),
        );
    // A gable profile in X/Z-as-height, extruded along the cottage's depth.
    let roof = Geometry::extrude([[-0.4, 0.0], [6.4, 0.0], [3.0, 2.2]], 5.8).placed(
        // A quarter turn about X stands the profile up and runs it along -Z,
        // so it starts at the back eave and ends at the front one.
        Pose::at([0.0, 3.0, 5.4]).rotated(DQuat::from_rotation_x(-PI / 2.0)),
    );
    Ok(Part::builder("village:cottage")
        .element(
            Element::new("walls", walls, "wall")
                .cut_material("trim")
                .collision(Collision::Bounds),
        )
        .element(
            Element::new(
                "floor",
                Geometry::cuboid([5.4, 0.2, 4.4]).placed(Pose::at([0.3, 0.0, 0.3])),
                "floor",
            )
            .interior(),
        )
        .element(Element::new("roof", roof, "roof").collision(Collision::Hull))
        .element(
            Element::new(
                "glass",
                Geometry::union_all([
                    Geometry::cuboid([1.2, 1.2, 0.03]).placed(Pose::at([0.8, 1.1, 0.14])),
                    Geometry::cuboid([1.2, 1.2, 0.03]).placed(Pose::at([4.0, 1.1, 0.14])),
                ]),
                "glass",
            )
            .standalone(),
        )
        .build()?)
}

/// A lamp post with a lit head.
fn lamp() -> Result<Part> {
    Ok(Part::builder("village:lamp")
        .element(
            Element::new("post", Geometry::cylinder(0.08, 3.2, 16), "trim")
                .collision(Collision::Bounds),
        )
        .element(Element::new(
            "light",
            Geometry::cylinder(0.22, 0.12, 16).placed(Pose::at([0.0, 3.2, 0.0])),
            "light",
        ))
        .build()?)
}

/// Three cottages round a lamp, each in its own adobe colour.
fn village() -> Result<Building> {
    let mut builder = Building::builder("village:green")
        .part(cottage()?)
        .part(lamp()?)
        .material("wall", "library:adobe")
        .material("trim", "library:formed-concrete")
        .material("floor", "library:wood-floor")
        .material("roof", "library:clay-roof-tiles")
        .material("glass", "library:glass")
        .material("light", "library:emissive-strip")
        .instance(Instance::new("lamp", "village:lamp").placed(Pose::at([0.0, 0.0, 0.0])));
    for (index, (x, z, turn, colour)) in [
        (-10.0, 2.0, 0.4, [0.62, 0.52, 0.40]),
        (4.0, 4.0, -0.3, [0.55, 0.58, 0.60]),
        (-3.0, -10.0, PI, [0.66, 0.60, 0.48]),
    ]
    .into_iter()
    .enumerate()
    {
        let id = format!("cottage-{index}");
        builder = builder
            .instance(
                Instance::new(&id, "village:cottage")
                    .placed(Pose::at([x, 0.0, z]).rotated(DQuat::from_rotation_y(turn)))
                    // A binding with parameters is a material of its own; the
                    // content step bakes one set per distinct colour.
                    .binding(
                        "wall",
                        Binding::new("library:adobe").param("color", ParamValue::Color(colour)),
                    ),
            )
            .room(
                Room::new(format!("{id}/room"), [5.4, 2.6, 4.4])
                    .placed(
                        Pose::at([x, 0.0, z])
                            .rotated(DQuat::from_rotation_y(turn))
                            .compose(Pose::at([0.3, 0.2, 0.3])),
                    )
                    .portal("door"),
            );
    }
    Ok(builder.build()?)
}

fn main() -> Result<()> {
    // Only the materials the village wears, from the default library; a game
    // adds its own graphs and definitions to both before this.
    let graphs = stdlib::graphs();
    let mut definitions = stdlib::materials();
    let used = [
        "library:adobe",
        "library:formed-concrete",
        "library:wood-floor",
        "library:clay-roof-tiles",
        "library:glass",
        "library:emissive-strip",
    ];
    definitions
        .materials
        .retain(|key, _| used.contains(&key.as_str()));

    let content = Content::new(ASSETS).resolution(Some(512));
    let shipped = content.ship(
        &graphs,
        &definitions,
        "materials",
        &[("buildings/village.ashlar".to_owned(), village()?)],
        &LodPolicy::ladder(),
    )?;
    println!(
        "{} material sets; village triangles by level {:?}",
        shipped.materials.sets.len(),
        shipped.buildings[0].1.triangles
    );
    Ok(())
}
