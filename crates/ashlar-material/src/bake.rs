//! Graph to texture set: `f32` planes, the normal derived from height, and the
//! encoded bytes a file or an `Image` is made of.
//!
//! A bake is the CPU backend run once per texel over the whole repeat. The
//! graph is lowered for [`Target::Bake`], which folds every parameter into a
//! constant, and the resulting expression is rasterised into plain `f32`
//! planes — [`Planes`] — before anything is quantised. Those planes are the
//! honest intermediate: the mip chain filters them, the normal is derived from
//! one of them, and a test reads them without arguing about rounding.
//!
//! Rows are rasterised across [`std::thread::scope`] threads, one contiguous
//! span each, with no extra dependency and no shared mutable state: a texel
//! reads its own coordinate and nothing else, so the answer does not depend on
//! how the rows were divided. Two bakes of the same request are the same bytes,
//! and so are two bakes under different [`BakeRequest::threads`].
//!
//! # Planes first
//!
//! A buffered filter — a blur, an occlusion, a distance — cannot be a texel's
//! own expression, so the lowering leaves a plan and the bake rasterises it
//! into a [`Plane`] first, in dependency order, filters
//! it, and lets the texel pass sample it. That pass is in
//! [`planes`](crate::planes), and [`rasterise_with`] is how a caller that bakes
//! the same graph again keeps the planes that did not change.
//!
//! # The chain
//!
//! [`BakeRequest::mips`] adds every level below level 0, each the wrapped 2x2
//! box filter of the one above it down to 1x1, with the normals renormalised
//! and the roughness widened for the variance they lost. That pass is in
//! [`mips`](crate::mips), it runs over the same `f32` planes, and it is what
//! keeps a wall from turning to glass at distance. A set without it is a set
//! whose levels the renderer will invent by point sampling.

use std::collections::BTreeMap;
use std::num::NonZeroUsize;
use std::time::Duration;
// `std::time::Instant::now` panics in a browser, where there is no clock the
// standard library can read; `web-time` is the same type over `performance.now`.
#[cfg(not(target_arch = "wasm32"))]
use std::time::Instant;
#[cfg(target_arch = "wasm32")]
use web_time::Instant;

use crate::{
    GraphError, MaterialGraph, MaterialGraphLibrary, ParamValue,
    interp::{EvalError, Inputs, Interpreter, Plane},
    ir::{Ir, Op, Target, ValueId, lower},
    memo::MemoTable,
    planes::{BakeCache, put, rasterise_buffers_memoised, run_jobs, spans, take, texels, unit},
    strip::STRIP,
};

pub use crate::planes::MIN_ROWS_PER_THREAD;

/// The smallest bake a request may ask for.
///
/// Below this a repeat has too few texels for the finest lattice a graph is
/// likely to carry, and the normal's central difference spans a large enough
/// slice of the field to smear it.
pub const MIN_RESOLUTION: u32 = 256;

/// The largest bake a request may ask for, which is also
/// [`MAX_PERIOD`](crate::MAX_PERIOD): past it a lattice cell is under a texel
/// and nothing is resolved that was not already resolved.
pub const MAX_RESOLUTION: u32 = 4096;

/// What a bake is asked for.
///
/// The graph is interchange data rather than a built [`Material`](crate::Material):
/// a bake validates what it was handed, because the parameters in `params`
/// replace the graph's own defaults before validation and a replacement can be
/// the thing that is wrong.
#[derive(Clone, Copy, Debug)]
pub struct BakeRequest<'a> {
    /// The graph to bake.
    pub graph: &'a MaterialGraph,
    /// Where [`Subgraph`](crate::nodes::Subgraph) nodes are resolved from. A
    /// graph that instances nothing may be baked against an empty library.
    pub library: &'a MaterialGraphLibrary,
    /// Parameter values that override the graph's defaults, by name. A name
    /// the graph does not declare, or a value of the wrong type, is an error at
    /// `params[name]` rather than a silent no-op.
    pub params: &'a BTreeMap<String, ParamValue>,
    /// Texels per repeat, in both axes. A power of two in
    /// [`MIN_RESOLUTION`]`..=`[`MAX_RESOLUTION`], and at least as many texels
    /// as the graph's finest lattice has cells.
    ///
    /// That last part is [`BakeError::Lattice`]: a material whose finest
    /// lattice is 4096 baked at 256 gives a sixteenth of a texel per cell, and
    /// what comes out is not a coarser version of the surface but a different
    /// and arbitrary one. It is refused rather than warned about because the
    /// answer would not be *wrong by a little* at any resolution below the
    /// lattice, and because the fix — ask for more texels, or coarsen the
    /// graph — is one an author can always make.
    pub resolution: u32,
    /// Whether to build the mip chain.
    ///
    /// `true` fills every map's [`Encoded::mips`] with each level down to 1x1,
    /// filtered from the `f32` planes as [`mips`](crate::mips) describes; the
    /// levels are a third again of level 0's work and bytes.
    ///
    /// `false` is level 0 alone, which is what a PNG can hold and what the
    /// golden hashes are taken over. It is also a legal KTX2 file, of one
    /// level. It is the wrong answer for anything a
    /// renderer samples at a distance: the hardware does not filter a map that
    /// arrives with one level, it point samples it, and a wall of them crawls
    /// as the camera moves.
    pub mips: bool,
    /// How many threads to rasterise across, or `None` for as many as the work
    /// is worth.
    ///
    /// A bake is the only thing this crate spawns threads for, and one per core
    /// is right when a bake is what the machine is doing. It is wrong twice
    /// over: when several bakes run at once — a test harness, or `ashlar-bevy`
    /// baking a building's materials from a task pool — each would ask for the
    /// whole machine; and when a bake is being made to answer the same bytes
    /// under a different division of the rows, which is what pins that it does.
    ///
    /// So `None` is one thread per core, bounded by
    /// [`MIN_ROWS_PER_THREAD`]: the smallest bake takes eight threads rather
    /// than whatever the machine happens to have, and a bake from a task pool
    /// leaves the pool something to run. A count given here is honoured as
    /// given, so a test can ask for a division of the rows the default would
    /// never choose; either way it is clamped to the resolution, because a
    /// thread with no row is a thread with nothing to do.
    pub threads: Option<NonZeroUsize>,
}

/// Why a bake did not happen.
///
/// Non-exhaustive: a bake that grows a reason to refuse must not break a
/// caller's match, and a caller that cares about one reason — a resolution it
/// can raise, a graph it can report — matches that arm and treats the rest as
/// what they are, which is "this did not bake".
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum BakeError {
    /// The graph did not validate, did not lower, or was handed a parameter it
    /// does not declare. Carries the same path a build error carries.
    #[error("{0}")]
    Graph(#[from] GraphError),
    /// The requested resolution is not a power of two in range.
    #[error("resolution: {0} is not a power of two between {MIN_RESOLUTION} and {MAX_RESOLUTION}")]
    Resolution(u32),
    /// The graph lays a lattice finer than the texels asked for, so a cell
    /// would land inside a texel and the bake would be an arbitrary sample of a
    /// field rather than the field.
    #[error(
        "resolution: {resolution} texels cannot carry a lattice of {lattice:?} cells; bake at \
         the lattice or above, or coarsen the graph"
    )]
    Lattice {
        /// The resolution asked for.
        resolution: u32,
        /// The finest lattice the graph lays, per axis.
        lattice: [u32; 2],
    },
    /// The lowered expression could not be evaluated.
    ///
    /// Not something a graph can cause. A bake folds every parameter, so no
    /// [`Op::Param`] survives to be missing a value, and
    /// it rasterises every plane before the expression that samples it, so no
    /// [`Op::Sample`] is unanswered. What is left is a
    /// plan built out of order — a buffered node lowered to sample a plane that
    /// comes after it — which is this crate being wrong rather than a graph.
    #[error("{0}")]
    Eval(#[from] EvalError),
    /// A backend other than the interpreter could not run this bake.
    ///
    /// The one arm this crate never raises. A caller that hands a
    /// [`Backend`] to a content step is a caller with a device, a driver and a
    /// bind group behind it, and none of those failures has a shape this crate
    /// knows; what it carries is the message that backend gave. The CPU bake is
    /// always the fallback, because the CPU bake is the reference.
    #[error("backend: {0}")]
    Backend(String),
}

/// How a content step bakes, where the step is not the one that chose.
///
/// A [`bake_with_report`] is one of these and so is a GPU bake through
/// `ashlar-bevy`. A step that writes files — the showcase's asset writer, a
/// game's build script — names the graphs, the parameters and the resolutions,
/// and has no business also deciding which backend runs them: that is the
/// caller's, and on the command line it is a flag. So the step takes one of
/// these and calls it, and the crate that has a render device is the crate that
/// supplies one.
pub type Backend<'a> = &'a dyn Fn(&BakeRequest<'_>) -> Result<(TextureSet, BakeReport), BakeError>;

/// The texture format one encoded plane is in.
///
/// The four formats the design ships, and the four Bevy's KTX2 loader accepts
/// without transcoding.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PlaneFormat {
    /// Eight bits per channel, RGBA, with the sRGB transfer function applied.
    /// Base colour and nothing else: a normal or a roughness through a transfer
    /// function is a wrong number.
    Rgba8Srgb,
    /// Eight bits per channel, RGBA, linear.
    Rgba8Unorm,
    /// Sixteen bits, one channel, linear.
    R16Unorm,
    /// Sixteen-bit floating point per channel, RGBA, linear. Emissive, which
    /// is the one output allowed past one.
    Rgba16Float,
}

impl PlaneFormat {
    /// How many bytes one texel occupies, which is what a level's length is a
    /// multiple of.
    pub const fn bytes_per_texel(self) -> usize {
        match self {
            Self::Rgba8Srgb | Self::Rgba8Unorm => 4,
            Self::R16Unorm => 2,
            Self::Rgba16Float => 8,
        }
    }
}

/// One encoded map: a format, and its levels from largest to smallest.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Encoded {
    /// How the bytes are to be read.
    pub format: PlaneFormat,
    /// Level 0 first, down to 1x1 where the request asked for mips and level 0
    /// alone where it did not. [`ktx2::write`](crate::ktx2::write) puts these
    /// on disk in the order and the layout the container asks for.
    pub mips: Vec<Vec<u8>>,
}

/// The maps a material presents to a renderer, encoded.
///
/// The channel contract is `ashlar-bevy`'s, unchanged: sRGB base colour, a
/// linear tangent-space normal, linear `R` occlusion, `G` roughness, `B`
/// metallic, and the two optional maps a graph binds or does not.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TextureSet {
    /// Texels per repeat, in both axes, at level 0.
    pub resolution: u32,
    /// Base colour, sRGB, alpha 255.
    pub base_color: Encoded,
    /// Tangent-space normal, linear, `+Y` along `+V`.
    pub normal: Encoded,
    /// Occlusion, roughness, metallic packed into `R`, `G`, `B`; alpha 255.
    pub orm: Encoded,
    /// Height, linear, present when the graph binds one.
    pub height: Option<Encoded>,
    /// Emissive, linear and unclamped, present when the graph binds one.
    pub emissive: Option<Encoded>,
}

/// The rasterised material before anything is quantised.
///
/// One entry per texel, row-major from the top, so texel `(x, y)` is at
/// `y * resolution + x` and sits at UV `((x + 0.5) / n, (y + 0.5) / n)` — the
/// centre of its cell, not its corner, which is what makes the first and last
/// texel of a row equidistant from the seam.
///
/// The four required planes are always present because [`PbrOutput`](crate::PbrOutput)
/// always binds them; `height` and `emissive` follow the graph. Nothing here is
/// clamped: these are the numbers the graph computed, and the clamping happens
/// once, in [`Self::encode`].
#[derive(Clone, Debug, PartialEq)]
pub struct Planes {
    /// Texels per repeat, in both axes.
    pub resolution: u32,
    /// Metres of relief per unit height *per UV unit*, as the output declared
    /// it. What the normal was derived with.
    ///
    /// [`PbrOutput::new`](crate::PbrOutput::new) defaults it to zero, and zero
    /// is a legal value that validates and bakes, so a graph that binds a
    /// height and forgets the strength writes a perfectly flat normal map
    /// without complaint. That is the one way to get a wrong picture out of a
    /// correct bake, and reading this field off the planes is how a content
    /// step can notice before it writes a file.
    pub normal_strength: f32,
    /// Linear RGB.
    pub base_color: Vec<[f32; 3]>,
    /// Perceptual roughness.
    pub roughness: Vec<f32>,
    /// Metallic.
    pub metallic: Vec<f32>,
    /// Ambient occlusion.
    pub occlusion: Vec<f32>,
    /// The UV-space normal derived from `height`, unit length: `(-dH/du, -dH/dv, 1)`
    /// normalised, with image rows increasing in v. Encoded maps preserve this
    /// convention; an OpenGL/Mikk renderer must flip the green/Y component.
    /// The Bevy adapter does this for generated surfaces. Flat
    /// `[0, 0, 1]` everywhere when the graph binds no height.
    pub normal: Vec<[f32; 3]>,
    /// The height field the normal came from, when the graph binds one.
    pub height: Option<Vec<f32>>,
    /// Linear emissive radiance, which may exceed one.
    pub emissive: Option<Vec<[f32; 3]>>,
}

/// What quantising to eight bits does with the part of a value that does not
/// fit.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Dither {
    /// Spread the error over a 4x4 ordered (Bayer) cell, so a gradient that
    /// crosses less than one code per texel comes out as a gradient rather than
    /// as bands. This is what a bake writes.
    #[default]
    Ordered,
    /// Round each texel on its own. Banding, but a value a test can name.
    None,
}

/// What a bake cost.
///
/// `elapsed` is wall clock over the whole call, which is what a content step
/// waits on and what phase one's capture records.
///
/// Only [`PartialEq`], because [`Self::height_range`] is two `f32`s read off a
/// plane. Two reports of the same bake compare equal all the same: the
/// rasteriser is deterministic, so those are the same two texels every time.
#[derive(Clone, Debug, PartialEq)]
pub struct BakeReport {
    /// Texels per repeat, in both axes.
    pub resolution: u32,
    /// Wall-clock time for the call that produced it.
    pub elapsed: Duration,
    /// Instructions in the lowered expression, after folding, sharing and dead
    /// code removal.
    ///
    /// This is the whole material, and the outputs' share of it is what runs
    /// once per texel. The rest is the sub-expressions the buffered filters
    /// rasterised into planes, each of which ran once per texel of its own
    /// plane instead; [`Self::buffers`] says how many of those there were.
    pub ops: usize,
    /// How many planes the buffered filters needed.
    ///
    /// A plane is a rasterisation of its sub-expression plus a pass of its
    /// filter, so this is the count that says why a bake with one blur in it
    /// costs what it does. Two nodes over the same input at the same settings
    /// are one plane here, and a buffered node no output reads is none.
    pub buffers: usize,
    /// Instructions each bound output reaches, by port name. These sum past
    /// [`Self::ops`] whenever two outputs share a sub-expression, which is the
    /// point of sharing it.
    pub ops_per_output: BTreeMap<String, usize>,
    /// How many levels each map carries: one where the request asked for no
    /// mips, and [`mips::levels`](crate::mips::levels) of the resolution — nine
    /// at 256, eleven at 1024 — where it did.
    ///
    /// A chain is a function of the resolution alone, so this is known before a
    /// level is filtered. [`rasterise`] reports it while answering level 0
    /// only, because it is what encoding those planes would write.
    pub levels: usize,
    /// The lowest and highest texel of the height plane, before anything is
    /// clamped, or `None` where the graph binds no height.
    ///
    /// Reported rather than enforced. A height outside `0..=1` is an authoring
    /// mistake — the encoded map saturates while the normal, derived from the
    /// plane itself, keeps the slope — but refusing it here would mean a graph
    /// that bakes at one resolution and not at another, since the extremes a
    /// lattice reaches depend on where it is sampled. So the bake says what it
    /// saw and lets the step that owns the graph decide.
    pub height_range: Option<(f32, f32)>,
    /// Whether the expression read the mesh's cut flag, which a bake answers
    /// **zero** for.
    ///
    /// A bake writes one texture set and a wall wears it on the face the kernel
    /// cut and on the face it cut into, so there is no second answer to give:
    /// the flag is what a *compiled* material reads off a vertex attribute, and
    /// a bake of the same graph is the uncut picture of it. That is a fact worth
    /// reporting rather than hiding, because a graph authored to darken its own
    /// reveals bakes to a surface with no reveals in it, and the content step
    /// that wrote the file should be able to say so.
    ///
    /// Unlike [`Time`](crate::nodes::Time), which bakes at zero for the same
    /// reason and is not reported: zero seconds is an instant a clock really
    /// passes through, and every bake of a pulsing light is the picture at
    /// startup. Zero is not a face a cut-aware material has two answers for.
    pub cut_flag: bool,
    /// What looked wrong about this bake at *this* resolution.
    ///
    /// A graph's own warnings are [`Material::warnings`](crate::Material) and
    /// are about the graph alone, because a graph is authored once and baked at
    /// whatever resolution a caller asks for. These are the ones that cannot be
    /// made there: a strand relief whose blades are thinner than a texel is a
    /// perfectly good graph and a plane that cannot draw it, and which of those
    /// two it is depends entirely on the number the request carried.
    ///
    /// Reported rather than enforced, exactly as [`Self::height_range`] is: the
    /// content step that owns the graph decides whether to raise the resolution
    /// or widen the blade.
    pub warnings: Vec<crate::GraphWarning>,
}

impl BakeReport {
    /// What a bake cost, from the pieces every backend has.
    ///
    /// The counts come off the lowering and the levels off the resolution, so
    /// the only thing a rasteriser contributes is the planes and the clock. It
    /// is public because a backend other than the interpreter has to be able to
    /// answer the same report a content step already prints — a GPU bake that
    /// said nothing about its op count would be a bake an author could not
    /// compare with the one beside it.
    pub fn of(ir: &Ir, planes: &Planes, mips: bool, elapsed: Duration) -> Self {
        Self {
            resolution: planes.resolution,
            elapsed,
            ops: ir.len(),
            buffers: ir.buffers().len(),
            ops_per_output: ir
                .roots()
                .iter()
                .map(|(port, root)| (port.clone(), reachable(ir, *root)))
                .collect(),
            levels: if mips {
                crate::mips::levels(planes.resolution)
            } else {
                1
            },
            height_range: planes.height.as_deref().map(range),
            cut_flag: ir.insts().iter().any(|inst| inst.op == Op::CutFlag),
            warnings: strand_lattice_warnings(ir, planes.resolution),
        }
    }
}

/// Strand reliefs whose blades are narrower than a texel of this bake.
///
/// The lattice check the plan asks for, made where it can actually be made: a
/// `width_metres` is a length and a resolution is a count, and only a *bake*
/// holds both. A graph authored for 2048 texels over a two-metre repeat draws
/// its two-millimetre blades two texels wide; the same graph previewed at 512
/// draws them at half a texel, and what comes out is a coverage plane that
/// flickers between blades rather than a lawn.
///
/// The threshold is one texel of the blade's *width*, not of its length, and it
/// is the width because that is the axis that runs out first: a blade is twenty
/// times longer than it is wide, so by the time the length is sub-texel there
/// is nothing left to warn about.
///
/// A warning rather than a refusal because the splat still does something
/// reasonable — [`coverage`](crate::planes) scales a sub-texel disc's peak by
/// how wide it really is, so the plane dims rather than dropping out — and
/// because the resolution is the caller's to choose.
#[expect(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "a resolution is bounded by MAX_RESOLUTION, which an f32 counts \
              exactly, and the resolution this suggests is clamped into u16 \
              before its cast"
)]
fn strand_lattice_warnings(ir: &Ir, resolution: u32) -> Vec<crate::GraphWarning> {
    let mut warnings = Vec::new();
    for plan in ir.buffers() {
        let Some(strands) = plan.strands.as_ref() else {
            continue;
        };
        let size = plan.resolution.unwrap_or(resolution).max(1);
        let texel_metres = strands.repeat_metres() / size as f32;
        let width = strands.settings().width_metres;
        if width >= texel_metres || !width.is_finite() {
            continue;
        }
        // What resolution would draw it: one texel per blade over the repeat,
        // rounded up. Held inside `u16` before the cast, because a layer whose
        // blades are a micron wide would otherwise print a number nobody can
        // bake at anyway.
        let wanted = (strands.repeat_metres() / width.max(f32::MIN_POSITIVE))
            .ceil()
            .clamp(1.0, f32::from(u16::MAX)) as u32;
        warnings.push(crate::GraphWarning::new(
            &plan.path,
            format!(
                "strand layer {:?} is {:.2} mm wide and a texel at {size} over a {:.2} m repeat \
                 is {:.2} mm, so the relief cannot draw a blade; bake at {wanted} or wider, or \
                 widen the layer",
                strands.layer(),
                width * 1000.0,
                strands.repeat_metres(),
                texel_metres * 1000.0,
            ),
        ));
    }
    warnings
}

/// Bake a graph into an encoded texture set.
///
/// The steps, in order: fold the request's parameters into the graph and
/// validate it; lower it for [`Target::Bake`]; rasterise every bound output
/// into an `f32` plane with rows across threads; derive the normal from the
/// height plane; and encode, with an ordered dither on the base colour.
///
/// ```
/// use std::collections::BTreeMap;
/// use std::num::NonZeroUsize;
///
/// use ashlar_material::{
///     MaterialGraph, MaterialGraphLibrary, PbrOutput,
///     bake::{BakeRequest, bake},
///     nodes::Noise,
/// };
///
/// let graph = MaterialGraph::builder("study:grit")
///     .node("grain", Noise::value().period(16))
///     .output(
///         PbrOutput::new()
///             .roughness("grain")
///             .height("grain")
///             .normal_strength(0.01),
///     )
///     .into_graph();
///
/// let textures = bake(&BakeRequest {
///     graph: &graph,
///     library: &MaterialGraphLibrary::default(),
///     params: &BTreeMap::new(),
///     resolution: 256,
///     mips: false,
///     // `None` is one thread per core, which is what a content step wants.
///     // A doctest is one of many running at once, so it takes two.
///     threads: NonZeroUsize::new(2),
/// })?;
///
/// assert_eq!(textures.base_color.mips[0].len(), 256 * 256 * 4);
/// // The normal came from the height the graph bound, so both are there.
/// assert!(textures.height.is_some());
/// assert_eq!(textures.normal.mips.len(), 1);
/// # Ok::<(), ashlar_material::bake::BakeError>(())
/// ```
pub fn bake(request: &BakeRequest<'_>) -> Result<TextureSet, BakeError> {
    bake_with_report(request).map(|(set, _)| set)
}

/// [`bake`], and what it cost.
pub fn bake_with_report(request: &BakeRequest<'_>) -> Result<(TextureSet, BakeReport), BakeError> {
    let started = Instant::now();
    let (planes, mut report) = rasterise(request)?;
    let set = if request.mips {
        planes.encode_mips(Dither::Ordered)
    } else {
        planes.encode(Dither::Ordered)
    };
    debug_assert_eq!(set.base_color.mips.len(), report.levels);
    report.elapsed = started.elapsed();
    Ok((set, report))
}

/// Bake a graph as far as the `f32` planes, without quantising anything.
///
/// This is what [`bake`] does first, and what the mip chain will filter. A
/// caller that wants the texels rather than the bytes — a test, a debug view,
/// or a step that encodes differently — reads them here.
pub fn rasterise(request: &BakeRequest<'_>) -> Result<(Planes, BakeReport), BakeError> {
    rasterise_with(request, &mut BakeCache::default())
}

/// [`rasterise`], keeping the planes its buffered filters made.
///
/// A bake makes a cache of its own and drops it, which is right for a content
/// step that bakes each graph once. A caller that bakes the same graph again
/// and again — the preview, re-baking on every turn of a slider — hands one in
/// here and pays for a plane only when the expression under it, its filter or
/// its resolution changed. The bytes are the same either way: a cached plane is
/// the plane that key describes, or it is not in the cache.
///
/// The cache holds whole planes and nothing evicts, so a caller that bakes many
/// different graphs through one cache should
/// [`clear`](crate::planes::BakeCache::clear) it between them.
pub fn rasterise_with(
    request: &BakeRequest<'_>,
    cache: &mut BakeCache,
) -> Result<(Planes, BakeReport), BakeError> {
    let started = Instant::now();
    let resolution = request.resolution;
    let Plan {
        ir,
        normal_strength,
    } = plan(request)?;
    let (buffers, memo) = rasterise_buffers_memoised(&ir, resolution, request.threads, cache)?;
    let planes = rasterise_ir(
        &ir,
        resolution,
        normal_strength,
        request.threads,
        &buffers,
        &memo,
    )?;
    let report = BakeReport::of(&ir, &planes, request.mips, started.elapsed());
    Ok((planes, report))
}

/// What a bake decides before it looks at a texel.
///
/// The lowered expression and the relief its normal is derived with: everything
/// [`rasterise`] needs that is not texels. A backend other than the
/// interpreter starts here — it takes the [`Ir`], rasterises the planes and the
/// outputs its own way, and comes back through [`derive_normal`] and
/// [`Planes::encode_mips`] for the parts that are not per-texel.
#[derive(Clone, Debug, PartialEq)]
pub struct Plan {
    /// The whole material as one expression, with the request's parameters
    /// folded in.
    pub ir: Ir,
    /// [`PbrOutput::normal_strength`](crate::PbrOutput), which the normal is
    /// derived with and which [`Planes`] carries.
    pub normal_strength: f32,
}

/// Validate and lower a request without rasterising a texel.
///
/// Everything a bake can refuse except an unevaluable expression is refused
/// here — the resolution, the mip chain, a parameter the graph does not
/// declare, a graph that does not validate, a node this backend cannot lower —
/// and it costs the lowering rather than the texels. That is what a startup
/// preflight wants: `ashlar-bevy` reads a `Surface::Graph` at load and turns a
/// broken graph into the same kind of startup error a missing PNG already is,
/// without baking a megatexel to find out.
///
/// The [`Ir`] comes back because it is also the answer to "what does this
/// cost": [`Ir::len`] is the op count a report prints.
pub fn preflight(request: &BakeRequest<'_>) -> Result<Ir, BakeError> {
    plan(request).map(|plan| plan.ir)
}

/// Everything a bake decides before it looks at a texel.
///
/// [`preflight`] and what it leaves out: the relief the normal is derived
/// with, which a backend deriving its own normal from a height plane needs and
/// which lives on the graph's output rather than in the expression.
pub fn plan(request: &BakeRequest<'_>) -> Result<Plan, BakeError> {
    require_resolution(request.resolution)?;
    let graph = request.graph.with_params(request.params)?;
    let material = graph.build_in(request.library)?;
    let lattice = material.finest_lattice();
    if lattice[0] > request.resolution || lattice[1] > request.resolution {
        return Err(BakeError::Lattice {
            resolution: request.resolution,
            lattice,
        });
    }
    let normal_strength = material.graph().output.normal_strength;
    Ok(Plan {
        ir: lower(&material, Target::Bake)?,
        normal_strength,
    })
}

fn require_resolution(resolution: u32) -> Result<(), BakeError> {
    if in_range(resolution) {
        Ok(())
    } else {
        Err(BakeError::Resolution(resolution))
    }
}

/// Whether a bake may be asked for at this many texels per repeat.
///
/// Shared with [`partition`](crate::partition), whose bound textures are baked
/// through this same pipeline and so are under the same bounds. One predicate
/// rather than two, because a resolution one of them took and the other refused
/// would be a shader whose static half could not be baked.
pub(crate) fn in_range(resolution: u32) -> bool {
    resolution.is_power_of_two() && (MIN_RESOLUTION..=MAX_RESOLUTION).contains(&resolution)
}

/// The lowest and highest value in a plane, ignoring any `NaN`. A plane with
/// no number in it at all answers `(0, 0)` rather than an inverted range, so a
/// caller never has to defend against a low above a high.
fn range(plane: &[f32]) -> (f32, f32) {
    let (low, high) = plane
        .iter()
        .copied()
        .filter(|v| !v.is_nan())
        .fold((f32::INFINITY, f32::NEG_INFINITY), |(low, high), value| {
            (low.min(value), high.max(value))
        });
    if low <= high { (low, high) } else { (0.0, 0.0) }
}

/// How many instructions one root reaches, which is what that output costs per
/// texel if nothing else were baked with it.
fn reachable(ir: &Ir, root: ValueId) -> usize {
    ir.reaches(&[root]).into_iter().filter(|live| *live).count()
}

/// The roots a plane is written from. Absent only where the graph binds no such
/// output; the four required ones are always bound.
#[derive(Clone, Copy, Debug)]
struct Roots {
    base_color: Option<ValueId>,
    roughness: Option<ValueId>,
    metallic: Option<ValueId>,
    occlusion: Option<ValueId>,
    height: Option<ValueId>,
    emissive: Option<ValueId>,
}

/// One thread's contiguous span of rows, as the slice of every plane it owns.
struct Rows<'a> {
    first: u32,
    base_color: &'a mut [[f32; 3]],
    roughness: &'a mut [f32],
    metallic: &'a mut [f32],
    occlusion: &'a mut [f32],
    height: Option<&'a mut [f32]>,
    emissive: Option<&'a mut [[f32; 3]]>,
}

fn rasterise_ir(
    ir: &Ir,
    resolution: u32,
    normal_strength: f32,
    threads: Option<NonZeroUsize>,
    buffers: &[Plane],
    memo: &MemoTable,
) -> Result<Planes, EvalError> {
    let texels = texels(resolution);
    let roots = Roots {
        base_color: ir.root("base_color"),
        roughness: ir.root("roughness"),
        metallic: ir.root("metallic"),
        occlusion: ir.root("occlusion"),
        height: ir.root("height"),
        emissive: ir.root("emissive"),
    };
    let mut base_color = vec![[0.0_f32; 3]; texels];
    let mut roughness = vec![0.0_f32; texels];
    let mut metallic = vec![0.0_f32; texels];
    let mut occlusion = vec![0.0_f32; texels];
    let mut height = roots.height.map(|_| vec![0.0_f32; texels]);
    let mut emissive = roots.emissive.map(|_| vec![[0.0_f32; 3]; texels]);

    let mut jobs = Vec::new();
    {
        let mut base_rest = base_color.as_mut_slice();
        let mut rough_rest = roughness.as_mut_slice();
        let mut metal_rest = metallic.as_mut_slice();
        let mut occlude_rest = occlusion.as_mut_slice();
        let mut height_rest = height.as_deref_mut();
        let mut emissive_rest = emissive.as_deref_mut();
        for (first, rows) in spans(resolution, threads) {
            let count = texels_in(rows, resolution);
            jobs.push(Rows {
                first,
                base_color: take(&mut base_rest, count),
                roughness: take(&mut rough_rest, count),
                metallic: take(&mut metal_rest, count),
                occlusion: take(&mut occlude_rest, count),
                height: height_rest.as_mut().map(|rest| take(rest, count)),
                emissive: emissive_rest.as_mut().map(|rest| take(rest, count)),
            });
        }
    }

    // The outputs are the last stage of the bake and the biggest reader of
    // what the planes left behind: the SOI cobblestone's height is 1,640
    // instructions of which the occlusion plane already computed every one.
    let bound: Vec<ValueId> = [
        roots.base_color,
        roots.roughness,
        roots.metallic,
        roots.occlusion,
        roots.height,
        roots.emissive,
    ]
    .into_iter()
    .flatten()
    .collect();
    let frontier = memo.frontier(ir, &bound);
    let (bound, frontier) = (bound.as_slice(), frontier.as_slice());

    run_jobs(jobs, |job| {
        rasterise_rows(ir, bound, roots, resolution, buffers, frontier, job)
    })?;

    let normal = derive_normal(height.as_deref(), resolution, normal_strength, threads);
    Ok(Planes {
        resolution,
        normal_strength,
        base_color,
        roughness,
        metallic,
        occlusion,
        normal,
        height,
        emissive,
    })
}

/// Evaluate one span of rows, a strip of texels at a time.
///
/// The walk is flattened and the slot file allocated once for the span,
/// because a bake is a few million texels and the expression is the same one
/// every time. A row is a whole number of strips at every legal resolution —
/// [`MIN_RESOLUTION`] is 256 — so the tail below never runs here; it is there
/// because the loop should not depend on that to be right.
#[expect(
    clippy::cast_precision_loss,
    reason = "a texel coordinate is bounded by MAX_RESOLUTION, well inside what an f32 counts exactly"
)]
fn rasterise_rows(
    ir: &Ir,
    bound: &[ValueId],
    roots: Roots,
    resolution: u32,
    buffers: &[Plane],
    frontier: &[(ValueId, &Plane)],
    job: Rows<'_>,
) -> Result<(), EvalError> {
    let Rows {
        first,
        base_color,
        roughness,
        metallic,
        occlusion,
        mut height,
        mut emissive,
    } = job;
    // Only what the outputs reach, and only down to what the planes left
    // behind: the sub-expressions the buffered filters rasterised are in this
    // arena too, and they were evaluated once per texel of their own plane
    // rather than once per texel of this one.
    let interpreter = Interpreter::for_values_memoised(ir, bound, frontier);
    let plan = interpreter.strip_plan(bound);
    let mut file = plan.file();
    let at = |root: Option<ValueId>| root.and_then(|root| plan.slot(root));
    let (base_at, rough_at, metal_at) = (
        at(roots.base_color),
        at(roots.roughness),
        at(roots.metallic),
    );
    let (occlude_at, height_at, emissive_at) =
        (at(roots.occlusion), at(roots.height), at(roots.emissive));
    let inputs = Inputs {
        buffers,
        ..Inputs::default()
    };
    let size = resolution as f32;
    let rows = texels_in_rows(base_color.len(), resolution);
    let width = resolution as usize;
    let mut uvs = [[0.0_f32; 2]; STRIP];
    for row in 0..rows {
        let v = ((first + row) as f32 + 0.5) / size;
        let mut x = 0;
        while x < width {
            let span = STRIP.min(width - x);
            for (slot, uv) in uvs.iter_mut().take(span).enumerate() {
                *uv = [((x + slot) as f32 + 0.5) / size, v];
            }
            plan.run(
                texels_in(first + row, resolution) + x,
                &uvs[..span],
                &inputs,
                &mut file,
            )?;
            for slot in 0..span {
                let index = texels_in(row, resolution) + x + slot;
                if let Some(at) = base_at {
                    put(base_color, index, file.value(at, slot));
                }
                if let Some(at) = rough_at {
                    put(roughness, index, file.value(at, slot)[0]);
                }
                if let Some(at) = metal_at {
                    put(metallic, index, file.value(at, slot)[0]);
                }
                if let Some(at) = occlude_at {
                    put(occlusion, index, file.value(at, slot)[0]);
                }
                if let (Some(at), Some(plane)) = (height_at, height.as_deref_mut()) {
                    put(plane, index, file.value(at, slot)[0]);
                }
                if let (Some(at), Some(plane)) = (emissive_at, emissive.as_deref_mut()) {
                    put(plane, index, file.value(at, slot));
                }
            }
            x += span;
        }
    }
    Ok(())
}

/// The tangent-space normal of a height field, by wrapped central difference.
///
/// The difference is taken over the two neighbouring texels and divided by the
/// distance between them, so the gradient is in height per UV unit whatever the
/// resolution: a bake at 512 and a bake at 2048 describe the same surface.
/// `strength` is the output's `normal_strength`, the relief a unit of height
/// stands for across one repeat, and scaling the gradient by it is what turns a
/// dimensionless field into a slope.
///
/// That per-UV convention is worth spelling out, because the pixel loop this
/// replaces did it the other way. The showcase computes
/// `normalize(-1.5 * dh, ..., 1)` where `dh` is a raw two-texel difference at
/// 1024, so its 1.5 is per two texels and not per UV unit; the same surface
/// here wants `strength = 1.5 * 2 / 1024`, about `0.003`. Porting a number
/// across without the conversion is a normal map hundreds of times too strong,
/// which reads as a surface made of gravel.
///
/// The sign is the showcase's, and for the reason its own comment gives: rows
/// run down the texture, so a row index is `+V`, and the green channel carries
/// the tangent frame's `+V`. A surface rising toward larger `v` therefore tilts
/// its normal toward smaller `v`, which is the minus in front of both
/// derivatives. Matching it is what keeps the study's relief lit the way it is
/// lit today.
///
/// A graph that binds no height gets a flat normal rather than no normal, so a
/// texture set always has all three required maps.
#[expect(
    clippy::cast_precision_loss,
    reason = "a texel coordinate is bounded by MAX_RESOLUTION, well inside what an f32 counts exactly"
)]
pub fn derive_normal(
    height: Option<&[f32]>,
    resolution: u32,
    strength: f32,
    threads: Option<NonZeroUsize>,
) -> Vec<[f32; 3]> {
    let mut normal = vec![[0.0, 0.0, 1.0]; texels(resolution)];
    let Some(height) = height else {
        return normal;
    };
    let size = resolution as f32;
    // The two taps are two texels apart, and the field spans one UV unit.
    let span = 2.0 / size;
    let width = resolution as usize;
    // A plane is `resolution` square, so the two moduli are the same number.
    // They are named apart because the vertical wrap is a wrap over rows: the
    // first code to hand-build a `Planes` is the phase-two mip pass, and a
    // row count read out of a column count is the kind of thing that survives
    // until the first plane that is not square.
    let rows = resolution as usize;
    let mut rest = normal.as_mut_slice();
    let mut jobs = Vec::new();
    for (first, count) in spans(resolution, threads) {
        jobs.push((first, take(&mut rest, texels_in(count, resolution))));
    }
    // Nothing in a central difference can fail, so the error type is empty.
    let Ok(()) = run_jobs(jobs, |(first, chunk)| {
        let span_rows = texels_in_rows(chunk.len(), resolution);
        for row in 0..span_rows {
            let y = (first + row) as usize;
            let up = ((y + rows - 1) % rows) * width;
            let down = ((y + 1) % rows) * width;
            let here = y * width;
            for x in 0..width {
                let left = (x + width - 1) % width;
                let right = (x + 1) % width;
                let du = (at(height, here + right) - at(height, here + left)) / span;
                let dv = (at(height, down + x) - at(height, up + x)) / span;
                put(
                    chunk,
                    row as usize * width + x,
                    unit(-strength * du, -strength * dv),
                );
            }
        }
        Ok::<(), std::convert::Infallible>(())
    });
    normal
}

impl Planes {
    /// Quantise every plane into the format its map ships in.
    ///
    /// Only the base colour is dithered: it is the one map read as a picture,
    /// and the one whose transfer function compresses the darks enough for an
    /// eight-bit gradient to band. A dithered normal or roughness would be
    /// noise in a number something else divides by.
    ///
    /// Everything but emissive is clamped into `0..=1` here, which is the only
    /// place clamping happens. A height that leaves that range keeps its full
    /// value in [`Self::height`] and in the normal derived from it, so a graph
    /// whose height leaves `0..=1` ships a height map that is flat where its
    /// own normal still has slope. That is the one thing the design's bake
    /// promises not to do, and it is a graph to fix rather than a range to
    /// widen: widening it would change what `height` means to a parallax
    /// shader, and clamping before the normal would silently flatten the
    /// crests instead. [`BakeReport::height_range`] is how a caller finds out
    /// before a texture ships; a `Levels` node with the plane's own range as
    /// its input range is the fix.
    pub fn encode(&self, dither: Dither) -> TextureSet {
        // Every plane is one texel per cell of a square of `resolution`, and
        // the dither cell is indexed on that assumption. Nothing in this crate
        // can build a `Planes` that disagrees; the phase-two mip pass will
        // build them by hand, and a level whose plane and resolution disagree
        // would encode to bytes that are wrong rather than to an error.
        debug_assert!(
            self.planes_agree(),
            "a Planes whose planes are not resolution squared encodes to nonsense"
        );
        TextureSet {
            resolution: self.resolution,
            base_color: Encoded {
                format: PlaneFormat::Rgba8Srgb,
                mips: vec![encode_srgb(&self.base_color, self.resolution, dither)],
            },
            normal: Encoded {
                format: PlaneFormat::Rgba8Unorm,
                mips: vec![encode_normal(&self.normal)],
            },
            orm: Encoded {
                format: PlaneFormat::Rgba8Unorm,
                mips: vec![encode_orm(&self.occlusion, &self.roughness, &self.metallic)],
            },
            height: self.height.as_ref().map(|height| Encoded {
                format: PlaneFormat::R16Unorm,
                mips: vec![encode_height(height)],
            }),
            emissive: self.emissive.as_ref().map(|emissive| Encoded {
                format: PlaneFormat::Rgba16Float,
                mips: vec![encode_emissive(emissive)],
            }),
        }
    }

    /// Whether every plane holds exactly one texel per cell of the square
    /// [`Self::resolution`] describes.
    fn planes_agree(&self) -> bool {
        let texels = texels(self.resolution);
        self.base_color.len() == texels
            && self.roughness.len() == texels
            && self.metallic.len() == texels
            && self.occlusion.len() == texels
            && self.normal.len() == texels
            && self.height.as_ref().is_none_or(|p| p.len() == texels)
            && self.emissive.as_ref().is_none_or(|p| p.len() == texels)
    }
}

/// The linear-to-sRGB transfer function, as the sRGB specification writes it:
/// a short linear segment near black, and a power curve above it.
///
/// Public because it is the number a test compares against and the number a
/// tool needs to show a texel the way the texture will.
pub fn linear_to_srgb(value: f32) -> f32 {
    if value <= 0.003_130_8 {
        value * 12.92
    } else {
        1.055 * value.powf(1.0 / 2.4) - 0.055
    }
}

/// One 4x4 ordered dither cell, in the usual recursive order. The entries are
/// `0..16` and become the fraction of a code added before truncation, so the
/// error a texel drops is paid back by its neighbours and a gradient of less
/// than one code per texel is a gradient rather than a staircase.
const BAYER: [u8; 16] = [0, 8, 2, 10, 12, 4, 14, 6, 3, 11, 1, 9, 15, 7, 13, 5];

/// The fraction of a code to add before truncating, at one texel.
///
/// With no dither this is one half everywhere, which is ordinary rounding: a
/// value exactly between two codes takes the larger, as the 8-bit tables do.
///
/// One offset serves all three channels of a texel, so the error a texel drops
/// is the same error in each: the noise this leaves is luminance noise and
/// carries no chroma of its own. Reading the cell from a different corner per
/// channel is the usual alternative and it was measured rather than assumed —
/// every shift and transpose of a recursive 4x4 cell correlates with every
/// other at 0.55 or worse, a two-step diagonal shift being the cell itself plus
/// a sixteenth — so what it buys is a fraction of a decorrelation, and what it
/// costs is chroma noise in an albedo map. A cell with room for three genuinely
/// independent channels is a bigger mask than this, and belongs with the rest
/// of the encoder's phase-two work.
fn offset(index: usize, resolution: u32, dither: Dither) -> f32 {
    match dither {
        Dither::None => 0.5,
        Dither::Ordered => {
            // Nothing in this crate builds a `Planes` of resolution zero, and
            // a zero here would be a divide rather than a wrong texel.
            let width = (resolution as usize).max(1);
            let (x, y) = (index % width, index / width);
            let cell = BAYER[(y % 4) * 4 + x % 4];
            (f32::from(cell) + 0.5) / 16.0
        }
    }
}

/// A value in `0..=1` as an integer code, with the dither's fraction added.
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "the value is clamped to 0..=max before the cast, so it is exactly a code"
)]
fn code(value: f32, max: f32, offset: f32) -> u32 {
    // A NaN clamps to the low end rather than casting to something arbitrary.
    let scaled = if value.is_nan() {
        0.0
    } else {
        value.clamp(0.0, 1.0)
    };
    (scaled * max + offset).floor().clamp(0.0, max) as u32
}

#[expect(
    clippy::cast_possible_truncation,
    reason = "code() answers at most max, which is 255 here"
)]
fn byte(value: f32, offset: f32) -> u8 {
    code(value, 255.0, offset) as u8
}

fn encode_srgb(plane: &[[f32; 3]], resolution: u32, dither: Dither) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(plane.len() * 4);
    for (index, texel) in plane.iter().enumerate() {
        let offset = offset(index, resolution, dither);
        for channel in texel {
            bytes.push(byte(linear_to_srgb(*channel), offset));
        }
        bytes.push(255);
    }
    bytes
}

fn encode_normal(plane: &[[f32; 3]]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(plane.len() * 4);
    for texel in plane {
        for axis in texel {
            bytes.push(byte(axis.mul_add(0.5, 0.5), 0.5));
        }
        bytes.push(255);
    }
    bytes
}

fn encode_orm(occlusion: &[f32], roughness: &[f32], metallic: &[f32]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(occlusion.len() * 4);
    for index in 0..occlusion.len() {
        bytes.push(byte(at(occlusion, index), 0.5));
        bytes.push(byte(at(roughness, index), 0.5));
        bytes.push(byte(at(metallic, index), 0.5));
        bytes.push(255);
    }
    bytes
}

#[expect(
    clippy::cast_possible_truncation,
    reason = "code() answers at most max, which is 65535 here"
)]
fn encode_height(plane: &[f32]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(plane.len() * 2);
    for texel in plane {
        bytes.extend_from_slice(&(code(*texel, 65535.0, 0.5) as u16).to_le_bytes());
    }
    bytes
}

fn encode_emissive(plane: &[[f32; 3]]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(plane.len() * 8);
    for texel in plane {
        for channel in texel {
            bytes.extend_from_slice(&f32_to_f16_bits(*channel).to_le_bytes());
        }
        // Opaque, as the alpha of every other map is.
        bytes.extend_from_slice(&f32_to_f16_bits(1.0).to_le_bytes());
    }
    bytes
}

/// The bits of an IEEE 754 binary16, rounded to nearest with ties to even.
///
/// Written out rather than taken from a dependency, because it is thirty lines
/// and this crate's dependency list is three entries on purpose. Overflow
/// saturates at the largest finite half rather than becoming infinity: a
/// texture with an infinite texel is a renderer's problem later and a
/// vanishingly small amount of emissive energy here. A `NaN` stays a `NaN`,
/// with a payload bit set so dropping the low mantissa bits cannot turn it into
/// an infinity.
pub fn f32_to_f16_bits(value: f32) -> u16 {
    let bits = value.to_bits();
    let sign = ((bits >> 16) & 0x8000) as u16;
    // The biased exponent, so every comparison below is unsigned: a half's
    // exponent runs from `2^-24` at 102 to `2^15` at 142, with 112 the bias
    // between the two formats.
    let exponent = (bits >> 23) & 0xff;
    let mantissa = bits & 0x007f_ffff;
    if exponent == 0xff {
        let payload = if mantissa == 0 { 0 } else { 0x0200 };
        return sign | 0x7c00 | payload;
    }
    // An f32 subnormal is smaller than 2^-126 and every half is at least
    // 2^-24, so it rounds to zero without arithmetic. So does anything below
    // half of the smallest subnormal half.
    if exponent == 0 || exponent < 102 {
        return sign;
    }
    if exponent > 142 {
        return sign | 0x7bff;
    }
    let half = if exponent < 113 {
        // Subnormal: the half is `m * 2^-24`, so shift the implicit one in
        // and round away what does not fit.
        round_shift(mantissa | 0x0080_0000, 126 - exponent)
    } else {
        // Normal: the exponent and mantissa laid out as one field are the
        // half's own layout scaled by the thirteen bits being dropped, so a
        // carry out of the mantissa walks into the exponent by itself.
        round_shift(((exponent - 112) << 23) | mantissa, 13)
    };
    sign | half.min(0x7bff) as u16
}

/// `value >> shift`, rounded to nearest with ties to even.
fn round_shift(value: u32, shift: u32) -> u32 {
    let half = 1_u32 << (shift - 1);
    let truncated = value >> shift;
    let remainder = value & ((half << 1) - 1);
    if remainder > half || (remainder == half && truncated & 1 == 1) {
        truncated + 1
    } else {
        truncated
    }
}

/// One element, or zero past the end. A plane is always exactly as long as its
/// neighbours, so the fallback never reads; it is here so that the encoders and
/// the normal pass index without a panic path.
fn at(plane: &[f32], index: usize) -> f32 {
    plane.get(index).copied().unwrap_or_default()
}

fn texels_in(rows: u32, resolution: u32) -> usize {
    rows as usize * resolution as usize
}

fn texels_in_rows(len: usize, resolution: u32) -> u32 {
    u32::try_from(len / resolution.max(1) as usize).unwrap_or(0)
}

/// Quantise one bound plane into an image, with its whole mip chain.
///
/// The shader partition's textures are not the five PBR maps, so they do not go
/// through [`Planes::encode_mips`] — but they must quantise the *same way*, or a
/// live material and its baked twin stop being the same picture at the eighth
/// bit. This is that encoder, over one plane's four channels instead of five
/// named maps: the same ordered dither, the same sRGB transfer, the same
/// sixteen-bit height codes, the same half floats, and the same wrapped 2x2 box
/// filter down to 1x1.
///
/// `lanes` is one `[r, g, b, a]` per texel, row-major over `resolution`
/// squared, in the numbers the expression answered — linear colour, a scalar in
/// `r`, a tangent-space normal in `-1..=1`. `normal` says it is the last of
/// those: the values are written half and half about zero the way
/// [`Planes::encode`] writes a normal map, and every level is renormalised
/// before it is quantised so a mip of a normal map is still unit length. The
/// shader undoes the encoding for exactly this case; see
/// [`wgsl`](crate::wgsl).
///
/// The chain that is *filtered* is the un-normalised one, which is
/// [`mips`](crate::mips)'s rule and not an implementation detail: the mean of
/// four unit normals is short where they disagree, and renormalising before
/// averaging would throw that disagreement away a level early. Renormalising is
/// therefore what a level *ships* rather than what the next level is built
/// from, exactly as `encode_mips` does it, which is what makes a bound normal
/// map and the `normal.ktx2` of the same graph the same bytes at every level.
///
/// Channels a format does not have are dropped: `R16Unorm` keeps `r`, and every
/// other format writes all four. `a` is whatever the caller put there — one,
/// for an opaque map — except on a normal map, where the chain writes the
/// footprint's own coherence into it; see [`bound_mips`].
pub fn encode_bound(
    lanes: &[[f32; 4]],
    resolution: u32,
    format: PlaneFormat,
    normal: bool,
) -> Encoded {
    let mut size = resolution;
    let mut mips = Vec::with_capacity(crate::mips::levels(resolution));
    for level in bound_mips(lanes, resolution, normal) {
        mips.push(encode_lanes(&level, size, format, normal));
        size = (size / 2).max(1);
    }
    Encoded { format, mips }
}

/// The chain [`encode_bound`] quantises: every level of one bound plane, in the
/// numbers it ships, largest first and ending at 1x1.
///
/// Level `n` is at index `n` and is `resolution >> n` texels square, never
/// below one, so the chain is [`mips::levels`](crate::mips::levels) long. The
/// filter is [`mips`](crate::mips)'s own — a 2x2 box over the *un-normalised*
/// chain, with each level renormalised on the way out where `normal` says these
/// are normals — which is what makes a bound map and the KTX2 of the same graph
/// the same picture at every level rather than only at the first.
///
/// **A normal map's alpha carries `|n_avg|`**, the length of the mean of the
/// level-0 normals over that level's footprint, clamped to `0..=1`. That number
/// is the whole of the Toksvig term, and a compiled material has nowhere else to
/// put it: a bake holds the normal plane and the roughness plane at once and so
/// can widen one by the other as it filters, while a partition binds separate
/// images that know nothing of each other. So the coherence travels *with the
/// normal*, in the channel a tangent-space normal map does not use, and the
/// generated fragment reads it back at whatever level the hardware chose and
/// applies `r' = sqrt(r^2 + (1 - a) * k)` over
/// [`TOKSVIG_K`](crate::mips::TOKSVIG_K) — the same formula, over the same
/// box-filtered un-widened roughness, at the same level.
///
/// `|n_avg|` rather than `1 - |n_avg|` because one is what an alpha channel
/// already means: level 0 is unit normals and so writes one, an image that was
/// never given the treatment reads one, a format with no alpha reads one, and
/// one is *no widening at all*. The two quantise identically in eight bits —
/// they differ by a reflection, and a reflection has the same step — so the tie
/// is broken by which way round the failure is safe.
///
/// This is the reading half of the encoder, for a test or a harness that wants
/// the levels as numbers; a bake wants [`encode_bound`], which is this and a
/// quantisation per level. Unlike [`Planes::encode_mips`], which encodes each
/// level and drops it, both of these hold the whole chain: four thirds of level
/// 0 in `f32`, about twenty-two megabytes for one bound image at 1024. That is
/// the price of one definition of the chain rather than two, and it is paid
/// against an [`Encoded`] that holds every quantised level anyway.
#[must_use]
pub fn bound_mips(lanes: &[[f32; 4]], resolution: u32, normal: bool) -> Vec<Vec<[f32; 4]>> {
    let mut level: Vec<[f32; 4]> = lanes.to_vec();
    level.resize(texels(resolution), [0.0, 0.0, 0.0, 1.0]);
    let mut size = resolution;
    let mut chain = Vec::with_capacity(crate::mips::levels(resolution));
    loop {
        chain.push(if normal {
            shipped(&level)
        } else {
            level.clone()
        });
        if size <= 1 {
            break;
        }
        level = crate::mips::halve(&level, size);
        size = (size / 2).max(1);
    }
    chain
}

/// One level of a normal chain as it ships: the mean renormalised, with its own
/// length — the coherence the level below will need and the roughness the
/// fragment computes wants — in the alpha channel.
fn shipped(level: &[[f32; 4]]) -> Vec<[f32; 4]> {
    level
        .iter()
        .map(|texel| {
            let mean = [texel[0], texel[1], texel[2]];
            let unit = crate::mips::unit(mean);
            [unit[0], unit[1], unit[2], crate::mips::coherence(mean)]
        })
        .collect()
}

/// One level of [`encode_bound`].
#[expect(
    clippy::cast_possible_truncation,
    reason = "code() answers at most max, which is 255 or 65535 here"
)]
fn encode_lanes(level: &[[f32; 4]], size: u32, format: PlaneFormat, normal: bool) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(level.len() * format.bytes_per_texel());
    // The colour is dithered and everything else is rounded, which is the
    // policy the five maps above are written under: banding in an albedo is
    // what a 4x4 Bayer cell exists to hide, and dither in a direction field or
    // a roughness is noise in data nobody looks at directly. Keyed on the sRGB
    // format because that is exactly the map the bake dithers, so a bound
    // texture and the map it replaces come out byte for byte the same — which
    // is the claim the material sheet's baked-beside-live pairs make.
    let dither = if format == PlaneFormat::Rgba8Srgb {
        Dither::Ordered
    } else {
        Dither::None
    };
    for (index, texel) in level.iter().enumerate() {
        let offset = offset(index, size, dither);
        // A normal runs -1..=1 and every format here but the half floats holds
        // 0..=1, so it is written half and half about zero, as a normal map is.
        let mapped = |channel: usize| {
            let value = texel[channel];
            if normal && channel < 3 {
                value.mul_add(0.5, 0.5)
            } else {
                value
            }
        };
        match format {
            PlaneFormat::Rgba8Srgb => {
                for channel in 0..3 {
                    bytes.push(byte(linear_to_srgb(mapped(channel)), offset));
                }
                bytes.push(byte(texel[3], offset));
            }
            PlaneFormat::Rgba8Unorm => {
                for channel in 0..3 {
                    bytes.push(byte(mapped(channel), offset));
                }
                bytes.push(byte(texel[3], offset));
            }
            PlaneFormat::R16Unorm => {
                bytes.extend_from_slice(&(code(mapped(0), 65535.0, 0.5) as u16).to_le_bytes());
            }
            // Half floats clamp nothing, so a normal keeps its own sign here
            // and the shader reads it back without a decode.
            PlaneFormat::Rgba16Float => {
                for channel in texel {
                    bytes.extend_from_slice(&f32_to_f16_bits(*channel).to_le_bytes());
                }
            }
        }
    }
    bytes
}
