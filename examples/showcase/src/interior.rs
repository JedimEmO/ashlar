//! A two-storey house with an authored interior.
//!
//! The interior is built the way ADR 0005 says to build one: walls, floors,
//! liners and a partition are elements that are added together, and only the
//! openings are subtracted. Carving a room out of a solid mass would be shorter
//! to write, and it would be wrong: a collision proxy is convex and
//! conservative, so an opening subtracted inside a mass is not subtracted from
//! its proxy, and the proxy of a carved mass would fill the room. Adding a
//! liner over a wall's inner face instead keeps ADR 0002's proxies correct
//! while still giving the interior its own finish.
//!
//! The kit reuses the corporate palette's slot names and `metro:` keys, so the
//! material library that serves the tower serves this house too.
use std::f64::consts::{FRAC_PI_2, PI};

use ashlar::{
    Building, BuildingBuilder, Collision, Element, Geometry, Instance, MergeGroup, Part, Pose,
    Room, ValidationError,
};
use glam::DQuat;

/// Exterior width along X, which the front and back walls run along.
const WIDTH: f64 = 8.0;
/// Exterior depth along Z, which the side walls run along.
const DEPTH: f64 = 6.0;
/// Clear height of one storey, and the y the next one starts at.
const STOREY: f64 = 3.0;
/// Outer wall thickness.
const WALL: f64 = 0.3;
/// Liner thickness over the inner face of a wall.
const LINER: f64 = 0.02;
/// Partition thickness.
const PARTITION: f64 = 0.1;
/// Floor and roof slab thickness.
const SLAB: f64 = 0.2;
/// How far a cutter runs past the faces it opens, so it is an opening.
const MARGIN: f64 = 0.05;
/// Depth a cutter needs to pass through a wall and its liner.
const THROUGH_WALL: f64 = WALL + LINER + 2.0 * MARGIN;
/// Clear door opening width and height.
const DOOR_WIDTH: f64 = 1.0;
const DOOR_HEIGHT: f64 = 2.1;
/// Clear window opening width and height, and its sill.
const WINDOW_WIDTH: f64 = 1.2;
const WINDOW_HEIGHT: f64 = 1.2;
const SILL: f64 = 0.9;
/// Glass pane thickness.
const GLASS: f64 = 0.04;
/// Where the partition stands, its far face on the building's centre line so
/// both rooms keep to the 0.1 m grid.
const PARTITION_X: f64 = 3.9;
/// Clear width of each room, either side of the partition.
const LEFT_WIDTH: f64 = PARTITION_X - WALL;
const RIGHT_WIDTH: f64 = WIDTH - WALL - (PARTITION_X + PARTITION);
/// Clear depth of a room, between the front and back liners.
const DEPTH_CLEAR: f64 = DEPTH - 2.0 * WALL;
/// Distance from a wall body's inner face to the inner face of its liner.
const INNER: f64 = WALL + LINER;
/// Clear width and depth inside the wall liners, where the floor finish lies.
const CLEAR_WIDTH: f64 = WIDTH - 2.0 * INNER;
const CLEAR_DEPTH: f64 = DEPTH - 2.0 * INNER;

/// One hole through a wall: its size, where it sits, and what the wall's own
/// cutter publishes if it is the opening a game walks through.
struct Opening {
    width: f64,
    height: f64,
    bottom: f64,
    centre: f64,
    portal: &'static str,
    slot: Option<&'static str>,
}

/// A window at `centre` along the wall, sill height and all.
fn window(centre: f64, portal: &'static str) -> Opening {
    Opening {
        width: WINDOW_WIDTH,
        height: WINDOW_HEIGHT,
        bottom: SILL,
        centre,
        portal,
        slot: None,
    }
}

/// The front door, which runs into the floor rather than stopping on it.
fn door(centre: f64) -> Opening {
    Opening {
        width: DOOR_WIDTH,
        height: DOOR_HEIGHT + MARGIN,
        bottom: -MARGIN,
        centre,
        portal: "front-door",
        slot: Some("metal"),
    }
}

/// One cutter through a wall of thickness `THROUGH_WALL`.
fn cutter(opening: &Opening, portal: bool) -> Geometry {
    let mut geometry =
        Geometry::cuboid([opening.width, opening.height, THROUGH_WALL]).placed(Pose::at([
            opening.centre - opening.width / 2.0,
            opening.bottom,
            -MARGIN,
        ]));
    // Only the wall's cutter is the portal, and only it wears the door's metal.
    // The liner's cutter is the same hole and nothing else.
    if portal {
        geometry = geometry.portal(opening.portal);
        if let Some(slot) = opening.slot {
            geometry = geometry.cut_material(slot);
        }
    }
    geometry
}

fn floor_part() -> Result<Part, ValidationError> {
    Part::builder("house:floor")
        .element(
            Element::new("slab", Geometry::cuboid([WIDTH, SLAB, DEPTH]), "stone")
                .collision(Collision::Bounds),
        )
        .element(liner_element("floor", SLAB))
        .build()
}

/// The ceiling finish that hangs under a slab with a room below it.
///
/// Its own part rather than an element of the slab above, because it is placed
/// in the slab's group: the ground rooms' ceiling sits in `upper`, flush under
/// the upper storey's floor slab, and the upper rooms' ceiling in `roof`, flush
/// under the roof. A ceiling is the underside of the floor above, so in that
/// group it unions with its slab with no seam, and a game that hides the storeys
/// above the player hides the ceiling with them. That is what lets a top-down
/// camera see into the storey it shows instead of its own ceiling.
fn ceiling_part() -> Result<Part, ValidationError> {
    Part::builder("house:ceiling")
        .element(liner_element("ceiling", 0.0))
        .build()
}

/// A 0.02 m interior finish over the clear footprint inside the wall liners.
///
/// `y` is where the liner sits in its part's frame: over a slab's top for the
/// floor liner, at the part's origin for a ceiling, whose placement then puts
/// it flush under the slab above. It overlaps the partition, which stands
/// inside the footprint, by the liner's thickness; that is harmless to the
/// union and keeps the partition and wall liner heights unchanged.
fn liner_element(id: &str, y: f64) -> Element {
    Element::new(
        id,
        Geometry::cuboid([CLEAR_WIDTH, LINER, CLEAR_DEPTH]).placed(Pose::at([INNER, y, INNER])),
        if id == "floor" { "floor" } else { "spine" },
    )
    .interior()
}

/// Thin trim has no proxy; the surrounding wall remains the collision source.
fn trim(id: &str, geometry: Geometry) -> Element {
    Element::new(id, geometry, "trim")
}

fn bar(size: [f64; 3], at: [f64; 3]) -> Geometry {
    Geometry::cuboid(size).placed(Pose::at(at))
}

fn roof_part() -> Result<Part, ValidationError> {
    Part::builder("house:roof")
        .element(Element::new(
            "slab",
            Geometry::cuboid([WIDTH, SLAB, DEPTH]),
            "roof",
        ))
        .element(trim(
            "fascia",
            bar([WIDTH + 0.24, 0.17, DEPTH + 0.24], [-0.12, 0.02, -0.12])
                .subtract(bar([WIDTH - 0.08, 0.4, DEPTH - 0.08], [0.04, -0.1, 0.04])),
        ))
        .element(trim(
            "drip-edge",
            bar([WIDTH + 0.34, 0.045, DEPTH + 0.34], [-0.17, 0.21, -0.17])
                .subtract(bar([WIDTH - 0.02, 0.2, DEPTH - 0.02], [0.01, 0.15, 0.01])),
        ))
        .build()
}

/// An outer wall and the liner over its inner face, opened where `openings`
/// say. The liner is its own element, so each opening needs its own cutter.
fn wall_part(id: &str, length: f64, openings: &[Opening]) -> Result<Part, ValidationError> {
    let mut body = Geometry::cuboid([length, STOREY, WALL]);
    let mut liner = Geometry::cuboid([length, STOREY, LINER]).placed(Pose::at([0.0, 0.0, WALL]));
    for opening in openings {
        body = body.subtract(cutter(opening, true));
        liner = liner.subtract(cutter(opening, false));
    }
    let mut part = Part::builder(id)
        .element(
            Element::new("wall", body, "stone")
                .cut_material("reveal")
                .collision(Collision::Bounds),
        )
        .element(Element::new("liner", liner, "spine").interior());
    let mut skirting = bar([length, 0.11, 0.035], [0.0, 0.025, WALL + LINER]);
    for (index, opening) in openings.iter().enumerate() {
        let left = opening.centre - opening.width / 2.0;
        let top = opening.bottom + opening.height;
        let bottom = opening.bottom.max(0.025);
        let frame = |z: f64| {
            bar(
                [opening.width + 0.15, top - bottom + 0.08, 0.07],
                [left - 0.075, bottom, z],
            )
            .subtract(bar(
                [opening.width, top - bottom + 0.02, 0.2],
                [left, bottom - 0.02, z - 0.05],
            ))
        };
        part = part
            .element(trim(&format!("frame-{index}"), frame(-0.06)))
            .element(trim(&format!("casing-{index}"), frame(WALL + LINER - 0.01)).interior());
        if opening.bottom > 0.0 {
            part = part
                .element(trim(
                    &format!("sill-{index}"),
                    Geometry::chamfered_cuboid([opening.width + 0.24, 0.09, 0.55], 0.015)
                        .placed(Pose::at([left - 0.12, opening.bottom - 0.09, -0.15])),
                ))
                .element(trim(
                    &format!("transom-{index}"),
                    bar(
                        [opening.width, 0.045, 0.055],
                        [
                            left,
                            opening.bottom + opening.height * 0.65,
                            WALL / 2.0 - 0.03,
                        ],
                    ),
                ));
        } else {
            skirting = skirting.subtract(bar([opening.width, 0.3, 0.2], [left, -0.05, WALL]));
            part = part.element(trim(
                &format!("threshold-{index}"),
                bar([opening.width, 0.025, WALL + 0.14], [left, 0.0, -0.08]),
            ));
        }
    }
    part.element(trim("skirting", skirting).interior()).build()
}

fn partition_part() -> Result<Part, ValidationError> {
    let body = Geometry::cuboid([PARTITION, STOREY, DEPTH_CLEAR]).subtract(
        Geometry::cuboid([PARTITION + 2.0 * MARGIN, DOOR_HEIGHT + MARGIN, DOOR_WIDTH])
            .placed(Pose::at([
                -MARGIN,
                -MARGIN,
                (DEPTH_CLEAR - DOOR_WIDTH) / 2.0,
            ]))
            .portal("inner-door"),
    );
    Part::builder("house:partition")
        .element(
            Element::new("wall", body, "spine")
                .interior()
                .collision(Collision::Bounds),
        )
        .element(
            trim(
                "door-casings",
                Geometry::union_all([-0.045, PARTITION - 0.01].map(|x| {
                    let near = (DEPTH_CLEAR - DOOR_WIDTH) / 2.0;
                    Geometry::union_all([
                        bar([0.055, DOOR_HEIGHT + 0.08, 0.075], [x, 0.0, near - 0.075]),
                        bar(
                            [0.055, DOOR_HEIGHT + 0.08, 0.075],
                            [x, 0.0, near + DOOR_WIDTH],
                        ),
                        bar(
                            [0.055, 0.08, DOOR_WIDTH + 0.15],
                            [x, DOOR_HEIGHT, near - 0.075],
                        ),
                    ])
                })),
            )
            .interior(),
        )
        .element(
            trim(
                "skirting",
                Geometry::union_all([-0.035, PARTITION].map(|x| {
                    bar([0.035, 0.11, DEPTH_CLEAR], [x, 0.025, 0.0]).subtract(bar(
                        [0.3, 0.3, DOOR_WIDTH],
                        [-0.1, -0.05, (DEPTH_CLEAR - DOOR_WIDTH) / 2.0],
                    ))
                })),
            )
            .interior(),
        )
        .build()
}

/// A pane centred on its own origin, so a placement is the window's centre.
fn glass_part() -> Result<Part, ValidationError> {
    let pane = Geometry::cuboid([WINDOW_WIDTH, WINDOW_HEIGHT, GLASS]).placed(Pose::at([
        -WINDOW_WIDTH / 2.0,
        -WINDOW_HEIGHT / 2.0,
        -GLASS / 2.0,
    ]));
    Part::builder("house:glass")
        .element(Element::new("pane", pane, "glass").standalone())
        .build()
}

fn placed(id: String, part: &str, at: [f64; 3], group: &str) -> Instance {
    Instance::new(id, part).placed(Pose::at(at)).group(group)
}

fn rotated(id: String, part: &str, at: [f64; 3], yaw: f64, group: &str) -> Instance {
    Instance::new(id, part)
        .placed(Pose::at(at).rotated(DQuat::from_rotation_y(yaw)))
        .group(group)
}

/// The floor, four walls and partition of both storeys, each in its group,
/// with the ceiling finish under the slab above each storey's rooms placed in
/// the group of that slab.
fn storeys(mut builder: BuildingBuilder) -> BuildingBuilder {
    for (index, base, ceiling, ceiling_group) in [
        (0_i32, 0.0_f64, STOREY - SLAB - LINER, "upper"),
        (1, STOREY, 2.0 * STOREY - LINER, "roof"),
    ] {
        let group = if index == 0 { "ground" } else { "upper" };
        let front = if index == 0 {
            "house:wall-front-door"
        } else {
            "house:wall-front"
        };
        builder = builder
            .instance(placed(
                format!("floor-{index}"),
                "house:floor",
                [0.0, base - SLAB, 0.0],
                group,
            ))
            .instance(placed(
                format!("ceiling-{index}"),
                "house:ceiling",
                [0.0, ceiling, 0.0],
                ceiling_group,
            ))
            .instance(placed(
                format!("front-{index}"),
                front,
                [0.0, base, 0.0],
                group,
            ))
            .instance(rotated(
                format!("back-{index}"),
                "house:wall-back",
                [WIDTH, base, DEPTH],
                PI,
                group,
            ))
            .instance(rotated(
                format!("left-{index}"),
                "house:wall-left",
                [0.0, base, DEPTH],
                FRAC_PI_2,
                group,
            ))
            .instance(rotated(
                format!("right-{index}"),
                "house:wall-right",
                [WIDTH, base, 0.0],
                -FRAC_PI_2,
                group,
            ))
            .instance(placed(
                format!("partition-{index}"),
                "house:partition",
                [PARTITION_X, base, WALL],
                group,
            ));
    }
    builder
}

/// One pane in every window, centred in the wall's own thickness.
fn glass(mut builder: BuildingBuilder) -> BuildingBuilder {
    for (index, base) in [(0_i32, 0.0_f64), (1, STOREY)] {
        let group = if index == 0 { "ground" } else { "upper" };
        let y = base + SILL + WINDOW_HEIGHT / 2.0;
        let middle = f64::midpoint(WALL, LINER);
        for (side, at, yaw) in [
            ("front", [2.0, y, middle], 0.0),
            ("back", [6.0, y, DEPTH - middle], PI),
            ("left", [middle, y, DEPTH / 2.0], FRAC_PI_2),
            ("right", [WIDTH - middle, y, DEPTH / 2.0], -FRAC_PI_2),
        ] {
            builder = builder.instance(rotated(
                format!("glass-{side}-{index}"),
                "house:glass",
                at,
                yaw,
                group,
            ));
        }
    }
    builder
}

/// Two rooms a storey, either side of the partition, sized to the clear space
/// between the liners and listing the openings that reach them.
fn rooms(mut builder: BuildingBuilder) -> BuildingBuilder {
    for (index, base, height) in [(0_i32, 0.0_f64, STOREY - SLAB), (1, STOREY, STOREY)] {
        let group = if index == 0 { "ground" } else { "upper" };
        builder = builder
            .room(
                Room::new(
                    format!("room-left-{index}"),
                    [LEFT_WIDTH, height, DEPTH_CLEAR],
                )
                .placed(Pose::at([WALL, base, WALL]))
                .group(group)
                .portal("window-front")
                .portal("window-left")
                .portal("inner-door"),
            )
            .room(
                Room::new(
                    format!("room-right-{index}"),
                    [RIGHT_WIDTH, height, DEPTH_CLEAR],
                )
                .placed(Pose::at([PARTITION_X + PARTITION, base, WALL]))
                .group(group)
                .portal("front-door")
                .portal("window-back")
                .portal("window-right")
                .portal("inner-door"),
            );
    }
    builder
}

/// A two-storey house, eight metres by six, with two rooms on each storey.
///
/// The ground front wall carries the front door and the upper one does not, so
/// they are two parts; every other wall is one part placed once per storey.
pub fn building() -> Result<Building, ValidationError> {
    let mut builder = Building::builder("metro:house").merged();
    for (slot, key) in [
        ("stone", "library:plaster"),
        ("spine", "showcase:interior-plaster"),
        ("reveal", "library:formed-concrete"),
        ("metal", "library:painted-metal"),
        ("glass", "library:glass"),
        ("trim", "showcase:dark-metal"),
        ("floor", "library:wood-floor"),
        ("roof", "library:clay-roof-tiles"),
        ("rubble", "library:rubble"),
    ] {
        builder = builder.material(slot, key);
    }
    builder = builder
        .part(floor_part()?)
        .part(ceiling_part()?)
        .part(wall_part(
            "house:wall-front-door",
            WIDTH,
            &[window(2.0, "window-front"), door(6.0)],
        )?)
        .part(wall_part(
            "house:wall-front",
            WIDTH,
            &[window(2.0, "window-front")],
        )?)
        .part(wall_part(
            "house:wall-back",
            WIDTH,
            &[window(2.0, "window-back")],
        )?)
        .part(wall_part(
            "house:wall-left",
            DEPTH,
            &[window(DEPTH / 2.0, "window-left")],
        )?)
        .part(wall_part(
            "house:wall-right",
            DEPTH,
            &[window(DEPTH / 2.0, "window-right")],
        )?)
        .part(partition_part()?)
        .part(glass_part()?)
        .part(roof_part()?)
        .group(MergeGroup::new("ground").storey(0))
        .group(MergeGroup::new("upper").storey(1))
        .group(MergeGroup::new("roof").storey(2))
        .instance(placed(
            "roof".into(),
            "house:roof",
            [0.0, 2.0 * STOREY, 0.0],
            "roof",
        ));
    builder = storeys(builder);
    builder = glass(builder);
    rooms(builder).build()
}
