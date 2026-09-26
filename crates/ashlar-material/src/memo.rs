//! What one stage of a bake already computed, and the next one reads back.
//!
//! A bake is a sequence of *stages*: plane 0, plane 1, …, plane n−1 — each a
//! rasterisation of the plan's own sub-expression and then its filter — and
//! last the outputs. Every stage at the bake's own resolution walks the same
//! texel grid with the same UVs, the same planes under it and the same
//! expression, so a value one stage computed at a texel is, bit for bit, the
//! value a later stage would compute there. Computing it twice is the largest
//! redundancy in a bake: the SOI cobblestone's stages between them evaluate
//! about 17,800 instructions per texel out of an expression of 4,196, because
//! the height that the occlusion plane rasterises is rasterised again by the
//! curvature plane, again by the `height` output, and again inside two more
//! planes and three more outputs.
//!
//! This module is the bookkeeping that cuts it to one. It is pure functions
//! over an [`Ir`] — nothing here rasterises anything — and the rasterisers in
//! [`planes`](crate::planes) and [`bake`](crate::bake) act on what it says.
//!
//! # The frontier
//!
//! A stage does not need every value an earlier stage computed: it needs the
//! ones its own walk *reaches*, and only where the walk has not already been
//! stopped above them. So the walk is top-down from the stage's roots, and it
//! stops at any value an earlier stage has: that value is read out of a plane
//! rather than evaluated, and everything below it is not visited at all. The
//! values the walk stops at are the stage's [`frontier`], and they are the
//! only ones a plane has to be kept for. Everything else an earlier stage
//! computed is below somebody's frontier and costs nothing to leave behind.
//!
//! # What is not memoised
//!
//! A value whose instruction reads no operand — [`Op::Const`](crate::ir::Op),
//! [`Op::Uv`](crate::ir::Op) and the other sources — has nothing under it to
//! save. Keeping a plane of it would cost up to forty-eight megabytes at 2048
//! to spare one register write per texel, and a constant is not even that:
//! [`Interpreter`](crate::interp::Interpreter) writes the constants into the
//! register file once per bake and never walks them. So the sources are the
//! one thing every stage re-derives, and [`MemoPlan::evaluations`] is
//! everything else, each counted once.
//!
//! A plan that pinned its own `resolution` is a stage on a different grid: its
//! texel `k` is not the bake's texel `k`, so it neither reads nor provides
//! anything here and evaluates its closure from scratch, exactly as it did
//! before.
//!
//! # Availability is a runtime fact
//!
//! [`MemoPlan`] says what *could* be read and what therefore has to be
//! written. What is actually there is [`MemoTable`], because a stage whose
//! filtered plane came out of a [`BakeCache`](crate::planes::BakeCache) never
//! ran and so left nothing behind. Every stage takes the frontier of what the
//! table actually holds ([`MemoTable::frontier`]) and evaluates the rest. The
//! bytes are the same either way — a value read back is the value that would
//! have been recomputed — and only the time changes.

use crate::interp::Plane;
use crate::ir::{Ir, IrType, ValueId};

/// The output ports a bake rasterises, in the order
/// [`bake`](crate::bake) names them.
///
/// The analysis needs the outputs stage's roots before anything is rasterised.
/// A port this list and the rasteriser disagree about costs a plane that
/// nobody reads or a value evaluated twice, never a wrong texel: what a stage
/// actually reads back is decided at run time by [`MemoTable::frontier`] over
/// the roots the rasteriser itself passes.
pub const OUTPUTS: [&str; 6] = [
    "base_color",
    "roughness",
    "metallic",
    "occlusion",
    "height",
    "emissive",
];

/// One stage of a bake: what it evaluates, and on which grid.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Stage {
    /// The values it rasterises. One for a plane, up to six for the outputs.
    pub roots: Vec<ValueId>,
    /// Texels per side. A stage whose resolution is not the bake's shares no
    /// texel with any other stage.
    pub resolution: u32,
}

/// The stages a bake runs, in order: one per [`Ir::buffers`] entry, then the
/// outputs.
pub fn stages(ir: &Ir, resolution: u32) -> Vec<Stage> {
    let mut stages: Vec<Stage> = ir
        .buffers()
        .iter()
        .map(|plan| Stage {
            roots: vec![plan.root],
            resolution: plan.resolution.unwrap_or(resolution).max(1),
        })
        .collect();
    stages.push(Stage {
        roots: OUTPUTS.iter().filter_map(|port| ir.root(port)).collect(),
        resolution: resolution.max(1),
    });
    stages
}

/// Which instructions `roots` reach when the walk stops at `stop`.
///
/// [`Ir::reaches`] with a floor: a value `stop` names is marked, and its
/// operands are not visited through it. That is what makes a memoised
/// rasterisation cheap — the closure below a value that is already a plane is
/// never walked — and it is why the walk is top-down rather than the backwards
/// sweep [`Ir::reaches`] can afford.
///
/// A value below the frontier that some *other*, unmemoised path also reaches
/// is still marked, because that path walks into it. Reachability is therefore
/// exactly "what this stage has to evaluate, plus what it reads back".
pub fn reaches_above(ir: &Ir, roots: &[ValueId], stop: &[bool]) -> Vec<bool> {
    let mut live = vec![false; ir.len()];
    let mut stack: Vec<ValueId> = roots.to_vec();
    while let Some(value) = stack.pop() {
        match live.get_mut(value.index()) {
            Some(flag @ false) => *flag = true,
            // Already visited, or an id from another expression. Either way
            // there is nothing under it left to walk.
            _ => continue,
        }
        if stop.get(value.index()).copied() == Some(true) {
            continue;
        }
        let Some(inst) = ir.inst(value) else {
            continue;
        };
        stack.extend_from_slice(inst.operands.as_slice());
    }
    live
}

/// The values a stage reads back rather than evaluates: the ones `available`
/// names that its walk actually reaches.
///
/// Ascending, so the answer is the same list however the walk happened to
/// order its stack, which is what makes a rasterisation reproducible.
pub fn frontier(ir: &Ir, roots: &[ValueId], available: &[bool]) -> Vec<ValueId> {
    let live = reaches_above(ir, roots, available);
    live.iter()
        .enumerate()
        .filter(|(index, reached)| **reached && available.get(*index).copied() == Some(true))
        .map(|(index, _)| ValueId::at(index))
        .collect()
}

/// Whether a value is worth a plane of its own.
fn memoisable(ir: &Ir, index: usize) -> bool {
    ir.insts()
        .get(index)
        .is_some_and(|inst| !inst.operands.is_empty())
}

/// How many instructions each value reaches, itself included.
///
/// One forward sweep over the arena with a bitset per value: an operand is
/// always earlier than the instruction that reads it, so a value's reach is
/// its own bit and the union of its operands'. That is
/// `len * len / 64` words of work and a couple of megabytes at the size of the
/// largest graph in the study, against the megatexel a single plane costs.
pub fn reach_counts(ir: &Ir) -> Vec<u32> {
    let words = ir.len().div_ceil(64).max(1);
    let mut bits = vec![0_u64; words * ir.len()];
    let mut counts = vec![0_u32; ir.len()];
    for (index, inst) in ir.insts().iter().enumerate() {
        let (earlier, rest) = bits.split_at_mut(index * words);
        let Some(row) = rest.get_mut(..words) else {
            continue;
        };
        if let Some(word) = row.get_mut(index / 64) {
            *word |= 1 << (index % 64);
        }
        for operand in inst.operands.as_slice() {
            let at = operand.index() * words;
            let Some(source) = earlier.get(at..at + words) else {
                continue;
            };
            for (slot, word) in row.iter_mut().zip(source) {
                *slot |= *word;
            }
        }
        counts[index] = row.iter().map(|word| word.count_ones()).sum();
    }
    counts
}

/// What one stage reads, writes and lets go of.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct StageMemo {
    /// The values it reads back from planes an earlier stage left: its
    /// frontier, if every earlier stage ran.
    pub reads: Vec<ValueId>,
    /// The values it must keep a plane of, because a later stage's frontier
    /// names them.
    pub writes: Vec<ValueId>,
    /// The values whose planes may be dropped once it is done: the ones it is
    /// the last stage to read.
    pub drops: Vec<ValueId>,
    /// How many instructions it evaluates per texel, sources excluded.
    pub evaluated: usize,
}

/// The whole bake's memoisation, stage by stage.
///
/// Computed from the [`Ir`] alone, before a texel is rasterised.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MemoPlan {
    stages: Vec<StageMemo>,
    sources: usize,
    peak_lanes: usize,
    min_reach: u32,
}

/// How many bytes of memo planes a bake may hold at once.
///
/// **The one thing that has to be bounded here.** A frontier is not the
/// handful of planes it looks like from a diagram: the SOI cobblestone's
/// stages read 158 distinct values back from earlier stages, and keeping every
/// one of them would be 4.5 GB at 2048 — the bake's own twenty-four planes are
/// about 400 MB, so that is an order of magnitude more memory to save the last
/// few per cent of the interpreter's work.
///
/// **What the number has to be is a fact about the graphs, and they grew.** At
/// 256 MiB the ladder below now falls all the way to a `min_reach` of 256 on
/// the SOI cobblestone and 512 on the brick, which is most of the memoisation
/// thrown away: measured over the stages, the SOI bake evaluates 10,806
/// instructions a texel against an expression of 7,785 and the brick 6,341
/// against 4,886 — thirty to forty per cent of the interpreter's work spent
/// recomputing what an earlier stage already had, where phase two of the
/// performance plan measured that cost at a few per cent on the graphs of the
/// day. One gibibyte buys the `min_reach` 16 rung on every preset in the
/// study — 8,254 and 5,002 instructions a texel — for a table that peaks at
/// 856 MB beside planes that are already 400 MB, on a machine that bakes at
/// 2048 with tens of gigabytes free.
pub const MEMO_BUDGET: usize = 1024 << 20;

/// The reaches the budget search tries, in order, and the rule that decides
/// which values are worth a plane.
///
/// A value that reaches four instructions saves four instructions a texel and
/// costs the same sixteen megabytes as one that saves two thousand, so when
/// the table will not fit, the small ones are what goes. Walking a ladder
/// rather than sorting by saving-per-byte is deliberate: each rung is a whole
/// re-analysis, because dropping a value moves every frontier above it, and a
/// rung is a rule that can be stated — "a plane is kept for a value that
/// reaches at least this much" — rather than a ranking that has to be read off
/// a table to be understood.
const REACHES: [u32; 10] = [0, 4, 8, 16, 32, 64, 128, 256, 512, u32::MAX];

impl MemoPlan {
    /// Work out what every stage reads and what has to be kept for it, within
    /// [`MEMO_BUDGET`].
    pub fn analyse(ir: &Ir, resolution: u32) -> Self {
        Self::within(ir, resolution, MEMO_BUDGET)
    }

    /// The same, under a budget of this many bytes.
    ///
    /// The coarsest rung is "keep nothing", so there is always an answer: a
    /// budget under one plane is a bake that memoises nothing and costs what
    /// it did before.
    pub fn within(ir: &Ir, resolution: u32, budget: usize) -> Self {
        let reach = reach_counts(ir);
        let texel = (resolution.max(1) as usize).pow(2) * size_of::<f32>();
        let mut plan = Self::default();
        for min_reach in REACHES {
            plan = Self::with_reach(ir, resolution, &reach, min_reach);
            if plan.peak_lanes.saturating_mul(texel) <= budget {
                break;
            }
        }
        plan
    }

    /// The analysis proper: what every stage reads, writes and drops, keeping
    /// a plane only for a value that reaches at least `min_reach`
    /// instructions.
    ///
    /// One forward pass over the stages, carrying the values the stages before
    /// have computed: a stage's frontier is what it reaches of those, and
    /// everything it evaluates itself becomes available to the stages after
    /// it. A second pass turns the frontiers around — a value read by a later
    /// stage is a value the stage that first computed it has to write, and the
    /// last stage to read it is where its plane is dropped.
    pub fn analyse_with(ir: &Ir, resolution: u32, min_reach: u32) -> Self {
        Self::with_reach(ir, resolution, &reach_counts(ir), min_reach)
    }

    fn with_reach(ir: &Ir, resolution: u32, reach: &[u32], min_reach: u32) -> Self {
        let keep = |index: usize| {
            memoisable(ir, index) && reach.get(index).copied().unwrap_or(0) >= min_reach
        };
        let stages = stages(ir, resolution);
        // Where each value was first computed, and — the same thing seen from
        // a stage about to run — which values it may read rather than
        // evaluate.
        let mut produced: Vec<Option<usize>> = vec![None; ir.len()];
        let mut available = vec![false; ir.len()];
        let mut memo: Vec<StageMemo> = Vec::with_capacity(stages.len());
        for (at, stage) in stages.iter().enumerate() {
            if stage.resolution != resolution.max(1) {
                // A grid of its own: nothing here is about its texels.
                let live = ir.reaches(&stage.roots);
                memo.push(StageMemo {
                    evaluated: count_evaluated(ir, &live, &[]),
                    ..StageMemo::default()
                });
                continue;
            }
            let live = reaches_above(ir, &stage.roots, &available);
            let reads = frontier(ir, &stage.roots, &available);
            memo.push(StageMemo {
                evaluated: count_evaluated(ir, &live, &available),
                reads,
                ..StageMemo::default()
            });
            for (index, reached) in live.iter().enumerate() {
                if *reached && available.get(index).copied() != Some(true) && keep(index) {
                    produced[index] = Some(at);
                }
            }
            for (index, slot) in available.iter_mut().enumerate() {
                *slot = produced.get(index).copied().flatten().is_some();
            }
        }
        let mut last_read: Vec<Option<usize>> = vec![None; ir.len()];
        for (at, stage) in memo.iter().enumerate() {
            for value in &stage.reads {
                last_read[value.index()] = Some(at);
            }
        }
        for (index, read_at) in last_read.iter().enumerate() {
            let (Some(read_at), Some(written_at)) =
                (*read_at, produced.get(index).copied().flatten())
            else {
                continue;
            };
            if let Some(stage) = memo.get_mut(written_at) {
                stage.writes.push(ValueId::at(index));
            }
            if let Some(stage) = memo.get_mut(read_at) {
                stage.drops.push(ValueId::at(index));
            }
        }
        let lanes = |value: &ValueId| ir.type_of(*value).map_or(0, IrType::components);
        let mut live = 0_usize;
        let mut peak_lanes = 0_usize;
        for stage in &memo {
            live += stage.writes.iter().map(lanes).sum::<usize>();
            peak_lanes = peak_lanes.max(live);
            live = live.saturating_sub(stage.drops.iter().map(lanes).sum::<usize>());
        }
        Self {
            stages: memo,
            sources: ir
                .insts()
                .iter()
                .filter(|inst| inst.operands.is_empty())
                .count(),
            peak_lanes,
            min_reach,
        }
    }

    /// One stage's plan, by its position in [`stages`].
    pub fn stage(&self, at: usize) -> Option<&StageMemo> {
        self.stages.get(at)
    }

    /// Every stage's plan, in order.
    pub fn stages(&self) -> &[StageMemo] {
        &self.stages
    }

    /// How many instructions the whole bake evaluates per texel, counting the
    /// sources no stage memoises once rather than once per stage.
    ///
    /// With nothing dropped for the budget this is exactly [`Ir::len`] — every
    /// value with an operand under it computed once and read back after — and
    /// that equality is what the tests pin. Before memoisation it was the sum
    /// of the stages' closures, which for the SOI cobblestone is 13,932
    /// against an expression of 4,196.
    pub fn evaluations(&self) -> usize {
        self.stages
            .iter()
            .map(|stage| stage.evaluated)
            .sum::<usize>()
            + self.sources
    }

    /// How many planes are held at once at the worst point of the bake.
    pub fn peak_planes(&self) -> usize {
        let mut live = 0_usize;
        let mut peak = 0_usize;
        for stage in &self.stages {
            live += stage.writes.len();
            peak = peak.max(live);
            live = live.saturating_sub(stage.drops.len());
        }
        peak
    }

    /// How many `f32`s per texel the table holds at that worst point, which
    /// times the texel count is what it costs in bytes.
    pub fn peak_lanes(&self) -> usize {
        self.peak_lanes
    }

    /// Which values a stage *before* the outputs reads back, as a flag per
    /// value.
    ///
    /// The outputs are the last stage, so a value no earlier stage names is
    /// one whose only reader is them. A caller that rasterises the outputs
    /// wants all of it; one that drops the table on the way out —
    /// [`rasterise_buffers`](crate::planes::rasterise_buffers),
    /// [`rasterise_wanted`](crate::planes::rasterise_wanted), and so the CPU
    /// fallback in `ashlar-bevy` — would otherwise allocate and fill a plane
    /// of every one of those and free it unread.
    pub fn read_before_outputs(&self, len: usize) -> Vec<bool> {
        let mut wanted = vec![false; len];
        let planes = self.stages.len().saturating_sub(1);
        for stage in self.stages.iter().take(planes) {
            for value in &stage.reads {
                if let Some(slot) = wanted.get_mut(value.index()) {
                    *slot = true;
                }
            }
        }
        wanted
    }

    /// [`Self::peak_lanes`] for a bake that keeps only what
    /// [`Self::read_before_outputs`] names.
    pub fn peak_lanes_for_planes(&self, ir: &Ir) -> usize {
        let wanted = self.read_before_outputs(ir.len());
        let kept = |value: &&ValueId| wanted.get(value.index()).copied() == Some(true);
        let lanes = |value: &ValueId| ir.type_of(*value).map_or(0, IrType::components);
        let last = self.stages.len().saturating_sub(1);
        let (mut live, mut peak) = (0_usize, 0_usize);
        for (at, stage) in self.stages.iter().enumerate() {
            live += stage.writes.iter().filter(kept).map(lanes).sum::<usize>();
            peak = peak.max(live);
            // The outputs stage's drops never run: the pass this measures is
            // over by then, and the table goes with it.
            if at != last {
                live = live.saturating_sub(stage.drops.iter().filter(kept).map(lanes).sum());
            }
        }
        peak
    }

    /// The rung of the budget ladder this plan was made at: the fewest
    /// instructions a value reaches and is still worth a plane. Zero is
    /// "everything".
    pub fn min_reach(&self) -> u32 {
        self.min_reach
    }
}

/// How many instructions of a live set a stage actually evaluates: the ones it
/// does not read back, and not the sources, which [`MemoPlan::evaluations`]
/// counts once for the whole bake.
fn count_evaluated(ir: &Ir, live: &[bool], available: &[bool]) -> usize {
    live.iter()
        .enumerate()
        .filter(|(index, reached)| {
            **reached && available.get(*index).copied() != Some(true) && memoisable(ir, *index)
        })
        .count()
}

/// What the stages before this one left behind, by value.
///
/// One slot per instruction rather than a map: a bake looks a value up once
/// per stage and a few million times per texel through the interpreter, and
/// the table is a few dozen bytes per instruction with nothing in it.
#[derive(Clone, Debug, Default)]
pub struct MemoTable {
    planes: Vec<Option<Plane>>,
}

impl MemoTable {
    /// An empty table for an expression of `len` instructions.
    pub fn new(len: usize) -> Self {
        Self {
            planes: vec![None; len],
        }
    }

    /// Keep a value's plane for the stages that read it.
    pub fn insert(&mut self, value: ValueId, plane: Plane) {
        if let Some(slot) = self.planes.get_mut(value.index()) {
            *slot = Some(plane);
        }
    }

    /// Let a value's plane go, once the last stage that reads it is done.
    pub fn drop_value(&mut self, value: ValueId) {
        if let Some(slot) = self.planes.get_mut(value.index()) {
            *slot = None;
        }
    }

    /// One value's plane, if it is still held.
    pub fn get(&self, value: ValueId) -> Option<&Plane> {
        self.planes.get(value.index()).and_then(Option::as_ref)
    }

    /// How many planes are held.
    pub fn len(&self) -> usize {
        self.planes.iter().filter(|plane| plane.is_some()).count()
    }

    /// Whether nothing is held, which is a bake whose every stage ran from
    /// scratch.
    pub fn is_empty(&self) -> bool {
        self.planes.iter().all(Option::is_none)
    }

    /// What a stage over `roots` may read back rather than evaluate, as the
    /// interpreter wants it: the frontier of what is *actually* held, which is
    /// what makes a cached stage's absence cost time rather than correctness.
    pub fn frontier<'a>(&'a self, ir: &Ir, roots: &[ValueId]) -> Vec<(ValueId, &'a Plane)> {
        let available: Vec<bool> = self.planes.iter().map(Option::is_some).collect();
        frontier(ir, roots, &available)
            .into_iter()
            .filter_map(|value| Some((value, self.get(value)?)))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::{Filter, IrType, Lowering, Op, Target};

    /// A synthetic expression with the shape that matters: one chain of work,
    /// a plane over it, a second plane that reads both the chain and the first
    /// plane, and an output that reads the chain and the second plane. The SOI
    /// graph is this shape a hundred times over.
    ///
    /// Built through [`Lowering`] rather than by hand so that the arena it
    /// makes is one a bake could have been handed: dead code dropped, common
    /// sub-expressions merged, and the plans in dependency order.
    fn chained() -> (Ir, ValueId) {
        let mut low = Lowering::new(Target::Bake);
        let uv = low.uv();
        let mut chain = low.emit(Op::Extract(0), [uv]);
        for _ in 0..8 {
            chain = low.emit(Op::Sin, [chain]);
        }
        let first = low.buffer(chain, Filter::Blur { radius: 0.01 }, None);
        let sampled = low.sample(first, uv);
        let second_root = low.emit(Op::Add, [chain, sampled]);
        let second = low.buffer(second_root, Filter::Blur { radius: 0.02 }, None);
        let sampled_second = low.sample(second, uv);
        let height = low.emit(Op::Mul, [chain, sampled_second]);
        let ir = low
            .finish([("height".to_owned(), height)])
            .expect("the expression lowers");
        let root = ir.root("height").expect("the height root");
        (ir, root)
    }

    /// The value the chain ends at, which both planes and the output reach.
    fn chain_root(ir: &Ir) -> ValueId {
        ir.buffers()
            .first()
            .map(|plan| plan.root)
            .expect("the first plane")
    }

    #[test]
    fn a_walk_stops_at_a_value_that_is_already_a_plane() {
        let (ir, root) = chained();
        let chain = chain_root(&ir);
        let mut stop = vec![false; ir.len()];
        stop[chain.index()] = true;
        let live = reaches_above(&ir, &[root], &stop);
        assert!(live[chain.index()], "the memoised value is reached");
        // The chain is eight `Sin`s, and it is only under the memoised value,
        // so none of it is walked. The `Uv` under it still is, because the
        // samples read it too and that path was not memoised.
        for (index, inst) in ir.insts().iter().enumerate() {
            assert!(
                !(matches!(inst.op, Op::Sin) && live[index] && index != chain.index()),
                "v{index} is under the memoised value and must not be walked"
            );
        }
        assert_eq!(frontier(&ir, &[root], &stop), vec![chain]);
    }

    #[test]
    fn a_root_that_is_already_a_plane_is_read_rather_than_walked() {
        let (ir, _) = chained();
        let chain = chain_root(&ir);
        let mut stop = vec![false; ir.len()];
        stop[chain.index()] = true;
        assert_eq!(frontier(&ir, &[chain], &stop), vec![chain]);
    }

    #[test]
    fn every_instruction_of_a_chained_graph_is_evaluated_once() {
        let (ir, _) = chained();
        let plan = MemoPlan::analyse(&ir, 64);
        assert_eq!(
            plan.evaluations(),
            ir.len(),
            "every value once, the sources counted once for the bake"
        );
        assert_eq!(plan.stages().len(), ir.buffers().len() + 1);
        let chain = chain_root(&ir);
        assert_eq!(
            plan.stage(0).map(|stage| stage.writes.clone()),
            Some(vec![chain]),
            "the first plane keeps the chain for the stages that read it"
        );
        assert_eq!(
            plan.stage(1).map(|stage| stage.reads.clone()),
            Some(vec![chain])
        );
        assert_eq!(
            plan.stage(2).map(|stage| stage.reads.clone()),
            Some(vec![chain]),
            "and so do the outputs"
        );
        assert_eq!(
            plan.stage(2).map(|stage| stage.drops.clone()),
            Some(vec![chain]),
            "dropped after the last stage that reads it"
        );
        assert_eq!(plan.peak_planes(), 1);
    }

    #[test]
    fn a_source_is_never_kept_as_a_plane() {
        let (ir, _) = chained();
        let plan = MemoPlan::analyse(&ir, 64);
        for stage in plan.stages() {
            for value in &stage.writes {
                assert!(
                    memoisable(&ir, value.index()),
                    "{value} reads no operand and has nothing under it to save"
                );
            }
        }
    }

    #[test]
    fn a_plan_at_its_own_resolution_neither_reads_nor_writes() {
        let mut low = Lowering::new(Target::Bake);
        let uv = low.uv();
        let mut chain = low.emit(Op::Extract(0), [uv]);
        for _ in 0..4 {
            chain = low.emit(Op::Sin, [chain]);
        }
        let pinned = low.buffer(chain, Filter::Blur { radius: 0.01 }, Some(32));
        let sampled = low.sample(pinned, uv);
        let height = low.emit(Op::Add, [chain, sampled]);
        let ir = low
            .finish([("height".to_owned(), height)])
            .expect("the expression lowers");
        let plan = MemoPlan::analyse(&ir, 64);
        let pinned = plan.stage(0).expect("the pinned stage");
        assert!(pinned.reads.is_empty() && pinned.writes.is_empty());
        // The pinned stage evaluates its whole chain and the outputs evaluate
        // it again, so this graph is exactly the one memoisation cannot help.
        assert!(plan.evaluations() > ir.len());
    }

    #[test]
    fn a_table_frontier_is_what_the_table_actually_holds() {
        let (ir, root) = chained();
        let chain = chain_root(&ir);
        let mut table = MemoTable::new(ir.len());
        assert!(table.is_empty());
        assert!(table.frontier(&ir, &[root]).is_empty());
        table.insert(chain, Plane::new(2, IrType::Float, vec![0.0; 4]));
        assert_eq!(table.len(), 1);
        let held = table.frontier(&ir, &[root]);
        assert_eq!(held.len(), 1);
        assert_eq!(held.first().map(|(value, _)| *value), Some(chain));
        table.drop_value(chain);
        assert!(table.frontier(&ir, &[root]).is_empty());
    }
}
