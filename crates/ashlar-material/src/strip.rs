//! Instruction-outer evaluation: a strip of texels at a time, per instruction.
//!
//! [`Interpreter::run_at`](crate::interp::Interpreter::run_at) walks the whole
//! expression for one texel, so every instruction pays a match, three operand
//! reads and a register store to produce a single `f32`. At about nine
//! nanoseconds an instruction per texel that is what is left of a bake's time:
//! the cobblestone evaluates 2,852 instructions at each of four million texels.
//!
//! Turning the two loops inside out fixes it. A *strip* of [`STRIP`] texels is
//! evaluated per instruction, so each op arm is a loop over contiguous `f32`s
//! that the compiler vectorises, and the dispatch is paid once for sixty-four
//! texels rather than once for each. Nothing about the arithmetic changes: the
//! same `f32` operations happen in the same order on the same lanes, `sin`,
//! `cos`, `exp2`, `log2`, `powf`, `atan2` and `sqrt` are the same scalar `std`
//! calls per lane, and [`Op::Sample`] is the same bilinear read per texel. A
//! bake's bytes are the bytes they were, which the golden tests pin and which
//! this module's own tests pin op by op.
//!
//! # Lane-major, and why
//!
//! A slot holds [`STRIP`] values of lane zero, then [`STRIP`] of lane one,
//! then lane two — not a `[f32; 3]` per texel. Component-wise ops vectorise
//! either way, but lane-major is what makes the rest of them contiguous too:
//! [`Op::Extract`] and [`Op::Compose`] become a copy of one block,
//! [`Op::Dot`] and [`Op::Length`] become three products down three parallel
//! blocks rather than a horizontal sum per texel, and cutting a value to the
//! lanes its type owns is a block left untouched rather than two stores
//! skipped per texel.
//!
//! # Slots, not registers
//!
//! A register file the width of the expression would be `4196 × 3 × STRIP`
//! floats — three megabytes, which is not a working set. So a value gets a
//! *slot*, and a slot is handed back once the last instruction that reads it
//! has run: liveness over the walk order, which is one forward pass because an
//! operand is always earlier than its reader. What is left is the peak of
//! simultaneously live values, which on these graphs is tens rather than
//! thousands. A constant and a memoised value are materialised at their first
//! reader rather than up front, for the same reason: otherwise every constant
//! in the expression would be live from the first texel.
//!
//! Slots are pooled *by width*, and that is load-bearing. A slot only ever
//! holds values of one [`IrType`](crate::ir::IrType) — apart from the memoised
//! loads, which take the widest class and write all three lanes — so the lanes
//! above that width are written once, when the file is created, and are zero
//! forever after. That is exactly what [`narrow`](crate::interp::narrow)
//! guarantees per texel, and it is why a `Float` op here writes one block
//! rather than three while [`Op::Length`] can still read all three lanes of a
//! `Float` operand and find the zeroes it expects.
//!
//! The values a rasteriser reads after a strip has run — a plane's root, the
//! bound outputs, and the values [`memo`](crate::memo) says to keep — are
//! *pinned*: their slots are never handed back.

use crate::interp::{EvalError, Inputs, Plane, cell, dot, hash2, hash3, plane, smoothstep};
use crate::ir::{Inst, Ir, MAX_OPERANDS, Op, ValueId};

/// How many texels one pass over an instruction covers.
///
/// Sixty-four is eight AVX vectors of a lane block, four cache lines of the
/// one block a `Float` slot keeps warm, and a divisor of every legal bake
/// resolution — [`MIN_RESOLUTION`](crate::bake::MIN_RESOLUTION) is 256 — so a
/// row is whole strips and the short tail below runs only for a plane pinned
/// to a resolution of its own. Measured against 32 on the three benchmark
/// presets it is the faster of the two, by three per cent on the two large
/// ones and inside the noise on the small one.
pub(crate) const STRIP: usize = 64;

/// How many `f32`s one slot occupies: three lanes of a strip.
const BLOCK: usize = 3 * STRIP;

/// One step of a strip evaluation.
///
/// The walk is flattened into this before the first texel, so nothing per
/// strip decides anything: the slots are already chosen and the operands are
/// already offsets into the file.
#[derive(Clone, Copy, Debug)]
enum Step<'a> {
    /// Write a constant across the strip. Once per strip rather than once per
    /// bake, because the slot it sits in is handed back after its last reader.
    Splat {
        /// The value, already cut to the lanes its type owns.
        value: [f32; 3],
        /// How many lanes to write.
        width: usize,
        /// Where in the file it goes.
        out: usize,
    },
    /// Read a value an earlier stage of the bake left in a plane, texel by
    /// texel. All three lanes, which is what the per-texel walk stores.
    Load {
        /// The plane the earlier stage wrote.
        plane: &'a Plane,
        /// Where in the file it goes.
        out: usize,
    },
    /// Evaluate one instruction over the whole strip.
    Eval(Call),
}

/// One instruction as the strip runs it.
#[derive(Clone, Copy, Debug)]
struct Call {
    /// What to compute.
    op: Op,
    /// The operand slots, as offsets into the file.
    args: [usize; MAX_OPERANDS],
    /// How many of them the op reads.
    arity: usize,
    /// How many lanes the result owns. The rest of the slot is already zero
    /// and is left alone.
    width: usize,
    /// Where the result goes.
    out: usize,
}

/// A flattened walk: what to run, in order, and where every value lives.
///
/// Built once per thread from an [`Interpreter`](crate::interp::Interpreter),
/// then run over strip after strip against a [`StripFile`] of its own.
#[derive(Clone, Debug)]
pub(crate) struct StripPlan<'a> {
    steps: Vec<Step<'a>>,
    slots: usize,
    /// The pinned values and the offset each one's slot sits at, so a
    /// rasteriser can read them back after a strip has run.
    pinned: Vec<(ValueId, usize)>,
    stamp: u64,
}

/// Where [`StripPlan::stamp`] comes from, so a file cannot be handed to a plan
/// that did not size it. A counter rather than anything derived from the
/// expression: all a stamp has to do is differ.
static STAMPS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

/// The slot file one thread evaluates into.
///
/// Zero when it is made, and the lanes a slot's width does not own are never
/// written, which is the invariant the whole layout rests on.
#[derive(Clone, Debug)]
pub(crate) struct StripFile {
    lanes: Vec<f32>,
    stamp: u64,
}

impl StripFile {
    /// One value's three lanes at one texel of the strip, as the per-texel
    /// register file would have held them.
    #[inline]
    pub(crate) fn value(&self, at: usize, texel: usize) -> [f32; 3] {
        let mut value = [0.0; 3];
        for (lane, slot) in value.iter_mut().enumerate() {
            *slot = self
                .lanes
                .get(at + lane * STRIP + texel)
                .copied()
                .unwrap_or_default();
        }
        value
    }
}

/// Where a value comes from, before a slot is chosen for it.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Source {
    /// Nothing in this walk defines it.
    Absent,
    /// A hoisted constant, by its position in the interpreter's list.
    Splat(usize),
    /// A memoised plane, by its position in the interpreter's list.
    Load(usize),
    /// An instruction of the walk, by its position in it.
    Eval,
}

/// Not a position, and not a slot. What "never read" and "has no slot" both
/// say.
const NEVER: usize = usize::MAX;

/// The slot every operand that has no slot of its own points at.
///
/// Nothing ever writes it, so it reads as the three zeroes an unwritten
/// register of the per-texel file holds. A validated expression never needs
/// it; what it buys is that a walk handed an id from somewhere else answers
/// zero rather than another value's lanes.
const ZERO_SLOT: usize = 0;

impl<'a> StripPlan<'a> {
    /// Flatten a walk into steps and slots.
    ///
    /// `order`, `constants` and `memo` are the interpreter's own three lists.
    /// `pinned` is what the caller will read back afterwards — a plane's root,
    /// the bound outputs, and the values a later stage wants kept — and those
    /// slots are never reused.
    #[expect(
        clippy::too_many_lines,
        reason = "the allocation pass is one walk with the liveness woven \
                  through it; cutting it in two would mean handing half a \
                  dozen parallel vectors across the seam"
    )]
    pub(crate) fn build(
        ir: &Ir,
        order: &[(usize, Inst)],
        constants: &[(usize, [f32; 3])],
        memo: &[(usize, &'a Plane)],
        pinned: &[ValueId],
    ) -> Self {
        let len = ir.len();
        let mut source = vec![Source::Absent; len];
        for (at, (index, _)) in constants.iter().enumerate() {
            if let Some(slot) = source.get_mut(*index) {
                *slot = Source::Splat(at);
            }
        }
        for (at, (index, _)) in memo.iter().enumerate() {
            if let Some(slot) = source.get_mut(*index) {
                *slot = Source::Load(at);
            }
        }
        for (index, _) in order {
            if let Some(slot) = source.get_mut(*index) {
                *slot = Source::Eval;
            }
        }

        // An operand is always earlier than the instruction that reads it, so
        // one forward pass over the walk is the whole liveness answer.
        let mut first_use = vec![NEVER; len];
        let mut last_use = vec![NEVER; len];
        for (at, (_, inst)) in order.iter().enumerate() {
            for operand in inst.operands.as_slice() {
                let index = operand.index();
                if let Some(slot) = first_use.get_mut(index)
                    && *slot == NEVER
                {
                    *slot = at;
                }
                if let Some(slot) = last_use.get_mut(index) {
                    *slot = at;
                }
            }
        }
        let mut is_pinned = vec![false; len];
        for value in pinned {
            if let Some(slot) = is_pinned.get_mut(value.index()) {
                *slot = true;
            }
        }

        // A constant or a memoised plane is materialised where it is first
        // read, not at the start: the expression holds hundreds of constants,
        // and holding every one of them live from texel zero is the working
        // set this layout exists to avoid. One that only the caller reads back
        // goes in at the front, because there is nowhere else.
        let positions = order.len().max(1);
        let mut materialise: Vec<Vec<usize>> = vec![Vec::new(); positions];
        for (index, from) in source.iter().enumerate() {
            if !matches!(from, Source::Splat(_) | Source::Load(_)) {
                continue;
            }
            let at = match first_use.get(index).copied() {
                Some(NEVER) | None if is_pinned.get(index).copied() == Some(true) => 0,
                // Read by nobody: a plane the walk never asks for would be a
                // texel read per strip for nothing.
                Some(NEVER) | None => continue,
                Some(at) => at,
            };
            if let Some(bucket) = materialise.get_mut(at) {
                bucket.push(index);
            }
        }

        let mut pool = Pool::default();
        // Slot zero is the one nothing writes; see `ZERO_SLOT`.
        let zero = pool.take(3);
        debug_assert_eq!(zero, ZERO_SLOT, "the zero slot is the first one taken");
        let mut at: Vec<usize> = vec![NEVER; len];
        let mut steps = Vec::with_capacity(order.len() + constants.len() + memo.len());
        let mut fixed: Vec<(ValueId, usize)> = Vec::with_capacity(pinned.len());

        for step in 0..positions {
            for index in materialise.get(step).map_or(&[][..], Vec::as_slice) {
                let index = *index;
                match source.get(index).copied() {
                    Some(Source::Splat(which)) => {
                        let Some((_, value)) = constants.get(which) else {
                            continue;
                        };
                        let width = ir
                            .type_of(ValueId::at(index))
                            .map_or(3, crate::ir::IrType::components);
                        let slot = pool.take(width);
                        place(&mut at, &is_pinned, &mut fixed, index, slot);
                        steps.push(Step::Splat {
                            value: *value,
                            width,
                            out: slot * BLOCK,
                        });
                    }
                    Some(Source::Load(which)) => {
                        let Some((_, plane)) = memo.get(which) else {
                            continue;
                        };
                        // A loaded value writes all three lanes whatever its
                        // type, because that is what the per-texel walk
                        // stores; so it takes a slot of the widest class.
                        let slot = pool.take(3);
                        place(&mut at, &is_pinned, &mut fixed, index, slot);
                        steps.push(Step::Load {
                            plane,
                            out: slot * BLOCK,
                        });
                    }
                    _ => {}
                }
            }
            let Some((index, inst)) = order.get(step) else {
                continue;
            };
            let (index, inst) = (*index, *inst);
            let mut args = [ZERO_SLOT * BLOCK; MAX_OPERANDS];
            for (slot, operand) in args.iter_mut().zip(inst.operands.as_slice()) {
                *slot = match at.get(operand.index()).copied() {
                    Some(NEVER) | None => ZERO_SLOT * BLOCK,
                    Some(found) => found * BLOCK,
                };
            }
            let width = inst.value_type.components();
            // The result's slot is taken before the operands' are handed back,
            // so it is never one of them — which is what lets the file be
            // split once and the operands read in place.
            let out = pool.take(width);
            place(&mut at, &is_pinned, &mut fixed, index, out);
            steps.push(Step::Eval(Call {
                op: inst.op,
                args,
                arity: inst.operands.len(),
                width,
                out: out * BLOCK,
            }));
            for operand in inst.operands.as_slice() {
                release(
                    &mut pool,
                    &mut at,
                    &is_pinned,
                    &last_use,
                    operand.index(),
                    step,
                );
            }
            release(&mut pool, &mut at, &is_pinned, &last_use, index, step);
        }

        Self {
            steps,
            slots: pool.slots.len(),
            pinned: fixed,
            stamp: STAMPS.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
        }
    }

    /// A file sized for this plan, with every lane zero.
    pub(crate) fn file(&self) -> StripFile {
        StripFile {
            lanes: vec![0.0; self.slots * BLOCK],
            stamp: self.stamp,
        }
    }

    /// Where a pinned value's slot sits, for reading it back after a strip.
    pub(crate) fn slot(&self, value: ValueId) -> Option<usize> {
        self.pinned
            .iter()
            .find(|(pinned, _)| *pinned == value)
            .map(|(_, at)| *at)
    }

    /// How many slots the walk needs at once: what the liveness pass bought,
    /// and what the tests look at.
    #[cfg(test)]
    pub(crate) fn slots(&self) -> usize {
        self.slots
    }

    /// How many values are read back out of a plane rather than evaluated.
    #[cfg(test)]
    pub(crate) fn loads(&self) -> usize {
        self.steps
            .iter()
            .filter(|step| matches!(step, Step::Load { .. }))
            .count()
    }

    /// Evaluate one strip of texels.
    ///
    /// `first` is the position of the strip's first texel in the row-major
    /// order of the grid being written, which is what the memoised planes are
    /// indexed by. `uvs` is one coordinate per texel, computed by the
    /// rasteriser exactly as it always computed it, and its length is the
    /// strip's — [`STRIP`], but for a short tail.
    pub(crate) fn run(
        &self,
        first: usize,
        uvs: &[[f32; 2]],
        inputs: &Inputs<'_>,
        file: &mut StripFile,
    ) -> Result<(), EvalError> {
        if file.stamp != self.stamp || file.lanes.len() != self.slots * BLOCK {
            file.lanes.clear();
            file.lanes.resize(self.slots * BLOCK, 0.0);
            file.stamp = self.stamp;
        }
        let uvs = uvs.get(..uvs.len().min(STRIP)).unwrap_or(uvs);
        let span = uvs.len();
        for step in &self.steps {
            match *step {
                Step::Splat { value, width, out } => {
                    splat(&mut file.lanes, out, width, span, value);
                }
                Step::Load { plane, out } => {
                    for texel in 0..span {
                        let value = plane.texel_at(first + texel);
                        for (lane, component) in value.into_iter().enumerate() {
                            if let Some(slot) = file.lanes.get_mut(out + lane * STRIP + texel) {
                                *slot = component;
                            }
                        }
                    }
                }
                Step::Eval(call) => eval(&mut file.lanes, call, span, uvs, inputs)?,
            }
        }
        Ok(())
    }
}

/// Give a value its slot, and remember where a pinned one landed.
fn place(
    at: &mut [usize],
    is_pinned: &[bool],
    fixed: &mut Vec<(ValueId, usize)>,
    index: usize,
    slot: usize,
) {
    if let Some(entry) = at.get_mut(index) {
        *entry = slot;
    }
    if is_pinned.get(index).copied() == Some(true) {
        fixed.push((ValueId::at(index), slot * BLOCK));
    }
}

/// Hand a value's slot back once the instruction at `step` was its last
/// reader.
///
/// A pinned value is never handed back, and a slot is released at most once
/// because releasing clears the value's entry — which matters for an
/// instruction that reads one value twice.
fn release(
    pool: &mut Pool,
    at: &mut [usize],
    is_pinned: &[bool],
    last_use: &[usize],
    index: usize,
    step: usize,
) {
    if is_pinned.get(index).copied() == Some(true) {
        return;
    }
    let done = match last_use.get(index).copied() {
        // Nothing reads it: its slot is free the moment it is written.
        Some(NEVER) => true,
        Some(last) => last == step,
        None => false,
    };
    let Some(slot) = at.get_mut(index) else {
        return;
    };
    if done && *slot != NEVER {
        pool.give(*slot);
        *slot = NEVER;
    }
}

/// The slots in play, pooled by the width of what they hold.
///
/// Separate free lists per width are what keeps the lanes above a value's type
/// zero: a `Float` slot only ever holds a `Float`, so lanes one and two of it
/// are the zeroes the file was born with and no op has to write them.
#[derive(Debug, Default)]
struct Pool {
    /// The width each slot was cut for, which is also how many there are.
    slots: Vec<usize>,
    /// Free slots by width: index one for a `Float`, two for a `Vec2`, three
    /// for a `Vec3`. Index zero is unused and costs a word.
    free: [Vec<usize>; 4],
}

impl Pool {
    fn take(&mut self, width: usize) -> usize {
        let width = width.clamp(1, 3);
        if let Some(slot) = self.free.get_mut(width).and_then(Vec::pop) {
            return slot;
        }
        self.slots.push(width);
        self.slots.len().saturating_sub(1)
    }

    fn give(&mut self, slot: usize) {
        let Some(width) = self.slots.get(slot).copied() else {
            return;
        };
        if let Some(free) = self.free.get_mut(width) {
            free.push(slot);
        }
    }
}

/// One operand, as the two halves of the file either side of the result.
///
/// The result's slot is taken before its operands are released, so no operand
/// ever shares it: an operand sits wholly below the result's block or wholly
/// above it, which is what lets the file be split once and the operands read
/// without copying.
#[derive(Clone, Copy)]
struct Reg<'f> {
    low: &'f [f32],
    high: &'f [f32],
    out: usize,
    base: usize,
}

impl<'f> Reg<'f> {
    /// One lane block of this operand, as far as the strip runs.
    ///
    /// The lanes above the operand's own width read zero, because a slot of
    /// that width never wrote them — which is exactly what the per-texel
    /// register file holds after [`narrow`](crate::interp::narrow).
    #[inline]
    fn lane(self, lane: usize, span: usize) -> &'f [f32] {
        let at = self.base + lane * STRIP;
        if self.base < self.out {
            self.low.get(at..at + span).unwrap_or(&[])
        } else {
            let at = at.saturating_sub(self.out + BLOCK);
            self.high.get(at..at + span).unwrap_or(&[])
        }
    }
}

/// One lane block of the result, as far as the strip runs.
#[inline]
fn out_lane(out: &mut [f32], lane: usize, span: usize) -> &mut [f32] {
    out.get_mut(lane * STRIP..lane * STRIP + span)
        .unwrap_or(&mut [])
}

/// Write one three-lane value across the strip, over the lanes the result's
/// type owns.
#[inline]
fn splat(lanes: &mut [f32], out: usize, width: usize, span: usize, value: [f32; 3]) {
    for lane in 0..width {
        let component = value.get(lane).copied().unwrap_or(0.0);
        if let Some(block) = lanes.get_mut(out + lane * STRIP..out + lane * STRIP + span) {
            block.fill(component);
        }
    }
}

/// Zero the lanes above the first, for an op whose answer is a scalar in a
/// register wider than one lane. The ordinary case — a `Float` result — writes
/// nothing here.
#[inline]
fn zero_above(out: &mut [f32], width: usize, span: usize) {
    for lane in 1..width {
        out_lane(out, lane, span).fill(0.0);
    }
}

/// Evaluate one instruction over the strip.
///
/// Every arm is the arm [`apply`](crate::interp::apply) has, lane by lane and
/// in the same order, with one difference that is not arithmetic: only the
/// lanes the result's type owns are written, because the rest of the slot
/// already holds the zero [`narrow`](crate::interp::narrow) would have put
/// there.
#[expect(
    clippy::too_many_lines,
    reason = "one arm per Op, which is the shape the per-texel `apply` has too; \
              splitting it would put half an instruction set in another function"
)]
fn eval(
    lanes: &mut [f32],
    call: Call,
    span: usize,
    uvs: &[[f32; 2]],
    inputs: &Inputs<'_>,
) -> Result<(), EvalError> {
    let Call {
        op,
        args,
        arity,
        width,
        out,
    } = call;
    let (low, rest) = lanes.split_at_mut(out.min(lanes.len()));
    let (block, high) = rest.split_at_mut(BLOCK.min(rest.len()));
    let (low, high): (&[f32], &[f32]) = (low, high);
    let reg = |which: usize| Reg {
        low,
        high,
        out,
        base: args.get(which).copied().unwrap_or(0),
    };
    let (a, b, c) = (reg(0), reg(1), reg(2));
    match op {
        Op::Uv => {
            for lane in 0..width {
                for (slot, uv) in out_lane(block, lane, span).iter_mut().zip(uvs) {
                    *slot = uv.get(lane).copied().unwrap_or(0.0);
                }
            }
        }
        Op::Param(index) => {
            let index = index as usize;
            let value = *inputs
                .params
                .get(index)
                .ok_or(EvalError::MissingParam(index, inputs.params.len()))?;
            splat(block, 0, width, span, value);
        }
        Op::Time => splat(block, 0, width, span, [inputs.time, 0.0, 0.0]),
        Op::WorldPos => splat(block, 0, width, span, inputs.world_pos),
        Op::WorldNormal => splat(block, 0, width, span, inputs.world_normal),
        Op::CutFlag => splat(block, 0, width, span, [inputs.cut_flag, 0.0, 0.0]),
        // The plane was rasterised before this expression ran, and reading it
        // bilinearly with wrap is what makes a buffered filter a field rather
        // than a grid.
        Op::Sample(buffer) => {
            let plane = plane(inputs, buffer)?;
            let (us, vs) = (a.lane(0, span), a.lane(1, span));
            let mut taps = [[0.0_f32; STRIP]; 3];
            for (texel, (u, v)) in us.iter().zip(vs).enumerate() {
                let value = plane.sample([*u, *v]);
                for (lane, component) in value.into_iter().enumerate() {
                    if let Some(slot) = taps.get_mut(lane).and_then(|tap| tap.get_mut(texel)) {
                        *slot = component;
                    }
                }
            }
            for lane in 0..width {
                let tap = taps.get(lane).map_or(&[][..], |tap| &tap[..]);
                for (slot, value) in out_lane(block, lane, span).iter_mut().zip(tap) {
                    *slot = *value;
                }
            }
        }
        // Hoisted into a `Splat` step, so this arm is only what keeps the
        // match total.
        Op::Const(value) => splat(block, 0, width, span, [value, 0.0, 0.0]),
        Op::Add => binary(block, width, span, a, b, |x, y| x + y),
        Op::Sub => binary(block, width, span, a, b, |x, y| x - y),
        Op::Mul => binary(block, width, span, a, b, |x, y| x * y),
        // Zero rather than an infinity: a division by a mask is ordinary in a
        // graph, and one poisoned texel spreads through every filter after it.
        Op::Div => binary(
            block,
            width,
            span,
            a,
            b,
            |x, y| {
                if y == 0.0 { 0.0 } else { x / y }
            },
        ),
        Op::Min => binary(block, width, span, a, b, f32::min),
        Op::Max => binary(block, width, span, a, b, f32::max),
        Op::Abs => unary(block, width, span, a, f32::abs),
        Op::Floor => unary(block, width, span, a, f32::floor),
        // The WGSL definition, which wraps a negative coordinate forward;
        // `f32::fract` truncates towards zero and would not.
        Op::Fract => unary(block, width, span, a, |x| x - x.floor()),
        Op::Sqrt => unary(block, width, span, a, f32::sqrt),
        Op::Pow => binary(block, width, span, a, b, f32::powf),
        Op::Exp2 => unary(block, width, span, a, f32::exp2),
        Op::Log2 => unary(block, width, span, a, f32::log2),
        Op::Sin => unary(block, width, span, a, f32::sin),
        Op::Cos => unary(block, width, span, a, f32::cos),
        Op::Atan2 => binary(block, width, span, a, b, f32::atan2),
        Op::Mix => ternary(block, width, span, a, b, c, |x, y, t| x + (y - x) * t),
        Op::Step => binary(
            block,
            width,
            span,
            a,
            b,
            |edge, x| {
                if x < edge { 0.0 } else { 1.0 }
            },
        ),
        Op::Smoothstep => ternary(block, width, span, a, b, c, smoothstep),
        // `f32::clamp` panics when the bounds cross; a graph may well write
        // bounds that cross, and a bake must not stop because it did.
        Op::Clamp => ternary(block, width, span, a, b, c, |x, low, high| {
            x.max(low).min(high)
        }),
        Op::Select => {
            let condition = a.lane(0, span);
            for lane in 0..width {
                let (on_true, on_false) = (b.lane(lane, span), c.lane(lane, span));
                for (slot, ((condition, on_true), on_false)) in out_lane(block, lane, span)
                    .iter_mut()
                    .zip(condition.iter().zip(on_true).zip(on_false))
                {
                    *slot = if *condition >= 0.5 {
                        *on_true
                    } else {
                        *on_false
                    };
                }
            }
        }
        Op::Hash2(seed) => {
            let (xs, ys) = (a.lane(0, span), a.lane(1, span));
            let (pxs, pys) = (b.lane(0, span), b.lane(1, span));
            for (slot, (((x, y), px), py)) in out_lane(block, 0, span)
                .iter_mut()
                .zip(xs.iter().zip(ys).zip(pxs).zip(pys))
            {
                *slot = hash2(cell(*x, *px), cell(*y, *py), seed);
            }
            zero_above(block, width, span);
        }
        Op::Hash3(seed) => {
            let coordinate = [a.lane(0, span), a.lane(1, span), a.lane(2, span)];
            let period = [b.lane(0, span), b.lane(1, span), b.lane(2, span)];
            for (texel, slot) in out_lane(block, 0, span).iter_mut().enumerate() {
                let read = |source: &[f32]| source.get(texel).copied().unwrap_or_default();
                *slot = hash3(
                    cell(read(coordinate[0]), read(period[0])),
                    cell(read(coordinate[1]), read(period[1])),
                    cell(read(coordinate[2]), read(period[2])),
                    seed,
                );
            }
            zero_above(block, width, span);
        }
        Op::Length => {
            let (xs, ys, zs) = (a.lane(0, span), a.lane(1, span), a.lane(2, span));
            for (slot, ((x, y), z)) in out_lane(block, 0, span)
                .iter_mut()
                .zip(xs.iter().zip(ys).zip(zs))
            {
                let value = [*x, *y, *z];
                *slot = dot(value, value).sqrt();
            }
            zero_above(block, width, span);
        }
        Op::Dot => {
            let left = [a.lane(0, span), a.lane(1, span), a.lane(2, span)];
            let right = [b.lane(0, span), b.lane(1, span), b.lane(2, span)];
            for (texel, slot) in out_lane(block, 0, span).iter_mut().enumerate() {
                let read = |source: &[f32]| source.get(texel).copied().unwrap_or_default();
                *slot = dot(
                    [read(left[0]), read(left[1]), read(left[2])],
                    [read(right[0]), read(right[1]), read(right[2])],
                );
            }
            zero_above(block, width, span);
        }
        Op::Normalize => {
            let (xs, ys, zs) = (a.lane(0, span), a.lane(1, span), a.lane(2, span));
            let mut lengths = [0.0_f32; STRIP];
            for (slot, ((x, y), z)) in lengths.iter_mut().zip(xs.iter().zip(ys).zip(zs)) {
                let value = [*x, *y, *z];
                *slot = dot(value, value).sqrt();
            }
            for lane in 0..width {
                let source = a.lane(lane, span);
                for (slot, (component, length)) in out_lane(block, lane, span)
                    .iter_mut()
                    .zip(source.iter().zip(&lengths))
                {
                    *slot = if *length == 0.0 {
                        0.0
                    } else {
                        *component / *length
                    };
                }
            }
        }
        Op::Compose => {
            for lane in 0..width {
                if lane < arity {
                    let source = reg(lane).lane(0, span);
                    for (slot, value) in out_lane(block, lane, span).iter_mut().zip(source) {
                        *slot = *value;
                    }
                } else {
                    out_lane(block, lane, span).fill(0.0);
                }
            }
        }
        Op::Extract(channel) => {
            let channel = usize::from(channel);
            if channel < 3 {
                let source = a.lane(channel, span);
                for (slot, value) in out_lane(block, 0, span).iter_mut().zip(source) {
                    *slot = *value;
                }
            } else {
                out_lane(block, 0, span).fill(0.0);
            }
            zero_above(block, width, span);
        }
    }
    Ok(())
}

/// One component-wise op of one operand, lane by lane down the strip.
#[inline]
fn unary(out: &mut [f32], width: usize, span: usize, a: Reg<'_>, each: impl Fn(f32) -> f32) {
    for lane in 0..width {
        let source = a.lane(lane, span);
        for (slot, x) in out_lane(out, lane, span).iter_mut().zip(source) {
            *slot = each(*x);
        }
    }
}

/// One component-wise op of two operands.
#[inline]
fn binary(
    out: &mut [f32],
    width: usize,
    span: usize,
    a: Reg<'_>,
    b: Reg<'_>,
    each: impl Fn(f32, f32) -> f32,
) {
    for lane in 0..width {
        let (left, right) = (a.lane(lane, span), b.lane(lane, span));
        for (slot, (x, y)) in out_lane(out, lane, span)
            .iter_mut()
            .zip(left.iter().zip(right))
        {
            *slot = each(*x, *y);
        }
    }
}

/// One component-wise op of three operands.
#[inline]
fn ternary(
    out: &mut [f32],
    width: usize,
    span: usize,
    a: Reg<'_>,
    b: Reg<'_>,
    c: Reg<'_>,
    each: impl Fn(f32, f32, f32) -> f32,
) {
    for lane in 0..width {
        let (left, middle, right) = (a.lane(lane, span), b.lane(lane, span), c.lane(lane, span));
        for (slot, ((x, y), z)) in out_lane(out, lane, span)
            .iter_mut()
            .zip(left.iter().zip(middle).zip(right))
        {
            *slot = each(*x, *y, *z);
        }
    }
}

#[cfg(test)]
mod tests {
    #![expect(
        clippy::many_single_char_names,
        reason = "the fixture names its operands the way the ops document \
                  them: a, b and c, over u and v"
    )]
    #![expect(
        clippy::cast_precision_loss,
        reason = "a texel coordinate is bounded by the resolution these tests \
                  evaluate at, well inside what an f32 counts exactly"
    )]
    use super::*;
    use crate::interp::Interpreter;
    use crate::ir::{Filter, IrType, Lowering, Target};

    /// Every value in the expression is read back, so nothing may share a slot
    /// and the comparison can be made instruction by instruction.
    fn pin_everything(ir: &Ir) -> Vec<ValueId> {
        (0..ir.len()).map(ValueId::at).collect()
    }

    /// An expression that emits every [`Op`] the interpreter answers for,
    /// built out of the texel coordinate so that nothing folds.
    ///
    /// The point is coverage rather than meaning: the two evaluators are
    /// compared against each other, so what the numbers *are* does not matter
    /// and a `NaN` running through half of it is a feature.
    fn every_op() -> Ir {
        let mut low = Lowering::new(Target::Bake);
        let uv = low.uv();
        let u = low.emit(Op::Extract(0), [uv]);
        let v = low.emit(Op::Extract(1), [uv]);
        // A plane to sample, so `Op::Sample` and a `BufferPlan` are in the
        // arena the way a buffered filter puts them there.
        let plan_root = low.emit(Op::Mul, [u, v]);
        let buffer = low.buffer(plan_root, Filter::None, None);
        let sampled = low.sample(buffer, uv);

        let mut values: Vec<ValueId> = vec![u, v, sampled];
        // A macro rather than a closure: the emission borrows the lowering and
        // the push borrows the list, and as two statements they do not overlap.
        macro_rules! keep {
            ($emitted:expr) => {{
                let value = $emitted;
                values.push(value);
                value
            }};
        }
        let a = keep!(low.emit(Op::Sub, [u, v]));
        let b = keep!(low.emit(Op::Add, [u, sampled]));
        let c = keep!(low.emit(Op::Mul, [v, sampled]));
        let vec2 = keep!(low.emit(Op::Compose, [u, v]));
        let vec2b = keep!(low.emit(Op::Compose, [a, b]));
        let vec3 = keep!(low.emit(Op::Compose, [u, v, a]));
        let vec3b = keep!(low.emit(Op::Compose, [b, c, u]));

        // The sources that are not the coordinate.
        let constant = low.constant(0.375);
        keep!(low.emit(Op::Add, [u, constant]));
        let param = keep!(low.emit(Op::Param(0), []));
        keep!(low.emit(Op::Mul, [param, v]));
        let time = keep!(low.emit(Op::Time, []));
        keep!(low.emit(Op::Add, [time, u]));
        let cut = keep!(low.emit(Op::CutFlag, []));
        keep!(low.emit(Op::Sub, [cut, v]));
        let world_pos = keep!(low.emit(Op::WorldPos, []));
        keep!(low.emit(Op::Add, [world_pos, vec3]));
        let world_normal = keep!(low.emit(Op::WorldNormal, []));
        keep!(low.emit(Op::Mul, [world_normal, vec3b]));

        // Infinity and `NaN` in the arithmetic without an infinite coordinate
        // reaching `Op::Sample`.
        let big = low.constant(1.0e30_f32);
        let huge = keep!(low.emit(Op::Mul, [u, big]));
        let infinite = keep!(low.emit(Op::Exp2, [huge]));
        let not_a_number = keep!(low.emit(Op::Sub, [infinite, infinite]));
        keep!(low.emit(Op::Add, [not_a_number, vec3]));
        keep!(low.emit(Op::Mul, [infinite, vec3b]));
        keep!(low.emit(Op::Min, [not_a_number, u]));
        keep!(low.emit(Op::Max, [not_a_number, u]));
        keep!(low.emit(Op::Div, [u, not_a_number]));
        keep!(low.emit(Op::Normalize, [vec3]));

        for op in [
            Op::Abs,
            Op::Floor,
            Op::Fract,
            Op::Sqrt,
            Op::Exp2,
            Op::Log2,
            Op::Sin,
            Op::Cos,
            Op::Normalize,
        ] {
            keep!(low.emit(op, [u]));
            keep!(low.emit(op, [vec3]));
        }
        for op in [
            Op::Add,
            Op::Sub,
            Op::Mul,
            Op::Div,
            Op::Min,
            Op::Max,
            Op::Pow,
            Op::Atan2,
            Op::Step,
        ] {
            keep!(low.emit(op, [a, b]));
            keep!(low.emit(op, [vec3, vec3b]));
        }
        for op in [Op::Mix, Op::Smoothstep, Op::Clamp] {
            keep!(low.emit(op, [a, b, c]));
            keep!(low.emit(op, [vec3, vec3b, vec3]));
        }
        keep!(low.emit(Op::Select, [u, a, b]));
        keep!(low.emit(Op::Select, [v, vec3, vec3b]));
        keep!(low.emit(Op::Hash2(7), [vec2, vec2b]));
        keep!(low.emit(Op::Hash3(11), [vec3, vec3b]));
        keep!(low.emit(Op::Length, [vec3]));
        keep!(low.emit(Op::Length, [vec2]));
        keep!(low.emit(Op::Dot, [vec3, vec3b]));
        keep!(low.emit(Op::Dot, [vec2, vec2b]));
        for channel in 0_u8..4 {
            keep!(low.emit(Op::Extract(channel), [vec3]));
        }

        // Everything folded into one root, so nothing is dropped as dead. A
        // `Vec2` does not convert to a scalar, so a lane is extracted from it.
        let mut root = u;
        let scalars: Vec<ValueId> = values
            .iter()
            .map(|value| match low.type_of(*value) {
                IrType::Float => *value,
                _ => low.emit(Op::Extract(0), [*value]),
            })
            .collect();
        for scalar in scalars {
            root = low.emit(Op::Add, [root, scalar]);
        }
        low.finish([("height".to_owned(), root)])
            .expect("the expression lowers")
    }

    /// Coordinates a graph is allowed to reach an op with, the ordinary ones
    /// among them.
    ///
    /// No infinity and nothing past `2^63` texels, because
    /// [`Plane::sample`](crate::interp::Plane::sample) adds one to a floored
    /// coordinate that has already saturated the cast, and in a debug build
    /// that is an overflow panic before either evaluator has an opinion. It is
    /// the sampler's, it predates this module, and the infinities that matter
    /// here arrive through the ops instead — `exp2` of a huge number, and the
    /// `NaN` that subtracting two of those makes.
    fn coordinates() -> Vec<[f32; 2]> {
        let axis = [
            0.0_f32,
            0.5,
            1.0,
            0.001_953_125,
            -0.25,
            -7.75,
            1_000_000.5,
            -1_000_000.5,
            f32::NAN,
            -0.0,
            f32::MIN_POSITIVE,
        ];
        let mut uvs = Vec::new();
        for u in axis {
            for v in axis {
                uvs.push([u, v]);
            }
        }
        uvs
    }

    /// A plane with the same awkward numbers in it, so a sample reads them too.
    fn awkward_plane() -> Plane {
        Plane::new(
            4,
            IrType::Float,
            vec![
                0.0,
                -0.0,
                1.0,
                -3.5,
                f32::NAN,
                f32::INFINITY,
                f32::NEG_INFINITY,
                f32::MAX,
                f32::MIN_POSITIVE,
                0.25,
                -0.75,
                123.5,
                -1e30,
                1e30,
                0.5,
                -0.5,
            ],
        )
    }

    /// Bit equality, with the two `NaN`s of one arithmetic counted equal, which
    /// is the rule [`interp`](crate::interp)'s own sampler tests use.
    ///
    /// It is not a loophole for this module: which quiet `NaN` an `f32` add
    /// produced is not part of the contract, and it is not stable *inside* the
    /// per-texel walk either — `a + b` where both are `NaN` answers `a`'s
    /// payload in one lane of `apply` and `b`'s in another, because LLVM is
    /// free to swap the operands of a commutative `fadd`. Everything that is
    /// part of the contract is compared to the bit.
    fn same_bits(left: [f32; 3], right: [f32; 3]) -> bool {
        left.iter()
            .zip(right)
            .all(|(l, r)| l.to_bits() == r.to_bits() || (l.is_nan() && r.is_nan()))
    }

    /// Run both evaluators over the same coordinates and insist on the same
    /// bits in every register.
    fn agree(ir: &Ir, inputs: &Inputs<'_>, uvs: &[[f32; 2]]) {
        let interpreter = Interpreter::new(ir);
        let pinned = pin_everything(ir);
        let plan = interpreter.strip_plan(&pinned);
        let mut file = plan.file();
        let mut registers = interpreter.registers();
        for (chunk, block) in uvs.chunks(STRIP).enumerate() {
            plan.run(chunk * STRIP, block, inputs, &mut file)
                .expect("the strip evaluates");
            for (texel, uv) in block.iter().enumerate() {
                interpreter
                    .run(*uv, inputs, &mut registers)
                    .expect("the walk evaluates");
                for value in &pinned {
                    let Some(at) = plan.slot(*value) else {
                        continue;
                    };
                    let (walked, striped) = (registers.get(*value), file.value(at, texel));
                    assert!(
                        same_bits(walked, striped),
                        "value {value:?} ({:?}) at uv {uv:?}: {:?} walked, {:?} striped",
                        ir.inst(*value).map(|inst| inst.op),
                        walked.map(f32::to_bits),
                        striped.map(f32::to_bits)
                    );
                }
            }
        }
    }

    #[test]
    fn a_strip_evaluates_every_op_to_the_bits_the_per_texel_walk_does() {
        let ir = every_op();
        let plane = awkward_plane();
        let inputs = Inputs {
            time: 2.75,
            world_pos: [-1.5, 0.0, f32::INFINITY],
            world_normal: [0.0, -0.0, f32::NAN],
            cut_flag: 1.0,
            params: &[[1.5, -2.0, 3.0]],
            buffers: &[plane],
        };
        // Every op of the interpreter is in this expression, so a strip arm
        // that drifted from its `apply` arm has nowhere to hide.
        let emitted: std::collections::BTreeSet<(u16, u32)> =
            ir.insts().iter().map(|inst| inst.op.code()).collect();
        assert!(
            emitted.len() >= 34,
            "the fixture should reach every op; it reached {}",
            emitted.len()
        );
        agree(&ir, &inputs, &coordinates());
    }

    #[expect(
        clippy::too_many_lines,
        reason = "one test: bake the planes, build the frontier by hand, and \
                  compare the two evaluators over it"
    )]
    #[test]
    fn a_strip_over_a_shipped_graph_is_the_per_texel_walk_bit_for_bit() {
        use std::collections::BTreeMap;
        use std::num::NonZeroUsize;

        // A graph out of the standard library rather than a fixture: real
        // planes under real `Op::Sample`s, and the mix of widths and shared
        // sub-expressions a hand-written expression never quite has.
        let library = crate::stdlib::graphs();
        let graph = library
            .get("weathering:edge_wear")
            .expect("the standard library holds it");
        let resolution = crate::bake::MIN_RESOLUTION;
        let threads = NonZeroUsize::new(2);
        let request = crate::bake::BakeRequest {
            graph,
            library: &library,
            params: &BTreeMap::new(),
            resolution,
            mips: false,
            threads,
        };
        let ir = crate::bake::plan(&request).expect("the graph lowers").ir;
        assert!(
            !ir.buffers().is_empty(),
            "the point of this graph is that it has planes"
        );
        let mut cache = crate::planes::BakeCache::default();
        let buffers = crate::planes::rasterise_buffers(&ir, resolution, threads, &mut cache)
            .expect("the planes rasterise");
        let bound: Vec<ValueId> = crate::memo::OUTPUTS
            .iter()
            .filter_map(|port| ir.root(port))
            .collect();
        let inputs = Inputs {
            buffers: &buffers,
            ..Inputs::default()
        };

        // Values in the middle of the outputs' own walk, turned into planes
        // the way a stage of a bake turns them into planes, so the memoised
        // `Load` steps are exercised against real numbers rather than a
        // fixture's. This graph's own stages happen to share nothing with its
        // outputs, so the frontier is built here instead of read off the bake.
        let live = ir.reaches(&bound);
        let candidates: Vec<ValueId> = live
            .iter()
            .enumerate()
            .filter(|(index, reached)| {
                **reached
                    && ir
                        .insts()
                        .get(*index)
                        .is_some_and(|inst| !inst.operands.is_empty())
            })
            .map(|(index, _)| ValueId::at(index))
            .step_by(13)
            .collect();
        assert!(candidates.len() >= 2, "the graph should offer a few");
        let whole = Interpreter::for_values(&ir, &bound);
        let mut registers = whole.registers();
        let size = resolution as f32;
        let texels = (resolution as usize).pow(2);
        let mut kept: Vec<Vec<f32>> = candidates
            .iter()
            .map(|_| Vec::with_capacity(texels))
            .collect();
        for row in 0..resolution as usize {
            let v = (row as f32 + 0.5) / size;
            for column in 0..resolution as usize {
                let uv = [(column as f32 + 0.5) / size, v];
                whole
                    .run(uv, &inputs, &mut registers)
                    .expect("it evaluates");
                for (lanes, value) in kept.iter_mut().zip(&candidates) {
                    let width = ir.type_of(*value).map_or(1, IrType::components);
                    lanes.extend_from_slice(&registers.get(*value)[..width]);
                }
            }
        }
        let planes: Vec<Plane> = candidates
            .iter()
            .zip(kept)
            .map(|(value, lanes)| {
                Plane::new(
                    resolution,
                    ir.type_of(*value).unwrap_or(IrType::Float),
                    lanes,
                )
            })
            .collect();
        let frontier: Vec<(ValueId, &Plane)> = candidates.iter().copied().zip(&planes).collect();

        let interpreter = Interpreter::for_values_memoised(&ir, &bound, &frontier);
        let plan = interpreter.strip_plan(&bound);
        assert!(
            plan.loads() > 0,
            "the walk should be reading planes back, which is half of this"
        );
        let mut file = plan.file();
        let mut theirs = interpreter.registers();
        // Four rows rather than the whole grid: the claim is per texel, and
        // the goldens bake the rest of it.
        for row in [0_usize, 1, resolution as usize / 2, resolution as usize - 1] {
            let v = (row as f32 + 0.5) / size;
            for first in (0..resolution as usize).step_by(STRIP) {
                let uvs: Vec<[f32; 2]> = (first..first + STRIP)
                    .map(|column| [(column as f32 + 0.5) / size, v])
                    .collect();
                let base = row * resolution as usize + first;
                plan.run(base, &uvs, &inputs, &mut file)
                    .expect("the strip evaluates");
                for (texel, uv) in uvs.iter().enumerate() {
                    interpreter
                        .run_at(base + texel, *uv, &inputs, &mut theirs)
                        .expect("the walk evaluates");
                    for value in &bound {
                        let Some(at) = plan.slot(*value) else {
                            continue;
                        };
                        assert!(
                            same_bits(theirs.get(*value), file.value(at, texel)),
                            "output {value:?} at uv {uv:?}: {:?} walked, {:?} striped",
                            theirs.get(*value).map(f32::to_bits),
                            file.value(at, texel).map(f32::to_bits)
                        );
                    }
                }
            }
        }
        // And the slot reuse is doing its job: one slot per instruction would
        // be the working set this layout exists to avoid.
        assert!(
            plan.slots() * 3 < ir.len(),
            "slots {} against {} instructions",
            plan.slots(),
            ir.len()
        );
    }

    #[test]
    fn a_memoised_walk_run_by_strips_refuses_the_per_texel_entry_point() {
        // The other half of pre-step (b): `run` on a walk that reads planes
        // back is a refusal rather than a register file full of the last
        // texel's numbers.
        let mut low = Lowering::new(Target::Bake);
        let uv = low.uv();
        let u = low.emit(Op::Extract(0), [uv]);
        let doubled = low.emit(Op::Add, [u, u]);
        let ir = low
            .finish([("height".to_owned(), doubled)])
            .expect("it lowers");
        let root = ir.root("height").expect("the height root");
        let plane = Plane::new(2, IrType::Float, vec![10.0, 20.0, 30.0, 40.0]);
        let memoised = Interpreter::for_values_memoised(&ir, &[root], &[(root, &plane)]);
        let mut registers = memoised.registers();
        assert_eq!(
            memoised.run([0.25, 0.25], &Inputs::default(), &mut registers),
            Err(crate::interp::EvalError::Memoised(1))
        );
    }
}
