//! Furniture for the kit's interiors: bunks, tables, stools, consoles, lockers,
//! shelving, a galley counter and a holo-table.
//!
//! Every piece stands on its own origin at floor level, its footprint centred
//! on it and its working face towards -Z, so a room places it by a pose on
//! its floor facing where the piece should face. Every element is
//! `interior()`, so a camera that hides the exterior still sees the room
//! furnished, and `standalone()`, so a merged building keeps furniture out of
//! its storey solids. Large pieces carry a `Bounds` proxy to walk round;
//! stools and lights carry none.
use ashlar::{Axis, Collision, Element, Geometry, Part, Pose, UvMode, ValidationError};
use glam::DQuat;

use super::{ROUND, SMALL, luminous, plate};

/// An interior, standalone furniture element.
fn fitting(id: &str, geometry: Geometry, slot: &str) -> Element {
    Element::new(id, geometry, slot).interior().standalone()
}

/// A bunk: a steel frame, a mattress and a folded blanket, a shelf over its
/// head and a reading light. Two metres along X.
///
/// # Errors
///
/// If the part does not validate.
pub fn bunk() -> Result<Part, ValidationError> {
    Part::builder("scifi:bunk")
        .element(
            fitting(
                "frame",
                Geometry::union_all([
                    plate([2.0, 0.34, 0.9], [-1.0, 0.08, -0.45]),
                    plate([0.08, 0.9, 0.9], [0.92, 0.0, -0.45]),
                    plate([0.08, 0.6, 0.9], [-1.0, 0.0, -0.45]),
                ]),
                "trim",
            )
            .collision(Collision::Bounds),
        )
        .element(fitting(
            "mattress",
            plate([1.82, 0.14, 0.82], [-0.91, 0.42, -0.41]),
            "cushion",
        ))
        .element(fitting(
            "blanket",
            plate([0.9, 0.05, 0.84], [-0.2, 0.555, -0.42]),
            "hull",
        ))
        .element(fitting(
            "pillow",
            plate([0.3, 0.1, 0.55], [0.55, 0.56, -0.275]),
            "cushion",
        ))
        .element(fitting(
            "shelf",
            plate([0.9, 0.05, 0.3], [0.0, 1.35, 0.13]),
            "trim",
        ))
        .element(fitting(
            "reading-light",
            luminous([0.24, 0.05, 0.08], [0.55, 1.3, 0.3]),
            "light",
        ))
        .element(fitting(
            "shelf-supports",
            Geometry::union_all([0.06, 0.78].map(|x| plate([0.065, 0.98, 0.08], [x, 0.4, 0.35]))),
            "trim",
        ))
        .element(fitting(
            "storage-fronts",
            plate([0.76, 0.21, 0.05], [-0.83, 0.14, -0.49]).arrayed(2, Pose::at([0.86, 0.0, 0.0])),
            "hull",
        ))
        .element(fitting(
            "drawer-pulls",
            plate([0.3, 0.045, 0.04], [-0.6, 0.25, -0.52]).arrayed(2, Pose::at([0.86, 0.0, 0.0])),
            "trim",
        ))
        .build()
}

/// A round table on a turned pedestal.
///
/// # Errors
///
/// If the part does not validate.
pub fn table() -> Result<Part, ValidationError> {
    let pedestal = vec![
        [0.0, 0.0],
        [0.32, 0.0],
        [0.32, 0.04],
        [0.08, 0.12],
        [0.06, 0.7],
        [0.12, 0.72],
        [0.0, 0.72],
    ];
    Part::builder("scifi:table")
        .element(
            fitting("pedestal", Geometry::revolve(pedestal, SMALL), "trim")
                .uv(UvMode::Cylindrical { axis: Axis::Y }),
        )
        .element(
            fitting(
                "top",
                Geometry::cylinder(0.55, 0.05, ROUND).placed(Pose::at([0.0, 0.72, 0.0])),
                "hull",
            )
            .uv(UvMode::Cylindrical { axis: Axis::Y })
            .collision(Collision::Bounds),
        )
        .element(fitting(
            "top-edge",
            Geometry::revolve(
                [[0.53, 0.73], [0.57, 0.73], [0.57, 0.765], [0.53, 0.765]],
                ROUND,
            ),
            "trim",
        ))
        .build()
}

/// A stool: a turned seat on a single post.
///
/// # Errors
///
/// If the part does not validate.
pub fn stool() -> Result<Part, ValidationError> {
    let profile = vec![
        [0.0, 0.0],
        [0.2, 0.0],
        [0.2, 0.03],
        [0.04, 0.08],
        [0.035, 0.42],
        [0.19, 0.44],
        [0.2, 0.48],
        [0.0, 0.49],
    ];
    Part::builder("scifi:stool")
        .element(
            fitting("body", Geometry::revolve(profile, SMALL), "trim")
                .uv(UvMode::Cylindrical { axis: Axis::Y }),
        )
        .element(fitting(
            "seat-pad",
            Geometry::revolve(
                [
                    [0.0, 0.48],
                    [0.17, 0.48],
                    [0.18, 0.5],
                    [0.16, 0.535],
                    [0.0, 0.535],
                ],
                SMALL,
            ),
            "cushion",
        ))
        .element(fitting(
            "foot-ring",
            Geometry::revolve(
                [[0.12, 0.16], [0.15, 0.16], [0.15, 0.19], [0.12, 0.19]],
                SMALL,
            ),
            "trim",
        ))
        .element(fitting(
            "foot-ring-spokes",
            plate([0.28, 0.03, 0.035], [-0.14, 0.16, -0.0175]),
            "trim",
        ))
        .build()
}

/// A console: a desk with an angled, glowing screen and a lit keyboard strip.
/// 1.4 m wide, its operator stands at -Z.
///
/// # Errors
///
/// If the part does not validate.
pub fn console() -> Result<Part, ValidationError> {
    let tilt = Pose::at([-0.62, 0.8, 0.12]).rotated(DQuat::from_rotation_x(-0.45));
    Part::builder("scifi:console")
        .element(
            fitting(
                "desk",
                Geometry::union_all([
                    plate([1.4, 0.76, 0.5], [-0.7, 0.0, -0.05]),
                    plate([1.4, 0.06, 0.7], [-0.7, 0.76, -0.25]),
                ]),
                "hull",
            )
            .uv(UvMode::Box)
            .collision(Collision::Bounds),
        )
        .element(fitting(
            "screen-housing",
            Geometry::chamfered_cuboid([1.24, 0.62, 0.08], 0.02).placed(tilt),
            "trim",
        ))
        .element(fitting(
            "screen",
            Geometry::cuboid([1.12, 0.52, 0.02])
                .placed(tilt.compose(Pose::at([0.06, 0.05, -0.015]))),
            "dark",
        ))
        .element(fitting(
            "keys",
            Geometry::cuboid([0.07, 0.025, 0.055])
                .placed(Pose::at([-0.45, 0.825, -0.22]))
                .arrayed(10, Pose::at([0.09, 0.0, 0.0]))
                .arrayed(2, Pose::at([0.0, 0.0, 0.08])),
            "trim",
        ))
        .element(fitting(
            "kick",
            plate([1.3, 0.1, 0.04], [-0.65, 0.0, -0.1]),
            "dark",
        ))
        .element(fitting(
            "screen-readouts",
            Geometry::union_all([
                Geometry::cuboid([0.46, 0.025, 0.012])
                    .placed(tilt.compose(Pose::at([0.12, 0.43, -0.028]))),
                Geometry::cuboid([0.32, 0.025, 0.012])
                    .placed(tilt.compose(Pose::at([0.12, 0.33, -0.028]))),
                Geometry::cuboid([0.4, 0.025, 0.012])
                    .placed(tilt.compose(Pose::at([0.12, 0.23, -0.028]))),
                Geometry::cuboid([0.28, 0.3, 0.012])
                    .placed(tilt.compose(Pose::at([0.78, 0.15, -0.028]))),
            ]),
            "neon",
        ))
        .element(fitting(
            "desk-service-cover",
            super::service_cover([-0.58, 0.18, -0.12], 1.16, 0.44),
            "trim",
        ))
        .build()
}

/// A bank of three tall lockers with vents and an indicator light on each.
/// 1.5 m wide and 0.5 deep, doors towards -Z.
///
/// # Errors
///
/// If the part does not validate.
pub fn lockers() -> Result<Part, ValidationError> {
    let mut part = Part::builder("scifi:lockers").element(
        fitting(
            "cabinet",
            Geometry::chamfered_cuboid([1.5, 2.0, 0.5], 0.03).placed(Pose::at([-0.75, 0.0, -0.25])),
            "hull",
        )
        .uv(UvMode::Box)
        .collision(Collision::Bounds),
    );
    for index in 0..3 {
        let x = -0.72 + 0.49 * f64::from(index);
        part = part
            .element(fitting(
                &format!("door-{index}"),
                plate([0.45, 1.86, 0.03], [x, 0.07, -0.275]),
                "trim",
            ))
            .element(fitting(
                &format!("vent-{index}"),
                plate([0.3, 0.12, 0.02], [x + 0.075, 1.6, -0.29]),
                "dark",
            ))
            .element(fitting(
                &format!("indicator-{index}"),
                luminous([0.05, 0.05, 0.02], [x + 0.35, 1.1, -0.29]),
                "light",
            ));
    }
    part.element(fitting(
        "pulls",
        plate([0.035, 0.22, 0.045], [-0.38, 0.82, -0.32]).arrayed(3, Pose::at([0.49, 0.0, 0.0])),
        "trim",
    ))
    .element(fitting(
        "vent-louvres",
        plate([0.28, 0.018, 0.025], [-0.635, 1.62, -0.305])
            .arrayed(3, Pose::at([0.0, 0.035, 0.0]))
            .arrayed(3, Pose::at([0.49, 0.0, 0.0])),
        "trim",
    ))
    .build()
}

/// Open shelving loaded with small crates. 1.6 m wide, 0.5 deep.
///
/// # Errors
///
/// If the part does not validate.
pub fn shelving() -> Result<Part, ValidationError> {
    let shelves = Geometry::cuboid([1.6, 0.04, 0.5])
        .placed(Pose::at([-0.8, 0.3, -0.25]))
        .arrayed(4, Pose::at([0.0, 0.5, 0.0]));
    let posts = Geometry::union_all([-0.8, 0.75].map(|x| {
        Geometry::union_all(
            [-0.25, 0.2].map(|z| Geometry::cuboid([0.05, 1.9, 0.05]).placed(Pose::at([x, 0.0, z]))),
        )
    }));
    let boxes = Geometry::union_all([
        plate([0.45, 0.3, 0.38], [-0.7, 0.34, -0.2]),
        plate([0.35, 0.25, 0.36], [0.1, 0.34, -0.2]),
        plate([0.5, 0.35, 0.4], [-0.3, 0.84, -0.2]),
        plate([0.3, 0.2, 0.3], [0.35, 0.84, -0.15]),
        plate([0.6, 0.3, 0.4], [-0.72, 1.34, -0.2]),
    ]);
    Part::builder("scifi:shelving")
        .element(
            fitting("rack", Geometry::union_all([shelves, posts]), "rust")
                .collision(Collision::Bounds),
        )
        .element(fitting("boxes", boxes, "hull").uv(UvMode::Box))
        .element(fitting(
            "rear-bracing",
            Geometry::union_all([
                super::brace([-0.76, 0.2, 0.225], [0.76, 1.83, 0.225], 0.023),
                super::brace([0.76, 0.2, 0.225], [-0.76, 1.83, 0.225], 0.023),
            ]),
            "trim",
        ))
        .build()
}

/// A galley counter: cabinets under a worktop with a sink and a glowing hob,
/// wall cupboards over it. 2.0 m wide and 0.62 deep.
///
/// # Errors
///
/// If the part does not validate.
pub fn galley() -> Result<Part, ValidationError> {
    let counter = Geometry::chamfered_cuboid([2.0, 0.88, 0.62], 0.02)
        .placed(Pose::at([-1.0, 0.0, -0.31]))
        .subtract(Geometry::cuboid([0.5, 0.2, 0.4]).placed(Pose::at([0.3, 0.74, -0.2])));
    Part::builder("scifi:galley")
        .element(
            fitting("counter", counter, "hull")
                .cut_material("trim")
                .uv(UvMode::Box)
                .collision(Collision::Bounds),
        )
        .element(fitting(
            "hob",
            plate([0.5, 0.025, 0.4], [-0.75, 0.88, -0.2]),
            "dark",
        ))
        .element(fitting(
            "cupboards",
            plate([2.0, 0.6, 0.36], [-1.0, 1.45, -0.05]),
            "hull",
        ))
        .element(fitting(
            "under-light",
            luminous([1.8, 0.03, 0.05], [-0.9, 1.42, -0.02]),
            "light",
        ))
        .element(fitting(
            "cupboard-doors",
            Geometry::cuboid([0.47, 0.54, 0.02])
                .placed(Pose::at([-0.97, 1.48, -0.07]))
                .arrayed(4, Pose::at([0.49, 0.0, 0.0])),
            "trim",
        ))
        .element(fitting(
            "cabinet-doors",
            plate([0.57, 0.65, 0.035], [-0.92, 0.13, -0.335])
                .arrayed(3, Pose::at([0.63, 0.0, 0.0])),
            "trim",
        ))
        .element(fitting(
            "handles",
            plate([0.24, 0.04, 0.035], [-0.76, 0.69, -0.36]).arrayed(3, Pose::at([0.63, 0.0, 0.0])),
            "hull",
        ))
        .element(fitting(
            "tap",
            Geometry::union_all([
                Geometry::cylinder(0.025, 0.24, 10).placed(Pose::at([0.56, 0.86, 0.24])),
                super::brace([0.56, 1.085, 0.24], [0.56, 1.085, 0.03], 0.025),
            ]),
            "trim",
        ))
        .element(fitting(
            "hob-rings",
            Geometry::revolve(
                [[0.06, 0.0], [0.08, 0.0], [0.08, 0.012], [0.06, 0.012]],
                SMALL,
            )
            .placed(Pose::at([-0.63, 0.907, 0.0]))
            .arrayed(2, Pose::at([0.25, 0.0, 0.0])),
            "warning",
        ))
        .element(fitting(
            "menu-frame",
            plate([0.27, 0.42, 0.045], [-0.94, 0.98, 0.21]),
            "trim",
        ))
        .element(fitting(
            "menu-lines",
            luminous([0.17, 0.035, 0.02], [-0.89, 1.05, 0.195])
                .arrayed(3, Pose::at([0.0, 0.1, 0.0])),
            "neon",
        ))
        .element(fitting(
            "backsplash",
            plate([1.94, 0.75, 0.065], [-0.97, 0.86, 0.26]),
            "trim",
        ))
        .build()
}

/// A holo-table: a turned pedestal under a glowing disc and a faint
/// projection column.
///
/// # Errors
///
/// If the part does not validate.
pub fn holo_table() -> Result<Part, ValidationError> {
    let base = vec![
        [0.0, 0.0],
        [0.7, 0.0],
        [0.7, 0.1],
        [0.45, 0.3],
        [0.4, 0.82],
        [0.62, 0.9],
        [0.62, 0.96],
        [0.0, 0.96],
    ];
    Part::builder("scifi:holo-table")
        .element(
            fitting("base", Geometry::revolve(base, ROUND), "hull")
                .uv(UvMode::Cylindrical { axis: Axis::Y })
                .collision(Collision::Bounds),
        )
        .element(
            fitting(
                "disc",
                Geometry::cylinder(0.55, 0.02, ROUND).placed(Pose::at([0.0, 0.96, 0.0])),
                "light",
            )
            .uv(UvMode::Cylindrical { axis: Axis::Y }),
        )
        .element(fitting(
            "projection",
            Geometry::revolve([[0.0, 0.98], [0.3, 0.98], [0.12, 1.7], [0.0, 1.72]], SMALL),
            "light",
        ))
        .element(fitting(
            "ring",
            Geometry::cylinder(0.64, 0.03, ROUND)
                .subtract(Geometry::cylinder(0.6, 0.1, ROUND).placed(Pose::at([0.0, -0.03, 0.0])))
                .placed(Pose::at([0.0, 0.9, 0.0])),
            "trim",
        ))
        .element(fitting(
            "control-pads",
            plate([0.15, 0.035, 0.1], [-0.075, 0.955, -0.56]).arrayed(
                4,
                Pose::default().rotated(DQuat::from_rotation_y(std::f64::consts::FRAC_PI_2)),
            ),
            "trim",
        ))
        .build()
}

/// A ceiling light: a round glowing panel in a steel ring, hung at the part
/// origin (the ceiling) and reaching down from it.
///
/// # Errors
///
/// If the part does not validate.
pub fn ceiling_light() -> Result<Part, ValidationError> {
    Part::builder("scifi:ceiling-light")
        .element(fitting(
            "ring",
            Geometry::cylinder(0.42, 0.06, ROUND).placed(Pose::at([0.0, -0.06, 0.0])),
            "trim",
        ))
        .element(fitting(
            "panel",
            Geometry::cylinder(0.36, 0.02, ROUND).placed(Pose::at([0.0, -0.08, 0.0])),
            "light",
        ))
        .element(fitting(
            "guard",
            Geometry::union_all([
                plate([0.74, 0.035, 0.025], [-0.37, -0.1, -0.0125]),
                plate([0.025, 0.035, 0.74], [-0.0125, -0.1, -0.37]),
            ]),
            "trim",
        ))
        .build()
}

/// A storage crate small enough for a room: a plated box with a lit label.
///
/// # Errors
///
/// If the part does not validate.
pub fn crate_small() -> Result<Part, ValidationError> {
    Part::builder("scifi:crate-small")
        .element(
            fitting(
                "body",
                Geometry::chamfered_cuboid([0.8, 0.6, 0.6], 0.05)
                    .placed(Pose::at([-0.4, 0.0, -0.3])),
                "hull",
            )
            .uv(UvMode::Box)
            .collision(Collision::Bounds),
        )
        .element(fitting(
            "straps",
            Geometry::union_all([
                plate([0.08, 0.62, 0.62], [-0.28, -0.01, -0.31]),
                plate([0.08, 0.62, 0.62], [0.2, -0.01, -0.31]),
            ]),
            "trim",
        ))
        .element(fitting(
            "label",
            luminous([0.3, 0.1, 0.02], [-0.15, 0.4, -0.32]),
            "light",
        ))
        .element(fitting(
            "lid",
            plate([0.61, 0.04, 0.48], [-0.305, 0.575, -0.24]),
            "trim",
        ))
        .build()
}

/// Every furniture part.
///
/// # Errors
///
/// If a part does not validate.
pub fn parts() -> Result<Vec<Part>, ValidationError> {
    Ok(vec![
        bunk()?,
        table()?,
        stool()?,
        console()?,
        lockers()?,
        shelving()?,
        galley()?,
        holo_table()?,
        ceiling_light()?,
        crate_small()?,
    ])
}
