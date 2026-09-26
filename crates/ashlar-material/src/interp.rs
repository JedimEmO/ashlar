//! The IR interpreter: one texel at a time, over `f32`.
//!
//! This is the reference backend. Every [`Op`] is implemented here and, when
//! the shader backend lands, again as WGSL; the conformance test compares the
//! two on exactly this set, and nothing else in the crate is written twice.
//!
//! Evaluation is a walk over the instruction list into a register file. Every
//! register is three lanes wide whatever the value's type, with the unused
//! lanes zero, so a component-wise op is one arm however wide its operands are.
//! [`narrow`] is what holds that invariant up: an op is computed on all three
//! lanes and the answer is cut to the width its instruction declares, because
//! plenty of ops are non-zero at zero — `exp2(0)` is one — and a `Vec2` whose
//! third lane is one is a `Vec2` whose length is wrong.
//! The register file is handed in rather than allocated, because the plane
//! rasteriser runs one per row across `std::thread::scope` and a bake is a few
//! million texels.
//!
//! The walk itself is built once, in [`Interpreter::for_values`], and not
//! decided again per texel: it holds the instructions it will run in order,
//! which is what keeps a plane whose sub-expression is two instructions from
//! paying for the four thousand it does not reach, and it leaves
//! [`Op::Const`] out, because a constant writes the same register at every
//! texel and the register file is written once at the start.
//!
//! A bake goes further: a value an earlier stage of it already computed is
//! read back out of a plane rather than evaluated again, and the whole
//! sub-expression under it is left out of the walk.
//! [`Interpreter::for_values_memoised`] is that walk and
//! [`Interpreter::run_at`] drives it, texel by texel;
//! [`memo`](crate::memo) is where a bake works out which values those are.
//!
//! # Planes
//!
//! [`Op::Sample`] is the one instruction that reads something other than its
//! operands: a [`Plane`] that the pipeline in [`planes`](crate::planes)
//! rasterised and filtered before this expression ran. A plane is sampled
//! bilinearly with wrap, so a buffered filter tiles for the same reason every
//! generator does, and it is handed in through [`Inputs::buffers`] rather than
//! held here, because the interpreter is a pure function of a texel and the
//! planes belong to the bake that made them.
//!
//! # Randomness
//!
//! [`Op::Hash2`] and [`Op::Hash3`] are the only source of randomness, and they
//! are the wrapping multiply-xorshift the showcase's pixel loop already used:
//! integer, exact, and identical in WGSL, which is what lets a noise tile.
//! [`hash2_bits`] at seed zero is that function bit for bit, which the
//! crate's interpreter tests hold against a copy of the original.

use crate::ir::{BufferId, Inst, Ir, IrType, MAX_OPERANDS, Op, ValueId};

/// What an expression reads besides its texel coordinate.
///
/// The first four fields are zero in a bake, because a bake has no frame and no
/// mesh. They exist for the shader backend, where [`Op::Time`],
/// [`Op::WorldPos`], [`Op::WorldNormal`] and [`Op::CutFlag`] are what a texture
/// cannot be. [`Self::buffers`] is the one a bake fills: the planes its
/// buffered filters left behind.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Inputs<'a> {
    /// Seconds since the app started.
    pub time: f32,
    /// The fragment's world position, in metres.
    pub world_pos: [f32; 3],
    /// The fragment's world normal, unit length.
    pub world_normal: [f32; 3],
    /// One on a cut face, zero elsewhere.
    pub cut_flag: f32,
    /// The live parameters, in [`Ir::params`] order. A scalar uses one lane.
    pub params: &'a [[f32; 3]],
    /// The rasterised planes, in [`Ir::buffers`] order. An [`Op::Sample`] past
    /// the end of this is [`EvalError::Unbaked`] rather than a guess.
    pub buffers: &'a [Plane],
}

/// Why an expression could not be evaluated.
///
/// Neither is anything an author can write: validation and lowering catch
/// those. Both are a backend handing the interpreter something it did not
/// prepare.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum EvalError {
    /// A buffered filter's plane was not handed in. The planes a bake
    /// rasterises cover every buffer its expression samples, and they are
    /// rasterised in an order that has each plan's own samples already
    /// answered, so this is a backend that evaluated an expression against
    /// planes that are not its own.
    #[error("buffer {0} has no rasterised plane")]
    Unbaked(usize),
    /// A live parameter had no value. The caller passed fewer than
    /// [`Ir::params`] lists.
    #[error("live parameter {0} has no value; {1} were given")]
    MissingParam(usize, usize),
    /// [`Interpreter::run`] was called on an interpreter built by
    /// [`Interpreter::for_values_memoised`]. Such a walk leaves out the whole
    /// sub-expression under every memoised value and reads it back from a
    /// plane instead, which only [`Interpreter::run_at`] can do, because only
    /// it is told which texel to read. Answering anyway would be a register
    /// file holding whatever the last texel left there — numbers that are
    /// neither wrong in a way a test would see nor right. The count is how
    /// many values that walk reads back.
    #[error("this walk reads {0} memoised values and must be driven by `run_at`")]
    Memoised(usize),
}

/// One rasterised plane: the square of texels an [`Op::Sample`] reads.
///
/// A plane is what a buffered filter leaves behind, and the one place this
/// crate holds a picture rather than an expression. It is square, row-major
/// from the top, and a texel holds only the lanes its [`IrType`] owns laid end
/// to end — a height plane is one `f32` a texel and a colour plane is three —
/// because the planes are the largest thing a bake allocates and two thirds of
/// a height plane would be zeroes.
///
/// Texel `(x, y)` sits at UV `((x + 0.5) / n, (y + 0.5) / n)`, the centre of
/// its cell, which is where the rasteriser evaluated it and where
/// [`Self::sample`] reads it back exactly. Everything else is bilinear, and
/// every read wraps: a plane is one repeat of a field that tiles, so a
/// coordinate outside `0..1` is the same field again rather than an edge.
#[derive(Clone, Debug, PartialEq)]
pub struct Plane {
    resolution: u32,
    value_type: IrType,
    lanes: Vec<f32>,
}

impl Plane {
    /// A plane over the lanes a rasteriser wrote.
    ///
    /// The lanes are trimmed or zero-extended to exactly one texel per cell of
    /// the square, because a plane whose length disagrees with its resolution
    /// would sample as a shear rather than as an error.
    pub fn new(resolution: u32, value_type: IrType, mut lanes: Vec<f32>) -> Self {
        let texels = (resolution as usize).saturating_mul(resolution as usize);
        lanes.resize(texels.saturating_mul(value_type.components()), 0.0);
        Self {
            resolution,
            value_type,
            lanes,
        }
    }

    /// Texels per side.
    pub fn resolution(&self) -> u32 {
        self.resolution
    }

    /// What a texel of it means.
    pub fn value_type(&self) -> IrType {
        self.value_type
    }

    /// How many `f32`s one texel occupies.
    pub fn components(&self) -> usize {
        self.value_type.components()
    }

    /// Every lane, row-major, a texel at a time. What a filter reads and
    /// writes.
    pub fn lanes(&self) -> &[f32] {
        &self.lanes
    }

    /// How many texels there are.
    pub fn texels(&self) -> usize {
        self.lanes.len() / self.components().max(1)
    }

    /// One texel by its position in the row-major order, with the lanes its
    /// type does not own left zero. Past the end reads zero.
    pub fn texel_at(&self, index: usize) -> [f32; 3] {
        let mut value = [0.0; 3];
        let base = index * self.components();
        for (lane, slot) in value.iter_mut().enumerate().take(self.components()) {
            *slot = self.lanes.get(base + lane).copied().unwrap_or_default();
        }
        value
    }

    /// One coordinate wrapped into `0..resolution`, which is the column or the
    /// row a tap past the edge lands on.
    ///
    /// A bake's plane is a power of two, and there the wrap is the low bits:
    /// `n.rem_euclid(2^k)` is `n & (2^k - 1)` for every `i64` in two's
    /// complement, the negative ones a tap past the edge produces included,
    /// and a mask is one instruction where a signed remainder is a division.
    /// `rem_euclid` stays for the sizes that are not powers of two, so a plane
    /// a caller sized by hand wraps the same way it always did.
    pub(crate) fn wrap(&self, coordinate: i64) -> usize {
        Wrap::of(i64::from(self.resolution.max(1))).at(coordinate)
    }

    /// How many `f32`s one row of texels occupies. What a filter walking rows
    /// rather than texels steps by.
    pub(crate) fn row_stride(&self) -> usize {
        self.resolution.max(1) as usize * self.components()
    }

    /// One texel by column and row, wrapped, so a neighbour past the edge is
    /// the neighbour on the other side.
    pub fn texel(&self, x: i64, y: i64) -> [f32; 3] {
        self.texel_at(self.wrap(y) * self.resolution.max(1) as usize + self.wrap(x))
    }

    /// The field at a UV, bilinearly, with wrap.
    ///
    /// One call's worth of the crate's own bilinear reader, which is where the
    /// arithmetic lives. A filter inside the crate samples the same plane tens
    /// or hundreds of times a texel and takes that reader directly, so it pays
    /// for the plane's geometry once rather than per call; from outside, this
    /// is the sample.
    pub fn sample(&self, uv: [f32; 2]) -> [f32; 3] {
        self.sampler().sample(uv)
    }

    /// A reader over this plane with its geometry worked out once.
    ///
    /// See [`Sampler`]: the numbers a sample computes are the same either way,
    /// and what is saved is the size, the mask, the stride and the component
    /// count that [`Self::sample`] would work out again per call.
    pub(crate) fn sampler(&self) -> Sampler<'_> {
        Sampler::of(self)
    }
}

/// A bilinear reader over one plane, with the plane's geometry hoisted.
///
/// A filter samples its plane tens or hundreds of times a texel — a slope blur
/// of 32 steps takes 161 samples, a directional Gaussian of radius 0.05 at 2048
/// takes 205 — and every one of those went through [`Plane::sample`], which
/// worked out the resolution as an `f32`, the component count from the type,
/// the row stride and whether the wrap is a mask before it read a lane. None of
/// that changes between two samples of one plane, so it is computed once here
/// and the per-sample work is the four reads and the two lerps.
///
/// The arithmetic is unchanged and deliberately so: [`Plane::sample`] is this
/// code, called once. Everything hoisted is integer or type bookkeeping, so
/// what a filter reads is the `f32` it read before, bit for bit.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Sampler<'a> {
    lanes: &'a [f32],
    /// The resolution as an `f32`, which is what a UV is scaled by.
    size: f32,
    /// How the wrap is done: see [`Wrap`].
    wrap: Wrap,
    components: usize,
    row_stride: usize,
}

impl<'a> Sampler<'a> {
    #[expect(
        clippy::cast_precision_loss,
        reason = "a resolution is bounded by MAX_RESOLUTION, well inside what an f32 counts exactly"
    )]
    fn of(plane: &'a Plane) -> Self {
        let components = plane.components();
        Self {
            lanes: &plane.lanes,
            size: plane.resolution.max(1) as f32,
            wrap: Wrap::of(i64::from(plane.resolution.max(1))),
            components,
            row_stride: plane.resolution.max(1) as usize * components,
        }
    }

    /// The field at a UV, bilinearly, with wrap.
    #[expect(
        clippy::inline_always,
        reason = "the callers are filter loops that take tens of samples a texel, \
                  and left to its own judgement the inliner keeps this out of \
                  line: measured, the call and its stack traffic are most of \
                  what a sample costs"
    )]
    #[inline(always)]
    pub(crate) fn sample(&self, uv: [f32; 2]) -> [f32; 3] {
        // Half a texel back, because a texel's value belongs to its centre:
        // without it a sample at a texel centre would land between two of them
        // and every plane would read half a texel along from where it was
        // written.
        let x = uv[0].mul_add(self.size, -0.5);
        let y = uv[1].mul_add(self.size, -0.5);
        let ((left, x0), (top, y0)) = (floor_cell(x), floor_cell(y));
        let (fx, fy) = (x - left, y - top);
        // The four taps share two rows and two columns, so the wrap and the
        // stride are four multiplies for the whole sample: going through
        // `texel` would redo both per tap and per lane, and a bilinear read is
        // what every buffered filter spends its time on.
        let (row_top, row_bottom) = (
            self.wrap.at(y0) * self.row_stride,
            self.wrap.at(y0 + 1) * self.row_stride,
        );
        let (left_column, right_column) = (
            self.wrap.at(x0) * self.components,
            self.wrap.at(x0 + 1) * self.components,
        );
        let mut value = [0.0; 3];
        for (lane, slot) in value.iter_mut().enumerate().take(self.components) {
            let row = |base: usize| {
                let a = self.lane_at(base + left_column + lane);
                let b = self.lane_at(base + right_column + lane);
                a + (b - a) * fx
            };
            let (top_row, bottom_row) = (row(row_top), row(row_bottom));
            *slot = top_row + (bottom_row - top_row) * fy;
        }
        value
    }

    /// The first lane of the field at a UV, which is the whole of a height or
    /// a mask.
    ///
    /// The same four reads and two lerps [`Self::sample`] does for lane zero,
    /// without the three-lane array a caller of a height plane then walks past.
    #[expect(
        clippy::inline_always,
        reason = "the callers are filter loops that take tens of samples a texel, \
                  and left to its own judgement the inliner keeps this out of \
                  line: measured, the call and its stack traffic are most of \
                  what a sample costs"
    )]
    #[inline(always)]
    pub(crate) fn float(&self, uv: [f32; 2]) -> f32 {
        let x = uv[0].mul_add(self.size, -0.5);
        let y = uv[1].mul_add(self.size, -0.5);
        let ((left, x0), (top, y0)) = (floor_cell(x), floor_cell(y));
        let (fx, fy) = (x - left, y - top);
        let (row_top, row_bottom) = (
            self.wrap.at(y0) * self.row_stride,
            self.wrap.at(y0 + 1) * self.row_stride,
        );
        let (left_column, right_column) = (
            self.wrap.at(x0) * self.components,
            self.wrap.at(x0 + 1) * self.components,
        );
        let row = |base: usize| {
            let a = self.lane_at(base + left_column);
            let b = self.lane_at(base + right_column);
            a + (b - a) * fx
        };
        let (top_row, bottom_row) = (row(row_top), row(row_bottom));
        top_row + (bottom_row - top_row) * fy
    }

    /// One lane by its index into the plane's lanes. Past the end reads zero,
    /// which is what a plane whose resolution is zero has everywhere.
    #[expect(
        clippy::inline_always,
        reason = "a wrap is a mask and a bounds test; out of line it is a call"
    )]
    #[inline(always)]
    fn lane_at(&self, index: usize) -> f32 {
        self.lanes.get(index).copied().unwrap_or_default()
    }
}

/// How a coordinate past the edge of a plane comes back onto it.
///
/// [`Plane::wrap`]'s arithmetic, with the "is this a power of two" question
/// asked once rather than per tap. A bake's plane is a power of two, and there
/// the wrap is the low bits — `n.rem_euclid(2^k)` is `n & (2^k - 1)` for every
/// `i64` in two's complement, the negative ones a tap past the edge produces
/// included — and `rem_euclid` stays for the sizes that are not, so a plane a
/// caller sized by hand wraps the way it always did.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Wrap {
    size: i64,
    mask: i64,
    masked: bool,
}

impl Wrap {
    /// The wrap of a plane `size` texels a side.
    pub(crate) fn of(size: i64) -> Self {
        let size = size.max(1);
        Self {
            size,
            mask: size - 1,
            // `count_ones` rather than `is_power_of_two`, which the signed
            // integers do not carry; a size is at least one, so one bit set is
            // exactly a power of two.
            masked: size.count_ones() == 1,
        }
    }

    /// One coordinate wrapped into `0..size`.
    #[expect(
        clippy::inline_always,
        reason = "a wrap is a mask and a bounds test; out of line it is a call"
    )]
    #[inline(always)]
    pub(crate) fn at(self, coordinate: i64) -> usize {
        let wrapped = if self.masked {
            coordinate & self.mask
        } else {
            coordinate.rem_euclid(self.size)
        };
        usize::try_from(wrapped).unwrap_or_default()
    }

    /// How many texels a side, which is what a distance across the wrap is
    /// measured against.
    #[inline]
    pub(crate) fn size(self) -> i64 {
        self.size
    }

    /// The same as [`Self::at`], staying an `i64`, which is what an index
    /// computed by multiplying a wrapped row by a size wants.
    #[expect(
        clippy::inline_always,
        reason = "a wrap is a mask and a bounds test; out of line it is a call"
    )]
    #[inline(always)]
    pub(crate) fn signed(self, coordinate: i64) -> i64 {
        if self.masked {
            coordinate & self.mask
        } else {
            coordinate.rem_euclid(self.size)
        }
    }
}

/// A plane coordinate's floor and the cell it names: `(x.floor(), x.floor() as
/// i64)`, as one step because both callers want the pair.
///
/// This was a branch of integer arithmetic until 2026-09-20, and the reason it
/// no longer is says something about the crate. On the bare `x86-64` baseline
/// `f32::floor` is a call into `compiler_builtins`' bit-twiddling `floorf`, and
/// a bilinear sample takes two of them: truncating through an `i32` instead
/// measured a quarter of what a sample cost. The workspace now builds x86-64
/// with `+sse4.1` (see `.cargo/config.toml`), where the floor is a single
/// `roundss` that vectorises, and the integer form measured *slower* than this
/// one — SOI cobblestone -3.6%, brick -8.0%, at 2048 on eight threads. So the
/// hand-written floor was not a better floor, only a better one for a compiler
/// that had to synthesise it.
///
/// A target without `roundss` still pays for the call and still bakes the same
/// bytes, which is what let the two forms swap places at all: they were pinned
/// against each other over 200,000 bit patterns for as long as both existed,
/// and the two reference digests are unchanged across the swap.
#[expect(
    clippy::inline_always,
    reason = "two per bilinear sample, and a sample is what a filter does tens \
              of times a texel"
)]
#[inline(always)]
fn floor_cell(coordinate: f32) -> (f32, i64) {
    let floor = coordinate.floor();
    (floor, cell_index(floor))
}

/// A floored plane coordinate as a cell index, before the wrap that
/// [`Plane::texel`] applies.
///
/// A `NaN` becomes zero rather than something arbitrary: a graph is allowed to
/// compute a coordinate that is not a number, and a texel of the plane is a
/// better answer than a panic or a read of whatever the cast produced.
#[expect(
    clippy::cast_possible_truncation,
    reason = "the saturating float cast is the intended arithmetic; a coordinate \
              past 2^63 texels is not a plane anyone rasterised"
)]
fn cell_index(coordinate: f32) -> i64 {
    if coordinate.is_nan() {
        0
    } else {
        coordinate as i64
    }
}

/// A register file: one three-lane value per instruction.
///
/// Reused across texels, so rasterising a plane allocates once per thread.
/// The registers of the expression's constants are written when the file is
/// sized and not again, which is why the file remembers which interpreter
/// sized it: handing one to a second interpreter has to refill them, or the
/// second would read the first one's constants where its own should be.
#[derive(Clone, Debug, Default)]
pub struct Registers {
    values: Vec<[f32; 3]>,
    /// The interpreter whose constants sit in `values`, or zero for a file no
    /// interpreter has sized yet. [`Interpreter::stamp`] is where it comes
    /// from, and a clone of a file carries it because a clone holds the same
    /// numbers.
    filled_for: u64,
}

/// Two register files are equal when they hold the same numbers. Which
/// interpreter sized a file is bookkeeping, not part of what it holds.
impl PartialEq for Registers {
    fn eq(&self, other: &Self) -> bool {
        self.values == other.values
    }
}

impl Registers {
    /// One value's three lanes, or zero where the id belongs to another
    /// expression.
    pub fn get(&self, value: ValueId) -> [f32; 3] {
        self.values.get(value.index()).copied().unwrap_or_default()
    }

    /// One value's first lane, which is the whole of a
    /// [`Float`](crate::ir::IrType::Float).
    pub fn float(&self, value: ValueId) -> f32 {
        self.get(value)[0]
    }
}

/// Evaluates an [`Ir`] at a texel.
///
/// [`Self::new`] runs the whole instruction list; [`Self::for_values`] runs
/// only what the values named reach, which is what a backend wants whenever the
/// expression holds more than it is asking for — a plane's own sub-expression
/// while the outputs are being rasterised, or the outputs while a plane's is.
#[derive(Clone, Debug)]
pub struct Interpreter<'a> {
    ir: &'a Ir,
    /// The values read out of a plane an earlier stage of the bake left
    /// behind, each with the register it fills. Loaded per texel by
    /// [`Self::run_at`] and never by [`Self::prepare`], because unlike a
    /// constant a memoised value is a different number at every texel.
    memo: Vec<(usize, &'a Plane)>,
    /// The instructions to evaluate, in order, each with the register it
    /// writes. Held rather than filtered per texel: a bake walks this list a
    /// few million times, and a plane whose sub-expression is two instructions
    /// used to pay for a flag test on the other four thousand.
    order: Vec<(usize, Inst)>,
    /// The constant registers and what they hold. An [`Op::Const`] writes the
    /// same number at every texel, so it is written when the register file is
    /// sized and left out of the walk.
    constants: Vec<(usize, [f32; 3])>,
    /// Which interpreter this is, for [`Registers::filled_for`]. Not the
    /// expression: two interpreters over one [`Ir`] can have different live
    /// sets and therefore different constants, and a register file one of them
    /// sized has the other's constants missing rather than stale. A clone
    /// keeps the stamp, because a clone has the same constants.
    stamp: u64,
}

/// Where [`Interpreter::stamp`] comes from. A counter rather than anything
/// derived from the [`Ir`], so that no two live interpreters share a stamp and
/// no freed one can hand its stamp to the interpreter allocated after it.
static STAMPS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

impl<'a> Interpreter<'a> {
    /// Read an expression, every instruction of it.
    pub fn new(ir: &'a Ir) -> Self {
        Self::over(ir, None, Vec::new())
    }

    /// Read the part of an expression that the given values reach.
    ///
    /// The registers of everything else stay zero, and nothing a named value
    /// reaches can be one of them, so what is read through [`Self::eval`] or
    /// out of the [`Registers`] afterwards is the same number either way. What
    /// changes is the cost — a bake evaluates its outputs a few million times
    /// and its buffered sub-expressions are not among them — and which
    /// [`Op::Sample`]s are reached, which is what lets a plane be rasterised
    /// before the planes that come after it exist.
    pub fn for_values(ir: &'a Ir, values: &[ValueId]) -> Self {
        Self::over(ir, Some(ir.reaches(values)), Vec::new())
    }

    /// Read the part of an expression the given values reach, down to the
    /// values an earlier stage of the bake already computed.
    ///
    /// Each memoised value is a [`Plane`] of what it evaluated to, texel for
    /// texel, at this stage's resolution — so the walk stops there: the value
    /// is loaded out of its plane by [`Self::run_at`] and the whole
    /// sub-expression under it is never visited. That is the same `f32` the
    /// walk would have produced, because the stage that wrote the plane
    /// evaluated the same instructions against the same texel grid, which is
    /// what [`memo`](crate::memo) is about.
    ///
    /// Only [`Self::run_at`] drives one of these: it is the entry point that
    /// knows which texel to read, and [`Self::run`] would leave the memoised
    /// registers at whatever they last held.
    pub fn for_values_memoised(
        ir: &'a Ir,
        values: &[ValueId],
        memo: &[(ValueId, &'a Plane)],
    ) -> Self {
        let mut stop = vec![false; ir.len()];
        for (value, _) in memo {
            if let Some(slot) = stop.get_mut(value.index()) {
                *slot = true;
            }
        }
        let live = crate::memo::reaches_above(ir, values, &stop);
        // Only the memoised values this walk actually reaches: a plane the
        // expression never asks for is a texel read per texel for nothing.
        let loaded = memo
            .iter()
            .filter(|(value, _)| live.get(value.index()).copied() == Some(true))
            .map(|(value, plane)| (value.index(), *plane))
            .collect();
        Self::over(ir, Some(live), loaded)
    }

    /// Split the instructions this interpreter answers for into the constants,
    /// which a register file is born holding, the memoised values, which a
    /// texel is read for, and the walk.
    fn over(ir: &'a Ir, live: Option<Vec<bool>>, memo: Vec<(usize, &'a Plane)>) -> Self {
        let mut order = Vec::new();
        let mut constants = Vec::new();
        for (index, inst) in ir.insts().iter().enumerate() {
            if let Some(live) = &live
                && live.get(index).copied() != Some(true)
            {
                continue;
            }
            if memo.iter().any(|(at, _)| *at == index) {
                continue;
            }
            if let Op::Const(value) = inst.op {
                // The same cut `run` makes when it stores a register, so a
                // hoisted constant is the register the walk used to write.
                constants.push((index, narrow([value, 0.0, 0.0], inst.value_type)));
            } else {
                order.push((index, *inst));
            }
        }
        Self {
            ir,
            memo,
            order,
            constants,
            stamp: STAMPS.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
        }
    }

    /// A register file sized for this expression, with its constants in place.
    pub fn registers(&self) -> Registers {
        let mut registers = Registers::default();
        self.prepare(&mut registers);
        registers
    }

    /// Size a register file for this expression and write its constants.
    ///
    /// A file that is already this interpreter's is left alone, which is the
    /// per-texel case; one from another interpreter — or from none at all,
    /// which is what [`Registers::default`] is — is resized and refilled,
    /// because its constant registers hold the other one's numbers and nothing
    /// in the walk would overwrite them.
    fn prepare(&self, registers: &mut Registers) {
        if registers.filled_for == self.stamp && registers.values.len() == self.ir.len() {
            return;
        }
        registers.values.resize(self.ir.len(), [0.0; 3]);
        for (index, value) in &self.constants {
            if let Some(slot) = registers.values.get_mut(*index) {
                *slot = *value;
            }
        }
        registers.filled_for = self.stamp;
    }

    /// Evaluate at one texel, into a register file.
    ///
    /// The registers are resized if they came from another expression, so a
    /// caller that reuses one across a rebake is never reading stale widths.
    ///
    /// [`EvalError::Memoised`] for a walk that reads values back out of
    /// planes: those are indexed by texel and this entry point does not know
    /// which texel it is at, so the honest answer is a refusal rather than a
    /// register file that is right everywhere except where it matters.
    pub fn run(
        &self,
        uv: [f32; 2],
        inputs: &Inputs<'_>,
        registers: &mut Registers,
    ) -> Result<(), EvalError> {
        if !self.memo.is_empty() {
            return Err(EvalError::Memoised(self.memo.len()));
        }
        self.prepare(registers);
        self.walk(uv, inputs, registers)
    }

    /// Evaluate at one texel of a bake's grid, into a register file.
    ///
    /// What [`Self::run`] is for a rasteriser: `texel` is the position in the
    /// row-major order of the plane being written, and it is what the memoised
    /// values are read at — by index and not by UV, because what a later stage
    /// needs is the `f32` the earlier stage stored and not a bilinear read of
    /// the four around it.
    ///
    /// The memoised registers are loaded here rather than where the constants
    /// are, which is the one thing about this that is not obvious: a register
    /// file that is already this interpreter's is left alone when it is handed
    /// back, so a memoised value written there would be written once per span
    /// and stand for every texel of it.
    pub fn run_at(
        &self,
        texel: usize,
        uv: [f32; 2],
        inputs: &Inputs<'_>,
        registers: &mut Registers,
    ) -> Result<(), EvalError> {
        self.prepare(registers);
        for (index, plane) in &self.memo {
            if let Some(slot) = registers.values.get_mut(*index) {
                *slot = plane.texel_at(texel);
            }
        }
        self.walk(uv, inputs, registers)
    }

    /// The walk itself, over a register file that is already this
    /// interpreter's.
    fn walk(
        &self,
        uv: [f32; 2],
        inputs: &Inputs<'_>,
        registers: &mut Registers,
    ) -> Result<(), EvalError> {
        for (index, inst) in &self.order {
            // The operands of a validated `Ir` are always earlier registers,
            // so this is one slice read and a bounds check per operand rather
            // than a `Registers::get` call that repeats the borrow.
            let mut args = [[0.0_f32; 3]; MAX_OPERANDS];
            for (slot, operand) in args.iter_mut().zip(inst.operands.as_slice()) {
                *slot = registers
                    .values
                    .get(operand.index())
                    .copied()
                    .unwrap_or_default();
            }
            let value = match inst.op {
                Op::Uv => [uv[0], uv[1], 0.0],
                Op::Param(index) => {
                    let index = index as usize;
                    *inputs
                        .params
                        .get(index)
                        .ok_or(EvalError::MissingParam(index, inputs.params.len()))?
                }
                Op::Time => [inputs.time, 0.0, 0.0],
                Op::WorldPos => inputs.world_pos,
                Op::WorldNormal => inputs.world_normal,
                Op::CutFlag => [inputs.cut_flag, 0.0, 0.0],
                // The plane was rasterised before this expression ran, and
                // reading it bilinearly with wrap is what makes a buffered
                // filter a field rather than a grid: it is sampled at whatever
                // coordinate reached it, which at the resolution the plane was
                // rasterised at lands on a texel centre exactly.
                Op::Sample(buffer) => plane(inputs, buffer)?.sample([args[0][0], args[0][1]]),
                // Hoisted into the register file by `prepare`, so this arm is
                // only what keeps the match total.
                Op::Const(value) => [value, 0.0, 0.0],
                // Everything left reads only its operands, which is exactly
                // the set `apply` answers for, and none of it can fail — so
                // the walk carries no `Result` through its hot path.
                op => apply(op, &args[..inst.operands.len()]).unwrap_or_default(),
            };
            let value = narrow(value, inst.value_type);
            if let Some(slot) = registers.values.get_mut(*index) {
                *slot = value;
            }
        }
        Ok(())
    }

    /// Evaluate at one texel and read one value. Allocates a register file, so
    /// it is for a test or a one-off rather than for a plane.
    pub fn eval(
        &self,
        uv: [f32; 2],
        inputs: &Inputs<'_>,
        value: ValueId,
    ) -> Result<[f32; 3], EvalError> {
        let mut registers = self.registers();
        self.run(uv, inputs, &mut registers)?;
        Ok(registers.get(value))
    }

    /// The same walk, flattened for a strip of texels at a time.
    ///
    /// What the rasterisers drive. The steps and the numbers are this
    /// interpreter's — the same live walk, the same hoisted constants, the
    /// same memoised planes — laid out so that one instruction covers
    /// [`STRIP`](crate::strip::STRIP) texels instead of one. `pinned` is the
    /// values the caller reads back after a strip has run, whose slots are
    /// therefore never reused.
    pub(crate) fn strip_plan(&self, pinned: &[ValueId]) -> crate::strip::StripPlan<'a> {
        crate::strip::StripPlan::build(self.ir, &self.order, &self.constants, &self.memo, pinned)
    }

    /// Evaluate at one texel and read one value's first lane.
    pub fn eval_float(
        &self,
        uv: [f32; 2],
        inputs: &Inputs<'_>,
        value: ValueId,
    ) -> Result<f32, EvalError> {
        Ok(self.eval(uv, inputs, value)?[0])
    }
}

/// The plane one buffer id names.
pub(crate) fn plane<'a>(inputs: &Inputs<'a>, buffer: BufferId) -> Result<&'a Plane, EvalError> {
    inputs
        .buffers
        .get(buffer.index())
        .ok_or(EvalError::Unbaked(buffer.index()))
}

/// Apply one pure op to its operand registers.
///
/// `None` for the ops that read something an expression does not carry: the
/// sources ([`Op::Const`], [`Op::Uv`], [`Op::Param`], [`Op::Time`],
/// [`Op::WorldPos`], [`Op::WorldNormal`], [`Op::CutFlag`]) and [`Op::Sample`].
/// Every op is computed on all three lanes; the caller cuts the answer to the
/// width the instruction declares with [`narrow`]. Constant folding and the
/// interpreter both go through this pair, so a folded constant and an evaluated
/// one can never disagree.
pub(crate) fn apply(op: Op, args: &[[f32; 3]]) -> Option<[f32; 3]> {
    let a = args.first().copied().unwrap_or_default();
    let b = args.get(1).copied().unwrap_or_default();
    let c = args.get(2).copied().unwrap_or_default();
    let each = |f: fn(f32) -> f32| [f(a[0]), f(a[1]), f(a[2])];
    let pair = |f: fn(f32, f32) -> f32| [f(a[0], b[0]), f(a[1], b[1]), f(a[2], b[2])];
    let lanes = |f: &dyn Fn(usize) -> f32| [f(0), f(1), f(2)];
    Some(match op {
        Op::Add => pair(|x, y| x + y),
        Op::Sub => pair(|x, y| x - y),
        Op::Mul => pair(|x, y| x * y),
        // Zero rather than an infinity: a division by a mask is ordinary in a
        // graph, and one poisoned texel spreads through every filter after it.
        Op::Div => pair(|x, y| if y == 0.0 { 0.0 } else { x / y }),
        Op::Min => pair(f32::min),
        Op::Max => pair(f32::max),
        Op::Abs => each(f32::abs),
        Op::Floor => each(f32::floor),
        // The WGSL definition, which wraps a negative coordinate forward;
        // `f32::fract` truncates towards zero and would not.
        Op::Fract => each(|x| x - x.floor()),
        Op::Sqrt => each(f32::sqrt),
        Op::Pow => pair(f32::powf),
        Op::Exp2 => each(f32::exp2),
        Op::Log2 => each(f32::log2),
        Op::Sin => each(f32::sin),
        Op::Cos => each(f32::cos),
        Op::Atan2 => pair(f32::atan2),
        Op::Mix => lanes(&|lane| a[lane] + (b[lane] - a[lane]) * c[lane]),
        Op::Step => pair(|edge, x| if x < edge { 0.0 } else { 1.0 }),
        Op::Smoothstep => lanes(&|lane| smoothstep(a[lane], b[lane], c[lane])),
        // `f32::clamp` panics when the bounds cross; a graph may well write
        // bounds that cross, and a bake must not stop because it did.
        Op::Clamp => lanes(&|lane| a[lane].max(b[lane]).min(c[lane])),
        Op::Select => {
            if a[0] >= 0.5 {
                b
            } else {
                c
            }
        }
        Op::Hash2(seed) => [hash2(cell(a[0], b[0]), cell(a[1], b[1]), seed), 0.0, 0.0],
        Op::Hash3(seed) => [
            hash3(cell(a[0], b[0]), cell(a[1], b[1]), cell(a[2], b[2]), seed),
            0.0,
            0.0,
        ],
        Op::Length => [dot(a, a).sqrt(), 0.0, 0.0],
        Op::Dot => [dot(a, b), 0.0, 0.0],
        Op::Normalize => {
            let length = dot(a, a).sqrt();
            if length == 0.0 {
                [0.0; 3]
            } else {
                [a[0] / length, a[1] / length, a[2] / length]
            }
        }
        Op::Compose => lanes(&|lane| args.get(lane).map_or(0.0, |arg| arg[0])),
        Op::Extract(channel) => [
            a.get(usize::from(channel)).copied().unwrap_or(0.0),
            0.0,
            0.0,
        ],
        Op::Const(_)
        | Op::Uv
        | Op::Param(_)
        | Op::Time
        | Op::WorldPos
        | Op::WorldNormal
        | Op::CutFlag
        | Op::Sample(_) => return None,
    })
}

/// Cut a value to the lanes its type owns.
///
/// The lanes a `Float` or a `Vec2` does not own must read zero, because
/// [`Op::Length`], [`Op::Dot`], [`Op::Normalize`] and [`Op::Extract`] read all
/// three and cannot tell a lane that carries a component from one that carries
/// the leftovers of a component-wise op. Most ops leave the spare lanes zero on
/// their own; the ones that are non-zero at zero — `exp2`, `cos`, `pow(0, 0)`,
/// `step(0, 0)` — do not, which is what this is for.
pub fn narrow(value: [f32; 3], value_type: IrType) -> [f32; 3] {
    let mut narrowed = [0.0; 3];
    for (slot, lane) in narrowed.iter_mut().zip(value).take(value_type.components()) {
        *slot = lane;
    }
    narrowed
}

/// The dot product of three lanes. A `Vec2`'s third lane is zero, so this is
/// the two-component dot product as well.
pub(crate) fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

/// The smooth ramp from `low` to `high`.
///
/// Edges that meet are a step rather than a division by zero, and edges that
/// cross ramp backwards, which is what the division says and is no worse than
/// what the author wrote. The bounds of the clamp are literals, so it cannot
/// be the kind of clamp that panics.
#[expect(
    clippy::float_cmp,
    reason = "the exact equality is the point: it is the division by zero this \
              guards, and a ramp a margin wide is still a ramp"
)]
pub(crate) fn smoothstep(low: f32, high: f32, x: f32) -> f32 {
    if high == low {
        return if x < low { 0.0 } else { 1.0 };
    }
    let t = ((x - low) / (high - low)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// The lattice cell a coordinate falls in, reduced by a period.
///
/// Flooring is what makes the hash a lattice hash: a coordinate between two
/// cells belongs to the lower one. The reduction is what makes it wrap, and
/// therefore what makes every noise in the crate tile — cell `0` and cell
/// `period` are the same cell, so the field meets itself at the seam. A period
/// below one asks for no reduction, which is what a field with no period does.
///
/// **Both halves are done in integers where the numbers allow it**, and that is
/// the whole of why this is not two lines. A hash is a tenth of the
/// instructions a texel evaluates on every preset in the study, and each one
/// calls this two or three times, so on the SOI cobblestone this runs about
/// 2,600 times a texel against 324 `Op::Floor`s — it is where the crate's
/// float-to-integer work actually is. `f32::rem_euclid` is `compiler_builtins`'
/// `fmodf`, which is a loop and a vectorisation blocker in the strip around it,
/// and no target feature makes it anything else: there is no hardware `fmod`
/// the way `+sse4.1` gives [`floor_cell`] a hardware floor. That asymmetry is
/// why the reduction below stayed in integers when the floor went back to the
/// instruction.
///
/// The guards are what make the integer path the *same* answer rather than a
/// close one. Below `2^24` every `f32` is a whole number exactly and
/// [`floor_cell`] has already produced the floor as an `i64` on the way past,
/// so `i64::rem_euclid` of it is the mathematical remainder — which is what
/// `fmodf` answers too, exactly, because a remainder is representable. Outside
/// that range the coordinate's cell index may have saturated the cast and the
/// period may not be whole, so the float path stays, and with it the
/// infinities, the `NaN` and the enormous.
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "a lattice coordinate is a cell index; the saturating float cast \
              and the two's-complement wrap are the intended arithmetic, and \
              a coordinate past 2^31 cells is not a texture anyone baked"
)]
#[expect(
    clippy::cast_precision_loss,
    clippy::float_cmp,
    reason = "the round trip is the test: the branch above bounds the period \
              under 2^24, where every integer is an exact f32, so the equality \
              holds exactly when the period is whole — which is the one thing \
              the integer reduction needs to know"
)]
pub(crate) fn cell(coordinate: f32, period: f32) -> u32 {
    let (floored, index) = floor_cell(coordinate);
    if coordinate.abs() < 16_777_216.0 {
        if period < 1.0 {
            return index as u32;
        }
        if period < 16_777_216.0 {
            let whole = period as i64;
            if whole as f32 == period {
                return index.rem_euclid(whole) as u32;
            }
        }
    }
    let reduced = if period >= 1.0 {
        floored.rem_euclid(period)
    } else {
        floored
    };
    reduced as i64 as u32
}

/// The xxHash-derived primes the lattice hash mixes with. The first two are
/// the showcase's; the third carries a third axis and the fourth the seed.
const PRIMES: [u32; 4] = [374_761_393, 668_265_263, 3_266_489_917, 2_654_435_761];

/// The raw hash of one two-dimensional lattice cell.
///
/// The showcase's `x * P5 + y * P4`, avalanched by the same shift-multiply, so
/// at seed zero this is that function bit for bit and the study's textures do
/// not move when they become graphs. The seed enters as a third term, which
/// keeps that identity and gives any other seed a different field.
pub fn hash2_bits(x: u32, y: u32, seed: u32) -> u32 {
    avalanche(
        x.wrapping_mul(PRIMES[0])
            .wrapping_add(y.wrapping_mul(PRIMES[1]))
            .wrapping_add(seed.wrapping_mul(PRIMES[3])),
    )
}

/// The raw hash of one three-dimensional lattice cell.
pub fn hash3_bits(x: u32, y: u32, z: u32, seed: u32) -> u32 {
    avalanche(
        x.wrapping_mul(PRIMES[0])
            .wrapping_add(y.wrapping_mul(PRIMES[1]))
            .wrapping_add(z.wrapping_mul(PRIMES[2]))
            .wrapping_add(seed.wrapping_mul(PRIMES[3])),
    )
}

/// The mixing step: one xorshift, one multiply, one more xorshift.
fn avalanche(mixed: u32) -> u32 {
    let mixed = (mixed ^ (mixed >> 13)).wrapping_mul(1_274_126_177);
    mixed ^ (mixed >> 16)
}

/// A raw hash as a value in `0..=1`.
///
/// The showcase divides in `f64`; this divides in `f32`. The results differ by
/// the rounding of that one division and nothing else, which is far under the
/// step of an eight-bit channel.
#[expect(
    clippy::cast_precision_loss,
    reason = "the documented difference from the showcase's f64 normalisation"
)]
pub fn normalise(bits: u32) -> f32 {
    bits as f32 / u32::MAX as f32
}

/// The hash of one two-dimensional lattice cell, in `0..=1`.
pub fn hash2(x: u32, y: u32, seed: u32) -> f32 {
    normalise(hash2_bits(x, y, seed))
}

/// The hash of one three-dimensional lattice cell, in `0..=1`.
pub fn hash3(x: u32, y: u32, z: u32, seed: u32) -> f32 {
    normalise(hash3_bits(x, y, z, seed))
}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::float_cmp,
        reason = "these are exact answers: a ramp's ends, and a step"
    )]
    use super::*;

    #[test]
    fn a_lattice_cell_is_floored_and_wrapped() {
        assert_eq!(cell(3.7, 8.0), 3);
        assert_eq!(cell(8.0, 8.0), 0, "the seam is the origin");
        assert_eq!(cell(-0.5, 8.0), 7, "a step back from zero is the last cell");
        assert_eq!(cell(-1.0, 0.0), u32::MAX, "no period is no reduction");
        assert_eq!(cell(2.9, 1.0), 0);
    }

    #[test]
    fn a_value_keeps_only_the_lanes_its_type_owns() {
        assert_eq!(narrow([1.0, 2.0, 3.0], IrType::Float), [1.0, 0.0, 0.0]);
        assert_eq!(narrow([1.0, 2.0, 3.0], IrType::Vec2), [1.0, 2.0, 0.0]);
        assert_eq!(narrow([1.0, 2.0, 3.0], IrType::Vec3), [1.0, 2.0, 3.0]);
    }

    /// A plane of distinct lanes, so a wrap that lands one texel off shows.
    fn ramp(resolution: u32, value_type: IrType) -> Plane {
        let count = (resolution as usize).pow(2) * value_type.components();
        #[expect(
            clippy::cast_precision_loss,
            reason = "a few thousand lanes, each an exact f32"
        )]
        let lanes = (0..count).map(|lane| lane as f32 * 0.25 - 3.0).collect();
        Plane::new(resolution, value_type, lanes)
    }

    /// What [`Plane::wrap`] answered before the power-of-two path existed.
    fn wrap_by_remainder(plane: &Plane, coordinate: i64) -> usize {
        let size = i64::from(plane.resolution().max(1));
        usize::try_from(coordinate.rem_euclid(size)).unwrap_or_default()
    }

    /// What [`Plane::sample`] answered before it read rows: four `texel` reads
    /// through `rem_euclid`, one lane at a time.
    fn sample_by_remainder(plane: &Plane, uv: [f32; 2]) -> [f32; 3] {
        #[expect(clippy::cast_precision_loss, reason = "as in `Plane::sample`")]
        let size = plane.resolution().max(1) as f32;
        let x = uv[0].mul_add(size, -0.5);
        let y = uv[1].mul_add(size, -0.5);
        let (left, top) = (x.floor(), y.floor());
        let (fx, fy) = (x - left, y - top);
        let (x0, y0) = (cell_index(left), cell_index(top));
        let at = |x: i64, y: i64| {
            let row = wrap_by_remainder(plane, y) * plane.resolution().max(1) as usize;
            plane.texel_at(row + wrap_by_remainder(plane, x))
        };
        let mut value = [0.0; 3];
        for (lane, slot) in value.iter_mut().enumerate().take(plane.components()) {
            let row = |y: i64| {
                let a = at(x0, y)[lane];
                let b = at(x0 + 1, y)[lane];
                a + (b - a) * fx
            };
            let (top_row, bottom_row) = (row(y0), row(y0 + 1));
            *slot = top_row + (bottom_row - top_row) * fy;
        }
        value
    }

    /// Bit equality, with the two `NaN`s of one arithmetic counted equal: a
    /// coordinate that is not a number gives a lane that is not a number, and
    /// which quiet `NaN` an f32 add produced is not part of the contract.
    fn same_bits(left: [f32; 3], right: [f32; 3]) -> bool {
        left.iter()
            .zip(right)
            .all(|(l, r)| l.to_bits() == r.to_bits() || (l.is_nan() && r.is_nan()))
    }

    #[test]
    fn a_power_of_two_plane_wraps_by_mask_exactly_as_it_wrapped_by_remainder() {
        // Both paths, so the mask is checked against the remainder and the
        // remainder against itself.
        for resolution in [1_u32, 2, 8, 64, 3, 5, 100] {
            let plane = ramp(resolution, IrType::Float);
            for coordinate in [
                0_i64,
                1,
                -1,
                7,
                -7,
                63,
                64,
                -64,
                -65,
                i64::from(resolution),
                -i64::from(resolution),
                1 << 40,
                -(1 << 40),
                i64::MAX,
                i64::MIN,
                i64::MIN + 1,
            ] {
                assert_eq!(
                    plane.wrap(coordinate),
                    wrap_by_remainder(&plane, coordinate),
                    "resolution {resolution}, coordinate {coordinate}"
                );
            }
        }
    }

    // The test that pinned `floor_cell`'s integer branch against `f32::floor`
    // over 200,000 bit patterns went with that branch on 2026-09-20: with one
    // implementation left it asserted `x.floor() == x.floor()`. What it
    // covered besides the floor was `cell_index` over the infinities, the
    // subnormals and `NaN`, and the test below still reaches all of those
    // through `cell`.

    #[test]
    fn a_lattice_cell_is_what_the_float_reduction_answered() {
        // `cell` reduces in integers where the numbers allow it, and it runs
        // two or three times per hash — about 2,600 times a texel on the SOI
        // cobblestone — so what it answers has to be the float formulation it
        // replaced over the whole `f32`, not over the lattices a bake reaches.
        // The periods are the awkward ones by name and then random bit
        // patterns, which are mostly enormous, subnormal or `NaN`: exactly the
        // half the guards have to hand back to the float path.
        fn by_float(coordinate: f32, period: f32) -> u32 {
            let floored = coordinate.floor();
            let reduced = if period >= 1.0 {
                floored.rem_euclid(period)
            } else {
                floored
            };
            #[expect(
                clippy::cast_possible_truncation,
                clippy::cast_sign_loss,
                reason = "the arithmetic under test, written the way it was"
            )]
            let answer = reduced as i64 as u32;
            answer
        }
        let mut state = 0x9e37_79b9_7f4a_7c15_u64;
        let mut bits = || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            let bytes = state.to_le_bytes();
            f32::from_bits(u32::from_le_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]))
        };
        let periods = [
            0.0_f32,
            -0.0,
            0.5,
            -3.0,
            1.0,
            2.0,
            3.0,
            8.0,
            // `MAX_PERIOD`, which is the largest lattice a graph may ask for.
            4096.0,
            1.5,
            16_777_215.0,
            16_777_216.0,
            16_777_218.0,
            f32::INFINITY,
            f32::NAN,
        ];
        let coordinates = [
            0.0_f32,
            -0.0,
            1.0,
            -1.0,
            0.5,
            -0.5,
            3.7,
            -7.25,
            8.0,
            16_777_215.0,
            16_777_216.0,
            -16_777_216.0,
            f32::MIN_POSITIVE,
            f32::INFINITY,
            f32::NEG_INFINITY,
            f32::NAN,
        ];
        for coordinate in coordinates {
            for period in periods {
                assert_eq!(
                    cell(coordinate, period),
                    by_float(coordinate, period),
                    "cell({coordinate}, {period})"
                );
            }
        }
        for _ in 0..200_000 {
            let (coordinate, period) = (bits(), bits());
            assert_eq!(
                cell(coordinate, period),
                by_float(coordinate, period),
                "cell({coordinate} {:#x}, {period} {:#x})",
                coordinate.to_bits(),
                period.to_bits()
            );
            // And again against the periods a graph can actually write, which
            // is where the integer path is taken rather than declined.
            for period in periods {
                assert_eq!(
                    cell(coordinate, period),
                    by_float(coordinate, period),
                    "cell({coordinate} {:#x}, {period})",
                    coordinate.to_bits()
                );
            }
        }
    }

    #[test]
    fn a_single_lane_sample_is_the_first_lane_of_the_whole_one() {
        // `Sampler::float` is what a filter over a height or a mask reads, and
        // it is only allowed to exist because it is lane zero of `sample`.
        for (resolution, value_type) in [
            (8_u32, IrType::Float),
            (16, IrType::Vec2),
            (4, IrType::Vec3),
            (5, IrType::Float),
            (0, IrType::Float),
        ] {
            let plane = ramp(resolution, value_type);
            let sampler = plane.sampler();
            for u in [0.0_f32, 0.5, 0.031_25, -0.25, -7.75, 1_000_000.5, f32::NAN] {
                for v in [0.0_f32, 0.375, -0.5, 12.25, -9_999.5, f32::NAN] {
                    let uv = [u, v];
                    let (one, three) = (sampler.float(uv), sampler.sample(uv)[0]);
                    assert!(
                        one.to_bits() == three.to_bits() || (one.is_nan() && three.is_nan()),
                        "resolution {resolution}, {value_type:?}, uv {uv:?}: \
                         {one} against {three}"
                    );
                }
            }
        }
    }

    #[test]
    fn a_row_read_sample_is_the_four_texel_sample_bit_for_bit() {
        for (resolution, value_type) in [
            (8_u32, IrType::Float),
            (16, IrType::Vec2),
            (4, IrType::Vec3),
            (5, IrType::Float),
            (0, IrType::Float),
        ] {
            let plane = ramp(resolution, value_type);
            for u in [
                0.0_f32,
                -0.0,
                0.5,
                1.0,
                0.031_25,
                -0.25,
                -7.75,
                1_000_000.5,
                -1_000_000.5,
                // Either side of where the floor stops truncating through an
                // `i32`: at these resolutions the scaled coordinate is 2^24.
                2_097_152.0,
                -2_097_152.0,
                2_097_152.5,
                -1e-40,
                f32::NAN,
            ] {
                for v in [0.0_f32, -0.0, 0.375, -0.5, 12.25, -9_999.5, 1e-41, f32::NAN] {
                    let uv = [u, v];
                    assert!(
                        same_bits(plane.sample(uv), sample_by_remainder(&plane, uv)),
                        "resolution {resolution}, {value_type:?}, uv {uv:?}: \
                         {:?} against {:?}",
                        plane.sample(uv),
                        sample_by_remainder(&plane, uv)
                    );
                }
            }
        }
    }

    /// A three-instruction expression: the texel's `u`, and twice it.
    fn doubled_u() -> (crate::ir::Ir, ValueId, ValueId) {
        use crate::ir::{Lowering, Target};
        let mut low = Lowering::new(Target::Bake);
        let uv = low.uv();
        let u = low.emit(Op::Extract(0), [uv]);
        let doubled = low.emit(Op::Add, [u, u]);
        let ir = low
            .finish([("height".to_owned(), doubled)])
            .expect("the expression lowers");
        let root = ir.root("height").expect("the height root");
        let u = ir
            .insts()
            .iter()
            .position(|inst| matches!(inst.op, Op::Extract(0)))
            .map(ValueId::at)
            .expect("the extract survives");
        (ir, u, root)
    }

    #[test]
    fn a_memoised_value_is_read_at_the_texel_it_is_asked_for() {
        // The trap this pins: `prepare` returns early for a register file it
        // has already sized, so anything written there would stand for every
        // texel of a span. A memoised value is a different number at every
        // texel and must be loaded by `run_at`, after `prepare`.
        let (ir, u, root) = doubled_u();
        let plane = Plane::new(2, IrType::Float, vec![10.0, 20.0, 30.0, 40.0]);
        let inputs = Inputs::default();

        let read_root = Interpreter::for_values_memoised(&ir, &[root], &[(root, &plane)]);
        let mut registers = read_root.registers();
        read_root
            .run_at(0, [0.25, 0.25], &inputs, &mut registers)
            .expect("the walk has nothing that can fail");
        assert_eq!(registers.float(root), 10.0);
        read_root
            .run_at(3, [0.75, 0.75], &inputs, &mut registers)
            .expect("the walk has nothing that can fail");
        assert_eq!(
            registers.float(root),
            40.0,
            "a second texel, a second value"
        );

        // And a memoised value below the root carries the difference up.
        let read_u = Interpreter::for_values_memoised(&ir, &[root], &[(u, &plane)]);
        let mut registers = read_u.registers();
        read_u
            .run_at(1, [0.75, 0.25], &inputs, &mut registers)
            .expect("the walk has nothing that can fail");
        assert_eq!(registers.float(root), 40.0);
        read_u
            .run_at(2, [0.25, 0.75], &inputs, &mut registers)
            .expect("the walk has nothing that can fail");
        assert_eq!(registers.float(root), 60.0);
    }

    #[test]
    fn a_memoised_walk_answers_what_the_whole_walk_answers() {
        // The plane holds what the expression itself computes at those texels,
        // so reading it back and evaluating it are the same number — which is
        // the whole claim memoisation rests on.
        let (ir, u, root) = doubled_u();
        let whole = Interpreter::for_values(&ir, &[root]);
        let inputs = Inputs::default();
        let mut registers = whole.registers();
        // The centres of a two by two plane, in its row-major order.
        let grid = [[0.25, 0.25], [0.75, 0.25], [0.25, 0.75], [0.75, 0.75]];
        let mut lanes = Vec::new();
        for uv in grid {
            whole
                .run(uv, &inputs, &mut registers)
                .expect("the walk has nothing that can fail");
            lanes.push(registers.float(u));
        }
        let plane = Plane::new(2, IrType::Float, lanes);
        let memoised = Interpreter::for_values_memoised(&ir, &[root], &[(u, &plane)]);
        let mut theirs = memoised.registers();
        for (texel, uv) in grid.into_iter().enumerate() {
            whole
                .run(uv, &inputs, &mut registers)
                .expect("the walk has nothing that can fail");
            memoised
                .run_at(texel, uv, &inputs, &mut theirs)
                .expect("the walk has nothing that can fail");
            assert_eq!(
                registers.float(root).to_bits(),
                theirs.float(root).to_bits(),
                "texel {texel}"
            );
        }
    }

    #[test]
    fn a_ramp_between_meeting_edges_is_a_step() {
        assert!((smoothstep(0.0, 1.0, 0.5) - 0.5).abs() < 1e-6);
        assert_eq!(smoothstep(0.5, 0.5, 0.6), 1.0);
        assert_eq!(smoothstep(0.5, 0.5, 0.4), 0.0);
        assert_eq!(smoothstep(0.0, 1.0, -3.0), 0.0);
        assert_eq!(smoothstep(0.0, 1.0, 3.0), 1.0);
    }
}
