//! Cards: the level past the last band of real strands.
//!
//! [`strands`](crate::strands) draws a layer as geometry and stops at the last
//! [`lod_metres`](ashlar::StrandSettings::lod_metres) distance, where what the
//! camera sees is the relief the same strands were splatted into. This module
//! is the level in between: past the thinnest blades and before the texture is
//! on its own, a tuft is two crossed quads wearing a *picture* of that tuft.
//!
//! Four things are worth knowing before leaning on it.
//!
//! **A card is a tuft, not a patch.** The placed strands of a chunk are
//! gathered by the clump they belong to — [`Strand::clump_id`](ashlar_strands::Strand::clump_id), which every
//! member of one tuft shares — and each group answers one card at the mean of
//! its roots, sized from its own strands. A layer that declares no clump
//! lattice is gathered on a coarse cell instead, chosen so that a card stands
//! for about as many strands as a clump would have held. The card is square
//! because [`cards`](ashlar_strands::cards) draws a square: a crossed
//! quad is the same picture seen from two directions, and a tall thin card seen
//! from the side would be a wide flat one from the other.
//!
//! **The atlas is cached and the quads are not.** One atlas is the same few
//! dozen strands for every wall wearing the material, so it is keyed by
//! [`CardKey`] and shared through a [`CardCache`];
//! where the tufts *are* depends on the mesh under them, so the quads are built
//! per surface beside the strand chunks they take over from.
//!
//! **The mip chain preserves alpha coverage.** A card is drawn as
//! [`AlphaMode::Mask`] and never blended — ADR 0003 puts transparency out of
//! scope and a cutout keeps depth writes, shadows and the prepass working — and
//! a box-filtered mip halves the alpha of every edge texel, so a cutout thins
//! with every level and a card of grass evaporates at exactly the distance it
//! exists for. [`mips`] rescales each level's alpha until
//! [`CardAtlas::coverage`] answers what level zero answered.
//!
//! **A card does not sway.** The strand levels draw through
//! [`StrandWindMaterial`](crate::wind::StrandWindMaterial), whose two generated
//! vertex stages declare position, normal, UV, colour and the wind attribute
//! and no tangent; a card binds a normal map, and a normal map without a
//! tangent frame is a texture nothing reads. So a card is a plain
//! [`StandardMaterial`] and Bevy's own vertex stage draws it. What that gives
//! up is a tip travel of about twelve millimetres at ten metres and beyond,
//! which is a fraction of a pixel.

use std::collections::{BTreeMap, HashSet};

use ashlar_strands::{CardAtlas, CardRequest, PlacedStrand, StrandSet};
use bevy::{
    asset::RenderAssetUsages,
    camera::visibility::VisibilityRange,
    image::{ImageAddressMode, ImageFilterMode, ImageSampler, ImageSamplerDescriptor},
    mesh::{Indices, PrimitiveTopology},
    prelude::*,
    render::render_resource::{Extent3d, TextureDataOrder, TextureDimension, TextureFormat},
};

use crate::strands::{BLADE_TRANSMISSION, CROSSFADE};

/// How many quads one card is built from.
///
/// Two, crossed at a right angle. However the camera stands, one of them is
/// within forty-five degrees of face on, so what the eye reads is between one
/// tuft and one and a half — which is about what a tuft is. Three quads at
/// sixty degrees would steady that further and would also draw between one and
/// three quarters and two tufts' worth of blades for a tuft, which is a lawn
/// that thickens at the switch.
pub const CARD_QUADS: u32 = 2;

/// How many variants across and down the atlas.
///
/// Two, which is four tufts. One is a stamp repeated across the whole lawn and
/// reads as one; four is enough that a field of cards does not, and the atlas
/// is still a quarter of a megabyte.
pub const CARD_VARIANTS: u32 = 2;

/// Texels across the whole atlas.
///
/// A quarter of it — 128 texels — is one variant, which is a good deal more
/// than a card standing ten metres away ever samples. The chain below is what
/// spends that: an atlas drawn at the size it is *nearest* seen at, and then
/// filtered down honestly, is the only way the far end of the band keeps its
/// coverage.
pub const CARD_RESOLUTION: u32 = 256;

/// The alpha a card's cutout is tested against.
///
/// A half, which is where a box filter's own errors are symmetric and where
/// [`mips`] holds the coverage. It is bound on the material as
/// [`AlphaMode::Mask`] and used again when the chain is built, and the two have
/// to be the same number or the chain preserves a coverage nothing draws.
pub const CARD_CUTOFF: f32 = 0.5;

/// How much taller than its longest strand a card is drawn.
///
/// A half again. The atlas spreads a variant's roots across four fifths of its
/// block, so a blade leaning most of its own length lands inside the card
/// rather than being cut off at the seam between two variants. Higher wastes
/// texels on sky; lower cuts the outermost blades along a straight vertical
/// line, which is the one thing in a card that reads as a card.
///
/// It does *not* change how tall the grass on a card looks. The quad is built
/// at the same metres the atlas was drawn at, so the strands in the picture are
/// the size the strands under it were.
pub const CARD_REACH: f32 = 1.5;

/// The most a tuft's own spread may widen its card, as a multiple of
/// [`CARD_REACH`]'s size.
///
/// A card is sized from the strands it stands for — the picture scales with the
/// quad, so a wider tuft is a bigger card of bigger blades — and this is the
/// stop on that. A group that somehow gathered two distant tufts would
/// otherwise answer one quad spanning both, which is the floating billboard
/// every card system is accused of.
const CARD_SPREAD: f32 = 1.5;

/// How far a card sinks into the surface, as a fraction of its own size.
///
/// A fiftieth, so that a card standing on curved ground meets it rather than
/// hovering a millimetre above it. The atlas is drawn from the ground up with
/// the bottom row of a block as the surface, so this is the whole of what keeps
/// the roots buried.
const CARD_SINK: f32 = 0.02;

/// How far the card's normals are tilted from facing out of the card towards
/// standing up, in the card's own tangent frame.
///
/// A canopy is lit from above and a vertical plane is not: a card shaded by its
/// own face normal goes dark under a high key light exactly where the relief
/// beside it is bright, which is the brightness step at the switch that a level
/// of detail exists to avoid. Tilting is done here, in the encoded normal map,
/// rather than by leaning the mesh normal over: the mesh frame stays exactly
/// perpendicular to the quad, so the tangent the normal map is read in is the
/// one it was drawn in.
///
/// Four fifths is about thirty-eight degrees up from the face.
const CARD_TILT: f32 = 0.8;

/// About how many strands one card stands for, where a layer has no clumps to
/// gather by.
///
/// Sixteen, which is what a clump lattice a quarter as fine as the root lattice
/// holds — the shape `benchmark:grass` authors by hand. A layer with no clumps
/// has no tufts to find, so this is what decides how coarse the cell that
/// stands in for one is.
const CARD_STRANDS: u32 = 16;

/// The finest and coarsest lattice a card may be gathered on, per axis.
const CARD_CELLS: std::ops::RangeInclusive<u32> = 1..=1024;

/// How many strands one card's picture is drawn from, at the ends.
///
/// A card drawn from four strands is a sketch and one drawn from ten thousand
/// is a green rectangle. [`CardAtlas`] says the same thing at more length.
const CARDS_PER_VARIANT: std::ops::RangeInclusive<u32> = 4..=64;

/// The smallest a variant's block may become before the chain stops.
///
/// A box filter over a whole atlas blends across the seams between variants,
/// and at four texels a block that is most of what is left. A chain that stops
/// early costs nothing: the last level it wrote is what a sampler minifying
/// past it reads.
const SMALLEST_BLOCK: u32 = 4;

/// How far the per-tuft tint may pull a card away from the layer's mean colour.
///
/// A card's texels already carry the colours of the strands drawn into them, so
/// the vertex colour is a *ratio* — this tuft against the average tuft — and
/// never the tuft's colour outright, which would multiply the colour in twice
/// and turn a lawn to soot at the switch. The clamp is what keeps a layer whose
/// mean is nearly black in one channel from answering a ratio of a thousand.
const TINT_RANGE: std::ops::RangeInclusive<f32> = 0.5..=2.0;

/// What one layer's cards are drawn and built at.
///
/// Derived from the scattered set rather than authored, because every number
/// here is a fact about the strands: how long they are decides how big a card
/// is, and how many of them share a clump decides how many a card stands for.
/// A [`ashlar::StrandSettings`] says *where* cards are drawn and this says what
/// they are.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CardSettings {
    /// How many metres of world one variant covers, on each axis.
    pub metres: f32,
    /// How many variants across and down the atlas.
    pub variants: u32,
    /// Texels across the whole atlas.
    pub resolution: u32,
    /// How many strands are drawn into one variant.
    pub strands_per_card: u32,
    /// The rank below which a strand may appear on a card.
    ///
    /// The whole set. A card is a picture of a tuft and a tuft is all of its
    /// strands; the thinning a level of detail does is to the *geometry*, and a
    /// card is what replaces the geometry rather than the last cut of it.
    pub keep: f32,
    /// The lattice a card is gathered on, per axis.
    ///
    /// Where a layer clumps this is the clump lattice, recovered from how many
    /// distinct clumps the set holds. It is not what the cards are keyed on
    /// there — the clump's own hash is — but it is how wide a tuft may be, and
    /// that is what says whether two strands of one clump are one card or two
    /// pieces of surface that happen to share it. Where a layer does not clump,
    /// this *is* the grouping, and it is coarse enough to gather about
    /// `CARD_STRANDS` strands.
    pub cells: u32,
    /// Whether the layer gathers its strands into clumps at all.
    ///
    /// What decides which of the two groupings above is used. A clumped layer
    /// must not be keyed on the cell as well: the `clump` port moves a root
    /// *after* its clump was chosen, so a tuft straddles two or three cells and
    /// keying on both cuts it into that many cards.
    pub clumped: bool,
    /// The mean of every strand's own root-to-tip midpoint colour, linear.
    ///
    /// What a card's vertex tint is measured *against*: the atlas already
    /// carries the colours of the strands drawn into it, so a tuft darker than
    /// this one is drawn darker by the ratio and a tuft at the mean is drawn as
    /// the atlas has it.
    pub colour: [f32; 3],
}

impl CardSettings {
    /// What one scattered set's cards are drawn at.
    #[must_use]
    #[expect(
        clippy::cast_precision_loss,
        clippy::cast_possible_truncation,
        reason = "a strand count is bounded by the lattice, and a lattice is a \
                  small integer an f32 counts exactly"
    )]
    pub fn of(set: &StrandSet) -> Self {
        let strands = set.strands();
        let mut longest = 0.0_f32;
        let mut colour = [0.0_f64; 3];
        let mut clumped = false;
        // The clump lattice is not a field of a set — a strand carries the
        // clump's hash and not the lattice it came off — so it is counted. One
        // pass over sixty-five thousand floats, once per layer, behind the same
        // cache the scatter is behind.
        let mut clumps: HashSet<u32> = HashSet::new();
        for strand in strands {
            longest = longest.max(strand.length);
            for (sum, (root, tip)) in colour
                .iter_mut()
                .zip(strand.root_color.into_iter().zip(strand.tip_color))
            {
                *sum += f64::from(root + tip) * 0.5;
            }
            clumped |= strand.clump_id != 0.0;
            clumps.insert(strand.clump_id.to_bits());
        }
        let population = if clumped {
            clumps.len() as f64
        } else {
            strands.len() as f64 / f64::from(CARD_STRANDS)
        };
        // Rounded to a power of two, because every lattice in a material graph
        // is one: a period has to divide the repeat, and the clump lattice this
        // is trying to land on was authored at 64 or 32 rather than at 61. The
        // count of distinct hashes is a little short of the truth — four
        // thousand hashes over a repeat collide — and without the snap that
        // shortfall lands the grouping *between* two clump cells and cuts every
        // tuft into three.
        let cells = power_of_two(population.sqrt()).clamp(*CARD_CELLS.start(), *CARD_CELLS.end());
        let per_card = (strands.len() / (cells as usize).pow(2).max(1)) as u32;
        let mean = |sum: f64| (sum / strands.len().max(1) as f64) as f32;
        Self {
            // A strand of no length would ask for a card of no size, and a
            // quad of no size is a divide rather than a picture.
            metres: (longest * CARD_REACH).max(1e-3),
            variants: CARD_VARIANTS,
            resolution: CARD_RESOLUTION,
            strands_per_card: per_card.clamp(*CARDS_PER_VARIANT.start(), *CARDS_PER_VARIANT.end()),
            keep: 1.0,
            cells,
            clumped,
            colour: [mean(colour[0]), mean(colour[1]), mean(colour[2])],
        }
    }

    /// The request [`atlas`] draws from this.
    fn request<'a>(&self, set: &'a StrandSet) -> CardRequest<'a> {
        CardRequest {
            set,
            keep: self.keep,
            variants: self.variants,
            resolution: self.resolution,
            metres: self.metres,
            strands_per_card: self.strands_per_card,
        }
    }
}

/// What makes two atlases the same atlas: the set behind it, and every number
/// the picture was drawn at.
///
/// The policy `StrandKey` argues for, continued: the exact values rather than a
/// digest of them, because a collision would not be a slow frame but one lawn
/// wearing a picture of another's blades. The cutoff is in it because the chain
/// is rescaled around it, so two cutoffs are two sets of texels and not two
/// materials over one.
///
/// The set is named by [`StrandSets::origin`](crate::strands::StrandSets::origin)
/// with the layer beside it, and that is the one place the two halves of this
/// crate meet: a game names the asset key of the file it read, and a tool names
/// the graph and the values it scattered from. Neither knows what the other's
/// string looks like and neither has to — what a key has to be is *distinct*,
/// and an asset path is not a graph key.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct CardKey {
    /// Where the set the picture was drawn from came from.
    origin: String,
    /// The layer of it.
    layer: String,
    /// [`CardSettings::metres`], by its bits: `f32` is not [`Eq`] and a key has
    /// to be.
    metres: u32,
    /// [`CardSettings::keep`], by its bits.
    keep: u32,
    /// The alpha the chain preserves the coverage at, by its bits.
    cutoff: u32,
    /// Variants across and down.
    variants: u32,
    /// Texels across.
    resolution: u32,
    /// Strands drawn into one variant.
    strands_per_card: u32,
}

impl CardKey {
    /// The key of one layer's atlas.
    #[must_use]
    pub fn new(origin: &str, layer: &str, settings: &CardSettings, cutoff: f32) -> Self {
        Self {
            origin: origin.to_owned(),
            layer: layer.to_owned(),
            metres: settings.metres.to_bits(),
            keep: settings.keep.to_bits(),
            cutoff: cutoff.to_bits(),
            variants: settings.variants,
            resolution: settings.resolution,
            strands_per_card: settings.strands_per_card,
        }
    }

    /// Where the set this atlas was drawn from came from.
    #[must_use]
    pub fn origin(&self) -> &str {
        &self.origin
    }

    /// Which layer of it.
    #[must_use]
    pub fn layer(&self) -> &str {
        &self.layer
    }
}

/// The atlases drawn so far, by what they were drawn from.
///
/// One atlas is the same few dozen strands for every wall wearing the material,
/// so it is drawn once and shared. It is a cache of its own rather than a
/// compartment of `runtime_bake::BakeCache` because that one
/// belongs to the graph engine and this one does not: a game reads a baked set
/// off disk, draws its cards from it and never compiles a graph. A tool holds
/// both, side by side.
///
/// A [`Resource`] for `runtime_bake::BakeCache`'s reason: a
/// caller that grows strands in a system already holds one, and the cache is
/// the sort of thing there should be exactly one of per app.
#[derive(Resource, Debug, Default)]
pub struct CardCache {
    atlases: std::collections::HashMap<CardKey, CardImages>,
}

impl CardCache {
    /// An empty cache.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// The images of an atlas this cache has already drawn, if it has.
    #[must_use]
    pub fn get(&self, key: &CardKey) -> Option<&CardImages> {
        self.atlases.get(key)
    }

    /// Hand one card atlas to `images` and remember its handles.
    ///
    /// The main-thread half of drawing one: the rasterise and the mip chain are
    /// plain data, and this is the only part that touches the ECS. Re-inserting
    /// a key replaces the entry, which is what a re-scatter of an edited graph
    /// wants; the previous images go when the last handle to them does.
    pub fn insert(
        &mut self,
        key: CardKey,
        textures: CardTextures,
        images: &mut Assets<Image>,
    ) -> CardImages {
        let atlas = CardImages {
            base_color: images.add(textures.base_color),
            normal: images.add(textures.normal),
        };
        self.atlases.insert(key, atlas.clone());
        atlas
    }

    /// How many distinct atlases are held.
    #[must_use]
    pub fn len(&self) -> usize {
        self.atlases.len()
    }

    /// Whether nothing has been drawn yet.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.atlases.is_empty()
    }

    /// Forget every atlas.
    ///
    /// What a caller does when the library behind the keys was reloaded in
    /// place: a key names a set rather than describing one, so the same key can
    /// mean different strands after a reload.
    pub fn clear(&mut self) {
        self.atlases.clear();
    }
}

/// A card atlas as the two maps a cutout material binds, before they are
/// assets.
///
/// The split `runtime_bake::GraphTextures` makes, and for
/// the same reason: the expensive half is a rasterise and a mip chain, and only
/// the handing-over needs `Assets<Image>`.
#[derive(Debug)]
pub struct CardTextures {
    /// sRGB colour with a straight alpha mask.
    pub base_color: Image,
    /// The linear tangent-space normal derived from the thickness the strands
    /// drew.
    pub normal: Image,
}

/// The handles one card atlas contributes, once its images are assets.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CardImages {
    /// sRGB colour with a straight alpha mask.
    pub base_color: Handle<Image>,
    /// Linear tangent-space normal.
    pub normal: Handle<Image>,
}

/// Draw one layer's atlas.
///
/// [`ashlar_strands::cards`] with this crate's own constants bound to
/// it, which is the whole of what this adds: the rasteriser is in the crate
/// that owns the strands, because a card and a splat are two views of one set
/// and neither may be the one that owns it.
#[must_use]
pub fn atlas(set: &StrandSet, settings: &CardSettings) -> CardAtlas {
    ashlar_strands::cards(&settings.request(set))
}

/// One atlas and every level below it, with the coverage of level zero kept.
///
/// The chain a cutout needs and a box filter does not give: halving the alpha
/// of every edge texel moves it under the cutoff, so each level draws a little
/// less than the one above it and a field of cards fades out as the camera
/// pulls back — which is the failure a card exists to prevent. Each level's
/// alpha is therefore scaled until [`CardAtlas::coverage`] answers what level
/// zero answered, which is the standard answer to this and is cheap because the
/// atlas is small.
///
/// The chain stops while a variant's block is still four texels across,
/// because below that a box filter is averaging one tuft into its neighbour.
#[must_use]
pub fn mips(level_zero: &CardAtlas, cutoff: f32) -> Vec<CardAtlas> {
    let target = level_zero.coverage(cutoff);
    let mut levels = vec![level_zero.clone()];
    while let Some(last) = levels.last() {
        let block = last.resolution / last.variants.max(1);
        if last.resolution <= 1 || block <= SMALLEST_BLOCK {
            break;
        }
        let mut next = halve(last);
        preserve(&mut next, target, cutoff);
        levels.push(next);
    }
    levels
}

/// One level, box filtered into the next.
///
/// The colour is averaged *weighted by alpha* and the alpha is averaged plain.
/// A straight-alpha texel that nothing covered carries black, so an unweighted
/// average would drag the edge of every blade towards black as the chain goes
/// down — the classic halo, arriving from the opposite direction.
fn halve(level: &CardAtlas) -> CardAtlas {
    let size = (level.resolution.max(2) / 2) as usize;
    let wide = level.resolution.max(2) as usize;
    let mut base_color = Vec::with_capacity(size * size);
    let mut normal = Vec::with_capacity(size * size);
    for y in 0..size {
        for x in 0..size {
            let corners = [
                2 * y * wide + 2 * x,
                2 * y * wide + 2 * x + 1,
                (2 * y + 1) * wide + 2 * x,
                (2 * y + 1) * wide + 2 * x + 1,
            ];
            let mut colour = [0.0_f32; 3];
            let mut alpha = 0.0_f32;
            let mut facing = Vec3::ZERO;
            for corner in corners {
                let texel = level.base_color.get(corner).copied().unwrap_or([0.0; 4]);
                for (sum, lane) in colour.iter_mut().zip(texel) {
                    *sum += lane * texel[3];
                }
                alpha += texel[3];
                facing +=
                    Vec3::from_array(level.normal.get(corner).copied().unwrap_or([0.0, 0.0, 1.0]));
            }
            let weight = if alpha > 0.0 { alpha } else { 1.0 };
            base_color.push([
                colour[0] / weight,
                colour[1] / weight,
                colour[2] / weight,
                alpha * 0.25,
            ]);
            normal.push(facing.normalize_or(Vec3::Z).to_array());
        }
    }
    CardAtlas {
        resolution: level.resolution.max(2) / 2,
        variants: level.variants,
        base_color,
        normal,
    }
}

/// Scale one level's alpha until it covers what level zero covered.
///
/// A bisection over the scale rather than a formula, because coverage is a step
/// function of it: there is no expression for "the factor at which this many
/// texels cross a half", and twelve halvings of a range from nothing to sixteen
/// land within a thousandth of the one that does.
fn preserve(level: &mut CardAtlas, target: f32, cutoff: f32) {
    if target <= 0.0 || level.base_color.is_empty() {
        return;
    }
    let alpha: Vec<f32> = level.base_color.iter().map(|texel| texel[3]).collect();
    let covered = |scale: f32| {
        let over = alpha
            .iter()
            .filter(|value| (*value * scale).min(1.0) >= cutoff)
            .count();
        ratio(over, alpha.len())
    };
    let (mut low, mut high) = (0.0_f32, 16.0_f32);
    let mut best = 1.0_f32;
    let mut closest = (covered(1.0) - target).abs();
    for _ in 0..12 {
        let middle = f32::midpoint(low, high);
        let coverage = covered(middle);
        let error = (coverage - target).abs();
        if error < closest {
            closest = error;
            best = middle;
        }
        if coverage < target {
            low = middle;
        } else {
            high = middle;
        }
    }
    for (texel, value) in level.base_color.iter_mut().zip(alpha) {
        texel[3] = (value * best).min(1.0);
    }
}

/// A chain of levels as the two images a card material binds.
///
/// The colour is written sRGB-encoded so that the sampler's own decode answers
/// the linear texels the atlas holds, which is what every base colour in this
/// crate does; the alpha is a mask and is written as it is, because a transfer
/// function on a coverage would move the cutoff. The normal is linear, tilted
/// towards standing up on the way out, which `CARD_TILT` sets.
///
/// The sampler clamps rather than repeats. An atlas is a set of separate
/// pictures laid beside each other and a card's UVs never leave its own
/// variant; repeating one would only decide what happens at a seam that is not
/// supposed to be sampled.
#[must_use]
pub fn textures(levels: &[CardAtlas]) -> CardTextures {
    let resolution = levels.first().map_or(1, |level| level.resolution.max(1));
    let colour = levels
        .iter()
        .map(|level| {
            let mut bytes = Vec::with_capacity(level.base_color.len() * 4);
            for texel in &level.base_color {
                for lane in &texel[..3] {
                    bytes.push(byte(linear_to_srgb(*lane)));
                }
                bytes.push(byte(texel[3]));
            }
            bytes
        })
        .collect::<Vec<_>>();
    let normal = levels
        .iter()
        .map(|level| {
            let mut bytes = Vec::with_capacity(level.normal.len() * 4);
            for texel in &level.normal {
                let tilted = (Vec3::from_array(*texel) + Vec3::Y * CARD_TILT).normalize_or(Vec3::Z);
                for axis in tilted.to_array() {
                    bytes.push(byte(axis.mul_add(0.5, 0.5)));
                }
                bytes.push(u8::MAX);
            }
            bytes
        })
        .collect::<Vec<_>>();
    CardTextures {
        base_color: chained(&colour, resolution, TextureFormat::Rgba8UnormSrgb),
        normal: chained(&normal, resolution, TextureFormat::Rgba8Unorm),
    }
}

/// One mip chain as an image with a clamped trilinear sampler.
fn chained(mips: &[Vec<u8>], resolution: u32, format: TextureFormat) -> Image {
    let mut image = Image::new_uninit(
        Extent3d {
            width: resolution,
            height: resolution,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        format,
        RenderAssetUsages::RENDER_WORLD,
    );
    image.texture_descriptor.mip_level_count = u32::try_from(mips.len()).unwrap_or(1).max(1);
    image.data_order = TextureDataOrder::MipMajor;
    image.data = Some(mips.concat());
    image.sampler = ImageSampler::Descriptor(ImageSamplerDescriptor {
        address_mode_u: ImageAddressMode::ClampToEdge,
        address_mode_v: ImageAddressMode::ClampToEdge,
        min_filter: ImageFilterMode::Linear,
        mag_filter: ImageFilterMode::Linear,
        mipmap_filter: ImageFilterMode::Linear,
        anisotropy_clamp: crate::ANISOTROPY,
        ..default()
    });
    image
}

/// The linear-to-sRGB transfer function, as the sRGB specification writes it: a
/// short linear segment near black, and a power curve above it.
///
/// Written out here rather than taken from `ashlar_material::bake`, and that is
/// the split rather than a copy made carelessly: this module is the *game* half
/// now, and the crate that owns the bake is the graph engine a game does not
/// link. The two are the same eight lines because they are the same
/// specification, and a test below holds them against each other — through the
/// dev-dependency, which a game does not compile.
fn linear_to_srgb(value: f32) -> f32 {
    if value <= 0.003_130_8 {
        value * 12.92
    } else {
        1.055 * value.powf(1.0 / 2.4) - 0.055
    }
}

/// A value in `0..=1` as a byte, rounded.
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "the value is clamped to 0..=255 before the cast, so it is exactly a code"
)]
fn byte(value: f32) -> u8 {
    if value.is_nan() {
        return 0;
    }
    (value.clamp(0.0, 1.0) * 255.0).round() as u8
}

/// The material a card level draws through.
///
/// A [`StandardMaterial`] with the atlas in it, and every setting on it is a
/// fact about a cutout.
///
/// The alpha mode is [`AlphaMode::Mask`] and never [`AlphaMode::Blend`]: ADR
/// 0003 puts transparency out of scope, and a masked card still writes depth,
/// still casts the shadow it is asked to and still draws in the prepass. The
/// base colour is white because the colour is the atlas's, with a per-tuft
/// ratio over it as the mesh's vertex colour. Culling is off and the material
/// double sided for the reason a blade's is: a card has no inside, and the
/// crossed quad behind it is seen from behind half the time.
#[must_use]
pub fn material(
    definition: &ashlar::MaterialDefinition,
    atlas: &CardImages,
    roughness: f32,
    cutoff: f32,
) -> StandardMaterial {
    StandardMaterial {
        base_color: Color::WHITE,
        base_color_texture: Some(atlas.base_color.clone()),
        normal_map_texture: Some(atlas.normal.clone()),
        perceptual_roughness: roughness.clamp(0.0, 1.0),
        metallic: 0.0,
        alpha_mode: AlphaMode::Mask(cutoff.clamp(0.0, 1.0)),
        cull_mode: None,
        double_sided: true,
        // The same transmission the strands carry, and it matters more here: a
        // card is the picture of a tuft that the geometry was, so a card lit
        // only from the front is a lawn that changes how it answers the sun at
        // the switch.
        diffuse_transmission: BLADE_TRANSMISSION,
        emissive: LinearRgba::rgb(
            definition.emissive[0],
            definition.emissive[1],
            definition.emissive[2],
        ),
        ..default()
    }
}

/// Where a definition's cards are drawn, or nothing where it asks for none.
///
/// The band from the last [`lod_metres`](ashlar::StrandSettings::lod_metres)
/// distance to [`card_metres`](ashlar::StrandSettings::card_metres), with the
/// same margins the strand levels carry so that the last level's fade out is
/// the card level's fade in. Past the far end there is no entry at all, which
/// is the plan's "nothing but the relief".
///
/// Nothing where the definition names no level-of-detail distance either: a
/// layer that named none is drawn at full detail at every distance, and a card
/// level behind it would draw a picture of a tuft inside the tuft. That pairing
/// is refused by [`ashlar::StrandSettings::check`] before it reaches here.
#[must_use]
pub fn band(settings: &ashlar::StrandSettings) -> Option<VisibilityRange> {
    let cards = settings.card_metres?;
    let last = settings.lod_metres.last().copied()?;
    Some(VisibilityRange {
        start_margin: (last * (1.0 - CROSSFADE))..(last * (1.0 + CROSSFADE)),
        end_margin: (cards * (1.0 - CROSSFADE))..(cards * (1.0 + CROSSFADE)),
        // The chunk's own origin, as every strand level is measured from: two
        // levels crossfading have to be measured from the same point, and the
        // card level is the last of them.
        use_aabb: false,
    })
}

/// One chunk's cards, as a mesh.
#[derive(Debug)]
pub struct CardMesh {
    /// The geometry, ready for `Assets<Mesh>`.
    pub mesh: Mesh,
    /// How many cards it holds, which is how many tufts were gathered.
    pub cards: usize,
    /// How many triangles that came to.
    pub triangles: usize,
}

/// Build the cards of one chunk's placed strands.
///
/// `cell_metres` is how much world one gather cell covers — the material's own
/// repeat in metres divided by [`CardSettings::cells`] — which is what says how
/// far apart two strands of one tuft may be. It is the surface's number rather
/// than the layer's, which is why it is an argument here and not a field of the
/// settings the atlas is keyed by: the same picture serves a wall tiled every
/// two metres and one tiled every half.
///
/// `origin` is the point the chunk's levels are drawn about, as
/// [`StrandChunk::origin`](crate::strands::StrandChunk::origin) carries it: the
/// mesh is written relative to it so that Bevy measures the card band from the
/// same patch of lawn the strand levels were measured from, which is what makes
/// the crossfade between the two exact.
#[must_use]
pub fn mesh(
    placed: &[PlacedStrand],
    settings: &CardSettings,
    cell_metres: f32,
    origin: [f32; 3],
) -> CardMesh {
    let tufts = gather(placed, settings, cell_metres);
    let quads = tufts.len() * CARD_QUADS.max(1) as usize;
    let mut positions = Vec::with_capacity(quads * 4);
    let mut normals = Vec::with_capacity(quads * 4);
    let mut tangents = Vec::with_capacity(quads * 4);
    let mut uvs = Vec::with_capacity(quads * 4);
    let mut colors = Vec::with_capacity(quads * 4);
    let mut indices = Vec::with_capacity(quads * 6);
    let centre = Vec3::from_array(origin);
    for (variant, members) in &tufts {
        let tuft = Tuft::of(placed, members, settings);
        let rect = uv_of(settings.variants, *variant);
        let tint = tuft.tint(settings);
        // Where the crossed pair is turned to about the surface normal. Per
        // card, from its own hash, and it is not a nicety: every card of a
        // layer otherwise stands in the *same* two planes, so a lawn seen from
        // anywhere near along one of them shows its cards as a regular grid of
        // slivers on the clump lattice. A turn per tuft leaves the same number
        // of quads facing the same way on average and no lattice in them.
        let turned = f32::from(u16::try_from(*variant >> 16).unwrap_or(0)) / f32::from(u16::MAX);
        for quad in 0..CARD_QUADS.max(1) {
            let turn =
                (ratio(quad as usize, CARD_QUADS.max(1) as usize) + turned) * std::f32::consts::PI;
            let (sine, cosine) = turn.sin_cos();
            let across = (tuft.tangent * cosine + tuft.bitangent * sine).normalize_or(tuft.tangent);
            // `across × up` is the direction the card faces, which makes
            // (across, up, face) the frame the atlas was drawn in: `+x` across,
            // `+y` up, `+z` out of the card. The tangent handedness below is
            // what hands that frame to the normal map unchanged.
            let face = across.cross(tuft.up).normalize_or(Vec3::Z);
            let base = tuft.root - centre - tuft.up * (tuft.side * CARD_SINK);
            let half = across * (tuft.side * 0.5);
            let up = tuft.up * tuft.side;
            let first = u32::try_from(positions.len()).unwrap_or(u32::MAX);
            for (corner, uv) in [
                (base - half, [rect[0], rect[3]]),
                (base + half, [rect[2], rect[3]]),
                (base - half + up, [rect[0], rect[1]]),
                (base + half + up, [rect[2], rect[1]]),
            ] {
                positions.push(corner.to_array());
                normals.push(face.to_array());
                // `w` of one, so that Bevy's own bitangent — the cross of the
                // normal and the tangent — is the card's up. The atlas's `+y`
                // is up, so that is the frame it has to be read in.
                tangents.push([across.x, across.y, across.z, 1.0]);
                uvs.push(uv);
                colors.push([tint[0], tint[1], tint[2], 1.0]);
            }
            indices.extend_from_slice(&[
                first,
                first + 1,
                first + 2,
                first + 1,
                first + 3,
                first + 2,
            ]);
        }
    }
    let triangles = indices.len() / 3;
    CardMesh {
        mesh: Mesh::new(
            PrimitiveTopology::TriangleList,
            RenderAssetUsages::default(),
        )
        .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, positions)
        .with_inserted_attribute(Mesh::ATTRIBUTE_NORMAL, normals)
        .with_inserted_attribute(Mesh::ATTRIBUTE_TANGENT, tangents)
        .with_inserted_attribute(Mesh::ATTRIBUTE_UV_0, uvs)
        .with_inserted_attribute(Mesh::ATTRIBUTE_COLOR, colors)
        .with_inserted_indices(Indices::U32(indices)),
        cards: tufts.len(),
        triangles,
    }
}

/// Which strands belong to which card, in a deterministic order.
///
/// Two questions, asked in that order.
///
/// **Which tuft is it.** Where a layer clumps, the key is the clump's own hash,
/// which is what a tuft *is*: every member of one carries it. It is only a
/// hash, and four thousand of them over a repeat collide often enough to
/// matter, but a collision is between two clumps somewhere else on the
/// material and the split below is what takes those apart. The cell a root fell
/// in is deliberately *not* in the key, because the
/// `clump` port moves a root after its
/// clump was chosen and a tuft therefore straddles two or three cells. Where a
/// layer declares no clumps every hash is zero, and then the cell is the whole
/// of the grouping — the coarse one [`CardSettings::cells`] describes.
///
/// **Which piece of surface it is on.** A set is a list over one repeat and a
/// surface may reach that repeat several times — the two faces of a slab and
/// all six faces of a box are the ordinary cases — so one clump of one cell can
/// be two tufts a hand's breadth apart, and a card at the mean of those would
/// be a quad standing in the air between them with no normal to speak of. So
/// the members of a key are split again by where they are and which way the
/// surface faces: a strand joins the first group whose first member stands on
/// the same side of the surface and within one cell's diagonal of it, and
/// starts a new one otherwise. One cell is the widest a tuft can be, so this
/// never cuts one up; the two faces of a slab disagree about the normal
/// whatever their spacing, which is the case a distance alone would miss on
/// anything thin.
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "a root is in 0..1 and the lattice is bounded by CARD_CELLS"
)]
fn gather(
    placed: &[PlacedStrand],
    settings: &CardSettings,
    cell_metres: f32,
) -> Vec<(u32, Vec<usize>)> {
    let cells = settings.cells.max(1);
    let lattice = f32::from(u16::try_from(cells).unwrap_or(1));
    // The diagonal of one cell, which is the furthest two members of one tuft
    // can be from each other on a flat piece of surface.
    let apart =
        (cell_metres.max(settings.metres) * std::f32::consts::SQRT_2).max(f32::MIN_POSITIVE);
    let mut tufts: BTreeMap<(u32, u32, u32), Vec<(Vec3, Vec3, Vec<usize>)>> = BTreeMap::new();
    for (index, strand) in placed.iter().enumerate() {
        let cell = |axis: f32| {
            if settings.clumped {
                return 0;
            }
            ((axis.clamp(0.0, 1.0) * lattice) as u32).min(cells - 1)
        };
        let key = (
            strand.strand.clump_id.to_bits(),
            cell(strand.strand.root[0]),
            cell(strand.strand.root[1]),
        );
        let here = Vec3::from_array(strand.position);
        let facing = Vec3::from_array(strand.normal);
        let pieces = tufts.entry(key).or_default();
        match pieces.iter_mut().find(|(first, normal, _)| {
            normal.dot(facing) > 0.0 && first.distance_squared(here) <= apart * apart
        }) {
            Some((_, _, members)) => members.push(index),
            None => pieces.push((here, facing, vec![index])),
        }
    }
    tufts
        .into_iter()
        .flat_map(|(key, pieces)| {
            pieces
                .into_iter()
                .enumerate()
                .map(move |(piece, (_, _, members))| {
                    (variant_of(key, u32::try_from(piece).unwrap_or(0)), members)
                })
        })
        .collect()
}

/// Which variant of the atlas one tuft wears.
///
/// A hash of the key and which piece of surface it fell on, so that
/// neighbouring tufts do not share a picture and the same tuft wears the same
/// one every run. [`CardAtlas::uv_of`] wraps an index past the last variant, so
/// nothing here needs a modulo.
fn variant_of(key: (u32, u32, u32), piece: u32) -> u32 {
    let mut hash = 0x811c_9dc5_u32;
    for word in [key.0, key.1, key.2, piece] {
        for byte in word.to_le_bytes() {
            hash = (hash ^ u32::from(byte)).wrapping_mul(0x0100_0193);
        }
    }
    hash
}

/// One variant's rectangle, asked of the atlas's own answer.
///
/// An empty [`CardAtlas`] rather than the one in the cache, because a card mesh
/// is built from the settings and the texels are a handle by then — and asking
/// the type that lays the variants out is what keeps the quad and the picture
/// on the same rectangle.
fn uv_of(variants: u32, variant: u32) -> [f32; 4] {
    CardAtlas {
        resolution: 0,
        variants,
        base_color: Vec::new(),
        normal: Vec::new(),
    }
    .uv_of(variant)
}

/// One gathered tuft, as the card that stands for it.
struct Tuft {
    /// The mean of its roots, in the surface's own coordinates.
    root: Vec3,
    /// The surface normal there, which is the card's own up.
    up: Vec3,
    /// Which way `+u` runs, made perpendicular to [`Self::up`].
    tangent: Vec3,
    /// The third axis of the frame.
    bitangent: Vec3,
    /// How wide and how tall the card is, in metres. A card is square.
    side: f32,
    /// The mean of its strands' midpoint colours, linear.
    colour: [f32; 3],
}

impl Tuft {
    /// The card one group of placed strands comes to.
    #[expect(
        clippy::cast_precision_loss,
        reason = "a tuft holds a few dozen strands, which an f32 counts exactly"
    )]
    fn of(placed: &[PlacedStrand], members: &[usize], settings: &CardSettings) -> Self {
        let count = members.len().max(1) as f32;
        let mut root = Vec3::ZERO;
        let mut up = Vec3::ZERO;
        let mut tangent = Vec3::ZERO;
        let mut colour = [0.0_f32; 3];
        for strand in members.iter().filter_map(|index| placed.get(*index)) {
            root += Vec3::from_array(strand.position);
            up += Vec3::from_array(strand.normal);
            tangent += Vec3::from_array(strand.tangent);
            for (sum, (base, tip)) in colour.iter_mut().zip(
                strand
                    .strand
                    .root_color
                    .into_iter()
                    .zip(strand.strand.tip_color),
            ) {
                *sum += f32::midpoint(base, tip);
            }
        }
        root /= count;
        let up = up.normalize_or(Vec3::Y);
        // Gram-Schmidt against the mean normal, because the mean of a few dozen
        // tangents on curved ground is not perpendicular to the mean of their
        // normals and a card's frame has to be.
        let tangent = (tangent - up * up.dot(tangent)).normalize_or(up.any_orthonormal_vector());
        // How far the tuft's own roots reach, across the surface: the card is
        // at least the size its strands ask for and at most half again that,
        // so a group that gathered more than one tuft cannot answer one quad
        // spanning both.
        let spread = members
            .iter()
            .filter_map(|index| placed.get(*index))
            .map(|strand| (Vec3::from_array(strand.position) - root).length())
            .fold(0.0_f32, f32::max);
        Self {
            root,
            up,
            tangent,
            bitangent: up.cross(tangent).normalize_or(Vec3::Z),
            side: (spread * 2.0).clamp(settings.metres, settings.metres * CARD_SPREAD),
            colour: [colour[0] / count, colour[1] / count, colour[2] / count],
        }
    }

    /// This tuft's colour against the layer's mean, which is what the card's
    /// vertex colour carries.
    ///
    /// A ratio and not a colour. The atlas already holds the colours of the
    /// strands drawn into it, so a tint of the tuft's own colour would multiply
    /// the lawn's colour in twice; what a card has to say is only how this tuft
    /// differs from the average one, which is the whole of what `clump_tint`
    /// varies.
    fn tint(&self, settings: &CardSettings) -> [f32; 3] {
        let lane = |mine: f32, mean: f32| {
            if mean <= f32::MIN_POSITIVE {
                return 1.0;
            }
            (mine / mean).clamp(*TINT_RANGE.start(), *TINT_RANGE.end())
        };
        [
            lane(self.colour[0], settings.colour[0]),
            lane(self.colour[1], settings.colour[1]),
            lane(self.colour[2], settings.colour[2]),
        ]
    }
}

/// The power of two nearest a count, in the ratio sense: 48 is nearer 64 than
/// 32, because a lattice twice as fine is as wrong as one twice as coarse.
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "the exponent is bounded by the clamp its caller applies"
)]
fn power_of_two(count: f64) -> u32 {
    if count <= 1.0 || count.is_nan() {
        return 1;
    }
    1_u32 << (count.log2().round() as u32).min(30)
}

/// `part / whole` over counts an `f32` holds exactly.
fn ratio(part: usize, whole: usize) -> f32 {
    if whole == 0 {
        return 0.0;
    }
    #[expect(
        clippy::cast_precision_loss,
        reason = "a texel count is bounded by CARD_RESOLUTION squared, which is \
                  under the f32 integer limit"
    )]
    if whole > usize::from(u16::MAX) {
        return part as f32 / whole as f32;
    }
    f32::from(u16::try_from(part).unwrap_or(u16::MAX))
        / f32::from(u16::try_from(whole).unwrap_or(u16::MAX))
}

#[cfg(test)]
mod tests {
    use std::num::NonZeroUsize;

    use ashlar_material::{
        MaterialGraph, MaterialGraphLibrary, PbrOutput, StrandLayer,
        nodes::Noise,
        strands::{StrandRequest, scatter},
    };

    use super::*;

    /// A lawn of `count` cells gathered into `clumps`, scattered for real: the
    /// numbers a card is built from are derived from a set, so a test that made
    /// one up would be testing its own arithmetic.
    fn lawn(count: u32, clumps: u32, length: f32) -> StrandSet {
        let graph = MaterialGraph::builder("test:lawn")
            .node("patches", Noise::value().period(4))
            .output(PbrOutput::new().roughness("patches"))
            .strands(
                "blades",
                StrandLayer::new()
                    .count(count)
                    .density(1.0)
                    .lean(0.4)
                    .direction([1.0, 0.0])
                    .colors([0.05, 0.12, 0.01], [0.3, 0.45, 0.05])
                    .clumps(clumps, 0.6, 0.4)
                    .clump_tint(0.3)
                    .metres(length, 0.004),
            )
            .into_graph();
        scatter(&StrandRequest {
            graph: &graph,
            library: &MaterialGraphLibrary::default(),
            params: &BTreeMap::default(),
            layer: "blades",
            field_resolution: 256,
            threads: NonZeroUsize::new(2),
        })
        .expect("the test lawn scatters")
    }

    /// One repeat of ground, one strand planted per entry of the set.
    fn plant(set: &StrandSet) -> Vec<PlacedStrand> {
        ashlar_strands::place(
            set,
            &ashlar_strands::SurfaceTriangles {
                positions: &[[0.0; 3], [1.0, 0.0, 0.0], [1.0, 0.0, 1.0], [0.0, 0.0, 1.0]],
                normals: &[[0.0, 1.0, 0.0]; 4],
                uvs: &[[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]],
                indices: &[0, 1, 2, 0, 2, 3],
            },
            1.0,
        )
    }

    /// How much world one gather cell covers on the square metre above.
    fn cell(cards: &CardSettings) -> f32 {
        1.0 / f32::from(u16::try_from(cards.cells.max(1)).unwrap_or(1))
    }

    fn settings(lod: &[f32], cards: Option<f32>) -> ashlar::StrandSettings {
        let settings = ashlar::StrandSettings::new(["blades"]).lod_metres(lod.iter().copied());
        match cards {
            Some(metres) => settings.card_metres(metres),
            None => settings,
        }
    }

    #[test]
    #[expect(
        clippy::float_cmp,
        reason = "the claim is that two spellings of one curve are the same \
                  function, so an exact comparison is the claim; a tolerance \
                  would pass a copy that had drifted"
    )]
    fn the_srgb_curve_written_out_here_is_the_bakes_own() {
        // `cards` is the game half and cannot depend on the crate that owns
        // `linear_to_srgb`, so the curve is written out twice. This is the one
        // place the two meet — through the dev-dependency, which a game does
        // not compile — and it is an exact comparison because they are the
        // same eight lines rather than two approximations of one curve.
        for step in 0..=256_u32 {
            let value = f32::from(u16::try_from(step).unwrap_or(0)) / 256.0;
            assert_eq!(
                linear_to_srgb(value),
                ashlar_material::bake::linear_to_srgb(value),
                "at {value}"
            );
        }
    }

    #[test]
    fn a_layers_card_settings_are_read_off_its_own_strands() {
        let set = lawn(32, 8, 0.12);
        let cards = CardSettings::of(&set);
        // The card is as big as the longest strand, with the headroom a leaning
        // blade needs to stay inside its own variant.
        let longest = set
            .strands()
            .iter()
            .map(|strand| strand.length)
            .fold(0.0_f32, f32::max);
        assert!(
            (cards.metres - longest * CARD_REACH).abs() < 1e-6,
            "{} against {longest}",
            cards.metres
        );
        // The clump lattice is recovered from how many distinct clumps there
        // are, and a card is one clump.
        assert_eq!(cards.cells, 8);
        assert!(
            cards.strands_per_card >= *CARDS_PER_VARIANT.start(),
            "{cards:?}"
        );
        // And a longer layer is a different atlas, which is the key's job.
        let longer = CardSettings::of(&lawn(32, 8, 0.24));
        assert!(longer.metres > cards.metres);
    }

    #[test]
    fn an_atlas_key_changes_with_every_number_the_picture_was_drawn_at() {
        let origin = "graph:test:lawn@512";
        let base = CardSettings::of(&lawn(32, 8, 0.12));
        let one = CardKey::new(origin, "blades", &base, CARD_CUTOFF);
        assert_eq!(one, CardKey::new(origin, "blades", &base, CARD_CUTOFF));
        assert_eq!((one.origin(), one.layer()), (origin, "blades"));
        // A layer constant moves `metres`, which is in the key.
        assert_ne!(
            one,
            CardKey::new(
                origin,
                "blades",
                &CardSettings::of(&lawn(32, 8, 0.24)),
                CARD_CUTOFF
            )
        );
        for other in [
            CardSettings {
                variants: 4,
                ..base
            },
            CardSettings {
                resolution: 512,
                ..base
            },
            CardSettings {
                strands_per_card: 8,
                ..base
            },
            CardSettings { keep: 0.5, ..base },
        ] {
            assert_ne!(
                one,
                CardKey::new(origin, "blades", &other, CARD_CUTOFF),
                "{other:?}"
            );
        }
        // The cutoff decides how the chain is rescaled, so two cutoffs are two
        // sets of texels.
        assert_ne!(one, CardKey::new(origin, "blades", &base, 0.3));
        // And so are the two halves of what the set *is*: which layer, and
        // where the set came from — a graph one way round and the asset key of
        // a baked file the other.
        assert_ne!(one, CardKey::new(origin, "fibres", &base, CARD_CUTOFF));
        assert_ne!(
            one,
            CardKey::new(
                "materials/library/grass/set.strands",
                "blades",
                &base,
                CARD_CUTOFF
            )
        );
    }

    #[test]
    fn a_card_is_two_crossed_quads_per_tuft_and_the_same_mesh_twice() {
        let set = lawn(32, 8, 0.12);
        let placed = plant(&set);
        let cards = CardSettings::of(&set);
        let built = mesh(&placed, &cards, cell(&cards), [0.0; 3]);
        // One card per tuft, and a tuft is a clump: the lattice is eight cells,
        // so there are sixty-four of them over the repeat.
        assert_eq!(built.cards, 64, "one card per clump");
        assert_eq!(built.triangles, built.cards * CARD_QUADS as usize * 2);
        assert_eq!(
            built.mesh.count_vertices(),
            built.cards * CARD_QUADS as usize * 4
        );
        // Deterministic, because a chunk is rebuilt whenever a surface is and
        // two runs that answered different meshes would flicker.
        let again = mesh(&placed, &cards, cell(&cards), [0.0; 3]);
        assert_eq!(
            built
                .mesh
                .attribute(Mesh::ATTRIBUTE_POSITION)
                .map(bevy::mesh::VertexAttributeValues::len),
            again
                .mesh
                .attribute(Mesh::ATTRIBUTE_POSITION)
                .map(bevy::mesh::VertexAttributeValues::len)
        );
        assert_eq!(positions(&built.mesh), positions(&again.mesh));
    }

    fn positions(mesh: &Mesh) -> Vec<[f32; 3]> {
        match mesh.attribute(Mesh::ATTRIBUTE_POSITION) {
            Some(bevy::mesh::VertexAttributeValues::Float32x3(values)) => values.clone(),
            _ => Vec::new(),
        }
    }

    #[test]
    fn every_card_wears_a_variant_rectangle_of_the_atlas_it_binds() {
        let set = lawn(32, 8, 0.12);
        let cards = CardSettings::of(&set);
        let built = mesh(&plant(&set), &cards, cell(&cards), [0.0; 3]);
        let drawn = atlas(&set, &cards);
        let rectangles: Vec<[f32; 4]> = (0..cards.variants * cards.variants)
            .map(|variant| drawn.uv_of(variant))
            .collect();
        let Some(bevy::mesh::VertexAttributeValues::Float32x2(uvs)) =
            built.mesh.attribute(Mesh::ATTRIBUTE_UV_0)
        else {
            panic!("a card mesh carries UVs");
        };
        for uv in uvs {
            assert!(
                (0.0..=1.0).contains(&uv[0]) && (0.0..=1.0).contains(&uv[1]),
                "{uv:?} is outside the atlas"
            );
            assert!(
                rectangles.iter().any(|rect| {
                    (uv[0] - rect[0]).abs() < 1e-6 || (uv[0] - rect[2]).abs() < 1e-6
                }),
                "{uv:?} is not on a variant's own edge"
            );
        }
        // And every quad's four corners are one rectangle rather than four.
        for quad in uvs.as_chunks::<4>().0 {
            let rect = [
                quad[0][0].min(quad[1][0]),
                quad[2][1].min(quad[0][1]),
                quad[0][0].max(quad[1][0]),
                quad[2][1].max(quad[0][1]),
            ];
            assert!(
                rectangles
                    .iter()
                    .any(|variant| variant.iter().zip(rect).all(|(a, b)| (a - b).abs() < 1e-6)),
                "{quad:?} is not one variant"
            );
        }
    }

    #[test]
    fn two_pieces_of_surface_over_one_repeat_are_two_sets_of_cards() {
        // The case a box projection makes ordinary: both faces of a slab wear
        // the same repeat, so one clump of one cell is two tufts a slab's
        // thickness apart. One card at the mean of those would stand in the air
        // between them, facing nothing.
        let set = lawn(32, 8, 0.12);
        let cards = CardSettings::of(&set);
        let front = plant(&set);
        let slab = 0.5_f32;
        let mut both = front.clone();
        both.extend(front.iter().map(|strand| {
            let mut back = *strand;
            back.position[1] -= slab;
            back.normal = [0.0, -1.0, 0.0];
            back
        }));
        let one = mesh(&front, &cards, cell(&cards), [0.0; 3]);
        let two = mesh(&both, &cards, cell(&cards), [0.0; 3]);
        assert_eq!(two.cards, one.cards * 2, "one set of cards per face");
        // And the second face's cards hang off it rather than standing on the
        // first: its normal points down, so its cards do too.
        let Some(bevy::mesh::VertexAttributeValues::Float32x3(normals)) =
            two.mesh.attribute(Mesh::ATTRIBUTE_NORMAL)
        else {
            panic!("a card mesh carries normals");
        };
        assert!(
            normals.iter().all(|normal| normal[1].abs() < 0.5),
            "a card faces along the surface rather than out of it"
        );
        let points = positions(&two.mesh);
        let low = points.iter().map(|point| point[1]).fold(f32::MAX, f32::min);
        assert!(low < -slab, "{low} is not below the second face");
    }

    #[test]
    fn a_card_stands_on_the_surface_and_is_square() {
        let set = lawn(32, 8, 0.12);
        let cards = CardSettings::of(&set);
        let built = mesh(&plant(&set), &cards, cell(&cards), [0.0; 3]);
        let points = positions(&built.mesh);
        for quad in points.as_chunks::<4>().0 {
            let width = (Vec3::from_array(quad[1]) - Vec3::from_array(quad[0])).length();
            let height = (Vec3::from_array(quad[2]) - Vec3::from_array(quad[0])).length();
            assert!(
                (width - height).abs() < 1e-5,
                "{width} by {height} is not square"
            );
            // The ground is `y = 0` here, and a card sinks into it rather than
            // hovering over it.
            assert!(quad[0][1] <= 0.0, "{:?} floats", quad[0]);
            assert!(quad[2][1] > 0.0, "{:?} does not stand up", quad[2]);
        }
        // Every card is at least the size its strands ask for and never more
        // than the stop allows.
        let side =
            |quad: &[[f32; 3]]| (Vec3::from_array(quad[1]) - Vec3::from_array(quad[0])).length();
        for quad in points.as_chunks::<4>().0 {
            let side = side(quad);
            assert!(
                side >= cards.metres - 1e-6 && side <= cards.metres * CARD_SPREAD + 1e-6,
                "{side} is outside the card's own range"
            );
        }
    }

    #[test]
    fn a_chain_keeps_the_coverage_level_zero_had() {
        let set = lawn(64, 16, 0.12);
        let cards = CardSettings::of(&set);
        let levels = mips(&atlas(&set, &cards), CARD_CUTOFF);
        assert!(levels.len() > 3, "{} levels", levels.len());
        let target = levels[0].coverage(CARD_CUTOFF);
        assert!(target > 0.0 && target < 1.0, "{target}");
        for (level, atlas) in levels.iter().enumerate() {
            assert_eq!(atlas.resolution, cards.resolution >> level);
            let coverage = atlas.coverage(CARD_CUTOFF);
            // A tenth of the coverage itself. A level of sixteen texels a
            // variant cannot hold a finer answer than that, and the failure
            // this pins is a cutout that halves with every level rather than
            // one that wanders by a texel.
            assert!(
                (coverage - target).abs() <= target * 0.1,
                "level {level} covers {coverage} against {target}"
            );
        }
        // The chain stops while a variant is still a picture rather than a
        // texel of its neighbour.
        let last = levels.last().expect("a chain has levels");
        assert!(last.resolution / last.variants >= SMALLEST_BLOCK / 2);
    }

    #[test]
    fn an_atlas_becomes_two_images_with_that_chain_in_them() {
        let set = lawn(32, 8, 0.12);
        let cards = CardSettings::of(&set);
        let levels = mips(&atlas(&set, &cards), CARD_CUTOFF);
        let textures = textures(&levels);
        for image in [&textures.base_color, &textures.normal] {
            assert_eq!(image.texture_descriptor.size.width, cards.resolution);
            assert_eq!(
                image.texture_descriptor.mip_level_count as usize,
                levels.len()
            );
            let bytes: usize = levels
                .iter()
                .map(|level| (level.resolution as usize).pow(2) * 4)
                .sum();
            assert_eq!(image.data.as_ref().map(Vec::len), Some(bytes));
        }
        // The colour is sRGB-encoded so the sampler's decode answers the linear
        // texels the atlas holds; the normal is not, because a transfer
        // function on a direction is not a direction.
        assert_eq!(
            textures.base_color.texture_descriptor.format,
            TextureFormat::Rgba8UnormSrgb
        );
        assert_eq!(
            textures.normal.texture_descriptor.format,
            TextureFormat::Rgba8Unorm
        );
    }

    #[test]
    fn the_card_band_meets_the_last_strand_band_and_ends_the_layer() {
        let with_cards = settings(&[2.5, 7.0], Some(18.0));
        let drawn = band(&with_cards).expect("a card distance is a band");
        let levels = crate::strands::strand_levels(
            &with_cards,
            ashlar_material::strands::StrandShape::default(),
        );
        let last = levels.last().and_then(|level| level.range.clone());
        assert_eq!(
            Some(drawn.start_margin.clone()),
            last.map(|range| range.end_margin),
            "one level's fade out is the card level's fade in"
        );
        assert!(drawn.end_margin.start > drawn.start_margin.end);
        assert!(!drawn.use_aabb, "measured from the chunk's own origin");
        // Past the far end there is nothing but the relief.
        assert!(!drawn.is_visible_at_all(20.0));
        assert!(drawn.is_visible_at_all(10.0));
        // And a definition that asks for no cards has no band at all, which is
        // what every library written before cards existed gets.
        assert!(band(&settings(&[2.5, 7.0], None)).is_none());
        // Nor has one that named no distance for cards to take over from.
        assert!(band(&settings(&[], Some(18.0))).is_none());
    }
}
