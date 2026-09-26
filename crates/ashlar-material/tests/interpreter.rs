//! The interpreter: every operator, and the hash the study's textures rest on.
//!
//! The hash is checked against a copy of the showcase's own function rather
//! than against a table of numbers, because what has to hold is that the two
//! are the same function: the study's textures move the day they stop being.
#![allow(
    clippy::unwrap_used,
    reason = "fixtures built in the test; a failure is a test failure"
)]
#![allow(
    clippy::float_cmp,
    reason = "these equalities are exact by construction: a zero, a hash of \
              the same lattice cell, and a normalised u32; a margin here would \
              pass a seam that does not actually meet"
)]
use ashlar_material::{
    interp::{EvalError, Inputs, Interpreter, Plane, hash2, hash2_bits, hash3_bits, normalise},
    ir::{Filter, IrType, Lowering, Op, Target, ValueId},
};
use proptest::prelude::*;

/// The lattice hash's reference form, split at the point it stops being integer.
///
/// It began as the showcase's hand-written pixel loop, since retired, and is
/// kept here as an independent oracle: this crate's hash at seed zero has to be
/// this function and not merely one like it, or every surface baked from it
/// changes.
fn showcase_hash(x: u32, y: u32) -> u32 {
    let mut h = x
        .wrapping_mul(374_761_393)
        .wrapping_add(y.wrapping_mul(668_265_263));
    h = (h ^ (h >> 13)).wrapping_mul(1_274_126_177);
    h ^ (h >> 16)
}

/// The showcase's normalisation, which is the same division in `f64`.
fn showcase_value(x: u32, y: u32) -> f64 {
    f64::from(showcase_hash(x, y)) / f64::from(u32::MAX)
}

/// The texel every operator case is evaluated at, and the time it reads.
const UV: [f32; 2] = [0.75, 0.25];
const TIME: f32 = 0.5;

/// Evaluate one op over operands that are not constants, so nothing folds on
/// the way in: the first is u, the second v, the third the time input.
fn run(op: Op, arity: usize) -> [f32; 3] {
    let mut cx = Lowering::new(Target::Bake);
    let coord = cx.uv();
    let u = cx.emit(Op::Extract(0), [coord]);
    let v = cx.emit(Op::Extract(1), [coord]);
    let time = cx.emit(Op::Time, []);
    let value = match arity {
        1 => cx.emit(op, [u]),
        2 => cx.emit(op, [u, v]),
        _ => cx.emit(op, [u, v, time]),
    };
    evaluate(cx, value)
}

/// Finish a hand-built expression and read its one root at [`UV`].
fn evaluate(cx: Lowering, value: ValueId) -> [f32; 3] {
    let ir = cx.finish([("out".to_string(), value)]).unwrap();
    let inputs = Inputs {
        time: TIME,
        ..Inputs::default()
    };
    Interpreter::new(&ir)
        .eval(UV, &inputs, ir.root("out").unwrap())
        .unwrap()
}

/// What every operator answers, to six decimals.
fn close(left: f32, right: f32) -> bool {
    (left - right).abs() < 1e-6
}

#[test]
fn every_operator_computes_what_it_says_it_does() {
    // Binary, over u = 0.75 and v = 0.25.
    for (op, expected) in [
        (Op::Add, 1.0),
        (Op::Sub, 0.5),
        (Op::Mul, 0.187_5),
        (Op::Div, 3.0),
        (Op::Min, 0.25),
        (Op::Max, 0.75),
        (Op::Pow, 0.930_604_9),
        (Op::Atan2, 1.249_045_8),
        // `step(edge, x)`: u is the edge, and v is below it.
        (Op::Step, 0.0),
    ] {
        let value = run(op, 2)[0];
        assert!(close(value, expected), "{op:?} answered {value}");
    }

    // Unary, over u = 0.75.
    for (op, expected) in [
        (Op::Abs, 0.75),
        (Op::Floor, 0.0),
        (Op::Fract, 0.75),
        (Op::Sqrt, 0.866_025_4),
        (Op::Exp2, 1.681_792_8),
        (Op::Log2, -0.415_037_5),
        (Op::Sin, 0.681_638_8),
        (Op::Cos, 0.731_688_9),
    ] {
        let value = run(op, 1)[0];
        assert!(close(value, expected), "{op:?} answered {value}");
    }

    // Ternary, over u = 0.75, v = 0.25 and time = 0.5.
    for (op, expected) in [
        (Op::Mix, 0.5),
        // The ramp runs backwards from 0.75 to 0.25, and 0.5 is its middle.
        (Op::Smoothstep, 0.5),
        // 0.75 held between 0.25 and 0.5.
        (Op::Clamp, 0.5),
        // The condition is 0.75, which is at least a half, so the first branch.
        (Op::Select, 0.25),
    ] {
        let value = run(op, 3)[0];
        assert!(close(value, expected), "{op:?} answered {value}");
    }
}

#[test]
fn the_vector_operators_read_every_lane() {
    let mut cx = Lowering::new(Target::Bake);
    let coord = cx.uv();
    let length = cx.emit(Op::Length, [coord]);
    assert!(close(evaluate(cx, length)[0], 0.790_569_4));

    let mut cx = Lowering::new(Target::Bake);
    let coord = cx.uv();
    let dot = cx.emit(Op::Dot, [coord, coord]);
    assert!(close(evaluate(cx, dot)[0], 0.625));

    let mut cx = Lowering::new(Target::Bake);
    let coord = cx.uv();
    let normalized = cx.emit(Op::Normalize, [coord]);
    let value = evaluate(cx, normalized);
    assert!(close(value[0], 0.948_683_3));
    assert!(close(value[1], 0.316_227_77));

    // A zero vector normalises to zero rather than to a NaN that would spread
    // through every filter downstream of it.
    let mut cx = Lowering::new(Target::Bake);
    let zero = cx.constant(0.0);
    let origin = cx.vector(&[zero, zero]);
    let normalized = cx.emit(Op::Normalize, [origin]);
    assert_eq!(evaluate(cx, normalized), [0.0; 3]);

    // A lane out of range reads zero; the type system says it cannot happen,
    // and the interpreter does not panic when something says otherwise.
    let mut cx = Lowering::new(Target::Bake);
    let coord = cx.uv();
    let beyond = cx.emit(Op::Extract(2), [coord]);
    assert_eq!(evaluate(cx, beyond)[0], 0.0);

    // The lanes a Vec2 does not own stay zero after an op that is non-zero at
    // zero, or its length counts a third component that is not there. Every
    // case above rides on a raw Uv, whose third lane is zero to begin with;
    // this one does not, and these are the widths the second node set works in.
    let mut cx = Lowering::new(Target::Bake);
    let coord = cx.uv();
    let raised = cx.emit(Op::Exp2, [coord]);
    let length = cx.emit(Op::Length, [raised]);
    // hypot(2^0.75, 2^0.25), and 1.732 if the spare lane carried exp2(0) = 1.
    assert!(close(evaluate(cx, length)[0], 2.059_766_8));

    let mut cx = Lowering::new(Target::Bake);
    let coord = cx.uv();
    let ones = cx.emit(Op::Step, [coord, coord]);
    let normalized = cx.emit(Op::Normalize, [ones]);
    let value = evaluate(cx, normalized);
    assert!(close(value[0], std::f32::consts::FRAC_1_SQRT_2));
    assert!(close(value[1], std::f32::consts::FRAC_1_SQRT_2));
    assert_eq!(value[2], 0.0, "a Vec2 has no third lane to normalise");

    // The same for a Float: one lane, and the other two are nobody's.
    let mut cx = Lowering::new(Target::Bake);
    let coord = cx.uv();
    let u = cx.emit(Op::Extract(0), [coord]);
    let raised = cx.emit(Op::Exp2, [u]);
    let spare = cx.emit(Op::Extract(1), [raised]);
    assert_eq!(evaluate(cx, spare)[0], 0.0);
}

#[test]
fn folding_an_expression_and_evaluating_it_answer_the_same_thing() {
    // The same arithmetic twice: once on constants, which the lowering folds
    // as it emits, and once on a Uv the interpreter evaluates at the texel the
    // constants name. The two backends of this crate are the folder and the
    // interpreter; if they can differ here, the reference backend has no
    // reference, and a lane no type owns is where they would differ first.
    let mut cx = Lowering::new(Target::Bake);
    let one = cx.constant(1.0);
    let corner = cx.vector(&[one, one]);
    let raised = cx.emit(Op::Exp2, [corner]);
    let folded = cx.emit(Op::Length, [raised]);
    let ir = cx.finish([("out".to_string(), folded)]).unwrap();
    let root = ir.root("out").unwrap();
    assert_eq!(
        ir.inst(root).map(|inst| inst.op),
        // hypot(exp2(1), exp2(1)), and hypot(2, 2, 1) if a lane no Vec2 owns
        // had carried exp2(0).
        Some(Op::Const(8.0_f32.sqrt())),
        "constant operands fold at lowering"
    );

    let mut cx = Lowering::new(Target::Bake);
    let coord = cx.uv();
    let raised = cx.emit(Op::Exp2, [coord]);
    let length = cx.emit(Op::Length, [raised]);
    let ir = cx.finish([("out".to_string(), length)]).unwrap();
    let evaluated = Interpreter::new(&ir)
        .eval_float([1.0, 1.0], &Inputs::default(), ir.root("out").unwrap())
        .unwrap();
    assert!(close(evaluated, 8.0_f32.sqrt()), "interpreted {evaluated}");
}

#[test]
fn a_vector_is_composed_of_scalars_and_says_so_when_it_is_not() {
    // `vec3(uv, 0)` is a mistake rather than a narrowing: Compose reads one
    // lane per operand, so the v axis would vanish without a word.
    let mut cx = Lowering::new(Target::Bake);
    let coord = cx.uv();
    let zero = cx.constant(0.0);
    let _ = cx.vector(&[coord, zero]);
    let error = cx
        .finish([("out".to_string(), zero)])
        .expect_err("a vector operand is not a scalar");
    assert!(
        error.to_string().contains("Vec2"),
        "the reason names the width that arrived: {error}"
    );
}

#[test]
fn an_operation_on_two_vector_widths_is_a_mistake_rather_than_a_guess() {
    // The same contract as above, through `emit` rather than through `vector`:
    // `emit` is public and `Op::Compose` reads lane zero of each operand
    // whichever door it was emitted by, so the check belongs to the op.
    let mut cx = Lowering::new(Target::Bake);
    let coord = cx.uv();
    let zero = cx.constant(0.0);
    let _ = cx.emit(Op::Compose, [coord, zero]);
    let error = cx
        .finish([("out".to_string(), zero)])
        .expect_err("a compose operand is not a scalar");
    assert!(error.to_string().contains("Vec2"), "{error}");

    // A Vec2 meeting a Vec3 in a component-wise op: a scalar broadcasts to its
    // partner's width, but these two convert to one another in neither
    // direction, and the wider would read a lane nobody wrote.
    let mut cx = Lowering::new(Target::Bake);
    let coord = cx.uv();
    let world = cx.emit(Op::WorldPos, []);
    let sum = cx.emit(Op::Add, [coord, world]);
    let error = cx
        .finish([("out".to_string(), sum)])
        .expect_err("a Vec2 and a Vec3 do not meet");
    let reason = error.to_string();
    assert!(
        reason.contains("Vec2") && reason.contains("Vec3"),
        "the reason names both widths: {reason}"
    );
}

#[test]
fn the_lattice_hash_is_the_showcase_hash_bit_for_bit() {
    for (x, y) in [
        (0, 0),
        (1, 0),
        (0, 1),
        (7, 13),
        (127, 127),
        (1024, 4096),
        (u32::MAX, 1),
        (3, u32::MAX),
    ] {
        assert_eq!(
            hash2_bits(x, y, 0),
            showcase_hash(x, y),
            "the seedless hash at ({x}, {y})"
        );
        // The normalisation differs from the showcase's only by rounding the
        // division in `f32` instead of `f64`.
        let difference = f64::from(hash2(x, y, 0)) - showcase_value(x, y);
        assert!(difference.abs() < 1e-6, "normalised at ({x}, {y})");
        assert!(
            (0.0..=1.0).contains(&hash2(x, y, 0)),
            "a hash is a value in the unit range"
        );
    }
    // A seed moves the whole field, and the third axis is a field of its own.
    assert_ne!(hash2_bits(7, 13, 0), hash2_bits(7, 13, 1));
    assert_eq!(hash3_bits(7, 13, 0, 0), hash2_bits(7, 13, 0));
    assert_ne!(hash3_bits(7, 13, 1, 0), hash3_bits(7, 13, 2, 0));
    assert_eq!(normalise(0), 0.0);
    assert!(close(normalise(u32::MAX), 1.0));
}

#[test]
fn a_hashed_lattice_meets_itself_at_the_seam() {
    // The op the noises will be built on: a coordinate scaled to a lattice,
    // and the period that lattice wraps at.
    let hashed = |u: f32, v: f32| {
        let mut cx = Lowering::new(Target::Bake);
        let coord = cx.uv();
        let eight = cx.constant(8.0);
        let lattice = cx.emit(Op::Mul, [coord, eight]);
        let period = cx.vector(&[eight, eight]);
        let value = cx.emit(Op::Hash2(0), [lattice, period]);
        let ir = cx.finish([("out".to_string(), value)]).unwrap();
        Interpreter::new(&ir)
            .eval_float([u, v], &Inputs::default(), ir.root("out").unwrap())
            .unwrap()
    };
    // Cell (6, 2) of the lattice, which is what the coordinate lands in.
    assert!(close(hashed(0.75, 0.25), hash2(6, 2, 0)));
    // The wrap is the whole point: the far edge of the repeat is the near one.
    for coordinate in [0.0, 0.13, 0.37, 0.99] {
        assert_eq!(hashed(1.0, coordinate), hashed(0.0, coordinate));
        assert_eq!(hashed(coordinate, 1.0), hashed(coordinate, 0.0));
    }
}

#[test]
fn a_sample_of_a_plane_nothing_rasterised_says_so() {
    let mut cx = Lowering::new(Target::Bake);
    let coord = cx.uv();
    let u = cx.emit(Op::Extract(0), [coord]);
    let buffer = cx.buffer(u, Filter::Blur { radius: 0.01 }, Some(256));
    let sampled = cx.sample(buffer, coord);
    let ir = cx.finish([("roughness".to_string(), sampled)]).unwrap();

    // The plan survives lowering, and so does the expression it will rasterise.
    assert_eq!(ir.buffers().len(), 1);
    assert_eq!(ir.buffers()[0].filter, Filter::Blur { radius: 0.01 });
    assert_eq!(ir.buffers()[0].resolution, Some(256));
    assert_eq!(ir.type_of(ir.buffers()[0].root), Some(IrType::Float));

    // An expression built by hand is at no node, so the plan's path is empty;
    // a plan a graph made carries `nodes[id]`, and that is what a backend that
    // cannot run the filter reports at.
    assert_eq!(ir.buffers()[0].path, "");

    // And the planes are a bake's to hand in: with none, the sample says so
    // rather than reading zero.
    let error = Interpreter::new(&ir)
        .eval_float(UV, &Inputs::default(), ir.root("roughness").unwrap())
        .unwrap_err();
    assert_eq!(error, EvalError::Unbaked(0));
}

#[test]
fn a_plane_is_sampled_bilinearly_and_wraps() {
    // Four texels: 1 at the origin and 0 elsewhere. A texel's value belongs to
    // the centre of its cell, so that is where it reads back exactly.
    let plane = Plane::new(2, IrType::Float, vec![1.0, 0.0, 0.0, 0.0]);
    assert_eq!(plane.sample([0.25, 0.25]), [1.0, 0.0, 0.0]);
    assert_eq!(plane.sample([0.75, 0.25]), [0.0, 0.0, 0.0]);

    // Halfway between two texel centres is halfway between their values.
    assert!((plane.sample([0.5, 0.25])[0] - 0.5).abs() < 1e-6);

    // And a coordinate outside the unit is the same field again: the texel
    // before the origin is the last one, which is what makes a filter over a
    // plane tile.
    assert_eq!(plane.sample([1.25, 0.25]), plane.sample([0.25, 0.25]));
    assert_eq!(plane.sample([-0.75, 0.25]), plane.sample([0.25, 0.25]));
    assert_eq!(plane.texel(-1, -1), plane.texel(1, 1));

    // A colour plane keeps its three lanes, and a scalar one leaves the other
    // two at zero whatever it was asked for.
    let colour = Plane::new(1, IrType::Vec3, vec![0.25, 0.5, 0.75]);
    assert_eq!(colour.sample([0.5, 0.5]), [0.25, 0.5, 0.75]);
    assert_eq!(colour.texels(), 1);
}

#[test]
fn a_live_parameter_with_no_value_is_an_error_and_not_a_guess() {
    let mut cx = Lowering::new(Target::Shader {
        live: vec!["wear".to_string()],
    });
    let param = cx.emit(Op::Param(3), []);
    let ir = cx.finish([("roughness".to_string(), param)]).unwrap();
    let error = Interpreter::new(&ir)
        .eval_float(UV, &Inputs::default(), ir.root("roughness").unwrap())
        .unwrap_err();
    assert_eq!(error, EvalError::MissingParam(3, 0));
}

/// Every op the interpreter is expected to answer for, with its arity.
///
/// [`Op::Sample`] is absent: its plane is phase two, and it answers an error
/// by design.
fn operators() -> Vec<(Op, usize)> {
    vec![
        (Op::Const(0.5), 0),
        (Op::Uv, 0),
        (Op::Param(0), 0),
        (Op::Time, 0),
        (Op::WorldPos, 0),
        (Op::WorldNormal, 0),
        (Op::CutFlag, 0),
        (Op::Add, 2),
        (Op::Sub, 2),
        (Op::Mul, 2),
        (Op::Div, 2),
        (Op::Min, 2),
        (Op::Max, 2),
        (Op::Abs, 1),
        (Op::Floor, 1),
        (Op::Fract, 1),
        (Op::Sqrt, 1),
        (Op::Pow, 2),
        (Op::Exp2, 1),
        (Op::Log2, 1),
        (Op::Sin, 1),
        (Op::Cos, 1),
        (Op::Atan2, 2),
        (Op::Mix, 3),
        (Op::Step, 2),
        (Op::Smoothstep, 3),
        (Op::Clamp, 3),
        (Op::Select, 3),
        (Op::Hash2(17), 2),
        (Op::Hash3(17), 2),
        (Op::Length, 1),
        (Op::Dot, 2),
        (Op::Normalize, 1),
        (Op::Compose, 2),
        (Op::Extract(1), 1),
        // Out of range on purpose: the type system forbids it, so the
        // interpreter is what has to not fall over when something else emits it.
        (Op::Extract(7), 1),
    ]
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    /// Whatever reaches an operator, the answer is an answer.
    ///
    /// The operands are the texel coordinate and the frame inputs rather than
    /// constants, so nothing folds at lowering and the interpreter is what is
    /// under test. A NaN or an infinity out is allowed — `sqrt` of a negative
    /// is a NaN in every language — but a panic or a stuck evaluation is not.
    #[test]
    fn the_interpreter_answers_for_every_operator_on_any_finite_input(
        u in -1.0e6_f32..1.0e6,
        v in -1.0e6_f32..1.0e6,
        time in -1.0e6_f32..1.0e6,
        world in prop::array::uniform3(-1.0e6_f32..1.0e6),
        cut in prop::bool::ANY,
    ) {
        for (op, arity) in operators() {
            let mut cx = Lowering::new(Target::Bake);
            let coord = cx.uv();
            let x = cx.emit(Op::Extract(0), [coord]);
            let y = cx.emit(Op::Extract(1), [coord]);
            let t = cx.emit(Op::Time, []);
            let value = match arity {
                0 => cx.emit(op, []),
                1 => cx.emit(op, [x]),
                2 => cx.emit(op, [x, y]),
                _ => cx.emit(op, [x, y, t]),
            };
            let ir = cx.finish([("out".to_string(), value)]).unwrap();
            let params = [[0.25, 0.5, 0.75]];
            let inputs = Inputs {
                time,
                world_pos: world,
                world_normal: world,
                cut_flag: if cut { 1.0 } else { 0.0 },
                params: &params,
                ..Inputs::default()
            };
            let root = ir.root("out").unwrap();
            let answer = Interpreter::new(&ir).eval([u, v], &inputs, root);
            prop_assert!(answer.is_ok(), "{op:?} at ({u}, {v})");
            // And it is an answer of the width it claims: the lanes the value's
            // type does not own read zero, whatever the operator wrote across
            // the register. Length, Dot, Normalize and Extract read all three
            // and cannot tell a component from a leftover.
            let lanes = answer.unwrap_or_default();
            let owned = ir.type_of(root).map_or(3, IrType::components);
            for (lane, value) in lanes.iter().enumerate().skip(owned) {
                prop_assert_eq!(*value, 0.0, "{:?} lane {} at ({}, {})", op, lane, u, v);
            }
        }
    }
}
