//! Periodicity inference: how often a field repeats across UV `[0, 1)`.
//!
//! This is the static half of the promise that a baked texture tiles. Every
//! port carries a [`Period`], inferred bottom-up from the generators, and the
//! material output must have one. The rules are per axis, because a wall of
//! eight rows of one column is a real thing to author.

use serde::{Deserialize, Serialize};

use crate::{
    Node,
    nodes::{CircleMap, Resolved, Transform, bond_period},
};

/// The largest period a bake can carry, being the largest bake resolution: a
/// repeat narrower than a texel is not a repeat.
pub const MAX_PERIOD: u32 = 4096;

/// How a field repeats across UV `[0, 1)`.
///
/// `Tiled { u: 8, v: 1 }` means the field is laid on a lattice of eight cells
/// along u and one along v, and wraps at the UV repeat: a texture baked from
/// it meets itself at both seams, and it is the same field again a whole unit
/// along. `Free` means it does not repeat at all, which is fine inside a graph
/// and an error at its output.
///
/// The count is that lattice, and not always a shift the field is invariant
/// under. Brickwork's cells are copies of one another, so a wall really is the
/// same wall `1 / v` along, which is why the bond divides the rows rather than
/// the rows standing as the period. A noise's cells are not copies: the eight
/// cells of a noise of period eight hash differently on purpose, and only the
/// whole unit brings it back. What a bake rests on is the wrap, which every
/// node satisfies; what the count is for is the lattice a transform scales and
/// a pointwise node lands on the multiple of.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Period {
    /// Repeats `u` times along u and `v` times along v.
    Tiled {
        /// Repeats along u.
        u: u32,
        /// Repeats along v.
        v: u32,
    },
    /// Does not repeat.
    Free,
}

impl Period {
    /// One repeat in each axis: what a constant, a parameter or a UV carries.
    ///
    /// This is also the neutral element of [`Self::lcm`], which is why a
    /// constant costs a pointwise node nothing.
    pub const UNIT: Self = Self::Tiled { u: 1, v: 1 };

    /// The same period in both axes.
    pub const fn square(repeats: u32) -> Self {
        Self::Tiled {
            u: repeats,
            v: repeats,
        }
    }

    /// Whether this field repeats at all.
    pub fn is_tiled(self) -> bool {
        matches!(self, Self::Tiled { .. })
    }

    /// The repeats per axis, or `None` when the field is free.
    pub fn repeats(self) -> Option<[u32; 2]> {
        match self {
            Self::Tiled { u, v } => Some([u, v]),
            Self::Free => None,
        }
    }

    /// The period a pointwise combination of the two has: the least common
    /// multiple per axis.
    ///
    /// Free with either side free, and free when the multiple runs past
    /// [`MAX_PERIOD`], because a repeat a texture cannot hold is not one.
    #[must_use]
    pub fn lcm(self, other: Self) -> Self {
        let (Some(left), Some(right)) = (self.repeats(), other.repeats()) else {
            return Self::Free;
        };
        match (lcm(left[0], right[0]), lcm(left[1], right[1])) {
            (Some(u), Some(v)) => Self::Tiled { u, v },
            _ => Self::Free,
        }
    }

    /// The two axes exchanged, as a quarter turn leaves them.
    fn transposed(self) -> Self {
        match self {
            Self::Tiled { u, v } => Self::Tiled { u: v, v: u },
            Self::Free => Self::Free,
        }
    }
}

impl std::fmt::Display for Period {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Tiled { u, v } => write!(f, "{u}x{v}"),
            Self::Free => f.write_str("free"),
        }
    }
}

/// The least common multiple, or `None` past what a bake can carry.
fn lcm(left: u32, right: u32) -> Option<u32> {
    let product = u64::from(left / gcd(left, right)) * u64::from(right);
    u32::try_from(product).ok().filter(|lcm| *lcm <= MAX_PERIOD)
}

fn gcd(mut left: u32, mut right: u32) -> u32 {
    while right != 0 {
        (left, right) = (right, left % right);
    }
    left.max(1)
}

/// What inference found out about one node.
pub(crate) struct Inferred {
    /// The period of the node's output.
    pub period: Period,
    /// A period the author probably did not mean, though it is legal.
    pub warning: Option<String>,
    /// Why this node is free, when no input of it was.
    pub reason: Option<String>,
}

impl Inferred {
    fn tiled(period: Period) -> Self {
        Self {
            period,
            warning: None,
            reason: None,
        }
    }

    fn free(reason: impl Into<String>) -> Self {
        Self {
            period: Period::Free,
            warning: None,
            reason: Some(reason.into()),
        }
    }
}

/// A node that keeps whatever period reached it.
pub(crate) fn passthrough(period: Period) -> Inferred {
    Inferred::tiled(period)
}

/// The period a *bound* [`GraphInput`](crate::nodes::GraphInput) carries: the
/// one the field an instance wired into it has.
///
/// This is the arm of [`node_period`] that the node alone cannot answer, so it
/// is answered by the build of the instance, which is the one place where the
/// binding is known. A field that does not tile keeps its freedom across the
/// boundary and says which input it came in through, because inside the
/// instanced graph there is no node to blame for it: what the author has to
/// change is out at the instance, and the message is the only thing that can
/// point there.
pub(crate) fn bound_input(name: &str, period: Period) -> Inferred {
    if period.is_tiled() {
        Inferred::tiled(period)
    } else {
        Inferred::free(format!("the field bound to input {name:?} does not tile"))
    }
}

/// The period of one node, given the periods that reached its inputs.
///
/// The rules, in one place:
///
/// - A generator tiles at its own integer period, per axis, which for
///   [`Bricks`](crate::nodes::Bricks) is the rows divided by the bond, for
///   [`Tiles`](crate::nodes::Tiles) the grid divided by the lattice, and for
///   [`Weave`](crate::nodes::Weave) the thread counts divided by the pattern's
///   repeat. The last two are facts about those nodes' own fields, so a count
///   the lattice or the pattern does not close over is refused at the node
///   rather than inferred free, exactly as a bond that does not divide is.
/// - A constant, a parameter and [`Uv`](crate::nodes::Uv) tile once, and so do
///   the runtime inputs — [`Time`](crate::nodes::Time),
///   [`WorldPos`](crate::nodes::WorldPos),
///   [`WorldNormal`](crate::nodes::WorldNormal) and
///   [`CutFlag`](crate::nodes::CutFlag) — which are not UV fields and so are
///   period-neutral rather than free, for the reason
///   [`Time`](crate::nodes::Time) writes out.
/// - A [`GraphInput`](crate::nodes::GraphInput) tiles once unbound, because
///   what it carries then is its default literal. Bound, it carries the period
///   of the field bound to it, which is not a question this function can
///   answer: the instanced graph is built again under the binding, and the
///   input resolves to the bound period there.
/// - A pointwise node tiles at the least common multiple of the inputs it
///   reads, and warns when that is larger than every one of them. An operand a
///   unary [`Math`](crate::nodes::Math) never reads is not one of them.
/// - [`Transform`](crate::nodes::Transform) multiplies by an integer scale and
///   exchanges the axes on a quarter turn; anything else is free, as is a
///   clamped transform that moves the coordinate at all.
/// - [`Warp`](crate::nodes::Warp) takes the multiple of its source and its
///   offset, which is its source's period whenever the offset divides it.
/// - [`Mirror`](crate::nodes::Mirror) folds the whole unit rather than the
///   lattice, so it keeps the period that reached it and is free only when
///   that was.
/// - [`Kaleidoscope`](crate::nodes::Kaleidoscope) is free unless it has four
///   sectors, whose fold tiles once whatever it folded, a free field included.
/// - [`CircleMap`](crate::nodes::CircleMap) tiles once whatever it read, for
///   the reason [`Shape`](crate::nodes::Shape) gives: what it draws is a disc
///   inside the repeat with a fill outside it, and the coordinate is wrapped
///   before the disc is measured. Its source has to tile all the same, and
///   `CircleMap::check_types` is where that is required.
/// - [`Tile`](crate::nodes::Tile) wraps its instances by construction and tiles
///   once, whatever it scattered — unless the mask that decides where its
///   instances appear does not tile, which is the one input of it that is read
///   across the whole repeat rather than inside a cell. A
///   [`CircleSplatter`](crate::nodes::CircleSplatter) is the same rule: it lays
///   its instances on a ring inside the repeat rather than on a grid across it,
///   but the mask is still the one input read outside an instance.
/// - [`DirectionalWarp`](crate::nodes::DirectionalWarp) takes the multiple of
///   its source and its angle field, as [`Warp`](crate::nodes::Warp) does.
/// - A buffered filter is wrapped, so it keeps its input's period. A
///   [`Blur`](crate::nodes::Blur) takes the multiple of the inputs its own kind
///   reads, which for everything but a slope blur is the field alone.
pub(crate) fn node_period(node: &Node, inputs: &[Resolved]) -> Inferred {
    let of = |name: &str| {
        inputs
            .iter()
            .find(|input| input.name == name)
            .map_or(Period::UNIT, |input| input.period)
    };
    match node {
        // A coordinate addresses one unit, a shape sits in one, scratches wrap
        // at its edges and a circle map draws a disc inside one with a fill
        // outside it: each of these lays one cell across the unit whatever
        // reached it. The circle map is here rather than under the resamplers
        // because that is what it is — the coordinate is wrapped before the
        // disc is measured, and the node's own check keeps the disc inside the
        // repeat, so what its source tiled at has no bearing on the count. That
        // the source tiles at all is required separately, by
        // `CircleMap::check_types`, because the angular axis wraps into it.
        Node::Uv(_) | Node::Shape(_) | Node::Scratches(_) | Node::CircleMap(_) => {
            Inferred::tiled(Period::UNIT)
        }
        // An unbound input is its default literal, which comes back to itself
        // once for the reason `UNBOUND_PERIOD` writes out. A bound one does not
        // reach this arm with its binding lost: the instance is built again
        // under the binding, and the input resolves to the bound period there.
        Node::GraphInput(_) => Inferred::tiled(crate::nodes::UNBOUND_PERIOD),
        // The runtime inputs are not fields over UV at all, so the question a
        // period answers — how many times does this come back to itself across
        // UV `[0, 1)` — has no bearing on them: at one instant, at one
        // fragment, each is the same number across the whole repeat, exactly as
        // a parameter is. So they carry the unit period a parameter carries,
        // which is the neutral element of the multiple below and therefore
        // costs a pointwise node nothing. Calling them free instead would
        // refuse every graph that reads one, on the strength of a question they
        // are not answers to. It is the same answer as the arm above for a
        // different reason, and it stays its own arm: merging the two would
        // file the runtime inputs under a rule about the coordinate.
        // And so are the two nodes built out of the world-space pair: a
        // triplanar addresses its source by world position rather than by UV,
        // so what its *source* tiles at says nothing about how often the
        // result comes back to itself across the repeat — the source's own
        // tiling is required by `Triplanar::check_types` instead, because that
        // is the seam the node exists to remove.
        Node::Time(_)
        | Node::WorldPos(_)
        | Node::WorldNormal(_)
        | Node::CutFlag(_)
        | Node::Triplanar(_)
        | Node::WorldMask(_) => Inferred::tiled(crate::nodes::runtime::WORLD_PERIOD),
        // The base lattice rather than the finest, as `Noise` documents: the
        // octaves above it sit on multiples of it, so the count every other
        // rule takes the multiple of is this one. The finest is
        // `period * lacunarity^(octaves - 1)`, which `Noise::check` holds under
        // `MAX_PERIOD` where the noise is authored. What a `Transform` does to
        // that finest octave is [`node_lattice`]'s to say, and it says it: the
        // lattice is carried through the graph and multiplied where a resampler
        // scales the frame, so a deep noise read through a scale is refused by
        // the bake's own resolution check rather than sampled.
        Node::Noise(noise) => Inferred::tiled(Period::Tiled {
            u: noise.period[0],
            v: noise.period[1],
        }),
        Node::Voronoi(voronoi) => Inferred::tiled(Period::Tiled {
            u: voronoi.period[0],
            v: voronoi.period[1],
        }),
        // The bond is what a wall repeats at in v: a running bond over eight
        // rows tiles four times, because row one is half a brick along from row
        // zero and only row two is back where row zero was. A bond that does
        // not divide the rows has no period in v at all, and the node's own
        // check rejected it before inference ran, so the free arm is a belt.
        Node::Bricks(bricks) => bond_period(bricks.offset, bricks.rows).map_or_else(
            || {
                Inferred::free(
                    "a bond that does not divide the rows never comes back to itself in v",
                )
            },
            |v| {
                Inferred::tiled(Period::Tiled {
                    u: bricks.columns,
                    v,
                })
            },
        ),
        Node::Tiles(tiles) => {
            let [u, v] = tiles.repeats();
            Inferred::tiled(Period::Tiled { u, v })
        }
        // The crossing is what a cloth repeats at, not the threads: a plain
        // weave of eight threads comes back to itself every second one, so it
        // tiles four times each way. The node's own check refused a thread
        // count the pattern does not divide, which is what makes the division
        // exact and the count above one.
        Node::Weave(weave) => {
            let [u, v] = weave.repeats();
            Inferred::tiled(Period::Tiled { u, v })
        }
        Node::Pattern(pattern) => Inferred::tiled(Period::Tiled {
            u: pattern.x,
            v: pattern.y,
        }),
        Node::Transform(transform) => transformed(of("input"), transform),
        Node::Mirror(_) => mirrored(of("input")),
        // A scatter wraps its instances by construction, so what it *drew*
        // cannot stop it tiling however freely the source ran. Its mask can:
        // that one is read at the centre of each cell, across the whole repeat,
        // and the cell a texel at `u = 1` lands in is a whole UV unit along
        // from the one at `u = 0` — the same value of any field that tiles, and
        // anybody's guess of one that does not.
        //
        // A circle splatter takes the same rule rather than a second one. Its
        // own arithmetic would in fact close over a free mask — the coordinate
        // is wrapped before the ring is measured, so every instance centre
        // lands inside the unit and is read at the same point in every repeat —
        // but a mask is what an author reaches for to say *where* a scatter
        // happens, a field the crate has refused to make a promise about is not
        // a place, and one rule for the two scatters is worth more than the
        // graphs this refuses. A ring that wants one reads it through something
        // that tiles, exactly as the output does.
        Node::Tile(_) | Node::CircleSplatter(_) => {
            if of("mask") == Period::Free {
                Inferred::free(
                    "a scatter reads its mask once per instance, across the whole repeat, so a \
                     mask that does not tile leaves the instances at the seam disagreeing with \
                     the ones a unit along",
                )
            } else {
                Inferred::tiled(Period::UNIT)
            }
        }
        // A unary operator never reads `b`, so `b` cannot move the lattice the
        // answer lands on any more than it can widen its type.
        Node::Math(math) if math.op.is_unary() => {
            let read: Vec<Resolved> = inputs
                .iter()
                .filter(|input| input.name == "a")
                .cloned()
                .collect();
            pointwise(&read)
        }
        // And the same for a blur, whose height is read by the slope kind
        // alone: a Gaussian over a graph that also holds a height would
        // otherwise take the multiple of a field it never emits.
        Node::Blur(blur) => {
            let read: Vec<Resolved> = inputs
                .iter()
                .filter(|input| blur.reads(&input.name))
                .cloned()
                .collect();
            pointwise(&read)
        }
        // The quadrant fold runs from 0 to 1 and back within each axis, so it
        // reads the same value at 1 as at 0 whatever reached it — a free source
        // included, which is why there is no guard on the input here.
        Node::Kaleidoscope(kaleidoscope) => {
            if kaleidoscope.count == 4 {
                Inferred::tiled(Period::UNIT)
            } else {
                Inferred::free(format!(
                    "only a four-sector kaleidoscope tiles; this one has {}",
                    kaleidoscope.count
                ))
            }
        }
        // A relief is one repeat of the layer it splats, splatted wrapped, so
        // it comes back to itself once across UV whatever lattice the roots sit
        // on. The layer's own `count` is not this node's to carry: a strand
        // layer is a root of the material in its own right, and `strand_ports`
        // has already joined that count into the material's repeat — from the
        // layer, where it is declared, rather than from however many nodes
        // happen to read it.
        Node::StrandRelief(_) => Inferred::tiled(Period::UNIT),
        _ => pointwise(inputs),
    }
}

/// The finest lattice one node's output lands on, per axis.
///
/// Not [`Period`], which is the count a field comes back to itself at and grows
/// through every least common multiple downstream: a blend of a lattice of four
/// with one of six has a period of twelve and a finest cell of a sixth, and it
/// is the sixth that a bake needs texels for. This is that cell count, and a
/// bake takes the largest over the whole graph.
///
/// A generator answers the lattice it lays. A noise answers its *finest* octave
/// rather than its base lattice, because the octaves are what a bake has to
/// resolve; `Noise::check` holds that product under [`MAX_PERIOD`] where the
/// noise is authored, so it is a count a resolution can be compared against.
///
/// Everything else carries what reached it, because a resampler changes how
/// fine a lattice lands and reading each node alone would miss it:
///
/// - [`Transform`](crate::nodes::Transform) multiplies by its scale, rounded
///   *up*, with the axes exchanged on an odd quarter turn. Up rather than to
///   nearest, because this is a count of texels to be at least: a scale of 1.5
///   over a lattice of eight lays cells no coarser than a twelfth.
/// - [`Tile`](crate::nodes::Tile) multiplies its *source* by its instance
///   count, an instance being the whole source shrunk into one cell, and takes
///   its mask's lattice as it stands, a mask being read at the cells rather
///   than inside them.
/// - A four-sector [`Kaleidoscope`](crate::nodes::Kaleidoscope) doubles both
///   axes, because the quadrant fold runs the source's unit twice across the
///   result's. The angular fold turns the frame rather than scaling it, and
///   carries the lattice through unchanged.
/// - [`CircleSplatter`](crate::nodes::CircleSplatter) squeezes its whole source
///   into one instance `scale` wide, so it answers `ceil(source / scale)` per
///   axis — or its mask's count where that is finer, a mask being read at the
///   instances rather than inside them, as a [`Tile`](crate::nodes::Tile)'s is.
/// - [`CircleMap`](crate::nodes::CircleMap) answers one count in both axes,
///   because it mixes them: the source's u is laid `turns` times round the
///   ring and its v is squeezed into a window `radius - inner` wide, and the
///   finer of the two decides how many texels the whole disc wants. There is
///   no axis of the result that only one of them lands on.
/// - Everything else — a blend, a warp, a mirror, a buffered filter — answers
///   the largest lattice that reached it. A mirror in particular keeps the count
///   it folded, for the reason [`mirrored`] gives about the period.
///
/// A [`Shape`](crate::nodes::Shape) and a [`Scratches`](crate::nodes::Scratches)
/// lay one: what they draw is an instance in the repeat rather than a lattice,
/// and the resolution they want is a question about their own edges. So does an
/// unbound [`GraphInput`](crate::nodes::GraphInput), which is a literal; a bound
/// one lays the lattice of the field bound to it, which the instance learns
/// when it is built again under that binding.
pub(crate) fn node_lattice(node: &Node, inputs: &[Resolved]) -> [u32; 2] {
    let widest = || {
        inputs.iter().fold([1, 1], |finest, input| {
            [
                finest[0].max(input.lattice[0]),
                finest[1].max(input.lattice[1]),
            ]
        })
    };
    let of = |name: &str| {
        inputs
            .iter()
            .find(|input| input.name == name)
            .map_or([1, 1], |input| input.lattice)
    };
    match node {
        Node::Noise(noise) => {
            let factor = noise
                .lacunarity
                .saturating_pow(noise.octaves.saturating_sub(1));
            [
                noise.period[0].saturating_mul(factor),
                noise.period[1].saturating_mul(factor),
            ]
        }
        Node::Voronoi(voronoi) => voronoi.period,
        // A literal lays no lattice at all, and unbound that is what an input
        // is. The bound case is answered where the binding is known, which is
        // the build of the instance rather than here.
        //
        // A strand relief lays one too, and for a different reason that has the
        // same answer: the strands themselves *are* a lattice a resolution has
        // to resolve, and this node names its layer rather than declaring one,
        // so it does not hold the count. What resolves it instead is the check
        // the splat makes at bake time, where both the layer's width and the
        // resolution are known and the answer can be said in millimetres — see
        // `strand_lattice_warnings`.
        Node::GraphInput(_) | Node::StrandRelief(_) => [1, 1],
        // The cells of a wall, which is the rows rather than the bond the wall
        // repeats at: two rows of a running bond are two rows to rasterise
        // whether or not the second is the first moved along.
        Node::Bricks(bricks) => [bricks.columns, bricks.rows],
        Node::Tiles(tiles) => [tiles.columns, tiles.rows],
        // The threads themselves rather than the crossing they repeat at: two
        // threads of a plain weave are two threads to rasterise whether or not
        // the second crosses the way the first did.
        Node::Weave(weave) => [weave.x, weave.y],
        Node::Pattern(pattern) => [pattern.x, pattern.y],
        Node::Transform(transform) => {
            let source = of("input");
            let turned = if quarter_turns(transform.rotate).is_some_and(|turns| turns % 2 == 1) {
                [source[1], source[0]]
            } else {
                source
            };
            [
                scaled(turned[0], transform.scale[0]),
                scaled(turned[1], transform.scale[1]),
            ]
        }
        Node::Tile(tile) => {
            let source = of("input");
            let mask = of("mask");
            // The source is shrunk into a cell and so lays its lattice `count`
            // times over; the mask is read at the cells themselves and lays its
            // own, unscaled. A bake needs texels for whichever is finer.
            [
                source[0].saturating_mul(tile.count[0]).max(mask[0]),
                source[1].saturating_mul(tile.count[1]).max(mask[1]),
            ]
        }
        Node::Kaleidoscope(kaleidoscope) if kaleidoscope.count == 4 => {
            let source = of("input");
            [source[0].saturating_mul(2), source[1].saturating_mul(2)]
        }
        Node::CircleMap(map) => circle_lattice(map, of("input")),
        Node::CircleSplatter(splatter) => {
            let source = of("input");
            let mask = of("mask");
            // One instance is the source's whole unit drawn `scale` across, so
            // what the source laid over a repeat it now lays over that fraction
            // of one. The mask is read at the instances rather than inside
            // them and lays its own, unsqueezed.
            [
                squeezed(source[0], splatter.scale).max(mask[0]),
                squeezed(source[1], splatter.scale).max(mask[1]),
            ]
        }
        _ => widest(),
    }
}

/// The lattice a windowed polar read lands on, in both axes.
///
/// The source's u goes round the ring `turns` times, so that axis lays its
/// count times the turns. Its v is squeezed into the window, so a source of
/// eight cells read across a window a tenth of the repeat wide lays those eight
/// cells in a tenth and wants eighty across the repeat. The node mixes the two
/// axes — the ring runs across both u and v of the result — so the answer is
/// the finer of them in both, and there is no axis a bake could resolve less
/// finely.
fn circle_lattice(map: &CircleMap, source: [u32; 2]) -> [u32; 2] {
    let angular = source[0].saturating_mul(map.turns);
    let radial = squeezed(source[1].saturating_mul(map.rings), map.radius - map.inner);
    let finest = angular.max(radial);
    [finest, finest]
}

/// A lattice squeezed into a window: `ceil(lattice / span)`, never below one.
///
/// Up rather than to nearest, for the reason [`scaled`] gives, and a span no
/// float can make sense of — the node's own check keeps it above zero, so this
/// is a belt — answers a count past [`MAX_PERIOD`], which refuses the bake
/// rather than passing it.
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "the bound above the cast is what makes it exact"
)]
fn squeezed(lattice: u32, span: f32) -> u32 {
    let past = MAX_PERIOD.saturating_mul(2);
    if !span.is_finite() || span <= 0.0 {
        return past;
    }
    let cells = (f64::from(lattice) / f64::from(span)).ceil();
    if cells >= f64::from(past) {
        return past;
    }
    (cells as u32).max(1)
}

/// A lattice through a scale: rounded up, and never below one.
///
/// Up because what this count is for is deciding whether a bake has texels
/// enough, and a cell rounded down is a cell the bake would sample rather than
/// write. A scale no float can make sense of — a zero is refused at the node and
/// an infinity by `finite`, so this is a belt — answers a count past
/// [`MAX_PERIOD`], which refuses the bake rather than passing it.
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "the bound above the cast is what makes it exact"
)]
fn scaled(lattice: u32, scale: f32) -> u32 {
    let factor = scale.abs().ceil();
    if !factor.is_finite() {
        return MAX_PERIOD.saturating_mul(2);
    }
    if factor < 1.0 {
        return lattice.max(1);
    }
    if factor > f32::from(u16::MAX) {
        return MAX_PERIOD.saturating_mul(2);
    }
    lattice.saturating_mul(factor as u32)
}

/// How many quarter turns a rotation is, or `None` where it is not a whole
/// number of them.
fn quarter_turns(degrees: f32) -> Option<u32> {
    let turns = (degrees / 90.0).round();
    if (degrees - turns * 90.0).abs() > 1e-3 {
        return None;
    }
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "rem_euclid leaves it in 0..4"
    )]
    Some(turns.rem_euclid(4.0) as u32)
}

/// The least common multiple of every input, with the warning the design asks
/// for when the result is larger than any of them.
fn pointwise(inputs: &[Resolved]) -> Inferred {
    let mut period = Period::UNIT;
    for input in inputs {
        period = period.lcm(input.period);
    }
    let Some([u, v]) = period.repeats() else {
        let free: Vec<&str> = inputs
            .iter()
            .filter(|input| input.period == Period::Free)
            .map(|input| input.name.as_ref())
            .collect();
        return Inferred::free(if free.is_empty() {
            format!(
                "the periods of {} have no common multiple a bake can carry",
                list(inputs)
            )
        } else {
            format!("input {} does not tile", free.join(" and "))
        });
    };
    let largest = inputs
        .iter()
        .filter_map(|input| input.period.repeats())
        .fold([1, 1], |[au, av], [bu, bv]| [au.max(bu), av.max(bv)]);
    let warning = (u > largest[0] || v > largest[1]).then(|| {
        format!(
            "period {u}x{v} is larger than every input ({}); one repeat of this graph now spans \
             several of theirs",
            list(inputs)
        )
    });
    Inferred {
        period,
        warning,
        reason: None,
    }
}

fn list(inputs: &[Resolved]) -> String {
    inputs
        .iter()
        .map(|input| format!("{}: {}", input.name, input.period))
        .collect::<Vec<_>>()
        .join(", ")
}

/// A transform keeps a period only where it is an integer change of one.
fn transformed(input: Period, transform: &Transform) -> Inferred {
    let Transform {
        scale,
        rotate,
        translate,
        repeat,
        ..
    } = *transform;
    // A clamped transform only keeps a period where it reads the source where
    // it already was, so a translation counts as much as a scale does.
    let spin = rotate.rem_euclid(360.0);
    let identity = (scale[0] - 1.0).abs() < 1e-6
        && (scale[1] - 1.0).abs() < 1e-6
        && spin.min(360.0 - spin) < 1e-6
        && translate[0].abs() < 1e-6
        && translate[1].abs() < 1e-6;
    if !repeat && !identity {
        return Inferred::free(
            "a clamped transform reads the source outside its repeat, where it no longer meets \
             itself; set `repeat` to keep a period",
        );
    }
    if input == Period::Free {
        return Inferred::free("a transform of a free field is free");
    }
    let Some(quarter) = quarter_turns(rotate) else {
        return Inferred::free(format!(
            "a rotation of {rotate} degrees is not a quarter turn, so the lattice no longer meets \
             itself"
        ));
    };
    let rotated = if quarter % 2 == 1 {
        input.transposed()
    } else {
        input
    };
    let (Some(factors), Some([u, v])) = (integer_scale(scale), rotated.repeats()) else {
        return Inferred::free(format!(
            "a scale of {}x{} is not an integer number of repeats",
            scale[0], scale[1]
        ));
    };
    match (lcm_free(u, factors[0]), lcm_free(v, factors[1])) {
        (Some(u), Some(v)) => Inferred::tiled(Period::Tiled { u, v }),
        _ => Inferred::free(format!(
            "scaling {rotated} by {}x{} runs past the {MAX_PERIOD} repeats a bake can carry",
            scale[0], scale[1]
        )),
    }
}

/// A scale is only allowed to multiply a period by a whole number of repeats.
fn integer_scale(scale: [f32; 2]) -> Option<[u32; 2]> {
    let mut factors = [0; 2];
    for (factor, value) in factors.iter_mut().zip(scale) {
        let magnitude = value.abs();
        let rounded = magnitude.round();
        // 4096.0 is MAX_PERIOD: past it nothing tiles anyway.
        if (magnitude - rounded).abs() > 1e-4 || !(1.0..=4096.0).contains(&rounded) {
            return None;
        }
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "the bounds check above put it in 1..=MAX_PERIOD"
        )]
        {
            *factor = rounded as u32;
        }
    }
    Some(factors)
}

/// Multiplying a period by a scale, bounded the way [`Period::lcm`] is.
fn lcm_free(period: u32, factor: u32) -> Option<u32> {
    let product = u64::from(period) * u64::from(factor);
    u32::try_from(product)
        .ok()
        .filter(|scaled| *scaled <= MAX_PERIOD)
}

/// A mirror folds the whole unit, so it keeps whatever lattice it folded.
///
/// The fold on the axis is `f(t) = 0.5 - |0.5 - fract(t)|`, which is continuous
/// and repeats once: the folded axis meets itself at the seam whatever reached
/// it, and the source's cell boundaries land on the result at the spacing they
/// had, reflected about the middle. So the count survives, rather than halving
/// as a fold that paired adjacent repeats would — that one meets itself only
/// where the cells are copies of one another, which [`Period`] says a lattice
/// generator does not promise. [`Mirror`](crate::nodes::Mirror) carries the
/// same in its own words, since a lowering has to match it.
///
/// A free field stays free. The fold gives the axis it folded a wrap, but it
/// leaves the other axis exactly as free as it found it, and a [`Period`] is
/// free in both axes or in neither.
fn mirrored(input: Period) -> Inferred {
    if input == Period::Free {
        return Inferred::free(
            "a mirror folds one axis and leaves the other, so a field that does not tile does \
             not tile after it either",
        );
    }
    Inferred::tiled(input)
}

#[cfg(test)]
mod tests {
    use super::{MAX_PERIOD, Period, gcd, integer_scale, lcm};

    #[test]
    fn the_multiple_of_two_periods_is_the_smallest_one_both_divide() {
        assert_eq!(lcm(8, 12), Some(24));
        assert_eq!(lcm(4, 8), Some(8));
        assert_eq!(lcm(1, 7), Some(7));
        assert_eq!(lcm(MAX_PERIOD, MAX_PERIOD), Some(MAX_PERIOD));
        // Two large coprime periods multiply past anything a bake can hold,
        // and the product must not wrap around into a small one.
        assert_eq!(lcm(2048, 3), None);
        assert_eq!(lcm(u32::MAX, u32::MAX - 1), None);
        // Validation rejects a period of zero before inference sees it, but
        // the arithmetic underneath must not divide by it regardless.
        assert_eq!(gcd(0, 0), 1);
        assert_eq!(lcm(0, 0), Some(0));
    }

    #[test]
    fn only_a_whole_number_of_repeats_is_a_scale_a_period_survives() {
        assert_eq!(integer_scale([1.0, 3.0]), Some([1, 3]));
        assert_eq!(
            integer_scale([-2.0, 2.0]),
            Some([2, 2]),
            "a flip is a scale"
        );
        assert_eq!(integer_scale([2.000_01, 1.0]), Some([2, 1]), "within slop");
        assert_eq!(integer_scale([1.5, 1.0]), None);
        assert_eq!(
            integer_scale([0.5, 1.0]),
            None,
            "magnification loses the wrap"
        );
        assert_eq!(integer_scale([f32::NAN, 1.0]), None);
        assert_eq!(integer_scale([1e30, 1.0]), None);
    }

    #[test]
    fn a_free_period_swallows_everything_it_touches() {
        assert_eq!(Period::Free.transposed(), Period::Free);
        assert_eq!(
            Period::Tiled { u: 2, v: 9 }.transposed(),
            Period::Tiled { u: 9, v: 2 }
        );
        assert_eq!(Period::UNIT.to_string(), "1x1");
        assert_eq!(Period::Free.to_string(), "free");
    }
}
