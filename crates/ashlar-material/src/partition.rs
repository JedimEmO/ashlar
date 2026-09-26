//! The shader partition: what a graph costs per fragment, and what it binds.
//!
//! A bake folds every parameter and evaluates the whole graph once per texel. A
//! shader cannot: a parameter the author declared [`Live`](crate::Exposure::Live)
//! has no value until the frame it is drawn in, and [`Time`](crate::nodes::Time),
//! [`WorldPos`](crate::nodes::WorldPos),
//! [`WorldNormal`](crate::nodes::WorldNormal) and
//! [`CutFlag`](crate::nodes::CutFlag) have none at all. So the compiler colours
//! every value in the lowered expression:
//!
//! - **Runtime** if it depends on one of those, directly or through the plane
//!   an [`Op::Sample`] reads.
//! - **Static** otherwise.
//!
//! Then it cuts at the frontier. Every maximal static sub-expression a runtime
//! op reads becomes a **bound texture**, baked once by the plane pipeline this
//! crate already has; what is left is the **runtime partition**, an expression
//! over [`Op::Param`], the four runtime inputs, and samples of those textures.
//! An output that reaches nothing runtime is itself a bound texture, which the
//! shader samples straight into its PBR slot — so a graph with no live inputs
//! compiles to a material that samples a handful of textures and does nothing
//! else, and [`CostReport::verdict`] says in so many words to bake it instead.
//!
//! The partition is an [`Ir`] like any other, which is the point: its
//! [`Ir::buffers`] are the bound textures in binding order, so
//! [`rasterise_buffers`](crate::planes::rasterise_buffers) bakes the static
//! half with no new code, and the WGSL emitter prints the runtime half with the
//! same op set the interpreter runs.
//!
//! # What the cut costs
//!
//! A bound texture is a *quantised, bilinearly filtered* copy of the
//! sub-expression it replaced. That is the price of cutting there, and it is
//! why the cut is made as late as possible: a value built only out of constants
//! and the coordinate is re-emitted on the runtime side rather than bound,
//! because a texture of a constant is a constant and a texture of the
//! coordinate is the coordinate.
//!
//! A live parameter deep in the graph pulls everything downstream of it into
//! the shader. One at the final blend costs one blend per fragment; one that
//! feeds every noise costs the whole graph per fragment, and a
//! [`Warp`](crate::nodes::Warp) over the bound outputs — which is what a
//! per-building `variation` parameter is — costs every instruction that depends
//! on the coordinate, four times over wherever a normal is derived from it. The
//! report counts all of it, so the cost is seen when the graph is written
//! rather than when it ships.
//!
//! # What cannot be cut
//!
//! A [`Blur`](crate::nodes::Blur), an
//! [`OcclusionFromHeight`](crate::nodes::OcclusionFromHeight), a
//! [`Distance`](crate::nodes::Distance), an [`Erode`](crate::nodes::Erode) or a
//! [`Dilate`](crate::nodes::Dilate) reads an unbounded neighbourhood. It is a
//! plane or it is nothing, and a plane is baked once — so a parameter that
//! reaches one of them cannot be live, however the target was written. Rather
//! than refuse the graph, the partition **freezes** such a parameter: it folds
//! it at its value the way a bake would, records it in [`Partition::frozen`],
//! and says so in the report. A cracking or chipping control that sets a
//! filter's input is of this kind, and a shader that silently ignored a slider
//! would be worse than one that says which sliders it could not honour. A *runtime input* upstream
//! of such a filter has no value to fold and is refused by node path.
//!
//! Two filters survive the cut. [`Filter::Normal`] is emitted as four taps of
//! its height at offset coordinates — the honest cost, counted in
//! [`CostReport::taps`] — because that is what "a finite difference at texel
//! size" comes to without a plane. And a [`Buffer`](crate::nodes::Buffer) with
//! no pinned resolution is a cache point rather than a filter, so it is
//! inlined; one that pinned a resolution is refused, because there is no plane
//! for the pin to be about.
//!
//! # Subgraphs
//!
//! A [`Subgraph`](crate::nodes::Subgraph)'s parameters are bound at the
//! instance and folded, never live: the node holds values rather than inputs,
//! so nothing an outer graph declares can reach one, and an instanced graph's
//! own uniform is a uniform the graph that instanced it never declared. An
//! inlined subgraph is therefore always static unless something *around* it is
//! runtime, and there is no case here for it. Exporting a subgraph's parameter
//! to the outer uniform block is a design nobody has written; when somebody
//! does, this is where it lands.
//!
//! # The normal
//!
//! [`PbrOutput`](crate::PbrOutput) derives the material's normal from its
//! height rather than binding one, and the bake does that derivation outside
//! the IR. The partition puts it back in, as a root named `normal` beside the
//! PBR ports: a static height gives a bound normal map, derived by the same
//! [`Filter::Normal`] the bake's own central difference is, and a runtime
//! height gives the four taps. A graph whose `normal_strength` is zero gets no
//! such root, because a flat normal map is the mesh's own normal written down.
//!
//! A bound normal map carries one thing a baked one does not, and
//! [`Partition::widening`] is where that is decided: the alpha of its chain
//! holds how coherent each level's normals are, so the fragment can widen the
//! roughness with distance the way [`mips`](crate::mips) widens a bake's. A
//! normal that is four taps of a live height carries no such history, and
//! [`CostReport::widens`] is how an author finds out.

use std::collections::{BTreeMap, BTreeSet};

use crate::{
    Exposure, GraphError, Material,
    bake::{MAX_RESOLUTION, MIN_RESOLUTION, PlaneFormat},
    ir::{
        BufferId, BufferPlan, Filter, Ir, IrType, Lowering, Op, Operands, Target, ValueId, lower,
    },
    nodes::frame::{add, div, k, k2, mul, normalize, sub},
    require,
};

/// The most textures one compiled graph may bind.
///
/// A fixed count is what lets one Bevy material extension serve every graph:
/// the bind group declares its texture and sampler pairs in the type, so the
/// number cannot depend on which graph is being drawn. Eight is what is left
/// over with room to spare — WebGPU's baseline guarantees sixteen sampled
/// textures and sixteen samplers per shader stage, and `StandardMaterial`
/// already spends six of each on the maps it owns.
///
/// A graph that would bind more than this packs its *scalar* textures four to
/// an RGBA image first, and is refused by path only if it still does not fit.
pub const MAX_BOUND_TEXTURES: usize = 8;

/// Channels one bound texture holds, and so how many scalars pack into one.
const LANES_PER_TEXTURE: u8 = 4;

/// What a *packed* image is, whatever its lanes came from.
///
/// A format is a property of the image and not of any one plane in it, so the
/// moment several planes share an image they have to share one format too: a
/// height asking for sixteen-bit codes cannot live in the same image as a
/// roughness asking for eight unorm bits, and a shader decoding each lane at
/// its plane's own format would read channels the encoder never wrote. Half
/// floats are what everything that packs goes into, because they hold every
/// lane unchanged — they clamp nothing and quantise least, which is what an
/// interior cut would have chosen anyway.
const PACKED_FORMAT: PlaneFormat = PlaneFormat::Rgba16Float;

/// The path a cut that belongs to no node is reported under.
///
/// A frontier is the boundary itself rather than a node: the value on the
/// static side of it may be shared by a dozen nodes or be half of one, and
/// naming any of them would be naming the wrong thing.
const FRONTIER: &str = "partition";

/// Where the shader gets one PBR output from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OutputSource {
    /// Straight out of a bound texture, into the slot. The output reaches
    /// nothing runtime, so the fragment does no arithmetic for it at all.
    Texture(BufferId),
    /// Computed per fragment, from this root of [`Partition::runtime`].
    Runtime(ValueId),
    /// The same value at every texel and every frame: a constant, or the
    /// coordinate. The shader writes it as a literal, so it costs nothing and
    /// binds nothing.
    Inline(ValueId),
}

impl OutputSource {
    /// The value in [`Partition::runtime`] this output is, where it is one.
    ///
    /// `None` for [`Self::Texture`], whose value is a fetch rather than an
    /// expression; [`Ir::root`] on the partition's own IR answers for that one
    /// as well, and answers the sample that reads it.
    pub fn value(self) -> Option<ValueId> {
        match self {
            Self::Runtime(root) | Self::Inline(root) => Some(root),
            Self::Texture(_) => None,
        }
    }

    /// Whether the fragment computes this output rather than reading it.
    pub fn is_runtime(self) -> bool {
        matches!(self, Self::Runtime(_))
    }
}

/// One value the shader reads out of a texture instead of computing.
///
/// Every [`Op::Sample`] in [`Partition::runtime`] names one of these, by the
/// [`BufferId`] that is also its position in [`Partition::textures`] and in
/// [`Ir::buffers`]. So the three lists are parallel, and baking the static half
/// of a shader is [`rasterise_buffers`](crate::planes::rasterise_buffers) over
/// the partition's own IR with nothing new written.
///
/// [`Self::id`] and [`Self::lane`] are where the baked plane goes: normally one
/// image per binding at lane zero, and, for a graph that would otherwise bind
/// more images than [`MAX_BOUND_TEXTURES`], four scalar bindings to an image.
/// An `id` of `None` is a plane no fragment reads — an intermediate the plane
/// pipeline rasterises on the way to one that is read, a blur's input being the
/// usual one. It costs a bake and no binding, and it is dropped once the plane
/// that samples it is filtered.
///
/// [`Self::format`] is what the *image* is, not what the plane holds, and it
/// means nothing where `id` is `None`, since an intermediate plane stays `f32`
/// and is never encoded. A plane whose values leave `0..=1` is encoded the way
/// the bake encodes the same map: a normal half-and-half about zero, an
/// emissive as half floats. Every binding sharing an `id` carries the same
/// format, because the image has only one — packing therefore gives up the
/// bake's quantisation for the format that holds every lane unchanged.
#[derive(Clone, Debug, PartialEq)]
pub struct BoundTexture {
    /// Which sample reads it, and this binding's position in every list.
    pub buffer: BufferId,
    /// Which image it lives in, or `None` where nothing in the fragment reads
    /// it. Two bindings with one id are two channels of one packed image.
    pub id: Option<u32>,
    /// The first channel of that image this binding occupies.
    pub lane: u8,
    /// The expression to rasterise, in [`Partition::runtime`].
    pub root: ValueId,
    /// What the plane holds, and so how many channels this binding takes.
    pub value_type: IrType,
    /// What to run over the rasterised plane. [`Filter::None`] for a frontier
    /// cut, and the node's own filter for a buffered one.
    pub filter: Filter,
    /// The resolution a node pinned, or `None` for [`Partition::resolution`].
    pub resolution: Option<u32>,
    /// The texture format the image is in.
    pub format: PlaneFormat,
    /// The PBR output this texture *is*, when it is one: a static output is
    /// sampled straight into its slot, and this is which.
    pub port: Option<String>,
}

/// What a compiled graph costs, in the numbers an author can act on.
///
/// Every count here is per fragment except [`Self::textures`], which is per
/// material. [`Self::ops`] counts only the outputs the fragment computes: an
/// output that is a texture read or a literal costs nothing, which is why a
/// graph with no live inputs reports zero rather than reporting its own size.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CostReport {
    /// Instructions the fragment evaluates, over every runtime output at once.
    /// Outputs that share a sub-expression share it here too.
    pub ops: usize,
    /// Instructions each output reaches, by port name, with `normal` beside the
    /// PBR ports. Zero for an output that is a texture or a literal. These sum
    /// past [`Self::ops`] wherever two outputs share work.
    pub ops_per_output: BTreeMap<String, usize>,
    /// Texture reads the fragment makes, bound outputs included.
    pub samples: usize,
    /// Images the material binds, after packing.
    pub textures: usize,
    /// Values the fragment reads out of those images. More than
    /// [`Self::textures`] where scalars were packed.
    pub bindings: usize,
    /// Planes the static half has to rasterise, intermediates included. More
    /// than [`Self::bindings`] wherever one plane feeds another — a blur's
    /// input is a plane that is never uploaded.
    pub planes: usize,
    /// Offset evaluations the neighbourhood taps make: four per
    /// [`Filter::Normal`] left on the runtime side, which is what a finite
    /// difference costs without a plane to take it over.
    pub taps: usize,
    /// What those taps cost before anything is shared: [`Self::taps`] times the
    /// size of the sub-expression each one re-evaluates.
    ///
    /// A ceiling rather than an addition to [`Self::ops`]. The copies go
    /// through the same common-subexpression elimination as everything else, so
    /// a height that is one multiply over a bound texture costs four extra
    /// multiplies rather than four extra graphs; [`Self::ops`] is what they
    /// actually came to, and this is what they would have come to had nothing
    /// collapsed.
    pub tap_ops: usize,
    /// Live parameters, and so the size of the uniform block.
    pub params: usize,
    /// Parameters the author asked to keep live that had to be folded, because
    /// they reach a filter that can only be a baked plane.
    pub frozen: Vec<String>,
    /// Whether the fragment's roughness widens with distance the way a baked
    /// material's would — which is a narrower question than whether it widens.
    ///
    /// True in the two ways the two sides agree. A material that binds a normal
    /// map widens: its chain carries the footprint's coherence in its alpha,
    /// and the fragment adds the Toksvig term the bake would have folded in. It
    /// costs six instructions and no extra fetch — the coherence is the alpha
    /// of a read the shader already makes — so it is in neither [`Self::ops`]
    /// nor [`Self::samples`], both of which count the IR. A material with no
    /// `normal` output agrees the other way: its normal plane is `[0, 0, 1]` at
    /// every texel, the mean of that is a unit vector at every level, the
    /// bake's own Toksvig term is `(1 - 1) * k`, and both sides ship the plain
    /// box filter of the roughness. Nothing widens, and nothing was supposed
    /// to.
    ///
    /// False only where the bake would have widened and the fragment cannot:
    /// a normal computed per fragment is four taps of a live height *at the
    /// level being drawn*, which answers a direction and carries no record of
    /// what a lower level averaged away. There is no `|n_avg|` to read, the
    /// surface keeps its level-0 roughness at every distance, and that is a
    /// glint that outstays its detail. The only honest thing to do about it is
    /// say so here.
    ///
    /// Whether the emitter writes the six instructions is a different fact, and
    /// [`Partition::widening`] is where it lives.
    pub widens: bool,
    /// One line an author can read: `"no live inputs: bake this"` when the
    /// fragment computes nothing, and the cost otherwise.
    pub verdict: String,
}

impl std::fmt::Display for CostReport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        writeln!(f, "{}", self.verdict)?;
        writeln!(
            f,
            "  {} ops and {} texture reads per fragment, {} uniform(s)",
            self.ops, self.samples, self.params
        )?;
        writeln!(
            f,
            "  {} bound value(s) in {} texture(s), from {} plane(s)",
            self.bindings, self.textures, self.planes
        )?;
        if self.taps > 0 {
            writeln!(
                f,
                "  {} neighbourhood taps, at most {} ops before sharing",
                self.taps, self.tap_ops
            )?;
        }
        for (port, ops) in &self.ops_per_output {
            if *ops > 0 {
                writeln!(f, "  {port}: {ops} ops")?;
            }
        }
        // [`CostReport::widens`] is already the agreement rather than the
        // mechanism, so a flat normal plane — which neither side widens — never
        // reaches here. What is left is the real divergence, and it is only
        // worth printing where the graph bound a roughness at all: a graph that
        // binds none lets `StandardMaterial`'s own constant through, and telling
        // an author that a field they did not write does not widen is noise.
        if !self.widens && self.ops_per_output.contains_key("roughness") {
            writeln!(
                f,
                "  roughness does not widen with distance: nothing binds a normal map"
            )?;
        }
        if !self.frozen.is_empty() {
            writeln!(
                f,
                "  frozen (reaches a filter that must be a plane, or a strand layer): {}",
                self.frozen.join(", ")
            )?;
        }
        Ok(())
    }
}

/// A graph split into what a texture can hold and what a fragment must compute.
#[derive(Clone, Debug, PartialEq)]
pub struct Partition {
    runtime: Ir,
    textures: Vec<BoundTexture>,
    outputs: BTreeMap<String, OutputSource>,
    report: CostReport,
    live: Vec<String>,
    frozen: Vec<String>,
    resolution: u32,
    widening: Option<BufferId>,
}

impl Partition {
    /// The runtime expression: [`Op::Param`], the runtime inputs, and samples
    /// of the bound textures.
    ///
    /// Its [`Ir::buffers`] are [`Self::textures`], in the same order, so
    /// [`rasterise_buffers`](crate::planes::rasterise_buffers) over this IR at
    /// [`Self::resolution`] bakes the static half.
    pub fn runtime(&self) -> &Ir {
        &self.runtime
    }

    /// Every bound texture, in [`BufferId`] order.
    pub fn textures(&self) -> &[BoundTexture] {
        &self.textures
    }

    /// How many images the bindings pack into.
    pub fn images(&self) -> usize {
        self.report.textures
    }

    /// Where one output comes from, by port name: the [`PbrOutput`](crate::PbrOutput)
    /// ports the graph bound, plus `normal` where it derives one.
    pub fn output(&self, port: &str) -> Option<OutputSource> {
        self.outputs.get(port).copied()
    }

    /// Every output's source, by port name.
    pub fn outputs(&self) -> &BTreeMap<String, OutputSource> {
        &self.outputs
    }

    /// What it costs.
    pub fn report(&self) -> &CostReport {
        &self.report
    }

    /// The parameters that stayed live, in uniform-block order: the names of
    /// [`Ir::params`] on [`Self::runtime`], index for index.
    pub fn live(&self) -> &[String] {
        &self.live
    }

    /// The parameters that could not stay live, and were folded instead.
    ///
    /// Two reasons put a name here. One is a filter that must be a plane, which
    /// is what the rest of this module is about. The other is a strand layer:
    /// the scatter folds every parameter a strand field reads, because a strand
    /// set is geometry, so such a name is frozen for the blades even where it
    /// is still live for the fragment. A name may therefore appear in
    /// [`Self::live`] as well, and that is the honest report of a material
    /// whose two halves answer differently.
    pub fn frozen(&self) -> &[String] {
        &self.frozen
    }

    /// Texels per repeat the bound textures are baked at.
    pub fn resolution(&self) -> u32 {
        self.resolution
    }

    /// The bound normal map whose alpha the fragment widens its roughness by,
    /// where there is one.
    ///
    /// A bake widens the roughness of every level by how short the mean of that
    /// level's normals got, and can only do it because it holds both planes at
    /// once. A partition binds separate images, so the coherence rides in the
    /// alpha of the normal map's own chain
    /// ([`bound_mips`](crate::bake::bound_mips)) and the emitter reads it back
    /// at the level the hardware chose. This is which image that is — the one
    /// the `normal` port is, when the port *is* an image.
    ///
    /// `None` where the material binds no normal map, computes its normal per
    /// fragment, or has no roughness to widen; the fragment then writes its
    /// level-0 roughness at every distance and [`CostReport::widens`] says so.
    pub fn widening(&self) -> Option<BufferId> {
        self.widening
    }

    /// Whether the fragment computes nothing: every output is a texture or a
    /// literal, and the material is its own bake.
    pub fn is_static(&self) -> bool {
        !self.outputs.values().any(|source| source.is_runtime())
    }
}

/// The port name the plane at `index` is dispatched under by a backend that
/// evaluates an expression in pieces.
///
/// A WGSL identifier, because it becomes a field of the generated surface
/// struct, and prefixed so that it can never collide with a
/// [`PbrOutput`](crate::PbrOutput) port.
pub fn plane_port(index: usize) -> String {
    format!("{PLANE_PREFIX}{index}")
}

/// Whether a port name is one [`plane_port`] made rather than a
/// [`PbrOutput`](crate::PbrOutput) port.
///
/// A backend that dispatched the planes and then wants the material's own
/// outputs asks with this, rather than knowing the prefix: the two lists live
/// in one [`Ir::roots`] and the spelling is this module's business.
pub fn is_plane_port(port: &str) -> bool {
    port.starts_with(PLANE_PREFIX)
}

/// What [`plane_port`] names a plane with: a WGSL identifier, since it becomes
/// a field of the generated surface struct, and one no PBR port can collide
/// with.
const PLANE_PREFIX: &str = "ashlar_plane_";

/// Everything a compute backend needs to evaluate a lowered expression over
/// its own planes: the whole of it as the "runtime" half, with every plane as
/// a bound texture and every plane's root named as a port.
///
/// This is the GPU bake's shape, and it is the [`partition`] above turned
/// inside out. There, the *shader* is the small half and the planes are baked
/// on the CPU; here nothing is folded into a texture because nothing has to be
/// — a compute dispatch has the whole expression — and the planes are bound
/// only because a buffered filter reads a neighbourhood and is a plane in
/// every backend. So the frontier is not cut at all: [`Partition::runtime`] is
/// the expression that came in, and the only thing added is a name per plane,
/// so that a kernel can be written for one plane at a time and the filters can
/// run between the dispatches.
///
/// The bound textures are all [`PlaneFormat::Rgba16Float`], which is what
/// the cut answers for a plane that is nobody's PBR output. That is the
/// format the emitter decodes *nothing* for, which is what a backend uploading
/// its planes as raw `f32` wants: a bake's intermediate plane is the numbers it
/// computed, and quantising it on the way to the next dispatch would be a
/// second bake rather than the same one.
///
/// `resolution` is the texel count a plane that pinned none, and the outputs,
/// are evaluated at.
///
/// ```
/// use ashlar_material::{
///     MaterialGraph, PbrOutput,
///     ir::{Target, lower},
///     nodes::{Blur, Noise},
///     partition::{over_planes, plane_port},
/// };
///
/// let material = MaterialGraph::builder("test:soft")
///     .node("grain", Noise::value().period(8))
///     .node("soft", Blur::new("grain").radius(0.02))
///     .output(PbrOutput::new().roughness("soft"))
///     .build()?;
/// let ir = lower(&material, Target::Bake)?;
/// let split = over_planes(&ir, 256)?;
///
/// // One plane, named as a port beside the graph's own roughness.
/// assert_eq!(split.textures().len(), 1);
/// assert!(split.runtime().root(&plane_port(0)).is_some());
/// assert!(split.runtime().root("roughness").is_some());
/// # Ok::<(), ashlar_material::GraphError>(())
/// ```
pub fn over_planes(ir: &Ir, resolution: u32) -> Result<Partition, GraphError> {
    require(
        crate::bake::in_range(resolution),
        "resolution",
        &format!(
            "{resolution} is not a power of two between {MIN_RESOLUTION} and {MAX_RESOLUTION}"
        ),
    )?;
    let planes: Vec<(String, ValueId)> = ir
        .buffers()
        .iter()
        .enumerate()
        .map(|(index, plan)| (plane_port(index), plan.root))
        .collect();
    let runtime = ir.clone().rooted_at(planes)?;
    let outputs: BTreeMap<String, OutputSource> = runtime
        .roots()
        .iter()
        .map(|(port, root)| (port.clone(), OutputSource::Runtime(*root)))
        .collect();
    let textures = bind_textures(&runtime, &outputs)?;
    // No widening here, and it is not an omission. This split is a *bake* in
    // pieces: what it answers is level 0, and the chain — the box filter, the
    // renormalisation and the Toksvig term over it — is what happens to level 0
    // afterwards, on the CPU, in [`mips`](crate::mips). A kernel that widened
    // its own answer would be widening a level that has lost nothing yet. So it
    // agrees with the bake by *being* it, and reports so: this is the one place
    // where nothing widens and nothing should, without a flat normal saying it.
    let report = report(&runtime, &outputs, &textures, 0, 0, Vec::new(), true);
    let live = runtime
        .params()
        .iter()
        .map(|binding| binding.name.clone())
        .collect();
    Ok(Partition {
        runtime,
        textures,
        outputs,
        report,
        live,
        frozen: Vec::new(),
        resolution,
        widening: None,
    })
}

/// Split a material for a shader target.
///
/// `resolution` is what the bound textures are baked at, under the same bounds
/// a bake is: a power of two in
/// [`MIN_RESOLUTION`]`..=`[`MAX_RESOLUTION`],
/// and at least as many texels as the graph's finest lattice has cells. It is
/// also the epsilon the neighbourhood taps use, because the CPU bake's own
/// central difference is over two texels of a plane that size and the bake is
/// the reference.
///
/// The live set is the target's list *and* every parameter the graph exposes as
/// [`Exposure::Live`]. The author's declaration is a floor: a parameter marked
/// live moves, and a target that forgot to list it is not a reason to bake it
/// in. A name in the target that the graph does not declare is an error at
/// `params[name]`, the way a bake's would be, rather than a slider that
/// silently does nothing.
///
/// ```
/// use ashlar_material::{
///     MaterialGraph, Param, PbrOutput,
///     ir::Target,
///     nodes::{Math, MathOp, Noise, Time},
///     partition::partition,
/// };
///
/// // An emissive strip that pulses: one multiply per fragment over a baked
/// // colour, and nothing else.
/// let material = MaterialGraph::builder("test:strip")
///     .node("glow", Noise::value().period(8))
///     .node("clock", Time::new())
///     .node("pulse", Math::new(MathOp::Sin, "clock", 0.0))
///     .node("lit", Math::new(MathOp::Mul, "glow", "pulse"))
///     .output(PbrOutput::new().emissive("lit"))
///     .build()?;
///
/// let split = partition(&material, &Target::shader_for(&material), 256)?;
/// assert!(!split.is_static());
/// // The noise stayed in a texture; the clock did not.
/// assert_eq!(split.report().textures, 1);
/// assert_eq!(split.report().params, 0);
/// # Ok::<(), ashlar_material::GraphError>(())
/// ```
pub fn partition(
    material: &Material,
    target: &Target,
    resolution: u32,
) -> Result<Partition, GraphError> {
    let Target::Shader { live } = target else {
        return Err(GraphError::new(
            "target",
            "a partition splits a shader; Target::Bake is the whole graph baked, and `bake` is \
             what runs it",
        ));
    };
    require(
        crate::bake::in_range(resolution),
        "resolution",
        &format!(
            "{resolution} is not a power of two between {MIN_RESOLUTION} and {MAX_RESOLUTION}"
        ),
    )?;
    let lattice = material.finest_lattice();
    require(
        lattice[0] <= resolution && lattice[1] <= resolution,
        "resolution",
        &format!(
            "{resolution} texels cannot carry a lattice of {lattice:?} cells; bind the textures \
             at the lattice or above, or coarsen the graph"
        ),
    )?;
    let mut wanted: BTreeSet<String> = BTreeSet::new();
    for name in live {
        require(
            material.param(name).is_some(),
            &format!("params[{name}]"),
            &format!(
                "graph {:?} has no parameter {name:?} to keep live",
                material.graph().id
            ),
        )?;
        wanted.insert(name.clone());
    }
    wanted.extend(
        material
            .graph()
            .params
            .iter()
            .filter(|param| param.exposure == Exposure::Live)
            .map(|param| param.name.clone()),
    );
    let Settled {
        ir,
        live,
        mut frozen,
    } = settle(material, wanted)?;
    frozen.extend(folded_by_strands(material, &live));
    frozen.sort();
    frozen.dedup();
    cut(material, &ir, resolution, live, frozen)
}

/// The live parameters a strand layer's fields read, which every scatter folds.
///
/// A partition lowers the PBR half of the graph and nothing else, so it cannot
/// see a strand field at all. The scatter can, and it folds: a strand set is
/// geometry, and geometry is rebuilt rather than driven by a uniform, which
/// [`strands::scatter`](crate::strands::scatter) says at length. So a parameter
/// that stays live for the fragment is still frozen for the blades, and a
/// caller that was told only the first would ship a slider that moves the
/// colour of a lawn's texture and not the colour of its geometry.
///
/// A name here may therefore also be in [`Partition::live`], and that is the
/// honest report rather than a contradiction: the two halves of a material with
/// strands in it answer differently, and this is the only place that says so.
fn folded_by_strands(material: &Material, live: &[String]) -> Vec<String> {
    if material.graph().strands.is_empty() {
        return Vec::new();
    }
    let mut folded: BTreeSet<String> = BTreeSet::new();
    for layer in material.graph().strands.values() {
        for name in crate::strands::params_reached(material, layer) {
            if live.contains(&name) {
                folded.insert(name);
            }
        }
    }
    folded.into_iter().collect()
}

/// The lowering a partition works from, once no plane depends on a uniform.
struct Settled {
    ir: Ir,
    live: Vec<String>,
    frozen: Vec<String>,
}

/// Lower, and fold back any parameter that reaches a filter only a plane can
/// run, until nothing does.
///
/// Folding a parameter can only make more of the expression static, so the set
/// of parameters to freeze shrinks the live set monotonically and the loop
/// stops after at most one pass per parameter. When it stops there is no plan
/// left whose root is runtime and whose filter needs a plane, which is what the
/// cut below relies on.
fn settle(material: &Material, mut wanted: BTreeSet<String>) -> Result<Settled, GraphError> {
    let mut frozen: BTreeSet<String> = BTreeSet::new();
    loop {
        let live: Vec<String> = wanted.iter().cloned().collect();
        let ir = lower(material, Target::Shader { live: live.clone() })?;
        let runtime = colour(&ir);
        let mut freeze: BTreeSet<String> = BTreeSet::new();
        for plan in ir.buffers() {
            let roots = plan_roots(&ir, plan);
            let moves = roots
                .iter()
                .any(|root| runtime.get(root.index()).copied() == Some(true));
            if emittable(plan.filter, plan.resolution) || !moves {
                continue;
            }
            let reached = behind(&ir, &roots);
            let mut named = false;
            for (index, inst) in ir.insts().iter().enumerate() {
                if reached.get(index).copied() != Some(true) {
                    continue;
                }
                if let Op::Param(binding) = inst.op
                    && let Some(slot) = ir.params().get(binding as usize)
                {
                    freeze.insert(slot.name.clone());
                    named = true;
                }
            }
            // No uniform to fold means a runtime *input* reaches the filter,
            // and there is nothing to fold a clock down to.
            require(
                named,
                &plan.path,
                "this filter reads a whole neighbourhood, so it is a plane baked once, and a \
                 runtime input cannot reach one; move the filter above the runtime input, or \
                 bake the material",
            )?;
        }
        if freeze.is_empty() {
            // The bindings rather than the names asked for, and in their order:
            // `Partition::live` is what the uniform block holds, index for
            // index, so a caller reading the two together cannot pair a name
            // with somebody else's slot.
            let live = ir
                .params()
                .iter()
                .map(|binding| binding.name.clone())
                .collect();
            return Ok(Settled {
                ir,
                live,
                frozen: frozen.into_iter().collect(),
            });
        }
        let mut shrank = false;
        for name in &freeze {
            shrank |= wanted.remove(name);
        }
        // Every name in `freeze` came out of `Ir::params`, which is the live
        // set, so removing them always shrinks it and the loop always stops.
        // This is the belt on that: a lowering that ever handed back a binding
        // nobody asked for would otherwise spin here for ever.
        require(
            shrank,
            "params",
            "a plane depends on a uniform this partition did not declare",
        )?;
        frozen.extend(freeze);
    }
}

/// Which values depend on something a texture cannot hold.
///
/// One forward sweep: an operand is always an earlier instruction, and so is
/// the root of the plane an [`Op::Sample`] reads, so everything this needs is
/// already decided by the time it is read. A sample carries what its *plane*
/// depends on as well as what its coordinate does, which is the one arm that is
/// not just "any operand": the plane is baked, so a uniform upstream of it
/// would be frozen into the texture rather than read from the block.
fn colour(ir: &Ir) -> Vec<bool> {
    let mut runtime = vec![false; ir.len()];
    for (index, inst) in ir.insts().iter().enumerate() {
        let source = matches!(
            inst.op,
            Op::Param(_) | Op::Time | Op::WorldPos | Op::WorldNormal | Op::CutFlag
        );
        let carried = inst
            .operands
            .as_slice()
            .iter()
            .any(|operand| runtime.get(operand.index()).copied() == Some(true));
        let planed = match inst.op {
            Op::Sample(buffer) => ir.buffers().get(buffer.index()).is_some_and(|plan| {
                // Every root the plane's contents depend on, which for a slope
                // blur is the height it walks down as well as the field it
                // smears: both are baked into the one plane this samples.
                plan_roots(ir, plan)
                    .iter()
                    .any(|root| runtime.get(root.index()).copied() == Some(true))
            }),
            _ => false,
        };
        if let Some(slot) = runtime.get_mut(index) {
            *slot = source || carried || planed;
        }
    }
    runtime
}

/// Which static values are cheaper to write out than to bind.
///
/// A constant, the coordinate, and anything built out of those two by composing
/// and extracting lanes. A texture of a constant is a constant, and a texture of
/// the coordinate is the coordinate; binding either would spend an image and
/// quantise a value that was exact.
fn cheap(ir: &Ir) -> Vec<bool> {
    let mut free = vec![false; ir.len()];
    for (index, inst) in ir.insts().iter().enumerate() {
        let kind = matches!(
            inst.op,
            Op::Const(_) | Op::Uv | Op::Compose | Op::Extract(_)
        );
        let operands = inst
            .operands
            .as_slice()
            .iter()
            .all(|operand| free.get(operand.index()).copied() == Some(true));
        if let Some(slot) = free.get_mut(index) {
            *slot = kind && operands;
        }
    }
    free
}

/// Whether a filter can be evaluated per fragment instead of baked.
///
/// [`Filter::Normal`] can, as four taps of its height — that is what a finite
/// difference at texel size is without a plane to take it over. A
/// [`Filter::None`] with no pinned resolution is a cache point rather than a
/// filter, and inlining it computes exactly the value it held. Everything else
/// reads a neighbourhood whose size is a reach in UV rather than a tap count,
/// and there is no honest number of taps for it.
fn emittable(filter: Filter, resolution: Option<u32>) -> bool {
    match filter {
        Filter::Normal { .. } => true,
        Filter::None => resolution.is_none(),
        _ => false,
    }
}

/// How many instructions one sub-expression is, in the lowering it came from.
///
/// [`Ir::reaches`] does not follow a sample into the plane it reads, which is
/// what this wants: a tap re-evaluates the expression and re-reads the plane,
/// it does not rasterise it again.
fn size(ir: &Ir, root: ValueId) -> usize {
    ir.reaches(&[root]).into_iter().filter(|live| *live).count()
}

/// What one value reaches, *through* the planes it samples.
///
/// The other reach, and the one the freeze scan needs: a plane is baked, so a
/// uniform two planes upstream of a blur is as unbakeable as one wired straight
/// into it, and asking which parameters a plane's contents depend on means
/// following every sample inside it down to its own root. Operands and plan
/// roots are both always earlier instructions, so one backwards sweep marks
/// everything.
fn behind(ir: &Ir, roots: &[ValueId]) -> Vec<bool> {
    let mut live = vec![false; ir.len()];
    for root in roots {
        if let Some(slot) = live.get_mut(root.index()) {
            *slot = true;
        }
    }
    for index in (0..ir.len()).rev() {
        if live.get(index).copied() != Some(true) {
            continue;
        }
        let Some(inst) = ir.insts().get(index) else {
            continue;
        };
        let mut mark = |value: ValueId| {
            if let Some(slot) = live.get_mut(value.index()) {
                *slot = true;
            }
        };
        for operand in inst.operands.as_slice() {
            mark(*operand);
        }
        if let Op::Sample(buffer) = inst.op
            && let Some(plan) = ir.buffers().get(buffer.index())
        {
            for root in plan_roots(ir, plan) {
                mark(root);
            }
        }
    }
    live
}

/// Every value one plane's contents depend on: its own root, and the root of
/// every plane its filter reads.
///
/// One entry for all but a slope blur, which is the one filter that reads a
/// second plane. A guide is always an earlier plan, so the walk terminates, and
/// a guide's root is always an earlier instruction than the plan that names it,
/// which is what lets the two sweeps above read this in their own order.
fn plan_roots(ir: &Ir, plan: &BufferPlan) -> Vec<ValueId> {
    let mut roots = vec![plan.root];
    let mut next = plan.filter.guide();
    while let Some(id) = next.take() {
        let Some(guide) = ir.buffers().get(id.index()) else {
            break;
        };
        roots.push(guide.root);
        next = guide.filter.guide();
    }
    roots
}

/// Which side of the cut an output landed on, before the ids were renumbered.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Side {
    Texture,
    Runtime,
    Inline,
}

/// The cut itself: one forward pass that rebuilds the expression with the
/// static frontier replaced by samples of planes.
struct Cut<'a> {
    ir: &'a Ir,
    runtime: Vec<bool>,
    cheap: Vec<bool>,
    cx: Lowering,
    /// The copy of each static instruction, in the new arena.
    statics: Vec<ValueId>,
    /// The runtime translation of each runtime instruction.
    live: Vec<ValueId>,
    /// The plane each static frontier value was bound into.
    frontier: Vec<Option<BufferId>>,
    /// The plane each of the old lowering's plans became.
    planes: Vec<Option<BufferId>>,
    resolution: u32,
    taps: usize,
    tap_ops: usize,
}

impl<'a> Cut<'a> {
    fn new(ir: &'a Ir, resolution: u32) -> Self {
        Self {
            ir,
            runtime: colour(ir),
            cheap: cheap(ir),
            cx: Lowering::with_bindings(ir.target().clone(), ir.params().to_vec()),
            statics: vec![ValueId::default(); ir.len()],
            live: vec![ValueId::default(); ir.len()],
            frontier: vec![None; ir.len()],
            planes: vec![None; ir.buffers().len()],
            resolution,
            taps: 0,
            tap_ops: 0,
        }
    }

    fn is_runtime(&self, value: ValueId) -> bool {
        self.runtime.get(value.index()).copied() == Some(true)
    }

    fn is_cheap(&self, value: ValueId) -> bool {
        self.cheap.get(value.index()).copied() == Some(true)
    }

    /// Rebuild every instruction, statics into the arena and runtime ops on top
    /// of them. Operands are always earlier, so one forward pass is enough and
    /// nothing here recurses.
    fn sweep(&mut self) {
        for index in 0..self.ir.len() {
            let Some(inst) = self.ir.insts().get(index).copied() else {
                continue;
            };
            let value = if self.runtime.get(index).copied() == Some(true) {
                self.emit_runtime(inst)
            } else {
                self.copy_static(inst)
            };
            let slot = if self.runtime.get(index).copied() == Some(true) {
                self.live.get_mut(index)
            } else {
                self.statics.get_mut(index)
            };
            if let Some(slot) = slot {
                *slot = value;
            }
        }
    }

    /// One static instruction, copied into the new arena so that a plane can be
    /// rooted at it.
    fn copy_static(&mut self, inst: crate::ir::Inst) -> ValueId {
        let operands: Vec<ValueId> = inst
            .operands
            .as_slice()
            .iter()
            .map(|operand| {
                self.statics
                    .get(operand.index())
                    .copied()
                    .unwrap_or_default()
            })
            .collect();
        match inst.op {
            Op::Sample(buffer) => {
                let plane = self.plane(buffer);
                let uv = operands.first().copied().unwrap_or_default();
                self.cx.sample(plane, uv)
            }
            op => self.cx.emit(op, Operands::from_ids(&operands)),
        }
    }

    /// The plane one of the old lowering's plans becomes, rooted at the copy of
    /// its own sub-expression.
    fn plane(&mut self, buffer: BufferId) -> BufferId {
        if let Some(Some(id)) = self.planes.get(buffer.index()).copied() {
            return id;
        }
        let Some(plan) = self.ir.buffers().get(buffer.index()).cloned() else {
            return BufferId::default();
        };
        let root = self
            .statics
            .get(plan.root.index())
            .copied()
            .unwrap_or_default();
        // A filter that names a second plane — a slope blur, and nothing else
        // today — names it by an index into the *old* lowering's plan list, so
        // it is rebuilt here like any other dependency. The guide is an earlier
        // plan than this one, so the recursion is as deep as the chain of
        // guides and no deeper.
        let filter = match plan.filter.guide() {
            Some(guide) => plan.filter.with_guide(self.plane(guide)),
            None => plan.filter,
        };
        self.cx.at(plan.path.clone(), "a buffered filter");
        // A strand plane carries its scatter rather than its contents, and the
        // scatter is what says what the plane holds: rebuilding the plan
        // without it would compile a lawn into a texture of zeroes.
        let id = match plan.strands.clone() {
            Some(strands) => self.cx.strand_buffer(root, filter, strands),
            None => self.cx.buffer(root, filter, plan.resolution),
        };
        if let Some(slot) = self.planes.get_mut(buffer.index()) {
            *slot = Some(id);
        }
        id
    }

    /// The value a runtime op reads for one operand: the operand's own runtime
    /// translation, a cheap static written out again, or a sample of the plane
    /// the static frontier was baked into.
    fn operand(&mut self, value: ValueId) -> ValueId {
        if self.is_runtime(value) {
            return self.live.get(value.index()).copied().unwrap_or_default();
        }
        if self.is_cheap(value) {
            return self.statics.get(value.index()).copied().unwrap_or_default();
        }
        let plane = self.bind(value, FRONTIER.to_owned());
        let uv = self.cx.uv();
        self.cx.sample(plane, uv)
    }

    /// Bind one static value as a plane of its own, once.
    fn bind(&mut self, value: ValueId, path: String) -> BufferId {
        if let Some(Some(id)) = self.frontier.get(value.index()).copied() {
            return id;
        }
        let root = self.statics.get(value.index()).copied().unwrap_or_default();
        self.cx.at(path, "a bound texture");
        let id = self.cx.buffer(root, Filter::None, None);
        if let Some(slot) = self.frontier.get_mut(value.index()) {
            *slot = Some(id);
        }
        id
    }

    /// One runtime instruction, with its operands cut.
    fn emit_runtime(&mut self, inst: crate::ir::Inst) -> ValueId {
        if let Op::Sample(buffer) = inst.op {
            return self.emit_sample(buffer, inst.operands);
        }
        let operands: Vec<ValueId> = inst
            .operands
            .as_slice()
            .iter()
            .map(|operand| self.operand(*operand))
            .collect();
        self.cx.emit(inst.op, Operands::from_ids(&operands))
    }

    /// A sample the runtime side kept: either of a plane whose contents are
    /// static, or of one that is not a plane at all any more.
    fn emit_sample(&mut self, buffer: BufferId, operands: Operands) -> ValueId {
        let Some(plan) = self.ir.buffers().get(buffer.index()).cloned() else {
            return self
                .cx
                .reject("a sample named a plane that was never planned");
        };
        let coordinate = operands.as_slice().first().copied().unwrap_or_default();
        let uv = self.operand(coordinate);
        if !self.is_runtime(plan.root) {
            let plane = self.plane(buffer);
            return self.cx.sample(plane, uv);
        }
        let source = self
            .live
            .get(plan.root.index())
            .copied()
            .unwrap_or_default();
        match plan.filter {
            Filter::Normal { strength } => {
                self.taps += 4;
                self.tap_ops += 4 * size(self.ir, plan.root);
                self.normal(source, uv, strength)
            }
            // A cache point with no plane behind it is its own input.
            Filter::None if plan.resolution.is_none() => self.cx.substitute(source, uv),
            _ => {
                self.cx.at(plan.path.clone(), "a buffered filter");
                self.cx.reject(
                    "this filter is a plane baked once, and what reaches it moves per frame; \
                     move the filter above the live parameter, or bake the material",
                )
            }
        }
    }

    /// The tangent-space normal of a runtime height, by the four offset taps the
    /// bake's own central difference takes over a plane.
    ///
    /// Same span, same sign, same `normalize(vec3(x, y, 1))` as
    /// [`planes`](crate::planes) — the bake is the reference, and a partition
    /// that differenced over a different distance would be a different surface.
    /// See [`Self::tap`] for why the offset coordinate is not wrapped.
    ///
    /// One difference against the bake is left standing. `planes::unit` guards a
    /// non-finite length and answers a flat normal, on the grounds that a height
    /// of infinity is a graph to fix rather than a normal of `NaN`; this
    /// derivation is [`Op::Normalize`], which is `NaN` there, and so is the
    /// shader's. A comparison would guard it, except that WGSL permits an
    /// implementation to assume infinities and `NaN`s never occur, so the guard
    /// would be a promise the shader side cannot keep. The bake's flat normal is
    /// a courtesy rather than a contract, and this says so rather than pretending
    /// the two agree.
    fn normal(&mut self, height: ValueId, uv: ValueId, strength: f32) -> ValueId {
        // The bake's two taps are two texels apart over one UV unit.
        let texel = 1.0 / f32::from(u16::try_from(self.resolution).unwrap_or(u16::MAX)).max(1.0);
        let right = self.tap(height, uv, texel, 0.0);
        let left = self.tap(height, uv, -texel, 0.0);
        let down = self.tap(height, uv, 0.0, texel);
        let up = self.tap(height, uv, 0.0, -texel);
        let span = k(&mut self.cx, 2.0 * texel);
        let scale = k(&mut self.cx, -strength);
        let du = sub(&mut self.cx, right, left);
        let du = div(&mut self.cx, du, span);
        let x = mul(&mut self.cx, du, scale);
        let dv = sub(&mut self.cx, down, up);
        let dv = div(&mut self.cx, dv, span);
        let y = mul(&mut self.cx, dv, scale);
        let one = k(&mut self.cx, 1.0);
        let vector = self.cx.vector(&[x, y, one]);
        normalize(&mut self.cx, vector)
    }

    /// One tap: the same expression, read a texel away.
    ///
    /// The offset coordinate is *not* wrapped, for the same reason the
    /// fragment's own coordinate is not: period inference already guarantees
    /// the expression repeats with period one, so a `fract` changes no value,
    /// and it puts a derivative discontinuity along every seam — which is a
    /// line of the wrong mip on whatever the tap samples. A plane a tap reads
    /// is sampled with a repeating sampler, and the lattice hash reduces its
    /// cell with a Euclidean remainder, so a coordinate a hair past one is
    /// already the coordinate a hair past zero.
    fn tap(&mut self, height: ValueId, uv: ValueId, du: f32, dv: f32) -> ValueId {
        let offset = k2(&mut self.cx, du, dv);
        let moved = add(&mut self.cx, uv, offset);
        self.cx.substitute(height, moved)
    }
}

/// Rebuild the expression either side of the frontier, and describe what came
/// out.
fn cut(
    material: &Material,
    ir: &Ir,
    resolution: u32,
    live: Vec<String>,
    frozen: Vec<String>,
) -> Result<Partition, GraphError> {
    let mut cut = Cut::new(ir, resolution);
    cut.sweep();
    let uv = cut.cx.uv();
    let mut roots: Vec<(String, ValueId)> = Vec::new();
    let mut sides: BTreeMap<String, Side> = BTreeMap::new();
    for (port, root) in ir.roots() {
        let (value, side) = if cut.is_runtime(*root) {
            (
                cut.live.get(root.index()).copied().unwrap_or_default(),
                Side::Runtime,
            )
        } else if cut.is_cheap(*root) {
            (
                cut.statics.get(root.index()).copied().unwrap_or_default(),
                Side::Inline,
            )
        } else {
            let plane = cut.bind(*root, format!("output.{port}"));
            (cut.cx.sample(plane, uv), Side::Texture)
        };
        roots.push((port.clone(), value));
        sides.insert(port.clone(), side);
    }
    let strength = material.graph().output.normal_strength;
    if let Some(height) = ir.root("height")
        && strength != 0.0
    {
        let (value, side) = if cut.is_runtime(height) {
            cut.taps += 4;
            cut.tap_ops += 4 * size(ir, height);
            let source = cut.live.get(height.index()).copied().unwrap_or_default();
            (cut.normal(source, uv, strength), Side::Runtime)
        } else {
            let root = cut.statics.get(height.index()).copied().unwrap_or_default();
            cut.cx.at("output.normal".to_owned(), "a bound texture");
            let plane = cut.cx.buffer(root, Filter::Normal { strength }, None);
            (cut.cx.sample(plane, uv), Side::Texture)
        };
        roots.push(("normal".to_owned(), value));
        sides.insert("normal".to_owned(), side);
    }
    let (taps, tap_ops) = (cut.taps, cut.tap_ops);
    let runtime = cut.cx.finish(roots)?;
    let outputs = sources(&runtime, &sides);
    let textures = bind_textures(&runtime, &outputs)?;
    let widening = widening(&outputs, &textures);
    let report = report(
        &runtime,
        &outputs,
        &textures,
        taps,
        tap_ops,
        frozen.clone(),
        widens_like_the_bake(&outputs, widening),
    );
    Ok(Partition {
        runtime,
        textures,
        outputs,
        report,
        live,
        frozen,
        resolution,
        widening,
    })
}

/// Where each output ended up, once the dead instructions were dropped and the
/// ids renumbered.
///
/// The side each root landed on was decided before the renumbering, and the
/// value it landed on is whatever [`Ir::root`] now answers, so this is a read
/// of the finished expression rather than a map carried across it.
fn sources(runtime: &Ir, sides: &BTreeMap<String, Side>) -> BTreeMap<String, OutputSource> {
    let mut outputs = BTreeMap::new();
    for (port, side) in sides {
        let Some(root) = runtime.root(port) else {
            continue;
        };
        let source = match side {
            Side::Runtime => OutputSource::Runtime(root),
            Side::Inline => OutputSource::Inline(root),
            // A bound output was emitted as exactly one sample of its own
            // plane; the fallback is a belt, not a case.
            Side::Texture => match runtime.inst(root).map(|inst| inst.op) {
                Some(Op::Sample(buffer)) => OutputSource::Texture(buffer),
                _ => OutputSource::Runtime(root),
            },
        };
        outputs.insert(port.clone(), source);
    }
    outputs
}

/// Which planes the fragment itself reads.
///
/// Not every plane: a blur's input is rasterised so that the blur can be, and
/// then dropped. Only the ones an [`Op::Sample`] the outputs reach names need
/// an image, and only those count against [`MAX_BOUND_TEXTURES`].
fn sampled(runtime: &Ir) -> Vec<bool> {
    let roots: Vec<ValueId> = runtime.roots().values().copied().collect();
    let seen = runtime.reaches(&roots);
    let mut read = vec![false; runtime.buffers().len()];
    for (index, inst) in runtime.insts().iter().enumerate() {
        if seen.get(index).copied() != Some(true) {
            continue;
        }
        if let Op::Sample(buffer) = inst.op
            && let Some(slot) = read.get_mut(buffer.index())
        {
            *slot = true;
        }
    }
    read
}

/// Give every plane the fragment reads an image and a channel, packing scalars
/// only when the images would otherwise not fit.
fn bind_textures(
    runtime: &Ir,
    outputs: &BTreeMap<String, OutputSource>,
) -> Result<Vec<BoundTexture>, GraphError> {
    let mut ports: Vec<Vec<&str>> = vec![Vec::new(); runtime.buffers().len()];
    for (port, source) in outputs {
        if let OutputSource::Texture(buffer) = source
            && let Some(slot) = ports.get_mut(buffer.index())
        {
            slot.push(port.as_str());
        }
    }
    let read = sampled(runtime);
    let bindings = read.iter().filter(|seen| **seen).count();
    // One image per binding unless that would not fit; then the scalars go four
    // to an image, which is the only packing that is free of a convention —
    // pairing a two-lane value with a one-lane one would make the layout depend
    // on the order the frontier happened to be found in.
    let packing = bindings > MAX_BOUND_TEXTURES;
    let mut textures = Vec::with_capacity(runtime.buffers().len());
    let mut next = 0_u32;
    let mut open: Option<(u32, u8)> = None;
    for plan in runtime.buffers() {
        let named = ports.get(plan.id.index()).map_or(&[][..], Vec::as_slice);
        if read.get(plan.id.index()).copied() != Some(true) {
            textures.push(BoundTexture {
                buffer: plan.id,
                id: None,
                lane: 0,
                root: plan.root,
                value_type: plan.value_type,
                filter: plan.filter,
                resolution: plan.resolution,
                format: format_of(None),
                port: None,
            });
            continue;
        }
        let packable = packing && plan.value_type == IrType::Float && plan.resolution.is_none();
        let (id, lane) = match open {
            Some((id, lane)) if packable && lane < LANES_PER_TEXTURE => {
                open = Some((id, lane + 1));
                (id, lane)
            }
            _ => {
                let id = next;
                next = next.saturating_add(1);
                open = packable.then_some((id, 1));
                (id, 0)
            }
        };
        // A plane serving two outputs at once has to be readable as both, and
        // the only format that reads as anything is the one that quantises
        // nothing. A plane that packs has the same problem against the other
        // lanes of its image, and the same answer.
        let format = match named {
            _ if packable => PACKED_FORMAT,
            [port] => format_of(Some(port)),
            [] => format_of(None),
            _ => PlaneFormat::Rgba16Float,
        };
        textures.push(BoundTexture {
            buffer: plan.id,
            id: Some(id),
            lane,
            root: plan.root,
            value_type: plan.value_type,
            filter: plan.filter,
            resolution: plan.resolution,
            format,
            port: named.first().map(|port| (*port).to_owned()),
        });
    }
    let images = usize::try_from(next).unwrap_or(usize::MAX);
    require(
        images <= MAX_BOUND_TEXTURES,
        "output",
        &format!(
            "this graph binds {images} textures, and a compiled material may bind \
             {MAX_BOUND_TEXTURES}; fold a live parameter back into the bake, or move it \
             downstream so that less of the graph is cut"
        ),
    )?;
    Ok(textures)
}

/// Whether the compiled roughness widens with distance the way the bake's
/// would, which is [`CostReport::widens`] and is not the same question as
/// whether [`widening`] found an image.
///
/// Two of the three ways to have no widening are agreement rather than
/// shortfall, and conflating them makes the report cry wolf on a shipped graph:
///
/// - **No roughness.** The material never bound the field, `StandardMaterial`'s
///   own constant goes through untouched, and there is nothing for either side
///   to have widened.
/// - **No normal.** A graph with `normal_strength` at zero writes no `normal`
///   output, so its normal plane is `[0, 0, 1]` everywhere. The mean of that
///   has length one at every level, the bake's Toksvig term is zero at every
///   level, and both sides ship the plain box filter of the roughness. The two
///   agree exactly, by widening nothing.
/// - **A normal the fragment computes.** Here the bake *would* widen and the
///   fragment has nothing to widen by. This is the one that is a difference,
///   and the one the report prints.
fn widens_like_the_bake(
    outputs: &BTreeMap<String, OutputSource>,
    widening: Option<BufferId>,
) -> bool {
    if !outputs.contains_key("roughness") || !outputs.contains_key("normal") {
        return true;
    }
    widening.is_some()
}

/// Which bound image, if any, carries the coherence the fragment widens its
/// roughness by.
///
/// Three things have to hold, and each is a real case rather than a guard.
///
/// - **There is a roughness to widen.** A material with no roughness port lets
///   `StandardMaterial`'s own constant through untouched, and multiplying a
///   widening into that would be this material inventing a field the graph
///   never bound.
/// - **The normal is an image rather than an expression.** A normal derived per
///   fragment from a live height is four taps of the height *at this level*: it
///   answers a direction and carries no record of what a lower level averaged
///   away, so there is no `|n_avg|` to read. The report says so; see
///   [`CostReport::widens`].
/// - **That image is one the encoder writes the coherence into.** The list is
///   the eight-bit RGBA pair and nothing else, which is what
///   `ashlar_bevy::shader::bound_image` hands to
///   [`encode_bound`](crate::bake::encode_bound) as a normal map. Naming the
///   same two formats here rather than excluding the one that has no alpha
///   keeps the two predicates from drifting apart: a format that grew an alpha
///   but that the encoder still did not treat as a normal map would otherwise
///   ship ones at every level while this said the fragment could widen by them.
///   Today `format_of` answers `Rgba8Unorm` for the `normal` port and a
///   three-lane plane never packs, so the fourth channel belongs to nobody else
///   and the question does not arise; this is the guard that keeps it that way.
fn widening(
    outputs: &BTreeMap<String, OutputSource>,
    textures: &[BoundTexture],
) -> Option<BufferId> {
    if !outputs.contains_key("roughness") {
        return None;
    }
    let OutputSource::Texture(buffer) = outputs.get("normal")? else {
        return None;
    };
    let texture = textures.get(buffer.index())?;
    let carries_coherence = texture.id.is_some()
        && matches!(
            texture.format,
            PlaneFormat::Rgba8Unorm | PlaneFormat::Rgba8Srgb
        );
    carries_coherence.then_some(*buffer)
}

/// What image format one bound plane goes into, when it has an image to itself.
///
/// A plane that is a PBR output goes into the format the bake writes that map
/// in, so a live material and its baked twin quantise the same way. An interior
/// cut is an arbitrary field — the graph never promised it stays inside `0..=1`
/// — so it goes into half floats, which clamp nothing.
///
/// A plane that shares an image with others does not get to choose:
/// see [`PACKED_FORMAT`].
fn format_of(port: Option<&str>) -> PlaneFormat {
    match port {
        Some("base_color") => PlaneFormat::Rgba8Srgb,
        Some("height") => PlaneFormat::R16Unorm,
        Some("normal" | "roughness" | "metallic" | "occlusion") => PlaneFormat::Rgba8Unorm,
        // `emissive`, the one output allowed past one, and every interior cut,
        // which is an arbitrary field. Half floats clamp neither.
        Some(_) | None => PlaneFormat::Rgba16Float,
    }
}

/// Count what the fragment does.
fn report(
    runtime: &Ir,
    outputs: &BTreeMap<String, OutputSource>,
    textures: &[BoundTexture],
    taps: usize,
    tap_ops: usize,
    frozen: Vec<String>,
    widens: bool,
) -> CostReport {
    let computed: Vec<ValueId> = outputs
        .values()
        .filter_map(|source| match source {
            OutputSource::Runtime(root) => Some(*root),
            _ => None,
        })
        .collect();
    let ops = runtime
        .reaches(&computed)
        .into_iter()
        .filter(|live| *live)
        .count();
    let ops_per_output = outputs
        .iter()
        .map(|(port, source)| {
            let ops = match source {
                OutputSource::Runtime(root) => size(runtime, *root),
                // A texture read into its slot and a literal are both free:
                // one is what a StandardMaterial already does, the other is a
                // number in the shader text.
                OutputSource::Texture(_) | OutputSource::Inline(_) => 0,
            };
            (port.clone(), ops)
        })
        .collect();
    let bound: Vec<ValueId> = runtime.roots().values().copied().collect();
    let seen = runtime.reaches(&bound);
    let samples = runtime
        .insts()
        .iter()
        .enumerate()
        .filter(|(index, inst)| {
            seen.get(*index).copied() == Some(true) && matches!(inst.op, Op::Sample(_))
        })
        .count();
    let images = textures
        .iter()
        .filter_map(|texture| texture.id)
        .collect::<BTreeSet<_>>()
        .len();
    let bindings = textures
        .iter()
        .filter(|texture| texture.id.is_some())
        .count();
    let params = runtime.params().len();
    let verdict = if computed.is_empty() {
        "no live inputs: bake this".to_owned()
    } else {
        format!(
            "live: {params} uniform(s), {ops} ops and {samples} texture reads per fragment over \
             {images} bound texture(s)"
        )
    };
    CostReport {
        ops,
        ops_per_output,
        samples,
        textures: images,
        bindings,
        planes: textures.len(),
        taps,
        tap_ops,
        params,
        frozen,
        verdict,
        widens,
    }
}
