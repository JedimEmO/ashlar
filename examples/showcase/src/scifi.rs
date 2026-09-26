//! A frontier sci-fi kit in the idiom of Anarchy Online and Star Wars Galaxies:
//! rounded, lathe-turned architecture — adobe domes, capsule habitats on
//! stilts, a flared hab tower, tanks, moisture vaporators — beside the
//! chamfered prefab modules and tube walkways of a corporate colony.
//!
//! Every piece is a reusable [`Part`] on semantic slots (`adobe`, `hull`,
//! `trim`, `deck`, `glass`, `light`, `dark`, `concrete`, `rust`, `marking`,
//! `ground`, `corrugated`, `paving`, `awning`, and inside `spine`, `floor` and
//! `cushion`, with `neon` and `warning` accents), and `PALETTE` binds those slots to the default material
//! library. A building varies a piece by overriding a slot's graph parameters
//! on an instance — a faction's hull colour, a weathered hut — never by
//! copying geometry.
//!
//! The enterable buildings — domes, the dome house, the pod and its tube, the
//! module and the tower — are in [`habitat`], built the way the house in
//! `interior.rs` is: added shells, interior finishes, floors, ceilings in the
//! storey above, rooms and portals, furnished from [`furniture`]. Each is
//! placed with [`Habitat::place`], which adds its parts, furniture and rooms
//! to a building, and a camera cutting storeys sees into every one of them.
//!
//! The shapes lean on [`Geometry::revolve`]: a profile of `[radius, height]`
//! points turned about the part's Y axis. Round pieces keep that axis at the
//! part origin, which is what lets `UvMode::Cylindrical` lay their material
//! metre-true round the circumference. A capsule lies along X with its axis
//! through the origin for the same reason.
#![allow(
    clippy::too_many_lines,
    reason = "a piece is one builder chain, read top to bottom; splitting it hides the piece"
)]
use std::f64::consts::{FRAC_PI_2, PI, TAU};

use ashlar::{
    Axis, Binding, Building, BuildingBuilder, Collision, Element, Geometry, Instance, ParamValue,
    Part, Pose, Socket, UvMode, ValidationError,
};
use glam::{DQuat, DVec3};

pub mod furniture;
pub mod habitat;
pub use habitat::{Habitat, Look};

/// Segments round a large lathe-turned body.
const ROUND: u32 = 48;
/// Segments round a small one: a pipe, a pole, a porthole.
const SMALL: u32 = 20;

/// Slot to library key for every piece of the kit.
pub(crate) const PALETTE: [(&str, &str); 19] = [
    ("adobe", "library:adobe"),
    ("hull", "library:hull-plating"),
    ("trim", "showcase:dark-metal"),
    ("deck", "library:tread-plate"),
    ("glass", "showcase:window-cold"),
    ("light", "library:emissive-strip"),
    ("dark", "library:dark-recess"),
    ("concrete", "showcase:stained-concrete"),
    ("rust", "library:rusted-steel"),
    ("marking", "library:signage-ink"),
    ("ground", "library:desert-sand"),
    ("corrugated", "library:corrugated-steel"),
    ("spine", "library:interior-panelling"),
    ("floor", "library:tread-plate"),
    ("cushion", "library:signage-ink"),
    ("awning", "library:signage-ink"),
    ("paving", "library:paving-slabs"),
    ("neon", "showcase:neon-cyan"),
    ("warning", "showcase:neon-amber"),
];

// ------------------------------------------------------------------ helpers

/// Points along an elliptical arc about `centre`, from angle `from` to `to`
/// (radians, anticlockwise from +X), both ends included.
fn arc(centre: [f64; 2], radii: [f64; 2], from: f64, to: f64, steps: u32) -> Vec<[f64; 2]> {
    (0..=steps)
        .map(|step| {
            let angle = from + (to - from) * f64::from(step) / f64::from(steps);
            [
                snap(centre[0] + radii[0] * angle.cos()),
                snap(centre[1] + radii[1] * angle.sin()),
            ]
        })
        .collect()
}

/// Round away the float dust a cosine leaves at a right angle, so a profile
/// point meant to lie on the axis lies on it exactly.
fn snap(value: f64) -> f64 {
    (value * 1e9).round() / 1e9
}

/// A lathe-turned element on the part's Y axis, mapped round its circumference.
fn turned(id: &str, profile: Vec<[f64; 2]>, slot: &str) -> Element {
    Element::new(id, Geometry::revolve(profile, ROUND), slot)
        .uv(UvMode::Cylindrical { axis: Axis::Y })
}

/// A rotation about Y by `angle` radians.
fn yaw(angle: f64) -> DQuat {
    DQuat::from_rotation_y(angle)
}

/// A pose that is `local` swung round the part's Y axis by `angle`.
fn around(angle: f64, local: Pose) -> Pose {
    let turn = yaw(angle);
    Pose::at((turn * local.translation).to_array()).rotated(turn * local.rotation)
}

/// A cylinder lying along -Z from the origin: what bores a porthole or a pipe
/// into a face that looks down -Z.
fn bore(radius: f64, depth: f64, segments: u32) -> Geometry {
    Geometry::cylinder(radius, depth, segments)
        .placed(Pose::default().rotated(DQuat::from_rotation_x(-FRAC_PI_2)))
}

/// A cylinder lying along +X from the origin.
fn along_x(radius: f64, length: f64, segments: u32) -> Geometry {
    Geometry::cylinder(radius, length, segments)
        .placed(Pose::default().rotated(DQuat::from_rotation_z(-FRAC_PI_2)))
}

/// Move a geometry by `pose`, after the placement it already has.
fn at(mut geometry: Geometry, pose: Pose) -> Geometry {
    geometry.pose = pose.compose(geometry.pose);
    geometry
}

/// An arched opening in X/Y, `width` across and `height` to the crown,
/// extruded `depth` along -Z from the origin: a doorway cutter or a door leaf.
fn arch(width: f64, height: f64, depth: f64) -> Geometry {
    let radius = width / 2.0;
    let spring = height - radius;
    let mut profile = vec![[-radius, 0.0], [radius, 0.0]];
    profile.extend(arc([0.0, spring], [radius, radius], 0.0, PI, 12));
    // The extrusion runs along Y; a quarter turn about X lays the profile's
    // second coordinate up the wall and the depth along -Z.
    Geometry::extrude(profile, depth)
        .placed(Pose::default().rotated(DQuat::from_rotation_x(-FRAC_PI_2)))
}

/// A box by its minimum corner, chamfered when it is big enough to read one.
fn plate(size: [f64; 3], at: [f64; 3]) -> Geometry {
    let smallest = size.into_iter().fold(f64::INFINITY, f64::min);
    let bevel = (smallest * 0.2).min(0.04);
    if bevel >= 0.005 {
        Geometry::chamfered_cuboid(size, bevel).placed(Pose::at(at))
    } else {
        Geometry::cuboid(size).placed(Pose::at(at))
    }
}

/// A luminous strip has square edges inside its housing. Bevels on a
/// glowing face do not read, and keeping twelve triangles matters when the
/// strip must survive every level of detail.
fn luminous(size: [f64; 3], at: [f64; 3]) -> Geometry {
    Geometry::cuboid(size).placed(Pose::at(at))
}

/// A warning lens with a bevelled cap and enough sides for its small silhouette.
fn beacon(id: &str, radius: f64, at: [f64; 3]) -> Element {
    Element::new(
        id,
        Geometry::revolve(
            [
                [0.0, -radius],
                [radius, -radius * 0.5],
                [radius, radius * 0.5],
                [0.0, radius],
            ],
            12,
        )
        .placed(Pose::at(at)),
        "warning",
    )
    .far()
}

/// A recessed service cover. Its raised lip and inset centre cast a readable
/// shadow without making a paper-thin decal or cutting the structural wall.
fn service_cover(at_: [f64; 3], width: f64, height: f64) -> Geometry {
    plate([width, height, 0.12], at_).subtract(
        Geometry::cuboid([width - 0.16, height - 0.16, 0.08]).placed(Pose::at([
            at_[0] + 0.08,
            at_[1] + 0.08,
            at_[2] - 0.02,
        ])),
    )
}

/// A straight brace between two joints, with a little overlap at both ends.
fn brace(a: [f64; 3], b: [f64; 3], radius: f64) -> Geometry {
    let a = DVec3::from_array(a);
    let delta = DVec3::from_array(b) - a;
    Geometry::cylinder(radius, delta.length() + 0.04, 8).placed(
        Pose::at((a - delta.normalize() * 0.02).to_array())
            .rotated(DQuat::from_rotation_arc(DVec3::Y, delta.normalize())),
    )
}

/// A framed blade with a corporate three-bar emblem, looking along -Z.
/// Brackets reach back to the wall; the luminous strokes stay at every LOD.
fn blade(mut part: ashlar::PartBuilder, at_: [f64; 3]) -> ashlar::PartBuilder {
    let pose = Pose::at(at_);
    part = part
        .element(Element::new(
            "blade-frame",
            at(plate([0.64, 1.8, 0.22], [0.0, 0.0, 0.0]), pose),
            "trim",
        ))
        .element(Element::new(
            "blade-brackets",
            at(
                Geometry::union_all([
                    plate([0.16, 0.12, 0.7], [0.24, 0.18, 0.1]),
                    plate([0.16, 0.12, 0.7], [0.24, 1.48, 0.1]),
                ]),
                pose,
            ),
            "trim",
        ));
    part.element(
        Element::new(
            "blade-emblem",
            at(
                Geometry::union_all([
                    luminous([0.1, 1.4, 0.04], [0.1, 0.2, -0.03]),
                    luminous([0.27, 0.12, 0.04], [0.22, 1.35, -0.03]),
                    luminous([0.27, 0.12, 0.04], [0.22, 0.8, -0.03]),
                    luminous([0.27, 0.12, 0.04], [0.22, 0.25, -0.03]),
                ]),
                pose,
            ),
            "neon",
        )
        .far(),
    )
}

// ------------------------------------------------------------------ pieces

/// A moisture vaporator: a turned steel column on three splayed legs, finned
/// at the condenser and ringed with a light where the collector sits.
///
/// # Errors
///
/// If the part does not validate.
pub(crate) fn vaporator() -> Result<Part, ValidationError> {
    let column = vec![
        [0.0, 0.0],
        [0.42, 0.0],
        [0.42, 0.3],
        [0.27, 0.42],
        [0.27, 2.6],
        [0.42, 2.72],
        [0.42, 3.1],
        [0.22, 3.3],
        [0.22, 4.4],
        [0.34, 4.5],
        [0.34, 4.62],
        [0.1, 4.8],
        [0.0, 4.8],
    ];
    let third = Pose::default().rotated(yaw(TAU / 3.0));
    Part::builder("scifi:vaporator")
        .element(turned("column", column, "trim").collision(Collision::Hull))
        .element(Element::new(
            "fins",
            Geometry::cuboid([0.04, 1.9, 0.34])
                .placed(Pose::at([-0.02, 0.55, 0.2]))
                .arrayed(6, Pose::default().rotated(yaw(TAU / 6.0))),
            "trim",
        ))
        .element(Element::new(
            "legs",
            Geometry::chamfered_cuboid([0.1, 0.1, 1.3], 0.02)
                .placed(Pose::at([-0.05, 1.0, 0.2]).rotated(DQuat::from_rotation_x(0.62)))
                .arrayed(3, third),
            "rust",
        ))
        .element(Element::new(
            "feet",
            Geometry::cylinder(0.22, 0.08, SMALL)
                .placed(Pose::at([0.0, 0.0, 1.12]))
                .arrayed(3, third),
            "rust",
        ))
        .element(
            Element::new(
                "collector-light",
                Geometry::cylinder(0.435, 0.07, ROUND).placed(Pose::at([0.0, 2.88, 0.0])),
                "light",
            )
            .uv(UvMode::Cylindrical { axis: Axis::Y }),
        )
        .element(Element::new(
            "antenna",
            Geometry::cylinder(0.02, 1.1, 8).placed(Pose::at([0.0, 4.8, 0.0])),
            "trim",
        ))
        .element(beacon("tip", 0.06, [0.0, 5.92, 0.0]))
        // The farmer's side: a control box with a lit readout, a valve wheel
        // on the collector, and a drain pipe down to a drum at the foot.
        .element(Element::new(
            "control-box",
            plate([0.36, 0.5, 0.22], [-0.18, 1.3, -0.49]),
            "hull",
        ))
        .element(Element::new(
            "readout",
            luminous([0.22, 0.12, 0.02], [-0.11, 1.62, -0.505]),
            "light",
        ))
        .element(Element::new(
            "valve",
            at(
                Geometry::cylinder(0.14, 0.03, SMALL).subtract(
                    Geometry::cylinder(0.1, 0.1, SMALL).placed(Pose::at([0.0, -0.03, 0.0])),
                ),
                Pose::at([0.3, 2.9, -0.47]).rotated(DQuat::from_rotation_x(FRAC_PI_2)),
            ),
            "rust",
        ))
        .element(Element::new(
            "drain",
            Geometry::union_all([
                at(along_x(0.04, 0.5, 8), Pose::at([0.4, 2.76, 0.0])),
                Geometry::cylinder(0.04, 2.36, 8).placed(Pose::at([0.88, 0.42, 0.0])),
            ]),
            "trim",
        ))
        .element(
            Element::new(
                "drum",
                Geometry::revolve([[0.0, 0.0], [0.24, 0.0], [0.24, 0.5], [0.0, 0.5]], SMALL)
                    .placed(Pose::at([0.88, 0.005, 0.0])),
                "rust",
            )
            .uv(UvMode::Cylindrical { axis: Axis::Y }),
        )
        .element(Element::new(
            "drain-couplings",
            Geometry::cylinder(0.075, 0.12, 12)
                .placed(Pose::at([0.88, 0.55, 0.0]))
                .arrayed(2, Pose::at([0.0, 1.65, 0.0])),
            "rust",
        ))
        .element(Element::new(
            "collector-collar",
            Geometry::cylinder(0.47, 0.12, SMALL).placed(Pose::at([0.0, 3.04, 0.0])),
            "rust",
        ))
        .build()
}

/// A storage tank on a plinth: a domed steel drum with ribs, a ladder and a
/// valve run.
///
/// # Errors
///
/// If the part does not validate.
pub(crate) fn tank() -> Result<Part, ValidationError> {
    let mut drum = vec![[0.0, 0.4], [2.2, 0.4], [2.2, 5.0]];
    drum.extend(
        arc([0.0, 5.0], [2.2, 1.0], 0.0, FRAC_PI_2, 10)
            .into_iter()
            .skip(1),
    );
    let rungs = Geometry::cuboid([0.5, 0.04, 0.04])
        .placed(Pose::at([-0.25, 0.8, -2.42]))
        .arrayed(14, Pose::at([0.0, 0.35, 0.0]));
    let rails = Geometry::union_all([
        Geometry::cuboid([0.05, 5.4, 0.05]).placed(Pose::at([-0.3, 0.4, -2.44])),
        Geometry::cuboid([0.05, 5.4, 0.05]).placed(Pose::at([0.25, 0.4, -2.44])),
    ]);
    Part::builder("scifi:tank")
        .element(
            turned(
                "plinth",
                vec![[0.0, 0.0], [2.6, 0.0], [2.6, 0.3], [2.4, 0.42], [0.0, 0.42]],
                "concrete",
            )
            .collision(Collision::Hull),
        )
        .element(turned("drum", drum, "trim").collision(Collision::Hull))
        .element(
            Element::new(
                "ribs",
                Geometry::cylinder(2.26, 0.12, ROUND)
                    .placed(Pose::at([0.0, 1.4, 0.0]))
                    .arrayed(3, Pose::at([0.0, 1.2, 0.0])),
                "rust",
            )
            .uv(UvMode::Cylindrical { axis: Axis::Y }),
        )
        .element(Element::new(
            "ladder",
            Geometry::union_all([rungs, rails]),
            "rust",
        ))
        .element(Element::new(
            "pipe",
            Geometry::union_all([
                at(along_x(0.16, 3.0, SMALL), Pose::at([2.1, 1.0, 0.0])),
                Geometry::cylinder(0.16, 1.0, SMALL).placed(Pose::at([4.95, 0.0, 0.0])),
                Geometry::cylinder(0.3, 0.08, SMALL).placed(Pose::at([4.95, 0.35, 0.0])),
            ]),
            "trim",
        ))
        .element(Element::new(
            "hatch",
            Geometry::cylinder(0.45, 0.18, SMALL).placed(Pose::at([0.0, 5.9, 0.0])),
            "rust",
        ))
        .element(Element::new(
            "ladder-brackets",
            Geometry::union_all([0.8, 2.8, 4.8].map(|y| {
                Geometry::union_all(
                    [-0.28, 0.27].map(|x| plate([0.07, 0.09, 0.35], [x, y + 0.02, -2.45])),
                )
            })),
            "trim",
        ))
        .element(Element::new(
            "pipe-couplings",
            at(along_x(0.23, 0.13, SMALL), Pose::at([2.5, 1.0, 0.0]))
                .arrayed(2, Pose::at([1.8, 0.0, 0.0])),
            "rust",
        ))
        .element(Element::new(
            "hatch-rim",
            Geometry::cylinder(0.51, 0.08, SMALL).placed(Pose::at([0.0, 5.95, 0.0])),
            "trim",
        ))
        .build()
}

/// Length of a straight wall segment.
pub(crate) const WALL_LENGTH: f64 = 6.0;

/// A battered perimeter wall: an adobe section that leans back as it rises,
/// capped with a steel coping, `WALL_LENGTH` along +X between `left` and
/// `right` sockets.
///
/// # Errors
///
/// If the part does not validate.
pub(crate) fn wall() -> Result<Part, ValidationError> {
    // The section in [height, depth] pairs, stood up by a quarter turn about Z
    // so the extrusion runs along -X, then moved to span 0..length.
    let section = [[0.0, -0.7], [0.0, 0.7], [3.0, 0.42], [3.0, -0.42]];
    let body = Geometry::extrude(section, WALL_LENGTH)
        .placed(Pose::at([WALL_LENGTH, 0.0, 0.0]).rotated(DQuat::from_rotation_z(FRAC_PI_2)));
    Part::builder("scifi:wall")
        .element(
            Element::new("body", body, "adobe")
                .uv(UvMode::Box)
                .collision(Collision::Bounds),
        )
        .element(Element::new(
            "coping",
            plate([WALL_LENGTH, 0.18, 1.0], [0.0, 3.0, -0.5]),
            "trim",
        ))
        .element(Element::new(
            "vents",
            Geometry::cuboid([0.5, 0.14, 0.06])
                .placed(Pose::at([0.75, 0.9, -0.66]))
                .arrayed(3, Pose::at([2.0, 0.0, 0.0])),
            "dark",
        ))
        .socket(Socket::new(
            "left",
            Pose::default().rotated(yaw(-FRAC_PI_2)),
        ))
        .socket(Socket::new(
            "right",
            Pose::at([WALL_LENGTH, 0.0, 0.0]).rotated(yaw(FRAC_PI_2)),
        ))
        .element(Element::new(
            "buttresses",
            Geometry::chamfered_cuboid([0.25, 2.95, 1.46], 0.06)
                .placed(Pose::at([0.25, 0.02, -0.73]))
                .arrayed(2, Pose::at([5.25, 0.0, 0.0])),
            "adobe",
        ))
        .element(Element::new(
            "coping-drip",
            plate([5.9, 0.08, 1.08], [0.05, 3.06, -0.54]),
            "trim",
        ))
        .build()
}

/// A round pylon for a wall's corners and ends, ringed with a light.
///
/// # Errors
///
/// If the part does not validate.
pub(crate) fn wall_post() -> Result<Part, ValidationError> {
    let profile = vec![
        [0.0, 0.0],
        [1.05, 0.0],
        [0.95, 0.4],
        [0.8, 3.3],
        [0.95, 3.45],
        [0.95, 3.8],
        [0.55, 4.25],
        [0.0, 4.35],
    ];
    Part::builder("scifi:wall-post")
        .element(turned("body", profile, "adobe").collision(Collision::Hull))
        .element(
            Element::new(
                "band",
                Geometry::cylinder(0.97, 0.1, ROUND).placed(Pose::at([0.0, 3.55, 0.0])),
                "light",
            )
            .uv(UvMode::Cylindrical { axis: Axis::Y }),
        )
        .socket(Socket::new(
            "a",
            Pose::at([-0.8, 0.0, 0.0]).rotated(yaw(-FRAC_PI_2)),
        ))
        .socket(Socket::new(
            "b",
            Pose::at([0.8, 0.0, 0.0]).rotated(yaw(FRAC_PI_2)),
        ))
        .element(Element::new(
            "cap-collar",
            Geometry::revolve(
                [[0.75, 3.38], [1.01, 3.38], [1.01, 3.49], [0.75, 3.49]],
                ROUND,
            ),
            "trim",
        ))
        .build()
}

/// Width of the gate between its sockets.
pub(crate) const GATE_WIDTH: f64 = 6.0;

/// A blast gate between two tall pylons, under a plated lintel.
///
/// # Errors
///
/// If the part does not validate.
pub(crate) fn gate() -> Result<Part, ValidationError> {
    let pylon = vec![
        [0.0, 0.0],
        [1.25, 0.0],
        [1.1, 0.5],
        [0.95, 5.4],
        [1.15, 5.6],
        [1.15, 6.1],
        [0.6, 6.7],
        [0.0, 6.8],
    ];
    let pylon_at = |x: f64| Geometry::revolve(pylon.clone(), ROUND).placed(Pose::at([x, 0.0, 0.0]));
    Part::builder("scifi:gate")
        .element(
            Element::new("pylon-left", pylon_at(0.9), "adobe")
                .uv(UvMode::Box)
                .collision(Collision::Hull),
        )
        .element(
            Element::new("pylon-right", pylon_at(GATE_WIDTH - 0.9), "adobe")
                .uv(UvMode::Box)
                .collision(Collision::Hull),
        )
        .element(
            Element::new(
                "lintel",
                plate([GATE_WIDTH - 2.0, 1.1, 1.4], [1.0, 4.7, -0.7]),
                "hull",
            )
            .uv(UvMode::Box),
        )
        .element(
            Element::new(
                "leaves",
                Geometry::union_all([
                    plate([1.87, 4.6, 0.3], [1.1, 0.05, -0.15]),
                    plate([1.87, 4.6, 0.3], [3.03, 0.05, -0.15]),
                ]),
                "hull",
            )
            .uv(UvMode::Box)
            .collision(Collision::Bounds),
        )
        .element(Element::new(
            "seam",
            plate([0.06, 4.5, 0.34], [2.97, 0.1, -0.17]),
            "trim",
        ))
        .element(Element::new(
            "warning-light",
            luminous([GATE_WIDTH - 2.4, 0.1, 0.1], [1.2, 4.55, -0.76]),
            "light",
        ))
        .socket(Socket::new(
            "left",
            Pose::default().rotated(yaw(-FRAC_PI_2)),
        ))
        .socket(Socket::new(
            "right",
            Pose::at([GATE_WIDTH, 0.0, 0.0]).rotated(yaw(FRAC_PI_2)),
        ))
        .element(Element::new(
            "door-stiffeners",
            Geometry::union_all([1.32, 3.28].map(|x| {
                plate([1.38, 0.15, 0.12], [x, 0.65, -0.23]).arrayed(3, Pose::at([0.0, 1.3, 0.0]))
            })),
            "trim",
        ))
        .element(Element::new(
            "corporate-header",
            service_cover([1.55, 4.96, -0.79], 2.9, 0.55),
            "trim",
        ))
        .element(
            Element::new(
                "header-bars",
                luminous([0.65, 0.08, 0.04], [1.86, 5.17, -0.82])
                    .arrayed(3, Pose::at([0.81, 0.0, 0.0])),
                "neon",
            )
            .far(),
        )
        .build()
}

/// Radius of the landing pad's deck.
pub(crate) const PAD_RADIUS: f64 = 8.6;

/// An octagonal landing pad: a concrete plinth, a tread-plate deck, a painted
/// ring and sixteen edge lights, with a ramp down to the ground.
///
/// # Errors
///
/// If the part does not validate.
pub(crate) fn landing_pad() -> Result<Part, ValidationError> {
    let base = vec![
        [0.0, 0.0],
        [PAD_RADIUS + 0.5, 0.0],
        [PAD_RADIUS + 0.5, 0.5],
        [PAD_RADIUS + 0.1, 0.85],
        [0.0, 0.85],
    ];
    // A wedge whose profile is [height, run], stood up by a quarter turn about
    // Z so it runs out along -Z from the deck edge and spans X from -1.5 to 1.5.
    let ramp = Geometry::extrude([[0.0, 0.0], [0.85, 0.0], [0.0, -5.0]], 3.0)
        .placed(Pose::at([1.5, 0.0, -PAD_RADIUS + 0.3]).rotated(DQuat::from_rotation_z(FRAC_PI_2)));
    Part::builder("scifi:landing-pad")
        .element(
            Element::new("plinth", Geometry::revolve(base, 8), "concrete")
                .uv(UvMode::Box)
                .collision(Collision::Hull),
        )
        .element(
            Element::new(
                "deck",
                Geometry::cylinder(PAD_RADIUS, 0.04, 8).placed(Pose::at([0.0, 0.85, 0.0])),
                "deck",
            )
            .uv(UvMode::Box),
        )
        .element(
            Element::new(
                "ring",
                Geometry::cylinder(4.3, 0.02, ROUND)
                    .subtract(
                        Geometry::cylinder(3.9, 0.1, ROUND).placed(Pose::at([0.0, -0.04, 0.0])),
                    )
                    .placed(Pose::at([0.0, 0.89, 0.0])),
                "marking",
            )
            .uv(UvMode::Box),
        )
        .element(Element::new(
            "edge-lights",
            Geometry::cuboid([0.3, 0.1, 0.16])
                .placed(Pose::at([-0.15, 0.85, -PAD_RADIUS - 0.25]))
                .arrayed(16, Pose::default().rotated(yaw(TAU / 16.0))),
            "light",
        ))
        .element(
            Element::new("ramp", ramp, "deck")
                .uv(UvMode::Box)
                .collision(Collision::Hull),
        )
        .element(Element::new(
            "edge-casings",
            plate([0.46, 0.13, 0.32], [-0.23, 0.8, -PAD_RADIUS - 0.33])
                .arrayed(16, Pose::default().rotated(yaw(TAU / 16.0))),
            "trim",
        ))
        .element(Element::new(
            "touchdown-bars",
            Geometry::union_all([-1.4, 1.0].map(|x| plate([0.4, 0.025, 3.0], [x, 0.90, -1.5])))
                .union(plate([2.4, 0.025, 0.4], [-1.2, 0.90, -0.2])),
            "marking",
        ))
        .element(Element::new(
            "apron-joints",
            plate([0.08, 0.015, 3.2], [-0.04, 0.895, 4.6])
                .arrayed(8, Pose::default().rotated(yaw(TAU / 8.0))),
            "dark",
        ))
        .build()
}

/// A plated cargo crate, strapped at its edges.
///
/// # Errors
///
/// If the part does not validate.
pub(crate) fn crate_box() -> Result<Part, ValidationError> {
    Part::builder("scifi:crate")
        .element(
            Element::new(
                "body",
                Geometry::chamfered_cuboid([1.2, 1.0, 1.0], 0.08),
                "hull",
            )
            .uv(UvMode::Box)
            .collision(Collision::Bounds),
        )
        .element(Element::new(
            "straps",
            Geometry::union_all([
                plate([0.12, 1.04, 1.04], [0.15, -0.02, -0.02]),
                plate([0.12, 1.04, 1.04], [0.93, -0.02, -0.02]),
            ]),
            "trim",
        ))
        .element(Element::new(
            "label",
            luminous([0.5, 0.18, 0.02], [0.35, 0.62, -0.03]),
            "light",
        ))
        .element(Element::new(
            "lift-pockets",
            Geometry::union_all([0.36, 0.77].map(|x| service_cover([x, 0.13, -0.06], 0.24, 0.21))),
            "trim",
        ))
        .build()
}

/// A lamp post: a turned pole under a glowing head.
///
/// # Errors
///
/// If the part does not validate.
pub(crate) fn lamp() -> Result<Part, ValidationError> {
    let pole = vec![
        [0.0, 0.0],
        [0.28, 0.0],
        [0.28, 0.15],
        [0.1, 0.35],
        [0.07, 3.6],
        [0.3, 3.8],
        [0.3, 3.95],
        [0.0, 4.05],
    ];
    Part::builder("scifi:lamp")
        .element(
            Element::new("pole", Geometry::revolve(pole, 16), "trim")
                .uv(UvMode::Cylindrical { axis: Axis::Y })
                .collision(Collision::Bounds),
        )
        .element(
            Element::new(
                "glow",
                Geometry::cylinder(0.27, 0.1, SMALL).placed(Pose::at([0.0, 3.7, 0.0])),
                "light",
            )
            .uv(UvMode::Cylindrical { axis: Axis::Y }),
        )
        .element(Element::new(
            "head-shield",
            Geometry::revolve(
                [
                    [0.08, 3.85],
                    [0.45, 3.85],
                    [0.45, 3.91],
                    [0.2, 4.1],
                    [0.08, 4.1],
                ],
                SMALL,
            ),
            "trim",
        ))
        .element(Element::new(
            "access-panel",
            service_cover([-0.1, 0.35, -0.12], 0.2, 0.4),
            "trim",
        ))
        .build()
}

/// A fuel barrel: a turned drum with two rolling hoops.
///
/// # Errors
///
/// If the part does not validate.
pub(crate) fn barrel() -> Result<Part, ValidationError> {
    let drum = vec![
        [0.0, 0.0],
        [0.3, 0.0],
        [0.3, 0.02],
        [0.32, 0.04],
        [0.32, 0.86],
        [0.3, 0.88],
        [0.3, 0.9],
        [0.0, 0.9],
    ];
    Part::builder("scifi:barrel")
        .element(turned("drum", drum, "rust").collision(Collision::Hull))
        .element(
            Element::new(
                "hoops",
                Geometry::cylinder(0.335, 0.04, SMALL)
                    .placed(Pose::at([0.0, 0.28, 0.0]))
                    .arrayed(2, Pose::at([0.0, 0.3, 0.0])),
                "trim",
            )
            .uv(UvMode::Cylindrical { axis: Axis::Y }),
        )
        .element(Element::new(
            "filler",
            Geometry::cylinder(0.075, 0.035, 8).placed(Pose::at([0.12, 0.89, 0.06])),
            "trim",
        ))
        .build()
}

/// A dish antenna on a lattice mast.
///
/// # Errors
///
/// If the part does not validate.
pub(crate) fn antenna() -> Result<Part, ValidationError> {
    let mut dish = vec![[0.0, 0.0]];
    dish.extend(
        arc([0.0, 2.2], [2.2, 2.2], -FRAC_PI_2, -0.5, 8)
            .into_iter()
            .skip(1),
    );
    let lip = dish.last().copied().unwrap_or([1.9, 1.0]);
    dish.push([lip[0] - 0.08, lip[1] + 0.05]);
    dish.extend(
        arc([0.0, 2.25], [2.12, 2.12], -0.5, -FRAC_PI_2 + 0.08, 8)
            .into_iter()
            .skip(1),
    );
    dish.push([0.0, 0.14]);
    let leg = |angle: f64| {
        Geometry::chamfered_cuboid([0.14, 5.0, 0.14], 0.02).placed(around(
            angle,
            Pose::at([-0.07, 0.0, -0.9]).rotated(DQuat::from_rotation_x(0.16)),
        ))
    };
    Part::builder("scifi:antenna")
        .element(
            Element::new(
                "mast",
                Geometry::union_all([leg(0.0), leg(TAU / 3.0), leg(2.0 * TAU / 3.0)]),
                "rust",
            )
            .collision(Collision::Hull),
        )
        .element(Element::new(
            "dish",
            at(
                Geometry::revolve(dish, 36),
                Pose::at([0.0, 5.6, 0.0]).rotated(DQuat::from_rotation_x(-1.0)),
            ),
            "hull",
        ))
        .element(Element::new(
            "feed",
            Geometry::cylinder(0.05, 1.6, 8)
                .placed(Pose::at([0.0, 5.7, 0.0]).rotated(DQuat::from_rotation_x(-1.0))),
            "trim",
        ))
        .element(beacon("beacon", 0.1, [0.0, 5.3, 0.0]))
        .element(Element::new(
            "mast-bracing",
            Geometry::union_all((0..3).flat_map(|side| {
                let turn = yaw(f64::from(side) * TAU / 3.0);
                [0.45, 1.8, 3.15].map(move |y| {
                    let radius = 0.9 - y * 0.16;
                    let a = turn * DVec3::new(0.0, y, -radius);
                    let b = yaw(TAU / 3.0) * DVec3::new(0.0, y + 1.25, -(radius - 0.2));
                    brace(a.to_array(), (turn * b).to_array(), 0.045)
                })
            })),
            "trim",
        ))
        .element(Element::new(
            "gimbal",
            Geometry::cylinder(0.2, 1.1, SMALL).placed(Pose::at([0.0, 4.6, 0.0])),
            "trim",
        ))
        .element(Element::new(
            "mast-feet",
            Geometry::cylinder(0.27, 0.12, 12)
                .placed(Pose::at([0.0, 0.0, -0.9]))
                .arrayed(3, Pose::default().rotated(yaw(TAU / 3.0))),
            "concrete",
        ))
        .build()
}

/// Length and radius of the quonset hangar.
const HANGAR_LENGTH: f64 = 14.0;
const HANGAR_RADIUS: f64 = 5.5;

/// A half-cylinder of `radius` about the X axis, standing on Y = 0 and running
/// from X = 0 to `length`, as an extrusion: the profile is `[height, depth]`
/// pairs stood up by a quarter turn about Z.
fn vault(radius: f64, length: f64, steps: u32) -> Geometry {
    let profile: Vec<[f64; 2]> = (0..=steps)
        .map(|step| {
            let angle = PI * f64::from(step) / f64::from(steps);
            [snap(radius * angle.sin()), snap(radius * angle.cos())]
        })
        .collect();
    Geometry::extrude(profile, length)
        .placed(Pose::at([length, 0.0, 0.0]).rotated(DQuat::from_rotation_z(FRAC_PI_2)))
}

/// A quonset hangar: a corrugated half-cylinder on a concrete apron, ribbed,
/// with a big segmented door in its +X end and a personnel door in its flank.
/// Its axis is the part's X axis at ground level, so the corrugation is laid
/// round the vault.
///
/// # Errors
///
/// If the part does not validate.
pub(crate) fn hangar() -> Result<Part, ValidationError> {
    let (l, r) = (HANGAR_LENGTH, HANGAR_RADIUS);
    let doorway =
        Geometry::chamfered_cuboid([1.2, 4.2, 7.0], 0.1).placed(Pose::at([l - 0.6, 0.0, -3.5]));
    let side_door =
        Geometry::chamfered_cuboid([1.1, 2.2, 1.2], 0.08).placed(Pose::at([2.0, 0.0, -r - 0.6]));
    let body = vault(r, l, 32).subtract(Geometry::union_all([doorway, side_door]));
    let mut rib = vec![];
    for step in 0..=24 {
        let angle = PI * f64::from(step) / 24.0;
        rib.push([snap((r + 0.1) * angle.sin()), snap((r + 0.1) * angle.cos())]);
    }
    for step in (0..=24).rev() {
        let angle = PI * f64::from(step) / 24.0;
        rib.push([
            snap((r - 0.05) * angle.sin()),
            snap((r - 0.05) * angle.cos()),
        ]);
    }
    let ribs = Geometry::extrude(rib, 0.2)
        // Extruded along -X from its placement, so this first rib spans 0.1 to
        // 0.3 and stands a centimetre up: no face on the vault's end or foot.
        .placed(Pose::at([0.3, 0.01, 0.0]).rotated(DQuat::from_rotation_z(FRAC_PI_2)))
        .arrayed(5, Pose::at([(l - 0.4) / 4.0, 0.0, 0.0]));
    Part::builder("scifi:hangar")
        .element(
            Element::new("vault", body, "corrugated")
                .cut_material("dark")
                .uv(UvMode::Cylindrical { axis: Axis::X })
                .collision(Collision::Hull),
        )
        .element(Element::new("ribs", ribs, "trim"))
        .element(
            Element::new(
                "apron",
                plate([l + 4.0, 0.2, 2.0 * r + 2.0], [-1.0, -0.19, -r - 1.0]),
                "concrete",
            )
            .uv(UvMode::Box),
        )
        .element(
            Element::new(
                "door",
                Geometry::union_all((0..4).map(|leaf| {
                    plate(
                        [0.14, 4.1, 1.72],
                        [l - 0.44, 0.05, -3.45 + 1.73 * f64::from(leaf)],
                    )
                })),
                "hull",
            )
            .collision(Collision::Bounds),
        )
        .element(Element::new(
            "door-lights",
            luminous([0.1, 0.12, 6.6], [l + 0.02, 4.4, -3.3]),
            "light",
        ))
        .element(Element::new(
            "side-door",
            plate([1.0, 2.15, 0.08], [2.05, 0.02, -r + 0.52]),
            "hull",
        ))
        .element(Element::new(
            "vent",
            Geometry::cylinder(0.45, 1.0, SMALL).placed(Pose::at([l * 0.3, r - 0.3, 0.0])),
            "rust",
        ))
        .element(Element::new(
            "door-tracks",
            Geometry::union_all([
                plate([0.28, 4.35, 0.24], [l - 0.16, 0.03, -3.69]),
                plate([0.28, 4.35, 0.24], [l - 0.16, 0.03, 3.45]),
                plate([0.4, 0.32, 7.62], [l - 0.22, 4.22, -3.81]),
            ]),
            "trim",
        ))
        .element(Element::new(
            "door-battens",
            plate([0.1, 0.1, 6.7], [l - 0.32, 0.7, -3.35]).arrayed(4, Pose::at([0.0, 0.85, 0.0])),
            "trim",
        ))
        .element(Element::new(
            "rib-shoes",
            Geometry::union_all([-r, r - 0.3].map(|z| {
                plate([0.5, 0.45, 0.3], [0.02, 0.02, z])
                    .arrayed(5, Pose::at([(l - 0.4) / 4.0, 0.0, 0.0]))
            })),
            "trim",
        ))
        .element(Element::new(
            "vent-cowl",
            Geometry::cylinder(0.62, 0.14, SMALL).placed(Pose::at([l * 0.3, r + 0.65, 0.0])),
            "trim",
        ))
        .build()
}

/// Length of a force-fence panel between its sockets.
pub(crate) const FENCE_LENGTH: f64 = 4.0;

/// A force fence: two turned posts with three glowing beams between them.
///
/// # Errors
///
/// If the part does not validate.
pub(crate) fn force_fence() -> Result<Part, ValidationError> {
    let post = vec![
        [0.0, 0.0],
        [0.32, 0.0],
        [0.32, 0.2],
        [0.18, 0.35],
        [0.16, 2.6],
        [0.26, 2.75],
        [0.26, 2.95],
        [0.0, 3.05],
    ];
    let post_at = |x: f64| Geometry::revolve(post.clone(), SMALL).placed(Pose::at([x, 0.0, 0.0]));
    Part::builder("scifi:force-fence")
        .element(
            Element::new(
                "posts",
                Geometry::union_all([post_at(0.0), post_at(FENCE_LENGTH)]),
                "trim",
            )
            .collision(Collision::Bounds),
        )
        .element(
            Element::new(
                "beams",
                Geometry::cuboid([FENCE_LENGTH - 0.3, 0.05, 0.05])
                    .placed(Pose::at([0.15, 0.6, -0.025]))
                    .arrayed(3, Pose::at([0.0, 0.75, 0.0])),
                "light",
            )
            .collision(Collision::Bounds),
        )
        .element(Element::new(
            "emitters",
            Geometry::cuboid([0.12, 1.8, 0.12])
                .placed(Pose::at([0.12, 0.45, -0.06]))
                .arrayed(2, Pose::at([FENCE_LENGTH - 0.36, 0.0, 0.0])),
            "dark",
        ))
        .socket(Socket::new(
            "left",
            Pose::default().rotated(yaw(-FRAC_PI_2)),
        ))
        .socket(Socket::new(
            "right",
            Pose::at([FENCE_LENGTH, 0.0, 0.0]).rotated(yaw(FRAC_PI_2)),
        ))
        .element(Element::new(
            "foot-collars",
            Geometry::union_all(
                [0.0, FENCE_LENGTH].map(|x| {
                    Geometry::cylinder(0.36, 0.12, SMALL).placed(Pose::at([x, 0.06, 0.0]))
                }),
            ),
            "concrete",
        ))
        .build()
}

/// A power generator: a glowing core in a cage of bars between a turned base
/// and a finned cap.
///
/// # Errors
///
/// If the part does not validate.
pub(crate) fn generator() -> Result<Part, ValidationError> {
    let base = vec![
        [0.0, 0.0],
        [1.8, 0.0],
        [1.8, 0.4],
        [1.4, 0.8],
        [1.4, 1.1],
        [0.0, 1.1],
    ];
    let cap = vec![
        [0.0, 3.9],
        [1.4, 3.9],
        [1.6, 4.2],
        [1.6, 4.5],
        [0.9, 5.0],
        [0.4, 5.1],
        [0.0, 5.1],
    ];
    Part::builder("scifi:generator")
        .element(turned("base", base, "hull").collision(Collision::Hull))
        .element(turned("cap", cap, "hull"))
        .element(
            Element::new(
                "core",
                Geometry::cylinder(0.7, 2.8, ROUND).placed(Pose::at([0.0, 1.1, 0.0])),
                "light",
            )
            .uv(UvMode::Cylindrical { axis: Axis::Y })
            .collision(Collision::Bounds),
        )
        .element(Element::new(
            "cage",
            Geometry::chamfered_cuboid([0.14, 2.8, 0.14], 0.03)
                .placed(Pose::at([-0.07, 1.1, -1.15]))
                .arrayed(10, Pose::default().rotated(yaw(TAU / 10.0))),
            "trim",
        ))
        .element(
            Element::new(
                "bands",
                Geometry::cylinder(1.25, 0.12, ROUND)
                    .subtract(
                        Geometry::cylinder(1.08, 0.4, ROUND).placed(Pose::at([0.0, -0.1, 0.0])),
                    )
                    .placed(Pose::at([0.0, 1.8, 0.0]))
                    .arrayed(2, Pose::at([0.0, 1.3, 0.0])),
                "rust",
            )
            .uv(UvMode::Cylindrical { axis: Axis::Y }),
        )
        .element(Element::new(
            "fins",
            Geometry::cuboid([0.06, 0.7, 0.6])
                .placed(Pose::at([-0.03, 4.25, -1.9]))
                .arrayed(12, Pose::default().rotated(yaw(TAU / 12.0))),
            "trim",
        ))
        .element(Element::new(
            "conduits",
            Geometry::union_all([
                at(along_x(0.14, 2.2, SMALL), Pose::at([1.6, 0.35, 0.6])),
                at(along_x(0.14, 2.2, SMALL), Pose::at([1.6, 0.35, -0.6])),
            ]),
            "rust",
        ))
        .element(Element::new(
            "service-hatch",
            service_cover([-0.4, 0.22, -1.75], 0.8, 0.45),
            "trim",
        ))
        .element(Element::new(
            "pipe-joints",
            Geometry::union_all(
                [-0.6, 0.6].map(|z| at(along_x(0.21, 0.16, 12), Pose::at([3.4, 0.35, z]))),
            ),
            "trim",
        ))
        .build()
}

/// A vendor kiosk: a plated booth with a serving hatch, an awning and a lit
/// sign on its roof.
///
/// # Errors
///
/// If the part does not validate.
pub(crate) fn kiosk() -> Result<Part, ValidationError> {
    let body = Geometry::chamfered_cuboid([3.0, 2.6, 2.2], 0.25).subtract(
        Geometry::chamfered_cuboid([2.2, 1.0, 0.8], 0.08).placed(Pose::at([0.4, 1.0, -0.4])),
    );
    blade(Part::builder("scifi:kiosk"), [3.3, 1.25, -0.55])
        .element(
            Element::new("booth", body, "hull")
                .cut_material("dark")
                .uv(UvMode::Box)
                .collision(Collision::Bounds),
        )
        .element(Element::new(
            "counter",
            plate([2.4, 0.08, 0.6], [0.3, 0.98, -0.35]),
            "trim",
        ))
        .element(Element::new(
            "awning",
            at(
                plate([3.4, 0.12, 1.4], [-0.2, 0.0, -1.3]),
                Pose::at([0.0, 2.35, 0.25]).rotated(DQuat::from_rotation_x(-0.25)),
            ),
            "adobe",
        ))
        .element(Element::new(
            "sign",
            luminous([2.2, 0.6, 0.12], [0.4, 2.75, 0.6]),
            "light",
        ))
        .element(Element::new(
            "sign-frame",
            plate([2.4, 0.12, 0.2], [0.3, 2.6, 0.56]),
            "trim",
        ))
        .element(Element::new(
            "sign-stanchions",
            plate([0.12, 0.28, 0.14], [0.5, 2.56, 0.59]).arrayed(2, Pose::at([1.88, 0.0, 0.0])),
            "trim",
        ))
        .element(Element::new(
            "menu-bars",
            plate([1.4, 0.07, 0.035], [0.8, 2.87, 0.565]).arrayed(3, Pose::at([0.0, 0.15, 0.0])),
            "trim",
        ))
        .element(Element::new(
            "counter-brackets",
            Geometry::union_all([0.6, 2.2].map(|x| brace([x, 0.6, 0.05], [x, 0.98, -0.28], 0.045))),
            "trim",
        ))
        .element(Element::new(
            "blade-wall-brackets",
            plate([0.98, 0.12, 0.14], [2.8, 1.43, 0.12]).arrayed(2, Pose::at([0.0, 1.3, 0.0])),
            "trim",
        ))
        .build()
}

/// Length of a conduit run between its sockets.
pub(crate) const CONDUIT_LENGTH: f64 = 6.0;

/// A pipe run: two flanged pipes on a pair of A-frame trestles, chainable end
/// to end.
///
/// # Errors
///
/// If the part does not validate.
pub(crate) fn conduit() -> Result<Part, ValidationError> {
    let trestle = |x: f64| {
        Geometry::union_all([
            Geometry::chamfered_cuboid([0.12, 1.5, 0.12], 0.02)
                .placed(Pose::at([x - 0.06, 0.0, -0.5]).rotated(DQuat::from_rotation_x(0.3))),
            Geometry::chamfered_cuboid([0.12, 1.5, 0.12], 0.02)
                .placed(Pose::at([x - 0.06, 0.0, 0.38]).rotated(DQuat::from_rotation_x(-0.3))),
            Geometry::chamfered_cuboid([0.2, 0.1, 1.0], 0.02).placed(Pose::at([
                x - 0.1,
                1.2,
                -0.5,
            ])),
        ])
    };
    Part::builder("scifi:conduit")
        .element(
            Element::new(
                "pipes",
                Geometry::union_all([
                    at(
                        along_x(0.2, CONDUIT_LENGTH, SMALL),
                        Pose::at([0.0, 1.5, -0.25]),
                    ),
                    at(
                        along_x(0.14, CONDUIT_LENGTH, SMALL),
                        Pose::at([0.0, 1.45, 0.25]),
                    ),
                ]),
                "trim",
            )
            .collision(Collision::Bounds),
        )
        .element(Element::new(
            "flanges",
            Geometry::union_all([
                at(along_x(0.27, 0.1, SMALL), Pose::at([0.02, 1.5, -0.25])),
                at(along_x(0.2, 0.1, SMALL), Pose::at([0.02, 1.45, 0.25])),
            ])
            .arrayed(2, Pose::at([CONDUIT_LENGTH / 2.0, 0.0, 0.0])),
            "rust",
        ))
        .element(Element::new(
            "trestles",
            Geometry::union_all([trestle(1.0), trestle(CONDUIT_LENGTH - 1.0)]),
            "rust",
        ))
        .socket(Socket::new("a", Pose::default().rotated(yaw(-FRAC_PI_2))))
        .socket(Socket::new(
            "b",
            Pose::at([CONDUIT_LENGTH, 0.0, 0.0]).rotated(yaw(FRAC_PI_2)),
        ))
        .element(Element::new(
            "saddles",
            Geometry::union_all([1.0, 5.0].map(|x| {
                Geometry::union_all([
                    at(along_x(0.24, 0.18, SMALL), Pose::at([x - 0.09, 1.5, -0.25])),
                    at(along_x(0.18, 0.18, SMALL), Pose::at([x - 0.09, 1.45, 0.25])),
                ])
            })),
            "trim",
        ))
        .build()
}

/// A flat rectangle of ground from `min` to `max` on X/Z, with its top at
/// Y = 0: each scene lays one just larger than what stands on it, so a capture
/// framed to the scene's bounds frames the buildings rather than the desert.
///
/// # Errors
///
/// If the part does not validate.
pub(crate) fn ground(min: [f64; 2], max: [f64; 2]) -> Result<Part, ValidationError> {
    Part::builder("scifi:ground")
        .element(
            Element::new(
                "sand",
                Geometry::cuboid([max[0] - min[0], 0.3, max[1] - min[1]])
                    .placed(Pose::at([min[0], -0.3, min[1]])),
                "ground",
            )
            .uv(UvMode::Box),
        )
        .build()
}

/// A strip of paving laid on the ground, two metres wide and four long along
/// +Z from its origin: the paths between a settlement's doors.
///
/// # Errors
///
/// If the part does not validate.
pub(crate) fn path() -> Result<Part, ValidationError> {
    Part::builder("scifi:path")
        .element(
            Element::new(
                "slabs",
                Geometry::cuboid([2.0, 0.08, 0.98])
                    .placed(Pose::at([-1.0, -0.02, 0.01]))
                    .arrayed(4, Pose::at([0.0, 0.0, 1.0])),
                "paving",
            )
            .uv(UvMode::Box),
        )
        .build()
}

/// A shade awning: a canvas sheet pitched on four poles, for a market stall
/// or a yard.
///
/// # Errors
///
/// If the part does not validate.
pub(crate) fn awning() -> Result<Part, ValidationError> {
    let poles = Geometry::union_all(
        [
            (-1.4, -1.1, 2.5),
            (1.4, -1.1, 2.5),
            (-1.4, 1.1, 2.1),
            (1.4, 1.1, 2.1),
        ]
        .map(|(x, z, height): (f64, f64, f64)| {
            Geometry::cylinder(0.05, height, 8).placed(Pose::at([x, 0.0, z]))
        }),
    );
    let sheet = Geometry::chamfered_cuboid([3.2, 0.04, 2.6], 0.01)
        .placed(Pose::at([-1.6, 2.28, -1.3]).rotated(DQuat::from_rotation_x(-0.18)));
    Part::builder("scifi:awning")
        .element(Element::new("poles", poles, "rust"))
        .element(Element::new("sheet", sheet, "awning"))
        .element(Element::new(
            "crossbars",
            Geometry::union_all(
                [-1.1, 1.1]
                    .map(|z| brace([-1.45, 2.3 - 0.18 * z, z], [1.45, 2.3 - 0.18 * z, z], 0.045)),
            ),
            "trim",
        ))
        .element(Element::new(
            "footplates",
            Geometry::union_all([-1.4, 1.4].into_iter().flat_map(|x| {
                [-1.1, 1.1].map(move |z| plate([0.26, 0.06, 0.26], [x - 0.13, 0.0, z - 0.13]))
            })),
            "trim",
        ))
        .build()
}

/// A lit sign on a post: a glowing panel in a steel frame.
///
/// # Errors
///
/// If the part does not validate.
pub(crate) fn sign_post() -> Result<Part, ValidationError> {
    Part::builder("scifi:sign-post")
        .element(Element::new(
            "post",
            Geometry::cylinder(0.07, 2.6, SMALL),
            "trim",
        ))
        .element(Element::new(
            "frame",
            plate([1.3, 0.8, 0.12], [-0.65, 2.3, -0.06]),
            "trim",
        ))
        .element(Element::new(
            "panel",
            luminous([1.18, 0.68, 0.02], [-0.59, 2.36, -0.08]),
            "light",
        ))
        .element(Element::new(
            "glyphs",
            plate([0.18, 0.36, 0.025], [-0.4, 2.5, -0.1]).arrayed(3, Pose::at([0.3, 0.0, 0.0])),
            "marking",
        ))
        .build()
}

/// Two shared utility packages give habitats different silhouettes without
/// copying their shells or changing the room and portal layout.
fn utility_pack(solar: bool) -> Result<Part, ValidationError> {
    let mut part = Part::builder(if solar {
        "scifi:solar-pack"
    } else {
        "scifi:cooling-pack"
    })
    .element(Element::new(
        "skid",
        plate([1.0, 0.16, 1.1], [-0.5, 0.0, -0.55]),
        "trim",
    ));
    if solar {
        part = part.element(Element::new(
            "rack",
            Geometry::union_all([
                brace([-0.35, 0.08, -0.38], [-0.35, 0.75, 0.38], 0.05),
                brace([0.35, 0.08, -0.38], [0.35, 0.75, 0.38], 0.05),
                brace([-0.35, 0.08, 0.38], [-0.35, 0.75, 0.38], 0.05),
                brace([0.35, 0.08, 0.38], [0.35, 0.75, 0.38], 0.05),
            ]),
            "trim",
        ));
        let tilt = Pose::at([0.0, 0.4, 0.0]).rotated(DQuat::from_rotation_x(-0.65));
        part = part
            .element(Element::new(
                "panel-frame",
                at(plate([1.16, 0.08, 1.2], [-0.58, 0.0, -0.6]), tilt),
                "trim",
            ))
            .element(Element::new(
                "cells",
                at(
                    Geometry::cuboid([0.3, 0.025, 1.04])
                        .placed(Pose::at([-0.48, 0.085, -0.52]))
                        .arrayed(3, Pose::at([0.33, 0.0, 0.0])),
                    tilt,
                ),
                "dark",
            ));
    } else {
        part = part
            .element(Element::new(
                "housing",
                plate([0.88, 0.8, 0.94], [-0.44, 0.13, -0.47]),
                "hull",
            ))
            .element(Element::new(
                "grille",
                plate([0.65, 0.56, 0.035], [-0.325, 0.25, -0.49]),
                "dark",
            ))
            .element(Element::new(
                "louvres",
                plate([0.7, 0.05, 0.09], [-0.35, 0.28, -0.52])
                    .arrayed(5, Pose::at([0.0, 0.1, 0.0])),
                "trim",
            ))
            .element(Element::new(
                "exhaust",
                Geometry::cylinder(0.19, 0.35, 12).placed(Pose::at([0.0, 0.9, 0.1])),
                "rust",
            ));
    }
    part.build()
}

/// Stable mixing makes one habitat's finish independent of how many props
/// another habitat happens to place. It also keeps the palette finite.
fn variation(seed: u64, name: &str) -> usize {
    let mut value = seed ^ 0xcbf2_9ce4_8422_2325;
    for byte in name.bytes() {
        value = (value ^ u64::from(byte)).wrapping_mul(0x100_0000_01b3);
    }
    ((value ^ (value >> 32)) & 0xffff) as usize
}

/// Dress placed habitat shells with shared equipment and a finite palette.
/// No shell is duplicated, and every accessory is outside the room volume.
fn settlement(builder: BuildingBuilder, seed: u64) -> Result<Building, ValidationError> {
    let mut recipe = builder.build()?.into_recipe();
    let mut equipment = Vec::new();
    for instance in &recipe.instances {
        let anchor = match instance.part.as_str() {
            "scifi:dome-hut" => [3.8, 0.0, 0.0],
            "scifi:module" => [4.4, 3.9, 3.3],
            "scifi:hab-pod" => [1.5, -habitat::POD_AXIS, 1.8],
            _ => continue,
        };
        let choice = variation(seed, &instance.id);
        equipment.push(
            Instance::new(
                format!("{}-utility", instance.id),
                if choice.is_multiple_of(2) {
                    "scifi:solar-pack"
                } else {
                    "scifi:cooling-pack"
                },
            )
            .placed(instance.pose.compose(Pose::at(anchor))),
        );
    }
    recipe.instances.extend(equipment);
    recipe.build()
}

/// The pieces that are one part each and not entered: props, walls, set
/// pieces and ground.
///
/// # Errors
///
/// If a part does not validate.
pub(crate) fn simple_parts() -> Result<Vec<Part>, ValidationError> {
    Ok(vec![
        vaporator()?,
        tank()?,
        wall()?,
        wall_post()?,
        gate()?,
        landing_pad()?,
        crate_box()?,
        lamp()?,
        barrel()?,
        antenna()?,
        hangar()?,
        force_fence()?,
        generator()?,
        kiosk()?,
        conduit()?,
        path()?,
        awning()?,
        sign_post()?,
        utility_pack(false)?,
        utility_pack(true)?,
    ])
}

/// Every part of the kit — the simple pieces, the habitats' shells and their
/// furniture — so a scene can register what it uses and a test can check all
/// of them.
///
/// # Errors
///
/// If a part does not validate.
pub fn parts() -> Result<Vec<Part>, ValidationError> {
    let mut parts = simple_parts()?;
    parts.extend(habitat::parts()?);
    for part in &mut parts {
        for element in &mut part.elements {
            if matches!(element.material_slot.as_str(), "light" | "neon" | "warning") {
                *element = element.clone().far();
            }
        }
    }
    parts.push(ground([-10.0, -10.0], [10.0, 10.0])?);
    Ok(parts)
}

/// Register every part, a ground from `min` to `max`, the storey groups, and
/// bind the palette.
fn kit(id: &str, min: [f64; 2], max: [f64; 2]) -> Result<BuildingBuilder, ValidationError> {
    let mut builder = habitat::declare_groups(Building::builder(id).part(ground(min, max)?));
    for part in parts()? {
        if part.id != "scifi:ground" {
            builder = builder.part(part);
        }
    }
    for (slot, key) in PALETTE {
        builder = builder.material(slot, key);
    }
    // A hangar's sheeting has weathered, not rusted through; cushions are a
    // worn blue-grey, and the awnings a sun-faded red.
    Ok(builder
        .binding(
            "hull",
            Binding::new("library:hull-plating")
                .param("color", ParamValue::Color([0.13, 0.17, 0.19]))
                .param("wear", ParamValue::Float(0.45)),
        )
        .binding(
            "corrugated",
            Binding::new("library:corrugated-steel").param("age", ParamValue::Float(0.3)),
        )
        .binding(
            "cushion",
            Binding::new("library:signage-ink")
                .param("color", ParamValue::Color([0.16, 0.2, 0.24])),
        )
        .binding(
            "awning",
            Binding::new("library:signage-ink")
                .param("color", ParamValue::Color([0.42, 0.14, 0.06])),
        ))
}

/// The names [`piece`] accepts: every habitat, and every single-part piece
/// and piece of furniture.
#[must_use]
pub fn piece_names() -> Vec<String> {
    let mut names: Vec<String> = Habitat::ALL
        .iter()
        .map(|habitat| habitat.name().to_owned())
        .collect();
    let singles = simple_parts()
        .unwrap_or_default()
        .into_iter()
        .chain(furniture::parts().unwrap_or_default());
    names.extend(singles.map(|part| part.id.trim_start_matches("scifi:").to_owned()));
    names
}

/// One piece of the kit alone on a patch of ground just larger than it: the
/// catalogue entry a reviewer frames to look at the piece itself, inside and
/// out.
///
/// # Errors
///
/// If `name` is not one of [`piece_names`], or the recipe does not validate.
pub fn piece(name: &str) -> Result<Building, ValidationError> {
    if let Some(habitat) = Habitat::named(name) {
        let (min, max) = habitat.footprint();
        let builder = kit(&format!("scifi:piece-{name}"), min, max)?
            .instance(Instance::new("ground", "scifi:ground"));
        return habitat
            .place(builder, name, Pose::default(), Look::default())
            .build();
    }
    let id = format!("scifi:{name}");
    let part = parts()?
        .into_iter()
        .find(|part| part.id == id)
        .ok_or_else(|| ValidationError {
            path: "parts".into(),
            reason: format!("no piece {name}"),
        })?;
    let [low, high] = part
        .elements
        .iter()
        .filter_map(|element| element.geometry.bounds())
        .reduce(|[a, b], [c, d]| [a.min(c), b.max(d)])
        .unwrap_or([DVec3::splat(-1.0), DVec3::splat(1.0)]);
    let margin = 1.5;
    let lift = -low.y.min(0.0);
    kit(
        &format!("scifi:piece-{name}"),
        [low.x - margin, low.z - margin],
        [high.x + margin, high.z + margin],
    )?
    .instance(Instance::new("ground", "scifi:ground"))
    .instance(Instance::new(name, id).placed(Pose::at([0.0, lift, 0.0])))
    .build()
}

/// A pose on the ground plane at `(x, z)`, turned `yaw` radians about Y.
fn spot(x: f64, z: f64, turn: f64) -> Pose {
    Pose::at([x, 0.0, z]).rotated(yaw(turn))
}

/// Every piece of the kit in rows, on bare ground: the catalogue.
///
/// # Errors
///
/// If the assembled recipe does not validate.
pub fn kit_scene() -> Result<Building, ValidationError> {
    let mut builder = kit("scifi:kit", [-46.0, -16.0], [44.0, 20.0])?
        .instance(Instance::new("ground", "scifi:ground"));
    for (habitat, x, z) in [
        (Habitat::DomeHut, -38.0, -8.0),
        (Habitat::DomeAnnex, -30.0, -8.0),
        (Habitat::DomeTall, -20.0, -8.0),
        (Habitat::Tower, -7.0, -8.0),
        (Habitat::Module, -44.0, 8.0),
    ] {
        builder = habitat.place(builder, habitat.name(), spot(x, z, 0.0), Look::default());
    }
    builder = habitat::pod_chain(
        builder,
        "chain",
        spot(-28.0, 12.0, 0.0),
        &[Look::default(), Look::default()],
    );
    for (part, x, z) in [
        ("scifi:tank", 4.0, -8.0),
        ("scifi:vaporator", 11.0, -8.0),
        ("scifi:antenna", 16.0, -8.0),
        ("scifi:landing-pad", 31.0, -6.0),
        ("scifi:crate", 18.0, 10.0),
        ("scifi:lamp", 21.0, 10.0),
        ("scifi:barrel", 23.0, 10.0),
        ("scifi:hangar", 26.0, 10.0),
        ("scifi:force-fence", 8.0, 14.0),
        ("scifi:generator", 14.0, 14.0),
        ("scifi:kiosk", -2.0, 15.0),
        ("scifi:conduit", 0.0, 17.0),
        ("scifi:awning", 20.0, 16.0),
        ("scifi:sign-post", 16.0, 17.0),
        ("scifi:path", 4.0, 13.0),
    ] {
        builder = builder.instance(
            Instance::new(part.trim_start_matches("scifi:"), part).placed(spot(x, z, 0.0)),
        );
    }
    builder
        .instance(Instance::new("wall", "scifi:wall").placed(spot(-4.0, 6.0, 0.0)))
        .instance(Instance::new("post", "scifi:wall-post").placed(spot(-4.9, 6.0, 0.0)))
        .instance(Instance::new("gate", "scifi:gate").placed(spot(-4.0 + WALL_LENGTH, 6.0, 0.0)))
        .build()
}

/// Half the side of the homestead's walled square, to the corner posts.
const YARD: f64 = 12.8;

/// The adobe colours the homestead's buildings are plastered in, no two alike.
const ADOBE: [[f32; 3]; 4] = [
    [0.55, 0.39, 0.22],
    [0.6, 0.44, 0.27],
    [0.5, 0.35, 0.2],
    [0.58, 0.46, 0.33],
];

/// A walled desert homestead: two domes with their annexes and a two-storey
/// house, vaporators in the yard, a tank, a generator, a market awning and
/// kiosk, paths between the doors, a gate in the south wall and a landing pad
/// outside it, with lamps, signs, crates, barrels and a dish.
///
/// # Errors
///
/// If the assembled recipe does not validate.
pub fn outpost() -> Result<Building, ValidationError> {
    outpost_seeded(37)
}

/// The homestead with reproducible plaster, weathering and utility packages.
///
/// # Errors
///
/// If the assembled recipe does not validate.
pub fn outpost_seeded(seed: u64) -> Result<Building, ValidationError> {
    let mut builder = kit("scifi:outpost", [-18.0, -38.0], [18.0, 17.0])?
        .instance(Instance::new("ground", "scifi:ground"));
    let look = |index: usize| Look {
        adobe: Some(ADOBE[variation(seed, &format!("plaster-{index}")) % ADOBE.len()]),
        ..Look::default()
    };
    // The house faces the gate; the two domes face the yard, each with an
    // annex against its flank.
    builder = Habitat::DomeTall.place(builder, "house", spot(5.5, -4.5, -0.3), look(3));
    for (index, (x, z, turn)) in [(-6.5, 5.5, PI + 0.4), (6.5, 7.0, PI - 0.5)]
        .into_iter()
        .enumerate()
    {
        let side = turn + 1.3;
        builder = Habitat::DomeHut.place(
            builder,
            &format!("dome-{index}"),
            spot(x, z, turn),
            look(index),
        );
        builder = Habitat::DomeAnnex.place(
            builder,
            &format!("annex-{index}"),
            spot(x + 4.6 * side.cos(), z - 4.6 * side.sin(), side),
            look(index + 2),
        );
    }
    for (index, (x, z, turn)) in [
        (-1.0, 10.0, 0.0),
        (-5.0, -3.0, 0.7),
        (11.5, 1.5, 1.4),
        (-0.5, 2.0, 2.1),
    ]
    .into_iter()
    .enumerate()
    {
        builder = builder.instance(
            Instance::new(format!("vaporator-{index}"), "scifi:vaporator").placed(spot(x, z, turn)),
        );
    }
    builder = builder
        .instance(Instance::new("tank", "scifi:tank").placed(spot(-9.5, 9.5, 0.4)))
        .instance(Instance::new("antenna", "scifi:antenna").placed(spot(10.0, 10.5, 0.6)))
        .instance(Instance::new("generator", "scifi:generator").placed(spot(-10.2, -1.0, 0.0)))
        .instance(Instance::new("kiosk", "scifi:kiosk").placed(spot(-8.8, -9.5, 0.9)))
        .instance(Instance::new("market", "scifi:awning").placed(spot(-9.0, -6.7, 0.9)))
        .instance(Instance::new("sign-0", "scifi:sign-post").placed(spot(-6.0, -9.0, 0.6)));
    // Paths from the gate into the yard and to each door.
    for (index, (x, z, turn)) in [
        (-2.8, -12.4, 0.0),
        (-2.8, -8.4, 0.0),
        (-2.8, -4.4, 0.0),
        (-2.8, -0.4, 0.3),
        (-3.9, 3.4, 0.9),
        (-0.8, 3.0, -0.9),
        (2.6, 4.2, -1.2),
        (-0.8, -6.4, -FRAC_PI_2),
    ]
    .into_iter()
    .enumerate()
    {
        builder = builder.instance(
            Instance::new(format!("path-{index}"), "scifi:path").placed(spot(x, z, turn)),
        );
    }
    // The perimeter: a post at each corner and four segments per side, the
    // second segment of the south side a gate.
    let corners = [
        (-YARD, -YARD, 0.0),
        (YARD, -YARD, -FRAC_PI_2),
        (YARD, YARD, PI),
        (-YARD, YARD, FRAC_PI_2),
    ];
    for (side, (x, z, turn)) in corners.into_iter().enumerate() {
        builder = builder.instance(
            Instance::new(format!("post-{side}"), "scifi:wall-post").placed(spot(x, z, turn)),
        );
        let direction = DVec3::new(turn.cos(), 0.0, -turn.sin());
        for segment in 0..4 {
            let origin =
                DVec3::new(x, 0.0, z) + direction * (0.8 + WALL_LENGTH * f64::from(segment));
            let pose = Pose::at(origin.to_array()).rotated(yaw(turn));
            let (id, part) = if side == 0 && segment == 1 {
                ("gate".to_owned(), "scifi:gate")
            } else {
                (format!("wall-{side}-{segment}"), "scifi:wall")
            };
            builder = builder.instance(Instance::new(id, part).placed(pose));
        }
    }
    // The pad outside the gate, lamps and a sign at the gate, fences along
    // the approach.
    let gate_centre = -YARD + 0.8 + WALL_LENGTH + GATE_WIDTH / 2.0;
    builder = builder
        .instance(Instance::new("pad", "scifi:landing-pad").placed(spot(
            gate_centre,
            -YARD - 13.5,
            PI,
        )))
        .instance(Instance::new("sign-1", "scifi:sign-post").placed(spot(
            gate_centre + 5.5,
            -YARD - 2.5,
            0.0,
        )));
    for (index, x) in [gate_centre - 3.8, gate_centre + 3.8]
        .into_iter()
        .enumerate()
    {
        builder = builder.instance(
            Instance::new(format!("lamp-{index}"), "scifi:lamp").placed(spot(x, -YARD - 1.8, 0.0)),
        );
    }
    for (index, (x, z)) in [(-8.0, 1.5), (8.5, -9.0), (0.8, 7.8)]
        .into_iter()
        .enumerate()
    {
        builder = builder.instance(
            Instance::new(format!("yard-lamp-{index}"), "scifi:lamp").placed(spot(x, z, 0.0)),
        );
    }
    for (index, (x, z, turn)) in [
        (9.0, -1.0, 0.3),
        (10.3, -1.4, 1.1),
        (9.6, 0.0, 2.0),
        (-5.0, 11.0, 0.6),
        (-7.2, -10.6, 0.2),
    ]
    .into_iter()
    .enumerate()
    {
        builder = builder.instance(
            Instance::new(format!("crate-{index}"), "scifi:crate").placed(spot(x, z, turn)),
        );
    }
    for (index, (x, turn)) in [
        (gate_centre - 4.8, FRAC_PI_2),
        (gate_centre + 4.8, FRAC_PI_2),
    ]
    .into_iter()
    .enumerate()
    {
        builder = builder.instance(
            Instance::new(format!("fence-{index}"), "scifi:force-fence").placed(spot(
                x,
                -YARD - 1.0,
                turn,
            )),
        );
    }
    for (index, (x, z)) in [(-10.5, 5.0), (-10.0, 4.2), (-11.0, 4.3), (-9.6, -11.0)]
        .into_iter()
        .enumerate()
    {
        builder = builder.instance(
            Instance::new(format!("barrel-{index}"), "scifi:barrel").placed(spot(x, z, 0.0)),
        );
    }
    settlement(builder, seed)
}

/// A corporate colony: the hab tower at its heart, a chain of pods joined by
/// walkway tubes, two prefab modules in faction colours, a landing pad and a
/// hangar, dishes, power and fencing.
///
/// # Errors
///
/// If the assembled recipe does not validate.
pub fn colony() -> Result<Building, ValidationError> {
    colony_seeded(73)
}

/// The corporate colony with a finite, seeded palette and shared roof or
/// ground utility packages. The layout and docking sockets stay fixed.
///
/// # Errors
///
/// If the assembled recipe does not validate.
pub fn colony_seeded(seed: u64) -> Result<Building, ValidationError> {
    const OMNI: [f32; 3] = [0.18, 0.21, 0.23];
    const TEAL: [f32; 3] = [0.06, 0.17, 0.18];
    const OLIVE: [f32; 3] = [0.16, 0.17, 0.12];
    let hull = |color: [f32; 3], wear: f32| Look {
        hull: Some(
            if variation(seed, &format!("{color:?}")).is_multiple_of(3) {
                OLIVE
            } else {
                color
            },
        ),
        wear: Some(if variation(seed, &format!("{wear}")).is_multiple_of(2) {
            0.35
        } else {
            0.6
        }),
        ..Look::default()
    };
    let mut builder = kit("scifi:colony", [-26.0, -30.0], [50.0, 16.0])?
        .instance(Instance::new("ground", "scifi:ground"));
    builder = Habitat::Tower.place(builder, "tower", spot(0.0, 0.0, 0.0), hull(OMNI, 0.2));
    builder = habitat::pod_chain(
        builder,
        "hab",
        spot(10.0, 4.0, 0.0),
        &[hull(OMNI, 0.3), hull(TEAL, 0.25), hull(OMNI, 0.5)],
    );
    for (index, (x, z, turn, color)) in [(-16.0, 4.0, 0.5, TEAL), (-12.0, -12.0, -0.3, OLIVE)]
        .into_iter()
        .enumerate()
    {
        builder = Habitat::Module.place(
            builder,
            &format!("module-{index}"),
            spot(x, z, turn),
            hull(color, 0.45),
        );
    }
    builder = builder
        .instance(Instance::new("pad", "scifi:landing-pad").placed(spot(8.0, -16.0, 0.4)))
        .instance(Instance::new("antenna-0", "scifi:antenna").placed(spot(-6.0, 12.0, 0.3)))
        .instance(Instance::new("antenna-1", "scifi:antenna").placed(spot(44.0, 12.0, 2.2)))
        .instance(Instance::new("tank", "scifi:tank").placed(spot(-21.0, -3.0, 0.0)))
        .instance(Instance::new("hangar", "scifi:hangar").placed(spot(24.0, -22.0, 0.0)))
        .instance(Instance::new("generator", "scifi:generator").placed(spot(-21.0, -18.0, 0.0)))
        .instance(Instance::new("conduit-0", "scifi:conduit").placed(spot(-19.0, -16.0, -0.3)))
        .instance(Instance::new("conduit-1", "scifi:conduit").attach("a", "conduit-0", "b"))
        .instance(Instance::new("sign", "scifi:sign-post").placed(spot(3.5, -6.0, 0.0)))
        .instance(Instance::new("awning", "scifi:awning").placed(spot(-4.5, -8.0, 0.2)))
        .instance(Instance::new("kiosk", "scifi:kiosk").placed(spot(-4.5, -9.4, 0.2)));
    for index in 0..4 {
        let id = format!("fence-{index}");
        builder = builder.instance(if index == 0 {
            Instance::new(id, "scifi:force-fence").placed(spot(-2.0, -27.0, 0.0))
        } else {
            Instance::new(id, "scifi:force-fence").attach(
                "left",
                format!("fence-{}", index - 1),
                "right",
            )
        });
    }
    for (index, (x, z)) in [
        (-5.0, -5.0),
        (5.0, 6.0),
        (14.0, -3.0),
        (22.0, -3.0),
        (-6.0, 6.0),
        (34.0, -3.0),
    ]
    .into_iter()
    .enumerate()
    {
        builder = builder
            .instance(Instance::new(format!("lamp-{index}"), "scifi:lamp").placed(spot(x, z, 0.0)));
    }
    for (index, (x, z, turn)) in [
        (-17.0, -2.0, 0.2),
        (-18.2, -2.3, 0.9),
        (-17.5, -0.8, 1.7),
        (18.0, -16.0, 0.4),
    ]
    .into_iter()
    .enumerate()
    {
        builder = builder.instance(
            Instance::new(format!("crate-{index}"), "scifi:crate").placed(spot(x, z, turn)),
        );
    }
    for (index, (x, z, turn)) in [(0.0, -6.5, 0.0), (0.0, -10.5, 0.0), (4.0, -12.5, 0.9)]
        .into_iter()
        .enumerate()
    {
        builder = builder.instance(
            Instance::new(format!("path-{index}"), "scifi:path").placed(spot(x, z, turn)),
        );
    }
    settlement(builder, seed)
}

#[cfg(test)]
mod variety_tests {
    use super::*;

    #[test]
    fn seeded_settlements_repeat_and_reuse_their_shells() {
        for build in [outpost_seeded, colony_seeded] {
            let a = build(73).expect("seed builds");
            let repeat = build(73).expect("seed repeats");
            let b = build(74).expect("another seed builds");
            let fingerprint = |building: &Building| format!("{:?}", building.recipe());
            assert_eq!(fingerprint(&a), fingerprint(&repeat));
            assert_ne!(fingerprint(&a), fingerprint(&b));
            assert_eq!(a.recipe().parts, b.recipe().parts);
            assert_eq!(a.rooms(), b.rooms());
            for building in [&a, &b] {
                crate::library::materials()
                    .check_for(building)
                    .expect("all bindings exist");
                assert!(
                    building
                        .recipe()
                        .instances
                        .iter()
                        .any(|i| i.id.ends_with("-utility"))
                );
            }
        }
    }
}
