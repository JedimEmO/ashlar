//! The kit's enterable buildings: adobe domes, a two-storey dome house, the
//! capsule hab pod and its walkway tube, the prefab module and the hab tower.
//!
//! Each is built the ADR 0005 way, as the house in `interior.rs` is: walls,
//! floors and ceilings are elements added together, the inner faces carry
//! `interior()` liners on the `spine` slot, floors carry an interior finish on
//! `floor`, and only openings are subtracted. Every opening a person walks
//! through is a portal on the wall's own cutter, and every room is a
//! [`Room`] declared on the building by [`Habitat::place`].
//!
//! # Round walls and their proxies
//!
//! A collision proxy is convex, so the hull of a whole drum would fill the
//! room inside it. A round wall is therefore a ring of wedges, each a
//! [`Geometry::revolve_arc`] of the wall's section with its own `Hull`, and a
//! wedge's hull is a thin slab that stops within a few centimetres of the
//! wall's inner face. A doorway is one more sector of the same ring, exactly
//! as wide as the door, which carries the door's cutter — and so its portal —
//! and no proxy at all: a proxy over it would close the door to anything that
//! walks. Flat walls are split the same way, by `keep`, into runs either
//! side of a door and the strip that holds it.
//!
//! # Angles
//!
//! Angles here are degrees, measured the way a revolve sweeps: from +X
//! towards -Z, which is a positive turn about Y. A piece's front, where its
//! door is, faces -Z, at 90 degrees.
use std::f64::consts::{FRAC_PI_2, PI};

use ashlar::{
    Axis, Binding, BuildingBuilder, Collision, Element, Geometry, Instance, MergeGroup, ParamValue,
    Part, Pose, Room, Socket, UvMode, ValidationError,
};
use glam::DQuat;

use super::{ROUND, SMALL, along_x, arc, at, beacon, bore, luminous, plate, turned, yaw};

/// Thickness of every interior finish.
const LINER: f64 = 0.02;
/// Clear width and height of a kit door: wider and taller than the house's
/// 1.0 by 2.1, as the kit's chunkier proportions ask.
const DOOR_WIDTH: f64 = 1.2;
const DOOR_HEIGHT: f64 = 2.3;

/// The storey groups every kit building declares, so a camera can cut the
/// world storey by storey whichever building it is looking into.
pub(crate) const GROUPS: [(&str, i32); 4] = [("ground", 0), ("upper", 1), ("top", 2), ("roof", 3)];

/// Declare [`GROUPS`] on a building.
pub(crate) fn declare_groups(mut builder: BuildingBuilder) -> BuildingBuilder {
    for (id, storey) in GROUPS {
        builder = builder.group(MergeGroup::new(id).storey(storey));
    }
    builder
}

// ------------------------------------------------------------------ helpers

/// A rotation about Y by `degrees`, in the sweep's sense.
fn turn(degrees: f64) -> DQuat {
    yaw(degrees.to_radians())
}

/// The part of `geometry` inside the axis box from `min` to `max`, as the
/// geometry with the rest of space subtracted. A bound of infinity leaves
/// that side open. The faces the split makes are cut faces, and they meet the
/// neighbouring run's flush.
fn keep(geometry: Geometry, min: [f64; 3], max: [f64; 3]) -> Geometry {
    const FAR: f64 = 200.0;
    let mut cutters = Vec::new();
    for axis in 0..3 {
        let slab = |from: f64, to: f64| {
            let mut size = [2.0 * FAR; 3];
            let mut corner = [-FAR; 3];
            size[axis] = to - from;
            corner[axis] = from;
            Geometry::cuboid(size).placed(Pose::at(corner))
        };
        if min[axis].is_finite() {
            cutters.push(slab(min[axis] - FAR, min[axis]));
        }
        if max[axis].is_finite() {
            cutters.push(slab(max[axis], max[axis] + FAR));
        }
    }
    match cutters.len() {
        0 => geometry,
        1 => geometry.subtract(cutters.remove(0)),
        _ => geometry.subtract(Geometry::union_all(cutters)),
    }
}

/// An opening in a round wall: the angle of its centre and its half-width in
/// degrees, the cutter in the part's frame, and the portal it publishes.
struct Opening {
    angle: f64,
    half: f64,
    cutter: Geometry,
    portal: Option<&'static str>,
}

/// Whether an opening overlaps the sector from `from` to `to` degrees.
fn overlaps(opening: &Opening, from: f64, to: f64) -> bool {
    (-1..=1).any(|lap: i32| {
        let centre = opening.angle + 360.0 * f64::from(lap);
        centre + opening.half > from && centre - opening.half < to
    })
}

/// Whether an opening's centre lies in the sector from `from` to `to`.
fn centred(opening: &Opening, from: f64, to: f64) -> bool {
    (-1..=1).any(|lap: i32| {
        let centre = opening.angle + 360.0 * f64::from(lap);
        centre >= from && centre < to
    })
}

/// One sector of a round wall, `profile` turned from `from` to `to` degrees
/// in the frame `frame`, with every opening that reaches it subtracted. The
/// opening centred in it publishes its portal.
fn sector(
    profile: &[[f64; 2]],
    from: f64,
    to: f64,
    frame: DQuat,
    openings: &[Opening],
) -> Geometry {
    let mut geometry = Geometry::revolve_arc(profile.to_vec(), ROUND, to - from)
        .placed(Pose::default().rotated(frame * turn(from)));
    for opening in openings
        .iter()
        .filter(|opening| overlaps(opening, from, to))
    {
        let mut cutter = opening.cutter.clone();
        if let (Some(id), true) = (opening.portal, centred(opening, from, to)) {
            cutter = cutter.portal(id);
        }
        geometry = geometry.subtract(cutter);
    }
    geometry
}

/// A round wall from `from` to `to` degrees as wedges no wider than `wedge`,
/// each with a hull proxy.
#[expect(clippy::too_many_arguments, reason = "one ring, described in full")]
fn ring(
    id: &str,
    profile: &[[f64; 2]],
    from: f64,
    to: f64,
    wedge: f64,
    frame: DQuat,
    openings: &[Opening],
    slot: &str,
    cut: &str,
    axis: Axis,
) -> Vec<Element> {
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "a small positive count of wedges"
    )]
    let count = ((to - from) / wedge).ceil().max(1.0) as u32;
    let step = (to - from) / f64::from(count);
    (0..count)
        .map(|index| {
            let start = from + step * f64::from(index);
            Element::new(
                format!("{id}-{index}"),
                sector(profile, start, start + step, frame, openings),
                slot,
            )
            .cut_material(cut)
            .uv(UvMode::Cylindrical { axis })
            .collision(Collision::Hull)
        })
        .collect()
}

/// The sector of a round wall that holds a door: its cutter and portal, and
/// no proxy, so the doorway can be walked through.
#[expect(clippy::too_many_arguments, reason = "one sector, described in full")]
fn doorway(
    id: &str,
    profile: &[[f64; 2]],
    centre: f64,
    width: f64,
    frame: DQuat,
    openings: &[Opening],
    slot: &str,
    cut: &str,
    axis: Axis,
) -> Element {
    Element::new(
        id,
        sector(
            profile,
            centre - width / 2.0,
            centre + width / 2.0,
            frame,
            openings,
        ),
        slot,
    )
    .cut_material(cut)
    .uv(UvMode::Cylindrical { axis })
}

/// A round wall with one door: the door sector and the wedges round the rest.
#[expect(clippy::too_many_arguments, reason = "one wall, described in full")]
fn round_wall(
    id: &str,
    profile: &[[f64; 2]],
    door: f64,
    door_sector: f64,
    wedge: f64,
    frame: DQuat,
    openings: &[Opening],
    slot: &str,
    cut: &str,
    axis: Axis,
) -> Vec<Element> {
    let mut elements = vec![doorway(
        &format!("{id}-door"),
        profile,
        door,
        door_sector,
        frame,
        openings,
        slot,
        cut,
        axis,
    )];
    elements.extend(ring(
        id,
        profile,
        door + door_sector / 2.0,
        door - door_sector / 2.0 + 360.0,
        wedge,
        frame,
        openings,
        slot,
        cut,
        axis,
    ));
    elements
}

/// A thin revolved finish with every opening subtracted, no portals and no
/// proxy: the liner over a round wall's inner face.
fn round_liner(id: &str, profile: Vec<[f64; 2]>, openings: &[Opening], frame: DQuat) -> Element {
    let mut geometry = Geometry::revolve(profile, ROUND).placed(Pose::default().rotated(frame));
    for opening in openings {
        geometry = geometry.subtract(opening.cutter.clone());
    }
    Element::new(id, geometry, "spine")
        .interior()
        .uv(UvMode::Cylindrical { axis: Axis::Y })
}

/// A disc of floor finish from `bottom` to `top` of `radius`, walkable.
fn round_floor(id: &str, radius: f64, bottom: f64, top: f64) -> Element {
    Element::new(
        id,
        Geometry::revolve(
            [[0.0, bottom], [radius, bottom], [radius, top], [0.0, top]],
            ROUND,
        ),
        "floor",
    )
    .interior()
    .uv(UvMode::Box)
    .collision(Collision::Hull)
}

/// An arched door cutter, `width` by `height`, through a wall from `inner` to
/// `outer` metres off the part's axis, on the side at `angle` degrees.
fn arch_cutter(
    width: f64,
    height: f64,
    inner: f64,
    outer: f64,
    angle: f64,
    floor: f64,
) -> Geometry {
    at(
        super::arch(width, height, outer - inner),
        rotated_about_y(angle, Pose::at([0.0, floor, -inner])),
    )
}

/// A pose at `local`, for the front at -Z, swung round to face `angle`.
fn rotated_about_y(angle: f64, local: Pose) -> Pose {
    super::around((angle - 90.0).to_radians(), local)
}

/// An instance of a furniture part at a local pose within a building placed
/// at `origin`, in `group`.
fn furnish(
    builder: BuildingBuilder,
    id: String,
    part: &str,
    origin: Pose,
    local: Pose,
    group: &str,
) -> BuildingBuilder {
    builder.instance(
        Instance::new(id, part)
            .placed(origin.compose(local))
            .group(group),
    )
}

/// A local pose on a floor at height `y`, at `(x, z)`, turned `degrees` about Y.
fn spot(x: f64, y: f64, z: f64, degrees: f64) -> Pose {
    Pose::at([x, y, z]).rotated(turn(degrees))
}

/// A room from `min` to `max` in the building's local frame, placed with it.
fn room(
    id: String,
    origin: Pose,
    min: [f64; 3],
    max: [f64; 3],
    group: &str,
    portals: &[&str],
) -> Room {
    let mut room = Room::new(id, [max[0] - min[0], max[1] - min[1], max[2] - min[2]])
        .placed(origin.compose(Pose::at(min)))
        .group(group);
    for portal in portals {
        room = room.portal(*portal);
    }
    room
}

// ------------------------------------------------------------------ domes

/// The proportions of one adobe dome.
#[derive(Clone, Copy, Debug)]
struct Dome {
    /// Outer radius of the drum.
    radius: f64,
    /// Height of the drum, where the dome springs.
    drum: f64,
    /// Rise of the dome over the drum.
    rise: f64,
    /// Wall thickness.
    wall: f64,
    /// Door sector, in degrees.
    door_sector: f64,
    /// Widest wedge, in degrees.
    wedge: f64,
    /// Porthole angles.
    ports: &'static [f64],
}

const HUT: Dome = Dome {
    radius: 3.2,
    drum: 2.75,
    rise: 2.3,
    wall: 0.35,
    door_sector: 30.0,
    wedge: 30.0,
    ports: &[30.0, 150.0],
};

const ANNEX: Dome = Dome {
    radius: 1.95,
    drum: 2.6,
    rise: 1.35,
    wall: 0.3,
    door_sector: 48.0,
    wedge: 45.0,
    ports: &[270.0],
};

/// Height of a dome's porthole centres.
const PORT_HEIGHT: f64 = 1.65;

impl Dome {
    fn inner(self) -> f64 {
        self.radius - self.wall
    }

    fn door_width(self) -> f64 {
        if self.radius < 2.5 { 1.0 } else { DOOR_WIDTH }
    }

    /// The drum's section: flared at its foot, straight above.
    fn drum_profile(self) -> Vec<[f64; 2]> {
        let r = self.radius;
        vec![
            [self.inner(), 0.0],
            [1.12 * r, 0.0],
            [1.07 * r, 0.12 * r],
            [r, 0.22 * r],
            [r, self.drum],
            [self.inner(), self.drum],
        ]
    }

    /// The dome's section: an elliptical shell from the drum to the crown.
    fn cap_profile(self) -> Vec<[f64; 2]> {
        let mut profile = arc(
            [0.0, self.drum],
            [self.radius, self.rise],
            0.0,
            FRAC_PI_2,
            14,
        );
        let mut inner = arc(
            [0.0, self.drum],
            [self.inner(), self.rise - self.wall],
            0.0,
            FRAC_PI_2,
            14,
        );
        inner.reverse();
        profile.extend(inner);
        profile
    }

    /// The finish under the dome.
    fn cap_liner(self) -> Vec<[f64; 2]> {
        let mut profile = arc(
            [0.0, self.drum],
            [self.inner(), self.rise - self.wall],
            0.0,
            FRAC_PI_2,
            14,
        );
        let mut inner = arc(
            [0.0, self.drum],
            [self.inner() - LINER, self.rise - self.wall - LINER],
            0.0,
            FRAC_PI_2,
            14,
        );
        inner.reverse();
        profile.extend(inner);
        profile
    }

    fn crown(self) -> f64 {
        self.drum + self.rise
    }

    /// The inside of the dome's crown, where a ceiling light hangs.
    fn ceiling(self) -> f64 {
        self.drum + self.rise - self.wall - LINER
    }

    /// The door and the portholes.
    fn openings(self) -> Vec<Opening> {
        let r = self.radius;
        let mut openings = vec![Opening {
            angle: 90.0,
            half: self.door_sector / 2.0,
            cutter: arch_cutter(
                self.door_width(),
                DOOR_HEIGHT,
                self.inner() - 0.2,
                1.12 * r + 0.3,
                90.0,
                0.0,
            ),
            portal: Some("door"),
        }];
        for angle in self.ports {
            openings.push(Opening {
                angle: *angle,
                half: 8.0,
                cutter: at(
                    bore(0.3, self.wall + 0.6, SMALL),
                    rotated_about_y(*angle, Pose::at([0.0, PORT_HEIGHT, -(self.inner() - 0.2)])),
                ),
                portal: None,
            });
        }
        openings
    }

    /// The room inside: the square the drum's liner inscribes, less a margin.
    fn room(self) -> ([f64; 3], [f64; 3]) {
        let half = (self.inner() - LINER - 0.12) / std::f64::consts::SQRT_2;
        ([-half, 0.05, -half], [half, self.drum - 0.02, half])
    }
}

/// An arched door ring standing just proud of a wall: two legs and the arch
/// over them round a `width` by `height` opening, `depth` deep along -Z from
/// its origin, lifted a centimetre so its feet share no plane with the wall's.
fn door_ring(width: f64, height: f64, depth: f64) -> Geometry {
    at(
        super::arch(width + 0.36, height + 0.18, depth).subtract(at(
            super::arch(width, height, depth + 0.4),
            Pose::at([0.0, -0.02, 0.2]),
        )),
        Pose::at([0.0, 0.01, 0.0]),
    )
}

/// A dome's drum as one part: wedged drum, door sector, finishes, floor,
/// portholes, door ring, threshold and the utility box on its back.
fn dome_part(id: &str, dome: Dome) -> Result<Part, ValidationError> {
    let r = dome.radius;
    let openings = dome.openings();
    let mut part = Part::builder(id);
    for element in round_wall(
        "drum",
        &dome.drum_profile(),
        90.0,
        dome.door_sector,
        dome.wedge,
        DQuat::IDENTITY,
        &openings,
        "adobe",
        "adobe",
        Axis::Y,
    ) {
        part = part.element(element);
    }
    let base = 1.12 * r;
    let door = dome.door_width();
    part = part
        .element(round_liner(
            "liner",
            vec![
                [dome.inner() - LINER, 0.05],
                [dome.inner(), 0.05],
                [dome.inner(), dome.drum],
                [dome.inner() - LINER, dome.drum],
            ],
            &openings,
            DQuat::IDENTITY,
        ))
        .element(round_floor("floor", dome.inner(), 0.0, 0.05))
        .element(Element::new(
            "door-ring",
            at(
                door_ring(door, DOOR_HEIGHT, 0.34),
                Pose::at([0.0, 0.0, -(r - 0.12)]),
            ),
            "trim",
        ))
        .element(Element::new(
            "door-light",
            luminous(
                [door * 0.5, 0.05, 0.06],
                [-door * 0.25, DOOR_HEIGHT + 0.26, -r - 0.24],
            ),
            "light",
        ))
        .element(Element::new(
            "threshold",
            plate(
                [door + 0.3, 0.045, base - dome.inner() + 0.5],
                [-door / 2.0 - 0.15, 0.005, -base - 0.5],
            ),
            "concrete",
        ));
    for (index, angle) in dome.ports.iter().enumerate() {
        let local = rotated_about_y(*angle, Pose::at([0.0, PORT_HEIGHT, -(r - 0.02)]));
        part = part
            .element(
                Element::new(
                    format!("port-glass-{index}"),
                    at(
                        bore(0.3, 0.04, SMALL),
                        rotated_about_y(
                            *angle,
                            Pose::at([0.0, PORT_HEIGHT, -(dome.inner() + 0.1)]),
                        ),
                    ),
                    "glass",
                )
                .standalone(),
            )
            .element(Element::new(
                format!("port-rim-{index}"),
                at(
                    bore(0.39, 0.14, SMALL)
                        .subtract(at(bore(0.3, 0.3, SMALL), Pose::at([0.0, 0.0, 0.08]))),
                    local,
                ),
                "trim",
            ));
    }
    if r > 2.5 {
        // A utility box on the back wall, its feed pipe running up the dome.
        let unit = 0.28 * r;
        part = part
            .element(
                Element::new(
                    "utility",
                    plate([2.0 * unit, 1.2 * unit, unit], [-unit, 0.02, r - 0.25]),
                    "hull",
                )
                .uv(UvMode::Box)
                .collision(Collision::Bounds),
            )
            .element(Element::new(
                "utility-pipe",
                Geometry::cylinder(0.08, 0.5 * r + 0.8, SMALL).placed(Pose::at([
                    0.4 * unit,
                    1.2 * unit,
                    r + 0.3,
                ])),
                "rust",
            ))
            .element(Element::new(
                "utility-light",
                luminous([0.3, 0.06, 0.04], [-0.15, 0.9 * unit, r + unit - 0.23]),
                "light",
            ));
    }
    part.element(Element::new(
        "door-hood",
        at(
            door_ring(door + 0.04, DOOR_HEIGHT + 0.03, 0.48),
            Pose::at([0.0, 0.03, -(r - 0.23)]),
        ),
        "adobe",
    ))
    .element(Element::new(
        "entry-reader",
        super::service_cover([door / 2.0 + 0.22, 1.05, -r - 0.13], 0.24, 0.4),
        "trim",
    ))
    .element(
        Element::new(
            "reader-light",
            luminous([0.12, 0.05, 0.03], [door / 2.0 + 0.28, 1.32, -r - 0.15]),
            "warning",
        )
        .far(),
    )
    .element(Element::new(
        "reader-mount",
        plate([0.28, 0.44, 0.4], [door / 2.0 + 0.2, 1.03, -r - 0.08]),
        "trim",
    ))
    .build()
}

/// A dome's roof as one part, in the storey above its room as a ceiling is:
/// the dome, its finish, the cornice and the roof vent.
fn dome_roof(id: &str, dome: Dome) -> Result<Part, ValidationError> {
    let r = dome.radius;
    let crown = dome.crown();
    Part::builder(id)
        .element(
            Element::new(
                "dome",
                Geometry::revolve(dome.cap_profile(), ROUND),
                "adobe",
            )
            .uv(UvMode::Cylindrical { axis: Axis::Y })
            .collision(Collision::Hull),
        )
        .element(
            Element::new(
                "dome-liner",
                Geometry::revolve(dome.cap_liner(), ROUND),
                "spine",
            )
            .interior()
            .uv(UvMode::Cylindrical { axis: Axis::Y }),
        )
        .element(turned(
            "cornice",
            vec![
                [r - 0.05, dome.drum - 0.14],
                [r + 0.1, dome.drum - 0.14],
                [r + 0.12, dome.drum - 0.04],
                [r + 0.02, dome.drum + 0.12],
                [r - 0.05, dome.drum + 0.1],
            ],
            "adobe",
        ))
        .element(
            Element::new(
                "vent",
                Geometry::cylinder(0.12 * r, 0.3 * r, SMALL).placed(Pose::at([
                    0.0,
                    crown - 0.1,
                    0.0,
                ])),
                "trim",
            )
            .uv(UvMode::Cylindrical { axis: Axis::Y }),
        )
        .element(
            Element::new(
                "vent-cap",
                Geometry::cylinder(0.18 * r, 0.05 * r, SMALL).placed(Pose::at([
                    0.0,
                    crown + 0.2 * r - 0.1,
                    0.0,
                ])),
                "rust",
            )
            .uv(UvMode::Cylindrical { axis: Axis::Y }),
        )
        .element(Element::new(
            "vent-louvres",
            Geometry::revolve(
                [
                    [0.115 * r, 0.0],
                    [0.155 * r, 0.0],
                    [0.155 * r, 0.06],
                    [0.115 * r, 0.06],
                ],
                SMALL,
            )
            .placed(Pose::at([0.0, crown + 0.03, 0.0]))
            .arrayed(3, Pose::at([0.0, 0.12, 0.0])),
            "trim",
        ))
        .build()
}

// ------------------------------------------------------------------ dome house

/// The two-storey dome house: its ground drum, the deck that is the ground
/// storey's ceiling, the upper storey's floor and a terrace, and the upper
/// drum and dome.
const TALL_RADIUS: f64 = 4.1;
const TALL_INNER: f64 = 3.75;
const TALL_DRUM: f64 = 3.4;
const TALL_DECK: f64 = 3.7;
const UPPER_RADIUS: f64 = 2.5;
const UPPER_INNER: f64 = 2.2;
const UPPER_DRUM: f64 = TALL_DECK + 2.45;
const UPPER_RISE: f64 = 1.7;
/// The hatch the ladder climbs through, in the deck.
const HATCH: ([f64; 2], [f64; 2]) = ([0.6, -0.4], [1.3, 0.4]);

fn tall_ground() -> Result<Part, ValidationError> {
    let profile = vec![
        [TALL_INNER, 0.0],
        [4.6, 0.0],
        [4.4, 0.4],
        [TALL_RADIUS, 0.7],
        [TALL_RADIUS, 3.2],
        [3.95, TALL_DRUM],
        [TALL_INNER, TALL_DRUM],
    ];
    let mut openings = vec![Opening {
        angle: 90.0,
        half: 12.0,
        cutter: arch_cutter(DOOR_WIDTH, DOOR_HEIGHT, TALL_INNER - 0.2, 4.9, 90.0, 0.0),
        portal: Some("door"),
    }];
    for angle in [30.0, 150.0, 210.0, 330.0] {
        openings.push(Opening {
            angle,
            half: 6.0,
            cutter: at(
                bore(0.32, 1.0, SMALL),
                rotated_about_y(angle, Pose::at([0.0, 1.8, -(TALL_INNER - 0.2)])),
            ),
            portal: None,
        });
    }
    let mut part = Part::builder("scifi:dome-tall-ground");
    for element in round_wall(
        "drum",
        &profile,
        90.0,
        24.0,
        30.0,
        DQuat::IDENTITY,
        &openings,
        "adobe",
        "adobe",
        Axis::Y,
    ) {
        part = part.element(element);
    }
    part = part
        .element(round_liner(
            "liner",
            vec![
                [TALL_INNER - LINER, 0.05],
                [TALL_INNER, 0.05],
                [TALL_INNER, TALL_DRUM],
                [TALL_INNER - LINER, TALL_DRUM],
            ],
            &openings,
            DQuat::IDENTITY,
        ))
        .element(round_floor("floor", TALL_INNER, 0.0, 0.05))
        .element(Element::new(
            "door-ring",
            at(
                door_ring(DOOR_WIDTH, DOOR_HEIGHT, 0.34),
                Pose::at([0.0, 0.0, -(TALL_RADIUS - 0.12)]),
            ),
            "trim",
        ))
        .element(Element::new(
            "threshold",
            plate([1.7, 0.045, 1.5], [-0.85, 0.005, -5.3]),
            "concrete",
        ))
        .element(Element::new(
            "door-light",
            luminous(
                [0.6, 0.05, 0.06],
                [-0.3, DOOR_HEIGHT + 0.26, -TALL_RADIUS - 0.24],
            ),
            "light",
        ));
    for (index, angle) in [30.0, 150.0, 210.0, 330.0].into_iter().enumerate() {
        part = part.element(
            Element::new(
                format!("port-glass-{index}"),
                at(
                    bore(0.32, 0.04, SMALL),
                    rotated_about_y(angle, Pose::at([0.0, 1.8, -(TALL_INNER + 0.1)])),
                ),
                "glass",
            )
            .standalone(),
        );
        part = part.element(Element::new(
            format!("port-rim-{index}"),
            at(
                bore(0.42, 0.14, SMALL)
                    .subtract(at(bore(0.32, 0.3, SMALL), Pose::at([0.0, 0.0, 0.08]))),
                rotated_about_y(angle, Pose::at([0.0, 1.8, -(TALL_RADIUS - 0.02)])),
            ),
            "trim",
        ));
    }
    // The ladder inside, from the floor up through the hatch, and the one
    // outside at the back from the ground to the terrace.
    let ladder = |height: f64| {
        Geometry::union_all([
            Geometry::cuboid([0.44, 0.04, 0.04])
                .placed(Pose::at([-0.22, 0.35, -0.02]))
                .arrayed(
                    #[expect(
                        clippy::cast_possible_truncation,
                        clippy::cast_sign_loss,
                        reason = "a small positive rung count"
                    )]
                    {
                        (height / 0.33) as u32
                    },
                    Pose::at([0.0, 0.33, 0.0]),
                ),
            Geometry::cuboid([0.05, height, 0.05]).placed(Pose::at([-0.27, 0.0, -0.025])),
            Geometry::cuboid([0.05, height, 0.05]).placed(Pose::at([0.22, 0.0, -0.025])),
        ])
    };
    let (low, high) = HATCH;
    part = part
        .element(
            Element::new(
                "inner-ladder",
                at(
                    ladder(TALL_DECK + 0.9),
                    Pose::at([f64::midpoint(low[0], high[0]), 0.05, high[1] - 0.05])
                        .rotated(turn(0.0)),
                ),
                "rust",
            )
            .interior()
            .standalone(),
        )
        .element(Element::new(
            "outer-ladder",
            at(
                ladder(TALL_DECK + 1.0),
                Pose::at([0.0, 0.0, 4.42]).rotated(turn(180.0)),
            ),
            "rust",
        ));
    part.element(Element::new(
        "ladder-standoffs",
        plate([0.68, 0.09, 0.46], [-0.34, 0.9, 4.05]).arrayed(3, Pose::at([0.0, 0.9, 0.0])),
        "rust",
    ))
    .element(Element::new(
        "entry-reader",
        super::service_cover([0.92, 1.1, -4.17], 0.28, 0.46),
        "trim",
    ))
    .build()
}

fn tall_deck() -> Result<Part, ValidationError> {
    let (low, high) = HATCH;
    let disc = || {
        Geometry::revolve(
            [
                [0.0, TALL_DRUM],
                [4.0, TALL_DRUM],
                [4.0, TALL_DECK],
                [0.0, TALL_DECK],
            ],
            ROUND,
        )
    };
    let inf = f64::INFINITY;
    let pieces = [
        ("deck-west", [-inf, -inf, -inf], [low[0], inf, inf]),
        ("deck-east", [high[0], -inf, -inf], [inf, inf, inf]),
        ("deck-south", [low[0], -inf, -inf], [high[0], inf, low[1]]),
        ("deck-north", [low[0], -inf, high[1]], [high[0], inf, inf]),
    ];
    let mut part = Part::builder("scifi:dome-tall-deck");
    for (id, min, max) in pieces {
        part = part.element(
            Element::new(id, keep(disc(), min, max), "adobe")
                .uv(UvMode::Box)
                .collision(Collision::Hull),
        );
    }
    let hatch = |bottom: f64| {
        Geometry::cuboid([high[0] - low[0], 0.2, high[1] - low[1]])
            .placed(Pose::at([low[0], bottom, low[1]]))
    };
    part = part
        // The ground storey's ceiling, under the deck and in the deck's group.
        .element(
            Element::new(
                "ceiling",
                Geometry::revolve(
                    [
                        [0.0, TALL_DRUM - LINER],
                        [TALL_INNER, TALL_DRUM - LINER],
                        [TALL_INNER, TALL_DRUM],
                        [0.0, TALL_DRUM],
                    ],
                    ROUND,
                )
                .subtract(hatch(TALL_DRUM - 0.1)),
                "spine",
            )
            .interior()
            .uv(UvMode::Box),
        )
        // The upper storey's floor finish over the deck, inside its drum.
        .element(
            Element::new(
                "upper-floor",
                Geometry::revolve(
                    [
                        [0.0, TALL_DECK],
                        [UPPER_INNER, TALL_DECK],
                        [UPPER_INNER, TALL_DECK + LINER],
                        [0.0, TALL_DECK + LINER],
                    ],
                    ROUND,
                )
                .subtract(hatch(TALL_DECK - 0.1)),
                "floor",
            )
            .interior()
            .uv(UvMode::Box),
        )
        .element(
            Element::new(
                "roof-light",
                Geometry::cylinder(UPPER_RADIUS + 0.05, 0.06, ROUND).placed(Pose::at([
                    0.0,
                    TALL_DECK + 0.01,
                    0.0,
                ])),
                "light",
            )
            .uv(UvMode::Cylindrical { axis: Axis::Y }),
        );
    // The terrace parapet, open at the back where the outer ladder arrives.
    let parapet = [
        [3.8, TALL_DECK],
        [4.0, TALL_DECK],
        [4.0, TALL_DECK + 0.95],
        [3.8, TALL_DECK + 0.95],
    ];
    for element in ring(
        "parapet",
        &parapet,
        270.0 + 10.0,
        270.0 - 10.0 + 360.0,
        30.0,
        DQuat::IDENTITY,
        &[],
        "adobe",
        "adobe",
        Axis::Y,
    ) {
        part = part.element(element);
    }
    part.build()
}

fn tall_upper() -> Result<Part, ValidationError> {
    let profile = vec![
        [UPPER_INNER, TALL_DECK],
        [UPPER_RADIUS, TALL_DECK],
        [UPPER_RADIUS, UPPER_DRUM],
        [UPPER_INNER, UPPER_DRUM],
    ];
    let openings = vec![
        Opening {
            angle: 90.0,
            half: 16.0,
            cutter: arch_cutter(
                1.1,
                2.2,
                UPPER_INNER - 0.2,
                UPPER_RADIUS + 0.3,
                90.0,
                TALL_DECK,
            ),
            portal: Some("door-up"),
        },
        Opening {
            angle: 270.0,
            half: 8.0,
            cutter: at(
                bore(0.3, 0.8, SMALL),
                rotated_about_y(
                    270.0,
                    Pose::at([0.0, TALL_DECK + 1.6, -(UPPER_INNER - 0.2)]),
                ),
            ),
            portal: None,
        },
    ];
    let mut part = Part::builder("scifi:dome-tall-upper");
    for element in round_wall(
        "drum",
        &profile,
        90.0,
        34.0,
        30.0,
        DQuat::IDENTITY,
        &openings,
        "adobe",
        "adobe",
        Axis::Y,
    ) {
        part = part.element(element);
    }
    part.element(round_liner(
        "liner",
        vec![
            [UPPER_INNER - LINER, TALL_DECK + LINER],
            [UPPER_INNER, TALL_DECK + LINER],
            [UPPER_INNER, UPPER_DRUM],
            [UPPER_INNER - LINER, UPPER_DRUM],
        ],
        &openings,
        DQuat::IDENTITY,
    ))
    .element(Element::new(
        "door-ring",
        at(
            door_ring(1.1, 2.2, 0.3),
            Pose::at([0.0, TALL_DECK, -(UPPER_RADIUS - 0.1)]),
        ),
        "trim",
    ))
    .element(
        Element::new(
            "port-glass",
            at(
                bore(0.3, 0.04, SMALL),
                rotated_about_y(
                    270.0,
                    Pose::at([0.0, TALL_DECK + 1.6, -(UPPER_INNER + 0.1)]),
                ),
            ),
            "glass",
        )
        .standalone(),
    )
    .element(
        Element::new(
            "upper-door-light",
            luminous([0.6, 0.055, 0.08], [-0.3, TALL_DECK + 2.37, -2.63]),
            "warning",
        )
        .far(),
    )
    .build()
}

/// The dome house's top: the upper dome and its finish, in the storey above
/// the loft as a ceiling is, with the cornice, vent and aerial.
fn tall_roof() -> Result<Part, ValidationError> {
    let mut cap = arc(
        [0.0, UPPER_DRUM],
        [UPPER_RADIUS, UPPER_RISE],
        0.0,
        FRAC_PI_2,
        12,
    );
    let mut inside = arc(
        [0.0, UPPER_DRUM],
        [UPPER_INNER, UPPER_RISE - 0.3],
        0.0,
        FRAC_PI_2,
        12,
    );
    inside.reverse();
    cap.extend(inside);
    let mut cap_liner = arc(
        [0.0, UPPER_DRUM],
        [UPPER_INNER, UPPER_RISE - 0.3],
        0.0,
        FRAC_PI_2,
        12,
    );
    let mut under = arc(
        [0.0, UPPER_DRUM],
        [UPPER_INNER - LINER, UPPER_RISE - 0.3 - LINER],
        0.0,
        FRAC_PI_2,
        12,
    );
    under.reverse();
    cap_liner.extend(under);
    let crown = UPPER_DRUM + UPPER_RISE;
    Part::builder("scifi:dome-tall-roof")
        .element(
            Element::new("dome", Geometry::revolve(cap, ROUND), "adobe")
                .uv(UvMode::Cylindrical { axis: Axis::Y })
                .collision(Collision::Hull),
        )
        .element(
            Element::new("dome-liner", Geometry::revolve(cap_liner, ROUND), "spine")
                .interior()
                .uv(UvMode::Cylindrical { axis: Axis::Y }),
        )
        .element(turned(
            "cornice",
            vec![
                [UPPER_RADIUS - 0.05, UPPER_DRUM - 0.12],
                [UPPER_RADIUS + 0.1, UPPER_DRUM - 0.12],
                [UPPER_RADIUS + 0.12, UPPER_DRUM - 0.03],
                [UPPER_RADIUS + 0.02, UPPER_DRUM + 0.1],
                [UPPER_RADIUS - 0.05, UPPER_DRUM + 0.08],
            ],
            "adobe",
        ))
        .element(
            Element::new(
                "vent",
                Geometry::cylinder(0.3, 0.7, SMALL).placed(Pose::at([0.0, crown - 0.1, 0.0])),
                "trim",
            )
            .uv(UvMode::Cylindrical { axis: Axis::Y }),
        )
        .element(Element::new(
            "vent-cap",
            Geometry::cylinder(0.45, 0.1, SMALL).placed(Pose::at([0.0, crown + 0.6, 0.0])),
            "rust",
        ))
        .element(Element::new(
            "aerial",
            Geometry::cylinder(0.03, 1.6, 8).placed(Pose::at([0.9, UPPER_DRUM + 0.8, 0.9])),
            "trim",
        ))
        .element(beacon("aerial-light", 0.06, [0.9, UPPER_DRUM + 2.45, 0.9]))
        .element(Element::new(
            "vent-louvres",
            Geometry::revolve(
                [[0.28, 0.0], [0.37, 0.0], [0.37, 0.06], [0.28, 0.06]],
                SMALL,
            )
            .placed(Pose::at([0.0, crown + 0.05, 0.0]))
            .arrayed(3, Pose::at([0.0, 0.16, 0.0])),
            "trim",
        ))
        .build()
}

// ------------------------------------------------------------------ hab pod

/// Radius of the pod's shell, its wall, half the length of its barrel, and
/// how far the deck lies below the axis.
const POD_RADIUS: f64 = 2.0;
const POD_WALL: f64 = 0.12;
const POD_INNER: f64 = POD_RADIUS - POD_WALL;
/// Half the length of the pod's straight barrel; a bulkhead closes each end.
pub(crate) const POD_BARREL: f64 = 2.6;
/// How far the pod's axis stands above the ground it is placed on.
pub(crate) const POD_AXIS: f64 = 2.5;
/// The deck's top, below the axis.
const POD_DECK: f64 = -1.0;
/// The radius of the docking port through each end cap.
const DOCK: f64 = 1.5;
/// The pod's door height above its deck.
const LOCK_TOP: f64 = 1.1;

/// A collar built from short, face-joined sectors. Its full union is a
/// closed sleeve; at a distance each narrow sector drops by its own extent,
/// instead of the whole ring's broad bounds keeping fine trim alive.
fn collar(inner: f64, outer: f64, width: f64) -> Geometry {
    let point = |radius: f64, step: u32| {
        let angle = f64::from(step % 48) * std::f64::consts::TAU / 48.0;
        [radius * angle.cos(), -radius * angle.sin()]
    };
    at(
        Geometry::union_all((0..48).map(|step| {
            Geometry::extrude(
                [
                    point(inner, step),
                    point(outer, step),
                    point(outer, step + 1),
                    point(inner, step + 1),
                ],
                width,
            )
        })),
        Pose::default().rotated(lie()),
    )
}

/// The rotation that lays a revolve on Y along X: +Y to +X, +X to -Y. A
/// sector's angle then runs from straight down towards -Z.
fn lie() -> DQuat {
    DQuat::from_rotation_z(-FRAC_PI_2)
}

/// The pod's barrel section between two stations along its axis.
fn barrel(from: f64, to: f64) -> Vec<[f64; 2]> {
    vec![
        [POD_INNER, from],
        [POD_RADIUS, from],
        [POD_RADIUS, to],
        [POD_INNER, to],
    ]
}

/// The side hatch, through the barrel at -Z.
fn hatch_cutter() -> Geometry {
    Geometry::cuboid([1.0, LOCK_TOP - POD_DECK, 1.3]).placed(Pose::at([-0.5, POD_DECK, -2.3]))
}

/// A porthole cutter through the barrel at station `x` on side `z_sign`.
fn pod_port(x: f64, z_sign: f64) -> Geometry {
    let turn_round = if z_sign < 0.0 { 0.0 } else { PI };
    at(
        bore(0.28, 0.6, SMALL),
        Pose::at([x, 0.0, z_sign * (POD_INNER - 0.2)]).rotated(yaw(turn_round)),
    )
}

/// Where the pod's portholes are: station and side.
const POD_PORTS: [(f64, f64); 5] = [
    (-2.0, -1.0),
    (2.0, -1.0),
    (-1.6, 1.0),
    (0.0, 1.0),
    (1.6, 1.0),
];

/// A bulkhead closing the barrel at station `x`, facing `sign` along X, with a
/// door: two convex runs either side and the door strip between.
fn bulkhead(x: f64, sign: f64, portal: &'static str) -> Vec<Element> {
    let near = if sign > 0.0 { x - 0.1 } else { x };
    let disc = || at(along_x(POD_INNER, 0.1, ROUND), Pose::at([near, 0.0, 0.0]));
    let door = Geometry::cuboid([0.3, LOCK_TOP - POD_DECK, 1.0]).placed(Pose::at([
        near - 0.1,
        POD_DECK,
        -0.5,
    ]));
    let inf = f64::INFINITY;
    vec![
        Element::new(
            format!("{portal}-south"),
            keep(disc(), [-inf, -inf, -inf], [inf, inf, -0.6]),
            "spine",
        )
        .interior()
        .collision(Collision::Hull),
        Element::new(
            format!("{portal}-north"),
            keep(disc(), [-inf, -inf, 0.6], [inf, inf, inf]),
            "spine",
        )
        .interior()
        .collision(Collision::Hull),
        Element::new(
            format!("{portal}-door"),
            keep(disc(), [-inf, -inf, -0.6], [inf, inf, 0.6]).subtract(door.portal(portal)),
            "spine",
        )
        .cut_material("trim")
        .interior(),
    ]
}

/// One end cap of the pod: a shell dome opened by the docking port.
fn end_cap(sign: f64) -> Geometry {
    let l = POD_BARREL;
    let mut outer = arc([0.0, 0.0], [POD_RADIUS, POD_RADIUS], 0.0, FRAC_PI_2, 12);
    let mut inner = arc([0.0, 0.0], [POD_INNER, POD_INNER], 0.0, FRAC_PI_2, 12);
    inner.reverse();
    outer.extend(inner);
    let profile: Vec<[f64; 2]> = outer.into_iter().map(|[r, y]| [r, sign * y]).collect();
    let cap = Geometry::revolve(profile, ROUND);
    let port = Geometry::cylinder(DOCK, POD_RADIUS + 1.0, ROUND).placed(Pose::at([
        0.0,
        if sign > 0.0 { 0.3 } else { -(POD_RADIUS + 1.3) },
        0.0,
    ]));
    at(
        cap.subtract(port),
        Pose::at([sign * l, 0.0, 0.0]).rotated(lie()),
    )
}

/// The section of the finish over the barrel's inside.
fn pod_liner() -> Vec<[f64; 2]> {
    let l = POD_BARREL;
    vec![
        [POD_INNER - LINER, -l + 0.1],
        [POD_INNER, -l + 0.1],
        [POD_INNER, l - 0.1],
        [POD_INNER - LINER, l - 0.1],
    ]
}

/// The pod's roof, in the storey above its cabin as a ceiling is: the top of
/// the barrel and its finish, the ribs over it, a light strip, and the roof
/// plant, conduit and mast.
fn hab_pod_roof() -> Result<Part, ValidationError> {
    let l = POD_BARREL;
    let frame = lie();
    let mut part = Part::builder("scifi:hab-pod-roof");
    for element in ring(
        "barrel",
        &barrel(-l, l),
        135.0,
        255.0,
        30.0,
        frame,
        &[],
        "hull",
        "dark",
        Axis::X,
    ) {
        part = part.element(element);
    }
    part = part
        .element(
            Element::new(
                "liner",
                sector(&pod_liner(), 135.0, 255.0, frame, &[]),
                "spine",
            )
            .interior()
            .uv(UvMode::Cylindrical { axis: Axis::X }),
        )
        .element(
            Element::new(
                "ceiling-light",
                Geometry::cuboid([2.0 * l - 1.0, 0.04, 0.2]).placed(Pose::at([
                    -l + 0.5,
                    POD_INNER - 0.14,
                    -0.1,
                ])),
                "light",
            )
            .interior(),
        );
    // Ribs, clear of the hatch.
    for (index, x) in [-2.45, -1.3, 1.3, 2.45].into_iter().enumerate() {
        part = part.element(
            Element::new(
                format!("rib-{index}"),
                at(
                    collar(POD_RADIUS - 0.025, POD_RADIUS + 0.06, 0.14),
                    Pose::at([x - 0.07, 0.0, 0.0]),
                ),
                "trim",
            )
            .uv(UvMode::Cylindrical { axis: Axis::X }),
        );
    }
    part.element(
        Element::new(
            "air-handler",
            plate([1.6, 0.55, 1.1], [-0.8, POD_RADIUS - 0.12, -0.55]),
            "hull",
        )
        .uv(UvMode::Box),
    )
    .element(Element::new(
        "grille",
        plate([1.3, 0.3, 0.04], [-0.65, POD_RADIUS + 0.02, -0.59]),
        "dark",
    ))
    .element(Element::new(
        "conduit",
        Geometry::union_all([
            at(
                along_x(0.07, 3.4, SMALL),
                Pose::at([-1.7, POD_RADIUS - 0.35, 1.05]),
            ),
            at(
                along_x(0.05, 3.4, SMALL),
                Pose::at([-1.7, POD_RADIUS - 0.45, 1.25]),
            ),
        ]),
        "rust",
    ))
    .element(Element::new(
        "mast",
        Geometry::cylinder(0.035, 1.4, 8).placed(Pose::at([1.4, POD_RADIUS - 0.2, 0.0])),
        "trim",
    ))
    .element(beacon("mast-light", 0.07, [1.4, POD_RADIUS + 1.25, 0.0]))
    .element(Element::new(
        "grille-slats",
        plate([1.25, 0.035, 0.06], [-0.625, POD_RADIUS + 0.065, -0.615])
            .arrayed(4, Pose::at([0.0, 0.07, 0.0])),
        "trim",
    ))
    .element(Element::new(
        "plant-feet",
        Geometry::union_all(
            [-0.65, 0.51].map(|x| plate([0.14, 0.3, 1.05], [x, POD_RADIUS - 0.29, -0.525])),
        ),
        "trim",
    ))
    .build()
}

/// The pod: a ribbed capsule with a deck, bunks' worth of cabin, a side
/// hatch on a landing and stair, bulkhead doors at both ends behind docking
/// ports, portholes, and roof plant. The part origin is the capsule's centre.
fn hab_pod() -> Result<Part, ValidationError> {
    let l = POD_BARREL;
    let mut openings = vec![Opening {
        angle: 90.0,
        half: 45.0,
        cutter: hatch_cutter(),
        portal: Some("hatch"),
    }];
    for (x, z) in POD_PORTS {
        openings.push(Opening {
            angle: if z < 0.0 { 90.0 } else { 270.0 },
            half: 9.0,
            cutter: pod_port(x, z),
            portal: None,
        });
    }
    let frame = lie();
    let hatch_half = 0.65;
    let mut part = Part::builder("scifi:hab-pod");
    // The barrel: full-length wedges round the top and back, and three
    // stations along the front where the hatch is.
    for element in ring(
        "barrel",
        &barrel(-l, l),
        255.0,
        405.0,
        30.0,
        frame,
        &openings[1..],
        "hull",
        "dark",
        Axis::X,
    ) {
        part = part.element(element);
    }
    for (id, from, to) in [
        ("barrel-aft", -l, -hatch_half),
        ("barrel-fore", hatch_half, l),
    ] {
        for element in ring(
            id,
            &barrel(from, to),
            45.0,
            135.0,
            30.0,
            frame,
            &openings[1..],
            "hull",
            "dark",
            Axis::X,
        ) {
            part = part.element(element);
        }
    }
    part = part.element(doorway(
        "barrel-hatch",
        &barrel(-hatch_half, hatch_half),
        90.0,
        90.0,
        frame,
        &openings[..1],
        "hull",
        "dark",
        Axis::X,
    ));
    for (sign, portal) in [(1.0, "lock-east"), (-1.0, "lock-west")] {
        for element in bulkhead(sign * l, sign, portal) {
            part = part.element(element);
        }
        part = part.element(
            Element::new(
                if sign > 0.0 { "cap-east" } else { "cap-west" },
                end_cap(sign),
                "hull",
            )
            .cut_material("trim")
            .uv(UvMode::Cylindrical { axis: Axis::X }),
        );
    }
    // Finishes: the deck, and the liner over the lower barrel's inside; the
    // roof carries the rest.
    let liner = sector(&pod_liner(), 255.0, 495.0, frame, &openings);
    part = part
        .element(
            Element::new("liner", liner, "spine")
                .interior()
                .uv(UvMode::Cylindrical { axis: Axis::X }),
        )
        .element(
            Element::new(
                "deck",
                Geometry::cuboid([2.0 * l - 0.2, 0.12, 2.96]).placed(Pose::at([
                    -l + 0.1,
                    POD_DECK - 0.12,
                    -1.48,
                ])),
                "floor",
            )
            .interior()
            .uv(UvMode::Box)
            .collision(Collision::Bounds),
        );
    // Portholes.
    for (index, (x, z)) in POD_PORTS.into_iter().enumerate() {
        let turn_round = if z < 0.0 { 0.0 } else { PI };
        part = part
            .element(
                Element::new(
                    format!("port-glass-{index}"),
                    at(
                        bore(0.28, 0.04, SMALL),
                        Pose::at([x, 0.0, z * (POD_INNER + 0.05)]).rotated(yaw(turn_round)),
                    ),
                    "glass",
                )
                .standalone(),
            )
            .element(Element::new(
                format!("port-rim-{index}"),
                at(
                    bore(0.37, 0.12, SMALL)
                        .subtract(at(bore(0.28, 0.3, SMALL), Pose::at([0.0, 0.0, 0.07]))),
                    Pose::at([x, 0.0, z * (POD_RADIUS + 0.06)]).rotated(yaw(turn_round)),
                ),
                "trim",
            ));
    }
    // The hatch collar: jambs and a hood standing off the barrel round the
    // opening, a landing at deck height, and a stair to the ground.
    let deck = POD_DECK;
    part = part
        .element(
            Element::new(
                "hatch-collar",
                Geometry::union_all([
                    plate(
                        [0.2, LOCK_TOP - deck + 0.3, 0.7],
                        [-0.72, deck + 0.01, -2.2],
                    ),
                    plate([0.2, LOCK_TOP - deck + 0.3, 0.7], [0.52, deck + 0.01, -2.2]),
                    plate([1.44, 0.26, 0.9], [-0.72, LOCK_TOP + 0.02, -2.4]),
                ]),
                "trim",
            )
            .uv(UvMode::Box),
        )
        .element(Element::new(
            "hatch-light",
            luminous([0.8, 0.05, 0.06], [-0.4, LOCK_TOP + 0.1, -2.47]),
            "light",
        ))
        .element(Element::new(
            "hatch-marking",
            plate([0.5, 0.18, 0.02], [0.62, 0.35, -2.23]),
            "marking",
        ))
        .element(
            Element::new(
                "landing",
                plate([1.8, 0.14, 1.1], [-0.9, deck - 0.14, -2.75]),
                "deck",
            )
            .uv(UvMode::Box)
            .collision(Collision::Bounds),
        );
    let steps = 7;
    let rise = (POD_AXIS + deck) / f64::from(steps);
    let run = 0.3;
    let mut treads = Vec::new();
    for step in 0..steps {
        let top = -POD_AXIS + rise * f64::from(step + 1) - 0.14;
        treads.push(plate(
            [1.2, 0.06, run],
            [-0.6, top, -2.75 - run * f64::from(steps - step)],
        ));
    }
    let foot = -2.75 - run * f64::from(steps);
    let slope = ((POD_AXIS + deck) / (run * f64::from(steps))).atan();
    let stringer = |x: f64| {
        Geometry::chamfered_cuboid(
            [
                0.08,
                0.2,
                (run * f64::from(steps)).hypot(POD_AXIS + deck) + 0.1,
            ],
            0.02,
        )
        .placed(Pose::at([x, -POD_AXIS, foot]).rotated(DQuat::from_rotation_x(-slope)))
    };
    part = part
        .element(
            Element::new(
                "stair",
                Geometry::union_all(treads.into_iter().chain([stringer(-0.68), stringer(0.6)])),
                "deck",
            )
            .uv(UvMode::Box)
            .collision(Collision::Hull),
        )
        .element(Element::new(
            "handrails",
            Geometry::union_all([
                at(
                    Geometry::cylinder(0.03, (run * f64::from(steps)).hypot(POD_AXIS + deck), 8),
                    Pose::at([-0.66, -POD_AXIS + 0.95, foot])
                        .rotated(DQuat::from_rotation_x(FRAC_PI_2 - slope)),
                ),
                at(
                    Geometry::cylinder(0.03, (run * f64::from(steps)).hypot(POD_AXIS + deck), 8),
                    Pose::at([0.64, -POD_AXIS + 0.95, foot])
                        .rotated(DQuat::from_rotation_x(FRAC_PI_2 - slope)),
                ),
                Geometry::cylinder(0.03, 0.95, 8).placed(Pose::at([-0.66, -POD_AXIS, foot + 0.02])),
                Geometry::cylinder(0.03, 0.95, 8).placed(Pose::at([0.64, -POD_AXIS, foot + 0.02])),
            ]),
            "trim",
        ));
    // Stilts and feet, and the roof plant.
    let corners = [(-2.3, -1.0), (-2.3, 1.0), (2.3, -1.0), (2.3, 1.0)];
    for (index, (x, z)) in corners.into_iter().enumerate() {
        part = part.element(
            Element::new(
                format!("stilt-{index}"),
                Geometry::cylinder(0.13, POD_AXIS - 1.1, SMALL).placed(Pose::at([x, -POD_AXIS, z])),
                "rust",
            )
            .collision(Collision::Bounds),
        );
    }
    part = part
        .element(Element::new(
            "feet",
            Geometry::union_all(corners.map(|(x, z)| {
                Geometry::cylinder(0.3, 0.08, SMALL).placed(Pose::at([x, -POD_AXIS + 0.005, z]))
            })),
            "concrete",
        ))
        .element(Element::new(
            "dock-collars",
            Geometry::union_all([1.0, -1.0].map(|sign: f64| {
                let station = sign * (l + (POD_RADIUS * POD_RADIUS - DOCK * DOCK).sqrt());
                at(
                    collar(DOCK - 0.02, DOCK + 0.12, 0.12),
                    Pose::at([station - 0.06, 0.0, 0.0]),
                )
            })),
            "trim",
        ));
    part.socket(Socket::new(
        "east",
        Pose::at([l, 0.0, 0.0]).rotated(yaw(FRAC_PI_2)),
    ))
    .socket(Socket::new(
        "west",
        Pose::at([-l, 0.0, 0.0]).rotated(yaw(-FRAC_PI_2)),
    ))
    .element(Element::new(
        "landing-braces",
        Geometry::union_all(
            [-0.75, 0.75].map(|x| super::brace([x, -1.8, -1.0], [x, deck - 0.16, -2.6], 0.075)),
        ),
        "trim",
    ))
    .element(Element::new(
        "stilt-braces",
        Geometry::union_all([-2.3, 2.3].into_iter().flat_map(|x| {
            [-1.0, 1.0].map(move |z| super::brace([x, -2.3, z], [x, -1.2, -z], 0.055))
        })),
        "trim",
    ))
    .element(Element::new(
        "service-cover",
        super::service_cover([1.45, 0.35, -1.98], 0.65, 0.48),
        "trim",
    ))
    .element(
        Element::new(
            "hatch-emblem",
            luminous([0.06, 0.3, 0.035], [0.76, 0.3, -2.26]).arrayed(3, Pose::at([0.13, 0.0, 0.0])),
            "warning",
        )
        .far(),
    )
    .build()
}

// ------------------------------------------------------------------ tube

/// Length of a walkway tube, from one pod's bulkhead to the next one's.
pub(crate) const TUBE_LENGTH: f64 = 8.0;
const TUBE_RADIUS: f64 = 1.45;
const TUBE_INNER: f64 = 1.35;

/// The section of the finish over the tube's inside.
fn tube_liner() -> Vec<[f64; 2]> {
    vec![
        [TUBE_INNER - LINER, 0.01],
        [TUBE_INNER, 0.01],
        [TUBE_INNER, TUBE_LENGTH - 0.01],
        [TUBE_INNER - LINER, TUBE_LENGTH - 0.01],
    ]
}

/// The tube's roof, in the storey above the walkway: the top of the shell,
/// its finish and the light strip along it.
fn tube_roof() -> Result<Part, ValidationError> {
    let section = [
        [TUBE_INNER, 0.0],
        [TUBE_RADIUS, 0.0],
        [TUBE_RADIUS, TUBE_LENGTH],
        [TUBE_INNER, TUBE_LENGTH],
    ];
    let mut part = Part::builder("scifi:tube-roof");
    for element in ring(
        "shell",
        &section,
        135.0,
        255.0,
        30.0,
        lie(),
        &[],
        "hull",
        "trim",
        Axis::X,
    ) {
        part = part.element(element);
    }
    part.element(
        Element::new(
            "liner",
            sector(&tube_liner(), 135.0, 255.0, lie(), &[]),
            "spine",
        )
        .interior()
        .uv(UvMode::Cylindrical { axis: Axis::X }),
    )
    .element(
        Element::new(
            "light",
            Geometry::cuboid([TUBE_LENGTH - 1.0, 0.04, 0.12]).placed(Pose::at([
                0.5,
                TUBE_INNER - 0.1,
                -0.06,
            ])),
            "light",
        )
        .interior(),
    )
    .build()
}

/// The walkway tube between two pods: a ribbed, windowed shell with a deck,
/// from the bulkhead face of one pod (its origin, `a`) to the next (`b`).
/// Its axis is the part's X axis at the pods' axis height.
fn tube() -> Result<Part, ValidationError> {
    let length = TUBE_LENGTH;
    let band =
        |z: f64| Geometry::cuboid([length - 3.4, 0.55, 0.8]).placed(Pose::at([1.7, 0.05, z]));
    let openings = [
        Opening {
            angle: 90.0,
            half: 20.0,
            cutter: band(-TUBE_RADIUS - 0.3),
            portal: None,
        },
        Opening {
            angle: 270.0,
            half: 20.0,
            cutter: band(TUBE_RADIUS - 0.5),
            portal: None,
        },
    ];
    let section = [
        [TUBE_INNER, 0.0],
        [TUBE_RADIUS, 0.0],
        [TUBE_RADIUS, length],
        [TUBE_INNER, length],
    ];
    let mut part = Part::builder("scifi:tube");
    for element in ring(
        "shell",
        &section,
        255.0,
        495.0,
        30.0,
        lie(),
        &openings,
        "hull",
        "trim",
        Axis::X,
    ) {
        part = part.element(element);
    }
    let liner = sector(&tube_liner(), 255.0, 495.0, lie(), &openings);
    let dock = (POD_RADIUS * POD_RADIUS - DOCK * DOCK).sqrt();
    let pylon = |x: f64| {
        Geometry::chamfered_cuboid([0.5, POD_AXIS - TUBE_RADIUS + 0.1, 0.5], 0.06)
            .placed(Pose::at([x - 0.25, -POD_AXIS + 0.005, -0.25]))
    };
    part.element(
        Element::new("liner", liner, "spine")
            .interior()
            .uv(UvMode::Cylindrical { axis: Axis::X }),
    )
    .element(
        Element::new(
            "deck",
            Geometry::cuboid([length, 0.08, 1.56]).placed(Pose::at([0.0, POD_DECK - 0.08, -0.78])),
            "floor",
        )
        .interior()
        .uv(UvMode::Box)
        .collision(Collision::Bounds),
    )
    .element(
        Element::new(
            "windows",
            Geometry::union_all([
                plate([length - 3.5, 0.5, 0.04], [1.75, 0.07, -TUBE_INNER + 0.12]),
                plate([length - 3.5, 0.5, 0.04], [1.75, 0.07, TUBE_INNER - 0.16]),
            ]),
            "glass",
        )
        .standalone(),
    )
    .element(
        Element::new(
            "ribs",
            at(
                collar(TUBE_RADIUS - 0.025, TUBE_RADIUS + 0.07, 0.16),
                Pose::at([dock + 0.2, 0.0, 0.0]),
            )
            .arrayed(3, Pose::at([(length - 2.0 * dock - 0.56) / 2.0, 0.0, 0.0])),
            "trim",
        )
        .uv(UvMode::Cylindrical { axis: Axis::X }),
    )
    .element(Element::new(
        "flanges",
        Geometry::union_all([
            at(
                collar(TUBE_INNER + 0.03, DOCK + 0.1, 0.1),
                Pose::at([dock - 0.1, 0.0, 0.0]),
            ),
            at(
                collar(TUBE_INNER + 0.03, DOCK + 0.1, 0.1),
                Pose::at([length - dock, 0.0, 0.0]),
            ),
        ]),
        "trim",
    ))
    .element(
        Element::new(
            "pylons",
            Geometry::union_all([pylon(length / 2.0 - 1.4), pylon(length / 2.0 + 1.4)]),
            "concrete",
        )
        .uv(UvMode::Box)
        .collision(Collision::Bounds),
    )
    .socket(Socket::new("a", Pose::default().rotated(yaw(-FRAC_PI_2))))
    .socket(Socket::new(
        "b",
        Pose::at([length, 0.0, 0.0]).rotated(yaw(FRAC_PI_2)),
    ))
    .element(
        Element::new(
            "window-heads",
            Geometry::union_all([-1.39, 1.33].map(|z| luminous([4.6, 0.08, 0.08], [1.7, 0.62, z]))),
            "neon",
        )
        .far(),
    )
    .element(Element::new(
        "window-mullions",
        Geometry::union_all([-1.46, 1.38].map(|z| {
            plate([0.07, 0.65, 0.08], [2.25, 0.01, z]).arrayed(3, Pose::at([1.7, 0.0, 0.0]))
        })),
        "trim",
    ))
    .build()
}

// ------------------------------------------------------------------ module

const MODULE: [f64; 3] = [8.0, 3.6, 6.0];
/// The module's body stands on its plinth at this height.
const MODULE_BASE: f64 = 0.3;
const MODULE_WALL: f64 = 0.4;
const MODULE_FLOOR: f64 = MODULE_BASE + 0.3;
const MODULE_CEILING: f64 = MODULE_BASE + MODULE[1] - 0.4;
/// Where the airlock's partition stands, and its thickness.
const PARTITION_X: f64 = 2.6;
const PARTITION: f64 = 0.12;
/// The front door, at the airlock.
const MODULE_DOOR: [f64; 2] = [0.7, 1.9];
/// The office's windows: front, and the right-hand end.
const FRONT_WINDOW: [f64; 2] = [4.0, 7.0];
const SIDE_WINDOW: [f64; 2] = [1.5, 4.5];
const WINDOW_SILL: f64 = MODULE_FLOOR + 0.95;
const WINDOW_HEAD: f64 = MODULE_FLOOR + 2.0;

/// The module's shell before it is split into runs.
fn module_shell() -> Geometry {
    let [w, h, d] = MODULE;
    let t = MODULE_WALL;
    Geometry::chamfered_cuboid([w, h, d], 0.55)
        .placed(Pose::at([0.0, MODULE_BASE, 0.0]))
        .subtract(
            Geometry::cuboid([w - 2.0 * t, MODULE_CEILING - MODULE_FLOOR, d - 2.0 * t])
                .placed(Pose::at([t, MODULE_FLOOR, t])),
        )
}

/// The module's roof, in the storey above its rooms as a ceiling is: the
/// roof run of the shell, the ceiling finish and its light panels, and the
/// roof plant and dish.
fn module_roof() -> Result<Part, ValidationError> {
    let [w, h, d] = MODULE;
    let t = MODULE_WALL;
    let inf = f64::INFINITY;
    let ceiling = MODULE_CEILING;
    let top = MODULE_BASE + h;
    let mut dish = vec![[0.0, 0.0]];
    dish.extend(
        arc([0.0, 0.9], [0.9, 0.9], -FRAC_PI_2, -0.35, 6)
            .into_iter()
            .skip(1),
    );
    let lip = dish.last().copied().unwrap_or([0.8, 0.5]);
    dish.push([lip[0] - 0.05, lip[1] + 0.03]);
    dish.extend(
        arc([0.0, 0.93], [0.85, 0.85], -0.35, -FRAC_PI_2 + 0.12, 6)
            .into_iter()
            .skip(1),
    );
    dish.push([0.0, 0.12]);
    Part::builder("scifi:module-roof")
        .element(
            Element::new(
                "shell-roof",
                keep(module_shell(), [-inf, ceiling, -inf], [inf, inf, inf]),
                "hull",
            )
            .cut_material("dark")
            .uv(UvMode::Box)
            .collision(Collision::Hull),
        )
        .element(
            Element::new(
                "ceiling",
                Geometry::cuboid([w - 2.0 * t, LINER, d - 2.0 * t]).placed(Pose::at([
                    t,
                    ceiling - LINER,
                    t,
                ])),
                "spine",
            )
            .interior()
            .uv(UvMode::Box),
        )
        .element(
            Element::new(
                "roof-plant",
                Geometry::union_all([
                    plate([2.4, 0.8, 1.8], [0.9, top - 0.02, 2.6]),
                    Geometry::cylinder(0.35, 0.6, SMALL).placed(Pose::at([5.2, top - 0.02, 1.6])),
                    Geometry::cylinder(0.35, 0.6, SMALL).placed(Pose::at([6.2, top - 0.02, 1.6])),
                ]),
                "trim",
            )
            .uv(UvMode::Box),
        )
        .element(Element::new(
            "dish",
            at(
                Geometry::revolve(dish, 32),
                Pose::at([5.8, top + 1.0, 4.3]).rotated(DQuat::from_rotation_x(-0.6)),
            ),
            "hull",
        ))
        .element(Element::new(
            "dish-mount",
            Geometry::cylinder(0.08, 1.1, 8).placed(Pose::at([5.8, top - 0.02, 4.3])),
            "trim",
        ))
        .element(Element::new(
            "vent-pipes",
            Geometry::union_all([
                Geometry::cylinder(0.1, 0.5, SMALL).placed(Pose::at([7.0, top - 0.02, 5.2])),
                Geometry::cylinder(0.1, 0.5, SMALL).placed(Pose::at([7.3, top - 0.02, 5.2])),
            ]),
            "rust",
        ))
        .element(
            Element::new(
                "ceiling-panels",
                Geometry::cuboid([0.9, 0.03, 0.9])
                    .placed(Pose::at([1.05, ceiling - LINER - 0.03, 2.55]))
                    .arrayed(3, Pose::at([2.1, 0.0, 0.0])),
                "light",
            )
            .interior(),
        )
        .element(Element::new(
            "roof-edge",
            Geometry::union_all([
                plate([6.9, 0.14, 0.18], [0.55, top - 0.14, 0.31]),
                plate([6.9, 0.14, 0.18], [0.55, top - 0.14, 5.51]),
            ]),
            "trim",
        ))
        .element(Element::new(
            "plant-grille",
            plate([2.1, 0.055, 0.08], [1.05, top + 0.12, 2.55])
                .arrayed(5, Pose::at([0.0, 0.12, 0.0])),
            "dark",
        ))
        .element(Element::new(
            "exhaust-cowls",
            Geometry::union_all([5.2, 6.2].map(|x| {
                Geometry::cylinder(0.46, 0.12, SMALL).placed(Pose::at([x, top + 0.52, 1.6]))
            })),
            "hull",
        ))
        .build()
}

/// The module: a heavily chamfered shell split into floor, roof and wall
/// runs, an airlock entry and an office behind a partition, the door and
/// windows, finishes, and the roof plant and dish.
fn module() -> Result<Part, ValidationError> {
    let [w, h, d] = MODULE;
    let t = MODULE_WALL;
    let shell = module_shell;
    let door = Geometry::cuboid([MODULE_DOOR[1] - MODULE_DOOR[0], DOOR_HEIGHT + 0.05, 1.0])
        .placed(Pose::at([MODULE_DOOR[0], MODULE_FLOOR - 0.05, -0.3]));
    let front_window = Geometry::cuboid([
        FRONT_WINDOW[1] - FRONT_WINDOW[0],
        WINDOW_HEAD - WINDOW_SILL,
        1.0,
    ])
    .placed(Pose::at([FRONT_WINDOW[0], WINDOW_SILL, -0.3]));
    let side_window = Geometry::cuboid([
        1.0,
        WINDOW_HEAD - WINDOW_SILL,
        SIDE_WINDOW[1] - SIDE_WINDOW[0],
    ])
    .placed(Pose::at([w - t - 0.3, WINDOW_SILL, SIDE_WINDOW[0]]));
    let inf = f64::INFINITY;
    let (floor, ceiling) = (MODULE_FLOOR, MODULE_CEILING);
    let runs = [
        ("floor", [-inf, -inf, -inf], [inf, floor, inf], None),
        (
            "front-a",
            [-inf, floor, -inf],
            [MODULE_DOOR[0] - 0.1, ceiling, t],
            None,
        ),
        (
            "front-door",
            [MODULE_DOOR[0] - 0.1, floor, -inf],
            [MODULE_DOOR[1] + 0.1, ceiling, t],
            Some(door.clone().portal("door")),
        ),
        (
            "front-b",
            [MODULE_DOOR[1] + 0.1, floor, -inf],
            [inf, ceiling, t],
            Some(front_window.clone().portal("window")),
        ),
        ("back", [-inf, floor, d - t], [inf, ceiling, inf], None),
        ("left", [-inf, floor, t], [t, ceiling, d - t], None),
        (
            "right",
            [w - t, floor, t],
            [inf, ceiling, d - t],
            Some(side_window.clone().portal("window-side")),
        ),
    ];
    let mut part = Part::builder("scifi:module");
    for (id, min, max, cutter) in runs {
        let mut run = keep(shell(), min, max);
        if let Some(cutter) = cutter {
            run = run.subtract(cutter);
        }
        let mut element = Element::new(format!("shell-{id}"), run, "hull")
            .cut_material("dark")
            .uv(UvMode::Box);
        if id != "front-door" {
            element = element.collision(Collision::Hull);
        }
        part = part.element(element);
    }
    // Finishes, cut where the openings are.
    // The wall finishes stand on the floor finish and stop under the ceiling
    // finish, so no two finishes share a plane.
    let inner = [t, floor + LINER, t];
    let clear = [w - 2.0 * t, ceiling - floor - 2.0 * LINER, d - 2.0 * t];
    let liner = |id: &str, size: [f64; 3], at_: [f64; 3], cutters: &[&Geometry]| {
        let mut geometry = Geometry::cuboid(size).placed(Pose::at(at_));
        for cutter in cutters {
            geometry = geometry.subtract((*cutter).clone());
        }
        Element::new(id, geometry, "spine")
            .interior()
            .uv(UvMode::Box)
    };
    part = part
        .element(liner(
            "liner-front",
            [clear[0], clear[1], LINER],
            [inner[0], inner[1], t],
            &[&door, &front_window],
        ))
        .element(liner(
            "liner-back",
            [clear[0], clear[1], LINER],
            [inner[0], inner[1], d - t - LINER],
            &[],
        ))
        .element(liner(
            "liner-left",
            [LINER, clear[1], clear[2]],
            [t, inner[1], t],
            &[],
        ))
        .element(liner(
            "liner-right",
            [LINER, clear[1], clear[2]],
            [w - t - LINER, inner[1], t],
            &[&side_window],
        ))
        .element(
            Element::new(
                "floor-finish",
                Geometry::cuboid([clear[0], LINER, clear[2]]).placed(Pose::at([t, floor, t])),
                "floor",
            )
            .interior()
            .uv(UvMode::Box),
        );
    // The airlock partition: two runs and the strip with its inner door.
    let partition_door = Geometry::cuboid([PARTITION + 0.2, DOOR_HEIGHT + 0.05, DOOR_WIDTH])
        .placed(Pose::at([PARTITION_X - 0.1, floor - 0.05, 3.9]));
    let wall = || {
        Geometry::cuboid([
            PARTITION,
            ceiling - floor - 2.0 * LINER,
            d - 2.0 * t - 2.0 * LINER,
        ])
        .placed(Pose::at([PARTITION_X, floor + LINER, t + LINER]))
    };
    part = part
        .element(
            Element::new(
                "partition-a",
                keep(wall(), [-inf, -inf, -inf], [inf, inf, 3.8]),
                "spine",
            )
            .interior()
            .uv(UvMode::Box)
            .collision(Collision::Bounds),
        )
        .element(
            Element::new(
                "partition-door",
                keep(
                    wall(),
                    [-inf, -inf, 3.8],
                    [inf, inf, 3.9 + DOOR_WIDTH + 0.1],
                )
                .subtract(partition_door.portal("inner-door")),
                "spine",
            )
            .cut_material("trim")
            .interior()
            .uv(UvMode::Box),
        )
        .element(
            Element::new(
                "partition-b",
                keep(
                    wall(),
                    [-inf, -inf, 3.9 + DOOR_WIDTH + 0.1],
                    [inf, inf, inf],
                ),
                "spine",
            )
            .interior()
            .uv(UvMode::Box)
            .collision(Collision::Bounds),
        );
    // Glazing, and the exterior: plinth, steps, door frame and leaves slid
    // open, light strips, roof plant and dish.
    let top = MODULE_BASE + h;
    part = part
        .element(
            Element::new(
                "glass",
                Geometry::union_all([
                    plate(
                        [
                            FRONT_WINDOW[1] - FRONT_WINDOW[0] + 0.1,
                            WINDOW_HEAD - WINDOW_SILL + 0.1,
                            0.05,
                        ],
                        [FRONT_WINDOW[0] - 0.05, WINDOW_SILL - 0.05, 0.18],
                    ),
                    plate(
                        [
                            0.05,
                            WINDOW_HEAD - WINDOW_SILL + 0.1,
                            SIDE_WINDOW[1] - SIDE_WINDOW[0] + 0.1,
                        ],
                        [w - 0.23, WINDOW_SILL - 0.05, SIDE_WINDOW[0] - 0.05],
                    ),
                ]),
                "glass",
            )
            .standalone(),
        )
        .element(
            Element::new(
                "plinth",
                Geometry::chamfered_cuboid([w + 0.4, 0.34, d + 0.4], 0.08)
                    .placed(Pose::at([-0.2, 0.0, -0.2])),
                "concrete",
            )
            .uv(UvMode::Box)
            .collision(Collision::Bounds),
        )
        .element(
            Element::new(
                "steps",
                Geometry::union_all([
                    plate([1.8, 0.2, 0.9], [MODULE_DOOR[0] - 0.3, 0.005, -1.1]),
                    plate([1.8, 0.2, 0.55], [MODULE_DOOR[0] - 0.3, 0.2, -0.75]),
                ]),
                "concrete",
            )
            .uv(UvMode::Box)
            .collision(Collision::Hull),
        )
        .element(Element::new(
            "door-frame",
            Geometry::union_all([
                plate(
                    [0.16, DOOR_HEIGHT + 0.2, 0.3],
                    [MODULE_DOOR[0] - 0.22, MODULE_FLOOR - 0.05, -0.12],
                ),
                plate(
                    [0.16, DOOR_HEIGHT + 0.2, 0.3],
                    [MODULE_DOOR[1] + 0.06, MODULE_FLOOR - 0.05, -0.12],
                ),
                plate(
                    [MODULE_DOOR[1] - MODULE_DOOR[0] + 0.44, 0.2, 0.3],
                    [
                        MODULE_DOOR[0] - 0.22,
                        MODULE_FLOOR + DOOR_HEIGHT + 0.05,
                        -0.12,
                    ],
                ),
            ]),
            "trim",
        ))
        .element(
            Element::new(
                "door-leaves",
                plate(
                    [0.62, DOOR_HEIGHT - 0.02, 0.08],
                    [MODULE_DOOR[1] + 0.24, MODULE_FLOOR + 0.01, -0.2],
                ),
                "trim",
            )
            .standalone(),
        )
        .element(Element::new(
            "light-strips",
            Geometry::union_all([
                luminous(
                    [1.6, 0.06, 0.06],
                    [
                        MODULE_DOOR[0] - 0.2,
                        MODULE_FLOOR + DOOR_HEIGHT + 0.3,
                        -0.16,
                    ],
                ),
                luminous([w - 1.4, 0.05, 0.05], [0.7, 0.62, -0.02]),
            ]),
            "light",
        ))
        .element(Element::new(
            "sign",
            plate([1.6, 0.4, 0.04], [2.4, top - 1.0, -0.06]),
            "marking",
        ));
    part.element(
        Element::new(
            "window-frames",
            Geometry::union_all([
                plate([3.3, 1.35, 0.24], [3.85, 1.4, -0.1]).subtract(
                    Geometry::cuboid([2.98, 1.03, 0.6]).placed(Pose::at([4.01, 1.56, -0.2])),
                ),
                plate([0.24, 1.35, 3.3], [7.86, 1.4, 1.35]).subtract(
                    Geometry::cuboid([0.6, 1.03, 2.98]).placed(Pose::at([7.7, 1.56, 1.51])),
                ),
            ]),
            "trim",
        )
        .uv(UvMode::Box),
    )
    .element(Element::new(
        "window-mullions",
        Geometry::union_all([
            plate([0.09, 1.07, 0.22], [5.45, 1.54, -0.04]),
            plate([0.22, 1.07, 0.09], [7.83, 1.54, 2.95]),
        ]),
        "trim",
    ))
    .element(Element::new(
        "window-hoods",
        Geometry::union_all([
            plate([3.48, 0.12, 0.46], [3.76, 2.76, -0.31]),
            plate([0.46, 0.12, 3.48], [7.85, 2.76, 1.26]),
        ]),
        "trim",
    ))
    .element(
        Element::new(
            "window-head-lights",
            Geometry::union_all([
                luminous([2.84, 0.045, 0.06], [4.08, 2.64, -0.13]),
                luminous([0.06, 0.045, 2.84], [8.07, 2.64, 1.58]),
            ]),
            "neon",
        )
        .far(),
    )
    .element(Element::new(
        "panel-joints",
        plate([0.085, 2.45, 0.08], [0.7, 0.85, 5.97]).arrayed(4, Pose::at([2.15, 0.0, 0.0])),
        "trim",
    ))
    .element(Element::new(
        "service-cover",
        at(
            super::service_cover([0.0, 0.0, 0.0], 1.3, 1.0),
            Pose::at([-0.04, 1.25, 3.8]).rotated(yaw(FRAC_PI_2)),
        ),
        "trim",
    ))
    .element(
        Element::new(
            "sign-glyphs",
            luminous([0.12, 0.23, 0.035], [2.65, top - 0.92, -0.085])
                .arrayed(4, Pose::at([0.3, 0.0, 0.0])),
            "neon",
        )
        .far(),
    )
    .build()
}

// ------------------------------------------------------------------ tower

const PLINTH_INNER: f64 = 2.25;
const LOBBY_CEILING: f64 = 3.3;
const SHAFT_TOP: f64 = 9.0;
const DECK_FLOOR: f64 = 10.5;
const DECK_CEILING: f64 = 12.8;
const GLAZING: f64 = 3.75;
/// The lift core, behind the lobby and the deck.
const CORE: (f64, f64) = (1.3, 0.65);

fn core(id: &str, from: f64, to: f64) -> Element {
    Element::new(
        id,
        Geometry::cylinder(CORE.1, to - from, ROUND).placed(Pose::at([0.0, from, CORE.0])),
        "hull",
    )
    .interior()
    .uv(UvMode::Cylindrical { axis: Axis::Y })
    .collision(Collision::Hull)
}

fn lift_door(id: &str, floor: f64) -> Element {
    Element::new(
        id,
        at(
            Geometry::cuboid([0.9, 2.1, 0.04]).placed(Pose::at([-0.45, 0.0, 0.0])),
            Pose::at([0.0, floor + 0.01, CORE.0 - CORE.1 - 0.02]),
        ),
        "light",
    )
    .interior()
}

fn tower_base() -> Result<Part, ValidationError> {
    let profile = vec![
        [PLINTH_INNER, 0.0],
        [4.4, 0.0],
        [4.2, 0.45],
        [3.4, 1.2],
        [2.8, 2.4],
        [2.55, 3.5],
        [PLINTH_INNER, 3.5],
    ];
    let door = Geometry::cuboid([1.4, 2.5, 2.8]).placed(Pose::at([-0.7, 0.0, -4.7]));
    let openings = [Opening {
        angle: 90.0,
        half: 21.0,
        cutter: door.clone(),
        portal: Some("door"),
    }];
    let mut part = Part::builder("scifi:tower-base");
    for element in round_wall(
        "plinth",
        &profile,
        90.0,
        42.0,
        40.0,
        DQuat::IDENTITY,
        &openings,
        "adobe",
        "concrete",
        Axis::Y,
    ) {
        part = part.element(element);
    }
    part.element(round_liner(
        "liner",
        vec![
            [PLINTH_INNER - LINER, 0.05],
            [PLINTH_INNER, 0.05],
            [PLINTH_INNER, LOBBY_CEILING],
            [PLINTH_INNER - LINER, LOBBY_CEILING],
        ],
        &openings,
        DQuat::IDENTITY,
    ))
    .element(round_floor("floor", PLINTH_INNER, 0.0, 0.05))
    .element(core("core", 0.05, LOBBY_CEILING - LINER))
    .element(lift_door("lift-door", 0.05))
    // The porch: a squared portal frame standing proud of the battered base.
    .element(
        Element::new(
            "porch",
            Geometry::union_all([
                plate([0.5, 2.95, 0.5], [-1.2, 0.01, -4.75]),
                plate([0.5, 2.95, 0.5], [0.7, 0.01, -4.75]),
                plate([2.4, 0.5, 0.6], [-1.2, 2.96, -4.8]),
            ]),
            "hull",
        )
        .uv(UvMode::Box)
        .collision(Collision::Hull),
    )
    .element(Element::new(
        "porch-light",
        luminous([1.6, 0.08, 0.06], [-0.8, 2.85, -4.82]),
        "light",
    ))
    .element(Element::new(
        "porch-sign",
        plate([1.2, 0.3, 0.04], [-0.6, 3.05, -4.84]),
        "marking",
    ))
    .element(Element::new(
        "threshold",
        plate([2.0, 0.045, 3.2], [-1.0, 0.005, -5.9]),
        "concrete",
    ))
    .build()
}

fn tower_shaft() -> Result<Part, ValidationError> {
    let slits = Geometry::union_all((0..4).map(|quarter| {
        Geometry::cuboid([0.36, 3.6, 0.8]).placed(super::around(
            f64::from(quarter) * FRAC_PI_2 + PI / 4.0,
            Pose::at([-0.18, 4.6, -2.8]),
        ))
    }));
    let shaft = Geometry::revolve(
        [[2.1, 3.5], [2.4, 3.5], [2.4, SHAFT_TOP], [2.1, SHAFT_TOP]],
        ROUND,
    )
    .subtract(slits);
    Part::builder("scifi:tower-shaft")
        .element(
            Element::new("shaft", shaft, "hull")
                .cut_material("dark")
                .uv(UvMode::Cylindrical { axis: Axis::Y })
                .collision(Collision::Hull),
        )
        .element(
            Element::new(
                "slit-glass",
                Geometry::union_all((0..4).map(|quarter| {
                    Geometry::cuboid([0.3, 3.5, 0.04]).placed(super::around(
                        f64::from(quarter) * FRAC_PI_2 + PI / 4.0,
                        Pose::at([-0.15, 4.65, -2.25]),
                    ))
                })),
                "glass",
            )
            .standalone(),
        )
        .element(
            Element::new(
                "bands",
                Geometry::cylinder(2.47, 0.14, ROUND)
                    .placed(Pose::at([0.0, 4.2, 0.0]))
                    .arrayed(2, Pose::at([0.0, 4.25, 0.0])),
                "trim",
            )
            .uv(UvMode::Cylindrical { axis: Axis::Y }),
        )
        .element(
            Element::new(
                "lobby-ceiling",
                Geometry::revolve(
                    [
                        [0.0, LOBBY_CEILING],
                        [PLINTH_INNER, LOBBY_CEILING],
                        [PLINTH_INNER, 3.5],
                        [0.0, 3.5],
                    ],
                    ROUND,
                ),
                "concrete",
            )
            .collision(Collision::Hull),
        )
        .element(
            Element::new(
                "lobby-ceiling-finish",
                Geometry::revolve(
                    [
                        [0.0, LOBBY_CEILING - LINER],
                        [PLINTH_INNER - LINER, LOBBY_CEILING - LINER],
                        [PLINTH_INNER - LINER, LOBBY_CEILING],
                        [0.0, LOBBY_CEILING],
                    ],
                    ROUND,
                ),
                "spine",
            )
            .interior(),
        )
        .element(core("core", LOBBY_CEILING - LINER, DECK_FLOOR))
        .element(Element::new(
            "downpipe",
            Geometry::cylinder(0.09, SHAFT_TOP - 3.5 + 1.0, SMALL)
                .placed(Pose::at([0.0, 3.5, 2.5])),
            "rust",
        ))
        .element(Element::new(
            "shaft-fins",
            plate([0.18, 4.6, 0.45], [-0.09, 3.7, -2.62])
                .arrayed(4, Pose::default().rotated(yaw(FRAC_PI_2))),
            "trim",
        ))
        .element(
            Element::new(
                "shaft-neon",
                luminous([0.075, 3.9, 0.06], [-0.0375, 4.05, -2.66])
                    .arrayed(4, Pose::default().rotated(yaw(FRAC_PI_2))),
                "neon",
            )
            .far(),
        )
        .build()
}

fn tower_deck() -> Result<Part, ValidationError> {
    let base = vec![
        [0.0, SHAFT_TOP],
        [2.4, SHAFT_TOP],
        [3.7, 9.6],
        [4.0, 10.2],
        [4.0, DECK_FLOOR],
        [0.0, DECK_FLOOR],
    ];
    let glass_section = [
        [GLAZING, DECK_FLOOR],
        [GLAZING + 0.04, DECK_FLOOR],
        [GLAZING + 0.04, DECK_CEILING],
        [GLAZING, DECK_CEILING],
    ];
    let mut part = Part::builder("scifi:tower-deck")
        .element(turned("deck-base", base, "hull").collision(Collision::Hull))
        .element(round_floor(
            "floor",
            GLAZING,
            DECK_FLOOR,
            DECK_FLOOR + LINER,
        ))
        .element(core("core", DECK_FLOOR + LINER, DECK_CEILING - LINER))
        .element(lift_door("lift-door", DECK_FLOOR + LINER))
        .element(Element::new(
            "mullions",
            Geometry::cuboid([0.08, DECK_CEILING - DECK_FLOOR, 0.12])
                .placed(Pose::at([-0.04, DECK_FLOOR, -(GLAZING + 0.16)]))
                .arrayed(
                    16,
                    Pose::default().rotated(yaw(std::f64::consts::TAU / 16.0)),
                ),
            "trim",
        ))
        .element(
            Element::new(
                "deck-light",
                Geometry::cylinder(4.02, 0.06, ROUND).placed(Pose::at([0.0, 10.3, 0.0])),
                "light",
            )
            .uv(UvMode::Cylindrical { axis: Axis::Y }),
        );
    // The glazing as runs of glass, each with its own proxy, so the deck is
    // enclosed without its proxy filling the room.
    for element in ring(
        "glazing",
        &glass_section,
        0.0,
        360.0,
        22.5,
        DQuat::IDENTITY,
        &[],
        "glass",
        "glass",
        Axis::Y,
    ) {
        part = part.element(element.standalone());
    }
    part.build()
}

/// The tower's roof over the observation deck, in the storey above it as a
/// ceiling is: the roof, its finish, the cap and the mast.
fn tower_roof() -> Result<Part, ValidationError> {
    let roof = vec![
        [0.0, DECK_CEILING],
        [4.0, DECK_CEILING],
        [4.0, 13.1],
        [3.5, 13.7],
        [2.4, 14.1],
        [0.0, 14.1],
    ];
    let mut cap = vec![[0.0, 14.1]];
    cap.extend(arc([0.0, 14.1], [2.3, 1.8], 0.0, FRAC_PI_2, 10));
    Part::builder("scifi:tower-roof")
        .element(turned("deck-roof", roof, "hull").collision(Collision::Hull))
        .element(turned("cap", cap, "hull"))
        .element(
            Element::new(
                "ceiling",
                Geometry::revolve(
                    [
                        [0.0, DECK_CEILING - LINER],
                        [GLAZING, DECK_CEILING - LINER],
                        [GLAZING, DECK_CEILING],
                        [0.0, DECK_CEILING],
                    ],
                    ROUND,
                ),
                "spine",
            )
            .interior(),
        )
        .element(Element::new(
            "mast",
            Geometry::union_all([
                Geometry::cylinder(0.09, 4.0, 10).placed(Pose::at([0.0, 15.7, 0.0])),
                Geometry::cuboid([1.6, 0.06, 0.06]).placed(Pose::at([-0.8, 17.4, -0.03])),
                Geometry::cuboid([1.1, 0.06, 0.06]).placed(Pose::at([-0.55, 18.4, -0.03])),
            ]),
            "trim",
        ))
        .element(beacon("beacon", 0.16, [0.0, 19.8, 0.0]))
        .element(Element::new(
            "crown-coping",
            Geometry::revolve(
                [[3.92, 12.99], [4.18, 12.99], [4.18, 13.14], [3.92, 13.22]],
                ROUND,
            ),
            "trim",
        ))
        .element(
            Element::new(
                "crown-neon",
                Geometry::revolve(
                    [[3.97, 12.89], [4.055, 12.89], [4.055, 12.95], [3.97, 12.95]],
                    ROUND,
                ),
                "neon",
            )
            .far(),
        )
        .build()
}

// ------------------------------------------------------------------ placing

/// A per-instance finish for a placed habitat: an adobe colour, a hull colour
/// and a hull wear, each overriding the palette on the shell.
#[derive(Clone, Copy, Debug, Default)]
pub struct Look {
    /// Adobe colour, linear RGB.
    pub adobe: Option<[f32; 3]>,
    /// Hull plating colour, linear RGB.
    pub hull: Option<[f32; 3]>,
    /// Hull plating wear, `0..=1`.
    pub wear: Option<f32>,
}

impl Look {
    fn dress(self, mut instance: Instance, adobe: bool, hull: bool) -> Instance {
        if adobe && let Some(color) = self.adobe {
            instance = instance.binding(
                "adobe",
                Binding::new("library:adobe").param("color", ParamValue::Color(color)),
            );
        }
        if hull && (self.hull.is_some() || self.wear.is_some()) {
            let mut binding = Binding::new("library:hull-plating");
            if let Some(color) = self.hull {
                binding = binding.param("color", ParamValue::Color(color));
            }
            if let Some(wear) = self.wear {
                binding = binding.param("wear", ParamValue::Float(wear));
            }
            instance = instance.binding("hull", binding);
        }
        instance
    }

    /// A dome's interior: plastered walls, and a packed earth floor where
    /// the part lays one.
    fn adobe_inside(instance: Instance, floor: bool) -> Instance {
        let instance = instance.material("spine", "showcase:interior-plaster");
        if floor {
            instance.material("floor", "library:adobe")
        } else {
            instance
        }
    }
}

/// The kit's enterable buildings.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Habitat {
    /// An adobe dome home with one round room.
    DomeHut,
    /// A small adobe dome used for storage.
    DomeAnnex,
    /// A two-storey dome house with a roof terrace.
    DomeTall,
    /// A capsule habitat on stilts.
    HabPod,
    /// A walkway tube from one pod's bulkhead to the next.
    Tube,
    /// A prefab module: an airlock and an office.
    Module,
    /// The hab tower: a lobby, a lift and an observation deck.
    Tower,
}

impl Habitat {
    /// Every habitat.
    pub const ALL: [Self; 7] = [
        Self::DomeHut,
        Self::DomeAnnex,
        Self::DomeTall,
        Self::HabPod,
        Self::Tube,
        Self::Module,
        Self::Tower,
    ];

    /// The catalogue name: `scifi-<name>` is its scene.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::DomeHut => "dome-hut",
            Self::DomeAnnex => "dome-annex",
            Self::DomeTall => "dome-tall",
            Self::HabPod => "hab-pod",
            Self::Tube => "tube",
            Self::Module => "module",
            Self::Tower => "tower",
        }
    }

    /// The habitat with a catalogue name.
    #[must_use]
    pub fn named(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|habitat| habitat.name() == name)
    }

    /// The ground the habitat stands on, from its placement origin: `[min_x,
    /// min_z]` and `[max_x, max_z]`.
    #[must_use]
    pub fn footprint(self) -> ([f64; 2], [f64; 2]) {
        match self {
            Self::DomeHut => ([-4.6, -5.6], [4.6, 4.6]),
            Self::DomeAnnex => ([-3.0, -3.8], [3.0, 3.0]),
            Self::DomeTall => ([-5.6, -6.4], [5.6, 5.6]),
            Self::HabPod => ([-5.0, -6.0], [5.0, 2.8]),
            Self::Tube => ([-1.0, -2.2], [TUBE_LENGTH + 1.0, 2.2]),
            Self::Module => ([-1.0, -2.2], [9.0, 7.0]),
            Self::Tower => ([-5.2, -7.0], [5.2, 5.2]),
        }
    }

    /// Place the habitat with its origin at `ground` on the ground plane, and
    /// declare its rooms. Instances and rooms are named `id` and `id-…`.
    pub fn place(
        self,
        builder: BuildingBuilder,
        id: &str,
        ground: Pose,
        look: Look,
    ) -> BuildingBuilder {
        match self {
            Self::DomeHut => place_dome(builder, id, ground, look, "scifi:dome-hut", HUT),
            Self::DomeAnnex => place_dome(builder, id, ground, look, "scifi:dome-annex", ANNEX),
            Self::DomeTall => place_dome_tall(builder, id, ground, look),
            Self::HabPod => place_pod(builder, id, ground, look),
            Self::Tube => place_tube(builder, id, ground, look, &[]),
            Self::Module => place_module(builder, id, ground, look),
            Self::Tower => place_tower(builder, id, ground, look),
        }
    }
}

fn place_dome(
    builder: BuildingBuilder,
    id: &str,
    ground: Pose,
    look: Look,
    part: &str,
    dome: Dome,
) -> BuildingBuilder {
    let shell = Look::adobe_inside(look.dress(Instance::new(id, part), true, false), true)
        .placed(ground)
        .group("ground");
    let roof = Look::adobe_inside(
        look.dress(
            Instance::new(format!("{id}-roof"), format!("{part}-roof")),
            true,
            false,
        ),
        false,
    )
    .placed(ground)
    .group("upper");
    let mut builder = builder.instance(shell).instance(roof);
    let (min, max) = dome.room();
    builder = builder.room(room(
        format!("{id}-room"),
        ground,
        min,
        max,
        "ground",
        &["door"],
    ));
    let floor = 0.05;
    let r = dome.inner();
    let furniture: Vec<(&str, Pose)> = if dome.radius > 2.5 {
        vec![
            ("scifi:bunk", spot(0.2, floor, r - 0.62, 180.0)),
            ("scifi:table", spot(1.35, floor, -0.45, 0.0)),
            ("scifi:stool", spot(1.35, floor, -1.3, 0.0)),
            ("scifi:stool", spot(2.1, floor, -0.1, 0.0)),
            ("scifi:console", spot(-r + 0.45, floor, -0.3, 270.0)),
            ("scifi:shelving", spot(-1.55, floor, 1.75, 225.0)),
            ("scifi:crate-small", spot(1.7, floor, 1.55, 20.0)),
            ("scifi:ceiling-light", spot(0.0, dome.ceiling(), 0.0, 0.0)),
        ]
    } else {
        vec![
            ("scifi:shelving", spot(0.0, floor, r - 0.35, 180.0)),
            ("scifi:crate-small", spot(-0.7, floor, -0.2, 70.0)),
            ("scifi:crate-small", spot(-0.7, floor + 0.6, -0.2, 80.0)),
            ("scifi:ceiling-light", spot(0.0, dome.ceiling(), 0.0, 0.0)),
        ]
    };
    for (index, (part, local)) in furniture.into_iter().enumerate() {
        // A ceiling light hangs from the dome, in the dome's storey.
        let group = if part == "scifi:ceiling-light" {
            "upper"
        } else {
            "ground"
        };
        builder = furnish(
            builder,
            format!("{id}-f{index}"),
            part,
            ground,
            local,
            group,
        );
    }
    builder
}

fn place_dome_tall(
    builder: BuildingBuilder,
    id: &str,
    ground: Pose,
    look: Look,
) -> BuildingBuilder {
    let dressed = |instance: Instance, group: &str, floor: bool| {
        Look::adobe_inside(look.dress(instance, true, false), floor)
            .placed(ground)
            .group(group)
    };
    let mut builder = builder
        .instance(dressed(
            Instance::new(id, "scifi:dome-tall-ground"),
            "ground",
            true,
        ))
        .instance(dressed(
            Instance::new(format!("{id}-deck"), "scifi:dome-tall-deck"),
            "upper",
            true,
        ))
        .instance(dressed(
            Instance::new(format!("{id}-upper"), "scifi:dome-tall-upper"),
            "upper",
            false,
        ))
        .instance(dressed(
            Instance::new(format!("{id}-roof"), "scifi:dome-tall-roof"),
            "top",
            false,
        ));
    let half = (TALL_INNER - LINER - 0.15) / std::f64::consts::SQRT_2;
    let upper = (UPPER_INNER - LINER - 0.12) / std::f64::consts::SQRT_2;
    builder = builder
        .room(room(
            format!("{id}-hall"),
            ground,
            [-half, 0.05, -half],
            [half, TALL_DRUM - LINER, half],
            "ground",
            &["door"],
        ))
        .room(room(
            format!("{id}-loft"),
            ground,
            [-upper, TALL_DECK + LINER, -upper],
            [upper, UPPER_DRUM - 0.02, upper],
            "upper",
            &["door-up"],
        ));
    let floor = 0.05;
    let loft = TALL_DECK + LINER;
    let furniture = [
        ("scifi:holo-table", spot(-1.2, floor, 1.3, 0.0), "ground"),
        (
            "scifi:galley",
            spot(0.0, floor, TALL_INNER - 0.55, 180.0),
            "ground",
        ),
        ("scifi:table", spot(-1.6, floor, -1.1, 0.0), "ground"),
        ("scifi:stool", spot(-2.3, floor, -1.4, 0.0), "ground"),
        ("scifi:stool", spot(-1.0, floor, -1.7, 0.0), "ground"),
        (
            "scifi:lockers",
            spot(TALL_INNER - 0.4, floor, -1.4, 90.0),
            "ground",
        ),
        (
            "scifi:console",
            spot(-TALL_INNER + 0.45, floor, 0.4, 270.0),
            "ground",
        ),
        (
            "scifi:ceiling-light",
            spot(-1.4, TALL_DRUM - LINER, 0.4, 0.0),
            "upper",
        ),
        (
            "scifi:ceiling-light",
            spot(1.4, TALL_DRUM - LINER, -1.6, 0.0),
            "upper",
        ),
        (
            "scifi:bunk",
            spot(-0.4, loft, UPPER_INNER - 0.6, 180.0),
            "upper",
        ),
        ("scifi:crate-small", spot(-1.4, loft, -0.9, 35.0), "upper"),
        (
            "scifi:ceiling-light",
            spot(0.0, UPPER_DRUM + UPPER_RISE - 0.32, 0.0, 0.0),
            "top",
        ),
    ];
    for (index, (part, local, group)) in furniture.into_iter().enumerate() {
        builder = furnish(
            builder,
            format!("{id}-f{index}"),
            part,
            ground,
            local,
            group,
        );
    }
    builder
}

fn place_pod(builder: BuildingBuilder, id: &str, ground: Pose, look: Look) -> BuildingBuilder {
    let axis = ground.compose(Pose::at([0.0, POD_AXIS, 0.0]));
    let mut builder = builder
        .instance(
            look.dress(Instance::new(id, "scifi:hab-pod"), false, true)
                .placed(axis)
                .group("ground"),
        )
        .instance(
            look.dress(
                Instance::new(format!("{id}-roof"), "scifi:hab-pod-roof"),
                false,
                true,
            )
            .placed(axis)
            .group("upper"),
        );
    let l = POD_BARREL;
    builder = builder.room(room(
        format!("{id}-cabin"),
        axis,
        [-l + 0.1, POD_DECK, -1.4],
        [l - 0.1, LOCK_TOP, 1.4],
        "ground",
        &["hatch", "lock-east", "lock-west"],
    ));
    let deck = POD_DECK;
    let furniture = [
        ("scifi:bunk", spot(-1.35, deck, 1.05, 180.0)),
        ("scifi:galley", spot(1.35, deck, 1.12, 180.0)),
        ("scifi:console", spot(1.6, deck, -1.1, 0.0)),
        ("scifi:lockers", spot(-1.65, deck, -1.1, 0.0)),
    ];
    for (index, (part, local)) in furniture.into_iter().enumerate() {
        builder = furnish(
            builder,
            format!("{id}-f{index}"),
            part,
            axis,
            local,
            "ground",
        );
    }
    builder
}

/// Place a tube from the east bulkhead of a pod at `ground` (a pod's own
/// ground origin, moved along +X by the pod's barrel), naming the pods'
/// door portals its walkway opens onto.
fn place_tube(
    builder: BuildingBuilder,
    id: &str,
    ground: Pose,
    look: Look,
    portals: &[&str],
) -> BuildingBuilder {
    let axis = ground.compose(Pose::at([0.0, POD_AXIS, 0.0]));
    let builder = builder
        .instance(
            look.dress(Instance::new(id, "scifi:tube"), false, true)
                .placed(axis)
                .group("ground"),
        )
        .instance(
            look.dress(
                Instance::new(format!("{id}-roof"), "scifi:tube-roof"),
                false,
                true,
            )
            .placed(axis)
            .group("upper"),
        );
    builder.room(room(
        format!("{id}-walk"),
        axis,
        [0.1, POD_DECK, -0.75],
        [TUBE_LENGTH - 0.1, LOCK_TOP, 0.75],
        "ground",
        portals,
    ))
}

fn place_module(builder: BuildingBuilder, id: &str, ground: Pose, look: Look) -> BuildingBuilder {
    let mut builder = builder
        .instance(
            look.dress(Instance::new(id, "scifi:module"), false, true)
                .placed(ground)
                .group("ground"),
        )
        .instance(
            look.dress(
                Instance::new(format!("{id}-roof"), "scifi:module-roof"),
                false,
                true,
            )
            .placed(ground)
            .group("upper"),
        );
    let t = MODULE_WALL + LINER;
    let floor = MODULE_FLOOR + LINER;
    let ceiling = MODULE_CEILING - LINER;
    builder = builder
        .room(room(
            format!("{id}-airlock"),
            ground,
            [t, floor, t],
            [PARTITION_X, ceiling, MODULE[2] - t],
            "ground",
            &["door", "inner-door"],
        ))
        .room(room(
            format!("{id}-office"),
            ground,
            [PARTITION_X + PARTITION, floor, t],
            [MODULE[0] - t, ceiling, MODULE[2] - t],
            "ground",
            &["inner-door", "window", "window-side"],
        ));
    let furniture = [
        (
            "scifi:lockers",
            spot(1.45, floor, MODULE[2] - t - 0.3, 180.0),
        ),
        ("scifi:crate-small", spot(2.0, floor, 1.4, 90.0)),
        ("scifi:console", spot(4.7, floor, 0.95, 180.0)),
        ("scifi:console", spot(6.5, floor, 0.95, 180.0)),
        ("scifi:holo-table", spot(5.6, floor, 4.2, 0.0)),
        (
            "scifi:shelving",
            spot(MODULE[0] - t - 0.3, floor, 4.6, 90.0),
        ),
        ("scifi:stool", spot(4.7, floor, 1.75, 0.0)),
        ("scifi:stool", spot(6.5, floor, 1.75, 0.0)),
    ];
    for (index, (part, local)) in furniture.into_iter().enumerate() {
        builder = furnish(
            builder,
            format!("{id}-f{index}"),
            part,
            ground,
            local,
            "ground",
        );
    }
    builder
}

fn place_tower(builder: BuildingBuilder, id: &str, ground: Pose, look: Look) -> BuildingBuilder {
    let dressed = |instance: Instance, group: &str, adobe: bool| {
        look.dress(instance, adobe, true)
            .placed(ground)
            .group(group)
    };
    let mut builder = builder
        .instance(dressed(
            Instance::new(id, "scifi:tower-base"),
            "ground",
            true,
        ))
        .instance(dressed(
            Instance::new(format!("{id}-shaft"), "scifi:tower-shaft"),
            "upper",
            false,
        ))
        .instance(dressed(
            Instance::new(format!("{id}-deck"), "scifi:tower-deck"),
            "top",
            false,
        ))
        .instance(dressed(
            Instance::new(format!("{id}-roof"), "scifi:tower-roof"),
            "roof",
            false,
        ));
    builder = builder
        .room(room(
            format!("{id}-lobby"),
            ground,
            [-1.3, 0.05, -1.7],
            [1.3, LOBBY_CEILING - LINER, 0.55],
            "ground",
            &["door"],
        ))
        .room(room(
            format!("{id}-observation"),
            ground,
            [-2.4, DECK_FLOOR + LINER, -2.6],
            [2.4, DECK_CEILING - LINER, 0.5],
            "top",
            &[],
        ));
    let deck = DECK_FLOOR + LINER;
    let furniture = [
        ("scifi:console", spot(-1.3, 0.05, -0.9, 60.0), "ground"),
        ("scifi:lockers", spot(1.35, 0.05, 0.2, 90.0), "ground"),
        (
            "scifi:ceiling-light",
            spot(0.0, LOBBY_CEILING - LINER, -0.6, 0.0),
            "upper",
        ),
        ("scifi:console", spot(0.0, deck, -3.1, 0.0), "top"),
        ("scifi:console", spot(-2.3, deck, -2.0, 45.0), "top"),
        ("scifi:console", spot(2.3, deck, -2.0, -45.0), "top"),
        ("scifi:holo-table", spot(0.0, deck, -1.35, 0.0), "top"),
        (
            "scifi:ceiling-light",
            spot(0.0, DECK_CEILING - LINER, -1.0, 0.0),
            "roof",
        ),
    ];
    for (index, (part, local, group)) in furniture.into_iter().enumerate() {
        builder = furnish(
            builder,
            format!("{id}-f{index}"),
            part,
            ground,
            local,
            group,
        );
    }
    builder
}

/// A chain of `count` pods along the local +X axis from `ground`, joined by
/// tubes, each pod with its own look.
pub(crate) fn pod_chain(
    mut builder: BuildingBuilder,
    id: &str,
    ground: Pose,
    looks: &[Look],
) -> BuildingBuilder {
    let pitch = 2.0 * POD_BARREL + TUBE_LENGTH;
    for (index, look) in looks.iter().enumerate() {
        #[expect(clippy::cast_precision_loss, reason = "a handful of pods")]
        let at_pod = ground.compose(Pose::at([pitch * index as f64, 0.0, 0.0]));
        builder = Habitat::HabPod.place(builder, &format!("{id}-pod-{index}"), at_pod, *look);
        if index + 1 < looks.len() {
            builder = place_tube(
                builder,
                &format!("{id}-tube-{index}"),
                at_pod.compose(Pose::at([POD_BARREL, 0.0, 0.0])),
                *look,
                &["lock-east", "lock-west"],
            );
        }
    }
    builder
}

/// Every part the habitats and their furniture use.
///
/// # Errors
///
/// If a part does not validate.
pub fn parts() -> Result<Vec<Part>, ValidationError> {
    let mut parts = vec![
        dome_part("scifi:dome-hut", HUT)?,
        dome_roof("scifi:dome-hut-roof", HUT)?,
        dome_part("scifi:dome-annex", ANNEX)?,
        dome_roof("scifi:dome-annex-roof", ANNEX)?,
        tall_ground()?,
        tall_deck()?,
        tall_upper()?,
        tall_roof()?,
        hab_pod()?,
        hab_pod_roof()?,
        tube()?,
        tube_roof()?,
        module()?,
        module_roof()?,
        tower_base()?,
        tower_shaft()?,
        tower_deck()?,
        tower_roof()?,
    ];
    parts.extend(super::furniture::parts()?);
    Ok(parts)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keep_bounds_a_box_to_what_it_keeps() {
        let piece = keep(
            Geometry::cuboid([4.0, 4.0, 4.0]),
            [1.0, f64::NEG_INFINITY, f64::NEG_INFINITY],
            [f64::INFINITY, 2.0, f64::INFINITY],
        );
        piece.check().expect("a kept box validates");
    }

    #[test]
    fn a_door_sector_is_centred_on_its_door() {
        let opening = Opening {
            angle: 90.0,
            half: 10.0,
            cutter: Geometry::cuboid([1.0; 3]),
            portal: Some("door"),
        };
        assert!(centred(&opening, 75.0, 105.0));
        assert!(!centred(&opening, 105.0, 135.0));
        assert!(overlaps(&opening, 95.0, 125.0));
        assert!(!overlaps(&opening, 105.0, 135.0));
        // Across the wrap: a wedge from 435 to 465 is the one from 75 to 105.
        assert!(centred(&opening, 435.0, 465.0));
    }
    #[test]
    fn decorative_collars_leave_the_cabin_axis_clear_and_drop_at_distance() {
        use ashlar::{GeometryMesher, LodPolicy};
        use ashlar_manifold::ManifoldMesher;
        for part in [hab_pod(), hab_pod_roof(), tube()] {
            let part = part.expect("habitat builds");
            for element in part
                .elements
                .iter()
                .filter(|e| e.id.starts_with("rib") || e.id == "dock-collars" || e.id == "flanges")
            {
                let mesh = ManifoldMesher::default()
                    .mesh(&element.geometry)
                    .expect("collar meshes");
                for triangle in mesh.triangles() {
                    let normal = (triangle[1] - triangle[0]).cross(triangle[2] - triangle[0]);
                    if normal.x.abs() < 1e-8 {
                        continue;
                    }
                    let cross = |a: glam::DVec3, b: glam::DVec3| a.y * b.z - a.z * b.y;
                    let signs = [
                        cross(triangle[0], triangle[1]),
                        cross(triangle[1], triangle[2]),
                        cross(triangle[2], triangle[0]),
                    ];
                    assert!(
                        !(signs.iter().all(|s| *s >= -1e-10) || signs.iter().all(|s| *s <= 1e-10)),
                        "{}/{} seals the axis",
                        part.id,
                        element.id
                    );
                }
                assert!(
                    element
                        .geometry
                        .simplified(&LodPolicy::ladder()[1])
                        .is_none(),
                    "{}/{} keeps fine trim",
                    part.id,
                    element.id
                );
            }
        }
    }
}
