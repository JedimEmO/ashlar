//! Procedural material graphs: tileable PBR surfaces as plain data.
//!
//! This crate sits beside [`ashlar`](https://crates.io/crates/ashlar) rather
//! than above or below it: a graph knows nothing about buildings, and a
//! building names a graph by key only. There is no Bevy here, no IO and no
//! image codec. `ashlar-bevy` is the first crate that depends on both.
//!
//! A [`MaterialGraph`] wires generators, transforms, filters and blends into a
//! [`PbrOutput`]. It is editable interchange data that Serde reads and writes;
//! [`Material`] is the immutable, validated snapshot a backend consumes, and
//! [`MaterialGraph::build`] is the only way to one. Deserialized graphs must be
//! built before use, exactly as `ashlar` recipes must.
//!
//! Every port carries an integer [`Period`]: `Tiled { u, v }` means the field
//! is laid on a lattice of that many cells across UV `[0, 1)` and meets itself
//! at the seam, so a texture baked from it tiles. Generators wrap their
//! lattice at an authored integer period, pointwise nodes take the least common
//! multiple of their inputs, and transforms may only change a period in ways
//! that keep it an integer. The output must tile, so a graph that cannot is
//! rejected by node path before anything is baked.
//!
//! The one thing a period does not say is that a *coordinate* meets itself:
//! [`Uv`](nodes::Uv) tiles once because the lattice it addresses does, while
//! `u` runs from 0 to 1 and jumps back. Generators wrap the coordinate
//! themselves, so this only shows where a coordinate is used as a value —
//! a [`Warp`](nodes::Warp) offset built from a bare `Uv` displaces
//! differently on each side of the seam. [`Uv`](nodes::Uv) says so, and says
//! what to build such an offset from instead.
//!
//! # From a graph to texels
//!
//! A built material is lowered once, into the typed per-texel expression in
//! [`ir`]: one SSA instruction list that [`interp`]'s interpreter runs and
//! that the shader backend will print as WGSL. A node writes its lowering
//! once, so a baked material and its live twin are the same material; the only
//! things implemented twice are the three dozen [`Op`](ir::Op)s. Randomness is
//! integer hashing over a lattice that wraps at the port's period, which is
//! why a noise here tiles at all, and the lowering folds constants — this is
//! how a [`Exposure::Bake`] parameter disappears — shares identical
//! sub-expressions, and drops everything no output reads.
//!
//! Every node the vocabulary declares lowers, and so does every output of every
//! one of them. The runtime inputs are the one place where *which backend is
//! asking* changes the answer: [`Time`](nodes::Time) and
//! [`CutFlag`](nodes::CutFlag) are zero in a bake, because the instant an app
//! started and an uncut face are both pictures, while
//! [`WorldPos`](nodes::WorldPos), [`WorldNormal`](nodes::WorldNormal) and the
//! [`Triplanar`](nodes::Triplanar) and [`WorldMask`](nodes::WorldMask) built
//! out of them are **refused** by a bake at the node that asked — a world
//! position of zero is a whole wall at one point, and a world normal of zero is
//! not a direction — with a message naming the `Shader` surface that does
//! answer. Two settings do not lower, and each is a refusal naming the node that
//! asked rather than a picture that is quietly something else: a rounded or cut
//! corner on [`Bricks`](nodes::Bricks) is a backend waiting its turn;
//! [`NoiseKind::Simplex`] is not waiting for anything, because a simplex
//! lattice has no integer period at all — its skew is `(sqrt(3) - 1) / 2`, and
//! no whole number of repeats brings an irrational skew back to itself — so it
//! validates, infers a period, and says why rather than promising one later.
//!
//! A resampler — [`Transform`](nodes::Transform), [`Warp`](nodes::Warp),
//! [`Mirror`](nodes::Mirror), [`Kaleidoscope`](nodes::Kaleidoscope),
//! [`Tile`](nodes::Tile) — is the one thing in the lowering that costs rather
//! than folds. An expression is one value per texel and [`Op::Uv`](ir::Op::Uv)
//! is that texel's coordinate, so a node that reads its source somewhere else
//! emits that source again through
//! [`Lowering::substitute`](ir::Lowering::substitute), with only the
//! instructions that depend on the coordinate copied.
//! [`Subgraph`](nodes::Subgraph) is inlined into the arena that instanced it,
//! with its parameters bound at the instance and folded, so it costs nothing at
//! runtime and shares sub-expressions with the graph around it. What it binds
//! to a [`GraphInput`](nodes::GraphInput) is wired rather than folded: a whole
//! field, carrying its type, its period and its lattice inward, which is why an
//! instance is inferred and kept once per binding signature rather than once per
//! graph key.
//!
//! # Buffered filters
//!
//! A [`Blur`](nodes::Blur), an [`OcclusionFromHeight`](nodes::OcclusionFromHeight),
//! a [`Distance`](nodes::Distance), an [`Erode`](nodes::Erode), a
//! [`Dilate`](nodes::Dilate), a [`Curvature`](nodes::Curvature) or an
//! [`EdgeDetect`](nodes::EdgeDetect) reads a *neighbourhood*, which per texel
//! would be the whole expression again once per tap. So the lowering cuts the
//! graph there: everything upstream is rasterised once into a
//! [`Plane`](interp::Plane), the filter runs over it as ordinary Rust, and what
//! is downstream reads the result bilinearly and with wrap. [`Buffer`](nodes::Buffer)
//! is that cut with no filter, for pinning a resolution or paying for a
//! sub-expression once.
//!
//! One of them reads *two* planes. A slope [`Blur`](nodes::Blur) smears a field
//! along the relief of a height, and neither of those is a function of the
//! other, so its [`Filter`](ir::Filter) names the plane it walks down beside
//! the one it was given — the only place in the IR where a filter carries a
//! [`BufferId`](ir::BufferId). Everything that walks the plan list follows that
//! name: the dead-plane sweep keeps a guide nothing samples, the renumbering
//! rewrites it, the cache key stands the guide's own key in for an index that
//! means nothing outside one lowering, and a partition rebuilds the guide
//! before the plane that reads it.
//!
//! And one of them reads *no* plane. A [`StrandRelief`](nodes::StrandRelief) is
//! a strand layer of the same graph splatted from directly above, and it
//! answers seven fields of one splat: a [`Coverage`](nodes::StrandReliefOutput)
//! union, a `Height` max, `Along`, `Id` and `Color` weighted by each
//! contribution's own coverage, and `Mass` and `Occlusion`, which accumulate
//! *without* saturating — the first is how much grass stands over a texel and
//! the second is that weighted by how high it stood, which is what a graph
//! builds a pile and its shadow out of. What decides its texels is a scatter
//! rather than a sub-expression: its
//! [`BufferPlan`](ir::BufferPlan) carries a [`StrandPlan`](ir::StrandPlan) —
//! the layer's own lowering, its constants and the repeat it is measured
//! against — and its root is a constant nothing rasterises.
//! [`strand_plane`](planes::strand_plane) is what splats it, on the CPU on both
//! backends, and the plan's key goes into the plane key beside the filter code
//! so that two layers' coverage planes are two planes. The three outputs of one
//! layer share one scatter, which the cache keys by that same plan.
//!
//! Wrapped is the word that matters: every filter reads its neighbours across
//! the seam, so a blurred noise still meets itself and the period inference
//! above still means what it says. Every radius is a reach in UV rather than a
//! count of texels, so the same graph describes the same surface at 512 and at
//! 4096. Two nodes that ask for the same filter over the same expression share
//! one plane, a buffered node no output reads leaves none, and
//! [`BakeCache`](planes::BakeCache) is how a caller that bakes the same graph
//! again keeps the planes that did not change.
//!
//! # A bake
//!
//! [`bake`](bake::bake) runs that expression once per texel over one repeat and answers a
//! [`TextureSet`](bake::TextureSet): sRGB base colour, a tangent-space normal,
//! occlusion, roughness and metallic packed into one linear map, and the
//! height and emissive maps a graph binds or does not. Rows are rasterised
//! across [`std::thread::scope`] threads and a texel reads nothing but its own
//! coordinate, so the bytes do not depend on how the rows were divided. The
//! normal is derived from the height plane rather than computed beside it, by
//! a wrapped central difference scaled by
//! [`PbrOutput`]'s `normal_strength` over the texel size — which is why relief
//! and parallax read one surface, and why that strength is per UV unit rather
//! than per texel. The `f32` planes are exposed as [`Planes`](bake::Planes)
//! before anything is quantised, because that is what the mip chain filters and
//! what a test reads without arguing about rounding.
//!
//! The planes a buffered filter needs are rasterised first, in dependency
//! order, and the texel pass evaluates only what the outputs reach. A bake is
//! also refused when the graph lays a lattice finer than the texels asked for
//! — [`BakeError::Lattice`](bake::BakeError::Lattice) — because what comes out
//! below that is not a coarser surface but an arbitrary sample of one. That
//! count is [`Material::finest_lattice`], carried through the graph rather than
//! read off each node alone: a resampler multiplies it, and a graph counts what
//! it instances.
//!
//! [`BakeRequest::mips`](bake::BakeRequest::mips) adds the rest of the chain:
//! every level down to 1x1, each the 2x2 box filter of the one above it over
//! the same `f32` planes. Two of them are not a plain average — the normals are
//! the direction of the mean of the level-0 normals, and the roughness is
//! widened by how much shorter that mean got — which is what keeps a bumpy wall
//! from turning to glass at distance. That pass is [`mips`], and what it
//! decides is written down there.
//!
//! # A shader
//!
//! The same lowering compiles. [`partition`](partition::partition) colours
//! every value in it: **runtime** if it depends on a parameter the author
//! exposed as [`Exposure::Live`], or on [`Time`](nodes::Time),
//! [`WorldPos`](nodes::WorldPos), [`WorldNormal`](nodes::WorldNormal) or
//! [`CutFlag`](nodes::CutFlag) — the four things a texture cannot hold — and
//! **static** otherwise — which is also why nothing downstream of a world-space
//! input is ever baked. Then it cuts at the frontier: every maximal static
//! sub-expression a runtime op reads becomes a bound texture, baked by the same
//! plane pipeline the buffered filters use, and what is left is an expression
//! over the uniforms, the runtime inputs and samples of those textures. An
//! output that reaches nothing runtime is a texture in its own right, sampled
//! straight into its PBR slot.
//!
//! [`CostReport`](partition::CostReport) is what an author reads before
//! shipping one: runtime ops per output, texture reads, bound textures, and the
//! neighbourhood taps a normal derived per fragment costs. A graph with no live
//! inputs reports zero and says to bake it instead. A parameter that reaches a
//! filter which can only ever be a plane — a blur, an occlusion, a distance, a
//! morphology — cannot move per frame, so it is folded like a bake parameter
//! and named in [`Partition::frozen`](partition::Partition::frozen), because a
//! slider that silently does nothing is worse than one that says why.
//!
//! [`wgsl`] prints what is left: one `let` per instruction inside a function
//! that takes the coordinate and the four runtime inputs and answers the ports
//! the graph bound, with a `MaterialExtension` fragment, a prepass fragment and
//! a compute kernel around it, all three out of one emitter. Every op is
//! written as the arithmetic [`interp`] does rather than as the WGSL built-in
//! that is nearly it, because the interpreter is the reference and the
//! conformance test compares them number for number.
//!
//! # Strands
//!
//! Some surfaces are not one field over one plane. Grass, fur, moss fibre,
//! carpet and thatch are made of things that stand *off* the surface and have a
//! silhouette, and a height map of them is a picture of grass rather than
//! grass. So a graph carries a second kind of output beside [`PbrOutput`]: a
//! named [`StrandLayer`], which reads the same nodes and the same parameters —
//! that is the point of it living in the graph — but reads them once per
//! *strand*, at the strand's root, rather than once per texel.
//!
//! [`strands::scatter`] is that reading. The roots are a jittered lattice of
//! the layer's own `count` cells that wraps at the repeat, which is
//! [`Tile`](nodes::Tile)'s scatter exactly, down to the hash, with
//! [`per_cell`](StrandLayer::per_cell) strands standing in each of them —
//! density the period does not have to carry, because a cell's strands share
//! its coordinate and differ only in the salt their hashes are taken under; the fields are
//! evaluated by point evaluation through the same [`interp`] interpreter, so
//! the answer does not depend on a bake resolution; and the answer is a
//! [`StrandSet`](strands::StrandSet) sorted by rank, so a level of detail is a
//! *prefix* of the slice and the survivors of a tighter cut are a subset of the
//! survivors of a looser one. The fields are held to the tiling rule every
//! bound output is held to, and the layer's lattice joins the material's
//! repeat.
//!
//! A strand is more than its fields. Two axes of randomness run through it and
//! they are not interchangeable: a field varies a lawn over *metres*, a
//! per-strand hash varies it per blade, and a **clump** varies it per tuft. A
//! layer that declares a [`clump_count`](StrandLayer::clump_count) lays a
//! second, coarser lattice; every strand belongs to the clump its root falls
//! in and knows how far out in it it stands, and the clump then *owns* the
//! blade. [`clump_share`](StrandLayer::clump_share) of every per-strand
//! variation — length, width, lean, direction, bend and the facing turn — is
//! taken from that clump's hash instead of the strand's own,
//! [`clump_tint`](StrandLayer::clump_tint) varies the whole tuft's colour
//! against its neighbours, and [`clump_tips`](StrandLayer::clump_tips) pulls
//! the strand's *tip* towards the clump centre while its root stays where the
//! lattice put it. That last part
//! is what makes a tuft rather than a huddle. Beside them,
//! [`facing_variation`](StrandLayer::facing_variation) turns a strand about its
//! own curve so a patch leaning alike is not also presented alike, and
//! [`height_offset_metres`](StrandLayer::height_offset_metres) sinks roots
//! below the surface so a pile has half-buried blades in it.
//!
//! A strand field is static. One that reaches [`Time`](nodes::Time),
//! [`WorldPos`](nodes::WorldPos), [`WorldNormal`](nodes::WorldNormal),
//! [`CutFlag`](nodes::CutFlag), [`Triplanar`](nodes::Triplanar) or
//! [`WorldMask`](nodes::WorldMask) is refused at the node that asked, because a
//! strand is placed before there is a mesh under it or a frame around it; an
//! [`Exposure::Live`] parameter is folded at the value the request bound, for
//! the same reason. Wind is the renderer's. A field that reaches a
//! [`StrandRelief`](nodes::StrandRelief) is refused by the same walk, because a
//! relief is splatted *from* the fields and a field that read one would be
//! asking for itself.
//!
//! Three things read one set, and they are what makes a level-of-detail chain
//! honest. [`strands::place`] plants it on a mesh and [`strands::mesh`] turns
//! the planted strands into ribbons or tubes; [`nodes::StrandRelief`] splats the
//! *same* strands into the PBR half. So the relief the camera sees once the
//! geometry has faded out is the geometry, rather than a second drawing of it
//! that has to be kept in step by hand.
//!
//! "The same" has one sharp edge, and
//! [`strands::FIELD_RESOLUTION`] is where it is kept. A strand field that
//! reaches a buffered filter is a *plane*, and a plane has a size; a scatter
//! whose planes were rasterised at two different sizes is two different
//! scatters. So that size is a constant of this crate rather than a parameter
//! of the splat, and both the mesh builder and the relief read it — otherwise a
//! relief inside a 2048 bake would gate its blades on a 2048 distance where the
//! geometry gated on a 512 one, and the two halves would quietly be different
//! lawns.
//!
//! # Files
//!
//! A set goes to disk as KTX2, one file per map, through [`ktx2::write`]: the
//! container that carries a mip chain, where a PNG carries level 0 and the
//! renderer invents the rest by point sampling. [`ktx2::inspect`] reads one
//! back far enough to say whether it is the file it claims to be, which is what
//! a startup preflight needs and all it can afford. Neither direction needs a
//! dependency; [`ktx2::write_with`] and `ktx2::Supercompression::Zstd`, which
//! compress each level under scheme 2, are the one thing here that does, and
//! they are behind the crate's `zstd` feature so that only a step writing files
//! compiles an encoder.
//!
//! # A surface, and the two conversions
//!
//! Node references are strings, and errors carry the path
//! `nodes[id].inputs[name]`. A node value may also be written inline where an
//! input is expected, in which case it is hoisted into the graph under the id
//! `parent.port`, so a helper node needs a name only when something else reads
//! it.
//!
//! ```
//! use ashlar_material::{
//!     BlendMode, Channel, Input, Material, MaterialGraph, MathOp, Param, PbrOutput, Period,
//!     nodes::{Blend, Colorize, Decompose, Invert, Levels, Math, Noise, Uv},
//! };
//!
//! let concrete = MaterialGraph::builder("study:concrete")
//!     .param(Param::color("tint", [0.66, 0.65, 0.64]))
//!     .param(Param::float("wear", 0.0).range(0.0, 1.0).live())
//!     .node("coarse", Noise::value().period(4))
//!     .node("mottle", Noise::value().period(32))
//!     .node("grain", Noise::value().period(128))
//!     .node("pores", Levels::new("grain").in_low(0.87))
//!     // The panel's joints run across it and not down it, so they are a band
//!     // in v rather than a wall: eight rows, and the height drops where a row
//!     // is within 0.007 of its edge. A `Bricks` of one column would also carve
//!     // the vertical joint a cast panel does not have.
//!     .node("rows", Math::new(MathOp::Mul, Decompose::new(Uv::new(), Channel::G), 8.0))
//!     .node("into_row", Math::new(MathOp::Sub, Math::unary(MathOp::Fract, "rows"), 0.5))
//!     .node("seams", Math::new(MathOp::Step, Math::unary(MathOp::Abs, "into_row"), 0.493))
//!     .node("relief", Blend::new(BlendMode::Add, "mottle", "grain").opacity(0.5))
//!     // An added blend at half opacity runs to 1.5, and `height` is a
//!     // `0..=1` field: without this the joints would still be carved but
//!     // every crest would clip, and the height map would stop describing the
//!     // surface the normal is derived from. The relief is bedded at 0.16 so
//!     // that taking the joint out again lands the field in `0..=1`.
//!     .node("bedded", Levels::new("relief").in_range(0.0, 1.5).out_range(0.16, 1.0))
//!     .node("height", Blend::new(BlendMode::Subtract, "bedded", "seams").opacity(0.16))
//!     .node("albedo", Colorize::new("coarse").gradient([
//!         (0.0, [0.60, 0.59, 0.58]),
//!         (1.0, [0.74, 0.73, 0.71]),
//!     ]))
//!     .node("tinted", Blend::new(BlendMode::Multiply, "albedo", Input::param("tint")))
//!     // An inline node needs no id of its own; this one becomes `dirty.b`.
//!     .node("dirty", Blend::new(BlendMode::Multiply, "tinted", Invert::new("pores")))
//!     .output(
//!         PbrOutput::new()
//!             .base_color("dirty")
//!             .roughness(Levels::new("coarse").out_range(0.85, 0.97))
//!             .metallic(0.0)
//!             .height("height")
//!             .normal_strength(0.015),
//!     )
//!     .build()?;
//!
//! // The lattice periods 4, 32 and 128 divide one another, so one repeat of
//! // the material is one repeat of its coarsest noise.
//! assert_eq!(concrete.period(), Period::Tiled { u: 128, v: 128 });
//! assert!(concrete.warnings().is_empty());
//! // A colour reached the base colour input; a float reached roughness.
//! assert_eq!(concrete.port("dirty").map(|p| p.value_type), Some(ashlar_material::ValueType::Color));
//! # Ok::<(), ashlar_material::GraphError>(())
//! ```
//!
//! # A standard library
//!
//! [`stdlib`] is the one set of graphs this crate ships rather than reads: a
//! handful of weathering compounds and pattern recipes, keyed
//! `weathering:<name>` and `patterns:<name>`, each a graph with typed
//! [`GraphInput`](nodes::GraphInput)s for the substrate's channels, parameters
//! for the author's decisions, and its deciding mask exported as an extra. A
//! game merges them into its own library with
//! [`MaterialGraphLibrary::extend`] and names the keys from
//! [`Subgraph`](nodes::Subgraph) nodes; they are here rather than in an
//! example so that the crate's own tests hold them to the tiling, lattice and
//! lowering rules that make a compound safe to instance at all. The module
//! documentation says what a compound is allowed to do, which comes to this:
//! it changes the channels it names and no others, it never reads a runtime
//! input, and where the caller wants it gated by the world it takes a plain
//! float `bias` input for the caller to wire.
//!
//! # glam
//!
//! The crate is built against the same [`glam`] `ashlar` re-exports, so a
//! consumer that holds both never converts a vector at the boundary.

pub use glam;

pub mod bake;
pub mod export;
mod graph;
pub mod interp;
pub mod ir;
pub mod ktx2;
mod library;
pub mod memo;
pub mod mips;
pub mod nodes;
pub mod partition;
mod period;
pub mod planes;
pub mod stdlib;
pub mod strands;
mod strip;
pub mod wgsl;

pub use graph::{
    Exposure, GraphParam, Input, LUMINANCE, MAX_SEGMENTS, Material, MaterialGraph,
    MaterialGraphBuilder, Param, ParamValue, PbrOutput, Port, StrandLayer, StrandProfile, Value,
    ValueType, luminance,
};
pub use library::MaterialGraphLibrary;
pub use nodes::{
    BlendMode, BrickOutput, Channel, MathOp, MirrorAxis, Node, NoiseKind, SdfOp, ShapeKind,
    ShapeOutput, SurfaceOutput, WeaveOutput, WeavePattern,
};
pub use period::{MAX_PERIOD, Period};

/// A graph error with a path suitable for an editor or command-line diagnostic.
///
/// The same shape as `ashlar`'s validation error, and for the same reason: a
/// surface that will not bake should say which input of which node is wrong,
/// not that something somewhere is wrong.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("{path}: {reason}")]
pub struct GraphError {
    /// Location within the graph, such as `nodes[relief].inputs[b]`.
    pub path: String,
    /// The violated invariant.
    pub reason: String,
}

impl GraphError {
    pub(crate) fn new(path: impl Into<String>, reason: impl Into<String>) -> Self {
        Self {
            path: path.into(),
            reason: reason.into(),
        }
    }

    /// Re-root a path under a library key, so a subgraph failure names the graph it is in.
    pub(crate) fn under(mut self, prefix: &str) -> Self {
        self.path = format!("{prefix}.{}", self.path);
        self
    }
}

/// Something a graph may still bake, but probably not the way its author meant.
///
/// Warnings are collected rather than returned, so one is never the reason a
/// build failed. A tool prints them; a bake ignores them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GraphWarning {
    /// Location within the graph, in the same form [`GraphError`] uses.
    pub path: String,
    /// What is suspicious about it.
    pub message: String,
}

impl std::fmt::Display for GraphWarning {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.path, self.message)
    }
}

impl GraphWarning {
    pub(crate) fn new(path: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            path: path.into(),
            message: message.into(),
        }
    }
}

pub(crate) fn require(condition: bool, path: &str, reason: &str) -> Result<(), GraphError> {
    if condition {
        Ok(())
    } else {
        Err(GraphError::new(path, reason))
    }
}

/// The name rule `ashlar` parts and slots follow, kept identical on purpose:
/// an identity is whatever the author typed, as long as they typed something.
pub(crate) fn name(value: &str, path: &str) -> Result<(), GraphError> {
    require(!value.trim().is_empty(), path, "name must not be blank")
}

pub(crate) fn finite(value: f32, path: &str, what: &str) -> Result<(), GraphError> {
    require(value.is_finite(), path, &format!("{what} must be finite"))
}
