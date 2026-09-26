//! Transforms: the nodes that change where a field is read, not what it carries.
//!
//! Each one may only change a period in ways that keep it an integer. Where it
//! cannot, the result is [`Period::Free`](crate::Period::Free), which is an
//! error only if it reaches the output.

use serde::{Deserialize, Serialize};

use crate::ir::{Lower, Lowering, NodeInputs, ValueId};
use crate::nodes::frame::{
    TURN, about_centre, add, axes, cell_hash, clamp, complement, cos, count, div, floor, fract, k,
    k2, max, mirror_fold, mul, polar, quadrant_fold, select, signed_hash, sin, step, sub, turn,
    turn_by, vec2,
};
use crate::{
    GraphError, Input, finite,
    nodes::{
        Check, Output, Resolved,
        generator::{check_count, check_unit},
        ports,
    },
    require,
};

/// Scale, rotation and translation of the sampling frame.
///
/// The order the three compose is written down here, because the period rule
/// rests on it: the coordinate is scaled, then rotated about the centre of the
/// repeat, then translated.
///
/// ```text
/// out(p) = in(rotate(scale * p) + translate)
/// ```
///
/// The period rules follow from that and are the whole point of the node: an
/// integer scale multiplies the period, a quarter turn exchanges the two axes'
/// periods **before** the scale multiplies them, and a translation changes
/// nothing while the source wraps. So a source at `4x8` read through a quarter
/// turn and a scale of two by three tiles `16x12`: the u axis takes the
/// source's 8 and multiplies by 2. Anything else — a fractional scale, an
/// arbitrary rotation, a clamped frame that moves at all — is free, and a free
/// field cannot reach the output.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
#[must_use]
pub struct Transform {
    /// The field to read.
    pub input: Input,
    /// Repeats of the source per repeat of the result, in u and v.
    pub scale: [f32; 2],
    /// Rotation in degrees. Only quarter turns keep a period.
    pub rotate: f32,
    /// Translation in UV, which never changes a period.
    pub translate: [f32; 2],
    /// Whether the source wraps outside `[0, 1)`. Clamping breaks the seam.
    pub repeat: bool,
}

impl Default for Transform {
    fn default() -> Self {
        Self {
            input: Input::default(),
            scale: [1.0; 2],
            rotate: 0.0,
            translate: [0.0; 2],
            repeat: true,
        }
    }
}

impl Transform {
    /// Read a field through a transformed frame.
    pub fn new(input: impl Into<Input>) -> Self {
        Self {
            input: input.into(),
            ..Self::default()
        }
    }

    /// Set the same scale in both axes.
    pub fn scale(mut self, scale: f32) -> Self {
        self.scale = [scale; 2];
        self
    }

    /// Set the scale per axis.
    pub fn scales(mut self, u: f32, v: f32) -> Self {
        self.scale = [u, v];
        self
    }

    /// Rotate by an angle in degrees.
    pub fn rotate(mut self, degrees: f32) -> Self {
        self.rotate = degrees;
        self
    }

    /// Translate in UV.
    pub fn translate(mut self, u: f32, v: f32) -> Self {
        self.translate = [u, v];
        self
    }

    /// Clamp the source outside `[0, 1)` instead of wrapping it.
    pub fn clamped(mut self) -> Self {
        self.repeat = false;
        self
    }
}

ports!(Transform, Output::SameAs("input"), "input": Any => input);

impl Check for Transform {
    fn check(&self, path: &str) -> Result<(), GraphError> {
        for (axis, scale) in ["u", "v"].into_iter().zip(self.scale) {
            let path = format!("{path}.scale");
            finite(scale, &path, "scale")?;
            require(
                scale != 0.0,
                &path,
                &format!("scale in {axis} must not be zero"),
            )?;
        }
        finite(self.rotate, &format!("{path}.rotate"), "rotation")?;
        for value in self.translate {
            finite(value, &format!("{path}.translate"), "translation")?;
        }
        Ok(())
    }
}

/// Scatters a source across the repeat: the dirt, pebble and debris workhorse.
///
/// Instances wrap at the edge of the repeat, so the result tiles once whatever
/// the source did.
///
/// [`Self::mask`] is what turns a scatter into a distribution. It is read once
/// per instance, at the *centre of the instance's own cell*, and compared
/// against a hash of that cell: an instance is drawn where the mask is at or
/// above its own number, so a mask of a half keeps about half of them and a
/// mask that falls off to nothing thins them out rather than fading them. That
/// is the whole reason it is a threshold and not a multiply — pebbles that
/// dissolve at the edge of a patch read as a mistake, and pebbles that thin out
/// read as gravel — and it is why the hash is per instance: without one, a mask
/// could only ever turn every instance of a cell's neighbourhood on together.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
#[must_use]
pub struct Tile {
    /// The field to scatter.
    pub input: Input,
    /// Where instances appear, read once per instance at its cell's centre and
    /// resolved against that instance's own hash. One everywhere by default,
    /// which keeps every instance and emits nothing.
    pub mask: Input,
    /// Instances across the repeat, in u and v.
    pub count: [u32; 2],
    /// How far neighbouring instances may overlap, in instance widths.
    pub overlap: f32,
    /// How much an instance's size may vary, in `0..=1`.
    pub scale_variation: f32,
    /// How much an instance's rotation may vary, in `0..=1` of a half turn.
    /// Any spin at all turns the instance's square, so the seam check measures
    /// it by its corner from here on rather than by half its width.
    pub rotation_variation: f32,
    /// How much an instance's opacity may vary, in `0..=1`.
    pub opacity_variation: f32,
    /// Varies the instances.
    pub seed: u32,
}

impl Default for Tile {
    fn default() -> Self {
        Self {
            input: Input::default(),
            mask: Input::float(1.0),
            count: [4, 4],
            overlap: 0.0,
            scale_variation: 0.0,
            rotation_variation: 0.0,
            opacity_variation: 0.0,
            seed: 0,
        }
    }
}

impl Tile {
    /// Scatter a field across the repeat.
    pub fn new(input: impl Into<Input>) -> Self {
        Self {
            input: input.into(),
            ..Self::default()
        }
    }

    /// Draw an instance only where a field is at or above its own hash.
    pub fn mask(mut self, mask: impl Into<Input>) -> Self {
        self.mask = mask.into();
        self
    }

    /// Set the same instance count in both axes.
    pub fn count(mut self, count: u32) -> Self {
        self.count = [count; 2];
        self
    }

    /// Set the instance count per axis.
    pub fn counts(mut self, u: u32, v: u32) -> Self {
        self.count = [u, v];
        self
    }

    /// Let neighbouring instances overlap.
    pub fn overlap(mut self, overlap: f32) -> Self {
        self.overlap = overlap;
        self
    }

    /// Vary instance size.
    pub fn scale_variation(mut self, variation: f32) -> Self {
        self.scale_variation = variation;
        self
    }

    /// Vary instance rotation.
    pub fn rotation_variation(mut self, variation: f32) -> Self {
        self.rotation_variation = variation;
        self
    }

    /// Vary instance opacity.
    pub fn opacity_variation(mut self, variation: f32) -> Self {
        self.opacity_variation = variation;
        self
    }

    /// Vary the instances.
    pub fn seed(mut self, seed: u32) -> Self {
        self.seed = seed;
        self
    }
}

ports!(
    Tile,
    Output::SameAs("input"),
    "input": Any => input,
    "mask": Float => mask,
);

impl Check for Tile {
    fn check(&self, path: &str) -> Result<(), GraphError> {
        check_count(self.count[0], "count", path)?;
        check_count(self.count[1], "count", path)?;
        finite(self.overlap, &format!("{path}.overlap"), "overlap")?;
        for (field, value) in [
            ("scale_variation", self.scale_variation),
            ("rotation_variation", self.rotation_variation),
            ("opacity_variation", self.opacity_variation),
        ] {
            crate::nodes::generator::check_unit(value, field, path)?;
        }
        Ok(())
    }
}

/// Domain distortion: the source read at a coordinate the offset field moved.
///
/// A float offset displaces along both axes; a [`Uv`](super::Uv)-shaped
/// `Vec2` displaces each axis separately.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
#[must_use]
pub struct Warp {
    /// The field to read.
    pub source: Input,
    /// How far to move the reading coordinate, before `amount`.
    pub offset: Input,
    /// Scale of the displacement, in UV.
    pub amount: f32,
}

impl Default for Warp {
    fn default() -> Self {
        Self {
            source: Input::default(),
            offset: Input::default(),
            amount: 0.1,
        }
    }
}

impl Warp {
    /// Read a source through an offset field.
    pub fn new(source: impl Into<Input>, offset: impl Into<Input>) -> Self {
        Self {
            source: source.into(),
            offset: offset.into(),
            ..Self::default()
        }
    }

    /// Set the displacement scale in UV.
    pub fn amount(mut self, amount: f32) -> Self {
        self.amount = amount;
        self
    }
}

ports!(
    Warp,
    Output::SameAs("source"),
    "source": Any => source,
    "offset": Displacement => offset,
);

impl Check for Warp {
    fn check(&self, path: &str) -> Result<(), GraphError> {
        finite(self.amount, &format!("{path}.amount"), "amount")
    }
}

/// Domain distortion along an angle field: the source read at a coordinate
/// moved a fixed distance in a direction the field chooses.
///
/// [`Warp`] moves every texel by a *vector* the offset field carries, so the
/// displacement's length and direction come from one place and a field that is
/// bright in the middle bunches the source up there. This one takes the
/// direction alone and holds the distance fixed, which is a different tool: the
/// source is carried along a flow rather than squeezed by one, so lines stay
/// the width they were and bend instead of pinching. It is what draws a crack
/// that runs, a grain that follows a weld, or rain that falls the way the wind
/// was going.
///
/// The angle field is in **turns**, as [`MathOp::Sin`](super::MathOp::Sin) is:
/// a noise in `0..=1` reaching this sweeps the whole circle, which is what an
/// author reaching for a noise wants, and a quarter is a quarter turn without
/// anybody converting anything. [`Self::amount`] is the displacement in UV.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
#[must_use]
pub struct DirectionalWarp {
    /// The field to read.
    pub source: Input,
    /// Which way to move the reading coordinate, in turns.
    pub angle: Input,
    /// How far to move it, in UV.
    pub amount: f32,
}

impl Default for DirectionalWarp {
    fn default() -> Self {
        Self {
            source: Input::default(),
            angle: Input::default(),
            amount: 0.05,
        }
    }
}

impl DirectionalWarp {
    /// Read a source through an angle field.
    pub fn new(source: impl Into<Input>, angle: impl Into<Input>) -> Self {
        Self {
            source: source.into(),
            angle: angle.into(),
            ..Self::default()
        }
    }

    /// Set the displacement in UV.
    pub fn amount(mut self, amount: f32) -> Self {
        self.amount = amount;
        self
    }
}

ports!(
    DirectionalWarp,
    Output::SameAs("source"),
    "source": Any => source,
    "angle": Float => angle,
);

impl Check for DirectionalWarp {
    fn check(&self, path: &str) -> Result<(), GraphError> {
        finite(self.amount, &format!("{path}.amount"), "amount")
    }
}

/// Displace along the unnormalised gradient of a scalar guide.
///
/// `out(p) = source(fract(p + amount * gradient(height, p)))`.
/// Central differences use an explicit UV span, independent of bake resolution.
/// A constant guide does not move the source. Guide amplitude controls movement.
/// This is the gradient-driven contract used by Designer-style warping; its
/// amount is in UV squared per guide unit, not Designer's intensity units.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
#[must_use]
pub struct GradientWarp {
    /// Field to read.
    pub source: Input,
    /// Scalar displacement guide.
    pub height: Input,
    /// Gradient multiplier; negative values reverse displacement.
    pub amount: f32,
    /// Half-width of the central difference in UV units.
    pub sample_distance: f32,
}
impl Default for GradientWarp {
    fn default() -> Self {
        Self {
            source: Input::default(),
            height: Input::default(),
            amount: 0.001,
            sample_distance: 1.0 / 1024.0,
        }
    }
}
impl GradientWarp {
    /// Warp a source using a scalar guide's gradient.
    pub fn new(source: impl Into<Input>, height: impl Into<Input>) -> Self {
        Self {
            source: source.into(),
            height: height.into(),
            ..Self::default()
        }
    }
    /// Set the multiplier of the UV derivative.
    pub fn amount(mut self, amount: f32) -> Self {
        self.amount = amount;
        self
    }
    /// Set the half-width of the central difference, in UV.
    pub fn sample_distance(mut self, distance: f32) -> Self {
        self.sample_distance = distance;
        self
    }
}
ports!(GradientWarp, Output::SameAs("source"), "source": Any => source, "height": Float => height);
impl Check for GradientWarp {
    fn check(&self, path: &str) -> Result<(), GraphError> {
        finite(self.amount, &format!("{path}.amount"), "amount")?;
        require(
            self.sample_distance.is_finite()
                && self.sample_distance > 0.0
                && self.sample_distance <= 0.25,
            &format!("{path}.sample_distance"),
            "sample distance must be finite and in (0, 0.25]",
        )
    }
}
impl Lower for GradientWarp {
    fn lower(&self, cx: &mut Lowering, inputs: &NodeInputs) -> ValueId {
        let uv = cx.uv();
        let guide = inputs.value("height");
        let mut derivatives = Vec::with_capacity(2);
        for offset in [[self.sample_distance, 0.0], [0.0, self.sample_distance]] {
            let delta = k2(cx, offset[0], offset[1]);
            let plus = add(cx, uv, delta);
            let plus = fract(cx, plus);
            let minus = sub(cx, uv, delta);
            let minus = fract(cx, minus);
            let high = cx.substitute(guide, plus);
            let low = cx.substitute(guide, minus);
            let difference = sub(cx, high, low);
            let gain = k(cx, self.amount / (2.0 * self.sample_distance));
            derivatives.push(mul(cx, difference, gain));
        }
        let delta = vec2(cx, derivatives[0], derivatives[1]);
        let moved = add(cx, uv, delta);
        let frame = fract(cx, moved);
        cx.substitute(inputs.value("source"), frame)
    }
}

/// Displace in one fixed direction by a scalar intensity field.
/// Unlike [`DirectionalWarp`], the field controls distance, not angle.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
#[must_use]
pub struct IntensityWarp {
    /// Field to read.
    pub source: Input,
    /// Multiplier of displacement at each coordinate.
    pub intensity: Input,
    /// Fixed direction in turns.
    pub angle: f32,
    /// UV displacement for an intensity of one.
    pub amount: f32,
}
impl Default for IntensityWarp {
    fn default() -> Self {
        Self {
            source: Input::default(),
            intensity: Input::default(),
            angle: 0.0,
            amount: 0.05,
        }
    }
}
impl IntensityWarp {
    /// Read the source along a fixed direction with varying displacement.
    pub fn new(source: impl Into<Input>, intensity: impl Into<Input>) -> Self {
        Self {
            source: source.into(),
            intensity: intensity.into(),
            ..Self::default()
        }
    }
    /// Set the direction in turns.
    pub fn angle(mut self, angle: f32) -> Self {
        self.angle = angle;
        self
    }
    /// Set the displacement for an intensity of one, in UV.
    pub fn amount(mut self, amount: f32) -> Self {
        self.amount = amount;
        self
    }
}
ports!(IntensityWarp, Output::SameAs("source"), "source": Any => source, "intensity": Float => intensity);
impl Check for IntensityWarp {
    fn check(&self, path: &str) -> Result<(), GraphError> {
        finite(self.angle, &format!("{path}.angle"), "angle")?;
        finite(self.amount, &format!("{path}.amount"), "amount")
    }
}
impl Lower for IntensityWarp {
    fn lower(&self, cx: &mut Lowering, inputs: &NodeInputs) -> ValueId {
        let uv = cx.uv();
        let theta = self.angle * TURN;
        let direction = k2(cx, theta.cos() * self.amount, theta.sin() * self.amount);
        let delta = mul(cx, direction, inputs.value("intensity"));
        let moved = add(cx, uv, delta);
        let frame = fract(cx, moved);
        cx.substitute(inputs.value("source"), frame)
    }
}

/// Which axis a [`Mirror`] folds.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub enum MirrorAxis {
    /// Fold in u.
    #[default]
    U,
    /// Fold in v.
    V,
}

/// Reflects the field about the middle of the UV unit, in one axis.
///
/// The fold is over the whole unit rather than over the lattice, which is what
/// makes it safe on any source. For the u axis:
///
/// ```text
/// out(u, v) = in(f(u), v),  f(u) = 0.5 - |0.5 - fract(u)|
/// ```
///
/// `f` is continuous, runs from 0 up to 0.5 and back down, and repeats once, so
/// the result meets itself at the u seam whatever it folded. It reads the first
/// half of its source and writes the second half as that half reversed, which
/// is what a mirror is for: the symmetry an author asked for, and a seam that
/// matches because both sides of it are the same texel.
///
/// The lattice survives the fold. A source of eight cells along u lands as
/// eight cells along u, four of them the reflections of the other four, so the
/// folded axis keeps the period that reached it and the other axis is
/// untouched. A fold that paired each repeat with the next one would halve the
/// count instead, but it meets itself only where the source's cells are copies
/// of one another — brickwork, not a noise, as [`Period`](crate::Period)
/// describes — so this node does not fold that way.
///
/// A mirror of a [`Period::Free`](crate::Period::Free) field is free even
/// though the axis it folded now wraps, because the axis it left alone still
/// does not and a period is free in both axes or neither.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
#[must_use]
pub struct Mirror {
    /// The field to fold.
    pub input: Input,
    /// Which axis to fold.
    pub axis: MirrorAxis,
}

impl Mirror {
    /// Fold a field in u.
    pub fn new(input: impl Into<Input>) -> Self {
        Self {
            input: input.into(),
            axis: MirrorAxis::U,
        }
    }

    /// Choose the axis to fold.
    pub fn axis(mut self, axis: MirrorAxis) -> Self {
        self.axis = axis;
        self
    }
}

ports!(Mirror, Output::SameAs("input"), "input": Any => input);
impl Check for Mirror {}

/// Folds the repeat into `count` sectors about its centre.
///
/// Only the four-sector fold keeps a period, and, as with [`Mirror`], it is
/// written down here because the period rule rests on it. The four sectors are
/// the quadrants of the repeat, reflected into one another about its centre
/// lines:
///
/// ```text
/// out(u, v) = in(g(u), g(v)),  g(t) = 1 - |1 - 2 * fract(t)|
/// ```
///
/// `g` runs from 0 to 1 and back again, so the result reads the same value at
/// `u = 1` as at `u = 0` whatever the source's period — a source that does not
/// tile at all included — and the fold therefore tiles once in each axis rather
/// than inheriting anything.
///
/// This is the quadrant fold, and not the four-sector case of the angular one:
/// any other count folds about the centre into wedges whose boundaries are
/// diagonals, which do not line up with the edges of the repeat, so the fold
/// and the wrap disagree there and the result is
/// [`Period::Free`](crate::Period::Free). The two are different constructions
/// that this node carries under one field, as the period rule already says; a
/// lowering writes them separately.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
#[must_use]
pub struct Kaleidoscope {
    /// The field to fold.
    pub input: Input,
    /// How many sectors.
    pub count: u32,
}

impl Default for Kaleidoscope {
    fn default() -> Self {
        Self {
            input: Input::default(),
            count: 4,
        }
    }
}

impl Kaleidoscope {
    /// Fold a field into four sectors.
    pub fn new(input: impl Into<Input>) -> Self {
        Self {
            input: input.into(),
            ..Self::default()
        }
    }

    /// Set the number of sectors.
    pub fn count(mut self, count: u32) -> Self {
        self.count = count;
        self
    }
}

ports!(Kaleidoscope, Output::SameAs("input"), "input": Any => input);

impl Check for Kaleidoscope {
    fn check(&self, path: &str) -> Result<(), GraphError> {
        require(
            self.count >= 1,
            &format!("{path}.count"),
            "a kaleidoscope needs at least one sector",
        )
    }
}

/// Reads a source round a disc: the source's u becomes the angle, its v the
/// radius.
///
/// This is the polar resampler, and it is a *windowed* one because a polar
/// frame over the whole repeat cannot tile. A field read in polar coordinates
/// answers a different angle at `u = 0` than at `u = 1`, so it does not meet
/// itself at the seam and its period is [`Period::Free`](crate::Period::Free),
/// exactly as a non-quadrant [`Kaleidoscope`] is. What does tile is a disc no
/// wider than the repeat with a fill outside it, which is the rule
/// [`Shape`](super::Shape) already holds itself to; that is why
/// [`Self::radius`] stops at a half, why the coordinate is wrapped before it is
/// measured, and why everything outside the window answers [`Self::outside`]
/// rather than the source.
///
/// At a texel whose polar coordinates about the centre of the repeat are
/// `(r, theta)`, with `n = (r - inner) / (radius - inner)` the normalised
/// radius across the window:
///
/// ```text
/// out(p) = in(fract(theta / tau * turns + twist * n), n * rings)
/// ```
///
/// So [`Self::turns`] is how many copies of the source go round the ring,
/// [`Self::rings`] how many go from the hole out to the rim, and
/// [`Self::twist`] how far round the source is carried per unit of radius, in
/// turns: a twist of one drags a spoke a whole turn between the two radii,
/// which is what draws a swirl, a screw slot or a brushed dial. The angle is
/// wrapped and the radius is not, because the angle closes on itself and the
/// radius runs out to an edge the window already stops at.
///
/// **The centre pinches.** Every angle meets at `r = inner`, so the whole
/// `v = 0` row of the source is drawn at that one point when `inner` is zero.
/// A source whose `v = 0` row is constant — a gradient, a ramp, a ring of
/// stripes that closes — lands cleanly there; a noise does not, and shows a
/// knot at the middle. So a rivet head reads a gradient at the centre and a
/// noise only further out, and a source that has to be noisy all the way in
/// wants an `inner` above zero, which replaces the point with a hole.
///
/// **A radius of exactly a half touches the seam**, which the default is: the
/// rim is then tangent to each of the repeat's four edges at their midpoints,
/// and the angle is half a turn on one side of that touch and none on the
/// other. The field is still exactly periodic — the coordinate is wrapped
/// before it is measured, so the texel at `u = 1` is the texel at `u = 0` — but
/// the texels either side of the touch read opposite ends of the source's u,
/// so a sliver against the seam a texel wide carries a jump rather than a
/// gradient. It is a hairline at any resolution a map is baked at; a radius a
/// little under a half has no such point at all, because the fill then stands
/// between the rim and the seam.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
#[must_use]
pub struct CircleMap {
    /// The field to read round the disc.
    pub input: Input,
    /// The outer radius of the window, in UV. At most a half, so that the disc
    /// sits inside the repeat it is centred in.
    pub radius: f32,
    /// The inner radius of the window, in UV. Above zero it is a hole rather
    /// than a point, which is what unpinches the centre.
    pub inner: f32,
    /// Copies of the source round the ring.
    pub turns: u32,
    /// Copies of the source between the two radii.
    pub rings: u32,
    /// Turns of angle the source is carried per unit of normalised radius.
    pub twist: f32,
    /// What the field answers outside the window.
    pub outside: f32,
}

impl Default for CircleMap {
    fn default() -> Self {
        Self {
            input: Input::default(),
            radius: 0.5,
            inner: 0.0,
            turns: 1,
            rings: 1,
            twist: 0.0,
            outside: 0.0,
        }
    }
}

impl CircleMap {
    /// Read a field round a disc that fills the repeat.
    pub fn new(input: impl Into<Input>) -> Self {
        Self {
            input: input.into(),
            ..Self::default()
        }
    }

    /// Set the outer radius of the window.
    pub fn radius(mut self, radius: f32) -> Self {
        self.radius = radius;
        self
    }

    /// Set the inner radius, which opens a hole at the centre.
    pub fn inner(mut self, inner: f32) -> Self {
        self.inner = inner;
        self
    }

    /// Set how many copies of the source go round the ring.
    pub fn turns(mut self, turns: u32) -> Self {
        self.turns = turns;
        self
    }

    /// Set how many copies of the source go from the hole to the rim.
    pub fn rings(mut self, rings: u32) -> Self {
        self.rings = rings;
        self
    }

    /// Carry the source round by this many turns per unit of radius.
    pub fn twist(mut self, twist: f32) -> Self {
        self.twist = twist;
        self
    }

    /// Set what the field answers outside the window.
    pub fn outside(mut self, outside: f32) -> Self {
        self.outside = outside;
        self
    }
}

ports!(CircleMap, Output::SameAs("input"), "input": Any => input);

impl Check for CircleMap {
    fn check(&self, path: &str) -> Result<(), GraphError> {
        check_unit(self.radius, "radius", path)?;
        check_unit(self.inner, "inner", path)?;
        finite(self.twist, &format!("{path}.twist"), "twist")?;
        finite(self.outside, &format!("{path}.outside"), "outside")?;
        check_count(self.turns, "turns", path)?;
        check_count(self.rings, "rings", path)?;
        require(
            self.inner < self.radius,
            &format!("{path}.inner"),
            &format!(
                "the window runs from `inner` out to `radius`, so an inner radius of {} is not \
                 inside an outer one of {}; a disc with no width has nowhere to read the source",
                self.inner, self.radius
            ),
        )?;
        // The same claim a shape's own check makes, for the same reason: the
        // disc is centred in the repeat, its period is one, and a disc that
        // reached past the repeat would be cut at the seam and would not meet
        // what is on the other side of it.
        require(
            self.radius <= 0.5,
            &format!("{path}.radius"),
            &format!(
                "a circle map of radius {} reaches past the repeat it is centred in, so it would \
                 be cut at the seam; the radius must be at most a half, and a disc that should \
                 run off the edge is a Transform of a smaller one",
                self.radius
            ),
        )
    }

    fn check_types(&self, inputs: &[Resolved], path: &str) -> Result<(), GraphError> {
        // The angular axis wraps into the source's u, so the source has to meet
        // itself there or the disc shows a radial seam at angle zero. It is the
        // same requirement a triplanar makes of its source, and for the same
        // reason: the node addresses the source by something other than UV, so
        // the source's own tiling is the only thing that closes the frame.
        let tiles = inputs
            .iter()
            .find(|input| input.name == "input")
            .is_none_or(|input| input.period.is_tiled());
        require(
            tiles,
            &format!("{path}.inputs[input]"),
            "a circle map wraps the source's u axis round the ring, so the source meets itself \
             every turn or it does not meet itself at all; this one does not tile",
        )
    }
}

/// Scatters a source round one or more rings inside the repeat: the bolt
/// circle, the ring of rivets, the ticks of a dial, the holes of a drain.
///
/// [`Tile`] lays its instances on a grid and this one lays them on a circle;
/// everything else about the two is the same tool. An instance is the source's
/// own unit shrunk to [`Self::scale`], varied by hashes of its own place in the
/// ring, drawn only where the coordinate it was read at lands inside that unit,
/// and combined with its neighbours by taking the larger.
///
/// What is different is the neighbourhood, and it is the node's whole bill. A
/// texel reads **three instances per ring** — the one nearest it in angle and
/// the one either side of it — because the instances sit on a circle and only
/// those two can reach across the gap to it. So the source is evaluated
/// `3 * rings` times, and a splatter of two rings costs six of whatever it
/// scattered. The limit that comes with reading three is honest and worth
/// stating: an instance wider than twice the gap between two of them is clipped
/// rather than drawn, so [`Self::scale`] wants to stay under the spacing
/// [`Self::count`] leaves.
///
/// The rings stand at the outer edge of the bands they cut [`Self::inner`] to
/// [`Self::radius`] into, so the outermost is at `radius` whatever `rings` is,
/// and the default single ring is the ring at `radius`. That is what lets one
/// check keep every instance inside the repeat: the outermost ring plus the
/// reach of an instance standing on it is at most a half, which is the rule a
/// [`Shape`](super::Shape) and a [`CircleMap`] live under and the reason this
/// node tiles once whatever it scattered.
///
/// How far an instance reaches is not one number, because what is drawn is the
/// source's unit *square* rather than a disc. Square to the repeat it reaches
/// half its width; turned, it presents a corner instead, at its width over the
/// root of two. Both [`Self::face_centre`] and [`Self::rotation_variation`]
/// turn it, and either of them makes the check ask for the longer reach — so a
/// ring that fits square does not always fit facing, and the node says so at
/// `radius` rather than letting the seam take the corner off.
///
/// Every variation of an instance's size or place takes away rather than adds
/// — a smaller instance, a radius pulled inward, an opacity below one — for the
/// reason [`Tile`]'s does: the check above is made once, against the fields as
/// they are written, and a variation that could grow an instance past them
/// would make it a bound on nothing. A rotation takes nothing away and cannot
/// be read that way, which is why it is the one variation the bound itself has
/// to know about.
///
/// [`Self::mask`] is what turns a ring into a distribution, and it reads the
/// way [`Tile`]'s reads: once per instance, at that instance's own centre, and
/// against a hash of that instance, so a mask of a half keeps about half the
/// bolts at full strength rather than dimming all of them. Bolts that thin out
/// read as bolts missing; bolts that fade read as a mistake.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
#[must_use]
pub struct CircleSplatter {
    /// The field to scatter.
    pub input: Input,
    /// Where instances appear, read once per instance at its own centre and
    /// resolved against that instance's own hash. One everywhere by default,
    /// which keeps every instance and emits nothing.
    pub mask: Input,
    /// Instances round each ring.
    pub count: u32,
    /// Concentric rings between [`Self::inner`] and [`Self::radius`].
    pub rings: u32,
    /// The radius of the outermost ring, in UV. What an instance reaches past
    /// it — half a width square to the repeat, a corner once it is turned —
    /// still has to fit inside the repeat.
    pub radius: f32,
    /// Where the innermost band starts, in UV. The first ring stands one band
    /// out from it, so this is a standoff rather than a ring of its own.
    pub inner: f32,
    /// The width of one instance, in UV: the source's whole unit is drawn this
    /// far across.
    pub scale: f32,
    /// How much an instance's size may shrink, in `0..=1`.
    pub scale_variation: f32,
    /// How much an instance's rotation may vary, in `0..=1` of a half turn.
    /// Any spin at all turns the instance's square, so the seam check measures
    /// it by its corner from here on rather than by half its width.
    pub rotation_variation: f32,
    /// How far an instance may be pulled in towards the centre, in `0..=1` of
    /// the spacing between two rings.
    pub radius_variation: f32,
    /// How much an instance's opacity may vary, in `0..=1`.
    pub opacity_variation: f32,
    /// Whether to turn each instance to face the centre: its u axis then runs
    /// outward along the radius and its v round the ring, so a source drawn as
    /// a tooth or a slot lines up with the circle instead of with the repeat.
    /// It turns the instance's square as well, so the seam check measures a
    /// facing instance by its corner, as it does a spun one.
    pub face_centre: bool,
    /// Varies the instances.
    pub seed: u32,
}

impl Default for CircleSplatter {
    fn default() -> Self {
        Self {
            input: Input::default(),
            mask: Input::float(1.0),
            count: 8,
            rings: 1,
            radius: 0.35,
            inner: 0.0,
            scale: 0.2,
            scale_variation: 0.0,
            rotation_variation: 0.0,
            radius_variation: 0.0,
            opacity_variation: 0.0,
            face_centre: false,
            seed: 0,
        }
    }
}

impl CircleSplatter {
    /// Scatter a field round a ring inside the repeat.
    pub fn new(input: impl Into<Input>) -> Self {
        Self {
            input: input.into(),
            ..Self::default()
        }
    }

    /// Draw an instance only where a field is at or above its own hash.
    pub fn mask(mut self, mask: impl Into<Input>) -> Self {
        self.mask = mask.into();
        self
    }

    /// Set how many instances go round each ring.
    pub fn count(mut self, count: u32) -> Self {
        self.count = count;
        self
    }

    /// Set how many concentric rings there are.
    pub fn rings(mut self, rings: u32) -> Self {
        self.rings = rings;
        self
    }

    /// Set the radius of the outermost ring.
    pub fn radius(mut self, radius: f32) -> Self {
        self.radius = radius;
        self
    }

    /// Set where the innermost band starts.
    pub fn inner(mut self, inner: f32) -> Self {
        self.inner = inner;
        self
    }

    /// Set the width of one instance, in UV.
    pub fn scale(mut self, scale: f32) -> Self {
        self.scale = scale;
        self
    }

    /// Vary instance size.
    pub fn scale_variation(mut self, variation: f32) -> Self {
        self.scale_variation = variation;
        self
    }

    /// Vary instance rotation.
    pub fn rotation_variation(mut self, variation: f32) -> Self {
        self.rotation_variation = variation;
        self
    }

    /// Vary how far out an instance stands.
    pub fn radius_variation(mut self, variation: f32) -> Self {
        self.radius_variation = variation;
        self
    }

    /// Vary instance opacity.
    pub fn opacity_variation(mut self, variation: f32) -> Self {
        self.opacity_variation = variation;
        self
    }

    /// Turn each instance to face the centre.
    pub fn face_centre(mut self) -> Self {
        self.face_centre = true;
        self
    }

    /// Vary the instances.
    pub fn seed(mut self, seed: u32) -> Self {
        self.seed = seed;
        self
    }

    /// How far apart two rings stand, which is the band a radius variation may
    /// pull an instance back across.
    ///
    /// The rings divide `inner..=radius` into `rings` equal bands and sit at
    /// the outer edge of each, so this is also how far the innermost ring
    /// stands off `inner`.
    fn spacing(&self) -> f32 {
        (self.radius - self.inner) / count(self.rings.max(1))
    }

    /// The radius of one ring, counted outward from [`Self::inner`].
    fn standoff(&self, ring: u32) -> f32 {
        self.inner + self.spacing() * (count(ring) + 1.0)
    }
}

ports!(
    CircleSplatter,
    Output::SameAs("input"),
    "input": Any => input,
    "mask": Float => mask,
);

impl Check for CircleSplatter {
    fn check(&self, path: &str) -> Result<(), GraphError> {
        check_count(self.count, "count", path)?;
        check_count(self.rings, "rings", path)?;
        check_unit(self.radius, "radius", path)?;
        check_unit(self.inner, "inner", path)?;
        check_unit(self.scale, "scale", path)?;
        for (field, value) in [
            ("scale_variation", self.scale_variation),
            ("rotation_variation", self.rotation_variation),
            ("radius_variation", self.radius_variation),
            ("opacity_variation", self.opacity_variation),
        ] {
            check_unit(value, field, path)?;
        }
        require(
            self.scale > 0.0,
            &format!("{path}.scale"),
            "an instance of no width is not a scatter; `scale` is how far across the repeat one \
             copy of the source is drawn, and it must be above zero",
        )?;
        require(
            self.inner < self.radius,
            &format!("{path}.inner"),
            &format!(
                "the rings are laid from `inner` out to `radius`, so an inner standoff of {} \
                 leaves nowhere for them inside an outer radius of {}",
                self.inner, self.radius
            ),
        )?;
        // The same claim a shape's own check makes, for the same reason: the
        // ring is centred in the repeat, its period is one, and an instance
        // that reached past the repeat would be cut at the seam and would not
        // meet what is on the other side of it. The bound is written with the
        // size variation *adding* to an instance, which is the conservative
        // reading of a variation that in fact only takes away: it is the width
        // a reader of the two fields expects an instance to reach, and a ring
        // that only just fits is one edit away from a seam.
        //
        // What an instance reaches from its own centre is half its width while
        // it stands square to the repeat and its half-diagonal once it can
        // turn, because the unit drawn is a square and a turned square presents
        // a corner. Facing the centre turns every instance and a rotation
        // variation turns them by their own hashes, so either field asks for
        // the longer reach whatever angle the turn comes out at.
        let turned = self.face_centre || self.rotation_variation > 0.0;
        let half = if turned {
            std::f32::consts::FRAC_1_SQRT_2
        } else {
            0.5
        };
        let reach = self.radius + self.scale * (1.0 + self.scale_variation) * half;
        let bound = if turned {
            "`radius` plus an instance's corner — its width over the root of two, which is what a \
             turned instance reaches — must be at most a half, which means a smaller `radius`, a \
             smaller `scale`, or an instance left square to the repeat"
        } else {
            "`radius` plus half an instance must be at most a half, which means a smaller \
             `radius`, a smaller `scale` or both"
        };
        require(
            reach <= 0.5,
            &format!("{path}.radius"),
            &format!(
                "an instance of this ring reaches {reach} from the centre of the repeat it is \
                 laid in, so it would be cut at the seam; {bound}"
            ),
        )
    }
}

/// How far off a whole turn a rotation may be and still count as none.
///
/// The same slop period inference reads a transform's identity with, so the
/// expression a lowering emits and the period it was checked against agree
/// about what "no rotation" means.
const ROTATION_SLOP: f32 = 1e-6;

/// A coordinate held inside the repeat, the way the node's `repeat` field asks
/// for.
///
/// A wrap is a `fract`, which is the identity on every generator in the crate —
/// each already reduces its own lattice modulo its period, so reading one at
/// `u = 1.7` and at `u = 0.7` is the same read — and is *not* the identity on a
/// field built out of a bare [`Uv`](super::Uv), which runs from zero to one and
/// jumps back. Doing it anyway is what makes a transform's period claim true
/// whatever it transformed.
///
/// A clamp is the other reading: the source is held at its edge outside the
/// unit, which is a frame that does not wrap and is why a clamped transform
/// that moves the coordinate at all is [`Period::Free`](crate::Period::Free).
fn held(cx: &mut Lowering, coordinate: ValueId, repeat: bool) -> ValueId {
    if repeat {
        return fract(cx, coordinate);
    }
    let zero = k(cx, 0.0);
    let one = k(cx, 1.0);
    clamp(cx, coordinate, zero, one)
}

/// Scale, then a turn about the centre of the repeat, then a translation —
/// the order [`Transform`]'s own documentation writes, because the period rule
/// rests on it.
impl Lower for Transform {
    fn lower(&self, cx: &mut Lowering, inputs: &NodeInputs) -> ValueId {
        let uv = cx.uv();
        let scale = k2(cx, self.scale[0], self.scale[1]);
        let scaled = mul(cx, uv, scale);
        let turned = if self.rotate.abs() < ROTATION_SLOP {
            scaled
        } else {
            let centre = k2(cx, 0.5, 0.5);
            let about = sub(cx, scaled, centre);
            let spun = turn(cx, about, self.rotate.to_radians());
            add(cx, spun, centre)
        };
        let shift = k2(cx, self.translate[0], self.translate[1]);
        let moved = add(cx, turned, shift);
        let frame = held(cx, moved, self.repeat);
        cx.substitute(inputs.value("input"), frame)
    }
}

/// The source read at a coordinate the offset field moved.
///
/// The offset is read at the texel's *own* coordinate and the source at the
/// moved one, which is what domain distortion is: `out(p) = in(p + a * d(p))`.
/// The moved coordinate is wrapped, so the result meets itself at the seam
/// whenever both the source and the offset do, which is the period rule.
impl Lower for Warp {
    fn lower(&self, cx: &mut Lowering, inputs: &NodeInputs) -> ValueId {
        let uv = cx.uv();
        let offset = inputs.value("offset");
        let amount = k(cx, self.amount);
        let displacement = mul(cx, offset, amount);
        let moved = add(cx, uv, displacement);
        let frame = fract(cx, moved);
        cx.substitute(inputs.value("source"), frame)
    }
}

/// The source read one fixed step along the direction the angle field names.
///
/// `out(p) = in(p + a * (cos(2*pi*t(p)), sin(2*pi*t(p))))`, with the angle read
/// at the texel's own coordinate and the source at the moved one, exactly as
/// [`Warp`] reads its offset. The moved coordinate is wrapped, so the result
/// meets itself at the seam whenever both the source and the angle field do,
/// which is the period rule a pointwise multiple gives it.
impl Lower for DirectionalWarp {
    fn lower(&self, cx: &mut Lowering, inputs: &NodeInputs) -> ValueId {
        let uv = cx.uv();
        let turns = inputs.value("angle");
        let full = k(cx, TURN);
        let radians = mul(cx, turns, full);
        let across = cos(cx, radians);
        let up = sin(cx, radians);
        let direction = vec2(cx, across, up);
        let amount = k(cx, self.amount);
        let displacement = mul(cx, direction, amount);
        let moved = add(cx, uv, displacement);
        let frame = fract(cx, moved);
        cx.substitute(inputs.value("source"), frame)
    }
}

/// The fold [`Mirror`] documents: `0.5 - |0.5 - fract(t)|` on one axis, and the
/// other axis untouched.
impl Lower for Mirror {
    fn lower(&self, cx: &mut Lowering, inputs: &NodeInputs) -> ValueId {
        let uv = cx.uv();
        let (u, v) = axes(cx, uv);
        let frame = match self.axis {
            MirrorAxis::U => {
                let folded = mirror_fold(cx, u);
                vec2(cx, folded, v)
            }
            MirrorAxis::V => {
                let folded = mirror_fold(cx, v);
                vec2(cx, u, folded)
            }
        };
        cx.substitute(inputs.value("input"), frame)
    }
}

/// Two folds under one field, as [`Kaleidoscope`] says.
///
/// Four sectors are the quadrants of the repeat, folded about its centre lines
/// with `1 - |1 - 2 * fract(t)|` on each axis: that runs from 0 to 1 and back
/// within the unit, so the result reads the same value at `u = 1` as at
/// `u = 0` whatever the source's period, and tiles once.
///
/// Any other count folds about the centre into wedges. The coordinate is taken
/// to polar about the middle of the repeat, the angle is folded into one
/// sector and mirrored within it, and the source is read back along the wedge
/// at angle zero. The wedge boundaries are diagonals that do not line up with
/// the edges of the repeat, which is why that fold is
/// [`Period::Free`](crate::Period::Free) and why it is written apart from the
/// quadrant one rather than as its general case.
impl Lower for Kaleidoscope {
    fn lower(&self, cx: &mut Lowering, inputs: &NodeInputs) -> ValueId {
        let uv = cx.uv();
        let frame = if self.count == 4 {
            let (u, v) = axes(cx, uv);
            let folded_u = quadrant_fold(cx, u);
            let folded_v = quadrant_fold(cx, v);
            vec2(cx, folded_u, folded_v)
        } else {
            let centre = k2(cx, 0.5, 0.5);
            let point = sub(cx, uv, centre);
            let (angle, radius) = polar(cx, point);
            let sector = TURN / count(self.count.max(1));
            let scale = k(cx, 1.0 / sector);
            let turns = mul(cx, angle, scale);
            // The same fold a mirror takes, which lands the angle in half a
            // sector and reflects alternate wedges into one another.
            let folded = mirror_fold(cx, turns);
            let back = k(cx, sector);
            let folded = mul(cx, folded, back);
            let cosine = cos(cx, folded);
            let sine = sin(cx, folded);
            let across = mul(cx, radius, cosine);
            let up = mul(cx, radius, sine);
            let wedge = vec2(cx, across, up);
            add(cx, wedge, centre)
        };
        cx.substitute(inputs.value("input"), frame)
    }
}

/// The windowed polar read [`CircleMap`] documents, written as the arithmetic
/// the interpreter runs.
///
/// The coordinate is wrapped before it is measured from the centre, which is
/// the whole of why this tiles: the disc at `u = 1.3` is the disc at `u = 0.3`,
/// and the node's own check keeps the disc inside the repeat so the wrap never
/// cuts it. Inside the window the source is read at the angle and the
/// normalised radius, through [`Lowering::substitute`] as every other resampler
/// reads its source; outside it the answer is the fill, chosen by a select
/// rather than blended, because a fill is a fill and an author who wants the
/// source to fade out at the rim has a ramp for that.
impl Lower for CircleMap {
    fn lower(&self, cx: &mut Lowering, inputs: &NodeInputs) -> ValueId {
        let uv = cx.uv();
        let point = about_centre(cx, uv);
        let (angle, radius) = polar(cx, point);

        // The normalised radius across the window, which becomes the source's
        // v axis. The check keeps the span above zero, so the divide is one.
        let inner = k(cx, self.inner);
        let across = sub(cx, radius, inner);
        let span = k(cx, self.radius - self.inner);
        let normalised = div(cx, across, span);

        // The angle in turns, `turns` copies of the source round the ring, plus
        // the twist the radius carries. The wrap is what closes the source's u
        // axis on itself at angle zero.
        let per_turn = k(cx, count(self.turns) / TURN);
        let around = mul(cx, angle, per_turn);
        let twist = k(cx, self.twist);
        let carried = mul(cx, normalised, twist);
        let along = add(cx, around, carried);
        let along = fract(cx, along);
        let rings = k(cx, count(self.rings));
        let outward = mul(cx, normalised, rings);
        let frame = vec2(cx, along, outward);
        let read = cx.substitute(inputs.value("input"), frame);

        // Inside the window is at or past the inner radius and at or before the
        // outer one. The fill is widened to whatever the source carries, so a
        // colour source is filled with a grey rather than with a red.
        let outer = k(cx, self.radius);
        let past_inner = step(cx, inner, radius);
        let before_outer = step(cx, radius, outer);
        let inside = mul(cx, past_inner, before_outer);
        let carried_type = cx.type_of(read);
        let fill = k(cx, self.outside);
        let fill = cx.convert(fill, carried_type);
        select(cx, inside, read, fill)
    }
}

/// The salt the second and later numbers of one instance are hashed with. The
/// first is [`generator::SALT`]'s own neighbour, so that a scatter and a noise
/// on the same lattice do not lay the same numbers.
///
/// Five of them, and the fifth is only ever reached by a
/// [`CircleSplatter`]'s standoff; a [`Tile`] draws from the first four. Which
/// number is which is named below rather than written as an index at the call.
const INSTANCE_SALT: [u32; 5] = [
    0x27D4_EB2D,
    0x1656_67B1,
    0x9E37_79B9,
    0x85EB_CA6B,
    0xC2B2_AE35,
];

/// Scatter a source across the repeat, one instance per cell.
///
/// The repeat is cut into `count` cells and each carries one instance of the
/// source: the source's own unit, shrunk into the cell, varied by the cell's
/// own hashes. An instance is drawn only where the coordinate it was read at
/// lands inside the source's unit, because the source is a field that repeats
/// and would otherwise be drawn everywhere rather than in its cell; that window
/// is what makes a scatter a scatter.
///
/// The instances are combined by taking the larger, which is what a scatter of
/// masks, pebbles and dirt wants. `overlap` widens each instance past its own
/// cell, and it is the one field that costs: while it is zero a texel reads its
/// own cell alone, and past zero it reads the nine around it, so the source is
/// evaluated nine times per texel. The cost is the honest one — an instance
/// that crosses a cell boundary has to be drawn from both sides of it — and it
/// is why `overlap` defaults to nothing.
///
/// Every instance wraps at the edge of the repeat, because the cells wrap, so
/// the result tiles once whatever the source did.
impl Lower for Tile {
    fn lower(&self, cx: &mut Lowering, inputs: &NodeInputs) -> ValueId {
        let source = inputs.value("input");
        // A mask that keeps everything is emitted as nothing at all: the gate
        // below is a hash and a step per instance, and a scatter that was never
        // masked should not pay for one because the port exists.
        let mask = (!keeps_everything(&self.mask)).then(|| inputs.value("mask"));
        let repeats = k2(cx, count(self.count[0]), count(self.count[1]));
        let uv = cx.uv();
        let coordinate = mul(cx, uv, repeats);
        let base = floor(cx, coordinate);
        let inside = sub(cx, coordinate, base);
        let reach = i32::from(self.overlap > 0.0);
        let mut brightest = k(cx, 0.0);
        for down in -reach..=reach {
            for across in -reach..=reach {
                let step_to = k2(cx, neighbour(across), neighbour(down));
                let cell = add(cx, base, step_to);
                let local = sub(cx, inside, step_to);
                let drawn = instance(cx, self, source, mask, cell, local, repeats);
                brightest = max(cx, brightest, drawn);
            }
        }
        brightest
    }
}

/// Whether a mask input keeps every instance, and so needs no gate at all.
///
/// A literal at or above one, which is the default and what a graph that never
/// touched the port carries. Anything else — a parameter, a node, a literal
/// below one — is a gate, even where a fold would have answered the same thing,
/// because this is read off the authored input rather than the lowered value.
fn keeps_everything(mask: &Input) -> bool {
    matches!(mask, Input::Const(crate::Value::Float(value)) if *value >= 1.0)
}

/// One instance of a scattered source, read from the cell it belongs to.
fn instance(
    cx: &mut Lowering,
    tile: &Tile,
    source: ValueId,
    mask: Option<ValueId>,
    cell: ValueId,
    local: ValueId,
    repeats: ValueId,
) -> ValueId {
    // The instance's own size, rotation and opacity, each a hash of its cell.
    let width = cell_hash(
        cx,
        cell,
        repeats,
        tile.seed.wrapping_add(INSTANCE_SALT[SIZE_SALT]),
    );
    let spin = signed_hash(
        cx,
        cell,
        repeats,
        tile.seed.wrapping_add(INSTANCE_SALT[SPIN_SALT]),
    );
    let fade = cell_hash(
        cx,
        cell,
        repeats,
        tile.seed.wrapping_add(INSTANCE_SALT[FADE_SALT]),
    );

    // A variation takes size away rather than adding it, so that an instance
    // never outgrows the overlap the node declared and the neighbourhood that
    // was searched for it stays the right one.
    let shrink = k(cx, tile.scale_variation);
    let taken = mul(cx, width, shrink);
    let size = complement(cx, taken);
    let grown = k(cx, 1.0 + tile.overlap);
    let size = mul(cx, size, grown);

    let centre = k2(cx, 0.5, 0.5);
    let about = sub(cx, local, centre);
    let about = if tile.rotation_variation > 0.0 {
        // Half a turn either way at full variation, which is as far as a
        // rotation can go before it meets itself.
        let spread = k(cx, tile.rotation_variation * TURN * 0.5);
        let angle = mul(cx, spin, spread);
        turn_by(cx, about, angle)
    } else {
        about
    };
    let scaled = div(cx, about, size);
    let read = add(cx, scaled, centre);

    // Outside the source's own unit there is no instance, whatever the source
    // would answer there.
    let (x, y) = axes(cx, read);
    let zero = k(cx, 0.0);
    let one = k(cx, 1.0);
    let mut window = None;
    for axis in [x, y] {
        let after = step(cx, zero, axis);
        let before = step(cx, axis, one);
        let band = mul(cx, after, before);
        window = Some(match window {
            Some(previous) => mul(cx, previous, band),
            None => band,
        });
    }
    let window = window.unwrap_or(one);

    let drawn = cx.substitute(source, read);
    let variation = k(cx, tile.opacity_variation);
    let taken = mul(cx, fade, variation);
    let opacity = complement(cx, taken);
    let masked = mul(cx, drawn, window);
    let faded = mul(cx, masked, opacity);
    let Some(mask) = mask else {
        return faded;
    };
    // The mask at the centre of this instance's own cell, against a number of
    // the instance's own: an instance is either drawn or it is not, and which
    // it is stays the same wherever in the cell the texel doing the asking is.
    // The cells wrap, so the centre a texel at `u = 1` computes is a whole UV
    // unit along from the one at `u = 0` — the same value of any field that
    // tiles, which is what keeps a masked scatter tiling and why a mask that
    // does not tile makes the whole node free.
    let half = k2(cx, 0.5, 0.5);
    let middle = add(cx, cell, half);
    let centre = div(cx, middle, repeats);
    let here = cx.substitute(mask, centre);
    let gate = cell_hash(
        cx,
        cell,
        repeats,
        tile.seed.wrapping_add(INSTANCE_SALT[GATE_SALT]),
    );
    let keep = step(cx, gate, here);
    mul(cx, faded, keep)
}

/// A neighbouring cell's offset as the float a coordinate is moved by.
#[expect(
    clippy::cast_precision_loss,
    reason = "a neighbourhood offset is one of minus one, zero and one"
)]
fn neighbour(value: i32) -> f32 {
    value as f32
}

/// Which of [`INSTANCE_SALT`]'s numbers each per-instance hash takes.
///
/// Named rather than indexed at the call, because a [`Tile`] and a
/// [`CircleSplatter`] draw from the same four and a splatter adds a fifth: two
/// nodes that disagreed about which number was the size and which the gate
/// would be two nodes whose instances correlate for no reason an author could
/// see.
const SIZE_SALT: usize = 0;
/// The number an instance's rotation is taken from.
const SPIN_SALT: usize = 1;
/// The number an instance's opacity is taken from.
const FADE_SALT: usize = 2;
/// The number an instance's mask threshold is taken from.
const GATE_SALT: usize = 3;
/// The number an instance's standoff is taken from, which only a splatter has.
const RADIUS_SALT: usize = 4;

/// Scatter a source round the rings [`CircleSplatter`] lays, three instances
/// per ring per texel.
///
/// The coordinate is wrapped before the ring is measured, which is the whole of
/// why this tiles: the ring at `u = 1.3` is the ring at `u = 0.3`, and the
/// node's own check keeps every instance inside the repeat so the wrap never
/// cuts one. The texel's own angle names the slot it is nearest to; that
/// instance and the one either side of it are drawn, and the larger of the
/// three wins, as a [`Tile`]'s nine do. Nothing further round the ring can
/// reach the texel unless an instance is wider than two slots, which is the
/// limit the node's own documentation states rather than one the arithmetic
/// hides.
impl Lower for CircleSplatter {
    fn lower(&self, cx: &mut Lowering, inputs: &NodeInputs) -> ValueId {
        let uv = cx.uv();
        let point = about_centre(cx, uv);
        let splat = Splat {
            node: self,
            source: inputs.value("input"),
            // A mask that keeps everything is emitted as nothing at all, for
            // the reason a scatter's is: the gate is a hash and a step per
            // instance, and this node would pay for one three times per ring.
            mask: (!keeps_everything(&self.mask)).then(|| inputs.value("mask")),
            point,
            repeats: k2(cx, count(self.count), count(self.rings)),
        };

        // Which instance the texel is nearest to in angle. The instances sit at
        // whole slots of `TURN / count`, so the angle counted in slots and
        // rounded names one, and `floor(t + 0.5)` is that rounding written as
        // the arithmetic the interpreter runs. A texel past the last slot
        // rounds to `count`, which is slot zero a whole turn on — the same
        // instance, and the hash wraps there for the same reason a scatter's
        // cells do.
        let (angle, _) = polar(cx, point);
        let per_slot = k(cx, count(self.count) / TURN);
        let along = mul(cx, angle, per_slot);
        let half = k(cx, 0.5);
        let rounded = add(cx, along, half);
        let nearest = floor(cx, rounded);

        let mut brightest = k(cx, 0.0);
        for ring in 0..self.rings {
            for offset in -1..=1 {
                let slot = if offset == 0 {
                    nearest
                } else {
                    let round = k(cx, neighbour(offset));
                    add(cx, nearest, round)
                };
                let drawn = splat.draw(cx, slot, ring);
                brightest = max(cx, brightest, drawn);
            }
        }
        brightest
    }
}

/// One instance of a [`CircleSplatter`], and everything about the frame it is
/// drawn in that every instance shares.
///
/// A struct rather than a function of nine arguments: the source, the mask, the
/// wrapped coordinate and the hash lattice are the same for all `3 * rings` of
/// them, and only the slot and the ring change.
struct Splat<'a> {
    node: &'a CircleSplatter,
    source: ValueId,
    mask: Option<ValueId>,
    /// The texel as a point about the centre of the repeat, already wrapped.
    point: ValueId,
    /// The lattice the per-instance hashes wrap at: the slots round a ring, and
    /// the rings themselves.
    repeats: ValueId,
}

impl Splat<'_> {
    /// Draw the instance in one slot of one ring.
    fn draw(&self, cx: &mut Lowering, slot: ValueId, ring: u32) -> ValueId {
        let node = self.node;
        let index = k(cx, count(ring));
        let cell = vec2(cx, slot, index);

        // Where the instance stands: the angle its slot names, and its ring's
        // radius less whatever the radius variation pulls it back by. The pull
        // never crosses a whole band, so an instance stays between the ring
        // inside it and its own and the check made against `radius` holds.
        let per_slot = k(cx, TURN / count(node.count.max(1)));
        let angle = mul(cx, slot, per_slot);
        let cosine = cos(cx, angle);
        let sine = sin(cx, angle);
        let direction = vec2(cx, cosine, sine);
        let mut standoff = k(cx, node.standoff(ring));
        if node.radius_variation > 0.0 {
            let hash = self.hash(cx, cell, RADIUS_SALT);
            let band = k(cx, node.radius_variation * node.spacing());
            let taken = mul(cx, hash, band);
            standoff = sub(cx, standoff, taken);
        }
        let centre = mul(cx, direction, standoff);

        // The texel in the instance's own frame: measured from its centre,
        // turned where it faces the centre or its own hash spins it, and
        // divided by its width so that what is drawn is the source's unit.
        let about = sub(cx, self.point, centre);
        let about = match self.spin(cx, cell, angle) {
            Some(radians) => turn_by(cx, about, radians),
            None => about,
        };
        let width = self.width(cx, cell);
        let scaled = div(cx, about, width);
        let middle = k2(cx, 0.5, 0.5);
        let read = add(cx, scaled, middle);

        // Outside the source's own unit there is no instance, whatever the
        // source would answer there.
        let (x, y) = axes(cx, read);
        let zero = k(cx, 0.0);
        let one = k(cx, 1.0);
        let mut window = None;
        for axis in [x, y] {
            let after = step(cx, zero, axis);
            let before = step(cx, axis, one);
            let band = mul(cx, after, before);
            window = Some(match window {
                Some(previous) => mul(cx, previous, band),
                None => band,
            });
        }
        let window = window.unwrap_or(one);

        let drawn = cx.substitute(self.source, read);
        let masked = mul(cx, drawn, window);
        let faded = if node.opacity_variation > 0.0 {
            let hash = self.hash(cx, cell, FADE_SALT);
            let variation = k(cx, node.opacity_variation);
            let taken = mul(cx, hash, variation);
            let opacity = complement(cx, taken);
            mul(cx, masked, opacity)
        } else {
            masked
        };
        let Some(mask) = self.mask else {
            return faded;
        };
        // The mask at this instance's own centre, against a number of the
        // instance's own, exactly as a scatter reads its cells: an instance is
        // either drawn or it is not, and which it is stays the same wherever in
        // it the texel doing the asking is. Every centre lands inside the unit,
        // because the check keeps the whole ring inside the repeat.
        let where_it_stands = add(cx, centre, middle);
        let here = cx.substitute(mask, where_it_stands);
        let gate = self.hash(cx, cell, GATE_SALT);
        let keep = step(cx, gate, here);
        mul(cx, faded, keep)
    }

    /// One of this instance's own numbers, in `0..=1`.
    fn hash(&self, cx: &mut Lowering, cell: ValueId, salt: usize) -> ValueId {
        let seed = self.node.seed.wrapping_add(INSTANCE_SALT[salt]);
        cell_hash(cx, cell, self.repeats, seed)
    }

    /// How far to turn the texel before the instance reads it, or `None` where
    /// the instance is drawn square to the repeat and unspun.
    ///
    /// The two turns are one turn. Facing the centre turns the instance's
    /// *frame* by its own angle, which is the texel turned back by it, and the
    /// rotation variation adds a spin of up to half a turn either way — as far
    /// as a rotation can go before it meets itself.
    fn spin(&self, cx: &mut Lowering, cell: ValueId, angle: ValueId) -> Option<ValueId> {
        let node = self.node;
        let facing = node.face_centre.then(|| {
            let back = k(cx, -1.0);
            mul(cx, angle, back)
        });
        if node.rotation_variation <= 0.0 {
            return facing;
        }
        let seed = node.seed.wrapping_add(INSTANCE_SALT[SPIN_SALT]);
        let hash = signed_hash(cx, cell, self.repeats, seed);
        let spread = k(cx, node.rotation_variation * TURN * 0.5);
        let wobble = mul(cx, hash, spread);
        Some(match facing {
            Some(facing) => add(cx, facing, wobble),
            None => wobble,
        })
    }

    /// The width of one instance: its own [`CircleSplatter::scale`], less
    /// whatever its hash shrinks it by.
    fn width(&self, cx: &mut Lowering, cell: ValueId) -> ValueId {
        let node = self.node;
        let full = k(cx, node.scale);
        if node.scale_variation <= 0.0 {
            return full;
        }
        let hash = self.hash(cx, cell, SIZE_SALT);
        let shrink = k(cx, node.scale_variation);
        let taken = mul(cx, hash, shrink);
        let kept = complement(cx, taken);
        mul(cx, kept, full)
    }
}
