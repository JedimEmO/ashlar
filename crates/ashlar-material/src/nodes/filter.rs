//! Pointwise filters: one texel in, one texel out, no neighbourhood.
//!
//! These are the nodes that survive into a shader unchanged, and their period
//! is the least common multiple of their inputs'.

use serde::{Deserialize, Serialize};

use crate::ir::{Filter, IrType, Lower, Lowering, NodeInputs, Op, ValueId};
use crate::nodes::frame::{
    abs, add, affine, axes, cell_hash, clamp, complement, cos, div, dot, inward_ramp, k, k2, max,
    min, mix, mul, normalize, select, sin, smoothstep, step, sub, vec2,
};
use crate::{
    GraphError, Input, Value, ValueType, finite,
    nodes::{Check, Output, Resolved, generator::check_unit, ports},
    require,
};

/// How two fields are combined.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub enum BlendMode {
    /// `b` over `a`, which at full opacity is `b`.
    #[default]
    Normal,
    /// The product. Darkens.
    Multiply,
    /// The inverse product of the inverses. Lightens.
    Screen,
    /// Multiply where `a` is dark, screen where it is light.
    Overlay,
    /// The sum.
    Add,
    /// `a` less `b`.
    Subtract,
    /// The absolute difference.
    Difference,
    /// The larger.
    Lighten,
    /// The smaller.
    Darken,
    /// A gentler overlay: Pegtop's `(1 - 2b) a² + 2ab`, which is smooth where
    /// the two-branch form most editors use has a kink at a half.
    SoftLight,
    /// `b` where a per-texel hash falls under the opacity, `a` elsewhere.
    Dissolve,
}

/// Two fields combined through a blend mode and an opacity.
///
/// `a` is the backdrop and `b` is what goes over it, so an opacity of zero is
/// `a` untouched. Opacity is an input, not a constant, so a mask can drive it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
#[must_use]
pub struct Blend {
    /// How the two combine.
    pub mode: BlendMode,
    /// The backdrop.
    pub a: Input,
    /// What goes over it.
    pub b: Input,
    /// How much of the blend is taken, usually in `0..=1`.
    pub opacity: Input,
}

impl Default for Blend {
    fn default() -> Self {
        Self {
            mode: BlendMode::default(),
            a: Input::default(),
            b: Input::default(),
            opacity: Input::float(1.0),
        }
    }
}

impl Blend {
    /// Combine two fields at full opacity.
    pub fn new(mode: BlendMode, a: impl Into<Input>, b: impl Into<Input>) -> Self {
        Self {
            mode,
            a: a.into(),
            b: b.into(),
            ..Self::default()
        }
    }

    /// Set how much of the blend is taken.
    pub fn opacity(mut self, opacity: impl Into<Input>) -> Self {
        self.opacity = opacity.into();
        self
    }
}

ports!(
    Blend,
    Output::Join("a", "b"),
    "a": Field => a,
    "b": Field => b,
    "opacity": Float => opacity,
);
impl Check for Blend {}

/// Whether [`Levels`] works per channel or on luminance.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub enum LevelsChannel {
    /// Every channel through the same curve, independently.
    #[default]
    PerChannel,
    /// The curve applied to luminance, the colour scaled to match.
    Luminance,
}

/// Input range, gamma and output range: the contrast workhorse.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
#[must_use]
pub struct Levels {
    /// The field to remap.
    pub input: Input,
    /// Input value that becomes `out_low`.
    pub in_low: f32,
    /// Input value that becomes `out_high`.
    pub in_high: f32,
    /// Exponent applied between the two ranges. One is linear.
    pub gamma: f32,
    /// Output value for `in_low`.
    pub out_low: f32,
    /// Output value for `in_high`.
    pub out_high: f32,
    /// Whether to work per channel or on luminance.
    pub channel: LevelsChannel,
}

impl Default for Levels {
    fn default() -> Self {
        Self {
            input: Input::default(),
            in_low: 0.0,
            in_high: 1.0,
            gamma: 1.0,
            out_low: 0.0,
            out_high: 1.0,
            channel: LevelsChannel::default(),
        }
    }
}

impl Levels {
    /// Remap a field, starting from the identity.
    pub fn new(input: impl Into<Input>) -> Self {
        Self {
            input: input.into(),
            ..Self::default()
        }
    }

    /// Set the input value that becomes `out_low`.
    pub fn in_low(mut self, in_low: f32) -> Self {
        self.in_low = in_low;
        self
    }

    /// Set the input value that becomes `out_high`.
    pub fn in_high(mut self, in_high: f32) -> Self {
        self.in_high = in_high;
        self
    }

    /// Set both ends of the input range.
    pub fn in_range(mut self, low: f32, high: f32) -> Self {
        self.in_low = low;
        self.in_high = high;
        self
    }

    /// Set the exponent between the ranges.
    pub fn gamma(mut self, gamma: f32) -> Self {
        self.gamma = gamma;
        self
    }

    /// Set both ends of the output range.
    pub fn out_range(mut self, low: f32, high: f32) -> Self {
        self.out_low = low;
        self.out_high = high;
        self
    }

    /// Work on luminance rather than per channel.
    pub fn luminance(mut self) -> Self {
        self.channel = LevelsChannel::Luminance;
        self
    }
}

ports!(Levels, Output::SameAs("input"), "input": Field => input);

impl Check for Levels {
    fn check(&self, path: &str) -> Result<(), GraphError> {
        for (field, value) in [
            ("in_low", self.in_low),
            ("in_high", self.in_high),
            ("out_low", self.out_low),
            ("out_high", self.out_high),
            ("gamma", self.gamma),
        ] {
            finite(value, &format!("{path}.{field}"), field)?;
        }
        require(
            (self.in_high - self.in_low).abs() > 0.0,
            &format!("{path}.in_high"),
            "the input range must not be empty",
        )?;
        require(
            self.gamma > 0.0,
            &format!("{path}.gamma"),
            "gamma must be positive",
        )
    }
}

/// A monotone curve through control points, evaluated as a lookup.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
#[must_use]
pub struct Curve {
    /// The field to remap.
    pub input: Input,
    /// Control points as `[x, y]`, in strictly increasing `x`.
    pub points: Vec<[f32; 2]>,
}

impl Default for Curve {
    fn default() -> Self {
        Self {
            input: Input::default(),
            points: vec![[0.0, 0.0], [1.0, 1.0]],
        }
    }
}

impl Curve {
    /// Remap a field, starting from the identity curve.
    pub fn new(input: impl Into<Input>) -> Self {
        Self {
            input: input.into(),
            ..Self::default()
        }
    }

    /// Replace the control points.
    pub fn points(mut self, points: impl IntoIterator<Item = [f32; 2]>) -> Self {
        self.points = points.into_iter().collect();
        self
    }
}

ports!(Curve, Output::SameAs("input"), "input": Field => input);

impl Check for Curve {
    fn check(&self, path: &str) -> Result<(), GraphError> {
        let path = format!("{path}.points");
        require(self.points.len() >= 2, &path, "a curve needs two points")?;
        let mut previous = f32::NEG_INFINITY;
        for [x, y] in &self.points {
            finite(*x, &path, "a control point")?;
            finite(*y, &path, "a control point")?;
            require(*x > previous, &path, "control points must increase in x")?;
            previous = *x;
        }
        Ok(())
    }
}

/// One stop of a colour gradient.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GradientStop {
    /// Where the stop sits along the input, usually in `0..=1`.
    pub position: f32,
    /// Linear RGB at that position.
    pub color: [f32; 3],
}

impl From<(f32, [f32; 3])> for GradientStop {
    fn from((position, color): (f32, [f32; 3])) -> Self {
        Self { position, color }
    }
}

/// A float through a gradient of linear colour stops.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
#[must_use]
pub struct Colorize {
    /// The field to colour.
    pub input: Input,
    /// Stops in increasing position. Values between stops interpolate linearly.
    pub gradient: Vec<GradientStop>,
}

impl Default for Colorize {
    fn default() -> Self {
        Self {
            input: Input::default(),
            gradient: vec![
                GradientStop {
                    position: 0.0,
                    color: [0.0; 3],
                },
                GradientStop {
                    position: 1.0,
                    color: [1.0; 3],
                },
            ],
        }
    }
}

impl Colorize {
    /// Colour a field, starting from black to white.
    pub fn new(input: impl Into<Input>) -> Self {
        Self {
            input: input.into(),
            ..Self::default()
        }
    }

    /// Replace the gradient. A `(position, color)` pair is a stop.
    pub fn gradient(mut self, stops: impl IntoIterator<Item = impl Into<GradientStop>>) -> Self {
        self.gradient = stops.into_iter().map(Into::into).collect();
        self
    }
}

ports!(Colorize, Output::Fixed(ValueType::Color), "input": Float => input);

impl Check for Colorize {
    fn check(&self, path: &str) -> Result<(), GraphError> {
        let path = format!("{path}.gradient");
        require(!self.gradient.is_empty(), &path, "a gradient needs a stop")?;
        let mut previous = f32::NEG_INFINITY;
        for stop in &self.gradient {
            finite(stop.position, &path, "a stop position")?;
            for channel in stop.color {
                finite(channel, &path, "a stop colour")?;
            }
            require(
                stop.position > previous,
                &path,
                "stops must increase in position",
            )?;
            previous = stop.position;
        }
        Ok(())
    }
}

/// Brightness, contrast, hue and saturation.
///
/// Hue and saturation have no meaning on one channel, so this node works in
/// colour: a float input broadcasts on the way in, and the result is a colour.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
#[must_use]
pub struct Adjust {
    /// The colour to adjust.
    pub input: Input,
    /// Added to every channel.
    pub brightness: f32,
    /// Scale about mid grey. One leaves the contrast alone.
    pub contrast: f32,
    /// Hue rotation in degrees.
    pub hue: f32,
    /// Saturation scale. Zero is greyscale, one leaves it alone.
    pub saturation: f32,
}

impl Default for Adjust {
    fn default() -> Self {
        Self {
            input: Input::default(),
            brightness: 0.0,
            contrast: 1.0,
            hue: 0.0,
            saturation: 1.0,
        }
    }
}

impl Adjust {
    /// Adjust a colour, starting from no change at all.
    pub fn new(input: impl Into<Input>) -> Self {
        Self {
            input: input.into(),
            ..Self::default()
        }
    }

    /// Add to every channel.
    pub fn brightness(mut self, brightness: f32) -> Self {
        self.brightness = brightness;
        self
    }

    /// Scale about mid grey.
    pub fn contrast(mut self, contrast: f32) -> Self {
        self.contrast = contrast;
        self
    }

    /// Rotate the hue, in degrees.
    pub fn hue(mut self, degrees: f32) -> Self {
        self.hue = degrees;
        self
    }

    /// Scale the saturation.
    pub fn saturation(mut self, saturation: f32) -> Self {
        self.saturation = saturation;
        self
    }
}

ports!(Adjust, Output::Fixed(ValueType::Color), "input": Color => input);

impl Check for Adjust {
    fn check(&self, path: &str) -> Result<(), GraphError> {
        for (field, value) in [
            ("brightness", self.brightness),
            ("contrast", self.contrast),
            ("hue", self.hue),
            ("saturation", self.saturation),
        ] {
            finite(value, &format!("{path}.{field}"), field)?;
        }
        require(
            self.saturation >= 0.0,
            &format!("{path}.saturation"),
            "saturation must not be negative",
        )
    }
}

/// A scalar operator. The unary ones ignore `b`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub enum MathOp {
    /// `a + b`.
    #[default]
    Add,
    /// `a - b`.
    Sub,
    /// `a * b`.
    Mul,
    /// `a / b`, zero where `b` is zero.
    Div,
    /// The smaller.
    Min,
    /// The larger.
    Max,
    /// `a` raised to `b`.
    Pow,
    /// `|a|`. Unary.
    Abs,
    /// The square root of `a`, zero below zero. Unary.
    Sqrt,
    /// The largest integer at most `a`. Unary.
    Floor,
    /// `a` less its floor. Unary.
    Fract,
    /// One where `a` is at least `b`, zero elsewhere.
    Step,
    /// A smooth ramp from zero at `b` to one at `b + 1`.
    Smoothstep,
    /// The sine of `a` turns. Unary.
    Sin,
    /// The cosine of `a` turns. Unary.
    Cos,
    /// The angle of `(b, a)` in turns.
    Atan2,
    /// The base-two logarithm of `a`. Unary.
    Log2,
    /// Two raised to `a`. Unary.
    Exp2,
}

impl MathOp {
    /// Whether the operator reads `a` alone.
    ///
    /// A unary operator answers in its first operand's type and lands on its
    /// first operand's lattice: an operand it never reads must not widen a
    /// float to a colour, nor move the period, just by being wired.
    pub fn is_unary(self) -> bool {
        matches!(
            self,
            Self::Abs
                | Self::Sqrt
                | Self::Floor
                | Self::Fract
                | Self::Sin
                | Self::Cos
                | Self::Log2
                | Self::Exp2
        )
    }
}

/// One scalar operator over one or two operands, per channel.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
#[must_use]
pub struct Math {
    /// Which operator.
    pub op: MathOp,
    /// The first operand.
    pub a: Input,
    /// The second operand, ignored by the unary operators.
    pub b: Input,
}

impl Math {
    /// Apply an operator to two operands.
    pub fn new(op: MathOp, a: impl Into<Input>, b: impl Into<Input>) -> Self {
        Self {
            op,
            a: a.into(),
            b: b.into(),
        }
    }

    /// Apply a unary operator.
    pub fn unary(op: MathOp, a: impl Into<Input>) -> Self {
        Self {
            op,
            a: a.into(),
            b: Input::float(0.0),
        }
    }
}

ports!(
    Math,
    |math| if math.op.is_unary() {
        Output::SameAs("a")
    } else {
        Output::Join("a", "b")
    },
    "a": Field => a,
    "b": Field => b,
);
impl Check for Math {}

/// Which channel [`Decompose`] takes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub enum Channel {
    /// Red, or u on a `Vec2`.
    #[default]
    R,
    /// Green, or v on a `Vec2`.
    G,
    /// Blue. A `Vec2` has no blue.
    B,
}

/// One channel of a colour, or one axis of a UV.
///
/// This is the only node that reads a [`ValueType::Vec2`], and the way a
/// gradient along one axis becomes a float: decompose a [`Uv`](super::Uv).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
#[must_use]
pub struct Decompose {
    /// The value to take a channel of.
    pub input: Input,
    /// Which channel.
    pub channel: Channel,
}

impl Decompose {
    /// Take a channel of a value.
    pub fn new(input: impl Into<Input>, channel: Channel) -> Self {
        Self {
            input: input.into(),
            channel,
        }
    }
}

ports!(Decompose, Output::Fixed(ValueType::Float), "input": Any => input);

impl Check for Decompose {
    fn check_types(&self, inputs: &[Resolved], path: &str) -> Result<(), GraphError> {
        let vec2 = inputs
            .iter()
            .any(|input| input.name == "input" && input.value_type == ValueType::Vec2);
        require(
            !vec2 || self.channel != Channel::B,
            &format!("{path}.channel"),
            "a Vec2 has u and v and no blue channel",
        )
    }
}

/// Three floats into a colour.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
#[must_use]
pub struct Combine {
    /// Red.
    pub r: Input,
    /// Green.
    pub g: Input,
    /// Blue.
    pub b: Input,
}

impl Combine {
    /// Build a colour from three channels.
    pub fn new(r: impl Into<Input>, g: impl Into<Input>, b: impl Into<Input>) -> Self {
        Self {
            r: r.into(),
            g: g.into(),
            b: b.into(),
        }
    }
}

ports!(
    Combine,
    Output::Fixed(ValueType::Color),
    "r": Float => r,
    "g": Float => g,
    "b": Float => b,
);
impl Check for Combine {}

/// Two floats into a [`ValueType::Vec2`].
///
/// [`Combine`]'s two-lane twin, and the other half of [`Decompose`]: a graph
/// that has taken a coordinate apart, done arithmetic on the axes and wants a
/// displacement back has had no way to write one. A [`Warp`](super::Warp)
/// offset built per axis wants this, and so does a strand layer's `direction`,
/// which is a `Vec2` port and reaches nothing else that produces one.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
#[must_use]
pub struct Combine2 {
    /// The first axis.
    pub u: Input,
    /// The second axis.
    pub v: Input,
}

impl Combine2 {
    /// Build a `Vec2` from two axes.
    pub fn new(u: impl Into<Input>, v: impl Into<Input>) -> Self {
        Self {
            u: u.into(),
            v: v.into(),
        }
    }
}

ports!(
    Combine2,
    Output::Fixed(ValueType::Vec2),
    "u": Float => u,
    "v": Float => v,
);
impl Check for Combine2 {}

/// Which field a [`Direction`] builds its orientation out of.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub enum DirectionSource {
    /// An angle in **turns**, as [`MathOp::Sin`] and
    /// [`DirectionalWarp`](super::DirectionalWarp) take one: the direction is
    /// `(cos, sin)` of it, so a noise in `0..=1` sweeps the whole circle.
    #[default]
    Angle,
    /// A height field: the direction is its wrapped gradient, pointing uphill,
    /// at unit length. Flat ground has no gradient and answers a zero vector.
    Slope,
}

/// An orientation field: a unit [`ValueType::Vec2`] per texel.
///
/// [`Uv`](super::Uv) is the only other node in the vocabulary that produces a
/// `Vec2`, and a coordinate is not an orientation, so without this a graph
/// cannot build one at all. What wants one is anything that has to *comb* a
/// field rather than displace it: a [`Warp`](super::Warp) offset that follows a
/// surface, and a strand layer's `direction`, which is the way a blade or a
/// fibre leans.
///
/// The two sources are two different questions. [`DirectionSource::Angle`] is
/// an author saying which way, in turns, out of a noise or a gradient.
/// [`DirectionSource::Slope`] is the surface saying it: the gradient of a
/// height, which points uphill. [`Self::rotate_quarter`] turns either a quarter
/// turn counter-clockwise, which on a slope is the flow *along* the contours
/// rather than up them — the difference between fur combed down a shoulder and
/// fur combed around it.
///
/// The gradient is taken over a rasterised plane, by the same wrapped central
/// difference [`NormalFromHeight`] takes, because "at texel size" is a fact
/// about the plane rather than about the graph. So the slope form costs one
/// plane and the angle form costs nothing.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
#[must_use]
pub struct Direction {
    /// The angle, in turns, or the height field, depending on [`Self::source`].
    pub input: Input,
    /// Which question the input answers.
    pub source: DirectionSource,
    /// Turn the answer a quarter turn counter-clockwise.
    pub rotate_quarter: bool,
}

impl Direction {
    /// A direction from an angle in turns.
    pub fn from_angle(input: impl Into<Input>) -> Self {
        Self {
            input: input.into(),
            source: DirectionSource::Angle,
            rotate_quarter: false,
        }
    }

    /// A direction from the uphill gradient of a height field.
    pub fn from_slope(input: impl Into<Input>) -> Self {
        Self {
            input: input.into(),
            source: DirectionSource::Slope,
            rotate_quarter: false,
        }
    }

    /// Turn the answer a quarter turn, which takes a gradient along the
    /// contours instead of up them.
    pub fn rotate_quarter(mut self) -> Self {
        self.rotate_quarter = true;
        self
    }
}

ports!(
    Direction,
    Output::Fixed(ValueType::Vec2),
    "input": Float => input,
);
impl Check for Direction {}

/// One less the input, per channel.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
#[must_use]
pub struct Invert {
    /// The field to invert.
    pub input: Input,
}

impl Invert {
    /// Invert a field.
    pub fn new(input: impl Into<Input>) -> Self {
        Self {
            input: input.into(),
        }
    }
}

ports!(Invert, Output::SameAs("input"), "input": Field => input);
impl Check for Invert {}

/// The input held between two bounds.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
#[must_use]
pub struct Clamp {
    /// The field to hold.
    pub input: Input,
    /// The lower bound.
    pub low: Input,
    /// The upper bound.
    pub high: Input,
}

impl Default for Clamp {
    fn default() -> Self {
        Self {
            input: Input::default(),
            low: Input::float(0.0),
            high: Input::float(1.0),
        }
    }
}

impl Clamp {
    /// Hold a field to the unit interval.
    pub fn new(input: impl Into<Input>) -> Self {
        Self {
            input: input.into(),
            ..Self::default()
        }
    }

    /// Set both bounds.
    pub fn range(mut self, low: impl Into<Input>, high: impl Into<Input>) -> Self {
        self.low = low.into();
        self.high = high.into();
        self
    }
}

ports!(
    Clamp,
    Output::SameAs("input"),
    "input": Field => input,
    "low": Float => low,
    "high": Float => high,
);
impl Check for Clamp {}

/// A linear blend of two fields by a third.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
#[must_use]
pub struct Mix {
    /// The field at `t` zero.
    pub a: Input,
    /// The field at `t` one.
    pub b: Input,
    /// Where between them to read.
    pub t: Input,
}

impl Mix {
    /// Blend two fields by a third.
    pub fn new(a: impl Into<Input>, b: impl Into<Input>, t: impl Into<Input>) -> Self {
        Self {
            a: a.into(),
            b: b.into(),
            t: t.into(),
        }
    }
}

ports!(
    Mix,
    Output::Join("a", "b"),
    "a": Field => a,
    "b": Field => b,
    "t": Float => t,
);
impl Check for Mix {}

/// One of two fields, chosen by a condition.
///
/// The condition is a float because the graph has no boolean type: a `Bool`
/// parameter arrives as zero or one, and anything at least a half takes the
/// true branch.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
#[must_use]
pub struct Switch {
    /// At least a half takes `on_true`.
    pub condition: Input,
    /// The field taken when the condition holds.
    pub on_true: Input,
    /// The field taken when it does not.
    pub on_false: Input,
}

impl Switch {
    /// Choose between two fields.
    pub fn new(
        condition: impl Into<Input>,
        on_true: impl Into<Input>,
        on_false: impl Into<Input>,
    ) -> Self {
        Self {
            condition: condition.into(),
            on_true: on_true.into(),
            on_false: on_false.into(),
        }
    }
}

ports!(
    Switch,
    Output::Join("on_true", "on_false"),
    "condition": Float => condition,
    "on_true": Field => on_true,
    "on_false": Field => on_false,
);
impl Check for Switch {}

/// A tangent-space normal from a height field, by finite difference.
///
/// [`PbrOutput`](crate::PbrOutput) derives the material's own normal from its
/// height the same way; this node exists so a graph can blend normals itself.
/// The convention is OpenGL, with `+Y` up.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
#[must_use]
pub struct NormalFromHeight {
    /// The height field.
    pub height: Input,
    /// Metres of relief per unit height, over one repeat.
    pub strength: f32,
}

impl Default for NormalFromHeight {
    fn default() -> Self {
        Self {
            height: Input::default(),
            strength: 0.02,
        }
    }
}

impl NormalFromHeight {
    /// Derive a normal from a height field.
    pub fn new(height: impl Into<Input>) -> Self {
        Self {
            height: height.into(),
            ..Self::default()
        }
    }

    /// Set the relief in metres per unit height.
    pub fn strength(mut self, metres: f32) -> Self {
        self.strength = metres;
        self
    }
}

ports!(
    NormalFromHeight,
    Output::Fixed(ValueType::Color),
    "height": Float => height,
);

impl Check for NormalFromHeight {
    fn check(&self, path: &str) -> Result<(), GraphError> {
        check_unit(self.strength, "strength", path)
    }
}

/// A band of a height field, as a mask: one between [`Self::low`] and
/// [`Self::high`], nothing outside them, and [`Self::softness`] of ramp on each
/// bound.
///
/// The utility a height-driven material reaches for a dozen times: snow above a
/// line, damp below one, grout in the bottom of a joint, paint on the proud
/// face and bare metal in the dent. It is two [`Math`] smoothsteps and an
/// invert written out as the node an author was going to build anyway, and
/// naming it is most of its value — a graph that says `HeightToMask` reads as
/// what it is where a pair of smoothsteps reads as arithmetic.
///
/// A softness of zero is a hard band, because a smoothstep whose edges meet is
/// a step: the crate's own division-guarded smoothstep answers that rather than
/// an indeterminate value, which is the one place this node depends on the
/// emitter writing the op out rather than calling the built-in.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
#[must_use]
pub struct HeightToMask {
    /// The field to cut a band out of.
    pub input: Input,
    /// Where the band opens.
    pub low: f32,
    /// Where it closes.
    pub high: f32,
    /// How wide each of the two ramps is, centred on its bound.
    pub softness: f32,
}

impl Default for HeightToMask {
    fn default() -> Self {
        Self {
            input: Input::default(),
            low: 0.5,
            high: 1.0,
            softness: 0.1,
        }
    }
}

impl HeightToMask {
    /// Everything at or above a height.
    pub fn above(input: impl Into<Input>, low: f32) -> Self {
        Self {
            input: input.into(),
            low,
            ..Self::default()
        }
    }

    /// Everything at or below a height.
    pub fn below(input: impl Into<Input>, high: f32) -> Self {
        Self {
            input: input.into(),
            low: 0.0,
            high,
            ..Self::default()
        }
    }

    /// A band between two heights.
    pub fn band(input: impl Into<Input>, low: f32, high: f32) -> Self {
        Self {
            input: input.into(),
            low,
            high,
            ..Self::default()
        }
    }

    /// Set the width of each ramp.
    pub fn softness(mut self, softness: f32) -> Self {
        self.softness = softness;
        self
    }
}

ports!(
    HeightToMask,
    Output::Fixed(ValueType::Float),
    "input": Float => input,
);

impl Check for HeightToMask {
    fn check(&self, path: &str) -> Result<(), GraphError> {
        finite(self.low, &format!("{path}.low"), "low")?;
        finite(self.high, &format!("{path}.high"), "high")?;
        require(
            self.high >= self.low,
            &format!("{path}.high"),
            "high must be at least low; a band that closes before it opens is a mask of nothing",
        )?;
        check_unit(self.softness, "softness", path)
    }
}

/// Which boolean two signed distance fields are put through.
///
/// The names are the operation on the *shapes* the fields describe rather than
/// on the numbers, and a field is negative inside its shape, so the union of
/// two shapes is the smaller of the two distances and their intersection is the
/// larger. Writing it the other way round is the mistake this enum exists to
/// stop an author making twice.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub enum SdfOp {
    /// Both shapes, which is `min(a, b)`.
    #[default]
    Union,
    /// What the two shapes share, which is `max(a, b)`.
    Intersect,
    /// `a` with `b` taken out of it, which is `max(a, -b)`: negating a signed
    /// distance turns its shape inside out, and intersecting with that is what
    /// cutting a hole means.
    Subtract,
}

/// Two signed distance fields combined, with an optional fillet where they
/// meet.
///
/// What this reads and what it answers are distances rather than masks — a
/// [`Shape`](super::Shape) under
/// [`ShapeOutput::Distance`](super::ShapeOutput::Distance), or another combine
/// — so booleans chain, and an [`SdfMask`] at the end of the chain is what
/// turns the last field into a picture. Combining masks instead would give the
/// same silhouette and a field with nothing left in it: the minimum of two
/// ramps has no distance to fillet, to ramp at a width of its own or to offset
/// later, which is the whole reason the distance is carried this far.
///
/// `smooth` is the width of the fillet in UV, and zero is the hard boolean, bit
/// for bit: the polynomial below divides by that width, and a fillet of no
/// width is not a rounded crease but the crease itself, so the node emits
/// `min`, `max` or `max(a, -b)` and nothing else. Anything more rounds the
/// meeting of the two fields over a band that wide — a weld rather than a
/// joint, a boss growing out of a plate rather than sitting on it — and costs
/// about a dozen instructions to do it.
///
/// The fillet is only approximately a distance, as every smooth minimum is: it
/// is exact outside the band and a little short inside it, which shows as a
/// ramp slightly narrower than the one asked for when a mask is taken right at
/// a fillet. Nothing downstream minds, and the alternative is a root per texel.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
#[must_use]
pub struct SdfCombine {
    /// Which boolean.
    pub op: SdfOp,
    /// The first field, and the one a subtraction keeps.
    pub a: Input,
    /// The second field, and the one a subtraction takes away.
    pub b: Input,
    /// The width of the fillet where the two fields meet, in UV. Zero is the
    /// hard boolean.
    pub smooth: f32,
}

impl SdfCombine {
    /// Combine two distance fields, hard.
    pub fn new(op: SdfOp, a: impl Into<Input>, b: impl Into<Input>) -> Self {
        Self {
            op,
            a: a.into(),
            b: b.into(),
            smooth: 0.0,
        }
    }

    /// Round the meeting of the two over a band this wide, in UV.
    pub fn smooth(mut self, smooth: f32) -> Self {
        self.smooth = smooth;
        self
    }
}

ports!(
    SdfCombine,
    Output::Fixed(ValueType::Float),
    "a": Float => a,
    "b": Float => b,
);

impl Check for SdfCombine {
    fn check(&self, path: &str) -> Result<(), GraphError> {
        // A width in UV, like every other width here, and a negative one would
        // put the fillet on the wrong side of the crease and answer a field
        // that bulges outward where it should tuck in.
        check_unit(self.smooth, "smooth", path)
    }
}

/// A signed distance read as a mask: one inside the shape, zero outside it,
/// with the falloff taken inward from the boundary.
///
/// This is the ramp a [`Shape`](super::Shape) puts its own distance through,
/// factored into a node of its own so that the two cannot drift apart. A shape
/// under [`ShapeOutput::Distance`](super::ShapeOutput::Distance) through this
/// node is that shape under
/// [`ShapeOutput::Mask`](super::ShapeOutput::Mask), instruction for
/// instruction, and a test pins it — which is what makes a boolean between two
/// shapes readable as a shape rather than as a second thing that only looks
/// like one.
///
/// `edge` is the width of the falloff in UV, taken inward, so a mask reaches
/// exactly as far as the boundary of the field it ramped whatever it is set to.
/// Zero is a hard edge, which rests on the crate's own guarded smoothstep
/// answering a step where the built-in would divide by zero.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
#[must_use]
pub struct SdfMask {
    /// The signed distance to read.
    pub input: Input,
    /// Width of the falloff at the edge, in UV, taken inward.
    pub edge: f32,
}

impl Default for SdfMask {
    fn default() -> Self {
        Self {
            input: Input::default(),
            // The same default a `Shape`'s edge has, so that a shape read
            // through this node without a setting in sight is the shape's own
            // mask rather than a harder-edged relative of it.
            edge: 0.05,
        }
    }
}

impl SdfMask {
    /// Read a signed distance as a mask.
    pub fn new(input: impl Into<Input>) -> Self {
        Self {
            input: input.into(),
            ..Self::default()
        }
    }

    /// Set the width of the falloff, in UV.
    pub fn edge(mut self, edge: f32) -> Self {
        self.edge = edge;
        self
    }
}

ports!(
    SdfMask,
    Output::Fixed(ValueType::Float),
    "input": Float => input,
);

impl Check for SdfMask {
    fn check(&self, path: &str) -> Result<(), GraphError> {
        check_unit(self.edge, "edge", path)
    }
}

/// One lane of a vector.
///
/// A float arrived broadcast, and every channel of a broadcast is the float
/// itself, so decomposing one is free.
impl Lower for Decompose {
    fn lower(&self, cx: &mut Lowering, inputs: &NodeInputs) -> ValueId {
        let input = inputs.value("input");
        if cx.type_of(input) == IrType::Float {
            return input;
        }
        let channel = match self.channel {
            Channel::R => 0,
            Channel::G => 1,
            Channel::B => 2,
        };
        cx.emit(Op::Extract(channel), [input])
    }
}

/// Three channels into one value.
impl Lower for Combine {
    fn lower(&self, cx: &mut Lowering, inputs: &NodeInputs) -> ValueId {
        let (r, g, b) = (inputs.value("r"), inputs.value("g"), inputs.value("b"));
        cx.vector(&[r, g, b])
    }
}

/// Two axes into one displacement.
impl Lower for Combine2 {
    fn lower(&self, cx: &mut Lowering, inputs: &NodeInputs) -> ValueId {
        let (u, v) = (inputs.value("u"), inputs.value("v"));
        cx.vector(&[u, v])
    }
}

/// The relief a [`Direction`] takes its gradient through.
///
/// One, because the strength cancels: the plane holds `(-s * du, -s * dv, 1)`
/// normalised, and the ratio of the first two lanes — which is all the
/// direction below reads, before normalising them again — does not depend on
/// `s`. So there is no number here for an author to get wrong, and a height
/// whose slope is read twice is one plane rather than two.
const SLOPE_RELIEF: f32 = 1.0;

/// An orientation, from an angle or from the slope of a height.
///
/// The angle form is the unit circle: turns into radians, then `(cos, sin)`.
/// The slope form reads the same plane [`NormalFromHeight`] reads and takes the
/// two tangential lanes of the normal back apart. Those are `-s` times the
/// central difference of the height, so negating them points the vector uphill
/// and normalising it makes it a direction rather than a slope; ground that is
/// flat leaves a normal of `(0, 0, 1)` and so a direction of zero, which is
/// what "no direction here" has to look like.
///
/// The quarter turn is written out as `(x, y) -> (-y, x)` rather than emitted
/// through the frame's constant rotation, which would fold `cos(pi / 2)` to the
/// `-4.4e-8` an `f32` sine and cosine actually answer and leave every combed
/// direction that far off true.
impl Lower for Direction {
    fn lower(&self, cx: &mut Lowering, inputs: &NodeInputs) -> ValueId {
        let input = inputs.value("input");
        let direction = match self.source {
            DirectionSource::Angle => {
                let per_turn = k(cx, TURN);
                let radians = mul(cx, input, per_turn);
                let (cosine, sine) = (cos(cx, radians), sin(cx, radians));
                vec2(cx, cosine, sine)
            }
            DirectionSource::Slope => {
                let buffer = cx.buffer(
                    input,
                    Filter::Normal {
                        strength: SLOPE_RELIEF,
                    },
                    None,
                );
                let uv = cx.uv();
                let normal = cx.sample(buffer, uv);
                let (across, along) = axes(cx, normal);
                let tangential = vec2(cx, across, along);
                let uphill = k(cx, -1.0);
                let gradient = mul(cx, tangential, uphill);
                normalize(cx, gradient)
            }
        };
        if !self.rotate_quarter {
            return direction;
        }
        let (x, y) = axes(cx, direction);
        let flip = k(cx, -1.0);
        let turned = mul(cx, y, flip);
        vec2(cx, turned, x)
    }
}

/// One less the input, at whatever width the input arrived at.
impl Lower for Invert {
    fn lower(&self, cx: &mut Lowering, inputs: &NodeInputs) -> ValueId {
        let input = inputs.value("input");
        let one = cx.constant(1.0);
        cx.emit(Op::Sub, [one, input])
    }
}

/// A linear blend, which is the operation the IR is named after.
impl Lower for Mix {
    fn lower(&self, cx: &mut Lowering, inputs: &NodeInputs) -> ValueId {
        let (a, b, t) = (inputs.value("a"), inputs.value("b"), inputs.value("t"));
        cx.emit(Op::Mix, [a, b, t])
    }
}

/// The input range mapped onto the output range, through a gamma.
///
/// `t` is clamped to `0..=1` before the gamma, which is what makes this a range
/// remap rather than an extrapolation: an input below `in_low` is `out_low` and
/// one above `in_high` is `out_high`. It also keeps the base of the power
/// non-negative, which a fractional exponent needs.
fn remap(cx: &mut Lowering, levels: &Levels, value: ValueId) -> ValueId {
    let low = cx.constant(levels.in_low);
    // Validation rejected an empty input range, so this never divides by zero.
    let span = cx.constant(levels.in_high - levels.in_low);
    let offset = cx.emit(Op::Sub, [value, low]);
    let ramp = cx.emit(Op::Div, [offset, span]);
    let zero = cx.constant(0.0);
    let one = cx.constant(1.0);
    let held = cx.emit(Op::Clamp, [ramp, zero, one]);
    #[expect(
        clippy::float_cmp,
        reason = "an exact one is the identity exponent, and raising to it is \
                  an instruction every graph would otherwise carry"
    )]
    let curved = if levels.gamma == 1.0 {
        held
    } else {
        let gamma = cx.constant(levels.gamma);
        cx.emit(Op::Pow, [held, gamma])
    };
    let out_low = cx.constant(levels.out_low);
    let out_high = cx.constant(levels.out_high);
    cx.emit(Op::Mix, [out_low, out_high, curved])
}

impl Lower for Levels {
    fn lower(&self, cx: &mut Lowering, inputs: &NodeInputs) -> ValueId {
        let input = inputs.value("input");
        if self.channel == LevelsChannel::PerChannel || cx.type_of(input) == IrType::Float {
            // Every op is component-wise, so one expression is the whole of
            // per-channel; on one channel the two modes are the same curve.
            return remap(cx, self, input);
        }
        // On luminance the curve moves the brightness and the colour follows
        // it: the ratio is what the curve did, and a black texel stays black
        // rather than taking the whole output range.
        let brightness = cx.convert(input, IrType::Float);
        let curved = remap(cx, self, brightness);
        let ratio = cx.emit(Op::Div, [curved, brightness]);
        cx.emit(Op::Mul, [input, ratio])
    }
}

/// A float through a gradient of linear stops.
///
/// Each segment is mixed in over the one before it by its own clamped ramp:
/// below a segment the ramp is zero and leaves what came before, above it the
/// ramp is one and the segment's colour stands. So the ends hold the first and
/// last stop and everything between is piecewise linear, which is what a
/// gradient of linear stops means.
impl Lower for Colorize {
    fn lower(&self, cx: &mut Lowering, inputs: &NodeInputs) -> ValueId {
        let input = inputs.value("input");
        let zero = cx.constant(0.0);
        let one = cx.constant(1.0);
        let Some(first) = self.gradient.first() else {
            // Validation rejects an empty gradient; a colour is still owed here.
            return cx.value(Value::Color([0.0; 3]));
        };
        let mut colour = cx.value(Value::Color(first.color));
        for pair in self.gradient.windows(2) {
            let [previous, next] = pair else {
                continue;
            };
            let start = cx.constant(previous.position);
            // Validation rejects stops that do not increase, so this is
            // positive and the ramp runs the way it reads.
            let span = cx.constant(next.position - previous.position);
            let offset = cx.emit(Op::Sub, [input, start]);
            let ramp = cx.emit(Op::Div, [offset, span]);
            let held = cx.emit(Op::Clamp, [ramp, zero, one]);
            let stop = cx.value(Value::Color(next.color));
            colour = cx.emit(Op::Mix, [colour, stop, held]);
        }
        colour
    }
}

/// One turn in radians. Every angle in the graph is in turns, and every
/// angle in the IR is not.
const TURN: f32 = std::f32::consts::TAU;

impl Lower for Math {
    fn lower(&self, cx: &mut Lowering, inputs: &NodeInputs) -> ValueId {
        let a = inputs.value("a");
        // `b` is resolved where the operator takes a second operand and
        // nowhere else. That is the same rule the answer's type below applies
        // and the same one period inference applies, and a unary lowering that
        // reads `b` anyway would be the one of the three that disagreed —
        // dead code removal drops the instructions either way.
        let b = || inputs.value("b");
        let value = match self.op {
            MathOp::Add => cx.emit(Op::Add, [a, b()]),
            MathOp::Sub => cx.emit(Op::Sub, [a, b()]),
            MathOp::Mul => cx.emit(Op::Mul, [a, b()]),
            MathOp::Div => cx.emit(Op::Div, [a, b()]),
            MathOp::Min => cx.emit(Op::Min, [a, b()]),
            MathOp::Max => cx.emit(Op::Max, [a, b()]),
            MathOp::Pow => cx.emit(Op::Pow, [a, b()]),
            MathOp::Abs => cx.emit(Op::Abs, [a]),
            // Zero below zero rather than the NaN a square root would answer,
            // because one NaN texel spreads through every filter after it.
            MathOp::Sqrt => {
                let zero = cx.constant(0.0);
                let positive = cx.emit(Op::Max, [a, zero]);
                cx.emit(Op::Sqrt, [positive])
            }
            MathOp::Floor => cx.emit(Op::Floor, [a]),
            MathOp::Fract => cx.emit(Op::Fract, [a]),
            // `b` is the edge and `a` is what is weighed against it.
            MathOp::Step => cx.emit(Op::Step, [b(), a]),
            // The ramp runs one unit from `b`, as the operator documents.
            MathOp::Smoothstep => {
                let edge = b();
                let one = cx.constant(1.0);
                let high = cx.emit(Op::Add, [edge, one]);
                cx.emit(Op::Smoothstep, [edge, high, a])
            }
            // The graph's angles are turns; the IR's are radians.
            MathOp::Sin => {
                let turn = cx.constant(TURN);
                let radians = cx.emit(Op::Mul, [a, turn]);
                cx.emit(Op::Sin, [radians])
            }
            MathOp::Cos => {
                let turn = cx.constant(TURN);
                let radians = cx.emit(Op::Mul, [a, turn]);
                cx.emit(Op::Cos, [radians])
            }
            MathOp::Atan2 => {
                let radians = cx.emit(Op::Atan2, [a, b()]);
                let turn = cx.constant(TURN);
                cx.emit(Op::Div, [radians, turn])
            }
            MathOp::Log2 => cx.emit(Op::Log2, [a]),
            MathOp::Exp2 => cx.emit(Op::Exp2, [a]),
        };
        // A binary operator answers in the two operands joined, and a unary one
        // in `a` alone, because it never read `b`. Either way the conversion is
        // what keeps the expression's type and the port's declared one the same.
        let answer = if self.op.is_unary() {
            cx.type_of(a)
        } else {
            cx.type_of(a).max(cx.type_of(b()))
        };
        cx.convert(value, answer)
    }
}

/// The lattice a [`BlendMode::Dissolve`] hashes on.
///
/// The finest a bake can carry, so a dissolve is a per-texel dither at every
/// resolution a bake allows rather than a pattern of visible squares at the
/// large ones. It is deliberately not counted as a lattice the bake has to
/// resolve — [`node_lattice`](crate::period) leaves a blend alone — because
/// what a dissolve lays is noise rather than a feature, and asking a bake for
/// four thousand texels to resolve a dither is the wrong end of the trade.
/// It still tiles: cell zero and cell four thousand and ninety-six are the same
/// cell, as they are for every other hash in the crate. The number is
/// [`MAX_PERIOD`](crate::MAX_PERIOD) written out, because a cast of it would be
/// a cast and this is a literal either way.
const DISSOLVE_LATTICE: f32 = 4096.0;

/// The seed a dissolve hashes with. [`Blend`] has no seed of its own, and a
/// dither wants to be the same dither wherever it is asked for.
const DISSOLVE_SEED: u32 = 0x2545_F491;

/// A number per texel, in `0..=1`, that tiles.
fn dither(cx: &mut Lowering) -> ValueId {
    let uv = cx.uv();
    let lattice = k2(cx, DISSOLVE_LATTICE, DISSOLVE_LATTICE);
    let coordinate = mul(cx, uv, lattice);
    cell_hash(cx, coordinate, lattice, DISSOLVE_SEED)
}

impl Lower for Blend {
    fn lower(&self, cx: &mut Lowering, inputs: &NodeInputs) -> ValueId {
        let (a, b, opacity) = (
            inputs.value("a"),
            inputs.value("b"),
            inputs.value("opacity"),
        );
        // A dissolve is the one mode whose opacity is not a weight: it is the
        // threshold the per-texel hash is read against, so there is no blend
        // for the mix below to take part of.
        if self.mode == BlendMode::Dissolve {
            let hash = dither(cx);
            let above = step(cx, opacity, hash);
            let take = complement(cx, above);
            return mix(cx, a, b, take);
        }
        let blended = match self.mode {
            BlendMode::Multiply => mul(cx, a, b),
            BlendMode::Add => add(cx, a, b),
            BlendMode::Subtract => sub(cx, a, b),
            // The inverse of the product of the inverses.
            BlendMode::Screen => {
                let left = complement(cx, a);
                let right = complement(cx, b);
                let product = mul(cx, left, right);
                complement(cx, product)
            }
            // Multiply where the backdrop is dark and screen where it is
            // light, chosen per channel by a step rather than a branch.
            BlendMode::Overlay => {
                let doubled = mul(cx, a, b);
                let two = k(cx, 2.0);
                let dark = mul(cx, doubled, two);
                let left = complement(cx, a);
                let right = complement(cx, b);
                let product = mul(cx, left, right);
                let product = mul(cx, product, two);
                let light = complement(cx, product);
                let half = k(cx, 0.5);
                let bright = step(cx, half, a);
                mix(cx, dark, light, bright)
            }
            BlendMode::Difference => {
                let gap = sub(cx, a, b);
                abs(cx, gap)
            }
            BlendMode::Lighten => max(cx, a, b),
            BlendMode::Darken => min(cx, a, b),
            // Pegtop's soft light, `(1 - 2b) a^2 + 2ab`, which is smooth
            // everywhere and has no discontinuity at a half the way the W3C's
            // two-branch formula does. At `b = 0.5` it is the backdrop
            // untouched, below it the backdrop is darkened and above it
            // lightened, which is what the mode is for.
            BlendMode::SoftLight => {
                let two = k(cx, 2.0);
                let twice = mul(cx, b, two);
                let weight = complement(cx, twice);
                let squared = mul(cx, a, a);
                let darkened = mul(cx, weight, squared);
                let product = mul(cx, a, b);
                let lightened = mul(cx, product, two);
                add(cx, darkened, lightened)
            }
            // Answered above, before the opacity is read as a weight; this
            // arm is unreachable and is here so that the match is a match.
            BlendMode::Normal | BlendMode::Dissolve => b,
        };
        // Opacity is what the blend is worth over the backdrop, so zero is `a`
        // untouched whatever the mode did.
        mix(cx, a, blended, opacity)
    }
}

/// The tangent at each control point of a monotone cubic through them.
///
/// Fritsch and Carlson's construction: start from the average of the two
/// neighbouring secants, then pull a tangent back wherever it would make the
/// segment overshoot. The circle condition `alpha^2 + beta^2 <= 9` is what
/// guarantees the result is monotone on every segment, so a curve through
/// control points that only rise never dips between them — which is the whole
/// reason an author reaches for a curve rather than a chain of mixes.
///
/// A flat segment pins both of its tangents at zero, because anything else
/// would have to leave the segment and come back.
fn tangents(points: &[[f32; 2]]) -> Vec<f32> {
    let secants: Vec<f32> = points
        .windows(2)
        .map(|pair| match pair {
            [left, right] => (right[1] - left[1]) / (right[0] - left[0]),
            _ => 0.0,
        })
        .collect();
    let mut slopes: Vec<f32> = Vec::with_capacity(points.len());
    for index in 0..points.len() {
        let before = index.checked_sub(1).and_then(|before| secants.get(before));
        let after = secants.get(index);
        slopes.push(match (before, after) {
            (Some(before), Some(after)) => (before + after) * 0.5,
            (Some(before), None) => *before,
            (None, Some(after)) => *after,
            (None, None) => 0.0,
        });
    }
    for (index, secant) in secants.iter().enumerate() {
        let (Some(left), Some(right)) =
            (slopes.get(index).copied(), slopes.get(index + 1).copied())
        else {
            continue;
        };
        if *secant == 0.0 {
            if let Some(slot) = slopes.get_mut(index) {
                *slot = 0.0;
            }
            if let Some(slot) = slopes.get_mut(index + 1) {
                *slot = 0.0;
            }
            continue;
        }
        let alpha = left / secant;
        let beta = right / secant;
        let radius = alpha.mul_add(alpha, beta * beta);
        if radius <= 9.0 {
            continue;
        }
        let pull = 3.0 / radius.sqrt();
        if let Some(slot) = slopes.get_mut(index) {
            *slot = pull * alpha * secant;
        }
        if let Some(slot) = slopes.get_mut(index + 1) {
            *slot = pull * beta * secant;
        }
    }
    slopes
}

/// A monotone cubic through control points, as one expression.
///
/// Every segment is a Hermite cubic, clamped to its own span so that outside it
/// the segment answers its own end values, and the segments are selected by
/// where the input falls: below the first control point the curve is the first
/// value and above the last it is the last, which is what a lookup does at its
/// ends. The selection is a `mix` by a `step` rather than a `select`, because a
/// colour is remapped channel by channel and a select's condition is one
/// number.
impl Lower for Curve {
    fn lower(&self, cx: &mut Lowering, inputs: &NodeInputs) -> ValueId {
        let input = inputs.value("input");
        let slopes = tangents(&self.points);
        let zero = k(cx, 0.0);
        let one = k(cx, 1.0);
        let mut value = None;
        for (index, pair) in self.points.windows(2).enumerate() {
            let ([left, right], Some(entry), Some(exit)) = (
                pair,
                slopes.get(index).copied(),
                slopes.get(index + 1).copied(),
            ) else {
                continue;
            };
            // Validation rejected control points that do not increase in x, so
            // the span is positive and the ramp runs the way it reads.
            let span = right[0] - left[0];
            let start = k(cx, left[0]);
            let stride = k(cx, 1.0 / span);
            let offset = sub(cx, input, start);
            let ramp = mul(cx, offset, stride);
            let t = clamp(cx, ramp, zero, one);
            let squared = mul(cx, t, t);
            let cubed = mul(cx, squared, t);
            // The Hermite basis, gathered per control point so that each is one
            // multiply: `y0 (2t^3 - 3t^2 + 1) + h m0 (t^3 - 2t^2 + t)
            //           + y1 (3t^2 - 2t^3) + h m1 (t^3 - t^2)`.
            let mut segment = None;
            for (weights, scale) in [
                ([2.0, -3.0, 0.0, 1.0], left[1]),
                ([1.0, -2.0, 1.0, 0.0], span * entry),
                ([-2.0, 3.0, 0.0, 0.0], right[1]),
                ([1.0, -1.0, 0.0, 0.0], span * exit),
            ] {
                let basis = polynomial(cx, cubed, squared, t, weights);
                let scale = k(cx, scale);
                let term = mul(cx, basis, scale);
                segment = Some(match segment {
                    Some(previous) => add(cx, previous, term),
                    None => term,
                });
            }
            let Some(segment) = segment else {
                continue;
            };
            value = Some(match value {
                Some(previous) => {
                    let after = step(cx, start, input);
                    mix(cx, previous, segment, after)
                }
                None => segment,
            });
        }
        // Validation rejected a curve with fewer than two points, so there is
        // always a segment; a curve that somehow had none is the input.
        value.unwrap_or(input)
    }
}

/// `w0 t^3 + w1 t^2 + w2 t + w3`, with the powers already to hand.
fn polynomial(
    cx: &mut Lowering,
    cubed: ValueId,
    squared: ValueId,
    t: ValueId,
    weights: [f32; 4],
) -> ValueId {
    let mut total = None;
    for (power, weight) in [
        (Some(cubed), weights[0]),
        (Some(squared), weights[1]),
        (Some(t), weights[2]),
        (None, weights[3]),
    ] {
        if weight == 0.0 {
            continue;
        }
        let scale = k(cx, weight);
        let term = match power {
            Some(power) => mul(cx, power, scale),
            None => scale,
        };
        total = Some(match total {
            Some(previous) => add(cx, previous, term),
            None => term,
        });
    }
    total.unwrap_or_else(|| k(cx, 0.0))
}

/// The matrix that rotates a colour about the grey axis by `radians`.
///
/// Rotating about `(1, 1, 1)` is what "hue" means with no colour space to
/// convert into and out of: grey stays grey, because it is the axis, and every
/// other colour turns around it at whatever distance it sits. It is not a
/// perceptual hue rotation — a hue wheel in a perceptual space is not a circle
/// about this axis — and it is the one every image editor's cheap slider is.
fn hue_matrix(radians: f32) -> [[f32; 3]; 3] {
    let (sine, cosine) = radians.sin_cos();
    let third = 1.0 / 3.0;
    let diagonal = cosine + (1.0 - cosine) * third;
    let near = (1.0 - cosine).mul_add(third, third.sqrt() * sine);
    let far = (1.0 - cosine).mul_add(third, -(third.sqrt() * sine));
    [
        [diagonal, far, near],
        [near, diagonal, far],
        [far, near, diagonal],
    ]
}

/// Hue, then saturation, then contrast, then brightness.
///
/// The order is the one an image editor's panel runs top to bottom, and it is
/// the order that makes each slider mean what it says: a hue rotation before a
/// desaturation turns a colour that is still there to turn, and a contrast
/// before a brightness scales about mid grey rather than about wherever the
/// brightness left it.
impl Lower for Adjust {
    fn lower(&self, cx: &mut Lowering, inputs: &NodeInputs) -> ValueId {
        let input = inputs.value("input");
        let mut colour = input;
        if self.hue.abs() > 0.0 {
            let matrix = hue_matrix(self.hue.to_radians());
            let mut channels = Vec::with_capacity(3);
            for row in matrix {
                let weights = cx.value(Value::Color(row));
                channels.push(dot(cx, colour, weights));
            }
            colour = cx.vector(&channels);
        }
        #[expect(
            clippy::float_cmp,
            reason = "an exact one is the identity, and the instructions it \
                      would emit are ones every graph would otherwise carry"
        )]
        if self.saturation != 1.0 {
            let grey = cx.convert(colour, IrType::Float);
            let amount = k(cx, self.saturation);
            colour = mix(cx, grey, colour, amount);
        }
        #[expect(clippy::float_cmp, reason = "an exact one is the identity, as above")]
        if self.contrast != 1.0 {
            let middle = k(cx, 0.5);
            let about = sub(cx, colour, middle);
            let amount = k(cx, self.contrast);
            let scaled = mul(cx, about, amount);
            colour = add(cx, scaled, middle);
        }
        if self.brightness != 0.0 {
            let amount = k(cx, self.brightness);
            colour = add(cx, colour, amount);
        }
        cx.convert(colour, IrType::Vec3)
    }
}

/// The input held between two bounds, per channel.
impl Lower for Clamp {
    fn lower(&self, cx: &mut Lowering, inputs: &NodeInputs) -> ValueId {
        let input = inputs.value("input");
        let low = inputs.value("low");
        let high = inputs.value("high");
        clamp(cx, input, low, high)
    }
}

/// One of two fields, chosen by a condition that is one number for the whole
/// value.
impl Lower for Switch {
    fn lower(&self, cx: &mut Lowering, inputs: &NodeInputs) -> ValueId {
        let condition = inputs.value("condition");
        let on_true = inputs.value("on_true");
        let on_false = inputs.value("on_false");
        select(cx, condition, on_true, on_false)
    }
}

/// A tangent-space normal from a height field, by the same wrapped central
/// difference the bake derives its own normal with.
///
/// This is a buffered node in everything but name. "At texel size" is a fact
/// about the plane rather than about the graph — a graph is authored once and
/// baked at whatever resolution was asked for — so the height is rasterised
/// into a plane and the difference is taken over the plane's own texels, which
/// is what [`Filter::Normal`] does. The cost is one plane, shared with any
/// other node that rasterises the same height.
///
/// The answer is a colour in `-1..=1` rather than in `0..=1`: it is a
/// direction, and the half-and-half encoding a normal map ships in belongs to
/// the encoder. [`PbrOutput`](crate::PbrOutput) derives the material's own
/// normal the same way from the height it binds, so a graph that blends normals
/// and a graph that binds a height are describing one surface.
impl Lower for NormalFromHeight {
    fn lower(&self, cx: &mut Lowering, inputs: &NodeInputs) -> ValueId {
        let height = inputs.value("height");
        let buffer = cx.buffer(
            height,
            Filter::Normal {
                strength: self.strength,
            },
            None,
        );
        let uv = cx.uv();
        cx.sample(buffer, uv)
    }
}

/// Two ramps that face one another: open at `low`, close at `high`.
///
/// Written as a product of a rising smoothstep and a falling one rather than as
/// a pair of branches, because a product is one expression in both backends and
/// has no discontinuity where the two ramps overlap — a band narrower than its
/// own softness comes out lower rather than wrong, which is the honest answer
/// to asking for a band that has no room to open.
impl Lower for HeightToMask {
    fn lower(&self, cx: &mut Lowering, inputs: &NodeInputs) -> ValueId {
        let input = inputs.value("input");
        let half = self.softness * 0.5;
        let opening = {
            let (from, to) = (k(cx, self.low - half), k(cx, self.low + half));
            smoothstep(cx, from, to, input)
        };
        let closing = {
            let (from, to) = (k(cx, self.high - half), k(cx, self.high + half));
            let past = smoothstep(cx, from, to, input);
            complement(cx, past)
        };
        mul(cx, opening, closing)
    }
}

/// Which of the two fields the fillet joins them at.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Fillet {
    /// The smaller, which is the union of the two shapes.
    Smaller,
    /// The larger, which is their intersection.
    Larger,
}

/// The polynomial smooth minimum, and its dual the smooth maximum.
///
/// `h` says where the two fields stand relative to one another, measured in
/// fillet widths and held to `0..=1`: zero where `a` is at least a width below
/// `b`, one where it is at least a width above, and linear between. So
/// `mix(b, a, h)` is the hard boolean itself at both ends of the band and a
/// blend of the two fields inside it, and `width * h * (1 - h)` is the fillet
/// proper — a quadratic that vanishes at both ends, which is what lets a texel
/// outside the band answer the hard boolean, to the last place a `mix` at its
/// own endpoint holds, and the two halves meet with no step between them.
///
/// The maximum is the same expression with the ramp running the other way and
/// the fillet added rather than taken off, which is `-min(-a, -b)` written out.
/// It is written as one function rather than as two because the two differ by
/// two signs, and two spellings of this are two chances for a union and an
/// intersection to round by different amounts.
fn filleted(cx: &mut Lowering, a: ValueId, b: ValueId, width: f32, joining: Fillet) -> ValueId {
    let radius = k(cx, width);
    let apart = sub(cx, b, a);
    let ramp = div(cx, apart, radius);
    let sense = if joining == Fillet::Smaller {
        0.5
    } else {
        -0.5
    };
    let shifted = affine(cx, ramp, sense, 0.5);
    let (low, high) = (k(cx, 0.0), k(cx, 1.0));
    let blend = clamp(cx, shifted, low, high);
    let between = mix(cx, b, a, blend);
    let rest = complement(cx, blend);
    let quadratic = mul(cx, blend, rest);
    let fillet = mul(cx, quadratic, radius);
    if joining == Fillet::Smaller {
        sub(cx, between, fillet)
    } else {
        add(cx, between, fillet)
    }
}

/// The boolean the operator names, hard or filleted.
///
/// The hard forms are emitted whenever the fillet has no width at all, and not
/// as a saving: the polynomial divides by that width, and the crate's guarded
/// division answers zero where a built-in would answer an infinity, which would
/// put the blend at a half everywhere and make the node the *average* of its
/// two inputs. A fillet of no width is the crease itself, so the crease is what
/// is written.
impl Lower for SdfCombine {
    fn lower(&self, cx: &mut Lowering, inputs: &NodeInputs) -> ValueId {
        let a = inputs.value("a");
        let b = inputs.value("b");
        // Taking a shape away is intersecting with everything that is not it,
        // and the complement of a signed distance is its negation: a point a
        // tenth inside the shape is a tenth outside its complement, and the
        // boundary, being zero, stays where it was.
        let b = if self.op == SdfOp::Subtract {
            let zero = k(cx, 0.0);
            sub(cx, zero, b)
        } else {
            b
        };
        match (self.op, self.smooth > 0.0) {
            (SdfOp::Union, false) => min(cx, a, b),
            (SdfOp::Intersect | SdfOp::Subtract, false) => max(cx, a, b),
            (SdfOp::Union, true) => filleted(cx, a, b, self.smooth, Fillet::Smaller),
            (SdfOp::Intersect | SdfOp::Subtract, true) => {
                filleted(cx, a, b, self.smooth, Fillet::Larger)
            }
        }
    }
}

/// The inward ramp, which is the one [`Shape`](super::Shape) applies to its own
/// distance and is written once for the two of them.
impl Lower for SdfMask {
    fn lower(&self, cx: &mut Lowering, inputs: &NodeInputs) -> ValueId {
        let input = inputs.value("input");
        inward_ramp(cx, input, self.edge)
    }
}
