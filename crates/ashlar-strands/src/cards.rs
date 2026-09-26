//! Cards: a strand set seen from the side, drawn into an atlas.
//!
//! [`mesh`](crate::mesh) draws a strand as triangles and `ashlar-material`'s
//! `StrandRelief` splat draws it as a footprint on the surface it grows out of.
//! This is the third view of the same set and the cheapest: a few dozen
//! strands rasterised *side on* into a square of texels, so that a renderer can
//! stand two crossed quads where a hundred blades used to be.
//!
//! It is the level past the last one [`StrandSet::prefix`] cuts. A rank prefix
//! stops paying when the strands it keeps are under a pixel wide; below that
//! the only thing worth drawing is the silhouette, and a silhouette is a
//! picture.
//!
//! # What a card is
//!
//! One *variant* of the atlas is a tuft: a handful of strands from a rank
//! prefix of the set, spread across the card's width and drawn against the
//! plane the wind would blow them in. The strands of a variant are chosen by
//! their own [`Strand::phase`], which is a free hash — so the variants are
//! disjoint, deterministic, and do not track the rank the level of detail cuts
//! on.
//!
//! What comes out is [`CardAtlas`]: linear colour with straight (not
//! premultiplied) alpha, and a normal per texel. Straight alpha because the
//! renderer draws a card as [`AlphaMode::Mask`], which compares the alpha
//! against a cutoff and never blends — ADR 0003 puts transparency out of scope,
//! and a cutout keeps depth writes, shadows and the prepass working.
//!
//! # The normal is derived, not carried
//!
//! A card has no geometry to take a normal from: a texel of it may be covered
//! by a blade seen edge-on, by two blades crossing, or by the gap between them.
//! So the atlas accumulates how much strand stands at each texel and derives
//! the normal from the *gradient* of that, exactly as a bake derives a normal
//! from a height field. A texel in the middle of a blade faces the camera and
//! one at its edge turns away, which is the whole of what a card's shading has
//! to say.
//!
//! # Deterministic
//!
//! Nothing here reads a clock, a thread count or a hash map. The same set at
//! the same request answers the same atlas, which is what lets a renderer cache
//! one under a key made of the request.

use crate::{Strand, StrandSet, geometry::width_at};

/// How far, in texels, coverage fades across the edge of a strand.
///
/// Half a texel either way, the same filter
/// the relief splat uses and for the same reason: a strand is about a
/// texel wide at the resolutions a card is drawn at, and a hard-edged stamp of
/// something that thin is a dotted line.
const FEATHER_TEXELS: f32 = 0.5;

/// How far apart consecutive stamps along one strand may land, as a fraction of
/// its root radius.
const STAMP_SPACING: f32 = 0.5;

/// The most points along one strand a card steps.
const MAX_STAMPS: u32 = 512;

/// How much of the card's width one variant's roots are spread over.
///
/// Four fifths, so a tuft has a margin either side and a blade that leans to
/// the edge of it still lands inside the card rather than being clipped at the
/// seam between two variants of the atlas.
const ROOT_SPREAD: f32 = 0.8;

/// How steep the derived normal is.
///
/// The gradient of the accumulated thickness is in units of "strands deep per
/// texel", which is not a slope in any physical sense, so this is the number
/// that turns it into one. A third gives a blade a readable round shoulder
/// without making the gaps between blades read as trenches.
const NORMAL_SLOPE: f32 = 0.33;

/// What a card atlas is asked for.
#[derive(Clone, Copy, Debug)]
pub struct CardRequest<'a> {
    /// The scattered set to draw.
    pub set: &'a StrandSet,
    /// The rank below which a strand may appear on a card, as
    /// [`StrandSet::prefix`] cuts it.
    ///
    /// A prefix rather than the whole set, because a card stands where the
    /// geometry has already been thinned twice and the strands it is standing
    /// in for are the ones that survived.
    pub keep: f32,
    /// How many variants across and down the atlas. One is a single card; two
    /// is four tufts, which is enough that a field of cards does not read as a
    /// stamp repeated.
    pub variants: u32,
    /// Texels across the whole atlas, on each axis. A power of two, so the
    /// variants divide it.
    pub resolution: u32,
    /// How many metres of world one variant covers, on each axis.
    ///
    /// A card is square: it has to be, because a crossed quad is the same
    /// picture from two directions and a tall thin card seen from the side
    /// would be a wide flat one from the other.
    pub metres: f32,
    /// How many strands to draw into one variant.
    ///
    /// The count a real tuft has, rather than the whole prefix: a card drawn
    /// from ten thousand strands is a green rectangle.
    pub strands_per_card: u32,
}

/// A strand set drawn side on, as the two maps a cutout material binds.
///
/// Row-major, `resolution` by `resolution`, with the variants laid out left to
/// right and then top to bottom. Plain vectors of plain arrays, because this
/// crate has no renderer in it: [`CardAtlas::uv_of`] is how a caller finds the
/// variant it wants.
#[derive(Clone, Debug, PartialEq)]
pub struct CardAtlas {
    /// Texels on each axis.
    pub resolution: u32,
    /// Variants on each axis.
    pub variants: u32,
    /// Linear RGB and straight alpha, one per texel.
    pub base_color: Vec<[f32; 4]>,
    /// A unit normal in the card's own tangent frame — `+x` across, `+y` up,
    /// `+z` out of the card — one per texel.
    pub normal: Vec<[f32; 3]>,
}

impl CardAtlas {
    /// The UV rectangle one variant occupies, as `[u0, v0, u1, v1]`.
    ///
    /// Variants are numbered row-major from the top left, and an index past the
    /// last one wraps, so a caller keying a card on a clump hash needs no
    /// modulo of its own.
    #[must_use]
    pub fn uv_of(&self, variant: u32) -> [f32; 4] {
        let across = self.variants.max(1);
        let index = variant % (across * across);
        let step = 1.0 / f32::from(u16::try_from(across).unwrap_or(1));
        let x = f32::from(u16::try_from(index % across).unwrap_or(0)) * step;
        let y = f32::from(u16::try_from(index / across).unwrap_or(0)) * step;
        [x, y, x + step, y + step]
    }

    /// What fraction of the atlas is covered at or above `cutoff`.
    ///
    /// The number a mip chain has to preserve: an ordinary box filter halves
    /// the alpha of every edge texel, so a cutout thins with every level and a
    /// card of grass evaporates at distance. A caller building mips rescales
    /// each level's alpha until this answers what level zero answered.
    #[must_use]
    pub fn coverage(&self, cutoff: f32) -> f32 {
        if self.base_color.is_empty() {
            return 0.0;
        }
        let covered = self
            .base_color
            .iter()
            .filter(|texel| texel[3] >= cutoff)
            .count();
        ratio(covered, self.base_color.len())
    }
}

/// Draw a strand set side on into an atlas of cards.
///
/// ```
/// use ashlar_strands::{CardRequest, Strand, StrandSet, StrandShape, cards};
///
/// // A few hundred leaning blades, differing in the free hash the variants
/// // and the spread across a card are taken from.
/// let strands = (0..512_u16)
///     .map(|index| {
///         let hash = f32::from(index) / 512.0;
///         Strand {
///             root: [hash, hash],
///             rank: hash,
///             phase: hash,
///             length: 0.12,
///             width: 0.004,
///             direction: [1.0, 0.0],
///             lean: 0.4,
///             bend: 0.3,
///             root_color: [0.05, 0.12, 0.01],
///             tip_color: [0.3, 0.45, 0.05],
///             ..Default::default()
///         }
///     })
///     .collect();
/// let set = StrandSet::new("blades".to_owned(), strands, [32, 32], StrandShape::default());
///
/// let atlas = cards(&CardRequest {
///     set: &set,
///     keep: 1.0,
///     variants: 2,
///     resolution: 128,
///     metres: 0.14,
///     strands_per_card: 24,
/// });
/// assert_eq!(atlas.base_color.len(), 128 * 128);
/// // Something was drawn, and it did not fill the card: a tuft has sky in it.
/// let covered = atlas.coverage(0.5);
/// assert!(covered > 0.0 && covered < 0.5, "{covered}");
/// ```
#[must_use]
pub fn cards(request: &CardRequest<'_>) -> CardAtlas {
    let resolution = request.resolution.max(1);
    let variants = request.variants.max(1);
    let texels = (resolution as usize).pow(2);
    let mut thickness = vec![0.0_f32; texels];
    let mut colour = vec![[0.0_f32; 3]; texels];
    let mut weight = vec![0.0_f32; texels];

    // One variant is a square block of the atlas, and every strand of it is
    // clipped to that block: a blade leaning out of its own card has left the
    // tuft the card is a picture of.
    let side = resolution / variants;
    let chosen = request.set.prefix(request.keep.clamp(0.0, 1.0));
    for variant in 0..variants * variants {
        let origin = [(variant % variants) * side, (variant / variants) * side];
        for (index, strand) in members(chosen, variant, variants, request.strands_per_card) {
            draw(
                strand,
                index,
                request,
                side,
                origin,
                resolution,
                &mut thickness,
                &mut colour,
                &mut weight,
            );
        }
    }

    let base_color = (0..texels)
        .map(|texel| {
            let total = weight[texel];
            let alpha = thickness[texel].clamp(0.0, 1.0);
            if total <= 0.0 {
                // A texel nothing covered carries the colour of the strand
                // nearest it and an alpha of zero. There is no such strand, so
                // it carries black — and that is safe because the renderer
                // never blends: a masked texel is discarded, and a bilinear tap
                // that half-catches one is dragged below the cutoff by the
                // alpha rather than towards black by the colour.
                return [0.0, 0.0, 0.0, 0.0];
            }
            [
                colour[texel][0] / total,
                colour[texel][1] / total,
                colour[texel][2] / total,
                alpha,
            ]
        })
        .collect();
    let normal = normals(&thickness, resolution);
    CardAtlas {
        resolution,
        variants,
        base_color,
        normal,
    }
}

/// The strands of one variant, with their index within it.
///
/// Partitioned by [`Strand::phase`], which is a free hash: the variants are
/// disjoint, the choice does not track the rank a level of detail cuts on, and
/// the same set answers the same tufts every run.
fn members(
    strands: &[Strand],
    variant: u32,
    variants: u32,
    per_card: u32,
) -> impl Iterator<Item = (u32, &Strand)> {
    let blocks = (variants * variants).max(1);
    let band = ratio(variant as usize, blocks as usize);
    let next = ratio(variant as usize + 1, blocks as usize);
    strands
        .iter()
        .filter(move |strand| strand.phase >= band && strand.phase < next)
        .take(per_card.max(1) as usize)
        .enumerate()
        .map(|(index, strand)| (u32::try_from(index).unwrap_or(0), strand))
}

/// Draw one strand into one variant's block.
///
/// The card's frame is the strand's own lean plane flattened: `+x` is the
/// direction it leans in, `+y` is the surface normal. So the curve is the same
/// quadratic Bézier [`geometry`](crate::mesh) builds, with its two control
/// points resolved into an along component and an up component — which is
/// exactly the pair the relief splat splits out, read the other way
/// round.
#[expect(
    clippy::too_many_arguments,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "one rasteriser over three parallel lanes; the coordinates are \
              texel indices bounded by the atlas resolution"
)]
fn draw(
    strand: &Strand,
    index: u32,
    request: &CardRequest<'_>,
    side: u32,
    origin: [u32; 2],
    resolution: u32,
    thickness: &mut [f32],
    colour: &mut [[f32; 3]],
    weight: &mut [f32],
) {
    let metres = request.metres.max(f32::MIN_POSITIVE);
    let texels = f32::from(u16::try_from(side.max(1)).unwrap_or(1)) / metres;
    let set = request.set;
    let lean = strand.lean.clamp(0.0, 1.0) * std::f32::consts::FRAC_PI_2;
    let droop = lean + strand.bend.max(0.0) * std::f32::consts::FRAC_PI_2;
    let half = strand.length.max(0.0) * 0.5;
    let (first_sin, first_cos) = lean.sin_cos();
    let (second_sin, second_cos) = droop.sin_cos();
    // Where the root stands across the card. Spread by the strand's own free
    // hash rather than by where it was scattered, because the strands of one
    // variant come from all over the repeat and a card is a tuft rather than a
    // window onto the lawn.
    let spread = (strand.phase * f32::from(u16::try_from(index + 7).unwrap_or(7)) * 9.7).fract();
    let block = f32::from(u16::try_from(side.max(1)).unwrap_or(1));
    let root_x = (0.5 + (spread - 0.5) * ROOT_SPREAD) * block;
    // Which way the strand leans across the card. A card is the lean plane
    // flattened, so every strand of a variant would otherwise lean the same
    // way and the tuft would be a comb running off one edge. Half of them are
    // drawn from the other side, which is what a tuft actually looks like: the
    // blades on the far side of it lean away from the camera, and a side view
    // of one of those is the same blade mirrored.
    let facing = if (spread * 7.31).fract() < 0.5 {
        -1.0_f32
    } else {
        1.0
    };
    // The bottom of the card is the ground, and a sunk root starts below it.
    let root_y = -strand.height_offset.max(0.0) * texels;
    let along = [
        facing * half * first_sin * texels,
        facing * half.mul_add(second_sin, half * first_sin) * texels,
    ];
    let up = [
        half * first_cos * texels,
        half.mul_add(second_cos, half * first_cos) * texels,
    ];
    let half_width = strand.width.max(0.0) * 0.5 * texels;
    let polygon =
        along[0].abs() + (along[1] - along[0]).abs() + up[0].abs() + (up[1] - up[0]).abs();
    let stride = (half_width * STAMP_SPACING).max(FEATHER_TEXELS);
    let steps =
        ((polygon / stride).ceil().clamp(1.0, f32::from(u16::MAX)) as u32).clamp(4, MAX_STAMPS);
    let occlusion = set.root_occlusion().clamp(0.0, 1.0);

    for step in 0..=steps {
        let t = ratio(step as usize, steps.max(1) as usize);
        let inverse = 1.0 - t;
        let quadratic =
            |first: f32, second: f32| first.mul_add(2.0 * inverse * t, second * (t * t));
        let x = root_x + quadratic(along[0], along[1]);
        let y = root_y + quadratic(up[0], up[1]);
        // A ribbon turned edge-on presents its thickness rather than its width,
        // and a card sees that turn the other way round from a top-down splat:
        // what a side view loses is the *sine*. A fibre has no face to turn, so
        // the same expression answers its full radius whatever the facing is.
        let turned = match set.profile() {
            crate::StrandProfile::Blade => strand.facing.cos().abs().max(0.15),
            crate::StrandProfile::Fibre => 1.0,
        };
        let radius = half_width * width_at(t, set.taper(), set.midpoint()) * turned;
        let lit = 1.0 - occlusion * (1.0 - t);
        let tint = [
            lerp(strand.root_color[0], strand.tip_color[0], t) * lit,
            lerp(strand.root_color[1], strand.tip_color[1], t) * lit,
            lerp(strand.root_color[2], strand.tip_color[2], t) * lit,
        ];
        stamp(
            [x, y],
            radius,
            tint,
            side,
            origin,
            resolution,
            thickness,
            colour,
            weight,
        );
    }
}

/// One disc of one strand, clipped to its own variant's block.
///
/// The card's `y` runs up from the ground and a texel row runs down from the
/// top, so the row index is the block's height less the height in texels.
#[expect(
    clippy::too_many_arguments,
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    reason = "texel coordinates, bounded by the atlas resolution"
)]
fn stamp(
    centre: [f32; 2],
    radius: f32,
    tint: [f32; 3],
    side: u32,
    origin: [u32; 2],
    resolution: u32,
    thickness: &mut [f32],
    colour: &mut [[f32; 3]],
    weight: &mut [f32],
) {
    let reach = radius + FEATHER_TEXELS;
    let block = f32::from(u16::try_from(side.max(1)).unwrap_or(1));
    let first = (centre[0] - reach).floor().max(0.0) as i64;
    let last = (centre[0] + reach).ceil().min(block) as i64;
    let low = (block - centre[1] - reach).floor().max(0.0) as i64;
    let high = (block - centre[1] + reach).ceil().min(block) as i64;
    for row in low..high {
        let dy = block - (row as f32 + 0.5) - centre[1];
        for column in first..last {
            let dx = column as f32 + 0.5 - centre[0];
            let alpha = coverage(dx.hypot(dy), radius);
            if alpha <= 0.0 {
                continue;
            }
            let x = i64::from(origin[0]) + column;
            let y = i64::from(origin[1]) + row;
            let Ok(texel) = usize::try_from(y * i64::from(resolution) + x) else {
                continue;
            };
            let Some(depth) = thickness.get_mut(texel) else {
                continue;
            };
            // Thickness accumulates, as the relief's `Mass` does: two blades
            // crossing are thicker than one, and the derived normal is what
            // reads that.
            *depth += alpha;
            weight[texel] += alpha;
            for (sum, lane) in colour[texel].iter_mut().zip(tint) {
                *sum += alpha * lane;
            }
        }
    }
}

/// The normal field, from the gradient of the accumulated thickness.
///
/// A central difference, clamped at the edges rather than wrapped: an atlas is
/// a set of separate pictures and does not tile, which is the one place this
/// crate's usual rule does not hold.
fn normals(thickness: &[f32], resolution: u32) -> Vec<[f32; 3]> {
    let size = resolution.max(1) as usize;
    let at = |x: usize, y: usize| thickness.get(y * size + x).copied().unwrap_or(0.0);
    let mut out = Vec::with_capacity(thickness.len());
    for y in 0..size {
        for x in 0..size {
            let left = at(x.saturating_sub(1), y);
            let right = at((x + 1).min(size - 1), y);
            // Rows run down and the card's `y` runs up, so the vertical
            // difference is taken the other way round from the horizontal one.
            let below = at(x, (y + 1).min(size - 1));
            let above = at(x, y.saturating_sub(1));
            let normal = glam::Vec3::new(
                -(right - left) * NORMAL_SLOPE,
                -(below - above) * NORMAL_SLOPE,
                1.0,
            )
            .normalize_or(glam::Vec3::Z);
            out.push(normal.to_array());
        }
    }
    out
}

/// How much of a texel a disc of `radius` centred `distance` away covers.
///
/// The relief splat's own coverage exactly, and deliberately the same
/// function: a card and a splat are two views of one strand, and two
/// antialiasing rules would make the silhouette change at the switch between
/// them.
fn coverage(distance: f32, radius: f32) -> f32 {
    let alpha = (radius + FEATHER_TEXELS - distance) / (2.0 * FEATHER_TEXELS);
    let scale = (radius / FEATHER_TEXELS).min(1.0);
    (alpha * scale).clamp(0.0, 1.0)
}

/// `part / whole` over counts an `f32` holds exactly.
fn ratio(part: usize, whole: usize) -> f32 {
    let float = |value: usize| f32::from(u16::try_from(value).unwrap_or(u16::MAX));
    if whole == 0 {
        return 0.0;
    }
    #[expect(
        clippy::cast_precision_loss,
        reason = "a texel count is bounded by MAX_RESOLUTION squared, which is \
                  under the f32 integer limit"
    )]
    if whole > usize::from(u16::MAX) {
        return part as f32 / whole as f32;
    }
    float(part) / float(whole)
}

/// Between two numbers, by `t`.
fn lerp(from: f32, to: f32, t: f32) -> f32 {
    from + (to - from) * t
}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::float_cmp,
        reason = "a variant's rectangle and an empty texel are exact numbers \
                  this file computes; a margin would pass an atlas that had \
                  quietly stopped laying its variants out at all"
    )]
    use super::*;
    use crate::Strand;

    /// A set of `count` identical leaning blades, differing only in the free
    /// hash the variants and the spread are taken from.
    fn set(count: u32) -> StrandSet {
        let strands = (0..count)
            .map(|index| Strand {
                root: [0.5, 0.5],
                rank: ratio(index as usize, count as usize),
                phase: ratio(index as usize, count as usize),
                length: 0.12,
                width: 0.005,
                direction: [1.0, 0.0],
                lean: 0.35,
                bend: 0.30,
                root_color: [0.05, 0.10, 0.01],
                tip_color: [0.30, 0.45, 0.05],
                roughness: 0.9,
                ..Default::default()
            })
            .collect();
        StrandSet::new(
            "blades".to_owned(),
            strands,
            [32, 32],
            crate::StrandShape::default(),
        )
    }

    fn request(set: &StrandSet, variants: u32) -> CardRequest<'_> {
        CardRequest {
            set,
            keep: 1.0,
            variants,
            resolution: 128,
            metres: 0.14,
            strands_per_card: 16,
        }
    }

    #[test]
    fn an_atlas_is_the_size_it_was_asked_for_and_the_same_atlas_twice() {
        let set = set(256);
        let atlas = cards(&request(&set, 2));
        assert_eq!(atlas.resolution, 128);
        assert_eq!(atlas.base_color.len(), 128 * 128);
        assert_eq!(atlas.normal.len(), 128 * 128);
        assert_eq!(atlas, cards(&request(&set, 2)), "a card is a pure function");
    }

    #[test]
    fn every_variant_draws_something_and_none_of_them_fills_its_block() {
        // The two halves of what a card has to be. Nothing drawn is a hole in
        // the lawn at distance; a full block is a green rectangle, which is
        // what a card of too many strands becomes.
        let set = set(512);
        let atlas = cards(&request(&set, 2));
        let side = atlas.resolution / atlas.variants;
        for variant in 0..atlas.variants * atlas.variants {
            let origin = [
                (variant % atlas.variants) * side,
                (variant / atlas.variants) * side,
            ];
            let mut covered = 0;
            for row in 0..side {
                for column in 0..side {
                    let texel = (origin[1] + row) * atlas.resolution + origin[0] + column;
                    if atlas.base_color[texel as usize][3] >= 0.5 {
                        covered += 1;
                    }
                }
            }
            let block = (side * side) as usize;
            assert!(covered > block / 200, "variant {variant} drew {covered}");
            assert!(covered < block / 2, "variant {variant} filled {covered}");
        }
    }

    #[test]
    fn a_card_is_drawn_from_the_ground_up_and_leaves_the_sky_empty() {
        // A blade grows out of the bottom of its card. The top row of a variant
        // is a blade's tip at most and usually nothing; the bottom row is where
        // every root is.
        let set = set(512);
        let atlas = cards(&request(&set, 1));
        let alpha = |row: u32| {
            (0..atlas.resolution)
                .map(|column| atlas.base_color[(row * atlas.resolution + column) as usize][3])
                .sum::<f32>()
        };
        assert!(
            alpha(atlas.resolution - 1) > alpha(0),
            "{} at the ground against {} at the sky",
            alpha(atlas.resolution - 1),
            alpha(0)
        );
        assert_eq!(alpha(0), 0.0, "a blade this long does not reach the top");
    }

    #[test]
    fn the_variants_are_disjoint_tufts_and_their_uv_rectangles_tile_the_atlas() {
        let set = set(512);
        let atlas = cards(&request(&set, 2));
        assert_eq!(atlas.uv_of(0), [0.0, 0.0, 0.5, 0.5]);
        assert_eq!(atlas.uv_of(3), [0.5, 0.5, 1.0, 1.0]);
        // Past the last one wraps, so a caller keying on a clump hash needs no
        // modulo of its own.
        assert_eq!(atlas.uv_of(4), atlas.uv_of(0));
        // And one card of the same set is not four: a variant is a different
        // handful of strands rather than the same one scaled.
        let single = cards(&request(&set, 1));
        assert_ne!(single.base_color.len(), 0);
        assert!(single.coverage(0.5) > atlas.coverage(0.5) / 2.0);
    }

    #[test]
    fn the_normal_leans_away_from_the_thick_parts_and_is_flat_where_nothing_is() {
        let set = set(512);
        let atlas = cards(&request(&set, 1));
        // Every normal is a unit vector facing out of the card.
        for normal in &atlas.normal {
            let length = glam::Vec3::from_array(*normal).length();
            assert!((length - 1.0).abs() < 1e-4, "{normal:?} is not unit");
            assert!(normal[2] > 0.0, "{normal:?} faces into the card");
        }
        // The sky is flat, because there is nothing there to have a gradient.
        assert_eq!(atlas.normal[0], [0.0, 0.0, 1.0]);
        // And somewhere in the tuft is not.
        assert!(
            atlas
                .normal
                .iter()
                .any(|n| n[0].abs() > 0.05 || n[1].abs() > 0.05),
            "a card of blades has no relief in it at all"
        );
    }

    #[test]
    fn coverage_is_what_a_mip_chain_has_to_preserve() {
        let set = set(512);
        let atlas = cards(&request(&set, 2));
        let at_half = atlas.coverage(0.5);
        assert!(at_half > 0.0 && at_half < 1.0, "{at_half}");
        // A higher cutoff keeps less, which is the whole reason a box-filtered
        // mip thins a cutout: halving an edge texel's alpha moves it under the
        // line.
        assert!(atlas.coverage(0.9) <= at_half);
        assert!(atlas.coverage(0.1) >= at_half);
    }
}
