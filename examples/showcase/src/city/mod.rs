//! A dark city kit for metropolises: corporate towers of dark stained
//! concrete built from stackable storeys, the back alleys between them, a
//! street kit, and a seeded generator that lays them out block by block.
//!
//! The kit keeps the corporate kit's grid — 4.0 m bays and 3.8 m storeys — and
//! its facade language: a deep wall with recessed, mullioned windows, a strip
//! light in each window's head and a lit line down each chamfered corner. It is
//! darker and taller, and it is lit for the night:
//!
//! - **Towers** stand four to a lot, eighteen by sixteen at their foot, and
//!   step in twice as they rise, to ninety storeys, onto terraces. Each is
//!   dressed in one of three styles with tiers of its own — punched windows
//!   between fins, brutalist ribbons between precast spandrels, or glass
//!   behind a deep concrete frame — stands on a podium with a shopfront and a
//!   canopied entrance, and wears a crown: a screened penthouse, lit steps or
//!   lit blades.
//! - **Landmarks** fill a lot, thirty metres at their foot and over a hundred
//!   storeys tall, under a spire or a mast.
//! - **Tenements** are thirty-eight-metre blocks of a handful of storeys with
//!   fire escapes down their fronts.
//! - **Alleys** run between a lot's towers, six metres one way and ten the
//!   other: gutters, dumpsters, pipes, air-conditioners, fire escapes, blade
//!   signs in neon and cables strung overhead.
//! - **Night markets** fill a lot with stalls round a holo pillar.
//!
//! Every storey is one part, with its floor slab, its facade, an interior
//! liner over the facade's inner face, a floor and a ceiling finish, a lift
//! core and one open-plan room. The interiors are added, never carved, as ADR
//! 0005 says, and every straight run of wall is its own convex proxy. A storey
//! declares `bottom` and `top` sockets at its footprint's centre, so a tower is
//! a stack of `attach_aligned` instances and a narrower tier stands centred on
//! a wider one. Towers are drawn as parts, not merged: a metropolis is
//! thousands of storeys of a handful of parts, and instancing is what keeps it
//! cheap. What varies per storey — which offices are lit — is an instance's
//! material override, so it costs a handful of materials, not a part each.
//!
//! The street kit is laid on a sixty-metre pitch: a twelve-metre carriageway on
//! each grid line, forty-eight-metre blocks between them with a kerbed
//! three-metre sidewalk, and a forty-two-metre lot inside that. Roads and
//! crossings sit at Y = 0 and the main block surface at [`KERB`], with
//! sloping approaches to lowered pavement corners at the crossings.
//!
//! Slots are named for what they are, and [`PALETTE`] binds them all. Interior
//! finishes are `liner` (walls and ceilings) and `floor`.
use std::f64::consts::{FRAC_PI_2, PI, SQRT_2};

use ashlar::{
    Axis, Binding, Building, BuildingBuilder, Collision, Element, Geometry, Instance, ParamValue,
    Part, PartBuilder, Pose, Socket, UvMode, ValidationError,
};
use glam::DQuat;

mod alley;
mod metropolis;
mod storey;
mod street;

pub use metropolis::{
    Kind, alley_scene, block_scene, kit_scene, metropolis, piece, piece_names, tower_scene,
};

/// One structural bay.
pub const BAY: f64 = 4.0;
/// Floor to floor, as the corporate kit's.
pub const STOREY: f64 = 3.8;
/// Floor to floor of a lobby: a storey and a half.
pub const LOBBY: f64 = 5.7;
/// Structural floor slab.
const SLAB: f64 = 0.3;
/// Interior finish over a wall, a floor or a ceiling.
const LINER: f64 = 0.02;
/// How far a cutter runs past the faces it opens.
const MARGIN: f64 = 0.05;
/// Height of the block surface and sidewalks above the carriageway.
pub const KERB: f64 = 0.15;
/// Grid pitch of the street plan: one carriageway and one block.
pub const PITCH: f64 = 60.0;
/// Width of a carriageway.
pub const ROAD: f64 = 12.0;
/// Side of one block, kerb to kerb.
pub const BLOCK: f64 = 48.0;
/// Width of the sidewalk inside a block's kerb.
pub const SIDEWALK: f64 = 3.0;
/// Side of the buildable lot inside the sidewalk.
pub const LOT: f64 = BLOCK - 2.0 * SIDEWALK;

/// Segments round a large turned body.
const ROUND: u32 = 48;
/// Segments round a small one.
const SMALL: u32 = 20;

/// Slot to material key for every piece of the city kit.
pub const PALETTE: [(&str, &str); 32] = [
    // Structure.
    ("clad", "showcase:stained-concrete"),
    ("concrete", "showcase:dark-concrete"),
    ("dark", "library:dark-recess"),
    ("metal", "showcase:dark-metal"),
    ("trim", "library:steel"),
    ("rust", "library:rusted-steel"),
    ("deck", "library:tread-plate"),
    ("hull", "library:hull-plating"),
    ("bin", "library:painted-metal"),
    ("bags", "library:dark-recess"),
    // Interiors.
    ("liner", "showcase:interior-plaster"),
    ("floor", "library:ceramic-tile"),
    ("foliage", "showcase:hedge"),
    // Glazing: every pane dark until a storey lights it.
    ("glass", "showcase:window-dark"),
    ("pane-a", "showcase:window-dark"),
    ("pane-b", "showcase:window-dark"),
    ("pane-c", "showcase:window-dark"),
    // A lobby's shopfronts, lit.
    ("shop", "showcase:shopfront"),
    // Ground.
    ("road", "showcase:wet-road"),
    ("asphalt", "showcase:wet-asphalt"),
    ("walk", "showcase:dark-paving"),
    ("plaza", "showcase:dark-concrete"),
    ("marking", "library:signage-ink"),
    ("awning", "library:signage-ink"),
    // Light.
    ("light", "library:emissive-strip"),
    ("strip", "library:emissive-strip"),
    ("lamp", "showcase:sodium"),
    ("crown", "showcase:neon-cyan"),
    ("edge", "library:dark-recess"),
    ("neon", "showcase:neon-magenta"),
    ("holo", "library:holo-sign"),
    ("warning", "showcase:signal"),
];

/// A building builder with the whole palette bound.
fn palette(id: &str) -> BuildingBuilder {
    let mut builder = Building::builder(id);
    for (slot, key) in PALETTE {
        builder = builder.material(slot, key);
    }
    builder
}

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

/// Round away the float dust a cosine leaves at a right angle.
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

/// A lamp head: a small glowing ball.
fn beacon(id: &str, radius: f64, at: [f64; 3]) -> Element {
    Element::new(id, Geometry::ball(radius, 6).placed(Pose::at(at)), "light")
        .standalone()
        .far()
}

/// A pose on the ground at `(x, z)`, raised `y`, turned `turn` about Y.
fn spot(x: f64, y: f64, z: f64, turn: f64) -> Pose {
    Pose::at([x, y, z]).rotated(yaw(turn))
}

/// The `bottom` and `top` sockets of a storey whose footprint centre is at
/// `centre` on X/Z and which is `height` tall.
fn stacking(centre: [f64; 2], height: f64) -> [Socket; 2] {
    [
        Socket::new("bottom", Pose::at([centre[0], 0.0, centre[1]])),
        Socket::new("top", Pose::at([centre[0], height, centre[1]])),
    ]
}

/// Every part of the city kit, towers, toppers and street, in one list.
///
/// # Errors
///
/// If a part does not validate.
pub fn parts() -> Result<Vec<Part>, ValidationError> {
    let mut parts = storey::parts()?;
    parts.extend(street::parts()?);
    parts.extend(alley::parts()?);
    Ok(parts)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_grid_adds_up() {
        assert!((PITCH - ROAD - BLOCK).abs() < 1e-9);
        assert!((LOT - 42.0).abs() < 1e-9);
        assert!((STOREY - 3.8).abs() < 1e-9 && (BAY - 4.0).abs() < 1e-9);
    }
}
