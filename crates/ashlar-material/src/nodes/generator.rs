//! Generators: the nodes that make a field out of nothing.
//!
//! Every generator takes an integer period and wraps its lattice at it, which
//! is what makes the result tile. A seed varies the pattern without varying
//! the period.

use serde::{Deserialize, Serialize};

use crate::ir::{Lower, Lowering, NodeInputs, Op, ValueId};
use crate::nodes::frame::{
    TURN, about_centre, abs, add, affine, axes, cell_hash, clamp, complement, cos, count, div, dot,
    floor, fract, inward_ramp, k, k2, length, max, min, mirror_fold, mix, mul, normalize, polar,
    quadrant_fold, select, signed_hash, sin, smoothstep, step, sub, vec2,
};
use crate::{
    GraphError, MAX_PERIOD, Value, ValueType, finite,
    nodes::{Check, Output, ports},
    require,
};

/// The integer period of a generator, checked against the bake's limits.
pub(crate) fn check_period(period: [u32; 2], path: &str) -> Result<(), GraphError> {
    for (axis, value) in ["u", "v"].into_iter().zip(period) {
        require(
            (1..=MAX_PERIOD).contains(&value),
            &format!("{path}.period"),
            &format!("period in {axis} must be in 1..={MAX_PERIOD}"),
        )?;
    }
    Ok(())
}

/// A count of repeats in one axis, which is a period by another name.
pub(crate) fn check_count(count: u32, field: &str, path: &str) -> Result<(), GraphError> {
    require(
        (1..=MAX_PERIOD).contains(&count),
        &format!("{path}.{field}"),
        &format!("{field} must be in 1..={MAX_PERIOD}"),
    )
}

/// How many rows a bond takes to come back to where it started.
///
/// Row `r` of a bond is shifted along u by `r * offset` brick widths, and a
/// shift of a whole brick is the identity, because the wall already repeats
/// once per column. So the wall is invariant under a shift of `k` rows exactly
/// where `k * offset` is a whole number of bricks, and the smallest such `k` is
/// what this returns: one for a stack bond, two for the running half bond,
/// three for a third bond. `None` where no `k` within `rows` does, which says
/// the bond never comes back inside the UV repeat.
///
/// The slop an offset authored as a decimal needs is spent on the *wall*
/// rather than on one bond. A step that lands a ten-thousandth of a brick off
/// a whole one lands that far off once per bond, and `rows / k` times as far
/// off by the time the wall reaches its v seam, so a per-step slop is no slop
/// at all at a large row count: an offset of `0.0001` over four thousand rows
/// would pass a per-step test at every `k`, claim a period, and lay its top row
/// four tenths of a brick along from its bottom one. What is bounded here is
/// the drift the seam actually sees, which leaves an authored fraction —
/// `1.0 / 3.0` is no third in `f32` either — comfortably inside, and a decimal
/// that only looks like one outside.
fn bond_denominator(offset: f32, rows: u32) -> Option<u32> {
    let offset = f64::from(offset).rem_euclid(1.0);
    (1..=rows).find(|step| {
        let shift = f64::from(*step) * offset;
        let drift = (shift - shift.round()).abs();
        drift * f64::from(rows) / f64::from(*step) <= BOND_SLOP
    })
}

/// How far a bond may be off a whole brick by the time it reaches the v seam.
///
/// A ten-thousandth of a brick, which is under half a texel even where one
/// brick runs the whole width of a four-thousand-texel texture, and so is a
/// rounding rather than a line in the picture.
const BOND_SLOP: f64 = 1e-4;

/// How many times a wall of `rows` rows at this bond repeats in v.
///
/// `None` where the bond does not divide the wall, in which case the row above
/// the v seam disagrees with row zero by part of a brick and the wall does not
/// tile at all.
pub(crate) fn bond_period(offset: f32, rows: u32) -> Option<u32> {
    let bond = bond_denominator(offset, rows)?;
    rows.is_multiple_of(bond).then(|| rows / bond)
}

/// The parity a lattice that alternates needs, in one axis.
///
/// A layout whose cells alternate — a hex row offset, a herringbone weave —
/// pairs each cell with its neighbour, so it needs an even count to close, and
/// one pair is one repeat.
pub(crate) fn divisible(
    count: u32,
    cells: u32,
    field: &str,
    what: &str,
    path: &str,
) -> Result<(), GraphError> {
    require(
        count.is_multiple_of(cells),
        &format!("{path}.{field}"),
        &format!(
            "{what} comes back to itself every {cells} {field}, and {count} of them cannot be \
             divided that way; use a count divisible by {cells}"
        ),
    )
}

/// A unit interval parameter: finite, and where the node has no meaning outside it.
pub(crate) fn check_unit(value: f32, field: &str, path: &str) -> Result<(), GraphError> {
    let path = format!("{path}.{field}");
    finite(value, &path, field)?;
    require(
        (0.0..=1.0).contains(&value),
        &path,
        &format!("{field} must be in 0..=1"),
    )
}

/// The input coordinate, in UV space.
///
/// One repeat of the material is `0..1` in both axes, so this is the field
/// every other generator is written against.
///
/// Its period is `1x1`, which says that the lattice it addresses repeats once,
/// **not** that the value meets itself at the seam: `u` is 1 where `u` is 0,
/// and a ramp is discontinuous there. That is fine where a generator wraps the
/// coordinate itself, which is what every generator here does, and it is a seam
/// where a coordinate is used as a value — a [`Warp`](crate::nodes::Warp)
/// offset built from a bare `Uv` displaces by a different amount on each side
/// of the wrap. Build such an offset from a wrapping field instead, or from
/// `Uv` through something periodic in it, such as
/// [`Math`](crate::nodes::Math)'s `Sin`, which takes turns. The periodicity
/// property test carves out the bare coordinate for this reason.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[must_use]
pub struct Uv;

impl Uv {
    /// The input coordinate.
    pub fn new() -> Self {
        Self
    }
}

ports!(Uv, Output::Fixed(ValueType::Vec2));
impl Check for Uv {}

/// Which noise, and therefore what the lattice carries.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub enum NoiseKind {
    /// A random value per lattice cell, smoothed. The cheapest, and blockiest.
    #[default]
    Value,
    /// Gradients per lattice corner. The usual choice for cloud and stone.
    Perlin,
    /// Simplex gradients: the same character without the axis-aligned artefacts.
    Simplex,
}

/// Fractal noise on a wrapping integer lattice.
///
/// The lattice wraps at `period`, so the gradients at `u = 0` and
/// `u = period` are the same gradients and the field tiles by construction.
/// Octaves are summed at `lacunarity` times the frequency and `persistence`
/// times the amplitude; a fractional lacunarity would break the wrap, so it is
/// an integer. The sum is divided by the amplitude the octaves carry between
/// them, so the field stays in `0..=1` however many are asked for and a single
/// octave is the lattice itself.
///
/// Two things about that sum are visible from outside. Octave `k` hashes on
/// `seed + k`, because cell zero is cell zero in every octave and would
/// otherwise carry one number in all of them; the cost is that two noises are
/// correlated where their seeds and lattices line up, so `period(4).octaves(2)`
/// shares its fine octave with `period(8).seed(1)` exactly. And `period` is the
/// coarsest lattice rather than the finest: the octaves run up to
/// `period * lacunarity.pow(octaves - 1)`, which is the count validation holds
/// under [`MAX_PERIOD`](crate::MAX_PERIOD), while the inferred period — what a
/// transform scales and a pointwise node takes the multiple of — is the base.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
#[must_use]
pub struct Noise {
    /// Which noise.
    pub kind: NoiseKind,
    /// Lattice repeats across UV `[0, 1)`, in u and v.
    pub period: [u32; 2],
    /// Varies the pattern without varying the period.
    pub seed: u32,
    /// How many frequencies are summed. One is a single lattice.
    pub octaves: u32,
    /// Amplitude factor per octave.
    pub persistence: f32,
    /// Frequency factor per octave. Integer, or the lattice stops wrapping.
    pub lacunarity: u32,
}

impl Default for Noise {
    fn default() -> Self {
        Self {
            kind: NoiseKind::Value,
            period: [1, 1],
            seed: 0,
            octaves: 1,
            persistence: 0.5,
            lacunarity: 2,
        }
    }
}

impl Noise {
    /// A random value per lattice cell.
    pub fn value() -> Self {
        Self::of(NoiseKind::Value)
    }

    /// Gradient noise.
    pub fn perlin() -> Self {
        Self::of(NoiseKind::Perlin)
    }

    /// Simplex gradient noise.
    pub fn simplex() -> Self {
        Self::of(NoiseKind::Simplex)
    }

    fn of(kind: NoiseKind) -> Self {
        Self {
            kind,
            ..Self::default()
        }
    }

    /// Set the same lattice period in both axes.
    pub fn period(mut self, period: u32) -> Self {
        self.period = [period; 2];
        self
    }

    /// Set the lattice period per axis.
    pub fn periods(mut self, u: u32, v: u32) -> Self {
        self.period = [u, v];
        self
    }

    /// Vary the pattern at the same period.
    pub fn seed(mut self, seed: u32) -> Self {
        self.seed = seed;
        self
    }

    /// Sum this many frequencies.
    pub fn octaves(mut self, octaves: u32) -> Self {
        self.octaves = octaves;
        self
    }

    /// Set the amplitude factor per octave.
    pub fn persistence(mut self, persistence: f32) -> Self {
        self.persistence = persistence;
        self
    }

    /// Set the integer frequency factor per octave.
    pub fn lacunarity(mut self, lacunarity: u32) -> Self {
        self.lacunarity = lacunarity;
        self
    }
}

ports!(Noise, Output::Fixed(ValueType::Float));

impl Check for Noise {
    fn check(&self, path: &str) -> Result<(), GraphError> {
        check_period(self.period, path)?;
        require(
            (1..=12).contains(&self.octaves),
            &format!("{path}.octaves"),
            "octaves must be in 1..=12",
        )?;
        require(
            (1..=8).contains(&self.lacunarity),
            &format!("{path}.lacunarity"),
            "lacunarity must be in 1..=8",
        )?;
        check_unit(self.persistence, "persistence", path)?;
        // The finest octave is a lattice too, and it has to fit in a texture.
        let finest = u64::from(self.period[0].max(self.period[1]))
            * u64::from(self.lacunarity).pow(self.octaves - 1);
        require(
            finest <= u64::from(MAX_PERIOD),
            path,
            &format!(
                "the finest octave lands at period {finest}, past the {MAX_PERIOD} a bake can carry"
            ),
        )
    }
}

/// How distance is measured between a texel and a cell point.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub enum VoronoiMetric {
    /// Straight-line distance: round cells.
    #[default]
    Euclidean,
    /// Sum of axis distances: diamond cells.
    Manhattan,
    /// Largest axis distance: square cells.
    Chebyshev,
}

/// Which field a [`Voronoi`] presents.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub enum VoronoiOutput {
    /// Distance to the nearest cell point.
    #[default]
    Distance,
    /// A random float per cell, for per-cell colour variation.
    Cell,
    /// Distance to the boundary between the two nearest cells.
    Edge,
    /// One inside a band along that boundary, zero elsewhere.
    Border,
    /// The vector from the texel to its own cell point, as a
    /// [`ValueType::Vec2`], **in UV**.
    ///
    /// The search already computes this on its way to the distance and throws
    /// it away, so it costs nothing beyond the node itself. What it is for is
    /// gathering: added to a coordinate it pulls every texel of a cell onto one
    /// point, which is what makes a tuft of grass out of a scatter and a clump
    /// out of wet fur.
    ///
    /// In UV rather than in the cell units [`Self::Distance`] answers, because
    /// nothing in the vocabulary scales a `Vec2` — [`Math`](crate::nodes::Math)
    /// takes a float or a colour — so an offset in cell units could only be
    /// brought back to a displacement by taking it apart and putting it
    /// together again. A displacement is what every port that accepts one
    /// measures in, and this is one.
    Offset,
}

impl VoronoiOutput {
    /// What this output carries. Every field of a [`Voronoi`] is one channel
    /// except the offset, which is a displacement.
    pub fn value_type(self) -> ValueType {
        match self {
            Self::Distance | Self::Cell | Self::Edge | Self::Border => ValueType::Float,
            Self::Offset => ValueType::Vec2,
        }
    }
}

/// Cellular noise on a wrapping integer lattice.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
#[must_use]
pub struct Voronoi {
    /// Cells across UV `[0, 1)`, in u and v.
    pub period: [u32; 2],
    /// Varies the cell points without varying the period.
    pub seed: u32,
    /// How distance is measured.
    pub metric: VoronoiMetric,
    /// How far a cell point may stray from its cell centre, in `0..=1`.
    pub jitter: f32,
    /// How wide the band of [`VoronoiOutput::Border`] is, in cell units: the
    /// distance from a cell boundary at which the border has faded to nothing.
    /// It is in cell units rather than UV because what it draws is a line
    /// around a cell, and a cell is what it should stay in proportion to as the
    /// period is authored coarser or finer.
    pub width: f32,
    /// Which field to present.
    pub output: VoronoiOutput,
}

impl Default for Voronoi {
    fn default() -> Self {
        Self {
            period: [4, 4],
            seed: 0,
            metric: VoronoiMetric::default(),
            jitter: 1.0,
            width: 0.05,
            output: VoronoiOutput::default(),
        }
    }
}

impl Voronoi {
    /// Cellular noise at the default four cells per axis.
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the same cell count in both axes.
    pub fn period(mut self, period: u32) -> Self {
        self.period = [period; 2];
        self
    }

    /// Set the cell count per axis.
    pub fn periods(mut self, u: u32, v: u32) -> Self {
        self.period = [u, v];
        self
    }

    /// Vary the cell points at the same period.
    pub fn seed(mut self, seed: u32) -> Self {
        self.seed = seed;
        self
    }

    /// Choose how distance is measured.
    pub fn metric(mut self, metric: VoronoiMetric) -> Self {
        self.metric = metric;
        self
    }

    /// Set how far a cell point may stray from its centre.
    pub fn jitter(mut self, jitter: f32) -> Self {
        self.jitter = jitter;
        self
    }

    /// Set the width of the border band, in cell units.
    pub fn width(mut self, width: f32) -> Self {
        self.width = width;
        self
    }

    /// Choose which field to present.
    pub fn output(mut self, output: VoronoiOutput) -> Self {
        self.output = output;
        self
    }
}

ports!(Voronoi, |node| Output::Fixed(node.output.value_type()));

impl Check for Voronoi {
    fn check(&self, path: &str) -> Result<(), GraphError> {
        check_period(self.period, path)?;
        check_unit(self.jitter, "jitter", path)?;
        check_unit(self.width, "width", path)
    }
}

/// Which field a [`Bricks`] or [`Tiles`] presents.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub enum BrickOutput {
    /// One on a brick face and zero in the mortar, with a hard edge between
    /// them. A graph that wants the mortar instead reads this through
    /// [`Invert`](crate::nodes::Invert).
    #[default]
    Mask,
    /// The bevelled height of the brick face.
    Bevel,
    /// A random float per brick, for per-brick tint and wear.
    Id,
    /// The brick's own UV along one axis, for detail inside a face.
    Fill,
}

/// Running-bond brickwork.
///
/// `columns` bricks run along u and `rows` stack in v. The mortar is taken off
/// every side of a brick, so a wall has vertical joints as well as horizontal
/// ones, and narrowing it to one column does not remove them: eight rows of one
/// column is a wall whose single joint per row runs down it, moved along by the
/// bond, and not a panel with eight horizontal seams. A band in one axis alone
/// is [`Uv`] through [`Math`](crate::nodes::Math) — `fract(v * rows)`, centred,
/// through `Abs` and a `Step` — which is what a concrete panel's joint is
/// authored from.
///
/// The bond decides the period in v, and is written down here because that rule
/// rests on it. Row `r` is shifted along u by `r * offset` brick widths, and a
/// shift of a whole brick changes nothing, because the wall already repeats
/// once per column. The wall therefore comes back to itself every `bond` rows,
/// where `bond` is the smallest number of rows whose accumulated offset is a
/// whole number of bricks — one for a stack bond, two for the default running
/// half bond, three for a third bond — and the period is
/// `{ u: columns, v: rows / bond }`. A running bond over eight rows tiles
/// **four** times in v, not eight.
///
/// `bond` must divide `rows`, or the row above the v seam disagrees with row
/// zero by part of a brick and the wall does not tile at all: three rows at the
/// default half offset is rejected at `offset`, rather than inferred free,
/// because it is a fact about this node's own fields.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
#[must_use]
pub struct Bricks {
    /// Brick rows across UV `[0, 1)`. The period in v is these divided by the
    /// bond `offset` sets.
    pub rows: u32,
    /// Brick columns across UV `[0, 1)`: the period in u.
    pub columns: u32,
    /// Row-to-row offset in brick widths, in `0..=1`. A half is a running
    /// bond, zero a stack bond, and the denominator must divide `rows`. Write
    /// the fraction rather than a decimal that resembles one: what has to come
    /// back to a whole brick is the shift the wall has accumulated by its v
    /// seam, so an offset that is a little off walks across the wall and is
    /// refused here.
    pub offset: f32,
    /// Mortar taken off every side of a brick, in brick-local units where one
    /// brick is `1x1`: over eight rows, `0.007` is that fraction of a row
    /// rather than of the texture, so a seam keeps its width as the wall is
    /// authored coarser or finer. It is taken off both ends of both axes, so it
    /// must be under `0.5`, which is where the two sides meet and the face
    /// vanishes.
    pub mortar: f32,
    /// How far the brick face falls away at its edge. The mask is hard-edged
    /// by construction, so this shapes [`BrickOutput::Bevel`] alone.
    pub bevel: f32,
    /// How round the brick's corners are, in UV. Phase two: the mask cuts a
    /// rectangular face, and a wall that rounds it is refused by path rather
    /// than squared quietly.
    pub round: f32,
    /// How much the corner is cut back, in UV. Phase two, as `round` is.
    pub corner: f32,
    /// Varies the per-brick id without varying the bond.
    pub seed: u32,
    /// Which field to present.
    pub output: BrickOutput,
}

impl Default for Bricks {
    fn default() -> Self {
        Self {
            rows: 4,
            columns: 2,
            offset: 0.5,
            mortar: 0.02,
            bevel: 0.05,
            round: 0.0,
            corner: 0.0,
            seed: 0,
            output: BrickOutput::default(),
        }
    }
}

impl Bricks {
    /// A running bond at the default four rows of two.
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the number of rows, which the bond divides into the period in v.
    pub fn rows(mut self, rows: u32) -> Self {
        self.rows = rows;
        self
    }

    /// Set the number of columns, which is the period in u.
    pub fn columns(mut self, columns: u32) -> Self {
        self.columns = columns;
        self
    }

    /// Set the row-to-row offset in brick widths, whose denominator is what
    /// divides the rows into the period in v.
    pub fn offset(mut self, offset: f32) -> Self {
        self.offset = offset;
        self
    }

    /// Set the mortar taken off every side of a brick, in brick-local units,
    /// which must be under `0.5` or nothing is left of the face.
    pub fn mortar(mut self, mortar: f32) -> Self {
        self.mortar = mortar;
        self
    }

    /// Set how far the face falls away at the edge.
    pub fn bevel(mut self, bevel: f32) -> Self {
        self.bevel = bevel;
        self
    }

    /// Round the corners.
    pub fn round(mut self, round: f32) -> Self {
        self.round = round;
        self
    }

    /// Cut the corners back.
    pub fn corner(mut self, corner: f32) -> Self {
        self.corner = corner;
        self
    }

    /// Vary the per-brick id.
    pub fn seed(mut self, seed: u32) -> Self {
        self.seed = seed;
        self
    }

    /// Choose which field to present.
    pub fn output(mut self, output: BrickOutput) -> Self {
        self.output = output;
        self
    }
}

ports!(Bricks, Output::Fixed(ValueType::Float));

impl Check for Bricks {
    fn check(&self, path: &str) -> Result<(), GraphError> {
        check_count(self.rows, "rows", path)?;
        check_count(self.columns, "columns", path)?;
        check_unit(self.offset, "offset", path)?;
        require(
            bond_period(self.offset, self.rows).is_some(),
            &format!("{path}.offset"),
            &format!(
                "a bond that offsets every row by {} of a brick does not come back to a whole \
                 brick within {} rows, so the wall does not meet itself at the v seam; offset \
                 by a fraction whose denominator divides the row count, such as 0.5 over an \
                 even number of rows, and write it as a fraction rather than as a decimal that \
                 drifts across the wall",
                self.offset, self.rows
            ),
        )?;
        check_unit(self.mortar, "mortar", path)?;
        // Both sides of both axes take it off a cell one unit wide, so at a
        // half there is no face left and the mask is zero everywhere. That is
        // a blank texture with nothing to look at, so it is a path instead.
        require(
            self.mortar < 0.5,
            &format!("{path}.mortar"),
            &format!(
                "mortar of {} is taken off both sides of a brick one unit wide, which leaves \
                 no face at all; mortar must be under 0.5",
                self.mortar
            ),
        )?;
        check_unit(self.bevel, "bevel", path)?;
        check_unit(self.round, "round", path)?;
        check_unit(self.corner, "corner", path)
    }
}

/// Which tiling a [`Tiles`] lays out.
///
/// The lattice each one lays over the `columns` by `rows` grid of cells is
/// written down here, because it is what the periods rest on.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub enum TilePattern {
    /// A square grid: one tile per cell, so the lattice is the grid itself.
    #[default]
    Grid,
    /// Hexagons, as a grid of cells whose odd rows are offset half a cell along
    /// u. That is a running bond by another name: rows pair up, so `rows` must
    /// be even and one repeat in v is two rows.
    Hex,
    /// Herringbone: two-cell rectangles laid at right angles to one another,
    /// marching diagonally, which comes back to itself every **four** cells in
    /// each axis rather than every two. Both `rows` and `columns` must divide
    /// by four, and one repeat is four of each.
    ///
    /// Four, not two, is a fact about the weave rather than a margin: a
    /// two-by-two block of cells holds two of these rectangles, and the only
    /// ways to lay two in it are side by side or stacked, both of which are a
    /// bond rather than a weave. The diagonal march that makes it a herringbone
    /// takes four. The lowering writes the rule out.
    Herringbone,
}

impl TilePattern {
    /// How many cells of the grid one repeat of the lattice spans, in u and v.
    fn cells_per_repeat(self) -> [u32; 2] {
        match self {
            Self::Grid => [1, 1],
            Self::Hex => [1, 2],
            Self::Herringbone => [4, 4],
        }
    }
}

/// A regular tiling with the same outputs as brickwork.
///
/// `rows` and `columns` count cells of the grid the pattern is laid over; the
/// period is that grid divided by how many cells one repeat of the lattice
/// spans, which [`TilePattern`] names per pattern. A grid is its own lattice
/// and tiles `columns` by `rows`; a hex or a herringbone weave spans two cells
/// and needs an even count to close, which is checked rather than inferred
/// free, because it is a fact about this node's own fields.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
#[must_use]
pub struct Tiles {
    /// Which tiling, and so how many cells one repeat of it spans.
    pub pattern: TilePattern,
    /// Cell rows across UV `[0, 1)`. The period in v is these divided by the
    /// cells the lattice spans in v.
    pub rows: u32,
    /// Cell columns across UV `[0, 1)`, divided likewise for the period in u.
    pub columns: u32,
    /// Gap between tiles, in UV.
    pub gap: f32,
    /// How far the tile face falls away at its edge.
    pub bevel: f32,
    /// Varies the per-tile id.
    pub seed: u32,
    /// Which field to present.
    pub output: BrickOutput,
}

impl Default for Tiles {
    fn default() -> Self {
        Self {
            pattern: TilePattern::default(),
            rows: 4,
            columns: 4,
            gap: 0.02,
            bevel: 0.05,
            seed: 0,
            output: BrickOutput::default(),
        }
    }
}

impl Tiles {
    /// A grid at the default four by four.
    pub fn new() -> Self {
        Self::default()
    }

    /// Choose the tiling.
    pub fn pattern(mut self, pattern: TilePattern) -> Self {
        self.pattern = pattern;
        self
    }

    /// Set the number of cell rows, which the lattice divides into the period
    /// in v.
    pub fn rows(mut self, rows: u32) -> Self {
        self.rows = rows;
        self
    }

    /// Set the number of cell columns, divided likewise for the period in u.
    pub fn columns(mut self, columns: u32) -> Self {
        self.columns = columns;
        self
    }

    /// Set the gap between tiles.
    pub fn gap(mut self, gap: f32) -> Self {
        self.gap = gap;
        self
    }

    /// Set how far the face falls away at the edge.
    pub fn bevel(mut self, bevel: f32) -> Self {
        self.bevel = bevel;
        self
    }

    /// Vary the per-tile id.
    pub fn seed(mut self, seed: u32) -> Self {
        self.seed = seed;
        self
    }

    /// Choose which field to present.
    pub fn output(mut self, output: BrickOutput) -> Self {
        self.output = output;
        self
    }
}

impl Tiles {
    /// How often the lattice repeats across UV `[0, 1)`, in u and v.
    ///
    /// The grid divided by the cells one repeat of the lattice spans, which is
    /// a whole number because [`Check`] rejected a count the lattice does not
    /// close over.
    pub(crate) fn repeats(&self) -> [u32; 2] {
        let cells = self.pattern.cells_per_repeat();
        [self.columns / cells[0], self.rows / cells[1]]
    }
}

ports!(Tiles, Output::Fixed(ValueType::Float));

impl Check for Tiles {
    fn check(&self, path: &str) -> Result<(), GraphError> {
        check_count(self.rows, "rows", path)?;
        check_count(self.columns, "columns", path)?;
        match self.pattern {
            TilePattern::Grid => {}
            TilePattern::Hex => divisible(self.rows, 2, "rows", "a hexagonal lattice", path)?,
            TilePattern::Herringbone => {
                divisible(self.columns, 4, "columns", "a herringbone weave", path)?;
                divisible(self.rows, 4, "rows", "a herringbone weave", path)?;
            }
        }
        check_unit(self.gap, "gap", path)?;
        // The gap opens on both sides of every boundary, so half of it comes
        // off each tile: a gap as wide as a cell leaves no face at all, which
        // is a blank texture with nothing to look at.
        for (axis, cells) in [("u", self.columns), ("v", self.rows)] {
            require(
                self.gap * count(cells) < 1.0,
                &format!("{path}.gap"),
                &format!(
                    "a gap of {} across {cells} cells in {axis} is as wide as a tile, which \
                     leaves no face at all; a gap must be under one over the cell count",
                    self.gap
                ),
            )?;
        }
        check_unit(self.bevel, "bevel", path)
    }
}

/// Which wave a [`Pattern`] runs along each axis.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub enum PatternKind {
    /// A hard edge halfway through the repeat.
    #[default]
    Stripes,
    /// The two axes exclusive-ored into a checkerboard.
    Checker,
    /// A sine wave.
    Sine,
    /// A triangle wave.
    Triangle,
    /// A square wave.
    Square,
}

/// How the two axis waves of a [`Pattern`] combine.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub enum PatternMix {
    /// The product, which darkens where either does.
    #[default]
    Multiply,
    /// The sum, clamped.
    Add,
    /// The larger.
    Max,
    /// The smaller.
    Min,
    /// The mean.
    Average,
    /// The absolute difference, which is what makes a checker a checker.
    Difference,
}

/// The cheap regular patterns, one wave per axis.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
#[must_use]
pub struct Pattern {
    /// Which wave.
    pub kind: PatternKind,
    /// Repeats across UV `[0, 1)` in u.
    pub x: u32,
    /// Repeats across UV `[0, 1)` in v.
    pub y: u32,
    /// How the two axes combine.
    pub mix: PatternMix,
}

impl Default for Pattern {
    fn default() -> Self {
        Self {
            kind: PatternKind::default(),
            x: 1,
            y: 1,
            mix: PatternMix::default(),
        }
    }
}

impl Pattern {
    /// A pattern of one kind, at one repeat per axis.
    ///
    /// A checker comes with [`PatternMix::Difference`], because that is the
    /// combination that makes one; the rest come with the product.
    pub fn new(kind: PatternKind) -> Self {
        let mix = match kind {
            PatternKind::Checker => PatternMix::Difference,
            _ => PatternMix::default(),
        };
        Self {
            kind,
            mix,
            ..Self::default()
        }
    }

    /// Set the repeats in u.
    pub fn x(mut self, x: u32) -> Self {
        self.x = x;
        self
    }

    /// Set the repeats in v.
    pub fn y(mut self, y: u32) -> Self {
        self.y = y;
        self
    }

    /// Choose how the two axes combine.
    pub fn mix(mut self, mix: PatternMix) -> Self {
        self.mix = mix;
        self
    }
}

ports!(Pattern, Output::Fixed(ValueType::Float));

impl Check for Pattern {
    fn check(&self, path: &str) -> Result<(), GraphError> {
        check_count(self.x, "x", path)?;
        check_count(self.y, "y", path)?;
        // A checker is the exclusive-or of its two axes, and the exclusive-or
        // of two fields in `0..=1` is their difference. Any other combination
        // of the two square waves is a grid of bars, which is what `Stripes`
        // is for, so this is refused rather than quietly answered: a field
        // ignored is the one failure a path cannot be read off the picture for.
        require(
            self.kind != PatternKind::Checker || self.mix == PatternMix::Difference,
            &format!("{path}.mix"),
            &format!(
                "a checker is the difference of its two axes; {:?} would answer a grid of bars \
                 instead, which is what the Stripes kind draws",
                self.mix
            ),
        )
    }
}

/// Which shape a [`Shape`] draws.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub enum ShapeKind {
    /// A disc.
    #[default]
    Circle,
    /// A square.
    Box,
    /// A regular polygon with `sides` sides.
    Polygon,
    /// A star with `sides` points.
    Star,
    /// A stadium: the segment of `length` lying along u, with a cap of radius
    /// `size` at each end of it. A slot, a bolt shank, a brushed streak.
    Capsule,
    /// A cogwheel: a disc of radius `size` with `sides` teeth standing between
    /// it and a root circle `depth` further in.
    Gear,
}

/// Which field a [`Shape`] presents.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub enum ShapeOutput {
    /// One inside the shape and zero outside it, with `edge` the width of the
    /// falloff taken inward from the boundary.
    #[default]
    Mask,
    /// The signed distance to the boundary in UV, negative inside and positive
    /// outside, neither clamped nor ramped: `edge` does not touch it. This is
    /// the field a boolean between two shapes combines and a ramp then turns
    /// back into a mask, and it is why the mask above is a ramp over a distance
    /// rather than a shape drawn twice.
    Distance,
}

/// One signed-distance shape centred in the repeat. Combine with [`Tile`](super::Tile).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
#[must_use]
pub struct Shape {
    /// Which shape.
    pub kind: ShapeKind,
    /// Radius or half-width in UV.
    pub size: f32,
    /// Width of the falloff at the edge, in UV. Zero is a hard edge. Read by
    /// [`ShapeOutput::Mask`] alone.
    pub edge: f32,
    /// Sides for a polygon, points for a star, teeth for a gear. Ignored
    /// otherwise.
    pub sides: u32,
    /// The distance between a [`ShapeKind::Capsule`]'s two cap centres, in UV,
    /// so the capsule reaches `length / 2 + size` along u and `size` across it.
    /// Ignored otherwise.
    pub length: f32,
    /// How deep a [`ShapeKind::Gear`]'s teeth are cut, in UV: the teeth reach
    /// `size` and the root circle between them stands at `size - depth`, so the
    /// depth is spent inside the radius the shape already promised rather than
    /// added to it. Ignored otherwise.
    pub depth: f32,
    /// Corner radius in UV, at most `size`. A circle, a capsule and a gear have
    /// no corner for it to take and ignore it: the first two are arc already,
    /// and a gear's distance is radial rather than Euclidean, so drawing it
    /// smaller by a radius and moving the distance back out cancels exactly.
    /// What softens a tooth's corner is the ramp across its own flank.
    pub round: f32,
    /// Shell thickness in UV. Zero is the solid shape; anything more keeps the
    /// band running from the boundary to `hollow` inside it and drops the rest,
    /// which makes an annulus of a circle and a frame of a box. The shell is
    /// taken inward, as the edge falloff is, so a hollow shape reaches exactly
    /// as far as the solid one did.
    pub hollow: f32,
    /// Which field to present.
    pub output: ShapeOutput,
}

impl Default for Shape {
    fn default() -> Self {
        Self {
            kind: ShapeKind::default(),
            size: 0.4,
            edge: 0.05,
            sides: 6,
            length: 0.5,
            depth: 0.1,
            round: 0.0,
            hollow: 0.0,
            output: ShapeOutput::default(),
        }
    }
}

impl Shape {
    /// A shape of one kind, centred in the repeat.
    ///
    /// Every kind but the capsule starts at the same radius, because `size` is
    /// the radius each of them reaches and the repeat is the only thing that
    /// bounds it. A capsule reaches `length / 2 + size` along u instead, so the
    /// shared default would push its caps through the seam before an author had
    /// set anything; it starts narrow enough that the default length fits, and
    /// an author who widens it shortens it.
    pub fn new(kind: ShapeKind) -> Self {
        let defaults = Self::default();
        let size = match kind {
            ShapeKind::Capsule => 0.15,
            _ => defaults.size,
        };
        Self {
            kind,
            size,
            ..defaults
        }
    }

    /// Set the radius or half-width.
    pub fn size(mut self, size: f32) -> Self {
        self.size = size;
        self
    }

    /// Set the falloff width at the edge.
    pub fn edge(mut self, edge: f32) -> Self {
        self.edge = edge;
        self
    }

    /// Set the number of sides, points or teeth.
    pub fn sides(mut self, sides: u32) -> Self {
        self.sides = sides;
        self
    }

    /// Set the distance between a capsule's cap centres.
    pub fn length(mut self, length: f32) -> Self {
        self.length = length;
        self
    }

    /// Set how deep a gear's teeth are cut.
    pub fn depth(mut self, depth: f32) -> Self {
        self.depth = depth;
        self
    }

    /// Set the corner radius.
    pub fn round(mut self, round: f32) -> Self {
        self.round = round;
        self
    }

    /// Set the shell thickness, which hollows the shape out.
    pub fn hollow(mut self, hollow: f32) -> Self {
        self.hollow = hollow;
        self
    }

    /// Choose which field to present.
    pub fn output(mut self, output: ShapeOutput) -> Self {
        self.output = output;
        self
    }
}

ports!(Shape, Output::Fixed(ValueType::Float));

impl Check for Shape {
    fn check(&self, path: &str) -> Result<(), GraphError> {
        check_unit(self.size, "size", path)?;
        check_unit(self.edge, "edge", path)?;
        check_unit(self.length, "length", path)?;
        check_unit(self.depth, "depth", path)?;
        check_unit(self.round, "round", path)?;
        check_unit(self.hollow, "hollow", path)?;
        require(
            !matches!(
                self.kind,
                ShapeKind::Polygon | ShapeKind::Star | ShapeKind::Gear
            ) || self.sides >= 3,
            &format!("{path}.sides"),
            "a polygon, a star or a gear needs at least three sides",
        )?;
        // The rounding is the usual one — the shape is drawn `round` smaller
        // and the distance is then moved out by `round` — so a corner radius
        // wider than the shape would ask for a negative shape and answer a
        // field with no boundary in it at all.
        require(
            self.round <= self.size,
            &format!("{path}.round"),
            &format!(
                "a corner radius of {} is wider than the shape of size {} it rounds, and the \
                 rounding draws the shape that much smaller before it grows the distance back",
                self.round, self.size
            ),
        )?;
        // A gear's teeth are cut inward from `size`, so a depth past it would
        // put the root circle behind the centre.
        require(
            self.kind != ShapeKind::Gear || self.depth <= self.size,
            &format!("{path}.depth"),
            &format!(
                "a gear of size {} cannot cut teeth {} deep, because the root circle between them \
                 stands at size less depth and that is inside out",
                self.size, self.depth
            ),
        )?;
        // A shape is centred in the repeat and its period is one, which is a
        // claim about the seam: a shape that reached the edge would be cut
        // there and would not meet what is on the other side, because the far
        // side of a repeat is the same shape's near side and not its far one.
        // `size` is the radius the shape reaches — the half-width of a box, the
        // vertex of a polygon or the point of a star — so this is the whole of
        // what makes the period true.
        require(
            self.size + self.edge <= 0.5,
            &format!("{path}.size"),
            &format!(
                "a shape of size {} with an edge of {} reaches past the repeat it is centred \
                 in, so it would be cut at the seam; size and edge must add to at most a half, \
                 and a shape that should run off the edge is a Transform of a smaller one",
                self.size, self.edge
            ),
        )?;
        // A capsule is the one kind whose reach is not `size`: it is a segment
        // with a cap at each end, so it runs half its length further along u
        // and the same rule has to be read against that number instead.
        require(
            self.kind != ShapeKind::Capsule || self.length * 0.5 + self.size + self.edge <= 0.5,
            &format!("{path}.length"),
            &format!(
                "a capsule of length {} with caps of radius {} and an edge of {} reaches {} along \
                 u, which is past the repeat it is centred in and would be cut at the seam; half \
                 the length, the radius and the edge must add to at most a half",
                self.length,
                self.size,
                self.edge,
                self.length * 0.5 + self.size + self.edge
            ),
        )
    }
}

/// Randomised line segments across the repeat, wrapped at its edges.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
#[must_use]
pub struct Scratches {
    /// How many segments.
    pub count: u32,
    /// Segment length in UV.
    pub length: f32,
    /// Segment width in UV.
    pub width: f32,
    /// Mean direction in degrees.
    pub angle: f32,
    /// How far a segment may turn from the mean, in degrees.
    pub angle_spread: f32,
    /// Varies the segments.
    pub seed: u32,
}

impl Default for Scratches {
    fn default() -> Self {
        Self {
            count: 32,
            length: 0.2,
            width: 0.004,
            angle: 0.0,
            angle_spread: 180.0,
            seed: 0,
        }
    }
}

impl Scratches {
    /// The default scatter of thirty-two short segments.
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the number of segments.
    pub fn count(mut self, count: u32) -> Self {
        self.count = count;
        self
    }

    /// Set the segment length.
    pub fn length(mut self, length: f32) -> Self {
        self.length = length;
        self
    }

    /// Set the segment width.
    pub fn width(mut self, width: f32) -> Self {
        self.width = width;
        self
    }

    /// Set the mean direction in degrees.
    pub fn angle(mut self, degrees: f32) -> Self {
        self.angle = degrees;
        self
    }

    /// Set how far a segment may turn from the mean.
    pub fn angle_spread(mut self, degrees: f32) -> Self {
        self.angle_spread = degrees;
        self
    }

    /// Vary the segments.
    pub fn seed(mut self, seed: u32) -> Self {
        self.seed = seed;
        self
    }
}

ports!(Scratches, Output::Fixed(ValueType::Float));

impl Check for Scratches {
    fn check(&self, path: &str) -> Result<(), GraphError> {
        require(
            (1..=4096).contains(&self.count),
            &format!("{path}.count"),
            "count must be in 1..=4096",
        )?;
        check_unit(self.length, "length", path)?;
        check_unit(self.width, "width", path)?;
        finite(self.angle, &format!("{path}.angle"), "angle")?;
        finite(
            self.angle_spread,
            &format!("{path}.angle_spread"),
            "angle spread",
        )?;
        // A segment is drawn from the cells it could reach the texel from, and
        // that neighbourhood is what a scratch costs. Past three cells out it
        // is forty-nine segment tests a texel and climbing.
        let reach = scratch_reach(self);
        require(
            reach <= MAX_SCRATCH_REACH,
            &format!("{path}.length"),
            &format!(
                "{} scratches sit on a lattice of {} cells a side, and one {} long reaches {reach} \
                 cells across it, which is more neighbours than a texel may search; make them \
                 shorter, or make more of them so that the lattice is finer",
                self.count,
                scratch_grid(self.count),
                self.length,
            ),
        )
    }
}

/// How the two sets of threads in a [`Weave`] cross one another.
///
/// Every pattern here is one rule read at one crossing. Number the warp
/// threads along u and the weft threads along v; the warp is on top where the
/// residue of `warp - move * weft - offset`, modulo the pattern's repeat,
/// falls under the float. What tells the three apart is how wide that float is
/// and how far its start moves from one weft to the next — over one and under
/// one, a float of half the repeat marching a thread a row, or a float of all
/// but one thread whose single binding point is scattered instead.
///
/// The repeat is why a thread count has to divide by it. Thread `repeat`
/// crosses the way thread zero did, so a cloth whose count is not a multiple
/// of the repeat meets a different crossing on the far side of the seam;
/// [`Weave`]'s own check refuses that, the way [`Bricks`] refuses a bond that
/// does not divide its rows, because it is a fact about the node's own fields.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub enum WeavePattern {
    /// Over one and under one: the plainest cloth there is, and a
    /// checkerboard of crossings. Its repeat is two threads in each axis.
    #[default]
    Plain,
    /// A diagonal wale: the float is half the repeat and its start moves one
    /// thread along per weft, which is denim, gabardine and most of what reads
    /// as woven from any distance.
    Twill {
        /// How many threads the diagonal takes to come back to itself, at
        /// least three. The float is half of it, rounded down, so four is the
        /// two-over-two of a denim and three the one-over-two of a lighter
        /// cloth. Two would be the plain weave, and is refused by name.
        step: u32,
    },
    /// One binding point per thread and one per weft, scattered rather than
    /// laid in a line, so what shows is a surface of long floats with a sheen
    /// on it and no diagonal in it at all.
    Satin {
        /// How many threads the pattern takes to come back to itself. The move
        /// between one binding point and the next is chosen from this rather
        /// than authored, because a move that shares a factor with the repeat
        /// binds some threads twice and others never; a repeat that admits no
        /// move at all is four threads and six, and both are refused rather
        /// than laid as the twill they would come out as.
        step: u32,
    },
}

/// The crossing rule of one [`WeavePattern`], as the four numbers the check
/// and the lowering both read it out of.
///
/// The warp is on top at the crossing of thread `i` and weft `j` exactly where
/// `(i - move_by * j - offset)` modulo `repeat` is under `float`. Every pattern
/// is that one expression at different numbers, which is what lets one lowering
/// draw all three and one rule decide the period of all three.
#[derive(Clone, Copy, Debug)]
struct Crossing {
    /// How many threads the pattern takes to come back to itself, in each
    /// axis, which is what a thread count has to divide by.
    repeat: u32,
    /// How far the float's start moves from one weft to the next, in threads.
    move_by: u32,
    /// Where the float starts on weft zero, in threads.
    offset: u32,
    /// How many of the repeat's threads the warp floats over.
    float: u32,
}

/// How far a satin's binding point moves from one weft to the next, in
/// threads, or `None` where its repeat has no such move.
///
/// A satin binds every thread exactly once per repeat, which needs a move that
/// shares no factor with the repeat, and it is a satin rather than a twill
/// only where that move is neither one nor one short of the repeat — either of
/// those puts the binding points on a diagonal, which is the thing a satin
/// exists not to have. The largest move under half the repeat is the usual
/// choice and the one taken here: two over five threads, three over seven or
/// eight, four over nine. It is chosen rather than authored because a move
/// that divides wrong is not a look, it is a cloth with threads that never
/// bind.
///
/// Two, three, four and six admit no such move, and no other repeat does
/// not: under five there is no move to pick that is not one or one short of
/// the repeat, and six shares a factor with both of the moves it could take.
fn satin_move(repeat: u32) -> Option<u32> {
    (2..=repeat / 2)
        .rev()
        .find(|move_by| coprime(*move_by, repeat))
}

/// Whether two counts share no factor but one.
fn coprime(left: u32, right: u32) -> bool {
    let (mut left, mut right) = (left, right);
    while right != 0 {
        (left, right) = (right, left % right);
    }
    left == 1
}

impl WeavePattern {
    /// How many threads one repeat of this crossing spans, in each axis.
    ///
    /// This is the count a [`Weave`]'s thread counts have to divide by, and it
    /// is the same number in both axes because the rule is read off the
    /// difference of the two indices: a pattern that comes back to itself
    /// every `n` threads along comes back every `n` wefts up as well.
    #[must_use]
    pub fn repeat(self) -> u32 {
        match self {
            Self::Plain => 2,
            Self::Twill { step } | Self::Satin { step } => step,
        }
    }

    /// The crossing rule this pattern names, or `None` where it has none,
    /// which is a satin whose repeat admits no move of its own.
    fn crossing(self) -> Option<Crossing> {
        match self {
            Self::Plain => Some(Crossing {
                repeat: 2,
                move_by: 1,
                offset: 0,
                float: 1,
            }),
            // Half the repeat, rounded down, so an odd repeat floats over the
            // smaller half and the cloth is weft-faced rather than balanced.
            Self::Twill { step } => Some(Crossing {
                repeat: step,
                move_by: 1,
                offset: 0,
                float: step / 2,
            }),
            // All but one thread, and the offset of one is what puts that one
            // binding point at residue zero rather than at the far end of the
            // float, so the move scatters the points rather than the floats.
            Self::Satin { step } => satin_move(step).map(|move_by| Crossing {
                repeat: step,
                move_by,
                offset: 1,
                float: step - 1,
            }),
        }
    }

    /// What to call this pattern in an error.
    fn describe(self) -> &'static str {
        match self {
            Self::Plain => "a plain weave",
            Self::Twill { .. } => "a twill",
            Self::Satin { .. } => "a satin",
        }
    }
}

/// Which field a [`Weave`] presents.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub enum WeaveOutput {
    /// One on a thread and zero in the gap between two of them, with a hard
    /// edge between. This is the cloth against what is behind it, and a graph
    /// that wants the holes reads it through
    /// [`Invert`](crate::nodes::Invert).
    #[default]
    Mask,
    /// The cloth as relief, in `0..=1`: zero in a gap, three quarters along a
    /// thread between two crossings, and one at the crest of a thread passing
    /// over another. What the field shows at a crossing is whichever of the two
    /// threads is higher there, so the one underneath is behind it rather than
    /// in it.
    Height,
    /// One where a warp thread is the topmost surface: it covers the texel,
    /// and either it is over the weft there or no weft covers the texel at
    /// all.
    Warp,
    /// One where a weft thread is the topmost surface, by the same reading.
    /// Over the cloth this is exactly what [`Self::Warp`] is not, which is
    /// what makes the two a pair of tint masks rather than two thirds of one.
    Weft,
    /// A random float per thread, taken from whichever thread is on top, so a
    /// tint reads one number along a whole thread. In a gap it is the weft's,
    /// there being nothing there to tint.
    Id,
}

/// Woven cloth: two sets of threads crossing over and under one another.
///
/// `x` warp threads run along v and are counted across u; `y` weft threads run
/// along u and are counted across v. `width` is how much of its own pitch a
/// thread fills, so `0.8` leaves a fifth of the pitch as the gap between two
/// threads. It must be under one: the gap is what a crossing thread shows
/// through, and it is also what makes the relief continuous, since a thread's
/// cross-section has to have reached zero by the time the next thread's
/// crossing takes over.
///
/// The period is the thread counts divided by [`WeavePattern`]'s repeat, and
/// that is the whole of why the counts have to divide it. A plain weave
/// crosses one way on even threads and the other way on odd ones, so eight
/// threads carry four repeats of the crossing and seven carry none: thread
/// seven would meet thread zero at the seam and cross the same way it did,
/// which is a fault running the length of the cloth. Seven threads of a plain
/// weave are refused at `x` rather than inferred free, because it is a fact
/// about this node's own fields, exactly as a brick bond that does not divide
/// its rows is.
///
/// [`WeaveOutput::Height`] is that crossing as relief, and it is one model
/// rather than a choice per output. A thread is a half-cosine ridge across its
/// own width, standing three quarters high where nothing crosses it; where it
/// passes over another it is lifted by a quarter and the thread under it
/// pushed down by the same, in proportion to how much of the crossing thread
/// is actually there. That proportion is what makes the height continuous
/// along a thread that goes from over to under: the lift is carried by the
/// crossing thread's own cross-section, which has already fallen to zero by
/// the time the pattern changes, so the two sides of the change agree. A gap
/// reads zero, a thread between two crossings three quarters, and the crest of
/// a thread passing over another one, which is the unit a height map is read in
/// without a clamp anywhere. The thread underneath is pushed to a half, which is
/// what makes a crossing read as one thread lying over another rather than as
/// two ridges meeting; the field shows whichever of the two is higher, so that
/// half is behind the crest rather than in front of it.
///
/// [`WeaveOutput::Id`] is a hash per thread rather than per crossing, so a
/// tint runs the length of a thread. Like a wall's per-brick id it is a lattice
/// rather than a shift the field is invariant under: thread `repeat` crosses
/// the way thread zero did, but it does not hash the way thread zero did, and
/// only the whole unit brings the ids back.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
#[must_use]
pub struct Weave {
    /// Warp threads across UV `[0, 1)`: the ones running along v, counted
    /// along u. The period in u is these divided by the pattern's repeat.
    pub x: u32,
    /// Weft threads across UV `[0, 1)`: the ones running along u, counted
    /// along v, divided likewise for the period in v.
    pub y: u32,
    /// How much of its own pitch a thread fills, in `0..1`. What is left is
    /// the gap the crossing thread shows through.
    pub width: f32,
    /// Which crossing, and so what the thread counts have to divide by.
    pub pattern: WeavePattern,
    /// Which field to present.
    pub output: WeaveOutput,
    /// Varies the per-thread ids without varying the cloth.
    pub seed: u32,
}

impl Default for Weave {
    fn default() -> Self {
        Self {
            x: 8,
            y: 8,
            width: 0.8,
            pattern: WeavePattern::default(),
            output: WeaveOutput::default(),
            seed: 0,
        }
    }
}

impl Weave {
    /// A plain weave of eight threads each way.
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the warp thread count, which the pattern divides into the period
    /// in u.
    pub fn x(mut self, x: u32) -> Self {
        self.x = x;
        self
    }

    /// Set the weft thread count, divided likewise for the period in v.
    pub fn y(mut self, y: u32) -> Self {
        self.y = y;
        self
    }

    /// Set how much of its own pitch a thread fills.
    pub fn width(mut self, width: f32) -> Self {
        self.width = width;
        self
    }

    /// Choose the crossing.
    pub fn pattern(mut self, pattern: WeavePattern) -> Self {
        self.pattern = pattern;
        self
    }

    /// Choose which field to present.
    pub fn output(mut self, output: WeaveOutput) -> Self {
        self.output = output;
        self
    }

    /// Vary the per-thread ids.
    pub fn seed(mut self, seed: u32) -> Self {
        self.seed = seed;
        self
    }

    /// How often the crossing repeats across UV `[0, 1)`, in u and v.
    ///
    /// The thread counts divided by the threads one repeat of the pattern
    /// spans, which is a whole number because [`Check`] rejected a count the
    /// pattern does not close over. The guards are belts for a node that never
    /// reached that check: inference runs after it, and a period of zero is
    /// not a period.
    pub(crate) fn repeats(&self) -> [u32; 2] {
        let repeat = self.pattern.repeat().max(1);
        [(self.x / repeat).max(1), (self.y / repeat).max(1)]
    }

    /// Half a thread's width, which is how far its cross-section reaches from
    /// its own centreline in pitches.
    fn half(&self) -> f32 {
        self.width * 0.5
    }
}

ports!(Weave, Output::Fixed(ValueType::Float));

impl Check for Weave {
    fn check(&self, path: &str) -> Result<(), GraphError> {
        check_count(self.x, "x", path)?;
        check_count(self.y, "y", path)?;
        check_unit(self.width, "width", path)?;
        // A thread as wide as its own pitch leaves nothing for the thread
        // crossing it to show through, and the relief would step where the
        // pattern changes rather than meeting itself: the cross-section has to
        // have fallen to zero before the pitch ends. A thread of no width is
        // no cloth at all.
        require(
            self.width > 0.0 && self.width < 1.0,
            &format!("{path}.width"),
            &format!(
                "a thread {} of its own pitch wide leaves no gap for the thread crossing it, and \
                 nothing for the relief to fall to between two of them; width must be over zero \
                 and under one",
                self.width
            ),
        )?;
        match self.pattern {
            WeavePattern::Plain => {}
            WeavePattern::Twill { step } => {
                check_count(step, "pattern.step", path)?;
                require(
                    step >= 3,
                    &format!("{path}.pattern.step"),
                    &format!(
                        "a twill's repeat is how many threads its diagonal takes to come back to \
                         itself, and {step} is not one: two threads is the plain weave, which \
                         WeavePattern::Plain names on its own, so a twill takes at least three"
                    ),
                )?;
            }
            WeavePattern::Satin { step } => {
                check_count(step, "pattern.step", path)?;
                require(
                    self.pattern.crossing().is_some(),
                    &format!("{path}.pattern.step"),
                    &format!(
                        "a satin of {step} threads has no move that binds every thread once and \
                         still keeps its binding points off a diagonal, which would leave it a \
                         twill under another name; five, seven, eight and every repeat past that \
                         but six have one"
                    ),
                )?;
            }
        }
        let repeat = self.pattern.repeat();
        for (axis, threads) in [("x", self.x), ("y", self.y)] {
            require(
                threads.is_multiple_of(repeat),
                &format!("{path}.{axis}"),
                &format!(
                    "{} crosses the same way again every {repeat} threads, and {threads} of them \
                     cannot be divided that way, so the thread past the seam would cross the \
                     other way from thread zero and leave a fault down the cloth; use a count \
                     that is a multiple of {repeat}",
                    self.pattern.describe()
                ),
            )?;
        }
        Ok(())
    }
}

/// The texel coordinate itself, which is where every other generator's
/// lattice comes from.
impl Lower for Uv {
    fn lower(&self, cx: &mut Lowering, _inputs: &NodeInputs) -> ValueId {
        cx.uv()
    }
}

/// One octave of value noise: the showcase's own, op for op.
///
/// The cell the texel falls in is hashed at its four corners and interpolated
/// bilinearly, with each axis weighted by `smoothstep(0, 1, t)`, which is
/// `t * t * (3 - 2t)`. The corner coordinates are reduced modulo `period` by
/// [`Op::Hash2`] itself, so the lattice wraps and the field meets itself at the
/// seam by construction.
///
/// The arithmetic is written in the order the showcase's pixel loop wrote it —
/// two mixes along u, then a weighted sum along v — because the study's
/// textures are what that loop produced, and they must not move when the study
/// becomes a graph.
fn value_octave(cx: &mut Lowering, uv: ValueId, period: [u32; 2], seed: u32) -> ValueId {
    let repeats = cx.value(Value::Vec2([count(period[0]), count(period[1])]));
    let coordinate = cx.emit(Op::Mul, [uv, repeats]);
    let cell = cx.emit(Op::Floor, [coordinate]);
    let inside = cx.emit(Op::Sub, [coordinate, cell]);
    let zero = cx.constant(0.0);
    let one = cx.constant(1.0);
    let weights = cx.emit(Op::Smoothstep, [zero, one, inside]);
    let along_u = cx.emit(Op::Extract(0), [weights]);
    let along_v = cx.emit(Op::Extract(1), [weights]);

    // The three neighbouring corners. `cell` is already whole, so adding one
    // lands on the next cell exactly rather than near it.
    let east = cx.value(Value::Vec2([1.0, 0.0]));
    let north = cx.value(Value::Vec2([0.0, 1.0]));
    let diagonal = cx.value(Value::Vec2([1.0, 1.0]));
    let right = cx.emit(Op::Add, [cell, east]);
    let above = cx.emit(Op::Add, [cell, north]);
    let across = cx.emit(Op::Add, [cell, diagonal]);

    let lower_left = cx.emit(Op::Hash2(seed), [cell, repeats]);
    let lower_right = cx.emit(Op::Hash2(seed), [right, repeats]);
    let upper_left = cx.emit(Op::Hash2(seed), [above, repeats]);
    let upper_right = cx.emit(Op::Hash2(seed), [across, repeats]);

    let lower = cx.emit(Op::Mix, [lower_left, lower_right, along_u]);
    let upper = cx.emit(Op::Mix, [upper_left, upper_right, along_u]);
    let remainder = cx.emit(Op::Sub, [one, along_v]);
    let below = cx.emit(Op::Mul, [lower, remainder]);
    let over = cx.emit(Op::Mul, [upper, along_v]);
    cx.emit(Op::Add, [below, over])
}

/// The salt a second number from one lattice cell is hashed with.
///
/// A gradient, a jitter and a scatter all want more than one number out of one
/// cell, and the hash answers one, so the rest are the same cell under
/// different seeds. The salt is large and odd — the golden ratio's reciprocal
/// in 32 bits — so that the lanes are uncorrelated and so that a salted seed
/// does not land on the plain seed of the octave above it, which are `seed + 1`,
/// `seed + 2` and so on.
pub(crate) const SALT: u32 = 0x9E37_79B9;

/// The scale that takes a two-dimensional Perlin value into `0..=1`.
///
/// With unit gradients the interpolated dot product is bounded by
/// `sqrt(2) / 2`, so half the unit interval divided by that is what puts the
/// extremes at zero and one. It is a bound rather than a level the field
/// reaches often, which is why Perlin reads flatter than value noise at the
/// same amplitude and why a [`Levels`](crate::nodes::Levels) usually follows
/// one.
const PERLIN_SCALE: f32 = 0.5 / std::f32::consts::FRAC_1_SQRT_2;

/// One octave of Perlin noise on a lattice that wraps at `period`.
///
/// Each corner of the cell carries a gradient taken from its own hash, the
/// offset from the corner to the texel is projected onto it, and the four
/// projections are interpolated with the quintic weight `6t^5 - 15t^4 + 10t^3`,
/// whose first and second derivatives both vanish at the ends. The corners are
/// hashed through [`Op::Hash2`], which reduces a cell modulo the period, so the
/// gradients at `u = 0` and `u = period` are the same gradients: that is the
/// whole of why a Perlin noise here tiles.
///
/// The gradient is a pair of hashes normalised rather than an angle through a
/// sine and a cosine, so no noise in this crate calls `libm` and no two
/// backends can disagree about a transcendental function's last bit. A cell
/// whose two hashes both land on zero takes the zero vector, which
/// [`Op::Normalize`] answers as zero: one flat corner rather than a NaN.
///
/// The noise is zero at every lattice corner, because every offset there is
/// zero and so is every projection, and this maps that to a half.
fn perlin_octave(cx: &mut Lowering, uv: ValueId, period: [u32; 2], seed: u32) -> ValueId {
    let repeats = k2(cx, count(period[0]), count(period[1]));
    let coordinate = mul(cx, uv, repeats);
    let cell = floor(cx, coordinate);
    let inside = sub(cx, coordinate, cell);

    // The quintic, on both axes at once because every op is component-wise.
    let six = k(cx, 6.0);
    let fifteen = k(cx, 15.0);
    let ten = k(cx, 10.0);
    let scaled = mul(cx, inside, six);
    let inner = sub(cx, scaled, fifteen);
    let bracket = mul(cx, inside, inner);
    let sum = add(cx, bracket, ten);
    let squared = mul(cx, inside, inside);
    let cubed = mul(cx, squared, inside);
    let weights = mul(cx, cubed, sum);
    let (along_u, along_v) = axes(cx, weights);

    let mut corners = Vec::with_capacity(4);
    for corner in [[0.0, 0.0], [1.0, 0.0], [0.0, 1.0], [1.0, 1.0]] {
        let step_to = k2(cx, corner[0], corner[1]);
        let here = add(cx, cell, step_to);
        let gx = signed_hash(cx, here, repeats, seed);
        let gy = signed_hash(cx, here, repeats, seed.wrapping_add(SALT));
        let gradient = vec2(cx, gx, gy);
        let gradient = normalize(cx, gradient);
        let offset = sub(cx, inside, step_to);
        corners.push(dot(cx, gradient, offset));
    }
    let lower = mix(cx, corners[0], corners[1], along_u);
    let upper = mix(cx, corners[2], corners[3], along_u);
    let value = mix(cx, lower, upper, along_v);
    let centred = affine(cx, value, PERLIN_SCALE, 0.5);
    let zero = k(cx, 0.0);
    let one = k(cx, 1.0);
    clamp(cx, centred, zero, one)
}

/// Fractal noise: octaves summed at a multiplied period and a diminished
/// amplitude, then divided by the amplitude they carry between them.
///
/// The division is what keeps the result in `0..=1` however many octaves are
/// asked for; at one octave the amplitudes sum to one, so a single-octave noise
/// is the octave itself and nothing is emitted for the sum.
///
/// Each octave hashes on its own seed, because the lattice cell at the origin
/// is cell zero in every octave and would otherwise carry the same number in
/// all of them.
fn fractal(
    noise: &Noise,
    cx: &mut Lowering,
    octave: fn(&mut Lowering, ValueId, [u32; 2], u32) -> ValueId,
) -> ValueId {
    let uv = cx.uv();
    let mut total = cx.constant(0.0);
    let mut amplitude = 1.0_f32;
    let mut carried = 0.0_f32;
    for index in 0..noise.octaves {
        let factor = noise.lacunarity.saturating_pow(index);
        let period = [
            noise.period[0].saturating_mul(factor),
            noise.period[1].saturating_mul(factor),
        ];
        let layer = octave(cx, uv, period, noise.seed.wrapping_add(index));
        total = if index == 0 {
            layer
        } else {
            let weight = cx.constant(amplitude);
            let scaled = cx.emit(Op::Mul, [layer, weight]);
            cx.emit(Op::Add, [total, scaled])
        };
        carried += amplitude;
        amplitude *= noise.persistence;
    }
    if noise.octaves <= 1 {
        return total;
    }
    let normal = cx.constant(1.0 / carried);
    cx.emit(Op::Mul, [total, normal])
}

/// Why a simplex noise does not lower, said the same way wherever it is said.
///
/// The one refusal in the vocabulary that is not a backend waiting its turn;
/// [`NoiseKind::Simplex`] carries the argument.
pub(crate) const SIMPLEX_REFUSAL: &str = "a simplex lattice has no integer period: the skew that turns the square lattice into the \
     triangular one is (sqrt(3) - 1) / 2, which is irrational, so no whole number of repeats \
     brings the lattice back to itself and a simplex noise that tiles is not something this \
     crate can bake; Perlin has the same character and a lattice that wraps";

impl Lower for Noise {
    fn lower(&self, cx: &mut Lowering, _inputs: &NodeInputs) -> ValueId {
        match self.kind {
            NoiseKind::Value => fractal(self, cx, value_octave),
            NoiseKind::Perlin => fractal(self, cx, perlin_octave),
            NoiseKind::Simplex => cx.reject(SIMPLEX_REFUSAL),
        }
    }
}

/// Where a texel sits in the wall: its brick-local coordinates, and which
/// brick it is in.
///
/// The wall is `columns` bricks along u by `rows` along v, and row `r` is laid
/// `r * offset` brick widths along u, which is the bond the period in v rests
/// on. Every output of [`Bricks`] is read off these three numbers.
struct Brick {
    /// Position along the brick, in `0..1`.
    along: ValueId,
    /// Position up the brick, in `0..1`.
    up: ValueId,
    /// The brick's column and row, as a `Vec2`, before any wrap.
    cell: ValueId,
}

/// The wall, as brick-local coordinates.
///
/// The coordinate is wrapped first, and that is load-bearing for the identity
/// rather than for the mask. A brick's local coordinates survive the wrap on
/// their own — the bond closes, so a whole unit up the wall shifts the run by a
/// whole number of bricks and `fract` cannot tell — but the *column index* does
/// not: the row above the v seam is `rows * offset` bricks along from row zero,
/// and reducing that modulo the column count lands on a different brick unless
/// the two counts happen to divide. Wrapping the coordinate makes the wall at
/// `v = 1.3` the wall at `v = 0.3` for every output at once, which is what the
/// period claims and what the identity needs.
fn brick(bricks: &Bricks, cx: &mut Lowering) -> Brick {
    let uv = cx.uv();
    let wrapped = fract(cx, uv);
    let (u, v) = axes(cx, wrapped);

    let rows = k(cx, count(bricks.rows));
    let stacked = mul(cx, v, rows);
    let row = floor(cx, stacked);
    let up = sub(cx, stacked, row);

    let columns = k(cx, count(bricks.columns));
    let run = mul(cx, u, columns);
    let bond = k(cx, bricks.offset);
    let shift = mul(cx, row, bond);
    let shifted = add(cx, run, shift);
    let column = floor(cx, shifted);
    let along = sub(cx, shifted, column);
    let cell = vec2(cx, column, row);
    Brick { along, up, cell }
}

/// One on the brick face and zero in the mortar.
///
/// Within a brick the face runs from `mortar` to `1 - mortar` in both axes, so
/// the mask is a product of two hard bands and the edge belongs to the face: a
/// texel is mortar where its brick-local coordinate is strictly outside them.
///
/// The mortar is brick-local, so a wall of eight rows of one column takes
/// `mortar` off each side of a cell that is one unit wide and an eighth of one
/// tall. That is what makes a row seam `mortar` of a row rather than `mortar`
/// of the texture. It is taken off the u sides too: the wall of one column has
/// one vertical joint per row, at the bond's shift, so a field that is a band
/// in v alone is built from `Uv` and `Math` rather than from a narrow wall.
fn brick_mask(bricks: &Bricks, cx: &mut Lowering, laid: &Brick) -> ValueId {
    let near = cx.constant(bricks.mortar);
    let far = cx.constant(1.0 - bricks.mortar);
    let face = |cx: &mut Lowering, local: ValueId| {
        let after = cx.emit(Op::Step, [near, local]);
        let before = cx.emit(Op::Step, [local, far]);
        cx.emit(Op::Mul, [after, before])
    };
    let across = face(cx, laid.along);
    let up = face(cx, laid.up);
    cx.emit(Op::Mul, [across, up])
}

/// How far into the face a texel is, in brick-local units: negative in the
/// mortar, and zero exactly on the edge the mask closes at.
fn brick_depth(bricks: &Bricks, cx: &mut Lowering, laid: &Brick) -> ValueId {
    let near = k(cx, bricks.mortar);
    let far = k(cx, 1.0 - bricks.mortar);
    let inset = |cx: &mut Lowering, local: ValueId| {
        let after = sub(cx, local, near);
        let before = sub(cx, far, local);
        min(cx, after, before)
    };
    let across = inset(cx, laid.along);
    let up = inset(cx, laid.up);
    min(cx, across, up)
}

impl Lower for Bricks {
    fn lower(&self, cx: &mut Lowering, _inputs: &NodeInputs) -> ValueId {
        // The face this wall cuts is a rectangle. A wall that asked for a
        // shaped one is refused rather than handed the rectangle, because a
        // field quietly ignored is the one failure a path cannot be read off
        // the picture for.
        if self.round > 0.0 {
            return cx.reject("rounded brick corners have no lowering yet");
        }
        if self.corner > 0.0 {
            return cx.reject("cut brick corners have no lowering yet");
        }
        let laid = brick(self, cx);
        match self.output {
            BrickOutput::Mask => brick_mask(self, cx, &laid),
            // The bevel is the mask with a ramp where the mask has a step, and
            // at a bevel of zero it is the mask itself: a smooth ramp between
            // edges that meet is a step, and the edge belongs to the face in
            // both.
            BrickOutput::Bevel => {
                let depth = brick_depth(self, cx, &laid);
                let zero = k(cx, 0.0);
                let bevel = k(cx, self.bevel);
                smoothstep(cx, zero, bevel, depth)
            }
            BrickOutput::Id => {
                let repeats = k2(cx, count(self.columns), count(self.rows));
                cell_hash(cx, laid.cell, repeats, self.seed)
            }
            BrickOutput::Fill => laid.along,
        }
    }
}

/// The two nearest feature points of a cellular lattice, and which cell the
/// nearer one belongs to.
struct Nearest {
    /// Distance to the nearest point, in cell units.
    first: ValueId,
    /// Distance to the next nearest, in cell units.
    second: ValueId,
    /// The lattice cell the nearest point belongs to, as a `Vec2`.
    cell: ValueId,
    /// The vector from the texel to the nearest point, in cell units.
    offset: ValueId,
}

/// A distance, the way one metric measures it.
fn measure(cx: &mut Lowering, offset: ValueId, metric: VoronoiMetric) -> ValueId {
    match metric {
        VoronoiMetric::Euclidean => length(cx, offset),
        VoronoiMetric::Manhattan => {
            let size = abs(cx, offset);
            let (x, y) = axes(cx, size);
            add(cx, x, y)
        }
        VoronoiMetric::Chebyshev => {
            let size = abs(cx, offset);
            let (x, y) = axes(cx, size);
            max(cx, x, y)
        }
    }
}

/// Anything further than this is not a neighbour: a stand-in for infinity that
/// no distance inside a repeat comes near, and that a `min` can start from.
const FURTHEST: f32 = 1e9;

/// Take one candidate point into a running search for the two nearest.
///
/// The comparison is read against the best so far *before* it moves, so the
/// second nearest is the runner-up rather than the winner again.
fn consider(
    cx: &mut Lowering,
    found: &mut Nearest,
    cell: ValueId,
    towards: ValueId,
    distance: ValueId,
) {
    let closer = step(cx, distance, found.first);
    let loser = max(cx, distance, found.first);
    found.second = min(cx, found.second, loser);
    found.cell = select(cx, closer, cell, found.cell);
    found.offset = select(cx, closer, towards, found.offset);
    found.first = min(cx, found.first, distance);
}

/// An empty search, for the loops above to fill.
fn searching(cx: &mut Lowering, base: ValueId) -> Nearest {
    let furthest = k(cx, FURTHEST);
    Nearest {
        first: furthest,
        second: furthest,
        cell: base,
        offset: k2(cx, 0.0, 0.0),
    }
}

/// Search the nine cells around a texel for the two nearest feature points.
///
/// The lattice is wrapped by the hash, so the cell one step past the last is
/// the first and the search costs nothing at the seam that it does not cost
/// anywhere else. Nine cells is exactly enough while a point stays inside its
/// own cell, which is what bounding the jitter at one buys.
///
/// Distances are in cell units, so a cell is one across whatever the period is.
/// On a lattice whose two counts differ that measure is stretched, which is
/// what a Voronoi of `8x2` cells is: cells wider than they are tall.
fn nearest(
    cx: &mut Lowering,
    coordinate: ValueId,
    repeats: ValueId,
    seed: u32,
    jitter: f32,
    metric: VoronoiMetric,
) -> Nearest {
    let base = floor(cx, coordinate);
    let inside = sub(cx, coordinate, base);
    let mut found = searching(cx, base);
    for down in -1_i32..=1 {
        for across in -1_i32..=1 {
            let step_to = k2(cx, whole(across), whole(down));
            let here = add(cx, base, step_to);
            let jx = signed_hash(cx, here, repeats, seed);
            let jy = signed_hash(cx, here, repeats, seed.wrapping_add(SALT));
            let strayed = vec2(cx, jx, jy);
            let amount = k(cx, jitter * 0.5);
            let strayed = mul(cx, strayed, amount);
            let centre = k2(cx, whole(across) + 0.5, whole(down) + 0.5);
            let point = add(cx, centre, strayed);
            let towards = sub(cx, point, inside);
            let distance = measure(cx, towards, metric);
            consider(cx, &mut found, here, towards, distance);
        }
    }
    found
}

/// Half the gap between the two nearest cell points.
///
/// Where the texel lies on the segment joining them this is exactly the
/// distance to their perpendicular bisector, which is the cell boundary;
/// elsewhere it is an approximation to it that still vanishes on the boundary
/// and grows into the cell. It is what [`VoronoiOutput::Edge`] and the gap of a
/// hexagonal [`Tiles`] are both read off, and it costs two instructions over
/// the search that found the two.
fn boundary(cx: &mut Lowering, found: &Nearest) -> ValueId {
    let gap = sub(cx, found.second, found.first);
    let half = k(cx, 0.5);
    mul(cx, gap, half)
}

impl Lower for Voronoi {
    fn lower(&self, cx: &mut Lowering, _inputs: &NodeInputs) -> ValueId {
        let repeats = k2(cx, count(self.period[0]), count(self.period[1]));
        let uv = cx.uv();
        let coordinate = mul(cx, uv, repeats);
        let found = nearest(cx, coordinate, repeats, self.seed, self.jitter, self.metric);
        let one = k(cx, 1.0);
        match self.output {
            VoronoiOutput::Distance => min(cx, found.first, one),
            VoronoiOutput::Cell => {
                cell_hash(cx, found.cell, repeats, self.seed.wrapping_add(CELL_SALT))
            }
            VoronoiOutput::Edge => {
                let edge = boundary(cx, &found);
                min(cx, edge, one)
            }
            VoronoiOutput::Border => {
                let edge = boundary(cx, &found);
                let zero = k(cx, 0.0);
                let width = k(cx, self.width);
                let ramp = smoothstep(cx, zero, width, edge);
                complement(cx, ramp)
            }
            // The search measured the offset in cell units, because that is
            // what the distances above are in; one cell is `1 / period` of the
            // repeat, so dividing by the lattice puts it back in UV.
            VoronoiOutput::Offset => div(cx, found.offset, repeats),
        }
    }
}

/// The salt a per-cell identity is hashed with, so that a cell's id and the
/// jitter of the point inside it are different numbers.
const CELL_SALT: u32 = 0x85EB_CA6B;

/// Where a texel sits in a tiling: which tile, how far from its edge, and how
/// far along it.
///
/// Every [`TilePattern`] answers these three and nothing else, which is what
/// lets one set of outputs stand over three lattices. The distance is in UV,
/// because a gap authored in UV is the same width whichever axis it crosses and
/// whichever pattern is under it.
struct Laid {
    /// A random float per tile.
    id: ValueId,
    /// Distance from the texel to the tile's boundary, in UV.
    edge: ValueId,
    /// The texel's own coordinate within the tile, along one axis, in `0..1`.
    fill: ValueId,
}

/// The distance from a texel to the edge of the tile it is in, in UV, given its
/// place across that tile in each axis and the tile's size in each.
fn tile_edge(
    cx: &mut Lowering,
    along: ValueId,
    up: ValueId,
    width: ValueId,
    height: ValueId,
) -> ValueId {
    let inset = |cx: &mut Lowering, local: ValueId, size: ValueId| {
        let far = complement(cx, local);
        let nearer = min(cx, local, far);
        mul(cx, nearer, size)
    };
    let across = inset(cx, along, width);
    let up = inset(cx, up, height);
    min(cx, across, up)
}

/// A square grid: one tile per cell, so the lattice is the grid itself.
fn grid_tiles(tiles: &Tiles, cx: &mut Lowering) -> Laid {
    let repeats = k2(cx, count(tiles.columns), count(tiles.rows));
    let uv = cx.uv();
    let coordinate = mul(cx, uv, repeats);
    let cell = floor(cx, coordinate);
    let inside = sub(cx, coordinate, cell);
    let (along, up) = axes(cx, inside);
    let width = k(cx, 1.0 / count(tiles.columns));
    let height = k(cx, 1.0 / count(tiles.rows));
    Laid {
        id: cell_hash(cx, cell, repeats, tiles.seed),
        edge: tile_edge(cx, along, up, width, height),
        fill: along,
    }
}

/// Hexagons: the cells of a grid whose odd rows are offset half a cell along u,
/// which is the lattice a running bond lays and whose Voronoi cells are
/// hexagons.
///
/// The tile is that Voronoi cell, so the boundary is where two centres are
/// equally near and a gap opens symmetrically on both sides of it. The row
/// offset is why a hexagonal lattice needs an even row count: the parity of a
/// row has to survive the v seam.
///
/// Distances are measured in UV here rather than in cell units, because the gap
/// of a [`Tiles`] is authored in UV and a hexagon is read across both axes at
/// once.
fn hex_tiles(tiles: &Tiles, cx: &mut Lowering) -> Laid {
    let repeats = k2(cx, count(tiles.columns), count(tiles.rows));
    let uv = cx.uv();
    let coordinate = mul(cx, uv, repeats);
    let base = floor(cx, coordinate);
    let inside = sub(cx, coordinate, base);
    let mut found = searching(cx, base);
    for down in -1_i32..=1 {
        for across in -1_i32..=1 {
            let step_to = k2(cx, whole(across), whole(down));
            let here = add(cx, base, step_to);
            // Half a cell along u on every odd row, which is what makes the
            // lattice hexagonal rather than square.
            let (_, row) = axes(cx, here);
            let half = k(cx, 0.5);
            let halved = mul(cx, row, half);
            let lean = fract(cx, halved);
            let none = k(cx, 0.0);
            let sideways = vec2(cx, lean, none);
            let centre = k2(cx, whole(across) + 0.5, whole(down) + 0.5);
            let point = add(cx, centre, sideways);
            let towards = sub(cx, point, inside);
            let towards = div(cx, towards, repeats);
            let distance = length(cx, towards);
            consider(cx, &mut found, here, towards, distance);
        }
    }
    // The texel's own place across the tile. The offset points from the texel
    // to the centre, in UV, so a cell's width of it either side of a half runs
    // the fill from zero to one across the hexagon.
    let (sideways, _) = axes(cx, found.offset);
    let columns = k(cx, count(tiles.columns));
    let across = mul(cx, sideways, columns);
    let half = k(cx, 0.5);
    Laid {
        id: cell_hash(cx, found.cell, repeats, tiles.seed),
        edge: boundary(cx, &found),
        fill: sub(cx, half, across),
    }
}

/// Herringbone: two-cell rectangles laid at right angles, marching diagonally.
///
/// The whole pattern is one rule. Number the cells and let `k` be
/// `(column + row) mod 4`: the cell belongs to a horizontal tile where `k` is 0
/// or 1 and to a vertical one where it is 2 or 3, and the tile's own origin is
/// that many cells back along whichever axis it runs. That partitions the plane
/// exactly — every horizontal tile is the two cells starting where `k` is 0,
/// every vertical one the two starting where `k` is 2 — and it comes back to
/// itself every four cells in each axis, which is why a herringbone needs a
/// count divisible by four rather than merely even.
fn herringbone_tiles(tiles: &Tiles, cx: &mut Lowering) -> Laid {
    let repeats = k2(cx, count(tiles.columns), count(tiles.rows));
    let uv = cx.uv();
    let coordinate = mul(cx, uv, repeats);
    let cell = floor(cx, coordinate);
    let inside = sub(cx, coordinate, cell);
    let (column, row) = axes(cx, cell);
    let (along, up) = axes(cx, inside);
    let diagonal = add(cx, column, row);
    let quarter = k(cx, 0.25);
    let scaled = mul(cx, diagonal, quarter);
    let wrapped = fract(cx, scaled);
    let four = k(cx, 4.0);
    let step_of_four = mul(cx, wrapped, four);

    // Horizontal where the step is zero or one, which is what a comparison
    // against the halfway edge says without a branch.
    let halfway = k(cx, 1.5);
    let flat = step(cx, step_of_four, halfway);

    let long_u = k(cx, 2.0 / count(tiles.columns));
    let long_v = k(cx, 2.0 / count(tiles.rows));
    let short_u = k(cx, 1.0 / count(tiles.columns));
    let short_v = k(cx, 1.0 / count(tiles.rows));
    let half = k(cx, 0.5);

    // The tile a horizontal cell belongs to: `k` cells back along u, two cells
    // long and one high.
    let flat_column = sub(cx, column, step_of_four);
    let flat_origin = vec2(cx, flat_column, row);
    let flat_along = add(cx, along, step_of_four);
    let flat_along = mul(cx, flat_along, half);
    let flat_edge = tile_edge(cx, flat_along, up, long_u, short_v);

    // And a vertical cell: `k - 2` cells back along v, one cell wide and two
    // high.
    let two = k(cx, 2.0);
    let upright_step = sub(cx, step_of_four, two);
    let upright_row = sub(cx, row, upright_step);
    let upright_origin = vec2(cx, column, upright_row);
    let upright_up = add(cx, up, upright_step);
    let upright_up = mul(cx, upright_up, half);
    let upright_edge = tile_edge(cx, along, upright_up, short_u, long_v);

    let origin = mix(cx, upright_origin, flat_origin, flat);
    Laid {
        id: cell_hash(cx, origin, repeats, tiles.seed),
        edge: mix(cx, upright_edge, flat_edge, flat),
        fill: mix(cx, upright_up, flat_along, flat),
    }
}

impl Lower for Tiles {
    fn lower(&self, cx: &mut Lowering, _inputs: &NodeInputs) -> ValueId {
        let laid = match self.pattern {
            TilePattern::Grid => grid_tiles(self, cx),
            TilePattern::Hex => hex_tiles(self, cx),
            TilePattern::Herringbone => herringbone_tiles(self, cx),
        };
        match self.output {
            // The gap opens on both sides of the boundary, so half of it comes
            // off each tile and a gap of zero is a solid mask.
            BrickOutput::Mask => {
                let half = k(cx, self.gap * 0.5);
                step(cx, half, laid.edge)
            }
            BrickOutput::Bevel => {
                let half = k(cx, self.gap * 0.5);
                let shoulder = k(cx, self.gap.mul_add(0.5, self.bevel));
                smoothstep(cx, half, shoulder, laid.edge)
            }
            BrickOutput::Id => laid.id,
            BrickOutput::Fill => laid.fill,
        }
    }
}

/// One axis of a [`Pattern`], as the wave its kind names.
///
/// Every wave takes the coordinate in repeats and answers in `0..=1` with a
/// period of exactly one, which is what makes a pattern tile at the count it
/// was given.
fn wave(cx: &mut Lowering, coordinate: ValueId, kind: PatternKind) -> ValueId {
    let inside = fract(cx, coordinate);
    match kind {
        // A hard edge halfway through the repeat. A checker's two axes carry
        // the same wave; what makes it a checker is the difference it is
        // combined by.
        PatternKind::Stripes | PatternKind::Square | PatternKind::Checker => {
            let half = k(cx, 0.5);
            step(cx, half, inside)
        }
        PatternKind::Sine => {
            let turns = k(cx, TURN);
            let radians = mul(cx, inside, turns);
            let wave = sin(cx, radians);
            affine(cx, wave, 0.5, 0.5)
        }
        // Up over the first half of the repeat and down over the second, which
        // is the mirror fold at twice the height.
        PatternKind::Triangle => {
            let folded = mirror_fold(cx, inside);
            let doubled = k(cx, 2.0);
            mul(cx, folded, doubled)
        }
    }
}

impl Lower for Pattern {
    fn lower(&self, cx: &mut Lowering, _inputs: &NodeInputs) -> ValueId {
        let uv = cx.uv();
        let (u, v) = axes(cx, uv);
        let x = k(cx, count(self.x));
        let y = k(cx, count(self.y));
        let along = mul(cx, u, x);
        let up = mul(cx, v, y);
        let across = wave(cx, along, self.kind);
        let down = wave(cx, up, self.kind);
        match self.mix {
            PatternMix::Multiply => mul(cx, across, down),
            PatternMix::Add => {
                let sum = add(cx, across, down);
                let zero = k(cx, 0.0);
                let one = k(cx, 1.0);
                clamp(cx, sum, zero, one)
            }
            PatternMix::Max => max(cx, across, down),
            PatternMix::Min => min(cx, across, down),
            PatternMix::Average => {
                let sum = add(cx, across, down);
                let half = k(cx, 0.5);
                mul(cx, sum, half)
            }
            // The exclusive-or of two square waves, which is what makes a
            // checker a checker.
            PatternMix::Difference => {
                let gap = sub(cx, across, down);
                abs(cx, gap)
            }
        }
    }
}

/// The outward normal of the edge a polygon or a star presents in one folded
/// sector, per unit of `size`.
///
/// Both shapes are read the same way. Fold the angle into `0..=half` a sector,
/// so that zero is a point of the shape and `half` is the middle of the span
/// between two of them; the edge is then the straight line between the point at
/// angle zero, which is at radius one, and whatever the shape has at angle
/// `half`. A polygon has the middle of a side there, at the radius a side
/// passes through; a star has its valley, at half the radius. Everything else —
/// how many sides, how sharp — is in those two numbers.
///
/// Answering the normal rather than a formula per kind is what lets both come
/// out of one expression, and measuring the edge per unit `size` is what makes
/// `size` the radius the shape *reaches*, which is the number a [`Shape`]'s own
/// check compares against the repeat it has to fit inside.
fn shape_edge(kind: ShapeKind, half: f32) -> [f32; 2] {
    let (sine, cosine) = half.sin_cos();
    let inner = match kind {
        ShapeKind::Star => 0.5,
        // The middle of a side of a regular polygon whose vertices are at
        // radius one: the apothem, over the direction it sits in.
        _ => cosine,
    };
    let outer = [1.0_f32, 0.0];
    let opposite = [inner * cosine, inner * sine];
    let edge = [opposite[0] - outer[0], opposite[1] - outer[1]];
    // The normal of that edge, turned to point away from the centre.
    let normal = [edge[1], -edge[0]];
    let length = normal[0].hypot(normal[1]).max(f32::MIN_POSITIVE);
    let normal = [normal[0] / length, normal[1] / length];
    if normal[0].mul_add(outer[0], normal[1] * outer[1]) < 0.0 {
        [-normal[0], -normal[1]]
    } else {
        normal
    }
}

/// How far into a tooth of a [`ShapeKind::Gear`] the flank runs, as a fraction
/// of the half-tooth between a root and the point of a tooth.
///
/// A gear is a square wave in angle with its corners taken off, and this is how
/// much of each half-tooth the ramp spends: a third of it either side leaves a
/// flat crest and a flat root of the same width as the flank between them,
/// which reads as a cog rather than as a sine. It is a constant rather than a
/// field because a gear with an authored flank angle is a mechanism, and what
/// this node is for is the *look* of one.
const GEAR_FLANK: f32 = 1.0 / 3.0;

/// Whether a kind has a corner for [`Shape::round`] to take off.
///
/// The rounding shrinks the shape by the radius and then moves the distance
/// back out by it, which turns every corner into an arc and leaves every
/// straight edge and every arc where it was. A circle and a capsule are all arc
/// already, so the two halves cancel exactly; a gear's distance is radial
/// rather than Euclidean, so shrinking and growing it cancels there too. Those
/// three fold the radius away rather than pay two instructions to compute what
/// they started with.
fn rounds(kind: ShapeKind) -> bool {
    matches!(kind, ShapeKind::Box | ShapeKind::Polygon | ShapeKind::Star)
}

/// The signed distance from a texel to the edge of a shape centred in the
/// repeat: negative inside it, positive outside, in UV.
fn shape_distance(shape: &Shape, cx: &mut Lowering) -> ValueId {
    let uv = cx.uv();
    // The coordinate is wrapped first, which is the whole of why a shape tiles:
    // every other generator here wraps its own lattice through the hash, and a
    // shape has no lattice to wrap. Its check keeps it inside the repeat, so
    // the wrap never cuts it; what the wrap does is make the field at `u = 1.3`
    // the field at `u = 0.3`, which is what a period of one claims.
    let point = about_centre(cx, uv);
    // The shape is drawn this much smaller and the distance moved back out by
    // the same amount below, which is the rounding. Where the kind has nothing
    // to round the radius folds away here and the second half is never emitted,
    // so a circle costs what it always did.
    let round = if rounds(shape.kind) { shape.round } else { 0.0 };
    let size = k(cx, shape.size - round);
    let solid = match shape.kind {
        ShapeKind::Circle => {
            let radius = length(cx, point);
            sub(cx, radius, size)
        }
        ShapeKind::Box if round > 0.0 => {
            // The exact distance to the square, which is what a corner radius
            // needs and what the Chebyshev distance below is not: Chebyshev is
            // flat across a whole quadrant outside a corner, so taking the
            // radius off it would move the four sides in and leave the corner
            // as square as it was.
            let magnitude = abs(cx, point);
            let corner = sub(cx, magnitude, size);
            let (x, y) = axes(cx, corner);
            let furthest = max(cx, x, y);
            let zero = k(cx, 0.0);
            let beyond = max(cx, corner, zero);
            let outside = length(cx, beyond);
            let inside = min(cx, furthest, zero);
            add(cx, outside, inside)
        }
        // The Chebyshev distance, which is the box's own: a square of half-width
        // `size` is where both axes are inside it.
        ShapeKind::Box => {
            let magnitude = abs(cx, point);
            let (x, y) = axes(cx, magnitude);
            let furthest = max(cx, x, y);
            sub(cx, furthest, size)
        }
        ShapeKind::Polygon | ShapeKind::Star => {
            let (angle, radius) = polar(cx, point);
            let sector = TURN / count(shape.sides.max(3));
            // Fold the angle into half a sector, measured from the direction a
            // point of the shape sits in, so every sector is the same wedge.
            let scale = k(cx, 1.0 / sector);
            let turns = mul(cx, angle, scale);
            let folded = mirror_fold(cx, turns);
            let back = k(cx, sector);
            let folded = mul(cx, folded, back);
            let cosine = cos(cx, folded);
            let upright = sin(cx, folded);
            let outward = shape_edge(shape.kind, sector * 0.5);
            let nx = k(cx, outward[0]);
            let ny = k(cx, outward[1]);
            let across = mul(cx, cosine, nx);
            let up = mul(cx, upright, ny);
            let projection = add(cx, across, up);
            let reach = mul(cx, radius, projection);
            // The edge sits `normal . outer` along that normal, and `outer` is
            // the unit vector at angle zero, so the first lane is the whole of
            // the projection.
            let inset = mul(cx, size, nx);
            sub(cx, reach, inset)
        }
        // The distance to the segment lying along u, less the radius of the
        // caps. It is exact everywhere, which is what makes a hollow capsule a
        // slot of even width rather than one that pinches at the ends.
        ShapeKind::Capsule => {
            let magnitude = abs(cx, point);
            let (x, y) = axes(cx, magnitude);
            let half = k(cx, shape.length * 0.5);
            let past = sub(cx, x, half);
            let zero = k(cx, 0.0);
            let along = max(cx, past, zero);
            let arm = vec2(cx, along, y);
            let reach = length(cx, arm);
            sub(cx, reach, size)
        }
        // A disc whose radius is modulated by the angle, so what this answers
        // is a *radial* distance and not a Euclidean one: it is exact along a
        // crest and a root, where the boundary is an arc about the centre, and
        // it overstates the distance on a flank in proportion to how steeply
        // the tooth is cut. That matters to an `edge` ramp, which is wider on a
        // flank than it is on a crest, and to anything that reads the distance
        // as a distance; it does not move the boundary itself, which is where
        // the radius meets the tooth.
        ShapeKind::Gear => {
            let (angle, radius) = polar(cx, point);
            // Half a turn of offset puts the point of a tooth at angle zero,
            // where a polygon has its vertex, so the two agree about which way
            // the shape faces.
            let teeth = count(shape.sides.max(3));
            let turns = affine(cx, angle, teeth / TURN, 0.5);
            let ramp = quadrant_fold(cx, turns);
            let low = k(cx, GEAR_FLANK);
            let high = k(cx, 1.0 - GEAR_FLANK);
            let tooth = smoothstep(cx, low, high, ramp);
            // Zero in the root circle and `depth` out at the crest, measured
            // from a root radius that already carries the rounding.
            let reach = affine(cx, tooth, shape.depth, shape.size - round - shape.depth);
            sub(cx, radius, reach)
        }
    };
    let rounded = if round > 0.0 {
        let radius = k(cx, round);
        sub(cx, solid, radius)
    } else {
        solid
    };
    if shape.hollow > 0.0 {
        // The onion: `|d + h / 2| - h / 2` is zero where the shape's boundary
        // was and zero again `h` inside it, and positive everywhere between the
        // two, so what is left is the shell. It is taken inward, which is why a
        // hollow shape needs no check of its own — it reaches exactly as far as
        // the solid one the check already bounded.
        let half = k(cx, shape.hollow * 0.5);
        let shifted = add(cx, rounded, half);
        let folded = abs(cx, shifted);
        sub(cx, folded, half)
    } else {
        rounded
    }
}

impl Lower for Shape {
    fn lower(&self, cx: &mut Lowering, _inputs: &NodeInputs) -> ValueId {
        let distance = shape_distance(self, cx);
        match self.output {
            // One inside, zero outside, with the falloff taken inward from the
            // edge. At an edge of zero the two bounds meet and the ramp is a
            // step, which is the hard shape that field names.
            ShapeOutput::Mask => inward_ramp(cx, distance, self.edge),
            ShapeOutput::Distance => distance,
        }
    }
}

/// The lattice a scatter of scratches is laid on, in cells per axis.
///
/// One segment per cell, and the cells past `count` carry none, so a count is a
/// count rather than a suggestion. The grid is what makes a scratch cost a
/// neighbourhood rather than a loop over every segment: a texel tests the cells
/// a segment could reach it from and no others, so thirty-two scratches and
/// four hundred cost the same per texel.
pub(crate) fn scratch_grid(count: u32) -> u32 {
    let mut side = 1_u32;
    while side.saturating_mul(side) < count {
        side = side.saturating_add(1);
    }
    side
}

/// How many cells out a scratch can reach from the cell its centre is in.
///
/// A segment's centre lies somewhere in its own cell and the segment reaches
/// `length / 2 + width` from there, so a texel that far away can be on it. In
/// cells that is the reach times the grid, rounded up, and the neighbourhood a
/// lowering walks is that many cells in every direction.
pub(crate) fn scratch_reach(scratches: &Scratches) -> f32 {
    let grid = count(scratch_grid(scratches.count));
    ((scratches.length * 0.5 + scratches.width) * grid).ceil()
}

/// The furthest a scratch may reach, in cells.
///
/// Three is a seven-by-seven neighbourhood, which is forty-nine segment tests
/// per texel. Past that a graph is asking for a long scratch over a fine grid,
/// and what it wants is fewer and longer or more and shorter; the refusal says
/// so rather than letting a bake take an hour.
pub(crate) const MAX_SCRATCH_REACH: f32 = 3.0;

/// One cell's scratch, as seen from a texel in a neighbouring cell.
///
/// Zero where the cell carries no segment, which is every cell numbered past
/// the count the node asked for.
fn scratch(
    cx: &mut Lowering,
    scratches: &Scratches,
    here: ValueId,
    inside: ValueId,
    step_to: ValueId,
    repeats: ValueId,
    cells: f32,
) -> ValueId {
    // Where this cell's segment sits: anywhere inside the cell, in the units
    // the texel's own position is measured in, then taken into UV.
    let jx = cell_hash(cx, here, repeats, scratches.seed);
    let jy = cell_hash(cx, here, repeats, scratches.seed.wrapping_add(SALT));
    let jitter = vec2(cx, jx, jy);
    let centre = add(cx, step_to, jitter);
    let towards = sub(cx, inside, centre);
    let stride = k(cx, 1.0 / cells);
    let towards = mul(cx, towards, stride);

    let turned = signed_hash(
        cx,
        here,
        repeats,
        scratches.seed.wrapping_add(SALT.wrapping_mul(2)),
    );
    let spread = k(cx, scratches.angle_spread.to_radians());
    let turned = mul(cx, turned, spread);
    let mean = k(cx, scratches.angle.to_radians());
    let angle = add(cx, turned, mean);
    let cosine = cos(cx, angle);
    let sine = sin(cx, angle);
    let direction = vec2(cx, cosine, sine);

    // The nearest point of the segment is its own projection, held to half its
    // length either way.
    let along = dot(cx, towards, direction);
    let back = k(cx, -scratches.length * 0.5);
    let forward = k(cx, scratches.length * 0.5);
    let held = clamp(cx, along, back, forward);
    let nearest = mul(cx, direction, held);
    let away = sub(cx, towards, nearest);
    let distance = length(cx, away);
    let core = k(cx, scratches.width * 0.5);
    let edge = k(cx, scratches.width);
    let fade = smoothstep(cx, core, edge, distance);
    let line = complement(cx, fade);

    // A cell past the count carries no segment. Cells are numbered row by row
    // over the wrapped lattice, so the numbering survives the seam.
    let (column, row) = axes(cx, here);
    let column = wrapped(cx, column, cells);
    let row = wrapped(cx, row, cells);
    let width = k(cx, cells);
    let offset = mul(cx, row, width);
    let index = add(cx, offset, column);
    let last = k(cx, count(scratches.count) - 0.5);
    let present = step(cx, index, last);
    mul(cx, line, present)
}

impl Lower for Scratches {
    fn lower(&self, cx: &mut Lowering, _inputs: &NodeInputs) -> ValueId {
        let grid = scratch_grid(self.count);
        let cells = count(grid);
        let repeats = k2(cx, cells, cells);
        let uv = cx.uv();
        let coordinate = mul(cx, uv, repeats);
        let base = floor(cx, coordinate);
        let inside = sub(cx, coordinate, base);
        #[expect(
            clippy::cast_possible_truncation,
            reason = "the node's own check refused a reach past MAX_SCRATCH_REACH"
        )]
        let reach = scratch_reach(self).clamp(0.0, MAX_SCRATCH_REACH) as i32;
        let mut brightest = k(cx, 0.0);
        for down in -reach..=reach {
            for across in -reach..=reach {
                let step_to = k2(cx, whole(across), whole(down));
                let here = add(cx, base, step_to);
                let drawn = scratch(cx, self, here, inside, step_to, repeats, cells);
                brightest = max(cx, brightest, drawn);
            }
        }
        brightest
    }
}

/// Where a texel sits in the cloth: which two threads cross under it, and how
/// far it is from the centreline of each.
///
/// The coordinate is wrapped first, and that is what the period rests on: the
/// cloth at `u = 1.3` is the cloth at `u = 0.3`, thread index and crossing
/// together, rather than a thirteenth thread the far seam knows nothing about.
struct Cloth {
    /// The warp thread's index along u, a whole number in `0..x`.
    warp: ValueId,
    /// The weft thread's index along v, a whole number in `0..y`.
    weft: ValueId,
    /// How far the texel is from the warp thread's centreline, in pitches:
    /// zero on the line and a half at the middle of the gap to the next.
    across_warp: ValueId,
    /// The same across the weft thread.
    across_weft: ValueId,
}

/// The cloth, as thread indices and distances from two centrelines.
///
/// Both axes are done at once, on the `Vec2` the coordinate arrives as, which
/// is why this costs seven instructions rather than fourteen: a thread's place
/// in u and a weft's in v are the same arithmetic at different counts.
fn cloth(weave: &Weave, cx: &mut Lowering) -> Cloth {
    let uv = cx.uv();
    let wrapped = fract(cx, uv);
    let counts = k2(cx, count(weave.x), count(weave.y));
    let scaled = mul(cx, wrapped, counts);
    let base = floor(cx, scaled);
    let inside = sub(cx, scaled, base);
    let centre = k2(cx, 0.5, 0.5);
    let offset = sub(cx, inside, centre);
    let distance = abs(cx, offset);
    let (warp, weft) = axes(cx, base);
    let (across_warp, across_weft) = axes(cx, distance);
    Cloth {
        warp,
        weft,
        across_warp,
        across_weft,
    }
}

/// One where the warp thread is over the weft at this crossing and zero where
/// it dives under it.
///
/// The residue is read as a fraction of the repeat rather than as a count of
/// threads — `fract((warp - move * weft - offset) / repeat)` lands on a
/// multiple of one over the repeat — because that is one instruction fewer and
/// because the comparison is then against a threshold half a thread inside the
/// float rather than exactly on it. That half-thread of slack is what absorbs
/// the rounding of the division: the residue is a whole number of threads by
/// construction, and nothing short of a repeat in the thousands puts a float
/// that far off the number it stands for.
fn crossing(weave: &Weave, cx: &mut Lowering, laid: &Cloth) -> ValueId {
    let Some(rule) = weave.pattern.crossing() else {
        // Refused at `pattern.step` long before a lowering runs, so this is a
        // belt: a satin whose repeat admits no move has no crossing to draw.
        return cx.reject("a satin whose repeat admits no move has no lowering");
    };
    let mut start = laid.weft;
    if rule.move_by != 1 {
        let move_by = k(cx, count(rule.move_by));
        start = mul(cx, start, move_by);
    }
    if rule.offset != 0 {
        let offset = k(cx, count(rule.offset));
        start = add(cx, start, offset);
    }
    let from_start = sub(cx, laid.warp, start);
    let repeat = k(cx, count(rule.repeat));
    let turns = div(cx, from_start, repeat);
    let residue = fract(cx, turns);
    // `Op::Step` answers one where its second operand is at least its first,
    // so this reads "the float reaches at least as far as the residue does",
    // which is the comparison the other way round and saves a complement.
    let float = k(cx, (count(rule.float) - 0.5) / count(rule.repeat));
    step(cx, residue, float)
}

/// One where a thread covers the texel and zero in the gap beside it.
///
/// Read the other way round, as [`crossing`] reads its own: [`Op::Step`]
/// answers one where its second operand is at least its first, so this is
/// "half a thread reaches at least as far as the texel is from the
/// centreline".
fn covers(cx: &mut Lowering, across: ValueId, half: f32) -> ValueId {
    let half = k(cx, half);
    step(cx, across, half)
}

/// A thread's cross-section: one along its own centreline, falling to zero at
/// its edge as the half-cosine does.
///
/// Clamped before the cosine rather than after it, because past the thread's
/// own edge the cosine would come back up and draw a second thread inside the
/// gap. What the clamp leaves is a profile that is exactly zero everywhere
/// outside the thread, which is what lets the relief carry a crossing's lift
/// on it and still meet itself where the pattern changes.
fn profile(cx: &mut Lowering, across: ValueId, half: f32) -> ValueId {
    let reach = k(cx, 1.0 / half);
    let over = mul(cx, across, reach);
    let zero = k(cx, 0.0);
    let one = k(cx, 1.0);
    let held = clamp(cx, over, zero, one);
    let half_turn = k(cx, TURN * 0.5);
    let radians = mul(cx, held, half_turn);
    let wave = cos(cx, radians);
    affine(cx, wave, 0.5, 0.5)
}

/// How high a thread's crest stands where nothing crosses it, in the unit
/// [`WeaveOutput::Height`] answers in.
///
/// Three quarters, with the quarter above it and the half below it spent by
/// [`WEAVE_RISE`], so a thread over another reaches exactly one and a thread
/// under one exactly a half. They are constants rather than fields because
/// what decides how deep a cloth reads is the height scale the surface is
/// baked with, and a second amplitude here would be that one twice.
const WEAVE_CREST: f32 = 0.75;

/// How far a crossing lifts the thread on top of it and sinks the one under.
const WEAVE_RISE: f32 = 0.25;

/// The cloth as relief: each thread a ridge across its own width, lifted where
/// it passes over and pushed down where it passes under.
///
/// The lift is carried by the *crossing* thread's cross-section rather than by
/// the crossing itself, which is what makes the field continuous where the
/// pattern changes. The pattern changes at the middle of the gap between two
/// threads, and the profile that carries the lift is zero there, so both sides
/// of the change read the thread at its own plain crest and agree.
fn relief(weave: &Weave, cx: &mut Lowering, laid: &Cloth, over: ValueId) -> ValueId {
    let half = weave.half();
    let warp = profile(cx, laid.across_warp, half);
    let weft = profile(cx, laid.across_weft, half);
    // One where the warp is on top and minus one where it is under: the two
    // threads take a crossing in opposite directions, so one sign does both.
    let sign = affine(cx, over, 2.0, -1.0);
    let crest = |cx: &mut Lowering, crossed: ValueId, rise: f32| {
        let carried = mul(cx, sign, crossed);
        affine(cx, carried, rise, WEAVE_CREST)
    };
    let warp_crest = crest(cx, weft, WEAVE_RISE);
    let warp_height = mul(cx, warp, warp_crest);
    let weft_crest = crest(cx, warp, -WEAVE_RISE);
    let weft_height = mul(cx, weft, weft_crest);
    max(cx, warp_height, weft_height)
}

/// One where this thread is the topmost surface: it covers the texel, and
/// either it is the one on top there or nothing crosses it there at all.
///
/// The second half is what makes [`WeaveOutput::Warp`] and
/// [`WeaveOutput::Weft`] partition the cloth rather than leave the gaps
/// between crossing threads unclaimed by either.
fn on_top(cx: &mut Lowering, mine: ValueId, theirs: ValueId, over: ValueId) -> ValueId {
    let gap = complement(cx, theirs);
    let shown = max(cx, over, gap);
    mul(cx, mine, shown)
}

impl Lower for Weave {
    fn lower(&self, cx: &mut Lowering, _inputs: &NodeInputs) -> ValueId {
        let laid = cloth(self, cx);
        let half = self.half();
        match self.output {
            // The cloth against the gaps in it: a texel is cloth where either
            // thread covers it, whichever of the two is on top.
            WeaveOutput::Mask => {
                let warp = covers(cx, laid.across_warp, half);
                let weft = covers(cx, laid.across_weft, half);
                max(cx, warp, weft)
            }
            WeaveOutput::Height => {
                let over = crossing(self, cx, &laid);
                relief(self, cx, &laid, over)
            }
            // The three that ask which thread a texel actually shows. Each
            // reads the same two coverings and the same crossing, and the one
            // of the two masks its own output does not want is dropped by the
            // pass that drops an unread output.
            WeaveOutput::Warp | WeaveOutput::Weft | WeaveOutput::Id => {
                let warp = covers(cx, laid.across_warp, half);
                let weft = covers(cx, laid.across_weft, half);
                let over = crossing(self, cx, &laid);
                let under = complement(cx, over);
                let warp_top = on_top(cx, warp, weft, over);
                let weft_top = on_top(cx, weft, warp, under);
                match self.output {
                    WeaveOutput::Warp => warp_top,
                    WeaveOutput::Weft => weft_top,
                    // A thread's id is its own index hashed against the thread
                    // counts, so it wraps at the seam the way every other hash
                    // in the crate does, and the two sets are hashed under
                    // different seeds so that warp three and weft three are
                    // not one number.
                    _ => {
                        let repeats = k2(cx, count(self.x), count(self.y));
                        let none = k(cx, 0.0);
                        let along = vec2(cx, laid.warp, none);
                        let warp_id = cell_hash(cx, along, repeats, self.seed);
                        let up = vec2(cx, none, laid.weft);
                        let weft_id = cell_hash(cx, up, repeats, self.seed.wrapping_add(CELL_SALT));
                        select(cx, warp_top, warp_id, weft_id)
                    }
                }
            }
        }
    }
}

/// A lattice coordinate reduced into `0..cells`, the way the hash reduces it.
fn wrapped(cx: &mut Lowering, coordinate: ValueId, cells: f32) -> ValueId {
    let stride = k(cx, 1.0 / cells);
    let scaled = mul(cx, coordinate, stride);
    let inside = fract(cx, scaled);
    let width = k(cx, cells);
    let reduced = mul(cx, inside, width);
    floor(cx, reduced)
}

/// A small whole number as the float a coordinate is offset by.
#[expect(
    clippy::cast_precision_loss,
    reason = "a neighbourhood offset is a single digit"
)]
fn whole(value: i32) -> f32 {
    value as f32
}

#[cfg(test)]
mod tests {
    use super::{TilePattern, Tiles, bond_period};

    #[test]
    fn a_bond_divides_the_rows_or_the_wall_has_no_period_in_v() {
        // A stack bond is back where it started every row; a running bond
        // every second row; a third bond every third.
        assert_eq!(bond_period(0.0, 8), Some(8));
        assert_eq!(bond_period(1.0, 8), Some(8), "a whole brick is no offset");
        assert_eq!(bond_period(0.5, 8), Some(4));
        assert_eq!(bond_period(0.5, 2), Some(1));
        assert_eq!(bond_period(1.0 / 3.0, 9), Some(3));
        assert_eq!(bond_period(0.25, 8), Some(2));
        // The bond must divide the rows, or the row above the v seam is part
        // of a brick away from row zero.
        assert_eq!(bond_period(0.5, 3), None);
        assert_eq!(bond_period(0.25, 6), None);
        // An offset no small fraction approximates never comes back at all
        // within a wall this short.
        assert_eq!(bond_period(0.37, 8), None);
        assert_eq!(bond_period(0.37, 100), Some(1), "37 bricks over 100 rows");
    }

    #[test]
    fn a_bond_that_drifts_across_the_wall_comes_back_nowhere() {
        // The slop is on the wall and not on one row, so an offset that is
        // nearly nothing is not nothing: a ten-thousandth of a brick over four
        // thousand rows walks four tenths of a brick along, and a wall that
        // claimed to tile there would lay its top row visibly off its bottom
        // one. Every step is refused rather than the first, because every step
        // drifts by as much by the time the wall ends.
        assert_eq!(bond_period(0.0001, 4096), None);
        assert_eq!(bond_period(0.0001, 2000), None);
        // The same offset over a wall short enough to carry it is a stack
        // bond by any measure the seam can take.
        assert_eq!(bond_period(0.0001, 1), Some(1));
        // A decimal typed to four places is a third of a brick to three, and
        // over nine rows that is three ten-thousandths out at the seam.
        assert_eq!(bond_period(0.3333, 9), None);
        // The fraction it stands for is not, though `1.0 / 3.0` is no third
        // in `f32` either: what it is out by is a rounding and stays one.
        assert_eq!(bond_period(1.0 / 3.0, 999), Some(333));
    }

    #[test]
    fn a_lattice_spans_whole_cells_of_the_grid_it_is_laid_over() {
        let tiles = |pattern, columns, rows| {
            Tiles::new()
                .pattern(pattern)
                .columns(columns)
                .rows(rows)
                .repeats()
        };
        assert_eq!(tiles(TilePattern::Grid, 5, 3), [5, 3]);
        assert_eq!(tiles(TilePattern::Hex, 5, 6), [5, 3]);
        // A herringbone weave takes four cells in each axis to come back to
        // itself, not two: two of them hold two rectangles, and the only ways
        // to lay two in a two-by-two block are stacked or side by side, both of
        // which are a bond rather than a weave.
        assert_eq!(tiles(TilePattern::Herringbone, 8, 12), [2, 3]);
    }
}
