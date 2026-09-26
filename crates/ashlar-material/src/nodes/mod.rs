//! The node vocabulary: one module per family, one struct per node.
//!
//! Names follow [Material Maker](https://www.materialmaker.org/) where a
//! Material Maker node exists, so a graph can be ported by reading it. Every
//! node struct is its own builder: a constructor takes what the node cannot do
//! without, and the setters take the rest. Anything that accepts an
//! [`Input`] also accepts a node id, a literal, or another node
//! written inline.
//!
//! [`Node`] is `#[non_exhaustive]`, so a backend rejects what it does not
//! implement by path rather than the vocabulary freezing.
//!
//! # Adding a node
//!
//! A node is done when all of these hold, and a review checks the same list:
//! a struct with its own builder, `Default`, `#[serde(deny_unknown_fields)]`
//! and `#[must_use]`; `ports!` (or a hand `Ports` impl where the output type
//! depends on a field), a `Check` for every bound its doc comment states, and a
//! `Lower` in the existing ops; a variant in `vocabulary!`; an arm in
//! `period::node_period`, and in `period::node_lattice` where it is not the
//! default carry, each with its reason; in the crate's `tests/`, a behaviour
//! test that fails if the arithmetic is wrong, membership of the tiling
//! property set in `nodes.rs`, the rule in `periods.rs`, a strategy arm in
//! `properties.rs` and a `node_graphs` entry in `wgsl.rs`; a case in
//! `ashlar-bevy`'s `tests/conformance.rs` if it adds an IR op, which CI fails
//! without; and a row in the `ashlar-materials` skill's `references/nodes.md`.

use std::borrow::Cow;

use serde::{Deserialize, Serialize};

use crate::{
    GraphError, Input, Period, ValueType,
    ir::{Lower, Lowering, NodeInputs, ValueId},
};

mod buffered;
mod filter;
pub(crate) mod frame;
mod generator;
pub(crate) mod runtime;
mod structure;
mod transform;

pub use buffered::{
    Blur, BlurKind, Buffer, Curvature, CurvatureKind, CurvatureOutput, Dilate, Distance,
    EdgeDetect, Erode, MAX_SLOPE_STEPS, OcclusionFromHeight, SlopeMode, StrandRelief,
    StrandReliefOutput,
};
pub use filter::{
    Adjust, Blend, BlendMode, Channel, Clamp, Colorize, Combine, Combine2, Curve, Decompose,
    Direction, DirectionSource, GradientStop, HeightToMask, Invert, Levels, LevelsChannel, Math,
    MathOp, Mix, NormalFromHeight, SdfCombine, SdfMask, SdfOp, Switch,
};
/// The bond arithmetic [`Bricks`] is checked by and inferred from.
pub(crate) use generator::bond_period;
pub use generator::{
    BrickOutput, Bricks, Noise, NoiseKind, Pattern, PatternKind, PatternMix, Scratches, Shape,
    ShapeKind, ShapeOutput, TilePattern, Tiles, Uv, Voronoi, VoronoiMetric, VoronoiOutput, Weave,
    WeaveOutput, WeavePattern,
};
pub use runtime::{
    CutFlag, TRIPLANAR_SHARPNESS, Time, Triplanar, WorldAxis, WorldField, WorldMask, WorldNormal,
    WorldPos,
};
/// The period an unbound [`GraphInput`] carries, and why it is not free.
pub(crate) use structure::UNBOUND_PERIOD;
pub use structure::{Comment, GraphInput, Subgraph, SurfaceOutput};
pub use transform::{
    CircleMap, CircleSplatter, DirectionalWarp, GradientWarp, IntensityWarp, Kaleidoscope, Mirror,
    MirrorAxis, Tile, Transform, Warp,
};

/// What one input port accepts, after the two implicit conversions.
///
/// A `Float` reaching a `Color` port broadcasts; a `Color` reaching a `Float`
/// port becomes its Rec. 709 luminance. A `Vec2` converts to nothing, so it
/// only ever reaches a port that asked for one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Accepts {
    /// One channel. A colour arrives as its luminance.
    Float,
    /// Linear RGB. A float arrives broadcast across the channels.
    Color,
    /// A UV or an offset, and nothing else.
    Vec2,
    /// Either a float or a colour, whichever came: the port passes it through.
    Field,
    /// A displacement: a `Vec2` as it stands, or a float along both axes. A
    /// colour is its luminance along both, which is the two conversions above
    /// in a row rather than a third one.
    Displacement,
    /// Anything at all. Resamplers do not care what they carry.
    Any,
}

impl Accepts {
    /// Whether a value of this type reaches this port, converting if it must.
    pub fn admits(self, value_type: ValueType) -> bool {
        match self {
            Self::Float => value_type.converts_to(ValueType::Float),
            Self::Color => value_type.converts_to(ValueType::Color),
            Self::Vec2 => value_type == ValueType::Vec2,
            Self::Field => matches!(value_type, ValueType::Float | ValueType::Color),
            Self::Displacement => {
                value_type == ValueType::Vec2 || value_type.converts_to(ValueType::Float)
            }
            Self::Any => true,
        }
    }

    /// What to say in an error when something else arrived.
    pub fn describe(self) -> &'static str {
        match self {
            Self::Float => "a Float",
            Self::Color => "a Color",
            Self::Vec2 => "a Vec2",
            Self::Field => "a Float or a Color",
            Self::Displacement => "a Vec2 or a Float",
            Self::Any => "a value",
        }
    }
}

/// One wired input of a node.
///
/// The name is a [`Cow`] rather than a `&'static str` because a node may
/// declare its ports at runtime: a [`Subgraph`] takes one port per input the
/// graph it instances declares, and those names are the author's rather than
/// the vocabulary's. Everything written in Rust still names its ports with
/// literals, which borrow.
#[derive(Clone, Debug)]
pub struct InputPort<'a> {
    /// Port name, as it appears in an error path.
    pub name: Cow<'static, str>,
    /// What this port accepts.
    pub accepts: Accepts,
    /// What is wired to it.
    pub input: &'a Input,
}

/// What a node produces.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Output {
    /// Always this type, whatever came in.
    Fixed(ValueType),
    /// Whatever the named input port carries: a resampler or a pointwise filter.
    SameAs(&'static str),
    /// The two named ports combined: a colour if either is, a float otherwise.
    Join(&'static str, &'static str),
    /// Nothing. A comment is data an author reads, not a signal.
    None,
}

/// One resolved input: the type it carries, how it tiles, and how fine the
/// lattice reaching it is.
#[derive(Clone, Debug)]
pub(crate) struct Resolved {
    pub name: Cow<'static, str>,
    pub value_type: ValueType,
    pub period: Period,
    /// Cells across UV `[0, 1)`, per axis, which a resampler multiplies. A
    /// literal and a parameter lay none and answer one.
    pub lattice: [u32; 2],
}

/// The ports of one node kind. Implemented for every node struct.
pub(crate) trait Ports {
    fn inputs(&self) -> Vec<InputPort<'_>>;
    fn inputs_mut(&mut self) -> Vec<(Cow<'static, str>, &mut Input)>;
    fn output(&self) -> Output;
}

/// The checks one node kind makes on its own fields, and on the types that
/// reached it where a port alone cannot say.
pub(crate) trait Check {
    fn check(&self, _path: &str) -> Result<(), GraphError> {
        Ok(())
    }

    fn check_types(&self, _inputs: &[Resolved], _path: &str) -> Result<(), GraphError> {
        Ok(())
    }
}

/// Declare the ports of a node struct, in the order errors report them.
///
/// The output is usually one expression. A node whose output depends on its own
/// fields — a [`Math`], whose unary operators answer in their first operand
/// alone — writes it as `|node| ...` instead, and reads the node through that
/// name.
macro_rules! ports {
    ($ty:ty, |$node:ident| $output:expr $(, $name:literal : $accepts:ident => $field:ident)* $(,)?) => {
        impl $crate::nodes::Ports for $ty {
            fn inputs(&self) -> Vec<$crate::nodes::InputPort<'_>> {
                vec![$($crate::nodes::InputPort {
                    name: std::borrow::Cow::Borrowed($name),
                    accepts: $crate::nodes::Accepts::$accepts,
                    input: &self.$field,
                }),*]
            }

            fn inputs_mut(
                &mut self,
            ) -> Vec<(std::borrow::Cow<'static, str>, &mut $crate::Input)> {
                vec![$((std::borrow::Cow::Borrowed($name), &mut self.$field)),*]
            }

            fn output(&self) -> $crate::nodes::Output {
                let $node = self;
                $output
            }
        }
    };
    ($ty:ty, $output:expr $(, $name:literal : $accepts:ident => $field:ident)* $(,)?) => {
        $crate::nodes::ports!($ty, |_node| $output $(, $name : $accepts => $field)*);
    };
}
pub(crate) use ports;

/// Declare the vocabulary: the enum, its dispatch, and the conversions that
/// let a node value stand where a node or an input is expected.
macro_rules! vocabulary {
    ($( $(#[$doc:meta])* $variant:ident ),* $(,)?) => {
        /// Every node kind. One struct per kind, each its own builder.
        ///
        /// Non-exhaustive: a new node must not break a backend's match, and a
        /// backend that cannot lower one rejects it by path.
        #[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
        #[serde(deny_unknown_fields)]
        #[non_exhaustive]
        pub enum Node {
            $( $(#[$doc])* $variant($variant), )*
        }

        impl Node {
            /// The node kind's name, for diagnostics.
            pub fn kind(&self) -> &'static str {
                match self { $(Self::$variant(_) => stringify!($variant),)* }
            }

            /// Every wired input, in the order validation reports them.
            pub fn inputs(&self) -> Vec<InputPort<'_>> {
                match self { $(Self::$variant(node) => Ports::inputs(node),)* }
            }

            /// What this node produces.
            pub fn output(&self) -> Output {
                match self { $(Self::$variant(node) => Ports::output(node),)* }
            }

            pub(crate) fn inputs_mut(&mut self) -> Vec<(Cow<'static, str>, &mut Input)> {
                match self { $(Self::$variant(node) => Ports::inputs_mut(node),)* }
            }

            pub(crate) fn check(&self, path: &str) -> Result<(), GraphError> {
                match self { $(Self::$variant(node) => Check::check(node, path),)* }
            }

            pub(crate) fn check_types(
                &self,
                inputs: &[Resolved],
                path: &str,
            ) -> Result<(), GraphError> {
                match self { $(Self::$variant(node) => Check::check_types(node, inputs, path),)* }
            }

            pub(crate) fn lower(&self, cx: &mut Lowering, inputs: &NodeInputs) -> ValueId {
                match self { $(Self::$variant(node) => Lower::lower(node, cx, inputs),)* }
            }
        }

        $(
            impl From<$variant> for Node {
                fn from(node: $variant) -> Self {
                    Self::$variant(node)
                }
            }

            /// A node written where an input is expected is hoisted into the
            /// graph under the id `parent.port` when the graph is built.
            impl From<$variant> for Input {
                fn from(node: $variant) -> Self {
                    Self::Inline(Box::new(Node::$variant(node)))
                }
            }
        )*
    };
}

vocabulary! {
    /// The input coordinate.
    Uv,
    /// Seconds since the app started. A shader input; zero in a bake.
    Time,
    /// The fragment's world position. A shader input; zero in a bake.
    WorldPos,
    /// The fragment's world normal. A shader input; zero in a bake.
    WorldNormal,
    /// Whether the surface is a cut face. A shader input; zero in a bake.
    CutFlag,
    /// A tiled source projected on the three axis planes of the world and
    /// blended by the world normal. A shader input; a bake refuses it.
    Triplanar,
    /// One axis of the world normal or position as a mask: up-facing faces,
    /// low faces, north walls. A shader input; a bake refuses it.
    WorldMask,
    /// Value, Perlin or Simplex noise on a wrapping integer lattice, with FBM.
    Noise,
    /// Cellular noise: distance, cell id, edge or border.
    Voronoi,
    /// Running-bond brickwork, with a per-brick id for tint and wear.
    Bricks,
    /// Grid, hex or herringbone tiling, with the same outputs as bricks.
    Tiles,
    /// The cheap regular patterns: stripes, checker, sine, triangle, square.
    Pattern,
    /// One signed-distance shape centred in the repeat, as a mask or as the
    /// signed distance itself.
    Shape,
    /// Randomised line segments, tiled by construction.
    Scratches,
    /// Woven cloth: two sets of threads crossing over and under one another,
    /// plain, twill or satin, as a mask, a relief or a per-thread id.
    Weave,
    /// Scale, quarter-turn rotation and translation of the sampling frame.
    Transform,
    /// Scatters a source across the repeat, with per-instance variation.
    Tile,
    /// Domain distortion: the source read through an offset field.
    Warp,
    /// Domain distortion along an angle field, by a fixed distance.
    DirectionalWarp,
    /// Domain distortion driven by a scalar guide gradient.
    GradientWarp,
    /// Fixed direction, varying displacement intensity.
    IntensityWarp,
    /// Reflects alternate repeats into one another.
    Mirror,
    /// Rotational symmetry about the centre of the repeat.
    Kaleidoscope,
    /// A source read round a disc centred in the repeat: its u is the angle and
    /// its v the radius, with a fill outside the disc.
    CircleMap,
    /// Scatters a source round one or more rings inside the repeat, with
    /// per-instance variation.
    CircleSplatter,
    /// Two fields combined through a blend mode and an opacity.
    Blend,
    /// Input and output range, with a gamma between them.
    Levels,
    /// A monotone curve through control points.
    Curve,
    /// A float through a gradient of linear colour stops.
    Colorize,
    /// Brightness, contrast, hue and saturation.
    Adjust,
    /// One scalar operator over one or two operands.
    Math,
    /// One channel of a colour, or one axis of a UV.
    Decompose,
    /// Three floats into a colour.
    Combine,
    /// Two floats into a `Vec2`.
    Combine2,
    /// An orientation field: a unit `Vec2` from an angle or from a slope.
    Direction,
    /// One minus the input.
    Invert,
    /// The input held between two bounds.
    Clamp,
    /// A linear blend of two fields by a third.
    Mix,
    /// One of two fields, chosen by a condition.
    Switch,
    /// A tangent-space normal from a height field, by finite difference.
    NormalFromHeight,
    /// A band of a height field, as a mask, with soft bounds.
    HeightToMask,
    /// Two signed distance fields through a boolean, hard or filleted.
    SdfCombine,
    /// A signed distance read as a mask, by the ramp a shape uses on its own.
    SdfMask,
    /// Gaussian, directional or slope blur over a rasterised plane.
    Blur,
    /// Horizon-based ambient occlusion from a height field.
    OcclusionFromHeight,
    /// How far a height field stands above its own neighbourhood: the signed
    /// field, or the crests or hollows of it as a mask.
    Curvature,
    /// Where a field changes, as a mask.
    EdgeDetect,
    /// Wrapped distance from a mask, by jump flood.
    Distance,
    /// Shrinks the bright parts of a mask.
    Erode,
    /// Grows the bright parts of a mask.
    Dilate,
    /// An explicit cache point, for isolating cost or pinning a resolution.
    Buffer,
    /// One of this graph's own strand layers, splatted from above into a plane.
    StrandRelief,
    /// A typed signal this graph takes from whatever instances it.
    GraphInput,
    /// Another graph from the library, instanced and read by output.
    Subgraph,
    /// Ignored. Notes an author leaves in the graph.
    Comment,
}
