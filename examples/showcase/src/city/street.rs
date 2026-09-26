//! The street kit: carriageways, crossings, kerbed blocks and plazas, and the
//! furniture a street is read by — sodium lamps, holo billboards and signs,
//! benches, tram shelters, parked and flying cars, and the skybridges that
//! join towers.
//!
//! A road segment runs twelve metres along its own +Z, its carriageway from
//! X = -6 to 6, its surface at Y = 0. An intersection and a plaza are centred
//! on their origin. A block's origin is its minimum corner; its main surface
//! is at [`KERB`](super::KERB), with lowered crossing corners. Furniture
//! stands on its origin and faces -Z, which
//! is the way a lamp's arm reaches and a sign reads.
use super::{
    Axis, BLOCK, Collision, DQuat, Element, FRAC_PI_2, Geometry, KERB, PI, Part, Pose, ROAD, ROUND,
    SMALL, UvMode, ValidationError, along_x, at, beacon, plate, yaw,
};

/// Length of one road segment.
pub const SEGMENT: f64 = 12.0;

/// One straight carriageway segment.
fn road() -> Result<Part, ValidationError> {
    let cover = Geometry::cylinder(0.48, 0.025, 24).placed(Pose::at([2.8, 0.002, 6.0]));
    let slots = Geometry::cuboid([0.055, 0.06, 0.58])
        .placed(Pose::at([2.55, 0.012, 5.71]))
        .arrayed(6, Pose::at([0.09, 0.0, 0.0]));
    Part::builder("city:road")
        .element(
            Element::new(
                "surface",
                Geometry::cuboid([ROAD, 0.3, SEGMENT]).placed(Pose::at([-ROAD / 2.0, -0.3, 0.0])),
                "road",
            )
            .uv(UvMode::Box)
            .collision(Collision::Bounds),
        )
        .element(Element::new("cover", cover.subtract(slots), "metal"))
        .element(Element::new(
            "edge-lines",
            Geometry::union_all(
                [-5.5, 5.4]
                    .map(|x| Geometry::cuboid([0.1, 0.012, 8.0]).placed(Pose::at([x, 0.001, 2.0]))),
            ),
            "marking",
        ))
        .build()
}

/// A square of plain asphalt where two carriageways cross, with a drain.
fn intersection() -> Result<Part, ValidationError> {
    Part::builder("city:intersection")
        .element(
            Element::new(
                "surface",
                Geometry::cuboid([ROAD, 0.3, ROAD]).placed(Pose::at([
                    -ROAD / 2.0,
                    -0.3,
                    -ROAD / 2.0,
                ])),
                "asphalt",
            )
            .uv(UvMode::Box)
            .collision(Collision::Bounds),
        )
        .element(Element::new(
            "drain",
            Geometry::cuboid([0.8, 0.01, 0.5]).placed(Pose::at([3.5, 0.0, -5.2])),
            "dark",
        ))
        .build()
}

/// Zebra stripes and a stop line across a carriageway, three metres deep
/// along the road from the origin.
fn crosswalk() -> Result<Part, ValidationError> {
    let stripes = Geometry::cuboid([0.6, 0.015, 2.8])
        .placed(Pose::at([-5.4, 0.0, 0.1]))
        .arrayed(10, Pose::at([1.2, 0.0, 0.0]));
    Part::builder("city:crosswalk")
        .element(Element::new(
            "stripes",
            Geometry::union_all([
                stripes,
                Geometry::cuboid([ROAD - 0.4, 0.015, 0.35]).placed(Pose::at([
                    -ROAD / 2.0 + 0.2,
                    0.0,
                    3.4,
                ])),
            ]),
            "marking",
        ))
        .build()
}

/// A block has lowered crossing corners and sloping approaches back to the
/// pavement. Each approach is its own convex solid, so collision follows the
/// walking surface instead of leaving an invisible full-height kerb.
#[allow(
    clippy::too_many_lines,
    reason = "the paving sectors share one corner and slope contract"
)]
fn block() -> Result<Part, ValidationError> {
    let low = 0.02;
    let corner = 3.2;
    let ramp = 1.5;
    let inner = BLOCK - corner;
    let prism = |x: f64, length: f64, z: f64, depth: f64, near: f64, far: f64| {
        Geometry::hull([
            [x, -0.3, z],
            [x + length, -0.3, z],
            [x, -0.3, z + depth],
            [x + length, -0.3, z + depth],
            [x, near, z],
            [x + length, far, z],
            [x, near, z + depth],
            [x + length, far, z + depth],
        ])
    };
    let outline = [
        [corner + ramp, corner],
        [inner - ramp, corner],
        [inner, corner + ramp],
        [inner, inner - ramp],
        [inner - ramp, inner],
        [corner + ramp, inner],
        [corner, inner - ramp],
        [corner, corner + ramp],
    ];
    let mut part = Part::builder("city:pavement").element(
        Element::new(
            "surface",
            Geometry::extrude(outline, KERB + 0.3).placed(Pose::at([0.0, -0.3, 0.0])),
            "walk",
        )
        .uv(UvMode::Box)
        .collision(Collision::Hull),
    );
    let mut kerbs = Vec::new();
    for (side, (origin, turn)) in [
        ([0.0, 0.0, 0.0], 0.0),
        ([BLOCK, 0.0, 0.0], -FRAC_PI_2),
        ([BLOCK, 0.0, BLOCK], PI),
        ([0.0, 0.0, BLOCK], FRAC_PI_2),
    ]
    .into_iter()
    .enumerate()
    {
        let pose = Pose::at(origin).rotated(yaw(turn));
        let pieces = [
            ("crossing", prism(0.0, corner, 0.0, corner, low, low)),
            ("approach-a", prism(corner, ramp, 0.0, corner, low, KERB)),
            (
                "walk",
                prism(
                    corner + ramp,
                    BLOCK - 2.0 * (corner + ramp),
                    0.0,
                    corner,
                    KERB,
                    KERB,
                ),
            ),
            (
                "approach-b",
                prism(inner - ramp, ramp, 0.0, corner, KERB, low),
            ),
            (
                "inner-approach",
                Geometry::hull([
                    [corner, -0.3, corner],
                    [corner + ramp, -0.3, corner],
                    [corner, -0.3, corner + ramp],
                    [corner, low, corner],
                    [corner + ramp, KERB, corner],
                    [corner, KERB, corner + ramp],
                ]),
            ),
        ];
        for (name, geometry) in pieces {
            part = part.element(
                Element::new(format!("{name}-{side}"), at(geometry, pose), "walk")
                    .uv(UvMode::Box)
                    .collision(Collision::Hull),
            );
        }
        kerbs.push(at(
            Geometry::chamfered_cuboid(
                [BLOCK - 2.0 * (corner + ramp) - 0.04, KERB + 0.05, 0.32],
                0.025,
            )
            .placed(Pose::at([corner + ramp + 0.02, -0.04, -0.04])),
            pose,
        ));
        // The transition stones taper down to the two-centimetre crossing lip.
        for (x, near, far) in [
            (corner, low + 0.01, KERB + 0.01),
            (inner - ramp, KERB + 0.01, low + 0.01),
        ] {
            kerbs.push(at(
                prism(x + 0.02, ramp - 0.04, -0.04, 0.32, near, far)
                    .placed(Pose::at([0.0, 0.01, 0.0])),
                pose,
            ));
        }
    }
    part.element(Element::new("kerb", Geometry::union_all(kerbs), "concrete").uv(UvMode::Box))
        .build()
}

/// A twelve-metre square of plaza paving, laid a finger's depth proud.
fn plaza() -> Result<Part, ValidationError> {
    Part::builder("city:plaza")
        .element(
            Element::new(
                "paving",
                Geometry::cuboid([12.0, 0.02, 12.0]).placed(Pose::at([-6.0, 0.0, -6.0])),
                "plaza",
            )
            .uv(UvMode::Box),
        )
        .build()
}

/// A street lamp: a turned pole and an arm reaching out over the road.
fn lamp() -> Result<Part, ValidationError> {
    Part::builder("city:lamp")
        .element(
            Element::new(
                "pole",
                Geometry::revolve(
                    [
                        [0.0, 0.0],
                        [0.22, 0.0],
                        [0.22, 0.3],
                        [0.09, 0.5],
                        [0.07, 7.0],
                        [0.0, 7.1],
                    ],
                    16,
                ),
                "trim",
            )
            .uv(UvMode::Cylindrical { axis: Axis::Y })
            .collision(Collision::Bounds),
        )
        .element(Element::new(
            "arm",
            Geometry::cuboid([0.08, 0.08, 1.7]).placed(Pose::at([-0.04, 6.85, -1.7])),
            "trim",
        ))
        .element(
            Element::new(
                "head",
                plate([0.5, 0.18, 0.9], [-0.25, 6.67, -2.3]),
                "metal",
            )
            .uv(UvMode::Box),
        )
        .element(
            Element::new(
                "glow",
                Geometry::cuboid([0.4, 0.03, 0.8]).placed(Pose::at([-0.2, 6.64, -2.25])),
                "lamp",
            )
            .standalone()
            .far(),
        )
        .element(Element::new(
            "foot",
            Geometry::cuboid([0.58, 0.09, 0.58]).placed(Pose::at([-0.29, 0.0, -0.29])),
            "metal",
        ))
        .element(Element::new(
            "access",
            plate([0.15, 0.38, 0.06], [-0.075, 0.38, -0.14]),
            "metal",
        ))
        .element(Element::new(
            "head-fins",
            Geometry::cuboid([0.42, 0.055, 0.04])
                .placed(Pose::at([-0.21, 6.83, -2.2]))
                .arrayed(6, Pose::at([0.0, 0.0, 0.13])),
            "trim",
        ))
        .element(Element::new(
            "brace",
            Geometry::cuboid([0.06, 0.06, 1.15])
                .placed(Pose::at([-0.03, 6.38, -0.03]).rotated(DQuat::from_rotation_x(-2.72))),
            "trim",
        ))
        .build()
}

/// A holo billboard on two posts, framed in steel.
fn billboard() -> Result<Part, ValidationError> {
    let (width, height, bottom) = (7.0, 3.6, 5.0);
    let bar = |size: [f64; 3], corner: [f64; 3]| Geometry::cuboid(size).placed(Pose::at(corner));
    Part::builder("city:billboard")
        .element(
            Element::new(
                "posts",
                Geometry::union_all([
                    bar([0.4, bottom, 0.4], [-2.6, 0.0, -0.2]),
                    bar([0.4, bottom, 0.4], [2.2, 0.0, -0.2]),
                ]),
                "trim",
            )
            .collision(Collision::Bounds),
        )
        .element(Element::new(
            "frame",
            Geometry::union_all([
                bar(
                    [width + 0.4, 0.2, 0.3],
                    [-width / 2.0 - 0.2, bottom - 0.2, -0.15],
                ),
                bar(
                    [width + 0.4, 0.2, 0.3],
                    [-width / 2.0 - 0.2, bottom + height, -0.15],
                ),
                bar([0.2, height, 0.3], [-width / 2.0 - 0.2, bottom, -0.15]),
                bar([0.2, height, 0.3], [width / 2.0, bottom, -0.15]),
            ]),
            "trim",
        ))
        .element(
            Element::new(
                "screen",
                bar([width, height, 0.2], [-width / 2.0, bottom, -0.1]),
                "holo",
            )
            .standalone()
            .far(),
        )
        .build()
}

/// A bench of steel slats on two legs.
fn bench() -> Result<Part, ValidationError> {
    let legs = [-0.72, 0.64].map(|x| {
        Geometry::union_all([
            Geometry::cuboid([0.08, 0.47, 0.08]).placed(Pose::at([x, 0.03, -0.15])),
            Geometry::cuboid([0.08, 0.91, 0.08]).placed(Pose::at([x, 0.03, 0.19])),
            plate([0.24, 0.04, 0.55], [x - 0.08, 0.0, -0.25]),
            plate([0.09, 0.06, 0.55], [x, 0.67, -0.28]),
        ])
    });
    Part::builder("city:bench")
        .element(
            Element::new(
                "seat",
                plate([1.8, 0.065, 0.085], [-0.9, 0.43, -0.25])
                    .arrayed(5, Pose::at([0.0, 0.0, 0.105])),
                "metal",
            )
            .uv(UvMode::Box)
            .collision(Collision::Bounds),
        )
        .element(Element::new(
            "back",
            plate([1.8, 0.095, 0.06], [-0.9, 0.61, 0.2]).arrayed(3, Pose::at([0.0, 0.13, 0.0])),
            "metal",
        ))
        .element(Element::new("legs", Geometry::union_all(legs), "trim"))
        .build()
}

/// A tram stop: a plinth, a glass back, a plated roof on four posts, a bench,
/// a holo timetable and a light strip.
fn tram_stop() -> Result<Part, ValidationError> {
    let post = |x: f64, z: f64| Geometry::cylinder(0.07, 2.5, SMALL).placed(Pose::at([x, 0.15, z]));
    Part::builder("city:tram-stop")
        .element(
            Element::new(
                "plinth",
                plate([6.0, 0.15, 2.4], [-3.0, 0.0, -1.2]),
                "concrete",
            )
            .uv(UvMode::Box)
            .collision(Collision::Bounds),
        )
        .element(
            Element::new(
                "back",
                Geometry::cuboid([5.6, 2.3, 0.05]).placed(Pose::at([-2.8, 0.2, 0.95])),
                "glass",
            )
            .standalone()
            .collision(Collision::Bounds),
        )
        .element(
            Element::new("roof", plate([6.4, 0.2, 2.8], [-3.2, 2.62, -1.4]), "hull")
                .uv(UvMode::Box),
        )
        .element(Element::new(
            "posts",
            Geometry::union_all([
                post(-2.9, -1.0),
                post(2.9, -1.0),
                post(-2.9, 1.0),
                post(2.9, 1.0),
            ]),
            "trim",
        ))
        .element(Element::new(
            "bench",
            plate([3.0, 0.08, 0.45], [-1.5, 0.6, 0.35]),
            "metal",
        ))
        .element(
            Element::new(
                "timetable",
                Geometry::cuboid([0.1, 1.6, 1.0]).placed(Pose::at([2.7, 0.7, -0.5])),
                "holo",
            )
            .standalone()
            .far(),
        )
        .element(
            Element::new(
                "glow",
                Geometry::cuboid([5.4, 0.03, 0.1]).placed(Pose::at([-2.7, 2.59, -1.1])),
                "light",
            )
            .standalone()
            .far(),
        )
        .element(Element::new(
            "bench-legs",
            Geometry::cuboid([0.12, 0.45, 0.38])
                .placed(Pose::at([-1.15, 0.15, 0.38]))
                .arrayed(3, Pose::at([1.05, 0.0, 0.0])),
            "trim",
        ))
        .element(Element::new(
            "glazing-frame",
            Geometry::union_all([
                Geometry::cuboid([5.7, 0.07, 0.09]).placed(Pose::at([-2.85, 0.18, 0.93])),
                Geometry::cuboid([5.7, 0.07, 0.09]).placed(Pose::at([-2.85, 2.47, 0.93])),
                Geometry::cuboid([0.065, 2.3, 0.09])
                    .placed(Pose::at([-0.97, 0.2, 0.93]))
                    .arrayed(2, Pose::at([1.9, 0.0, 0.0])),
            ]),
            "trim",
        ))
        .element(Element::new(
            "roof-fascia",
            Geometry::union_all([
                plate([6.46, 0.12, 0.1], [-3.23, 2.68, -1.45]),
                plate([6.46, 0.12, 0.1], [-3.23, 2.68, 1.35]),
            ]),
            "trim",
        ))
        .element(Element::new(
            "route-board",
            plate([1.6, 0.36, 0.12], [-2.3, 2.16, -1.12]),
            "dark",
        ))
        .element(
            Element::new(
                "route-bars",
                Geometry::cuboid([0.62, 0.055, 0.035])
                    .placed(Pose::at([-2.15, 2.28, -1.15]))
                    .arrayed(2, Pose::at([0.0, 0.11, 0.0])),
                "light",
            )
            .far(),
        )
        .build()
}

/// Points on an ellipsoid of `radii` about `centre`, `rings` latitudes: what a
/// hull of a streamlined body is made of.
fn ellipsoid(centre: [f64; 3], radii: [f64; 3], rings: u32) -> Vec<[f64; 3]> {
    let mut points = vec![
        [centre[0], centre[1] - radii[1], centre[2]],
        [centre[0], centre[1] + radii[1], centre[2]],
    ];
    for ring in 1..rings {
        let latitude = -FRAC_PI_2 + PI * f64::from(ring) / f64::from(rings);
        for step in 0..2 * rings {
            let longitude = 2.0 * PI * f64::from(step) / f64::from(2 * rings);
            points.push([
                centre[0] + radii[0] * latitude.cos() * longitude.cos(),
                centre[1] + radii[1] * latitude.sin(),
                centre[2] + radii[2] * latitude.cos() * longitude.sin(),
            ]);
        }
    }
    points
}

/// A pod sleeve is made in two halves so its thin pieces drop at distance.
/// Explicit half profiles keep their bounds tight for the feature-size test.
fn pod_collar() -> Geometry {
    let half = Geometry::extrude(
        (0..=6).map(|step| {
            let angle = PI * f64::from(step) / 6.0;
            [0.3 * angle.cos(), 0.3 * angle.sin()]
        }),
        0.1,
    );
    Geometry::union_all([half.clone(), half.placed(Pose::default().rotated(yaw(PI)))])
        .placed(Pose::default().rotated(DQuat::from_rotation_z(-FRAC_PI_2)))
}

/// A hover car: a streamlined plated body along X, a glass canopy, two
/// thruster pods, head and tail lights. It floats half a metre up.
fn hover_car() -> Result<Part, ValidationError> {
    let lift = 0.55;
    Part::builder("city:hover-car")
        .element(
            Element::new(
                "body",
                Geometry::hull(ellipsoid([0.0, lift + 0.45, 0.0], [2.3, 0.45, 0.95], 6)),
                "hull",
            )
            .uv(UvMode::Box)
            .collision(Collision::Hull),
        )
        .element(
            Element::new(
                "canopy",
                Geometry::hull(ellipsoid([-0.2, lift + 0.8, 0.0], [1.1, 0.42, 0.72], 6)),
                "glass",
            )
            .standalone(),
        )
        .element(Element::new(
            "pods",
            Geometry::union_all([
                at(
                    along_x(0.28, 2.6, SMALL),
                    Pose::at([-1.5, lift + 0.3, 0.95]),
                ),
                at(
                    along_x(0.28, 2.6, SMALL),
                    Pose::at([-1.5, lift + 0.3, -0.95]),
                ),
            ]),
            "trim",
        ))
        .element(
            Element::new(
                "lights",
                Geometry::union_all([
                    Geometry::cuboid([0.05, 0.1, 1.0]).placed(Pose::at([2.10, lift + 0.4, -0.5])),
                    Geometry::cuboid([0.06, 0.12, 1.0]).placed(Pose::at([-2.12, lift + 0.4, -0.5])),
                ]),
                "light",
            )
            .standalone()
            .far(),
        )
        .element(Element::new(
            "cowl",
            Geometry::chamfered_cuboid([1.25, 0.18, 1.32], 0.07)
                .placed(Pose::at([0.55, 1.13, -0.66])),
            "hull",
        ))
        .element(Element::new(
            "bumper",
            plate([0.24, 0.2, 1.35], [1.87, 0.83, -0.675]),
            "trim",
        ))
        .element(Element::new(
            "intakes",
            Geometry::union_all(
                [-0.95, 0.95].map(|z| at(along_x(0.215, 0.05, 12), Pose::at([1.1, 0.85, z]))),
            ),
            "dark",
        ))
        .element(Element::new(
            "pod-collars",
            Geometry::union_all(
                [-0.95, 0.95]
                    .into_iter()
                    .flat_map(|z| [-1.35, 0.85].map(|x| at(pod_collar(), Pose::at([x, 0.85, z])))),
            ),
            "metal",
        ))
        .element(Element::new(
            "door-sills",
            Geometry::union_all([-0.83, 0.76].map(|z| plate([1.6, 0.1, 0.07], [-0.95, 1.03, z]))),
            "metal",
        ))
        .element(Element::new(
            "intake-slats",
            Geometry::union_all([-0.95, 0.95].map(|z| {
                Geometry::cuboid([0.04, 0.035, 0.32])
                    .placed(Pose::at([1.15, 0.75, z - 0.16]))
                    .arrayed(3, Pose::at([0.0, 0.085, 0.0]))
            })),
            "trim",
        ))
        .element(Element::new(
            "rear-bumper",
            plate([0.28, 0.22, 1.22], [-2.1, 0.86, -0.61]),
            "metal",
        ))
        .build()
}

/// Length of a skybridge: an eighteen-metre street and a metre into each tower.
pub const SKYBRIDGE: f64 = 20.0;

/// An enclosed skybridge along +X: a plated tube with a window band each
/// side, ribs, a deck and a light strip down its floor.
fn skybridge() -> Result<Part, ValidationError> {
    let radius = 2.0;
    let band =
        |z: f64| Geometry::cuboid([SKYBRIDGE - 2.0, 0.9, 0.8]).placed(Pose::at([1.0, 0.1, z]));
    Part::builder("city:skybridge")
        .element(
            Element::new(
                "tube",
                along_x(radius, SKYBRIDGE, ROUND).subtract(Geometry::union_all([
                    band(-radius - 0.3),
                    band(radius - 0.5),
                ])),
                "hull",
            )
            .cut_material("trim")
            .uv(UvMode::Cylindrical { axis: Axis::X }),
        )
        .element(
            Element::new(
                "windows",
                Geometry::union_all([
                    Geometry::cuboid([SKYBRIDGE - 2.1, 0.86, 0.04]).placed(Pose::at([
                        1.05,
                        0.12,
                        -radius + 0.25,
                    ])),
                    Geometry::cuboid([SKYBRIDGE - 2.1, 0.86, 0.04]).placed(Pose::at([
                        1.05,
                        0.12,
                        radius - 0.29,
                    ])),
                ]),
                "glass",
            )
            .standalone(),
        )
        .element(
            Element::new(
                "ribs",
                at(
                    along_x(radius + 0.08, 0.2, ROUND),
                    Pose::at([0.9, 0.0, 0.0]),
                )
                .arrayed(4, Pose::at([(SKYBRIDGE - 2.0) / 3.0, 0.0, 0.0])),
                "trim",
            )
            .uv(UvMode::Cylindrical { axis: Axis::X }),
        )
        .element(
            Element::new(
                "deck",
                Geometry::cuboid([SKYBRIDGE - 0.4, 0.1, 2.6]).placed(Pose::at([0.2, -1.3, -1.3])),
                "deck",
            )
            .uv(UvMode::Box)
            .interior()
            .collision(Collision::Bounds),
        )
        .element(
            Element::new(
                "floor-light",
                Geometry::cuboid([SKYBRIDGE - 1.0, 0.02, 0.1]).placed(Pose::at([0.5, -1.2, -0.05])),
                "light",
            )
            .interior()
            .standalone(),
        )
        .build()
}

/// A vertical holo sign on a steel frame, for the face of a tower.
fn facade_sign() -> Result<Part, ValidationError> {
    let (width, height) = (1.6, 10.0);
    Part::builder("city:facade-sign")
        .element(Element::new(
            "frame",
            Geometry::chamfered_cuboid([width + 0.3, height + 0.3, 0.3], 0.05).placed(Pose::at([
                -width / 2.0 - 0.15,
                -0.15,
                0.0,
            ])),
            "trim",
        ))
        .element(
            Element::new(
                "screen",
                Geometry::cuboid([width, height, 0.1]).placed(Pose::at([-width / 2.0, 0.0, -0.08])),
                "holo",
            )
            .standalone()
            .far(),
        )
        .element(beacon("cap", 0.12, [0.0, height + 0.35, 0.15]))
        .build()
}

/// Every part of the street kit.
pub(super) fn parts() -> Result<Vec<Part>, ValidationError> {
    Ok(vec![
        road()?,
        intersection()?,
        crosswalk()?,
        block()?,
        plaza()?,
        lamp()?,
        billboard()?,
        bench()?,
        tram_stop()?,
        hover_car()?,
        skybridge()?,
        facade_sign()?,
    ])
}

/// A rotation that points a piece's -Z at `direction` on X/Z.
pub(super) fn facing(direction: [f64; 2]) -> DQuat {
    // `yaw(a)` sends -Z to (-sin a, 0, -cos a).
    yaw((-direction[0]).atan2(-direction[1]))
}
