use ashlar::{Building, Element, Geometry, Instance, Part, Pose, ValidationError};

/// A geometry exercise, with architectural decisions confined to this recipe.
pub fn building() -> Result<Building, ValidationError> {
    let mut recipe = definitions()?;
    for level in 0..2 {
        for bay in 0..3 {
            let part = if level == 0 && bay == 1 {
                "door"
            } else {
                "window"
            };
            recipe = recipe
                .instance(
                    Instance::new(format!("front-{level}-{bay}"), part).placed(Pose::at([
                        f64::from(bay) * 4.0,
                        f64::from(level) * 4.5,
                        0.0,
                    ])),
                )
                .instance(
                    Instance::new(format!("back-{level}-{bay}"), "window").placed(Pose::at([
                        f64::from(bay) * 4.0,
                        f64::from(level) * 4.5,
                        8.0,
                    ])),
                );
        }
        for bay in 0..2 {
            for (side, x) in [("left", 0.0), ("right", 12.0)] {
                recipe = recipe.instance(
                    Instance::new(format!("{side}-{level}-{bay}"), "window").placed(
                        Pose::at([x, f64::from(level) * 4.5, f64::from(bay) * 4.0])
                            .rotated(glam::DQuat::from_rotation_y(-std::f64::consts::FRAC_PI_2)),
                    ),
                );
            }
        }
    }
    for bay in 0..4 {
        recipe = recipe.instance(
            Instance::new(format!("pier-{bay}"), "pier").placed(Pose::at([
                f64::from(bay) * 4.0 - 0.25,
                0.0,
                -0.35,
            ])),
        );
    }
    for (id, y) in [("floor", -0.3), ("roof", 9.0)] {
        recipe = recipe.instance(Instance::new(id, "slab").placed(Pose::at([-0.75, y, -0.5])));
    }
    recipe = recipe
        .instance(Instance::new("entrance-canopy", "canopy").placed(Pose::at([3.5, 3.5, -2.8])));
    for i in 0..2 {
        recipe = recipe.instance(
            Instance::new(format!("roof-tank-{i}"), "tank").placed(Pose::at([
                2.0 + f64::from(i) * 2.2,
                9.3,
                5.5,
            ])),
        );
    }
    recipe.build()
}

fn definitions() -> Result<ashlar::BuildingBuilder, ValidationError> {
    let panel = |id: &str, opening: Geometry| {
        Part::builder(id)
            .element(Element::new(
                "wall",
                Geometry::cuboid([4.0, 4.5, 0.65]).subtract(opening),
                "shell",
            ))
            // The sill is inset a centimetre from the wall's ends, so its side
            // faces are not coplanar with the wall's in another material.
            .element(Element::new(
                "sill",
                Geometry::cuboid([3.98, 0.18, 0.9]).placed(Pose::at([0.01, 0.0, -0.12])),
                "trim",
            ))
            .build()
    };
    let window = panel(
        "window",
        Geometry::cuboid([2.6, 1.4, 1.0]).placed(Pose::at([0.7, 1.7, -0.1])),
    )?;
    let door = panel(
        "door",
        Geometry::cuboid([2.4, 3.3, 1.0]).placed(Pose::at([0.8, -0.1, -0.1])),
    )?;
    let pier = Part::builder("pier")
        .element(Element::new(
            "shaft",
            Geometry::cuboid([0.5, 9.0, 1.0]),
            "trim",
        ))
        .build()?;
    let slab = Part::builder("slab")
        .element(Element::new(
            "slab",
            Geometry::cuboid([13.4, 0.3, 9.2]),
            "shell",
        ))
        .build()?;
    let canopy = Part::builder("canopy")
        .element(Element::new(
            "shelf",
            Geometry::extrude(
                [
                    [0.0, 0.0],
                    [5.0, 0.0],
                    [5.0, 2.8],
                    [3.8, 2.8],
                    [3.8, 1.8],
                    [0.0, 1.8],
                ],
                0.25,
            ),
            "accent",
        ))
        .build()?;
    let tank = Part::builder("tank")
        .element(Element::new(
            "body",
            Geometry::cylinder(0.8, 3.2, 48),
            "accent",
        ))
        .element(Element::new(
            "base",
            Geometry::cylinder(0.95, 0.15, 48),
            "trim",
        ))
        .element(Element::new(
            "cap",
            Geometry::cylinder(0.95, 0.15, 48).placed(Pose::at([0.0, 3.2, 0.0])),
            "trim",
        ))
        .build()?;
    let recipe = Building::builder("preview:assembly")
        .material("shell", "shell")
        .material("trim", "trim")
        .material("accent", "accent")
        .part(window)
        .part(door)
        .part(pier)
        .part(slab)
        .part(canopy)
        .part(tank);
    Ok(recipe)
}
