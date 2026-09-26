//! Lowering a validated material into the IR a backend runs.
//!
//! The interesting claims are what the lowering leaves out: a baked parameter
//! leaves no instruction to read it, a sub-expression two nodes share is one
//! instruction, and a node nothing reads is not there at all. Counting
//! instructions is the only way to assert that, so these tests count.
#![allow(
    clippy::unwrap_used,
    reason = "fixtures built in the test; a failure is a test failure"
)]
use ashlar_material::{
    Channel, Input, Material, MaterialGraph, MaterialGraphLibrary, MathOp, Node, Param, ParamValue,
    PbrOutput, SurfaceOutput, Value,
    interp::{Inputs, Interpreter},
    ir::{Ir, IrType, Lowering, Op, Target, ValueId, ir_hash, lower},
    luminance,
    nodes::{Bricks, Decompose, GraphInput, Math, Noise, Subgraph, Uv, Voronoi, Warp},
};

/// A material whose roughness is the u coordinate and nothing else.
fn ramp() -> Material {
    MaterialGraph::builder("test:ramp")
        .node("uv", Uv::new())
        .node("u", Decompose::new("uv", Channel::R))
        .output(PbrOutput::new().roughness("u"))
        .build()
        .unwrap()
}

/// The value of a root at one texel, with no live parameters.
fn value_at(ir: &Ir, port: &str, uv: [f32; 2]) -> f32 {
    let root = ir.root(port).expect("a bound output");
    Interpreter::new(ir)
        .eval_float(uv, &Inputs::default(), root)
        .unwrap()
}

/// The constant a value holds, for the folding cases.
fn constant(ir: &Ir, value: ValueId) -> f32 {
    match ir.inst(value).expect("a value").op {
        Op::Const(constant) => constant,
        other => panic!("expected a constant, found {other:?}"),
    }
}

/// How many instructions of one shape the expression holds.
fn count(ir: &Ir, matches: impl Fn(Op) -> bool) -> usize {
    ir.insts().iter().filter(|inst| matches(inst.op)).count()
}

#[test]
fn a_graph_lowers_to_an_expression_the_interpreter_runs() {
    let material = ramp();
    let ir = lower(&material, Target::Bake).unwrap();
    // Uv, the channel of it, the grey literal, the colour built from it, and
    // the two remaining output constants. Nothing else survives.
    assert_eq!(ir.len(), 6, "{:#?}", ir.insts());
    for uv in [[0.0, 0.0], [0.25, 0.5], [0.75, 0.125]] {
        assert!((value_at(&ir, "roughness", uv) - uv[0]).abs() < 1e-6);
    }
    // The output ports are the roots, and they carry the widths the renderer
    // needs: a colour is three lanes, everything else is one.
    assert_eq!(
        ir.type_of(ir.root("base_color").unwrap()),
        Some(IrType::Vec3)
    );
    assert_eq!(
        ir.type_of(ir.root("roughness").unwrap()),
        Some(IrType::Float)
    );
    assert_eq!(ir.root("height"), None, "an unbound output has no root");
    assert!(ir.buffers().is_empty());
    assert_eq!(ir.params(), &[]);
}

#[test]
fn a_baked_parameter_folds_away_and_a_live_one_stays_a_uniform() {
    let material = MaterialGraph::builder("test:params")
        .param(Param::float("wear", 0.25).range(0.0, 1.0).live())
        .param(Param::color("tint", [0.2, 0.4, 0.6]))
        .output(
            PbrOutput::new()
                .base_color(Input::param("tint"))
                .roughness(Input::param("wear")),
        )
        .build()
        .unwrap();

    // A bake has no uniforms, so every parameter is a constant by the time the
    // expression exists, live or not.
    let baked = lower(&material, Target::Bake).unwrap();
    assert_eq!(count(&baked, |op| matches!(op, Op::Param(_))), 0);
    assert!(baked.params().is_empty());
    assert!((constant(&baked, baked.root("roughness").unwrap()) - 0.25).abs() < 1e-7);

    // A shader keeps what the author declared live, and only that.
    let live = lower(&material, Target::shader_for(&material)).unwrap();
    assert_eq!(count(&live, |op| matches!(op, Op::Param(_))), 1);
    assert_eq!(live.params().len(), 1);
    assert_eq!(live.params()[0].name, "wear");
    assert_eq!(live.params()[0].value_type, IrType::Float);
    let roughness = live.root("roughness").unwrap();
    assert_eq!(live.inst(roughness).unwrap().op, Op::Param(0));
    // The uniform is read, not folded: the value comes from the caller.
    let interpreter = Interpreter::new(&live);
    let inputs = Inputs {
        params: &[[0.75, 0.0, 0.0]],
        ..Inputs::default()
    };
    let value = interpreter
        .eval_float([0.0, 0.0], &inputs, roughness)
        .unwrap();
    assert!((value - 0.75).abs() < 1e-7);
    // The colour parameter folded, because it is not in the live set.
    assert_eq!(
        live.type_of(live.root("base_color").unwrap()),
        Some(IrType::Vec3)
    );
    assert_eq!(count(&live, |op| matches!(op, Op::Param(_))), 1);

    // A colour kept live is a three-lane uniform.
    let tinted = lower(
        &material,
        Target::Shader {
            live: vec!["tint".to_string()],
        },
    )
    .unwrap();
    assert_eq!(tinted.params()[0].value_type, IrType::Vec3);
    assert_eq!(
        tinted.inst(tinted.root("base_color").unwrap()).unwrap().op,
        Op::Param(0)
    );
}

#[test]
fn one_sub_expression_is_emitted_once_however_many_nodes_read_it() {
    let shared = MaterialGraph::builder("test:shared")
        .node("uv", Uv::new())
        .node("u", Decompose::new("uv", Channel::R))
        .node("again", Decompose::new("uv", Channel::R))
        .output(
            PbrOutput::new()
                .base_color(0.5)
                .roughness("u")
                .metallic("again")
                .occlusion(1.0),
        )
        .build()
        .unwrap();
    let ir = lower(&shared, Target::Bake).unwrap();
    assert_eq!(
        ir.root("roughness"),
        ir.root("metallic"),
        "two nodes computing the same channel are one value"
    );
    assert_eq!(count(&ir, |op| matches!(op, Op::Extract(_))), 1);
    assert_eq!(count(&ir, |op| op == Op::Uv), 1);
    assert_eq!(ir.len(), 5, "{:#?}", ir.insts());

    // A different channel of the same coordinate is a different value, so the
    // merging above is about the expression and not about the node count.
    let both = MaterialGraph::builder("test:both")
        .node("uv", Uv::new())
        .node("u", Decompose::new("uv", Channel::R))
        .node("v", Decompose::new("uv", Channel::G))
        .output(
            PbrOutput::new()
                .base_color(0.5)
                .roughness("u")
                .metallic("v")
                .occlusion(1.0),
        )
        .build()
        .unwrap();
    let ir = lower(&both, Target::Bake).unwrap();
    assert_ne!(ir.root("roughness"), ir.root("metallic"));
    assert_eq!(ir.len(), 6);
}

#[test]
fn what_no_output_reaches_is_not_in_the_expression() {
    let graph = |unread: bool| {
        let mut builder = MaterialGraph::builder("test:dead")
            .node("uv", Uv::new())
            .node("u", Decompose::new("uv", Channel::R));
        if unread {
            // Validation allows a node nothing reads; lowering drops it.
            builder = builder.node("spare", Decompose::new("uv", Channel::G));
        }
        builder
            .output(PbrOutput::new().roughness("u"))
            .build()
            .unwrap()
    };
    let lean = lower(&graph(false), Target::Bake).unwrap();
    let with_spare = lower(&graph(true), Target::Bake).unwrap();
    assert_eq!(with_spare.len(), lean.len());
    assert_eq!(
        count(&with_spare, |op| op == Op::Extract(1)),
        0,
        "the unread channel was never needed"
    );
    assert_eq!(ir_hash(&lean), ir_hash(&with_spare));
}

#[test]
fn resampling_only_emits_the_sources_dependencies() {
    let mut cx = Lowering::new(Target::Bake);
    let uv = cx.uv();
    let u = cx.emit(Op::Extract(0), [uv]);
    let v = cx.emit(Op::Extract(1), [uv]);
    let mut unrelated = v;
    for _ in 0..64 {
        unrelated = cx.emit(Op::Mul, [unrelated, v]);
    }
    let source = cx.emit(Op::Mul, [u, u]);
    let coordinate = cx.value(Value::Vec2([0.25, 0.75]));
    let sampled = cx.substitute(source, coordinate);
    assert!(
        sampled.index() <= coordinate.index() + 2,
        "resampling a square must not copy the unrelated expression"
    );
    let ir = cx.finish([("roughness".to_string(), sampled)]).unwrap();
    assert!((value_at(&ir, "roughness", [0.0, 0.0]) - 0.0625).abs() < 1e-7);
}

#[test]
fn arithmetic_on_constants_happens_once_at_lowering() {
    let mut cx = Lowering::new(Target::Bake);
    let two = cx.constant(2.0);
    let three = cx.constant(3.0);
    let sum = cx.emit(Op::Add, [two, three]);
    let scaled = cx.emit(Op::Mul, [sum, three]);
    let ir = cx.finish([("roughness".to_string(), scaled)]).unwrap();
    assert_eq!(ir.len(), 1, "one constant, and the operands are gone");
    assert!((constant(&ir, ir.root("roughness").unwrap()) - 15.0).abs() < 1e-7);

    // A colour literal reaching a float port is a luminance, and a luminance
    // of a constant is a constant. The colour is asymmetric on purpose: white
    // would pass under any weights that sum to one, and these are Rec. 709's.
    let colour = [0.2, 0.5, 0.9];
    let mut cx = Lowering::new(Target::Bake);
    let value = cx.value(Value::Color(colour));
    let float = cx.convert(value, IrType::Float);
    let ir = cx.finish([("roughness".to_string(), float)]).unwrap();
    assert_eq!(ir.len(), 1);
    let folded = constant(&ir, ir.root("roughness").unwrap());
    assert!(
        (folded - luminance(colour)).abs() < 1e-6,
        "folded {folded}, and the graph says {}",
        luminance(colour)
    );
    assert!((folded - 0.465_1).abs() < 1e-4, "green carries most of it");
}

#[test]
fn a_colour_displacing_a_warp_converts_rather_than_refusing_to_lower() {
    // A validated material lowers, or names the node whose lowering has not
    // landed. `Warp.offset` takes a displacement, which admits anything that
    // reaches a float, so a colour reaches it — and a colour the backend had
    // no conversion for would be a graph validation passed and nothing can run.
    let material = MaterialGraph::builder("test:warp")
        .node(
            "w",
            Warp::new(
                Input::Const(Value::Float(0.5)),
                Input::Const(Value::Color([0.1, 0.2, 0.3])),
            ),
        )
        .output(PbrOutput::new().roughness("w"))
        .build()
        .unwrap();
    // Warp lowers, so the claim is what it computes: the source is a constant,
    // so the warp answers that constant wherever it read it, and what is under
    // test is that the colour on the offset port reached the lowering at all
    // rather than being refused as a conversion no port allows.
    let ir = lower(&material, Target::Bake).expect("a validated graph lowers");
    let roughness = ir.root("roughness").expect("roughness is always bound");
    let value = Interpreter::new(&ir)
        .eval_float([0.3, 0.7], &Inputs::default(), roughness)
        .unwrap();
    assert!((value - 0.5).abs() < 1e-6, "{value}");

    // And that conversion is the graph's two in a row: the colour's luminance,
    // along both axes.
    let colour = [0.1, 0.2, 0.3];
    let mut cx = Lowering::new(Target::Bake);
    let value = cx.value(Value::Color(colour));
    let offset = cx.convert(value, IrType::Vec2);
    let ir = cx.finish([("offset".to_string(), offset)]).unwrap();
    let root = ir.root("offset").unwrap();
    assert_eq!(ir.type_of(root), Some(IrType::Vec2));
    let displaced = Interpreter::new(&ir)
        .eval([0.0, 0.0], &Inputs::default(), root)
        .unwrap();
    for (axis, amount) in displaced.iter().enumerate().take(2) {
        assert!(
            (amount - luminance(colour)).abs() < 1e-6,
            "axis {axis} displaced by {amount}"
        );
    }
}

#[test]
fn the_conversions_are_the_two_the_graph_allows_and_whatever_they_chain_to() {
    // The whole table, in one place: every width that can arrive at a port
    // against every width a port can ask for. A `Vec2` converts to nothing,
    // which is what keeps a UV from quietly becoming a colour; everything else
    // is a broadcast, a luminance, or the two of them in a row.
    let widths = [IrType::Float, IrType::Vec2, IrType::Vec3];
    for from in widths {
        for to in widths {
            let mut cx = Lowering::new(Target::Bake);
            let zero = cx.constant(0.0);
            let value = match from {
                IrType::Float => zero,
                IrType::Vec2 => cx.vector(&[zero, zero]),
                IrType::Vec3 => cx.vector(&[zero, zero, zero]),
            };
            let converted = cx.convert(value, to);
            let answer = cx.finish([("out".to_string(), converted)]);
            assert_eq!(
                answer.is_ok(),
                from == to || from != IrType::Vec2,
                "{from} to {to}: {answer:?}"
            );
            if let Ok(ir) = answer {
                assert_eq!(ir.type_of(ir.root("out").unwrap()), Some(to));
            }
        }
    }
}

#[test]
fn a_cache_key_is_what_the_expression_is_and_not_what_it_was_asked_for() {
    let material = MaterialGraph::builder("test:live")
        .param(Param::float("wear", 0.5).live())
        .param(Param::color("tint", [0.2, 0.4, 0.6]).live())
        .output(
            PbrOutput::new()
                .base_color(Input::param("tint"))
                .roughness(Input::param("wear")),
        )
        .build()
        .unwrap();
    let shader = |live: &[&str]| {
        let live = live.iter().map(|name| (*name).to_string()).collect();
        lower(&material, Target::Shader { live }).unwrap()
    };
    let both = shader(&["wear", "tint"]);
    // The uniform block is in the graph's declaration order whatever order the
    // live set named them, so these are one expression and must be one entry.
    assert_eq!(ir_hash(&both), ir_hash(&shader(&["tint", "wear"])));
    // A live name no parameter answers to binds nothing and emits nothing.
    assert_eq!(
        ir_hash(&both),
        ir_hash(&shader(&["wear", "tint", "absent"]))
    );
    // What does move the key is a parameter that stopped being a uniform.
    assert_ne!(ir_hash(&both), ir_hash(&shader(&["wear"])));
}

#[test]
fn a_scalar_meeting_a_vector_is_broadcast_rather_than_read_lane_by_lane() {
    let mut cx = Lowering::new(Target::Bake);
    let uv = cx.uv();
    let two = cx.constant(2.0);
    let scaled = cx.emit(Op::Mul, [two, uv]);
    let ir = cx.finish([("scaled".to_string(), scaled)]).unwrap();
    let root = ir.root("scaled").unwrap();
    assert_eq!(ir.type_of(root), Some(IrType::Vec2));
    let value = Interpreter::new(&ir)
        .eval([0.3, 0.4], &Inputs::default(), root)
        .unwrap();
    assert!((value[0] - 0.6).abs() < 1e-6);
    assert!(
        (value[1] - 0.8).abs() < 1e-6,
        "the second lane is v, not zero"
    );
}

#[test]
fn the_hash_of_an_expression_is_stable_and_moves_with_what_it_computes() {
    let first = lower(&ramp(), Target::Bake).unwrap();
    let second = lower(&ramp(), Target::Bake).unwrap();
    assert_eq!(ir_hash(&first), ir_hash(&second));
    // Written down rather than only compared, because a key that is stable
    // within one process is not a cache key: this is the same number in every
    // process on every machine, and nothing about where a hash map put things
    // reaches it. It changes when the encoding or this lowering changes, which
    // is exactly when a cache written by an older build must be missed.
    assert_eq!(ir_hash(&first), 0xffb6_75cb_3732_4050);
    assert_eq!(
        first, second,
        "two lowerings of one graph are one expression"
    );

    let metallic = MaterialGraph::builder("test:ramp")
        .node("uv", Uv::new())
        .node("u", Decompose::new("uv", Channel::R))
        .output(PbrOutput::new().roughness("u").metallic(0.25))
        .build()
        .unwrap();
    let changed = lower(&metallic, Target::Bake).unwrap();
    assert_ne!(
        ir_hash(&first),
        ir_hash(&changed),
        "a constant is part of what a cache key stands for"
    );

    // The target is part of the key: the same graph baked and compiled are two
    // different things to cache.
    let live = MaterialGraph::builder("test:live")
        .param(Param::float("wear", 0.5).live())
        .output(PbrOutput::new().roughness(Input::param("wear")))
        .build()
        .unwrap();
    assert_ne!(
        ir_hash(&lower(&live, Target::Bake).unwrap()),
        ir_hash(&lower(&live, Target::shader_for(&live)).unwrap())
    );
}

#[test]
fn a_setting_this_backend_cannot_lower_is_rejected_by_path() {
    // Every node kind in the vocabulary lowers, so what is left to refuse is a
    // *setting* of one, and the claim here is about the shape of that refusal:
    // the node that asked is named, at the path validation would have used,
    // rather than the picture quietly being something else.
    let refused = |id: &str, node: Node| {
        let material = MaterialGraph::builder("test:refused")
            .node(id, node)
            .output(PbrOutput::new().roughness(id))
            .build()
            .unwrap();
        let error = lower(&material, Target::Bake).unwrap_err();
        assert_eq!(error.path, format!("nodes[{id}]"));
        error.reason
    };

    // A simplex noise, which is the one refusal in the crate that is not a
    // backend waiting its turn: the lattice has no integer period at all, so
    // the reason says why rather than "yet".
    let reason = refused("grain", Noise::simplex().period(8).into());
    assert!(
        reason.contains("no integer period"),
        "the reason says why a simplex cannot tile: {reason}"
    );
    assert!(
        !reason.contains("yet"),
        "and does not promise one: {reason}"
    );

    // And one that is: a brick corner this backend has no expression for.
    assert_eq!(
        refused("wall", Bricks::new().round(0.2).into()),
        "rounded brick corners have no lowering yet"
    );

    // A `Voronoi`, which was one of these until the second node set landed,
    // now lowers; the counterpart to a refusal naming the node is that a node
    // that lowers is never named.
    let cells = MaterialGraph::builder("test:cells")
        .node("cells", Voronoi::new().period(8))
        .output(PbrOutput::new().roughness("cells"))
        .build()
        .unwrap();
    assert!(lower(&cells, Target::Bake).is_ok());
}

#[test]
fn a_parameter_widens_the_way_the_graph_says_it_does() {
    // An Int parameter is a Float in the graph, and so it is in the IR.
    let material = MaterialGraph::builder("test:seed")
        .param(Param::int("seed", 7))
        .param(Param::bool("rough", true))
        .output(
            PbrOutput::new()
                .roughness(Input::param("seed"))
                .metallic(Input::param("rough")),
        )
        .build()
        .unwrap();
    let ir = lower(&material, Target::Bake).unwrap();
    assert!((constant(&ir, ir.root("roughness").unwrap()) - 7.0).abs() < 1e-7);
    assert!((constant(&ir, ir.root("metallic").unwrap()) - 1.0).abs() < 1e-7);
    assert_eq!(
        material.param("seed").map(|p| p.value),
        Some(ParamValue::Int(7))
    );
}

/// A library holding one compound: a signal input, one distinctive instruction
/// over it, and the input itself exported as a mask.
///
/// The square root is there to be counted. Nothing else in these graphs takes
/// one, so the number of `Op::Sqrt` instructions in a lowering is the number of
/// times the instance was emitted, which is the claim both cases below make.
fn compounds() -> MaterialGraphLibrary {
    let mut library = MaterialGraphLibrary::default();
    library.insert(
        MaterialGraph::builder("test:compound")
            .node("wear", GraphInput::float("wear", 0.25))
            .node("root", Math::unary(MathOp::Sqrt, "wear"))
            .output(PbrOutput::new().roughness("root").extra("mask", "wear"))
            .into_graph(),
    );
    library
}

/// How many square roots a lowering holds, which is how many times the
/// compound above was written into it.
fn instances(ir: &Ir) -> usize {
    count(ir, |op| matches!(op, Op::Sqrt))
}

#[test]
fn two_nodes_reading_one_instance_lower_it_once() {
    let library = compounds();
    // One node, reading the roughness the compound computes.
    let one = MaterialGraph::builder("test:one")
        .node("grain", Noise::value().period(8))
        .node(
            "a",
            Subgraph::new("test:compound")
                .input("wear", "grain")
                .output(SurfaceOutput::Roughness),
        )
        .output(PbrOutput::new().roughness("a"))
        .into_graph()
        .build_in(&library)
        .unwrap();
    // Two nodes on the same binding, one reading that roughness and one the
    // mask the compound exports beside it. The instance is the same instance,
    // so the expression is the same expression: what the second node adds is
    // the one instruction that reads its other output.
    let two = MaterialGraph::builder("test:two")
        .node("grain", Noise::value().period(8))
        .node(
            "a",
            Subgraph::new("test:compound")
                .input("wear", "grain")
                .output(SurfaceOutput::Roughness),
        )
        .node(
            "b",
            Subgraph::new("test:compound")
                .input("wear", "grain")
                .output(SurfaceOutput::Extra("mask".to_owned())),
        )
        .node("sum", Math::new(MathOp::Add, "a", "b"))
        .output(PbrOutput::new().roughness("sum"))
        .into_graph()
        .build_in(&library)
        .unwrap();

    let one = lower(&one, Target::Bake).unwrap();
    let two = lower(&two, Target::Bake).unwrap();
    assert_eq!(instances(&one), 1);
    assert_eq!(instances(&two), 1, "the instance was emitted twice");
    // And the whole expression is one instruction longer: the addition, over
    // two values the shared prefix had already computed.
    assert_eq!(two.len(), one.len() + 1, "{:#?}", two.insts());
}

#[test]
fn two_bindings_of_one_graph_are_two_instances() {
    let library = compounds();
    // The same key, bound to two fields. A compound is inferred under what is
    // wired into it, so these are two graphs as far as the lowering is
    // concerned, and each is written out in full.
    let material = MaterialGraph::builder("test:pair")
        .node("grain", Noise::value().period(8))
        .node("other", Noise::value().period(4).seed(3))
        .node(
            "a",
            Subgraph::new("test:compound")
                .input("wear", "grain")
                .output(SurfaceOutput::Roughness),
        )
        .node(
            "b",
            Subgraph::new("test:compound")
                .input("wear", "other")
                .output(SurfaceOutput::Roughness),
        )
        .node("sum", Math::new(MathOp::Add, "a", "b"))
        .output(PbrOutput::new().roughness("sum"))
        .into_graph()
        .build_in(&library)
        .unwrap();
    let ir = lower(&material, Target::Bake).unwrap();
    assert_eq!(instances(&ir), 2);
    // Which is the point of specialising: the two roots are different fields,
    // and a cache keyed by the graph alone would have handed the second node
    // the first one's expression.
    let at = |uv| value_at(&ir, "roughness", uv);
    assert!((at([0.3, 0.7]) - at([0.6, 0.1])).abs() > 1e-3);
}
