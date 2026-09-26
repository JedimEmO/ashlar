//! The arithmetic every node lowering is written out of.
//!
//! Two things live here. The first is shorthand: [`Lowering::emit`] is the
//! honest spelling of an instruction, and a node that emits forty of them reads
//! as a list of emits rather than as the formula it is, so the ops a lowering
//! reaches for most have a name here and a lowering reads as arithmetic.
//!
//! The second is the frame arithmetic the generators and the resamplers share —
//! folding an axis, hashing a lattice cell, turning a point about the centre of
//! the repeat — because a fold written twice is a fold that drifts, and the
//! period rules rest on these being one thing.

use crate::Value;
use crate::ir::{Lowering, Op, ValueId};

/// One turn in radians. Every angle in the graph is in turns or in degrees, and
/// every angle in the IR is in radians.
pub(crate) const TURN: f32 = std::f32::consts::TAU;

macro_rules! shorthand {
    ($($(#[$doc:meta])* $name:ident($($arg:ident),*) => $op:ident),* $(,)?) => {
        $(
            $(#[$doc])*
            pub(crate) fn $name(cx: &mut Lowering $(, $arg: ValueId)*) -> ValueId {
                cx.emit(Op::$op, [$($arg),*])
            }
        )*
    };
}

shorthand! {
    /// `a + b`.
    add(a, b) => Add,
    /// `a - b`.
    sub(a, b) => Sub,
    /// `a * b`.
    mul(a, b) => Mul,
    /// `a / b`, zero where `b` is zero.
    div(a, b) => Div,
    /// The smaller.
    min(a, b) => Min,
    /// The larger.
    max(a, b) => Max,
    /// One where `x` is at least `edge`.
    step(edge, x) => Step,
    /// The dot product.
    dot(a, b) => Dot,
    /// `|a|`.
    abs(a) => Abs,
    /// The largest whole number no greater than `a`.
    floor(a) => Floor,
    /// `a` less its floor, so a negative coordinate wraps forward.
    fract(a) => Fract,
    /// The sine, in radians.
    sin(a) => Sin,
    /// The cosine, in radians.
    cos(a) => Cos,
    /// The length of a vector.
    length(a) => Length,
    /// A vector scaled to unit length, or zero where it was zero.
    normalize(a) => Normalize,
    /// `a + (b - a) * t`.
    mix(a, b, t) => Mix,
    /// The smooth ramp from `low` to `high`.
    smoothstep(low, high, x) => Smoothstep,
    /// `x` held between two bounds.
    clamp(x, low, high) => Clamp,
    /// `on_true` where the condition is at least a half.
    select(condition, on_true, on_false) => Select,
}

/// A literal.
pub(crate) fn k(cx: &mut Lowering, value: f32) -> ValueId {
    cx.constant(value)
}

/// A literal `Vec2`.
pub(crate) fn k2(cx: &mut Lowering, u: f32, v: f32) -> ValueId {
    cx.value(Value::Vec2([u, v]))
}

/// A `Vec2` from two scalars.
pub(crate) fn vec2(cx: &mut Lowering, u: ValueId, v: ValueId) -> ValueId {
    cx.vector(&[u, v])
}

/// The two lanes of a `Vec2`, in order.
pub(crate) fn axes(cx: &mut Lowering, uv: ValueId) -> (ValueId, ValueId) {
    (cx.emit(Op::Extract(0), [uv]), cx.emit(Op::Extract(1), [uv]))
}

/// `one - value`, at whatever width the value arrived at.
pub(crate) fn complement(cx: &mut Lowering, value: ValueId) -> ValueId {
    let one = k(cx, 1.0);
    sub(cx, one, value)
}

/// `value * scale + offset`, with both constants.
pub(crate) fn affine(cx: &mut Lowering, value: ValueId, scale: f32, offset: f32) -> ValueId {
    let scale = k(cx, scale);
    let scaled = mul(cx, value, scale);
    let offset = k(cx, offset);
    add(cx, scaled, offset)
}

/// A count as the float a coordinate is scaled by.
///
/// Every count that reaches this is a period, a row, a column or an instance
/// count bounded by [`MAX_PERIOD`](crate::MAX_PERIOD), and `f32` holds every
/// whole number that small exactly. The lint is about counts that are not.
#[expect(
    clippy::cast_precision_loss,
    reason = "a period, row, column or instance count is bounded by MAX_PERIOD, \
              which f32 holds exactly"
)]
pub(crate) fn count(value: u32) -> f32 {
    value as f32
}

/// The hash of one lattice cell, in `0..=1`, with the lattice wrapped at
/// `repeats`.
///
/// This is the only randomness in the crate, and the wrap is what makes a
/// generator tile: cell zero and cell `repeats` are the same cell, so the field
/// meets itself at the seam by construction.
pub(crate) fn cell_hash(cx: &mut Lowering, cell: ValueId, repeats: ValueId, seed: u32) -> ValueId {
    cx.emit(Op::Hash2(seed), [cell, repeats])
}

/// A hash in `-1..=1`, which is what a jitter and a gradient want.
pub(crate) fn signed_hash(
    cx: &mut Lowering,
    cell: ValueId,
    repeats: ValueId,
    seed: u32,
) -> ValueId {
    let hash = cell_hash(cx, cell, repeats, seed);
    affine(cx, hash, 2.0, -1.0)
}

/// The mirror fold of one axis: `0.5 - |0.5 - fract(t)|`.
///
/// Continuous, running from 0 up to 0.5 and back down, and repeating once, so
/// whatever it folds meets itself at the seam. This is the fold
/// [`Mirror`](super::Mirror) and the period rule both describe, written once.
pub(crate) fn mirror_fold(cx: &mut Lowering, t: ValueId) -> ValueId {
    let wrapped = fract(cx, t);
    let half = k(cx, 0.5);
    let from_middle = sub(cx, half, wrapped);
    let distance = abs(cx, from_middle);
    sub(cx, half, distance)
}

/// The quadrant fold of one axis: `1 - |1 - 2 * fract(t)|`.
///
/// Runs from 0 to 1 and back again within the unit, so the result reads the
/// same value at 1 as at 0 whatever the source's period — a source that does
/// not tile at all included.
pub(crate) fn quadrant_fold(cx: &mut Lowering, t: ValueId) -> ValueId {
    let wrapped = fract(cx, t);
    let doubled = affine(cx, wrapped, 2.0, -1.0);
    let distance = abs(cx, doubled);
    complement(cx, distance)
}

/// A point turned by a constant angle in radians, about the origin.
///
/// The angle is a node's own field, so its sine and cosine are folded here
/// rather than emitted: a transform costs four multiplies and two adds, and no
/// `libm` call reaches a graph that only rotates by a quarter turn.
pub(crate) fn turn(cx: &mut Lowering, point: ValueId, radians: f32) -> ValueId {
    let (x, y) = axes(cx, point);
    let (sine, cosine) = radians.sin_cos();
    let (sine, cosine) = (k(cx, sine), k(cx, cosine));
    let xc = mul(cx, x, cosine);
    let ys = mul(cx, y, sine);
    let turned_x = sub(cx, xc, ys);
    let xs = mul(cx, x, sine);
    let yc = mul(cx, y, cosine);
    let turned_y = add(cx, xs, yc);
    vec2(cx, turned_x, turned_y)
}

/// A point turned by an angle the expression carries.
pub(crate) fn turn_by(cx: &mut Lowering, point: ValueId, radians: ValueId) -> ValueId {
    let (x, y) = axes(cx, point);
    let sine = sin(cx, radians);
    let cosine = cos(cx, radians);
    let xc = mul(cx, x, cosine);
    let ys = mul(cx, y, sine);
    let turned_x = sub(cx, xc, ys);
    let xs = mul(cx, x, sine);
    let yc = mul(cx, y, cosine);
    let turned_y = add(cx, xs, yc);
    vec2(cx, turned_x, turned_y)
}

/// A texel's coordinate as a point measured from the centre of the repeat.
///
/// The coordinate is wrapped before it is measured, and that wrap is the whole
/// of why a node built on this may claim a period of one: it makes the field at
/// `u = 1.3` the field at `u = 0.3`, so a disc drawn inside the repeat is the
/// same disc one unit along rather than a disc cut at the seam. It is the wrap
/// a [`Shape`](super::Shape) takes for the same reason, and the check that
/// keeps the disc inside the repeat is what makes it harmless.
pub(crate) fn about_centre(cx: &mut Lowering, uv: ValueId) -> ValueId {
    let wrapped = fract(cx, uv);
    let centre = k2(cx, 0.5, 0.5);
    sub(cx, wrapped, centre)
}

/// The polar coordinates of a point about the origin: the angle in radians,
/// which [`Op::Atan2`] answers in `-pi..=pi`, and the radius.
///
/// Written once because every node that reads a field round a circle — an
/// angular [`Kaleidoscope`](super::Kaleidoscope), a
/// [`CircleMap`](super::CircleMap), the sector fold a polygon
/// [`Shape`](super::Shape) takes — has to agree about which direction angle
/// zero points in and which way the angle runs. Two spellings of `atan2(y, x)`
/// are two chances to disagree, and a disagreement would show as a feature that
/// sits a quarter turn from where the node that placed it thinks it is.
pub(crate) fn polar(cx: &mut Lowering, point: ValueId) -> (ValueId, ValueId) {
    let (x, y) = axes(cx, point);
    let angle = cx.emit(Op::Atan2, [y, x]);
    let radius = length(cx, point);
    (angle, radius)
}

/// A signed distance read as a mask: one inside the shape, zero outside it,
/// with the falloff of width `edge` taken inward from the boundary.
///
/// Written once because two nodes answer it and they have to agree exactly. A
/// [`Shape`](super::Shape) ramps its own distance here, and an
/// [`SdfMask`](super::SdfMask) ramps whatever a boolean left behind; if those
/// were two spellings, a shape read as a mask and the same shape read as a
/// distance and masked afterwards would differ by however far the two had
/// drifted, and the whole point of carrying a distance through a boolean is
/// that it can be turned back into the mask it started as.
///
/// Inward rather than centred on the boundary, so a ramped shape reaches
/// exactly as far as the field it ramped. The complement is what makes it a
/// mask rather than a hole: the smoothstep rises from zero at `-edge` to one at
/// the boundary, and the inside is what is left over.
pub(crate) fn inward_ramp(cx: &mut Lowering, distance: ValueId, edge: f32) -> ValueId {
    let inner = k(cx, -edge);
    let zero = k(cx, 0.0);
    let ramp = smoothstep(cx, inner, zero, distance);
    complement(cx, ramp)
}
