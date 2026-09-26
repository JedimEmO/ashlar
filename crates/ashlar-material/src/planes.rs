//! The plane pipeline: what a buffered filter costs and how it is paid for.
//!
//! A pointwise node is an expression, and the interpreter runs it per texel. A
//! buffered node is not: a blur, an occlusion or a distance reads a
//! *neighbourhood*, which per texel would be the whole expression again once
//! per tap. So the lowering cuts the graph there. Everything upstream of a
//! buffered node is rasterised once into a [`Plane`], the filter runs over that
//! plane as ordinary Rust, and what is downstream reads the result through
//! [`Op::Sample`], bilinearly and with wrap.
//!
//! [`rasterise_buffers`] is that pass. It walks [`Ir::buffers`] in order, which
//! is already a dependency order — a plan can only sample a plane that existed
//! when it was made — so a plane is always rasterised against the planes it
//! reads.
//!
//! # Wrapped, so the result tiles
//!
//! Every filter here reads its neighbours across the seam: a texel in the first
//! column is filtered against the last column, not against a pad. That is the
//! whole reason a buffered filter is allowed in a crate whose promise is that a
//! bake tiles. A filter that clamped at the edge would leave a seam that no
//! amount of period inference would notice, because the period would still be
//! right and the picture still wrong.
//!
//! # Radii are reaches in UV
//!
//! Every radius a filter takes is how far it reaches across UV, not how many
//! texels it covers, so the same graph describes the same surface at 512 and at
//! 4096. The blur is a Gaussian truncated at its radius, which puts its
//! standard deviation at a third of it; the morphologies take a square
//! neighbourhood of the radius, which is separable and gives an opening back
//! the mask it was given rather than rounding its corners.
//!
//! # One value, one evaluation
//!
//! The planes are also *stages* of one bake, and every stage at the bake's own
//! resolution walks the same texel grid. So a value one stage computed is the
//! value a later stage would compute, and [`memo`](crate::memo) works out
//! which values those are: a stage writes a plane of each value a later stage
//! will want, a stage reads back what is there rather than evaluating it, and
//! a plane is dropped after the last stage that reads it. It is exact — the
//! `f32` read back is the `f32` that was stored — and it is what takes the SOI
//! cobblestone's stages from 13,932 instructions a texel between them to
//! rather over the 4,196 its expression holds.
//!
//! # What a plane costs
//!
//! A plane is `resolution` squared texels of `f32` per lane, which at 1024 is
//! four megabytes for a height and twelve for a colour, and it is rasterised
//! and filtered once per bake rather than once per texel. [`BakeCache`] is what
//! keeps that from being paid twice: planes are keyed by the sub-expression
//! they rasterise, the filter run over them and their resolution, so two nodes
//! that asked for the same thing share one plane, and a cache handed across
//! bakes — a preview re-baking a graph whose blur did not change — keeps it.

use std::collections::HashMap;
use std::num::NonZeroUsize;
use std::sync::Arc;

use crate::{
    interp::{EvalError, Inputs, Interpreter, Plane, Wrap},
    ir::{BufferPlan, Filter, Ir, IrType, Op, ValueId},
    memo::{MemoPlan, MemoTable},
    nodes::CurvatureKind,
    strip::STRIP,
};

/// The fewest rows a thread of its own is worth, and so what bounds a default
/// thread count.
///
/// A thread costs a spawn and a register file the width of the whole
/// expression, and a span shorter than this is over before it has paid for
/// either. What the bound is really for is the other direction: without it a
/// bake asks for the whole machine however small it is, so a test binary or a
/// task pool with several bakes in flight runs hundreds of threads over one
/// machine's worth of work and every one of them is slower for it. An explicit
/// thread count is deliberately not bounded by this, because the test that pins
/// the same bytes under a different division of the rows has to be able to ask
/// for a finer division than the default would ever pick.
pub const MIN_ROWS_PER_THREAD: u32 = 32;

/// How many directions the occlusion filter looks in, and how many samples it
/// takes along each.
///
/// Fixed rather than derived from the radius, and that is the point: the
/// samples are spread evenly over the radius in UV, so a bake at 512 and a bake
/// at 4096 march the same distances and answer the same occlusion. A count that
/// grew with the resolution would make the map depend on how many texels it was
/// written at, which is exactly what the radius being a UV reach is for.
const HORIZON_DIRECTIONS: usize = 8;
const HORIZON_STEPS: usize = 8;

/// Planes kept between bakes.
///
/// A bake makes its own and drops it, so nothing is shared by accident; a
/// caller that re-bakes the same graph often — the preview, re-baking on every
/// slider — hands one in through [`rasterise_with`](crate::bake::rasterise_with)
/// and keeps the planes whose sub-expression and filter did not change.
///
/// Keyed by what decides a plane's contents: the sub-expression rasterised, the
/// filter run over it, and the resolution. A parameter folded into that
/// expression is part of it, so turning a slider that reaches a blur's input is
/// a different key and a different plane, as it must be.
///
/// Nothing evicts. A session that bakes many different graphs should
/// [`clear`](Self::clear) between them, or hold one cache per graph.
#[derive(Clone, Debug, Default)]
pub struct BakeCache {
    planes: HashMap<PlaneKey, Plane>,
    /// The scatters a [`Filter::Strands`] plane was splatted from, by
    /// [`StrandPlan::key`].
    ///
    /// Beside the planes rather than folded into them because a layer's
    /// outputs are several planes over *one* scatter: a graph reading coverage,
    /// height and colour asks for three planes and would otherwise evaluate
    /// sixty-five thousand strands' worth of fields three times over. A plane
    /// is keyed by the strand key as well, so nothing here is ever read for a
    /// plane it did not produce.
    scatters: HashMap<Arc<[u8]>, Arc<crate::strands::StrandSet>>,
}

impl BakeCache {
    /// An empty cache.
    pub fn new() -> Self {
        Self::default()
    }

    /// How many planes are held.
    pub fn len(&self) -> usize {
        self.planes.len()
    }

    /// Whether it holds nothing.
    pub fn is_empty(&self) -> bool {
        self.planes.is_empty()
    }

    /// Drop every plane, and every scatter one was splatted from.
    pub fn clear(&mut self) {
        self.planes.clear();
        self.scatters.clear();
    }
}

/// Rasterise and filter every plane an expression samples, in order.
///
/// The answer is one plane per [`Ir::buffers`] entry, in that order, which is
/// what [`Inputs::buffers`] wants. A plan whose resolution is pinned is
/// rasterised at that resolution and sampled with wrap like any other, so a
/// graph can isolate the cost of an expensive filter by giving it fewer texels
/// than the bake has.
///
/// [`EvalError::Unbaked`] is the one failure, and it means a plan sampled a
/// plane that comes after it — a lowering that built its plans out of order,
/// not anything an author can write.
pub fn rasterise_buffers(
    ir: &Ir,
    resolution: u32,
    threads: Option<NonZeroUsize>,
    cache: &mut BakeCache,
) -> Result<Vec<Plane>, EvalError> {
    rasterise_into(
        ir,
        resolution,
        &vec![true; ir.buffers().len()],
        Table::Discarded,
        threads,
        cache,
    )
    .map(|(planes, _)| planes)
}

/// [`rasterise_buffers`], keeping what the outputs may read back.
///
/// The planes are the same planes. What comes with them is the
/// [`MemoTable`] the stages left behind: the values the outputs would
/// otherwise evaluate a second time, which for the SOI cobblestone is most of
/// what its height costs. A caller that is not about to rasterise the outputs
/// of this same expression at this same resolution wants
/// [`rasterise_buffers`] and lets the table go.
pub(crate) fn rasterise_buffers_memoised(
    ir: &Ir,
    resolution: u32,
    threads: Option<NonZeroUsize>,
    cache: &mut BakeCache,
) -> Result<(Vec<Plane>, MemoTable), EvalError> {
    rasterise_into(
        ir,
        resolution,
        &vec![true; ir.buffers().len()],
        Table::Kept,
        threads,
        cache,
    )
}

/// Rasterise the planes `wanted` names, the planes those read, and no others.
///
/// What a caller that already *has* some of the answer needs. `ashlar-bevy`
/// keeps a compiled graph's bound textures as encoded images keyed by
/// [`PlaneKey`], so a second graph over the same wall finds most of its images
/// already made and has only its own few planes left to rasterise — and a plane
/// it does not need is the whole expression above it not walked. Asking for
/// every plane is [`rasterise_buffers`], and costs exactly what it did.
///
/// `wanted` is a flag per [`Ir::buffers`] entry; a short one reads as `false`
/// past its end. The answer is one entry per plan: `Some` for a plane that was
/// rasterised — the wanted ones and the dependencies they pulled in with them —
/// and `None` for one that was skipped. A skipped plane is never one a
/// rasterised plane reads, because [`plane_closure`] is what decides.
pub fn rasterise_wanted(
    ir: &Ir,
    resolution: u32,
    wanted: &[bool],
    threads: Option<NonZeroUsize>,
    cache: &mut BakeCache,
) -> Result<Vec<Option<Plane>>, EvalError> {
    let needed = plane_closure(ir, wanted);
    let (planes, _) = rasterise_into(ir, resolution, &needed, Table::Discarded, threads, cache)?;
    Ok(planes
        .into_iter()
        .zip(&needed)
        .map(|(plane, needed)| needed.then_some(plane))
        .collect())
}

/// Every plan a rasterisation of `wanted` has to run, `wanted` included.
///
/// A plan needs the planes its own sub-expression samples and the guide its
/// filter walks, and theirs in turn. One backwards sweep is enough because a
/// plan can only name a plan before it, which is the same order
/// [`rasterise_buffers`] relies on.
pub fn plane_closure(ir: &Ir, wanted: &[bool]) -> Vec<bool> {
    let mut needed: Vec<bool> = (0..ir.buffers().len())
        .map(|index| wanted.get(index).copied().unwrap_or(false))
        .collect();
    for index in (0..ir.buffers().len()).rev() {
        if needed.get(index).copied() != Some(true) {
            continue;
        }
        for dep in plan_deps(ir, index) {
            if let Some(slot) = needed.get_mut(dep) {
                *slot = true;
            }
        }
    }
    needed
}

/// The planes one plan reads: the guide its filter walks, then the planes its
/// live sub-expression samples, each once and in the order they are first
/// named.
///
/// The one definition of what a plan depends on, and deliberately so: the
/// closure a rasterisation runs and the closure a [`PlaneKey`] encodes have to
/// be the same set, or a plane would be keyed by less than it was drawn from.
/// The order is part of the answer rather than incidental, because it is what
/// [`plane_key`] numbers the closure by.
fn plan_deps(ir: &Ir, index: usize) -> Vec<usize> {
    let mut deps: Vec<usize> = Vec::new();
    let Some(plan) = ir.buffers().get(index) else {
        return deps;
    };
    let push = |deps: &mut Vec<usize>, buffer: usize| {
        if !deps.contains(&buffer) {
            deps.push(buffer);
        }
    };
    if let Some(guide) = plan.filter.guide() {
        push(&mut deps, guide.index());
    }
    let reaches = ir.reaches(&[plan.root]);
    for (at, inst) in ir.insts().iter().enumerate() {
        if reaches.get(at).copied() != Some(true) {
            continue;
        }
        if let Op::Sample(buffer) = inst.op {
            push(&mut deps, buffer.index());
        }
    }
    deps
}

/// Every plan, with the ones `needed` does not name left as a one-texel
/// placeholder.
///
/// The placeholder is what keeps the answer parallel to [`Ir::buffers`], which
/// is what [`Inputs::buffers`] and every caller index into. Nothing reads one:
/// [`plane_closure`] has already marked every plane a rasterised plane samples
/// or walks, so a placeholder is only ever in a slot its caller was told to
/// ignore.
fn rasterise_into(
    ir: &Ir,
    resolution: u32,
    needed: &[bool],
    table: Table,
    threads: Option<NonZeroUsize>,
    cache: &mut BakeCache,
) -> Result<(Vec<Plane>, MemoTable), EvalError> {
    let mut planes: Vec<Plane> = Vec::with_capacity(ir.buffers().len());
    let keys = plane_keys(ir, resolution);
    let analysis = MemoPlan::analyse(ir, resolution);
    let mut memo = MemoTable::new(ir.len());
    // The outputs are the last stage, so a value only they read is a plane
    // that nobody in this pass will look at — 251 MB of it on the SOI
    // cobblestone. A caller that is not about to rasterise the outputs says so
    // with [`Table::Discarded`] and those planes are never written.
    let wanted_here: Vec<bool> = match table {
        Table::Kept => Vec::new(),
        Table::Discarded => analysis.read_before_outputs(ir.len()),
    };
    for ((index, plan), key) in ir.buffers().iter().enumerate().zip(&keys) {
        let size = plan.resolution.unwrap_or(resolution).max(1);
        let stage = analysis.stage(index);
        if needed.get(index).copied() != Some(true) {
            planes.push(Plane::new(
                1,
                plan.value_type,
                vec![0.0; plan.value_type.components()],
            ));
        } else if let Some(plane) = cache.planes.get(key) {
            // A stage that did not run leaves nothing behind, and the stages
            // after it evaluate what it would have provided: what is memoised
            // is whatever is in the table, never what the analysis hoped for.
            planes.push(plane.clone());
        } else if plan.strands.is_some() {
            // Not rasterised and not filtered: a strand plane is splatted from
            // a scatter of its own, so the plan's constant root is never walked
            // and the arena above it never sees a strand.
            let splatted = strand_plane(plan, size, threads, cache)?;
            cache.planes.insert(key.clone(), splatted.clone());
            planes.push(splatted);
        } else {
            // Only the planes already rasterised are in scope, which is what
            // makes an out-of-order plan an error here rather than a plane of
            // zeroes.
            let (raw, produced) = {
                // A plan at its own resolution walks a grid of its own: its
                // texel `k` is not the bake's texel `k`, so it neither reads
                // the table nor writes to it. The analysis says the same, and
                // this says it again where the planes are actually sized,
                // because a plane in the table at the wrong resolution would
                // be read by index and answer a different texel.
                let shares_the_grid = size == resolution.max(1);
                let frontier = if shares_the_grid {
                    memo.frontier(ir, &[plan.root])
                } else {
                    Vec::new()
                };
                let keep: Vec<ValueId> = match stage {
                    Some(stage) if shares_the_grid => stage
                        .writes
                        .iter()
                        .copied()
                        .filter(|value| match table {
                            Table::Kept => true,
                            Table::Discarded => {
                                wanted_here.get(value.index()).copied() == Some(true)
                            }
                        })
                        .collect(),
                    _ => Vec::new(),
                };
                rasterise_root(
                    ir,
                    plan.root,
                    plan.value_type,
                    size,
                    &planes,
                    threads,
                    Memo {
                        frontier: &frontier,
                        keep: &keep,
                    },
                )?
            };
            let filtered = filter(plan.filter, raw, &planes, threads);
            cache.planes.insert(key.clone(), filtered.clone());
            planes.push(filtered);
            for (value, plane) in produced {
                memo.insert(value, plane);
            }
        }
        // After the last stage that reads it, a memoised plane is megabytes
        // nothing will look at again.
        for value in stage.map_or(&[][..], |stage| &stage.drops) {
            memo.drop_value(*value);
        }
    }
    Ok((planes, memo))
}

/// Splat one strand plane: scatter the layer, then stamp its footprints.
///
/// The half of the plane pipeline that is a strand layer rather than a filter,
/// and public for the reason [`filter`] is: a GPU bake evaluates a plane's
/// sub-expression on the device and comes to the CPU for what a device cannot
/// do. Here it is the whole plane — a scatter is sixty-five thousand
/// point evaluations of a small expression and a splat is a few million texel
/// writes, neither of which is a kernel — so a strand plane takes the CPU path
/// on both backends, and they agree because there is only one of them.
///
/// The layer's *own* planes — a blur upstream of a density field, the slope a
/// [`Direction`](crate::nodes::Direction) reads — are rasterised here at
/// [`FIELD_RESOLUTION`](crate::strands::FIELD_RESOLUTION) and **not** at this
/// plane's own size. That constant says why at length; the short of it is that
/// a mesh builder scatters at that resolution, and a scatter whose field planes
/// were rasterised at another one is a different scatter, which would put the
/// relief's blades at a different lean from the geometry's. They are otherwise
/// ordinary planes over an ordinary expression and key exactly as one, through
/// the same `cache`.
///
/// A plan with no [`BufferPlan::strands`] is a plane of zeroes rather than a
/// panic: it is this crate lowering a strand filter without a scatter, which is
/// a bug here rather than anything an author can write.
pub fn strand_plane(
    plan: &BufferPlan,
    resolution: u32,
    threads: Option<NonZeroUsize>,
    cache: &mut BakeCache,
) -> Result<Plane, EvalError> {
    let Filter::Strands { output } = plan.filter else {
        return Ok(Plane::new(resolution.max(1), plan.value_type, Vec::new()));
    };
    let Some(scatter) = plan.strands.as_ref() else {
        return Ok(Plane::new(resolution.max(1), plan.value_type, Vec::new()));
    };
    let set = if let Some(set) = cache.scatters.get(scatter.key()) {
        Arc::clone(set)
    } else {
        let fields = scatter.fields();
        let field_planes =
            rasterise_buffers(fields, crate::strands::FIELD_RESOLUTION, threads, cache)?;
        let set = Arc::new(crate::strands::scatter_lowered(
            scatter.layer(),
            scatter.settings(),
            fields,
            &field_planes,
            threads,
        )?);
        cache
            .scatters
            .insert(Arc::from(scatter.key()), Arc::clone(&set));
        set
    };
    Ok(crate::strands::splat(
        &set,
        scatter.settings(),
        output,
        scatter.repeat_metres(),
        resolution,
        threads,
    ))
}

/// The identity of one plane: what it holds, what was run over it, and how
/// many texels it has.
///
/// **An exact key rather than a digest of one**, and that is the same policy
/// `ashlar-bevy`'s own `BakeKey` argues for in the layer above: a
/// sixty-four-bit digest would key a plane cache just as well until the day two
/// planes collided, and the failure then is not a slow frame but one wall
/// silently wearing another's texels, for one pair of expressions somewhere in
/// a content library. What is kept instead is the canonical encoding itself —
/// every number fixed width, every list preceded by its count and every string
/// by its length — so two encodings are equal exactly when the planes are, and
/// equality is a byte comparison. It is shared rather than copied: an [`Arc`]
/// is what a clone moves.
///
/// # What is encoded
///
/// Not the plane alone but its whole *closure*: the plane, the planes it
/// samples or is guided by, and theirs in turn — each written once, in the
/// order the walk below first reaches it from the root. A plane inside the
/// closure is named by its position in that list, and a sub-expression is
/// renumbered from its own root, so what the bytes describe is the shape of
/// the dependency graph and nothing about the arena it was lowered in. Two
/// graphs that blur the same noise encode the same bytes wherever the noise
/// and the blur happen to sit in their plan lists, which is the sharing this
/// key exists for.
///
/// Writing the closure as a *list* rather than nesting each dependency's key
/// inside its dependent is what keeps the key linear in the graph. Nested, a
/// plane that samples two planes with a common ancestor spells that ancestor
/// out twice, and a stack of such planes doubles at every level — which is not
/// a hypothetical: nested, `study:brick`'s largest key was 190 KiB and
/// `study:painted-metal`'s 155 KiB, against 117 KiB and 100 KiB flattened.
/// Flattened, no plan is written twice and a key is exactly the sum of the
/// plans it depends on, so the worst case is the whole plan list once.
///
/// # What it costs
///
/// **Not a fixed price, and not always small.** A key is the sum of its
/// closure's sub-expressions, so it grows with the graph above the filter. In
/// the study library, partitioned for a shader at 1024 — which is what
/// `ashlar-bevy`'s image cache asks for:
///
/// | graph | plans | largest key | all keys |
/// | --- | --- | --- | --- |
/// | `study:concrete` | 8 | 5.0 KiB | 34 KiB |
/// | `study:concrete-wet` | 8 | 7.5 KiB | 44 KiB |
/// | `study:metal` | 4 | 3.5 KiB | 10 KiB |
/// | `study:plaster` | 9 | 97 KiB | 522 KiB |
/// | `study:painted-metal` | 11 | 100 KiB | 731 KiB |
/// | `study:brick` | 12 | 117 KiB | 672 KiB |
///
/// So "a few kilobytes" is true of a shallow graph and wrong by twenty times
/// of a deep one. What makes the trade sound anyway is the other side of it: a
/// scalar plane at 1024 is four megabytes of `f32` and a colour is twelve, and
/// `study:brick` holds five of them, so the keys are well under a per cent of
/// what they are protecting — and they grow with the size of the graph, which
/// an author writes, rather than with the resolution, which a bake picks.
/// Hashing one and comparing it is the running cost, and at the worst of those
/// rows it is a megabyte of bytes walked a few times per compile, against a
/// bake measured in seconds.
///
/// # What a plane never holds
///
/// An [`Op::Param`] encodes as the binding's name and type and *not* its
/// value, which is only sound because a plane cannot hold a live parameter:
/// [`Target::Bake`](crate::ir::Target::Bake) folds every parameter before
/// lowering, and a shader partition refuses to leave a runtime input above a
/// filter only a plane can run — `preflight_shader` is the refusal by name. If
/// one ever did slip through, a rasterisation evaluates with
/// [`Inputs::default`] and would answer [`EvalError::MissingParam`] rather
/// than a wrong plane. The invariant is worth stating here because the key
/// deliberately does not name the graph, so this is the one input it trusts
/// another layer to have ruled out.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct PlaneKey(Arc<[u8]>);

impl PlaneKey {
    /// How many bytes the key is, which is what a caller weighing the exact
    /// policy against a digest wants to know.
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// Whether it encodes nothing, which no key a plan produced ever does.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

/// The canonical encoding a [`PlaneKey`] is, under construction.
///
/// Every write is fixed width or length-prefixed, which is what makes the
/// finished bytes injective: no value can run into the next one, so two
/// different planes cannot write the same stream. That holds for *every* list
/// written here, the filter's settings included — a list whose length happened
/// to be fixed today but was written without its count would become ambiguous
/// the day a filter took a third setting, and an ambiguous key is the one
/// failure the exact policy exists to rule out.
#[derive(Default)]
struct Encoding(Vec<u8>);

impl Encoding {
    fn number(&mut self, value: u64) {
        self.0.extend_from_slice(&value.to_le_bytes());
    }

    fn count(&mut self, value: usize) {
        self.number(u64::try_from(value).unwrap_or(u64::MAX));
    }

    fn text(&mut self, value: &str) {
        self.count(value.len());
        self.0.extend_from_slice(value.as_bytes());
    }

    /// A plane's position in the closure being encoded, tagged so that "no
    /// plane" and "the plane at position zero" cannot be confused.
    fn place(&mut self, place: Option<u64>) {
        match place {
            Some(place) => {
                self.0.push(1);
                self.number(place);
            }
            None => self.0.push(0),
        }
    }

    fn finish(self) -> PlaneKey {
        PlaneKey(Arc::from(self.0))
    }
}

/// The key of every plane a lowered expression rasterises, in [`Ir::buffers`]
/// order.
///
/// What [`rasterise_buffers`] keys its cache on, computed without rasterising
/// anything — which is what lets a caller holding *encoded* planes, as
/// `ashlar-bevy`'s bound images are, find out that it already has them before
/// paying for a single texel. A plan whose resolution is pinned is keyed at
/// that resolution, exactly as it is rasterised at it.
///
/// Quadratic in the plan count and linear in the size of the expression, since
/// every plan walks its own closure and every plan in that closure asks which
/// instructions its root reaches. That is a dozen walks of a few thousand
/// instructions for the graphs in the study library, against the megatexel one
/// plane costs; it is written down because the shape is worth knowing, not
/// because it has ever been the expensive part.
pub fn plane_keys(ir: &Ir, resolution: u32) -> Vec<PlaneKey> {
    (0..ir.buffers().len())
        .map(|index| plane_key(ir, index, resolution))
        .collect()
}

/// One plane's key: its closure, flattened.
fn plane_key(ir: &Ir, index: usize, resolution: u32) -> PlaneKey {
    let order = plan_closure_order(ir, index);
    // Where each plan of the closure sits in it. A plan outside the closure is
    // never named by one inside it, so the slots left at zero are never read.
    let mut places: Vec<u64> = vec![0; ir.buffers().len()];
    for (place, plan) in order.iter().enumerate() {
        if let Some(slot) = places.get_mut(*plan) {
            *slot = place as u64;
        }
    }
    let plans: Vec<&BufferPlan> = order
        .iter()
        .filter_map(|index| ir.buffers().get(*index))
        .collect();
    let mut encoding = Encoding::default();
    encoding.count(plans.len());
    for plan in plans {
        plan_encoding(
            ir,
            plan,
            plan.resolution.unwrap_or(resolution).max(1),
            &places,
            &mut encoding,
        );
    }
    encoding.finish()
}

/// The plans one plane's closure holds, each once, dependencies before the
/// plan that reads them and the plane itself last.
///
/// A post-order walk from the root plan rather than a sweep of the plan list,
/// and that is what makes the numbering canonical: it is decided by the shape
/// of the dependency graph alone, so the same sub-expression is numbered the
/// same however the lowering that produced it happened to order its plans.
/// Sweeping the list in index order would have keyed one wall two ways for two
/// graphs that built it in a different order, and quietly stopped sharing it.
fn plan_closure_order(ir: &Ir, index: usize) -> Vec<usize> {
    /// Enter a plan, or emit it once everything under it has been emitted.
    enum Step {
        Enter(usize),
        Emit(usize),
    }
    let mut order: Vec<usize> = Vec::new();
    let mut seen = vec![false; ir.buffers().len()];
    let mut stack = vec![Step::Enter(index)];
    while let Some(step) = stack.pop() {
        match step {
            Step::Emit(at) => order.push(at),
            Step::Enter(at) => {
                // A plan can only name a plan before it, so the plans are a
                // DAG and a plan already entered is one already finished or
                // one on the way to being: either way it is not entered twice.
                match seen.get_mut(at) {
                    Some(seen @ false) => *seen = true,
                    _ => continue,
                }
                stack.push(Step::Emit(at));
                for dep in plan_deps(ir, at).into_iter().rev() {
                    stack.push(Step::Enter(dep));
                }
            }
        }
    }
    order
}

/// One plan of a closure, written into the encoding.
///
/// `places` is where each plan of the closure sits in it, which is what stands
/// in for every [`BufferId`](crate::ir::BufferId): an index into one lowering's
/// plan list means nothing outside that lowering, and a position in the closure
/// means the same thing in every lowering that produced this closure.
fn plan_encoding(
    ir: &Ir,
    plan: &BufferPlan,
    resolution: u32,
    places: &[u64],
    encoding: &mut Encoding,
) {
    let place = |buffer: usize| places.get(buffer).copied();
    encoding.number(u64::from(resolution));
    let (tag, settings) = plan.filter.code();
    encoding.number(u64::from(tag));
    encoding.count(settings.len());
    for setting in settings {
        encoding.number(u64::from(setting));
    }
    encoding.place(plan.filter.guide().and_then(|guide| place(guide.index())));
    // What a strand plane is splatted from, which nothing in the arena above it
    // encodes: the layer's whole lowered field expression, its constants and
    // the repeat it is measured against. Length-prefixed like every other list
    // here, so a plan with a scatter and one without cannot write the same
    // bytes.
    match &plan.strands {
        Some(strands) => {
            encoding.count(strands.key().len());
            encoding.0.extend_from_slice(strands.key());
        }
        None => encoding.count(0),
    }
    encoding.number(plan.value_type.code());
    let live = ir.reaches(&[plan.root]);
    let mut numbers: Vec<u64> = vec![0; ir.len()];
    let mut next = 0;
    encoding.count(live.iter().filter(|live| **live).count());
    for (index, inst) in ir.insts().iter().enumerate() {
        if live.get(index).copied() != Some(true) {
            continue;
        }
        if let Some(slot) = numbers.get_mut(index) {
            *slot = next;
        }
        next += 1;
        let (tag, payload) = inst.op.code();
        encoding.number(u64::from(tag));
        // Two payloads are indices into another list rather than values, and
        // an index means nothing outside the lowering that handed it out. A
        // sample encodes as where the plane it reads sits in this closure,
        // which it is always in, because `plan_deps` is what built the
        // closure and what finds the sample. A parameter encodes as the
        // binding it names, so two graphs whose uniforms happen to sit at the
        // same index are not the same expression.
        match inst.op {
            Op::Sample(buffer) => {
                encoding.place(place(buffer.index()));
            }
            Op::Param(index) => {
                let binding = ir.params().get(index as usize);
                encoding.text(binding.map_or("", |binding| binding.name.as_str()));
                encoding.number(binding.map_or(0, |binding| binding.value_type.code()));
            }
            _ => encoding.number(u64::from(payload)),
        }
        encoding.count(inst.operands.len());
        for operand in inst.operands.as_slice() {
            encoding.number(numbers.get(operand.index()).copied().unwrap_or_default());
        }
        encoding.number(inst.value_type.code());
    }
}

/// Whether the caller of a plane pass will read the [`MemoTable`] it leaves
/// behind.
///
/// It decides nothing about the texels and everything about the memory. A
/// value's last reader is very often the outputs stage, and a pass whose table
/// is dropped on the way out — [`rasterise_buffers`], [`rasterise_wanted`], and
/// so the CPU fallback in `ashlar-bevy` — would still write a plane of every
/// one of them: 251 MB on the SOI cobblestone, allocated, filled and freed
/// without a reader. [`Self::Discarded`] keeps only the values a *plane* stage
/// reads back, which is what such a pass actually uses.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Table {
    /// The caller rasterises the outputs next and wants everything.
    Kept,
    /// The caller drops the table, so only what the plane stages read is
    /// worth writing.
    Discarded,
}

/// What a rasterisation reads back from the stages before it, and what it
/// leaves behind for the stages after.
///
/// Both empty for a plan at its own resolution, whose texels are nobody
/// else's, and both worked out by [`memo`](crate::memo).
#[derive(Clone, Copy, Default)]
struct Memo<'a> {
    /// The values read out of an earlier stage's plane instead of evaluated.
    frontier: &'a [(ValueId, &'a Plane)],
    /// The values this stage must leave a plane of behind.
    keep: &'a [ValueId],
}

/// Evaluate one sub-expression into a plane, rows across threads, reading
/// back what an earlier stage computed and keeping what a later one will
/// want.
///
/// The values it reads back and the values it keeps are [`Memo`]. The kept
/// planes are written from the register file beside the root's own, in the
/// same pass and by the same threads, so a kept value costs the lanes it
/// occupies and not a second walk over anything.
fn rasterise_root(
    ir: &Ir,
    root: ValueId,
    value_type: IrType,
    resolution: u32,
    buffers: &[Plane],
    threads: Option<NonZeroUsize>,
    memo: Memo<'_>,
) -> Result<(Plane, Vec<(ValueId, Plane)>), EvalError> {
    let Memo { frontier, keep } = memo;
    let components = value_type.components();
    let mut lanes = vec![0.0_f32; texels(resolution) * components];
    let stride = resolution as usize * components;
    let kept: Vec<(ValueId, usize)> = keep
        .iter()
        .filter_map(|value| Some((*value, ir.type_of(*value)?.components())))
        .collect();
    let mut extra: Vec<Vec<f32>> = kept
        .iter()
        .map(|(_, lanes)| vec![0.0_f32; texels(resolution) * lanes])
        .collect();
    {
        let mut rest = lanes.as_mut_slice();
        let mut extra_rest: Vec<&mut [f32]> = extra.iter_mut().map(Vec::as_mut_slice).collect();
        let mut jobs = Vec::new();
        for (first, rows) in spans(resolution, threads) {
            let mut mine = Vec::with_capacity(kept.len());
            for (slot, (_, lanes)) in extra_rest.iter_mut().zip(&kept) {
                mine.push(take(slot, rows as usize * resolution as usize * lanes));
            }
            jobs.push((first, take(&mut rest, rows as usize * stride), mine));
        }
        let kept = &kept;
        run_jobs(jobs, |(first, chunk, mut mine)| {
            // Only what the root reaches, and only down to the values
            // an earlier stage already has: a bake does not evaluate
            // the other outputs, the planes that come after this one,
            // or anything it can read back.
            let interpreter = Interpreter::for_values_memoised(ir, &[root], frontier);
            // The root and the values a later stage wants kept are
            // what is read out of the file after a strip, so their
            // slots are the ones that must survive it.
            let mut pinned = Vec::with_capacity(1 + kept.len());
            pinned.push(root);
            pinned.extend(kept.iter().map(|(value, _)| *value));
            let plan = interpreter.strip_plan(&pinned);
            let mut file = plan.file();
            let root_at = plan.slot(root);
            let kept_at: Vec<Option<usize>> =
                kept.iter().map(|(value, _)| plan.slot(*value)).collect();
            let inputs = Inputs {
                buffers,
                ..Inputs::default()
            };
            let mut uvs = [[0.0_f32; 2]; STRIP];
            let width = resolution as usize;
            let rows = chunk.len() / stride.max(1);
            for row in 0..rows {
                let y = first as usize + row;
                let v = centre(y, resolution);
                let mut x = 0;
                while x < width {
                    let span = STRIP.min(width - x);
                    for (at, uv) in uvs.iter_mut().take(span).enumerate() {
                        *uv = [centre(x + at, resolution), v];
                    }
                    plan.run(y * width + x, &uvs[..span], &inputs, &mut file)?;
                    for at in 0..span {
                        let value = root_at.map_or([0.0; 3], |slot| file.value(slot, at));
                        for (lane, component) in value.iter().enumerate().take(components) {
                            put(
                                chunk,
                                row * stride + (x + at) * components + lane,
                                *component,
                            );
                        }
                        for ((out, (_, lanes)), slot) in mine.iter_mut().zip(kept).zip(&kept_at) {
                            let value = slot.map_or([0.0; 3], |slot| file.value(slot, at));
                            let base = (row * width + x + at) * lanes;
                            for (lane, component) in value.iter().enumerate().take(*lanes) {
                                put(out, base + lane, *component);
                            }
                        }
                    }
                    x += span;
                }
            }
            Ok(())
        })?;
    }
    let produced = kept
        .iter()
        .zip(extra)
        .filter_map(|((value, _), lanes)| {
            Some((*value, Plane::new(resolution, ir.type_of(*value)?, lanes)))
        })
        .collect();
    Ok((Plane::new(resolution, value_type, lanes), produced))
}

/// The UV of the centre of a texel's cell, which is where the rasteriser reads
/// the field and where [`Plane::sample`] reads the plane back exactly.
#[expect(
    clippy::cast_precision_loss,
    reason = "a texel coordinate is bounded by MAX_RESOLUTION, well inside what an f32 counts exactly"
)]
fn centre(index: usize, resolution: u32) -> f32 {
    (index as f32 + 0.5) / resolution.max(1) as f32
}

/// Run one buffered filter over the plane its node rasterised.
///
/// The pass between a rasterisation and the [`Op::Sample`]s that read it, and
/// the half of a plane that is ordinary Rust over the lanes rather than an
/// expression. It is public because the rasterisation is the half a backend may
/// replace: a GPU bake evaluates a plane's own sub-expression on the device,
/// reads the texels back, and comes here for the filter — which is where the
/// two backends have to agree exactly, since a jump flood or a horizon march is
/// not something a tolerance covers.
///
/// `earlier` is the planes already rasterised, in [`Ir::buffers`] order, which
/// a slope blur reads its guide out of and every other filter ignores.
pub fn filter(
    filter: Filter,
    plane: Plane,
    earlier: &[Plane],
    threads: Option<NonZeroUsize>,
) -> Plane {
    match filter {
        // Neither of these two runs anything over the plane it was handed, and
        // they answer it for different reasons. A cache point has no filter at
        // all. A strand plane is *splatted* rather than filtered, by
        // [`strand_plane`]; reaching here with one is a backend that rasterised
        // its constant root as an ordinary sub-expression and then came for a
        // filter, and the honest answer to that is what it was handed.
        Filter::None | Filter::Strands { .. } => plane,
        Filter::Blur { radius } => blur(&plane, radius, threads),
        Filter::Directional { radius, radians } => directional(&plane, radius, radians, threads),
        Filter::Slope {
            radius,
            steps,
            mode,
            guide,
        } => match earlier.get(guide.index()) {
            Some(guide) => slope(&plane, guide, radius, steps, mode, threads),
            // A plan may only name a plane that was rasterised before it, so
            // this is a lowering that built its plans out of order rather than
            // anything an author can write, and the honest answer to a walk
            // with nothing to walk down is the plane it started from.
            None => plane,
        },
        Filter::Occlusion { radius, strength } => occlusion(&plane, radius, strength, threads),
        Filter::Distance { threshold } => distance(&plane, threshold, threads),
        Filter::Erode { radius } => morphology(&plane, radius, Extreme::Low, threads),
        Filter::Dilate { radius } => morphology(&plane, radius, Extreme::High, threads),
        Filter::Curvature { radius, kind } => curvature(&plane, radius, kind, threads),
        Filter::Edge { radius } => edge(&plane, radius, threads),
        Filter::Normal { strength } => normal_from_height(&plane, strength, threads),
    }
}

/// The tangent-space normal of a height plane, by wrapped central difference.
///
/// The same arithmetic the bake derives the material's own normal with, over
/// the same two-texel span and with the same sign, because a graph that blends
/// normals and a graph that binds a height have to be describing one surface.
/// [`crate::bake`] says why the difference is per UV unit rather than per texel
/// and why both derivatives are negated; the short of it is that `strength` is
/// metres of relief per unit of height across one repeat, so a bake at 512 and
/// a bake at 2048 answer the same slope, and rows run down the texture, so a
/// surface rising toward larger `v` tilts its normal toward smaller `v`.
///
/// This is a filter rather than an expression because "at texel size" is a fact
/// about the plane. A node cannot know it: a graph is authored once and baked at
/// whatever resolution the caller asked for.
#[expect(
    clippy::cast_precision_loss,
    reason = "a texel coordinate is bounded by MAX_RESOLUTION, well inside what an f32 counts exactly"
)]
fn normal_from_height(plane: &Plane, strength: f32, threads: Option<NonZeroUsize>) -> Plane {
    let resolution = plane.resolution();
    let components = IrType::Vec3.components();
    let stride = resolution as usize * components;
    let mut lanes = vec![0.0_f32; texels(resolution) * components];
    // The two taps are two texels apart, and the field spans one UV unit.
    let span = 2.0 / resolution.max(1) as f32;
    rows_across_threads(&mut lanes, resolution, stride, threads, |first, chunk| {
        for (row, line) in chunk.chunks_mut(stride.max(1)).enumerate() {
            let y = i64::from(first) + i64::try_from(row).unwrap_or_default();
            for x in 0..i64::from(resolution) {
                let du = (plane.texel(x + 1, y)[0] - plane.texel(x - 1, y)[0]) / span;
                let dv = (plane.texel(x, y + 1)[0] - plane.texel(x, y - 1)[0]) / span;
                let normal = unit(-strength * du, -strength * dv);
                for (lane, component) in normal.iter().enumerate() {
                    put(
                        line,
                        usize::try_from(x).unwrap_or_default() * components + lane,
                        *component,
                    );
                }
            }
        }
    });
    Plane::new(resolution, IrType::Vec3, lanes)
}

/// `(x, y, 1)` normalised, or flat where the height field was not finite.
///
/// A height of infinity is a graph an author should fix, not a normal of `NaN`
/// that encodes to noise. Both the bake's own normal and
/// [`Filter::Normal`] come through here, which is what makes them the same
/// derivation rather than two that agree today.
pub(crate) fn unit(x: f32, y: f32) -> [f32; 3] {
    let length = x.mul_add(x, y.mul_add(y, 1.0)).sqrt();
    if length.is_finite() && length > 0.0 {
        [x / length, y / length, 1.0 / length]
    } else {
        [0.0, 0.0, 1.0]
    }
}

/// Which axis a separable pass runs along.
///
/// The passes step the coordinate themselves rather than asking this for a
/// `(x, y)` pair: a tap along `U` keeps the row base its output texel already
/// has, and a tap along `V` keeps the column, so the pair would only be
/// wrapped back apart again.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Axis {
    U,
    V,
}

/// How far a UV radius reaches in texels, bounded so that a tap cannot wrap
/// past the plane and land on the texel it started from.
///
/// Zero is an identity filter rather than an error: a radius under half a texel
/// has nothing to reach, and a graph authored at 4096 and previewed at 256 is
/// allowed to have filters that do nothing at the smaller size.
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    reason = "the value is compared against the bound before the cast, and both \
              a resolution and half of one are exact in an f32"
)]
fn reach(radius: f32, resolution: u32) -> i64 {
    if !radius.is_finite() || radius <= 0.0 {
        return 0;
    }
    let bound = i64::from(resolution.max(1) / 2);
    let texels = (radius * resolution as f32).round();
    if texels <= 0.0 {
        0
    } else if texels >= bound as f32 {
        bound
    } else {
        texels as i64
    }
}

/// The half-kernel of a Gaussian truncated at `reach`.
///
/// The standard deviation is a third of the reach, so the taps that are kept
/// hold all but three parts in a thousand of the weight and the truncation is
/// not something an author has to think about. The weights are normalised over
/// what is kept, so a blur of a constant is that constant exactly whatever the
/// radius — which is the property a filter over a mask has to have, or every
/// blur would also be a slight darkening.
#[expect(
    clippy::cast_precision_loss,
    reason = "a reach is bounded by half of MAX_RESOLUTION"
)]
fn gaussian(reach: i64) -> Vec<f32> {
    let sigma = (reach as f32 / 3.0).max(1e-3);
    let mut weights: Vec<f32> = (0..=reach)
        .map(|offset| {
            let t = offset as f32 / sigma;
            (-0.5 * t * t).exp()
        })
        .collect();
    let total: f32 = weights
        .iter()
        .enumerate()
        .map(|(offset, weight)| if offset == 0 { *weight } else { 2.0 * weight })
        .sum();
    if total > 0.0 {
        for weight in &mut weights {
            *weight /= total;
        }
    }
    weights
}

/// A wrapped, separable Gaussian blur.
fn blur(plane: &Plane, radius: f32, threads: Option<NonZeroUsize>) -> Plane {
    let reach = reach(radius, plane.resolution());
    if reach == 0 {
        return plane.clone();
    }
    let kernel = gaussian(reach);
    let horizontal = convolve(plane, &kernel, Axis::U, threads);
    convolve(&horizontal, &kernel, Axis::V, threads)
}

/// One separable pass: the weighted sum of the taps along one axis, wrapped.
///
/// The taps are read out of the lane slice rather than through
/// [`Plane::texel`]: the tap order is the one the comment below names, and the
/// accumulation is the same `sum += weight * lane`, but a tap costs one wrap
/// and one add where a texel read cost a wrap per axis and a three-lane copy.
/// Along [`Axis::U`] every tap of a texel is in the row the output texel is in,
/// so that row's base is computed once; along [`Axis::V`] they are the same
/// column of other rows, so the column is.
fn convolve(plane: &Plane, kernel: &[f32], axis: Axis, threads: Option<NonZeroUsize>) -> Plane {
    let resolution = plane.resolution();
    let components = plane.components();
    let stride = resolution as usize * components;
    let source = plane.lanes();
    let mut lanes = vec![0.0_f32; texels(resolution) * components];
    rows_across_threads(&mut lanes, resolution, stride, threads, |first, chunk| {
        for (row, line) in chunk.chunks_mut(stride.max(1)).enumerate() {
            let y = i64::try_from(first as usize + row).unwrap_or_default();
            let row_base = plane.wrap(y) * plane.row_stride();
            for x in 0..resolution as usize {
                let x = i64::try_from(x).unwrap_or_default();
                let column = plane.wrap(x) * components;
                let mut sums = [0.0_f32; 3];
                for (offset, weight) in kernel.iter().enumerate() {
                    let offset = i64::try_from(offset).unwrap_or_default();
                    // The centre tap is one tap; every other is the pair that
                    // shares its weight, the negative before the positive.
                    let taps: &[i64] = if offset == 0 {
                        &[0]
                    } else {
                        &[-offset, offset]
                    };
                    for step in taps {
                        let base = match axis {
                            Axis::U => row_base + plane.wrap(x + step) * components,
                            Axis::V => plane.wrap(y + step) * plane.row_stride() + column,
                        };
                        for (lane, sum) in sums.iter_mut().enumerate().take(components) {
                            *sum += weight * source.get(base + lane).copied().unwrap_or_default();
                        }
                    }
                }
                let base = usize::try_from(x).unwrap_or_default() * components;
                for (lane, sum) in sums.iter().enumerate().take(components) {
                    put(line, base + lane, *sum);
                }
            }
        }
    });
    Plane::new(resolution, plane.value_type(), lanes)
}

/// A wrapped Gaussian along one direction only, which is what turns a grain
/// into a brushed metal.
///
/// The same kernel [`blur`] uses and the same reach, laid along one line
/// instead of over both axes, so a radius means what it means everywhere else
/// in the crate. It is not separable — a direction that is not an axis reads
/// between texels — so the taps go through [`Plane::sample`], which is
/// bilinear and wrapped, and the result tiles because every tap does.
#[expect(
    clippy::cast_precision_loss,
    reason = "a tap offset is bounded by half of MAX_RESOLUTION, which an f32 counts exactly"
)]
fn directional(plane: &Plane, radius: f32, radians: f32, threads: Option<NonZeroUsize>) -> Plane {
    let resolution = plane.resolution();
    let reach = reach(radius, resolution);
    if reach == 0 {
        return plane.clone();
    }
    let kernel = gaussian(reach);
    let texel = 1.0 / resolution.max(1) as f32;
    let (sine, cosine) = radians.sin_cos();
    let step = [cosine * texel, sine * texel];
    let components = plane.components();
    let stride = resolution as usize * components;
    // A kernel of reach `r` is `2r + 1` taps a texel, all of them bilinear
    // reads of this one plane — 205 of them at the radius the SOI cobblestone's
    // brushing asks for at 2048 — so the geometry is hoisted, as in `slope`.
    let source = plane.sampler();
    let mut lanes = vec![0.0_f32; texels(resolution) * components];
    rows_across_threads(&mut lanes, resolution, stride, threads, |first, chunk| {
        for (row, line) in chunk.chunks_mut(stride.max(1)).enumerate() {
            let v = centre(first as usize + row, resolution);
            for x in 0..resolution as usize {
                let u = centre(x, resolution);
                let mut sums = [0.0_f32; 3];
                for (offset, weight) in kernel.iter().enumerate() {
                    let offset = offset as f32;
                    // The centre tap is one tap; every other is the pair that
                    // shares its weight, the negative before the positive,
                    // exactly as the separable pass does.
                    let taps: &[f32] = if offset == 0.0 {
                        &[0.0]
                    } else {
                        &[-offset, offset]
                    };
                    for along in taps {
                        let at = [step[0].mul_add(*along, u), step[1].mul_add(*along, v)];
                        if components == 1 {
                            // A height or a mask is one lane, and asking for
                            // that lane alone keeps the three-lane array out of
                            // the innermost loop of the widest kernel here.
                            sums[0] += weight * source.float(at);
                            continue;
                        }
                        let value = source.sample(at);
                        for (sum, lane) in sums.iter_mut().zip(value).take(components) {
                            *sum += weight * lane;
                        }
                    }
                }
                let base = x * components;
                for (lane, sum) in sums.iter().enumerate().take(components) {
                    put(line, base + lane, *sum);
                }
            }
        }
    });
    Plane::new(resolution, plane.value_type(), lanes)
}

/// The plane smeared along the slope of another one, wrapped.
///
/// Material Maker's slope blur, and the same construction: from each texel the
/// filter walks `steps` times, each step `radius / steps` UV units *down* the
/// gradient of `guide`, and answers the mean of the source at where it started
/// and at every place it stopped. Flat ground does not move at all, so the
/// field is left exactly as it was wherever the guide is level, and smeared
/// only where it falls — which is what makes this wear rather than blur.
///
/// Reading downhill means the *field* travels up: a texel takes what lies
/// below it, so rust in a chip creeps onto the paint around it and damp in a
/// joint creeps onto the brick. Inverting the height walks the other way, and
/// carries what is on the crests down into the hollows.
///
/// The displacement is *normalised*: a step is `radius / steps` of UV whatever
/// the guide's own units are. Walking by the raw gradient, as a naive reading
/// of "displace along the slope" would, would make the reach depend on how
/// deep the height field happens to be — and every other radius in this crate
/// is a reach in UV, which is the promise that the same graph describes the
/// same surface at 512 and at 4096.
///
/// Normalised, but not by the length alone. Where the guide is flat the
/// gradient is nothing and its *direction* is whatever the last bits of two
/// subtractions say, so a walk normalised by the length would set off in an
/// arbitrary direction there — and two backends that agree on the height to a
/// millionth would disagree completely about which way it falls, which is a
/// difference no tolerance can be written for. So the step is divided by the
/// length or by [`SLOPE_FLOOR`], whichever is larger: below the floor the
/// displacement shrinks with the gradient instead of turning with it, which is
/// continuous through zero and is also what flat ground should do, which is
/// nothing.
///
/// The gradient is a central difference over the walk's own step rather than
/// over one texel, never shorter than a texel of the guide: a slope blur of
/// radius `r` follows the relief at the scale of `r`, and the one-texel
/// difference of a height with a grain on it is mostly the grain.
#[expect(
    clippy::cast_precision_loss,
    reason = "a step count is single digits and a resolution is bounded by MAX_RESOLUTION"
)]
fn slope(
    plane: &Plane,
    guide: &Plane,
    radius: f32,
    steps: u32,
    mode: crate::nodes::SlopeMode,
    threads: Option<NonZeroUsize>,
) -> Plane {
    let resolution = plane.resolution();
    let steps = steps.max(1);
    let walk = radius / steps as f32;
    // A reach under half a texel has nothing to walk to, as a blur of that
    // radius has nothing to reach.
    if !walk.is_finite() || reach(radius, resolution) == 0 {
        return plane.clone();
    }
    let epsilon = walk.max(1.0 / guide.resolution().max(1) as f32);
    let mean = if mode == crate::nodes::SlopeMode::Average {
        1.0 / (steps as f32 + 1.0)
    } else {
        1.0
    };
    let components = plane.components();
    let stride = resolution as usize * components;
    // The walk takes `4 * steps` samples of the guide and `steps + 1` of the
    // source per texel — 161 of them at the 32 steps the SOI cobblestone asks
    // for — so the plane geometry each one would work out again is worked out
    // here instead. The coordinates and the arithmetic are unchanged.
    let source = plane.sampler();
    let guide = guide.sampler();
    let mut lanes = vec![0.0_f32; texels(resolution) * components];
    rows_across_threads(&mut lanes, resolution, stride, threads, |first, chunk| {
        for (row, line) in chunk.chunks_mut(stride.max(1)).enumerate() {
            let start = centre(first as usize + row, resolution);
            for x in 0..resolution as usize {
                let (mut u, mut v) = (centre(x, resolution), start);
                let mut sums = source.sample([u, v]);
                for _ in 0..steps {
                    let du = guide.float([u + epsilon, v]) - guide.float([u - epsilon, v]);
                    let dv = guide.float([u, v + epsilon]) - guide.float([u, v - epsilon]);
                    let length = du.mul_add(du, dv * dv).sqrt();
                    if length.is_finite() {
                        let step = walk / length.max(SLOPE_FLOOR);
                        u -= step * du;
                        v -= step * dv;
                    }
                    let value = source.sample([u, v]);
                    // Only the lanes the plane owns: the rest are the zeroes
                    // `sample` left behind and nothing below writes them out.
                    for (sum, lane) in sums.iter_mut().zip(value).take(components) {
                        *sum = match mode {
                            crate::nodes::SlopeMode::Average => *sum + lane,
                            crate::nodes::SlopeMode::Min => sum.min(lane),
                            crate::nodes::SlopeMode::Max => sum.max(lane),
                        };
                    }
                }
                let base = x * components;
                for (lane, sum) in sums.iter().enumerate().take(components) {
                    put(line, base + lane, sum * mean);
                }
            }
        }
    });
    Plane::new(resolution, plane.value_type(), lanes)
}

/// How far a plane stands above its own neighbourhood, wrapped.
///
/// Positive on a crest, negative in a hollow, zero on anything flat or
/// straight — a ramp has no curvature, which is what tells this apart from an
/// [`edge`]. The two kinds differ only in what "neighbourhood" means: four taps
/// at the reach, which is the discrete Laplacian negated so that a crest is
/// positive, or the whole Gaussian of that radius, which is the difference of
/// the height and its blur and is smoother for the same reason a blur is.
///
/// What it answers is a *difference of heights* rather than a second
/// derivative. Dividing by the radius squared would make it a curvature in the
/// textbook sense and would also make a small radius answer enormous numbers
/// for a surface nobody would call sharply curved; leaving it as a difference
/// keeps the magnitude the relief of the feature, which is the number an author
/// already has a feel for from the height field itself. A gain turns it into a
/// mask, and [`Curvature`](crate::nodes::Curvature) carries one.
fn curvature(
    plane: &Plane,
    radius: f32,
    kind: CurvatureKind,
    threads: Option<NonZeroUsize>,
) -> Plane {
    let resolution = plane.resolution();
    let stride = resolution as usize;
    let mut lanes = vec![0.0_f32; texels(resolution)];
    let reach = reach(radius, resolution);
    if reach == 0 {
        // Nothing to compare against is no curvature, which is the identity a
        // radius under half a texel has, as it has for every other filter.
        return Plane::new(resolution, IrType::Float, lanes);
    }
    let blurred = match kind {
        CurvatureKind::Laplacian => None,
        CurvatureKind::Blurred => Some(blur(plane, radius, threads)),
    };
    rows_across_threads(&mut lanes, resolution, stride, threads, |first, chunk| {
        for (row, line) in chunk.chunks_mut(stride.max(1)).enumerate() {
            let y = i64::try_from(first as usize + row).unwrap_or_default();
            for x in 0..resolution as usize {
                let x = i64::try_from(x).unwrap_or_default();
                let height = plane.texel(x, y)[0];
                let around = match &blurred {
                    Some(blurred) => blurred.texel(x, y)[0],
                    None => {
                        0.25 * (plane.texel(x + reach, y)[0]
                            + plane.texel(x - reach, y)[0]
                            + plane.texel(x, y + reach)[0]
                            + plane.texel(x, y - reach)[0])
                    }
                };
                put(
                    line,
                    usize::try_from(x).unwrap_or_default(),
                    height - around,
                );
            }
        }
    });
    Plane::new(resolution, IrType::Float, lanes)
}

/// How much a plane changes across the reach, wrapped: the length of its two
/// central differences.
///
/// A difference rather than a slope — the pair is not divided by the span it
/// was taken over — so the edge of a mask that steps from zero to one is one
/// whatever the radius, and the radius sets how wide the line is rather than
/// how bright. A slope would answer one over the span there, which is a number
/// between 25 and 400 at the resolutions a bake runs at and a mask nobody can
/// use.
fn edge(plane: &Plane, radius: f32, threads: Option<NonZeroUsize>) -> Plane {
    let resolution = plane.resolution();
    let stride = resolution as usize;
    let mut lanes = vec![0.0_f32; texels(resolution)];
    let reach = reach(radius, resolution).max(1);
    rows_across_threads(&mut lanes, resolution, stride, threads, |first, chunk| {
        for (row, line) in chunk.chunks_mut(stride.max(1)).enumerate() {
            let y = i64::try_from(first as usize + row).unwrap_or_default();
            for x in 0..resolution as usize {
                let x = i64::try_from(x).unwrap_or_default();
                let du = plane.texel(x + reach, y)[0] - plane.texel(x - reach, y)[0];
                let dv = plane.texel(x, y + reach)[0] - plane.texel(x, y - reach)[0];
                put(
                    line,
                    usize::try_from(x).unwrap_or_default(),
                    du.mul_add(du, dv * dv).sqrt(),
                );
            }
        }
    });
    Plane::new(resolution, IrType::Float, lanes)
}

/// Which end of the neighbourhood a morphology keeps.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Extreme {
    /// Erode: the smallest value, so the bright parts shrink.
    Low,
    /// Dilate: the largest, so they grow.
    High,
}

impl Extreme {
    fn of(self, left: f32, right: f32) -> f32 {
        match self {
            Self::Low => left.min(right),
            Self::High => left.max(right),
        }
    }
}

/// A wrapped erosion or dilation over the square neighbourhood of a radius.
///
/// The square rather than the disc, for two reasons that point the same way: a
/// square is separable, so the cost is linear in the radius where a disc is
/// quadratic; and an erosion followed by a dilation over a square gives an
/// axis-aligned mask back unchanged, where a disc would round its corners. The
/// difference shows on a diagonal edge, which a square grows by a little more
/// than its radius, and that is the trade this takes.
fn morphology(
    plane: &Plane,
    radius: f32,
    extreme: Extreme,
    threads: Option<NonZeroUsize>,
) -> Plane {
    let reach = reach(radius, plane.resolution());
    if reach == 0 {
        return plane.clone();
    }
    let horizontal = sweep(plane, reach, extreme, Axis::U, threads);
    sweep(&horizontal, reach, extreme, Axis::V, threads)
}

/// One separable min or max pass along an axis, wrapped.
///
/// Reads rows the way [`convolve`] does, and in the same order — the centre,
/// then the negative tap of each offset before its positive — because a min and
/// a max are only associative in exact arithmetic when nothing is `NaN`, and a
/// height plane is allowed to hold one.
fn sweep(
    plane: &Plane,
    reach: i64,
    extreme: Extreme,
    axis: Axis,
    threads: Option<NonZeroUsize>,
) -> Plane {
    let resolution = plane.resolution();
    let components = plane.components();
    let stride = resolution as usize * components;
    let source = plane.lanes();
    let mut lanes = vec![0.0_f32; texels(resolution) * components];
    rows_across_threads(&mut lanes, resolution, stride, threads, |first, chunk| {
        for (row, line) in chunk.chunks_mut(stride.max(1)).enumerate() {
            let y = i64::try_from(first as usize + row).unwrap_or_default();
            let row_base = plane.wrap(y) * plane.row_stride();
            for x in 0..resolution as usize {
                let x = i64::try_from(x).unwrap_or_default();
                let column = plane.wrap(x) * components;
                let read =
                    |base: usize, lane: usize| source.get(base + lane).copied().unwrap_or_default();
                let mut best = [0.0_f32; 3];
                for (lane, slot) in best.iter_mut().enumerate().take(components) {
                    *slot = read(row_base + column, lane);
                }
                for offset in 1..=reach {
                    for step in [-offset, offset] {
                        let base = match axis {
                            Axis::U => row_base + plane.wrap(x + step) * components,
                            Axis::V => plane.wrap(y + step) * plane.row_stride() + column,
                        };
                        for (lane, slot) in best.iter_mut().enumerate().take(components) {
                            *slot = extreme.of(*slot, read(base, lane));
                        }
                    }
                }
                let base = usize::try_from(x).unwrap_or_default() * components;
                for (lane, value) in best.iter().enumerate().take(components) {
                    put(line, base + lane, *value);
                }
            }
        }
    });
    Plane::new(resolution, plane.value_type(), lanes)
}

/// Horizon-based occlusion over a height plane, wrapped.
///
/// For every texel the filter marches [`HORIZON_DIRECTIONS`] directions out to
/// the radius, and in each keeps the steepest rise it saw: the sine of that
/// angle is how much of that direction's sky the surface took away, and the
/// mean over the directions is how much of the whole sky it took. One is a
/// texel that sees all of it.
///
/// The height is read as a length in the same units as UV, which is what makes
/// the answer a slope: a joint 0.16 deep and a texel wide is a wall, and the
/// texel at its foot is dark on the two or three directions that face it and
/// open on the rest. A field that is flat, or that only falls away, occludes
/// nothing — nothing is above the horizon — which is why a pit darkens its own
/// floor and not the ground around it.
#[expect(
    clippy::cast_precision_loss,
    reason = "the direction and step counts are single digits"
)]
fn occlusion(plane: &Plane, radius: f32, strength: f32, threads: Option<NonZeroUsize>) -> Plane {
    let resolution = plane.resolution();
    let stride = resolution as usize;
    let mut lanes = vec![0.0_f32; texels(resolution)];
    let directions: [[f32; 2]; HORIZON_DIRECTIONS] = std::array::from_fn(|index| {
        let angle = std::f32::consts::TAU * index as f32 / HORIZON_DIRECTIONS as f32;
        [angle.cos(), angle.sin()]
    });
    // The march is the same eight distances from every texel, so they are the
    // same `radius * step / steps` each time and are worked out here rather
    // than sixty-four times a texel.
    let reaches: [f32; HORIZON_STEPS] =
        std::array::from_fn(|index| radius * (index + 1) as f32 / HORIZON_STEPS as f32);
    // Sixty-five bilinear reads of this one plane per texel, so its geometry is
    // hoisted the way `slope` and `directional` hoist theirs.
    let source = plane.sampler();
    rows_across_threads(&mut lanes, resolution, stride, threads, |first, chunk| {
        for (row, line) in chunk.chunks_mut(stride.max(1)).enumerate() {
            let y = first as usize + row;
            let v = centre(y, resolution);
            for x in 0..resolution as usize {
                let u = centre(x, resolution);
                let height = source.float([u, v]);
                let mut taken = 0.0_f32;
                for direction in &directions {
                    let mut steepest = 0.0_f32;
                    for reach in reaches {
                        let sample = source.float([
                            direction[0].mul_add(reach, u),
                            direction[1].mul_add(reach, v),
                        ]);
                        steepest = steepest.max((sample - height) / reach);
                    }
                    // The sine of the horizon angle, without the arc tangent:
                    // `sin(atan(t))` is `t / sqrt(1 + t^2)`.
                    taken += steepest / steepest.mul_add(steepest, 1.0).sqrt();
                }
                let occluded = strength * taken / HORIZON_DIRECTIONS as f32;
                put(line, x, (1.0 - occluded).clamp(0.0, 1.0));
            }
        }
    });
    Plane::new(resolution, IrType::Float, lanes)
}

/// The gradient below which a slope blur stops turning and starts shrinking.
///
/// A difference of heights over the walk's own step, so this is a slope of
/// about a hundredth of the height field across that step — flat by any
/// reading. What the floor is really for is that the *direction* of a gradient
/// that small is numerical noise, and a filter whose answer turns on noise is
/// one the two backends cannot agree about however closely they agree on the
/// plane itself. See [`slope`].
const SLOPE_FLOOR: f32 = 1e-2;

/// A texel that no seed has claimed yet.
const UNCLAIMED: [i32; 2] = [i32::MIN, i32::MIN];

/// Wrapped distance from a mask, by jump flood, in UV units.
///
/// Every texel at or above the threshold is a seed; the answer is the distance
/// to the nearest one measured across the wrap, so the field tiles and a mask
/// near one edge is close to the texels at the other. The flood is the usual
/// halving one — passes at `n/2, n/4, ... 1`, each texel taking the best seed
/// of its nine neighbours at that spacing — with one more pass at a spacing of
/// one, which is what turns the handful of texels a plain jump flood gets
/// slightly wrong into none that a texture shows. It is `log2(n) + 1` passes
/// rather than the `radius` passes a march would take, which is why a distance
/// costs about what a blur does however far the mask is.
///
/// A mask with nothing in it answers the furthest two texels can be on a torus
/// everywhere, rather than zero: nothing is near a mask that does not exist.
fn distance(plane: &Plane, threshold: f32, threads: Option<NonZeroUsize>) -> Plane {
    let resolution = plane.resolution();
    let size = i64::from(resolution.max(1));
    let stride = resolution as usize;
    let mut seeds: Vec<[i32; 2]> = (0..texels(resolution))
        .map(|index| {
            if plane.texel_at(index)[0] >= threshold {
                let (x, y) = (index % stride.max(1), index / stride.max(1));
                [
                    i32::try_from(x).unwrap_or_default(),
                    i32::try_from(y).unwrap_or_default(),
                ]
            } else {
                UNCLAIMED
            }
        })
        .collect();
    let mut next = seeds.clone();
    let mut spacing = resolution.max(1).next_power_of_two() / 2;
    loop {
        flood(
            &seeds,
            &mut next,
            resolution,
            i64::from(spacing.max(1)),
            threads,
        );
        std::mem::swap(&mut seeds, &mut next);
        if spacing <= 1 {
            break;
        }
        spacing /= 2;
    }
    // One more at a spacing of one: the pass above ends there, and a second
    // sweep at that spacing is what cleans up the texels the halving missed.
    flood(&seeds, &mut next, resolution, 1, threads);
    std::mem::swap(&mut seeds, &mut next);

    let mut lanes = vec![0.0_f32; texels(resolution)];
    let furthest = half_diagonal(size);
    let wrap = Wrap::of(size);
    rows_across_threads(&mut lanes, resolution, stride, threads, |first, chunk| {
        for (row, line) in chunk.chunks_mut(stride.max(1)).enumerate() {
            let y = i64::try_from(first as usize + row).unwrap_or_default();
            for x in 0..resolution as usize {
                let index = usize::try_from(y).unwrap_or_default() * stride + x;
                let seed = seeds.get(index).copied().unwrap_or(UNCLAIMED);
                let x = i64::try_from(x).unwrap_or_default();
                let texels = if seed == UNCLAIMED {
                    furthest
                } else {
                    wrapped_distance(seed, x, y, wrap)
                };
                put(
                    line,
                    usize::try_from(x).unwrap_or_default(),
                    to_uv(texels, size),
                );
            }
        }
    });
    Plane::new(resolution, IrType::Float, lanes)
}

/// One jump-flood pass: every texel takes the nearest seed its nine neighbours
/// at this spacing know about.
fn flood(
    seeds: &[[i32; 2]],
    next: &mut [[i32; 2]],
    resolution: u32,
    spacing: i64,
    threads: Option<NonZeroUsize>,
) {
    let size = i64::from(resolution.max(1));
    let stride = resolution as usize;
    // The wrap is the same question for all nine neighbours of all four million
    // texels of every one of the twelve passes, so it is asked once.
    let wrap = Wrap::of(size);
    rows_across_threads(next, resolution, stride, threads, |first, chunk| {
        for (row, line) in chunk.chunks_mut(stride.max(1)).enumerate() {
            let y = i64::try_from(first as usize + row).unwrap_or_default();
            // The three rows a pass reads are the same three for the whole
            // row of outputs, in the order `dy` walked them.
            let rows = [
                wrap.signed(y - spacing) * size,
                wrap.signed(y) * size,
                wrap.signed(y + spacing) * size,
            ];
            for x in 0..resolution as usize {
                let x = i64::try_from(x).unwrap_or_default();
                let columns = [
                    wrap.signed(x - spacing),
                    wrap.signed(x),
                    wrap.signed(x + spacing),
                ];
                let mut best = UNCLAIMED;
                let mut best_distance = f32::INFINITY;
                for base in rows {
                    for column in columns {
                        let index = base + column;
                        let candidate = seeds
                            .get(usize::try_from(index).unwrap_or_default())
                            .copied()
                            .unwrap_or(UNCLAIMED);
                        if candidate == UNCLAIMED {
                            continue;
                        }
                        let distance = wrapped_distance(candidate, x, y, wrap);
                        if distance < best_distance {
                            best_distance = distance;
                            best = candidate;
                        }
                    }
                }
                put(line, usize::try_from(x).unwrap_or_default(), best);
            }
        }
    });
}

/// The distance in texels from `(x, y)` to a seed, the short way around the
/// wrap.
#[expect(
    clippy::cast_precision_loss,
    reason = "a squared distance over a plane of at most 4096 is exact in an f32"
)]
fn wrapped_distance(seed: [i32; 2], x: i64, y: i64, wrap: Wrap) -> f32 {
    let size = wrap.size();
    let axis = |from: i64, to: i64| {
        let delta = wrap.signed(to - from);
        // The way round that is shorter: half a period along is the same
        // distance either way, and taking it forwards is arbitrary and right.
        delta.min(size - delta)
    };
    let dx = axis(x, i64::from(seed[0]));
    let dy = axis(y, i64::from(seed[1]));
    ((dx * dx + dy * dy) as f32).sqrt()
}

/// The furthest apart two texels can be on a wrapped plane: half the diagonal.
#[expect(
    clippy::cast_precision_loss,
    reason = "a plane of at most 4096 texels a side"
)]
fn half_diagonal(size: i64) -> f32 {
    let half = (size / 2) as f32;
    (2.0 * half * half).sqrt()
}

/// A distance in texels as a distance in UV.
#[expect(
    clippy::cast_precision_loss,
    reason = "a plane of at most 4096 texels a side"
)]
fn to_uv(texels: f32, size: i64) -> f32 {
    texels / size.max(1) as f32
}

/// Run `body` over contiguous spans of rows of `out`, one thread each.
///
/// The filters go through this one: a filter reads a plane and writes a plane,
/// and there is nothing in that it can fail at.
pub(crate) fn rows_across_threads<T: Send>(
    out: &mut [T],
    resolution: u32,
    stride: usize,
    threads: Option<NonZeroUsize>,
    body: impl Fn(u32, &mut [T]) + Sync,
) {
    let filled = try_rows_across_threads(out, resolution, stride, threads, |first, chunk| {
        body(first, chunk);
        Ok(())
    });
    debug_assert!(filled.is_ok(), "a filter pass cannot fail");
}

/// The same, for a pass that can fail: rasterising an expression, which can
/// meet a plane that is not there.
///
/// `stride` is how many elements make a row, so a span is always a whole number
/// of rows and no two threads share one. The first error any span answers is
/// the one returned; a panic is carried to the caller with its own message,
/// because it is a bug in this crate rather than anything a graph can cause.
pub(crate) fn try_rows_across_threads<T: Send>(
    out: &mut [T],
    resolution: u32,
    stride: usize,
    threads: Option<NonZeroUsize>,
    body: impl Fn(u32, &mut [T]) -> Result<(), EvalError> + Sync,
) -> Result<(), EvalError> {
    let mut rest = out;
    let mut jobs = Vec::new();
    for (first, rows) in spans(resolution, threads) {
        jobs.push((first, take(&mut rest, rows as usize * stride)));
    }
    run_jobs(jobs, |(first, chunk)| body(first, chunk))
}

/// Run `body` once per job, each on a scoped thread of its own, and answer the
/// first error any of them answered.
///
/// A single job runs inline on the calling thread instead. That is the whole
/// bake on a target with no threads to spawn — `wasm32-unknown-unknown`, where
/// `std::thread::scope` panics and [`spans`] asks for one span because the
/// core count is unknown — and it is the same arithmetic on the same slice
/// either way, so the bytes do not change. A panic in a worker is carried to
/// the caller with its own message, because it is a bug in this crate rather
/// than anything a graph can cause.
pub(crate) fn run_jobs<J: Send, E: Send>(
    jobs: Vec<J>,
    body: impl Fn(J) -> Result<(), E> + Sync,
) -> Result<(), E> {
    if jobs.len() <= 1 {
        return jobs.into_iter().try_for_each(body);
    }
    std::thread::scope(|scope| {
        let mut handles = Vec::with_capacity(jobs.len());
        let body = &body;
        for job in jobs {
            handles.push(scope.spawn(move || body(job)));
        }
        let mut first = Ok(());
        for handle in handles {
            match handle.join() {
                Ok(Ok(())) => {}
                Ok(Err(error)) => {
                    if first.is_ok() {
                        first = Err(error);
                    }
                }
                Err(payload) => std::panic::resume_unwind(payload),
            }
        }
        first
    })
}

/// One value of a span, or nothing where the index is past its end. A span is
/// always exactly as long as the rows it owns, so the fallback never fires; it
/// is what keeps the inner loops free of a panic path.
pub(crate) fn put<T>(span: &mut [T], index: usize, value: T) {
    if let Some(slot) = span.get_mut(index) {
        *slot = value;
    }
}

/// The front `count` elements, leaving the rest behind for the next span. This
/// is what hands each thread a disjoint slice of a plane.
#[expect(
    clippy::mut_mut,
    reason = "a cursor over a slice being split is exactly a &mut to a &mut slice"
)]
pub(crate) fn take<'a, T>(rest: &mut &'a mut [T], count: usize) -> &'a mut [T] {
    let whole = std::mem::take(rest);
    let count = count.min(whole.len());
    let (head, tail) = whole.split_at_mut(count);
    *rest = tail;
    head
}

/// How many texels a square plane of this resolution holds.
pub(crate) fn texels(resolution: u32) -> usize {
    resolution as usize * resolution as usize
}

/// The rows each thread takes: one contiguous span each, as many spans as the
/// request asked for, never more spans than rows.
///
/// A request that named no count gets one thread per core, bounded so that no
/// span falls under [`MIN_ROWS_PER_THREAD`]; a request that named one gets what
/// it named, because that is what makes the division of the rows something a
/// test can vary.
///
/// Contiguous rather than interleaved because every plane is written in place
/// and a span is one slice of it; and because a texel costs the same wherever
/// it is, so there is nothing to balance.
pub(crate) fn spans(resolution: u32, budget: Option<NonZeroUsize>) -> Vec<(u32, u32)> {
    let rows = resolution.max(1);
    let asked = if let Some(threads) = budget {
        // `u32::MAX` rather than one where a count does not fit a `u32`: the
        // clamp below is what turns any large count into one span per row.
        u32::try_from(threads.get()).unwrap_or(u32::MAX)
    } else {
        // One per core, and never so many that a span falls under the minimum.
        let cores = std::thread::available_parallelism().map_or(1, NonZeroUsize::get);
        u32::try_from(cores)
            .unwrap_or(u32::MAX)
            .min((rows / MIN_ROWS_PER_THREAD).max(1))
    };
    let threads = asked.clamp(1, rows);
    let per = rows.div_ceil(threads);
    let mut spans = Vec::new();
    let mut first = 0;
    while first < rows {
        let count = per.min(rows - first);
        spans.push((first, count));
        first += count;
    }
    spans
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::nodes::SlopeMode;

    /// A plane of pseudo-random lanes in `0..1`.
    ///
    /// Random rather than a ramp or a step, because what the digests below pin
    /// is *arithmetic order*: a field whose neighbours are close leaves a
    /// reassociated sum looking right to the last bits, and one whose
    /// neighbours are unrelated does not.
    #[expect(
        clippy::cast_precision_loss,
        reason = "twenty-four bits of the word, which is what an f32 counts exactly"
    )]
    fn noise(resolution: u32, value_type: IrType, seed: u32) -> Plane {
        let mut state = seed | 1;
        let lanes = (0..texels(resolution) * value_type.components())
            .map(|_| {
                state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                (state >> 8) as f32 / 16_777_216.0
            })
            .collect();
        Plane::new(resolution, value_type, lanes)
    }

    /// FNV-1a over the bits of every lane, in order.
    ///
    /// The bits rather than the numbers: two planes that differ by one unit in
    /// the last place of one texel are two different planes here, which is the
    /// whole point of pinning a filter this way.
    fn digest(plane: &Plane) -> u64 {
        let mut hash = 0xcbf2_9ce4_8422_2325_u64;
        for lane in plane.lanes() {
            for byte in lane.to_bits().to_le_bytes() {
                hash ^= u64::from(byte);
                hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
            }
        }
        hash
    }

    /// Every filter, over a plane of noise, against the bytes it answered
    /// before phase 4 of the bake performance plan touched it.
    ///
    /// The digests were taken at `bd6c198` — the phase-3 commit, the last one
    /// before the sampler was hoisted out of the filter loops, the floor
    /// stopped going through `libm` and the jump flood stopped wrapping per
    /// neighbour. A filter here is not allowed to answer anything else: the
    /// shipped assets and the WGSL conformance both rest on these being the
    /// same `f32` operations in the same order.
    ///
    /// Both kinds of plane are covered, because they take different paths:
    /// a power of two wraps by mask and anything else by remainder.
    ///
    /// All three arities are covered too, and `Vec2` is the one that earns its
    /// place: `slope` and `directional` accumulate over `take(components)`, so
    /// `Vec2` is the only width where some lanes of the accumulator are the
    /// plane's and the rest are the zeroes a sample leaves behind.
    #[expect(
        clippy::too_many_lines,
        reason = "one line per filter, and what it is for is that every filter is here"
    )]
    #[test]
    fn every_filter_answers_the_bytes_it_answered_before_phase_four() {
        let threads = NonZeroUsize::new(3);
        let height = noise(64, IrType::Float, 1);
        let guide = noise(64, IrType::Float, 2);
        let colour = noise(64, IrType::Vec3, 3);
        let odd = noise(40, IrType::Float, 4);
        let odd_guide = noise(40, IrType::Float, 5);
        let odd_colour = noise(40, IrType::Vec3, 6);
        let pair = noise(64, IrType::Vec2, 7);
        let odd_pair = noise(40, IrType::Vec2, 8);
        let cases: [(&str, Plane, u64); 26] = [
            (
                "blur float",
                blur(&height, 0.05, threads),
                0xbf42_67a1_9d1b_049b,
            ),
            (
                "blur vec3",
                blur(&colour, 0.05, threads),
                0xbf06_317c_5a2a_bd8b,
            ),
            ("blur odd", blur(&odd, 0.05, threads), 0x50b7_c896_3190_1cd7),
            (
                "blur wide",
                blur(&height, 0.4, threads),
                0x3b06_2e53_9079_d57d,
            ),
            (
                "directional float",
                directional(&height, 0.15, 0.7, threads),
                0x58c9_7e81_dc61_320a,
            ),
            (
                "directional vec3",
                directional(&colour, 0.15, 0.7, threads),
                0x17b9_8474_041e_a824,
            ),
            (
                "directional odd",
                directional(&odd, 0.15, -2.1, threads),
                0xc239_c48f_619d_0ea2,
            ),
            (
                "directional vec2",
                directional(&pair, 0.15, 0.7, threads),
                0x4415_3143_6f7c_eb58,
            ),
            (
                "slope average",
                slope(&height, &guide, 0.06, 8, SlopeMode::Average, threads),
                0x332d_380f_7818_4e14,
            ),
            (
                "slope min vec3",
                slope(&colour, &guide, 0.06, 8, SlopeMode::Min, threads),
                0x7dd2_f2e1_6020_3fb3,
            ),
            (
                "slope max",
                slope(&height, &guide, 0.06, 8, SlopeMode::Max, threads),
                0x821b_47e2_08a0_6043,
            ),
            (
                "slope odd",
                slope(&odd, &odd_guide, 0.06, 8, SlopeMode::Average, threads),
                0xeb1e_7bcd_5779_0840,
            ),
            (
                "slope across resolutions",
                slope(&height, &odd_guide, 0.06, 8, SlopeMode::Min, threads),
                0x7e10_a24f_94c3_3f97,
            ),
            (
                "slope average vec2",
                slope(&pair, &guide, 0.06, 8, SlopeMode::Average, threads),
                0x6a89_7323_f289_e431,
            ),
            (
                "slope min vec2",
                slope(&pair, &guide, 0.06, 8, SlopeMode::Min, threads),
                0xbca5_b8c8_170e_48cb,
            ),
            (
                "slope max vec2 odd",
                slope(&odd_pair, &odd_guide, 0.06, 8, SlopeMode::Max, threads),
                0x0d74_bac5_a764_7572,
            ),
            (
                "occlusion",
                occlusion(&height, 0.12, 1.0, threads),
                0x6c14_ab0e_64bd_352c,
            ),
            (
                "occlusion odd",
                occlusion(&odd, 0.12, 0.6, threads),
                0x4c90_26e7_2c0e_bbef,
            ),
            (
                "distance",
                distance(&height, 0.5, threads),
                0xd039_8d95_1ddc_e250,
            ),
            (
                "distance odd",
                distance(&odd, 0.5, threads),
                0x0fff_c726_5913_543d,
            ),
            (
                "erode",
                morphology(&height, 0.05, Extreme::Low, threads),
                0x5886_a7e3_af89_0392,
            ),
            (
                "dilate vec3",
                morphology(&colour, 0.05, Extreme::High, threads),
                0x3d9c_eb46_1d9a_f068,
            ),
            (
                "curvature laplacian",
                curvature(&height, 0.05, CurvatureKind::Laplacian, threads),
                0xb27c_2d8e_75c9_9a6b,
            ),
            (
                "curvature blurred",
                curvature(&height, 0.05, CurvatureKind::Blurred, threads),
                0x3378_75dd_1a3f_7e8b,
            ),
            (
                "edge",
                edge(&odd_colour, 0.05, threads),
                0x2b4d_016d_b16f_bdc7,
            ),
            (
                "normal",
                normal_from_height(&height, 2.0, threads),
                0x67de_4c80_403e_4217,
            ),
        ];
        let mut wrong = Vec::new();
        for (name, plane, expected) in &cases {
            let found = digest(plane);
            if found != *expected {
                wrong.push(format!("{name}: {found:#018x} against {expected:#018x}"));
            }
        }
        assert!(wrong.is_empty(), "{}", wrong.join("\n"));
    }
}
