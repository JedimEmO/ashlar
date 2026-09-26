//! Runtime inputs: the four things a texture cannot hold, and the two nodes
//! built out of the world-space pair.
//!
//! Every other node in the vocabulary is a field over UV, and a bake is what a
//! field over UV comes to. These four are not fields over UV at all. Time moves
//! between frames, a world position and a world normal move between fragments
//! of one repeat, and a cut flag is a fact about the mesh rather than about the
//! surface. A texture has no axis to store any of them along, so a graph that
//! reads one has to stay a shader; [`Target::Shader`](crate::ir::Target::Shader)
//! is where they mean anything, and
//! [`partition`](crate::partition::partition) is what says so — a value that
//! depends on one of these is runtime, and everything it feeds is emitted per
//! fragment rather than baked.
//!
//! # What a bake does with them, which is two different things
//!
//! [`Time`] and [`CutFlag`] are **zero in a bake**: a bake has no frame and no
//! mesh, and zero is a picture — the instant the app started, on a face nobody
//! cut. [`Inputs`](crate::interp::Inputs) defaults them to it, which is also
//! what lets the conformance test bake a shader graph and compare it against
//! the same expression on the GPU at the same instant. A bake of a cut-aware
//! graph is therefore one texture set for both kinds of face, which is what a
//! texture set *is*, and
//! [`BakeReport::cut_flag`](crate::bake::BakeReport::cut_flag) reports that it
//! happened.
//!
//! [`WorldPos`] and [`WorldNormal`] are **refused** by a bake instead, at the
//! node that asked, and so are [`Triplanar`] and [`WorldMask`], which are built
//! out of them. Zero is not a picture for either: every fragment at the origin
//! is a triplanar reading one point of its source across a whole wall, and a
//! world normal of zero is not a direction at all, so a mask of "the faces that
//! point up" would answer the same number everywhere and a bake would ship it.
//! A refusal by path naming the [`Shader`](crate::ir::Target::Shader) surface
//! that does answer is the honest form of "this is not a texture". It is a
//! refusal about the *graph* rather than about its outputs: lowering walks
//! every node a graph declares, so a world mask nothing reads refuses a bake as
//! surely as one wired to the base colour, and names itself while doing it. The op stays
//! ordinary — [`Op::WorldPos`] is emitted, interpreted and printed exactly as
//! it was, and the interpreter still reads it from
//! [`Inputs::world_pos`](crate::interp::Inputs::world_pos) — because what the
//! rule is about is a *material* somebody asked to bake rather than an
//! expression somebody asked to evaluate.
//!
//! All six are period-neutral rather than free, and [`Time`] says why.

use serde::{Deserialize, Serialize};

use crate::ir::{Lower, Lowering, NodeInputs, Op, Target, ValueId};
use crate::nodes::frame::{abs, add, div, k, mul, smoothstep, vec2};
use crate::{
    GraphError, Input, Period, ValueType, finite,
    nodes::{Check, Output, Resolved, ports},
    require,
};

/// Seconds since the app started.
///
/// The input a pulsing light strip is built out of: a
/// [`Math`](crate::nodes::Math) `Sin` of it drives an emissive, and the whole
/// material is one multiply per fragment over a baked emissive texture.
///
/// Zero in a bake, so a baked graph that reads this is the picture at startup.
///
/// # Periodicity
///
/// This and the five below carry [`Period::UNIT`](crate::Period::UNIT), the
/// period a [`Param`](crate::Param) carries, and that is not a claim that time
/// repeats once per UV unit. A [`Period`](crate::Period) answers one question —
/// how many times does this field come back to itself across UV `[0, 1)` — and
/// these have no dependence on UV to come back from: at one instant, at one
/// fragment, each is the same number across the whole repeat. The unit period
/// is the neutral element of the least common multiple, so they cost a
/// pointwise node nothing and leave the output's own tiling check meaning
/// exactly what it did. Calling them [`Free`](crate::Period::Free) instead
/// would refuse every graph that read one, on the strength of a question they
/// are not answers to.
///
/// The one thing that does not promise is seamlessness in *world* space: a
/// [`WorldPos`] read on either side of a UV seam is two different positions,
/// because the mesh put them there. That is a property of the mesh, and no
/// inference over UV can see it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[must_use]
pub struct Time;

impl Time {
    /// Seconds since the app started.
    pub fn new() -> Self {
        Self
    }
}

ports!(Time, Output::Fixed(ValueType::Float));
impl Check for Time {}

/// The fragment's world position, in metres.
///
/// Three lanes, and the graph's three-lane type is `Color`, so this arrives as
/// one: [`Decompose`](crate::nodes::Decompose) takes an axis off it the way it
/// takes a channel off a colour. The type carries a width and not a meaning,
/// which is the same reason a normal is a `Color` here. Period-neutral, as
/// [`Time`] explains.
///
/// **A bake refuses this**, at the node that asked, and says to give the
/// material a [`Surface::Shader`] instead. A graph that reads a world position
/// describes a surface that is different on every wall it is put on — grime
/// that ignores UV seams, [`Triplanar`] blending — which is exactly what a
/// texture cannot be, and a bake that folded it to zero would write one point
/// of that surface across a whole map without saying so.
///
/// [`Surface::Shader`]: https://docs.rs/ashlar/latest/ashlar/enum.Surface.html
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[must_use]
pub struct WorldPos;

impl WorldPos {
    /// The fragment's world position.
    pub fn new() -> Self {
        Self
    }
}

ports!(WorldPos, Output::Fixed(ValueType::Color));
impl Check for WorldPos {}

/// The fragment's world normal, unit length.
///
/// Three lanes, as a `Color`, for the reason [`WorldPos`] gives, and
/// period-neutral, as [`Time`] explains. What a triplanar weight or a
/// snow-on-upward-faces mask is built from.
///
/// **A bake refuses this**, as it refuses [`WorldPos`] and for a sharper
/// reason: zero is not a direction. A mask of the faces that point up, folded
/// against a normal of zero, is one number across the whole map, and the bake
/// would ship it as a surface.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[must_use]
pub struct WorldNormal;

impl WorldNormal {
    /// The fragment's world normal.
    pub fn new() -> Self {
        Self
    }
}

ports!(WorldNormal, Output::Fixed(ValueType::Color));
impl Check for WorldNormal {}

/// One on a cut face, zero elsewhere.
///
/// A building's cut faces are the ones the kernel made rather than the ones the
/// author drew, and a material that wants to treat them differently — raw
/// aggregate where a panel was sawn — needs to know which is which without a
/// second material key. `ashlar-bevy` carries it as a per-vertex attribute (its
/// `ATTRIBUTE_ASHLAR_CUT`), and the boundary of a cut is a weld seam, so every
/// triangle's three corners agree and the value a fragment reads is exactly one
/// or exactly zero rather than a ramp across the face beside it.
///
/// Period-neutral, as [`Time`] explains.
///
/// Zero in a bake — one texture serves both kinds of face, and
/// [`BakeReport::cut_flag`](crate::bake::BakeReport::cut_flag) says when a bake
/// answered that way — and zero on a mesh that carries no cut face at all,
/// which is the same answer read off a surface nothing was cut out of.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[must_use]
pub struct CutFlag;

impl CutFlag {
    /// Whether the surface is a cut face.
    pub fn new() -> Self {
        Self
    }
}

ports!(CutFlag, Output::Fixed(ValueType::Float));
impl Check for CutFlag {}

/// The default sharpness of a [`Triplanar`] blend.
///
/// Four, which over the six faces of a cube leaves a seam about a tenth of a
/// quadrant wide: sharp enough that a face reads as its own projection rather
/// than as a wash of three, soft enough that the diagonal is a blend rather
/// than a line. One is the softest this node allows and is the normal's own
/// lanes unweighted.
pub const TRIPLANAR_SHARPNESS: f32 = 4.0;

/// The source projected on the three axis planes of the world and blended by
/// the world normal.
///
/// The node for a surface that must ignore UV seams: the source is read at the
/// fragment's own world position rather than at its UV, three times — once per
/// axis plane — and the three are weighted by how far the fragment's normal
/// points along each axis. A wall, its return and the soffit above it are then
/// one continuous field of grime with no seam where the mesh's UVs stop
/// agreeing, and a face the mesher cut takes the same surface as the face it
/// was cut from.
///
/// It is a **resampler**, and it costs what one costs: the source's
/// sub-expression is emitted three times, at three coordinates, through
/// [`Lowering::substitute`](crate::ir::Lowering::substitute) — so what the
/// three share is shared and what depends on the coordinate is paid for three
/// times over. [`CostReport::ops`](crate::partition::CostReport::ops) counts
/// all of it, which is the honest number and is usually smaller than three
/// times the source, because the weights and the projections fold and share
/// like anything else.
///
/// # Metres, and why this node carries its own
///
/// A graph is authored per repeat, and a material definition's `tile_metres`
/// says how many metres a repeat covers — through the UV transform, which this
/// node does not read. So a triplanar carries the same number itself:
/// [`Self::tile_metres`] is how much world space one repeat of the source
/// covers, in metres, along every axis. A definition's own tiling still applies
/// to everything in the graph that *does* read UV, and the two are deliberately
/// separate: a triplanar grime over a UV-mapped brick is two tilings because it
/// is two surfaces.
///
/// # Periodicity
///
/// The output is period-neutral — [`Period::UNIT`](crate::Period::UNIT) — for
/// exactly the reason [`WorldPos`] is: it is not a field over UV, so the
/// question a period answers does not apply to it. What *is* required is that
/// the **source** tiles, and that is checked by path rather than assumed: the
/// world-space field this lays repeats every [`Self::tile_metres`] along each
/// axis, and a source that does not meet itself would put a seam at every one
/// of those metre lines — which is the seam the node exists to remove.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
#[must_use]
pub struct Triplanar {
    /// The field to project. Read three times, at three world-space
    /// coordinates.
    pub source: Input,
    /// Metres of world space one repeat of the source covers, along each axis.
    pub tile_metres: f32,
    /// How sharply the blend favours the axis the fragment points along: the
    /// exponent the three weights are raised to. See [`TRIPLANAR_SHARPNESS`].
    pub sharpness: f32,
}

impl Default for Triplanar {
    fn default() -> Self {
        Self {
            source: Input::default(),
            tile_metres: 1.0,
            sharpness: TRIPLANAR_SHARPNESS,
        }
    }
}

impl Triplanar {
    /// Project one source on the three axis planes.
    pub fn new(source: impl Into<Input>) -> Self {
        Self {
            source: source.into(),
            ..Self::default()
        }
    }

    /// How many metres of world space one repeat of the source covers.
    pub fn tile_metres(mut self, metres: f32) -> Self {
        self.tile_metres = metres;
        self
    }

    /// How sharply the blend favours the axis the fragment points along.
    pub fn sharpness(mut self, sharpness: f32) -> Self {
        self.sharpness = sharpness;
        self
    }
}

ports!(
    Triplanar,
    Output::SameAs("source"),
    "source": Field => source,
);

impl Check for Triplanar {
    fn check(&self, path: &str) -> Result<(), GraphError> {
        finite(self.tile_metres, &format!("{path}.tile_metres"), "a tiling")?;
        require(
            self.tile_metres > 0.0,
            &format!("{path}.tile_metres"),
            "a repeat covers some metres of world space; zero of them is a source read at one \
             point",
        )?;
        finite(self.sharpness, &format!("{path}.sharpness"), "a sharpness")?;
        require(
            self.sharpness >= 1.0,
            &format!("{path}.sharpness"),
            "a sharpness below one blends the far side of the surface into the near one; one is \
             the normal's own lanes and is the softest blend this node makes",
        )
    }

    fn check_types(&self, inputs: &[Resolved], path: &str) -> Result<(), GraphError> {
        let tiles = inputs
            .iter()
            .find(|input| input.name == "source")
            .is_none_or(|source| source.period.is_tiled());
        require(
            tiles,
            &format!("{path}.inputs[source]"),
            "a triplanar reads its source by world position, so the source meets itself every \
             `tile_metres` along each axis or it does not meet itself at all; this one does not \
             tile",
        )
    }
}

/// Which world-space vector a [`WorldMask`] reads.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub enum WorldField {
    /// The fragment's world normal, so the mask is about which way a face
    /// points. The value compared against the threshold is in `-1..=1`.
    #[default]
    Normal,
    /// The fragment's world position, so the mask is about where a face is.
    /// The value compared against the threshold is in metres.
    Position,
}

/// One direction in the world, signed.
///
/// Six rather than three and a flag: "the faces that point up" and "the faces
/// that point down" are two masks an author writes, and a boolean beside an
/// axis is a spelling of the same six that reads worse in a RON file.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub enum WorldAxis {
    /// Towards positive x.
    X,
    /// Towards positive y, which is up.
    #[default]
    Y,
    /// Towards positive z.
    Z,
    /// Towards negative x.
    NegX,
    /// Down.
    NegY,
    /// Towards negative z.
    NegZ,
}

impl WorldAxis {
    /// Which lane of the world-space vector this axis reads, and whether the
    /// lane is negated.
    fn lane(self) -> (u8, bool) {
        match self {
            Self::X => (0, false),
            Self::Y => (1, false),
            Self::Z => (2, false),
            Self::NegX => (0, true),
            Self::NegY => (1, true),
            Self::NegZ => (2, true),
        }
    }
}

/// Up-facing faces, low faces, north walls: one axis of the world as a mask.
///
/// A soft threshold on one component of the world normal or the world position,
/// and the shortcut an author was going to write out of [`WorldNormal`],
/// [`Decompose`](crate::nodes::Decompose) and a
/// [`Math`](crate::nodes::Math) `Smoothstep` anyway. `1` where the component is
/// past the threshold, `0` where it is short of it, and a ramp of `softness`
/// either side; a softness of zero is a hard edge, which rests on the crate's
/// own guarded smoothstep answering a step where its edges meet.
///
/// The two things it is for:
///
/// - **Which way a face points.** `WorldMask::up()` is one on the faces rain
///   lands on and zero on the walls under them — dust on a ledge, snow on a
///   sill, wetness on a coping — and it is a decision a *material* makes rather
///   than a slot somebody binds, which is the whole of why this node exists.
/// - **Where a face is.** `WorldMask::below(2.0)` is one within two metres of
///   the ground, which is as far as splash reaches and as high as anybody
///   scuffs a wall.
///
/// Period-neutral, as [`Time`] explains. **A bake refuses it**, as it refuses
/// the [`WorldNormal`] or [`WorldPos`] it reads.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
#[must_use]
pub struct WorldMask {
    /// Whether the mask is about which way a face points or about where it is.
    pub field: WorldField,
    /// The direction the component is taken along.
    pub axis: WorldAxis,
    /// Where the mask reaches a half: a cosine in `-1..=1` for
    /// [`WorldField::Normal`], and metres for [`WorldField::Position`].
    pub threshold: f32,
    /// Half the width of the ramp, in the threshold's own units. Zero is a
    /// hard edge.
    pub softness: f32,
}

/// The default threshold of a facing mask: sixty degrees off the axis.
///
/// A half, because that is the cosine of sixty degrees: a face within sixty
/// degrees of pointing up is one rain lands on, and a wall leaning back by less
/// than that is one it runs down.
const FACING_THRESHOLD: f32 = 0.5;

/// The default ramp of either mask.
///
/// A quarter — of a cosine for a facing mask, and of a metre for a height —
/// which is a ramp an author can see without being one they have to think
/// about, and is the number both constructors start from.
const WORLD_SOFTNESS: f32 = 0.25;

impl Default for WorldMask {
    fn default() -> Self {
        Self::up()
    }
}

impl WorldMask {
    /// The faces that point up.
    pub fn up() -> Self {
        Self::facing(WorldAxis::Y)
    }

    /// The faces that point along one axis.
    pub fn facing(axis: WorldAxis) -> Self {
        Self {
            field: WorldField::Normal,
            axis,
            threshold: FACING_THRESHOLD,
            softness: WORLD_SOFTNESS,
        }
    }

    /// The faces standing at or above `metres` of world height.
    pub fn above(metres: f32) -> Self {
        Self {
            field: WorldField::Position,
            axis: WorldAxis::Y,
            threshold: metres,
            softness: WORLD_SOFTNESS,
        }
    }

    /// The faces standing at or below `metres` of world height.
    ///
    /// The mask is one *under* the line, which is a threshold on the downward
    /// axis: the component read is `-y` and the threshold is `-metres`, so the
    /// same smooth ramp serves both directions and there is no second rule for
    /// a mask that runs the other way.
    pub fn below(metres: f32) -> Self {
        Self {
            axis: WorldAxis::NegY,
            threshold: -metres,
            ..Self::above(metres)
        }
    }

    /// Read the world position rather than the world normal, or the other way.
    pub fn field(mut self, field: WorldField) -> Self {
        self.field = field;
        self
    }

    /// Take the component along another axis.
    pub fn axis(mut self, axis: WorldAxis) -> Self {
        self.axis = axis;
        self
    }

    /// Where the mask reaches a half.
    pub fn threshold(mut self, threshold: f32) -> Self {
        self.threshold = threshold;
        self
    }

    /// Half the width of the ramp. Zero is a hard edge.
    pub fn softness(mut self, softness: f32) -> Self {
        self.softness = softness;
        self
    }
}

ports!(WorldMask, Output::Fixed(ValueType::Float));

impl Check for WorldMask {
    fn check(&self, path: &str) -> Result<(), GraphError> {
        finite(self.threshold, &format!("{path}.threshold"), "a threshold")?;
        finite(self.softness, &format!("{path}.softness"), "a softness")?;
        require(
            self.softness >= 0.0,
            &format!("{path}.softness"),
            "a softness is half the width of a ramp, which is never negative",
        )
    }
}

/// The period every node in this module carries, and the one rule they share.
///
/// [`Period::UNIT`]: none of them is a field over UV, so the neutral element of
/// the least common multiple is the honest answer. [`Time`] writes out why at
/// length.
pub(crate) const WORLD_PERIOD: Period = Period::UNIT;

/// Refuse a world-space input to a bake, at the node the driver is lowering.
///
/// `None` for a shader, which is where these mean something. The message names
/// the surface that answers rather than only the one that does not, because an
/// author who wrote a triplanar wants to know what to do with it.
fn outside_a_bake(cx: &mut Lowering, what: &str) -> Option<ValueId> {
    matches!(cx.target(), Target::Bake).then(|| {
        cx.reject(format!(
            "{what} is a fact about the mesh and the frame it is drawn in, and a texture has no \
             axis to store one along; deliver this material as a Shader surface, which is where \
             a world-space input means anything"
        ))
    })
}

/// The world position, or the refusal a bake answers instead.
fn world_pos(cx: &mut Lowering) -> ValueId {
    outside_a_bake(cx, "a world position").unwrap_or_else(|| cx.emit(Op::WorldPos, []))
}

/// The world normal, or the refusal a bake answers instead.
fn world_normal(cx: &mut Lowering) -> ValueId {
    outside_a_bake(cx, "a world normal").unwrap_or_else(|| cx.emit(Op::WorldNormal, []))
}

/// The clock and the cut flag: one source instruction and no operands.
macro_rules! source {
    ($($ty:ty => $op:ident),* $(,)?) => {
        $(
            impl Lower for $ty {
                fn lower(&self, cx: &mut Lowering, _inputs: &NodeInputs) -> ValueId {
                    cx.emit(Op::$op, [])
                }
            }
        )*
    };
}

source! {
    Time => Time,
    CutFlag => CutFlag,
}

/// The world-space pair: the same instruction, and a bake refused by path.
impl Lower for WorldPos {
    fn lower(&self, cx: &mut Lowering, _inputs: &NodeInputs) -> ValueId {
        world_pos(cx)
    }
}

impl Lower for WorldNormal {
    fn lower(&self, cx: &mut Lowering, _inputs: &NodeInputs) -> ValueId {
        world_normal(cx)
    }
}

/// Three projections of the source, weighted by the normal's own lanes.
///
/// ```text
/// w = |n| ^ sharpness
/// out = (w.x * in(p.zy / m) + w.y * in(p.xz / m) + w.z * in(p.xy / m)) / (w.x + w.y + w.z)
/// ```
///
/// The pairing of lanes is the usual one and is a choice about handedness
/// rather than about mathematics: each plane keeps the two axes the projection
/// does not run along, in the order that leaves the world's up axis the
/// projection's own v wherever there is one to keep. The division is
/// [`Op::Div`], which answers zero rather than an infinity where the sum is
/// zero — a normal of zero has no axis to favour, and zero is the field read
/// nowhere rather than a `NaN` spread across the surface.
///
/// The weights are raised through [`Op::Pow`], whose base here is an absolute
/// value and so never negative, which is the one case WGSL's `pow` and
/// `f32::powf` disagree about.
impl Lower for Triplanar {
    fn lower(&self, cx: &mut Lowering, inputs: &NodeInputs) -> ValueId {
        let source = inputs.value("source");
        let position = world_pos(cx);
        let normal = world_normal(cx);
        let scale = k(cx, 1.0 / self.tile_metres);
        let scaled = mul(cx, position, scale);
        let (x, y, z) = (
            cx.emit(Op::Extract(0), [scaled]),
            cx.emit(Op::Extract(1), [scaled]),
            cx.emit(Op::Extract(2), [scaled]),
        );
        let planes = [vec2(cx, z, y), vec2(cx, x, z), vec2(cx, x, y)];
        let magnitude = abs(cx, normal);
        let sharpness = k(cx, self.sharpness);
        let weights = cx.emit(Op::Pow, [magnitude, sharpness]);
        let mut total = None;
        let mut sum = None;
        for (lane, uv) in planes.into_iter().enumerate() {
            let weight = cx.emit(Op::Extract(lane_of(lane)), [weights]);
            let projected = cx.substitute(source, uv);
            let term = mul(cx, projected, weight);
            total = Some(match total {
                Some(running) => add(cx, running, term),
                None => term,
            });
            sum = Some(match sum {
                Some(running) => add(cx, running, weight),
                None => weight,
            });
        }
        match (total, sum) {
            (Some(total), Some(sum)) => div(cx, total, sum),
            // Three planes are three planes; the loop above cannot leave none.
            _ => cx.constant(0.0),
        }
    }
}

/// A lane index as the byte [`Op::Extract`] carries.
///
/// Three lanes, so the cast is exact; it is written as a function rather than
/// inline because the loop above is the only place in the crate that indexes a
/// lane by a counter.
fn lane_of(lane: usize) -> u8 {
    u8::try_from(lane).unwrap_or(0)
}

/// One component of a world-space vector through a smooth threshold.
///
/// `smoothstep(threshold - softness, threshold + softness, component)`, with
/// the component negated where the axis is. That is the same guarded smoothstep
/// [`HeightToMask`](crate::nodes::HeightToMask) rides on, so a softness of zero
/// is a step rather than a division by zero, in both backends.
impl Lower for WorldMask {
    fn lower(&self, cx: &mut Lowering, _inputs: &NodeInputs) -> ValueId {
        let vector = match self.field {
            WorldField::Normal => world_normal(cx),
            WorldField::Position => world_pos(cx),
        };
        let (lane, negated) = self.axis.lane();
        let component = cx.emit(Op::Extract(lane), [vector]);
        let component = if negated {
            let flip = k(cx, -1.0);
            mul(cx, component, flip)
        } else {
            component
        };
        let low = k(cx, self.threshold - self.softness);
        let high = k(cx, self.threshold + self.softness);
        smoothstep(cx, low, high, component)
    }
}
