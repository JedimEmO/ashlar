//! The typed per-texel expression IR every node lowers to.
//!
//! A node lowers once. `Bricks` emits the same instructions for the bake and
//! for the shader, so a node author never writes WGSL, and only the three
//! dozen [`Op`]s are implemented twice: in [`interp`](crate::interp) and, when
//! the shader backend lands, in the emitter. That is the set the conformance
//! test between the two backends covers.
//!
//! The arena is SSA and typed. An [`Ir`] is a list of [`Inst`]s in which every
//! operand names an earlier instruction, so evaluating the list front to back
//! evaluates the expression, and [`Ir::roots`] says which values are the
//! material's outputs. [`Lowering`] is what a node emits into, and it does the
//! three cheap things a compiler does while it emits: it folds constants,
//! which is how an [`Exposure::Bake`](crate::Exposure) parameter disappears; it
//! gives one id to identical sub-expressions, so a noise three nodes read is
//! computed once; and at [`Lowering::finish`] it drops every instruction no
//! root reaches.
//!
//! [`Op::Sample`] is the one boundary in it. A buffered filter cannot be a
//! texel's expression, so it leaves a [`BufferPlan`] — what to rasterise, what
//! [`Filter`] to run over the plane, and at what resolution — and reads the
//! result back through a sample. `finish` prunes and shares those plans the way
//! it prunes and shares instructions, because a plane is the expensive thing in
//! a bake.
//!
//! Randomness is integer hashing and nothing else. [`Op::Hash2`] takes a
//! lattice coordinate and the period to reduce it by, so the lattice wraps and
//! a noise built on it tiles by construction. The hash is the wrapping
//! multiply-xorshift the showcase's hand-written pixel loop used before there
//! were graphs, so the first graphs reproduced its textures exactly.
//!
//! ```
//! use ashlar_material::{
//!     Channel, MaterialGraph, PbrOutput,
//!     interp::{Inputs, Interpreter},
//!     ir::{Target, lower},
//!     nodes::{Decompose, Uv},
//! };
//!
//! // A material whose roughness is the u coordinate.
//! let material = MaterialGraph::builder("test:ramp")
//!     .node("uv", Uv::new())
//!     .node("u", Decompose::new("uv", Channel::R))
//!     .output(PbrOutput::new().roughness("u"))
//!     .build()?;
//!
//! let ir = lower(&material, Target::Bake)?;
//! let roughness = ir.root("roughness").expect("roughness is always bound");
//! let interpreter = Interpreter::new(&ir);
//! let value = interpreter.eval_float([0.25, 0.5], &Inputs::default(), roughness)?;
//! assert!((value - 0.25).abs() < 1e-6);
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```

use std::borrow::Cow;
use std::collections::{BTreeMap, HashMap};

use crate::{
    GraphError, GraphParam, Input, LUMINANCE, Material, Node, Value,
    interp::{apply, narrow},
    nodes::{Accepts, CurvatureKind, Output, Subgraph},
};

/// The most operands any [`Op`] takes, which is what makes an [`Inst`] `Copy`.
pub const MAX_OPERANDS: usize = 3;

/// The type of an IR value.
///
/// Three widths and no more: the graph's `Float` is one of these, its `Color`
/// is a [`Self::Vec3`], and its `Vec2` is a [`Self::Vec2`]. The ordering is by
/// width, so the wider of two operand types is their `max`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum IrType {
    /// One channel.
    Float,
    /// A UV or an offset.
    Vec2,
    /// Linear RGB, a world position or a normal.
    Vec3,
}

impl IrType {
    /// How many of a register's three lanes this type uses.
    pub fn components(self) -> usize {
        match self {
            Self::Float => 1,
            Self::Vec2 => 2,
            Self::Vec3 => 3,
        }
    }

    /// A stable number for the cache key. Written out, not derived.
    pub(crate) fn code(self) -> u64 {
        match self {
            Self::Float => 1,
            Self::Vec2 => 2,
            Self::Vec3 => 3,
        }
    }
}

impl std::fmt::Display for IrType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Float => "Float",
            Self::Vec2 => "Vec2",
            Self::Vec3 => "Vec3",
        })
    }
}

/// One value in the arena: an index into [`Ir::insts`].
///
/// An id is only meaningful inside the [`Ir`] or [`Lowering`] that made it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ValueId(u32);

impl ValueId {
    /// Position in the instruction list.
    pub fn index(self) -> usize {
        self.0 as usize
    }

    /// The value at one position, which is how a pass that walks the arena by
    /// index names what it found.
    ///
    /// Saturating rather than failing: an arena is bounded by what a lowering
    /// can emit, so an index past `u32` is not one any walk of an [`Ir`]
    /// produced, and the id it would answer names no instruction either way.
    pub(crate) fn at(index: usize) -> Self {
        Self(u32::try_from(index).unwrap_or(u32::MAX))
    }
}

impl std::fmt::Display for ValueId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "v{}", self.0)
    }
}

/// A plane a buffered filter rasterises, and [`Op::Sample`] reads.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct BufferId(u32);

impl BufferId {
    /// Position in [`Ir::buffers`].
    pub fn index(self) -> usize {
        self.0 as usize
    }
}

impl std::fmt::Display for BufferId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "b{}", self.0)
    }
}

/// One primitive operation.
///
/// Component-wise ops run on all three lanes of a register at once, so an
/// operation on two [`IrType::Vec3`]s is the same arm as one on two
/// [`IrType::Float`]s; [`Lowering::emit`] broadcasts a scalar operand to the
/// width of its partner first, so the lanes always line up.
///
/// Equality and hashing compare a constant by its bits, so a folded `NaN` is
/// equal to itself and common-subexpression elimination never merges two
/// constants that differ.
#[derive(Clone, Copy, Debug)]
pub enum Op {
    /// A literal. No operands.
    Const(f32),
    /// The texel's coordinate in `0..1`, as a [`IrType::Vec2`]. No operands.
    Uv,
    /// A live parameter by its index in [`Ir::params`]. No operands.
    Param(u32),
    /// Seconds since the app started. No operands, and zero in a bake.
    Time,
    /// The fragment's world position. No operands, and zero in a bake.
    WorldPos,
    /// The fragment's world normal. No operands, and zero in a bake.
    WorldNormal,
    /// Whether the surface is a cut face. No operands, and zero in a bake.
    CutFlag,
    /// `a + b`.
    Add,
    /// `a - b`.
    Sub,
    /// `a * b`.
    Mul,
    /// `a / b`, and zero where `b` is zero, so a division never poisons a bake.
    Div,
    /// The smaller of `a` and `b`.
    Min,
    /// The larger of `a` and `b`.
    Max,
    /// `|a|`.
    Abs,
    /// The largest whole number no greater than `a`.
    Floor,
    /// `a - floor(a)`, so a negative coordinate wraps forward.
    Fract,
    /// The square root of `a`.
    Sqrt,
    /// `a` raised to `b`.
    Pow,
    /// Two raised to `a`.
    Exp2,
    /// The base-two logarithm of `a`.
    Log2,
    /// The sine of `a` in radians. A node that takes turns scales first.
    Sin,
    /// The cosine of `a` in radians.
    Cos,
    /// The angle of the point `(b, a)` in radians, in `-pi..=pi`.
    Atan2,
    /// `a + (b - a) * t`, over operands `(a, b, t)`.
    Mix,
    /// Zero below the edge and one at or above it, over operands `(edge, x)`.
    Step,
    /// The smooth ramp between two edges, over operands `(low, high, x)`.
    Smoothstep,
    /// `x` held between two bounds, over operands `(x, low, high)`.
    Clamp,
    /// `on_true` where the condition is at least `0.5`, else `on_false`, over
    /// operands `(condition, on_true, on_false)`. The condition is a scalar.
    Select,
    /// A hash of one lattice cell, in `0..=1`, over operands
    /// `(coordinate, period)`, both [`IrType::Vec2`], with the seed in the op.
    ///
    /// The coordinate is floored to a cell and each axis reduced modulo the
    /// matching component of `period`, which is what makes the lattice wrap;
    /// a period component below one means no reduction on that axis.
    Hash2(u32),
    /// The same hash over a three-dimensional lattice: operands
    /// `(coordinate, period)`, both [`IrType::Vec3`].
    Hash3(u32),
    /// The length of `a`.
    Length,
    /// The dot product of `a` and `b`.
    Dot,
    /// `a` scaled to unit length, or zero where `a` is zero.
    Normalize,
    /// A bilinear, wrapped read of a rasterised plane at the UV in operand
    /// zero. The boundary between per-texel expressions and buffered filters.
    Sample(BufferId),
    /// A vector from two or three operands, each of which is a scalar: the
    /// lane of a wider value is [`Self::Extract`]ed before it is composed.
    Compose,
    /// One lane of a vector, by index. Out of range reads zero.
    Extract(u8),
}

impl Op {
    /// The op's tag and payload.
    ///
    /// Equality, hashing and [`ir_hash`] all go through this. The tags are
    /// written out rather than taken from the discriminant because a cache key
    /// outlives a build: reordering this enum must not silently invalidate,
    /// or worse silently reuse, what a previous version wrote to disk.
    pub(crate) fn code(self) -> (u16, u32) {
        match self {
            Self::Const(value) => (0, value.to_bits()),
            Self::Uv => (1, 0),
            Self::Param(index) => (2, index),
            Self::Time => (3, 0),
            Self::WorldPos => (4, 0),
            Self::WorldNormal => (5, 0),
            Self::CutFlag => (6, 0),
            Self::Add => (7, 0),
            Self::Sub => (8, 0),
            Self::Mul => (9, 0),
            Self::Div => (10, 0),
            Self::Min => (11, 0),
            Self::Max => (12, 0),
            Self::Abs => (13, 0),
            Self::Floor => (14, 0),
            Self::Fract => (15, 0),
            Self::Sqrt => (16, 0),
            Self::Pow => (17, 0),
            Self::Exp2 => (18, 0),
            Self::Log2 => (19, 0),
            Self::Sin => (20, 0),
            Self::Cos => (21, 0),
            Self::Atan2 => (22, 0),
            Self::Mix => (23, 0),
            Self::Step => (24, 0),
            Self::Smoothstep => (25, 0),
            Self::Clamp => (26, 0),
            Self::Select => (27, 0),
            Self::Hash2(seed) => (28, seed),
            Self::Hash3(seed) => (29, seed),
            Self::Length => (30, 0),
            Self::Dot => (31, 0),
            Self::Normalize => (32, 0),
            Self::Sample(buffer) => (33, buffer.0),
            Self::Compose => (34, 0),
            Self::Extract(channel) => (35, u32::from(channel)),
        }
    }

    /// Whether the op reads only its operands, so constant operands make a
    /// constant result.
    ///
    /// [`Self::Compose`] is pure but not folded: a folded vector is written as
    /// a `Compose` of constants, so folding it would be folding it into itself.
    fn foldable(self) -> bool {
        !matches!(
            self,
            Self::Const(_)
                | Self::Uv
                | Self::Param(_)
                | Self::Time
                | Self::WorldPos
                | Self::WorldNormal
                | Self::CutFlag
                | Self::Sample(_)
                | Self::Compose
        )
    }

    /// Which operands share the result's width, and what that width is.
    ///
    /// `None` where the op imposes nothing: its operands are already the types
    /// the node that emitted them chose.
    fn alignment(self) -> Option<(usize, Option<IrType>)> {
        match self {
            Self::Add
            | Self::Sub
            | Self::Mul
            | Self::Div
            | Self::Min
            | Self::Max
            | Self::Pow
            | Self::Atan2
            | Self::Step
            | Self::Mix
            | Self::Smoothstep
            | Self::Clamp
            | Self::Dot => Some((0, None)),
            // The condition stays a scalar; the two branches share a width.
            Self::Select => Some((1, None)),
            Self::Hash2(_) => Some((0, Some(IrType::Vec2))),
            Self::Hash3(_) => Some((0, Some(IrType::Vec3))),
            _ => None,
        }
    }
}

impl PartialEq for Op {
    fn eq(&self, other: &Self) -> bool {
        self.code() == other.code()
    }
}

impl Eq for Op {}

impl std::hash::Hash for Op {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.code().hash(state);
    }
}

/// The operands of one instruction: at most [`MAX_OPERANDS`], held inline.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Operands {
    ids: [ValueId; MAX_OPERANDS],
    len: u8,
}

impl Operands {
    /// The operands, in the order the op documents.
    pub fn as_slice(&self) -> &[ValueId] {
        let len = usize::from(self.len).min(MAX_OPERANDS);
        &self.ids[..len]
    }

    /// How many operands there are.
    pub fn len(&self) -> usize {
        self.as_slice().len()
    }

    /// Whether the op takes none, as every source op does.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Build from a slice, keeping the first [`MAX_OPERANDS`].
    ///
    /// Every caller passes an array of that length or shorter, so nothing is
    /// ever dropped; the truncation is what keeps the type unfailable.
    pub(crate) fn from_ids(ids: &[ValueId]) -> Self {
        let mut slots = [ValueId::default(); MAX_OPERANDS];
        let mut len = 0;
        for (slot, id) in slots.iter_mut().zip(ids) {
            *slot = *id;
            len += 1;
        }
        Self { ids: slots, len }
    }
}

macro_rules! operands_from {
    ($($len:literal),* $(,)?) => {
        $(
            impl From<[ValueId; $len]> for Operands {
                fn from(ids: [ValueId; $len]) -> Self {
                    Self::from_ids(&ids)
                }
            }
        )*
    };
}
operands_from!(0, 1, 2, 3);

/// One instruction: an op, what it reads, and the type of what it defines.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Inst {
    /// What it computes.
    pub op: Op,
    /// What it computes it from.
    pub operands: Operands,
    /// The type of the value it defines.
    pub value_type: IrType,
}

/// One live parameter, kept as a uniform instead of folded.
///
/// The index of the binding in [`Ir::params`] is the payload of the
/// [`Op::Param`] that reads it, and the order is the order the graph declares
/// them, so a uniform block built from this list is stable across lowerings.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParamBinding {
    /// The parameter's name in the graph.
    pub name: String,
    /// The type the expression reads it as.
    pub value_type: IrType,
}

/// What a buffered node runs over the plane it rasterised.
///
/// Plain data, and deliberately not an [`Op`]: a filter reads a neighbourhood
/// rather than a texel, so it is ordinary Rust over the plane — in
/// [`planes`](crate::planes) — and this is what the plan hands it. Every radius
/// is a *reach* in UV units, so a filter describes the same surface at every
/// resolution.
///
/// Non-exhaustive: a filter a backend cannot run is refused at
/// [`BufferPlan::path`], the way a node without a lowering is.
#[derive(Clone, Copy, Debug, PartialEq)]
#[non_exhaustive]
pub enum Filter {
    /// None. An explicit cache point, or a plane at a pinned resolution.
    None,
    /// A separable Gaussian, wrapped, reaching `radius` UV units.
    Blur {
        /// How far the kernel reaches, in UV units.
        radius: f32,
    },
    /// The same Gaussian along one direction only, which is what makes a
    /// brushed metal out of a grain.
    Directional {
        /// How far the kernel reaches, in UV units.
        radius: f32,
        /// Which way it reaches, in radians, counter-clockwise from `+u`.
        radians: f32,
    },
    /// The plane smeared downhill along the slope of another one: `steps`
    /// small displacements of the reading coordinate, each `radius / steps`
    /// UV units down the gradient of `guide`, reduced with where it started.
    ///
    /// The one filter that reads a second plane, and the reason
    /// [`filter`](crate::planes::filter) takes the planes before it: a slope
    /// blur is a source and a *height*, and neither is a function of the
    /// other. `guide` names a plan earlier in [`Ir::buffers`] than the one
    /// carrying this filter, which is what makes reading it in order enough.
    Slope {
        /// How far the last step may have walked, in UV units.
        radius: f32,
        /// How many displacements, and so how smooth the smear is.
        steps: u32,
        /// How samples along the walk combine.
        mode: crate::nodes::SlopeMode,
        /// The plane whose gradient the walk follows.
        guide: BufferId,
    },
    /// Horizon-based occlusion over a height plane, wrapped. One where the
    /// texel sees the whole sky.
    Occlusion {
        /// How far to march for a horizon, in UV units.
        radius: f32,
        /// How much of the horizon it is allowed to take away.
        strength: f32,
    },
    /// The distance, in UV units, to the nearest texel at or above
    /// `threshold`, measured across the wrap.
    Distance {
        /// What counts as inside the mask.
        threshold: f32,
    },
    /// The smallest value within `radius`, wrapped.
    Erode {
        /// Half the width of the square neighbourhood, in UV units.
        radius: f32,
    },
    /// The largest value within `radius`, wrapped.
    Dilate {
        /// Half the width of the square neighbourhood, in UV units.
        radius: f32,
    },
    /// How far the plane stands above its own neighbourhood: positive on a
    /// crest, negative in a hollow, zero on anything flat or straight.
    ///
    /// A *difference of heights* rather than a second derivative, which is
    /// what keeps it the same field at every resolution and what makes its
    /// magnitude the relief of the feature rather than the relief divided by
    /// the square of a radius. It needs a gain to become a mask, and the node
    /// carries one.
    Curvature {
        /// How far the neighbourhood reaches, in UV units.
        radius: f32,
        /// Which neighbourhood: four taps, or the whole Gaussian.
        kind: CurvatureKind,
    },
    /// How much the plane changes across `radius`: the length of the pair of
    /// central differences, taken as a difference rather than as a slope, so
    /// that the edge of a mask is one whatever the radius.
    Edge {
        /// Half the span the difference is taken over, in UV units.
        radius: f32,
    },
    /// The tangent-space normal of a height plane, by wrapped central
    /// difference over the plane's own texels.
    ///
    /// This is the one filter whose answer is wider than what it read: a
    /// height plane in, a [`IrType::Vec3`] plane out. It is a filter rather
    /// than an expression because "at texel size" is a fact about the plane
    /// and not about the graph — the bake's own normal is derived the same
    /// way, from the same difference, so a graph that blends normals and a
    /// graph that binds a height describe one surface.
    Normal {
        /// Metres of relief a unit of height stands for across one repeat.
        strength: f32,
    },
    /// A strand layer of the same graph, splatted from directly above.
    ///
    /// The one filter that reads *nothing* from the plane it is handed: the
    /// field it answers is scattered from the [`StrandPlan`] its
    /// [`BufferPlan::strands`] carries rather than filtered from a
    /// sub-expression, so the plan's own root is a constant and the plane it
    /// would have rasterised is never written. See
    /// [`StrandRelief`](crate::nodes::StrandRelief) for why this is a plane at
    /// all, and [`strand_plane`](crate::planes::strand_plane) for what it
    /// costs.
    Strands {
        /// Which field of the layer this plane holds.
        output: crate::nodes::StrandReliefOutput,
    },
}

impl Filter {
    /// The filter's tag and its settings' bits.
    ///
    /// Written out for the same reason [`Op::code`] is: this goes into a cache
    /// key that outlives a build, so reordering the enum must not silently
    /// reuse what a previous version wrote. It is also what makes two plans
    /// with the same settings one plan.
    ///
    /// A [`Self::Slope`]'s guide is deliberately *not* in it: a [`BufferId`]
    /// is an index into one lowering's plan list and means nothing outside it,
    /// exactly as [`Op::Sample`]'s does. What stands in for it is the guide
    /// plane's own key, which [`plane_key`](crate::planes) adds, and the
    /// renumbered id, which [`Ir::finish`] compares.
    pub(crate) fn code(self) -> (u16, [u32; 2]) {
        match self {
            Self::None => (0, [0, 0]),
            Self::Blur { radius } => (1, [radius.to_bits(), 0]),
            Self::Occlusion { radius, strength } => (2, [radius.to_bits(), strength.to_bits()]),
            Self::Distance { threshold } => (3, [threshold.to_bits(), 0]),
            Self::Erode { radius } => (4, [radius.to_bits(), 0]),
            Self::Dilate { radius } => (5, [radius.to_bits(), 0]),
            Self::Normal { strength } => (6, [strength.to_bits(), 0]),
            Self::Directional { radius, radians } => (7, [radius.to_bits(), radians.to_bits()]),
            Self::Slope {
                radius,
                steps,
                mode,
                ..
            } => (
                match mode {
                    crate::nodes::SlopeMode::Average => 8,
                    crate::nodes::SlopeMode::Min => 11,
                    crate::nodes::SlopeMode::Max => 12,
                },
                [radius.to_bits(), steps],
            ),
            Self::Curvature { radius, kind } => (9, [radius.to_bits(), kind.code()]),
            Self::Edge { radius } => (10, [radius.to_bits(), 0]),
            // The output alone, because everything else a strand plane is
            // decided by lives in [`StrandPlan::key`], which every key that
            // carries a filter code writes beside it.
            Self::Strands { output } => (13, [output.code(), 0]),
        }
    }

    /// The plane this filter reads besides the one it was given, if any.
    ///
    /// Only a slope blur has one. Everything that walks the plan list — the
    /// dead-plane sweep, the renumbering, the cache key and the partition's
    /// own copy of the plans — goes through this rather than matching the
    /// variant, so a second two-plane filter is a line here and nowhere else.
    pub(crate) fn guide(self) -> Option<BufferId> {
        match self {
            Self::Slope { guide, .. } => Some(guide),
            _ => None,
        }
    }

    /// The same filter reading another plane as its guide.
    pub(crate) fn with_guide(self, replacement: BufferId) -> Self {
        match self {
            Self::Slope {
                radius,
                steps,
                mode,
                ..
            } => Self::Slope {
                radius,
                steps,
                mode,
                guide: replacement,
            },
            other => other,
        }
    }

    /// What the plane holds after this filter, given what was rasterised into
    /// it.
    ///
    /// Most filters answer in the width they read. The ones that do not are
    /// written out: an occlusion, a distance, a curvature and an edge are one
    /// number however wide the field they measured, and a normal is three
    /// however narrow the height it differenced. [`Lowering::buffer`] reads
    /// this rather than the root's own type, because [`Op::Sample`] takes the
    /// plane's width from the plan and a plan that lied about it would sample
    /// a plane as a shear.
    pub(crate) fn output_type(self, input: IrType) -> IrType {
        match self {
            Self::None
            | Self::Blur { .. }
            | Self::Directional { .. }
            | Self::Slope { .. }
            | Self::Erode { .. }
            | Self::Dilate { .. } => input,
            Self::Occlusion { .. }
            | Self::Distance { .. }
            | Self::Curvature { .. }
            | Self::Edge { .. } => IrType::Float,
            Self::Normal { .. } => IrType::Vec3,
            // Whatever the output holds, and never what was rasterised: a
            // strand plane reads no input plane at all.
            Self::Strands { output } => ir_type(output.value_type()),
        }
    }
}

/// The scatter one [`Filter::Strands`] plane is splatted from.
///
/// A [`BufferPlan`] is otherwise one sub-expression of the arena it lives in,
/// and a strand layer is not one: it is *ten* fields, read once per strand at
/// its root rather than once per texel, plus the two dozen constants that say
/// how long a strand is and how many of them there are. So the layer is lowered
/// on its own — [`lower_strands`], the same lowering
/// [`scatter`](crate::strands::scatter) runs, folded the same way — and the
/// finished [`Ir`] is carried here rather than spliced into the arena above it.
///
/// The cost of that choice is honest and small: a field the PBR half also reads
/// is lowered twice and rasterised twice, once in each arena. What it buys is
/// that nothing in the outer arena has to know a strand exists — the dead-value
/// sweep, the renumbering, the memo analysis and the partition all see a plan
/// whose root is a constant — and that the plane a bake computes here is
/// *bit for bit* the set a mesh builder scatters, because it is the same
/// lowering run over the same material.
///
/// # What identifies one
///
/// [`Self::key`], a canonical byte encoding of everything a splat depends on:
/// the layer's name, the repeat it is measured against, every constant on the
/// layer, and the whole of the lowered field expression. It is exact rather
/// than a digest, for the reason [`PlaneKey`](crate::planes::PlaneKey) is, and
/// it is what [`plane_key`](crate::planes) writes into the plane's own key,
/// what [`ir_hash`] hashes, and what tells two plans of two different layers
/// apart when their filters and their constant roots agree.
#[derive(Clone, Debug)]
pub struct StrandPlan {
    layer: String,
    repeat_metres: f32,
    settings: crate::StrandLayer,
    fields: Ir,
    key: std::sync::Arc<[u8]>,
}

impl PartialEq for StrandPlan {
    /// By the key alone, which is the point of the key: it encodes everything
    /// the other fields decide, so two plans with equal bytes splat equal
    /// planes and comparing the arenas again would only be slower.
    fn eq(&self, other: &Self) -> bool {
        self.key == other.key
    }
}

impl StrandPlan {
    /// The plan for one layer, with its key computed once.
    pub(crate) fn new(
        layer: &str,
        repeat_metres: f32,
        settings: crate::StrandLayer,
        fields: Ir,
    ) -> Self {
        let key = strand_key(layer, repeat_metres, &settings, &fields);
        Self {
            layer: layer.to_owned(),
            repeat_metres,
            settings,
            fields,
            key,
        }
    }

    /// The layer's name in the graph that declared it.
    pub fn layer(&self) -> &str {
        &self.layer
    }

    /// How many metres one repeat of the material covers, which is what turns
    /// the layer's metres into UV.
    pub fn repeat_metres(&self) -> f32 {
        self.repeat_metres
    }

    /// The layer as the graph declared it, parameters folded.
    pub fn settings(&self) -> &crate::StrandLayer {
        &self.settings
    }

    /// The layer's ten fields, lowered, with their own planes to rasterise.
    pub fn fields(&self) -> &Ir {
        &self.fields
    }

    /// The canonical bytes that identify this scatter.
    pub fn key(&self) -> &[u8] {
        &self.key
    }
}

/// Everything a splat depends on, as bytes, in the length-prefixed style
/// [`PlaneKey`](crate::planes::PlaneKey) is written in and for the same reason:
/// no value can run into the next one, so two different scatters cannot encode
/// the same stream.
///
/// The expression is written as the finished arena rather than as a closure
/// walk, because a finished [`Ir`] has already had its dead values dropped and
/// its live ones renumbered from zero: it is a function of the reachable graph
/// alone, so two lowerings of the same layer are the same bytes wherever the
/// nodes happened to sit.
fn strand_key(
    layer: &str,
    repeat_metres: f32,
    settings: &crate::StrandLayer,
    fields: &Ir,
) -> std::sync::Arc<[u8]> {
    let mut bytes: Vec<u8> = Vec::new();
    let number = |bytes: &mut Vec<u8>, value: u64| bytes.extend_from_slice(&value.to_le_bytes());
    number(&mut bytes, u64::try_from(layer.len()).unwrap_or(u64::MAX));
    bytes.extend_from_slice(layer.as_bytes());
    for word in [
        repeat_metres.to_bits(),
        // The size the layer's own planes are rasterised at. A constant today,
        // and in the key all the same: the day it stops being one, a cache that
        // did not name it would hand a lawn the blades of another resolution.
        crate::strands::FIELD_RESOLUTION,
        settings.count[0],
        settings.count[1],
        settings.jitter.to_bits(),
        settings.seed,
        settings.length_metres.to_bits(),
        settings.width_metres.to_bits(),
        settings.length_variation.to_bits(),
        settings.width_variation.to_bits(),
        settings.direction_variation.to_bits(),
        settings.lean_variation.to_bits(),
        settings.bend_variation.to_bits(),
        settings.segments,
        settings.taper.to_bits(),
        settings.root_occlusion.to_bits(),
        match settings.profile {
            crate::StrandProfile::Blade => 0,
            crate::StrandProfile::Fibre => 1,
        },
        // Phase 3b's constants. Every one of them moves the set or the
        // footprint, so a key that named none of them would hand a cached
        // plane of the old lawn to the new one.
        settings.per_cell,
        settings.midpoint.to_bits(),
        settings.facing_variation.to_bits(),
        settings.height_offset_metres.to_bits(),
        settings.clump_count[0],
        settings.clump_count[1],
        settings.clump_share.to_bits(),
        settings.clump_tips.to_bits(),
        settings.clump_tint.to_bits(),
    ] {
        number(&mut bytes, u64::from(word));
    }
    // The whole arena, plans included. `ir_hash` is a digest and this is not,
    // so the instructions are written out rather than folded into a word.
    number(&mut bytes, u64::try_from(fields.len()).unwrap_or(u64::MAX));
    for inst in fields.insts() {
        let (tag, payload) = inst.op.code();
        number(&mut bytes, u64::from(tag));
        number(&mut bytes, u64::from(payload));
        number(
            &mut bytes,
            u64::try_from(inst.operands.len()).unwrap_or(u64::MAX),
        );
        for operand in inst.operands.as_slice() {
            number(&mut bytes, u64::from(operand.0));
        }
        number(&mut bytes, inst.value_type.code());
    }
    number(
        &mut bytes,
        u64::try_from(fields.buffers().len()).unwrap_or(u64::MAX),
    );
    for plan in fields.buffers() {
        number(&mut bytes, u64::from(plan.root.0));
        let (tag, settings) = plan.filter.code();
        number(&mut bytes, u64::from(tag));
        for setting in settings {
            number(&mut bytes, u64::from(setting));
        }
        number(
            &mut bytes,
            plan.filter.guide().map_or(u64::MAX, |id| u64::from(id.0)),
        );
        number(&mut bytes, plan.value_type.code());
        number(&mut bytes, plan.resolution.map_or(0, u64::from));
    }
    number(
        &mut bytes,
        u64::try_from(fields.roots().len()).unwrap_or(u64::MAX),
    );
    for (port, value) in fields.roots() {
        number(&mut bytes, u64::try_from(port.len()).unwrap_or(u64::MAX));
        bytes.extend_from_slice(port.as_bytes());
        number(&mut bytes, u64::from(value.0));
    }
    std::sync::Arc::from(bytes)
}

/// One sub-expression that is rasterised into a plane before anything samples
/// it.
///
/// This is the bake boundary: the plane pipeline rasterises [`Self::root`] at
/// [`Self::resolution`] or the bake's own, runs [`Self::filter`] over the
/// plane, and exposes the result to [`Op::Sample`], which reads it bilinearly
/// with wrap.
///
/// The plans are in dependency order, and a plan may only sample a plane
/// earlier in the list: a [`Op::Sample`] exists only after the plan it names,
/// and a plan's root is an instruction that was already emitted when the plan
/// was made. So rasterising them in order is enough, and nothing has to sort.
#[derive(Clone, Debug, PartialEq)]
pub struct BufferPlan {
    /// What [`Op::Sample`] names.
    pub id: BufferId,
    /// Where the node that asked for the buffer is, in the form validation
    /// uses: `nodes[id]`. Where a failure to run the filter is reported.
    pub path: String,
    /// The expression to rasterise.
    pub root: ValueId,
    /// What to run over the plane.
    pub filter: Filter,
    /// What the plane holds.
    pub value_type: IrType,
    /// The resolution the node pinned, or `None` for the bake's own.
    pub resolution: Option<u32>,
    /// The scatter a [`Filter::Strands`] plane is splatted from, and `None` for
    /// every other plane.
    ///
    /// Shared rather than copied, because the three outputs of one layer are
    /// three planes over one scatter and the arena inside is the expensive
    /// thing to clone.
    pub strands: Option<std::sync::Arc<StrandPlan>>,
}

/// What an [`Ir`] is being built for.
///
/// A bake folds every parameter, because a texture has no uniforms. A shader
/// keeps the named ones, and everything downstream of them is evaluated per
/// fragment; the partition that decides what that costs is the shader
/// backend's, and lands with it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Target {
    /// The CPU bake, and the reference for every other backend.
    #[default]
    Bake,
    /// A fragment shader, with these parameters kept as uniforms.
    Shader {
        /// Parameter names that stay [`Op::Param`] instead of folding.
        live: Vec<String>,
    },
}

impl Target {
    /// A shader target whose live set is what the graph's author declared:
    /// every parameter exposed as [`Exposure::Live`](crate::Exposure::Live).
    pub fn shader_for(material: &Material) -> Self {
        Self::Shader {
            live: material
                .graph()
                .params
                .iter()
                .filter(|param| param.exposure == crate::Exposure::Live)
                .map(|param| param.name.clone())
                .collect(),
        }
    }

    /// Whether this target keeps `name` as a uniform.
    fn keeps(&self, name: &str) -> bool {
        match self {
            Self::Bake => false,
            Self::Shader { live } => live.iter().any(|param| param == name),
        }
    }

    /// A stable code for the cache key.
    fn code(&self) -> u64 {
        match self {
            Self::Bake => 0,
            Self::Shader { .. } => 1,
        }
    }
}

/// A lowered material: the expression arena, its roots, and its buffer plan.
#[derive(Clone, Debug, PartialEq)]
pub struct Ir {
    insts: Vec<Inst>,
    roots: BTreeMap<String, ValueId>,
    params: Vec<ParamBinding>,
    buffers: Vec<BufferPlan>,
    target: Target,
}

impl Ir {
    /// Every instruction, in evaluation order.
    pub fn insts(&self) -> &[Inst] {
        &self.insts
    }

    /// One instruction by id.
    pub fn inst(&self, value: ValueId) -> Option<Inst> {
        self.insts.get(value.index()).copied()
    }

    /// The type of one value.
    pub fn type_of(&self, value: ValueId) -> Option<IrType> {
        self.inst(value).map(|inst| inst.value_type)
    }

    /// How many instructions there are, which is the cost a report quotes.
    pub fn len(&self) -> usize {
        self.insts.len()
    }

    /// Whether the expression is empty, which only an empty root set can be.
    pub fn is_empty(&self) -> bool {
        self.insts.is_empty()
    }

    /// The output expressions, by [`PbrOutput`](crate::PbrOutput) port name.
    pub fn roots(&self) -> &BTreeMap<String, ValueId> {
        &self.roots
    }

    /// One output expression by port name: `base_color`, `roughness`,
    /// `metallic`, `occlusion`, `height` or `emissive`.
    pub fn root(&self, port: &str) -> Option<ValueId> {
        self.roots.get(port).copied()
    }

    /// The live parameters, in uniform-block order.
    ///
    /// Every parameter the target keeps is here, whether or not any expression
    /// reads it: dropping the dead instructions does not drop a binding,
    /// because a uniform block whose layout moved when an author unplugged a
    /// slider would invalidate every pipeline built against it. A binding no
    /// [`Op::Param`] names is therefore ordinary, and it does count towards
    /// [`ir_hash`].
    pub fn params(&self) -> &[ParamBinding] {
        &self.params
    }

    /// The planes a bake must rasterise before the roots can be evaluated, in
    /// an order that satisfies their dependencies.
    ///
    /// Every plan here is one a live [`Op::Sample`] reads: a buffered node
    /// whose output no output reaches leaves no plane behind, and two nodes
    /// that asked for the same filter over the same expression at the same
    /// resolution share one plan, because a plane is the expensive thing in a
    /// bake and computing it twice is the mistake this list exists to avoid.
    pub fn buffers(&self) -> &[BufferPlan] {
        &self.buffers
    }

    /// Which instructions the given values reach, as a flag per index.
    ///
    /// What a backend evaluating only part of an expression needs: the plane
    /// pipeline rasterises one buffer root at a time, and the texel pass runs
    /// the outputs and not the sub-expressions that already became planes.
    ///
    /// This does not follow an [`Op::Sample`] into the plan it names. A sample
    /// reads a plane, and a plane is rasterised once rather than per texel, so
    /// what it reaches is not part of what the instruction costs.
    pub fn reaches(&self, values: &[ValueId]) -> Vec<bool> {
        let mut live = vec![false; self.insts.len()];
        for value in values {
            mark(*value, &mut live);
        }
        // Operands are always earlier than the instruction that reads them, so
        // one backwards sweep marks everything.
        for index in (0..self.insts.len()).rev() {
            if live.get(index).copied() != Some(true) {
                continue;
            }
            let Some(inst) = self.insts.get(index) else {
                continue;
            };
            for operand in inst.operands.as_slice() {
                mark(*operand, &mut live);
            }
        }
        live
    }

    /// What this was lowered for.
    pub fn target(&self) -> &Target {
        &self.target
    }

    /// The same expression with more values named as roots.
    ///
    /// Nothing is lowered, folded or dropped: the arena, the plans and the
    /// bindings are the ones that were already here, and all that changes is
    /// which values [`Self::roots`] answers for. That is exactly what a
    /// backend evaluating the expression in *pieces* needs — a GPU bake
    /// dispatches one kernel per plane and one for the outputs, and a kernel
    /// is written for named ports — and it is safe to do after the fact
    /// precisely because a finished [`Ir`] has no dead instructions in it: a
    /// value that is here is reached by something, so naming it cannot revive
    /// anything the dead-code pass had a reason to drop.
    ///
    /// A name already taken is replaced, and a value past the end of the arena
    /// is an error rather than a root that samples nothing.
    pub fn rooted_at(
        mut self,
        extra: impl IntoIterator<Item = (String, ValueId)>,
    ) -> Result<Self, GraphError> {
        for (port, value) in extra {
            crate::require(
                value.index() < self.insts.len(),
                "output",
                &format!("{port} names {value}, which this expression does not hold"),
            )?;
            self.roots.insert(port, value);
        }
        Ok(self)
    }
}

/// FNV-1a, written out rather than taken from the standard library's hasher.
///
/// A cache key is written to disk and compared across machines and across
/// compiler versions; `DefaultHasher` promises neither. Nothing here depends on
/// the order a hash map iterated, so the same IR hashes the same everywhere.
pub(crate) struct Fnv(u64);

impl Fnv {
    const OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const PRIME: u64 = 0x0000_0100_0000_01b3;

    pub(crate) fn new() -> Self {
        Self(Self::OFFSET)
    }

    fn byte(&mut self, value: u8) {
        self.0 ^= u64::from(value);
        self.0 = self.0.wrapping_mul(Self::PRIME);
    }

    /// A count, so two sections cannot run into one another.
    pub(crate) fn count(&mut self, value: usize) {
        self.number(u64::try_from(value).unwrap_or(u64::MAX));
    }

    pub(crate) fn number(&mut self, value: u64) {
        for byte in value.to_le_bytes() {
            self.byte(byte);
        }
    }

    /// A string and a terminator. `0xff` is not a byte UTF-8 can contain, so
    /// no string can forge the end of another one.
    pub(crate) fn text(&mut self, value: &str) {
        for byte in value.bytes() {
            self.byte(byte);
        }
        self.byte(0xff);
    }

    /// What the hash has come to.
    pub(crate) fn finish(&self) -> u64 {
        self.0
    }
}

/// A stable 64-bit hash of a lowered material, for a cache key.
///
/// Two lowerings of the same graph, parameters and target hash the same in any
/// process on any machine; changing a constant, a root, a live parameter or a
/// buffer changes the hash. It hashes the instruction stream itself rather than
/// the graph: two graphs that fold to the same expression hash the same, and
/// so do two [`Target::Shader`]s whose live sets resolved to the same bindings.
/// `ashlar-bevy`'s texture caches key on the bake request itself instead,
/// exactly, because a collision there would be one wall wearing another's
/// texels.
pub fn ir_hash(ir: &Ir) -> u64 {
    let mut hash = Fnv::new();
    hash.number(ir.target.code());
    // The bindings the live set resolved to, and not the live set itself: a
    // name no parameter answers to produced no binding and no instruction, so
    // it cannot make this a different expression.
    hash.count(ir.params.len());
    for binding in &ir.params {
        hash.text(&binding.name);
        hash.number(binding.value_type.code());
    }
    hash.count(ir.buffers.len());
    for plan in &ir.buffers {
        hash.number(u64::from(plan.id.0));
        hash.text(&plan.path);
        hash.number(u64::from(plan.root.0));
        let (tag, settings) = plan.filter.code();
        hash.number(u64::from(tag));
        for setting in settings {
            hash.number(u64::from(setting));
        }
        // A guide is a plan of this same list, and a renumbered one at that,
        // so the id is meaningful here in a way it is not in `Filter::code`.
        hash.number(
            plan.filter
                .guide()
                .map_or(u64::MAX, |guide| u64::from(guide.0)),
        );
        hash.number(plan.value_type.code());
        hash.number(plan.resolution.map_or(0, u64::from));
        // A strand plane's contents are none of the arena's, so the bytes that
        // do decide them are hashed here; without this two layers' coverage
        // planes hash the same and one bake cache entry answers for both.
        match &plan.strands {
            Some(strands) => {
                hash.count(strands.key.len());
                for byte in strands.key.iter() {
                    hash.number(u64::from(*byte));
                }
            }
            None => hash.count(0),
        }
    }
    hash.count(ir.roots.len());
    for (port, value) in &ir.roots {
        hash.text(port);
        hash.number(u64::from(value.0));
    }
    hash.count(ir.insts.len());
    for inst in &ir.insts {
        let (tag, payload) = inst.op.code();
        hash.number(u64::from(tag));
        hash.number(u64::from(payload));
        hash.count(inst.operands.len());
        for operand in inst.operands.as_slice() {
            hash.number(u64::from(operand.0));
        }
        hash.number(inst.value_type.code());
    }
    hash.finish()
}

/// One parameter as lowering sees it: a value to fold, or a uniform to keep.
struct ParamSlot {
    name: String,
    value: Value,
    binding: Option<u32>,
}

/// The context a node lowers into.
///
/// Emitting is infallible on purpose: a node lowering reads as arithmetic, and
/// the things that can go wrong — a node this backend does not implement, a
/// conversion no port allows — are recorded through [`Self::reject`] and
/// returned once by [`Self::finish`], with the path the driver was at.
#[derive(Debug)]
pub struct Lowering {
    insts: Vec<Inst>,
    unique: HashMap<(Op, Operands), ValueId>,
    next: u32,
    params: Vec<ParamSlot>,
    /// What an instance bound to the inputs of the graph being lowered, empty
    /// outside an instanced one.
    bound: BTreeMap<String, ValueId>,
    bindings: Vec<ParamBinding>,
    buffers: Vec<BufferPlan>,
    target: Target,
    path: String,
    kind: &'static str,
    error: Option<GraphError>,
    /// The strand layers being lowered above this context, outermost first.
    ///
    /// Empty everywhere but inside [`lower_strands`], and what keeps a
    /// [`StrandRelief`](crate::nodes::StrandRelief) from lowering a layer that
    /// is already being lowered. The driver lowers *every* node of a graph,
    /// including one nothing reads, so the guard cannot be reachability alone:
    /// a graph whose PBR half splats its own blades would otherwise recurse
    /// the moment a mesh builder scattered them. A relief a field actually
    /// reaches is refused by path before any of this, in [`lower_strands`].
    strands: Vec<String>,
}

impl std::fmt::Debug for ParamSlot {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ParamSlot")
            .field("name", &self.name)
            .field("binding", &self.binding)
            .finish_non_exhaustive()
    }
}

impl Lowering {
    /// An empty context with no parameters, for an expression built by hand.
    pub fn new(target: Target) -> Self {
        Self {
            insts: Vec::new(),
            unique: HashMap::new(),
            next: 0,
            params: Vec::new(),
            bound: BTreeMap::new(),
            bindings: Vec::new(),
            buffers: Vec::new(),
            target,
            path: String::new(),
            kind: "expression",
            error: None,
            strands: Vec::new(),
        }
    }

    /// A context that knows the material's parameters, and which of them the
    /// target keeps.
    fn for_material(material: &Material, target: Target) -> Self {
        let mut context = Self::new(target);
        for param in &material.graph().params {
            let binding = if context.target.keeps(&param.name) {
                let index = index_of(context.bindings.len());
                context.bindings.push(ParamBinding {
                    name: param.name.clone(),
                    value_type: ir_type(param.value.value_type()),
                });
                Some(index)
            } else {
                None
            };
            context.params.push(ParamSlot {
                name: param.name.clone(),
                value: param.value.value(),
                binding,
            });
        }
        context
    }

    /// What this lowering is for.
    pub fn target(&self) -> &Target {
        &self.target
    }

    /// A context with a uniform block already declared.
    ///
    /// What [`partition`](crate::partition) rebuilds a runtime expression into:
    /// an [`Op::Param`] carries an index into this list and nothing else, so a
    /// lowering that re-emits one has to have been handed the same bindings or
    /// it would give the parameter the wrong width. The slots
    /// [`Self::param`] looks names up in stay empty, because nothing here
    /// resolves a parameter by name — the indices are already bound.
    pub(crate) fn with_bindings(target: Target, bindings: Vec<ParamBinding>) -> Self {
        Self {
            bindings,
            ..Self::new(target)
        }
    }

    /// Where the driver is, for the next [`Self::reject`], and what path the
    /// next [`Self::buffer`] records.
    pub(crate) fn at(&mut self, path: String, kind: &'static str) {
        self.path = path;
        self.kind = kind;
    }

    /// Record a failure and answer a zero, so lowering runs to the end and the
    /// first thing that was wrong is what the caller is told about.
    pub fn reject(&mut self, reason: impl Into<String>) -> ValueId {
        let path = self.path.clone();
        self.fail(GraphError::new(path, reason))
    }

    /// The same, for a failure that already knows where it happened.
    ///
    /// What a driver that ran a *second* lowering needs: the layer under a
    /// strand relief is lowered in an arena of its own, and an error from
    /// inside it names a node of that layer's fields, which is the node to go
    /// and look at rather than the relief that asked.
    pub(crate) fn fail(&mut self, error: GraphError) -> ValueId {
        if self.error.is_none() {
            self.error = Some(error);
        }
        self.constant(0.0)
    }

    /// Whether a strand layer is being lowered above this context.
    pub(crate) fn inside_strands(&self) -> bool {
        !self.strands.is_empty()
    }

    /// Reject the node being lowered as one this backend does not implement.
    pub(crate) fn unsupported(&mut self) -> ValueId {
        let kind = self.kind;
        self.reject(format!("{kind} has no lowering yet"))
    }

    /// The type of a value. An id from another lowering answers `Float`.
    pub fn type_of(&self, value: ValueId) -> IrType {
        self.insts
            .get(value.index())
            .map_or(IrType::Float, |inst| inst.value_type)
    }

    /// A literal.
    pub fn constant(&mut self, value: f32) -> ValueId {
        self.push(Op::Const(value), Operands::default(), IrType::Float)
    }

    /// A graph literal, as the width its type carries.
    pub fn value(&mut self, value: Value) -> ValueId {
        match value {
            Value::Float(value) => self.constant(value),
            Value::Color(rgb) => {
                let components: Vec<ValueId> =
                    rgb.iter().map(|channel| self.constant(*channel)).collect();
                self.vector(&components)
            }
            Value::Vec2(uv) => {
                let components: Vec<ValueId> = uv.iter().map(|axis| self.constant(*axis)).collect();
                self.vector(&components)
            }
        }
    }

    /// The texel coordinate.
    pub fn uv(&mut self) -> ValueId {
        self.emit(Op::Uv, [])
    }

    /// A vector from two or three scalars.
    ///
    /// [`Op::Compose`] reads one lane from each operand, so a component that is
    /// already a vector is rejected rather than quietly narrowed to its first
    /// lane: `vec3(uv, 0)` is written by extracting the lanes of `uv` first.
    /// [`Self::emit`] holds that contract up for anything that composes without
    /// coming through here.
    pub fn vector(&mut self, components: &[ValueId]) -> ValueId {
        match components {
            [u, v] => self.emit(Op::Compose, [*u, *v]),
            [r, g, b] => self.emit(Op::Compose, [*r, *g, *b]),
            _ => self.reject("a vector is two or three components"),
        }
    }

    /// The value of an exposed parameter: folded, or read from the uniform
    /// block when the target keeps it live.
    pub fn param(&mut self, name: &str) -> ValueId {
        let Some(slot) = self.params.iter().find(|slot| slot.name == name) else {
            return self.reject(format!("unknown parameter {name:?}"));
        };
        let (binding, value) = (slot.binding, slot.value);
        if let Some(index) = binding {
            self.emit(Op::Param(index), [])
        } else {
            self.value(value)
        }
    }

    /// The value an instance bound to one of this graph's inputs.
    ///
    /// A [`GraphInput`](crate::nodes::GraphInput) asks for its own name here
    /// and lowers its default where the answer is `None`, which is every input
    /// of a graph lowered on its own and every input an instance left unbound.
    /// What pushes a binding is the driver that inlines a subgraph, the way it
    /// pushes the instanced graph's parameters.
    pub fn bound(&self, name: &str) -> Option<ValueId> {
        self.bound.get(name).copied()
    }

    /// Plan a plane for a buffered filter, and answer the id that samples it.
    ///
    /// The plan is reported at the path the driver is at, which while a node is
    /// lowering is that node: a filter a backend cannot run names the node that
    /// asked for it, as a node without a lowering does.
    ///
    /// Two calls with the same root, filter and resolution are two plans here
    /// and one plane after [`Self::finish`], which is also where a plan no live
    /// [`Op::Sample`] reads is dropped. So a lowering may ask for a buffer
    /// wherever it needs one without counting how many it has asked for.
    pub fn buffer(&mut self, root: ValueId, filter: Filter, resolution: Option<u32>) -> BufferId {
        self.plan(root, filter, resolution, None)
    }

    /// Plan a plane splatted from a strand layer rather than filtered from an
    /// expression, and answer the id that samples it.
    ///
    /// The root is still an expression, because everything that walks the plan
    /// list reads one; a strand plane's is a constant, and nothing rasterises
    /// it. What decides the texels is `strands` together with the filter's own
    /// output.
    pub(crate) fn strand_buffer(
        &mut self,
        root: ValueId,
        filter: Filter,
        strands: std::sync::Arc<StrandPlan>,
    ) -> BufferId {
        self.plan(root, filter, None, Some(strands))
    }

    fn plan(
        &mut self,
        root: ValueId,
        filter: Filter,
        resolution: Option<u32>,
        strands: Option<std::sync::Arc<StrandPlan>>,
    ) -> BufferId {
        let id = BufferId(index_of(self.buffers.len()));
        let value_type = filter.output_type(self.type_of(root));
        self.buffers.push(BufferPlan {
            id,
            path: self.path.clone(),
            root,
            filter,
            value_type,
            resolution,
            strands,
        });
        id
    }

    /// Read a planned buffer at a UV.
    pub fn sample(&mut self, buffer: BufferId, uv: ValueId) -> ValueId {
        let uv = self.convert(uv, IrType::Vec2);
        self.emit(Op::Sample(buffer), [uv])
    }

    /// The same expression, read at another coordinate.
    ///
    /// This is what a resampler is. The IR is one expression per texel, and
    /// [`Op::Uv`] is that texel's coordinate, so a
    /// [`Transform`](crate::nodes::Transform), a [`Warp`](crate::nodes::Warp),
    /// a [`Mirror`](crate::nodes::Mirror), a
    /// [`Kaleidoscope`](crate::nodes::Kaleidoscope) or a
    /// [`Tile`](crate::nodes::Tile) cannot move a value that has already been
    /// computed: it has to compute its source again somewhere else. So the
    /// sub-expression that reaches `root` is emitted a second time with `uv`
    /// standing where `Op::Uv` stood.
    ///
    /// Only the instructions that actually depend on the coordinate are
    /// re-emitted; a constant, a parameter or anything folded out of them is
    /// the same value at every texel and is shared rather than copied. What is
    /// re-emitted goes through [`Self::emit`] like anything else, so identical
    /// sub-expressions still collapse — two transforms that read one noise
    /// through the same frame cost one noise.
    ///
    /// An [`Op::Sample`] inside the sub-expression is re-emitted as a sample
    /// of the *same* plane at the new coordinate, which is exactly right: the
    /// plane is one repeat of a field that tiles, and reading it elsewhere is
    /// what resampling it means. A resampler therefore never costs a second
    /// plane.
    ///
    /// The cost is the honest one: a transform of an expensive source is that
    /// source again. A graph that reads one source through four frames pays
    /// for four, which is what it asked for.
    pub fn substitute(&mut self, root: ValueId, uv: ValueId) -> ValueId {
        let len = root.index().saturating_add(1);
        if len > self.insts.len() {
            return root;
        }
        // Only visit the source's dependencies. Earlier instructions may belong
        // to unrelated graph outputs or previously inlined instances; copying
        // those on every warp makes successive instances grow exponentially.
        let mut reachable = vec![false; len];
        reachable[root.index()] = true;
        for index in (0..len).rev() {
            if reachable[index] {
                for operand in self.insts[index].operands.as_slice() {
                    reachable[operand.index()] = true;
                }
            }
        }
        // An operand is always earlier than the instruction that reads it, so
        // one forward sweep decides which instructions move with the
        // coordinate and which are the same value wherever they are read.
        let mut moves = vec![false; len];
        for (index, &is_reachable) in reachable.iter().enumerate() {
            if !is_reachable {
                continue;
            }
            let Some(inst) = self.insts.get(index) else {
                continue;
            };
            let carried = inst
                .operands
                .as_slice()
                .iter()
                .any(|operand| moves.get(operand.index()).copied() == Some(true));
            if let Some(slot) = moves.get_mut(index) {
                *slot = inst.op == Op::Uv || carried;
            }
        }
        if moves.get(root.index()).copied() != Some(true) {
            return root;
        }
        let mut mapping: Vec<ValueId> = (0..len).map(|index| ValueId(index_of(index))).collect();
        for index in 0..len {
            if moves.get(index).copied() != Some(true) {
                continue;
            }
            // Emitting appends, so every index below `len` still names the
            // instruction it named when the sweep above read it.
            let Some(inst) = self.insts.get(index).copied() else {
                continue;
            };
            let replaced = if inst.op == Op::Uv {
                uv
            } else {
                let operands: Vec<ValueId> = inst
                    .operands
                    .as_slice()
                    .iter()
                    .map(|operand| mapping.get(operand.index()).copied().unwrap_or(*operand))
                    .collect();
                self.emit(inst.op, Operands::from_ids(&operands))
            };
            if let Some(slot) = mapping.get_mut(index) {
                *slot = replaced;
            }
        }
        mapping.get(root.index()).copied().unwrap_or(root)
    }

    /// Swap the parameter environment, answering the one that was in place.
    ///
    /// A [`Subgraph`](crate::nodes::Subgraph) is inlined into the same arena as
    /// the graph that instanced it, and its `Param` inputs name *its* own
    /// parameters rather than the outer graph's. So the driver pushes the
    /// instanced graph's bindings before it lowers the inner nodes and puts the
    /// outer ones back afterwards.
    fn swap_params(&mut self, params: Vec<ParamSlot>) -> Vec<ParamSlot> {
        std::mem::replace(&mut self.params, params)
    }

    /// Swap the bound inputs, answering the ones that were in place.
    ///
    /// The other half of what inlining a [`Subgraph`](crate::nodes::Subgraph)
    /// pushes. A parameter crosses the boundary as a number to fold; an input
    /// crosses as a value already in this arena, because the field wired into
    /// it was lowered by the graph that instanced it, on that node's own ports,
    /// before the inner graph was reached. So a
    /// [`GraphInput`](crate::nodes::GraphInput) inside costs nothing beyond the
    /// conversion to its declared type, and two inputs wired to one field share
    /// that field's instructions the way any two readers do.
    fn swap_bound(&mut self, bound: BTreeMap<String, ValueId>) -> BTreeMap<String, ValueId> {
        std::mem::replace(&mut self.bound, bound)
    }

    /// A value at another width: a scalar broadcast, or a colour through
    /// Rec. 709 luminance. These are the two conversions the graph allows, and
    /// a conversion it does not allow is a rejection rather than a guess.
    ///
    /// A colour reaching a [`IrType::Vec2`] is the two in a row rather than a
    /// third conversion: an [`Accepts::Displacement`] port takes a `Vec2` or a
    /// float, a colour reaches a float port as its luminance, and a float
    /// displaces along both axes. Nothing converts *out* of a `Vec2`, which is
    /// the graph's rule and is why a UV never silently becomes a colour.
    pub fn convert(&mut self, value: ValueId, to: IrType) -> ValueId {
        let from = self.type_of(value);
        match (from, to) {
            _ if from == to => value,
            (IrType::Float, IrType::Vec2) => self.vector(&[value, value]),
            (IrType::Float, IrType::Vec3) => self.vector(&[value, value, value]),
            (IrType::Vec3, IrType::Vec2) => {
                let luminance = self.convert(value, IrType::Float);
                self.convert(luminance, IrType::Vec2)
            }
            (IrType::Vec3, IrType::Float) => {
                let weights: Vec<ValueId> = LUMINANCE
                    .iter()
                    .map(|weight| self.constant(*weight))
                    .collect();
                let weights = self.vector(&weights);
                self.emit(Op::Dot, [value, weights])
            }
            _ => self.reject(format!("no conversion from {from} to {to}")),
        }
    }

    /// Emit one operation, folding it where its operands are constant and
    /// reusing an identical instruction where one was already emitted.
    pub fn emit(&mut self, op: Op, operands: impl Into<Operands>) -> ValueId {
        let operands = self.align(op, operands.into());
        if let Some(rejected) = self.check(op, operands) {
            return rejected;
        }
        if let Some(folded) = self.fold(op, operands) {
            return folded;
        }
        let value_type = self.result_type(op, operands);
        self.push(op, operands, value_type)
    }

    /// Refuse an operand shape no backend could honour, at the path the driver
    /// is at.
    ///
    /// Only [`Op::Compose`] has a contract the types do not already enforce: it
    /// reads lane zero of each operand, so composing a vector would drop every
    /// lane but its first. That is checked here rather than only in
    /// [`Self::vector`], because [`Self::emit`] is public and a node lowering
    /// may reach for the op directly.
    fn check(&mut self, op: Op, operands: Operands) -> Option<ValueId> {
        if op != Op::Compose {
            return None;
        }
        if !(2..=3).contains(&operands.len()) {
            return Some(self.reject("a vector is two or three components"));
        }
        let wide = operands
            .as_slice()
            .iter()
            .enumerate()
            .map(|(index, id)| (index, self.type_of(*id)))
            .find(|(_, value_type)| *value_type != IrType::Float)?;
        let (index, value_type) = wide;
        Some(self.reject(format!(
            "component {index} of a vector is a {value_type}; a vector is \
             composed of scalars, so extract its lanes first"
        )))
    }

    /// Broadcast the scalar operands of a component-wise op to the width the
    /// op works at, so every lane of every operand means the same thing.
    fn align(&mut self, op: Op, operands: Operands) -> Operands {
        let Some((from, imposed)) = op.alignment() else {
            return operands;
        };
        let mut ids = [ValueId::default(); MAX_OPERANDS];
        let len = operands.len();
        ids[..len].copy_from_slice(operands.as_slice());
        let width = imposed.unwrap_or_else(|| {
            ids[from.min(len)..len]
                .iter()
                .map(|id| self.type_of(*id))
                .max()
                .unwrap_or(IrType::Float)
        });
        for index in from..len {
            let Some(id) = ids.get(index).copied() else {
                continue;
            };
            let arrived = self.type_of(id);
            if arrived == width {
                continue;
            }
            if arrived == IrType::Float {
                let broadcast = self.convert(id, width);
                if let Some(slot) = ids.get_mut(index) {
                    *slot = broadcast;
                }
                continue;
            }
            // Two vector widths in one operation. Neither converts to the
            // other, and taking the wider would read a lane nobody wrote, so
            // this is the same class of mistake `check` refuses for a compose.
            self.reject(format!(
                "operand {index} of {op:?} is a {arrived} where the operation \
                 works at {width}; there is no conversion between them"
            ));
        }
        Operands::from_ids(&ids[..len])
    }

    /// The type an op defines, given the operands it was emitted with.
    fn result_type(&self, op: Op, operands: Operands) -> IrType {
        let widest = |operands: Operands| {
            operands
                .as_slice()
                .iter()
                .map(|id| self.type_of(*id))
                .max()
                .unwrap_or(IrType::Float)
        };
        match op {
            Op::Uv => IrType::Vec2,
            Op::WorldPos | Op::WorldNormal => IrType::Vec3,
            Op::Const(_)
            | Op::Time
            | Op::CutFlag
            | Op::Hash2(_)
            | Op::Hash3(_)
            | Op::Length
            | Op::Dot
            | Op::Extract(_) => IrType::Float,
            Op::Param(index) => self
                .bindings
                .get(index as usize)
                .map_or(IrType::Float, |binding| binding.value_type),
            Op::Sample(buffer) => self
                .buffers
                .get(buffer.index())
                .map_or(IrType::Float, |plan| plan.value_type),
            Op::Compose => {
                if operands.len() >= 3 {
                    IrType::Vec3
                } else {
                    IrType::Vec2
                }
            }
            // The condition of a select is a scalar; the branches carry the type.
            Op::Select => operands
                .as_slice()
                .get(1)
                .map_or(IrType::Float, |id| self.type_of(*id)),
            _ => widest(operands),
        }
    }

    /// Compute an op whose operands are all constant, and emit the answer.
    fn fold(&mut self, op: Op, operands: Operands) -> Option<ValueId> {
        if !op.foldable() {
            return None;
        }
        let mut args = [[0.0_f32; 3]; MAX_OPERANDS];
        for (slot, id) in args.iter_mut().zip(operands.as_slice()) {
            *slot = self.constant_value(*id)?;
        }
        let value_type = self.result_type(op, operands);
        // The same cut the interpreter makes when it stores a register, so a
        // constant that folded and one that was evaluated cannot differ in the
        // lanes their type does not own.
        let result = narrow(apply(op, &args[..operands.len()])?, value_type);
        Some(match value_type {
            IrType::Float => self.constant(result[0]),
            IrType::Vec2 | IrType::Vec3 => {
                let components: Vec<ValueId> = result
                    .iter()
                    .take(value_type.components())
                    .map(|component| self.constant(*component))
                    .collect();
                self.vector(&components)
            }
        })
    }

    /// The constant a value holds, seeing through a vector built of constants.
    fn constant_value(&self, value: ValueId) -> Option<[f32; 3]> {
        let inst = self.insts.get(value.index())?;
        match inst.op {
            Op::Const(constant) => Some([constant, 0.0, 0.0]),
            Op::Compose => {
                let mut components = [0.0; 3];
                for (slot, id) in components.iter_mut().zip(inst.operands.as_slice()) {
                    *slot = self.constant_value(*id)?[0];
                }
                Some(components)
            }
            _ => None,
        }
    }

    /// Append an instruction, or answer the identical one already emitted.
    fn push(&mut self, op: Op, operands: Operands, value_type: IrType) -> ValueId {
        if let Some(existing) = self.unique.get(&(op, operands)) {
            return *existing;
        }
        let id = ValueId(self.next);
        self.next = self.next.saturating_add(1);
        self.insts.push(Inst {
            op,
            operands,
            value_type,
        });
        self.unique.insert((op, operands), id);
        id
    }

    /// Finish: drop what no root reaches, and answer the arena or the first
    /// thing that went wrong.
    ///
    /// Dropping the dead instructions renumbers the live ones, so the ids this
    /// context handed out mean nothing afterwards. What survives is named:
    /// [`Ir::root`] answers for an output port and [`BufferPlan::root`] for a
    /// plane, and those are what a backend reads.
    ///
    /// Buffer plans are dropped and renumbered the same way, and for a sharper
    /// reason: a plane costs a rasterisation of its whole sub-expression and a
    /// pass of its filter, so one no live [`Op::Sample`] reads is a bake that
    /// does work nothing looks at, and two plans that agree on root, filter and
    /// resolution are one plane. The [`Op::Sample`]s are rewritten to the ids
    /// that survive.
    pub fn finish(
        self,
        roots: impl IntoIterator<Item = (String, ValueId)>,
    ) -> Result<Ir, GraphError> {
        if let Some(error) = self.error {
            return Err(error);
        }
        let roots: BTreeMap<String, ValueId> = roots.into_iter().collect();
        let mut live = vec![false; self.insts.len()];
        let mut wanted = vec![false; self.buffers.len()];
        for value in roots.values() {
            mark(*value, &mut live);
        }
        // Operands are always earlier than the instruction that reads them, and
        // so is the root of a plane a sample names — a plan can only be made of
        // instructions that already exist — so one backwards sweep marks
        // everything the roots reach through either.
        for index in (0..self.insts.len()).rev() {
            let (reached, inst) = match (live.get(index), self.insts.get(index)) {
                (Some(true), Some(inst)) => (true, *inst),
                _ => continue,
            };
            if !reached {
                continue;
            }
            for operand in inst.operands.as_slice() {
                mark(*operand, &mut live);
            }
            if let Op::Sample(buffer) = inst.op {
                // And the plane its guide names, and that one's guide: a
                // slope blur reads a second plane that no instruction samples,
                // so a sweep that only followed the samples would drop it and
                // renumber the id the filter holds. A guide is always an
                // earlier plan than the one that names it, and its root an
                // earlier instruction than either, so following the chain here
                // marks everything before this sweep reaches it.
                let mut next = Some(buffer);
                while let Some(buffer) = next.take() {
                    if wanted.get(buffer.index()).copied() == Some(true) {
                        break;
                    }
                    if let Some(flag) = wanted.get_mut(buffer.index()) {
                        *flag = true;
                    }
                    if let Some(plan) = self.buffers.get(buffer.index()) {
                        mark(plan.root, &mut live);
                        next = plan.filter.guide();
                    }
                }
            }
        }
        let mut mapping = vec![ValueId::default(); self.insts.len()];
        let mut insts = Vec::with_capacity(self.insts.len());
        let mut next = 0;
        for (index, inst) in self.insts.iter().enumerate() {
            if live.get(index).copied() != Some(true) {
                continue;
            }
            let operands = Operands::from_ids(
                &inst
                    .operands
                    .as_slice()
                    .iter()
                    .map(|operand| remap(&mapping, *operand))
                    .collect::<Vec<_>>(),
            );
            if let Some(slot) = mapping.get_mut(index) {
                *slot = ValueId(next);
            }
            next = next.saturating_add(1);
            insts.push(Inst { operands, ..*inst });
        }
        let (buffers, buffer_mapping) = plan_buffers(&self.buffers, &wanted, &mapping);
        for inst in &mut insts {
            if let Op::Sample(buffer) = inst.op {
                inst.op = Op::Sample(
                    buffer_mapping
                        .get(buffer.index())
                        .copied()
                        .unwrap_or_default(),
                );
            }
        }
        Ok(Ir {
            insts,
            roots: roots
                .into_iter()
                .map(|(port, value)| (port, remap(&mapping, value)))
                .collect(),
            params: self.bindings,
            buffers,
            target: self.target,
        })
    }
}

/// What makes two plans one plane: the sub-expression, the filter, the size,
/// the guide, and — for a strand plane, whose root is a constant and whose
/// filter carries only which output it holds — the scatter it is splatted from.
/// Without that last one two layers' coverage planes would agree on everything
/// above and collapse into one.
type Shared = (
    ValueId,
    (u16, [u32; 2]),
    Option<u32>,
    BufferId,
    Option<std::sync::Arc<[u8]>>,
);

/// The plans that survive, renumbered, and where each old id went.
///
/// Identical plans collapse into one: the plane a bake computes is decided by
/// what is rasterised, what is run over it and how many texels it has, so two
/// nodes that agree on all three are asking for one plane. The first plan's
/// path is the one kept, which is the node an author would look at first.
fn plan_buffers(
    plans: &[BufferPlan],
    wanted: &[bool],
    mapping: &[ValueId],
) -> (Vec<BufferPlan>, Vec<BufferId>) {
    let mut buffers: Vec<BufferPlan> = Vec::new();
    let mut renumbered = vec![BufferId::default(); plans.len()];
    let mut shared: HashMap<Shared, BufferId> = HashMap::new();
    for (index, plan) in plans.iter().enumerate() {
        if wanted.get(index).copied() != Some(true) {
            continue;
        }
        let root = remap(mapping, plan.root);
        // A guide is an earlier plan, so it already has its new id; two slope
        // blurs that agree on everything but the plane they walk are two
        // planes, which is what putting the guide in the key says.
        let filter = match plan.filter.guide() {
            Some(guide) => plan
                .filter
                .with_guide(renumbered.get(guide.index()).copied().unwrap_or_default()),
            None => plan.filter,
        };
        let guide = filter.guide().unwrap_or_default();
        let scatter = plan
            .strands
            .as_ref()
            .map(|strands| std::sync::Arc::clone(&strands.key));
        let id = *shared
            .entry((root, filter.code(), plan.resolution, guide, scatter))
            .or_insert_with(|| {
                let id = BufferId(index_of(buffers.len()));
                buffers.push(BufferPlan {
                    id,
                    root,
                    filter,
                    ..plan.clone()
                });
                id
            });
        if let Some(slot) = renumbered.get_mut(index) {
            *slot = id;
        }
    }
    (buffers, renumbered)
}

/// Note that a value the roots reach is kept, and nothing else is.
fn mark(value: ValueId, live: &mut [bool]) {
    if let Some(flag) = live.get_mut(value.index()) {
        *flag = true;
    }
}

/// A count as an index.
///
/// Saturating rather than fallible: a graph with four billion parameters or
/// buffers is not a graph, and neither the parameter list nor the buffer plan
/// is worth a `Result` for it.
fn index_of(count: usize) -> u32 {
    u32::try_from(count).unwrap_or(u32::MAX)
}

/// An id after the dead instructions were dropped.
fn remap(mapping: &[ValueId], value: ValueId) -> ValueId {
    mapping.get(value.index()).copied().unwrap_or_default()
}

/// The IR width a graph port type is carried in.
pub(crate) fn ir_type(value_type: crate::ValueType) -> IrType {
    match value_type {
        crate::ValueType::Float => IrType::Float,
        crate::ValueType::Color => IrType::Vec3,
        crate::ValueType::Vec2 => IrType::Vec2,
    }
}

/// The width one port reads its input at, given what reached it.
///
/// A port that names a type converts to it; a port that takes whatever came
/// keeps the width it was handed. This is where the graph's two implicit
/// conversions turn into instructions, once, for every node.
fn port_type(accepts: Accepts, arrived: IrType) -> IrType {
    match accepts {
        Accepts::Float => IrType::Float,
        Accepts::Color => IrType::Vec3,
        // A displacement is two axes; one float displaces along both.
        Accepts::Vec2 | Accepts::Displacement => IrType::Vec2,
        Accepts::Field | Accepts::Any => arrived,
    }
}

/// The lowered inputs of one node, by port name.
pub struct NodeInputs {
    ports: Vec<(Cow<'static, str>, ValueId)>,
    zero: ValueId,
}

impl NodeInputs {
    /// One input by port name, already converted to what the port accepts.
    ///
    /// A name no port carries answers a constant zero. The port list and the
    /// lowering are written side by side in the same file, so that is a
    /// programming error rather than anything an author can cause.
    pub fn value(&self, name: &str) -> ValueId {
        self.ports
            .iter()
            .find(|(port, _)| port == name)
            .map_or(self.zero, |(_, value)| *value)
    }

    /// Every input, in the order the node declares its ports.
    pub fn all(&self) -> &[(Cow<'static, str>, ValueId)] {
        &self.ports
    }
}

/// What one node kind lowers to.
///
/// Implemented once per node struct, beside the node itself. The default
/// rejects by path, which is how a backend refuses a node it does not
/// implement: `#[non_exhaustive]` keeps the vocabulary open, and this keeps a
/// missing lowering a diagnostic rather than a wrong picture. Every node in the
/// vocabulary lowers today, so nothing takes the default; a node added
/// tomorrow does, until it is written.
///
/// A node whose kind lowers but whose *settings* do not — a simplex noise,
/// whose lattice has no integer period at all, or a brick corner this backend
/// has no expression for — refuses in the same words at the same path, so what
/// a caller recognises is the shape of the answer rather than a list of kinds.
pub(crate) trait Lower {
    fn lower(&self, cx: &mut Lowering, _inputs: &NodeInputs) -> ValueId {
        cx.unsupported()
    }
}

/// Lower a validated material into the expression a backend runs.
///
/// Nodes are lowered in [`Material::order`], so every input is an id by the
/// time the node that reads it is reached, and the roots are the bound
/// [`PbrOutput`](crate::PbrOutput) ports by name. A node whose lowering has not
/// landed yet is reported at `nodes[id]`, the same path validation uses.
pub fn lower(material: &Material, target: Target) -> Result<Ir, GraphError> {
    let mut cx = Lowering::for_material(material, target);
    let roots = lower_graph(&mut cx, material, "");
    cx.finish(roots)
}

/// Lower one strand layer's fields, with its ports as the roots.
///
/// The same nodes, the same folding and the same sharing as [`lower`]; what
/// changes is which values are named at the end. A strand field is read once
/// per root rather than once per texel, so what a scatter wants is an
/// expression rooted at the ten fields and *not* at the PBR channels — rooting
/// those too would keep every instruction the surface needs alive in a walk
/// that evaluates it sixty-five thousand times for nothing.
///
/// A name the graph does not declare is an error at `strands[name]`, in the
/// shape a caller that mistyped a layer needs.
pub fn lower_strands(material: &Material, layer: &str, target: Target) -> Result<Ir, GraphError> {
    lower_strands_within(material, layer, target, Vec::new())
}

/// The target a strand layer is lowered for, wherever it is lowered from.
///
/// A shader target with nothing live, which is a deliberate pair of choices
/// rather than a spelling of [`Target::Bake`]. Nothing live is what folds a
/// [`Live`](crate::Exposure::Live) parameter at the value the material carries,
/// which is decision 6 of the strand plan: a strand set is geometry, and
/// geometry is rebuilt rather than driven by a uniform. And a *shader* target
/// is what keeps this from refusing a world-space input in the graph's PBR
/// half, which a graph delivered as a `Shader` surface is allowed to carry;
/// only the nodes a strand field reaches are a scatter's business, and
/// [`lower_strands`] refuses those by path.
pub fn strand_target() -> Target {
    Target::Shader { live: Vec::new() }
}

/// [`lower_strands`], with the layers already being lowered above it.
///
/// The stack is what a [`StrandRelief`](crate::nodes::StrandRelief) inside the
/// graph is guarded by: see [`Lowering::strands`].
fn lower_strands_within(
    material: &Material,
    layer: &str,
    target: Target,
    outer: Vec<String>,
) -> Result<Ir, GraphError> {
    let Some(strands) = material.graph().strands.get(layer) else {
        return Err(GraphError::new(
            format!("strands[{layer}]"),
            format!(
                "graph {:?} declares no strand layer of that name",
                material.graph().id
            ),
        ));
    };
    // Before the lowering rather than after it, so that a field reaching the
    // frame — or reaching a relief of the very strands it is a field of — is
    // answered by the node that asked rather than by whatever the expression
    // under it then did.
    crate::strands::refuse_dynamic(material, strands)?;
    let mut cx = Lowering::for_material(material, target);
    cx.strands = outer;
    cx.strands.push(layer.to_owned());
    let values = lower_nodes(&mut cx, material, "");
    let mut roots = Vec::new();
    for (port, accepts, input) in strands.ports() {
        cx.at(format!("strands[{layer}].inputs[{port}]"), "a strand field");
        roots.push((port.to_owned(), resolve(&mut cx, input, accepts, &values)));
    }
    cx.finish(roots)
}

/// Splat one of the graph's own strand layers into a plane, and answer the
/// sample that reads it back.
///
/// The second node the driver lowers rather than the node itself, and for the
/// reason [`inline`] is the first: what a relief names — a strand layer, its
/// ten fields and its two dozen constants — the node holds only by name, and
/// the material is where the name is resolved.
///
/// The plan's own root is a constant, because a strand plane is not filtered
/// from anything: what decides its texels is the [`StrandPlan`] beside it.
/// Everything that walks the plan list therefore sees an ordinary plane over a
/// trivial expression, which is exactly what the least invasive answer looks
/// like.
fn splat(
    cx: &mut Lowering,
    node: &crate::nodes::StrandRelief,
    material: &Material,
    path: &str,
) -> ValueId {
    if cx.inside_strands() {
        // A relief the fields actually read was refused by path before this
        // context existed; one nothing reads is dead and `finish` drops it, so
        // the honest answer here is a value rather than an error.
        return cx.constant(0.0);
    }
    let Some(layer) = material.graph().strands.get(&node.layer) else {
        return cx.reject(format!(
            "graph {:?} declares no strand layer {:?}",
            material.graph().id,
            node.layer
        ));
    };
    let fields =
        match lower_strands_within(material, &node.layer, strand_target(), cx.strands.clone()) {
            Ok(fields) => fields,
            Err(error) => return cx.fail(error),
        };
    let plan = std::sync::Arc::new(StrandPlan::new(
        &node.layer,
        node.repeat_metres,
        layer.clone(),
        fields,
    ));
    cx.at(path.to_owned(), "StrandRelief");
    let zero = cx.constant(0.0);
    let buffer = cx.strand_buffer(
        zero,
        Filter::Strands {
            output: node.output,
        },
        plan,
    );
    let uv = cx.uv();
    cx.sample(buffer, uv)
}

/// One material's nodes into one arena, answering its bound output ports.
///
/// Written apart from [`lower`] because a [`Subgraph`] runs it again: an
/// instanced graph is inlined into the arena that instanced it, so it costs
/// nothing at runtime and everything downstream of it folds and shares as if
/// the author had written it out. `prefix` is what an error inside an inlined
/// graph is reported under, and is empty for the material a bake asked for.
fn lower_graph(cx: &mut Lowering, material: &Material, prefix: &str) -> Vec<(String, ValueId)> {
    let values = lower_nodes(cx, material, prefix);
    let mut roots = Vec::new();
    for (port, accepts, input) in material.graph().output.ports() {
        // An extra is a mask an instance reads, not a channel a renderer binds,
        // so it is a root only where something can read it. At the top there is
        // nothing above to read one, and rooting it there would make a bake
        // rasterise a map nobody asked for and a partition cut for it.
        if prefix.is_empty() && material.graph().output.extra.contains_key(port.as_ref()) {
            continue;
        }
        cx.at(format!("{prefix}output.{port}"), "an output");
        roots.push((port.to_string(), resolve(cx, input, accepts, &values)));
    }
    roots
}

/// Every node of one material into one arena, by id.
///
/// The half of [`lower_graph`] that is about the graph rather than about what
/// reads it, written apart because a strand layer reads the same nodes under a
/// different set of roots. Every node is lowered, including one nothing reads:
/// that is what makes a world-space input in an unread corner refuse a bake, and
/// `finish` drops whatever no root reaches afterwards.
fn lower_nodes<'a>(
    cx: &mut Lowering,
    material: &'a Material,
    prefix: &str,
) -> BTreeMap<&'a str, ValueId> {
    let mut values: BTreeMap<&str, ValueId> = BTreeMap::new();
    for id in material.order() {
        let Some(node) = material.graph().nodes.get(id) else {
            continue;
        };
        // A comment carries no signal, so there is nothing to lower.
        if node.output() == Output::None {
            continue;
        }
        let path = format!("{prefix}nodes[{id}]");
        cx.at(path.clone(), node.kind());
        let zero = cx.constant(0.0);
        let mut ports = Vec::new();
        for port in node.inputs() {
            // A conversion refused while an input is resolved belongs to the
            // port, and validation names a port this way too.
            cx.at(format!("{path}.inputs[{}]", port.name), node.kind());
            ports.push((port.name, resolve(cx, port.input, port.accepts, &values)));
        }
        // What the node itself cannot do is the node's.
        cx.at(path.clone(), node.kind());
        let inputs = NodeInputs { ports, zero };
        // The two nodes the driver lowers rather than the node itself: each
        // names something the material holds and the node does not — a graph
        // by key, a strand layer by name.
        let value = match node {
            Node::Subgraph(subgraph) => inline(cx, subgraph, &inputs, material, &path),
            Node::StrandRelief(relief) => splat(cx, relief, material, &path),
            _ => node.lower(cx, &inputs),
        };
        values.insert(id.as_str(), value);
    }
    values
}

/// Inline one instanced graph and answer the output the node reads.
///
/// The instanced material was validated when this one was — which is where
/// recursion was refused, by key, through the library — so what is left here is
/// arithmetic. What the node bound crosses the boundary in two ways, and the
/// difference is what each of them costs. Its parameters are bound at the
/// instance and folded: a subgraph takes the values the node gave it and its
/// own defaults for the rest, and none of them is live, because a live
/// parameter of an instanced graph would need a uniform per instance and the
/// graph that instanced it never declared one. Its *inputs* are whole fields,
/// which `lower_graph` has already lowered on this node's own ports — they are
/// ordinary ports, so nothing about them is special until here — and what is
/// pushed is the value each of them came to, in this same arena.
fn inline(
    cx: &mut Lowering,
    node: &Subgraph,
    inputs: &NodeInputs,
    outer: &Material,
    path: &str,
) -> ValueId {
    // Which instance of that graph: an instance is specialised per binding
    // signature, so the key is the node together with the fields that reached
    // its ports, derived by the same function the build cached it under.
    let resolved = match outer.resolved_inputs(node, path) {
        Ok(resolved) => resolved,
        // The build refused a node whose ports do not resolve, so this is a
        // belt on an unreachable arm; it carries the inner path in the message,
        // because the path this rejection is filed under is this node's and the
        // port that failed is the thing to go and look at.
        Err(error) => return cx.reject(error.to_string()),
    };
    let key = crate::library::instance_key(node, &resolved);
    let Some(inner) = outer.instance(&key) else {
        return cx.reject(format!(
            "graph {key} was not resolved when this material was built; build it through the \
             library that holds it"
        ));
    };
    let params = inner
        .graph()
        .params
        .iter()
        .map(|param| ParamSlot {
            name: param.name.clone(),
            value: node.params.get(&param.name).unwrap_or(&param.value).value(),
            binding: None,
        })
        .collect();
    // The fields this node wired in, by the name the inner graph declares. A
    // port that resolved to nothing is not reachable: the build refused the
    // node before it had a key at all.
    let bound = node
        .inputs
        .keys()
        .map(|name| (name.clone(), inputs.value(name)))
        .collect();
    let outer_params = cx.swap_params(params);
    let outer_bound = cx.swap_bound(bound);
    let roots = lower_graph(cx, inner, &format!("{path}.graphs[{key}]."));
    cx.swap_bound(outer_bound);
    cx.swap_params(outer_params);
    cx.at(path.to_owned(), "Subgraph");
    let port = node.output.port();
    match roots.iter().find(|(name, _)| name == port) {
        Some((_, value)) => *value,
        // Validation refused a node that reads an output its graph does not
        // bind, so this is a belt on an unreachable arm.
        None => cx.reject(format!("graph {:?} binds no {port}", node.graph)),
    }
}

/// One input as a value of the width the port reading it needs.
fn resolve(
    cx: &mut Lowering,
    input: &Input,
    accepts: Accepts,
    values: &BTreeMap<&str, ValueId>,
) -> ValueId {
    let value = match input {
        Input::Const(literal) => cx.value(*literal),
        Input::Param(name) => cx.param(name),
        Input::Node(id) => match values.get(id.as_str()).copied() {
            Some(value) => value,
            None => cx.reject(format!("node {id:?} has no lowered value")),
        },
        // `build` hoists every inline node into the map, so a validated
        // material has none left.
        Input::Inline(_) => cx.reject("an inline input did not survive the build"),
    };
    let wanted = port_type(accepts, cx.type_of(value));
    cx.convert(value, wanted)
}
