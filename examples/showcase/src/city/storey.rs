//! Storeys of the dark city's towers, and the roofs, crowns and toppers that
//! finish them.
//!
//! Every tower speaks the corporate kit's language in darker concrete, in one
//! of three [`Style`]s, and each style steps in over three tiers of its own
//! plans:
//!
//! - **Frame**, the kit's own: one deep window per bay with splayed jambs, a
//!   tapered fin at every bay line, a steel sill under each window, a strip
//!   light in its head and a slab edge at every floor. Higher up the windows
//!   grow, a blade splits each bay and the top tier's fins deepen.
//! - **Band**, brutalist: heavy precast spandrels standing out of the wall
//!   with a joint at every floor, and deep ribbon windows between them that
//!   break into pairs of bays and then single bays as the tower rises; the
//!   spandrels stand further out above its foot. No fins.
//! - **Cage**, glass-heavy: floor-to-ceiling glass set deep behind a concrete
//!   frame of a pier at every bay line and a beam at every floor, on slimmer
//!   tiers, with blades splitting the bays higher up.
//!
//! Every style's corners are cut at forty-five degrees and carry a light line
//! in a channel down their middle. A lobby stands on a plinth under a podium
//! cornice, with a recessed shopfront between piers, a fascia, and a deep
//! portal under a canopy. A tier's roof is a terrace: a cornice where the tier
//! ends, a coped parapet with a lit edge under its coping, and planters along
//! it. A tower's top wears a [`Crown`] that its topper stands on.
//!
//! A storey is a [`Plan`]: a chamfered rectangle and a lift core, the style it
//! is dressed in and the tier it is. Its origin is the footprint's minimum
//! corner and its front faces -Z. Its wall is four straight runs and four
//! corners, each its own element and its own convex proxy, and it carries its
//! interior with it: liners, a floor, a lift core and an office's desks, added
//! and never carved (ADR 0005).
//!
//! The windows are sealed, so they are not portals: a tower storey's room has
//! none, and a lobby's has its entrance. The glass is split over three slots,
//! `pane-a` to `pane-c`, bay by bay, so a tower can light each storey's
//! offices a third at a time with an instance override and a thousand storeys
//! of one part do not all glow alike.
use super::{
    Axis, Collision, DQuat, Element, FRAC_PI_2, Geometry, LINER, MARGIN, PI, Part, Pose, ROUND,
    SLAB, SMALL, SQRT_2, STOREY, UvMode, ValidationError, along_x, arc, at, beacon, plate,
    stacking, turned, yaw,
};

/// A lobby's windows' sill over its finished floor: clear of its plinth.
const SILL: f64 = 0.35;
/// Height of a lobby's plinth.
const PLINTH: f64 = 0.55;

/// The three slots a storey's glass is split over.
pub(super) const PANES: [&str; 3] = ["pane-a", "pane-b", "pane-c"];

/// How a tower is dressed: its facade, its tiers' plans and its crowns.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Style {
    /// Punched windows between tapered fins, a slab edge at every floor.
    Frame,
    /// Brutalist ribbons between heavy precast spandrels.
    Band,
    /// Floor-to-ceiling glass behind a deep concrete frame.
    Cage,
}

impl Style {
    /// Every style, the kit's own first.
    pub(super) const ALL: [Self; 3] = [Self::Frame, Self::Band, Self::Cage];

    /// Its tiers' plans, widest first. Every style stands on the same
    /// eighteen-by-sixteen foot with the same bays, so a quad lot's alleys fit
    /// any of them.
    pub(super) fn plans(self) -> &'static [Plan; 3] {
        match self {
            Self::Frame => &TOWER,
            Self::Band => &BANDED,
            Self::Cage => &CAGE,
        }
    }

    /// The two crowns it may wear.
    pub(super) fn crowns(self) -> [Crown; 2] {
        match self {
            Self::Frame | Self::Cage => [Crown::Blades, Crown::Screen],
            Self::Band => [Crown::Stepped, Crown::Screen],
        }
    }

    /// How far its lowest tier's fins stand out of the wall at a bay line:
    /// where a lamp hung off a fin in an alley has to reach.
    pub(super) fn fin(self) -> f64 {
        Facade::of(self, 0).fin
    }
}

/// What stands under an opening.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Under {
    /// Nothing: the glass runs down to the frame.
    Nothing,
    /// A steel sill.
    Steel,
}

/// How one tier of a style is glazed and dressed. Heights are over the
/// storey's base.
#[derive(Clone, Copy, Debug)]
struct Facade {
    /// Thickness of the wall: deep enough that a window reads as a hole.
    wall: f64,
    /// How deep in the wall the glass stands.
    glass: f64,
    /// An opening's bottom and top.
    sill: f64,
    head: f64,
    /// Wall left between two openings, half of it at each end of a run.
    pier: f64,
    /// Bays per opening; zero for one ribbon the length of the run.
    group: usize,
    /// Lights across a bay, split by mullions.
    lights: u32,
    /// How much wider an opening is at the wall's face than at its glass,
    /// each side: a splayed jamb catches the light.
    splay: f64,
    /// How far a fin at the line between two openings stands out; zero for
    /// none.
    fin: f64,
    /// How far a blade down the middle of each bay stands out; zero for none.
    blade: f64,
    /// A slab edge at the floor line: its height and how far it stands out.
    band: [f64; 2],
    /// How far precast spandrels stand out of the wall over and under the
    /// openings, parted by a joint at the floor line; zero for none.
    spandrel: f64,
    /// What stands under an opening.
    under: Under,
    /// A transom this far over the sill; zero for none.
    transom: f64,
}

impl Facade {
    /// Style `style`'s tier `tier`. Each style's lowest tier is its heaviest,
    /// and its rhythm tightens as it rises: the frame's windows grow and a
    /// blade splits each bay, the bands' ribbons break into pairs and then
    /// single deep windows, the cage gains a blade in every bay.
    fn of(style: Style, tier: usize) -> Self {
        let frame = Self {
            wall: 0.45,
            glass: 0.27,
            sill: 0.67,
            head: 3.37,
            pier: 1.0,
            group: 1,
            lights: 2,
            splay: 0.1,
            fin: 0.4,
            blade: 0.0,
            band: [0.34, 0.06],
            spandrel: 0.0,
            under: Under::Steel,
            transom: 0.0,
        };
        let band = Self {
            wall: 0.6,
            glass: 0.45,
            sill: 1.17,
            head: 3.3,
            pier: 0.8,
            group: 0,
            lights: 3,
            splay: 0.0,
            fin: 0.0,
            blade: 0.0,
            band: [0.0, 0.0],
            spandrel: 0.06,
            under: Under::Nothing,
            transom: 0.0,
        };
        let cage = Self {
            wall: 0.7,
            glass: 0.5,
            sill: 0.4,
            head: 3.35,
            pier: 0.6,
            group: 1,
            lights: 2,
            splay: 0.0,
            fin: 0.0,
            blade: 0.0,
            band: [0.0, 0.0],
            spandrel: 0.0,
            under: Under::Nothing,
            transom: 0.75,
        };
        match (style, tier) {
            (Style::Frame, 0) => frame,
            (Style::Frame, 1) => Self {
                sill: 0.57,
                head: 3.42,
                lights: 1,
                blade: 0.25,
                ..frame
            },
            (Style::Frame, _) => Self {
                sill: 0.47,
                head: 3.52,
                lights: 1,
                blade: 0.25,
                fin: 0.6,
                ..frame
            },
            (Style::Band, 0) => band,
            (Style::Band, 1) => Self {
                group: 2,
                pier: 0.9,
                spandrel: 0.22,
                ..band
            },
            (Style::Band, _) => Self {
                group: 1,
                pier: 1.2,
                lights: 2,
                splay: 0.12,
                spandrel: 0.22,
                ..band
            },
            (Style::Cage, 0) => cage,
            (Style::Cage, 1) => Self {
                lights: 1,
                blade: 0.12,
                ..cage
            },
            (Style::Cage, _) => Self {
                sill: 0.35,
                head: 3.5,
                pier: 0.5,
                lights: 1,
                blade: 0.12,
                transom: 0.0,
                ..cage
            },
        }
    }

    /// The same facade on a lobby `height` tall: one tall window to a bay
    /// over the plinth, and no floor line of its own, which the plinth is.
    fn lobby(self, height: f64) -> Self {
        Self {
            sill: SLAB + LINER + SILL,
            head: height - 0.9,
            group: 1,
            pier: self.pier.max(0.8),
            blade: 0.0,
            band: [0.0, 0.0],
            spandrel: 0.0,
            transom: 0.0,
            ..self
        }
    }

    /// The openings of a run whose bays are `pitch` apart and centred at
    /// `centres`: where each starts and ends along the run, and its bays.
    /// Whole groups come from both ends and what is left over is one opening
    /// in the middle, so a run stays symmetric about its middle.
    fn openings(&self, pitch: f64, centres: &[f64]) -> Vec<(f64, f64, std::ops::Range<usize>)> {
        let count = centres.len();
        let group = if self.group == 0 { count } else { self.group };
        let mut ends = Vec::new();
        let mut left = count;
        while left >= 2 * group {
            ends.push(group);
            left -= 2 * group;
        }
        let mut sizes = ends.clone();
        if left > 0 {
            sizes.push(left);
        }
        sizes.extend(ends.iter().rev());
        let mut first = 0;
        sizes
            .into_iter()
            .map(|size| {
                let bays = first..first + size;
                first += size;
                (
                    centres[bays.start] - pitch / 2.0 + self.pier / 2.0,
                    centres[bays.end - 1] + pitch / 2.0 - self.pier / 2.0,
                    bays,
                )
            })
            .collect()
    }

    /// The cutter through the wall for an opening from `x0` to `x1`, in the
    /// front run's frame: square, or splayed from the glass out to the face.
    fn cutter(&self, x0: f64, x1: f64) -> Geometry {
        let (t, g) = (self.wall, self.glass);
        let tall = self.head - self.sill;
        if self.splay > 0.0 {
            // Past the face by the margin, on the splay's own line.
            let s = self.splay * (g + MARGIN) / g;
            Geometry::extrude(
                [
                    [x0 - s, -MARGIN],
                    [x1 + s, -MARGIN],
                    [x1, g],
                    [x1, t + MARGIN],
                    [x0, t + MARGIN],
                    [x0, g],
                ],
                tall,
            )
            .placed(Pose::at([0.0, self.sill, 0.0]))
        } else {
            Geometry::cuboid([x1 - x0, tall, t + 2.0 * MARGIN])
                .placed(Pose::at([x0, self.sill, -MARGIN]))
        }
    }
}

/// A chamfered corner's light line, in the corner face's frame (along it,
/// out of it): in the channel's back and three centimetres proud of the face,
/// so it still shows where a distant level has filled the channel.
const CORNER_LINE: [[f64; 2]; 4] = [[-0.08, -0.12], [0.08, -0.12], [0.08, 0.03], [-0.08, 0.03]];
/// The channel cut down a chamfered corner's middle for its light line.
const CORNER_CHANNEL: [[f64; 2]; 4] = [
    [-0.15, -0.12],
    [0.15, -0.12],
    [0.15, MARGIN],
    [-0.15, MARGIN],
];

/// A storey's footprint and core, the style it is dressed in and its tier.
#[derive(Clone, Copy, Debug)]
pub(super) struct Plan {
    /// What its parts are called: `city:{name}-floor` and so on.
    pub name: &'static str,
    pub width: f64,
    pub depth: f64,
    /// How far along each edge a corner is cut at forty-five degrees.
    pub chamfer: f64,
    /// The lift core's width and depth, against the back wall.
    pub core: [f64; 2],
    /// How its facade is dressed.
    pub style: Style,
    /// Which of its style's tiers it is, from the ground.
    pub tier: usize,
}

impl Plan {
    fn facade(&self) -> Facade {
        Facade::of(self.style, self.tier)
    }

    fn thickness(&self) -> f64 {
        self.facade().wall
    }

    /// This footprint `by` in from every edge, with the same chamfer: what a
    /// penthouse or a crown's step stands on. Its origin is `[by, 0, by]` in
    /// this one's frame.
    fn inset(&self, by: f64) -> Self {
        Self {
            width: self.width - 2.0 * by,
            depth: self.depth - 2.0 * by,
            ..*self
        }
    }

    /// Where a straight run starts along its edge, for a band `offset` in
    /// from the outer face.
    fn run_start(&self, offset: f64) -> f64 {
        self.chamfer + offset * (SQRT_2 - 1.0)
    }

    /// The footprint `offset` in from its outer face, as an X/Z polygon.
    pub(super) fn outline(&self, offset: f64) -> Vec<[f64; 2]> {
        let (w, d) = (self.width, self.depth);
        let a = self.run_start(offset);
        vec![
            [offset, a],
            [a, offset],
            [w - a, offset],
            [w - offset, a],
            [w - offset, d - a],
            [w - a, d - offset],
            [a, d - offset],
            [offset, d - a],
        ]
    }

    /// The four straight runs of a band from `inner` to `outer` in from the
    /// outer face (negative is outside it), `height` tall from `base`: front,
    /// right, back, left.
    fn runs(&self, inner: f64, outer: f64, base: f64, height: f64) -> [Geometry; 4] {
        let (w, d) = (self.width, self.depth);
        let start = self.run_start(inner);
        let thick = outer - inner;
        [
            Geometry::cuboid([w - 2.0 * start, height, thick])
                .placed(Pose::at([start, base, inner])),
            Geometry::cuboid([thick, height, d - 2.0 * start]).placed(Pose::at([
                w - outer,
                base,
                start,
            ])),
            Geometry::cuboid([w - 2.0 * start, height, thick]).placed(Pose::at([
                start,
                base,
                d - outer,
            ])),
            Geometry::cuboid([thick, height, d - 2.0 * start])
                .placed(Pose::at([inner, base, start])),
        ]
    }

    /// The four corners of the same band: front-left, front-right, back-right,
    /// back-left.
    fn corners(&self, inner: f64, outer: f64, base: f64, height: f64) -> [Geometry; 4] {
        let (near, far) = (self.run_start(inner), self.run_start(outer));
        self.mirrored(
            [[inner, near], [near, inner], [far, outer], [outer, far]],
            base,
            height,
        )
    }

    /// The whole band round the footprint from `inner` to `outer` in from its
    /// outer face, `height` tall from `base`: a cornice, a coping, a lit edge.
    /// One outline less the other, so it is one closed solid with no seam
    /// where a run meets a corner, which a union of runs and corners would
    /// leave slivers along.
    ///
    /// So far in that the chamfer would turn inside out, the hole is this
    /// footprint inset with its own chamfer instead: the ring is then wider
    /// across its corners than along its runs.
    fn ring(&self, inner: f64, outer: f64, base: f64, height: f64) -> Geometry {
        let hole = if outer < 0.9 * self.chamfer / (2.0 - SQRT_2) {
            self.outline(outer)
        } else {
            self.inset(outer)
                .outline(0.0)
                .into_iter()
                .map(|[x, z]| [x + outer, z + outer])
                .collect()
        };
        Geometry::extrude(self.outline(inner), height)
            .subtract(
                Geometry::extrude(hole, height + 2.0 * MARGIN)
                    .placed(Pose::at([0.0, -MARGIN, 0.0])),
            )
            .placed(Pose::at([0.0, base, 0.0]))
    }

    /// A quad at the front-left corner, extruded `height` from `base`, and its
    /// mirror images at the other three: front-left, front-right, back-right,
    /// back-left.
    fn mirrored(&self, quad: [[f64; 2]; 4], base: f64, height: f64) -> [Geometry; 4] {
        let (w, d) = (self.width, self.depth);
        let mirror = |flip_x: bool, flip_z: bool| {
            let points = quad.map(|[x, z]| {
                [
                    if flip_x { w - x } else { x },
                    if flip_z { d - z } else { z },
                ]
            });
            Geometry::extrude(points, height).placed(Pose::at([0.0, base, 0.0]))
        };
        [
            mirror(false, false),
            mirror(true, false),
            mirror(true, true),
            mirror(false, true),
        ]
    }

    /// A prism on each chamfered corner's face: `shape` is a quad in the
    /// face's own frame, along it and out of it, centred on the face's middle.
    fn on_corners(&self, shape: [[f64; 2]; 4], base: f64, height: f64) -> [Geometry; 4] {
        // The front-left corner's face runs from (0, c) to (c, 0); its middle
        // is (c/2, c/2), along it is (1, -1) and out of it (-1, -1).
        let m = self.chamfer / 2.0;
        let quad =
            shape.map(|[along, out]| [m + (along - out) / SQRT_2, m + (-along - out) / SQRT_2]);
        self.mirrored(quad, base, height)
    }

    /// The bay pitch of straight run `side` and where its bays' centres are,
    /// measured along the run from its start.
    fn bays(&self, side: usize) -> (f64, Vec<f64>) {
        let s = self.run_start(0.0);
        let length = if side.is_multiple_of(2) {
            self.width - 2.0 * s
        } else {
            self.depth - 2.0 * s
        };
        let count = (length / super::BAY).round().max(1.0);
        let pitch = length / count;
        #[allow(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "a bay count is a small positive whole number"
        )]
        let centres = (0..count as u32)
            .map(|index| s + pitch * (f64::from(index) + 0.5))
            .collect();
        (pitch, centres)
    }

    /// The lines between run `side`'s bays, along the run from its start.
    fn lines(&self, side: usize) -> Vec<f64> {
        let (_, centres) = self.bays(side);
        centres
            .windows(2)
            .map(|pair| f64::midpoint(pair[0], pair[1]))
            .collect()
    }

    /// Where the front run's frame lands on run `side`: X along the run, Y up,
    /// -Z out of the wall. Every run's bays are symmetric about its middle, so
    /// a distance along the front run names the same bay on any side.
    fn side(&self, side: usize) -> Pose {
        let (w, d) = (self.width, self.depth);
        match side {
            0 => Pose::default(),
            1 => Pose::at([w, 0.0, 0.0]).rotated(yaw(-FRAC_PI_2)),
            2 => Pose::at([w, 0.0, d]).rotated(yaw(PI)),
            _ => Pose::at([0.0, 0.0, d]).rotated(yaw(FRAC_PI_2)),
        }
    }

    /// The local box of the storey's open-plan room: inside the liners, from
    /// the front to the lift core.
    pub(super) fn room(&self, height: f64) -> ([f64; 3], [f64; 3]) {
        let inset = self.thickness() + LINER;
        let front_of_core = self.depth - inset - self.core[1];
        (
            [inset, SLAB + LINER, inset],
            [
                self.width - 2.0 * inset,
                height - SLAB - 2.0 * LINER,
                front_of_core - inset,
            ],
        )
    }
}

/// The solids of one element fused into one, or the one, or none.
fn fuse(solids: Vec<Geometry>) -> Option<Geometry> {
    match solids.len() {
        0 => None,
        1 => solids.into_iter().next(),
        _ => Some(Geometry::union_all(solids)),
    }
}

/// A fin at `line` along a run, standing `out` from the wall's face and
/// `height` tall: a root let into the wall that tapers to a narrower blade,
/// so it reads as cast rather than as a board.
fn fin(line: f64, out: f64, height: f64) -> Geometry {
    Geometry::extrude(
        [
            [line - 0.14, 0.05],
            [line + 0.14, 0.05],
            [line + 0.07, -out],
            [line - 0.07, -out],
        ],
        height,
    )
}

/// A flat-faced trim bar with its back tapered into the reveal. The exposed
/// rectangle is unchanged; the hidden return needs only a triangular section.
fn reveal_bar(width: f64, height: f64, depth: f64) -> Geometry {
    if height > width {
        Geometry::extrude([[0.0, 0.0], [width, 0.0], [width / 2.0, depth]], height)
    } else {
        Geometry::hull([
            [0.0, 0.0, 0.0],
            [width, 0.0, 0.0],
            [0.0, height, 0.0],
            [width, height, 0.0],
            [0.0, height / 2.0, depth],
            [width, height / 2.0, depth],
        ])
    }
}

/// Jambs, rails and mullions retain their full front faces. A millimetre
/// joint separates each member, avoiding tessellation at their crossings
/// and letting thin trim disappear independently at coarse levels.
fn frame_bars(
    x0: f64,
    x1: f64,
    bottom: f64,
    top: f64,
    mullions: &[f64],
    transom: f64,
) -> Vec<Geometry> {
    const GAP: f64 = 0.001;
    let (rail, mullion) = (0.06, 0.08);
    let bar = |[left, right]: [f64; 2], [low, high]: [f64; 2]| {
        reveal_bar(right - left, high - low, 0.1).placed(Pose::at([left, low, 0.0]))
    };
    let inner = [x0 + rail + GAP, x1 - rail - GAP];
    let mut bars = vec![
        bar([x0, x0 + rail], [bottom, top]),
        bar([x1 - rail, x1], [bottom, top]),
        bar(inner, [top - rail, top]),
        bar(inner, [bottom, bottom + rail]),
    ];
    let between = [bottom + rail + GAP, top - rail - GAP];
    let mut edges = vec![x0 + rail];
    for x in mullions {
        let (left, right) = (x - mullion / 2.0, x + mullion / 2.0);
        bars.push(bar([left, right], between));
        edges.extend([left, right]);
    }
    edges.push(x1 - rail);
    edges.sort_by(f64::total_cmp);
    if transom > 0.0 {
        let y = bottom + transom;
        for pair in edges.chunks(2) {
            bars.push(bar([pair[0] + GAP, pair[1] - GAP], [y, y + mullion]));
        }
    }
    bars
}

/// Clear width and height of a lobby's entrance.
const DOOR: [f64; 2] = [3.2, 3.0];

/// The portal ids a storey publishes, in the order its room lists them.
pub(super) fn portals(lobby: bool) -> Vec<String> {
    if lobby {
        vec!["entrance".to_owned()]
    } else {
        Vec::new()
    }
}

/// One storey of a tower: its style's facade for its tier, liners, slab,
/// finishes and core.
///
/// `lobby` replaces the front run with a shopfront and an entrance, stands the
/// storey on a plinth under a podium cornice, and furnishes the room with a
/// desk and planters; any other storey is an office.
#[allow(
    clippy::too_many_lines,
    reason = "a storey is one builder chain, read top to bottom"
)]
pub(super) fn rect_storey(
    id: &str,
    plan: &Plan,
    height: f64,
    lobby: bool,
) -> Result<Part, ValidationError> {
    let (w, d) = (plan.width, plan.depth);
    let sky = !lobby && height > STOREY;
    let look = if sky {
        Facade {
            sill: 0.8,
            head: height - 1.4,
            group: 0,
            lights: 1,
            band: [0.65, 0.5],
            wall: plan.thickness(),
            glass: plan.facade().glass,
            ..Facade::of(Style::Cage, 0)
        }
    } else if lobby {
        plan.facade().lobby(height)
    } else {
        plan.facade()
    };
    let t = look.wall;
    let floor_top = SLAB + LINER;
    let (bottom, top) = (look.sill, look.head);
    let tall = top - bottom;
    let mut part = Part::builder(id);
    let mut panes: [Vec<Geometry>; 3] = [Vec::new(), Vec::new(), Vec::new()];
    // Steel: sills, mullions and transoms.
    let mut metal = Vec::new();
    let mut strips = Vec::new();
    // Concrete standing out of the face: fins and blades, and slab edges and
    // ledges.
    let mut fins = Vec::new();
    let mut bands = Vec::new();
    let walls = plan.runs(0.0, t, 0.0, height);
    let liners = plan.runs(t, t + LINER, floor_top, height - LINER - floor_top);
    let edges = (look.band[0] > 0.0).then(|| plan.runs(-look.band[1], 0.0, 0.0, look.band[0]));
    for (side, (wall, liner)) in walls.into_iter().zip(liners).enumerate() {
        // The lobby's front is its shopfront and entrance.
        if lobby && side == 0 {
            continue;
        }
        let (pitch, centres) = plan.bays(side);
        let frame = plan.side(side);
        let local = |geometry: Geometry| at(geometry, frame);
        let start = centres[0] - pitch / 2.0;
        let end = centres[centres.len() - 1] + pitch / 2.0;
        let mut wall = wall;
        let mut liner = liner;
        if let Some(edges) = &edges {
            bands.push(edges[side].clone());
        }
        // Precast spandrels the length of the run over and under the
        // openings, three centimetres short of the floor line, so the joint
        // between this storey's and the next's is a shadow line.
        if look.spandrel > 0.0 {
            let run =
                |from: f64, to: f64| {
                    local(
                        Geometry::cuboid([end - start, to - from, look.spandrel])
                            .placed(Pose::at([start, from, -look.spandrel])),
                    )
                };
            bands.push(run(0.03, bottom - 0.02));
            bands.push(run(top + 0.02, height - 0.03));
        }
        let openings = look.openings(pitch, &centres);
        // Fins stand at the lines between openings; a run's own ends meet its
        // corners, which carry none.
        if look.fin > 0.0 {
            for pair in openings.windows(2) {
                let line = f64::midpoint(pair[0].1, pair[1].0);
                fins.push(local(fin(line, look.fin, height - 0.02)));
            }
        }
        for (x0, x1, bays) in openings {
            let width = x1 - x0;
            wall = wall.subtract(local(look.cutter(x0, x1)));
            liner = liner.subtract(local(
                Geometry::cuboid([width, tall, t + LINER + 2.0 * MARGIN])
                    .placed(Pose::at([x0, bottom, -MARGIN])),
            ));
            let mut mullions = Vec::new();
            for bay in bays.clone() {
                let from = (centres[bay] - pitch / 2.0).max(x0);
                let to = (centres[bay] + pitch / 2.0).min(x1);
                // Deep in the reveal, so the wall's thickness reads round it.
                panes[(side * 2 + bay) % 3].push(local(
                    Geometry::cuboid([to - from, tall, 0.04])
                        .placed(Pose::at([from, bottom, look.glass])),
                ));
                // Mullions between a bay's lights and at the bay lines inside
                // a grouped opening.
                mullions.extend(
                    (1..look.lights)
                        .map(|k| from + (to - from) * f64::from(k) / f64::from(look.lights)),
                );
                if bay > bays.start {
                    mullions.push(from);
                }
                // A blade down the bay's middle, the storey's full height,
                // from out of the face back into the glass.
                if look.blade > 0.0 {
                    fins.push(local(
                        Geometry::cuboid([0.12, height - 0.02, look.blade + look.glass + 0.02])
                            .placed(Pose::at([centres[bay] - 0.06, 0.0, -look.blade])),
                    ));
                }
            }
            // Under the opening, a centimetre or two below it so its top is
            // not the reveal's floor on the same plane; as wide as the opening
            // is at the face.
            let face = look.splay;
            match look.under {
                Under::Steel => metal.push(local(
                    Geometry::cuboid([width + 2.0 * face + 0.2, 0.08, 0.2]).placed(Pose::at([
                        x0 - face - 0.1,
                        bottom - 0.09,
                        -0.2,
                    ])),
                )),
                Under::Nothing => {}
            }
            metal.extend(
                frame_bars(x0, x1, bottom, top, &mullions, look.transom)
                    .into_iter()
                    .map(|bar| at(bar, Pose::at([0.0, 0.0, look.glass - 0.1])))
                    .map(local),
            );
            // In the opening's head, two centimetres back from the face.
            strips.push(local(reveal_bar(width, 0.05, 0.08).placed(Pose::at([
                x0,
                top - 0.05,
                0.02,
            ]))));
        }
        part = part
            .element(
                Element::new(format!("wall-{side}"), wall, "clad")
                    .cut_material("dark")
                    .uv(UvMode::Box)
                    .collision(Collision::Bounds),
            )
            .element(Element::new(format!("liner-{side}"), liner, "liner").interior());
    }
    let corner_liners = plan.corners(t, t + LINER, floor_top, height - LINER - floor_top);
    let channels = plan.on_corners(CORNER_CHANNEL, -MARGIN, height + 2.0 * MARGIN);
    for (index, ((corner, liner), channel)) in plan
        .corners(0.0, t, 0.0, height)
        .into_iter()
        .zip(corner_liners)
        .zip(channels)
        .enumerate()
    {
        part = part
            .element(
                Element::new(format!("corner-{index}"), corner.subtract(channel), "clad")
                    .uv(UvMode::Box)
                    .collision(Collision::Hull),
            )
            .element(Element::new(format!("liner-corner-{index}"), liner, "liner").interior());
    }
    // Light up the corners: dark unless a tower binds `edge` to its neon.
    part = part.element(
        Element::new(
            "corner-lines",
            Geometry::union_all(plan.on_corners(CORNER_LINE, 0.0, height)),
            "edge",
        )
        .standalone()
        .far(),
    );
    let inner = plan.outline(t);
    part = part
        .element(
            Element::new("slab", Geometry::extrude(inner.clone(), SLAB), "concrete")
                .uv(UvMode::Box)
                .collision(Collision::Bounds),
        )
        .element(
            Element::new(
                "floor",
                Geometry::extrude(inner, LINER).placed(Pose::at([0.0, SLAB, 0.0])),
                "floor",
            )
            .uv(UvMode::Box)
            .interior(),
        );
    if let Some(fins) = fuse(fins) {
        part = part.element(Element::new("fins", fins, "concrete").uv(UvMode::Box));
    }
    if let Some(bands) = fuse(bands) {
        part = part.element(Element::new("bands", bands, "concrete").uv(UvMode::Box));
    }
    for (slot, glass) in PANES.into_iter().zip(panes) {
        if let Some(glass) = fuse(glass) {
            part = part.element(Element::new(format!("glass-{slot}"), glass, slot).standalone());
        }
    }
    if let Some(metal) = fuse(metal) {
        part = part.element(Element::new("sills", metal, "metal"));
    }
    if let Some(strips) = fuse(strips) {
        part = part.element(Element::new("strips", strips, "strip").standalone());
    }
    // The lift core against the back wall, with its doors and call light.
    let [core_w, core_d] = plan.core;
    let core_front = d - t - LINER - core_d;
    let core_height = height - floor_top - LINER;
    part = part
        .element(
            Element::new(
                "core",
                Geometry::cuboid([core_w, core_height, core_d]).placed(Pose::at([
                    (w - core_w) / 2.0,
                    floor_top,
                    core_front,
                ])),
                "metal",
            )
            .uv(UvMode::Box)
            .interior()
            .collision(Collision::Bounds),
        )
        .element(
            Element::new(
                "lift-doors",
                Geometry::union_all([
                    Geometry::cuboid([1.1, 2.2, 0.02]).placed(Pose::at([
                        w / 2.0 - 1.25,
                        floor_top,
                        core_front - 0.02,
                    ])),
                    Geometry::cuboid([1.1, 2.2, 0.02]).placed(Pose::at([
                        w / 2.0 + 0.15,
                        floor_top,
                        core_front - 0.02,
                    ])),
                ]),
                "dark",
            )
            .interior(),
        )
        .element(
            Element::new(
                "lift-lights",
                Geometry::cuboid([2.5, 0.06, 0.03]).placed(Pose::at([
                    w / 2.0 - 1.25,
                    floor_top + 2.35,
                    core_front - 0.03,
                ])),
                "light",
            )
            .interior()
            .standalone(),
        );
    if sky {
        part = part
            .element(
                Element::new(
                    "transfer-beam",
                    plan.ring(-0.5, 0.0, height - 1.35, 1.25),
                    "concrete",
                )
                .uv(UvMode::Box),
            )
            .element(
                Element::new(
                    "transfer-light",
                    plan.ring(-0.52, -0.49, height - 0.7, 0.18),
                    "edge",
                )
                .standalone()
                .far(),
            );
    }
    if lobby && plan.name == SPIRE[0].name {
        part = landmark_portico(part, height);
    }
    part = lift_lobby(part, plan);
    if lobby {
        part = lobby_furniture(lobby_front(part, plan, height), plan);
    } else {
        part = office(part, plan);
    }
    let [bottom, top] = stacking([w / 2.0, d / 2.0], height);
    part.socket(bottom).socket(top).build()
}

/// A chair's bent seat/back and pedestal, kept separate so their joint does
/// not split the broad faces. Coordinates are relative to its floor contact.
fn chair(at: [f64; 3]) -> [Geometry; 2] {
    [
        Geometry::extrude(
            [
                [-0.40, 0.0],
                [-0.48, 0.0],
                [-0.48, 0.48],
                [-0.98, 0.48],
                [-0.98, 0.56],
                [-0.40, 0.56],
            ],
            0.5,
        )
        .placed(Pose::at(at).rotated(DQuat::from_rotation_z(-FRAC_PI_2))),
        Geometry::cuboid([0.32, 0.42, 0.36]).placed(Pose::at([at[0] + 0.09, at[1], at[2] + 0.1])),
    ]
}

/// Distinct work, meeting and service zones leave a central route to the
/// lifts. Furniture scales by plan, not by instance or by occupied storey.
#[expect(
    clippy::too_many_lines,
    reason = "one shared plan lays out the adjoining furniture zones"
)]
fn office(mut part: super::PartBuilder, plan: &Plan) -> super::PartBuilder {
    let (min, size) = plan.room(STOREY);
    let y = SLAB + LINER;
    let bar = |size, at| Geometry::cuboid(size).placed(Pose::at(at));
    let left = min[0] + 0.6;
    let front = min[2] + 0.7;
    let meeting_w = (size[0] * 0.34).min(5.2);
    let meeting_d = (size[2] - 2.0).clamp(2.2, 4.8);
    let edge = left + meeting_w;
    let back = front + meeting_d;
    // An open 1.2 m doorway at the back of the meeting enclosure.
    let panes = vec![
        bar([0.04, 2.25, meeting_d], [edge, y + 0.16, front]),
        bar([meeting_w - 1.2, 2.25, 0.04], [left, y + 0.16, back]),
    ];
    let frames = vec![
        bar(
            [0.08, 0.12, meeting_d + 0.08],
            [edge - 0.02, y, front - 0.02],
        ),
        bar(
            [0.08, 0.1, meeting_d + 0.08],
            [edge - 0.02, y + 2.41, front - 0.02],
        ),
        bar(
            [meeting_w - 0.001, 0.1, 0.08],
            [left - 0.02, y + 2.41, back - 0.02],
        ),
        bar([0.08, 2.288, 0.08], [edge - 0.02, y + 0.121, front - 0.02]),
        bar([0.08, 2.288, 0.08], [edge - 0.02, y + 0.121, back - 0.02]),
        bar([0.08, 2.399, 0.08], [edge - 1.24, y + 0.01, back - 0.02]),
    ];
    let mut tops = vec![bar(
        [meeting_w - 0.7, 0.07, 0.8],
        [left + 0.35, y + 0.73, front + 0.85],
    )];
    let mut bases = vec![bar(
        [0.65, 0.73, 0.5],
        [left + meeting_w / 2.0 - 0.325, y, front + 1.0],
    )];
    let mut seats = Vec::new();
    for (x, z, turn) in [
        (left + 0.4, front + 1.85, 0.0),
        (edge - 0.4, front + 0.65, PI),
    ] {
        let [seat, base] = chair([0.0, 0.0, 0.0]);
        let pose = Pose::at([x, y, z]).rotated(yaw(turn));
        seats.push(at(seat, pose));
        bases.push(at(base, pose));
    }
    let work_x = edge + 1.5;
    let work_w = (min[0] + size[0] - work_x - 0.65).min(5.0);
    let mut screens = Vec::new();
    let mut dividers = Vec::new();
    let rows = if size[2] > 10.0 {
        3
    } else if size[2] > 6.0 {
        2
    } else {
        1
    };
    for row in 0..rows {
        let z = front + f64::from(row) * 2.3;
        tops.push(bar([work_w, 0.07, 0.85], [work_x, y + 0.73, z]));
        for x in [work_x + 0.1, work_x + work_w - 0.2] {
            bases.push(bar([0.1, 0.73, 0.65], [x, y, z + 0.1]));
        }
        dividers.push(bar(
            [work_w - 0.04, 0.43, 0.06],
            [work_x + 0.02, y + 0.8, z + 0.03],
        ));
        let places = if work_w > 3.0 { 2 } else { 1 };
        for place in 0..places {
            let x = work_x + (f64::from(place) + 0.5) * work_w / f64::from(places) - 0.25;
            let [seat, base] = chair([x, y, z + 1.05]);
            seats.push(seat);
            bases.push(base);
            screens.push(bar([0.5, 0.32, 0.025], [x, y + 0.84, z + 0.2]));
            bases.push(bar([0.07, 0.2, 0.07], [x + 0.215, y + 0.76, z + 0.19]));
        }
    }
    // A service alcove behind the work zone, partitioned from the desks.
    let rack_x = (min[0] + size[0] - 2.1).max(f64::midpoint(plan.width, plan.core[0]) + 0.5);
    let rack_z = plan.depth - plan.thickness() - 1.0;
    let rack = bar([0.85, 2.1, 0.65], [rack_x, y, rack_z]);
    let rack_face = bar([0.73, 1.85, 0.04], [rack_x + 0.06, y + 0.12, rack_z - 0.04]);
    let service_left = f64::midpoint(plan.width, plan.core[0]) + 0.02;
    dividers.push(bar(
        [min[0] + size[0] - service_left - 1.2, 2.6, 0.08],
        [service_left, y, rack_z - 0.9],
    ));
    part = part
        .element(
            Element::new("meeting-glass", Geometry::union_all(panes), "glass")
                .interior()
                .standalone(),
        )
        .element(Element::new("meeting-frame", Geometry::union_all(frames), "metal").interior())
        .element(Element::new("desktops", Geometry::union_all(tops), "trim").interior())
        .element(Element::new("furniture-bases", Geometry::union_all(bases), "metal").interior())
        .element(Element::new("chairs", Geometry::union_all(seats), "dark").interior())
        .element(Element::new("dividers", Geometry::union_all(dividers), "liner").interior())
        .element(
            Element::new(
                "screens",
                if screens.len() == 1 {
                    screens.remove(0)
                } else {
                    Geometry::union_all(screens)
                },
                "light",
            )
            .interior()
            .standalone(),
        )
        .element(Element::new("server-rack", rack, "metal").interior())
        .element(Element::new("server-door", rack_face, "dark").interior())
        .element(
            Element::new(
                "server-trays",
                reveal_bar(0.66, 0.035, 0.045)
                    .placed(Pose::at([rack_x + 0.09, y + 0.4, rack_z - 0.07]))
                    .arrayed(5, Pose::at([0.0, 0.3, 0.0])),
                "trim",
            )
            .interior(),
        );
    part
}

/// Lift call panels and a dark landing mat define the circulation zone.
fn lift_lobby(part: super::PartBuilder, plan: &Plan) -> super::PartBuilder {
    let front = plan.depth - plan.thickness() - LINER - plan.core[1];
    let x = plan.width / 2.0;
    let y = SLAB + LINER;
    part.element(
        Element::new(
            "lift-call-housing",
            Geometry::cuboid([0.18, 0.38, 0.05]).placed(Pose::at([
                x - 0.09,
                y + 1.1,
                front - 0.05,
            ])),
            "trim",
        )
        .interior(),
    )
    .element(
        Element::new(
            "lift-call-panel",
            Geometry::cuboid([0.1, 0.22, 0.025]).placed(Pose::at([
                x - 0.05,
                y + 1.18,
                front - 0.075,
            ])),
            "light",
        )
        .interior()
        .standalone(),
    )
    .element(
        Element::new(
            "lift-landing",
            Geometry::cuboid([3.0, 0.015, 0.5]).placed(Pose::at([x - 1.5, y, front - 0.55])),
            "dark",
        )
        .interior(),
    )
}

/// The entrance remains open to reception; paired security lanes stand
/// beyond it, with seating beside the glazing and a holo directory at the core.
fn lobby_furniture(part: super::PartBuilder, plan: &Plan) -> super::PartBuilder {
    let x = plan.width / 2.0;
    let y = SLAB + LINER;
    let front = plan.depth - plan.thickness() - LINER - plan.core[1];
    let bar = |size, at| Geometry::cuboid(size).placed(Pose::at(at));
    let gates = [-2.2, 0.0, 2.2].map(|dx| bar([0.26, 0.95, 1.0], [x + dx - 0.13, y, front - 1.8]));
    let seats = [2.0, plan.width - 4.4].map(|sx| {
        Geometry::union_all([
            bar([2.4, 0.3, 0.75], [sx, y + 0.15, 3.1]),
            bar([2.4, 0.6, 0.12], [sx, y + 0.45, 3.73]),
        ])
    });
    part.element(Element::new("lobby-seating", Geometry::union_all(seats), "dark").interior())
        .element(
            Element::new(
                "lobby-seat-feet",
                Geometry::union_all(
                    [2.0, plan.width - 4.4].map(|sx| bar([2.1, 0.2, 0.5], [sx + 0.15, y, 3.22])),
                ),
                "metal",
            )
            .interior(),
        )
        .element(Element::new("security-gates", Geometry::union_all(gates), "metal").interior())
        .element(
            Element::new(
                "security-readers",
                Geometry::union_all(
                    [-2.2, 0.0, 2.2].map(|dx| {
                        bar([0.16, 0.025, 0.25], [x + dx - 0.08, y + 0.95, front - 1.75])
                    }),
                ),
                "light",
            )
            .interior()
            .standalone(),
        )
        .element(
            Element::new(
                "holo-wall-frame",
                bar([1.0, 1.35, 0.09], [x + 1.4, y + 1.0, front - 0.09]),
                "metal",
            )
            .interior(),
        )
        .element(
            Element::new(
                "holo-wall",
                bar([0.8, 1.15, 0.035], [x + 1.5, y + 1.1, front - 0.125]),
                "holo",
            )
            .interior()
            .standalone(),
        )
}

/// A lobby's front, its podium and its furniture.
///
/// Either side of the entrance a shopfront is recessed between concrete piers:
/// lit shop glass over the plinth up to a steel fascia with a neon line under
/// it, dark glass over that, and a beam across the top. The entrance is a deep
/// steel portal under a canopy with a fascia, a lit soffit and tie rods, and a
/// sign standing on it. Glazed sliding leaves sit behind the surround. A plinth runs round the storey's foot and a cornice
/// round its top: the podium the tower stands on.
#[allow(
    clippy::too_many_lines,
    clippy::many_single_char_names,
    reason = "one builder chain, read top to bottom; w, d, s, t are the plan's own measures"
)]
fn lobby_front(part: super::PartBuilder, plan: &Plan, height: f64) -> super::PartBuilder {
    let (w, d) = (plan.width, plan.depth);
    let s = plan.run_start(0.0);
    let [door_w, door_h] = DOOR;
    let left = w / 2.0 - door_w / 2.0;
    let right = w / 2.0 + door_w / 2.0;
    let t = plan.thickness();
    let floor_top = SLAB + LINER;
    // The portal's jambs, how far it stands out of the face and how deep it
    // is; the glass line behind the piers; the transom at the door's head, the
    // fascia over it and the beam across the top.
    let (jamb, proud, deep) = (0.5, 0.3, 0.75);
    let glass = 0.35;
    let transom = door_h + 0.3;
    let fascia = transom + 0.8;
    let beam = height - 0.6;
    let spans = [(s, left - jamb), (right + jamb, w - s)];
    let surround = Geometry::cuboid([door_w + 2.0 * jamb, beam, deep])
        .placed(Pose::at([left - jamb, 0.0, -proud]))
        .subtract(
            Geometry::cuboid([door_w, door_h + MARGIN, deep + 2.0 * MARGIN])
                .placed(Pose::at([left, -MARGIN, -proud - MARGIN]))
                .portal("entrance"),
        );
    let desk = Geometry::chamfered_cuboid([4.0, 1.1, 1.0], 0.12).placed(Pose::at([
        w / 2.0 - 2.0,
        floor_top,
        d * 0.42,
    ]));
    let planter = |x: f64| {
        at(
            Geometry::revolve([[0.0, 0.0], [0.55, 0.0], [0.62, 0.7], [0.0, 0.7]], SMALL),
            Pose::at([x, floor_top, 1.6]),
        )
    };
    let shrub = |x: f64| Geometry::ball(0.7, 6).placed(Pose::at([x, floor_top + 1.2, 1.6]));
    let pane = |from: f64, to: f64, bottom: f64, top: f64| {
        Geometry::cuboid([to - from, top - bottom, 0.08]).placed(Pose::at([from, bottom, glass]))
    };
    // Piers every two and a half metres or so, from the face back to the
    // glass; the fascia over the shop glass, and a neon line in the
    // building's own colour under its front edge.
    let mut piers = Vec::new();
    let mut fascias = Vec::new();
    let mut neon = Vec::new();
    // The plinth round the storey, open at the door, and under the shop glass
    // back to the glass.
    let mut plinth = vec![
        plan.ring(-0.08, 0.0, 0.0, PLINTH).subtract(
            Geometry::cuboid([door_w, PLINTH + 2.0 * MARGIN, 1.0])
                .placed(Pose::at([left, -MARGIN, -0.5])),
        ),
    ];
    for (from, to) in spans {
        let span = to - from;
        #[allow(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "a pier count is a small positive whole number"
        )]
        let count = (span / 2.5).round().max(1.0) as u32;
        for index in 1..count {
            let x = from + span * f64::from(index) / f64::from(count);
            piers.push(
                Geometry::cuboid([0.36, beam, glass + 0.2]).placed(Pose::at([x - 0.18, 0.0, -0.2])),
            );
        }
        fascias.push(
            Geometry::cuboid([span, fascia - transom, 0.35])
                .placed(Pose::at([from, transom, -0.25])),
        );
        neon.push(Geometry::cuboid([span - 0.3, 0.05, 0.04]).placed(Pose::at([
            from + 0.15,
            transom - 0.05,
            -0.24,
        ])));
        // Two centimetres into the corner and the portal at its ends, so its
        // ends are not the glass's.
        plinth.push(
            Geometry::cuboid([span + 0.04, PLINTH, glass + 0.07]).placed(Pose::at([
                from - 0.02,
                0.0,
                -0.02,
            ])),
        );
    }
    // Tie rods from the canopy's front up to the portal's face.
    let (rise, reach): (f64, f64) = (1.5, 3.0);
    let rod = |x: f64| {
        at(
            Geometry::cuboid([0.05, 0.05, rise.hypot(reach) + 0.1])
                .placed(Pose::at([-0.025, -0.025, 0.0])),
            Pose::at([x, door_h + 0.85, -proud - reach])
                .rotated(DQuat::from_rotation_x(-rise.atan2(reach))),
        )
    };
    let mut part = part
        .element(Element::new("piers", Geometry::union_all(piers), "clad").uv(UvMode::Box))
        .element(Element::new("fascia", Geometry::union_all(fascias), "metal").uv(UvMode::Box))
        .element(
            Element::new("shop-neon", Geometry::union_all(neon), "crown")
                .standalone()
                .far(),
        )
        .element(Element::new("plinth", Geometry::union_all(plinth), "concrete").uv(UvMode::Box))
        .element(
            Element::new(
                "cornice",
                plan.ring(-0.15, 0.0, height - 0.45, 0.45),
                "concrete",
            )
            .uv(UvMode::Box),
        )
        .element(
            Element::new(
                "beam",
                Geometry::cuboid([w - 2.0 * s, height - beam, t]).placed(Pose::at([s, beam, 0.0])),
                "clad",
            )
            .uv(UvMode::Box),
        );
    // The shop glass below the fascia, lit; dark glass above it. A proxy
    // each side, never one across the door.
    for (side, (from, to)) in ["left", "right"].into_iter().zip(spans) {
        part = part.element(
            Element::new(
                format!("shop-glass-{side}"),
                pane(from, to, 0.0, transom),
                "shop",
            )
            .standalone()
            .collision(Collision::Bounds),
        );
    }
    // A pair of glazed sliding leaves closes the visual hole under the
    // canopy. Leaves and their hardware are standalone, ready to animate;
    // none contributes a static proxy across the walkable portal.
    for (side, from, to) in [
        ("left", left + 0.03, w / 2.0 - 0.02),
        ("right", w / 2.0 + 0.02, right - 0.03),
    ] {
        let bottom = floor_top + 0.02;
        let top = door_h - 0.03;
        part = part
            .element(
                Element::new(
                    format!("door-glass-{side}"),
                    Geometry::cuboid([to - from - 0.08, top - bottom - 0.08, 0.06])
                        .placed(Pose::at([from + 0.04, bottom + 0.04, 0.28])),
                    "glass",
                )
                .standalone(),
            )
            .element(
                Element::new(
                    format!("door-frame-{side}"),
                    at(
                        Geometry::union_all(frame_bars(from, to, bottom, top, &[], 0.0)),
                        Pose::at([0.0, 0.0, 0.22]),
                    ),
                    "trim",
                )
                .standalone(),
            );
        let handle_x = if side == "left" {
            to - 0.16
        } else {
            from + 0.12
        };
        let hardware = [
            Geometry::cuboid([to - from - 0.12, 0.18, 0.04]).placed(Pose::at([
                from + 0.06,
                bottom + 0.06,
                0.25,
            ])),
            Geometry::cuboid([0.04, 0.65, 0.04]).placed(Pose::at([handle_x, bottom + 0.9, 0.15])),
            Geometry::cuboid([0.04, 0.05, 0.12]).placed(Pose::at([handle_x, bottom + 0.9, 0.16])),
            Geometry::cuboid([0.04, 0.05, 0.12]).placed(Pose::at([handle_x, bottom + 1.5, 0.16])),
        ];
        part = part.element(
            Element::new(
                format!("door-hardware-{side}"),
                Geometry::union_all(hardware),
                "trim",
            )
            .standalone(),
        );
    }
    part.element(
        Element::new(
            "upper-glass",
            Geometry::union_all(spans.map(|(from, to)| pane(from, to, fascia, beam))),
            "glass",
        )
        .standalone(),
    )
    .element(Element::new("portal", surround, "metal").cut_material("trim"))
    .element(
        Element::new(
            "canopy",
            plate(
                [door_w + 3.0, 0.25, reach + 0.3],
                [left - 1.5, door_h + 0.6, -proud - reach - 0.2],
            ),
            "metal",
        )
        .uv(UvMode::Box),
    )
    .element(Element::new(
        "canopy-fascia",
        Geometry::cuboid([door_w + 3.1, 0.48, 0.12]).placed(Pose::at([
            left - 1.55,
            door_h + 0.4,
            -proud - reach - 0.28,
        ])),
        "metal",
    ))
    .element(Element::new(
        "canopy-rods",
        Geometry::union_all([rod(left - 0.2), rod(right + 0.2)]),
        "trim",
    ))
    // A lit soffit under the canopy, over the steps to the door.
    .element(
        Element::new(
            "canopy-light",
            Geometry::cuboid([door_w + 2.4, 0.02, reach - 0.3]).placed(Pose::at([
                left - 1.2,
                door_h + 0.58,
                -proud - reach,
            ])),
            "light",
        )
        .standalone(),
    )
    .element(
        Element::new(
            "sign",
            Geometry::cuboid([door_w + 1.6, 0.9, 0.12]).placed(Pose::at([
                left - 0.8,
                door_h + 0.85,
                -proud - reach - 0.12,
            ])),
            "holo",
        )
        .standalone(),
    )
    .element(
        Element::new("desk", desk, "trim")
            .interior()
            .collision(Collision::Bounds),
    )
    .element(
        Element::new(
            "desk-light",
            Geometry::cuboid([3.6, 0.04, 0.02]).placed(Pose::at([
                w / 2.0 - 1.8,
                floor_top + 0.9,
                d * 0.42 - 0.02,
            ])),
            "light",
        )
        .interior()
        .standalone(),
    )
    .element(
        Element::new(
            "planters",
            Geometry::union_all([planter(2.2), planter(w - 2.2)]),
            "concrete",
        )
        .interior(),
    )
    .element(
        Element::new(
            "shrubs",
            Geometry::union_all([shrub(2.2), shrub(w - 2.2)]),
            "foliage",
        )
        .interior(),
    )
}

/// The ceiling finish of a storey of `plan`, its top on the part's origin
/// plane: hung under the slab of whatever stands on the storey, and placed in
/// that level's storey group, so hiding the storeys above one hides its
/// ceiling with them and a camera above sees into it. The house does the same.
pub(super) fn rect_ceiling(id: &str, plan: &Plan) -> Result<Part, ValidationError> {
    let panels = [0.28, 0.72].map(|fraction| {
        Geometry::cuboid([1.5, 0.025, (plan.depth * 0.35).min(4.0)]).placed(Pose::at([
            plan.width * fraction - 0.75,
            -0.025,
            plan.depth * 0.25,
        ]))
    });
    Part::builder(id)
        .element(
            Element::new(
                "ceiling",
                Geometry::extrude(plan.outline(plan.thickness()), LINER),
                "liner",
            )
            .uv(UvMode::Box)
            .interior(),
        )
        .element(
            Element::new("ceiling-lights", Geometry::union_all(panels), "light")
                .interior()
                .standalone(),
        )
        .build()
}

/// Thickness of a roof slab.
const ROOF: f64 = 0.4;
/// Height of a parapet above the roof slab.
const PARAPET: f64 = 1.1;

/// A flat roof over a storey of `plan`, which is also the terrace the next
/// tier stands on: the slab over the whole footprint with a cornice round it
/// where the tier ends, a parapet under a steel coping that overhangs both
/// faces for a drip, a lit band under the coping in `crown`, which a tower
/// binds to its own neon, and planters of shrubs along the parapet.
pub(super) fn rect_roof(id: &str, plan: &Plan) -> Result<Part, ValidationError> {
    let mut part = Part::builder(id)
        .element(
            Element::new(
                "slab",
                Geometry::extrude(plan.outline(0.0), ROOF),
                "concrete",
            )
            .uv(UvMode::Box)
            .collision(Collision::Bounds),
        )
        .element(
            Element::new("cornice", plan.ring(-0.15, 0.0, 0.0, ROOF), "concrete").uv(UvMode::Box),
        );
    for (index, run) in plan
        .runs(0.0, 0.25, ROOF, PARAPET)
        .into_iter()
        .chain(plan.corners(0.0, 0.25, ROOF, PARAPET))
        .enumerate()
    {
        part = part.element(
            Element::new(format!("parapet-{index}"), run, "clad")
                .uv(UvMode::Box)
                .collision(Collision::Bounds),
        );
    }
    let mut planters = Vec::new();
    let mut shrubs = Vec::new();
    for side in 0..4 {
        let (pitch, centres) = plan.bays(side);
        let start = centres[0] - pitch / 2.0;
        let length = centres[centres.len() - 1] + pitch / 2.0 - start;
        let frame = plan.side(side);
        planters.push(at(
            Geometry::cuboid([length - 2.0, 0.55, 0.6]).placed(Pose::at([start + 1.0, ROOF, 0.25])),
            frame,
        ));
        shrubs.push(at(
            Geometry::cuboid([length - 2.2, 0.35, 0.5]).placed(Pose::at([
                start + 1.1,
                ROOF + 0.55,
                0.3,
            ])),
            frame,
        ));
    }
    let part = part
        .element(
            Element::new(
                "coping",
                plan.ring(-0.06, 0.31, ROOF + PARAPET, 0.1),
                "metal",
            )
            .uv(UvMode::Box),
        )
        .element(
            Element::new(
                "crown-light",
                plan.ring(-0.03, 0.0, ROOF + PARAPET - 0.18, 0.1),
                "crown",
            )
            .standalone()
            .far(),
        )
        .element(
            Element::new("planters", Geometry::union_all(planters), "concrete")
                .uv(UvMode::Box)
                .collision(Collision::Bounds),
        )
        .element(Element::new(
            "shrubs",
            Geometry::union_all(shrubs),
            "foliage",
        ));
    let [bottom, top] = stacking([plan.width / 2.0, plan.depth / 2.0], ROOF);
    part.socket(bottom).socket(top).build()
}

// ------------------------------------------------------------------ crowns

/// What finishes a tower's top, standing on its last roof.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Crown {
    /// A plant penthouse behind a louvred steel screen with a lit band.
    Screen,
    /// Three set-back steps, ribbed, each with a lit edge under its coping.
    Stepped,
    /// Broad blades with lit edges on a tapered plant lantern, or on a
    /// cage, the facade's open frame tied by a ring beam.
    Blades,
}

impl Crown {
    /// Every crown.
    pub(super) const ALL: [Self; 3] = [Self::Screen, Self::Stepped, Self::Blades];

    /// What its part is called after the plan: `city:{plan}-{name}`.
    pub(super) fn name(self) -> &'static str {
        match self {
            Self::Screen => "crown-screen",
            Self::Stepped => "crown-stepped",
            Self::Blades => "crown-blades",
        }
    }

    /// How far in from the roof's edge its deck's edge is over `plan`.
    pub(super) fn inset(self, plan: &Plan) -> f64 {
        if plan.name == SPIRE[2].name {
            return 4.7;
        }
        match self {
            Self::Screen => PENTHOUSE,
            Self::Stepped => steps(plan).last().map_or(0.0, |(inset, _, _)| *inset),
            Self::Blades if plan.style != Style::Cage => lantern_inset(plan),
            Self::Blades => ATTIC_INSET,
        }
    }

    /// How far above the roof a topper stands on this crown over `plan`: on
    /// the penthouse, on the last step, or on the plant room among the
    /// blades.
    pub(super) fn deck(self, plan: &Plan) -> f64 {
        if plan.name == SPIRE[2].name {
            return HERO_CROWN_HEIGHT + 0.3;
        }
        match self {
            Self::Screen => SCREEN + 0.15,
            Self::Stepped => steps(plan)
                .last()
                .map_or(0.0, |(_, base, height)| base + height + 0.1),
            Self::Blades if plan.style != Style::Cage => LANTERN + 0.2,
            Self::Blades => ATTIC,
        }
    }
}

/// Height of a screened penthouse.
const SCREEN: f64 = 4.6;
/// How far a penthouse stands in from the roof's edge.
const PENTHOUSE: f64 = 1.6;
/// Height of the plant room among a crown's blades, and how far it stands in
/// from the roof's edge.
const ATTIC: f64 = 3.2;
const ATTIC_INSET: f64 = 1.0;

/// The frame crown is a tall, tapered plant enclosure. Its broad ribs and
/// sloping shoulders read against the sky after the facade's fins disappear.
const LANTERN: f64 = 12.0;

fn lantern_inset(plan: &Plan) -> f64 {
    plan.width.min(plan.depth) * 0.28
}

fn lantern(mut part: super::PartBuilder, plan: &Plan) -> super::PartBuilder {
    let inset = lantern_inset(plan);
    let top = plan.inset(inset);
    let base = plan.inset(1.0);
    let points = base
        .outline(0.0)
        .into_iter()
        .map(|[x, z]| [x + 1.0, 0.0, z + 1.0])
        .chain(
            top.outline(0.0)
                .into_iter()
                .map(|[x, z]| [x + inset, LANTERN, z + inset]),
        );
    part = part.element(
        Element::new("lantern", Geometry::hull(points), "clad")
            .uv(UvMode::Box)
            .collision(Collision::Hull),
    );
    let mut ribs = Vec::new();
    let mut lights = Vec::new();
    let mut vents = Vec::new();
    for side in 0..4 {
        let middle = if side % 2 == 0 {
            plan.width
        } else {
            plan.depth
        } / 2.0;
        let frame = plan.side(side);
        // Four substantial blades follow the enclosure's rake. Their backs
        // enter the enclosure by 20 cm, so they look rooted in the roof.
        let blade = |half: f64, front: f64, back: f64, low: f64, high: f64| {
            Geometry::hull([low, high].into_iter().flat_map(|y| {
                let shift = (inset - 1.0) * y / LANTERN;
                [
                    [middle - half, y, front + shift],
                    [middle + half, y, front + shift],
                    [middle - half, y, back + shift],
                    [middle + half, y, back + shift],
                ]
            }))
        };
        // Broad ventilation courses sit against the sloping plant enclosure,
        // behind its blades. Their shallow sections drop out at coarse LOD.
        for course in 0..7 {
            let y = 2.0 + f64::from(course);
            let face = 1.0 + (inset - 1.0) * y / LANTERN;
            let start = face + plan.chamfer + 0.12;
            vents.push(at(
                Geometry::cuboid([2.0 * (middle - start), 0.24, 0.26]).placed(Pose::at([
                    start,
                    y,
                    face - 0.12,
                ])),
                frame,
            ));
        }
        ribs.push(at(blade(0.9, 0.48, 1.2, 0.03, LANTERN + 0.05), frame));
        lights.push(at(blade(0.055, 0.44, 0.47, 0.35, LANTERN - 0.35), frame));
    }
    lights.push(at(
        top.ring(-0.12, 0.05, LANTERN - 0.25, 0.12),
        Pose::at([inset, 0.0, inset]),
    ));
    part.element(Element::new("blades", Geometry::union_all(ribs), "concrete").uv(UvMode::Box))
        .element(Element::new("vents", Geometry::union_all(vents), "dark").uv(UvMode::Box))
        .element(
            Element::new(
                "coping",
                at(
                    Geometry::extrude(top.outline(-0.2), 0.2),
                    Pose::at([inset, LANTERN, inset]),
                ),
                "metal",
            )
            .uv(UvMode::Box),
        )
        .element(
            Element::new("lights", Geometry::union_all(lights), "crown")
                .standalone()
                .far(),
        )
}

/// A stepped crown's steps over `plan`: how far each stands in from the
/// roof's edge, its base and its height. The steps pace in by a share of the
/// plan, so a slim tower's top step still carries a topper.
fn steps(plan: &Plan) -> [(f64, f64, f64); 3] {
    let pace = plan.width.min(plan.depth) * 0.08;
    let mut base = 0.0;
    [(0.0, 2.4), (1.0, 1.9), (2.0, 1.5)].map(|(index, height)| {
        let step = (0.9 + pace * index, base, height);
        base += height;
        step
    })
}

/// A crown of kind `kind` over the top storey of `plan`, standing on its roof:
/// the part's origin is the roof's top at the footprint's minimum corner.
#[allow(
    clippy::too_many_lines,
    reason = "three crowns, each one builder chain"
)]
pub(super) fn crown(id: &str, plan: &Plan, kind: Crown) -> Result<Part, ValidationError> {
    if plan.name == SPIRE[2].name {
        return landmark_crown(id, plan);
    }
    let mut part = Part::builder(id);
    match kind {
        Crown::Screen => {
            let sub = plan.inset(PENTHOUSE);
            let place = |geometry: Geometry| at(geometry, Pose::at([PENTHOUSE, 0.0, PENTHOUSE]));
            // Eight slats along each wall of the penthouse, standing off it
            // on posts at its bay lines and at the ends of its runs, which
            // leave its chamfered corners bare concrete.
            let mut louvres = Vec::new();
            for side in 0..4 {
                let (pitch, centres) = sub.bays(side);
                let start = centres[0] - pitch / 2.0;
                let end = centres[centres.len() - 1] + pitch / 2.0;
                for index in 0..8 {
                    let y = 1.5 + f64::from(index) * 0.34;
                    louvres.push(place(at(
                        Geometry::cuboid([end - start, 0.07, 0.2])
                            .placed(Pose::at([start, y, -0.35])),
                        sub.side(side),
                    )));
                }
                let mut posts = sub.lines(side);
                posts.extend([start, end]);
                for x in posts {
                    louvres.push(place(at(
                        Geometry::cuboid([0.08, SCREEN - 1.45, 0.34]).placed(Pose::at([
                            x - 0.04,
                            1.45,
                            -0.34,
                        ])),
                        sub.side(side),
                    )));
                }
            }
            part = part
                .element(
                    Element::new(
                        "penthouse",
                        place(Geometry::extrude(sub.outline(0.0), SCREEN)),
                        "concrete",
                    )
                    .uv(UvMode::Box)
                    .collision(Collision::Hull),
                )
                .element(Element::new(
                    "louvres",
                    Geometry::union_all(louvres),
                    "metal",
                ))
                .element(
                    Element::new(
                        "coping",
                        place(sub.ring(-0.45, 0.15, SCREEN, 0.15)),
                        "metal",
                    )
                    .uv(UvMode::Box),
                )
                .element(
                    Element::new("lights", place(sub.ring(-0.36, 0.02, 4.15, 0.12)), "crown")
                        .standalone()
                        .far(),
                );
        }
        Crown::Stepped => {
            let mut ribs = Vec::new();
            let mut copings = Vec::new();
            let mut lights = Vec::new();
            for (index, (inset, base, height)) in steps(plan).into_iter().enumerate() {
                let sub = plan.inset(inset);
                let place = |geometry: Geometry| at(geometry, Pose::at([inset, base, inset]));
                part = part.element(
                    Element::new(
                        format!("step-{index}"),
                        place(Geometry::extrude(sub.outline(0.0), height)),
                        "clad",
                    )
                    .uv(UvMode::Box)
                    .collision(Collision::Hull),
                );
                // A rib at every bay line, up to the lit edge.
                for side in 0..4 {
                    for line in sub.lines(side) {
                        ribs.push(place(at(
                            Geometry::cuboid([0.3, height - 0.3, 0.12]).placed(Pose::at([
                                line - 0.15,
                                0.0,
                                -0.12,
                            ])),
                            sub.side(side),
                        )));
                    }
                }
                copings.push(place(sub.ring(-0.08, 0.3, height, 0.1)));
                lights.push(place(sub.ring(-0.04, 0.0, height - 0.3, 0.12)));
            }
            if let Some(ribs) = fuse(ribs) {
                part = part.element(Element::new("ribs", ribs, "concrete").uv(UvMode::Box));
            }
            part = part
                .element(
                    Element::new("copings", Geometry::union_all(copings), "metal").uv(UvMode::Box),
                )
                .element(
                    Element::new("lights", Geometry::union_all(lights), "crown")
                        .standalone()
                        .far(),
                );
        }
        Crown::Blades if plan.style != Style::Cage => {
            part = lantern(part, plan);
        }
        Crown::Blades => {
            // The cage continues its open frame above the plant deck,
            // tied together by a level ring beam.
            let attic = plan.inset(ATTIC_INSET);
            let low = 6.5;
            let mut blades = vec![plan.ring(-0.3, ATTIC_INSET + 0.1, ATTIC - 0.3, 0.3)];
            let mut lights = vec![plan.ring(-0.33, -0.28, ATTIC - 0.24, 0.12)];
            for side in 0..4 {
                let frame = plan.side(side);
                for x in plan.lines(side) {
                    blades.push(at(fin(x, 0.55, low + 0.5), frame));
                    lights.push(at(
                        Geometry::cuboid([0.06, low - 1.9, 0.03]).placed(Pose::at([
                            x - 0.03,
                            0.3,
                            -0.58,
                        ])),
                        frame,
                    ));
                }
            }
            blades.extend(plan.on_corners(
                [[-0.16, -0.12], [0.16, -0.12], [0.08, 0.55], [-0.08, 0.55]],
                0.0,
                low,
            ));
            lights.extend(plan.on_corners(
                [[-0.03, 0.55], [0.03, 0.55], [0.03, 0.58], [-0.03, 0.58]],
                0.3,
                low - 1.9,
            ));
            blades.push(plan.ring(-0.55, 0.12, low - 0.1, 0.6));
            lights.push(plan.ring(-0.58, -0.55, low - 0.4, 0.12));
            part = part
                .element(
                    Element::new(
                        "attic",
                        at(
                            Geometry::extrude(attic.outline(0.0), ATTIC),
                            Pose::at([ATTIC_INSET, 0.0, ATTIC_INSET]),
                        ),
                        "concrete",
                    )
                    .uv(UvMode::Box)
                    .collision(Collision::Hull),
                )
                .element(
                    Element::new("blades", Geometry::union_all(blades), "concrete").uv(UvMode::Box),
                )
                .element(
                    Element::new("lights", Geometry::union_all(lights), "crown")
                        .standalone()
                        .far(),
                );
        }
    }
    let [bottom, top] = stacking([plan.width / 2.0, plan.depth / 2.0], kind.deck(plan));
    part.socket(bottom).socket(top).build()
}

// ------------------------------------------------------------------ toppers

/// A structural member between authored joints. The overlap seats it in both
/// joints, and eight sides suffice for a brace seen against the sky.
fn roof_brace(start: [f64; 3], end: [f64; 3], radius: f64) -> Geometry {
    let start = glam::DVec3::from_array(start);
    let delta = glam::DVec3::from_array(end) - start;
    Geometry::cylinder(radius, delta.length() + 0.04, 8).placed(
        Pose::at((start - delta.normalize() * 0.02).to_array())
            .rotated(DQuat::from_rotation_arc(glam::DVec3::Y, delta.normalize())),
    )
}

fn roof_bar(size: [f64; 3], at: [f64; 3]) -> Geometry {
    Geometry::cuboid(size).placed(Pose::at(at))
}

/// A needle spire on a drum base, ringed with lights: a landmark's crown.
pub(super) fn spire() -> Result<Part, ValidationError> {
    let profile = vec![
        [0.0, 0.0],
        [2.6, 0.0],
        [2.6, 1.6],
        [1.6, 2.4],
        [1.1, 8.0],
        [0.6, 16.0],
        [0.12, 26.0],
        [0.0, 26.4],
    ];
    Part::builder("city:spire")
        .element(turned("spire", profile, "trim").collision(Collision::Hull))
        .element(
            Element::new(
                "rings",
                Geometry::union_all([
                    Geometry::cylinder(1.25, 0.18, ROUND).placed(Pose::at([0.0, 6.0, 0.0])),
                    Geometry::cylinder(0.95, 0.18, ROUND).placed(Pose::at([0.0, 11.0, 0.0])),
                    Geometry::cylinder(0.62, 0.16, ROUND).placed(Pose::at([0.0, 16.0, 0.0])),
                ]),
                "light",
            )
            .uv(UvMode::Cylindrical { axis: Axis::Y })
            .standalone(),
        )
        .element(beacon("beacon", 0.22, [0.0, 26.6, 0.0]))
        .element(Element::new(
            "base-ribs",
            plate([0.22, 2.1, 0.4], [-0.11, 0.06, -2.67])
                .arrayed(8, Pose::default().rotated(yaw(PI / 4.0))),
            "metal",
        ))
        .element(Element::new(
            "base-flange",
            Geometry::revolve([[2.45, 0.05], [2.83, 0.05], [2.83, 0.22], [2.45, 0.22]], 24),
            "metal",
        ))
        .element(Element::new(
            "service-door",
            plate([0.72, 1.2, 0.15], [-0.36, 0.28, -2.65]),
            "dark",
        ))
        .build()
}

/// A lattice mast with cross bars and a beacon.
pub(super) fn mast() -> Result<Part, ValidationError> {
    let corners = [[-1.0, -1.0], [1.0, -1.0], [1.0, 1.0], [-1.0, 1.0]];
    let joint = |corner: usize, y: f64| {
        let half = 0.85 - y * 0.05;
        [corners[corner][0] * half, y, corners[corner][1] * half]
    };
    let legs = (0..4).map(|corner| roof_brace(joint(corner, 0.0), joint(corner, 14.0), 0.10));
    let mut bars = Vec::new();
    for level in 0..5 {
        let low = f64::from(level) * 2.8;
        let high = low + 2.8;
        for side in 0..4 {
            let next = (side + 1) % 4;
            bars.push(roof_brace(joint(side, high), joint(next, high), 0.055));
            bars.push(roof_brace(joint(side, low), joint(next, high), 0.045));
        }
    }
    let mut part = Part::builder("city:mast");
    // Each brace is a closed member seated in a leg, without fusing the
    // several nearly tangent members meeting at a joint.
    for (index, member) in bars.into_iter().enumerate() {
        part = part.element(Element::new(format!("brace-{index}"), member, "trim"));
    }
    part.element(Element::new("legs", Geometry::union_all(legs), "trim").collision(Collision::Hull))
        .element(Element::new(
            "whip",
            Geometry::cylinder(0.05, 5.0, 8).placed(Pose::at([0.0, 13.8, 0.0])),
            "trim",
        ))
        .element(beacon("beacon", 0.16, [0.0, 19.0, 0.0]))
        .element(Element::new(
            "foot-plates",
            Geometry::union_all(
                corners.map(|[x, z]| {
                    plate([0.46, 0.13, 0.46], [x * 0.85 - 0.23, 0.0, z * 0.85 - 0.23])
                }),
            ),
            "metal",
        ))
        .element(Element::new(
            "service-platform",
            roof_bar([1.1, 0.1, 1.1], [-0.55, 8.3, -0.55]),
            "deck",
        ))
        .element(Element::new(
            "ladder-rails",
            roof_bar([0.045, 8.4, 0.05], [-0.2, 0.0, -0.92])
                .arrayed(2, Pose::at([0.355, 0.0, 0.0])),
            "trim",
        ))
        .element(Element::new(
            "ladder-rungs",
            roof_bar([0.42, 0.035, 0.065], [-0.21, 0.3, -0.93])
                .arrayed(26, Pose::at([0.0, 0.31, 0.0])),
            "trim",
        ))
        .element(Element::new(
            "whip-socket",
            Geometry::cylinder(0.18, 0.65, 12).placed(Pose::at([0.0, 13.65, 0.0])),
            "trim",
        ))
        .element(Element::new(
            "ladder-brackets",
            Geometry::union_all(
                [0.5, 4.0, 8.0].map(|y| roof_bar([0.48, 0.06, 0.15 + y * 0.05], [-0.24, y, -0.93])),
            ),
            "trim",
        ))
        .build()
}

/// A raised octagonal helipad with a painted ring and H, and edge lights.
pub(super) fn helipad() -> Result<Part, ValidationError> {
    let radius = 7.0;
    let legs = (0..4).map(|index| {
        let angle = f64::from(index) * FRAC_PI_2 + PI / 4.0;
        Geometry::cylinder(0.25, 1.6, SMALL).placed(Pose::at([
            4.2 * angle.cos(),
            0.0,
            -4.2 * angle.sin(),
        ]))
    });
    let top = 1.9;
    let h_bar = |size: [f64; 3], corner: [f64; 3]| Geometry::cuboid(size).placed(Pose::at(corner));
    Part::builder("city:helipad")
        .element(
            Element::new("legs", Geometry::union_all(legs), "trim").collision(Collision::Bounds),
        )
        .element(
            Element::new(
                "deck",
                Geometry::cylinder(radius, 0.3, 8)
                    .placed(Pose::at([0.0, 1.6, 0.0]).rotated(yaw(PI / 8.0))),
                "deck",
            )
            .uv(UvMode::Box)
            .collision(Collision::Hull),
        )
        .element(Element::new(
            "markings",
            Geometry::union_all([
                Geometry::revolve(
                    [[4.6, top], [5.0, top], [5.0, top + 0.02], [4.6, top + 0.02]],
                    ROUND,
                ),
                h_bar([0.5, 0.02, 3.2], [-1.3, top, -1.6]),
                h_bar([0.5, 0.02, 3.2], [0.8, top, -1.6]),
                h_bar([1.6, 0.02, 0.5], [-0.8, top, -0.25]),
            ]),
            "marking",
        ))
        .element(
            Element::new(
                "edge-lights",
                Geometry::cuboid([0.3, 0.12, 0.3])
                    .placed(Pose::at([radius * 0.9 - 0.3, top, -0.15]))
                    .arrayed(8, Pose::default().rotated(yaw(PI / 4.0))),
                "light",
            )
            .standalone(),
        )
        .element(Element::new(
            "edge-housings",
            plate([0.44, 0.12, 0.44], [radius * 0.9 - 0.37, top - 0.08, -0.22])
                .arrayed(8, Pose::default().rotated(yaw(PI / 4.0))),
            "metal",
        ))
        .element(Element::new(
            "under-deck-beams",
            Geometry::union_all([
                roof_bar([9.0, 0.25, 0.2], [-4.5, 1.36, -3.0]),
                roof_bar([9.0, 0.25, 0.2], [-4.5, 1.36, 2.8]),
                roof_bar([0.2, 0.25, 9.0], [-3.0, 1.36, -4.5]),
                roof_bar([0.2, 0.25, 9.0], [2.8, 1.36, -4.5]),
            ]),
            "trim",
        ))
        // A lowered perimeter catch rail stays below the aircraft's deck.
        .element(Element::new(
            "catch-rails",
            Geometry::union_all((0..8).map(|side| {
                let angle = f64::from(side) * PI / 4.0;
                at(
                    Geometry::union_all([
                        roof_bar([4.8, 0.07, 0.07], [-2.4, 1.43, -6.76]),
                        roof_bar([0.06, 0.4, 0.08], [-2.3, 1.12, -6.77]),
                        roof_bar([0.06, 0.4, 0.08], [2.24, 1.12, -6.77]),
                        roof_bar([0.07, 0.07, 0.65], [-2.3, 1.12, -6.77]),
                        roof_bar([0.07, 0.07, 0.65], [2.24, 1.12, -6.77]),
                    ]),
                    Pose::default().rotated(yaw(angle)),
                )
            })),
            "trim",
        ))
        .build()
}

/// Rooftop plant: air handlers with fan grilles, a tank and a pipe run.
pub(super) fn roof_plant() -> Result<Part, ValidationError> {
    let fans = (0..3).map(|index| {
        Geometry::cylinder(0.55, 0.05, SMALL).placed(Pose::at([
            -2.4 + f64::from(index) * 1.6,
            1.6,
            0.0,
        ]))
    });
    Part::builder("city:roof-plant")
        .element(
            Element::new(
                "handler",
                plate([5.2, 1.6, 2.2], [-3.2, 0.0, -1.1]),
                "metal",
            )
            .uv(UvMode::Box)
            .collision(Collision::Bounds),
        )
        .element(Element::new("fans", Geometry::union_all(fans), "dark"))
        .element(
            Element::new(
                "tank",
                Geometry::revolve(
                    [[0.0, 0.0], [0.9, 0.0], [0.9, 2.2], [0.5, 2.6], [0.0, 2.6]],
                    ROUND,
                )
                .placed(Pose::at([3.1, 0.0, 0.0])),
                "trim",
            )
            .uv(UvMode::Cylindrical { axis: Axis::Y })
            .collision(Collision::Hull),
        )
        .element(Element::new(
            "pipe",
            at(along_x(0.12, 2.8, SMALL), Pose::at([0.0, 0.5, 1.6])),
            "rust",
        ))
        .element(Element::new(
            "fan-collars",
            Geometry::revolve(
                [[0.52, 0.0], [0.61, 0.0], [0.61, 0.13], [0.52, 0.13]],
                SMALL,
            )
            .placed(Pose::at([-2.4, 1.57, 0.0]))
            .arrayed(3, Pose::at([1.6, 0.0, 0.0])),
            "metal",
        ))
        .element(Element::new(
            "fan-guards",
            roof_bar([1.05, 0.04, 0.04], [-2.925, 1.69, -0.3])
                .arrayed(5, Pose::at([0.0, 0.0, 0.15]))
                .arrayed(3, Pose::at([1.6, 0.0, 0.0])),
            "trim",
        ))
        .element(Element::new(
            "louvres",
            roof_bar([4.7, 0.07, 0.15], [-2.95, 0.35, -1.2]).arrayed(5, Pose::at([0.0, 0.22, 0.0])),
            "trim",
        ))
        .element(Element::new(
            "duct",
            plate([0.9, 0.6, 0.9], [0.75, 0.3, 0.96]),
            "metal",
        ))
        .element(Element::new(
            "tank-return",
            Geometry::union_all([
                roof_brace([2.75, 0.5, 1.6], [3.1, 0.5, 1.6], 0.12),
                roof_brace([3.1, 0.5, 1.6], [3.1, 0.5, 0.65], 0.12),
            ]),
            "rust",
        ))
        .build()
}

/// A dish antenna on a squat mount.
pub(super) fn dish() -> Result<Part, ValidationError> {
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
    Part::builder("city:dish")
        .element(
            turned(
                "mount",
                vec![[0.0, 0.0], [0.8, 0.0], [0.5, 0.6], [0.3, 2.0], [0.0, 2.0]],
                "trim",
            )
            .collision(Collision::Hull),
        )
        .element(Element::new(
            "dish",
            at(
                Geometry::revolve(dish, 36),
                Pose::at([0.0, 2.4, 0.0]).rotated(DQuat::from_rotation_x(-0.8)),
            ),
            "hull",
        ))
        .element(Element::new(
            "gimbal",
            Geometry::cylinder(0.36, 0.95, SMALL)
                .placed(Pose::at([-0.475, 2.05, 0.0]).rotated(DQuat::from_rotation_z(-FRAC_PI_2))),
            "trim",
        ))
        .element(Element::new(
            "feed-struts",
            at(
                Geometry::union_all([
                    roof_brace([-1.55, 0.7, 0.0], [0.0, 1.9, 0.0], 0.04),
                    roof_brace([1.55, 0.7, 0.0], [0.0, 1.9, 0.0], 0.04),
                    roof_brace([0.0, 0.7, 1.55], [0.0, 1.9, 0.0], 0.04),
                ]),
                Pose::at([0.0, 2.4, 0.0]).rotated(DQuat::from_rotation_x(-0.8)),
            ),
            "trim",
        ))
        .element(Element::new(
            "receiver",
            at(
                Geometry::cylinder(0.14, 0.3, 12).placed(Pose::at([0.0, 1.78, 0.0])),
                Pose::at([0.0, 2.4, 0.0]).rotated(DQuat::from_rotation_x(-0.8)),
            ),
            "dark",
        ))
        .element(Element::new(
            "mount-feet",
            plate([0.3, 0.13, 0.3], [-0.15, 0.0, -0.85])
                .arrayed(4, Pose::default().rotated(yaw(FRAC_PI_2))),
            "metal",
        ))
        .build()
}

/// A cluster of thin radio masts of three heights, each with a warning light.
pub(super) fn antennas() -> Result<Part, ValidationError> {
    let masts = [([-1.2, -0.6], 9.0), ([0.9, -0.9], 14.0), ([0.2, 1.1], 6.5)];
    let poles = masts.iter().map(|([x, z], height)| {
        Geometry::cylinder(0.07, *height, 8).placed(Pose::at([*x, 0.0, *z]))
    });
    let arms = masts
        .iter()
        .map(|([x, z], height)| at(along_x(0.03, 1.2, 6), Pose::at([x - 0.6, height * 0.7, *z])));
    let mut part = Part::builder("city:antennas")
        .element(
            Element::new("base", plate([3.4, 0.3, 3.0], [-1.7, 0.0, -1.5]), "metal")
                .uv(UvMode::Box)
                .collision(Collision::Bounds),
        )
        .element(Element::new(
            "masts",
            at(
                Geometry::union_all(poles.chain(arms)),
                Pose::at([0.0, 0.3, 0.0]),
            ),
            "trim",
        ));
    for (index, ([x, z], height)) in masts.into_iter().enumerate() {
        part = part.element(
            Element::new(
                format!("warning-{index}"),
                Geometry::ball(0.14, 5).placed(Pose::at([x, 0.3 + height + 0.1, z])),
                "warning",
            )
            .standalone(),
        );
    }
    part = part
        .element(Element::new(
            "sockets",
            Geometry::union_all(masts.map(|([x, z], _)| {
                Geometry::cylinder(0.2, 0.4, 12).placed(Pose::at([x, 0.25, z]))
            })),
            "trim",
        ))
        .element(Element::new(
            "equipment",
            plate([0.7, 0.6, 0.45], [-0.35, 0.3, -0.2]),
            "metal",
        ));
    part.build()
}

/// A rooftop billboard: a holo screen twelve metres by five on a braced steel
/// frame, facing -Z, standing on its origin.
pub(super) fn roof_billboard() -> Result<Part, ValidationError> {
    let (width, height, lift) = (12.0, 5.0, 2.0);
    let bar = |size: [f64; 3], corner: [f64; 3]| Geometry::cuboid(size).placed(Pose::at(corner));
    let legs = [-5.0, 0.0, 5.0].map(|x| bar([0.3, lift + height, 0.3], [x - 0.15, 0.0, 0.6]));
    let braces = [-5.0, 0.0, 5.0].map(|x| {
        at(
            bar([0.16, 3.2, 0.16], [-0.08, 0.0, -0.08]),
            Pose::at([x, 0.0, 2.6]).rotated(DQuat::from_rotation_x(-0.55)),
        )
    });
    Part::builder("city:roof-billboard")
        .element(
            Element::new("frame", Geometry::union_all(legs), "trim").collision(Collision::Bounds),
        )
        .element(Element::new("braces", Geometry::union_all(braces), "trim"))
        .element(Element::new(
            "back",
            bar(
                [width + 0.4, height + 0.4, 0.2],
                [-width / 2.0 - 0.2, lift - 0.2, 0.4],
            ),
            "dark",
        ))
        .element(
            Element::new(
                "screen",
                bar([width, height, 0.1], [-width / 2.0, lift, 0.29]),
                "holo",
            )
            .standalone(),
        )
        .element(Element::new(
            "screen-frame",
            roof_bar([12.4, 5.4, 0.18], [-6.2, 1.8, 0.20])
                .subtract(roof_bar([11.9, 4.9, 0.4], [-5.95, 2.05, 0.1])),
            "trim",
        ))
        .element(Element::new(
            "maintenance-walk",
            roof_bar([11.0, 0.12, 0.85], [-5.5, 1.8, 0.8]),
            "deck",
        ))
        .element(Element::new(
            "rear-cross-braces",
            Geometry::union_all([
                roof_brace([-5.0, 2.1, 0.8], [0.0, 6.8, 0.8], 0.065),
                roof_brace([0.0, 2.1, 0.8], [5.0, 6.8, 0.8], 0.065),
            ]),
            "trim",
        ))
        .element(Element::new(
            "frame-shoes",
            plate([0.65, 0.16, 3.0], [-5.325, 0.0, 0.3]).arrayed(3, Pose::at([5.0, 0.0, 0.0])),
            "trim",
        ))
        .build()
}

/// A water tank on four legs, the kind that stands on an older roof.
pub(super) fn water_tank() -> Result<Part, ValidationError> {
    let legs = (0..4).map(|index| {
        let angle = f64::from(index) * FRAC_PI_2 + PI / 4.0;
        Geometry::cylinder(0.1, 2.4, 8).placed(Pose::at([
            1.3 * angle.cos(),
            0.0,
            1.3 * angle.sin(),
        ]))
    });
    Part::builder("city:water-tank")
        .element(
            Element::new("legs", Geometry::union_all(legs), "rust").collision(Collision::Bounds),
        )
        .element(
            turned(
                "tank",
                vec![[0.0, 2.4], [1.8, 2.4], [1.8, 5.4], [1.2, 6.4], [0.0, 6.6]],
                "rust",
            )
            .collision(Collision::Hull),
        )
        .element(Element::new(
            "hoops",
            Geometry::union_all([
                Geometry::cylinder(1.84, 0.08, ROUND).placed(Pose::at([0.0, 3.1, 0.0])),
                Geometry::cylinder(1.84, 0.08, ROUND).placed(Pose::at([0.0, 4.4, 0.0])),
            ]),
            "trim",
        ))
        .element(Element::new(
            "leg-bracing",
            Geometry::union_all((0..4).map(|side| {
                let a = f64::from(side) * FRAC_PI_2 + PI / 4.0;
                let b = a + FRAC_PI_2;
                roof_brace(
                    [1.3 * a.cos(), 0.25, 1.3 * a.sin()],
                    [1.3 * b.cos(), 2.25, 1.3 * b.sin()],
                    0.05,
                )
            })),
            "rust",
        ))
        .element(Element::new(
            "outlet",
            Geometry::union_all([
                roof_brace([0.0, 2.45, 0.0], [0.0, 0.45, 0.0], 0.12),
                roof_brace([0.0, 0.45, 0.0], [1.3, 0.45, 0.0], 0.12),
            ]),
            "rust",
        ))
        .element(Element::new(
            "hatch",
            Geometry::cylinder(0.32, 0.13, SMALL).placed(Pose::at([0.0, 6.55, 0.0])),
            "trim",
        ))
        .element(Element::new(
            "ladder-rails",
            roof_bar([0.045, 3.9, 0.05], [-0.26, 2.2, -1.91])
                .arrayed(2, Pose::at([0.475, 0.0, 0.0])),
            "trim",
        ))
        .element(Element::new(
            "ladder-rungs",
            roof_bar([0.54, 0.035, 0.065], [-0.27, 2.4, -1.92])
                .arrayed(12, Pose::at([0.0, 0.31, 0.0])),
            "trim",
        ))
        .element(Element::new(
            "ladder-brackets",
            roof_bar([0.6, 0.06, 0.45], [-0.3, 2.7, -1.91]).arrayed(3, Pose::at([0.0, 1.1, 0.0])),
            "trim",
        ))
        .build()
}

/// A shallow exhaust on a terrace, kept inside the exposed setback strip.
fn roof_vent() -> Result<Part, ValidationError> {
    Part::builder("city:roof-vent")
        .element(Element::new(
            "curb",
            plate([1.4, 0.25, 0.5], [-0.7, 0.0, -0.25]),
            "metal",
        ))
        .element(Element::new(
            "cowl",
            plate([1.5, 0.14, 0.54], [-0.75, 0.62, -0.27]),
            "metal",
        ))
        .element(Element::new(
            "cheeks",
            roof_bar([0.10, 0.44, 0.46], [-0.65, 0.22, -0.23])
                .arrayed(2, Pose::at([1.2, 0.0, 0.0])),
            "metal",
        ))
        .element(Element::new(
            "louvres",
            roof_bar([1.25, 0.065, 0.48], [-0.625, 0.3, -0.24])
                .arrayed(3, Pose::at([0.0, 0.12, 0.0])),
            "trim",
        ))
        .build()
}

/// A closed rooftop stair head, shared by the tenements' service decks.
fn roof_access() -> Result<Part, ValidationError> {
    Part::builder("city:roof-access")
        .element(
            Element::new(
                "body",
                plate([2.4, 2.5, 2.2], [-1.2, 0.0, -1.1]),
                "concrete",
            )
            .uv(UvMode::Box)
            .collision(Collision::Bounds),
        )
        .element(Element::new(
            "cap",
            plate([2.6, 0.14, 2.4], [-1.3, 2.45, -1.2]),
            "metal",
        ))
        .element(
            Element::new(
                "door",
                plate([1.05, 2.05, 0.09], [-0.525, 0.06, -1.15]),
                "metal",
            )
            .standalone(),
        )
        .element(Element::new(
            "handle",
            roof_bar([0.035, 0.28, 0.08], [0.32, 0.95, -1.2]),
            "trim",
        ))
        .element(Element::new(
            "vent",
            roof_bar([0.7, 0.045, 0.08], [-0.35, 1.63, -1.19])
                .arrayed(4, Pose::at([0.0, 0.09, 0.0])),
            "trim",
        ))
        .build()
}

// ------------------------------------------------------------------ plans

/// A frame tower's tiers, widest first: eighteen by sixteen at its foot,
/// which leaves a six-metre alley to its neighbour, stepping in twice.
pub(super) const TOWER: [Plan; 3] = [
    Plan {
        name: "tower-18x16",
        width: 18.0,
        depth: 16.0,
        chamfer: 1.0,
        core: [5.0, 4.0],
        style: Style::Frame,
        tier: 0,
    },
    Plan {
        name: "tower-14x12",
        width: 14.0,
        depth: 12.0,
        chamfer: 0.8,
        core: [4.4, 3.6],
        style: Style::Frame,
        tier: 1,
    },
    Plan {
        name: "tower-10x10",
        width: 10.0,
        depth: 10.0,
        chamfer: 0.8,
        core: [3.6, 3.0],
        style: Style::Frame,
        tier: 2,
    },
];

/// A banded tower's tiers: the same foot, then slabs that keep most of their
/// width and lose their depth, sixteen by eleven and twelve by eight, with
/// tighter corners.
pub(super) const BANDED: [Plan; 3] = [
    Plan {
        name: "slab-18x16",
        width: 18.0,
        depth: 16.0,
        chamfer: 1.0,
        core: [5.0, 4.0],
        style: Style::Band,
        tier: 0,
    },
    Plan {
        name: "slab-16x11",
        width: 16.0,
        depth: 11.0,
        chamfer: 0.6,
        core: [4.6, 3.2],
        style: Style::Band,
        tier: 1,
    },
    Plan {
        name: "slab-12x8",
        width: 12.0,
        depth: 8.0,
        chamfer: 0.6,
        core: [4.0, 2.6],
        style: Style::Band,
        tier: 2,
    },
];

/// A caged tower's tiers: the same foot, then slim square shafts, thirteen
/// metres and nine, with wide corners.
pub(super) const CAGE: [Plan; 3] = [
    Plan {
        name: "cage-18x16",
        width: 18.0,
        depth: 16.0,
        chamfer: 1.0,
        core: [5.0, 4.0],
        style: Style::Cage,
        tier: 0,
    },
    Plan {
        name: "cage-13x13",
        width: 13.0,
        depth: 13.0,
        chamfer: 1.2,
        core: [4.2, 3.6],
        style: Style::Cage,
        tier: 1,
    },
    Plan {
        name: "cage-9x9",
        width: 9.0,
        depth: 9.0,
        chamfer: 1.0,
        core: [3.4, 3.0],
        style: Style::Cage,
        tier: 2,
    },
];

/// A landmark's tiers: a whole lot's frame tower, thirty metres at its foot.
pub(super) const SPIRE: [Plan; 3] = [
    Plan {
        name: "spire-30x26",
        width: 30.0,
        depth: 26.0,
        chamfer: 1.6,
        core: [8.0, 6.0],
        style: Style::Frame,
        tier: 0,
    },
    Plan {
        name: "spire-22x20",
        width: 22.0,
        depth: 20.0,
        chamfer: 1.2,
        core: [6.0, 5.0],
        style: Style::Frame,
        tier: 1,
    },
    Plan {
        name: "spire-14x14",
        width: 14.0,
        depth: 14.0,
        chamfer: 1.0,
        core: [4.4, 4.0],
        style: Style::Frame,
        tier: 2,
    },
];

/// A tenement block: thirty-eight by twelve, a few storeys, fire escapes down
/// its front.
pub(super) const TENEMENT: Plan = Plan {
    name: "tenement-38x12",
    width: 38.0,
    depth: 12.0,
    chamfer: 0.6,
    core: [6.0, 3.6],
    style: Style::Frame,
    tier: 0,
};

/// Double-height arrival and transfer floors belong only to the landmark.
pub(super) const LANDMARK_LOBBY: f64 = 9.5;
pub(super) const SKY_HEIGHT: f64 = 7.6;
const HERO_CROWN_HEIGHT: f64 = 28.0;

fn landmark_portico(part: super::PartBuilder, height: f64) -> super::PartBuilder {
    let columns = [3.5, 9.0, 20.4, 25.9].map(|x| {
        Geometry::extrude(
            [[0.0, 0.0], [0.6, 0.0], [0.48, 0.75], [0.12, 0.75]],
            height - 0.7,
        )
        .placed(Pose::at([x, 0.15, -4.2]))
    });
    let feet = [3.5, 9.0, 20.4, 25.9]
        .map(|x| Geometry::cuboid([0.9, 0.35, 1.05]).placed(Pose::at([x - 0.15, 0.0, -4.35])));
    let mut part = part;
    for (index, column) in columns.into_iter().enumerate() {
        part = part.element(
            Element::new(format!("portico-column-{index}"), column, "clad")
                .uv(UvMode::Box)
                .collision(Collision::Hull),
        );
    }
    part.element(Element::new(
        "portico-feet",
        Geometry::union_all(feet),
        "metal",
    ))
    .element(
        Element::new(
            "portico-roof",
            Geometry::cuboid([25.2, 0.65, 4.8]).placed(Pose::at([2.4, height - 0.85, -4.7])),
            "concrete",
        )
        .uv(UvMode::Box),
    )
    .element(
        Element::new(
            "portico-fascia",
            Geometry::cuboid([24.8, 0.13, 0.04]).placed(Pose::at([2.6, height - 0.65, -4.74])),
            "crown",
        )
        .standalone()
        .far(),
    )
}

/// A tapering lantern carries the spire. Four deep ribs frame its glazing;
/// continuous lit blades and two collars make the form legible by night.
fn landmark_crown(id: &str, plan: &Plan) -> Result<Part, ValidationError> {
    let middle = plan.width / 2.0;
    let h = HERO_CROWN_HEIGHT;
    let taper = |half: f64, front: f64, back: f64, low: f64, high: f64| {
        Geometry::hull([low, high].into_iter().flat_map(|y| {
            let inset = 1.0 + 4.0 * y / h;
            [
                [middle - half, y, inset + front],
                [middle + half, y, inset + front],
                [middle - half, y, inset + back],
                [middle + half, y, inset + back],
            ]
        }))
    };
    let body = Geometry::hull([0.0, h].into_iter().flat_map(|y| {
        let inset = 1.15 + 4.0 * y / h;
        plan.inset(inset)
            .outline(0.0)
            .into_iter()
            .map(move |[x, z]| [x + inset, y, z + inset])
    }));
    let ribs = (0..4).map(|side| at(taper(0.85, -0.55, 0.25, 0.03, h + 0.04), plan.side(side)));
    let lights = (0..4).map(|side| at(taper(0.24, -0.6, -0.54, 1.5, h - 1.0), plan.side(side)));
    let [bottom, top] = stacking([middle, middle], h + 0.3);
    Part::builder(id)
        .element(Element::new("lantern-glass", body, "glass").standalone())
        .element(Element::new("lantern-ribs", Geometry::union_all(ribs), "clad").uv(UvMode::Box))
        .element(
            Element::new("lantern-lights", Geometry::union_all(lights), "crown")
                .standalone()
                .far(),
        )
        .element(Element::new(
            "lantern-base",
            plan.ring(-0.6, 1.2, 0.35, 1.0),
            "metal",
        ))
        .element(Element::new(
            "lantern-neck",
            at(
                Geometry::extrude(plan.inset(4.7).outline(0.0), 0.3),
                Pose::at([4.7, h, 4.7]),
            ),
            "metal",
        ))
        .socket(bottom)
        .socket(top)
        .build()
}

/// Forecourt strips converge on the portal, flanked by planted seating
/// islands. The route between them stays flush and open to the entrance.
fn landmark_forecourt() -> Result<Part, ValidationError> {
    let bar = |size, at| Geometry::cuboid(size).placed(Pose::at(at));
    let strips =
        (-5..=5).map(|i| bar([0.14, 0.015, 6.8], [f64::from(i) * 2.5 - 0.07, 0.02, -20.4]));
    let mut part = Part::builder("city:landmark-forecourt")
        .element(
            Element::new(
                "forecourt-paving",
                bar([29.0, 0.02, 7.4], [-14.5, 0.0, -20.6]),
                "plaza",
            )
            .uv(UvMode::Box),
        )
        .element(Element::new(
            "paving-inlays",
            Geometry::union_all(strips),
            "trim",
        ))
        .element(
            Element::new(
                "arrival-carpet",
                bar([4.4, 0.02, 6.8], [-2.2, 0.04, -20.3]),
                "dark",
            )
            .uv(UvMode::Box),
        );
    for (index, x) in [-11.0, 7.0].into_iter().enumerate() {
        part = part
            .element(Element::new(
                format!("forecourt-planter-{index}"),
                bar([4.0, 0.65, 1.2], [x, 0.035, -19.0]),
                "concrete",
            ))
            .element(Element::new(
                format!("forecourt-green-{index}"),
                bar([3.6, 0.4, 0.8], [x + 0.2, 0.685, -18.8]),
                "foliage",
            ))
            .element(Element::new(
                format!("forecourt-seat-{index}"),
                bar([3.6, 0.09, 0.5], [x + 0.2, 0.45, -19.45]),
                "metal",
            ));
    }
    part.build()
}

/// The landmark retains its crown part name, with a dedicated tapered lantern
/// in place of the standard stepped cap.
pub(super) const LANDMARK_CROWN: Crown = Crown::Stepped;

/// The part ids of a plan's storey, lobby, ceiling, roof and crowns.
pub(super) fn part_id(plan: &Plan, what: &str) -> String {
    format!("city:{}-{what}", plan.name)
}

/// Every storey, ceiling and roof of every plan, the lobbies, every crown of
/// every tower plan, and the toppers.
pub(super) fn parts() -> Result<Vec<Part>, ValidationError> {
    let mut parts = Vec::new();
    let towers: Vec<&Plan> = Style::ALL.iter().flat_map(|style| style.plans()).collect();
    for plan in towers.iter().copied().chain(&SPIRE).chain([&TENEMENT]) {
        parts.push(rect_storey(&part_id(plan, "floor"), plan, STOREY, false)?);
        parts.push(rect_ceiling(&part_id(plan, "ceiling"), plan)?);
        parts.push(rect_roof(&part_id(plan, "roof"), plan)?);
    }
    for plan in Style::ALL
        .iter()
        .map(|style| &style.plans()[0])
        .chain([&SPIRE[0], &TENEMENT])
    {
        parts.push(rect_storey(
            &part_id(plan, "lobby"),
            plan,
            if plan.name == SPIRE[0].name {
                LANDMARK_LOBBY
            } else {
                super::LOBBY
            },
            true,
        )?);
    }
    for plan in &SPIRE[..2] {
        parts.push(rect_storey(
            &part_id(plan, "sky-floor"),
            plan,
            SKY_HEIGHT,
            false,
        )?);
    }
    parts.push(landmark_forecourt()?);
    for plan in towers {
        for kind in Crown::ALL {
            parts.push(crown(&part_id(plan, kind.name()), plan, kind)?);
        }
    }
    parts.push(crown(
        &part_id(&SPIRE[2], LANDMARK_CROWN.name()),
        &SPIRE[2],
        LANDMARK_CROWN,
    )?);
    parts.extend([
        spire()?,
        mast()?,
        helipad()?,
        roof_plant()?,
        dish()?,
        antennas()?,
        roof_billboard()?,
        water_tank()?,
        roof_vent()?,
        roof_access()?,
    ]);
    Ok(parts)
}

/// The height of a roof slab, which is how far a crown or a topper stands
/// above the last storey.
pub(super) const ROOF_THICKNESS: f64 = ROOF;
