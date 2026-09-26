//! Metropolitan corporate kit: podium, curtain bays and a separate service spine.
//! Proportions and finishes belong to this content, never to the geometry backend.
use ashlar::{
    Binding, Building, BuildingBuilder, Collision, Element, Geometry, Instance, MergeGroup,
    ParamValue, Part, Pose, Socket, ValidationError,
};
use glam::{DQuat, DVec3};

const STOREY: f64 = 3.8;
const PODIUM: f64 = 6.45;

/// A detail that is solid to walk into; see the study kit's `solid`.
fn solid(id: &str, size: [f64; 3], at: [f64; 3], slot: &str) -> Element {
    detail(id, size, at, slot).collision(Collision::Bounds)
}

fn detail(id: &str, size: [f64; 3], at: [f64; 3], slot: &str) -> Element {
    let bevel = size
        .into_iter()
        .fold(f64::INFINITY, f64::min)
        .mul_add(0.18, 0.0)
        .min(0.045);
    super::block(id, size, at, bevel, slot)
}

/// Square-edged metal extrusions stay cheap when repeated over a whole facade.
fn extrusion(id: &str, size: [f64; 3], at: [f64; 3], slot: &str) -> Element {
    Element::new(id, Geometry::cuboid(size).placed(Pose::at(at)), slot)
}

fn glazed_bay(podium: bool) -> Result<Part, ValidationError> {
    let (id, height, bottom, opening) = if podium {
        ("metro:podium", 6.0, 0.55, 4.5)
    } else {
        ("metro:curtain", STOREY, 0.4, 2.95)
    };
    let body = Geometry::chamfered_cuboid([4.0, height, 0.8], 0.045)
        .subtract(Geometry::cuboid([3.16, opening, 1.3]).placed(Pose::at([0.42, bottom, -0.2])));
    let mut part = Part::builder(id)
        .element(
            Element::new("frame", body, "stone")
                .cut_material("reveal")
                .collision(Collision::Bounds),
        )
        .element(solid(
            "glass",
            [3.12, opening - 0.04, 0.065],
            [0.44, bottom + 0.02, 0.64],
            "glass",
        ))
        .element(extrusion(
            "mullion",
            [0.085, opening, 0.42],
            [1.9575, bottom, 0.25],
            "metal",
        ))
        .element(detail(
            "sill",
            [3.3, 0.12, 0.65],
            [0.35, bottom - 0.08, -0.1],
            "metal",
        ));
    for (name, x) in [("left-fin", 0.1), ("right-fin", 3.72)] {
        part = part.element(detail(name, [0.18, height, 1.25], [x, 0.0, -0.3], "spine"));
    }
    // The glazing channel follows the reveal, with a projecting pressure cap
    // on the central mullion and a transom that carries the upper pane.
    part = part
        .element(Element::new(
            "glazing-jambs",
            Geometry::cuboid([0.1, opening + 0.12, 0.12])
                .placed(Pose::at([0.36, bottom - 0.06, 0.52]))
                .arrayed(2, Pose::at([3.18, 0.0, 0.0])),
            "metal",
        ))
        .element(Element::new(
            "glazing-heads",
            Geometry::cuboid([3.12, 0.1, 0.12])
                .placed(Pose::at([0.44, bottom - 0.06, 0.52]))
                .arrayed(2, Pose::at([0.0, opening + 0.02, 0.0])),
            "metal",
        ))
        .element(extrusion(
            "pressure-cap",
            [0.13, opening, 0.08],
            [1.935, bottom, 0.20],
            "metal",
        ))
        .element(extrusion(
            "transom",
            [3.16, 0.075, 0.2],
            [0.42, bottom + opening * 0.72, 0.46],
            "metal",
        ))
        .element(extrusion(
            "sill-drip",
            [3.32, 0.045, 0.08],
            [0.34, bottom - 0.10, -0.14],
            "metal",
        ));
    if podium {
        part = part.element(detail(
            "head-cornice",
            [3.96, 0.16, 1.0],
            [0.02, height - 0.25, -0.24],
            "stone",
        ));
    }
    if !podium {
        part = part.element(detail(
            "occupancy-strip",
            [2.75, 0.065, 0.08],
            [0.62, 0.27, -0.08],
            "light",
        ));
    }
    part.build()
}

#[expect(clippy::too_many_lines, reason = "one declarative entrance assembly")]
fn lobby() -> Result<Part, ValidationError> {
    let body = Geometry::chamfered_cuboid([8.0, 6.0, 1.6], 0.065)
        .subtract(Geometry::cuboid([5.6, 4.8, 2.1]).placed(Pose::at([1.2, -0.02, -0.2])));
    Part::builder("metro:lobby")
        .element(
            Element::new("portal", body, "stone")
                .cut_material("reveal")
                .collision(Collision::Bounds),
        )
        // The glass sits a centimetre higher than the door frames it runs
        // between, so its horizontal edges are not coplanar with theirs. The
        // frames are trim and stay where they are.
        .element(solid(
            "glass-wall",
            [5.6, 4.5, 0.12],
            [1.2, 0.14, 1.25],
            "glass",
        ))
        .element(detail(
            "left-door-frame",
            [0.1, 4.5, 0.25],
            [2.38, 0.13, 1.05],
            "metal",
        ))
        .element(detail(
            "right-door-frame",
            [0.1, 4.5, 0.25],
            [5.52, 0.13, 1.05],
            "metal",
        ))
        .element(detail(
            "door-meeting",
            [0.08, 4.5, 0.25],
            [3.96, 0.13, 1.05],
            "metal",
        ))
        .element(solid(
            "threshold",
            [5.8, 0.15, 1.95],
            [1.1, -0.02, -0.6],
            "reveal",
        ))
        .element(detail(
            "canopy",
            [8.6, 0.3, 3.2],
            [-0.3, 4.92, -2.15],
            "metal",
        ))
        .element(detail(
            "canopy-light",
            [5.7, 0.055, 0.16],
            [1.15, 4.865, -1.85],
            "light",
        ))
        .element(detail(
            "access-terminal",
            [0.28, 0.9, 0.18],
            [1.21, 1.0, 0.3],
            "metal",
        ))
        .element(detail(
            "terminal-screen",
            [0.19, 0.34, 0.03],
            [1.255, 1.47, 0.27],
            "light",
        ))
        .element(extrusion(
            "canopy-fascia",
            [8.64, 0.18, 0.12],
            [-0.32, 4.98, -2.21],
            "metal",
        ))
        .element(Element::new(
            "soffit-ribs",
            Geometry::cuboid([0.09, 0.08, 2.9])
                .placed(Pose::at([0.45, 4.85, -1.95]))
                .arrayed(6, Pose::at([1.4, 0.0, 0.0])),
            "metal",
        ))
        .element(Element::new(
            "canopy-brackets",
            Geometry::union_all([0.4, 7.4].map(|x| {
                Geometry::extrude([[0.0, 0.0], [-1.9, 0.0], [0.0, -1.9]], 0.12).placed(
                    Pose::at([x + 0.12, 4.98, 0.1])
                        .rotated(DQuat::from_rotation_z(std::f64::consts::FRAC_PI_2)),
                )
            })),
            "metal",
        ))
        .element(Element::new(
            "door-pulls",
            Geometry::cuboid([0.055, 0.65, 0.11])
                .placed(Pose::at([3.73, 1.1, 1.17]))
                .arrayed(2, Pose::at([0.48, 0.0, 0.0])),
            "metal",
        ))
        .element(extrusion(
            "door-head",
            [3.28, 0.13, 0.3],
            [2.36, 3.25, 1.02],
            "metal",
        ))
        .socket(Socket::new(
            "entrance",
            Pose::at([4.0, 0.13, -0.6]).rotated(DQuat::from_rotation_y(std::f64::consts::PI)),
        ))
        .build()
}

fn service_riser() -> Result<Part, ValidationError> {
    let mut part = Part::builder("metro:service-riser")
        .element(solid("body", [2.6, STOREY, 6.7], [0.0, 0.0, 0.0], "dark"))
        // The plate's outer face is pulled 0.02 m inside the body's, so the
        // two are not coplanar and a union cannot give the body's panels to
        // the plate. Its rims still show past the body in z.
        .element(detail(
            "spine",
            [0.28, STOREY, 7.1],
            [2.3, 0.0, -0.2],
            "spine",
        ));
    for row in 0..5 {
        part = part.element(detail(
            &format!("louvre-{row}"),
            [1.65, 0.13, 0.22],
            [0.3, 0.65 + f64::from(row) * 0.37, -0.14],
            "metal",
        ));
    }
    part = part
        .element(extrusion(
            "vent-head",
            [1.85, 0.08, 0.30],
            [0.2, 2.4, -0.2],
            "metal",
        ))
        .element(Element::new(
            "vent-jambs",
            Geometry::cuboid([0.07, 1.83, 0.28])
                .placed(Pose::at([0.21, 0.59, -0.18]))
                .arrayed(2, Pose::at([1.74, 0.0, 0.0])),
            "metal",
        ))
        .element(Element::new(
            "side-seams",
            Geometry::cuboid([0.08, STOREY - 0.12, 0.07])
                .placed(Pose::at([-0.035, 0.06, 0.7]))
                .arrayed(3, Pose::at([0.0, 0.0, 2.6])),
            "metal",
        ));
    part.element(detail(
        "service-light",
        [0.09, 2.8, 0.06],
        [2.1, 0.5, -0.055],
        "signal",
    ))
    .build()
}

fn identity() -> Result<Part, ValidationError> {
    let mut part = Part::builder("metro:identity").element(detail(
        "backplate",
        [7.0, 1.25, 0.28],
        [0.0, 0.0, 0.0],
        "dark",
    ));
    for (i, x) in [0.4, 0.72, 1.04].into_iter().enumerate() {
        part = part.element(detail(
            &format!("mark-{i}"),
            [0.18, 0.77, 0.05],
            [x, 0.24, -0.05],
            "ink",
        ));
    }
    for (i, (x, width)) in [(1.6, 2.15), (3.95, 1.0), (5.15, 1.4)]
        .into_iter()
        .enumerate()
    {
        part = part.element(detail(
            &format!("designation-{i}"),
            [width, 0.12, 0.05],
            [x, 0.84, -0.05],
            "ink",
        ));
    }
    part = part
        .element(Element::new(
            "edge-frame",
            Geometry::cuboid([7.08, 1.33, 0.12])
                .placed(Pose::at([-0.04, -0.04, -0.03]))
                .subtract(Geometry::cuboid([6.92, 1.17, 0.3]).placed(Pose::at([0.04, 0.04, -0.1]))),
            "metal",
        ))
        .element(Element::new(
            "stand-offs",
            Geometry::cuboid([0.14, 0.9, 0.35])
                .placed(Pose::at([0.5, 0.15, 0.24]))
                .arrayed(2, Pose::at([5.8, 0.0, 0.0])),
            "metal",
        ));
    part.element(detail(
        "status-bar",
        [4.95, 0.045, 0.04],
        [1.6, 0.36, -0.04],
        "signal",
    ))
    .build()
}

#[expect(
    clippy::too_many_lines,
    reason = "kit parts and their unified material bindings"
)]
fn kit(id: &str) -> Result<BuildingBuilder, ValidationError> {
    let mut builder = Building::builder(id);
    for (slot, key) in [
        ("stone", "showcase:corporate-stone"),
        ("spine", "showcase:corporate-concrete"),
        ("reveal", "library:dark-recess"),
        ("metal", "library:painted-metal"),
        ("dark", "library:dark-recess"),
        ("glass", "library:glass"),
        ("light", "library:emissive-strip"),
        ("signal", "showcase:signal"),
        ("ink", "library:signage-ink"),
        ("paving", "library:paving-slabs"),
        ("rubble", "library:rubble"),
    ] {
        builder = builder.material(slot, key);
    }
    builder = builder.binding(
        "metal",
        Binding::new("library:painted-metal")
            .param("color", ParamValue::Color([0.048, 0.105, 0.155]))
            .param("wear", ParamValue::Float(0.2)),
    );
    builder = builder
        .part(glazed_bay(false)?)
        .part(glazed_bay(true)?)
        .part(lobby()?)
        .part(service_riser()?)
        .part(identity()?);
    // A parapet rail and a paving slab are walked past and over, not into.
    for (id, size, at, slot, collision) in [
        (
            "metro:base",
            [16.65, 0.45, 12.65],
            [-0.325, -0.47, -0.325],
            "stone",
            Collision::Bounds,
        ),
        (
            "metro:podium-cap",
            [17.2, 0.45, 13.2],
            [-0.6, 0.0, -0.6],
            "spine",
            Collision::Bounds,
        ),
        (
            "metro:tower-cap",
            [12.9, 0.5, 8.9],
            [-0.45, 0.0, -0.45],
            "spine",
            Collision::Bounds,
        ),
        (
            "metro:core-base",
            [2.6, PODIUM, 6.7],
            [0.0, 0.0, 0.0],
            "dark",
            Collision::Bounds,
        ),
        (
            "metro:core-crown",
            [2.95, 2.6, 7.1],
            [-0.175, 0.0, -0.2],
            "dark",
            Collision::Bounds,
        ),
        (
            "metro:rail",
            [4.0, 0.8, 0.22],
            [0.0, 0.0, -0.05],
            "metal",
            Collision::None,
        ),
        (
            "metro:paving",
            [8.0, 0.18, 8.0],
            [0.0, -0.2, 0.0],
            "paving",
            Collision::None,
        ),
    ] {
        let mut part =
            Part::builder(id).element(detail("solid", size, at, slot).collision(collision));
        if matches!(id, "metro:podium-cap" | "metro:tower-cap") {
            part =
                part.element(Element::new(
                    "drip-course",
                    Geometry::cuboid([size[0] + 0.08, 0.065, size[2] + 0.08])
                        .placed(Pose::at([at[0] - 0.04, at[1] + 0.09, at[2] - 0.04]))
                        .subtract(
                            Geometry::cuboid([size[0] - 0.12, 0.2, size[2] - 0.12])
                                .placed(Pose::at([at[0] + 0.06, at[1], at[2] + 0.06])),
                        ),
                    "metal",
                ));
        }
        if id == "metro:rail" {
            part = part
                .element(extrusion(
                    "handrail",
                    [4.0, 0.08, 0.30],
                    [0.0, 0.81, -0.09],
                    "metal",
                ))
                .element(Element::new(
                    "posts",
                    Geometry::cuboid([0.08, 0.82, 0.08])
                        .placed(Pose::at([0.12, 0.0, -0.08]))
                        .arrayed(3, Pose::at([1.84, 0.0, 0.0])),
                    "metal",
                ));
        }
        builder = builder.part(part.build()?);
    }
    let mut plant = Part::builder("metro:plant").element(solid(
        "body",
        [4.0, 2.4, 3.0],
        [0.0, 0.0, 0.0],
        "metal",
    ));
    for row in 0..6 {
        plant = plant.element(detail(
            &format!("cooling-{row}"),
            [3.5, 0.12, 0.2],
            [0.25, 0.4 + f64::from(row) * 0.28, -0.12],
            "reveal",
        ));
    }
    plant = plant
        .element(Element::new(
            "feet",
            Geometry::cuboid([0.32, 0.16, 3.16])
                .placed(Pose::at([0.32, -0.02, -0.08]))
                .arrayed(2, Pose::at([3.02, 0.0, 0.0])),
            "metal",
        ))
        .element(extrusion(
            "grille-head",
            [3.65, 0.10, 0.3],
            [0.175, 2.04, -0.20],
            "metal",
        ))
        .element(Element::new(
            "fan-cowls",
            Geometry::cylinder(0.62, 0.19, 16)
                .placed(Pose::at([1.0, 2.36, 1.5]))
                .arrayed(2, Pose::at([2.0, 0.0, 0.0])),
            "metal",
        ))
        .element(Element::new(
            "fan-grilles",
            Geometry::cuboid([0.85, 0.035, 0.045])
                .placed(Pose::at([0.575, 2.56, 1.22]))
                .arrayed(5, Pose::at([0.0, 0.0, 0.14]))
                .arrayed(2, Pose::at([2.0, 0.0, 0.0])),
            "reveal",
        ))
        .element(detail("duct", [1.15, 0.9, 1.3], [2.25, 0.3, 2.85], "metal"))
        .element(extrusion(
            "duct-collar",
            [1.23, 1.0, 0.12],
            [2.21, 0.25, 3.64],
            "metal",
        ));
    builder = builder.part(plant.build()?);
    Ok(builder)
}

fn ring(
    mut builder: BuildingBuilder,
    level: u32,
    origin: [f64; 3],
    bays: [u32; 2],
    podium: bool,
) -> BuildingBuilder {
    let width = f64::from(bays[0]) * 4.0;
    let depth = f64::from(bays[1]) * 4.0;
    for (side, count, corner, angle) in [
        ("front", bays[0], [0.0, 0.0, 0.0], 0.0),
        ("back", bays[0], [width, 0.0, depth], std::f64::consts::PI),
        (
            "left",
            bays[1],
            [0.0, 0.0, depth],
            std::f64::consts::FRAC_PI_2,
        ),
        (
            "right",
            bays[1],
            [width, 0.0, 0.0],
            -std::f64::consts::FRAC_PI_2,
        ),
    ] {
        let rotation = DQuat::from_rotation_y(angle);
        for bay in 0..count {
            if podium && side == "front" && (bay == 1 || bay == 2) {
                continue;
            }
            let at = DVec3::from_array(origin)
                + DVec3::from_array(corner)
                + rotation * DVec3::new(f64::from(bay) * 4.0, 0.0, 0.0);
            let mut instance = Instance::new(
                format!("{level}-{side}-{bay}"),
                if podium {
                    "metro:podium"
                } else {
                    "metro:curtain"
                },
            )
            .placed(Pose::at(at.to_array()).rotated(rotation));
            if !podium && (bay + level).is_multiple_of(3) {
                instance = instance.material("glass", "showcase:occupied-glass");
            }
            builder = builder.instance(instance);
        }
    }
    builder
}

/// The most upper storeys [`building`] assembles.
pub const MAX_STOREYS: u32 = 40;

/// Assemble one to [`MAX_STOREYS`] upper storeys over a double-height public
/// podium.
pub fn building(storeys: u32) -> Result<Building, ValidationError> {
    if !(1..=MAX_STOREYS).contains(&storeys) {
        return Err(ValidationError {
            path: "corporate.storeys".into(),
            reason: format!("expected 1..={MAX_STOREYS} upper storeys"),
        });
    }
    let mut builder = ring(
        kit(&format!("metro:tower-{storeys}"))?,
        0,
        [0.0, 0.0, 0.0],
        [4, 3],
        true,
    )
    .instance(Instance::new("base", "metro:base"))
    .instance(Instance::new("lobby", "metro:lobby").placed(Pose::at([4.0, 0.0, 0.0])))
    .instance(Instance::new("podium-cap", "metro:podium-cap").placed(Pose::at([0.0, 6.0, 0.0])))
    .instance(Instance::new("core-base", "metro:core-base").placed(Pose::at([13.35, 0.0, 5.0])))
    .instance(Instance::new("identity", "metro:identity").placed(Pose::at([4.5, 6.48, -0.52])));
    for floor in 0..storeys {
        let y = PODIUM + f64::from(floor) * STOREY;
        builder = ring(builder, floor + 1, [2.0, y, 2.0], [3, 2], false).instance(
            Instance::new(format!("core-{floor}"), "metro:service-riser")
                .placed(Pose::at([13.35, y, 5.0])),
        );
    }
    let roof = PODIUM + f64::from(storeys) * STOREY;
    builder = builder
        .instance(Instance::new("tower-cap", "metro:tower-cap").placed(Pose::at([2.0, roof, 2.0])))
        .instance(
            Instance::new("core-crown", "metro:core-crown").placed(Pose::at([13.35, roof, 5.0])),
        )
        .instance(Instance::new("roof-plant", "metro:plant").placed(Pose::at([
            5.0,
            roof + 0.5,
            4.0,
        ])));
    for (i, (at, yaw)) in [
        ([2.0, roof + 0.5, 2.0], 0.0),
        ([6.0, roof + 0.5, 2.0], 0.0),
        ([10.0, roof + 0.5, 2.0], 0.0),
        ([14.0, roof + 0.5, 10.0], std::f64::consts::PI),
        ([10.0, roof + 0.5, 10.0], std::f64::consts::PI),
        ([6.0, roof + 0.5, 10.0], std::f64::consts::PI),
    ]
    .into_iter()
    .enumerate()
    {
        builder = builder.instance(
            Instance::new(format!("rail-{i}"), "metro:rail")
                .placed(Pose::at(at).rotated(DQuat::from_rotation_y(yaw))),
        );
    }
    builder.build()
}

/// How far along the endless render each building of the block stands.
///
/// `library:stone-cladding`'s `variation` offsets the per-slab identity each
/// slab's tone is hashed from, so two values of it are two shuffles of one wall
/// rather than two materials —
/// which is exactly what a row of buildings in one street wants, and why this
/// is one material key at three values rather than three keys. The tower takes
/// the library's own value, because a tower on its own is what the `corporate`
/// scene shows and it should be the same wall there as here.
const VARIATION: [f32; 2] = [0.37, 0.71];

/// A tower and two lower wings around a paved forecourt, reusing one part library.
pub fn block() -> Result<Building, ValidationError> {
    let mut builder = kit("metro:city-block")?;
    for (name, floors, at, yaw, stone) in [
        ("tower", 4, [0.0, 0.0, 16.0], 0.0, None),
        (
            "annex",
            1,
            [-4.0, 0.0, -4.0],
            -std::f64::consts::FRAC_PI_2,
            Some(VARIATION[0]),
        ),
        (
            "wing",
            2,
            [32.0, 0.0, 12.0],
            std::f64::consts::FRAC_PI_2,
            Some(VARIATION[1]),
        ),
    ] {
        let pose = Pose::at(at).rotated(DQuat::from_rotation_y(yaw));
        let assembled = building(floors)?;
        // Which instances actually carry the slot, asked of the building
        // before it is taken apart: an override naming a slot the part does not
        // declare is a validation error, and most of this kit's parts are trim.
        let clad: Vec<bool> = assembled
            .recipe()
            .instances
            .iter()
            .map(|instance| assembled.material(&instance.id, "stone").is_some())
            .collect();
        for (mut instance, clad) in assembled.into_recipe().instances.into_iter().zip(clad) {
            instance.id = format!("{name}/{}", instance.id);
            instance.pose = Pose {
                translation: pose.transform_point(instance.pose.translation),
                rotation: pose.rotation * instance.pose.rotation,
            };
            // Each building renders its own wall, and all three wear one
            // material key: an instance override puts this building's own
            // `variation` on `showcase:corporate-stone` rather than naming a second
            // definition that differs from it in one number. The renderer keys
            // its texture sets by the binding, so the block is three bakes and
            // three cache entries however many bays ask for them, and a fourth
            // building at one of these values would pay for nothing at all.
            if let (Some(variation), true) = (stone, clad) {
                instance = instance.binding(
                    "stone",
                    Binding::new("showcase:corporate-stone")
                        .param("variation", ParamValue::Float(variation)),
                );
            }
            builder = builder.instance(instance);
        }
    }
    for x in -1..4 {
        for z in -1..2 {
            builder = builder.instance(
                Instance::new(format!("paving-{x}-{z}"), "metro:paving").placed(Pose::at([
                    f64::from(x) * 8.0,
                    0.0,
                    f64::from(z) * 8.0,
                ])),
            );
        }
    }
    builder.build()
}

/// Which storey band a pose's height falls in: 0 at or below the podium, then
/// one band per `STOREY` above it. The `1e-9` keeps a pose exactly on a floor
/// line in the band the authored height meant rather than the one below it.
#[allow(
    clippy::cast_possible_truncation,
    reason = "a storey index is a small positive count"
)]
fn storey_band(y: f64) -> i32 {
    if y < PODIUM - 1e-9 {
        0
    } else {
        1 + ((y - PODIUM + 1e-9) / STOREY).floor() as i32
    }
}

/// The same building with its elements unioned by storey.
///
/// The kit is authored in storeys, a storey boundary is a floor line where a
/// seam is honest, and the group is also the unit a hit will re-mesh, so its
/// size is the cost of one.
pub fn merged_by_storey(building: Building) -> Result<Building, ValidationError> {
    let mut recipe = building.into_recipe();
    recipe.merged = true;
    let mut declared: Vec<(String, i32)> = Vec::new();
    for instance in &mut recipe.instances {
        let prefix = match instance.id.rfind('/') {
            Some(slash) => &instance.id[..=slash],
            None => "",
        };
        let band = storey_band(instance.pose.translation.y);
        let id = format!("{prefix}storey-{band}");
        if !declared.iter().any(|(seen, _)| seen == &id) {
            declared.push((id.clone(), band));
        }
        instance.group = Some(id);
    }
    recipe.groups = declared
        .into_iter()
        .map(|(id, band)| MergeGroup::new(id).storey(band))
        .collect();
    // Glazing and lamps are what a game swaps, animates or draws with another
    // material model, so they stay their own meshes.
    for part in &mut recipe.parts {
        for element in &mut part.elements {
            if matches!(element.material_slot.as_str(), "glass" | "light" | "signal") {
                element.standalone = true;
            }
        }
    }
    recipe.build()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn tower_annex_and_block_share_the_same_parts() {
        let tower = building(4).expect("tower");
        let annex = building(1).expect("annex");
        let district = block().expect("block");
        assert_eq!(tower.recipe().parts, annex.recipe().parts);
        assert_eq!(tower.recipe().parts, district.recipe().parts);
        assert!(tower.recipe().instances.len() > annex.recipe().instances.len());
        assert!(district.recipe().instances.len() > tower.recipe().instances.len() * 2);
        assert!(building(0).is_err() && building(MAX_STOREYS + 1).is_err());
        assert!(building(MAX_STOREYS).is_ok());
    }

    #[test]
    fn each_building_of_the_block_renders_its_own_wall() {
        let district = block().expect("block");
        let mut seen = std::collections::BTreeSet::new();
        for instance in &district.recipe().instances {
            if let Some(binding) = district.binding(&instance.id, "stone") {
                seen.insert(binding.clone());
                // One key over the whole block, and the seed on the binding.
                assert_eq!(
                    binding.material, "showcase:corporate-stone",
                    "{}",
                    instance.id
                );
                let building = instance.id.split('/').next().expect("a prefixed id");
                let expected = match building {
                    "annex" => Some(ParamValue::Float(VARIATION[0])),
                    "wing" => Some(ParamValue::Float(VARIATION[1])),
                    _ => None,
                };
                assert_eq!(
                    binding.params.get("variation").copied(),
                    expected,
                    "{}",
                    instance.id
                );
            }
        }
        assert_eq!(seen.len(), 3, "three seeds over three buildings: {seen:?}");
        // The paving is shared: it is one forecourt, not three.
        assert_eq!(
            district.material("paving-0-0", "paving"),
            Some("library:paving-slabs")
        );
    }
}
