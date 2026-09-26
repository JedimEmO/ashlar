//! The mip chain: the same surface, read from further away.
//!
//! A texture set that ships level 0 alone is a texture set that shimmers. The
//! hardware picks a level per pixel and the one it finds is the full-resolution
//! map point-sampled, so a wall at distance draws one texel out of every sixty
//! four and draws a different one when the camera moves a hair. What this
//! module does is answer the question the hardware is really asking — *what is
//! the average of this patch of surface* — once, on the CPU, over the `f32`
//! [`Planes`] before anything is quantised.
//!
//! # A box filter, and why the seam survives it
//!
//! Every level is the 2x2 box filter of the level above it, down to 1x1. Box rather than something fancier because the levels have to agree: a
//! wider kernel reads texels the next level over reads again, and the hardware
//! blends two levels expecting each to be the plain average of its own
//! footprint.
//!
//! A wrapped filter is what the design asks for, and the honest form of it at
//! these sizes is *no wrap at all*. A bake is a power of two, so every level
//! above the last is even, and an even plane's 2x2 groups partition it exactly:
//! the first output texel is the first four texels, the last is the last four,
//! nothing reads a neighbour across the seam, and the seam between the last
//! output texel and the first is the seam the level above already had. That is
//! the whole reason the chain of a plane that tiles still tiles, and it is why
//! this filter has no edge case to get wrong — a kernel wider than its stride
//! would need one, and would be the thing that grew a hairline down every wall
//! at distance.
//!
//! An odd plane — nothing a bake makes, but [`Planes`] is plain public data —
//! halves by flooring, so its last row and column do not reach the chain. There
//! is no answer for them that is both a box filter and the same size as its
//! neighbours, and inventing one would be a filter that behaves differently
//! depending on a size this crate does not produce.
//!
//! Re-evaluating the graph at half resolution instead would be sharper and
//! wrong: each level would be a different picture of the surface rather than a
//! blurrier one, and moving between them is what the eye reads as crawling.
//!
//! # Normals and roughness are not box filtered
//!
//! Two of the planes lose their meaning under a plain average.
//!
//! The average of four unit normals is not a unit normal, and what a renderer
//! wants at the lower level is the *direction* of that average. So the chain
//! carries the un-normalised mean of the level-0 normals — filtering the mean
//! again at every level is exactly averaging the level-0 vectors over the whole
//! footprint — and each level ships that mean renormalised.
//!
//! Its length is the other half of the answer, and throwing it away is how a
//! bumpy surface turns to glass at distance: four normals pointing four ways
//! average to a short vector, renormalising restores the direction and loses
//! the spread, and the spread was the reason the surface was not shiny. So the
//! length feeds the roughness, by the design's Toksvig widening
//! `r' = sqrt(r^2 + (1 - |n_avg|) * k)` over [`TOKSVIG_K`], clamped to one. The
//! roughness it widens is the box-filtered level-0 roughness rather than the
//! widened roughness of the level above, so the variance the normals lost is
//! counted once rather than compounded down the chain.
//!
//! Everything else — base colour, metallic, occlusion, height, emissive — is
//! box filtered plainly, because a plain average is what each of them means.
//!
//! A *compiled* material reaches the same place by another road. It binds its
//! normal and its roughness as separate images, so nothing here can pair them:
//! [`bound_mips`](crate::bake::bound_mips) writes the coherence into the alpha
//! channel of the bound normal map's chain instead, and the generated fragment
//! applies this formula itself at whatever level the hardware chose. The
//! constant, the shape and the un-widened roughness it widens are the ones
//! above; see [`wgsl`](crate::wgsl).
//!
//! # What it costs
//!
//! A chain is a third again of the level it came from, in bytes and in texels:
//! every level below level 0 is a quarter of the one above, and a quarter plus a
//! sixteenth and so on is a third. Measured on a 1024-squared bake of a graph
//! binding all six outputs, level 0 takes about a tenth of a second and the ten
//! levels under it — filtered and encoded — add about a quarter of that again.
//! The heavier the graph, the smaller that share: the chain's cost is in the
//! texels and not in the expression, which is also why it is single-threaded.
//! An average of four numbers is memory-bound, and a bake has already spent the
//! machine on the pass that made level 0.
//!
//! [`Planes::encode_mips`] makes each level, encodes it and drops it, so a bake
//! holds one level below level 0 at a time rather than the chain;
//! [`Planes::mips`] is the chain itself, for a test or a tool that wants to read
//! the numbers.
//!
//! ```
//! use std::collections::BTreeMap;
//! use std::num::NonZeroUsize;
//!
//! use ashlar_material::{
//!     MaterialGraph, MaterialGraphLibrary, PbrOutput,
//!     bake::{BakeRequest, bake},
//!     mips::levels,
//!     nodes::Noise,
//! };
//!
//! let graph = MaterialGraph::builder("study:grit")
//!     .node("grain", Noise::value().period(16))
//!     .output(PbrOutput::new().roughness("grain").height("grain").normal_strength(0.01))
//!     .into_graph();
//!
//! let textures = bake(&BakeRequest {
//!     graph: &graph,
//!     library: &MaterialGraphLibrary::default(),
//!     params: &BTreeMap::new(),
//!     resolution: 256,
//!     mips: true,
//!     // A doctest is one of many running at once; a content step wants `None`.
//!     threads: NonZeroUsize::new(2),
//! })?;
//!
//! assert_eq!(textures.base_color.mips.len(), levels(256));
//! assert_eq!(textures.base_color.mips[0].len(), 256 * 256 * 4);
//! // The last level is one texel: four bytes of colour, two of height.
//! assert_eq!(textures.base_color.mips[8].len(), 4);
//! assert_eq!(textures.height.unwrap().mips[8].len(), 2);
//! # Ok::<(), ashlar_material::bake::BakeError>(())
//! ```

use crate::bake::{Dither, Planes, TextureSet};

/// How much the roughness widens for the normal variance a level lost.
///
/// The design fixes the shape — `r' = sqrt(r^2 + (1 - |n_avg|) * k)` — and
/// leaves the constant to whoever implements it, so here is the constant and
/// why it is two.
///
/// Toksvig's approximation is that a lobe of normals whose mean has length
/// `p` has a slope variance of about `1 - p`, and the usual way to mip a
/// normal map without losing its glint is to add twice that variance to the
/// squared roughness. Twice is where the two comes from. It is not the same
/// curve, because this crate's roughness plane is *perceptual* roughness —
/// what Bevy's `StandardMaterial` calls roughness, the square root of the GGX
/// alpha — and the variance form is written on alpha. Read on perceptual
/// roughness the widening is gentler than the physical one everywhere below
/// the clamp: a mirror under a slightly wobbly set of normals stays glossier
/// here than a variance-based renderer would draw it, and a surface whose
/// normals have gone entirely incoherent reaches the clamp at `1 - p = 0.5`
/// rather than somewhere past it.
///
/// That trade is deliberate. The failure this pass exists to prevent is a
/// surface that is too shiny at distance, and the constant is the knob: a
/// larger one buys more insurance and costs the glint on a surface that still
/// has one. Two is the value the shape and the physics agree on, and it is the
/// number to change if a material sheet says a wall is still sparkling at
/// forty metres.
pub const TOKSVIG_K: f32 = 2.0;

/// How many levels a chain over this resolution has, level 0 included.
///
/// A chain halves to 1x1, so this is a function of the resolution alone: 256
/// gives nine levels, 1024 gives eleven. A resolution that is not a power of
/// two halves by flooring and lands on 1x1 all the same.
pub fn levels(resolution: u32) -> usize {
    resolution.max(1).ilog2() as usize + 1
}

impl Planes {
    /// This level and every level below it, largest first, ending at 1x1.
    ///
    /// Level `n` is at index `n`, as it is in [`Encoded::mips`](crate::bake::Encoded::mips),
    /// so index 0 is a copy of these planes. That copy is why this is the
    /// reading API and not the baking one: the levels below level 0 come to a
    /// third of it, and the copy makes what a caller then holds — these planes
    /// and a chain that starts with them again — two and a third.
    /// [`Self::encode_mips`] is what a bake calls, and it keeps one level of
    /// `f32` planes at a time.
    pub fn mips(&self) -> Vec<Self> {
        let mut chain = vec![self.clone()];
        walk(self, |level| chain.push(level.clone()));
        chain
    }

    /// Quantise this level and every level below it, into one texture set.
    ///
    /// [`Self::encode`] with the whole chain behind it: the same formats, the
    /// same dither, one entry per level in each map's
    /// [`mips`](crate::bake::Encoded::mips) from largest to smallest. Each
    /// level is filtered, encoded and dropped before the next is made, so what
    /// this holds at once is the bytes it is answering plus one level of `f32`
    /// planes.
    pub fn encode_mips(&self, dither: Dither) -> TextureSet {
        let mut set = self.encode(dither);
        walk(self, |level| append(&mut set, level.encode(dither)));
        set
    }
}

/// Append one encoded level to the set that already holds the levels above it.
///
/// A map that is absent in one level is absent in all of them — every level is
/// filtered from the one above and a box filter does not invent a height — so
/// an `if let` here is the shape of "both or neither" rather than a case that
/// can happen.
fn append(set: &mut TextureSet, level: TextureSet) {
    set.base_color.mips.extend(level.base_color.mips);
    set.normal.mips.extend(level.normal.mips);
    set.orm.mips.extend(level.orm.mips);
    if let (Some(into), Some(from)) = (set.height.as_mut(), level.height) {
        into.mips.extend(from.mips);
    }
    if let (Some(into), Some(from)) = (set.emissive.as_mut(), level.emissive) {
        into.mips.extend(from.mips);
    }
}

/// The mean of the level-0 normals and the un-widened roughness over one
/// level's footprint: what the next level down is filtered from.
///
/// These are carried beside the levels rather than read back off them because
/// what a level *ships* is the answer and not the state. Its normal is
/// renormalised, which is the direction without the spread; its roughness is
/// already widened, and widening a widened roughness again would count the same
/// lost variance at every level below the one that lost it.
struct Means {
    roughness: Vec<f32>,
    normal: Vec<[f32; 3]>,
}

/// The same two planes, borrowed, so that level 0 can be the head of the walk
/// without being copied: its own roughness is un-widened and its own normals
/// are unit vectors, which is exactly what the state means.
struct MeansRef<'a> {
    roughness: &'a [f32],
    normal: &'a [[f32; 3]],
}

/// Filter every level below `level0`, largest first, handing each to `each` as
/// it is made.
///
/// A borrow rather than a value, because the walk has to keep the level to
/// build the next one from: a caller that wants the chain copies what it is
/// handed, and a caller that wants the bytes encodes it and lets it go.
fn walk(level0: &Planes, mut each: impl FnMut(&Planes)) {
    let mut resolution = level0.resolution;
    let mut source: Option<(Planes, Means)> = None;
    while resolution > 1 {
        let (level, means) = match &source {
            Some((planes, means)) => below(
                planes,
                MeansRef {
                    roughness: &means.roughness,
                    normal: &means.normal,
                },
            ),
            None => below(
                level0,
                MeansRef {
                    roughness: &level0.roughness,
                    normal: &level0.normal,
                },
            ),
        };
        resolution = level.resolution;
        each(&level);
        source = Some((level, means));
    }
}

/// One level from the one above it, and the state the level after that needs.
fn below(source: &Planes, means: MeansRef<'_>) -> (Planes, Means) {
    let resolution = source.resolution;
    let means = Means {
        roughness: halve(means.roughness, resolution),
        normal: halve(means.normal, resolution),
    };
    let level = Planes {
        resolution: (resolution / 2).max(1),
        normal_strength: source.normal_strength,
        base_color: halve(&source.base_color, resolution),
        roughness: widened(&means.roughness, &means.normal),
        metallic: halve(&source.metallic, resolution),
        occlusion: halve(&source.occlusion, resolution),
        normal: means.normal.iter().copied().map(unit).collect(),
        height: source.height.as_deref().map(|p| halve(p, resolution)),
        emissive: source.emissive.as_deref().map(|p| halve(p, resolution)),
    };
    (level, means)
}

/// The roughness a level ships: what it inherited, widened by the normal
/// variance its footprint lost, and never past one.
///
/// A roughness that is not a number comes out fully rough rather than not a
/// number: `f32::min` answers the operand that is one, and a texel of `NaN` in
/// a roughness map is a graph to fix, not a reason for the level below it to
/// stop being readable.
fn widened(roughness: &[f32], normal: &[[f32; 3]]) -> Vec<f32> {
    roughness
        .iter()
        .zip(normal)
        .map(|(rough, mean)| {
            let coherence = coherence(*mean);
            rough
                .mul_add(*rough, (1.0 - coherence) * TOKSVIG_K)
                .sqrt()
                .min(1.0)
        })
        .collect()
}

/// How coherent a footprint's normals are: the length of their mean, in
/// `0..=1`.
///
/// One for a footprint whose normals all point the same way, and down towards
/// zero as they disagree — which is exactly the quantity the Toksvig term reads
/// as `|n_avg|`. The clamp is not a formality: the mean of unit vectors cannot
/// be longer than one, but a plane handed in by a caller can hold anything, and
/// a coherence past one would *narrow* the roughness instead of widening it.
///
/// A bake reads it straight off the chain it is filtering. A compiled material
/// cannot — its normal and its roughness are separate images — so
/// [`bound_mips`](crate::bake::bound_mips) writes this number into the alpha
/// channel of the bound normal map's own chain and the generated fragment reads
/// it back there. One definition, two deliveries.
pub(crate) fn coherence(mean: [f32; 3]) -> f32 {
    length(mean).clamp(0.0, 1.0)
}

/// A vector's direction, or flat where it has none. The mean of four normals
/// pointing four ways can be the zero vector, and a `NaN` normal encodes to
/// noise where a flat one encodes to a surface that is merely wrong.
pub(crate) fn unit(vector: [f32; 3]) -> [f32; 3] {
    let length = length(vector);
    if length.is_finite() && length > 0.0 {
        [vector[0] / length, vector[1] / length, vector[2] / length]
    } else {
        [0.0, 0.0, 1.0]
    }
}

fn length(vector: [f32; 3]) -> f32 {
    vector[0]
        .mul_add(
            vector[0],
            vector[1].mul_add(vector[1], vector[2] * vector[2]),
        )
        .sqrt()
}

/// The 2x2 box filter of a square plane.
///
/// `resolution` is the plane's own, and the answer is half of it, floored and
/// never below one: the groups partition an even plane, and an odd one leaves
/// its last row and column out rather than growing an edge case.
///
/// A plane shorter than the resolution it says it is reads zero past its end
/// rather than panicking. Every plane this crate builds is exactly its
/// resolution squared, and the fallback is what keeps the inner loop free of a
/// panic path for the ones a caller builds.
pub(crate) fn halve<T: Texel>(plane: &[T], resolution: u32) -> Vec<T> {
    let width = resolution.max(1) as usize;
    let half = (resolution / 2).max(1) as usize;
    let mut out = Vec::with_capacity(half * half);
    for y in 0..half {
        for x in 0..half {
            let (top, bottom) = (2 * y * width, (2 * y + 1) * width);
            let sum = at(plane, top + 2 * x)
                .add(at(plane, top + 2 * x + 1))
                .add(at(plane, bottom + 2 * x))
                .add(at(plane, bottom + 2 * x + 1));
            out.push(sum.scale(0.25));
        }
    }
    out
}

/// One texel, or the zero texel past the end of a plane.
fn at<T: Texel>(plane: &[T], index: usize) -> T {
    plane.get(index).copied().unwrap_or_default()
}

/// What a plane holds one of per texel: a scalar, or a colour.
///
/// The filter is the same arithmetic either way, and writing it once is what
/// keeps the height's box filter and the base colour's from drifting apart.
pub(crate) trait Texel: Copy + Default {
    /// Texel-wise sum, for accumulating a group.
    fn add(self, other: Self) -> Self;
    /// Texel-wise scale, for dividing a group by its count.
    fn scale(self, by: f32) -> Self;
}

impl Texel for f32 {
    fn add(self, other: Self) -> Self {
        self + other
    }

    fn scale(self, by: f32) -> Self {
        self * by
    }
}

impl Texel for [f32; 3] {
    fn add(self, other: Self) -> Self {
        [self[0] + other[0], self[1] + other[1], self[2] + other[2]]
    }

    fn scale(self, by: f32) -> Self {
        [self[0] * by, self[1] * by, self[2] * by]
    }
}

impl Texel for [f32; 4] {
    fn add(self, other: Self) -> Self {
        [
            self[0] + other[0],
            self[1] + other[1],
            self[2] + other[2],
            self[3] + other[3],
        ]
    }

    fn scale(self, by: f32) -> Self {
        [self[0] * by, self[1] * by, self[2] * by, self[3] * by]
    }
}
