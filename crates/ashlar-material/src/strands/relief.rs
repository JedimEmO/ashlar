//! The splat: a scattered set of strands, seen from directly above.
//!
//! [`mesh`](super::mesh) turns a strand into triangles standing off a surface.
//! This turns the same strand into the footprint it casts on that surface, so
//! the PBR half of the material can read back the blades the geometry half
//! grows. It is what makes the level-of-detail chain honest: the relief the
//! camera sees once the blades have faded is the blades.
//!
//! # The footprint
//!
//! A strand is a quadratic Bézier, and [`geometry`](super::geometry) says which
//! one: the root, the root plus half a length along the leaning tangent, and
//! that plus half a length along the drooping one. Seen from above, the surface
//! normal is the viewing direction, so the footprint is that same curve with
//! its normal component dropped — a Bézier in the UV plane, laid along the
//! strand's own `direction`, whose control offsets are the *sines* of the lean
//! and the droop. What is left of the normal component is the cosines, and that
//! is the height the curve stands at, which is [`ReliefOutput::Height`].
//!
//! Metres become UV by dividing by
//! [`StrandPlan::repeat_metres`](crate::ir::StrandPlan::repeat_metres). A graph
//! cannot work that out for itself — the same graph dresses a two-metre
//! flagstone and a half-metre one — so the node is told.
//!
//! # Wrapped, antialiased and independent of the threads
//!
//! Wrapped, because every plane in this crate is: a strand near the seam
//! contributes to both sides of it, and a texel index is taken modulo the
//! resolution rather than clipped.
//!
//! Antialiased, because a blade is about one texel wide at the resolutions a
//! strand material is baked at, and a hard-edged splat of something that thin
//! is a dotted line. Coverage falls off over half a texel either side of the
//! ribbon's edge, which is the narrowest filter that leaves no gaps.
//!
//! Independent of the threads, because the *plane rows* are what is divided
//! rather than the strands: a thread walks every strand whose footprint reaches
//! its rows, in the one global order the set is sorted in, so a texel
//! accumulates the same contributions in the same order however the rows were
//! split. A splat that divided the strands instead would have to reduce
//! partial planes, and `a + b` of two coverages is not `b + a` in an `f32`.

use std::num::NonZeroUsize;

use super::{Strand, StrandSet};
use crate::{
    StrandLayer,
    interp::Plane,
    nodes::StrandReliefOutput as ReliefOutput,
    planes::{put, rows_across_threads, texels},
};

/// How far, in texels, coverage fades across the edge of a footprint.
///
/// Half a texel either way, which is the narrowest antialiasing that leaves no
/// gaps: a ribbon narrower than a texel then still writes coverage
/// proportional to how much of the texel it covers, rather than writing one
/// texel in three and nothing in between. Wider would blur a blade into the
/// blade beside it, which at sixteen thousand blades a square metre is the
/// whole picture.
const FEATHER_TEXELS: f32 = 0.5;

/// The fewest points along a strand's curve the splat steps, per segment of
/// the layer.
///
/// A floor rather than the count itself: what decides how many stamps a strand
/// needs is how far apart they land, which is a fact about the *plane* and not
/// about the layer. This is only here so that a short, wide strand still gets
/// enough stamps to show the curve its segments describe.
const STEPS_PER_SEGMENT: u32 = 4;

/// The most points along one strand the splat steps.
///
/// A strand whose footprint is longer than this many stamps of its own width
/// is a hair lying across a quarter of the repeat, and the bound is what keeps
/// one bad field from turning a splat into a march over the whole plane. At the
/// bound the stamps are further apart than a radius and the footprint dots;
/// that is the honest failure and it is visible, which a splat that took an
/// hour would not be.
const MAX_STAMPS: u32 = 256;

/// How far apart consecutive stamps may land, as a fraction of the radius.
///
/// Half, so every point of the footprint is inside a stamp and the strokes come
/// out continuous. It is a fraction of the *root* radius rather than of the
/// radius at each step, because a fully tapered blade's tip radius is zero and
/// the spacing has to be one number for the whole curve.
const STAMP_SPACING: f32 = 0.5;

/// Splat a scattered set into one plane of one of its fields.
///
/// `repeat_metres` is how many metres one repeat covers, and is what turns the
/// layer's metres into UV.
pub(crate) fn splat(
    set: &StrandSet,
    layer: &StrandLayer,
    output: ReliefOutput,
    repeat_metres: f32,
    resolution: u32,
    threads: Option<NonZeroUsize>,
) -> Plane {
    let value_type = crate::ir::ir_type(output.value_type());
    let components = value_type.components();
    let size = resolution.max(1);
    let mut lanes = vec![0.0_f32; texels(size) * components];
    let footprints: Vec<Footprint> = set
        .strands()
        .iter()
        .map(|strand| Footprint::of(strand, layer, repeat_metres, size))
        .collect();
    // Which strands reach which row of the plane, worked out once. Without it
    // every thread would test every strand against its own rows, which at
    // sixty-five thousand strands and eight threads is half a million bounding
    // tests per plane and the only part of a splat anyone would notice.
    let rows = row_buckets(&footprints, size);
    let stride = size as usize * components;
    rows_across_threads(&mut lanes, size, stride, threads, |first, chunk| {
        for (row, line) in chunk.chunks_mut(stride.max(1)).enumerate() {
            let y = first as usize + row;
            let mut accumulated: Vec<Texel> = vec![Texel::default(); size as usize];
            for index in rows.get(y).map_or(&[][..], Vec::as_slice) {
                let Some(footprint) = footprints.get(*index as usize) else {
                    continue;
                };
                footprint.stamp(y, size, &mut accumulated);
            }
            for (x, texel) in accumulated.iter().enumerate() {
                let value = texel.read(output);
                for (lane, component) in value.iter().enumerate().take(components) {
                    put(line, x * components + lane, *component);
                }
            }
        }
    });
    Plane::new(size, value_type, lanes)
}

/// What one texel has accumulated so far.
///
/// Two answers rather than one, because the two kinds of output want different
/// reductions. Coverage is a *union*: overlapping blades cover a texel more
/// completely, never less, so it accumulates as an over-composite and is
/// independent of how the contributions happen to be ordered beyond the last
/// bits. Everything else is a property of one strand, and the honest strand to
/// take it from is the one on top — which is what "seen from above" means, and
/// what keeps the colour a texel reads the colour of the blade that is covering
/// it.
#[derive(Clone, Copy, Default)]
struct Texel {
    coverage: f32,
    /// Every contribution summed, without the union's saturation.
    mass: f32,
    /// The same sum, weighted by the height each contribution stood at.
    occlusion: f32,
    /// The sum of each contribution's own alpha, which is what the weighted
    /// fields below are divided by.
    weight: f32,
    /// `along`, `id` and the three colour lanes, each already multiplied by the
    /// alpha that contributed them.
    along: f32,
    id: f32,
    color: [f32; 3],
    /// How high the topmost contribution stood, and whether there was one.
    top: Option<f32>,
}

#[derive(Clone, Copy)]
struct Top {
    height: f32,
    along: f32,
    id: f32,
    color: [f32; 3],
}

impl Texel {
    /// Add one contribution, at coverage `alpha`.
    fn add(&mut self, alpha: f32, top: Top) {
        if alpha <= 0.0 {
            return;
        }
        self.coverage = alpha.mul_add(1.0 - self.coverage, self.coverage);
        self.mass += alpha;
        self.occlusion += alpha * top.height;
        self.weight += alpha;
        self.along += alpha * top.along;
        self.id += alpha * top.id;
        for (sum, lane) in self.color.iter_mut().zip(top.color) {
            *sum += alpha * lane;
        }
        // The height, and the height alone, is the *highest* contribution
        // rather than the mean: where two blades cross, the surface is the one
        // on top, which is what Substance's tile sampler calls Max blending and
        // the only reduction that leaves a crossing looking like a crossing.
        // Strictly greater, so a tie is broken by the order the set is sorted
        // in rather than by which thread got there first — the rows are split
        // and the strands are not, so that order is the same everywhere.
        if self.top.is_none_or(|current| top.height > current) {
            self.top = Some(top.height);
        }
    }

    /// One weighted field, or zero where nothing covered the texel.
    ///
    /// Weighted by coverage rather than taken from the topmost contribution,
    /// and the difference is the whole of a crown that can brighten: the
    /// highest point of a tapered blade is its *narrowest*, so a hairline tip
    /// crossing a texel would otherwise overrule a full-width root, and every
    /// weighted field would be read off whichever strand happened to be
    /// thinnest there.
    fn mean(&self, total: f32) -> f32 {
        if self.weight > 0.0 {
            total / self.weight
        } else {
            0.0
        }
    }

    /// The lanes this texel writes for one output.
    fn read(&self, output: ReliefOutput) -> [f32; 3] {
        match output {
            ReliefOutput::Coverage => [self.coverage.clamp(0.0, 1.0), 0.0, 0.0],
            // Bare surface is zero rather than the height of a strand that is
            // not there, which is what lets a graph add a relief to its own
            // bed without first masking it by the coverage.
            ReliefOutput::Height => [self.top.unwrap_or(0.0), 0.0, 0.0],
            ReliefOutput::Id => [self.mean(self.id), 0.0, 0.0],
            ReliefOutput::Along => [self.mean(self.along), 0.0, 0.0],
            ReliefOutput::Color => [
                self.mean(self.color[0]),
                self.mean(self.color[1]),
                self.mean(self.color[2]),
            ],
            ReliefOutput::Mass => [self.mass, 0.0, 0.0],
            ReliefOutput::Occlusion => [self.occlusion, 0.0, 0.0],
        }
    }
}

/// One strand's footprint: the curve it casts on the UV plane, in texels.
///
/// Held in texels rather than in UV because everything below it is: a stamp
/// walks integer columns of one row, and a radius that has already been
/// converted once is a radius the inner loop does not convert again.
struct Footprint {
    /// Where the curve starts, in texels, unwrapped.
    root: [f32; 2],
    /// Which way it lies, unit or zero.
    direction: [f32; 2],
    /// How far along `direction` the two control points sit, in texels.
    controls: [f32; 2],
    /// How high those controls stand, in units of `length_metres`.
    heights: [f32; 2],
    /// Half the root width, in texels.
    half_width: f32,
    /// How much of the root width the tip gives up.
    taper: f32,
    /// Where along the strand it is widest.
    midpoint: f32,
    /// How far the ribbon is turned about its own curve, in radians.
    facing: f32,
    /// How much the colour darkens at the root.
    occlusion: f32,
    /// How many points along the curve to stamp.
    steps: u32,
    /// The two Bézier control directions' components along the surface normal
    /// and along `direction`, which is what the curve's own tangent is built
    /// from and what [`Self::tangent_up`] interpolates.
    slopes: [f32; 2],
    forwards: [f32; 2],
    /// Rows of the plane this footprint can reach, as an inclusive span of
    /// possibly-negative indices to be wrapped.
    span: [i64; 2],
    strand: Strand,
}

impl Footprint {
    /// The footprint one strand casts.
    #[expect(
        clippy::cast_precision_loss,
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "a resolution is bounded by MAX_RESOLUTION, which an f32 counts \
                  exactly; the stamp count is clamped into range before its cast, \
                  and the row span is a texel bound that `row_buckets` wraps"
    )]
    fn of(strand: &Strand, layer: &StrandLayer, repeat_metres: f32, resolution: u32) -> Self {
        let size = resolution.max(1) as f32;
        // Metres into texels: one repeat is `repeat_metres` metres wide and
        // `resolution` texels wide, and `repeat_metres` was validated positive
        // and finite where the node declared it.
        let texels = size / repeat_metres.max(f32::MIN_POSITIVE);
        let lean = strand.lean.clamp(0.0, 1.0) * std::f32::consts::FRAC_PI_2;
        let droop = lean + strand.bend * std::f32::consts::FRAC_PI_2;
        let half = strand.length.max(0.0) * 0.5;
        // The two Bézier control points, split into the component along the
        // surface — which is the footprint — and the component along the
        // normal, which is the height. `Curve::of` builds the same two.
        let (first_sin, first_cos) = lean.sin_cos();
        let (second_sin, second_cos) = droop.sin_cos();
        let length = layer.length_metres.abs().max(f32::MIN_POSITIVE);
        let half_width = strand.width.max(0.0) * 0.5 * texels;
        let root = [strand.root[0] * size, strand.root[1] * size];
        let controls = [half * first_sin * texels, {
            let along = half.mul_add(second_sin, half * first_sin);
            along * texels
        }];
        // How many stamps this footprint needs, from how long it is rather than
        // from how many segments the layer authored: a blade eighteen texels
        // long and two wide is a stroke, and the same layer over a shorter
        // repeat is a dot. The control polygon bounds the curve's own length,
        // which is all this needs.
        let polygon = controls[0].abs() + (controls[1] - controls[0]).abs();
        let stride = (half_width * STAMP_SPACING).max(FEATHER_TEXELS);
        let floor = layer.segments.clamp(1, crate::MAX_SEGMENTS) * STEPS_PER_SEGMENT;
        let steps = ((polygon / stride).ceil().clamp(1.0, f32::from(u16::MAX)) as u32)
            .clamp(floor, MAX_STAMPS);
        // The two control heights, in units of the layer's own length, less the
        // depth the root sank to. A blade whose root is buried stands that much
        // lower over every texel it covers, which is what the sink is for.
        let sunk = strand.height_offset.max(0.0) / length;
        let heights = [
            half.mul_add(first_cos, -(strand.height_offset.max(0.0) * 0.5)) / length,
            (half.mul_add(second_cos, half * first_cos) / length) - sunk,
        ];
        // How far the footprint reaches *along v*, which is the axis the rows
        // are divided on: the curve stays inside the convex hull of its three
        // control points, so its offset along the surface is bounded by the
        // larger control, and only the `v` component of the lean direction
        // turns that into rows. A blade lying along `u` then touches a handful
        // of rows rather than the span of its own length, which is most of what
        // a splat costs.
        let reach = strand.direction[1].abs() * controls[1].abs().max(controls[0].abs())
            + half_width
            + FEATHER_TEXELS;
        let span = [
            (root[1] - reach).floor() as i64,
            (root[1] + reach).ceil() as i64,
        ];
        Self {
            root,
            direction: strand.direction,
            controls,
            heights,
            half_width,
            slopes: [first_cos, second_cos],
            forwards: [first_sin, second_sin],
            taper: layer.taper.clamp(0.0, 1.0),
            midpoint: layer.midpoint.clamp(0.0, 1.0),
            facing: strand.facing,
            occlusion: layer.root_occlusion.clamp(0.0, 1.0),
            steps,
            span,
            strand: *strand,
        }
    }

    /// Every point of the curve, as a disc in texels: centre, radius, the
    /// height it stands at and how far along the strand it is.
    fn discs(&self) -> impl Iterator<Item = (f32, f32, f32, f32, f32)> + '_ {
        (0..=self.steps).map(move |step| {
            let t = f32::from(u16::try_from(step).unwrap_or(u16::MAX))
                / f32::from(u16::try_from(self.steps.max(1)).unwrap_or(u16::MAX));
            let inverse = 1.0 - t;
            // The quadratic through `0`, `controls[0]` and `controls[1]`,
            // written the way `Curve::at` writes it so the two agree.
            let quadratic =
                |first: f32, second: f32| first.mul_add(2.0 * inverse * t, second * (t * t));
            let along = quadratic(self.controls[0], self.controls[1]);
            // Never below the surface. A strand whose bend carries its tip past
            // horizontal is drooping onto the ground, and a sunk root starts
            // under it: both are real, and both mean the curve leaves the
            // half-space the relief describes. A plane that carried the
            // negative would read as a trench where a blade lay down, and the
            // graph adding it to a bed would dig one.
            let height = quadratic(self.heights[0], self.heights[1]).max(0.0);
            // The width the mesh builder gives this point, through the one
            // definition both of them read, narrowed by however much the
            // ribbon is turned away from the surface. `facing` turns the width
            // axis about the curve's own tangent, and what a top-down splat
            // sees of it is its projection onto the surface — a quarter turn is
            // edge-on and casts nearly nothing. `up` below is the tangent's own
            // vertical component, which is what the axis rotates towards.
            let width = super::width_at(t, self.taper, self.midpoint);
            let (turn, straight) = self.facing.sin_cos();
            let up = self.tangent_up(t);
            let seen = straight
                .mul_add(straight, up * up * turn * turn)
                .max(0.0)
                .sqrt();
            (
                self.direction[0].mul_add(along, self.root[0]),
                self.direction[1].mul_add(along, self.root[1]),
                self.half_width * width * seen,
                height,
                t,
            )
        })
    }

    /// How much of the curve's tangent at `t` points along the surface normal.
    ///
    /// The Bézier tangent is the straight interpolation of the two control
    /// directions, exactly as [`Curve::at`](super::geometry) builds it, and its
    /// vertical component is the cosine of how far the strand has leaned over.
    /// A turned ribbon's width axis rotates towards that component, so this is
    /// what decides how much of the width a top-down splat still sees.
    fn tangent_up(&self, t: f32) -> f32 {
        // The control directions in the (normal, direction) plane, as the pair
        // of sines and cosines `Footprint::of` already resolved: the along
        // components are `controls`, the up components are `heights`, and both
        // are scaled the same way, so their ratio is the tangent's slope.
        let up = self.slopes[0] + (self.slopes[1] - self.slopes[0]) * t;
        let forward = self.forwards[0] + (self.forwards[1] - self.forwards[0]) * t;
        let length = up.hypot(forward);
        if length > 0.0 {
            (up / length).abs()
        } else {
            1.0
        }
    }

    /// Add this footprint's contribution to one row of the plane.
    ///
    /// The row is named by its wrapped index, and the footprint's own span is
    /// unwrapped, so the two meet by walking the span and wrapping each row
    /// rather than by wrapping the span — which would be two spans wherever a
    /// strand crosses the seam.
    #[expect(
        clippy::cast_precision_loss,
        clippy::cast_possible_truncation,
        reason = "texel coordinates, bounded by MAX_RESOLUTION"
    )]
    fn stamp(&self, row: usize, resolution: u32, into: &mut [Texel]) {
        let size = i64::from(resolution.max(1));
        let row = i64::try_from(row).unwrap_or_default();
        for (x, y, radius, height, t) in self.discs() {
            let reach = radius + FEATHER_TEXELS;
            let low = (y - reach).floor() as i64;
            let high = (y + reach).ceil() as i64;
            // The unwrapped lines of this disc that land on this row: the first
            // one at or above `low` that is congruent, and then every wrap
            // after it. Stepping by the wrap rather than filtering the span is
            // what keeps a splat linear in the strands rather than in the
            // square of their reach.
            let mut line = low + (row - low).rem_euclid(size);
            while line <= high {
                let dy = line as f32 + 0.5 - y;
                line += size;
                if dy.abs() > reach {
                    continue;
                }
                let first = (x - reach).floor() as i64;
                let last = (x + reach).ceil() as i64;
                for column in first..=last {
                    let dx = column as f32 + 0.5 - x;
                    let alpha = coverage(dx.hypot(dy), radius);
                    if alpha <= 0.0 {
                        continue;
                    }
                    let index = usize::try_from(column.rem_euclid(size)).unwrap_or_default();
                    let Some(texel) = into.get_mut(index) else {
                        continue;
                    };
                    texel.add(alpha, self.top(height, t));
                }
            }
        }
    }

    /// What one point of the curve contributes beyond its coverage.
    fn top(&self, height: f32, t: f32) -> Top {
        let lit = 1.0 - self.occlusion * (1.0 - t);
        let strand = &self.strand;
        Top {
            height,
            along: t,
            // The strand's own free hash, which is what a graph tints by. It is
            // `phase` rather than `rank` deliberately: a rank is the
            // level-of-detail order, so tinting by it would make a lawn's
            // colour and its level of detail the same gradient.
            id: strand.phase,
            color: [
                lerp(strand.root_color[0], strand.tip_color[0], t) * lit,
                lerp(strand.root_color[1], strand.tip_color[1], t) * lit,
                lerp(strand.root_color[2], strand.tip_color[2], t) * lit,
            ],
        }
    }
}

/// Which strands reach which row, by index into the footprint list, ascending.
///
/// Ascending because that is the order the set is sorted in, and the order every
/// row must accumulate in for the plane not to depend on how the rows were
/// divided.
#[expect(
    clippy::cast_possible_truncation,
    reason = "a strand index is bounded by MAX_PERIOD squared, which is under u32::MAX"
)]
fn row_buckets(footprints: &[Footprint], resolution: u32) -> Vec<Vec<u32>> {
    let size = i64::from(resolution.max(1));
    let mut rows: Vec<Vec<u32>> = vec![Vec::new(); resolution.max(1) as usize];
    for (index, footprint) in footprints.iter().enumerate() {
        let [low, high] = footprint.span;
        // A footprint taller than the plane covers every row once, which is
        // also the bound that keeps this loop from walking a span a bad field
        // made enormous.
        if high - low >= size {
            for row in &mut rows {
                row.push(index as u32);
            }
            continue;
        }
        for line in low..=high {
            if let Some(row) = rows.get_mut(usize::try_from(line.rem_euclid(size)).unwrap_or(0)) {
                row.push(index as u32);
            }
        }
    }
    rows
}

/// How much of a texel a disc of `radius` centred `distance` away covers.
///
/// A linear ramp over [`FEATHER_TEXELS`] either side of the edge, which is the
/// cheapest filter that is continuous and exact at both ends: a texel well
/// inside the disc is covered, one well outside is not, and one on the edge is
/// covered in proportion to how far in it is. A radius smaller than the feather
/// — a blade thinner than a texel, which is most of them — still writes a peak
/// proportional to its width rather than saturating, which is what keeps a
/// coverage plane from reading every lawn as solid.
fn coverage(distance: f32, radius: f32) -> f32 {
    let alpha = (radius + FEATHER_TEXELS - distance) / (2.0 * FEATHER_TEXELS);
    let scale = (radius / FEATHER_TEXELS).min(1.0);
    (alpha * scale).clamp(0.0, 1.0)
}

/// Between two numbers, by `t`. The same interpolation
/// [`geometry`](super::geometry) puts on a vertex colour, so a texel under a
/// blade reads the colour that blade is drawn in.
fn lerp(from: f32, to: f32, t: f32) -> f32 {
    from + (to - from) * t
}
