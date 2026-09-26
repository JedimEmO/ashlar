//! The scatter: one strand layer into the deterministic set a mesh is built
//! from.
//!
//! A [`PbrOutput`](crate::PbrOutput) is one field over a continuous surface,
//! evaluated once per texel. A [`StrandLayer`] is the same graph read a
//! different way: once per *strand*, at the root, over a jittered lattice of
//! `count` cells that wraps at the repeat. What comes out is a [`StrandSet`] —
//! a list of strands over one repeat of the material, in rank order — and what
//! reads it is geometry rather than a texture.
//!
//! # What makes this deterministic
//!
//! Three things, and every one of them is a claim a test pins.
//!
//! The lattice is the one [`Tile`](crate::nodes::Tile) scatters on: a cell is
//! hashed by [`hash2`], the cells wrap, and a root is its
//! cell centre plus half a cell of jitter. So a scatter and a `Tile` of the same
//! count and seed put their instances in the same places, and both meet
//! themselves at the seam.
//!
//! The fields are evaluated by *point evaluation* rather than read off a baked
//! map, through the same [`Interpreter`] the bake runs, so a strand set does not
//! depend on the bake resolution. The one exception is a buffered filter
//! upstream of a field — a blur, a distance, the slope a
//! [`Direction`](crate::nodes::Direction) reads — which is a plane or it is
//! nothing; those are rasterised at [`StrandRequest::field_resolution`], which
//! is therefore part of what decides the answer.
//!
//! The rows of cells are divided across [`std::thread::scope`] threads, and a
//! strand reads nothing but its own cell and its own root, so the set does not
//! depend on how the rows were divided. The strands are then sorted by rank
//! with a stable sort over cell order, which is what makes a rank prefix — a
//! level of detail — a *subset* rather than a reshuffle.
//!
//! # What is refused
//!
//! A field that reaches [`Time`](crate::nodes::Time),
//! [`WorldPos`](crate::nodes::WorldPos),
//! [`WorldNormal`](crate::nodes::WorldNormal),
//! [`CutFlag`](crate::nodes::CutFlag), [`Triplanar`](crate::nodes::Triplanar)
//! or [`WorldMask`](crate::nodes::WorldMask) is refused at the node that asked,
//! the way a bake refuses a world position. A strand is placed before there is a
//! mesh under it or a frame around it, so none of those is a question the
//! scatter can answer; wind is the renderer's, and phase 4's vertex stage is
//! where it belongs. A [`Live`](crate::Exposure::Live) parameter is not refused
//! — it is folded at whatever value the request bound, because a strand set is
//! geometry and geometry is rebuilt rather than animated by a uniform.
//!
//! # What reads a set
//!
//! Two things here, and a third in phase 3. [`place`] plants the set on a mesh,
//! turning roots in UV into roots in metres with a tangent frame each, and
//! [`mesh`] turns those into the triangles that draw them. Both are plain
//! functions over plain arrays: a `StrandRelief` node and a card atlas read the
//! same set later, and none of the three is allowed to be the one that owns it.

mod relief;

use std::collections::{BTreeMap, BTreeSet};
use std::num::NonZeroUsize;

use crate::{
    GraphError, Input, Material, MaterialGraph, MaterialGraphLibrary, Node, ParamValue,
    StrandLayer,
    bake::{MAX_RESOLUTION, MIN_RESOLUTION},
    interp::{EvalError, Inputs, Interpreter, Plane, hash2},
    ir::{ValueId, lower_strands, strand_target},
    nodes::Subgraph,
    planes::{BakeCache, rasterise_buffers, try_rows_across_threads},
};

/// Texels per repeat for any plane a strand field reads.
///
/// A strand field is evaluated at its own root and wants no grid at all — until
/// it reaches a buffered filter, a blur or the slope a
/// [`Direction`](crate::nodes::Direction) reads, which is a plane or it is
/// nothing. Those are rasterised at this resolution.
///
/// It is a constant of this crate rather than a parameter of the splat, and
/// **that is the point**: the geometry and the relief have to scatter the *same*
/// strands, and a scatter whose field planes were rasterised at two different
/// sizes is two different scatters. A relief inside a 2048 bake would otherwise
/// read a 2048 slope where the mesh builder read a 512 one, and the blades in
/// the texture would lean differently from the blades standing on it — which is
/// precisely the disagreement this node exists to remove.
///
/// Five hundred and twelve rather than the bake's own resolution, and that is
/// deliberate too. Over a two-metre repeat this is four millimetres a texel,
/// which is about one blade wide: a strand layer cannot ask a field a finer
/// question than the blade it is answering for. Binding it to the bake instead
/// would make a lawn cost sixteen times as much to scatter for a difference
/// nothing can see, and would grow different grass at two texel densities.
///
/// [`StrandRequest::field_resolution`] may name another number, and a caller
/// that does takes the disagreement on: the relief is always splatted from a
/// scatter at *this* resolution, because a graph has no request to read.
pub const FIELD_RESOLUTION: u32 = 512;

pub use ashlar_strands::{
    BLADE_CAMBER, CardAtlas, CardRequest, FLOATS_PER_STRAND, PlacedStrand, Strand, StrandMesh,
    StrandSet, StrandShape, SurfaceTriangles, cards, file, mesh, place, tip_of, width_at,
};
pub(crate) use relief::splat;

/// What a scatter is asked for.
///
/// The shape [`BakeRequest`](crate::bake::BakeRequest) has, and for the same
/// reason: the graph is interchange data rather than a built
/// [`Material`], because the parameters in `params` replace
/// the graph's own defaults before validation and a replacement can be the
/// thing that is wrong.
#[derive(Clone, Copy, Debug)]
pub struct StrandRequest<'a> {
    /// The graph the layer belongs to.
    pub graph: &'a MaterialGraph,
    /// Where [`Subgraph`] nodes are resolved from.
    pub library: &'a MaterialGraphLibrary,
    /// Parameter values that override the graph's defaults, by name. A name the
    /// graph does not declare, or a value of the wrong type, is an error at
    /// `params[name]` rather than a silent no-op.
    pub params: &'a BTreeMap<String, ParamValue>,
    /// Which strand layer to scatter, by the name the graph declares it under.
    pub layer: &'a str,
    /// Texels per repeat for any plane a field reads.
    ///
    /// A field is evaluated at its root and needs no grid at all — until it
    /// reaches a buffered filter, which is a plane or it is nothing. Those are
    /// rasterised here, so this is part of what decides the answer and part of
    /// any key a caller caches a set under. A layer whose fields reach no
    /// buffered filter rasterises nothing and this costs it nothing.
    pub field_resolution: u32,
    /// How many threads to scatter the cell rows across, or `None` for as many
    /// as the work is worth. The set is the same either way.
    pub threads: Option<NonZeroUsize>,
}

/// Why a strand set was not scattered.
///
/// Non-exhaustive, as [`BakeError`](crate::bake::BakeError) is: a scatter that
/// grows a reason to refuse must not break a caller's match.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum StrandError {
    /// The graph did not validate, did not lower, was handed a parameter it
    /// does not declare, or bound a field to a strand layer that a strand
    /// layer cannot read. Carries the same path a build error carries.
    #[error("{0}")]
    Graph(#[from] GraphError),
    /// The requested resolution for the field planes is not a power of two in
    /// range.
    #[error(
        "field_resolution: {0} is not a power of two between {MIN_RESOLUTION} and {MAX_RESOLUTION}"
    )]
    Resolution(u32),
    /// The lowered expression could not be evaluated.
    ///
    /// Not something a graph can cause: every parameter is folded, so no
    /// [`Op::Param`](crate::ir::Op::Param) survives to be missing a value, and
    /// every plane is rasterised before the expression that samples it.
    #[error("{0}")]
    Eval(#[from] EvalError),
}

/// Scatter one strand layer over one repeat of its material.
///
/// ```
/// use std::num::NonZeroUsize;
/// use ashlar_material::{
///     MaterialGraph, MaterialGraphLibrary, PbrOutput, StrandLayer,
///     nodes::Noise,
///     strands::{StrandRequest, scatter},
/// };
///
/// let graph = MaterialGraph::builder("test:lawn")
///     .node("patches", Noise::value().period(4))
///     .output(PbrOutput::new().roughness("patches"))
///     .strands(
///         "blades",
///         StrandLayer::new().count(32).density("patches").metres(0.12, 0.003),
///     )
///     .into_graph();
///
/// let set = scatter(&StrandRequest {
///     graph: &graph,
///     library: &MaterialGraphLibrary::default(),
///     params: &Default::default(),
///     layer: "blades",
///     field_resolution: 256,
///     // A doctest is one of many running at once, so it takes two.
///     threads: NonZeroUsize::new(2),
/// })?;
///
/// // A noise in `0..=1` against a per-strand hash keeps about half of them,
/// // and the quarter a level of detail keeps is a prefix of the whole.
/// assert!(set.len() < 32 * 32);
/// assert!(set.prefix(0.25).len() < set.len());
/// # Ok::<(), ashlar_material::strands::StrandError>(())
/// ```
pub fn scatter(request: &StrandRequest<'_>) -> Result<StrandSet, StrandError> {
    if !crate::bake::in_range(request.field_resolution) {
        return Err(StrandError::Resolution(request.field_resolution));
    }
    let graph = request.graph.with_params(request.params)?;
    let material = graph.build_in(request.library)?;
    let layer = request.layer;
    let Some(declared) = material.strand(layer) else {
        return Err(StrandError::Graph(GraphError::new(
            format!("strands[{layer}]"),
            format!(
                "graph {:?} declares no strand layer of that name",
                material.graph().id
            ),
        )));
    };
    // The refusals a field is held to — the frame, and a relief of its own
    // strands — are made inside the lowering, so that the scatter here and the
    // `StrandRelief` node that splats the same layer make exactly one of them.
    let strands = lower_strands(&material, layer, strand_target())?;

    let planes = rasterise_buffers(
        &strands,
        request.field_resolution,
        request.threads,
        &mut BakeCache::default(),
    )?;
    Ok(scatter_lowered(
        layer,
        declared,
        &strands,
        &planes,
        request.threads,
    )?)
}

/// The scatter itself, over a layer that is already lowered and whose planes
/// are already rasterised.
///
/// What [`scatter`] does after it has lowered, and what
/// [`strand_plane`](crate::planes::strand_plane) does instead of lowering: a
/// `StrandRelief` carries its layer's lowering in the
/// [`StrandPlan`](crate::ir::StrandPlan) beside the plane, so the relief a bake
/// splats and the geometry a mesh builder grows are the same function of the
/// same arena rather than two that agree today.
pub(crate) fn scatter_lowered(
    name: &str,
    layer: &StrandLayer,
    ir: &crate::ir::Ir,
    planes: &[Plane],
    threads: Option<NonZeroUsize>,
) -> Result<StrandSet, EvalError> {
    let roots = Roots::of(ir);
    let interpreter = Interpreter::for_values(ir, &roots.all());
    let scattered = sow(layer, &interpreter, &roots, planes, threads)?;
    // `StrandSet::new` is what sorts, and it sorts *stably* over the vector
    // `sow` built in row-major cell order — so a rank two strands happen to
    // share is broken by the cell they came from, and the set is one answer
    // rather than whichever order a sort happened to land on.
    Ok(StrandSet::new(
        name.to_owned(),
        scattered,
        layer.count,
        StrandShape {
            profile: layer.profile,
            segments: layer.segments,
            taper: layer.taper,
            root_occlusion: layer.root_occlusion,
            midpoint: layer.midpoint,
        },
    ))
}

/// The ten roots of a lowered layer, in the order [`StrandLayer::ports`]
/// declares them.
///
/// Held as a struct rather than looked up by name per strand: a scatter reads
/// every one of these sixty-five thousand times, and a string compare per read
/// is the sort of cost that does not show in a profile until the lawn is the
/// whole scene.
struct Roots {
    density: ValueId,
    length: ValueId,
    width: ValueId,
    direction: ValueId,
    lean: ValueId,
    bend: ValueId,
    root_color: ValueId,
    tip_color: ValueId,
    roughness: ValueId,
    clump: ValueId,
}

impl Roots {
    /// Every port of the layer is bound — a layer carries a default for each —
    /// so a missing root is this crate being wrong rather than a graph, and a
    /// zero value is a better answer than a panic.
    fn of(ir: &crate::ir::Ir) -> Self {
        let root = |port: &str| ir.root(port).unwrap_or_default();
        Self {
            density: root("density"),
            length: root("length"),
            width: root("width"),
            direction: root("direction"),
            lean: root("lean"),
            bend: root("bend"),
            root_color: root("root_color"),
            tip_color: root("tip_color"),
            roughness: root("roughness"),
            clump: root("clump"),
        }
    }

    fn all(&self) -> [ValueId; 10] {
        [
            self.density,
            self.length,
            self.width,
            self.direction,
            self.lean,
            self.bend,
            self.root_color,
            self.tip_color,
            self.roughness,
            self.clump,
        ]
    }
}

/// The nine numbers one strand is made of, each a salt on the layer's own seed.
///
/// The hash answers one number per cell, and a strand needs nine, so the rest
/// are the same cell under different seeds — which is what
/// [`SALT`](crate::nodes) does for a gradient and what
/// [`Tile`](crate::nodes::Tile)'s own table does for an instance. Every one of
/// these is large and odd, and they are far apart, so two numbers of one strand
/// are uncorrelated: a length that tracked a rank would be a lawn whose short
/// blades are all in the far level of detail, and the seam between levels would
/// be the only thing anyone saw.
mod salt {
    /// Where the root strays along u.
    pub(super) const JITTER_U: u32 = 0x9E37_79B9;
    /// Where the root strays along v.
    pub(super) const JITTER_V: u32 = 0x85EB_CA6B;
    /// The level-of-detail order.
    pub(super) const RANK: u32 = 0xC2B2_AE35;
    /// The free hash a renderer sways the strand by.
    pub(super) const PHASE: u32 = 0x27D4_EB2D;
    /// What the density is a threshold against.
    pub(super) const GATE: u32 = 0x1656_67B1;
    /// What the length variation takes away.
    pub(super) const LENGTH: u32 = 0x2545_F491;
    /// What the width variation takes away.
    pub(super) const WIDTH: u32 = 0x4F6C_DD1D;
    /// How far the direction variation turns the strand.
    pub(super) const TURN: u32 = 0xB5AD_4ECE;
    /// What the lean variation takes away.
    pub(super) const LEAN: u32 = 0xA246_9BE5;
    /// What the bend variation takes away.
    pub(super) const BEND: u32 = 0x68E3_1DA4;
    /// How far the strand is turned about its own curve.
    pub(super) const FACING: u32 = 0x7FEB_352D;
    /// How far its root sinks below the surface.
    pub(super) const SINK: u32 = 0x846C_A68B;
    /// Where a clump's centre strays from its own cell centre.
    pub(super) const CLUMP_U: u32 = 0x3B9A_CA07;
    pub(super) const CLUMP_V: u32 = 0x5DEE_CE66;
    /// A clump's own identity, and the tint it takes from it.
    pub(super) const CLUMP_ID: u32 = 0x1B87_3593;
    pub(super) const CLUMP_TINT: u32 = 0x9E37_79B1;
}

/// One strand of one cell, as a lattice coordinate the hashes are taken at.
///
/// A cell stands [`StrandLayer::per_cell`] strands, and they differ only in the
/// salt their hashes are taken under — so the *third* coordinate is folded into
/// the seed rather than into the lattice, which is what keeps `count` the
/// period the repeat has to carry and keeps a cell's strands wrapping with it.
fn salted(seed: u32, index: u32) -> u32 {
    // A large odd multiplier, so two strands of one cell are as uncorrelated as
    // two cells are. Index zero leaves the seed alone, which is what makes a
    // `per_cell` of one the set a layer scattered before this existed.
    seed.wrapping_add(index.wrapping_mul(0x9E37_79B9))
}

/// One number of one cell, in `0..=1`.
///
/// The cells are already reduced into `0..count`, which is exactly what
/// [`Op::Hash2`](crate::ir::Op::Hash2) does to a lattice coordinate before it
/// hashes one, so this is the same number a `Tile` of the same count and seed
/// would compute for the same cell.
fn number(cell: [u32; 2], seed: u32, salt: u32) -> f32 {
    hash2(cell[0], cell[1], seed.wrapping_add(salt))
}

/// The same, in `-1..=1`, which is what a jitter and a turn want.
fn signed(cell: [u32; 2], seed: u32, salt: u32) -> f32 {
    number(cell, seed, salt).mul_add(2.0, -1.0)
}

/// A value a graph computed, with anything that is not a number taken as zero.
///
/// The bake writes a `NaN` into a plane and lets the encoder deal with it,
/// because a texel that is wrong is a texel. A strand is a position and a size,
/// and a `NaN` in one of those is a triangle with no coordinates: it does not
/// draw a bad blade, it takes the whole mesh out of the frustum test. So the
/// scatter is the one place in this crate that sanitises, and it says so here
/// rather than at each of the ten fields.
fn finite(value: f32) -> f32 {
    if value.is_finite() { value } else { 0.0 }
}

/// Walk the cell rows, one contiguous span per thread, and keep what the
/// density threshold left.
fn sow(
    layer: &StrandLayer,
    interpreter: &Interpreter<'_>,
    roots: &Roots,
    planes: &[Plane],
    threads: Option<NonZeroUsize>,
) -> Result<Vec<Strand>, EvalError> {
    let rows = layer.count[1].max(1);
    let mut per_row: Vec<Vec<Strand>> = vec![Vec::new(); rows as usize];
    let inputs = Inputs {
        buffers: planes,
        ..Inputs::default()
    };
    // One row of cells per element and one register file per thread, which is
    // the whole reason this drives `Interpreter::run` rather than `eval`: a
    // layer of 256 by 256 makes sixty-five thousand calls, and `eval` allocates
    // a register file the width of the expression for each one of them.
    try_rows_across_threads(&mut per_row, rows, 1, threads, |first, chunk| {
        let mut registers = interpreter.registers();
        for (offset, row) in chunk.iter_mut().enumerate() {
            let y = first + u32::try_from(offset).unwrap_or_default();
            for x in 0..layer.count[0].max(1) {
                // Every strand of a cell shares the cell and differs in the
                // salt its hashes are taken under, so they land in the same
                // cell of the same wrapping lattice and the period is still
                // `count`. The order is the cell's, then the index within it,
                // which is what keeps the row-major build deterministic.
                for index in 0..layer.per_cell.max(1) {
                    if let Some(strand) = grow(
                        layer,
                        interpreter,
                        roots,
                        &inputs,
                        &mut registers,
                        [x, y],
                        index,
                    )? {
                        row.push(strand);
                    }
                }
            }
        }
        Ok(())
    })?;
    Ok(per_row.into_iter().flatten().collect())
}

/// One cell's strand, or nothing where the density threshold turned it off.
#[expect(
    clippy::cast_precision_loss,
    reason = "a cell index is bounded by MAX_PERIOD, which f32 counts exactly"
)]
fn grow(
    layer: &StrandLayer,
    interpreter: &Interpreter<'_>,
    roots: &Roots,
    inputs: &Inputs<'_>,
    registers: &mut crate::interp::Registers,
    cell: [u32; 2],
    index: u32,
) -> Result<Option<Strand>, EvalError> {
    // The seed every hash of *this* strand is taken under: the layer's, salted
    // by which strand of the cell this is. Index zero leaves it alone, so a
    // layer of one strand per cell scatters the set it always scattered.
    let seed = salted(layer.seed, index);
    let lattice = [layer.count[0].max(1) as f32, layer.count[1].max(1) as f32];
    // The cell centre plus half a cell of jitter, which is the point a
    // `Voronoi` of the same lattice would put its feature point at. With
    // several strands to a cell the jitter is what separates them, so a layer
    // asking for more than one and no jitter stacks them exactly.
    let half = layer.jitter * 0.5;
    let root = [
        (cell[0] as f32 + 0.5 + half * signed(cell, seed, salt::JITTER_U)) / lattice[0],
        (cell[1] as f32 + 0.5 + half * signed(cell, seed, salt::JITTER_V)) / lattice[1],
    ];
    interpreter.run(root, inputs, registers)?;

    // The threshold first, so a cell that carries no strand costs the field
    // evaluation and nothing after it.
    let density = finite(registers.float(roots.density));
    if density < number(cell, seed, salt::GATE) {
        return Ok(None);
    }

    // Which clump this strand belongs to, and where that clump's centre is.
    // Worked out from the *root* rather than from the cell, so two strands of
    // one cell that jittered either side of a clump boundary belong to the
    // clumps they actually stand in.
    let clump = Clump::of(layer, root);

    // A variation takes size away rather than adding it, as a scatter's does:
    // a strand never outgrows the length its layer declared, so the metres on
    // the layer are the longest blade rather than the average one.
    //
    // `clump_share` of each one is taken from the clump's hash instead of the
    // strand's, so the blades of one tuft agree about how long they are and
    // how far they lie over while their neighbours in the next tuft do not.
    // Blender splits its child roughness by exactly this correlation length.
    let taken = |amount: f32, salt: u32| {
        let own = number(cell, seed, salt);
        let shared = clump.hash(layer, salt);
        let mixed = own + (shared - own) * layer.clump_share.clamp(0.0, 1.0);
        1.0 - amount * mixed
    };
    let length = layer.length_metres
        * finite(registers.float(roots.length)).max(0.0)
        * taken(layer.length_variation, salt::LENGTH);
    let width = layer.width_metres
        * finite(registers.float(roots.width)).max(0.0)
        * taken(layer.width_variation, salt::WIDTH);

    // Half a turn either way at full variation, which is as far as a direction
    // can go before it meets itself. Turning a zero vector leaves a zero
    // vector, which is the honest answer: a variation turns a direction, and
    // there is no direction here to turn.
    let bearing = registers.get(roots.direction);
    // The same mixing as `taken`, for the variations that run either way round
    // zero rather than taking size away.
    let turned_by = |salt: u32| {
        let own = signed(cell, seed, salt);
        let shared = clump.hash(layer, salt).mul_add(2.0, -1.0);
        own + (shared - own) * layer.clump_share.clamp(0.0, 1.0)
    };
    let angle = turned_by(salt::TURN) * layer.direction_variation * std::f32::consts::PI;
    let (sine, cosine) = angle.sin_cos();
    let turned = [
        finite(bearing[0]).mul_add(cosine, -(finite(bearing[1]) * sine)),
        finite(bearing[0]).mul_add(sine, finite(bearing[1]) * cosine),
    ];
    let reach = turned[0].hypot(turned[1]);
    let direction = if reach > 0.0 {
        [turned[0] / reach, turned[1] / reach]
    } else {
        [0.0, 0.0]
    };

    // The clump moves the strand after its fields were read, not before: a
    // tuft is several roots gathered onto one point, and a root that read the
    // gathering point's own fields would take that point's length and colour
    // as well, which is a tuft of clones rather than a tuft.
    let offset = registers.get(roots.clump);
    let gathered = [
        wrap(root[0] + finite(offset[0])),
        wrap(root[1] + finite(offset[1])),
    ];

    // A whole tuft darker than the tuft beside it, which is neither a field —
    // that varies a lawn over metres — nor a per-strand hash, which varies it
    // per blade. Substance drives per-instance luminance from the instance's
    // own index for the same reason.
    let tint = 1.0 - layer.clump_tint.clamp(0.0, 1.0) * clump.hash(layer, salt::CLUMP_TINT);
    let shade = |value: [f32; 3]| {
        let lit = colour(value);
        [lit[0] * tint, lit[1] * tint, lit[2] * tint]
    };

    Ok(Some(Strand {
        root: gathered,
        rank: number(cell, seed, salt::RANK),
        phase: number(cell, seed, salt::PHASE),
        length,
        width,
        direction,
        lean: (finite(registers.float(roots.lean)) * taken(layer.lean_variation, salt::LEAN))
            .clamp(0.0, 1.0),
        bend: (finite(registers.float(roots.bend)) * taken(layer.bend_variation, salt::BEND))
            .max(0.0),
        root_color: shade(registers.get(roots.root_color)),
        tip_color: shade(registers.get(roots.tip_color)),
        roughness: finite(registers.float(roots.roughness)).clamp(0.0, 1.0),
        // A half turn either way, which is as far as a ribbon can be turned
        // before it presents the same face again. Shared with the clump like
        // every other variation: "that clump controls the blade's direction",
        // and which way a blade *faces* is half of what a direction is.
        facing: turned_by(salt::FACING)
            * layer.facing_variation.clamp(0.0, 1.0)
            * std::f32::consts::FRAC_PI_2,
        height_offset: layer.height_offset_metres.max(0.0) * number(cell, seed, salt::SINK),
        clump_id: clump.id,
        clump_distance: clump.distance(root, layer),
        // From the *gathered* root, because that is where the strand actually
        // stands, and wrapped the short way round so a tuft at the seam pulls
        // across it rather than all the way back over the repeat.
        clump_pull: clump.pull(gathered, layer),
    }))
}

/// Which clump a root belongs to, and where that clump's centre is.
///
/// A cell of a second, coarser lattice, jittered the way the root lattice is,
/// so a clump is a point the strands around it lean towards rather than a
/// square they sit in. A layer that declares no clump lattice gets one of
/// these with nothing in it, and every read of it is the identity.
#[derive(Clone, Copy)]
struct Clump {
    /// The clump's own hash in `0..=1`, or zero where there is no lattice.
    id: f32,
    /// Where its centre is, in UV over one repeat.
    centre: [f32; 2],
    /// Whether there is a lattice at all.
    present: bool,
}

impl Clump {
    #[expect(
        clippy::cast_precision_loss,
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "a clump cell index is bounded by MAX_PERIOD, which an f32 counts exactly"
    )]
    fn of(layer: &StrandLayer, root: [f32; 2]) -> Self {
        if !layer.clumped() {
            return Self {
                id: 0.0,
                centre: [0.0; 2],
                present: false,
            };
        }
        let lattice = [layer.clump_count[0].max(1), layer.clump_count[1].max(1)];
        let cell = [
            ((root[0] * lattice[0] as f32).floor() as u32).min(lattice[0] - 1),
            ((root[1] * lattice[1] as f32).floor() as u32).min(lattice[1] - 1),
        ];
        // The same jittered-lattice construction the roots use, at the clump
        // seed, so a clump centre is a point a `Voronoi` of that lattice would
        // put its feature at.
        let half = layer.jitter.clamp(0.0, 1.0) * 0.5;
        let centre = [
            (cell[0] as f32 + 0.5 + half * signed(cell, layer.seed, salt::CLUMP_U))
                / lattice[0] as f32,
            (cell[1] as f32 + 0.5 + half * signed(cell, layer.seed, salt::CLUMP_V))
                / lattice[1] as f32,
        ];
        Self {
            id: number(cell, layer.seed, salt::CLUMP_ID),
            centre,
            present: true,
        }
    }

    /// One hash of this clump, or a half where there is no clump — a half being
    /// the mean of the per-strand hash it is mixed with, so sharing nothing
    /// with a clump that does not exist moves no variation.
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        clippy::cast_precision_loss,
        reason = "a clump cell index is bounded by MAX_PERIOD, which an f32 counts exactly"
    )]
    fn hash(self, layer: &StrandLayer, salt: u32) -> f32 {
        if !self.present {
            return 0.5;
        }
        let lattice = [layer.clump_count[0].max(1), layer.clump_count[1].max(1)];
        let cell = [
            ((self.centre[0] * lattice[0] as f32).floor() as u32).min(lattice[0] - 1),
            ((self.centre[1] * lattice[1] as f32).floor() as u32).min(lattice[1] - 1),
        ];
        number(cell, layer.seed, salt)
    }

    /// How far out in its cell the root stands, in `0..=1` from the clump's
    /// centre to the furthest corner of that cell.
    ///
    /// Measured the short way round the repeat, as the pull is, and normalised
    /// by the cell's own half-diagonal so that it means the same thing however
    /// coarse the clump lattice is.
    #[expect(
        clippy::cast_precision_loss,
        reason = "a clump lattice is bounded by MAX_PERIOD, which an f32 counts exactly"
    )]
    fn distance(self, root: [f32; 2], layer: &StrandLayer) -> f32 {
        if !self.present {
            return 0.0;
        }
        let lattice = [
            layer.clump_count[0].max(1) as f32,
            layer.clump_count[1].max(1) as f32,
        ];
        let axis = |from: f32, to: f32| {
            let delta = to - from;
            delta - delta.round()
        };
        let away = [
            axis(self.centre[0], root[0]) * lattice[0],
            axis(self.centre[1], root[1]) * lattice[1],
        ];
        // In cells rather than in UV, so the furthest a root can be from a
        // centre inside its own cell is half the cell diagonal.
        (away[0].hypot(away[1]) / std::f32::consts::SQRT_2).clamp(0.0, 1.0)
    }

    /// How far and which way the strand's tip is pulled, in UV.
    ///
    /// The short way round the repeat, because a clump at the seam has members
    /// on both sides of it and a pull measured the long way would throw a tip
    /// clean across the material.
    fn pull(self, root: [f32; 2], layer: &StrandLayer) -> [f32; 2] {
        if !self.present {
            return [0.0; 2];
        }
        let amount = layer.clump_tips.clamp(0.0, 1.0);
        let axis = |from: f32, to: f32| {
            let delta = to - from;
            // Bring the difference into `-0.5..=0.5`, which is the short way.
            (delta - delta.round()) * amount
        };
        [axis(root[0], self.centre[0]), axis(root[1], self.centre[1])]
    }
}

/// A coordinate brought back into one repeat, which is what "roots wrap at the
/// repeat" means: a clump that pushes a root past the seam puts it back on the
/// other side rather than off the material.
fn wrap(coordinate: f32) -> f32 {
    let wrapped = coordinate - coordinate.floor();
    // `1.0 - f32::EPSILON / 2` floors to zero but is not zero, so a coordinate
    // a hair under one can round up to exactly one here. One is the seam and
    // the seam is the origin.
    if wrapped >= 1.0 { 0.0 } else { wrapped }
}

/// A linear colour a graph computed, held to `0..=1`.
///
/// A strand's colour is a base colour rather than an emission, and the vertex
/// colour it becomes is multiplied into a `StandardMaterial`'s own, so there is
/// nothing above one for it to mean.
fn colour(value: [f32; 3]) -> [f32; 3] {
    [
        finite(value[0]).clamp(0.0, 1.0),
        finite(value[1]).clamp(0.0, 1.0),
        finite(value[2]).clamp(0.0, 1.0),
    ]
}

/// Refuse a field that reaches a runtime input or a strand relief, at the node
/// that asked.
///
/// The bake makes the runtime refusal inside the lowering, because a bake is a
/// [`Target`] and a target is what a node lowering can ask about. A scatter
/// cannot: it lowers for a shader target so that a graph delivered as a
/// `Shader` surface may still carry world-space inputs in its *PBR* half, which
/// is a surface the plan puts in scope. So the refusal is made here instead, by
/// walking the graph from the layer's own ports, and it names the same paths
/// validation names.
///
/// A [`StrandRelief`](crate::nodes::StrandRelief) is refused by the same walk
/// and for a different reason: a relief is splatted *from* a layer's fields, so
/// a field that read one would be a cycle. The rule is the conservative one —
/// **any** relief a field reaches, not only one of its own layer — because
/// two layers can reach each other's reliefs as easily as one can reach its
/// own, and one rule between them is worth more than the graphs the sharper
/// one would allow.
///
/// Inside a [`Subgraph`] the walk stops asking what reaches what and refuses
/// any such node the instanced graph declares at all, for the reason the same
/// paragraph gives: lowering inlines every node of an instance, so a world mask
/// nothing reads there refuses a bake too.
pub(crate) fn refuse_dynamic(material: &Material, layer: &StrandLayer) -> Result<(), GraphError> {
    let mut seen: BTreeSet<&str> = BTreeSet::new();
    let mut stack: Vec<&str> = layer
        .ports()
        .into_iter()
        .filter_map(|(_, _, input)| match input {
            Input::Node(id) => Some(id.as_str()),
            _ => None,
        })
        .collect();
    while let Some(id) = stack.pop() {
        if !seen.insert(id) {
            continue;
        }
        let Some(node) = material.graph().nodes.get(id) else {
            continue;
        };
        let path = format!("nodes[{id}]");
        refuse(node, &path)?;
        if let Node::Subgraph(subgraph) = node {
            instanced(material, subgraph, &path)?;
        }
        for port in node.inputs() {
            if let Input::Node(target) = port.input {
                stack.push(target.as_str());
            }
        }
    }
    Ok(())
}

/// Every parameter of the graph that one strand layer's fields can reach.
///
/// The same walk [`refuse_dynamic`] makes, collecting names instead of
/// refusing: from the layer's own ports, through `Input::Node` edges, gathering
/// every `Input::Param` on the way. A [`Subgraph`] is walked only as far as its
/// *inputs*, which is where the outer graph's parameters cross into it; the
/// parameters an instanced graph declares are its own and are bound at the
/// instance rather than by a caller.
///
/// It exists for [`partition`](crate::partition), which cannot see a strand
/// layer: the partition lowers the PBR half alone, so a
/// [`Live`](crate::Exposure::Live) parameter that only a strand field reads
/// would be reported as live and then folded by every scatter without anything
/// saying so. A slider that silently does nothing is the thing
/// [`Partition::frozen`](crate::partition::Partition::frozen) exists to name.
pub(crate) fn params_reached(material: &Material, layer: &StrandLayer) -> BTreeSet<String> {
    let mut found: BTreeSet<String> = BTreeSet::new();
    let mut seen: BTreeSet<String> = BTreeSet::new();
    let mut stack: Vec<String> = Vec::new();
    for (_, _, input) in layer.ports() {
        take(input, &mut stack, &mut found);
    }
    while let Some(id) = stack.pop() {
        if !seen.insert(id.clone()) {
            continue;
        }
        let Some(node) = material.graph().nodes.get(&id) else {
            continue;
        };
        for port in node.inputs() {
            take(port.input, &mut stack, &mut found);
        }
    }
    found
}

/// One input of [`params_reached`]'s walk: a parameter to keep, an edge to
/// follow, or a constant.
///
/// Owned ids rather than borrowed ones. The walk is over a few dozen nodes once
/// per partition, and a borrow out of the graph would have to outlive the frame
/// that took it.
fn take(input: &Input, stack: &mut Vec<String>, found: &mut BTreeSet<String>) {
    match input {
        Input::Param(name) => {
            found.insert(name.clone());
        }
        Input::Node(id) => stack.push(id.clone()),
        Input::Const(_) | Input::Inline(_) => {}
    }
}

/// Every node of the graph one [`Subgraph`] instanced, and of the graphs that
/// one instances in turn.
///
/// The build refused recursion by key, so this terminates. An instance the
/// material does not hold is one the build would have refused, so it is passed
/// over rather than guessed at.
fn instanced(material: &Material, node: &Subgraph, path: &str) -> Result<(), GraphError> {
    let resolved = material.resolved_inputs(node, path)?;
    let key = crate::library::instance_key(node, &resolved);
    let Some(inner) = material.instance(&key) else {
        return Ok(());
    };
    for (id, node) in &inner.graph().nodes {
        let path = format!("{path}.graphs[{key}].nodes[{id}]");
        refuse(node, &path)?;
        if let Node::Subgraph(subgraph) = node {
            instanced(inner, subgraph, &path)?;
        }
    }
    Ok(())
}

/// One node, refused where it is a fact about the mesh or the frame, or where
/// it is the strands this field is a field of.
fn refuse(node: &Node, path: &str) -> Result<(), GraphError> {
    if let Node::StrandRelief(relief) = node {
        return Err(GraphError::new(
            path,
            format!(
                "a strand relief is splatted from a layer's own fields, so a field that reads \
                 one is asking for itself; this one reads {:?}, and no strand field may reach a \
                 relief of any layer",
                relief.layer
            ),
        ));
    }
    let what = match node {
        Node::Time(_) => "a clock",
        Node::WorldPos(_) => "a world position",
        Node::WorldNormal(_) => "a world normal",
        Node::CutFlag(_) => "a cut flag",
        Node::Triplanar(_) => "a triplanar projection",
        Node::WorldMask(_) => "a world mask",
        _ => return Ok(()),
    };
    Err(GraphError::new(
        path,
        format!(
            "{what} is a fact about the mesh and the frame it is drawn in, and a strand is \
             scattered over one repeat of the material before there is either; build this field \
             out of the graph's own generators, and leave the wind to the renderer"
        ),
    ))
}
