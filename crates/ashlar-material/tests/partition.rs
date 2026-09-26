//! The shader partition: what is cut, what is bound, and what it costs.
//!
//! The claims here are all counts, because the partition's whole reason to
//! exist is that an author can see what a live parameter costs before it ships.
//! So every test says a number and says why that number, and the numbers the
//! shader side is measured against are the ones the full lowering gives for the
//! same graph.
#![allow(
    clippy::unwrap_used,
    reason = "fixtures built in the test; a failure is a test failure"
)]
use std::collections::{BTreeMap, BTreeSet};
use std::num::NonZeroUsize;

use ashlar_material::{
    BlendMode, Channel, Exposure, Input, Material, MaterialGraph, MathOp, Param, PbrOutput, Period,
    bake::PlaneFormat,
    interp::{Inputs, Interpreter, Plane},
    ir::{BufferId, Filter, Ir, Op, Target, ValueId},
    nodes::{
        Blend, Blur, Colorize, Curvature, CutFlag, Decompose, Distance, EdgeDetect, Levels, Math,
        Mix, Noise, NormalFromHeight, OcclusionFromHeight, Time, Triplanar, Warp, WorldMask,
        WorldNormal, WorldPos,
    },
    partition::{BoundTexture, MAX_BOUND_TEXTURES, OutputSource, Partition, partition},
    planes::{BakeCache, rasterise_buffers},
};

/// How many threads a rasterisation here divides its rows across.
///
/// Named rather than left to the default, and bounded: several of these run at
/// once under one test binary, and a bake that asked for the whole machine each
/// time would be slower for it. The bytes are the same whatever the number.
const THREADS: usize = 8;

/// The resolution every partition here binds its textures at. The smallest a
/// bake takes, because nothing here rasterises a texel: only the tap epsilon
/// depends on it, and that is asserted directly.
const RESOLUTION: u32 = 256;

/// Every parameter the graph declares, live.
fn everything(material: &Material) -> Target {
    Target::Shader {
        live: material
            .graph()
            .params
            .iter()
            .map(|param| param.name.clone())
            .collect(),
    }
}

/// Nothing live at all.
fn nothing() -> Target {
    Target::Shader { live: Vec::new() }
}

/// The instructions the fragment actually evaluates.
///
/// Not the whole arena: the partition carries the static sub-expressions it cut
/// as the roots of its planes, so counting instructions rather than what the
/// runtime outputs reach would count the bake as well as the shader.
fn fragment(split: &Partition) -> Vec<Op> {
    let roots: Vec<ValueId> = split
        .outputs()
        .values()
        .filter_map(|source| match source {
            OutputSource::Runtime(root) => Some(*root),
            _ => None,
        })
        .collect();
    let live = split.runtime().reaches(&roots);
    split
        .runtime()
        .insts()
        .iter()
        .enumerate()
        .filter(|(index, _)| live.get(*index).copied() == Some(true))
        .map(|(_, inst)| inst.op)
        .collect()
}

/// How many of the fragment's instructions are of one shape.
fn count(split: &Partition, matches: impl Fn(Op) -> bool) -> usize {
    fragment(split)
        .into_iter()
        .filter(|op| matches(*op))
        .count()
}

/// The ops that are arithmetic rather than a source or a fetch: what "one op"
/// means when an author says a live parameter costs one blend.
fn arithmetic(split: &Partition) -> usize {
    count(split, |op| {
        !matches!(
            op,
            Op::Const(_)
                | Op::Uv
                | Op::Param(_)
                | Op::Time
                | Op::WorldPos
                | Op::WorldNormal
                | Op::CutFlag
                | Op::Sample(_)
        )
    })
}

/// Every filter the partition bound a plane for.
fn filters(split: &Partition) -> Vec<Filter> {
    split
        .textures()
        .iter()
        .map(|texture| texture.filter)
        .collect()
}

/// A filter with a slope walk's guide forgotten: which plane it walks down is
/// a place in one lowering's own numbering rather than part of what it does.
fn unguided(filter: Filter) -> Filter {
    match filter {
        Filter::Slope {
            radius,
            steps,
            mode,
            guide: _,
        } => Filter::Slope {
            radius,
            steps,
            mode,
            guide: BufferId::default(),
        },
        other => other,
    }
}

/// A concrete-ish surface: one noise, one output, and a wear parameter that
/// multiplies the roughness at the very end.
fn at_the_output(live: bool) -> Material {
    let wear = Param::float("wear", 0.5).range(0.0, 1.0);
    MaterialGraph::builder("test:output-param")
        .param(if live { wear.live() } else { wear })
        .node("grain", Noise::value().period(16).octaves(3))
        .node(
            "worn",
            Math::new(MathOp::Mul, "grain", Input::param("wear")),
        )
        .output(PbrOutput::new().base_color("grain").roughness("worn"))
        .build()
        .unwrap()
}

#[test]
fn a_graph_with_no_live_inputs_reports_zero_runtime_ops_and_says_to_bake() {
    let material = at_the_output(false);
    let split = partition(&material, &nothing(), RESOLUTION).unwrap();
    let report = split.report();
    assert_eq!(report.verdict, "no live inputs: bake this");
    assert_eq!(report.ops, 0, "the fragment computes nothing: {report}");
    assert!(split.is_static());
    assert_eq!(report.params, 0);
    assert!(report.frozen.is_empty());
    // Base colour and roughness are textures; the two constant outputs are
    // literals, and a literal binds nothing.
    assert!(matches!(
        split.output("base_color"),
        Some(OutputSource::Texture(_))
    ));
    assert!(matches!(
        split.output("roughness"),
        Some(OutputSource::Texture(_))
    ));
    assert!(matches!(
        split.output("metallic"),
        Some(OutputSource::Inline(_))
    ));
    assert_eq!(report.textures, 2);
    assert_eq!(report.ops_per_output.values().copied().max(), Some(0));
    // The whole graph is still there — it is in the two planes rather than in
    // the fragment — so the partition did not lose the material, only moved it.
    assert_eq!(split.runtime().buffers().len(), 2);
    assert_eq!(report.bindings, 2);
    let baked = lower_bake(&material);
    let planes: usize = split
        .runtime()
        .buffers()
        .iter()
        .map(|plan| {
            split
                .runtime()
                .reaches(&[plan.root])
                .into_iter()
                .filter(|x| *x)
                .count()
        })
        .sum();
    assert!(
        planes >= baked.len() - 4,
        "the graph moved into the planes rather than vanishing: {planes} against {}",
        baked.len()
    );
}

/// The same material lowered for a bake, which is the size a partition is
/// measured against.
fn lower_bake(material: &Material) -> Ir {
    ashlar_material::ir::lower(material, Target::Bake).unwrap()
}

#[test]
fn a_live_parameter_at_the_output_pulls_in_one_op() {
    let material = at_the_output(true);
    let split = partition(&material, &everything(&material), RESOLUTION).unwrap();
    let report = split.report();
    assert_eq!(report.params, 1);
    assert_eq!(split.live(), ["wear"]);
    assert_ne!(report.verdict, "no live inputs: bake this");
    // The multiply, and nothing else: the noise it reads is a texture and the
    // parameter is a uniform.
    assert_eq!(arithmetic(&split), 1, "{report}");
    assert_eq!(
        count(&split, |op| op == Op::Mul),
        1,
        "the one op is the multiply the author wrote"
    );
    assert_eq!(report.ops_per_output["roughness"], 4, "{report}");
    // Base colour did not touch the parameter, so it stayed a texture.
    assert_eq!(report.ops_per_output["base_color"], 0);
    // Three octaves of value noise are not in the fragment.
    assert_eq!(count(&split, |op| matches!(op, Op::Hash2(_))), 0);
    let baked = lower_bake(&material);
    assert!(
        report.ops * 8 < baked.len(),
        "one op against a whole graph: {} against {}",
        report.ops,
        baked.len()
    );
}

#[test]
fn a_live_parameter_at_a_noise_pulls_in_the_noise() {
    // The only way a parameter reaches a generator is through the frame it is
    // read in: a seed and a period are fields of the node, not inputs. So this
    // is the shape the design's "a seed parameter costs the entire graph"
    // warning is actually about.
    let drift = |live: bool| {
        let param = Param::float("drift", 0.25).range(0.0, 1.0);
        MaterialGraph::builder("test:warped")
            .param(if live { param.live() } else { param })
            .node("grain", Noise::value().period(16))
            .node(
                "moved",
                Warp::new("grain", Input::param("drift")).amount(0.1),
            )
            .output(PbrOutput::new().roughness("moved"))
            .build()
            .unwrap()
    };
    let material = drift(true);
    let split = partition(&material, &everything(&material), RESOLUTION).unwrap();
    let report = split.report();
    // One octave of value noise hashes its cell's four corners. They are in the
    // fragment now, because the coordinate they are read at moves.
    assert_eq!(
        count(&split, |op| matches!(op, Op::Hash2(_))),
        4,
        "the noise itself came along: {report}"
    );
    // And it is the whole noise rather than a piece of it: everything the bake
    // lowered is on the runtime side, less the output constants the shader
    // writes as literals.
    let baked = lower_bake(&material);
    assert!(
        report.ops + 8 >= baked.len(),
        "the warp pulled in the graph: {} runtime ops against {} lowered",
        report.ops,
        baked.len()
    );
    assert_eq!(report.textures, 0, "nothing was left to bind: {report}");
    // The same graph with the parameter baked is the opposite answer.
    let folded = drift(false);
    let cold = partition(&folded, &nothing(), RESOLUTION).unwrap();
    assert_eq!(cold.report().ops, 0);
    assert_eq!(cold.report().verdict, "no live inputs: bake this");
}

#[test]
fn every_buffered_node_ends_up_bound() {
    // A blur, a distance and an occlusion, each of which reads a neighbourhood
    // and so can only ever be a plane. A live parameter at the end puts the
    // fragment on the runtime side, so the cut is a real one.
    let material = MaterialGraph::builder("test:filters")
        .param(Param::float("wear", 0.4).range(0.0, 1.0).live())
        .node("grain", Noise::value().period(16))
        .node("soft", Blur::new("grain").radius(0.01))
        .node("mask", Levels::new("grain").in_low(0.6))
        .node("away", Distance::new("mask").threshold(0.5).range(0.05))
        .node("relief", Blend::new(BlendMode::Multiply, "soft", "away"))
        .node("shade", OcclusionFromHeight::new("relief").radius(0.01))
        .node(
            "worn",
            Math::new(MathOp::Mul, "shade", Input::param("wear")),
        )
        .output(
            PbrOutput::new()
                .base_color("relief")
                .roughness("worn")
                .occlusion("shade")
                .height("relief")
                .normal_strength(0.01),
        )
        .build()
        .unwrap();
    let split = partition(&material, &everything(&material), RESOLUTION).unwrap();
    let bound = filters(&split);
    for wanted in [
        Filter::Blur { radius: 0.01 },
        Filter::Distance { threshold: 0.5 },
        Filter::Occlusion {
            radius: 0.01,
            strength: 1.0,
        },
    ] {
        assert!(
            bound.contains(&wanted),
            "{wanted:?} is a plane and must be bound: {bound:?}"
        );
    }
    // Every plane the bake would rasterise is bound here too, and the runtime
    // side holds no neighbourhood at all: a sample is a texture fetch.
    let baked = lower_bake(&material);
    for plan in baked.buffers() {
        assert!(
            bound.contains(&plan.filter),
            "the bake's {:?} was not bound: {bound:?}",
            plan.filter
        );
    }
    // The height is static, so its normal is a bound map derived by the same
    // central difference the bake uses rather than four taps per fragment.
    assert!(bound.contains(&Filter::Normal { strength: 0.01 }));
    assert_eq!(split.report().taps, 0);
    assert!(matches!(
        split.output("normal"),
        Some(OutputSource::Texture(_))
    ));
}

/// A surface built out of the third node set: a directional blur, a slope blur
/// over a second height, a cavity mask and an edge, with a wear parameter at
/// the end that puts the fragment on the runtime side.
fn third_set(live: bool) -> Material {
    let wear = Param::float("wear", 0.4).range(0.0, 1.0);
    MaterialGraph::builder("test:third")
        .param(if live { wear.live() } else { wear })
        .node("grain", Noise::value().period(16))
        .node("hill", Noise::value().period(4).seed(2))
        .node("brushed", Blur::directional("grain", 30.0).radius(0.01))
        .node(
            "worn_mask",
            Blur::slope("grain", "hill").radius(0.02).steps(4),
        )
        .node("hollows", Curvature::cavity("hill").radius(0.02))
        .node("lines", EdgeDetect::new("grain").radius(0.004))
        .node(
            "body",
            Blend::new(BlendMode::Multiply, "brushed", "worn_mask"),
        )
        .node("shaded", Blend::new(BlendMode::Multiply, "body", "hollows"))
        .node("drawn", Blend::new(BlendMode::Multiply, "shaded", "lines"))
        .node(
            "worn",
            Math::new(MathOp::Mul, "drawn", Input::param("wear")),
        )
        .output(PbrOutput::new().base_color("drawn").roughness("worn"))
        .build()
        .unwrap()
}

#[test]
fn the_third_sets_filters_bind_as_planes_and_a_slope_blur_binds_what_it_walks() {
    // The same claim as above over the filters the third node set added, and
    // one more that only a slope blur can make: its guide is a plane too. A
    // guide is named by an index into the plan list and no instruction samples
    // it, so a partition that rebuilt the plans without following the name
    // would leave a filter walking on a plane that is not there — or, worse, on
    // whichever plane took that index.
    let material = third_set(true);
    let split = partition(&material, &everything(&material), RESOLUTION).unwrap();
    let bound = filters(&split);
    assert!(
        bound.iter().any(|filter| matches!(
            filter,
            Filter::Directional { radius, .. } if (*radius - 0.01).abs() < 1e-6
        )),
        "{bound:?}"
    );
    assert!(
        bound
            .iter()
            .any(|filter| matches!(filter, Filter::Curvature { .. })),
        "{bound:?}"
    );
    assert!(
        bound
            .iter()
            .any(|filter| matches!(filter, Filter::Edge { .. })),
        "{bound:?}"
    );

    // The slope blur, and the plane it walks down, in the partition's own plan
    // list: the guide is an earlier plan than the filter that names it, which
    // is what makes rasterising them in order enough.
    let plans = split.runtime().buffers();
    let slope = plans
        .iter()
        .find(|plan| matches!(plan.filter, Filter::Slope { .. }))
        .expect("the slope blur is a plane");
    let Filter::Slope { guide, steps, .. } = slope.filter else {
        unreachable!("found by the pattern above")
    };
    assert_eq!(steps, 4);
    assert!(
        guide.index() < slope.id.index(),
        "{guide} after {}",
        slope.id
    );
    assert!(plans.iter().any(|plan| plan.id == guide), "{guide} is gone");

    // And the picture is the bake's: the planes rasterised through the
    // partition's plans, read at texel centres, are what the folded graph
    // bakes.
    let buffers = planes(&split);
    let runtime = Interpreter::new(split.runtime());
    let inputs = Inputs {
        params: &[[0.4, 0.0, 0.0]],
        buffers: &buffers,
        ..Inputs::default()
    };
    let baked = lower_bake(&third_set(false));
    let reference = rasterise_buffers(
        &baked,
        RESOLUTION,
        NonZeroUsize::new(THREADS),
        &mut BakeCache::new(),
    )
    .unwrap();
    let interpreter = Interpreter::new(&baked);
    let bake = Inputs {
        buffers: &reference,
        ..Inputs::default()
    };
    for (x, y) in [(0, 0), (7, 3), (128, 128), (255, 255)] {
        let uv = centre(x, y, RESOLUTION);
        let live = runtime
            .eval_float(uv, &inputs, split.runtime().root("roughness").unwrap())
            .unwrap();
        let cold = interpreter
            .eval_float(uv, &bake, baked.root("roughness").unwrap())
            .unwrap();
        assert!(
            (live - cold).abs() < 1e-5,
            "at {uv:?} the compiled surface is {live} and the baked one {cold}"
        );
    }
}

#[test]
fn a_parameter_that_reaches_the_height_a_slope_blur_walks_is_frozen_too() {
    // A plane's contents are what decide whether a uniform above it can move,
    // and a slope blur's contents are *two* expressions: the field it smears
    // and the height it walks down. A freeze scan that only followed the first
    // would leave a parameter live whose value is baked into a plane, which is
    // a slider that silently does nothing — the exact thing `frozen` exists to
    // say out loud.
    let material = MaterialGraph::builder("test:frozen-guide")
        .param(Param::float("tilt", 0.3).range(0.0, 1.0).live())
        .node("grain", Noise::value().period(16))
        .node("hill", Noise::value().period(4).seed(2))
        .node(
            "tilted",
            Math::new(MathOp::Mul, "hill", Input::param("tilt")),
        )
        .node("worn", Blur::slope("grain", "tilted").radius(0.02))
        .output(PbrOutput::new().roughness("worn"))
        .build()
        .unwrap();
    let split = partition(&material, &everything(&material), RESOLUTION).unwrap();
    assert_eq!(split.frozen(), ["tilt"]);
    assert!(split.live().is_empty());
    assert!(split.is_static(), "{}", split.report());
}

#[test]
fn a_time_driven_emissive_is_one_multiply_over_a_bound_texture() {
    let material = MaterialGraph::builder("test:strip")
        .node("glow", Noise::value().period(8))
        .node("tint", Levels::new("glow").out_range(0.2, 1.0))
        .node("clock", Time::new())
        .node("pulse", Math::unary(MathOp::Sin, "clock"))
        .node("lit", Math::new(MathOp::Mul, "tint", "pulse"))
        .output(PbrOutput::new().emissive("lit"))
        .build()
        .unwrap();
    let split = partition(&material, &nothing(), RESOLUTION).unwrap();
    let report = split.report();
    // No parameter is live, and the material is still not bakeable: the clock
    // is a runtime input in its own right.
    assert_eq!(report.params, 0);
    assert!(!split.is_static());
    assert_ne!(report.verdict, "no live inputs: bake this");
    // The clock, the turn it is scaled into, the sine, the author's multiply,
    // the texture read and the broadcast to three lanes: eight instructions for
    // a material that pulses.
    assert_eq!(report.ops, 8, "{report}");
    assert_eq!(count(&split, |op| op == Op::Sin), 1);
    assert_eq!(count(&split, |op| op == Op::Time), 1);
    // Two multiplies, and only one of them is the author's: `Math`'s `Sin`
    // takes turns, so the other is the turn scaled into radians.
    assert_eq!(count(&split, |op| op == Op::Mul), 2);
    // One texture: the whole graph above the clock, levels and all.
    assert_eq!(report.textures, 1, "{report}");
    assert_eq!(report.samples, 1);
    assert_eq!(count(&split, |op| matches!(op, Op::Hash2(_))), 0);
    let emissive = split.runtime().buffers().first().unwrap();
    assert_eq!(emissive.filter, Filter::None);
    assert!(matches!(
        split.output("emissive"),
        Some(OutputSource::Runtime(_))
    ));
    // The four constant outputs are literals: a graph that only pulses its
    // emissive binds nothing for its base colour.
    assert!(matches!(
        split.output("base_color"),
        Some(OutputSource::Inline(_))
    ));
}

#[test]
fn the_world_inputs_are_period_neutral_and_reach_the_ops() {
    let material = MaterialGraph::builder("test:world")
        .node("grain", Noise::value().period(8))
        .node("here", WorldPos::new())
        .node("up", WorldNormal::new())
        .node("cut", CutFlag::new())
        .node("height", Decompose::new("here", Channel::G))
        .node("facing", Decompose::new("up", Channel::G))
        .node("grime", Math::new(MathOp::Mul, "height", "facing"))
        .node("mixed", Mix::new("grain", "grime", "cut"))
        .output(PbrOutput::new().roughness("mixed"))
        .build()
        .unwrap();
    // Not a UV field, so it neither repeats nor fails to: the output check sees
    // the unit period a parameter carries and the graph tiles at its noise.
    assert_eq!(material.period(), Period::Tiled { u: 8, v: 8 });
    assert_eq!(material.port("here").unwrap().period, Period::UNIT);
    assert_eq!(material.port("cut").unwrap().period, Period::UNIT);
    let split = partition(&material, &nothing(), RESOLUTION).unwrap();
    assert_eq!(count(&split, |op| op == Op::WorldPos), 1);
    assert_eq!(count(&split, |op| op == Op::WorldNormal), 1);
    assert_eq!(count(&split, |op| op == Op::CutFlag), 1);
    assert!(!split.is_static());
    // The noise is the only thing a texture can hold.
    assert_eq!(split.report().textures, 1);
}

#[test]
fn a_runtime_height_costs_four_taps_of_itself() {
    let material = MaterialGraph::builder("test:live-height")
        .param(Param::float("depth", 0.3).range(0.0, 1.0).live())
        .node("grain", Noise::value().period(16))
        .node(
            "relief",
            Math::new(MathOp::Mul, "grain", Input::param("depth")),
        )
        .output(
            PbrOutput::new()
                .roughness(0.5)
                .height("relief")
                .normal_strength(0.02),
        )
        .build()
        .unwrap();
    let split = partition(&material, &everything(&material), RESOLUTION).unwrap();
    let report = split.report();
    assert_eq!(report.taps, 4, "four offset reads and no more: {report}");
    // The ceiling the report quotes is those four times the height
    // sub-expression, and the actual cost is under it because the copies share.
    let lowered = lower_bake(&material);
    let height = lowered.root("height").unwrap();
    let sub = lowered
        .reaches(&[height])
        .into_iter()
        .filter(|x| *x)
        .count();
    assert_eq!(report.tap_ops, 4 * sub, "{report}");
    assert!(report.ops < report.tap_ops, "{report}");
    // Four offset reads of the bound noise, plus the one the height itself
    // takes.
    assert_eq!(report.samples, 5, "{report}");
    assert!(matches!(
        split.output("normal"),
        Some(OutputSource::Runtime(_))
    ));
    // No normal plane was bound, because there is no height to bake.
    assert!(!filters(&split).contains(&Filter::Normal { strength: 0.02 }));
}

#[test]
fn a_normal_from_height_node_on_the_runtime_side_is_taps_too() {
    let material = MaterialGraph::builder("test:live-normal")
        .param(Param::float("depth", 0.3).range(0.0, 1.0).live())
        .node("grain", Noise::value().period(16))
        .node(
            "relief",
            Math::new(MathOp::Mul, "grain", Input::param("depth")),
        )
        .node("normal", NormalFromHeight::new("relief").strength(0.02))
        .node("shade", Decompose::new("normal", Channel::B))
        .output(PbrOutput::new().roughness("shade"))
        .build()
        .unwrap();
    let split = partition(&material, &everything(&material), RESOLUTION).unwrap();
    assert_eq!(split.report().taps, 4, "{}", split.report());
    assert!(!filters(&split).contains(&Filter::Normal { strength: 0.02 }));
    // And the same node over a static height is a plane, not taps.
    let baked = MaterialGraph::builder("test:baked-normal")
        .node("grain", Noise::value().period(16))
        .node("normal", NormalFromHeight::new("grain").strength(0.02))
        .node("shade", Decompose::new("normal", Channel::B))
        .output(PbrOutput::new().roughness("shade"))
        .build()
        .unwrap();
    let cold = partition(&baked, &nothing(), RESOLUTION).unwrap();
    assert_eq!(cold.report().taps, 0);
    assert!(filters(&cold).contains(&Filter::Normal { strength: 0.02 }));
}

#[test]
fn a_parameter_that_reaches_a_plane_is_frozen_rather_than_refused() {
    let material = MaterialGraph::builder("test:frozen")
        .param(Param::float("cracking", 0.6).range(0.0, 1.0).live())
        .param(Param::float("wear", 0.2).range(0.0, 1.0).live())
        .node("grain", Noise::value().period(16))
        .node(
            "driven",
            Math::new(MathOp::Mul, "grain", Input::param("cracking")),
        )
        .node("mask", Math::new(MathOp::Step, "driven", 0.35))
        .node("away", Distance::new("mask").threshold(0.5).range(0.05))
        .node("worn", Math::new(MathOp::Mul, "away", Input::param("wear")))
        .output(PbrOutput::new().roughness("worn"))
        .build()
        .unwrap();
    let split = partition(&material, &everything(&material), RESOLUTION).unwrap();
    // A jump flood is a plane baked once. The slider that reaches it is folded
    // at its value and named, rather than the graph being refused or the
    // slider silently doing nothing.
    assert_eq!(split.frozen(), ["cracking"]);
    assert_eq!(split.live(), ["wear"]);
    assert_eq!(split.report().params, 1);
    assert_eq!(split.report().frozen, ["cracking"]);
    assert!(split.report().to_string().contains("cracking"));
    assert!(filters(&split).contains(&Filter::Distance { threshold: 0.5 }));
}

#[test]
fn a_runtime_input_above_a_neighbourhood_filter_is_refused_by_path() {
    let material = MaterialGraph::builder("test:clocked-blur")
        .node("grain", Noise::value().period(16))
        .node("clock", Time::new())
        .node("moving", Math::new(MathOp::Mul, "grain", "clock"))
        .node("soft", Blur::new("moving").radius(0.01))
        .output(PbrOutput::new().roughness("soft"))
        .build()
        .unwrap();
    let error = partition(&material, &nothing(), RESOLUTION).unwrap_err();
    assert_eq!(error.path, "nodes[soft]");
    assert!(error.reason.contains("neighbourhood"), "{}", error.reason);
}

#[test]
fn the_target_and_the_resolution_are_checked_before_anything_is_cut() {
    let material = at_the_output(true);
    let bake = partition(&material, &Target::Bake, RESOLUTION).unwrap_err();
    assert_eq!(bake.path, "target");
    let odd = partition(&material, &everything(&material), 300).unwrap_err();
    assert_eq!(odd.path, "resolution");
    let unknown = partition(
        &material,
        &Target::Shader {
            live: vec!["wearr".to_owned()],
        },
        RESOLUTION,
    )
    .unwrap_err();
    assert_eq!(unknown.path, "params[wearr]");
    // A lattice finer than the texels the textures would be bound at is the
    // bake's own refusal, because a bound texture is a bake.
    let fine = MaterialGraph::builder("test:fine")
        .node("grain", Noise::value().period(512))
        .output(PbrOutput::new().roughness("grain"))
        .build()
        .unwrap();
    let coarse = partition(&fine, &nothing(), 256).unwrap_err();
    assert_eq!(coarse.path, "resolution");
}

#[test]
fn a_parameter_the_author_declared_live_stays_live_whatever_the_target_says() {
    let material = at_the_output(true);
    assert_eq!(material.param("wear").unwrap().exposure, Exposure::Live);
    // An empty live set, and the parameter is still a uniform: the author said
    // it moves, and a target that forgot to list it is not a reason to bake it
    // in.
    let split = partition(&material, &nothing(), RESOLUTION).unwrap();
    assert_eq!(split.live(), ["wear"]);
    assert_eq!(split.report().params, 1);
}

#[test]
fn scalar_bindings_pack_into_channels_only_when_the_images_would_not_fit() {
    // Ten separate static scalars, each behind its own live multiply, so each
    // is a frontier of its own. Unpacked that is more images than a bind group
    // holds. Every noise is on a lattice of four, so the periods all divide and
    // the sum still tiles.
    let many = |count: u32| {
        let mut builder = MaterialGraph::builder("test:many")
            .param(Param::float("wear", 0.5).range(0.0, 1.0).live());
        let mut total: Option<String> = None;
        for index in 0..count {
            let noise = format!("n{index}");
            let worn = format!("w{index}");
            builder = builder
                .node(&noise, Noise::value().period(4).seed(index))
                .node(
                    &worn,
                    Math::new(MathOp::Mul, Input::node(&noise), Input::param("wear")),
                );
            total = Some(match total {
                None => worn,
                Some(previous) => {
                    let id = format!("s{index}");
                    builder = builder.node(&id, Math::new(MathOp::Add, previous, worn));
                    id
                }
            });
        }
        builder
            .output(PbrOutput::new().roughness(Input::node(total.unwrap())))
            .build()
            .unwrap()
    };

    // Four of them fit as they are, one image each.
    let few = many(4);
    let loose = partition(&few, &everything(&few), RESOLUTION).unwrap();
    assert_eq!(loose.report().bindings, 4);
    assert_eq!(loose.report().textures, 4, "{}", loose.report());
    assert!(loose.textures().iter().all(|texture| texture.lane == 0));
    assert_eq!(loose.report().planes, 4, "no intermediates here");

    // Ten do not, so the scalars go four to an image.
    let material = many(10);
    let split = partition(&material, &everything(&material), RESOLUTION).unwrap();
    let report = split.report();
    assert_eq!(report.bindings, 10, "ten frontier cuts: {report}");
    assert_eq!(
        report.textures, 3,
        "ten scalars, four to an image: {report}"
    );
    assert!(report.textures <= MAX_BOUND_TEXTURES);
    // Every binding names a lane of an image, and no lane is used twice.
    let mut seen: BTreeSet<(u32, u8)> = BTreeSet::new();
    for texture in split.textures() {
        let id = texture
            .id
            .expect("every one of these is read by the fragment");
        assert!(texture.lane < 4, "{texture:?}");
        assert!(seen.insert((id, texture.lane)), "{texture:?}");
        // A packed image holds several unrelated fields, whose ranges the graph
        // never declared, so it is the format that clamps none of them.
        assert_eq!(texture.format, PlaneFormat::Rgba16Float);
    }
}

#[test]
fn every_lane_of_a_packed_image_agrees_with_the_others_on_its_format() {
    // Eight interior cuts, each a static noise under a live multiply, and two
    // static scalar outputs beside them: ten scalar bindings, which is past what
    // a bind group holds, so the partition packs. The two outputs are the point.
    // Left to itself a `height` binding asks for sixteen-bit codes and a
    // `roughness` one for eight unorm bits, and an image has one format: the
    // encoder would write the channels of whichever plane was seen first and the
    // shader would read each lane at its own, which is a wrong picture rather
    // than an error — a height-formatted image has one channel, so the roughness
    // lane beside it would read zero and the wall would render as a mirror.
    let mut builder = MaterialGraph::builder("test:mixed-pack")
        .param(Param::float("wear", 0.5).range(0.0, 1.0).live());
    let mut total: Option<String> = None;
    for index in 0..8_u32 {
        let noise = format!("n{index}");
        let worn = format!("w{index}");
        builder = builder
            .node(&noise, Noise::value().period(4).seed(index))
            .node(
                &worn,
                Math::new(MathOp::Mul, Input::node(&noise), Input::param("wear")),
            );
        total = Some(match total {
            None => worn,
            Some(previous) => {
                let id = format!("s{index}");
                builder = builder.node(&id, Math::new(MathOp::Add, previous, worn));
                id
            }
        });
    }
    let material = builder
        .node("rough", Noise::value().period(4).seed(100))
        .node("tall", Noise::value().period(4).seed(101))
        .output(
            PbrOutput::new()
                .occlusion(Input::node(total.unwrap()))
                .roughness(Input::node("rough"))
                .height(Input::node("tall")),
        )
        .build()
        .unwrap();
    let split = partition(&material, &everything(&material), RESOLUTION).unwrap();
    let report = split.report();
    assert!(report.textures <= MAX_BOUND_TEXTURES, "{report}");

    // One format per image, whatever the lanes in it came from.
    let mut formats: BTreeMap<u32, PlaneFormat> = BTreeMap::new();
    let mut lanes: BTreeMap<u32, Vec<&BoundTexture>> = BTreeMap::new();
    for texture in split.textures() {
        let Some(id) = texture.id else { continue };
        let seen = formats.entry(id).or_insert(texture.format);
        assert_eq!(
            *seen, texture.format,
            "image {id} holds two formats: {texture:?}"
        );
        lanes.entry(id).or_default().push(texture);
    }

    // The packing really happened, and the two PBR outputs are in it.
    assert!(
        lanes.values().any(|held| held.len() > 1),
        "ten scalar bindings should have packed: {report}"
    );
    for port in ["height", "roughness"] {
        let texture = split
            .textures()
            .iter()
            .find(|texture| texture.port.as_deref() == Some(port))
            .unwrap_or_else(|| panic!("{port} should be a bound texture"));
        let id = texture.id.unwrap();
        assert!(
            lanes[&id].len() > 1,
            "{port} should have packed with something: {texture:?}"
        );
        // Half floats: the one format every lane reads out of unchanged. A
        // packed image gives up the bake's own quantisation for its map, which
        // is the price of not fitting.
        assert_eq!(texture.format, PlaneFormat::Rgba16Float, "{texture:?}");
    }

    // And the same graph with four cuts instead of eight fits, so nothing packs
    // and each output keeps the format the bake writes that map in.
    let few = MaterialGraph::builder("test:mixed-loose")
        .param(Param::float("wear", 0.5).range(0.0, 1.0).live())
        .node("n0", Noise::value().period(4).seed(0))
        .node(
            "w0",
            Math::new(MathOp::Mul, Input::node("n0"), Input::param("wear")),
        )
        .node("rough", Noise::value().period(4).seed(100))
        .node("tall", Noise::value().period(4).seed(101))
        .output(
            PbrOutput::new()
                .occlusion(Input::node("w0"))
                .roughness(Input::node("rough"))
                .height(Input::node("tall")),
        )
        .build()
        .unwrap();
    let loose = partition(&few, &everything(&few), RESOLUTION).unwrap();
    let format_of = |port: &str| {
        loose
            .textures()
            .iter()
            .find(|texture| texture.port.as_deref() == Some(port))
            .map(|texture| texture.format)
    };
    assert_eq!(format_of("height"), Some(PlaneFormat::R16Unorm));
    assert_eq!(format_of("roughness"), Some(PlaneFormat::Rgba8Unorm));
    assert!(loose.textures().iter().all(|texture| texture.lane == 0));
}

#[test]
fn a_graph_that_binds_more_images_than_a_bind_group_holds_is_refused() {
    // Nine colours, each cut out from under a live mix. A colour is three
    // channels, so nothing packs, and nine images is one more than a bind group
    // has room for.
    let mut builder = MaterialGraph::builder("test:over-budget")
        .param(Param::float("wear", 0.5).range(0.0, 1.0).live());
    let mut blended: Option<String> = None;
    for index in 0..9_u32 {
        let noise = format!("n{index}");
        let colour = format!("c{index}");
        builder = builder
            .node(&noise, Noise::value().period(4).seed(index))
            .node(
                &colour,
                Colorize::new(Input::node(&noise)).gradient([
                    (0.0, [0.1, 0.2, 0.3]),
                    (
                        1.0,
                        [0.9, f32::from(u16::try_from(index).unwrap()) / 16.0, 0.4],
                    ),
                ]),
            );
        blended = Some(match blended {
            None => colour,
            Some(previous) => {
                let id = format!("m{index}");
                builder = builder.node(&id, Mix::new(previous, colour, Input::param("wear")));
                id
            }
        });
    }
    let material = builder
        .output(PbrOutput::new().base_color(Input::node(blended.unwrap())))
        .build()
        .unwrap();
    let error = partition(&material, &everything(&material), RESOLUTION).unwrap_err();
    assert_eq!(error.path, "output");
    assert!(
        error.reason.contains("binds 9 textures"),
        "{}",
        error.reason
    );
    assert!(
        error.reason.contains(&MAX_BOUND_TEXTURES.to_string()),
        "{}",
        error.reason
    );
}

#[test]
fn the_library_surfaces_partition_with_every_parameter_live_and_with_none() {
    let library = ashlar_material::stdlib::graphs();
    let mut surfaces = 0;
    for (key, material) in library.build_all().unwrap() {
        // A part — a layout, a substance, a weathering compound — binds only
        // the channels it exists for, so "every output is a texture or a
        // literal" is a claim about a finished surface.
        if !key.starts_with("library:") {
            continue;
        }
        surfaces += 1;
        let cold = partition(&material, &nothing(), 512).unwrap_or_else(|error| {
            panic!("{key} with nothing live: {error}");
        });
        // No library surface declares a parameter live or reads a runtime
        // input, so with nothing live every output is a texture or a literal
        // and the compiler says to bake it — which is what a definition that
        // names it under `Surface::Graph` does.
        assert!(cold.live().is_empty(), "{key}");
        assert_eq!(cold.report().verdict, "no live inputs: bake this", "{key}");
        assert!(cold.report().textures <= MAX_BOUND_TEXTURES, "{key}");
        // The two constant utilities — a dark recess and signage ink — are
        // literals throughout; every other surface is a field.
        if !matches!(key.as_str(), "library:dark-recess" | "library:signage-ink") {
            assert!(cold.report().textures >= 2, "{key}: {}", cold.report());
        }
        // A plane that only feeds another plane is baked and dropped; it costs
        // no binding, which is why every one of these fits.
        assert!(cold.report().planes >= cold.report().bindings, "{key}");
        assert_eq!(cold.report().bindings, cold.report().textures, "{key}");

        let hot = match partition(&material, &everything(&material), 512) {
            Ok(hot) => hot,
            // The grass's strand reliefs are eleven planes, and with every
            // parameter live none of them folds; it ships baked.
            Err(error) if key == "library:grass" => {
                assert!(error.reason.contains("binds 11 textures"), "{error}");
                continue;
            }
            Err(error) => panic!("{key} with everything live: {error}"),
        };
        let report = hot.report();
        assert!(report.textures <= MAX_BOUND_TEXTURES, "{key}: {report}");
        // Whatever was frozen was frozen because it reaches a plane, and what
        // stayed live plus what was frozen is what was asked for.
        let asked: BTreeSet<&str> = material
            .graph()
            .params
            .iter()
            .map(|param| param.name.as_str())
            .collect();
        let settled: BTreeSet<&str> = hot
            .live()
            .iter()
            .chain(hot.frozen())
            .map(String::as_str)
            .collect();
        assert_eq!(asked, settled, "{key}");
        // Every plane the bake needs is bound on the shader side too. A slope
        // walk names its guide by the plane's place in its own lowering, which
        // the two lowerings number differently, so it is compared without it.
        let baked = lower_bake(&material);
        let bound: Vec<Filter> = filters(&hot).into_iter().map(unguided).collect();
        for plan in baked.buffers() {
            assert!(
                bound.contains(&unguided(plan.filter)),
                "{key}: the bake's {:?} was not bound: {bound:?}",
                plan.filter
            );
        }
        // A parameter that feeds everything costs everything, so the report is
        // allowed to be large — but it must be finite and it must be reported.
        assert!(!report.verdict.is_empty(), "{key}");
    }
    assert!(
        surfaces >= 24,
        "every library surface was partitioned: {surfaces}"
    );
}

#[test]
fn the_report_prints_what_an_author_would_ask() {
    let material = at_the_output(true);
    let split = partition(&material, &everything(&material), RESOLUTION).unwrap();
    let printed = split.report().to_string();
    for wanted in ["live:", "per fragment", "bound value", "roughness"] {
        assert!(printed.contains(wanted), "{printed}");
    }
}

/// The planes the partition's own IR asks for, rasterised the way a bake
/// rasterises its buffers.
fn planes(split: &Partition) -> Vec<Plane> {
    rasterise_buffers(
        split.runtime(),
        split.resolution(),
        NonZeroUsize::new(THREADS),
        &mut BakeCache::new(),
    )
    .expect("the partition's plans are in dependency order")
}

/// The centre of texel `(x, y)`, which is where a plane holds its value and so
/// where a bilinear read of it is exact.
fn centre(x: u32, y: u32, resolution: u32) -> [f32; 2] {
    let size = f32::from(u16::try_from(resolution).unwrap());
    [
        (f32::from(u16::try_from(x).unwrap()) + 0.5) / size,
        (f32::from(u16::try_from(y).unwrap()) + 0.5) / size,
    ]
}

#[test]
fn a_bound_texture_carries_exactly_what_it_replaced() {
    // The claim the whole partition rests on: cutting at the frontier and
    // reading the cut back out of a texture is the same picture. Checked at
    // texel centres, where a bilinear read of a plane is the texel itself, so
    // what is left to differ is the cut and not the filtering.
    let wear = 0.6_f32;
    let graph = |live: bool| {
        let param = Param::float("wear", wear).range(0.0, 1.0);
        MaterialGraph::builder("test:carried")
            .param(if live { param.live() } else { param })
            .node("grain", Noise::value().period(8).octaves(3))
            .node("body", Levels::new("grain").in_range(0.1, 0.9))
            .node("soft", Blur::new("body").radius(0.02))
            .node("worn", Math::new(MathOp::Mul, "soft", Input::param("wear")))
            .output(PbrOutput::new().base_color("body").roughness("worn"))
            .build()
            .unwrap()
    };
    let material = graph(true);
    let split = partition(&material, &everything(&material), RESOLUTION).unwrap();
    assert!(!split.is_static());
    let buffers = planes(&split);
    let runtime = Interpreter::new(split.runtime());
    let inputs = Inputs {
        params: &[[wear, 0.0, 0.0]],
        buffers: &buffers,
        ..Inputs::default()
    };

    // The same material with the parameter folded, run the way a bake runs it.
    let folded = graph(false);
    let baked = lower_bake(&folded);
    let reference = rasterise_buffers(
        &baked,
        RESOLUTION,
        NonZeroUsize::new(THREADS),
        &mut BakeCache::new(),
    )
    .unwrap();
    let interpreter = Interpreter::new(&baked);
    let bake = Inputs {
        buffers: &reference,
        ..Inputs::default()
    };

    for (x, y) in [(0, 0), (1, 0), (37, 91), (128, 128), (255, 255), (0, 255)] {
        let uv = centre(x, y, RESOLUTION);
        for port in ["roughness", "base_color"] {
            let live = runtime
                .eval_float(uv, &inputs, split.runtime().root(port).unwrap())
                .unwrap();
            let cold = interpreter
                .eval_float(uv, &bake, baked.root(port).unwrap())
                .unwrap();
            assert!(
                (live - cold).abs() < 1e-5,
                "{port} at {uv:?}: {live} live against {cold} baked"
            );
        }
    }
}

#[test]
fn the_runtime_taps_answer_what_the_baked_normal_plane_does() {
    // The four taps are the bake's own central difference without a plane to
    // take it over, so at a texel centre they read the same two neighbours and
    // must answer the same normal. This is what keeps a live height and a baked
    // one describing one surface.
    let depth = 1.0_f32;
    let graph = |live: bool| {
        let param = Param::float("depth", depth).range(0.0, 1.0);
        MaterialGraph::builder("test:taps")
            .param(if live { param.live() } else { param })
            .node("grain", Noise::value().period(8))
            .node(
                "relief",
                Math::new(MathOp::Mul, "grain", Input::param("depth")),
            )
            .node("normal", NormalFromHeight::new("relief").strength(0.03))
            .node("lit", Decompose::new("normal", Channel::R))
            .output(PbrOutput::new().roughness("lit"))
            .build()
            .unwrap()
    };
    let material = graph(true);
    let split = partition(&material, &everything(&material), RESOLUTION).unwrap();
    assert_eq!(split.report().taps, 4);
    let buffers = planes(&split);
    let runtime = Interpreter::new(split.runtime());
    let inputs = Inputs {
        params: &[[depth, 0.0, 0.0]],
        buffers: &buffers,
        ..Inputs::default()
    };

    let folded = graph(false);
    let baked = lower_bake(&folded);
    assert_eq!(baked.buffers().len(), 1, "the baked normal is a plane");
    let reference = rasterise_buffers(
        &baked,
        RESOLUTION,
        NonZeroUsize::new(THREADS),
        &mut BakeCache::new(),
    )
    .unwrap();
    let interpreter = Interpreter::new(&baked);
    let bake = Inputs {
        buffers: &reference,
        ..Inputs::default()
    };
    for (x, y) in [(0, 0), (5, 3), (64, 200), (255, 0), (255, 255)] {
        let uv = centre(x, y, RESOLUTION);
        let live = runtime
            .eval_float(uv, &inputs, split.runtime().root("roughness").unwrap())
            .unwrap();
        let cold = interpreter
            .eval_float(uv, &bake, baked.root("roughness").unwrap())
            .unwrap();
        assert!(
            (live - cold).abs() < 1e-4,
            "the normal at {uv:?}: {live} tapped against {cold} from the plane"
        );
    }
}

#[test]
fn the_live_names_are_the_uniform_block_in_order() {
    // Declaration order, not alphabetical: a caller reading the names beside
    // the block has to be able to pair a name with its own slot.
    let material = MaterialGraph::builder("test:order")
        .param(Param::float("wear", 0.2).range(0.0, 1.0).live())
        .param(Param::color("tint", [0.5, 0.5, 0.5]).live())
        .param(Param::float("age", 0.4).range(0.0, 1.0).live())
        .node("grain", Noise::value().period(8))
        .node(
            "worn",
            Math::new(MathOp::Mul, "grain", Input::param("wear")),
        )
        .node("aged", Math::new(MathOp::Mul, "worn", Input::param("age")))
        .node(
            "lit",
            Blend::new(BlendMode::Multiply, Input::param("tint"), "aged"),
        )
        .output(PbrOutput::new().base_color("lit").roughness("aged"))
        .build()
        .unwrap();
    let split = partition(&material, &everything(&material), RESOLUTION).unwrap();
    assert_eq!(split.live(), ["wear", "tint", "age"]);
    let block: Vec<&str> = split
        .runtime()
        .params()
        .iter()
        .map(|binding| binding.name.as_str())
        .collect();
    assert_eq!(block, ["wear", "tint", "age"]);
}

// --------------------------------------------------------------------------
// The split a compute backend bakes through
// --------------------------------------------------------------------------

/// `over_planes` is the partition turned inside out: nothing is cut, and the
/// planes are named so a kernel can be written for one at a time.
#[test]
fn a_compute_split_keeps_the_whole_expression_and_names_every_plane() {
    use ashlar_material::{
        ir::lower,
        partition::{over_planes, plane_port},
    };

    let material = MaterialGraph::builder("test:compute")
        .node("grain", Noise::value().period(8))
        .node("soft", Blur::new("grain").radius(0.02))
        .node("cavity", OcclusionFromHeight::new("soft").radius(0.02))
        .output(
            PbrOutput::new()
                .roughness("soft")
                .occlusion("cavity")
                .height("soft")
                .normal_strength(0.01),
        )
        .build()
        .unwrap();
    let ir = lower(&material, Target::Bake).unwrap();
    let split = over_planes(&ir, RESOLUTION).unwrap();

    // Nothing was cut: the runtime half is the expression that went in, plan
    // for plan and instruction for instruction.
    assert_eq!(split.runtime().len(), ir.len());
    assert_eq!(split.runtime().buffers().len(), ir.buffers().len());
    assert_eq!(split.resolution(), RESOLUTION);
    // Every plane is a port a kernel can be written for, beside the graph's own
    // outputs, and every one of them is a bound texture the next kernel reads.
    for index in 0..ir.buffers().len() {
        assert_eq!(
            split.runtime().root(&plane_port(index)),
            Some(ir.buffers()[index].root),
            "plane {index} is not named at its own root"
        );
    }
    for port in ["roughness", "occlusion", "height"] {
        assert!(split.runtime().root(port).is_some(), "{port} is missing");
    }
    // Every bound texture is in the one format the emitter decodes nothing for:
    // a bake's intermediate plane is uploaded as the numbers it holds.
    for texture in split.textures() {
        assert_eq!(texture.format, PlaneFormat::Rgba16Float);
        assert!(texture.port.is_none(), "a compute split binds no PBR slot");
    }
}

/// One kernel per plane binds the planes before it and none of the ones it is
/// on the way to making — which is what lets the sequence run at all.
#[test]
fn a_plane_kernel_declares_only_the_planes_that_port_reads() {
    use ashlar_material::{
        ir::lower,
        partition::{over_planes, plane_port},
        wgsl::emit_compute_ports,
    };

    let material = MaterialGraph::builder("test:sequence")
        .node("grain", Noise::value().period(8))
        .node("soft", Blur::new("grain").radius(0.02))
        .node("cavity", OcclusionFromHeight::new("soft").radius(0.02))
        .output(PbrOutput::new().roughness("soft").occlusion("cavity"))
        .build()
        .unwrap();
    let ir = lower(&material, Target::Bake).unwrap();
    let split = over_planes(&ir, RESOLUTION).unwrap();
    assert_eq!(split.textures().len(), 2, "a blur and an occlusion");

    // The blur's own sub-expression is a noise over the coordinate: no plane.
    let first = emit_compute_ports(&split, RESOLUTION, &[plane_port(0)]).unwrap();
    assert!(first.bindings().is_empty(), "{}", first.module());
    // The occlusion reads the blur's plane, and only that one.
    let second = emit_compute_ports(&split, RESOLUTION, &[plane_port(1)]).unwrap();
    assert_eq!(second.bindings().len(), 1);
    assert_eq!(
        second.bindings()[0].buffers,
        vec![split.textures()[0].buffer]
    );
    // The outputs read both.
    let outputs = emit_compute_ports(
        &split,
        RESOLUTION,
        &["roughness".into(), "occlusion".into()],
    )
    .unwrap();
    assert_eq!(outputs.bindings().len(), 2);
    assert_eq!(outputs.ports().len(), 2);
    assert_eq!(outputs.output_len(), (RESOLUTION as usize).pow(2) * 2);
}

/// A plane port is whatever width its expression lowered to; a PBR port is
/// still held to the width of its slot.
#[test]
fn a_plane_port_carries_its_own_width_and_a_pbr_port_carries_its_slots() {
    use ashlar_material::{
        ir::{IrType, lower},
        partition::{over_planes, plane_port},
        wgsl::emit_compute_ports,
    };

    let material = MaterialGraph::builder("test:widths")
        .node("grain", Noise::value().period(8))
        .node(
            "tint",
            Colorize::new("grain").gradient([(0.0, [0.1, 0.2, 0.3]), (1.0, [0.9, 0.8, 0.7])]),
        )
        // A colour through a buffer: the plane holds three lanes.
        .node("soft", Blur::new("tint").radius(0.01))
        .output(PbrOutput::new().base_color("soft").roughness("grain"))
        .build()
        .unwrap();
    let ir = lower(&material, Target::Bake).unwrap();
    let split = over_planes(&ir, RESOLUTION).unwrap();
    let kernel = emit_compute_ports(
        &split,
        RESOLUTION,
        &[plane_port(0), "base_color".into(), "roughness".into()],
    )
    .unwrap();
    let widths: BTreeMap<&str, IrType> = kernel
        .ports()
        .iter()
        .map(|(port, width)| (port.as_str(), *width))
        .collect();
    assert_eq!(widths[plane_port(0).as_str()], IrType::Vec3);
    assert_eq!(widths["base_color"], IrType::Vec3);
    assert_eq!(widths["roughness"], IrType::Float);
}

/// Naming a value the expression does not hold is an error rather than a root
/// that reads nothing.
#[test]
fn a_root_past_the_end_of_the_arena_is_refused() {
    use ashlar_material::ir::lower;

    let small = lower(
        &MaterialGraph::builder("test:small")
            .node("grain", Noise::value().period(8))
            .output(PbrOutput::new().roughness("grain"))
            .build()
            .unwrap(),
        Target::Bake,
    )
    .unwrap();
    let large = lower(
        &MaterialGraph::builder("test:large")
            .node("grain", Noise::value().period(8))
            .node("other", Noise::value().period(4).seed(1))
            .node("mixed", Blend::new(BlendMode::Multiply, "grain", "other"))
            .node("soft", Blur::new("mixed").radius(0.02))
            .node("cavity", OcclusionFromHeight::new("soft").radius(0.02))
            .output(PbrOutput::new().roughness("cavity").occlusion("soft"))
            .build()
            .unwrap(),
        Target::Bake,
    )
    .unwrap();

    // A value of its own is fine: a finished lowering has no dead instructions,
    // so anything in the arena is something naming cannot revive.
    let own = small.root("roughness").unwrap();
    assert!(small.clone().rooted_at([("also".to_owned(), own)]).is_ok());

    // One out of somebody else's arena is not.
    let stranger = large.root("roughness").unwrap();
    assert!(
        stranger.index() >= small.len(),
        "the fixture is not a fixture"
    );
    let error = small
        .rooted_at([("nope".to_owned(), stranger)])
        .expect_err("a value the arena does not hold");
    assert!(error.reason.contains("does not hold"), "{error}");
}

#[test]
fn a_triplanar_is_three_evaluations_of_its_source_and_binds_nothing() {
    // What the design says a resampler costs, at the one node that resamples
    // three times: the source is emitted once per axis plane, at three
    // coordinates none of which is the texel's own, and every copy is runtime
    // because all three depend on the world position. So a triplanar over a
    // noise binds *no* texture at all — there is no static sub-expression left
    // to bind, the coordinate having been replaced before the noise was reached
    // — and the whole noise lands in the fragment three times over.
    let material = MaterialGraph::builder("test:triplanar")
        .node("grain", Noise::value().period(8))
        .node("n", Triplanar::new("grain").tile_metres(2.0))
        .output(PbrOutput::new().roughness("n"))
        .build()
        .unwrap();
    let split = partition(&material, &nothing(), RESOLUTION).unwrap();
    assert!(!split.is_static(), "a world-space surface is not a bake");
    assert_eq!(
        split.report().textures,
        0,
        "nothing static was left to bind"
    );
    assert_eq!(split.report().samples, 0);

    // One noise, baked, against three in the fragment. Not exactly three times
    // the size: the three projections share the world position, the scale and
    // the weights, and the hashes fold and share like anything else — which is
    // why the report counts what it came to rather than three times a size.
    let one = lower_bake(&plain_noise()).len();
    let three = split.report().ops;
    assert!(
        three > 2 * one && three < 4 * one,
        "a triplanar of a {one}-op noise came to {three} ops"
    );
    assert_eq!(
        split.report().ops,
        split.report().ops_per_output["roughness"]
    );

    // And the three are three *different* coordinates: the lattice hash is
    // emitted once per plane rather than once for all of them.
    let hashes = count(&split, |op| matches!(op, Op::Hash2(_)));
    assert_eq!(hashes % 3, 0, "{hashes} hashes is not three planes' worth");
    assert!(hashes >= 12, "a value noise is four corners a plane");
}

/// A noise on its own, for the size a triplanar's copies are measured against.
fn plain_noise() -> Material {
    MaterialGraph::builder("test:grain")
        .node("grain", Noise::value().period(8))
        .output(PbrOutput::new().roughness("grain"))
        .build()
        .unwrap()
}

#[test]
fn a_triplanar_over_a_buffered_filter_binds_the_plane_and_reads_it_three_times() {
    // The case that says the cut still works underneath a world-space
    // coordinate. A blur is a plane in every backend, and this one's contents
    // are a function of UV alone, so it is baked exactly as it always was — and
    // the three projections become three samples of that one plane at three
    // world-space coordinates. A resampler never costs a second plane, and a
    // triplanar is a resampler.
    let material = MaterialGraph::builder("test:softened")
        .node("grain", Noise::value().period(8))
        .node("soft", Blur::new("grain").radius(0.02))
        .node("n", Triplanar::new("soft").tile_metres(2.0))
        .output(PbrOutput::new().roughness("n"))
        .build()
        .unwrap();
    let split = partition(&material, &nothing(), RESOLUTION).unwrap();
    assert_eq!(split.report().textures, 1, "one plane, bound once");
    assert_eq!(split.report().samples, 3, "read once per axis plane");
    assert_eq!(
        split.report().planes,
        1,
        "the blur is one plan: its input rasterised and its filter run over it"
    );
    // The blur was not frozen and nothing was refused: what reaches the filter
    // is the coordinate and a noise, and neither of those moves per frame.
    assert!(split.frozen().is_empty());
}

#[test]
fn a_world_mask_is_runtime_and_a_filter_above_one_is_refused_by_path() {
    // A world mask is runtime by being what it is, so a blend it feeds is a
    // fragment's arithmetic over bound textures — the usual shape, and the one
    // a wetness gate wants.
    let material = MaterialGraph::builder("test:up")
        .node("grain", Noise::value().period(8))
        .node("up", WorldMask::up())
        .node("n", Math::new(MathOp::Mul, "grain", "up"))
        .output(PbrOutput::new().roughness("n"))
        .build()
        .unwrap();
    let split = partition(&material, &nothing(), RESOLUTION).unwrap();
    assert_eq!(split.report().textures, 1, "the noise is still a texture");
    // Three instructions: the lane of the world normal the axis names, the
    // ramp across the threshold, and the multiply into the bound texture. That
    // is what a mask costs written out, and it is why the node exists — an
    // author who wrote the three by hand would pay exactly the same.
    assert_eq!(arithmetic(&split), 3);

    // And the rule the partition already had holds for it: a filter that can
    // only ever be a plane cannot read one, because there is nothing to fold a
    // world normal down to. A parameter would have been frozen; this is
    // refused, by the path of the filter that asked.
    let material = MaterialGraph::builder("test:blurred-mask")
        .node("up", WorldMask::up())
        .node("n", Blur::new("up").radius(0.02))
        .output(PbrOutput::new().roughness("n"))
        .build()
        .unwrap();
    let error = partition(&material, &nothing(), RESOLUTION).unwrap_err();
    assert_eq!(error.path, "nodes[n]");
    assert!(error.reason.contains("runtime input"), "{error}");
}

#[test]
fn a_live_parameter_a_strand_field_reads_is_frozen_for_the_blades_and_says_so() {
    // A partition lowers the PBR half and nothing else, so nothing in it can
    // see a strand layer. The scatter folds every parameter a strand field
    // reads — a strand set is geometry — so a slider wired to both halves moves
    // the texture and not the blades, and this is the only place that says so.
    let material = MaterialGraph::builder("test:strand-live")
        .param(
            ashlar_material::Param::float("lushness", 0.7)
                .range(0.0, 1.0)
                .live(),
        )
        .param(
            ashlar_material::Param::float("sheen", 0.3)
                .range(0.0, 1.0)
                .live(),
        )
        .node("patches", ashlar_material::nodes::Noise::value().period(8))
        .node(
            "tinted",
            Math::new(MathOp::Mul, "patches", Input::param("lushness")),
        )
        .node(
            "glossed",
            Math::new(MathOp::Mul, "patches", Input::param("sheen")),
        )
        .output(PbrOutput::new().roughness("glossed"))
        .strands(
            "blades",
            ashlar_material::StrandLayer::new()
                .count(16)
                .density("patches")
                .length("tinted")
                .metres(0.1, 0.004),
        )
        .build()
        .unwrap();
    let split = partition(&material, &everything(&material), RESOLUTION).unwrap();
    // Both are still live: the fragment reads them from the uniform block, and
    // the strand layer is not what decides that.
    assert_eq!(split.live(), ["lushness", "sheen"]);
    // Only the one a strand field reaches is reported frozen, and it is frozen
    // *as well as* live rather than instead of it.
    assert_eq!(split.frozen(), ["lushness"]);
    assert_eq!(split.report().frozen, ["lushness"]);
    assert!(split.report().to_string().contains("strand layer"));

    // And a graph with no strand layer at all is unchanged, which is the check
    // that this costs the other nine tenths of the library nothing.
    let plain = MaterialGraph::builder("test:strand-live-none")
        .param(
            ashlar_material::Param::float("sheen", 0.3)
                .range(0.0, 1.0)
                .live(),
        )
        .node("patches", ashlar_material::nodes::Noise::value().period(8))
        .node(
            "glossed",
            Math::new(MathOp::Mul, "patches", Input::param("sheen")),
        )
        .output(PbrOutput::new().roughness("glossed"))
        .build()
        .unwrap();
    assert!(
        partition(&plain, &everything(&plain), RESOLUTION)
            .unwrap()
            .frozen()
            .is_empty()
    );
}
