//! What a back alley and a night market are made of: gutters and grates,
//! dumpsters and bin bags, the air-conditioners, pipes, fire escapes and
//! caged lamps that cling to a tower's side walls, blade signs in neon,
//! sagging cables, market stalls, vending machines and bollards.
//!
//! A piece that hangs on a wall is authored in the wall's frame: X along the
//! wall, Y up, and -Z out of it, its origin on the wall's face. The generator
//! turns that frame to the wall with `facing`, the way street furniture faces
//! its kerb. A piece that stands on the ground stands on its origin.
use super::{
    Axis, Collision, DQuat, Element, FRAC_PI_2, Geometry, Part, Pose, ROUND, SMALL, UvMode,
    ValidationError, along_x, at, plate, turned,
};

/// Length of one gutter piece.
pub(super) const GUTTER: f64 = 6.0;

/// A wet channel down the middle of an alley, six metres long along +Z, with
/// a drain grate at its middle.
fn gutter() -> Result<Part, ValidationError> {
    let bars = Geometry::cuboid([0.04, 0.012, 0.3])
        .placed(Pose::at([-0.22, 0.03, GUTTER / 2.0 - 0.15]))
        .arrayed(7, Pose::at([0.07, 0.0, 0.0]));
    Part::builder("city:gutter")
        .element(
            Element::new(
                "channel",
                Geometry::cuboid([1.2, 0.03, GUTTER]).placed(Pose::at([-0.6, 0.0, 0.0])),
                "asphalt",
            )
            .uv(UvMode::Box),
        )
        .element(Element::new("grate", bars, "dark"))
        .build()
}

/// A steel dumpster on castors, its lid shut.
fn dumpster() -> Result<Part, ValidationError> {
    let castor = |x: f64, z: f64| at(along_x(0.08, 0.06, SMALL), Pose::at([x - 0.03, 0.08, z]));
    Part::builder("city:dumpster")
        .element(
            Element::new(
                "body",
                Geometry::extrude(
                    [[-0.9, -0.45], [0.9, -0.45], [0.95, 0.5], [-0.95, 0.5]],
                    1.05,
                )
                .placed(Pose::at([0.0, 0.16, 0.0])),
                "bin",
            )
            .uv(UvMode::Box)
            .collision(Collision::Bounds),
        )
        .element(Element::new(
            "lid",
            plate([2.0, 0.07, 1.06], [-1.0, 1.21, -0.5]),
            "dark",
        ))
        .element(Element::new(
            "castors",
            Geometry::union_all([
                castor(-0.75, -0.3),
                castor(0.75, -0.3),
                castor(-0.75, 0.3),
                castor(0.75, 0.3),
            ]),
            "dark",
        ))
        .element(Element::new(
            "lid-ribs",
            plate([0.09, 0.065, 0.86], [-0.82, 1.26, -0.4]).arrayed(6, Pose::at([0.31, 0.0, 0.0])),
            "dark",
        ))
        .element(Element::new(
            "hinges",
            at(along_x(0.065, 0.25, 10), Pose::at([-0.72, 1.23, 0.54]))
                .arrayed(2, Pose::at([1.2, 0.0, 0.0])),
            "trim",
        ))
        .element(Element::new(
            "front-ribs",
            plate([0.085, 0.8, 0.055], [-0.7, 0.27, -0.47]).arrayed(4, Pose::at([0.44, 0.0, 0.0])),
            "bin",
        ))
        .element(Element::new(
            "handle",
            Geometry::union_all([
                plate([0.48, 0.05, 0.05], [-0.24, 1.04, -0.59]),
                plate([0.05, 0.05, 0.17], [-0.24, 1.04, -0.59]),
                plate([0.05, 0.05, 0.17], [0.19, 1.04, -0.59]),
            ]),
            "trim",
        ))
        .build()
}

/// A heap of bin bags.
fn trash() -> Result<Part, ValidationError> {
    Part::builder("city:trash")
        .element(Element::new(
            "bags",
            Geometry::union_all([
                Geometry::ball(0.38, 5).placed(Pose::at([0.0, 0.32, 0.0])),
                Geometry::ball(0.32, 5).placed(Pose::at([0.5, 0.27, 0.15])),
                Geometry::ball(0.3, 5).placed(Pose::at([-0.45, 0.25, 0.2])),
                Geometry::ball(0.28, 5).placed(Pose::at([0.15, 0.62, 0.05])),
                Geometry::ball(0.26, 5).placed(Pose::at([0.2, 0.22, -0.45])),
            ]),
            "bags",
        ))
        .build()
}

/// A window air-conditioner on the wall under a sill: its casing, a fan
/// grille and a rusted drip tray.
fn ac_unit() -> Result<Part, ValidationError> {
    Part::builder("city:ac-unit")
        .element(
            Element::new(
                "casing",
                plate([0.8, 0.5, 0.55], [-0.4, 0.0, -0.55]),
                "metal",
            )
            .uv(UvMode::Box),
        )
        .element(Element::new(
            "fan",
            Geometry::cylinder(0.17, 0.02, 12)
                .placed(Pose::at([0.14, 0.25, -0.55]).rotated(DQuat::from_rotation_x(-FRAC_PI_2))),
            "dark",
        ))
        .element(Element::new(
            "grille",
            Geometry::cuboid([0.3, 0.02, 0.02])
                .placed(Pose::at([-0.33, 0.1, -0.565]))
                .arrayed(6, Pose::at([0.0, 0.055, 0.0])),
            "trim",
        ))
        .element(Element::new(
            "tray",
            Geometry::cuboid([0.7, 0.04, 0.2]).placed(Pose::at([-0.35, -0.05, -0.45])),
            "rust",
        ))
        .element(Element::new(
            "support",
            Geometry::union_all([-0.3, 0.25].map(|x| {
                Geometry::union_all([
                    Geometry::cuboid([0.055, 0.1, 0.62]).placed(Pose::at([x, -0.09, -0.6])),
                    Geometry::cuboid([0.055, 0.35, 0.055]).placed(Pose::at([x, -0.37, -0.035])),
                ])
            })),
            "rust",
        ))
        .element(Element::new(
            "fan-guard",
            Geometry::cuboid([0.34, 0.025, 0.025])
                .placed(Pose::at([-0.03, 0.12, -0.595]))
                .arrayed(5, Pose::at([0.0, 0.065, 0.0])),
            "trim",
        ))
        .build()
}

/// Two pipes and their brackets running up the wall, `height` tall: one
/// storey's worth, stacked storey on storey.
fn pipes(id: &str, height: f64) -> Result<Part, ValidationError> {
    let brackets = [0.4, height - 1.2]
        .map(|y| Geometry::cuboid([0.42, 0.05, 0.26]).placed(Pose::at([-0.12, y, -0.26])));
    let part = Part::builder(id)
        .element(
            Element::new(
                "pipes",
                Geometry::union_all([
                    Geometry::cylinder(0.08, height, 8).placed(Pose::at([0.0, 0.0, -0.16])),
                    Geometry::cylinder(0.05, height, 8).placed(Pose::at([0.2, 0.0, -0.14])),
                ]),
                "rust",
            )
            .uv(UvMode::Cylindrical { axis: Axis::Y }),
        )
        .element(Element::new(
            "brackets",
            Geometry::union_all(brackets),
            "trim",
        ))
        .element(Element::new(
            "couplings",
            Geometry::union_all([(0.0, 0.16, 0.1), (0.2, 0.14, 0.07)].map(|(x, z, r)| {
                Geometry::cylinder(r, 0.12, 8).placed(Pose::at([x, height - 0.28, -z]))
            })),
            "metal",
        ));
    // The distribution branch belongs at the service level. Upper storeys
    // repeat just the risers and a single pair of sleeve joints.
    let part = if height > super::STOREY {
        part.element(Element::new(
            "wall-return",
            Geometry::union_all([
                Geometry::cylinder(0.08, 0.26, 12)
                    .placed(Pose::at([0.0, 3.4, -0.16]).rotated(DQuat::from_rotation_x(FRAC_PI_2))),
                Geometry::ball(0.09, 4).placed(Pose::at([0.0, 3.4, -0.16])),
            ]),
            "rust",
        ))
    } else {
        part
    };
    part.build()
}

/// One storey of a fire escape: a landing a bay wide at the floor, a railing
/// round it, and a steep stair up the facade to the landing above.
#[expect(
    clippy::too_many_lines,
    reason = "landing, stair and rail joints share one structural layout"
)]
fn fire_escape() -> Result<Part, ValidationError> {
    let (width, depth, rise) = (3.0, 1.2, super::STOREY);
    let bar = |size: [f64; 3], corner: [f64; 3]| Geometry::cuboid(size).placed(Pose::at(corner));
    let front = -depth - 0.05;
    let gusset = |x: f64| {
        Geometry::hull([
            [x, -0.08, front],
            [x, -0.08, -0.05],
            [x, -0.53, -0.05],
            [x + 0.08, -0.08, front],
            [x + 0.08, -0.08, -0.05],
            [x + 0.08, -0.53, -0.05],
        ])
    };
    let mut rails = vec![
        bar([0.05, 1.0, 0.05], [-width / 2.0, 0.0, front]),
        bar([0.05, 1.0, 0.05], [width / 2.0 - 0.05, 0.0, front]),
        bar([0.05, 1.0, 0.05], [-width / 2.0, 0.0, -0.1]),
        bar([0.05, 1.0, 0.05], [width / 2.0 - 0.05, 0.0, -0.1]),
        bar([width, 0.05, 0.05], [-width / 2.0, 0.95, front]),
        bar([width, 0.05, 0.05], [-width / 2.0, 0.48, front]),
        bar([0.05, 0.05, depth], [-width / 2.0, 0.95, front]),
        bar([0.05, 0.05, depth], [width / 2.0 - 0.05, 0.95, front]),
        // Brackets under the landing, into the wall.
        gusset(-width / 2.0 + 0.1),
        gusset(width / 2.0 - 0.18),
    ];
    // The stair climbs from the landing's right end to the left end of the
    // landing above, in the half of the landing nearest the wall.
    let run = width - 0.6;
    let slope = rise.atan2(-run);
    let length = run.hypot(rise);
    let stringer = |z: f64| {
        at(
            bar([length, 0.14, 0.04], [0.0, 0.0, 0.0]),
            Pose::at([run / 2.0, 0.0, z]).rotated(DQuat::from_rotation_z(slope)),
        )
    };
    let treads = (1..12).map(|step| {
        let t = f64::from(step) / 12.0;
        bar(
            [0.22, 0.03, 0.56],
            [run / 2.0 - run * t - 0.11, rise * t, -0.66],
        )
    });
    let stringers = vec![stringer(-0.66), stringer(-0.14)];
    let mut stair = Vec::new();
    stair.push(at(
        bar([length * 0.5, 0.045, 0.045], [0.0, 0.0, 0.0])
            .arrayed(2, Pose::at([length * 0.5, 0.0, 0.0])),
        Pose::at([run / 2.0, 0.8, -0.72]).rotated(DQuat::from_rotation_z(slope)),
    ));
    for step in [2, 5, 8, 11] {
        let t = f64::from(step) / 12.0;
        stair.push(bar(
            [0.045, 0.87, 0.08],
            [run / 2.0 - run * t - 0.02, rise * t, -0.72],
        ));
    }
    Part::builder("city:fire-escape")
        .element(
            Element::new(
                "landing",
                // The walking face stays rectangular; the thin underside
                // tapers to a web rather than carrying four extra triangles.
                Geometry::hull([
                    [0.0, 0.06, 0.0],
                    [0.065, 0.06, 0.0],
                    [0.0325, 0.0, 0.0],
                    [0.0, 0.06, depth],
                    [0.065, 0.06, depth],
                    [0.0325, 0.0, depth],
                ])
                .placed(Pose::at([-width / 2.0, -0.07, front]))
                .arrayed(25, Pose::at([0.122, 0.0, 0.0])),
                "deck",
            )
            .uv(UvMode::Box)
            .collision(Collision::Bounds),
        )
        .element(Element::new(
            "landing-frame",
            Geometry::union_all([
                bar(
                    [width + 0.04, 0.08, 0.09],
                    [-width / 2.0 - 0.02, -0.08, front - 0.01],
                ),
                bar(
                    [width + 0.04, 0.08, 0.09],
                    [-width / 2.0 - 0.02, -0.08, -0.15],
                ),
            ]),
            "rust",
        ))
        .element(Element::new(
            "rail-posts",
            Geometry::union_all(rails.drain(..4)),
            "rust",
        ))
        .element(Element::new(
            "rail-tops",
            Geometry::union_all(rails.drain(..4)),
            "rust",
        ))
        .element(Element::new(
            "rail-gussets",
            Geometry::union_all(rails),
            "rust",
        ))
        // Treads and stringers overlap as separate closed solids. Fusing all
        // eleven tread crossings adds hundreds of invisible internal splits.
        .element(Element::new(
            "stair-stringers",
            Geometry::union_all(stringers),
            "rust",
        ))
        .element(Element::new(
            "stair-treads",
            Geometry::union_all(treads),
            "rust",
        ))
        .element(Element::new("stair-handrail", stair.remove(0), "rust"))
        .element(Element::new(
            "stair-posts",
            Geometry::union_all(stair),
            "rust",
        ))
        .element(Element::new(
            "balusters",
            bar([0.035, 0.93, 0.035], [-1.22, 0.0, front + 0.005])
                .arrayed(9, Pose::at([0.3, 0.0, 0.0])),
            "rust",
        ))
        .element(Element::new(
            "wall-plates",
            bar([0.2, 0.58, 0.07], [-1.45, -0.55, -0.09]).arrayed(2, Pose::at([2.7, 0.0, 0.0])),
            "rust",
        ))
        .build()
}

/// The strokes of a neon glyph column on one face of a blade sign, as
/// (along the blade, up) rectangles.
const GLYPHS: [([f64; 2], [f64; 2]); 14] = [
    // The border.
    ([0.08, 0.1], [1.04, 0.05]),
    ([0.08, 2.85], [1.04, 0.05]),
    ([0.08, 0.1], [0.05, 2.8]),
    ([1.07, 0.1], [0.05, 2.8]),
    // Three characters, stacked.
    ([0.3, 2.3], [0.6, 0.05]),
    ([0.57, 1.95], [0.05, 0.7]),
    ([0.3, 2.05], [0.05, 0.3]),
    ([0.3, 1.55], [0.6, 0.05]),
    ([0.3, 1.25], [0.05, 0.3]),
    ([0.85, 1.25], [0.05, 0.3]),
    ([0.3, 1.25], [0.6, 0.05]),
    ([0.3, 0.9], [0.6, 0.05]),
    ([0.57, 0.35], [0.05, 0.6]),
    ([0.3, 0.55], [0.6, 0.05]),
];

/// A blade sign sticking out of the wall: a dark box a metre deep and three
/// tall on two brackets, with neon characters down both faces.
fn blade_sign() -> Result<Part, ValidationError> {
    let (thick, out, tall) = (0.24, 1.2, 3.0);
    let near = -0.45;
    let strokes = GLYPHS.iter().flat_map(|([along, up], [length, height])| {
        [1.0, -1.0].map(|face: f64| {
            let x = if face > 0.0 {
                thick / 2.0 + 0.005
            } else {
                -thick / 2.0 - 0.035
            };
            Geometry::cuboid([0.03, *height, *length]).placed(Pose::at([
                x,
                *up,
                near - along - length,
            ]))
        })
    });
    Part::builder("city:blade-sign")
        .element(Element::new(
            "brackets",
            Geometry::union_all([
                Geometry::cuboid([0.06, 0.06, 0.5]).placed(Pose::at([-0.03, 0.3, near - 0.02])),
                Geometry::cuboid([0.06, 0.06, 0.5]).placed(Pose::at([
                    -0.03,
                    tall - 0.4,
                    near - 0.02,
                ])),
            ]),
            "trim",
        ))
        .element(Element::new(
            "box",
            plate([thick, tall, out], [-thick / 2.0, 0.0, near - out]),
            "dark",
        ))
        .element(
            Element::new("neon", Geometry::union_all(strokes), "neon")
                .standalone()
                .far(),
        )
        .element(Element::new(
            "edge-frame",
            Geometry::union_all([
                plate([0.3, 0.08, 1.28], [-0.15, -0.04, -1.69]),
                plate([0.3, 0.08, 1.28], [-0.15, 2.96, -1.69]),
                plate([0.3, 3.0, 0.08], [-0.15, 0.0, -1.69]),
            ]),
            "trim",
        ))
        .build()
}

/// A cable sagging across a gap `span` long along +X, with lanterns hung from
/// it.
fn cable(id: &str, span: f64) -> Result<Part, ValidationError> {
    let sag = 0.06 * span;
    let steps = 10;
    let point = |step: u32| {
        let t = f64::from(step) / f64::from(steps);
        [t * span, -sag * (1.0 - (2.0 * t - 1.0).powi(2))]
    };
    let segments = (0..steps).map(|step| {
        let [x0, y0] = point(step);
        let [x1, y1] = point(step + 1);
        let length = (x1 - x0).hypot(y1 - y0);
        // Each run a little past its ends, so neighbours overlap into one
        // closed solid rather than meeting at a line.
        at(
            along_x(0.02, length + 0.06, 6),
            Pose::at([x0, y0, 0.0])
                .rotated(DQuat::from_rotation_z((y1 - y0).atan2(x1 - x0)))
                .compose(Pose::at([-0.03, 0.0, 0.0])),
        )
    });
    let lanterns = [3, 5, 7].map(|step| {
        let [x, y] = point(step);
        Geometry::ball(0.14, 5).placed(Pose::at([x, y - 0.2, 0.0]))
    });
    Part::builder(id)
        .element(Element::new("wire", Geometry::union_all(segments), "dark"))
        .element(
            Element::new("lanterns", Geometry::union_all(lanterns), "neon")
                .standalone()
                .far(),
        )
        .element(Element::new(
            "drops",
            Geometry::union_all([3, 5, 7].map(|step| {
                let [x, y] = point(step);
                Geometry::cylinder(0.025, 0.18, 6).placed(Pose::at([x, y - 0.18, 0.0]))
            })),
            "dark",
        ))
        .build()
}

/// A caged sodium lamp on a bracket, for the tip of a fin: what lights an
/// alley from above a door.
fn wall_lamp() -> Result<Part, ValidationError> {
    Part::builder("city:wall-lamp")
        .element(Element::new(
            "bracket",
            Geometry::union_all([
                Geometry::cuboid([0.06, 0.06, 0.5]).placed(Pose::at([-0.03, 0.3, -0.48])),
                Geometry::cuboid([0.06, 0.36, 0.06]).placed(Pose::at([-0.03, 0.0, -0.05])),
            ]),
            "trim",
        ))
        .element(
            Element::new(
                "lamp",
                Geometry::cuboid([0.22, 0.16, 0.22]).placed(Pose::at([-0.11, 0.1, -0.7])),
                "lamp",
            )
            .standalone()
            .far(),
        )
        .element(Element::new(
            "cage",
            Geometry::union_all([
                Geometry::cuboid([0.3, 0.03, 0.3]).placed(Pose::at([-0.15, 0.27, -0.74])),
                Geometry::cuboid([0.3, 0.03, 0.3]).placed(Pose::at([-0.15, 0.05, -0.74])),
                Geometry::cuboid([0.03, 0.25, 0.03]).placed(Pose::at([-0.15, 0.05, -0.74])),
                Geometry::cuboid([0.03, 0.25, 0.03]).placed(Pose::at([0.12, 0.05, -0.74])),
                Geometry::cuboid([0.03, 0.25, 0.03]).placed(Pose::at([-0.15, 0.05, -0.47])),
                Geometry::cuboid([0.03, 0.25, 0.03]).placed(Pose::at([0.12, 0.05, -0.47])),
            ]),
            "trim",
        ))
        .build()
}

/// A market stall: a counter, a sloped awning on four posts with a neon edge,
/// and a holo menu over the counter. It faces -Z.
fn stall() -> Result<Part, ValidationError> {
    let post = |x: f64, z: f64, height: f64| {
        Geometry::cylinder(0.05, height, 8).placed(Pose::at([x, 0.0, z]))
    };
    Part::builder("city:stall")
        .element(
            Element::new(
                "counter",
                plate([2.4, 1.0, 0.8], [-1.2, 0.0, -0.4]),
                "metal",
            )
            .uv(UvMode::Box)
            .collision(Collision::Bounds),
        )
        .element(Element::new(
            "posts",
            Geometry::union_all([
                post(-1.35, -1.1, 2.35),
                post(1.35, -1.1, 2.35),
                post(-1.35, 0.7, 2.75),
                post(1.35, 0.7, 2.75),
            ]),
            "trim",
        ))
        .element(
            Element::new(
                "awning",
                Geometry::cuboid([3.0, 0.05, 2.1])
                    .placed(Pose::at([-1.5, 2.3, -1.3]).rotated(DQuat::from_rotation_x(-0.2))),
                "awning",
            )
            .uv(UvMode::Box),
        )
        .element(
            Element::new(
                "edge",
                Geometry::cuboid([3.0, 0.05, 0.05]).placed(Pose::at([-1.5, 2.24, -1.36])),
                "neon",
            )
            .standalone()
            .far(),
        )
        .element(
            Element::new(
                "menu",
                Geometry::cuboid([1.6, 0.6, 0.05]).placed(Pose::at([-0.8, 1.45, 0.45])),
                "holo",
            )
            .standalone()
            .far(),
        )
        .element(Element::new(
            "menu-posts",
            Geometry::cuboid([0.04, 0.5, 0.16])
                .placed(Pose::at([-0.74, 0.98, 0.35]))
                .arrayed(2, Pose::at([1.44, 0.0, 0.0])),
            "trim",
        ))
        .build()
}

/// A vending machine with a lit front.
fn vending() -> Result<Part, ValidationError> {
    Part::builder("city:vending")
        .element(
            Element::new("body", plate([1.0, 1.9, 0.8], [-0.5, 0.0, -0.4]), "metal")
                .uv(UvMode::Box)
                .collision(Collision::Bounds),
        )
        .element(
            Element::new(
                "front",
                Geometry::cuboid([0.8, 1.2, 0.02]).placed(Pose::at([-0.4, 0.55, -0.42])),
                "holo",
            )
            .standalone()
            .far(),
        )
        .element(Element::new(
            "slot",
            Geometry::cuboid([0.5, 0.15, 0.03]).placed(Pose::at([-0.25, 0.2, -0.43])),
            "dark",
        ))
        .build()
}

/// A steel bollard with a lit ring.
fn bollard() -> Result<Part, ValidationError> {
    Part::builder("city:bollard")
        .element(
            Element::new(
                "post",
                Geometry::revolve(
                    [
                        [0.0, 0.0],
                        [0.13, 0.0],
                        [0.13, 0.95],
                        [0.08, 1.0],
                        [0.0, 1.0],
                    ],
                    16,
                ),
                "trim",
            )
            .uv(UvMode::Cylindrical { axis: Axis::Y })
            .collision(Collision::Bounds),
        )
        .element(
            Element::new(
                "ring",
                Geometry::cylinder(0.135, 0.04, 12).placed(Pose::at([0.0, 0.8, 0.0])),
                "strip",
            )
            .uv(UvMode::Cylindrical { axis: Axis::Y })
            .standalone()
            .far(),
        )
        .element(Element::new(
            "socket",
            Geometry::cylinder(0.22, 0.075, 16),
            "metal",
        ))
        .element(Element::new(
            "fixings",
            Geometry::union_all([-0.15, 0.15].into_iter().flat_map(|x| {
                [-0.1, 0.1]
                    .map(|z| Geometry::cylinder(0.025, 0.025, 6).placed(Pose::at([x, 0.075, z])))
            })),
            "trim",
        ))
        .build()
}

/// A holo pillar, a night market's middle: a turned plinth, a dark shaft and
/// four holo panels round its top, ringed in neon.
fn holo_pillar() -> Result<Part, ValidationError> {
    let panel = |turn: f64| {
        at(
            Geometry::cuboid([1.6, 2.4, 0.06]).placed(Pose::at([-0.8, 5.0, -0.72])),
            Pose::default().rotated(DQuat::from_rotation_y(turn)),
        )
    };
    Part::builder("city:holo-pillar")
        .element(
            turned(
                "plinth",
                vec![[0.0, 0.0], [1.6, 0.0], [1.6, 0.5], [1.2, 0.7], [0.0, 0.7]],
                "concrete",
            )
            .collision(Collision::Hull),
        )
        .element(
            Element::new(
                "shaft",
                Geometry::cuboid([1.2, 7.4, 1.2]).placed(Pose::at([-0.6, 0.7, -0.6])),
                "dark",
            )
            .uv(UvMode::Box)
            .collision(Collision::Bounds),
        )
        .element(
            Element::new(
                "panels",
                Geometry::union_all([
                    panel(0.0),
                    panel(FRAC_PI_2),
                    panel(std::f64::consts::PI),
                    panel(-FRAC_PI_2),
                ]),
                "holo",
            )
            .standalone()
            .far(),
        )
        .element(
            Element::new(
                "rings",
                Geometry::union_all([
                    Geometry::cylinder(0.95, 0.08, ROUND).placed(Pose::at([0.0, 4.7, 0.0])),
                    Geometry::cylinder(0.95, 0.08, ROUND).placed(Pose::at([0.0, 7.6, 0.0])),
                ]),
                "neon",
            )
            .uv(UvMode::Cylindrical { axis: Axis::Y })
            .standalone()
            .far(),
        )
        .build()
}

/// A loaded delivery pallet: runners, spaced deck boards and banded cases.
fn pallet() -> Result<Part, ValidationError> {
    Part::builder("city:pallet")
        .element(Element::new(
            "runners",
            Geometry::cuboid([0.12, 0.13, 1.0])
                .placed(Pose::at([-0.55, 0.0, -0.5]))
                .arrayed(3, Pose::at([0.48, 0.0, 0.0])),
            "rust",
        ))
        .element(Element::new(
            "boards",
            Geometry::cuboid([1.2, 0.06, 0.14])
                .placed(Pose::at([-0.6, 0.13, -0.5]))
                .arrayed(6, Pose::at([0.0, 0.0, 0.17])),
            "rust",
        ))
        .element(
            Element::new(
                "cases",
                Geometry::union_all([
                    plate([0.54, 0.58, 0.88], [-0.56, 0.19, -0.44]),
                    plate([0.54, 0.8, 0.88], [0.02, 0.19, -0.44]),
                ]),
                "bin",
            )
            .collision(Collision::Bounds),
        )
        .element(Element::new(
            "bands",
            Geometry::union_all([(-0.32, 0.61), (0.25, 0.83)].map(|(x, height)| {
                Geometry::cuboid([0.06, height, 0.92])
                    .placed(Pose::at([x, 0.18, -0.46]))
                    .subtract(
                        Geometry::cuboid([0.1, height - 0.06, 0.84]).placed(Pose::at([
                            x - 0.02,
                            0.21,
                            -0.42,
                        ])),
                    )
            })),
            "trim",
        ))
        .build()
}

/// A sealed drum with rolled hoops and a recessed lid.
fn barrel() -> Result<Part, ValidationError> {
    Part::builder("city:barrel")
        .element(
            Element::new("body", Geometry::cylinder(0.3, 0.87, 16), "bin")
                .uv(UvMode::Cylindrical { axis: Axis::Y })
                .collision(Collision::Hull),
        )
        .element(Element::new(
            "hoops",
            Geometry::union_all([0.04, 0.3, 0.64, 0.84].map(|y| {
                let arc = |radius: f64, step: u32| {
                    let angle = std::f64::consts::PI * f64::from(step) / 8.0;
                    [radius * angle.cos(), radius * angle.sin()]
                };
                let half = Geometry::extrude(
                    (0..=8)
                        .map(|step| arc(0.32, step))
                        .chain((0..=8).rev().map(|step| arc(0.29, step))),
                    0.04,
                );
                Geometry::union_all([
                    half.clone(),
                    half.placed(
                        Pose::default().rotated(DQuat::from_rotation_y(std::f64::consts::PI)),
                    ),
                ])
                .placed(Pose::at([0.0, y, 0.0]))
            })),
            "metal",
        ))
        .element(Element::new(
            "bung",
            Geometry::cylinder(0.055, 0.025, 8).placed(Pose::at([0.13, 0.87, 0.0])),
            "trim",
        ))
        .build()
}

/// A closed service shutter added against a lobby wall, with deep guides and a hood.
fn shutter() -> Result<Part, ValidationError> {
    Part::builder("city:service-shutter")
        .element(Element::new(
            "curtain",
            Geometry::cuboid([2.5, 2.9, 0.075]).placed(Pose::at([-1.25, 0.05, -0.09])),
            "metal",
        ))
        .element(Element::new(
            "slats",
            Geometry::cuboid([2.48, 0.055, 0.045])
                .placed(Pose::at([-1.24, 0.12, -0.13]))
                .arrayed(20, Pose::at([0.0, 0.14, 0.0])),
            "trim",
        ))
        .element(Element::new(
            "guides",
            Geometry::union_all([
                plate([0.14, 3.1, 0.23], [-1.38, 0.0, -0.2]),
                plate([0.14, 3.1, 0.23], [1.24, 0.0, -0.2]),
                plate([2.88, 0.34, 0.44], [-1.44, 2.94, -0.4]),
            ]),
            "rust",
        ))
        .element(Element::new(
            "pull",
            plate([0.4, 0.07, 0.08], [-0.2, 0.45, -0.2]),
            "dark",
        ))
        .build()
}

/// A framed exhaust grille; its blades stand off a dark backing plate.
fn vent() -> Result<Part, ValidationError> {
    Part::builder("city:wall-vent")
        .element(Element::new(
            "back",
            plate([1.5, 0.95, 0.08], [-0.75, 0.0, -0.09]),
            "dark",
        ))
        .element(Element::new(
            "frame",
            Geometry::cuboid([1.62, 1.07, 0.15])
                .placed(Pose::at([-0.81, -0.06, -0.17]))
                .subtract(Geometry::cuboid([1.4, 0.85, 0.3]).placed(Pose::at([-0.7, 0.05, -0.25]))),
            "rust",
        ))
        .element(Element::new(
            "blades",
            plate([1.44, 0.075, 0.17], [-0.72, 0.1, -0.22]).arrayed(6, Pose::at([0.0, 0.13, 0.0])),
            "metal",
        ))
        .build()
}

/// A cast frame and open bars over a shallow drainage pan.
fn steam_grate() -> Result<Part, ValidationError> {
    Part::builder("city:steam-grate")
        .element(Element::new(
            "pan",
            Geometry::cuboid([1.5, 0.025, 0.9]).placed(Pose::at([-0.75, 0.0, -0.45])),
            "dark",
        ))
        .element(Element::new(
            "bars",
            Geometry::cuboid([0.065, 0.045, 0.88])
                .placed(Pose::at([-0.72, 0.025, -0.44]))
                .arrayed(12, Pose::at([0.124, 0.0, 0.0])),
            "metal",
        ))
        .element(Element::new(
            "frame",
            Geometry::union_all([
                Geometry::cuboid([1.6, 0.07, 0.09]).placed(Pose::at([-0.8, 0.0, -0.5])),
                Geometry::cuboid([1.6, 0.07, 0.09]).placed(Pose::at([-0.8, 0.0, 0.41])),
                Geometry::cuboid([0.085, 0.07, 1.0]).placed(Pose::at([-0.8, 0.0, -0.5])),
                Geometry::cuboid([0.085, 0.07, 1.0]).placed(Pose::at([0.715, 0.0, -0.5])),
            ]),
            "metal",
        ))
        .build()
}

/// A pavement news terminal with a deep display hood and a collection tray.
fn newsstand() -> Result<Part, ValidationError> {
    Part::builder("city:newsstand")
        .element(
            Element::new(
                "cabinet",
                plate([1.15, 1.45, 0.7], [-0.575, 0.08, -0.35]),
                "bin",
            )
            .collision(Collision::Bounds),
        )
        .element(Element::new(
            "foot",
            plate([1.25, 0.08, 0.8], [-0.625, 0.0, -0.4]),
            "metal",
        ))
        .element(Element::new(
            "hood",
            Geometry::union_all([
                plate([1.27, 0.12, 0.92], [-0.635, 1.45, -0.54]),
                plate([0.09, 0.73, 0.17], [-0.62, 0.75, -0.5]),
                plate([0.09, 0.73, 0.17], [0.53, 0.75, -0.5]),
            ]),
            "trim",
        ))
        .element(
            Element::new(
                "screen",
                Geometry::cuboid([0.96, 0.59, 0.025]).placed(Pose::at([-0.48, 0.8, -0.38])),
                "holo",
            )
            .far(),
        )
        .element(Element::new(
            "tray",
            plate([0.88, 0.18, 0.08], [-0.44, 0.33, -0.42]),
            "dark",
        ))
        .build()
}

/// Three welded cycle stands on individual anchor shoes, leaving the gaps open.
fn cycle_rack() -> Result<Part, ValidationError> {
    let hoop = Geometry::union_all([
        Geometry::cuboid([0.055, 0.8, 0.055]).placed(Pose::at([0.0, 0.0, -0.4])),
        Geometry::cuboid([0.055, 0.8, 0.055]).placed(Pose::at([0.0, 0.0, 0.35])),
        Geometry::cuboid([0.055, 0.055, 0.8]).placed(Pose::at([0.0, 0.78, -0.4])),
        plate([0.23, 0.04, 0.18], [-0.08, 0.0, -0.45]),
        plate([0.23, 0.04, 0.18], [-0.08, 0.0, 0.3]),
    ]);
    Part::builder("city:cycle-rack")
        .element(Element::new(
            "stands",
            hoop.placed(Pose::at([-0.8, 0.0, 0.0]))
                .arrayed(3, Pose::at([0.8, 0.0, 0.0])),
            "trim",
        ))
        .build()
}

/// A food counter uses the market canopy with a serving shelf, pots and stools.
fn noodle_bar() -> Result<Part, ValidationError> {
    let mut part = stall()?;
    part.id = "city:noodle-bar".into();
    part.elements.extend([
        Element::new(
            "serving-shelf",
            plate([2.65, 0.09, 0.48], [-1.325, 0.99, -0.75]),
            "trim",
        ),
        Element::new(
            "pots",
            Geometry::cylinder(0.22, 0.23, 12)
                .placed(Pose::at([-0.65, 1.0, 0.06]))
                .arrayed(3, Pose::at([0.65, 0.0, 0.0])),
            "trim",
        ),
        Element::new(
            "stool-stems",
            Geometry::cylinder(0.055, 0.66, 8)
                .placed(Pose::at([-0.75, 0.0, -1.25]))
                .arrayed(3, Pose::at([0.75, 0.0, 0.0])),
            "trim",
        ),
        Element::new(
            "stool-feet",
            Geometry::cylinder(0.2, 0.06, 12)
                .placed(Pose::at([-0.75, 0.0, -1.25]))
                .arrayed(3, Pose::at([0.75, 0.0, 0.0])),
            "metal",
        ),
        Element::new(
            "stool-seats",
            Geometry::cylinder(0.24, 0.08, 12)
                .placed(Pose::at([-0.75, 0.66, -1.25]))
                .arrayed(3, Pose::at([0.75, 0.0, 0.0])),
            "bin",
        ),
    ]);
    Ok(part)
}

/// Every part of the alley and market kit.
pub(super) fn parts() -> Result<Vec<Part>, ValidationError> {
    Ok(vec![
        pallet()?,
        barrel()?,
        shutter()?,
        vent()?,
        steam_grate()?,
        newsstand()?,
        cycle_rack()?,
        noodle_bar()?,
        gutter()?,
        dumpster()?,
        trash()?,
        ac_unit()?,
        pipes("city:pipes", super::STOREY)?,
        pipes("city:pipes-lobby", super::LOBBY)?,
        fire_escape()?,
        blade_sign()?,
        cable("city:cable-6", 6.0)?,
        cable("city:cable-10", 10.0)?,
        wall_lamp()?,
        stall()?,
        vending()?,
        bollard()?,
        holo_pillar()?,
    ])
}
