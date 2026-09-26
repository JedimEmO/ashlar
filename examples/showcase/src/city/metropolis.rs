//! Towers, alleys and blocks, and a dark city from a seed.
use ashlar::{MergeGroup, Room};

use super::{
    BLOCK, Binding, Building, BuildingBuilder, FRAC_PI_2, Instance, KERB, LINER, LOBBY, LOT, PI,
    PITCH, ParamValue, Pose, ROAD, SIDEWALK, SLAB, STOREY, ValidationError,
    alley::GUTTER,
    palette, spot,
    storey::{
        Crown, LANDMARK_CROWN, LANDMARK_LOBBY, PANES, Plan, ROOF_THICKNESS, SKY_HEIGHT, SPIRE,
        Style, TENEMENT, part_id, portals,
    },
    street::{SEGMENT, facing},
};

/// What kind of building a tower is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// A quad lot's tower: eighteen by sixteen at its foot, stepping in twice
    /// as it rises, in one of three styles with plans of their own. The frame
    /// style, which a scene of one tower shows, steps in to fourteen by twelve
    /// past a dozen storeys and to ten by ten past forty.
    Tower,
    /// A landmark filling a whole lot: thirty by twenty-six at its foot,
    /// stepping in twice to fourteen metres square.
    Landmark,
    /// A tenement block, thirty-eight by twelve, a handful of storeys.
    Tenement,
}

/// What stands on a tower's roof.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Topper {
    /// A needle spire.
    Spire,
    /// A lattice mast.
    Mast,
    /// A raised helipad.
    Helipad,
    /// Rooftop plant.
    Plant,
    /// A dish antenna.
    Dish,
    /// A cluster of radio masts with warning lights.
    Antennas,
    /// A holo billboard facing the street.
    Billboard,
    /// A water tank on legs.
    Tank,
}

impl Topper {
    fn part(self) -> &'static str {
        match self {
            Self::Spire => "city:spire",
            Self::Mast => "city:mast",
            Self::Helipad => "city:helipad",
            Self::Plant => "city:roof-plant",
            Self::Dish => "city:dish",
            Self::Antennas => "city:antennas",
            Self::Billboard => "city:roof-billboard",
            Self::Tank => "city:water-tank",
        }
    }
}

/// The highest storey a tower may have, lobby included.
pub(crate) const MAX_STOREYS: u32 = 120;

/// One tower on a lot: how tall it is, what stands on its roof, how it is
/// dressed and what crowns it.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Spec {
    /// Storeys, lobby included.
    pub storeys: u32,
    pub topper: Topper,
    pub style: Style,
    /// What finishes its top under the topper; none on a tenement.
    pub crown: Option<Crown>,
}

impl Spec {
    /// A tower of `style` with its crown, or the crown a topper needs: a
    /// helipad or a billboard stands on a penthouse, never among blades or on
    /// a narrow step.
    fn new(storeys: u32, topper: Topper, style: Style, crown: Crown) -> Self {
        let crown = if matches!(topper, Topper::Helipad | Topper::Billboard) {
            Crown::Screen
        } else {
            crown
        };
        Self {
            storeys,
            topper,
            style,
            crown: Some(crown),
        }
    }
}

/// A building's tiers, widest first, and how many storeys above the lobby
/// each carries. The lobby stands on the first tier's plan.
///
/// A tower's style sets its steps: a frame tower keeps its foot for three
/// fifths of a mid-height tower. A banded slab leaves a short podium for a
/// long, broad middle tier; a cage steps in early to carry a narrow shaft.
fn tiers(kind: Kind, style: Style, storeys: u32) -> Vec<(&'static Plan, u32)> {
    let upper = storeys.clamp(2, MAX_STOREYS) - 1;
    let [foot, middle_plan, top_plan] = style.plans();
    // Up to how many storeys one tier carries a tower, then two, and the
    // shares of the foot for two tiers and of the foot and middle for three.
    // Twentieths, so a share is whole storeys.
    let (one, two, [foot_two, foot_three, middle_three]) = match style {
        Style::Frame => (12, 40, [12, 10, 6]),
        Style::Band => (14, 34, [4, 3, 14]),
        Style::Cage => (10, 32, [4, 3, 4]),
    };
    let share = |twentieths: u32| upper * twentieths / 20;
    match kind {
        Kind::Tenement => vec![(&TENEMENT, upper)],
        Kind::Tower if upper <= one => vec![(foot, upper)],
        Kind::Tower if upper <= two => {
            let base = share(foot_two);
            vec![(foot, base), (middle_plan, upper - base)]
        }
        Kind::Tower => {
            let base = share(foot_three);
            let middle = share(middle_three);
            vec![
                (foot, base),
                (middle_plan, middle),
                (top_plan, upper - base - middle),
            ]
        }
        Kind::Landmark => {
            let upper = upper.max(6);
            let base = upper * 2 / 5;
            let middle = upper * 7 / 20;
            vec![
                (&SPIRE[0], base),
                (&SPIRE[1], middle),
                (&SPIRE[2], upper - base - middle),
            ]
        }
    }
}

/// One level of a tower's stack: the part, how tall it is, the plan it
/// stands on, and whether it is a storey (with a room) or a roof.
struct Level {
    part: String,
    /// The ceiling part hung at the top of this level, if it is a storey.
    ceiling: Option<String>,
    height: f64,
    plan: &'static Plan,
    lobby: bool,
    storey: bool,
}

fn levels(kind: Kind, style: Style, storeys: u32) -> Vec<Level> {
    let tiers = tiers(kind, style, storeys);
    let mut stack = vec![Level {
        part: part_id(tiers[0].0, "lobby"),
        ceiling: Some(part_id(tiers[0].0, "ceiling")),
        height: if kind == Kind::Landmark {
            LANDMARK_LOBBY
        } else {
            LOBBY
        },
        plan: tiers[0].0,
        lobby: true,
        storey: true,
    }];
    for (plan, count) in tiers {
        for floor in 0..count {
            let sky = kind == Kind::Landmark && plan.tier < 2 && floor == count - 1;
            stack.push(Level {
                part: part_id(plan, if sky { "sky-floor" } else { "floor" }),
                ceiling: Some(part_id(plan, "ceiling")),
                height: if sky { SKY_HEIGHT } else { STOREY },
                plan,
                lobby: false,
                storey: true,
            });
        }
        // A tier's roof is the terrace the next tier stands on.
        stack.push(Level {
            part: part_id(plan, "roof"),
            ceiling: None,
            height: ROOF_THICKNESS,
            plan,
            lobby: false,
            storey: false,
        });
    }
    stack
}

/// The name of storey `index`'s group.
#[must_use]
pub(crate) fn storey_group(index: u32) -> String {
    format!("storey-{}", index.min(MAX_STOREYS))
}

/// Declare the storey groups a tower's levels are put in, `storey-0` (every
/// lobby) to [`MAX_STOREYS`]. The building stays unmerged — a group only gives
/// its instances and rooms a storey — so a game, or the preview's storey cut,
/// can hide everything above the floor a player stands on across a whole city.
pub(crate) fn storey_groups(mut builder: BuildingBuilder) -> BuildingBuilder {
    for index in 0..=MAX_STOREYS {
        builder = builder
            .group(MergeGroup::new(storey_group(index)).storey(i32::try_from(index).unwrap_or(0)));
    }
    builder
}

/// Glazing a storey's panes pick from, weighted: mostly dark, some lit.
const WINDOWS: [(&str, f64); 4] = [
    ("showcase:window-dark", 0.5),
    ("showcase:window-dim", 0.22),
    ("showcase:window-warm", 0.16),
    ("showcase:window-cold", 0.12),
];

/// The share of storeys dark from end to end: nobody works there any more.
const DARK_STOREYS: f64 = 0.3;

/// Neon a tower's crown, a sign or a lantern may wear.
const NEON: [&str; 4] = [
    "showcase:neon-magenta",
    "showcase:neon-cyan",
    "showcase:neon-amber",
    "showcase:neon-green",
];

/// Holo screens a billboard or a sign may show.
const HOLO: [&str; 3] = [
    "library:holo-sign",
    "showcase:holo-magenta",
    "showcase:holo-cyan",
];

/// The key a roll in `0..1` picks from a weighted list.
fn weighted(keys: &[(&'static str, f64)], roll: f64) -> &'static str {
    let total: f64 = keys.iter().map(|(_, weight)| weight).sum();
    let mut at = roll * total;
    for (key, weight) in keys {
        if at < *weight {
            return key;
        }
        at -= weight;
    }
    keys.last().map_or("", |(key, _)| key)
}

/// One of `keys`, by a roll in `0..1`.
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss,
    reason = "a roll in 0..1 scaled to a short list's index"
)]
fn one_of(keys: &[&'static str], roll: f64) -> &'static str {
    keys[((roll * keys.len() as f64) as usize).min(keys.len() - 1)]
}

/// A small hash of a string, for seeding a tower by its id.
fn name_hash(name: &str) -> u64 {
    name.bytes().fold(0xcbf2_9ce4_8422_2325, |hash, byte| {
        (hash ^ u64::from(byte)).wrapping_mul(0x0000_0100_0000_01b3)
    })
}

/// Add one building of `kind`, its rooms, its lit storeys, its crown and its
/// topper, standing with its footprint centred on `base` and its front facing
/// `base`'s -Z. A tower is dressed in `spec`'s style; a landmark and a
/// tenement have their own plans.
///
/// Every storey lights a third of its offices at a time from `WINDOWS`,
/// seeded by `seed` and the building's `prefix`, a share of them
/// (`DARK_STOREYS`) stay dark end to end, and the building wears one
/// neon colour round its crowns. Every level is put in its storey's group —
/// the lobby in `storey-0` — and a roof or a crown in the group of the storey
/// it covers, so `builder` must already declare them: see [`storey_groups`].
#[expect(
    clippy::too_many_lines,
    reason = "storey, crown and rooftop placement share one height cursor"
)]
pub(crate) fn tower(
    mut builder: BuildingBuilder,
    prefix: &str,
    kind: Kind,
    base: Pose,
    spec: Spec,
    seed: u64,
) -> BuildingBuilder {
    let Spec {
        storeys,
        topper,
        style,
        crown: finish,
    } = spec;
    if kind == Kind::Landmark {
        builder = builder.instance(
            Instance::new(format!("{prefix}/forecourt"), "city:landmark-forecourt")
                .placed(base)
                .group(storey_group(0)),
        );
        for (index, x) in [-13.0, 13.0].into_iter().enumerate() {
            builder = builder.instance(
                Instance::new(format!("{prefix}/holo-{index}"), "city:holo-pillar")
                    .placed(base.compose(Pose::at([x, 0.0, -16.0])))
                    .group(storey_group(0)),
            );
        }
    }
    let seed = seed ^ name_hash(prefix);
    let crown = one_of(&NEON, hash(seed, 0, 0, 1));
    // A landmark's corners are lit their whole height, and a third of the
    // towers'; nothing else's.
    let lines = match kind {
        Kind::Landmark => true,
        Kind::Tower => hash(seed, 0, 0, 3) < 0.35,
        Kind::Tenement => false,
    };
    let mut y = 0.0;
    let mut previous: Option<String> = None;
    let mut storey = 0_u32;
    let stack = levels(kind, style, storeys);
    let mut top: &Plan = stack[0].plan;
    let next_plans: Vec<_> = stack.iter().skip(1).map(|level| level.plan).collect();
    for (index, level) in stack.into_iter().enumerate() {
        let id = format!("{prefix}/{index:03}");
        let offset = [-level.plan.width / 2.0, -level.plan.depth / 2.0];
        let local = base.compose(Pose::at([offset[0], y, offset[1]]));
        let group = storey_group(storey);
        let mut instance = match &previous {
            None => Instance::new(&id, &level.part).placed(local),
            Some(below) => Instance::new(&id, &level.part).attach_aligned("bottom", below, "top"),
        }
        .group(&group);
        let floor = i64::from(storey);
        if level.storey {
            instance = instance.material("floor", "showcase:dark-paving");
            if !level.lobby {
                instance = instance.material("glass", "library:glass");
            }
            let dark = !level.lobby && hash(seed, floor, 0, 9) < DARK_STOREYS;
            for (salt, slot) in (10..).zip(PANES) {
                let roll = hash(seed, floor, 0, salt);
                if !dark {
                    instance = instance.material(slot, weighted(&WINDOWS, roll));
                }
            }
            if dark || hash(seed, floor, 0, 20) < 0.4 {
                instance = instance.material("strip", "library:dark-recess");
            }
            if kind == Kind::Tenement {
                instance = instance.material("clad", "showcase:alley-wall");
            }
            if lines {
                instance = instance.material("edge", crown);
            }
            if level.lobby {
                instance = instance.material("crown", crown);
            }
            let (min, size) = level.plan.room(level.height);
            let mut room = Room::new(format!("{prefix}/room-{index:03}"), size)
                .placed(local.compose(Pose::at(min)))
                .group(&group);
            for portal in portals(level.lobby) {
                room = room.portal(portal);
            }
            builder = builder.room(room);
        } else {
            instance = instance.material("crown", crown);
            // Between the planter and the next tier, a vent fits only where
            // the exposed strip is at least a metre and a half deep.
            if next_plans
                .get(index)
                .is_some_and(|next| level.plan.depth - next.depth >= 3.0)
            {
                let side = if hash(seed, floor, 0, 91) < 0.5 {
                    -1.0
                } else {
                    1.0
                };
                builder = builder.instance(
                    Instance::new(format!("{prefix}/vent-{index}"), "city:roof-vent")
                        .placed(base.compose(Pose::at([
                            side * level.plan.width * 0.22,
                            y + level.height,
                            -level.plan.depth / 2.0 + 1.16,
                        ])))
                        .group(&group),
                );
            }
        }
        builder = builder.instance(instance);
        // The ceiling hangs at the top of the storey, in the next storey's
        // group with the slab above it, so cutting that storey away shows
        // this one's room.
        if let Some(ceiling) = level.ceiling {
            builder = builder.instance(
                Instance::new(format!("{id}/ceiling"), ceiling)
                    .placed(local.compose(Pose::at([0.0, level.height - LINER, 0.0])))
                    .group(storey_group(storey + 1)),
            );
        }
        y += level.height;
        if level.storey {
            storey += 1;
        }
        top = level.plan;
        previous = Some(id);
    }
    // The crown stands on the last roof, and the topper on the crown's deck.
    let mut edge = 0.0;
    if let (Some(finish), Some(below)) = (finish, &previous) {
        builder = builder.instance(
            Instance::new(format!("{prefix}/crown"), part_id(top, finish.name()))
                .attach_aligned("bottom", below, "top")
                .group(storey_group(storey))
                .material("crown", crown),
        );
        y += finish.deck(top);
        edge = finish.inset(top);
    }
    // A billboard stands at the deck's front edge, facing the street;
    // anything else in the middle.
    let at = if topper == Topper::Billboard {
        [0.0, y, -top.depth / 2.0 + edge + 1.6]
    } else {
        [0.0, y, 0.0]
    };
    let mut instance = Instance::new(format!("{prefix}/top"), topper.part())
        .placed(base.compose(Pose::at(at)))
        .group(storey_group(storey));
    if topper == Topper::Billboard {
        instance = instance.material("holo", one_of(&HOLO, hash(seed, 0, 0, 2)));
    }
    builder = builder.instance(instance);
    if kind == Kind::Tenement {
        for (index, x) in [-10.0, 10.0].into_iter().enumerate() {
            let part = if hash(seed, 0, i64::try_from(index).unwrap_or_default(), 92) < 0.5 {
                "city:roof-access"
            } else {
                "city:roof-plant"
            };
            builder = builder.instance(
                Instance::new(format!("{prefix}/roof-service-{index}"), part)
                    .placed(base.compose(Pose::at([x, y, 0.0])))
                    .group(storey_group(storey)),
            );
        }
    }
    builder
}

/// What stands on one lot.
#[derive(Clone, Debug)]
pub(crate) enum Lot {
    /// Four towers, one in each corner of the lot with back alleys between
    /// them, or a small plaza where a corner is `None`.
    Quad([Option<Spec>; 4]),
    /// A landmark tower in the middle of a plaza.
    Landmark(u32, Topper),
    /// Two tenement blocks facing out, fire escapes down their fronts and a
    /// yard of bins between them.
    Tenements([u32; 2]),
    /// A night market: rows of stalls round a holo pillar.
    Market,
}

/// A small deterministic hash of a seed and three coordinates, in `0..1`.
#[allow(
    clippy::cast_precision_loss,
    clippy::cast_sign_loss,
    reason = "bit mixing, then 53 bits to a unit float"
)]
fn hash(seed: u64, a: i64, b: i64, salt: u64) -> f64 {
    let mut x = seed
        ^ (a as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15)
        ^ (b as u64).wrapping_mul(0xC2B2_AE3D_27D4_EB4F)
        ^ salt.wrapping_mul(0x1656_67B1_9E37_79F9);
    x = (x ^ (x >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    x ^= x >> 31;
    (x >> 11) as f64 / (1_u64 << 53) as f64
}

/// Hull colours a parked or flying car may wear: few and dark, so a city is a
/// few bakes rather than one per car.
const CAR_COLOURS: [[f32; 3]; 4] = [
    [0.06, 0.06, 0.07],
    [0.24, 0.03, 0.03],
    [0.03, 0.08, 0.14],
    [0.30, 0.30, 0.31],
];

/// Awning colours a market stall may have.
const AWNINGS: [[f32; 3]; 3] = [[0.45, 0.04, 0.05], [0.03, 0.3, 0.32], [0.5, 0.36, 0.03]];

/// The footprint of a quad lot's tower at its foot.
const FOOT: [f64; 2] = [18.0, 16.0];

/// The quadrant centres of a lot, on X/Z from the lot's minimum corner, and
/// which way the tower there faces: out to the street nearest it in Z. The
/// towers stand flush with the lot's edges, which leaves a six-metre alley
/// between them east to west and a ten-metre one north to south.
fn quadrant(index: usize) -> ([f64; 2], f64) {
    let qx = index % 2;
    let qz = index / 2;
    let x = if qx == 0 {
        FOOT[0] / 2.0
    } else {
        LOT - FOOT[0] / 2.0
    };
    let z = if qz == 0 {
        FOOT[1] / 2.0
    } else {
        LOT - FOOT[1] / 2.0
    };
    ([x, z], if qz == 0 { 0.0 } else { PI })
}

/// A tower's wall that faces an alley: where it starts on X/Z from the lot's
/// corner, which way it runs, which way is out of it, and how long it is.
struct Wall {
    start: [f64; 2],
    along: [f64; 2],
    out: [f64; 2],
    length: f64,
}

impl Wall {
    /// The centres of its bays and the lines between them, as distances
    /// along it: the storey's own layout, a metre of chamfer at each end.
    fn bays(&self) -> (Vec<f64>, Vec<f64>) {
        let chamfer = 1.0;
        let run = self.length - 2.0 * chamfer;
        let count = (run / super::BAY).round().max(1.0);
        let pitch = run / count;
        #[allow(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "a bay count is a small positive whole number"
        )]
        let count = count as u32;
        let centres = (0..count)
            .map(|index| chamfer + pitch * (f64::from(index) + 0.5))
            .collect();
        let lines = (1..count)
            .map(|index| chamfer + pitch * f64::from(index))
            .collect();
        (centres, lines)
    }

    /// A pose on the wall's face at `along`, raised `y`, standing `off` out
    /// of it, turned so a piece's -Z points out of the wall.
    fn at(&self, lot: [f64; 2], along: f64, y: f64, off: f64) -> Pose {
        Pose::at([
            lot[0] + self.start[0] + self.along[0] * along + self.out[0] * off,
            y,
            lot[1] + self.start[1] + self.along[1] * along + self.out[1] * off,
        ])
        .rotated(facing(self.out))
    }
}

/// The walls of quadrant `index`'s tower that face the lot's alleys.
fn alley_walls(index: usize) -> [Wall; 2] {
    let (w, d) = (FOOT[0], FOOT[1]);
    let east = index.is_multiple_of(2);
    let south = index / 2 == 0;
    let x_face = if east { w } else { LOT - w };
    let z_face = if south { d } else { LOT - d };
    let z0 = if south { 0.0 } else { LOT - d };
    let x0 = if east { 0.0 } else { LOT - w };
    [
        Wall {
            start: [x_face, z0],
            along: [0.0, 1.0],
            out: [if east { 1.0 } else { -1.0 }, 0.0],
            length: d,
        },
        Wall {
            start: [x0, z_face],
            along: [1.0, 0.0],
            out: [0.0, if south { 1.0 } else { -1.0 }],
            length: w,
        },
    ]
}

/// The height of storey `k`'s floor above the block: the lobby's, then one
/// storey each.
fn floor_height(k: u32) -> f64 {
    if k == 0 {
        KERB
    } else {
        KERB + LOBBY + f64::from(k - 1) * STOREY
    }
}

/// The city being laid: its builder, grid and seed.
struct City {
    builder: BuildingBuilder,
    seed: u64,
    blocks: [u32; 2],
}

impl City {
    fn new(id: &str, seed: u64, blocks: [u32; 2]) -> Result<Self, ValidationError> {
        let mut builder = storey_groups(palette(id));
        for part in super::parts()? {
            builder = builder.part(part);
        }
        Ok(Self {
            builder,
            seed,
            blocks,
        })
    }

    fn add(&mut self, instance: Instance) {
        let builder = std::mem::replace(&mut self.builder, Building::builder(""));
        self.builder = builder.instance(instance);
    }

    fn roll(&self, a: i64, b: i64, salt: u64) -> f64 {
        hash(self.seed, a, b, salt)
    }

    /// Carriageways on every grid line, intersections at every node, and a
    /// crossing on each approach to a node.
    fn streets(&mut self) {
        let [nx, nz] = self.blocks;
        for i in 0..=nx {
            for j in 0..=nz {
                let (x, z) = (f64::from(i) * PITCH, f64::from(j) * PITCH);
                self.add(
                    Instance::new(format!("node-{i}-{j}"), "city:intersection")
                        .placed(Pose::at([x, 0.0, z])),
                );
                // Along +Z from this node, and along +X.
                for (axis, count, limit) in [(0, j, nz), (1, i, nx)] {
                    if count >= limit {
                        continue;
                    }
                    let turn = if axis == 0 { 0.0 } else { FRAC_PI_2 };
                    for segment in 0..4 {
                        let along = ROAD / 2.0 + f64::from(segment) * SEGMENT;
                        let at = if axis == 0 {
                            [x, 0.0, z + along]
                        } else {
                            [x + along, 0.0, z]
                        };
                        self.add(
                            Instance::new(format!("road-{axis}-{i}-{j}-{segment}"), "city:road")
                                .placed(spot(at[0], 0.0, at[2], turn)),
                        );
                    }
                    let near = ROAD / 2.0;
                    let far = PITCH - ROAD / 2.0;
                    for (end, along, flip) in [("a", near, 0.0), ("b", far, PI)] {
                        let at = if axis == 0 {
                            [x, 0.0, z + along]
                        } else {
                            [x + along, 0.0, z]
                        };
                        self.add(
                            Instance::new(
                                format!("crossing-{axis}-{i}-{j}-{end}"),
                                "city:crosswalk",
                            )
                            .placed(spot(
                                at[0],
                                0.0,
                                at[2],
                                turn + flip,
                            )),
                        );
                    }
                }
            }
        }
    }

    /// A block's slab, its sodium lamps, bollards, vending machines, benches,
    /// stop and billboard, and the cars parked along its kerb and flying over
    /// its streets.
    #[allow(
        clippy::too_many_lines,
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "one block's furniture, side by side; a roll in 0..1 picks a colour"
    )]
    fn block_furniture(&mut self, i: u32, j: u32) {
        let (ii, jj) = (i64::from(i), i64::from(j));
        let bx = f64::from(i) * PITCH + ROAD / 2.0;
        let bz = f64::from(j) * PITCH + ROAD / 2.0;
        self.add(
            Instance::new(format!("block-{i}-{j}"), "city:pavement")
                .placed(Pose::at([bx, 0.0, bz])),
        );
        let tram = self.roll(ii, jj, 11) < 0.45;
        // Each side: a point on the kerb line at `along`, `inset` inside it,
        // and the outward direction.
        let sides: [([f64; 2], [f64; 2], [f64; 2]); 4] = [
            ([bx, bz], [1.0, 0.0], [0.0, -1.0]),
            ([bx, bz + BLOCK], [1.0, 0.0], [0.0, 1.0]),
            ([bx, bz], [0.0, 1.0], [-1.0, 0.0]),
            ([bx + BLOCK, bz], [0.0, 1.0], [1.0, 0.0]),
        ];
        for (side, (start, along, out)) in sides.into_iter().enumerate() {
            let point = |distance: f64, inset: f64| {
                [
                    start[0] + along[0] * distance - out[0] * inset,
                    start[1] + along[1] * distance - out[1] * inset,
                ]
            };
            let face = facing(out);
            for (index, distance) in [6.0, 18.0, 30.0, 42.0].into_iter().enumerate() {
                let [x, z] = point(distance, 1.2);
                self.add(
                    Instance::new(format!("lamp-{i}-{j}-{side}-{index}"), "city:lamp")
                        .placed(Pose::at([x, KERB, z]).rotated(face)),
                );
            }
            let salt = u64::try_from(side).unwrap_or(0);
            // Bollards at the corners, and a vending machine against a wall
            // now and then.
            for (index, distance) in [5.2, 42.8].into_iter().enumerate() {
                let [x, z] = point(distance, 0.8);
                self.add(
                    Instance::new(format!("bollard-{i}-{j}-{side}-{index}"), "city:bollard")
                        .placed(Pose::at([x, KERB, z])),
                );
            }
            if self.roll(ii, jj, 20 + salt) < 0.4 {
                let [x, z] = point(10.0 + 4.0 * self.roll(ii, jj, 24 + salt), 2.6);
                self.add(
                    Instance::new(
                        format!("vending-{i}-{j}-{side}"),
                        if self.roll(ii, jj, 32 + salt) < 0.5 {
                            "city:newsstand"
                        } else {
                            "city:vending"
                        },
                    )
                    .placed(Pose::at([x, KERB, z]).rotated(face))
                    .material("holo", one_of(&HOLO, self.roll(ii, jj, 28 + salt))),
                );
            }
            if side == 0 && tram {
                let [x, z] = point(24.0, 1.6);
                self.add(
                    Instance::new(format!("stop-{i}-{j}"), "city:tram-stop")
                        .placed(Pose::at([x, KERB, z]).rotated(face)),
                );
            } else if self.roll(ii, jj, 40 + salt) < 0.4 {
                let [x, z] = point(24.0, 1.0);
                self.add(
                    Instance::new(format!("bench-{i}-{j}-{side}"), "city:bench")
                        .placed(Pose::at([x, KERB, z]).rotated(face)),
                );
            }
            if self.roll(ii, jj, 48 + salt) < 0.65 {
                let [x, z] = point(36.0, 1.35);
                self.add(
                    Instance::new(format!("rack-{i}-{j}-{side}"), "city:cycle-rack")
                        .placed(Pose::at([x, KERB, z]).rotated(face)),
                );
            }
            // Drain frames sit in the carriageway against the kerb, away from crossings.
            let [x, z] = point(15.0, -0.55);
            self.add(
                Instance::new(format!("drain-{i}-{j}-{side}"), "city:steam-grate")
                    .placed(Pose::at([x, 0.0, z]).rotated(face)),
            );
            // Cars parked in the kerb lane of the carriageway beyond.
            for (index, distance) in [8.0, 20.0, 32.0, 44.0].into_iter().enumerate() {
                let roll = self.roll(ii, jj, 60 + salt * 8 + u64::try_from(index).unwrap_or(0));
                if roll < 0.35 {
                    let [x, z] = point(distance, -1.6);
                    let colour = CAR_COLOURS[(roll * 11.0) as usize % CAR_COLOURS.len()];
                    self.add(
                        Instance::new(format!("car-{i}-{j}-{side}-{index}"), "city:hover-car")
                            .placed(Pose::at([x, 0.0, z]).rotated(facing(along)))
                            .binding(
                                "hull",
                                Binding::new("library:hull-plating")
                                    .param("color", ParamValue::Color(colour)),
                            ),
                    );
                }
            }
        }
        if self.roll(ii, jj, 80) < 0.5 {
            self.add(
                Instance::new(format!("billboard-{i}-{j}"), "city:billboard")
                    .placed(Pose::at([bx + 3.5, KERB, bz + 3.5]).rotated(facing([-1.0, -1.0])))
                    .material("holo", one_of(&HOLO, self.roll(ii, jj, 81))),
            );
        }
        // Traffic in the air over the streets beside the block.
        for lane in 0..2 {
            let roll = self.roll(ii, jj, 90 + lane);
            if roll < 0.7 {
                let height = 18.0 + roll * 90.0;
                let (at, direction) = if lane == 0 {
                    (
                        [bx + 8.0 + roll * 30.0, height, bz - ROAD / 2.0 - 2.0],
                        [1.0, 0.0],
                    )
                } else {
                    (
                        [bx - ROAD / 2.0 + 2.0, height, bz + 8.0 + roll * 30.0],
                        [0.0, -1.0],
                    )
                };
                let colour = CAR_COLOURS[(roll * 7.0) as usize % CAR_COLOURS.len()];
                self.add(
                    Instance::new(format!("flyer-{i}-{j}-{lane}"), "city:hover-car")
                        .placed(Pose::at(at).rotated(facing([-direction[1], direction[0]])))
                        .binding(
                            "hull",
                            Binding::new("library:hull-plating")
                                .param("color", ParamValue::Color(colour)),
                        ),
                );
            }
        }
    }

    /// Plaza tiles at `centres` (from the lot corner), a finger proud of the
    /// block surface.
    fn plaza(&mut self, prefix: &str, lot: [f64; 2], centres: &[[f64; 2]]) {
        for (index, [x, z]) in centres.iter().enumerate() {
            self.add(
                Instance::new(format!("{prefix}/plaza-{index}"), "city:plaza").placed(Pose::at([
                    lot[0] + x,
                    KERB,
                    lot[1] + z,
                ])),
            );
        }
    }

    fn prop(&mut self, id: String, part: &str, lot: [f64; 2], at: [f64; 2], turn: f64) {
        self.add(Instance::new(id, part).placed(spot(lot[0] + at[0], KERB, lot[1] + at[1], turn)));
    }

    fn raise(&mut self, prefix: &str, kind: Kind, base: Pose, spec: Spec) {
        let builder = std::mem::replace(&mut self.builder, Building::builder(""));
        self.builder = tower(builder, prefix, kind, base, spec, self.seed);
    }

    /// The alleys of a quad lot: a gutter down each, and on every tower's
    /// alley walls pipes, air-conditioners, a fire escape, blade signs, and
    /// bins at their feet; cables strung across overhead.
    #[allow(
        clippy::too_many_lines,
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        clippy::cast_precision_loss,
        clippy::many_single_char_names,
        reason = "one alley's dressing, wall by wall; a roll in 0..1 picks a bay; i, j, k and x, z are grid and storey coordinates"
    )]
    fn alleys(&mut self, i: u32, j: u32, lot: [f64; 2], towers: &[Option<Spec>; 4]) {
        let prefix = format!("b{i}-{j}/alley");
        let (ii, jj) = (i64::from(i), i64::from(j));
        // The gutters: down the east-west alley's middle whole, and across
        // the north-south one either side of it.
        let middle_x = FOOT[0] + (LOT - 2.0 * FOOT[0]) / 2.0;
        let middle_z = FOOT[1] + (LOT - 2.0 * FOOT[1]) / 2.0;
        for step in 0..7 {
            let z = f64::from(step) * GUTTER;
            self.prop(
                format!("{prefix}/gutter-x-{step}"),
                "city:gutter",
                lot,
                [middle_x, z],
                0.0,
            );
        }
        for (index, x) in [0.0, 6.0, 12.0, LOT - 18.0, LOT - 12.0, LOT - 6.0]
            .into_iter()
            .enumerate()
        {
            self.prop(
                format!("{prefix}/gutter-z-{index}"),
                "city:gutter",
                lot,
                [x, middle_z],
                FRAC_PI_2,
            );
        }
        for (quad, spec) in towers.iter().enumerate() {
            let Some(Spec { storeys, style, .. }) = spec else {
                continue;
            };
            let base_storeys = tiers(Kind::Tower, *style, *storeys)[0].1 + 1;
            // How far out a lamp reaches for a fin, and how high a blade sign
            // hangs so both its brackets find a banded tower's spandrels.
            let fin = style.fin();
            let sign_lift = if *style == Style::Band { 0.8 } else { 0.4 };
            for (side, wall) in alley_walls(quad).into_iter().enumerate() {
                let id = format!("{prefix}/q{quad}-{side}");
                let salt = 1000 + 100 * quad as u64 + 10 * side as u64;
                let seed = self.seed ^ salt;
                let roll = |n: u64| hash(seed, ii, jj, n);
                let (centres, lines) = wall.bays();
                let reach = base_storeys.min(14);
                // Pipes up beside the first fin, on the first pier or at the
                // ribbons' end, from the ground to the top of the lowest tier
                // or the fourteenth storey.
                let first = lines.first().copied().unwrap_or(1.5);
                let pipe_at = match style {
                    Style::Frame => first + 0.25,
                    Style::Band => 1.1,
                    Style::Cage => first,
                };
                for k in 0..reach {
                    let part = if k == 0 {
                        "city:pipes-lobby"
                    } else {
                        "city:pipes"
                    };
                    let pose = wall.at(lot, pipe_at, floor_height(k), 0.0);
                    self.add(
                        Instance::new(format!("{id}/pipe-{k}"), part)
                            .placed(pose)
                            .group(storey_group(k)),
                    );
                }
                // A fire escape down one bay.
                let escape = (roll(1) * centres.len() as f64) as usize % centres.len();
                if roll(2) < 0.75 {
                    for k in 1..reach {
                        let pose = wall.at(lot, centres[escape], floor_height(k) + SLAB, 0.0);
                        self.add(
                            Instance::new(format!("{id}/escape-{k}"), "city:fire-escape")
                                .placed(pose)
                                .group(storey_group(k)),
                        );
                    }
                }
                // Air-conditioners under the windows of the other bays; a
                // cage's glass runs to the floor, so none on a cage.
                for (bay, along) in centres.iter().enumerate() {
                    if bay == escape || *style == Style::Cage {
                        continue;
                    }
                    for k in 1..reach {
                        if roll(10 + 20 * bay as u64 + u64::from(k)) < 0.3 {
                            let pose = wall.at(lot, *along, floor_height(k) + 0.02, 0.0);
                            self.add(
                                Instance::new(format!("{id}/ac-{bay}-{k}"), "city:ac-unit")
                                    .placed(pose)
                                    .group(storey_group(k)),
                            );
                        }
                    }
                }
                // Blade signs hung off the fins, a storey or two up.
                for (index, line) in lines.iter().enumerate() {
                    if roll(300 + index as u64) < 0.45 {
                        let k = 1 + (roll(310 + index as u64) * 2.0) as u32;
                        let pose = wall.at(lot, *line, floor_height(k) + sign_lift, 0.0);
                        self.add(
                            Instance::new(format!("{id}/blade-{index}"), "city:blade-sign")
                                .placed(pose)
                                .group(storey_group(k))
                                .material("neon", one_of(&NEON, roll(320 + index as u64))),
                        );
                    }
                }
                // Caged lamps on the fins over the alley floor.
                for (index, line) in lines.iter().enumerate() {
                    if index % 2 == 0 {
                        let pose = wall.at(lot, *line, KERB + 3.4, fin);
                        self.add(
                            Instance::new(format!("{id}/lamp-{index}"), "city:wall-lamp")
                                .placed(pose)
                                .group(storey_group(0)),
                        );
                    }
                }
                // Each wall has a service bay and a separate loading bay. Their
                // contents vary by seed, but never occupy the alley's centre lane.
                let service = centres[usize::from(roll(402) < 0.4)];
                let loading = *centres.last().unwrap_or(&service);
                self.add(
                    Instance::new(format!("{id}/shutter"), "city:service-shutter")
                        .placed(wall.at(lot, service, KERB, 0.02))
                        .group(storey_group(0)),
                );
                if roll(403) < 0.65 {
                    self.add(
                        Instance::new(format!("{id}/vent"), "city:wall-vent")
                            .placed(wall.at(lot, loading, KERB + 2.2, 0.02))
                            .group(storey_group(0)),
                    );
                }
                let kind = if roll(400) < 0.36 {
                    "city:dumpster"
                } else if roll(400) < 0.73 {
                    "city:pallet"
                } else {
                    "city:barrel"
                };
                self.add(
                    Instance::new(format!("{id}/loading"), kind)
                        .placed(wall.at(lot, loading, KERB, 0.78)),
                );
                let companion = if kind == "city:dumpster" {
                    "city:trash"
                } else if roll(404) < 0.5 {
                    "city:barrel"
                } else {
                    "city:pallet"
                };
                self.add(
                    Instance::new(format!("{id}/loading-side"), companion).placed(wall.at(
                        lot,
                        loading - 1.65,
                        KERB,
                        0.75,
                    )),
                );
                if roll(405) < 0.6 {
                    self.add(
                        Instance::new(format!("{id}/drain"), "city:steam-grate")
                            .placed(wall.at(lot, service, KERB, 1.35)),
                    );
                }
            }
        }
        // Cables strung across the alleys between facing towers.
        let seed = self.seed;
        for (pair, (west, east)) in [(0, 1), (2, 3)].into_iter().enumerate() {
            if let (Some(a), Some(b)) = (towers[west], towers[east]) {
                let (a, b) = (a.storeys, b.storeys);
                let tops = a.min(b).min(12);
                for index in 0..3_u32 {
                    let roll =
                        |n: u64| hash(seed, ii, jj, 2000 + 10 * pair as u64 + u64::from(index) + n);
                    let k = 1 + (roll(0) * f64::from(tops.saturating_sub(1).max(1))) as u32;
                    let z = if pair == 0 { 1.5 } else { LOT - FOOT[1] + 1.5 }
                        + roll(5) * (FOOT[1] - 3.0);
                    let y = floor_height(k) + 3.2;
                    self.add(
                        Instance::new(format!("{prefix}/cable-{pair}-{index}"), "city:cable-6")
                            .placed(Pose::at([lot[0] + FOOT[0], y, lot[1] + z]))
                            .group(storey_group(k))
                            .material("neon", one_of(&NEON, roll(7))),
                    );
                }
            }
        }
        for (pair, (south, north)) in [(0, 2), (1, 3)].into_iter().enumerate() {
            if let (Some(a), Some(b)) = (towers[south], towers[north]) {
                let (a, b) = (a.storeys, b.storeys);
                let tops = a.min(b).min(10);
                let roll = |n: u64| hash(seed, ii, jj, 2100 + 10 * pair as u64 + n);
                let k = 1 + (roll(0) * f64::from(tops.saturating_sub(1).max(1))) as u32;
                let x = if pair == 0 { 2.0 } else { LOT - FOOT[0] + 2.0 } + roll(5) * 14.0;
                self.add(
                    Instance::new(format!("{prefix}/cable-z-{pair}"), "city:cable-10")
                        .placed(
                            Pose::at([lot[0] + x, floor_height(k) + 3.2, lot[1] + FOOT[1]])
                                .rotated(super::yaw(-FRAC_PI_2)),
                        )
                        .group(storey_group(k))
                        .material("neon", one_of(&NEON, roll(7))),
                );
            }
        }
    }

    /// Fill block `(i, j)`'s lot with `lot`.
    #[allow(
        clippy::too_many_lines,
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "one arm per kind of lot; a roll in 0..1 picks a colour"
    )]
    fn lot(&mut self, i: u32, j: u32, lot: &Lot) {
        let corner = [
            f64::from(i) * PITCH + ROAD / 2.0 + SIDEWALK,
            f64::from(j) * PITCH + ROAD / 2.0 + SIDEWALK,
        ];
        let (ii, jj) = (i64::from(i), i64::from(j));
        let prefix = format!("b{i}-{j}");
        match lot {
            Lot::Quad(towers) => {
                for (index, tower_spec) in towers.iter().enumerate() {
                    let ([x, z], turn) = quadrant(index);
                    let id = format!("{prefix}/q{index}");
                    if let Some(spec) = tower_spec {
                        let base = spot(corner[0] + x, KERB, corner[1] + z, turn);
                        self.raise(&id, Kind::Tower, base, *spec);
                        // A holo sign up the front of one tower in two.
                        if self.roll(ii, jj, 100 + index as u64) < 0.5 {
                            self.add(
                                Instance::new(format!("{id}/sign"), "city:facade-sign")
                                    .placed(base.compose(Pose::at([
                                        -FOOT[0] / 2.0 + 2.6,
                                        LOBBY + 0.6,
                                        -FOOT[1] / 2.0 - 0.3,
                                    ])))
                                    .group(storey_group(1))
                                    .material("holo", one_of(&HOLO, self.roll(ii, jj, 110))),
                            );
                        }
                    } else {
                        self.plaza(&id, corner, &[[x, z]]);
                        self.prop(
                            format!("{id}/pillar"),
                            "city:holo-pillar",
                            corner,
                            [x, z],
                            0.0,
                        );
                        self.prop(
                            format!("{id}/bench"),
                            "city:bench",
                            corner,
                            [x, z - 3.5],
                            turn,
                        );
                    }
                }
                self.alleys(i, j, corner, towers);
            }
            Lot::Landmark(storeys, topper) => {
                let half = LOT / 2.0;
                let base = spot(corner[0] + half, KERB, corner[1] + half, 0.0);
                self.raise(
                    &format!("{prefix}/tower"),
                    Kind::Landmark,
                    base,
                    Spec::new(*storeys, *topper, Style::Frame, LANDMARK_CROWN),
                );
            }
            Lot::Tenements(storeys) => {
                let half = LOT / 2.0;
                let depth = TENEMENT.depth;
                for (index, (z, turn)) in [(depth / 2.0, 0.0), (LOT - depth / 2.0, PI)]
                    .into_iter()
                    .enumerate()
                {
                    let id = format!("{prefix}/tenement-{index}");
                    let base = spot(corner[0] + half, KERB, corner[1] + z, turn);
                    let spec = Spec {
                        storeys: storeys[index],
                        topper: Topper::Tank,
                        style: Style::Frame,
                        crown: None,
                    };
                    self.raise(&id, Kind::Tenement, base, spec);
                    // Fire escapes down the street front, every third bay.
                    for bay in [1.0, 4.0, 7.0] {
                        let along = -TENEMENT.width / 2.0 + 0.6 + (bay + 0.5) * (36.8 / 9.0);
                        for k in 1..storeys[index] {
                            self.add(
                                Instance::new(format!("{id}/escape-{bay}-{k}"), "city:fire-escape")
                                    .placed(base.compose(Pose::at([
                                        along,
                                        floor_height(k) - KERB + SLAB,
                                        -depth / 2.0,
                                    ])))
                                    .group(storey_group(k)),
                            );
                        }
                    }
                }
                // The yard between them: bins and a gutter.
                for step in 0..3 {
                    self.prop(
                        format!("{prefix}/yard-gutter-{step}"),
                        "city:gutter",
                        corner,
                        [f64::from(step) * GUTTER + half - 9.0, half],
                        FRAC_PI_2,
                    );
                }
                for (index, x) in [8.0, 18.0, 30.0].into_iter().enumerate() {
                    self.prop(
                        format!("{prefix}/dumpster-{index}"),
                        one_of(
                            &["city:dumpster", "city:pallet", "city:cycle-rack"],
                            self.roll(ii, jj, 610 + index as u64),
                        ),
                        corner,
                        [x, depth + 1.2],
                        PI,
                    );
                    self.prop(
                        format!("{prefix}/trash-{index}"),
                        if index == 1 {
                            "city:barrel"
                        } else {
                            "city:trash"
                        },
                        corner,
                        [x + 1.8, depth + 1.0],
                        0.0,
                    );
                }
                for (index, x) in [9.0, 30.0].into_iter().enumerate() {
                    self.prop(
                        format!("{prefix}/yard-rack-{index}"),
                        "city:cycle-rack",
                        corner,
                        [x, LOT - depth - 1.1],
                        0.0,
                    );
                }
                self.prop(
                    format!("{prefix}/yard-bench"),
                    "city:bench",
                    corner,
                    [half, LOT - depth - 1.1],
                    0.0,
                );
                self.prop(
                    format!("{prefix}/yard-news"),
                    "city:newsstand",
                    corner,
                    [4.0, half - 2.0],
                    -FRAC_PI_2,
                );
            }
            Lot::Market => {
                let half = LOT / 2.0;
                let mut tiles = Vec::new();
                for x in [-12.0, 0.0, 12.0] {
                    for z in [-12.0, 0.0, 12.0] {
                        tiles.push([half + x, half + z]);
                    }
                }
                self.plaza(&prefix, corner, &tiles);
                self.prop(
                    format!("{prefix}/pillar"),
                    "city:holo-pillar",
                    corner,
                    [half, half],
                    0.0,
                );
                // Two rows of stalls facing each other across a lane, either
                // side of the pillar.
                for (row, (z, turn)) in [(half - 7.0, PI), (half + 7.0, 0.0)]
                    .into_iter()
                    .enumerate()
                {
                    for column in 0..6_u32 {
                        let x = 5.0 + f64::from(column) * 6.4;
                        if (x - half).abs() < 4.0 {
                            continue;
                        }
                        let roll = self.roll(ii, jj, 500 + row as u64 * 10 + u64::from(column));
                        let colour = AWNINGS[(roll * 3.0) as usize % AWNINGS.len()];
                        self.add(
                            Instance::new(
                                format!("{prefix}/stall-{row}-{column}"),
                                if roll < 0.55 {
                                    "city:noodle-bar"
                                } else {
                                    "city:stall"
                                },
                            )
                            .placed(spot(corner[0] + x, KERB, corner[1] + z, turn))
                            .binding(
                                "awning",
                                Binding::new("library:signage-ink")
                                    .param("color", ParamValue::Color(colour)),
                            )
                            .material("neon", one_of(&NEON, roll))
                            .material("holo", one_of(&HOLO, 1.0 - roll)),
                        );
                        if roll > 0.45 {
                            let back = if row == 0 { -2.3 } else { 2.3 };
                            self.prop(
                                format!("{prefix}/delivery-{row}-{column}"),
                                "city:pallet",
                                corner,
                                [x + 0.4, z + back],
                                turn,
                            );
                        }
                    }
                }
                for (index, x, turn) in [(0, half - 4.0, -FRAC_PI_2), (1, half + 4.0, FRAC_PI_2)] {
                    self.prop(
                        format!("{prefix}/market-bench-{index}"),
                        "city:bench",
                        corner,
                        [x, half],
                        turn,
                    );
                    self.prop(
                        format!("{prefix}/market-rack-{index}"),
                        "city:cycle-rack",
                        corner,
                        [x, 3.5],
                        0.0,
                    );
                }
                for (index, [x, z]) in [[4.0, 4.0], [LOT - 4.0, 4.0], [4.0, LOT - 4.0]]
                    .into_iter()
                    .enumerate()
                {
                    self.prop(
                        format!("{prefix}/vending-{index}"),
                        if index == 1 {
                            "city:newsstand"
                        } else {
                            "city:vending"
                        },
                        corner,
                        [x, z],
                        0.0,
                    );
                }
            }
        }
    }

    /// Skybridges across the street between facing quadrant towers of two
    /// neighbouring blocks, at a storey of both lowest tiers.
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "a seeded roll in 0..1 chooses a storey within both lower tiers"
    )]
    fn skybridges(&mut self, lots: &[Lot]) {
        let [nx, nz] = self.blocks;
        let base = |lot: &Lot, index: usize| match lot {
            Lot::Quad(towers) => towers[index].and_then(|spec| {
                let lowest = tiers(Kind::Tower, spec.style, spec.storeys)[0].1;
                (lowest >= 6).then_some(lowest)
            }),
            _ => None,
        };
        let at = |i: u32, j: u32| &lots[(j * nx + i) as usize];
        for j in 0..nz {
            for i in 0..nx {
                let lot_x = f64::from(i) * PITCH + ROAD / 2.0 + SIDEWALK;
                let lot_z = f64::from(j) * PITCH + ROAD / 2.0 + SIDEWALK;
                // East across the street: this block's east quadrants to the
                // next block's west ones.
                if i + 1 < nx {
                    for (east, west) in [(1, 0), (3, 2)] {
                        if let (Some(a), Some(b)) = (base(at(i, j), east), base(at(i + 1, j), west))
                            && self.roll(i64::from(i), i64::from(j), 120 + east as u64) < 0.6
                        {
                            let ([_, z], _) = quadrant(east);
                            let top = a.min(b) - 1;
                            let level = 3
                                + (self.roll(i64::from(i), i64::from(j), 730 + u64::from(top))
                                    * f64::from(top - 2)) as u32;
                            let y = floor_height(level) + SLAB + LINER + 1.3;
                            self.add(
                                Instance::new(format!("bridge-x-{i}-{j}-{east}"), "city:skybridge")
                                    .placed(Pose::at([lot_x + LOT - 1.0, y, lot_z + z])),
                            );
                        }
                    }
                }
                // North across the street: this block's north quadrants to
                // the next block's south ones.
                if j + 1 < nz {
                    for (north, south) in [(2, 0), (3, 1)] {
                        if let (Some(a), Some(b)) =
                            (base(at(i, j), north), base(at(i, j + 1), south))
                            && self.roll(i64::from(i), i64::from(j), 130 + north as u64) < 0.6
                        {
                            let ([x, _], _) = quadrant(north);
                            let top = a.min(b) - 1;
                            let level = 3
                                + (self.roll(i64::from(i), i64::from(j), 730 + u64::from(top))
                                    * f64::from(top - 2)) as u32;
                            let y = floor_height(level) + SLAB + LINER + 1.3;
                            self.add(
                                Instance::new(
                                    format!("bridge-z-{i}-{j}-{north}"),
                                    "city:skybridge",
                                )
                                .placed(spot(
                                    lot_x + x,
                                    y,
                                    lot_z + LOT - 1.0,
                                    -FRAC_PI_2,
                                )),
                            );
                        }
                    }
                }
            }
        }
    }

    fn lay(mut self, lots: &[Lot]) -> Result<Building, ValidationError> {
        let [nx, nz] = self.blocks;
        self.streets();
        for j in 0..nz {
            for i in 0..nx {
                self.block_furniture(i, j);
                self.lot(i, j, &lots[(j * nx + i) as usize]);
            }
        }
        self.skybridges(lots);
        self.builder.build()
    }
}

/// How tall the city stands at a block: tall in the middle, falling off to
/// its edge.
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "a storey count is a small positive whole number; one pass over the grid"
)]
fn plan_lots(seed: u64, blocks: [u32; 2]) -> Vec<Lot> {
    let [nx, nz] = blocks;
    let centre = [f64::from(nx) / 2.0, f64::from(nz) / 2.0];
    let reach = (centre[0].powi(2) + centre[1].powi(2)).sqrt().max(0.5);
    let mut nearest = (f64::INFINITY, 0, 0);
    for j in 0..nz {
        for i in 0..nx {
            let d = ((f64::from(i) + 0.5 - centre[0]).powi(2)
                + (f64::from(j) + 0.5 - centre[1]).powi(2))
            .sqrt();
            if d < nearest.0 {
                nearest = (d, i, j);
            }
        }
    }
    let mut lots = Vec::new();
    for j in 0..nz {
        for i in 0..nx {
            let (ii, jj) = (i64::from(i), i64::from(j));
            let roll = |salt: u64| hash(seed, ii, jj, salt);
            let distance = ((f64::from(i) + 0.5 - centre[0]).powi(2)
                + (f64::from(j) + 0.5 - centre[1]).powi(2))
            .sqrt()
                / reach;
            let target = 8.0 + 52.0 * (1.0 - distance.min(1.0)).powf(1.5);
            let landmark = (i, j) == (nearest.1, nearest.2) || (distance < 0.3 && roll(1) < 0.2);
            let lot = if landmark {
                Lot::Landmark(
                    ((target * 1.9).round() as u32).clamp(60, MAX_STOREYS),
                    if roll(2) < 0.7 {
                        Topper::Spire
                    } else {
                        Topper::Mast
                    },
                )
            } else if roll(3) < 0.08 {
                Lot::Market
            } else if distance > 0.5 && roll(4) < 0.35 {
                Lot::Tenements([
                    (5.0 + roll(5) * 7.0).round() as u32,
                    (5.0 + roll(6) * 7.0).round() as u32,
                ])
            } else {
                let mut towers = [None; 4];
                let mut styles = [None; 4];
                for (index, slot) in towers.iter_mut().enumerate() {
                    let salt = 10 + 10 * index as u64;
                    if roll(salt) < 0.08 {
                        continue;
                    }
                    let storeys = (target * (0.55 + 0.9 * roll(salt + 1)))
                        .round()
                        .clamp(4.0, 90.0) as u32;
                    let top = roll(salt + 3);
                    // Weight the first choice towards frame towers, then
                    // avoid repeating either neighbour across an alley.
                    let look = roll(salt + 5);
                    let mut style = if look < 0.4 {
                        Style::Frame
                    } else if look < 0.7 {
                        Style::Band
                    } else {
                        Style::Cage
                    };
                    // Repeated neighbours obscure the silhouette differences in a
                    // dense lot. Keep the seed's first choice, then rotate a
                    // repeated style; the parts remain shared city-wide.
                    let west = (index % 2 == 1).then(|| styles[index - 1]).flatten();
                    let south = (index >= 2).then(|| styles[index - 2]).flatten();
                    while west == Some(style) || south == Some(style) {
                        style = match style {
                            Style::Frame => Style::Band,
                            Style::Band => Style::Cage,
                            Style::Cage => Style::Frame,
                        };
                    }
                    styles[index] = Some(style);
                    let crown = style.crowns()[usize::from(roll(salt + 6) < 0.5)];
                    let topper = if storeys > 40 {
                        if top < 0.35 {
                            Topper::Antennas
                        } else if top < 0.6 {
                            Topper::Mast
                        } else if top < 0.8 {
                            Topper::Helipad
                        } else {
                            Topper::Billboard
                        }
                    } else if top < 0.3 {
                        Topper::Plant
                    } else if top < 0.55 {
                        Topper::Billboard
                    } else if top < 0.75 {
                        Topper::Antennas
                    } else {
                        Topper::Dish
                    };
                    *slot = Some(Spec::new(storeys, topper, style, crown));
                }
                Lot::Quad(towers)
            };
            lots.push(lot);
        }
    }
    lots
}

/// A metropolis of `blocks` city blocks, laid out from `seed`.
///
/// Wet carriageways on a sixty-metre grid; kerbed blocks with sodium lamps,
/// bollards, vending machines, stops and parked cars; and on every lot four
/// towers with back alleys between them, a landmark, a pair of tenements or a
/// night market — tallest in the middle, over a hundred storeys at its
/// centre, falling off to the edge — with skybridges across the streets
/// between neighbours. Each quad tower draws its style and its crown from
/// the seed, rotating repeated styles across a lot's alleys so neighbours
/// differ. The same seed lays the same city.
///
/// # Errors
///
/// If the assembled recipe does not validate, which would be a bug here.
pub fn metropolis(seed: u64, blocks: [u32; 2]) -> Result<Building, ValidationError> {
    let lots = plan_lots(seed, blocks);
    City::new(&format!("city:metropolis-{seed}"), seed, blocks)?.lay(&lots)
}

/// Every kind of lot, every tower style at one, two and three tiers, every
/// crown and every topper, on five blocks: the catalogue.
///
/// The first lot stands the three styles side by side at full height, frame,
/// banded and caged, with a low frame tower; the second the same styles at
/// two tiers and one.
///
/// # Errors
///
/// If the assembled recipe does not validate.
pub fn kit_scene() -> Result<Building, ValidationError> {
    use Style::{Band, Cage, Frame};
    let tower = |storeys, topper, style, crown| Some(Spec::new(storeys, topper, style, crown));
    let lots = [
        Lot::Quad([
            tower(50, Topper::Antennas, Frame, Crown::Blades),
            tower(46, Topper::Mast, Band, Crown::Stepped),
            tower(9, Topper::Plant, Frame, Crown::Screen),
            tower(52, Topper::Dish, Cage, Crown::Blades),
        ]),
        Lot::Quad([
            tower(26, Topper::Billboard, Band, Crown::Screen),
            tower(24, Topper::Antennas, Cage, Crown::Screen),
            tower(14, Topper::Helipad, Frame, Crown::Screen),
            tower(10, Topper::Plant, Band, Crown::Stepped),
        ]),
        Lot::Landmark(72, Topper::Spire),
        Lot::Tenements([7, 9]),
        Lot::Market,
    ];
    City::new("city:kit", 1, [5, 1])?.lay(&lots)
}

/// One block of four towers, their alleys and the streets round it.
///
/// # Errors
///
/// If the assembled recipe does not validate.
pub fn block_scene() -> Result<Building, ValidationError> {
    let lots = [Lot::Quad([
        Some(Spec::new(34, Topper::Antennas, Style::Band, Crown::Stepped)),
        Some(Spec::new(
            18,
            Topper::Billboard,
            Style::Frame,
            Crown::Screen,
        )),
        Some(Spec::new(52, Topper::Mast, Style::Frame, Crown::Blades)),
        Some(Spec::new(12, Topper::Dish, Style::Cage, Crown::Screen)),
    ])];
    City::new("city:block", 3, [1, 1])?.lay(&lots)
}

/// A block of four low towers, so the camera that frames it stands at the
/// crossing of its alleys: what a back alley looks like close up.
///
/// # Errors
///
/// If the assembled recipe does not validate.
pub fn alley_scene() -> Result<Building, ValidationError> {
    let lots = [Lot::Quad([
        Some(Spec::new(8, Topper::Plant, Style::Frame, Crown::Screen)),
        Some(Spec::new(7, Topper::Antennas, Style::Band, Crown::Stepped)),
        Some(Spec::new(9, Topper::Tank, Style::Frame, Crown::Blades)),
        Some(Spec::new(6, Topper::Billboard, Style::Cage, Crown::Screen)),
    ])];
    City::new("city:alley", 9, [1, 1])?.lay(&lots)
}

/// One building of `kind` alone on a block, at a height that shows it.
///
/// # Errors
///
/// If the assembled recipe does not validate.
pub fn tower_scene(kind: Kind) -> Result<Building, ValidationError> {
    let spec = match kind {
        Kind::Tower => Spec::new(48, Topper::Antennas, Style::Frame, Crown::Blades),
        Kind::Landmark => Spec::new(110, Topper::Spire, Style::Frame, LANDMARK_CROWN),
        Kind::Tenement => Spec {
            storeys: 8,
            topper: Topper::Tank,
            style: Style::Frame,
            crown: None,
        },
    };
    let mut city = City::new(&format!("city:{kind:?}").to_lowercase(), 5, [1, 1])?;
    city.add(Instance::new("block", "city:pavement").placed(Pose::at([0.0, 0.0, 0.0])));
    let base = spot(BLOCK / 2.0, KERB, BLOCK / 2.0, 0.0);
    city.raise("tower", kind, base, spec);
    city.builder.build()
}

/// The names [`piece`] accepts: every part of the city kit.
#[must_use]
pub fn piece_names() -> Vec<String> {
    super::parts()
        .unwrap_or_default()
        .into_iter()
        .map(|part| part.id.trim_start_matches("city:").to_owned())
        .collect()
}

/// One part of the kit alone: a storey, roof or topper standing on a block,
/// or a street piece on its own.
///
/// # Errors
///
/// If `name` is not one of [`piece_names`], or the recipe does not validate.
pub fn piece(name: &str) -> Result<Building, ValidationError> {
    let id = format!("city:{name}");
    let part = super::parts()?
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
        .unwrap_or([glam::DVec3::splat(-1.0), glam::DVec3::splat(1.0)]);
    // A street piece is its own ground, and a prop under ten metres stands on
    // the preview's; a storey, a roof or a topper stands on a block.
    let street = matches!(
        name,
        "road" | "intersection" | "crosswalk" | "pavement" | "plaza"
    );
    let small = (high.x - low.x).max(high.z - low.z) < 10.0;
    let mut city = City::new(&format!("city:piece-{name}"), 1, [1, 1])?;
    let centre = (low + high) / 2.0;
    if street {
        city.add(Instance::new(name, id));
    } else if small {
        city.add(Instance::new(name, id).placed(Pose::at([-centre.x, -low.y.min(0.0), -centre.z])));
    } else {
        city.add(Instance::new("block", "city:pavement").placed(Pose::at([
            -BLOCK / 2.0,
            0.0,
            -BLOCK / 2.0,
        ])));
        city.add(Instance::new(name, id).placed(Pose::at([
            -centre.x,
            KERB - low.y.min(0.0),
            -centre.z,
        ])));
    }
    city.builder.build()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_hash_is_a_unit_float_and_deterministic() {
        for a in 0..50 {
            let value = hash(7, a, -a, 3);
            assert!((0.0..1.0).contains(&value));
            assert!((value - hash(7, a, -a, 3)).abs() < f64::EPSILON);
        }
        assert!((hash(7, 1, 2, 3) - hash(8, 1, 2, 3)).abs() > f64::EPSILON);
    }

    #[test]
    fn a_tower_steps_in_as_it_rises_and_counts_its_storeys() {
        let cases = Style::ALL.into_iter().flat_map(|style| {
            [
                (Kind::Tower, style, 10, 1),
                (Kind::Tower, style, 30, 2),
                (Kind::Tower, style, 90, 3),
            ]
        });
        for (kind, style, storeys, tiers_expected) in cases.chain([
            (Kind::Landmark, Style::Frame, 110, 3),
            (Kind::Tenement, Style::Frame, 8, 1),
        ]) {
            let stack = levels(kind, style, storeys);
            let counted = stack.iter().filter(|level| level.storey).count();
            assert_eq!(counted, storeys as usize, "{kind:?} {storeys}");
            let roofs = stack.iter().filter(|level| !level.storey).count();
            assert_eq!(roofs, tiers_expected, "{kind:?} {storeys}");
            // Each tier is no wider nor deeper than the one below it.
            let widths: Vec<f64> = stack.iter().map(|level| level.plan.width).collect();
            let depths: Vec<f64> = stack.iter().map(|level| level.plan.depth).collect();
            assert!(widths.windows(2).all(|pair| pair[1] <= pair[0]));
            assert!(depths.windows(2).all(|pair| pair[1] <= pair[0]));
        }
    }

    #[test]
    fn every_setback_leaves_a_terrace_its_planters_fit() {
        // A roof's planters stand from 0.25 m to 0.85 m in from its edge, so
        // the tier above has to stand a metre in on every side.
        for plans in Style::ALL.map(Style::plans).iter().copied().chain([&SPIRE]) {
            for pair in plans.windows(2) {
                let (below, above) = (&pair[0], &pair[1]);
                assert!(
                    below.width - above.width >= 2.0 && below.depth - above.depth >= 2.0,
                    "{} on {}",
                    above.name,
                    below.name
                );
            }
        }
    }

    #[test]
    fn the_alley_walls_are_where_the_towers_are() {
        for index in 0..4 {
            let ([x, z], _) = quadrant(index);
            for wall in alley_walls(index) {
                // A wall's face is one of the footprint's edges.
                let [sx, sz] = wall.start;
                let on_x = (sx - (x - FOOT[0] / 2.0)).abs() < 1e-9
                    || (sx - (x + FOOT[0] / 2.0)).abs() < 1e-9;
                let on_z = (sz - (z - FOOT[1] / 2.0)).abs() < 1e-9
                    || (sz - (z + FOOT[1] / 2.0)).abs() < 1e-9;
                assert!(on_x && on_z, "quadrant {index}");
            }
        }
    }
}
